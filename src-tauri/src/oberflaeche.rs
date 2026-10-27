//! Die Oberfläche im Fenster übernimmt die Anmeldung des Clients
//! (Anmeldedienst AD4, LeadLab #800; Konzept Client §7/§8).
//!
//! Ein Login im Client gilt für Daten **und** Oberfläche:
//!
//! 1. Vor dem Öffnen holt der Client ein frisches Access-Token aus dem Tresor
//!    und ruft `POST <oberfläche>/api/auth/client-sitzung` auf. Der BFF
//!    tauscht es bei Keycloak und antwortet mit `Set-Cookie: leadlab_sitzung`.
//! 2. Dieses Cookie setzt der Client per `set_cookie` ins Fenster — für die
//!    Domain aus der Antwort, ohne Domain für Oberflächen- und
//!    Startzentralen-Host einzeln — und navigiert erst dann. Greift
//!    `set_cookie` nicht (WebKitGTK/WebView2), geht es über den Einmalcode:
//!    `GET /api/auth/client-login?code=…&ziel=…` setzt das Cookie in einer
//!    gewöhnlichen Antwort.
//! 3. Die BFF-Sitzung hat kein Refresh-Token und endet nach 5 min; der Client
//!    verlängert sie vorher mit seinem Cookie (dieselbe Zeile).
//! 4. Navigiert die Oberfläche auf `/api/auth/login` (Sitzung abgelaufen),
//!    fängt der Client das ab, erneuert die Sitzung und geht zum Ziel.
//!
//! Die Server-Seite im Fenster bekommt dafür keine IPC-Rechte: Alles läuft im
//! Rust-Kern, die Seite sieht nur ihr Cookie.

use serde::Deserialize;
use tauri::webview::cookie::{Cookie, SameSite};
use url::Url;

use crate::anmeldung::Sitzung;
use crate::fehler::Fehler;
use crate::konfig::Anmeldung as AnmeldeKonfig;

pub const SITZUNGS_COOKIE: &str = "leadlab_sitzung";
pub const LOGIN_PFAD: &str = "/api/auth/login";
pub const SITZUNG_PFAD: &str = "/api/auth/client-sitzung";
pub const CLIENT_LOGIN_PFAD: &str = "/api/auth/client-login";
pub const ABMELDEN_PFAD: &str = "/api/auth/abmelden";
/// So lange vor `laeuft_bis` verlängert der Client die Sitzung.
pub const VERLAENGERN_VOR_SEK: u64 = 90;
/// Frühestens so oft (eine Sitzung, die kürzer läuft, wird trotzdem nicht im Sekundentakt verlängert).
pub const VERLAENGERN_MIN_SEK: u64 = 30;

/// Eine Sitzung der Oberfläche, wie der BFF sie ausgegeben hat.
#[derive(Debug, Clone, PartialEq)]
pub struct BffSitzung {
    /// Wert des Cookies `leadlab_sitzung`.
    pub wert: String,
    /// Die Set-Cookie-Zeile des BFF, gelesen.
    pub cookie: Cookie<'static>,
    /// Ende ohne Verlängerung (Unix-Sekunden).
    pub laeuft_bis: u64,
    /// Für den Rückfall über `/api/auth/client-login` (60 s, einmal).
    pub einmalcode: String,
}

#[derive(Deserialize)]
struct Antwort {
    laeuft_bis: f64,
    einmalcode: String,
}

#[derive(Deserialize)]
struct FehlerAntwort {
    #[serde(default)]
    detail: Option<String>,
}

/// Wurzel der Adresse (`https://uebung.x.de/`), an der die Auth-Routen hängen.
pub fn wurzel(u: &Url) -> Url {
    let mut w = u.clone();
    w.set_path("/");
    w.set_query(None);
    w.set_fragment(None);
    w
}

fn route(oberflaeche: &Url, pfad: &str) -> Url {
    let mut u = wurzel(oberflaeche);
    u.set_path(pfad);
    u
}

/// Liest eine Set-Cookie-Zeile; nur `leadlab_sitzung` mit Wert zählt.
pub fn cookie_aus_zeile(zeile: &str) -> Option<Cookie<'static>> {
    let c = Cookie::parse(zeile.to_string()).ok()?;
    (c.name() == SITZUNGS_COOKIE && !c.value().is_empty()).then_some(c)
}

/// Die Cookies, die der Client ins Fenster setzt. Trägt die Antwort eine
/// Domain (`.lead.lab`), gilt das eine Cookie für Oberfläche und
/// Startzentrale. Ohne Domain (Hosts auf verschiedenen Domains) bekommt jeder
/// Host sein eigenes — der BFF kennt die Sitzung am Wert, nicht am Host.
pub fn cookies_fuer(c: &Cookie<'static>, hosts: &[&Url]) -> Vec<Cookie<'static>> {
    let mut basis = c.clone();
    basis.set_path("/");
    basis.set_http_only(true);
    if basis.same_site().is_none() {
        basis.set_same_site(SameSite::Lax);
    }
    if basis.domain().is_some() {
        return vec![basis];
    }
    let mut namen: Vec<String> = Vec::new();
    for h in hosts {
        if let Some(n) = h.host_str() {
            if !namen.iter().any(|x| x == n) {
                namen.push(n.to_string());
            }
        }
    }
    namen
        .into_iter()
        .map(|n| {
            let mut k = basis.clone();
            k.set_domain(n);
            k
        })
        .collect()
}

/// `POST /api/auth/client-sitzung`: Sitzung aus dem Client-Token, mit `alt`
/// (Cookie-Wert) wird eine bestehende Client-Sitzung verlängert.
pub async fn sitzung_holen(
    client: &reqwest::Client,
    oberflaeche: &Url,
    token: &str,
    alt: Option<&str>,
) -> Result<BffSitzung, Fehler> {
    let mut anfrage = client.post(route(oberflaeche, SITZUNG_PFAD)).bearer_auth(token);
    if let Some(w) = alt {
        anfrage = anfrage.header(reqwest::header::COOKIE, format!("{SITZUNGS_COOKIE}={w}"));
    }
    let antwort = anfrage.send().await.map_err(|e| Fehler::Netz(format!("LeadLab-Server nicht erreichbar ({e}).")))?;
    let status = antwort.status();
    if !status.is_success() {
        let text = antwort.json::<FehlerAntwort>().await.ok().and_then(|f| f.detail).unwrap_or_default();
        let grund = if text.is_empty() { format!("Antwort {}", status.as_u16()) } else { text };
        return Err(match status.as_u16() {
            401 => Fehler::Server(format!("Die Anmeldung gilt nicht für die Oberfläche ({grund}).")),
            403 => Fehler::Server(grund),
            _ => Fehler::Server(format!("Sitzung für die Oberfläche nicht möglich ({grund}).")),
        });
    }
    let cookie = antwort
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(cookie_aus_zeile)
        .ok_or_else(|| Fehler::Server("Der Server hat kein Sitzungs-Cookie gesetzt.".into()))?;
    let a: Antwort = antwort.json().await.map_err(|e| Fehler::Server(format!("Antwort des Servers unlesbar: {e}")))?;
    Ok(BffSitzung { wert: cookie.value().to_string(), cookie, laeuft_bis: a.laeuft_bis.max(0.0) as u64, einmalcode: a.einmalcode })
}

/// Rückfall-Adresse: der Einmalcode setzt das Cookie in der Antwort.
pub fn client_login_url(oberflaeche: &Url, code: &str, ziel: &Url) -> Url {
    let mut u = route(oberflaeche, CLIENT_LOGIN_PFAD);
    u.query_pairs_mut().append_pair("code", code).append_pair("ziel", ziel.as_str());
    u
}

/// Ist das die Login-Umleitung der eigenen Oberfläche? Dann das Ziel, auf das
/// die Seite nach dem Login wollte (`ziel`, sonst die Wurzel). Fremde Hosts
/// und andere Pfade: `None` — die Navigation läuft normal.
pub fn login_ziel(u: &Url, eigene: &[&Url]) -> Option<Url> {
    if u.path().trim_end_matches('/') != LOGIN_PFAD {
        return None;
    }
    let herkunft = u.origin();
    let eigen = eigene.iter().find(|e| e.origin() == herkunft)?;
    let ziel = u.query_pairs().find(|(k, _)| k == "ziel").map(|(_, v)| v.into_owned());
    let ziel = match ziel {
        // Nur Ziele auf den eigenen Hosts (wie ZielPruefen im BFF).
        Some(z) if z.starts_with('/') && !z.starts_with("//") => wurzel(eigen).join(&z).ok(),
        Some(z) => Url::parse(&z).ok().filter(|z| eigene.iter().any(|e| e.origin() == z.origin())),
        None => None,
    };
    Some(ziel.unwrap_or_else(|| wurzel(eigen)))
}

/// Wann verlängern (Unix-Sekunden)?
pub fn verlaengern_um(laeuft_bis: u64, jetzt: u64) -> u64 {
    laeuft_bis.saturating_sub(VERLAENGERN_VOR_SEK).max(jetzt + VERLAENGERN_MIN_SEK)
}

/// Hat der Server die Sitzung selbst beendet? Eine Verlängerung mit dem alten
/// Cookie, die ein **neues** Cookie liefert, obwohl die alte Sitzung noch
/// liefe: abgemeldet in der Oberfläche oder vom Admin beendet.
pub fn vom_server_beendet(alt: &BffSitzung, neu: &BffSitzung, jetzt: u64) -> bool {
    neu.wert != alt.wert && jetzt + 5 < alt.laeuft_bis
}

/// `POST /api/auth/abmelden` mit dem Cookie der Sitzung.
pub async fn bff_abmelden(client: &reqwest::Client, oberflaeche: &Url, wert: &str) -> Result<(), Fehler> {
    let antwort = client
        .post(route(oberflaeche, ABMELDEN_PFAD))
        .header(reqwest::header::COOKIE, format!("{SITZUNGS_COOKIE}={wert}"))
        .send()
        .await
        .map_err(|e| Fehler::Netz(format!("LeadLab-Server nicht erreichbar ({e}).")))?;
    if !antwort.status().is_success() {
        return Err(Fehler::Server(format!("Abmelden: Server antwortet mit {}.", antwort.status().as_u16())));
    }
    Ok(())
}

/// Abmelden auf beiden Seiten, in dieser Reihenfolge: BFF-Sitzung beenden
/// (braucht das Cookie), Cookies im Fenster löschen, dann die Anmeldung des
/// Clients (Widerruf beim Anmeldedienst, Tresor löschen). Ein Netzfehler am
/// BFF hält das Abmelden nicht auf — die Sitzung endet dann nach 5 min von
/// selbst. Ergebnis: ob Keycloak den Widerruf bestätigt hat.
pub async fn abmelden_beidseitig(
    client: &reqwest::Client,
    bff: Option<(&Url, &str)>,
    cookies_loeschen: impl FnOnce(),
    sitzung: &Sitzung,
    k: Option<&AnmeldeKonfig>,
) -> Result<bool, Fehler> {
    if let Some((oberflaeche, wert)) = bff {
        let _ = bff_abmelden(client, oberflaeche, wert).await;
    }
    cookies_loeschen();
    match k {
        Some(k) => sitzung.abmelden(client, k).await,
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anmeldung::Speicher;
    use crate::tresor::{Eintrag, Schluessel, Tresor};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn cookie_aus_der_antwort() {
        let c = cookie_aus_zeile("leadlab_sitzung=abc123; Domain=.lead.lab; Max-Age=86400; Path=/; HttpOnly; SameSite=Lax").unwrap();
        assert_eq!(c.value(), "abc123");
        assert_eq!(c.domain(), Some("lead.lab"));
        let alle = cookies_fuer(&c, &[&url("http://uebung.lead.lab"), &url("http://lead.lab")]);
        assert_eq!(alle.len(), 1, "ein Cookie für die ganze Domain");
        assert_eq!(alle[0].http_only(), Some(true));
        assert_eq!(alle[0].path(), Some("/"));
        assert_eq!(alle[0].same_site(), Some(SameSite::Lax));
        // Secure aus der Produktion bleibt erhalten.
        let s = cookie_aus_zeile("leadlab_sitzung=x; Max-Age=86400; Path=/; HttpOnly; SameSite=Lax; Secure").unwrap();
        assert_eq!(s.secure(), Some(true));
        // Andere Cookies, gelöschtes Cookie, Unsinn: nichts.
        assert!(cookie_aus_zeile("leadlab_anmeldung=x; Path=/api/auth").is_none());
        assert!(cookie_aus_zeile("leadlab_sitzung=; Max-Age=0").is_none());
        assert!(cookie_aus_zeile("kaputt").is_none());
    }

    #[test]
    fn ohne_domain_je_host_ein_cookie() {
        let c = cookie_aus_zeile("leadlab_sitzung=w; Max-Age=86400; Path=/; HttpOnly; SameSite=Lax").unwrap();
        let ue = url("https://einsatz.example.org/session/7");
        let st = url("https://ffw.example.org/");
        let alle = cookies_fuer(&c, &[&ue, &st, &ue]);
        let domains: Vec<_> = alle.iter().map(|c| c.domain().unwrap().to_string()).collect();
        assert_eq!(domains, ["einsatz.example.org", "ffw.example.org"]);
        assert!(alle.iter().all(|c| c.value() == "w" && c.http_only() == Some(true)));
    }

    #[test]
    fn login_umleitung_abfangen() {
        let ue = url("http://uebung.lead.lab/");
        let st = url("http://lead.lab/");
        let eigene = [&ue, &st];
        let ziel = |s: &str| login_ziel(&url(s), &eigene).map(|u| u.to_string());
        assert_eq!(
            ziel("http://lead.lab/api/auth/login?ziel=http%3A%2F%2Fuebung.lead.lab%2Fsession%2F7").as_deref(),
            Some("http://uebung.lead.lab/session/7")
        );
        assert_eq!(ziel("http://uebung.lead.lab/api/auth/login").as_deref(), Some("http://uebung.lead.lab/"));
        assert_eq!(ziel("http://uebung.lead.lab/api/auth/login?ziel=%2Flagen%3Fa%3D1").as_deref(), Some("http://uebung.lead.lab/lagen?a=1"));
        // Fremdes Ziel → Wurzel der Oberfläche, keine offene Umleitung.
        assert_eq!(ziel("http://lead.lab/api/auth/login?ziel=https%3A%2F%2Fevil.example%2F").as_deref(), Some("http://lead.lab/"));
        assert_eq!(ziel("http://lead.lab/api/auth/login?ziel=%2F%2Fevil.example").as_deref(), Some("http://lead.lab/"));
        // Nicht abfangen: anderer Pfad, fremder Host, Callback, Keycloak selbst.
        assert_eq!(ziel("http://uebung.lead.lab/lagen"), None);
        assert_eq!(ziel("http://evil.example/api/auth/login"), None);
        assert_eq!(ziel("http://uebung.lead.lab/api/auth/callback?code=x"), None);
        assert_eq!(ziel("http://auth.lead.lab/realms/leadlab/protocol/openid-connect/auth"), None);
        assert_eq!(ziel("https://uebung.lead.lab/api/auth/login"), None, "anderes Schema ist eine andere Herkunft");
    }

    #[test]
    fn rueckfall_adresse() {
        let u = client_login_url(&url("https://uebung.x.de/irgendwo?a=1"), "c0de", &url("https://uebung.x.de/session/7"));
        assert_eq!(u.as_str(), "https://uebung.x.de/api/auth/client-login?code=c0de&ziel=https%3A%2F%2Fuebung.x.de%2Fsession%2F7");
    }

    #[test]
    fn verlaengern_und_serverende() {
        assert_eq!(verlaengern_um(1_000_300, 1_000_000), 1_000_210);
        assert_eq!(verlaengern_um(1_000_100, 1_000_000), 1_000_030, "nie im Sekundentakt");
        let c = cookie_aus_zeile("leadlab_sitzung=a; Path=/").unwrap();
        let alt = BffSitzung { wert: "a".into(), cookie: c.clone(), laeuft_bis: 1_000_300, einmalcode: String::new() };
        let gleich = BffSitzung { laeuft_bis: 1_000_500, ..alt.clone() };
        let neu = BffSitzung { wert: "b".into(), ..gleich.clone() };
        assert!(!vom_server_beendet(&alt, &gleich, 1_000_200));
        assert!(vom_server_beendet(&alt, &neu, 1_000_200), "noch gültig, aber neues Cookie: abgemeldet");
        assert!(!vom_server_beendet(&alt, &neu, 1_000_400), "abgelaufen (Ruhezustand): neue Sitzung ist richtig");
    }

    // ── Mock: BFF und Anmeldedienst in einem Server ─────────────────────────

    #[derive(Default)]
    struct Protokoll(Mutex<Vec<String>>);

    async fn server(p: Arc<Protokoll>, status_sitzung: u16) -> Url {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let basis = url(&format!("http://{}/", l.local_addr().unwrap()));
        let iss = format!("{}realms/leadlab", basis);
        tokio::spawn(async move {
            loop {
                let (mut s, _) = l.accept().await.unwrap();
                let (p, iss) = (p.clone(), iss.clone());
                tokio::spawn(async move {
                    let (r, mut w) = s.split();
                    let mut r = BufReader::new(r);
                    let mut erste = String::new();
                    r.read_line(&mut erste).await.unwrap();
                    let (mut laenge, mut kopf) = (0usize, Vec::new());
                    loop {
                        let mut zeile = String::new();
                        r.read_line(&mut zeile).await.unwrap();
                        if zeile == "\r\n" || zeile.is_empty() {
                            break;
                        }
                        if let Some(v) = zeile.to_ascii_lowercase().strip_prefix("content-length:") {
                            laenge = v.trim().parse().unwrap();
                        }
                        kopf.push(zeile.trim().to_string());
                    }
                    let mut rumpf = vec![0u8; laenge];
                    r.read_exact(&mut rumpf).await.unwrap();
                    let cookie = kopf.iter().find(|z| z.to_ascii_lowercase().starts_with("cookie:")).cloned().unwrap_or_default();
                    let auth = kopf.iter().find(|z| z.to_ascii_lowercase().starts_with("authorization:")).cloned().unwrap_or_default();
                    let (status, extra, text) = if erste.contains("/.well-known/openid-configuration") {
                        ("200 OK", String::new(), format!(
                            r#"{{"issuer":"{iss}","authorization_endpoint":"{iss}/auth","token_endpoint":"{iss}/token","revocation_endpoint":"{iss}/revoke"}}"#
                        ))
                    } else if erste.contains("/revoke") {
                        p.0.lock().unwrap().push(format!("widerruf {}", String::from_utf8_lossy(&rumpf)));
                        ("200 OK", String::new(), String::new())
                    } else if erste.starts_with("POST /api/auth/abmelden") {
                        p.0.lock().unwrap().push(format!("bff-abmelden {cookie}"));
                        ("204 No Content", String::new(), String::new())
                    } else if erste.starts_with("POST /api/auth/client-sitzung") {
                        p.0.lock().unwrap().push(format!("sitzung {auth} {cookie}"));
                        if status_sitzung != 200 {
                            ("401 Unauthorized", String::new(), r#"{"detail":"Token ungültig"}"#.to_string())
                        } else {
                            let wert = if cookie.contains("alt-wert") { "alt-wert" } else { "neu-wert" };
                            ("200 OK",
                             format!("Set-Cookie: leadlab_sitzung={wert}; Domain=.lead.lab; Max-Age=86400; Path=/; HttpOnly; SameSite=Lax\r\n"),
                             r#"{"benutzer":{"name":"Anna"},"laeuft_bis":1800000300.5,"einmalcode":"ein-code"}"#.to_string())
                        }
                    } else {
                        ("404 Not Found", String::new(), String::new())
                    };
                    let a = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
                    w.write_all(a.as_bytes()).await.unwrap();
                });
            }
        });
        basis
    }

    #[tokio::test]
    async fn sitzung_aus_dem_client_token() {
        let p = Arc::new(Protokoll::default());
        let basis = server(p.clone(), 200).await;
        let client = reqwest::Client::new();
        let mut ob = basis.clone();
        ob.set_path("/session/7");
        let s = sitzung_holen(&client, &ob, "client-token", None).await.unwrap();
        assert_eq!((s.wert.as_str(), s.laeuft_bis, s.einmalcode.as_str()), ("neu-wert", 1_800_000_300, "ein-code"));
        assert_eq!(s.cookie.domain(), Some("lead.lab"));
        // Verlängern schickt das alte Cookie mit.
        let v = sitzung_holen(&client, &ob, "token-2", Some("alt-wert")).await.unwrap();
        assert_eq!(v.wert, "alt-wert");
        let log = p.0.lock().unwrap().clone();
        assert!(log[0].contains("Bearer client-token") && !log[0].contains("leadlab_sitzung"), "{log:?}");
        assert!(log[1].contains("Bearer token-2") && log[1].contains("leadlab_sitzung=alt-wert"), "{log:?}");
    }

    #[tokio::test]
    async fn abgelehnte_sitzung_meldet_sich() {
        let p = Arc::new(Protokoll::default());
        let basis = server(p, 401).await;
        let e = sitzung_holen(&reqwest::Client::new(), &basis, "t", None).await.unwrap_err();
        assert!(e.to_string().contains("Token ungültig"), "{e}");
    }

    #[tokio::test]
    async fn abmelden_in_der_richtigen_reihenfolge() {
        let p = Arc::new(Protokoll::default());
        let basis = server(p.clone(), 200).await;
        let d = tempfile::tempdir().unwrap();
        let issuer = basis.join("realms/leadlab").unwrap();
        let k = AnmeldeKonfig { issuer: issuer.clone(), client_id: "leadlab-client".into(), scopes: vec![], formular: true };
        let tresor = Tresor::neu(d.path(), Schluessel::Fest([5; 32]));
        tresor
            .schreiben(&Eintrag {
                issuer: issuer.as_str().trim_end_matches('/').into(),
                client_id: "leadlab-client".into(),
                refresh_token: "offline-r".into(),
                ..Default::default()
            })
            .unwrap();
        let sitzung = Sitzung::neu(Tresor::neu(d.path(), Schluessel::Fest([5; 32])), Arc::new(Speicher::default()));
        assert!(sitzung.status(&k).angemeldet);
        let pp = p.clone();
        let widerrufen = abmelden_beidseitig(
            &reqwest::Client::new(),
            Some((&basis, "w-123")),
            move || pp.0.lock().unwrap().push("cookies-geloescht".into()),
            &sitzung,
            Some(&k),
        )
        .await
        .unwrap();
        assert!(widerrufen);
        let log = p.0.lock().unwrap().clone();
        assert_eq!(log.len(), 3, "{log:?}");
        assert!(log[0].starts_with("bff-abmelden") && log[0].contains("leadlab_sitzung=w-123"), "{log:?}");
        assert_eq!(log[1], "cookies-geloescht");
        assert!(log[2].starts_with("widerruf") && log[2].contains("token=offline-r"), "{log:?}");
        assert!(!sitzung.status(&k).angemeldet);
    }

    #[tokio::test]
    async fn abmelden_ohne_server_loescht_trotzdem() {
        let d = tempfile::tempdir().unwrap();
        let sitzung = Sitzung::neu(Tresor::neu(d.path(), Schluessel::Fest([5; 32])), Arc::new(Speicher::default()));
        let geloescht = Arc::new(Mutex::new(false));
        let g = geloescht.clone();
        let tot = url("http://127.0.0.1:9/");
        let w = abmelden_beidseitig(&reqwest::Client::new(), Some((&tot, "w")), move || *g.lock().unwrap() = true, &sitzung, None)
            .await
            .unwrap();
        assert!(!w && *geloescht.lock().unwrap());
    }
}
