//! Loopback-Server für lokale Pakete (Konzept §8.3, CS7 Hauptrepo #762).
//!
//! Lauscht auf `127.0.0.1:<zufallsport>` und antwortet nur unter einem
//! zufälligen Pfad-Token (`/t/<token>/…`). Andere Programme auf demselben
//! Rechner können die Adresse nicht raten; die Daten sind ohnehin offen.
//! Es gibt keine Schreibroute.
//!
//! | Pfad | Inhalt |
//! |---|---|
//! | `/status` | Version und vorhandene Pakete (Art, Zoomband, Rechteck) |
//! | `/kacheln/osm/{z}/{x}/{y}.png` | Straßenkarte aus `karte/osm/**.pmtiles` |
//! | `/kacheln/dop/{z}/{x}/{y}.jpg` | Luftbild aus `luftbild/dop/**.pmtiles` |
//! | `/gelaende/{z}/{x}/{y}.png` | Terrarium-Höhen aus `gelaende/dgm/*.pmtiles` |
//! | `/lod2/gebaeude?lat_min=…&lat_max=…&lon_min=…&lon_max=…` | wie `GET /api/lod2/gebaeude` |
//!
//! Fehlt eine Kachel oder ein Paket, gibt es 404 — die Oberfläche fragt dann
//! den Server (Frontend-Weiche, CS8). Welche Dateien es gibt, steht in der
//! Zustandsdatei der Ablage (nur vollständig geladene und geprüfte Dateien).
//!
//! CORS: `Access-Control-Allow-Origin: *` und für die Vorabfrage aus einer
//! https-Seite auf eine Loopback-Adresse `Access-Control-Allow-Private-Network`
//! (Private Network Access in WebView2).

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

use flate2::read::GzDecoder;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::ablage::Ablage;
use crate::pmtiles;

/// Obergrenze je LoD2-Abfrage wie `lod2MaxGebaeude` in der Go-API.
pub const LOD2_MAX: usize = 2000;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Paket {
    /// osm | dop | gelaende | lod2
    pub art: String,
    pub pfad: String,
    pub zoom: Option<[u8; 2]>,
    pub bbox: Option<[f64; 4]>,
}

struct Geteilt {
    ordner: RwLock<PathBuf>,
    dateien: Mutex<HashMap<PathBuf, (Option<SystemTime>, Arc<pmtiles::Datei>)>>,
    /// Ausgelieferte Kacheln und LoD2-Antworten seit dem Start (#792:
    /// zeigt, ob die Oberfläche den Dienst wirklich nutzt).
    geliefert: AtomicU64,
}

/// Ein laufender Loopback-Server.
#[derive(Clone)]
pub struct Lokal {
    pub port: u16,
    pub token: String,
    geteilt: Arc<Geteilt>,
}

fn token() -> String {
    let mut b = [0u8; 24];
    getrandom::fill(&mut b).expect("Zufallsquelle des Betriebssystems");
    hex::encode(b)
}

impl Lokal {
    /// Startet den Server auf einem freien Loopback-Port.
    pub async fn starten(ordner: PathBuf) -> std::io::Result<Lokal> {
        let l = TcpListener::bind("127.0.0.1:0").await?;
        let lokal = Lokal {
            port: l.local_addr()?.port(),
            token: token(),
            geteilt: Arc::new(Geteilt {
                ordner: RwLock::new(ordner),
                dateien: Mutex::new(HashMap::new()),
                geliefert: AtomicU64::new(0),
            }),
        };
        let bedienung = lokal.clone();
        tokio::spawn(async move {
            loop {
                let Ok((s, _)) = l.accept().await else { continue };
                let b = bedienung.clone();
                tokio::spawn(async move {
                    let _ = b.verbindung(s).await;
                });
            }
        });
        Ok(lokal)
    }

    /// Basisadresse für die Oberfläche: `http://127.0.0.1:<port>/t/<token>`.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/t/{}", self.port, self.token)
    }

    /// Wie viele Daten-Antworten (200, ohne `/status`) der Dienst gab.
    pub fn geliefert(&self) -> u64 {
        self.geteilt.geliefert.load(Ordering::Relaxed)
    }

    /// Neuer Datenordner (Assistent „Speicherort“).
    pub fn ordner_setzen(&self, ordner: PathBuf) {
        *self.geteilt.ordner.write().unwrap() = ordner;
        self.geteilt.dateien.lock().unwrap().clear();
    }

    fn ablage(&self) -> Ablage {
        Ablage::neu(self.geteilt.ordner.read().unwrap().clone())
    }

    /// Was lokal liegt: aus der Zustandsdatei, je PMTiles mit Zoomband und
    /// Rechteck aus dem Dateikopf.
    pub fn pakete(&self) -> Vec<Paket> {
        let a = self.ablage();
        let mut aus = Vec::new();
        for pfad in a.vorhanden().keys() {
            let art = art_von(pfad);
            let Some(art) = art else { continue };
            if art == "lod2" {
                if pfad.ends_with("/index.json") {
                    aus.push(Paket { art: art.into(), pfad: pfad.clone(), zoom: None, bbox: None });
                }
                continue;
            }
            if let Some(d) = self.datei(&a, pfad) {
                aus.push(Paket {
                    art: art.into(),
                    pfad: pfad.clone(),
                    zoom: Some([d.kopf.zoom_min, d.kopf.zoom_max]),
                    bbox: Some(d.kopf.bbox),
                });
            }
        }
        aus
    }

    fn datei(&self, a: &Ablage, pfad: &str) -> Option<Arc<pmtiles::Datei>> {
        let voll = a.ziel(pfad).ok()?;
        let stand = std::fs::metadata(&voll).and_then(|m| m.modified()).ok();
        let mut cache = self.geteilt.dateien.lock().unwrap();
        if let Some((s, d)) = cache.get(&voll) {
            if *s == stand {
                return Some(d.clone());
            }
        }
        let d = Arc::new(pmtiles::Datei::oeffnen(&voll).ok()?);
        cache.insert(voll, (stand, d.clone()));
        Some(d)
    }

    fn kachel(&self, art: &str, z: u8, x: u64, y: u64) -> Option<(Vec<u8>, &'static str)> {
        let a = self.ablage();
        for pfad in a.vorhanden().keys() {
            if art_von(pfad) != Some(art) {
                continue;
            }
            let Some(d) = self.datei(&a, pfad) else { continue };
            if !d.deckt(z, x, y) {
                continue;
            }
            if let Ok(Some(inhalt)) = d.kachel(z, x, y) {
                return Some((inhalt, d.kopf.typ.mime()));
            }
        }
        None
    }

    fn lod2(&self, abfrage: &BTreeMap<String, String>) -> Result<Value, (u16, String)> {
        let zahl = |n: &str| -> Result<f64, (u16, String)> {
            abfrage
                .get(n)
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite())
                .ok_or((422, format!("{n} fehlt oder ist keine Zahl")))
        };
        let (a1, a2, o1, o2) = (zahl("lat_min")?, zahl("lat_max")?, zahl("lon_min")?, zahl("lon_max")?);
        let (lat_min, lat_max, lon_min, lon_max) = (a1.min(a2), a1.max(a2), o1.min(o2), o1.max(o2));
        let a = self.ablage();
        let mut namen_gesamt = Vec::new();
        let mut gebaeude = Vec::new();
        let mut gekappt = false;
        let mut lizenz = Value::Null;
        let mut gefunden = false;
        for pfad in a.vorhanden().keys().filter(|p| art_von(p) == Some("lod2") && p.ends_with("/index.json")) {
            let Ok(voll) = a.ziel(pfad) else { continue };
            let Some(idx) = std::fs::read(&voll).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else {
                continue;
            };
            gefunden = true;
            if lizenz.is_null() {
                lizenz = idx.get("lizenz").cloned().unwrap_or(Value::Null);
            }
            let Some(kacheln) = idx.get("kacheln").and_then(Value::as_object) else { continue };
            let mut namen: Vec<&String> = kacheln.keys().collect();
            namen.sort();
            let ordner = voll.parent().unwrap_or(Path::new("."));
            for name in namen {
                let k = &kacheln[name];
                if !bbox_schneidet(k.get("bbox"), lat_min, lon_min, lat_max, lon_max) {
                    continue;
                }
                let Some(datei) = k.get("datei").and_then(Value::as_str) else { continue };
                if datei.contains('/') || datei.contains('\\') || datei.contains("..") {
                    continue;
                }
                let Some(inhalt) = lies_json(&ordner.join(datei)) else { continue };
                namen_gesamt.push(Value::String(name.clone()));
                for g in inhalt.get("gebaeude").and_then(Value::as_array).into_iter().flatten() {
                    if !grundriss_schneidet(g.get("grundriss"), lat_min, lon_min, lat_max, lon_max) {
                        continue;
                    }
                    if gebaeude.len() >= LOD2_MAX {
                        gekappt = true;
                        break;
                    }
                    gebaeude.push(g.clone());
                }
            }
        }
        if !gefunden {
            return Err((404, "kein LoD2-Paket".into()));
        }
        Ok(json!({ "verfuegbar": true, "kacheln": namen_gesamt, "gebaeude": gebaeude, "gekappt": gekappt, "lizenz": lizenz }))
    }

    async fn verbindung(&self, mut s: TcpStream) -> std::io::Result<()> {
        let mut puffer = Vec::with_capacity(4096);
        loop {
            // Kopf bis zur Leerzeile lesen (höchstens 16 KB)
            let ende = loop {
                if let Some(i) = puffer.windows(4).position(|w| w == b"\r\n\r\n") {
                    break i + 4;
                }
                if puffer.len() > 16384 {
                    return Ok(());
                }
                let mut stueck = [0u8; 4096];
                let n = s.read(&mut stueck).await?;
                if n == 0 {
                    return Ok(());
                }
                puffer.extend_from_slice(&stueck[..n]);
            };
            let kopf = String::from_utf8_lossy(&puffer[..ende]).into_owned();
            puffer.drain(..ende);
            let mut zeilen = kopf.split("\r\n");
            let erste = zeilen.next().unwrap_or("");
            let schliessen = zeilen.any(|z| z.to_ascii_lowercase().starts_with("connection:") && z.to_ascii_lowercase().contains("close"));
            let mut teile = erste.split_whitespace();
            let (methode, ziel) = (teile.next().unwrap_or(""), teile.next().unwrap_or(""));
            let a = self.antwort(methode, ziel);
            if a.status == 200 && methode == "GET" && !ziel.ends_with("/status") {
                self.geteilt.geliefert.fetch_add(1, Ordering::Relaxed);
            }
            s.write_all(&a.bytes(methode == "HEAD", schliessen)).await?;
            if schliessen {
                return Ok(());
            }
        }
    }

    /// Eine Anfrage beantworten (ohne Netz testbar).
    pub fn antwort(&self, methode: &str, ziel: &str) -> Antwort {
        let (pfad, abfrage) = ziel.split_once('?').unwrap_or((ziel, ""));
        let vorsatz = format!("/t/{}/", self.token);
        let Some(rest) = pfad.strip_prefix(&vorsatz) else {
            return Antwort::status(404, "Unbekannt");
        };
        if methode == "OPTIONS" {
            return Antwort { status: 204, typ: "text/plain", rumpf: Vec::new(), vorab: true };
        }
        if methode != "GET" && methode != "HEAD" {
            return Antwort::status(405, "Nur lesen");
        }
        let teile: Vec<&str> = rest.split('/').collect();
        match teile.as_slice() {
            ["status"] => Antwort::json(&json!({
                "version": env!("CARGO_PKG_VERSION"),
                "pakete": self.pakete(),
            })),
            ["kacheln", "osm", z, x, y] => self.kachel_antwort("osm", z, x, y, ".png"),
            ["kacheln", "dop", z, x, y] => self.kachel_antwort("dop", z, x, y, ".jpg"),
            ["gelaende", z, x, y] => self.kachel_antwort("gelaende", z, x, y, ".png"),
            ["lod2", "gebaeude"] => match self.lod2(&abfrage_lesen(abfrage)) {
                Ok(v) => Antwort::json(&v),
                Err((s, m)) => Antwort::status(s, &m),
            },
            _ => Antwort::status(404, "Unbekannt"),
        }
    }

    fn kachel_antwort(&self, art: &str, z: &str, x: &str, y: &str, endung: &str) -> Antwort {
        let y = y.strip_suffix(endung).unwrap_or(y);
        let (Ok(z), Ok(x), Ok(y)) = (z.parse::<u8>(), x.parse::<u64>(), y.parse::<u64>()) else {
            return Antwort::status(404, "Unbekannt");
        };
        match self.kachel(art, z, x, y) {
            Some((rumpf, typ)) => Antwort { status: 200, typ, rumpf, vorab: false },
            None => Antwort::status(404, "Keine lokale Kachel"),
        }
    }
}

fn art_von(pfad: &str) -> Option<&'static str> {
    if pfad.starts_with("karte/osm/") && pfad.ends_with(".pmtiles") {
        Some("osm")
    } else if pfad.starts_with("luftbild/dop/") && pfad.ends_with(".pmtiles") {
        Some("dop")
    } else if pfad.starts_with("gelaende/dgm/") && pfad.ends_with(".pmtiles") {
        Some("gelaende")
    } else if pfad.starts_with("gebaeude/lod2/") {
        Some("lod2")
    } else {
        None
    }
}

fn lies_json(pfad: &Path) -> Option<Value> {
    let roh = std::fs::read(pfad).ok()?;
    if pfad.extension().is_some_and(|e| e == "gz") {
        let mut aus = Vec::new();
        GzDecoder::new(&roh[..]).read_to_end(&mut aus).ok()?;
        serde_json::from_slice(&aus).ok()
    } else {
        serde_json::from_slice(&roh).ok()
    }
}

fn zahl(v: &Value, n: &str) -> Option<f64> {
    v.get(n).and_then(Value::as_f64)
}

/// Wie `_schneidet` im Fachdienst: überlappen sich die WGS84-Rechtecke?
fn bbox_schneidet(b: Option<&Value>, lat_min: f64, lon_min: f64, lat_max: f64, lon_max: f64) -> bool {
    let Some(b) = b else { return false };
    let (Some(a1), Some(a2), Some(o1), Some(o2)) = (zahl(b, "latMin"), zahl(b, "latMax"), zahl(b, "lonMin"), zahl(b, "lonMax")) else {
        return false;
    };
    !(a2 < lat_min || a1 > lat_max || o2 < lon_min || o1 > lon_max)
}

/// Wie `grundrissSchneidet` in der Go-API: über das umschließende Rechteck.
fn grundriss_schneidet(g: Option<&Value>, lat_min: f64, lon_min: f64, lat_max: f64, lon_max: f64) -> bool {
    let Some(punkte) = g.and_then(Value::as_array) else { return false };
    let mut b: Option<(f64, f64, f64, f64)> = None;
    for p in punkte {
        let (Some(la), Some(lo)) = (p.get(0).and_then(Value::as_f64), p.get(1).and_then(Value::as_f64)) else { continue };
        b = Some(match b {
            None => (la, la, lo, lo),
            Some((a1, a2, o1, o2)) => (a1.min(la), a2.max(la), o1.min(lo), o2.max(lo)),
        });
    }
    let Some((a1, a2, o1, o2)) = b else { return false };
    !(a2 < lat_min || a1 > lat_max || o2 < lon_min || o1 > lon_max)
}

fn abfrage_lesen(q: &str) -> BTreeMap<String, String> {
    url::form_urlencoded::parse(q.as_bytes()).into_owned().collect()
}

/// Eine HTTP-Antwort des Loopback-Servers.
#[derive(Debug)]
pub struct Antwort {
    pub status: u16,
    pub typ: &'static str,
    pub rumpf: Vec<u8>,
    /// CORS-Vorabfrage (OPTIONS)
    pub vorab: bool,
}

impl Antwort {
    fn status(status: u16, text: &str) -> Self {
        Antwort { status, typ: "text/plain; charset=utf-8", rumpf: text.as_bytes().to_vec(), vorab: false }
    }

    fn json(v: &Value) -> Self {
        Antwort { status: 200, typ: "application/json", rumpf: serde_json::to_vec(v).unwrap_or_default(), vorab: false }
    }

    pub fn bytes(&self, nur_kopf: bool, schliessen: bool) -> Vec<u8> {
        let grund = match self.status {
            200 => "OK",
            204 => "No Content",
            404 => "Not Found",
            405 => "Method Not Allowed",
            422 => "Unprocessable Entity",
            _ => "Status",
        };
        let mut k = format!(
            "HTTP/1.1 {} {grund}\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\n\
X-Content-Type-Options: nosniff\r\nCache-Control: {}\r\n",
            self.status,
            self.typ,
            self.rumpf.len(),
            if self.status == 200 { "private, max-age=86400" } else { "no-store" },
        );
        if self.vorab {
            k.push_str("Access-Control-Allow-Methods: GET, HEAD, OPTIONS\r\nAccess-Control-Allow-Headers: *\r\n\
Access-Control-Allow-Private-Network: true\r\nAccess-Control-Max-Age: 600\r\n");
        }
        if schliessen {
            k.push_str("Connection: close\r\n");
        }
        k.push_str("\r\n");
        let mut aus = k.into_bytes();
        if !nur_kopf {
            aus.extend_from_slice(&self.rumpf);
        }
        aus
    }
}

/// Initialisierungsskript für die Server-Oberfläche im Client-Fenster:
/// `window.__LEADLAB_CLIENT__ = { version, lokal, pakete }`, eingefroren.
/// Die lokale Assistentenseite (`ausgenommen`) bekommt es nicht; IPC-Rechte
/// bekommt die entfernte Seite ohnehin keine (capabilities ohne `remote`).
pub fn initialisierung(lokal: &Lokal, ausgenommen: &[String]) -> String {
    let daten = json!({ "version": env!("CARGO_PKG_VERSION"), "lokal": lokal.url(), "pakete": lokal.pakete() });
    let aus = serde_json::to_string(ausgenommen).unwrap_or_else(|_| "[]".into());
    format!(
        "(function(){{try{{var aus={aus};if(aus.indexOf(location.origin)>=0)return;\
Object.defineProperty(window,'__LEADLAB_CLIENT__',{{value:Object.freeze({daten}),writable:false,configurable:false}});}}catch(e){{}}}})();"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ablage::Zustand;
    use crate::pmtiles::tests::schreiben;
    use std::io::Write;

    fn gz(roh: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(roh).unwrap();
        e.finish().unwrap()
    }

    /// Datenordner mit OSM z14, Gelände z12, LoD2 (zwei Kacheln) und einer
    /// noch nicht fertigen Datei (fehlt in der Zustandsdatei).
    fn ordner() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let w = d.path();
        std::fs::create_dir_all(w.join("karte/osm/lk_test")).unwrap();
        std::fs::create_dir_all(w.join("gelaende/dgm")).unwrap();
        std::fs::create_dir_all(w.join("gebaeude/lod2/lk_test")).unwrap();
        std::fs::create_dir_all(w.join("luftbild/dop/lk_test")).unwrap();
        schreiben(&w.join("karte/osm/lk_test/z5-14.pmtiles"), 2, [9.2, 48.08, 9.23, 48.10],
                  &[(14, 8611, 5689, b"OSM".to_vec())], 0);
        schreiben(&w.join("gelaende/dgm/lk_test.pmtiles"), 2, [9.2, 48.08, 9.23, 48.10],
                  &[(12, 2152, 1422, b"TERRARIUM".to_vec())], 0);
        schreiben(&w.join("luftbild/dop/lk_test/z10-16.pmtiles"), 3, [9.2, 48.08, 9.23, 48.10],
                  &[(16, 34444, 22756, b"DOP".to_vec())], 0);
        let idx = json!({"lizenz": "© LGL BW, dl-de/by-2-0", "kacheln": {
            "32_500_5330": {"bbox": {"latMin": 48.08, "latMax": 48.09, "lonMin": 9.20, "lonMax": 9.21}, "datei": "32_500_5330.json.gz"},
            "32_501_5330": {"bbox": {"latMin": 48.08, "latMax": 48.09, "lonMin": 9.21, "lonMax": 9.22}, "datei": "32_501_5330.json.gz"}}});
        std::fs::write(w.join("gebaeude/lod2/lk_test/index.json"), idx.to_string()).unwrap();
        for (n, lo) in [("32_500_5330", 9.205), ("32_501_5330", 9.215)] {
            let g = json!({"gebaeude": [
                {"id": format!("{n}-a"), "grundriss": [[48.085, lo], [48.0851, lo + 0.0001]]},
                {"id": format!("{n}-b"), "grundriss": [[48.0899, lo], [48.0899, lo]]}]});
            std::fs::write(w.join(format!("gebaeude/lod2/lk_test/{n}.json.gz")), gz(g.to_string().as_bytes())).unwrap();
        }
        let mut z = Zustand::default();
        for p in ["karte/osm/lk_test/z5-14.pmtiles", "gelaende/dgm/lk_test.pmtiles",
                  "gebaeude/lod2/lk_test/index.json", "gebaeude/lod2/lk_test/32_500_5330.json.gz",
                  "gebaeude/lod2/lk_test/32_501_5330.json.gz"] {
            z.dateien.insert(p.into(), "a".repeat(64));
        }
        // luftbild/dop fehlt absichtlich im Zustand: nicht fertig geladen
        Ablage::neu(w).zustand_schreiben(&z).unwrap();
        d
    }

    async fn server(d: &tempfile::TempDir) -> Lokal {
        Lokal::starten(d.path().to_path_buf()).await.unwrap()
    }

    #[tokio::test]
    async fn kacheln_aus_den_paketen_sonst_404() {
        let d = ordner();
        let l = server(&d).await;
        let a = l.antwort("GET", &format!("/t/{}/kacheln/osm/14/8611/5689.png", l.token));
        assert_eq!((a.status, a.typ, &a.rumpf[..]), (200, "image/png", &b"OSM"[..]));
        assert_eq!(l.antwort("GET", &format!("/t/{}/kacheln/osm/14/8611/5690.png", l.token)).status, 404);
        assert_eq!(l.antwort("GET", &format!("/t/{}/kacheln/osm/15/1/1.png", l.token)).status, 404);
        let g = l.antwort("GET", &format!("/t/{}/gelaende/12/2152/1422.png", l.token));
        assert_eq!((g.status, &g.rumpf[..]), (200, &b"TERRARIUM"[..]));
        // DOP-Datei liegt zwar da, steht aber nicht im Zustand → nicht ausgeliefert
        assert_eq!(l.antwort("GET", &format!("/t/{}/kacheln/dop/16/34444/22756.jpg", l.token)).status, 404);
    }

    #[tokio::test]
    async fn ohne_token_nichts() {
        let d = ordner();
        let l = server(&d).await;
        assert_eq!(l.antwort("GET", "/t/falsch/kacheln/osm/14/8611/5689.png").status, 404);
        assert_eq!(l.antwort("GET", "/kacheln/osm/14/8611/5689.png").status, 404);
        assert_eq!(l.antwort("POST", &format!("/t/{}/status", l.token)).status, 405);
    }

    #[tokio::test]
    async fn status_nennt_die_pakete() {
        let d = ordner();
        let l = server(&d).await;
        let a = l.antwort("GET", &format!("/t/{}/status", l.token));
        let v: Value = serde_json::from_slice(&a.rumpf).unwrap();
        let arten: Vec<&str> = v["pakete"].as_array().unwrap().iter().map(|p| p["art"].as_str().unwrap()).collect();
        assert_eq!(arten, ["lod2", "gelaende", "osm"]);
        let osm = &v["pakete"][2];
        assert_eq!(osm["zoom"], json!([14, 14]));
        assert_eq!(osm["bbox"], json!([9.2, 48.08, 9.23, 48.1]));
    }

    #[tokio::test]
    async fn lod2_wie_die_api() {
        let d = ordner();
        let l = server(&d).await;
        let q = |s: &str| l.antwort("GET", &format!("/t/{}/lod2/gebaeude?{s}", l.token));
        let a = q("lat_min=48.084&lat_max=48.086&lon_min=9.20&lon_max=9.22");
        let v: Value = serde_json::from_slice(&a.rumpf).unwrap();
        assert_eq!(v["kacheln"], json!(["32_500_5330", "32_501_5330"]));
        let ids: Vec<&str> = v["gebaeude"].as_array().unwrap().iter().map(|g| g["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["32_500_5330-a", "32_501_5330-a"]);
        assert_eq!(v["lizenz"], json!("© LGL BW, dl-de/by-2-0"));
        assert_eq!(v["gekappt"], json!(false));
        // vertauschte Grenzen werden sortiert, kein Fehler
        let b: Value = serde_json::from_slice(&q("lat_min=48.086&lat_max=48.084&lon_min=9.21&lon_max=9.20").rumpf).unwrap();
        assert_eq!(b["gebaeude"].as_array().unwrap().len(), 1);
        assert_eq!(q("lat_min=x&lat_max=1&lon_min=1&lon_max=1").status, 422);
    }

    #[tokio::test]
    async fn ueber_tcp_mit_cors_und_keep_alive() {
        let d = ordner();
        let l = server(&d).await;
        let c = reqwest::Client::new();
        let basis = l.url();
        for _ in 0..3 {
            let r = c.get(format!("{basis}/kacheln/osm/14/8611/5689.png")).send().await.unwrap();
            assert_eq!(r.status().as_u16(), 200);
            assert_eq!(r.headers()["access-control-allow-origin"], "*");
            assert_eq!(r.bytes().await.unwrap().as_ref(), b"OSM");
        }
        let r = c.request(reqwest::Method::OPTIONS, format!("{basis}/lod2/gebaeude"))
            .header("Access-Control-Request-Private-Network", "true")
            .send().await.unwrap();
        assert_eq!(r.status().as_u16(), 204);
        assert_eq!(r.headers()["access-control-allow-private-network"], "true");
        assert!(basis.starts_with("http://127.0.0.1:"));
        c.get(format!("{basis}/status")).send().await.unwrap();
        c.get(format!("{basis}/kacheln/osm/14/1/1.png")).send().await.unwrap();
        assert_eq!(l.geliefert(), 3, "nur ausgelieferte Kacheln zählen, nicht /status, 404 oder OPTIONS");
        let r = c.get(format!("http://127.0.0.1:{}/t/falsch/status", l.port)).send().await.unwrap();
        assert_eq!(r.status().as_u16(), 404);
    }

    #[tokio::test]
    async fn neuer_ordner() {
        let d = ordner();
        let l = server(&d).await;
        let leer = tempfile::tempdir().unwrap();
        l.ordner_setzen(leer.path().to_path_buf());
        assert_eq!(l.antwort("GET", &format!("/t/{}/kacheln/osm/14/8611/5689.png", l.token)).status, 404);
        assert!(l.pakete().is_empty());
    }

    #[tokio::test]
    async fn initialisierungsskript() {
        let d = ordner();
        let l = server(&d).await;
        let js = initialisierung(&l, &["tauri://localhost".into(), "http://tauri.localhost".into()]);
        assert!(js.contains(&l.url()));
        assert!(js.contains("__LEADLAB_CLIENT__") && js.contains("Object.freeze"));
        assert!(js.contains(r#"["tauri://localhost","http://tauri.localhost"]"#));
        assert!(js.contains(r#""art":"osm""#));
        assert_ne!(l.token, server(&d).await.token, "jeder Start ein neues Token");
        assert_eq!(l.token.len(), 48);
    }

    /// Gegen einen echten Datenordner aus dem Paketbau des Servers:
    /// `LEADLAB_TEST_DATENORDNER=<ordner> cargo test -- --ignored echter`.
    #[tokio::test]
    #[ignore]
    async fn echter_datenordner() {
        let Ok(o) = std::env::var("LEADLAB_TEST_DATENORDNER") else { return };
        let l = Lokal::starten(PathBuf::from(o)).await.unwrap();
        let c = reqwest::Client::new();
        let status: Value = c.get(format!("{}/status", l.url())).send().await.unwrap().json().await.unwrap();
        println!("status: {status}");
        // Sigmaringen, Rathaus: z14 8611/5689, z16 34446/22757
        for (pfad, typ) in [("kacheln/osm/14/8611/5689.png", "image/png"), ("kacheln/osm/16/34446/22757.png", "image/png"),
                            ("kacheln/dop/16/34446/22757.jpg", "image/jpeg"), ("gelaende/14/8611/5689.png", "image/png")] {
            let r = c.get(format!("{}/{pfad}", l.url())).send().await.unwrap();
            let s = r.status().as_u16();
            let t = r.headers().get("content-type").map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
            let n = r.bytes().await.unwrap().len();
            println!("{pfad}: {s} {t} {n} Byte");
            assert_eq!((s, t.as_str()), (200, typ), "{pfad}");
        }
        assert_eq!(c.get(format!("{}/kacheln/osm/14/1/1.png", l.url())).send().await.unwrap().status().as_u16(), 404);
    }
}
