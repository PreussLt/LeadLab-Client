//! Konfigurationsadresse auflösen (Konzept §5).
//!
//! Der Nutzer gibt eine einzige Adresse ein. Endet sie auf `.json`, ist sie
//! die Konfigurationsdatei selbst, sonst liegt die Datei unter
//! `<adresse>/.well-known/leadlab-client.json`. Ohne Schema wird `https://`
//! ergänzt; `http://` gilt nur für Loopback, private Adressbereiche und Namen,
//! die es im Internet nicht gibt (`lead.lab`, `.lan`, `.local` …, siehe
//! [`LAN_ENDUNGEN`]) — Tests im LAN, Feuerwehrhaus.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::fehler::Fehler;

pub const SCHEMA: &str = "leadlab-client/1";
pub const WELL_KNOWN: &str = ".well-known/leadlab-client.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Anmeldung {
    pub issuer: Url,
    pub client_id: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Anmeldemaske Benutzername/Passwort im Client (#804). Fehlt das Feld,
    /// gilt `true`; ein Betreiber schaltet sie mit `false` ab, dann bleibt nur
    /// der Browser-Weg.
    #[serde(default = "ja")]
    pub formular: bool,
}

fn ja() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Zugang {
    Anmeldung,
    Offen,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Daten {
    pub katalog: Url,
    pub zugang: Zugang,
    #[serde(default)]
    pub signatur_schluessel: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Marke {
    #[serde(default)]
    pub landkreis: Option<String>,
    #[serde(default)]
    pub logo: Option<Url>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClientKonfig {
    pub schema: String,
    pub name: String,
    #[serde(default)]
    pub server_version: Option<String>,
    #[serde(default)]
    pub min_client_version: Option<String>,
    pub oberflaeche: Url,
    #[serde(default)]
    pub startzentrale: Option<Url>,
    #[serde(default)]
    pub anmeldung: Option<Anmeldung>,
    pub daten: Daten,
    #[serde(default)]
    pub client_update: Option<Url>,
    #[serde(default)]
    pub marke: Marke,
}

/// Namen, die im Internet nicht vergeben werden (#793): Wer sie benutzt, ist
/// im eigenen Netz (eigener DNS, /etc/hosts, mDNS) — wie bei lead.lab und den
/// privaten Adressen geht dort http. Eine Domain gilt, wenn sie gleich einer
/// Endung ist oder darauf endet (`start.ffw.lan`).
///
/// - `lead.lab`: Vorgabe des Feuerwehrhauses (LEADLAB_DOMAIN)
/// - `localhost`, `localdomain`: Loopback bzw. Vorgabe vieler Router
/// - `lan`, `home`, `corp`: üblich in Heim- und Firmennetzen, von der ICANN
///   nicht vergeben (home/corp wegen Namenskollisionen dauerhaft gesperrt)
/// - `local` (mDNS, RFC 6762), `home.arpa` (RFC 8375), `internal` (ICANN 2024
///   für private Netze reserviert)
///
/// Einzelne Namen ohne Punkt (`http://leadlab`) gelten nicht: Die
/// Suchdomäne des Rechners kann daraus einen öffentlichen Namen machen.
/// Spiegel: `werkzeug/update-manifest.mjs` (`LAN_ENDUNGEN`).
pub const LAN_ENDUNGEN: &[&str] =
    &["lead.lab", "localhost", "localdomain", "lan", "home", "corp", "local", "home.arpa", "internal"];

/// Liegt der Name unter einer der [`LAN_ENDUNGEN`]?
pub fn lan_name(domain: &str) -> bool {
    let d = domain.trim_end_matches('.').to_ascii_lowercase();
    LAN_ENDUNGEN.iter().any(|e| d == *e || d.strip_suffix(e).is_some_and(|r| r.ends_with('.')))
}

/// Darf diese Adresse ohne TLS angesprochen werden?
pub fn http_erlaubt(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => lan_name(d),
        Some(url::Host::Ipv4(ip)) => {
            let ip = IpAddr::V4(ip);
            ip.is_loopback() || matches!(ip, IpAddr::V4(v4) if v4.is_private() || v4.is_link_local())
        }
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Prüft das Schema einer Adresse (https Pflicht außer in den Ausnahmen).
pub fn pruefe_schema(url: &Url) -> Result<(), Fehler> {
    match url.scheme() {
        "https" => Ok(()),
        "http" if http_erlaubt(url) => Ok(()),
        "http" => Err(Fehler::Eingabe(format!(
            "{} ist unverschlüsselt. http:// geht nur im eigenen Netz: private Adressen und Namen \
             unter lead.lab, .lan, .local, .internal, .home.arpa (und localhost). Sonst bitte https.",
            url
        ))),
        s => Err(Fehler::Eingabe(format!("Schema „{s}“ wird nicht unterstützt."))),
    }
}

/// Aus der Nutzereingabe die Adresse der Konfigurationsdatei machen.
pub fn konfig_url(eingabe: &str) -> Result<Url, Fehler> {
    let roh = eingabe.trim();
    if roh.is_empty() {
        return Err(Fehler::Eingabe("Bitte eine Serveradresse eingeben.".into()));
    }
    let mit_schema = if roh.contains("://") { roh.to_string() } else { format!("https://{roh}") };
    let mut url = Url::parse(&mit_schema)
        .map_err(|e| Fehler::Eingabe(format!("„{roh}“ ist keine gültige Adresse ({e}).")))?;
    if url.host().is_none() {
        return Err(Fehler::Eingabe(format!("„{roh}“ enthält keinen Servernamen.")));
    }
    // Ohne Schema eingegeben und LAN-Name: dort läuft meist kein TLS.
    if !roh.contains("://") && http_erlaubt(&url) {
        url.set_scheme("http").ok();
    }
    pruefe_schema(&url)?;
    url.set_fragment(None);
    if url.path().ends_with(".json") {
        return Ok(url);
    }
    if !url.path().ends_with('/') {
        let p = format!("{}/", url.path());
        url.set_path(&p);
    }
    url.set_query(None);
    url.join(WELL_KNOWN).map_err(|e| Fehler::Eingabe(e.to_string()))
}

/// Inhalt prüfen: Schema-Kennung und alle Folgeadressen.
pub fn pruefe(konfig: &ClientKonfig) -> Result<(), Fehler> {
    if konfig.schema != SCHEMA {
        return Err(Fehler::Server(format!(
            "Der Server meldet Schema „{}“, dieser Client versteht „{SCHEMA}“.",
            konfig.schema
        )));
    }
    pruefe_schema(&konfig.oberflaeche)?;
    pruefe_schema(&konfig.daten.katalog)?;
    if konfig.daten.zugang == Zugang::Anmeldung && konfig.anmeldung.is_none() {
        return Err(Fehler::Server("Der Server verlangt eine Anmeldung, nennt aber keinen Anmeldedienst.".into()));
    }
    if let Some(min) = &konfig.min_client_version {
        if version_kleiner(env!("CARGO_PKG_VERSION"), min) {
            return Err(Fehler::Server(format!(
                "Dieser Server braucht mindestens Client-Version {min}, installiert ist {}.",
                env!("CARGO_PKG_VERSION")
            )));
        }
    }
    Ok(())
}

/// Vergleich „a < b“ für Versionen der Form 1.2.3 (fehlende Teile = 0).
pub fn version_kleiner(a: &str, b: &str) -> bool {
    let teile = |v: &str| -> Vec<u64> {
        v.split(['.', '-']).take(3).map(|t| t.parse().unwrap_or(0)).collect()
    };
    let (mut x, mut y) = (teile(a), teile(b));
    x.resize(3, 0);
    y.resize(3, 0);
    x < y
}

pub async fn lade(client: &reqwest::Client, eingabe: &str) -> Result<ClientKonfig, Fehler> {
    let url = konfig_url(eingabe)?;
    let antwort = client.get(url.clone()).send().await.map_err(|e| Fehler::Netz(format!("{url}: {e}")))?;
    if !antwort.status().is_success() {
        return Err(Fehler::Server(format!(
            "{url} antwortet mit {}. Ist das ein LeadLab-Server?",
            antwort.status()
        )));
    }
    let konfig: ClientKonfig = antwort
        .json()
        .await
        .map_err(|e| Fehler::Server(format!("{url} liefert keine gültige Client-Konfiguration ({e}).")))?;
    pruefe(&konfig)?;
    Ok(konfig)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_ohne_schema_wird_https_und_well_known() {
        let u = konfig_url("leadlab.example.de").unwrap();
        assert_eq!(u.as_str(), "https://leadlab.example.de/.well-known/leadlab-client.json");
    }

    #[test]
    fn unterpfad_bleibt_erhalten() {
        let u = konfig_url("https://example.de/leadlab").unwrap();
        assert_eq!(u.as_str(), "https://example.de/leadlab/.well-known/leadlab-client.json");
    }

    #[test]
    fn json_adresse_wird_direkt_genommen() {
        let u = konfig_url("https://example.de/konfig/client.json").unwrap();
        assert_eq!(u.as_str(), "https://example.de/konfig/client.json");
    }

    #[test]
    fn lan_namen_ohne_schema_werden_http() {
        assert_eq!(konfig_url("lead.lab").unwrap().scheme(), "http");
        assert_eq!(konfig_url("192.168.1.20:8080").unwrap().scheme(), "http");
        assert_eq!(konfig_url("localhost:11214").unwrap().scheme(), "http");
    }

    #[test]
    fn http_im_internet_wird_abgelehnt() {
        assert!(konfig_url("http://leadlab.example.de").is_err());
        assert!(konfig_url("http://8.8.8.8").is_err());
        assert!(konfig_url("ftp://example.de").is_err());
        assert!(konfig_url("   ").is_err());
    }

    /// Eigene Feuerwehrhaus-Namen ohne TLS (#793): Endungen, die es im
    /// Internet nicht gibt, gelten wie lead.lab.
    #[test]
    fn eigene_lan_namen_duerfen_http() {
        for a in [
            "http://start.ffw.lan",
            "http://ffw.lan:8080/",
            "http://uebung.ffw.lan/",
            "http://auth.ffw.lan/realms/leadlab",
            "http://leadlab.local",
            "http://leadlab.internal",
            "http://leadlab.ffw.home.arpa",
            "http://leadlab.localdomain",
            "http://leadlab.home",
            "http://leadlab.corp",
            "http://START.FFW.LAN",
            "http://ffw.lan./",
            "http://lead.lab",
            "http://uebung.lead.lab",
        ] {
            let u = Url::parse(a).unwrap();
            assert!(http_erlaubt(&u), "{a}");
            assert!(pruefe_schema(&u).is_ok(), "{a}");
        }
        // Ohne Schema eingegeben: dort läuft meist kein TLS.
        assert_eq!(konfig_url("start.ffw.lan").unwrap().as_str(), "http://start.ffw.lan/.well-known/leadlab-client.json");
    }

    #[test]
    fn oeffentliche_namen_bleiben_https() {
        for a in [
            "http://lan.example.de",
            "http://ffw-lan.de",
            "http://leadlab.local.example.de",
            "http://lanx",
            "http://leadlab",
            "http://evillead.lab",
            "http://internal.com",
        ] {
            let u = Url::parse(a).unwrap();
            assert!(!http_erlaubt(&u), "{a}");
        }
        let f = pruefe_schema(&Url::parse("http://lan.example.de").unwrap()).unwrap_err().to_string();
        assert!(f.contains(".lan"), "Fehlertext nennt die erlaubten Namen: {f}");
    }

    /// Das Update-Manifest prüft seine Basis mit derselben Liste.
    #[test]
    fn lan_endungen_wie_im_update_manifest() {
        let js = include_str!("../../werkzeug/update-manifest.mjs");
        let zeile = js.lines().find(|l| l.starts_with("export const LAN_ENDUNGEN")).expect("LAN_ENDUNGEN im Manifest-Werkzeug");
        let liste: Vec<&str> = zeile
            .split_once('[')
            .and_then(|(_, r)| r.split_once(']'))
            .map(|(l, _)| l.split(',').map(|e| e.trim().trim_matches('\'')).collect())
            .unwrap();
        assert_eq!(liste, LAN_ENDUNGEN);
    }

    #[test]
    fn versionen_vergleichen() {
        assert!(version_kleiner("0.1.0", "0.2.0"));
        assert!(version_kleiner("0.9.9", "1"));
        assert!(!version_kleiner("1.0.0", "1.0.0"));
        assert!(!version_kleiner("1.10.0", "1.9.0"));
    }

    fn beispiel() -> ClientKonfig {
        serde_json::from_str(include_str!("../../schema/beispiel-leadlab-client.json")).unwrap()
    }

    #[test]
    fn beispiel_ist_gueltig() {
        pruefe(&beispiel()).unwrap();
    }

    #[test]
    fn falsches_schema_wird_abgelehnt() {
        let mut k = beispiel();
        k.schema = "leadlab-client/9".into();
        assert!(pruefe(&k).is_err());
    }

    #[test]
    fn formular_ist_vorgabe_und_abschaltbar() {
        assert!(beispiel().anmeldung.unwrap().formular);
        let ohne: Anmeldung = serde_json::from_str(r#"{"issuer":"https://a.de/realms/x","client_id":"c"}"#).unwrap();
        assert!(ohne.formular, "ältere Server ohne das Feld: Formular an");
        let aus: Anmeldung =
            serde_json::from_str(r#"{"issuer":"https://a.de/realms/x","client_id":"c","formular":false}"#).unwrap();
        assert!(!aus.formular);
    }

    #[test]
    fn anmeldung_ohne_anmeldedienst_wird_abgelehnt() {
        let mut k = beispiel();
        k.anmeldung = None;
        assert!(pruefe(&k).is_err());
        k.daten.zugang = Zugang::Offen;
        assert!(pruefe(&k).is_ok());
    }
}
