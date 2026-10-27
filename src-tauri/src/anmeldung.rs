//! Anmeldedienst des Clients (Konzept §7; CS4 #759, AD2 #798).
//!
//! OIDC Authorization Code mit PKCE (S256) und Loopback-Redirect nach
//! RFC 8252: Der Client öffnet den Systembrowser auf die Anmeldeseite des
//! Servers (Keycloak, öffentlicher Client `leadlab-client`) und wartet auf
//! `http://127.0.0.1:<zufallsport>/rueckruf`. Den Code tauscht er gegen
//! Access-, Refresh- und ID-Token.
//!
//! - Angefragt wird zusätzlich `offline_access`: Das Refresh-Token ist dann
//!   ein Offline-Token, der Arbeitsplatz bleibt wochenlang angemeldet, bis
//!   jemand abmeldet oder ein Admin die Offline-Sitzung widerruft. Ohne die
//!   Realm-Rolle gibt Keycloak ein normales Refresh-Token (kein Fehler).
//! - Alle Tokens liegen im verschlüsselten **Tresor** im Konfigurationsordner
//!   (tresor.rs), nicht mehr im Schlüsselbund. Ein Refresh-Token aus dem
//!   Schlüsselbund der Fassung CS4 wird einmal übernommen und dort gelöscht.
//! - Das Access-Token öffnet `/daten/` (Zielgruppe `leadlab-daten`) und
//!   trägt `aud=leadlab-bff`, damit der BFF daraus die Sitzung der
//!   Oberfläche machen kann (AD3/AD4).
//! - Abmelden widerruft das Refresh-Token beim Anmeldedienst (RFC 7009) und
//!   löscht den Tresor.
//! - Anmeldemaske (#804): Benutzername/Passwort direkt am Token-Endpunkt
//!   (Resource Owner Password Credentials, `grant_type=password`), Ergebnis
//!   in denselben Tresor. Das Passwort bleibt im Speicher, wird nie
//!   geschrieben oder protokolliert und nach dem Aufruf überschrieben
//!   (`zeroize`; die Kopie im HTTP-Rumpf von reqwest erreicht das nicht).
//!   Zwei-Faktor und Identity-Provider bleiben dem Browser-Weg vorbehalten.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering as AtomOrdnung};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

use crate::fehler::Fehler;
use crate::konfig::{pruefe_schema, Anmeldung as AnmeldeKonfig};
use crate::tresor::{Eintrag, Tresor};

/// Dienstname im Schlüsselbund (nur noch für die Übernahme aus CS4).
pub const DIENST: &str = "de.leadlab.client";
/// Pfad der Rückrufadresse auf dem Loopback.
pub const RUECKRUF: &str = "/rueckruf";
/// So lange wartet der Client auf den Rückruf aus dem Browser.
pub const FRIST: Duration = Duration::from_secs(300);
/// So lange ohne Verbindung am Loopback, bis die Ansicht die Ausweichwege
/// nennt (Link kopieren, Anmeldung im Client-Fenster; #805).
pub const STILL_SEK: u64 = 20;
/// Access-Token gilt als abgelaufen, wenn weniger als das übrig ist.
pub const VORLAUF_SEK: u64 = 60;
/// Scope für das Offline-Token (wochenlange Anmeldung).
pub const OFFLINE: &str = "offline_access";

// ── PKCE ───────────────────────────────────────────────────────────────────

fn zufall(n: usize) -> String {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).expect("Zufallsquelle des Betriebssystems");
    URL_SAFE_NO_PAD.encode(b)
}

/// S256 des Verifiers (RFC 7636).
pub fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

#[derive(Debug, Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

impl Pkce {
    pub fn neu() -> Self {
        // 48 Byte → 64 Zeichen, im erlaubten Bereich 43–128 (RFC 7636 §4.1).
        let verifier = zufall(48);
        Pkce { challenge: challenge(&verifier), verifier, state: zufall(16) }
    }
}

// ── Endpunkte (OIDC Discovery) ─────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Endpunkte {
    pub issuer: Url,
    pub authorization_endpoint: Url,
    pub token_endpoint: Url,
    /// RFC 7009; Keycloak nennt ihn in der Discovery.
    #[serde(default)]
    pub revocation_endpoint: Option<Url>,
}

/// Liest `<issuer>/.well-known/openid-configuration` und prüft, dass der
/// Aussteller derselbe ist und alle Adressen https (bzw. LAN-http) sind.
pub async fn entdecken(client: &reqwest::Client, issuer: &Url) -> Result<Endpunkte, Fehler> {
    let basis = issuer.as_str().trim_end_matches('/');
    let url = format!("{basis}/.well-known/openid-configuration");
    let antwort = client.get(&url).send().await.map_err(|e| Fehler::Netz(format!("Anmeldedienst nicht erreichbar ({e}).")))?;
    if !antwort.status().is_success() {
        return Err(Fehler::Server(format!("Anmeldedienst antwortet mit {}.", antwort.status().as_u16())));
    }
    let ep: Endpunkte = antwort.json().await.map_err(|e| Fehler::Server(format!("Anmeldedienst unlesbar: {e}")))?;
    if ep.issuer.as_str().trim_end_matches('/') != basis {
        return Err(Fehler::Server(format!("Anmeldedienst meldet Aussteller {}, erwartet {basis}.", ep.issuer)));
    }
    pruefe_schema(&ep.authorization_endpoint)?;
    pruefe_schema(&ep.token_endpoint)?;
    if let Some(r) = &ep.revocation_endpoint {
        pruefe_schema(r)?;
    }
    Ok(ep)
}

/// Angefragte Scopes: die der Konfigurationsadresse als Basis, `openid`
/// vorn, `offline_access` dazu (Browser und Formular gleich).
pub fn scopes(k: &AnmeldeKonfig) -> String {
    let mut scopes = k.scopes.clone();
    if !scopes.iter().any(|s| s == "openid") {
        scopes.insert(0, "openid".into());
    }
    if !scopes.iter().any(|s| s == OFFLINE) {
        scopes.push(OFFLINE.into());
    }
    scopes.join(" ")
}

/// Adresse der Anmeldeseite für den Systembrowser.
pub fn anmelde_url(ep: &Endpunkte, k: &AnmeldeKonfig, redirect: &str, p: &Pkce) -> Url {
    let mut u = ep.authorization_endpoint.clone();
    u.query_pairs_mut()
        .append_pair("client_id", &k.client_id)
        .append_pair("response_type", "code")
        .append_pair("scope", &scopes(k))
        .append_pair("redirect_uri", redirect)
        .append_pair("state", &p.state)
        .append_pair("code_challenge", &p.challenge)
        .append_pair("code_challenge_method", "S256");
    u
}

// ── Laufende Browser-Anmeldung (#805) ──────────────────────────────────────

/// Eine laufende Anmeldung im Browser: Die Ansicht zeigt die Adresse zum
/// Kopieren, erfährt, ob sich am Loopback schon ein Browser gemeldet hat, und
/// kann abbrechen. `open` meldet nur Startfehler des Hilfsprogramms, nicht, ob
/// ein Browser die Adresse wirklich annimmt — darum wartet die Ansicht nie
/// stumm.
#[derive(Default)]
pub struct Warten {
    adresse: Mutex<Option<Url>>,
    /// Start des Wartens (Unix-Sekunden).
    seit: u64,
    /// Mindestens eine Verbindung am Loopback (ein Browser hat sich gemeldet).
    verbunden: AtomicBool,
    abgebrochen: AtomicBool,
    abbruch: tokio::sync::Notify,
    /// Startfehler des Browsers (open): Warten geht weiter, die Ansicht nennt die Ausweichwege sofort.
    browser_fehler: Mutex<Option<String>>,
}

impl Warten {
    pub fn neu() -> Self {
        Warten { seit: jetzt(), ..Default::default() }
    }
    pub fn adresse(&self) -> Option<Url> {
        self.adresse.lock().unwrap().clone()
    }
    pub fn seit(&self) -> u64 {
        self.seit
    }
    pub fn verbunden(&self) -> bool {
        self.verbunden.load(AtomOrdnung::SeqCst)
    }
    pub fn browser_fehler(&self) -> Option<String> {
        self.browser_fehler.lock().unwrap().clone()
    }
    /// Beendet das Warten: Der Loopback schließt, `anmelden` endet mit
    /// `Fehler::Abgebrochen`. Wirkt auch, wenn das Warten noch nicht begonnen hat.
    pub fn abbrechen(&self) {
        self.abgebrochen.store(true, AtomOrdnung::SeqCst);
        self.abbruch.notify_one();
    }
    pub fn abgebrochen(&self) -> bool {
        self.abgebrochen.load(AtomOrdnung::SeqCst)
    }
}

/// Meldung, wenn die Frist ohne Rückruf abläuft — mit der Ursache.
pub fn frist_text(frist: Duration) -> String {
    let s = frist.as_secs();
    let dauer = if s >= 120 {
        format!("{} Minuten", s / 60)
    } else if s >= 60 {
        "einer Minute".to_string()
    } else if s <= 1 {
        "einer Sekunde".to_string()
    } else {
        format!("{s} Sekunden")
    };
    format!("Innerhalb von {dauer} kam keine Antwort vom Browser. Die Anmeldung ist abgebrochen; bitte erneut anmelden.")
}

// ── Loopback-Rückruf ───────────────────────────────────────────────────────

/// Lauscht auf 127.0.0.1 mit einem freien Port; liefert Listener und
/// Rückrufadresse (Keycloak nimmt für Loopback jeden Port, RFC 8252 §7.3).
pub async fn lauschen() -> Result<(TcpListener, String), Fehler> {
    let l = TcpListener::bind("127.0.0.1:0").await?;
    let port = l.local_addr()?.port();
    Ok((l, format!("http://127.0.0.1:{port}{RUECKRUF}")))
}

const SEITE_OK: &str = "<!doctype html><meta charset=utf-8><title>LeadLab</title>\
<body style=\"font-family:sans-serif;margin:3em\"><h1>Angemeldet</h1>\
<p>Sie können dieses Fenster schließen und zum LeadLab-Client zurückkehren.</p>";

fn seite_fehler(text: &str) -> String {
    let sicher = text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        "<!doctype html><meta charset=utf-8><title>LeadLab</title>\
<body style=\"font-family:sans-serif;margin:3em\"><h1>Anmeldung fehlgeschlagen</h1><p>{sicher}</p>"
    )
}

/// Ergebnis einer Rückruf-Anfrage.
#[derive(Debug, PartialEq)]
pub enum Rueckruf {
    Code(String),
    Fehler(String),
    /// Fremde Anfrage (anderer Pfad, favicon …) — weiter warten.
    Fremd,
}

/// Wertet die Anfragezeile `GET /rueckruf?code=…&state=… HTTP/1.1` aus.
pub fn rueckruf_lesen(anfragezeile: &str, state: &str) -> Rueckruf {
    let mut teile = anfragezeile.split_whitespace();
    let (Some("GET"), Some(ziel)) = (teile.next(), teile.next()) else {
        return Rueckruf::Fremd;
    };
    let Ok(u) = Url::parse(&format!("http://127.0.0.1{ziel}")) else {
        return Rueckruf::Fremd;
    };
    if u.path() != RUECKRUF {
        return Rueckruf::Fremd;
    }
    let wert = |n: &str| u.query_pairs().find(|(k, _)| k == n).map(|(_, v)| v.into_owned());
    if wert("state").as_deref() != Some(state) {
        return Rueckruf::Fehler("Die Antwort des Anmeldedienstes gehört nicht zu dieser Anmeldung.".into());
    }
    if let Some(f) = wert("error") {
        let text = wert("error_description").unwrap_or(f);
        return Rueckruf::Fehler(format!("Anmeldedienst: {text}"));
    }
    match wert("code") {
        Some(c) if !c.is_empty() => Rueckruf::Code(c),
        _ => Rueckruf::Fehler("Der Anmeldedienst hat keinen Code geliefert.".into()),
    }
}

/// Wartet auf den einen Rückruf aus dem Browser und antwortet mit einer
/// kleinen Seite. Andere Anfragen (favicon) bekommen 404. Endet bei
/// `w.abbrechen()` mit `Fehler::Abgebrochen`; der Listener ist danach zu.
pub async fn rueckruf_abwarten(l: TcpListener, state: &str, frist: Duration, w: &Warten) -> Result<String, Fehler> {
    let warten = async {
        loop {
            let (mut s, _) = l.accept().await?;
            w.verbunden.store(true, AtomOrdnung::SeqCst);
            let mut puffer = vec![0u8; 8192];
            let mut n = 0;
            // Nur die erste Zeile zählt; mehr als 8 KB Kopf braucht kein Rückruf.
            while n < puffer.len() {
                let k = s.read(&mut puffer[n..]).await?;
                if k == 0 {
                    break;
                }
                n += k;
                if puffer[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&puffer[..n]);
            let zeile = text.lines().next().unwrap_or("");
            let (status, seite, ergebnis) = match rueckruf_lesen(zeile, state) {
                Rueckruf::Code(c) => ("200 OK", SEITE_OK.to_string(), Some(Ok(c))),
                Rueckruf::Fehler(f) => ("400 Bad Request", seite_fehler(&f), Some(Err(Fehler::Server(f)))),
                Rueckruf::Fremd => ("404 Not Found", String::new(), None),
            };
            let antwort = format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
Cache-Control: no-store\r\nConnection: close\r\n\r\n{seite}",
                seite.len()
            );
            let _ = s.write_all(antwort.as_bytes()).await;
            let _ = s.shutdown().await;
            if let Some(e) = ergebnis {
                return e;
            }
        }
    };
    if w.abgebrochen() {
        return Err(Fehler::Abgebrochen);
    }
    tokio::select! {
        r = tokio::time::timeout(frist, warten) => r.map_err(|_| Fehler::Eingabe(frist_text(frist)))?,
        _ = w.abbruch.notified() => Err(Fehler::Abgebrochen),
    }
}

// ── Token-Endpunkt ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// OIDC-ID-Token; nur für den Kontonamen auf der Einstellungsseite (#792).
    #[serde(default)]
    pub id_token: Option<String>,
    /// Keycloak: Restlaufzeit des Refresh-Tokens (bei Offline die Leerlaufgrenze).
    #[serde(default)]
    pub refresh_expires_in: Option<u64>,
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Deserialize)]
pub struct FehlerAntwort {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

/// Abgelehnt (invalid_grant: Sitzung beendet, Konto gesperrt) — neu anmelden.
pub const ABGELEHNT: &str = "Die Anmeldung ist abgelaufen. Bitte neu anmelden.";

/// Fehler des Token-Endpunkts beim Code-Tausch und bei der Erneuerung.
fn standard_fehler(status: u16, f: Option<FehlerAntwort>) -> Fehler {
    match f {
        Some(f) if f.error == "invalid_grant" => Fehler::Server(ABGELEHNT.into()),
        Some(f) => Fehler::Server(format!("Anmeldedienst: {}", f.error_description.unwrap_or(f.error))),
        None => Fehler::Server(format!("Anmeldedienst antwortet mit {status}.")),
    }
}

// Texte der Anmeldemaske (#804). Die Oberfläche vergleicht FALSCH, um
// Fehlversuche zu zählen (logik.ts, Vitest prüft den Gleichlauf).
pub const FORMULAR_FALSCH: &str = "Benutzername oder Passwort falsch.";
pub const FORMULAR_GESPERRT: &str =
    "Das Konto ist nach zu vielen Fehlversuchen vorübergehend gesperrt. Bitte einige Minuten warten und dann erneut anmelden.";
pub const FORMULAR_DEAKTIVIERT: &str = "Das Konto ist gesperrt. Bitte an die Verwaltung wenden.";
pub const FORMULAR_UNVOLLSTAENDIG: &str =
    "Das Konto ist noch nicht fertig eingerichtet (z. B. E-Mail bestätigen oder Passwort ändern). Bitte einmal „Im Browser anmelden“.";
pub const FORMULAR_NICHT_ERLAUBT: &str =
    "Der Server erlaubt die Anmeldung per Formular nicht. Bitte „Im Browser anmelden“.";
pub const FORMULAR_NUR_BROWSER: &str =
    "Dieses Konto meldet sich mit zweitem Faktor an. Bitte „Im Browser anmelden“.";
pub const FORMULAR_LEER: &str = "Bitte Benutzername und Passwort eingeben.";
pub const FORMULAR_AUS: &str = "Der Server hat die Anmeldung per Formular abgeschaltet. Bitte „Im Browser anmelden“.";

/// Fehler des Token-Endpunkts beim Passwort-Grant, gegen Keycloak 26.8
/// geprüft (Konzept Client §7): falsches Passwort und unbekannter Nutzer
/// `invalid_grant` „Invalid user credentials“; die Sperre nach Fehlversuchen
/// meldet Keycloak 26 genauso (ältere Fassungen „Account temporarily
/// disabled“); offene Pflichtaktion „Account is not fully set up“; Direct
/// Grants aus `unauthorized_client`; weist ein Schritt des Ablaufs `leadlab
/// direktzugang` ab (etwa ein fehlender OTP-Code), antwortet Keycloak 401 mit
/// einer HTML-Seite.
pub fn formular_fehler(status: u16, f: Option<FehlerAntwort>) -> Fehler {
    let text = match f {
        Some(f) if f.error == "invalid_grant" => {
            let d = f.error_description.unwrap_or_default().to_lowercase();
            if d.contains("temporarily") {
                FORMULAR_GESPERRT
            } else if d.contains("not fully set up") {
                FORMULAR_UNVOLLSTAENDIG
            } else if d.contains("disabled") {
                FORMULAR_DEAKTIVIERT
            } else {
                FORMULAR_FALSCH
            }
        }
        Some(f) if f.error == "unauthorized_client" => FORMULAR_NICHT_ERLAUBT,
        Some(f) => return Fehler::Server(format!("Anmeldedienst: {}", f.error_description.unwrap_or(f.error))),
        None if status == 401 => FORMULAR_NUR_BROWSER,
        None => return Fehler::Server(format!("Anmeldedienst antwortet mit {status}.")),
    };
    Fehler::Server(text.into())
}

async fn token_anfrage(
    client: &reqwest::Client,
    ep: &Endpunkte,
    form: &[(&str, &str)],
    fehler: fn(u16, Option<FehlerAntwort>) -> Fehler,
) -> Result<Tokens, Fehler> {
    let antwort = client
        .post(ep.token_endpoint.clone())
        .form(form)
        .send()
        .await
        .map_err(|e| Fehler::Netz(format!("Anmeldedienst nicht erreichbar ({e}).")))?;
    let status = antwort.status();
    if status.as_u16() == 400 || status.as_u16() == 401 {
        let f: Option<FehlerAntwort> = antwort.json().await.ok();
        return Err(fehler(status.as_u16(), f));
    }
    if !status.is_success() {
        return Err(Fehler::Server(format!("Anmeldedienst antwortet mit {}.", status.as_u16())));
    }
    antwort.json().await.map_err(|e| Fehler::Server(format!("Token-Antwort unlesbar: {e}")))
}

pub async fn code_tauschen(
    client: &reqwest::Client,
    ep: &Endpunkte,
    client_id: &str,
    code: &str,
    redirect: &str,
    verifier: &str,
) -> Result<Tokens, Fehler> {
    token_anfrage(
        client,
        ep,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("code", code),
            ("redirect_uri", redirect),
            ("code_verifier", verifier),
        ],
        standard_fehler,
    )
    .await
}

pub async fn erneuern(client: &reqwest::Client, ep: &Endpunkte, client_id: &str, refresh: &str) -> Result<Tokens, Fehler> {
    token_anfrage(
        client,
        ep,
        &[("grant_type", "refresh_token"), ("client_id", client_id), ("refresh_token", refresh)],
        standard_fehler,
    )
    .await
}

/// Passwort-Grant (#804). Das Passwort geht nur in den Formularrumpf an den
/// Token-Endpunkt; Fehlertexte nennen es nie.
pub async fn passwort_tauschen(
    client: &reqwest::Client,
    ep: &Endpunkte,
    k: &AnmeldeKonfig,
    benutzer: &str,
    passwort: &str,
) -> Result<Tokens, Fehler> {
    let scope = scopes(k);
    token_anfrage(
        client,
        ep,
        &[
            ("grant_type", "password"),
            ("client_id", &k.client_id),
            ("username", benutzer),
            ("password", passwort),
            ("scope", &scope),
        ],
        formular_fehler,
    )
    .await
}

/// Widerruf eines Refresh-Tokens (RFC 7009). Keycloak beendet damit auch
/// die Offline-Sitzung.
pub async fn widerrufen(client: &reqwest::Client, endpunkt: &Url, client_id: &str, refresh: &str) -> Result<(), Fehler> {
    let antwort = client
        .post(endpunkt.clone())
        .form(&[("token", refresh), ("token_type_hint", "refresh_token"), ("client_id", client_id)])
        .send()
        .await
        .map_err(|e| Fehler::Netz(format!("Anmeldedienst nicht erreichbar ({e}).")))?;
    if !antwort.status().is_success() {
        return Err(Fehler::Server(format!("Widerruf: Anmeldedienst antwortet mit {}.", antwort.status().as_u16())));
    }
    Ok(())
}

/// `exp` aus einem JWT, ungeprüft — nur, um die Erneuerung zu planen; die
/// Prüfung macht der Server.
pub fn ablauf(jwt: &str) -> Option<u64> {
    let teil = jwt.split('.').nth(1)?;
    let roh = URL_SAFE_NO_PAD.decode(teil.trim_end_matches('=')).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&roh).ok()?;
    v.get("exp")?.as_f64().map(|e| e as u64)
}

/// Ist das ein Offline-Token? Keycloak schreibt `typ: Offline` hinein; ohne
/// lesbaren Inhalt entscheidet der Scope der Antwort.
pub fn ist_offline(refresh: &str, scope: Option<&str>) -> bool {
    match jwt_inhalt(refresh).and_then(|v| v.get("typ").and_then(|t| t.as_str()).map(str::to_string)) {
        Some(typ) => typ.eq_ignore_ascii_case("offline"),
        None => scope.map(|s| s.split_whitespace().any(|x| x == OFFLINE)).unwrap_or(false),
    }
}

fn jwt_inhalt(jwt: &str) -> Option<serde_json::Value> {
    let teil = jwt.split('.').nth(1)?;
    let roh = URL_SAFE_NO_PAD.decode(teil.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&roh).ok()
}

/// Kontoname aus einem JWT (ungeprüft, nur zur Anzeige):
/// `preferred_username`, sonst `email`, sonst `name`, sonst `sub`.
pub fn konto_aus_jwt(jwt: &str) -> Option<String> {
    let v = jwt_inhalt(jwt)?;
    ["preferred_username", "email", "name", "sub"]
        .iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(str::to_string))
}

fn jetzt() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ── Schlüsselbund (Altbestand aus CS4) ──────────────────────────────────────

/// Ablage der Fassung CS4: das Refresh-Token im Schlüsselbund. Wird nur noch
/// gelesen (Übernahme in den Tresor) und gelöscht.
pub trait Altbestand: Send + Sync {
    fn lesen(&self, konto: &str) -> Result<Option<String>, Fehler>;
    fn loeschen(&self, konto: &str) -> Result<(), Fehler>;
}

/// Schlüsselbund des Betriebssystems (Crate `keyring`).
pub struct Schluesselbund;

fn bund_eintrag(konto: &str) -> Result<keyring::Entry, Fehler> {
    keyring::Entry::new(DIENST, konto).map_err(|e| Fehler::Eingabe(format!("Schlüsselbund: {e}")))
}

impl Altbestand for Schluesselbund {
    fn lesen(&self, konto: &str) -> Result<Option<String>, Fehler> {
        match bund_eintrag(konto)?.get_password() {
            Ok(w) => Ok(Some(w)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Fehler::Eingabe(format!("Schlüsselbund nicht lesbar: {e}"))),
        }
    }
    fn loeschen(&self, konto: &str) -> Result<(), Fehler> {
        match bund_eintrag(konto)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(Fehler::Eingabe(format!("Schlüsselbund: {e}"))),
        }
    }
}

/// Altbestand im Speicher (Tests; leer = nichts zu übernehmen).
#[derive(Default)]
pub struct Speicher(Mutex<std::collections::BTreeMap<String, String>>);

impl Speicher {
    pub fn schreiben(&self, konto: &str, wert: &str) {
        self.0.lock().unwrap().insert(konto.into(), wert.into());
    }
}

impl Altbestand for Speicher {
    fn lesen(&self, konto: &str) -> Result<Option<String>, Fehler> {
        Ok(self.0.lock().unwrap().get(konto).cloned())
    }
    fn loeschen(&self, konto: &str) -> Result<(), Fehler> {
        self.0.lock().unwrap().remove(konto);
        Ok(())
    }
}

pub fn konto(k: &AnmeldeKonfig) -> String {
    format!("{}|{}", k.issuer.as_str().trim_end_matches('/'), k.client_id)
}

// ── Sitzung ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct Status {
    pub angemeldet: bool,
    /// Kontoname aus dem ID-Token (im Tresor gespeichert).
    pub konto: Option<String>,
    /// Das Refresh-Token ist ein Offline-Token (wochenlange Anmeldung).
    pub offline: bool,
    /// Anmeldung im Browser (Unix-Sekunden).
    pub angemeldet_seit: Option<u64>,
    /// Ablauf des Refresh-Tokens, sofern lesbar (Unix-Sekunden).
    pub refresh_bis: Option<u64>,
}

/// Token-Verwaltung eines Clients über den Tresor.
pub struct Sitzung {
    tresor: Tresor,
    alt: Arc<dyn Altbestand>,
    /// Erneuern, Anmelden und Abmelden nacheinander: drei Download-Ströme
    /// tauschen das Refresh-Token sonst dreimal (und mit Rotation zweimal
    /// mit einem schon verbrauchten).
    sperre: tokio::sync::Mutex<()>,
    /// Konten, für die der Altbestand in diesem Lauf schon geprüft wurde.
    geprueft: Mutex<BTreeSet<String>>,
}

fn passt(e: &Eintrag, k: &AnmeldeKonfig) -> bool {
    e.issuer == k.issuer.as_str().trim_end_matches('/') && e.client_id == k.client_id
}

impl Sitzung {
    pub fn neu(tresor: Tresor, alt: Arc<dyn Altbestand>) -> Self {
        Sitzung { tresor, alt, sperre: tokio::sync::Mutex::new(()), geprueft: Mutex::new(BTreeSet::new()) }
    }

    pub fn tresor_pfad(&self) -> std::path::PathBuf {
        self.tresor.pfad()
    }

    /// Einmal je Lauf und Konto: ein Refresh-Token aus dem Schlüsselbund
    /// (Fassung CS4) in den Tresor übernehmen, dann den Eintrag löschen.
    fn migrieren(&self, k: &AnmeldeKonfig) {
        let kt = konto(k);
        if !self.geprueft.lock().unwrap().insert(kt.clone()) {
            return;
        }
        if self.tresor.lesen().filter(|e| passt(e, k)).is_some() {
            let _ = self.alt.loeschen(&kt);
            return;
        }
        let Ok(Some(refresh)) = self.alt.lesen(&kt) else { return };
        let e = Eintrag {
            issuer: k.issuer.as_str().trim_end_matches('/').to_string(),
            client_id: k.client_id.clone(),
            refresh_offline: ist_offline(&refresh, None),
            refresh_bis: ablauf(&refresh),
            refresh_token: refresh,
            angemeldet_seit: jetzt(),
            ..Default::default()
        };
        if self.tresor.schreiben(&e).is_ok() {
            let _ = self.alt.loeschen(&kt);
        }
    }

    fn eintrag(&self, k: &AnmeldeKonfig) -> Option<Eintrag> {
        self.migrieren(k);
        self.tresor.lesen().filter(|e| passt(e, k) && !e.refresh_token.is_empty())
    }

    pub fn status(&self, k: &AnmeldeKonfig) -> Status {
        match self.eintrag(k) {
            None => Status::default(),
            Some(e) => Status {
                angemeldet: true,
                konto: e.konto,
                offline: e.refresh_offline,
                angemeldet_seit: Some(e.angemeldet_seit).filter(|s| *s > 0),
                refresh_bis: e.refresh_bis,
            },
        }
    }

    /// Schreibt eine Token-Antwort in den Tresor. `vorher`: der bisherige
    /// Eintrag bei einer Erneuerung (ohne neues Refresh-Token bleibt das
    /// alte, Rotation ersetzt es).
    fn uebernehmen(&self, k: &AnmeldeKonfig, t: &Tokens, vorher: Option<Eintrag>) -> Result<Eintrag, Fehler> {
        let vorher = vorher.unwrap_or_default();
        let refresh = t.refresh_token.clone().unwrap_or(vorher.refresh_token);
        let jetzt = jetzt();
        let refresh_bis = ablauf(&refresh)
            .or_else(|| t.refresh_expires_in.filter(|s| *s > 0).map(|s| jetzt + s))
            .or(vorher.refresh_bis);
        let e = Eintrag {
            issuer: k.issuer.as_str().trim_end_matches('/').to_string(),
            client_id: k.client_id.clone(),
            konto: t
                .id_token
                .as_deref()
                .and_then(konto_aus_jwt)
                .or_else(|| konto_aus_jwt(&t.access_token))
                .or(vorher.konto),
            access_bis: ablauf(&t.access_token).or_else(|| t.expires_in.map(|s| jetzt + s)).unwrap_or(0),
            access_token: t.access_token.clone(),
            refresh_offline: ist_offline(&refresh, t.scope.as_deref()),
            refresh_bis,
            refresh_token: refresh,
            id_token: t.id_token.clone().or(vorher.id_token),
            angemeldet_seit: if vorher.angemeldet_seit > 0 { vorher.angemeldet_seit } else { jetzt },
        };
        self.tresor.schreiben(&e)?;
        Ok(e)
    }

    /// Vollständige Anmeldung: Discovery, Loopback, Browser, Code-Tausch.
    /// `oeffne` startet den Systembrowser mit der Anmeldeadresse. Schlägt das
    /// fehl, wartet der Client trotzdem: Die Adresse steht in `w` zum Kopieren
    /// oder für die Anmeldung im Client-Fenster (#805).
    pub async fn anmelden(
        &self,
        client: &reqwest::Client,
        k: &AnmeldeKonfig,
        oeffne: &(dyn Fn(&Url) -> Result<(), Fehler> + Send + Sync),
        frist: Duration,
        w: &Warten,
    ) -> Result<Status, Fehler> {
        let ep = entdecken(client, &k.issuer).await?;
        let (l, redirect) = lauschen().await?;
        let p = Pkce::neu();
        let adresse = anmelde_url(&ep, k, &redirect, &p);
        *w.adresse.lock().unwrap() = Some(adresse.clone());
        if w.abgebrochen() {
            return Err(Fehler::Abgebrochen);
        }
        if let Err(e) = oeffne(&adresse) {
            *w.browser_fehler.lock().unwrap() = Some(e.to_string());
        }
        let code = rueckruf_abwarten(l, &p.state, frist, w).await?;
        let t = code_tauschen(client, &ep, &k.client_id, &code, &redirect, &p.verifier).await?;
        {
            let _g = self.sperre.lock().await;
            self.uebernehmen(k, &t, None)?;
        }
        let _ = self.alt.loeschen(&konto(k));
        Ok(self.status(k))
    }

    /// Anmeldung über die Maske des Clients (#804): Benutzername/Passwort am
    /// Token-Endpunkt aus der Discovery, Ergebnis wie beim Code-Tausch in den
    /// Tresor. Das Passwort hält der Aufrufer in `Zeroizing`.
    pub async fn anmelden_mit_passwort(
        &self,
        client: &reqwest::Client,
        k: &AnmeldeKonfig,
        benutzer: &str,
        passwort: &str,
    ) -> Result<Status, Fehler> {
        if !k.formular {
            return Err(Fehler::Eingabe(FORMULAR_AUS.into()));
        }
        let benutzer = benutzer.trim();
        if benutzer.is_empty() || passwort.is_empty() {
            return Err(Fehler::Eingabe(FORMULAR_LEER.into()));
        }
        let ep = entdecken(client, &k.issuer).await?;
        let t = passwort_tauschen(client, &ep, k, benutzer, passwort).await?;
        {
            let _g = self.sperre.lock().await;
            self.uebernehmen(k, &t, None)?;
        }
        let _ = self.alt.loeschen(&konto(k));
        Ok(self.status(k))
    }

    /// Ein gültiges Access-Token: aus dem Tresor, sonst per Refresh-Token
    /// (Ergebnis zurück in den Tresor). `None` heißt: nicht angemeldet —
    /// auch, wenn Keycloak die Erneuerung ablehnt (Tresor ist dann geleert).
    pub async fn token(&self, client: &reqwest::Client, k: &AnmeldeKonfig) -> Result<Option<String>, Fehler> {
        let frisch = |e: &Eintrag| !e.access_token.is_empty() && e.access_bis > jetzt() + VORLAUF_SEK;
        match self.eintrag(k) {
            None => return Ok(None),
            Some(e) if frisch(&e) => return Ok(Some(e.access_token)),
            Some(_) => {}
        }
        let _g = self.sperre.lock().await;
        // Eine gleichzeitige Anfrage hat vielleicht schon erneuert.
        let Some(e) = self.eintrag(k) else { return Ok(None) };
        if frisch(&e) {
            return Ok(Some(e.access_token));
        }
        let ep = entdecken(client, &k.issuer).await?;
        match erneuern(client, &ep, &k.client_id, &e.refresh_token).await {
            Ok(t) => Ok(Some(self.uebernehmen(k, &t, Some(e))?.access_token)),
            Err(Fehler::Server(m)) if m == ABGELEHNT => {
                self.tresor.loeschen()?;
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Abmelden: Refresh-Token beim Anmeldedienst widerrufen (ein Netzfehler
    /// hält das lokale Abmelden nicht auf), Tresor löschen, einen alten
    /// Schlüsselbund-Eintrag entfernen. Ergebnis: ob der Widerruf geklappt hat.
    pub async fn abmelden(&self, client: &reqwest::Client, k: &AnmeldeKonfig) -> Result<bool, Fehler> {
        let _g = self.sperre.lock().await;
        let mut widerrufen_ok = false;
        if let Some(e) = self.tresor.lesen().filter(|e| passt(e, k) && !e.refresh_token.is_empty()) {
            if let Ok(ep) = entdecken(client, &k.issuer).await {
                if let Some(r) = &ep.revocation_endpoint {
                    widerrufen_ok = widerrufen(client, r, &k.client_id, &e.refresh_token).await.is_ok();
                }
            }
        }
        self.tresor.loeschen()?;
        let _ = self.alt.loeschen(&konto(k));
        Ok(widerrufen_ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tresor::Schluessel;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncBufReadExt, BufReader};

    fn jwt_mit(inhalt: &str) -> String {
        let kopf = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256"}"#);
        format!("{kopf}.{}.sig", URL_SAFE_NO_PAD.encode(inhalt))
    }

    fn jwt(exp: u64) -> String {
        jwt_mit(&format!(r#"{{"exp":{exp},"azp":"leadlab-client"}}"#))
    }

    /// Offline-Refresh-Token wie bei Keycloak: `typ: Offline`, exp = Leerlaufgrenze.
    fn offline(nr: usize) -> String {
        jwt_mit(&format!(r#"{{"exp":{},"typ":"Offline","jti":"r{nr}"}}"#, jetzt() + 2_592_000))
    }

    /// Ein Keycloak im Kleinen: Discovery, Token-Endpunkt (Code + Refresh mit
    /// Rotation) mit PKCE-Prüfung, Widerruf (RFC 7009).
    struct Kc {
        issuer: Url,
        refreshs: Arc<AtomicUsize>,
        challenge: Arc<Mutex<String>>,
        /// Das gerade gültige Refresh-Token (Rotation ersetzt es).
        gueltig: Arc<Mutex<String>>,
        widerrufen: Arc<Mutex<Vec<String>>>,
    }

    async fn keycloak() -> Kc {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let basis = format!("http://{}", l.local_addr().unwrap());
        let issuer = Url::parse(&format!("{basis}/realms/leadlab")).unwrap();
        let kc = Kc {
            issuer: issuer.clone(),
            refreshs: Arc::new(AtomicUsize::new(0)),
            challenge: Arc::new(Mutex::new(String::new())),
            gueltig: Arc::new(Mutex::new(String::new())),
            widerrufen: Arc::new(Mutex::new(Vec::new())),
        };
        let (iss, z, ch, g, w) =
            (issuer.clone(), kc.refreshs.clone(), kc.challenge.clone(), kc.gueltig.clone(), kc.widerrufen.clone());
        tokio::spawn(async move {
            loop {
                let (mut s, _) = l.accept().await.unwrap();
                let (iss, z, ch, g, wd) = (iss.clone(), z.clone(), ch.clone(), g.clone(), w.clone());
                tokio::spawn(async move {
                    let (r, mut w) = s.split();
                    let mut r = BufReader::new(r);
                    let mut erste = String::new();
                    r.read_line(&mut erste).await.unwrap();
                    let mut laenge = 0usize;
                    loop {
                        let mut zeile = String::new();
                        r.read_line(&mut zeile).await.unwrap();
                        if zeile == "\r\n" || zeile.is_empty() {
                            break;
                        }
                        if let Some(v) = zeile.to_ascii_lowercase().strip_prefix("content-length:") {
                            laenge = v.trim().parse().unwrap();
                        }
                    }
                    let mut rumpf = vec![0u8; laenge];
                    r.read_exact(&mut rumpf).await.unwrap();
                    let form: std::collections::HashMap<String, String> =
                        url::form_urlencoded::parse(&rumpf).into_owned().collect();
                    let feld = |n: &str| form.get(n).map(String::as_str);
                    let (status, text) = if erste.contains("/.well-known/openid-configuration") {
                        ("200 OK", format!(
                            r#"{{"issuer":"{iss}","authorization_endpoint":"{iss}/protocol/openid-connect/auth","token_endpoint":"{iss}/protocol/openid-connect/token","revocation_endpoint":"{iss}/protocol/openid-connect/revoke"}}"#
                        ))
                    } else if erste.contains("/protocol/openid-connect/revoke") && feld("client_id") == Some("leadlab-client") {
                        wd.lock().unwrap().push(feld("token").unwrap_or("").to_string());
                        ("200 OK", String::new())
                    } else if erste.starts_with("POST") && feld("client_id") == Some("leadlab-client") {
                        match feld("grant_type") {
                            Some("authorization_code")
                                if feld("code") == Some("der-code")
                                    && super::challenge(feld("code_verifier").unwrap()) == *ch.lock().unwrap() =>
                            {
                                let r = offline(0);
                                *g.lock().unwrap() = r.clone();
                                let id = jwt_mit(r#"{"preferred_username":"kbm.mueller"}"#);
                                ("200 OK", format!(
                                    r#"{{"access_token":"{}","expires_in":300,"refresh_token":"{r}","refresh_expires_in":2592000,"id_token":"{id}","scope":"openid offline_access"}}"#,
                                    jwt(jetzt() + 300)
                                ))
                            }
                            // Passwort-Grant (#804) mit den Antworten von Keycloak 26.8.
                            Some("password") => {
                                assert!(feld("scope").unwrap_or("").split(' ').any(|s| s == OFFLINE), "Formular fordert offline_access an");
                                match (feld("username"), feld("password")) {
                                    (Some("kbm.mueller"), Some("richtig")) => {
                                        let r = offline(0);
                                        *g.lock().unwrap() = r.clone();
                                        let id = jwt_mit(r#"{"preferred_username":"kbm.mueller"}"#);
                                        ("200 OK", format!(
                                            r#"{{"access_token":"{}","expires_in":300,"refresh_token":"{r}","refresh_expires_in":2592000,"id_token":"{id}","scope":"openid offline_access"}}"#,
                                            jwt(jetzt() + 300)
                                        ))
                                    }
                                    (Some("verwaltung"), _) => ("401 Unauthorized", "<!DOCTYPE html><title>Zugriff verweigert</title>".to_string()),
                                    (Some("neu"), _) => ("400 Bad Request", r#"{"error":"invalid_grant","error_description":"Account is not fully set up"}"#.to_string()),
                                    (Some("alt-gesperrt"), _) => ("400 Bad Request", r#"{"error":"invalid_grant","error_description":"Account temporarily disabled"}"#.to_string()),
                                    (Some("abgeschaltet"), _) => ("400 Bad Request", r#"{"error":"invalid_grant","error_description":"Account disabled"}"#.to_string()),
                                    (Some("formular-aus"), _) => ("400 Bad Request", r#"{"error":"unauthorized_client","error_description":"Client not allowed for direct access grants"}"#.to_string()),
                                    _ => ("400 Bad Request", r#"{"error":"invalid_grant","error_description":"Invalid user credentials"}"#.to_string()),
                                }
                            }
                            Some("refresh_token") if feld("refresh_token") == Some(g.lock().unwrap().as_str()) => {
                                let nr = z.fetch_add(1, Ordering::SeqCst) + 1;
                                let r = offline(nr);
                                *g.lock().unwrap() = r.clone();
                                ("200 OK", format!(r#"{{"access_token":"{}","refresh_token":"{r}"}}"#, jwt(jetzt() + 300)))
                            }
                            _ => ("400 Bad Request", r#"{"error":"invalid_grant"}"#.to_string()),
                        }
                    } else {
                        ("404 Not Found", String::new())
                    };
                    let a = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
                    w.write_all(a.as_bytes()).await.unwrap();
                });
            }
        });
        kc
    }

    fn konfig(issuer: &Url) -> AnmeldeKonfig {
        AnmeldeKonfig { issuer: issuer.clone(), client_id: "leadlab-client".into(), scopes: vec!["openid".into()], formular: true }
    }

    fn sitzung(ordner: &std::path::Path, alt: Arc<Speicher>) -> Sitzung {
        Sitzung::neu(Tresor::neu(ordner, Schluessel::Fest([9; 32])), alt)
    }

    /// „Browser“: folgt der Anmeldeadresse wie Keycloak nach dem Login —
    /// Rückruf mit Code und state an die Loopback-Adresse.
    fn browser(kc: &Kc) -> impl Fn(&Url) -> Result<(), Fehler> + Send + Sync {
        let ch = kc.challenge.clone();
        move |u: &Url| -> Result<(), Fehler> {
            let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
            assert!(q["scope"].split(' ').any(|s| s == OFFLINE), "Login fordert offline_access an");
            *ch.lock().unwrap() = q["code_challenge"].clone();
            let ziel = format!("{}?code=der-code&state={}", q["redirect_uri"], q["state"]);
            tokio::spawn(async move { reqwest::get(ziel).await.unwrap() });
            Ok(())
        }
    }

    #[test]
    fn pkce_nach_rfc7636() {
        // Beispiel aus RFC 7636 Anhang B
        assert_eq!(challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let p = Pkce::neu();
        assert!((43..=128).contains(&p.verifier.len()));
        assert_eq!(p.challenge, challenge(&p.verifier));
        assert_ne!(Pkce::neu().verifier, p.verifier);
    }

    #[test]
    fn anmeldeadresse() {
        let iss = Url::parse("https://auth.x.de/realms/leadlab").unwrap();
        let ep = Endpunkte {
            issuer: iss.clone(),
            authorization_endpoint: Url::parse("https://auth.x.de/realms/leadlab/protocol/openid-connect/auth").unwrap(),
            token_endpoint: Url::parse("https://auth.x.de/realms/leadlab/protocol/openid-connect/token").unwrap(),
            revocation_endpoint: None,
        };
        let mut k = konfig(&iss);
        k.scopes = vec![];
        let p = Pkce { verifier: "v".into(), challenge: "c".into(), state: "s".into() };
        let u = anmelde_url(&ep, &k, "http://127.0.0.1:5000/rueckruf", &p);
        let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], "leadlab-client");
        assert_eq!(q["scope"], "openid offline_access");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:5000/rueckruf");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["code_challenge"], "c");
        assert_eq!(q["state"], "s");
        assert_eq!(q["response_type"], "code");
        // Scopes der Konfigurationsadresse bleiben die Basis, offline_access nicht doppelt.
        k.scopes = vec!["openid".into(), "profile".into(), "offline_access".into()];
        let q: std::collections::HashMap<_, _> =
            anmelde_url(&ep, &k, "http://127.0.0.1:5000/rueckruf", &p).query_pairs().into_owned().collect();
        assert_eq!(q["scope"], "openid profile offline_access");
    }

    #[test]
    fn offline_erkennen() {
        assert!(ist_offline(&offline(1), None));
        assert!(!ist_offline(&jwt_mit(r#"{"typ":"Refresh"}"#), Some("openid offline_access")));
        assert!(ist_offline("undurchsichtig", Some("openid offline_access")));
        assert!(!ist_offline("undurchsichtig", Some("openid")));
        assert!(!ist_offline("undurchsichtig", None));
    }

    #[test]
    fn rueckruf_auswerten() {
        assert_eq!(rueckruf_lesen("GET /rueckruf?state=s&code=abc HTTP/1.1", "s"), Rueckruf::Code("abc".into()));
        assert!(matches!(rueckruf_lesen("GET /rueckruf?state=x&code=abc HTTP/1.1", "s"), Rueckruf::Fehler(_)));
        assert!(matches!(
            rueckruf_lesen("GET /rueckruf?state=s&error=access_denied&error_description=abgelehnt HTTP/1.1", "s"),
            Rueckruf::Fehler(m) if m.contains("abgelehnt")
        ));
        assert_eq!(rueckruf_lesen("GET /favicon.ico HTTP/1.1", "s"), Rueckruf::Fremd);
        assert_eq!(rueckruf_lesen("POST /rueckruf HTTP/1.1", "s"), Rueckruf::Fremd);
        assert!(matches!(rueckruf_lesen("GET /rueckruf?state=s HTTP/1.1", "s"), Rueckruf::Fehler(_)));
    }

    #[test]
    fn ablauf_aus_jwt() {
        assert_eq!(ablauf(&jwt(1_800_000_000)), Some(1_800_000_000));
        assert_eq!(ablauf("kein.jwt"), None);
    }

    #[test]
    fn konto_aus_dem_id_token() {
        let t = |json: &str| format!("e30.{}.sig", URL_SAFE_NO_PAD.encode(json));
        assert_eq!(konto_aus_jwt(&t(r#"{"preferred_username":"kbm.mueller","email":"m@lk.de"}"#)).as_deref(), Some("kbm.mueller"));
        assert_eq!(konto_aus_jwt(&t(r#"{"preferred_username":"","email":"m@lk.de"}"#)).as_deref(), Some("m@lk.de"));
        assert_eq!(konto_aus_jwt(&t(r#"{"sub":"1234"}"#)).as_deref(), Some("1234"));
        assert_eq!(konto_aus_jwt(&jwt(1)), None);
        assert_eq!(konto_aus_jwt("kaputt"), None);
    }

    #[tokio::test]
    async fn loopback_nimmt_den_code_und_ignoriert_fremdes() {
        let (l, redirect) = lauschen().await.unwrap();
        assert!(redirect.starts_with("http://127.0.0.1:") && redirect.ends_with(RUECKRUF));
        let ziel = redirect.clone();
        let browser = tokio::spawn(async move {
            let c = reqwest::Client::new();
            let favicon = ziel.replace(RUECKRUF, "/favicon.ico");
            assert_eq!(c.get(&favicon).send().await.unwrap().status().as_u16(), 404);
            let r = c.get(format!("{ziel}?state=s1&code=xyz")).send().await.unwrap();
            (r.status().as_u16(), r.text().await.unwrap())
        });
        let w = Warten::neu();
        let code = rueckruf_abwarten(l, "s1", Duration::from_secs(5), &w).await.unwrap();
        assert!(w.verbunden(), "Browser hat sich am Loopback gemeldet");
        assert_eq!(code, "xyz");
        let (status, seite) = browser.await.unwrap();
        assert_eq!(status, 200);
        assert!(seite.contains("Angemeldet"));
    }

    #[tokio::test]
    async fn loopback_frist() {
        let (l, _) = lauschen().await.unwrap();
        let w = Warten::neu();
        let e = rueckruf_abwarten(l, "s", Duration::from_millis(100), &w).await.unwrap_err();
        // Die Meldung nennt die Ursache (#805), nicht nur „Zeit abgelaufen“.
        assert!(e.to_string().contains("kam keine Antwort vom Browser"), "{e}");
        assert!(!w.verbunden(), "kein Browser hat sich gemeldet");
    }

    #[test]
    fn frist_text_nennt_die_dauer() {
        assert_eq!(
            frist_text(FRIST),
            "Innerhalb von 5 Minuten kam keine Antwort vom Browser. Die Anmeldung ist abgebrochen; bitte erneut anmelden."
        );
        assert!(frist_text(Duration::from_secs(60)).starts_with("Innerhalb von einer Minute "));
        assert!(frist_text(Duration::from_secs(30)).starts_with("Innerhalb von 30 Sekunden "));
        assert!(frist_text(Duration::from_millis(100)).starts_with("Innerhalb von einer Sekunde "));
    }

    #[tokio::test]
    async fn abbruch_schliesst_den_loopback() {
        let (l, redirect) = lauschen().await.unwrap();
        let w = Arc::new(Warten::neu());
        let w2 = w.clone();
        let warten = tokio::spawn(async move { rueckruf_abwarten(l, "s", FRIST, &w2).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        w.abbrechen();
        let e = tokio::time::timeout(Duration::from_secs(2), warten).await.expect("Abbruch wirkt sofort").unwrap().unwrap_err();
        assert!(matches!(e, Fehler::Abgebrochen));
        // Der Port ist zu: Ein später Rückruf landet nirgends.
        let r = reqwest::Client::new().get(format!("{redirect}?state=s&code=x")).send().await;
        assert!(r.is_err(), "Loopback nach dem Abbruch geschlossen");
    }

    #[tokio::test]
    async fn abbruch_vor_dem_warten_und_browser_ohne_start() {
        // Abbruch, bevor das Warten beginnt (Knopf schnell gedrückt): wirkt trotzdem.
        let (l, _) = lauschen().await.unwrap();
        let w = Warten::neu();
        w.abbrechen();
        assert!(matches!(rueckruf_abwarten(l, "s", FRIST, &w).await, Err(Fehler::Abgebrochen)));

        // Browser startet nicht: Die Anmeldung wartet weiter, die Adresse steht
        // zum Kopieren bereit, der Fehler wird angezeigt — und ein Browser, der
        // die kopierte Adresse öffnet, schließt die Anmeldung ab.
        let kc = keycloak().await;
        let k = konfig(&kc.issuer);
        let d = tempfile::tempdir().unwrap();
        let s = sitzung(d.path(), Arc::default());
        let client = reqwest::Client::new();
        let w = Arc::new(Warten::neu());
        let kein_browser = |_: &Url| -> Result<(), Fehler> { Err(Fehler::Eingabe("Browser lässt sich nicht öffnen (xdg-open fehlt).".into())) };
        let (w2, kopiert) = (w.clone(), browser(&kc));
        let nutzer = tokio::spawn(async move {
            for _ in 0..100 {
                if let Some(u) = w2.adresse() {
                    assert!(w2.browser_fehler().unwrap().contains("xdg-open"));
                    return kopiert(&u);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("keine Adresse");
        });
        let st = s.anmelden(&client, &k, &kein_browser, Duration::from_secs(5), &w).await.unwrap();
        nutzer.await.unwrap().unwrap();
        assert!(st.angemeldet && w.verbunden());
    }

    #[tokio::test]
    async fn anmelden_legt_alles_in_den_tresor() {
        let kc = keycloak().await;
        let k = konfig(&kc.issuer);
        let d = tempfile::tempdir().unwrap();
        let s = sitzung(d.path(), Arc::default());
        let client = reqwest::Client::new();
        assert_eq!(s.token(&client, &k).await.unwrap(), None);
        assert_eq!(s.status(&k), Status::default());

        let st = s.anmelden(&client, &k, &browser(&kc), Duration::from_secs(5), &Warten::neu()).await.unwrap();
        assert!(st.angemeldet && st.offline);
        assert_eq!(st.konto.as_deref(), Some("kbm.mueller"));
        assert!(st.angemeldet_seit.unwrap() >= jetzt() - 5);
        assert!(st.refresh_bis.unwrap() > jetzt() + 2_500_000);
        let e = Tresor::neu(d.path(), Schluessel::Fest([9; 32])).lesen().unwrap();
        assert_eq!(e.refresh_token, *kc.gueltig.lock().unwrap());
        assert!(e.id_token.is_some() && !e.access_token.is_empty() && e.access_bis > jetzt());
        assert_eq!(e.issuer, kc.issuer.as_str().trim_end_matches('/'));

        // Frisches Access-Token kommt aus dem Tresor, auch nach einem Neustart.
        let t1 = s.token(&client, &k).await.unwrap().unwrap();
        let s2 = sitzung(d.path(), Arc::default());
        assert_eq!(s2.token(&client, &k).await.unwrap().unwrap(), t1);
        assert_eq!(kc.refreshs.load(Ordering::SeqCst), 0);
        // Anderer Anmeldedienst → nicht angemeldet.
        let mut fremd = k.clone();
        fremd.client_id = "anderer".into();
        assert!(!s2.status(&fremd).angemeldet);
    }

    #[tokio::test]
    async fn erneuerung_schreibt_den_tresor_und_folgt_der_rotation() {
        let kc = keycloak().await;
        let k = konfig(&kc.issuer);
        let d = tempfile::tempdir().unwrap();
        let tresor = Tresor::neu(d.path(), Schluessel::Fest([9; 32]));
        let s = sitzung(d.path(), Arc::default());
        let client = reqwest::Client::new();
        s.anmelden(&client, &k, &browser(&kc), Duration::from_secs(5), &Warten::neu()).await.unwrap();
        let seit = s.status(&k).angemeldet_seit;
        // Access-Token abgelaufen.
        let mut e = tresor.lesen().unwrap();
        e.access_bis = jetzt();
        tresor.schreiben(&e).unwrap();

        // Drei gleichzeitige Anfragen (Download-Ströme) erneuern genau einmal.
        let (a, b, c) = tokio::join!(s.token(&client, &k), s.token(&client, &k), s.token(&client, &k));
        let (a, b, c) = (a.unwrap().unwrap(), b.unwrap().unwrap(), c.unwrap().unwrap());
        assert!(a == b && b == c);
        assert_eq!(kc.refreshs.load(Ordering::SeqCst), 1);
        let neu = tresor.lesen().unwrap();
        assert_ne!(neu.refresh_token, e.refresh_token, "rotiertes Refresh-Token im Tresor");
        assert_eq!(neu.refresh_token, *kc.gueltig.lock().unwrap());
        assert_eq!(neu.access_token, a);
        assert!(neu.access_bis > jetzt() + VORLAUF_SEK);
        // Antwort ohne ID-Token: Konto, ID-Token und „angemeldet seit“ bleiben.
        assert_eq!(neu.konto.as_deref(), Some("kbm.mueller"));
        assert_eq!(neu.id_token, e.id_token);
        assert_eq!(s.status(&k).angemeldet_seit, seit);
        assert!(neu.refresh_offline);
    }

    #[tokio::test]
    async fn invalid_grant_leert_den_tresor() {
        let kc = keycloak().await;
        let k = konfig(&kc.issuer);
        let d = tempfile::tempdir().unwrap();
        let tresor = Tresor::neu(d.path(), Schluessel::Fest([9; 32]));
        let s = sitzung(d.path(), Arc::default());
        let client = reqwest::Client::new();
        s.anmelden(&client, &k, &browser(&kc), Duration::from_secs(5), &Warten::neu()).await.unwrap();
        // Admin widerruft die Offline-Sitzung: das Token im Tresor gilt nicht mehr.
        *kc.gueltig.lock().unwrap() = "widerrufen".into();
        let mut e = tresor.lesen().unwrap();
        e.access_bis = 0;
        tresor.schreiben(&e).unwrap();
        assert_eq!(s.token(&client, &k).await.unwrap(), None);
        assert!(!s.status(&k).angemeldet);
        assert!(!tresor.pfad().exists());
    }

    #[tokio::test]
    async fn abmelden_widerruft_und_loescht() {
        let kc = keycloak().await;
        let k = konfig(&kc.issuer);
        let d = tempfile::tempdir().unwrap();
        let alt = Arc::new(Speicher::default());
        let s = sitzung(d.path(), alt.clone());
        let client = reqwest::Client::new();
        s.anmelden(&client, &k, &browser(&kc), Duration::from_secs(5), &Warten::neu()).await.unwrap();
        let refresh = kc.gueltig.lock().unwrap().clone();
        alt.schreiben(&konto(&k), "alter-eintrag");
        assert!(s.abmelden(&client, &k).await.unwrap());
        assert_eq!(*kc.widerrufen.lock().unwrap(), vec![refresh]);
        assert!(!s.status(&k).angemeldet);
        assert!(!s.tresor_pfad().exists());
        assert_eq!(alt.lesen(&konto(&k)).unwrap(), None, "Schlüsselbund-Eintrag entfernt");
        // Ohne Anmeldung: kein Widerruf, kein Fehler.
        assert!(!s.abmelden(&client, &k).await.unwrap());
        assert_eq!(kc.widerrufen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn abmelden_ohne_netz_meldet_lokal_ab() {
        let d = tempfile::tempdir().unwrap();
        let k = konfig(&Url::parse("http://127.0.0.1:9/realms/leadlab").unwrap());
        let tresor = Tresor::neu(d.path(), Schluessel::Fest([9; 32]));
        tresor
            .schreiben(&Eintrag {
                issuer: "http://127.0.0.1:9/realms/leadlab".into(),
                client_id: "leadlab-client".into(),
                refresh_token: "r".into(),
                ..Default::default()
            })
            .unwrap();
        let s = sitzung(d.path(), Arc::default());
        assert!(s.status(&k).angemeldet);
        assert!(!s.abmelden(&reqwest::Client::new(), &k).await.unwrap());
        assert!(!s.status(&k).angemeldet);
    }

    #[tokio::test]
    async fn uebernahme_aus_dem_schluesselbund() {
        let kc = keycloak().await;
        let k = konfig(&kc.issuer);
        let d = tempfile::tempdir().unwrap();
        let alt = Arc::new(Speicher::default());
        // Fassung CS4: normales Refresh-Token im Schlüsselbund, beim Server gültig.
        let r = jwt_mit(&format!(r#"{{"exp":{},"typ":"Refresh"}}"#, jetzt() + 43_200));
        *kc.gueltig.lock().unwrap() = r.clone();
        alt.schreiben(&konto(&k), &r);
        let s = sitzung(d.path(), alt.clone());
        let st = s.status(&k);
        assert!(st.angemeldet && !st.offline);
        assert_eq!(alt.lesen(&konto(&k)).unwrap(), None, "Eintrag nach der Übernahme gelöscht");
        let e = Tresor::neu(d.path(), Schluessel::Fest([9; 32])).lesen().unwrap();
        assert_eq!(e.refresh_token, r);
        // Damit lässt sich erneuern wie gewohnt.
        assert!(s.token(&reqwest::Client::new(), &k).await.unwrap().is_some());
        assert_eq!(kc.refreshs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn falscher_verifier_wird_abgelehnt() {
        let kc = keycloak().await;
        let ep = entdecken(&reqwest::Client::new(), &kc.issuer).await.unwrap();
        assert!(ep.revocation_endpoint.is_some());
        *kc.challenge.lock().unwrap() = challenge("richtig");
        let e = code_tauschen(&reqwest::Client::new(), &ep, "leadlab-client", "der-code", "http://127.0.0.1/x", "falsch")
            .await
            .unwrap_err();
        assert_eq!(e.to_string(), ABGELEHNT);
    }

    #[tokio::test]
    async fn passwort_legt_alles_in_den_tresor() {
        let kc = keycloak().await;
        let k = konfig(&kc.issuer);
        let d = tempfile::tempdir().unwrap();
        let alt = Arc::new(Speicher::default());
        alt.schreiben(&konto(&k), "alter-eintrag");
        let s = sitzung(d.path(), alt.clone());
        let client = reqwest::Client::new();
        let passwort = zeroize::Zeroizing::new(String::from("richtig"));
        let st = s.anmelden_mit_passwort(&client, &k, "  kbm.mueller ", &passwort).await.unwrap();
        assert!(st.angemeldet && st.offline);
        assert_eq!(st.konto.as_deref(), Some("kbm.mueller"));
        assert!(st.angemeldet_seit.unwrap() >= jetzt() - 5);
        // Tresor vollständig wie nach dem Browser-Weg.
        let e = Tresor::neu(d.path(), Schluessel::Fest([9; 32])).lesen().unwrap();
        assert_eq!(e.refresh_token, *kc.gueltig.lock().unwrap());
        assert!(e.refresh_offline && e.id_token.is_some() && !e.access_token.is_empty() && e.access_bis > jetzt());
        assert_eq!(e.konto.as_deref(), Some("kbm.mueller"));
        assert_eq!(alt.lesen(&konto(&k)).unwrap(), None, "Schlüsselbund-Altlast entfernt");
        // Erneuern und Abmelden laufen wie gewohnt.
        let mut e2 = e.clone();
        e2.access_bis = 0;
        Tresor::neu(d.path(), Schluessel::Fest([9; 32])).schreiben(&e2).unwrap();
        assert!(s.token(&client, &k).await.unwrap().is_some());
        assert!(s.abmelden(&client, &k).await.unwrap());
    }

    #[tokio::test]
    async fn passwort_fehler_sind_verstaendlich() {
        let kc = keycloak().await;
        let k = konfig(&kc.issuer);
        let d = tempfile::tempdir().unwrap();
        let s = sitzung(d.path(), Arc::default());
        let client = reqwest::Client::new();
        let fehler = |benutzer: &'static str, passwort: &'static str| {
            let (s, client, k) = (&s, &client, &k);
            async move { s.anmelden_mit_passwort(client, k, benutzer, passwort).await.unwrap_err().to_string() }
        };
        assert_eq!(fehler("kbm.mueller", "falsch").await, FORMULAR_FALSCH);
        assert_eq!(fehler("niemand", "x").await, FORMULAR_FALSCH);
        assert_eq!(fehler("alt-gesperrt", "x").await, FORMULAR_GESPERRT);
        assert_eq!(fehler("abgeschaltet", "x").await, FORMULAR_DEAKTIVIERT);
        assert_eq!(fehler("neu", "x").await, FORMULAR_UNVOLLSTAENDIG);
        assert_eq!(fehler("formular-aus", "x").await, FORMULAR_NICHT_ERLAUBT);
        assert_eq!(fehler("verwaltung", "x").await, FORMULAR_NUR_BROWSER);
        assert_eq!(fehler("kbm.mueller", "").await, FORMULAR_LEER);
        assert_eq!(fehler(" ", "x").await, FORMULAR_LEER);
        // Kein Fehlertext nennt das Passwort, der Tresor bleibt leer.
        assert!(!fehler("kbm.mueller", "geheim-123").await.contains("geheim-123"));
        assert!(!s.status(&k).angemeldet && !s.tresor_pfad().exists());
        // Vom Betreiber abgeschaltet: keine Anfrage an den Anmeldedienst.
        let mut aus = k.clone();
        aus.formular = false;
        assert_eq!(s.anmelden_mit_passwort(&client, &aus, "kbm.mueller", "richtig").await.unwrap_err().to_string(), FORMULAR_AUS);
        // Netzfehler wie bisher.
        let weg = konfig(&Url::parse("http://127.0.0.1:9/realms/leadlab").unwrap());
        assert!(matches!(s.anmelden_mit_passwort(&client, &weg, "a", "b").await, Err(Fehler::Netz(_))));
    }

    /// Gegen einen echten Keycloak (Wegwerf-Container mit realm-leadlab.json):
    /// `LEADLAB_TEST_KEYCLOAK=<issuer> LEADLAB_TEST_BENUTZER=… LEADLAB_TEST_PASSWORT=…
    /// cargo test -- --ignored echter_keycloak`.
    #[tokio::test]
    #[ignore]
    async fn echter_keycloak_passwort() {
        let Ok(iss) = std::env::var("LEADLAB_TEST_KEYCLOAK") else { return };
        let k = konfig(&Url::parse(&iss).unwrap());
        let d = tempfile::tempdir().unwrap();
        let s = sitzung(d.path(), Arc::default());
        let c = reqwest::Client::new();
        let b = std::env::var("LEADLAB_TEST_BENUTZER").unwrap();
        let p = zeroize::Zeroizing::new(std::env::var("LEADLAB_TEST_PASSWORT").unwrap());
        assert_eq!(s.anmelden_mit_passwort(&c, &k, &b, "falsch").await.unwrap_err().to_string(), FORMULAR_FALSCH);
        let st = s.anmelden_mit_passwort(&c, &k, &b, &p).await.unwrap();
        println!("angemeldet: {st:?}");
        assert!(st.angemeldet && st.offline && st.konto.as_deref() == Some(b.as_str()));
        if let Ok(admin) = std::env::var("LEADLAB_TEST_ADMIN") {
            assert_eq!(s.anmelden_mit_passwort(&c, &k, &admin, &p).await.unwrap_err().to_string(), FORMULAR_NUR_BROWSER);
        }
        assert!(s.abmelden(&c, &k).await.unwrap(), "Widerruf am echten Keycloak");
    }

    #[tokio::test]
    async fn discovery_prueft_den_aussteller() {
        let kc = keycloak().await;
        let mut anders = kc.issuer.clone();
        anders.set_path("/realms/anders");
        // anderer Pfad → 404 vom Testserver
        assert!(entdecken(&reqwest::Client::new(), &anders).await.is_err());
        assert!(entdecken(&reqwest::Client::new(), &kc.issuer).await.is_ok());
    }
}
