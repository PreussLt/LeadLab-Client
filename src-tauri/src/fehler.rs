//! Ein Fehlertyp für alle Befehle. Er wird als Text an die Oberfläche
//! gereicht, die ihn ohne Umformulierung anzeigt; die Texte sind deshalb
//! Sätze für Menschen, keine Codes.

use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum Fehler {
    #[error("{0}")]
    Eingabe(String),
    #[error("{0}")]
    Netz(String),
    #[error("{0}")]
    Server(String),
    #[error("{0}")]
    Pruefsumme(String),
    #[error("Abgebrochen.")]
    Abgebrochen,
    #[error("Dateifehler: {0}")]
    Datei(#[from] std::io::Error),
}

impl Serialize for Fehler {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}
