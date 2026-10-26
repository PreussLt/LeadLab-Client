//! Updates des Clients (Konzept §8.4, CS9 Hauptrepo #764).
//!
//! Die Adresse des Update-Manifests nennt die Konfigurationsadresse
//! (`client_update`, z. B. `https://<domain>/daten/client/update.json`, von
//! nginx ohne Anmeldung ausgeliefert). Das Manifest ist das `latest.json` aus
//! der Release-CI (tauri-action), mit Download-Adressen auf dem eigenen Server
//! (`werkzeug/update-manifest.mjs`). Jedes Paket ist mit dem Updater-Schlüssel
//! des Client-Repos signiert; den öffentlichen Teil trägt `tauri.conf.json`
//! (`plugins.updater.pubkey`). Steht dort noch der Platzhalter, prüft der
//! Client nicht — ein Update ohne geprüfte Signatur gibt es nicht.

use serde::Serialize;
use tauri::{AppHandle, Runtime};
use tauri_plugin_updater::UpdaterExt;
use url::Url;

use crate::fehler::Fehler;

pub const PLATZHALTER: &str = "PLATZHALTER";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct UpdateInfo {
    pub verfuegbar: bool,
    pub version: Option<String>,
    pub notizen: Option<String>,
    /// Warum nicht geprüft wurde (Platzhalter-Schlüssel, keine Adresse).
    pub hinweis: Option<String>,
}

impl UpdateInfo {
    pub fn keins(hinweis: Option<&str>) -> Self {
        UpdateInfo { verfuegbar: false, version: None, notizen: None, hinweis: hinweis.map(str::to_string) }
    }
}

/// Ist der Updater-Schlüssel eingetragen? (Sonst Platzhalter aus dem Repo.)
pub fn schluessel_eingetragen(pubkey: Option<&str>) -> bool {
    matches!(pubkey, Some(k) if !k.trim().is_empty() && !k.contains(PLATZHALTER))
}

/// Darf diese Manifest-Adresse genommen werden? Gleiche Regel wie für die
/// Konfigurationsadresse: https, http nur im LAN.
pub fn adresse_pruefen(url: &Url) -> Result<(), Fehler> {
    crate::konfig::pruefe_schema(url)
}

fn pubkey<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("pubkey"))
        .and_then(|p| p.as_str())
        .map(str::to_string)
}

/// Fragt das Manifest ab. `None` als Adresse oder Platzhalter-Schlüssel →
/// „kein Update“ mit Hinweis, kein Fehler.
pub async fn pruefen<R: Runtime>(app: &AppHandle<R>, adresse: Option<&Url>) -> Result<(UpdateInfo, Option<tauri_plugin_updater::Update>), Fehler> {
    let Some(adresse) = adresse else {
        return Ok((UpdateInfo::keins(Some("Der Server nennt keine Update-Adresse.")), None));
    };
    if !schluessel_eingetragen(pubkey(app).as_deref()) {
        return Ok((UpdateInfo::keins(Some("Dieser Client ist ohne Update-Schlüssel gebaut.")), None));
    }
    adresse_pruefen(adresse)?;
    let updater = app
        .updater_builder()
        .endpoints(vec![adresse.clone()])
        .map_err(|e| Fehler::Eingabe(e.to_string()))?
        .build()
        .map_err(|e| Fehler::Eingabe(e.to_string()))?;
    match updater.check().await {
        Ok(Some(u)) => Ok((
            UpdateInfo { verfuegbar: true, version: Some(u.version.clone()), notizen: u.body.clone(), hinweis: None },
            Some(u),
        )),
        Ok(None) => Ok((UpdateInfo::keins(None), None)),
        Err(e) => Err(Fehler::Netz(format!("Update-Prüfung fehlgeschlagen: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platzhalter_ist_kein_schluessel() {
        let aus_dem_repo: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let k = aus_dem_repo["plugins"]["updater"]["pubkey"].as_str();
        assert!(!schluessel_eingetragen(k), "im Repo steht der Platzhalter, kein echter Schlüssel");
        assert!(!schluessel_eingetragen(None));
        assert!(!schluessel_eingetragen(Some("  ")));
        assert!(schluessel_eingetragen(Some("dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEFCQ0QK")));
    }

    #[test]
    fn manifest_nur_https_oder_lan() {
        assert!(adresse_pruefen(&Url::parse("https://leadlab.example.de/daten/client/update.json").unwrap()).is_ok());
        assert!(adresse_pruefen(&Url::parse("http://lead.lab/daten/client/update.json").unwrap()).is_ok());
        assert!(adresse_pruefen(&Url::parse("http://leadlab.example.de/daten/client/update.json").unwrap()).is_err());
    }

    #[test]
    fn erstellen_ohne_update_artefakte_im_repo() {
        // Lokale Bauten brauchen keinen privaten Schlüssel; die Release-CI
        // schaltet die Update-Artefakte per --config ein.
        let c: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(c["bundle"]["createUpdaterArtifacts"], serde_json::json!(false));
    }
}
