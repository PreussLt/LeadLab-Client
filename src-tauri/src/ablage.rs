//! Datenordner des Clients.
//!
//! Aufbau: die Paketdateien unter ihrem Katalog-`pfad`, laufende Downloads als
//! `<pfad>.teil`, dazu `zustand.json` mit dem Bestand (`pfad → sha256`). Der
//! Zustand wird erst nach erfolgreicher Prüfung einer Datei geschrieben —
//! eine Datei ohne Eintrag gilt als nicht vorhanden und wird neu geladen.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fehler::Fehler;
use crate::katalog::Auswahl;

pub const ZUSTAND: &str = "zustand.json";
pub const TEIL: &str = ".teil";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Zustand {
    #[serde(default)]
    pub auswahl: Auswahl,
    #[serde(default)]
    pub dateien: BTreeMap<String, String>,
    #[serde(default)]
    pub katalog_erzeugt: Option<String>,
}

/// Katalogpfade sind relativ, ohne `..`, ohne Laufwerk und ohne Backslash.
/// Ein Katalog darf so nie außerhalb des Datenordners schreiben.
pub fn sicherer_pfad(pfad: &str) -> Result<PathBuf, Fehler> {
    let fehler = || Fehler::Server(format!("Unzulässiger Dateipfad im Katalog: {pfad}"));
    if pfad.is_empty() || pfad.contains('\\') || pfad.contains(':') || pfad.ends_with(TEIL) || pfad == ZUSTAND {
        return Err(fehler());
    }
    let p = Path::new(pfad);
    let mut aus = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => aus.push(s),
            _ => return Err(fehler()),
        }
    }
    Ok(aus)
}

pub struct Ablage {
    pub wurzel: PathBuf,
}

impl Ablage {
    pub fn neu(wurzel: impl Into<PathBuf>) -> Self {
        Self { wurzel: wurzel.into() }
    }

    pub fn ziel(&self, pfad: &str) -> Result<PathBuf, Fehler> {
        Ok(self.wurzel.join(sicherer_pfad(pfad)?))
    }

    pub fn teil(&self, pfad: &str) -> Result<PathBuf, Fehler> {
        let mut z = self.ziel(pfad)?.into_os_string();
        z.push(TEIL);
        Ok(z.into())
    }

    pub fn zustand_lesen(&self) -> Zustand {
        std::fs::read(self.wurzel.join(ZUSTAND))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Atomar schreiben: erst Nachbardatei, dann umbenennen.
    pub fn zustand_schreiben(&self, z: &Zustand) -> Result<(), Fehler> {
        std::fs::create_dir_all(&self.wurzel)?;
        let neu = self.wurzel.join(format!("{ZUSTAND}.neu"));
        std::fs::write(&neu, serde_json::to_vec_pretty(z).expect("Zustand serialisierbar"))?;
        std::fs::rename(neu, self.wurzel.join(ZUSTAND))?;
        Ok(())
    }

    /// Bestand ohne Einträge, deren Datei nicht mehr auf der Platte liegt.
    pub fn vorhanden(&self) -> BTreeMap<String, String> {
        self.zustand_lesen()
            .dateien
            .into_iter()
            .filter(|(p, _)| self.ziel(p).map(|z| z.is_file()).unwrap_or(false))
            .collect()
    }

    pub fn loeschen(&self, pfad: &str) -> Result<(), Fehler> {
        let z = self.ziel(pfad)?;
        if z.exists() {
            std::fs::remove_file(&z)?;
        }
        let _ = std::fs::remove_file(self.teil(pfad)?);
        // leere Elternordner bis zur Wurzel wegräumen
        let mut eltern = z.parent();
        while let Some(e) = eltern {
            if e == self.wurzel || std::fs::remove_dir(e).is_err() {
                break;
            }
            eltern = e.parent();
        }
        Ok(())
    }
}

/// Freier Speicher am Ort (oder am nächsten existierenden Elternordner).
pub fn freier_speicher(ort: &Path) -> Option<u64> {
    let mut p = Some(ort);
    while let Some(x) = p {
        if x.exists() {
            return fs2::available_space(x).ok();
        }
        p = x.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pfade_bleiben_im_datenordner() {
        assert!(sicherer_pfad("karte/osm/a.pmtiles").is_ok());
        for boese in ["../a", "/etc/passwd", "a/../../b", "C:/x", "a\\b", "", "zustand.json", "a.teil", "./a"] {
            assert!(sicherer_pfad(boese).is_err(), "{boese} müsste abgelehnt werden");
        }
    }

    #[test]
    fn zustand_rundreise_und_vorhanden_prueft_platte() {
        let d = tempfile::tempdir().unwrap();
        let a = Ablage::neu(d.path());
        std::fs::create_dir_all(d.path().join("karte")).unwrap();
        std::fs::write(d.path().join("karte/a.pmtiles"), b"x").unwrap();
        let mut z = Zustand::default();
        z.dateien.insert("karte/a.pmtiles".into(), "a".repeat(64));
        z.dateien.insert("karte/weg.pmtiles".into(), "b".repeat(64));
        a.zustand_schreiben(&z).unwrap();
        assert_eq!(a.zustand_lesen(), z);
        let v = a.vorhanden();
        assert_eq!(v.len(), 1);
        assert!(v.contains_key("karte/a.pmtiles"));
    }

    #[test]
    fn loeschen_raeumt_leere_ordner() {
        let d = tempfile::tempdir().unwrap();
        let a = Ablage::neu(d.path());
        std::fs::create_dir_all(d.path().join("x/y")).unwrap();
        std::fs::write(d.path().join("x/y/z.bin"), b"1").unwrap();
        a.loeschen("x/y/z.bin").unwrap();
        assert!(!d.path().join("x").exists());
        assert!(d.path().exists());
    }
}
