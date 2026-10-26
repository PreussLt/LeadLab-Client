//! Downloader (Konzept §6.1, §8.2).
//!
//! - Jede Datei kommt erst als `<pfad>.teil` auf die Platte. Liegt schon ein
//!   Teil, wird er gehasht und per `Range: bytes=<länge>-` fortgesetzt. Ein
//!   Server ohne Range (200 statt 206) bedeutet Neubeginn.
//! - Nach dem letzten Byte muss der SHA-256 dem Katalog entsprechen, sonst
//!   wird der Teil verworfen. Erst dann wird umbenannt und der Zustand
//!   fortgeschrieben.
//! - Höchstens `STROEME` Dateien gleichzeitig; die Basisadressen des Katalogs
//!   werden der Reihe nach versucht (Spiegel).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use url::Url;

use crate::ablage::{Ablage, Zustand};
use crate::fehler::Fehler;
use crate::katalog::{Datei, Plan};

pub const STROEME: usize = 3;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Fortschritt {
    pub pfad: String,
    pub geladen: u64,
    pub gesamt: u64,
    /// "laeuft" | "fertig" | "fehler"
    pub stand: &'static str,
    pub meldung: Option<String>,
}

/// Blob-Adresse: `<basis>sha256/<ersten zwei>/<hash>`.
pub fn blob_url(basis: &Url, sha256: &str) -> Result<Url, Fehler> {
    let mut b = basis.clone();
    if !b.path().ends_with('/') {
        let p = format!("{}/", b.path());
        b.set_path(&p);
    }
    b.join(&format!("sha256/{}/{}", &sha256[..2], sha256))
        .map_err(|e| Fehler::Server(e.to_string()))
}

async fn hash_von(pfad: &Path) -> Result<(Sha256, u64), Fehler> {
    let mut h = Sha256::new();
    let mut n = 0u64;
    let mut f = tokio::fs::File::open(pfad).await?;
    let mut puffer = vec![0u8; 1 << 20];
    loop {
        let k = f.read(&mut puffer).await?;
        if k == 0 {
            break;
        }
        h.update(&puffer[..k]);
        n += k as u64;
    }
    Ok((h, n))
}

/// Lädt eine Datei vollständig und geprüft an ihren Zielort.
pub async fn lade_datei(
    client: &reqwest::Client,
    basis: &[Url],
    token: Option<&str>,
    ablage: &Ablage,
    datei: &Datei,
    abbruch: &AtomicBool,
    melde: &(dyn Fn(u64) + Send + Sync),
) -> Result<(), Fehler> {
    let ziel = ablage.ziel(&datei.pfad)?;
    let teil = ablage.teil(&datei.pfad)?;
    if let Some(e) = teil.parent() {
        tokio::fs::create_dir_all(e).await?;
    }
    let mut letzter = None;
    for b in basis {
        match versuch(client, b, token, &teil, datei, abbruch, melde).await {
            Ok(()) => {
                tokio::fs::rename(&teil, &ziel).await?;
                return Ok(());
            }
            Err(e @ (Fehler::Abgebrochen | Fehler::Datei(_))) => return Err(e),
            Err(e) => letzter = Some(e),
        }
    }
    Err(letzter.unwrap_or_else(|| Fehler::Server("Keine Download-Adresse.".into())))
}

async fn versuch(
    client: &reqwest::Client,
    basis: &Url,
    token: Option<&str>,
    teil: &Path,
    datei: &Datei,
    abbruch: &AtomicBool,
    melde: &(dyn Fn(u64) + Send + Sync),
) -> Result<(), Fehler> {
    let (mut hasher, mut start) = if teil.exists() { hash_von(teil).await? } else { (Sha256::new(), 0) };
    if start > datei.bytes {
        tokio::fs::remove_file(teil).await?;
        hasher = Sha256::new();
        start = 0;
    }
    let url = blob_url(basis, &datei.sha256)?;
    let mut anfrage = client.get(url.clone());
    if let Some(t) = token {
        anfrage = anfrage.bearer_auth(t);
    }
    if start > 0 && start < datei.bytes {
        anfrage = anfrage.header(reqwest::header::RANGE, format!("bytes={start}-"));
    }
    let mut datei_zu = tokio::fs::OpenOptions::new();
    datei_zu.create(true).write(true);
    if start < datei.bytes {
        let antwort = anfrage.send().await.map_err(|e| Fehler::Netz(format!("{url}: {e}")))?;
        match antwort.status().as_u16() {
            206 => {
                datei_zu.append(true);
            }
            200 => {
                // Server hat die Range ignoriert: von vorn.
                hasher = Sha256::new();
                start = 0;
                datei_zu.truncate(true);
            }
            401 | 403 => return Err(Fehler::Server("Der Server verlangt eine Anmeldung für die Daten.".into())),
            s => return Err(Fehler::Server(format!("{url} antwortet mit {s}."))),
        }
        let mut f = datei_zu.open(teil).await?;
        let mut n = start;
        melde(n);
        let mut strom = antwort.bytes_stream();
        while let Some(stueck) = strom.next().await {
            if abbruch.load(Ordering::Relaxed) {
                f.flush().await?;
                return Err(Fehler::Abgebrochen);
            }
            let stueck = match stueck {
                Ok(s) => s,
                Err(e) => {
                    // Bisheriges sichern, damit der nächste Versuch fortsetzt.
                    f.flush().await?;
                    return Err(Fehler::Netz(format!("{url}: {e}")));
                }
            };
            hasher.update(&stueck);
            f.write_all(&stueck).await?;
            n += stueck.len() as u64;
            if n > datei.bytes {
                drop(f);
                tokio::fs::remove_file(teil).await?;
                return Err(Fehler::Pruefsumme(format!("{} ist größer als angekündigt.", datei.pfad)));
            }
            melde(n);
        }
        f.flush().await?;
    }
    let ist = hex::encode(hasher.finalize());
    if ist != datei.sha256 {
        tokio::fs::remove_file(teil).await?;
        return Err(Fehler::Pruefsumme(format!(
            "{}: Prüfsumme stimmt nicht (Download beschädigt), wird neu geladen.",
            datei.pfad
        )));
    }
    Ok(())
}

/// Führt einen Plan aus: verwaiste Dateien löschen, fehlende laden, Zustand
/// nach jeder fertigen Datei schreiben (ein Abbruch verliert nichts).
pub async fn ausfuehren(
    client: reqwest::Client,
    basis: Vec<Url>,
    token: Option<String>,
    ablage: Arc<Ablage>,
    plan: Plan,
    auswahl: crate::katalog::Auswahl,
    erzeugt: Option<String>,
    abbruch: Arc<AtomicBool>,
    melde: Arc<dyn Fn(Fortschritt) + Send + Sync>,
) -> Result<(), Fehler> {
    let zustand = Arc::new(Mutex::new({
        let mut z = ablage.zustand_lesen();
        z.auswahl = auswahl;
        z
    }));
    for p in &plan.loeschen {
        ablage.loeschen(p)?;
        zustand.lock().await.dateien.remove(p);
    }
    ablage.zustand_schreiben(&*zustand.lock().await)?;

    let ergebnisse: Vec<Result<(), Fehler>> = futures_util::stream::iter(plan.laden.into_iter())
        .map(|datei| {
            let (client, basis, token, ablage, abbruch, melde, zustand) = (
                client.clone(),
                basis.clone(),
                token.clone(),
                ablage.clone(),
                abbruch.clone(),
                melde.clone(),
                zustand.clone(),
            );
            async move {
                let pfad = datei.pfad.clone();
                let gesamt = datei.bytes;
                let m = melde.clone();
                let p2 = pfad.clone();
                let teilmeldung = move |geladen: u64| {
                    m(Fortschritt { pfad: p2.clone(), geladen, gesamt, stand: "laeuft", meldung: None })
                };
                let r = lade_datei(&client, &basis, token.as_deref(), &ablage, &datei, &abbruch, &teilmeldung).await;
                match &r {
                    Ok(()) => {
                        let mut z = zustand.lock().await;
                        z.dateien.insert(pfad.clone(), datei.sha256.clone());
                        ablage.zustand_schreiben(&z)?;
                        melde(Fortschritt { pfad, geladen: gesamt, gesamt, stand: "fertig", meldung: None });
                    }
                    Err(e) => melde(Fortschritt {
                        pfad,
                        geladen: 0,
                        gesamt,
                        stand: "fehler",
                        meldung: Some(e.to_string()),
                    }),
                }
                r
            }
        })
        .buffer_unordered(STROEME)
        .collect()
        .await;

    let mut z: Zustand = zustand.lock().await.clone();
    if abbruch.load(Ordering::Relaxed) {
        return Err(Fehler::Abgebrochen);
    }
    let fehler: Vec<String> = ergebnisse.into_iter().filter_map(|r| r.err()).map(|e| e.to_string()).collect();
    if !fehler.is_empty() {
        return Err(Fehler::Server(format!("{} Datei(en) nicht geladen: {}", fehler.len(), fehler.join(" · "))));
    }
    z.katalog_erzeugt = erzeugt;
    ablage.zustand_schreiben(&z)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::net::TcpListener;

    /// Kleiner Blob-Server mit Range-Unterstützung (oder bewusst ohne).
    async fn server(inhalt: Vec<u8>, mit_range: bool, abbrechen_nach: Option<usize>) -> (Url, Arc<AtomicUsize>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let adresse = l.local_addr().unwrap();
        let anfragen = Arc::new(AtomicUsize::new(0));
        let zaehler = anfragen.clone();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = l.accept().await.unwrap();
                let nr = zaehler.fetch_add(1, Ordering::SeqCst);
                let inhalt = inhalt.clone();
                tokio::spawn(async move {
                    let (r, mut w) = s.split();
                    let mut r = BufReader::new(r);
                    let mut start = 0usize;
                    loop {
                        let mut zeile = String::new();
                        if r.read_line(&mut zeile).await.unwrap() == 0 || zeile == "\r\n" {
                            break;
                        }
                        let klein = zeile.to_ascii_lowercase();
                        if let Some(rest) = klein.strip_prefix("range: bytes=") {
                            start = rest.trim().trim_end_matches('-').parse().unwrap();
                        }
                    }
                    let (status, koerper) = if mit_range && start > 0 {
                        ("206 Partial Content", &inhalt[start..])
                    } else {
                        ("200 OK", &inhalt[..])
                    };
                    let kopf = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        koerper.len()
                    );
                    w.write_all(kopf.as_bytes()).await.unwrap();
                    // erste Anfrage bricht auf Wunsch mittendrin ab
                    let n = match abbrechen_nach {
                        Some(k) if nr == 0 => k.min(koerper.len()),
                        _ => koerper.len(),
                    };
                    let _ = w.write_all(&koerper[..n]).await;
                });
            }
        });
        (Url::parse(&format!("http://{adresse}/daten/blobs/")).unwrap(), anfragen)
    }

    fn datei(inhalt: &[u8]) -> Datei {
        Datei { pfad: "karte/test.pmtiles".into(), bytes: inhalt.len() as u64, sha256: hex::encode(Sha256::digest(inhalt)) }
    }

    fn inhalt() -> Vec<u8> {
        (0..300_000u32).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn blob_adresse() {
        let b = Url::parse("https://x.de/daten/blobs").unwrap();
        let u = blob_url(&b, &"ab".repeat(32)).unwrap();
        assert_eq!(u.as_str(), format!("https://x.de/daten/blobs/sha256/ab/{}", "ab".repeat(32)));
    }

    #[tokio::test]
    async fn laedt_und_prueft() {
        let i = inhalt();
        let (basis, _) = server(i.clone(), true, None).await;
        let d = tempfile::tempdir().unwrap();
        let a = Ablage::neu(d.path());
        let f = datei(&i);
        lade_datei(&reqwest::Client::new(), &[basis], None, &a, &f, &AtomicBool::new(false), &|_| {}).await.unwrap();
        assert_eq!(std::fs::read(d.path().join("karte/test.pmtiles")).unwrap(), i);
        assert!(!a.teil(&f.pfad).unwrap().exists());
    }

    #[tokio::test]
    async fn setzt_abgebrochenen_download_per_range_fort() {
        let i = inhalt();
        let (basis, anfragen) = server(i.clone(), true, Some(100_000)).await;
        let d = tempfile::tempdir().unwrap();
        let a = Ablage::neu(d.path());
        let f = datei(&i);
        let c = reqwest::Client::new();
        // erster Versuch reißt ab → Fehler, Teil bleibt liegen
        assert!(lade_datei(&c, &[basis.clone()], None, &a, &f, &AtomicBool::new(false), &|_| {}).await.is_err());
        let teil_len = std::fs::metadata(a.teil(&f.pfad).unwrap()).unwrap().len();
        assert!(teil_len > 0 && teil_len < i.len() as u64, "Teil: {teil_len}");
        // zweiter Versuch macht weiter, statt neu zu laden
        let erste_meldung = Arc::new(std::sync::Mutex::new(None));
        let em = erste_meldung.clone();
        lade_datei(&c, &[basis], None, &a, &f, &AtomicBool::new(false), &move |n| {
            em.lock().unwrap().get_or_insert(n);
        })
        .await
        .unwrap();
        assert_eq!(*erste_meldung.lock().unwrap(), Some(teil_len));
        assert_eq!(std::fs::read(a.ziel(&f.pfad).unwrap()).unwrap(), i);
        assert_eq!(anfragen.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn server_ohne_range_faengt_neu_an() {
        let i = inhalt();
        let (basis, _) = server(i.clone(), false, None).await;
        let d = tempfile::tempdir().unwrap();
        let a = Ablage::neu(d.path());
        let f = datei(&i);
        std::fs::create_dir_all(d.path().join("karte")).unwrap();
        std::fs::write(a.teil(&f.pfad).unwrap(), &i[..5000]).unwrap();
        lade_datei(&reqwest::Client::new(), &[basis], None, &a, &f, &AtomicBool::new(false), &|_| {}).await.unwrap();
        assert_eq!(std::fs::read(a.ziel(&f.pfad).unwrap()).unwrap(), i);
    }

    #[tokio::test]
    async fn falsche_pruefsumme_verwirft_den_teil() {
        let i = inhalt();
        let (basis, _) = server(i.clone(), true, None).await;
        let d = tempfile::tempdir().unwrap();
        let a = Ablage::neu(d.path());
        let mut f = datei(&i);
        f.sha256 = "0".repeat(64);
        let r = lade_datei(&reqwest::Client::new(), &[basis], None, &a, &f, &AtomicBool::new(false), &|_| {}).await;
        assert!(matches!(r, Err(Fehler::Pruefsumme(_))));
        assert!(!a.teil(&f.pfad).unwrap().exists());
        assert!(!a.ziel(&f.pfad).unwrap().exists());
    }

    #[tokio::test]
    async fn zweite_basis_springt_ein() {
        let i = inhalt();
        let (gut, _) = server(i.clone(), true, None).await;
        let tot = Url::parse("http://127.0.0.1:9/daten/blobs/").unwrap();
        let d = tempfile::tempdir().unwrap();
        let a = Ablage::neu(d.path());
        let f = datei(&i);
        lade_datei(&reqwest::Client::new(), &[tot, gut], None, &a, &f, &AtomicBool::new(false), &|_| {}).await.unwrap();
        assert!(a.ziel(&f.pfad).unwrap().exists());
    }

    #[tokio::test]
    async fn ausfuehren_schreibt_zustand_und_loescht_verwaistes() {
        let i = inhalt();
        let (basis, _) = server(i.clone(), true, None).await;
        let d = tempfile::tempdir().unwrap();
        let a = Arc::new(Ablage::neu(d.path()));
        std::fs::write(d.path().join("alt.bin"), b"x").unwrap();
        let f = datei(&i);
        let plan = Plan { laden: vec![f.clone()], loeschen: vec!["alt.bin".into()], ..Default::default() };
        let meldungen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let m = meldungen.clone();
        ausfuehren(
            reqwest::Client::new(),
            vec![basis],
            None,
            a.clone(),
            plan,
            Default::default(),
            Some("2026-10-26".into()),
            Arc::new(AtomicBool::new(false)),
            Arc::new(move |fs: Fortschritt| m.lock().unwrap().push(fs.stand)),
        )
        .await
        .unwrap();
        let z = a.zustand_lesen();
        assert_eq!(z.dateien.get(&f.pfad), Some(&f.sha256));
        assert_eq!(z.katalog_erzeugt.as_deref(), Some("2026-10-26"));
        assert!(!d.path().join("alt.bin").exists());
        assert_eq!(meldungen.lock().unwrap().last(), Some(&"fertig"));
    }
}
