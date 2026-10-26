//! Einstellungen des Clients (eine JSON-Datei im Konfigurationsordner des
//! Benutzers). Geheimnisse stehen hier nie — das Refresh-Token der
//! Anmeldung liegt im Schlüsselbund des Betriebssystems (anmeldung.rs).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fehler::Fehler;
use crate::konfig::ClientKonfig;

pub const DATEI: &str = "einstellungen.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Einstellungen {
    /// Was der Nutzer als Konfigurationsadresse eingegeben hat.
    #[serde(default)]
    pub konfig_adresse: Option<String>,
    /// Zuletzt gelesene Konfiguration — erlaubt den Start, wenn der Server
    /// beim Öffnen kurz nicht antwortet.
    #[serde(default)]
    pub konfig: Option<ClientKonfig>,
    #[serde(default)]
    pub datenordner: Option<PathBuf>,
    /// Assistent einmal vollständig durchlaufen.
    #[serde(default)]
    pub eingerichtet: bool,
}

impl Einstellungen {
    pub fn lesen(ordner: &Path) -> Self {
        std::fs::read(ordner.join(DATEI))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn schreiben(&self, ordner: &Path) -> Result<(), Fehler> {
        std::fs::create_dir_all(ordner)?;
        let neu = ordner.join(format!("{DATEI}.neu"));
        std::fs::write(&neu, serde_json::to_vec_pretty(self).expect("serialisierbar"))?;
        std::fs::rename(neu, ordner.join(DATEI))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rundreise_und_kaputte_datei() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(Einstellungen::lesen(d.path()), Einstellungen::default());
        let e = Einstellungen {
            konfig_adresse: Some("lead.lab".into()),
            datenordner: Some(d.path().join("daten")),
            eingerichtet: true,
            ..Default::default()
        };
        e.schreiben(d.path()).unwrap();
        assert_eq!(Einstellungen::lesen(d.path()), e);
        std::fs::write(d.path().join(DATEI), b"{kaputt").unwrap();
        assert_eq!(Einstellungen::lesen(d.path()), Einstellungen::default());
    }
}
