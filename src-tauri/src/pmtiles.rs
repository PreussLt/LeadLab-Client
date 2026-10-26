//! PMTiles v3 lesen (CS7, Hauptrepo #762).
//!
//! Die Pakete baut der Paketbau des Servers (`tools/datenauslieferung/pmtiles.py`
//! im Hauptrepo): Kopf (127 Byte), gzip-komprimierte Verzeichnisse,
//! Kachel-Ids entlang der Hilbert-Kurve, Blattverzeichnisse ab 16 KiB.
//! Spezifikation: <https://github.com/protomaps/PMTiles/blob/main/spec/v3/spec.md>.
//!
//! Gelesen wird per Dateizugriff (`seek` + `read`), nichts wird ganz geladen.
//! Verzeichnisse bleiben nach dem ersten Zugriff im Speicher.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use flate2::read::GzDecoder;

pub const KOPF: usize = 127;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kacheltyp {
    Unbekannt,
    Mvt,
    Png,
    Jpeg,
    Webp,
}

impl Kacheltyp {
    pub fn mime(self) -> &'static str {
        match self {
            Kacheltyp::Png => "image/png",
            Kacheltyp::Jpeg => "image/jpeg",
            Kacheltyp::Webp => "image/webp",
            Kacheltyp::Mvt => "application/vnd.mapbox-vector-tile",
            Kacheltyp::Unbekannt => "application/octet-stream",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Kopf {
    pub wurzel_off: u64,
    pub wurzel_len: u64,
    pub blatt_off: u64,
    pub daten_off: u64,
    pub intern_kompression: u8,
    pub kachel_kompression: u8,
    pub typ: Kacheltyp,
    pub zoom_min: u8,
    pub zoom_max: u8,
    /// west, süd, ost, nord
    pub bbox: [f64; 4],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Eintrag {
    pub kachel_id: u64,
    pub offset: u64,
    pub laenge: u32,
    pub lauf: u32,
}

fn u64_le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn i32_le(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

pub fn kopf_lesen(b: &[u8]) -> Result<Kopf, String> {
    if b.len() < KOPF || &b[..7] != b"PMTiles" {
        return Err("keine PMTiles-Datei".into());
    }
    if b[7] != 3 {
        return Err(format!("PMTiles-Version {}, erwartet 3", b[7]));
    }
    let typ = match b[99] {
        1 => Kacheltyp::Mvt,
        2 => Kacheltyp::Png,
        3 => Kacheltyp::Jpeg,
        4 => Kacheltyp::Webp,
        _ => Kacheltyp::Unbekannt,
    };
    let e7 = |o| i32_le(b, o) as f64 / 1e7;
    Ok(Kopf {
        wurzel_off: u64_le(b, 8),
        wurzel_len: u64_le(b, 16),
        blatt_off: u64_le(b, 40),
        daten_off: u64_le(b, 56),
        intern_kompression: b[97],
        kachel_kompression: b[98],
        typ,
        zoom_min: b[100],
        zoom_max: b[101],
        bbox: [e7(102), e7(106), e7(110), e7(114)],
    })
}

fn drehen(n: u64, xy: &mut [u64; 2], rx: u64, ry: u64) {
    if ry == 0 {
        if rx != 0 {
            // Wie die Referenz: über die volle Koordinate, nur die unteren
            // Bits zählen danach — Überlauf ist gewollt (Zweierkomplement).
            xy[0] = n.wrapping_sub(1).wrapping_sub(xy[0]);
            xy[1] = n.wrapping_sub(1).wrapping_sub(xy[1]);
        }
        xy.swap(0, 1);
    }
}

/// Kachel-Id nach Spezifikation (Hilbert-Kurve je Zoomstufe).
pub fn zxy_zu_id(z: u8, x: u64, y: u64) -> Option<u64> {
    if z > 31 || x >= (1u64 << z) || y >= (1u64 << z) {
        return None;
    }
    let mut acc = ((1u64 << (2 * z as u64)) - 1) / 3;
    let mut xy = [x, y];
    for a in (0..z as u64).rev() {
        let s = 1u64 << a;
        let rx = u64::from(xy[0] & s != 0);
        let ry = u64::from(xy[1] & s != 0);
        acc += ((3 * rx) ^ ry) << (2 * a);
        drehen(s, &mut xy, rx, ry);
    }
    Some(acc)
}

fn varint(b: &[u8], pos: &mut usize) -> Result<u64, String> {
    let mut wert = 0u64;
    let mut schub = 0;
    loop {
        let byte = *b.get(*pos).ok_or("Verzeichnis zu kurz")?;
        *pos += 1;
        wert |= u64::from(byte & 0x7F) << schub;
        if byte & 0x80 == 0 {
            return Ok(wert);
        }
        schub += 7;
        if schub > 63 {
            return Err("Varint zu lang".into());
        }
    }
}

pub fn verzeichnis_lesen(gepackt: &[u8], kompression: u8) -> Result<Vec<Eintrag>, String> {
    let mut roh = Vec::new();
    match kompression {
        2 => {
            GzDecoder::new(gepackt).read_to_end(&mut roh).map_err(|e| e.to_string())?;
        }
        1 | 0 => roh.extend_from_slice(gepackt),
        k => return Err(format!("Verzeichnis-Kompression {k} nicht unterstützt")),
    }
    let mut pos = 0;
    let n = varint(&roh, &mut pos)? as usize;
    let mut e = vec![Eintrag { kachel_id: 0, offset: 0, laenge: 0, lauf: 0 }; n];
    let mut letzte = 0u64;
    for x in e.iter_mut() {
        letzte += varint(&roh, &mut pos)?;
        x.kachel_id = letzte;
    }
    for x in e.iter_mut() {
        x.lauf = varint(&roh, &mut pos)? as u32;
    }
    for x in e.iter_mut() {
        x.laenge = varint(&roh, &mut pos)? as u32;
    }
    for i in 0..n {
        let v = varint(&roh, &mut pos)?;
        e[i].offset = if v == 0 && i > 0 { e[i - 1].offset + u64::from(e[i - 1].laenge) } else { v.wrapping_sub(1) };
    }
    Ok(e)
}

fn suche(e: &[Eintrag], kid: u64) -> Option<Eintrag> {
    match e.binary_search_by(|x| x.kachel_id.cmp(&kid)) {
        Ok(i) => Some(e[i]),
        Err(0) => None,
        Err(i) => {
            let x = e[i - 1];
            (x.lauf == 0 || kid - x.kachel_id < u64::from(x.lauf)).then_some(x)
        }
    }
}

/// Eine geöffnete PMTiles-Datei.
pub struct Datei {
    pub pfad: PathBuf,
    pub kopf: Kopf,
    datei: Mutex<File>,
    wurzel: Vec<Eintrag>,
    blaetter: Mutex<HashMap<u64, Vec<Eintrag>>>,
}

impl Datei {
    pub fn oeffnen(pfad: &Path) -> Result<Self, String> {
        let mut f = File::open(pfad).map_err(|e| format!("{}: {e}", pfad.display()))?;
        let mut kopf = [0u8; KOPF];
        f.read_exact(&mut kopf).map_err(|e| e.to_string())?;
        let kopf = kopf_lesen(&kopf)?;
        let roh = lies(&mut f, kopf.wurzel_off, kopf.wurzel_len as usize)?;
        let wurzel = verzeichnis_lesen(&roh, kopf.intern_kompression)?;
        Ok(Datei { pfad: pfad.to_path_buf(), kopf, datei: Mutex::new(f), wurzel, blaetter: Mutex::new(HashMap::new()) })
    }

    /// Deckt die Datei diese Kachel ab (Zoomband und Rechteck aus dem Kopf)?
    pub fn deckt(&self, z: u8, x: u64, y: u64) -> bool {
        if z < self.kopf.zoom_min || z > self.kopf.zoom_max {
            return false;
        }
        let [w, s, o, n] = kachel_bbox(z, x, y);
        let [bw, bs, bo, bn] = self.kopf.bbox;
        !(o < bw || w > bo || n < bs || s > bn)
    }

    pub fn kachel(&self, z: u8, x: u64, y: u64) -> Result<Option<Vec<u8>>, String> {
        let Some(kid) = zxy_zu_id(z, x, y) else { return Ok(None) };
        let mut eintraege = self.wurzel.clone();
        for _ in 0..4 {
            let Some(e) = suche(&eintraege, kid) else { return Ok(None) };
            if e.lauf > 0 {
                let mut f = self.datei.lock().unwrap();
                let roh = lies(&mut f, self.kopf.daten_off + e.offset, e.laenge as usize)?;
                return Ok(Some(if self.kopf.kachel_kompression == 2 {
                    let mut aus = Vec::new();
                    GzDecoder::new(&roh[..]).read_to_end(&mut aus).map_err(|e| e.to_string())?;
                    aus
                } else {
                    roh
                }));
            }
            let mut blaetter = self.blaetter.lock().unwrap();
            if !blaetter.contains_key(&e.offset) {
                let mut f = self.datei.lock().unwrap();
                let roh = lies(&mut f, self.kopf.blatt_off + e.offset, e.laenge as usize)?;
                blaetter.insert(e.offset, verzeichnis_lesen(&roh, self.kopf.intern_kompression)?);
            }
            eintraege = blaetter[&e.offset].clone();
        }
        Ok(None)
    }
}

fn lies(f: &mut File, off: u64, laenge: usize) -> Result<Vec<u8>, String> {
    f.seek(SeekFrom::Start(off)).map_err(|e| e.to_string())?;
    let mut puffer = vec![0u8; laenge];
    f.read_exact(&mut puffer).map_err(|e| e.to_string())?;
    Ok(puffer)
}

/// WGS84-Rechteck einer Kachel: west, süd, ost, nord.
pub fn kachel_bbox(z: u8, x: u64, y: u64) -> [f64; 4] {
    let n = (1u64 << z) as f64;
    let lon = |x: f64| x / n * 360.0 - 180.0;
    let lat = |y: f64| (std::f64::consts::PI * (1.0 - 2.0 * y / n)).sinh().atan().to_degrees();
    [lon(x as f64), lat(y as f64 + 1.0), lon(x as f64 + 1.0), lat(y as f64)]
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    fn gz(roh: &[u8]) -> Vec<u8> {
        let mut e = GzEncoder::new(Vec::new(), Compression::default());
        e.write_all(roh).unwrap();
        e.finish().unwrap()
    }

    fn varint_schreiben(mut n: u64, aus: &mut Vec<u8>) {
        loop {
            let b = (n & 0x7F) as u8;
            n >>= 7;
            if n == 0 {
                aus.push(b);
                return;
            }
            aus.push(b | 0x80);
        }
    }

    fn verzeichnis(e: &[Eintrag]) -> Vec<u8> {
        let mut aus = Vec::new();
        varint_schreiben(e.len() as u64, &mut aus);
        let mut letzte = 0;
        for x in e {
            varint_schreiben(x.kachel_id - letzte, &mut aus);
            letzte = x.kachel_id;
        }
        for x in e {
            varint_schreiben(u64::from(x.lauf), &mut aus);
        }
        for x in e {
            varint_schreiben(u64::from(x.laenge), &mut aus);
        }
        for (i, x) in e.iter().enumerate() {
            if i > 0 && x.offset == e[i - 1].offset + u64::from(e[i - 1].laenge) {
                varint_schreiben(0, &mut aus);
            } else {
                varint_schreiben(x.offset + 1, &mut aus);
            }
        }
        gz(&aus)
    }

    /// Kleiner Schreiber für Tests: `blatt` > 0 erzwingt Blattverzeichnisse
    /// mit so vielen Einträgen. Kacheln in beliebiger Reihenfolge.
    pub fn schreiben(pfad: &Path, typ: u8, bbox: [f64; 4], kacheln: &[(u8, u64, u64, Vec<u8>)], blatt: usize) {
        let mut sortiert: Vec<_> = kacheln.iter().map(|(z, x, y, d)| (zxy_zu_id(*z, *x, *y).unwrap(), *z, d)).collect();
        sortiert.sort_by_key(|k| k.0);
        let mut daten = Vec::new();
        let mut eintraege = Vec::new();
        for (kid, _, d) in &sortiert {
            eintraege.push(Eintrag { kachel_id: *kid, offset: daten.len() as u64, laenge: d.len() as u32, lauf: 1 });
            daten.extend_from_slice(d);
        }
        let (wurzel, blaetter) = if blatt == 0 {
            (verzeichnis(&eintraege), Vec::new())
        } else {
            let mut w = Vec::new();
            let mut b = Vec::new();
            for teil in eintraege.chunks(blatt) {
                let v = verzeichnis(teil);
                w.push(Eintrag { kachel_id: teil[0].kachel_id, offset: b.len() as u64, laenge: v.len() as u32, lauf: 0 });
                b.extend_from_slice(&v);
            }
            (verzeichnis(&w), b)
        };
        let zmin = sortiert.iter().map(|k| k.1).min().unwrap_or(0);
        let zmax = sortiert.iter().map(|k| k.1).max().unwrap_or(0);
        let meta = gz(b"{}");
        let w_off = KOPF as u64;
        let m_off = w_off + wurzel.len() as u64;
        let b_off = m_off + meta.len() as u64;
        let d_off = b_off + blaetter.len() as u64;
        let mut kopf = Vec::new();
        kopf.extend_from_slice(b"PMTiles\x03");
        for v in [w_off, wurzel.len() as u64, m_off, meta.len() as u64, b_off, blaetter.len() as u64, d_off,
                  daten.len() as u64, eintraege.len() as u64, eintraege.len() as u64, eintraege.len() as u64] {
            kopf.extend_from_slice(&v.to_le_bytes());
        }
        kopf.extend_from_slice(&[1, 2, 1, typ, zmin, zmax]);
        for v in bbox {
            kopf.extend_from_slice(&((v * 1e7).round() as i32).to_le_bytes());
        }
        kopf.push(zmin);
        kopf.extend_from_slice(&[0u8; 8]);
        assert_eq!(kopf.len(), KOPF);
        let mut f = File::create(pfad).unwrap();
        for teil in [&kopf, &wurzel, &meta, &blaetter, &daten] {
            f.write_all(teil).unwrap();
        }
    }

    #[test]
    fn kachel_ids_wie_die_spezifikation() {
        assert_eq!(zxy_zu_id(0, 0, 0), Some(0));
        assert_eq!(zxy_zu_id(1, 0, 0), Some(1));
        assert_eq!(zxy_zu_id(1, 0, 1), Some(2));
        assert_eq!(zxy_zu_id(1, 1, 1), Some(3));
        assert_eq!(zxy_zu_id(1, 1, 0), Some(4));
        assert_eq!(zxy_zu_id(2, 0, 0), Some(5));
        assert_eq!(zxy_zu_id(12, 3423, 1763), Some(19078479));
        assert_eq!(zxy_zu_id(2, 4, 0), None);
    }

    #[test]
    fn liest_wurzel_und_blaetter() {
        let d = tempfile::tempdir().unwrap();
        let kacheln: Vec<_> = (0..40u64)
            .flat_map(|i| (0..30u64).map(move |j| (14u8, 8600 + i, 5700 + j, format!("{i}/{j}").into_bytes())))
            .collect();
        for blatt in [0, 64] {
            let p = d.path().join(format!("t{blatt}.pmtiles"));
            schreiben(&p, 2, [9.0, 48.0, 9.3, 48.2], &kacheln, blatt);
            let f = Datei::oeffnen(&p).unwrap();
            assert_eq!(f.kopf.typ, Kacheltyp::Png);
            assert_eq!((f.kopf.zoom_min, f.kopf.zoom_max), (14, 14));
            for (z, x, y, inhalt) in kacheln.iter().step_by(37) {
                assert_eq!(f.kachel(*z, *x, *y).unwrap().as_deref(), Some(&inhalt[..]));
            }
            assert_eq!(f.kachel(14, 1, 1).unwrap(), None);
            assert_eq!(f.kachel(13, 4300, 2850).unwrap(), None);
        }
    }

    /// Datei aus dem Paketbau des Servers (tools/datenauslieferung/pmtiles.py,
    /// mit Dubletten und Lauf): derselbe Inhalt wie in pmtiles_fixture.json.
    #[test]
    fn liest_was_der_paketbau_schreibt() {
        let wurzel = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let f = Datei::oeffnen(&wurzel.join("paketbau.pmtiles")).unwrap();
        let soll: Vec<(u8, u64, u64, String)> =
            serde_json::from_slice(&std::fs::read(wurzel.join("paketbau.json")).unwrap()).unwrap();
        for (z, x, y, inhalt) in soll {
            assert_eq!(f.kachel(z, x, y).unwrap(), Some(inhalt.into_bytes()), "{z}/{x}/{y}");
        }
        assert_eq!(f.kopf.typ, Kacheltyp::Jpeg);
    }

    #[test]
    fn deckt_nach_zoom_und_rechteck() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.pmtiles");
        schreiben(&p, 2, [9.2, 48.08, 9.23, 48.10], &[(10, 538, 355, b"x".to_vec()), (12, 2152, 1422, b"y".to_vec())], 0);
        let f = Datei::oeffnen(&p).unwrap();
        assert!(f.deckt(12, 2152, 1422));
        assert!(!f.deckt(12, 2152, 1430), "südlich des Rechtecks");
        assert!(!f.deckt(13, 4304, 2844), "Zoomstufe außerhalb");
        assert!(!f.deckt(12, 100, 100));
    }

    #[test]
    fn kaputte_datei() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("k.pmtiles");
        std::fs::write(&p, b"kein pmtiles").unwrap();
        assert!(Datei::oeffnen(&p).is_err());
    }
}
