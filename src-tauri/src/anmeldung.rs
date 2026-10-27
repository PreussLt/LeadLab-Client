//! Anmeldung für die Datenauslieferung (Konzept §7, CS4 #759).
//!
//! OIDC Authorization Code mit PKCE (S256) und Loopback-Redirect nach
//! RFC 8252: Der Client öffnet den Systembrowser auf die Anmeldeseite des
//! Servers (Keycloak, öffentlicher Client `leadlab-client`) und wartet auf
//! `http://127.0.0.1:<zufallsport>/rueckruf`. Den Code tauscht er gegen
//! Access- und Refresh-Token.
//!
//! - Das **Refresh-Token** liegt im Schlüsselbund des Betriebssystems
//!   (Windows Credential Manager, Secret Service unter Linux; Crate
//!   `keyring`), nie in einer Datei. Konto ist `<issuer>|<client_id>`, so hat
//!   jeder Server seinen eigenen Eintrag.
//! - Das **Access-Token** lebt nur im Speicher und wird vor Ablauf erneuert.
//!   Es öffnet ausschließlich `/daten/` (Zielgruppe `leadlab-daten`); die
//!   Oberfläche im Fenster meldet sich wie im Browser selbst an.

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

/// Dienstname im Schlüsselbund.
pub const DIENST: &str = "de.leadlab.client";
/// Pfad der Rückrufadresse auf dem Loopback.
pub const RUECKRUF: &str = "/rueckruf";
/// So lange wartet der Client auf den Rückruf aus dem Browser.
pub const FRIST: Duration = Duration::from_secs(300);
/// Access-Token gilt als abgelaufen, wenn weniger als das übrig ist.
pub const VORLAUF_SEK: u64 = 60;

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
    Ok(ep)
}

/// Adresse der Anmeldeseite für den Systembrowser.
pub fn anmelde_url(ep: &Endpunkte, k: &AnmeldeKonfig, redirect: &str, p: &Pkce) -> Url {
    let mut scopes = k.scopes.clone();
    if !scopes.iter().any(|s| s == "openid") {
        scopes.insert(0, "openid".into());
    }
    let mut u = ep.authorization_endpoint.clone();
    u.query_pairs_mut()
        .append_pair("client_id", &k.client_id)
        .append_pair("response_type", "code")
        .append_pair("scope", &scopes.join(" "))
        .append_pair("redirect_uri", redirect)
        .append_pair("state", &p.state)
        .append_pair("code_challenge", &p.challenge)
        .append_pair("code_challenge_method", "S256");
    u
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
/// kleinen Seite. Andere Anfragen (favicon) bekommen 404.
pub async fn rueckruf_abwarten(l: TcpListener, state: &str, frist: Duration) -> Result<String, Fehler> {
    let warten = async {
        loop {
            let (mut s, _) = l.accept().await?;
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
    tokio::time::timeout(frist, warten)
        .await
        .map_err(|_| Fehler::Eingabe("Anmeldung nicht abgeschlossen (Zeit abgelaufen).".into()))?
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
}

#[derive(Deserialize)]
struct FehlerAntwort {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

/// Abgelehnt (invalid_grant: Sitzung beendet, Konto gesperrt) — neu anmelden.
pub const ABGELEHNT: &str = "Die Anmeldung ist abgelaufen. Bitte neu anmelden.";

async fn token_anfrage(client: &reqwest::Client, ep: &Endpunkte, form: &[(&str, &str)]) -> Result<Tokens, Fehler> {
    let antwort = client
        .post(ep.token_endpoint.clone())
        .form(form)
        .send()
        .await
        .map_err(|e| Fehler::Netz(format!("Anmeldedienst nicht erreichbar ({e}).")))?;
    let status = antwort.status();
    if status.as_u16() == 400 || status.as_u16() == 401 {
        let f: Option<FehlerAntwort> = antwort.json().await.ok();
        return Err(match f {
            Some(f) if f.error == "invalid_grant" => Fehler::Server(ABGELEHNT.into()),
            Some(f) => Fehler::Server(format!("Anmeldedienst: {}", f.error_description.unwrap_or(f.error))),
            None => Fehler::Server(format!("Anmeldedienst antwortet mit {}.", status.as_u16())),
        });
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
    )
    .await
}

pub async fn erneuern(client: &reqwest::Client, ep: &Endpunkte, client_id: &str, refresh: &str) -> Result<Tokens, Fehler> {
    token_anfrage(client, ep, &[("grant_type", "refresh_token"), ("client_id", client_id), ("refresh_token", refresh)]).await
}

/// `exp` aus einem JWT, ungeprüft — nur, um die Erneuerung zu planen; die
/// Prüfung macht der Server.
pub fn ablauf(jwt: &str) -> Option<u64> {
    let teil = jwt.split('.').nth(1)?;
    let roh = URL_SAFE_NO_PAD.decode(teil.trim_end_matches('=')).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&roh).ok()?;
    v.get("exp")?.as_f64().map(|e| e as u64)
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

// ── Schlüsselbund ──────────────────────────────────────────────────────────

/// Ablage des Refresh-Tokens. Im Betrieb der Schlüsselbund, in Tests ein Speicher.
pub trait Tresor: Send + Sync {
    fn lesen(&self, konto: &str) -> Result<Option<String>, Fehler>;
    fn schreiben(&self, konto: &str, wert: &str) -> Result<(), Fehler>;
    fn loeschen(&self, konto: &str) -> Result<(), Fehler>;
}

/// Schlüsselbund des Betriebssystems (Crate `keyring`).
pub struct Schluesselbund;

fn eintrag(konto: &str) -> Result<keyring::Entry, Fehler> {
    keyring::Entry::new(DIENST, konto).map_err(|e| Fehler::Eingabe(format!("Schlüsselbund: {e}")))
}

impl Tresor for Schluesselbund {
    fn lesen(&self, konto: &str) -> Result<Option<String>, Fehler> {
        match eintrag(konto)?.get_password() {
            Ok(w) => Ok(Some(w)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Fehler::Eingabe(format!("Schlüsselbund nicht lesbar: {e}"))),
        }
    }
    fn schreiben(&self, konto: &str, wert: &str) -> Result<(), Fehler> {
        eintrag(konto)?
            .set_password(wert)
            .map_err(|e| Fehler::Eingabe(format!("Schlüsselbund nicht beschreibbar: {e}")))
    }
    fn loeschen(&self, konto: &str) -> Result<(), Fehler> {
        match eintrag(konto)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(Fehler::Eingabe(format!("Schlüsselbund: {e}"))),
        }
    }
}

/// Speicher-Tresor für Tests und als Rückfall ohne Schlüsselbund.
#[derive(Default)]
pub struct Speicher(Mutex<std::collections::BTreeMap<String, String>>);

impl Tresor for Speicher {
    fn lesen(&self, konto: &str) -> Result<Option<String>, Fehler> {
        Ok(self.0.lock().unwrap().get(konto).cloned())
    }
    fn schreiben(&self, konto: &str, wert: &str) -> Result<(), Fehler> {
        self.0.lock().unwrap().insert(konto.into(), wert.into());
        Ok(())
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

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Status {
    pub angemeldet: bool,
    /// Kontoname aus dem ID-Token — erst bekannt, wenn in diesem Lauf ein
    /// Token geholt wurde (nach einem Neustart liegt nur das Refresh-Token vor).
    pub konto: Option<String>,
}

/// Token-Verwaltung eines Clients: hält das Access-Token im Speicher und das
/// Refresh-Token im Tresor.
pub struct Sitzung {
    tresor: Arc<dyn Tresor>,
    /// (Konto, Access-Token, Ablauf in Unix-Sekunden)
    zugang: Mutex<Option<(String, String, u64)>>,
    /// (Konto, Anzeigename) aus dem letzten Token.
    name: Mutex<Option<(String, String)>>,
}

impl Sitzung {
    pub fn neu(tresor: Arc<dyn Tresor>) -> Self {
        Sitzung { tresor, zugang: Mutex::new(None), name: Mutex::new(None) }
    }

    pub fn status(&self, k: &AnmeldeKonfig) -> Status {
        let angemeldet = matches!(self.tresor.lesen(&konto(k)), Ok(Some(_)));
        let konto = konto(k);
        let name = self.name.lock().unwrap().clone().filter(|(kt, _)| angemeldet && *kt == konto).map(|(_, n)| n);
        Status { angemeldet, konto: name }
    }

    fn uebernehmen(&self, k: &AnmeldeKonfig, t: &Tokens) -> Result<(), Fehler> {
        let konto = konto(k);
        if let Some(r) = &t.refresh_token {
            self.tresor.schreiben(&konto, r)?;
        }
        let bis = ablauf(&t.access_token).or_else(|| t.expires_in.map(|s| jetzt() + s)).unwrap_or(0);
        let name = t.id_token.as_deref().and_then(konto_aus_jwt).or_else(|| konto_aus_jwt(&t.access_token));
        if let Some(n) = name {
            *self.name.lock().unwrap() = Some((konto.clone(), n));
        }
        *self.zugang.lock().unwrap() = Some((konto, t.access_token.clone(), bis));
        Ok(())
    }

    /// Vollständige Anmeldung: Discovery, Loopback, Browser, Code-Tausch.
    /// `oeffne` startet den Systembrowser mit der Anmeldeadresse.
    pub async fn anmelden(
        &self,
        client: &reqwest::Client,
        k: &AnmeldeKonfig,
        oeffne: &(dyn Fn(&Url) -> Result<(), Fehler> + Send + Sync),
        frist: Duration,
    ) -> Result<Status, Fehler> {
        let ep = entdecken(client, &k.issuer).await?;
        let (l, redirect) = lauschen().await?;
        let p = Pkce::neu();
        oeffne(&anmelde_url(&ep, k, &redirect, &p))?;
        let code = rueckruf_abwarten(l, &p.state, frist).await?;
        let t = code_tauschen(client, &ep, &k.client_id, &code, &redirect, &p.verifier).await?;
        self.uebernehmen(k, &t)?;
        Ok(self.status(k))
    }

    /// Ein gültiges Access-Token: aus dem Speicher, sonst per Refresh-Token.
    /// `None` heißt: nicht angemeldet (oder abgelaufen und abgelehnt).
    pub async fn token(&self, client: &reqwest::Client, k: &AnmeldeKonfig) -> Result<Option<String>, Fehler> {
        let konto = konto(k);
        if let Some((kt, t, bis)) = self.zugang.lock().unwrap().clone() {
            if kt == konto && bis > jetzt() + VORLAUF_SEK {
                return Ok(Some(t));
            }
        }
        let Some(refresh) = self.tresor.lesen(&konto)? else {
            return Ok(None);
        };
        let ep = entdecken(client, &k.issuer).await?;
        match erneuern(client, &ep, &k.client_id, &refresh).await {
            Ok(t) => {
                self.uebernehmen(k, &t)?;
                Ok(Some(t.access_token))
            }
            Err(Fehler::Server(m)) if m == ABGELEHNT => {
                self.tresor.loeschen(&konto)?;
                *self.zugang.lock().unwrap() = None;
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    pub fn abmelden(&self, k: &AnmeldeKonfig) -> Result<(), Fehler> {
        *self.zugang.lock().unwrap() = None;
        *self.name.lock().unwrap() = None;
        self.tresor.loeschen(&konto(k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncBufReadExt, BufReader};

    fn jwt(exp: u64) -> String {
        let kopf = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256"}"#);
        let inhalt = URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp},"azp":"leadlab-client"}}"#));
        format!("{kopf}.{inhalt}.sig")
    }

    /// Ein Keycloak im Kleinen: Discovery, Token-Endpunkt (Code + Refresh)
    /// mit PKCE-Prüfung gegen die zuletzt gesehene Challenge.
    struct Kc {
        issuer: Url,
        refreshs: Arc<AtomicUsize>,
        challenge: Arc<Mutex<String>>,
    }

    async fn keycloak() -> Kc {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let basis = format!("http://{}", l.local_addr().unwrap());
        let issuer = Url::parse(&format!("{basis}/realms/leadlab")).unwrap();
        let refreshs = Arc::new(AtomicUsize::new(0));
        let challenge = Arc::new(Mutex::new(String::new()));
        let (iss, z, ch) = (issuer.clone(), refreshs.clone(), challenge.clone());
        tokio::spawn(async move {
            loop {
                let (mut s, _) = l.accept().await.unwrap();
                let (iss, z, ch) = (iss.clone(), z.clone(), ch.clone());
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
                    let (status, text) = if erste.contains("/.well-known/openid-configuration") {
                        ("200 OK", format!(
                            r#"{{"issuer":"{iss}","authorization_endpoint":"{iss}/protocol/openid-connect/auth","token_endpoint":"{iss}/protocol/openid-connect/token"}}"#
                        ))
                    } else if erste.starts_with("POST") && form.get("client_id").map(String::as_str) == Some("leadlab-client") {
                        match form.get("grant_type").map(String::as_str) {
                            Some("authorization_code")
                                if form.get("code").map(String::as_str) == Some("der-code")
                                    && super::challenge(form.get("code_verifier").unwrap()) == *ch.lock().unwrap() =>
                            {
                                ("200 OK", format!(
                                    r#"{{"access_token":"{}","expires_in":300,"refresh_token":"r1"}}"#,
                                    jwt(jetzt() + 300)
                                ))
                            }
                            Some("refresh_token") if form.get("refresh_token").map(String::as_str) == Some("r1") => {
                                z.fetch_add(1, Ordering::SeqCst);
                                ("200 OK", format!(r#"{{"access_token":"{}","refresh_token":"r1"}}"#, jwt(jetzt() + 300)))
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
        Kc { issuer, refreshs, challenge }
    }

    fn konfig(issuer: &Url) -> AnmeldeKonfig {
        AnmeldeKonfig { issuer: issuer.clone(), client_id: "leadlab-client".into(), scopes: vec!["openid".into()] }
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
        };
        let mut k = konfig(&iss);
        k.scopes = vec![];
        let p = Pkce { verifier: "v".into(), challenge: "c".into(), state: "s".into() };
        let u = anmelde_url(&ep, &k, "http://127.0.0.1:5000/rueckruf", &p);
        let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], "leadlab-client");
        assert_eq!(q["scope"], "openid");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:5000/rueckruf");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["code_challenge"], "c");
        assert_eq!(q["state"], "s");
        assert_eq!(q["response_type"], "code");
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
        let code = rueckruf_abwarten(l, "s1", Duration::from_secs(5)).await.unwrap();
        assert_eq!(code, "xyz");
        let (status, seite) = browser.await.unwrap();
        assert_eq!(status, 200);
        assert!(seite.contains("Angemeldet"));
    }

    #[tokio::test]
    async fn loopback_frist() {
        let (l, _) = lauschen().await.unwrap();
        let e = rueckruf_abwarten(l, "s", Duration::from_millis(100)).await.unwrap_err();
        assert!(e.to_string().contains("Zeit abgelaufen"));
    }

    #[tokio::test]
    async fn anmelden_erneuern_abmelden() {
        let kc = keycloak().await;
        let k = konfig(&kc.issuer);
        let tresor = Arc::new(Speicher::default());
        let s = Sitzung::neu(tresor.clone());
        let client = reqwest::Client::new();
        assert_eq!(s.token(&client, &k).await.unwrap(), None);
        assert!(!s.status(&k).angemeldet);

        // „Browser“: folgt der Anmeldeadresse wie Keycloak nach dem Login —
        // Rückruf mit Code und state an die Loopback-Adresse.
        let ch = kc.challenge.clone();
        let oeffne = move |u: &Url| -> Result<(), Fehler> {
            let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
            *ch.lock().unwrap() = q["code_challenge"].clone();
            let ziel = format!("{}?code=der-code&state={}", q["redirect_uri"], q["state"]);
            tokio::spawn(async move { reqwest::get(ziel).await.unwrap() });
            Ok(())
        };
        let st = s.anmelden(&client, &k, &oeffne, Duration::from_secs(5)).await.unwrap();
        assert!(st.angemeldet);
        assert_eq!(tresor.lesen(&konto(&k)).unwrap().as_deref(), Some("r1"));
        let t1 = s.token(&client, &k).await.unwrap().unwrap();
        assert_eq!(kc.refreshs.load(Ordering::SeqCst), 0, "frisches Token aus dem Speicher");

        // Neustart des Clients: nur das Refresh-Token im Tresor.
        let s2 = Sitzung::neu(tresor.clone());
        let t2 = s2.token(&client, &k).await.unwrap().unwrap();
        assert_eq!(kc.refreshs.load(Ordering::SeqCst), 1);
        assert_eq!(ablauf(&t2).is_some(), ablauf(&t1).is_some());

        // Refresh-Token abgelehnt → Tresor leer, nicht angemeldet.
        tresor.schreiben(&konto(&k), "verbraucht").unwrap();
        let s3 = Sitzung::neu(tresor.clone());
        assert_eq!(s3.token(&client, &k).await.unwrap(), None);
        assert!(!s3.status(&k).angemeldet);

        s.abmelden(&k).unwrap();
        assert_eq!(tresor.lesen(&konto(&k)).unwrap(), None);
    }

    #[tokio::test]
    async fn falscher_verifier_wird_abgelehnt() {
        let kc = keycloak().await;
        let ep = entdecken(&reqwest::Client::new(), &kc.issuer).await.unwrap();
        *kc.challenge.lock().unwrap() = challenge("richtig");
        let e = code_tauschen(&reqwest::Client::new(), &ep, "leadlab-client", "der-code", "http://127.0.0.1/x", "falsch")
            .await
            .unwrap_err();
        assert_eq!(e.to_string(), ABGELEHNT);
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
