//! Konfigurationsmodus (Hauptrepo #792, Konzept §8.6): der Zustand des
//! Clients in einer Struktur für die Einstellungsseite.
//!
//! Hier stehen die reinen Teile — welche Pakete lokal liegen, was nach einem
//! Download in die Zustandsdatei kommt, was beim Entfernen eines Pakets
//! wegfällt. Netz, Fenster und Schlüsselbund sammelt `lib.rs` dazu.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::ablage::{PaketStand, Zustand};
use crate::fehler::Fehler;
use crate::katalog::Katalog;

/// Eine Zeile der Pakettabelle.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PaketZeile {
    /// Katalog-Id; `None` für Dateien ohne Paketangabe (Zustandsdatei aus
    /// einer Version vor #792).
    pub id: Option<String>,
    pub region: Option<String>,
    pub gruppe: String,
    pub titel: String,
    pub version: Option<String>,
    /// Belegter Platz der vorhandenen Dateien.
    pub bytes: u64,
    pub dateien: usize,
    /// Alle Dateien des Pakets liegen geprüft vor.
    pub vollstaendig: bool,
    /// Zoomband über alle PMTiles des Pakets.
    pub zoom: Option<[u8; 2]>,
    pub geprueft: Option<u64>,
    /// Warum „Paket entfernen“ nicht geht (`None` = geht).
    pub sperre: Option<String>,
}

/// Paketstände nach einem erfolgreichen Download: alle aufgelösten Pakete
/// des Plans, mit neuem Prüfzeitpunkt, wenn in diesem Lauf eine ihrer Dateien
/// geladen wurde, sonst mit dem alten.
pub fn stand_nach_laden(
    alt: &BTreeMap<String, PaketStand>,
    kat: &Katalog,
    ids: &[String],
    geladen: &BTreeSet<String>,
    jetzt: u64,
) -> BTreeMap<String, PaketStand> {
    let mut aus = BTreeMap::new();
    for id in ids {
        let Some(p) = kat.pakete.iter().find(|p| &p.id == id) else { continue };
        let neu = p.dateien.iter().any(|d| geladen.contains(&d.pfad));
        let geprueft = if neu { Some(jetzt) } else { alt.get(id).and_then(|a| a.geprueft).or(Some(jetzt)) };
        aus.insert(
            id.clone(),
            PaketStand {
                region: p.region.clone(),
                region_name: kat.regionen.iter().find(|r| r.id == p.region).map(|r| r.name.clone()),
                gruppe: p.gruppe.clone(),
                titel: p.titel.clone(),
                version: p.version.clone(),
                pflicht: p.pflicht,
                braucht: p.braucht.clone(),
                dateien: p.dateien.iter().map(|d| (d.pfad.clone(), d.bytes)).collect(),
                geprueft,
            },
        );
    }
    aus
}

fn zoom_vereinen(a: Option<[u8; 2]>, b: [u8; 2]) -> [u8; 2] {
    match a {
        Some([lo, hi]) => [lo.min(b[0]), hi.max(b[1])],
        None => b,
    }
}

/// Warum ein Paket nicht einzeln weg kann.
fn sperre(z: &Zustand, id: &str, p: &PaketStand) -> Option<String> {
    if p.pflicht && z.auswahl.regionen.contains(&p.region) {
        let r = p.region_name.clone().unwrap_or_else(|| p.region.clone());
        return Some(format!("Grundpaket der Region {r}. Die Region unter „Daten verwalten“ abwählen."));
    }
    let nutzer: Vec<&str> =
        z.pakete.iter().filter(|(i, q)| i.as_str() != id && q.braucht.iter().any(|b| b == id)).map(|(_, q)| q.titel.as_str()).collect();
    if !nutzer.is_empty() {
        return Some(format!("Wird gebraucht von: {}.", nutzer.join(", ")));
    }
    None
}

/// Pakettabelle aus Zustandsdatei, tatsächlichem Bestand (`vorhanden`),
/// Zoombändern der PMTiles (`zoom`, je Pfad) und Dateigrößen auf der Platte.
/// Vorhandene Dateien, die kein gespeichertes Paket nennt, erscheinen je
/// Ordner als eigene Zeile ohne Id.
pub fn zeilen(
    z: &Zustand,
    vorhanden: &BTreeMap<String, String>,
    zoom: &BTreeMap<String, [u8; 2]>,
    groesse: &dyn Fn(&str) -> u64,
) -> Vec<PaketZeile> {
    let mut aus = Vec::new();
    let mut erfasst = BTreeSet::new();
    for (id, p) in &z.pakete {
        let da: Vec<&String> = p.dateien.keys().filter(|f| vorhanden.contains_key(*f)).collect();
        if da.is_empty() {
            continue;
        }
        let mut band = None;
        for f in &da {
            erfasst.insert((*f).clone());
            if let Some(b) = zoom.get(*f) {
                band = Some(zoom_vereinen(band, *b));
            }
        }
        aus.push(PaketZeile {
            id: Some(id.clone()),
            region: Some(p.region_name.clone().unwrap_or_else(|| p.region.clone())),
            gruppe: p.gruppe.clone(),
            titel: p.titel.clone(),
            version: Some(p.version.clone()),
            bytes: da.iter().map(|f| groesse(f)).sum(),
            dateien: da.len(),
            vollstaendig: da.len() == p.dateien.len(),
            zoom: band,
            geprueft: p.geprueft,
            sperre: sperre(z, id, p),
        });
    }
    let mut lose: BTreeMap<String, Vec<&String>> = BTreeMap::new();
    for f in vorhanden.keys().filter(|f| !erfasst.contains(*f)) {
        let ordner = f.rsplit_once('/').map(|(o, _)| o.to_string()).unwrap_or_default();
        lose.entry(ordner).or_default().push(f);
    }
    for (ordner, dateien) in lose {
        let mut band = None;
        for f in &dateien {
            if let Some(b) = zoom.get(*f) {
                band = Some(zoom_vereinen(band, *b));
            }
        }
        aus.push(PaketZeile {
            id: None,
            region: None,
            gruppe: ordner.split('/').next().unwrap_or("").to_string(),
            titel: if ordner.is_empty() { "(Wurzel)".into() } else { ordner },
            version: None,
            bytes: dateien.iter().map(|f| groesse(f)).sum(),
            dateien: dateien.len(),
            vollstaendig: true,
            zoom: band,
            geprueft: None,
            sperre: Some("Ohne Paketangabe geladen. Über „Daten verwalten“ abwählen.".into()),
        });
    }
    aus
}

/// Ein Paket entfernen: neue Zustandsdatei und die Pfade, die gelöscht
/// werden dürfen (keine, die ein anderes lokales Paket noch nennt). Das
/// Paket fällt auch aus der Auswahl, sonst lädt „Daten verwalten“ es wieder.
pub fn entfernen(z: &Zustand, id: &str) -> Result<(Zustand, Vec<String>), Fehler> {
    let p = z.pakete.get(id).ok_or_else(|| Fehler::Eingabe(format!("Paket {id} liegt nicht lokal.")))?;
    if let Some(grund) = sperre(z, id, p) {
        return Err(Fehler::Eingabe(grund));
    }
    let andere: BTreeSet<&String> =
        z.pakete.iter().filter(|(i, _)| i.as_str() != id).flat_map(|(_, q)| q.dateien.keys()).collect();
    let weg: Vec<String> = p.dateien.keys().filter(|f| !andere.contains(f)).cloned().collect();
    let mut neu = z.clone();
    neu.pakete.remove(id);
    neu.auswahl.pakete.retain(|x| x != id);
    for f in &weg {
        neu.dateien.remove(f);
    }
    Ok((neu, weg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::katalog::{Datei, Paket, Region};

    fn paket(id: &str, gruppe: &str, pflicht: bool, dateien: &[(&str, u64)], braucht: &[&str]) -> Paket {
        Paket {
            id: id.into(),
            gruppe: gruppe.into(),
            titel: format!("Titel {id}"),
            region: "lk_test".into(),
            version: "2026-10".into(),
            pflicht,
            lizenz: None,
            dateien: dateien.iter().map(|(p, b)| Datei { pfad: (*p).into(), bytes: *b, sha256: "a".repeat(64) }).collect(),
            braucht: braucht.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn katalog() -> Katalog {
        Katalog {
            schema: crate::katalog::SCHEMA.into(),
            erzeugt: Some("2026-10-27T10:00:00Z".into()),
            basis: vec!["https://lead.lab/daten/".parse().unwrap()],
            regionen: vec![Region { id: "lk_test".into(), name: "Landkreis Test".into(), bbox: None }],
            pakete: vec![
                paket("dgm", "gelaende", true, &[("gelaende/dgm/lk_test.pmtiles", 100)], &[]),
                paket("osm", "karte", false, &[("karte/osm/lk_test/z5-14.pmtiles", 50), ("karte/osm/lk_test/z15-16.pmtiles", 70)], &[]),
                paket("dop", "luftbild", false, &[("luftbild/dop/lk_test/z10-16.pmtiles", 900)], &["osm"]),
            ],
        }
    }

    fn zustand() -> Zustand {
        let kat = katalog();
        let ids: Vec<String> = ["dgm", "osm", "dop"].map(String::from).to_vec();
        let mut z = Zustand::default();
        z.auswahl.regionen = vec!["lk_test".into()];
        z.auswahl.pakete = vec!["dop".into(), "osm".into()];
        z.pakete = stand_nach_laden(&BTreeMap::new(), &kat, &ids, &BTreeSet::new(), 1000);
        for p in &kat.pakete {
            for d in &p.dateien {
                z.dateien.insert(d.pfad.clone(), d.sha256.clone());
            }
        }
        z
    }

    #[test]
    fn stand_nach_laden_haelt_alten_pruefzeitpunkt() {
        let kat = katalog();
        let ids: Vec<String> = vec!["dgm".into(), "osm".into(), "fehlt".into()];
        let alt = stand_nach_laden(&BTreeMap::new(), &kat, &ids, &BTreeSet::new(), 1000);
        assert_eq!(alt.len(), 2, "unbekannte Ids fallen weg");
        assert_eq!(alt["dgm"].region_name.as_deref(), Some("Landkreis Test"));
        assert_eq!(alt["osm"].dateien.len(), 2);
        let geladen: BTreeSet<String> = ["karte/osm/lk_test/z15-16.pmtiles".to_string()].into();
        let neu = stand_nach_laden(&alt, &kat, &ids, &geladen, 2000);
        assert_eq!(neu["osm"].geprueft, Some(2000), "neu geladen");
        assert_eq!(neu["dgm"].geprueft, Some(1000), "unverändert");
    }

    #[test]
    fn zeilen_mit_zoom_groesse_und_losen_dateien() {
        let z = zustand();
        let mut vorhanden: BTreeMap<String, String> = z.dateien.clone();
        vorhanden.remove("karte/osm/lk_test/z15-16.pmtiles"); // fehlt auf der Platte
        vorhanden.insert("gebaeude/lod2/alt/index.json".into(), "b".repeat(64)); // ohne Paketangabe
        let zoom: BTreeMap<String, [u8; 2]> = [
            ("karte/osm/lk_test/z5-14.pmtiles".to_string(), [5, 14]),
            ("gelaende/dgm/lk_test.pmtiles".to_string(), [8, 12]),
        ]
        .into();
        let groesse = |f: &str| if f.contains("dgm") { 111 } else { 10 };
        let r = zeilen(&z, &vorhanden, &zoom, &groesse);
        assert_eq!(r.len(), 4);
        let osm = r.iter().find(|x| x.id.as_deref() == Some("osm")).unwrap();
        assert!(!osm.vollstaendig);
        assert_eq!((osm.dateien, osm.bytes, osm.zoom), (1, 10, Some([5, 14])));
        let dgm = r.iter().find(|x| x.id.as_deref() == Some("dgm")).unwrap();
        assert_eq!(dgm.bytes, 111);
        assert!(dgm.sperre.as_deref().unwrap().contains("Grundpaket der Region Landkreis Test"));
        let lose = r.iter().find(|x| x.id.is_none()).unwrap();
        assert_eq!((lose.gruppe.as_str(), lose.titel.as_str()), ("gebaeude", "gebaeude/lod2/alt"));
        assert!(lose.sperre.is_some());
        let osm_sperre = r.iter().find(|x| x.id.as_deref() == Some("osm")).unwrap().sperre.clone();
        assert!(osm_sperre.unwrap().contains("Titel dop"), "dop braucht osm");
    }

    #[test]
    fn entfernen_raeumt_auswahl_und_dateien() {
        let z = zustand();
        let (neu, weg) = entfernen(&z, "dop").unwrap();
        assert_eq!(weg, vec!["luftbild/dop/lk_test/z10-16.pmtiles".to_string()]);
        assert!(!neu.pakete.contains_key("dop"));
        assert_eq!(neu.auswahl.pakete, vec!["osm".to_string()]);
        assert!(!neu.dateien.contains_key("luftbild/dop/lk_test/z10-16.pmtiles"));
        // danach ist osm frei
        let (neu2, weg2) = entfernen(&neu, "osm").unwrap();
        assert_eq!(weg2.len(), 2);
        assert!(neu2.auswahl.pakete.is_empty());
    }

    #[test]
    fn entfernen_gesperrt_und_geteilte_dateien() {
        let z = zustand();
        assert!(entfernen(&z, "dgm").unwrap_err().to_string().contains("Grundpaket"));
        assert!(entfernen(&z, "osm").unwrap_err().to_string().contains("gebraucht"));
        assert!(entfernen(&z, "gibtsnicht").is_err());
        // Zwei Pakete mit derselben Datei: die bleibt liegen.
        let mut z = z;
        z.pakete.get_mut("osm").unwrap().dateien.insert("luftbild/dop/lk_test/z10-16.pmtiles".into(), 900);
        z.pakete.get_mut("dop").unwrap().braucht.clear();
        let (_, weg) = entfernen(&z, "dop").unwrap();
        assert!(weg.is_empty());
    }
}
