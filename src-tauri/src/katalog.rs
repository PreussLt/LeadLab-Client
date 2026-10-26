//! Datenkatalog des Servers (Konzept §6.2) und der Abgleich mit der Ablage.
//!
//! Der Katalog nennt je Paket seine Dateien mit Name im Datenordner (`pfad`)
//! und Inhalts-Hash (`sha256`, zugleich der Name auf dem Server). Der Plan
//! vergleicht die Auswahl mit dem, was schon auf der Platte liegt: geladen
//! wird nur, was fehlt oder sich geändert hat; gelöscht nur, was kein
//! gewähltes Paket mehr nennt.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::fehler::Fehler;

pub const SCHEMA: &str = "leadlab-daten/1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Region {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub bbox: Option<[f64; 4]>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Datei {
    pub pfad: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Paket {
    pub id: String,
    pub gruppe: String,
    pub titel: String,
    pub region: String,
    pub version: String,
    #[serde(default)]
    pub pflicht: bool,
    #[serde(default)]
    pub lizenz: Option<String>,
    pub dateien: Vec<Datei>,
    #[serde(default)]
    pub braucht: Vec<String>,
}

impl Paket {
    pub fn bytes(&self) -> u64 {
        self.dateien.iter().map(|d| d.bytes).sum()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Katalog {
    pub schema: String,
    #[serde(default)]
    pub erzeugt: Option<String>,
    pub basis: Vec<Url>,
    #[serde(default)]
    pub regionen: Vec<Region>,
    pub pakete: Vec<Paket>,
}

/// Was der Nutzer gewählt hat: Regionen (bringen ihre Pflichtpakete mit)
/// und einzelne Pakete.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Auswahl {
    #[serde(default)]
    pub regionen: Vec<String>,
    #[serde(default)]
    pub pakete: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct Plan {
    /// Aufgelöste Paket-Ids (inkl. Pflicht und Abhängigkeiten), sortiert.
    pub pakete: Vec<String>,
    /// Zu ladende Dateien.
    pub laden: Vec<Datei>,
    /// Pfade im Datenordner, die weg können.
    pub loeschen: Vec<String>,
    pub bytes_gesamt: u64,
    pub bytes_zu_laden: u64,
    /// Gewählte Ids, die der Katalog nicht (mehr) kennt.
    pub unbekannt: Vec<String>,
}

pub fn pruefe(k: &Katalog) -> Result<(), Fehler> {
    if k.schema != SCHEMA {
        return Err(Fehler::Server(format!(
            "Der Datenkatalog hat Schema „{}“, dieser Client versteht „{SCHEMA}“.",
            k.schema
        )));
    }
    if k.basis.is_empty() {
        return Err(Fehler::Server("Der Datenkatalog nennt keine Download-Adresse.".into()));
    }
    let mut pfade = BTreeMap::new();
    for p in &k.pakete {
        for d in &p.dateien {
            if !sha256_gueltig(&d.sha256) {
                return Err(Fehler::Server(format!("Paket {}: ungültige Prüfsumme bei {}.", p.id, d.pfad)));
            }
            crate::ablage::sicherer_pfad(&d.pfad)?;
            // Zwei Pakete dürfen denselben Pfad nur mit demselben Inhalt nennen.
            if let Some(alt) = pfade.insert(d.pfad.clone(), d.sha256.clone()) {
                if alt != d.sha256 {
                    return Err(Fehler::Server(format!("Pfad {} kommt im Katalog mit zwei Inhalten vor.", d.pfad)));
                }
            }
        }
    }
    Ok(())
}

pub fn sha256_gueltig(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Auswahl auflösen: Pflichtpakete der Regionen, gewählte Pakete und deren
/// Abhängigkeiten (transitiv).
pub fn aufloesen(k: &Katalog, auswahl: &Auswahl) -> (BTreeSet<String>, Vec<String>) {
    let nach_id: BTreeMap<&str, &Paket> = k.pakete.iter().map(|p| (p.id.as_str(), p)).collect();
    let mut offen: Vec<String> = k
        .pakete
        .iter()
        .filter(|p| p.pflicht && auswahl.regionen.contains(&p.region))
        .map(|p| p.id.clone())
        .collect();
    offen.extend(auswahl.pakete.iter().cloned());
    let mut fertig = BTreeSet::new();
    let mut unbekannt = Vec::new();
    while let Some(id) = offen.pop() {
        if fertig.contains(&id) {
            continue;
        }
        match nach_id.get(id.as_str()) {
            Some(p) => {
                offen.extend(p.braucht.iter().cloned());
                fertig.insert(id);
            }
            None => {
                if !unbekannt.contains(&id) {
                    unbekannt.push(id);
                }
            }
        }
    }
    unbekannt.sort();
    (fertig, unbekannt)
}

/// Plan gegen den Bestand der Ablage (`pfad → sha256`).
pub fn plan(k: &Katalog, auswahl: &Auswahl, vorhanden: &BTreeMap<String, String>) -> Plan {
    let (ids, unbekannt) = aufloesen(k, auswahl);
    let mut soll: BTreeMap<String, Datei> = BTreeMap::new();
    for p in k.pakete.iter().filter(|p| ids.contains(&p.id)) {
        for d in &p.dateien {
            soll.insert(d.pfad.clone(), d.clone());
        }
    }
    let laden: Vec<Datei> = soll
        .values()
        .filter(|d| vorhanden.get(&d.pfad) != Some(&d.sha256))
        .cloned()
        .collect();
    let loeschen = vorhanden.keys().filter(|p| !soll.contains_key(*p)).cloned().collect();
    Plan {
        pakete: ids.into_iter().collect(),
        bytes_gesamt: soll.values().map(|d| d.bytes).sum(),
        bytes_zu_laden: laden.iter().map(|d| d.bytes).sum(),
        laden,
        loeschen,
        unbekannt,
    }
}

pub async fn lade(client: &reqwest::Client, url: &Url, token: Option<&str>) -> Result<Katalog, Fehler> {
    let mut anfrage = client.get(url.clone());
    if let Some(t) = token {
        anfrage = anfrage.bearer_auth(t);
    }
    let antwort = anfrage.send().await.map_err(|e| Fehler::Netz(format!("{url}: {e}")))?;
    match antwort.status().as_u16() {
        200 => {}
        401 | 403 => return Err(Fehler::Server("Der Server verlangt eine Anmeldung für die Daten.".into())),
        s => return Err(Fehler::Server(format!("Datenkatalog: {url} antwortet mit {s}."))),
    }
    let k: Katalog = antwort
        .json()
        .await
        .map_err(|e| Fehler::Server(format!("Datenkatalog nicht lesbar ({e}).")))?;
    pruefe(&k)?;
    Ok(k)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn katalog() -> Katalog {
        serde_json::from_str(include_str!("../../schema/beispiel-katalog.json")).unwrap()
    }

    fn auswahl(regionen: &[&str], pakete: &[&str]) -> Auswahl {
        Auswahl {
            regionen: regionen.iter().map(|s| s.to_string()).collect(),
            pakete: pakete.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn beispiel_ist_gueltig() {
        pruefe(&katalog()).unwrap();
    }

    #[test]
    fn region_bringt_ihre_pflichtpakete() {
        let (ids, _) = aufloesen(&katalog(), &auswahl(&["lk_sigmaringen"], &[]));
        assert_eq!(
            ids.into_iter().collect::<Vec<_>>(),
            vec!["gelaende-dgm.lk_sigmaringen", "karte-osm.lk_sigmaringen.z5-14"]
        );
    }

    #[test]
    fn abhaengigkeiten_werden_mitgenommen() {
        let (ids, _) = aufloesen(&katalog(), &auswahl(&[], &["karte-osm.lk_sigmaringen.z15-17"]));
        assert!(ids.contains("karte-osm.lk_sigmaringen.z5-14"));
    }

    #[test]
    fn unbekannte_ids_werden_gemeldet() {
        let (_, unbekannt) = aufloesen(&katalog(), &auswahl(&[], &["gibt-es-nicht"]));
        assert_eq!(unbekannt, vec!["gibt-es-nicht"]);
    }

    #[test]
    fn plan_laedt_nur_fehlendes_und_loescht_verwaistes() {
        let k = katalog();
        let mut vorhanden = BTreeMap::new();
        vorhanden.insert("karte/osm/lk_sigmaringen/z5-14.pmtiles".to_string(), "1".repeat(64));
        vorhanden.insert("gelaende/dgm/lk_sigmaringen.pmtiles".to_string(), "0".repeat(64)); // veraltet
        vorhanden.insert("alt/weg.pmtiles".to_string(), "9".repeat(64));
        let p = plan(&k, &auswahl(&["lk_sigmaringen"], &[]), &vorhanden);
        assert_eq!(p.laden.len(), 1);
        assert_eq!(p.laden[0].pfad, "gelaende/dgm/lk_sigmaringen.pmtiles");
        assert_eq!(p.bytes_zu_laden, 152_000_000);
        assert_eq!(p.bytes_gesamt, 152_000_000 + 48_213_004);
        assert_eq!(p.loeschen, vec!["alt/weg.pmtiles"]);
    }

    #[test]
    fn ungueltige_pruefsumme_oder_pfad_wird_abgelehnt() {
        let mut k = katalog();
        k.pakete[0].dateien[0].sha256 = "XYZ".into();
        assert!(pruefe(&k).is_err());
        let mut k = katalog();
        k.pakete[0].dateien[0].pfad = "../../etc/passwd".into();
        assert!(pruefe(&k).is_err());
    }

    #[test]
    fn gleicher_pfad_mit_zwei_inhalten_wird_abgelehnt() {
        let mut k = katalog();
        let mut zweites = k.pakete[0].clone();
        zweites.id = "doppelt".into();
        zweites.dateien[0].sha256 = "a".repeat(64);
        k.pakete.push(zweites);
        assert!(pruefe(&k).is_err());
    }
}
