//! LeadLab-Client — Tauri-Kern.
//!
//! Konzept: docs/konzepte/Konzept-Client-und-Datenauslieferung.md im
//! LeadLab-Hauptrepo (§5 Konfigurationsadresse, §6 Katalog, §8 Client).
//!
//! Das Hauptfenster zeigt zuerst den Einrichtungsassistenten (lokale Seite,
//! einzige Seite mit IPC-Rechten). Nach der Einrichtung navigiert es zur
//! Oberfläche des Servers; die entfernte Seite bekommt keine Befehle. Zurück
//! geht es über das Fenstermenü („Daten verwalten“, „Server wechseln“).
//!
//! Vollbild (#791): Der Assistent läuft im Fenster; erst der Wechsel zur
//! Oberfläche schaltet — je nach Einstellung oder `--vollbild`/`--fenster` —
//! ins Vollbild ohne Menüleiste, auf `--monitor N` oder dem Monitor mit dem
//! Mauszeiger. Ausstieg über F11/Esc und den Schwebeknopf (vollbild.rs).

pub mod ablage;
pub mod aktualisierung;
pub mod anmeldung;
pub mod einstellungen;
pub mod fehler;
pub mod katalog;
pub mod konfig;
pub mod laden;
pub mod lokal;
pub mod pmtiles;
pub mod uebersicht;
pub mod vollbild;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, State, Url};

use ablage::Ablage;
use einstellungen::Einstellungen;
use fehler::Fehler;
use katalog::{Auswahl, Katalog, Plan};
use konfig::ClientKonfig;

struct AppZustand {
    client: reqwest::Client,
    einstellungen: Mutex<Einstellungen>,
    /// Im Assistenten geprüft, aber noch nicht übernommen.
    geprueft: Mutex<Option<(String, ClientKonfig)>>,
    katalog: Mutex<Option<Katalog>>,
    abbruch: Arc<AtomicBool>,
    laeuft: Arc<AtomicBool>,
    /// Adresse der lokalen Assistentenseite (für den Rückweg vom Server).
    startseite: Mutex<Option<Url>>,
    /// Anmeldung für die Datenauslieferung (CS4): Access-Token im Speicher,
    /// Refresh-Token im Schlüsselbund.
    sitzung: Arc<anmeldung::Sitzung>,
    /// Loopback-Server für lokale Pakete (CS7).
    lokal: lokal::Lokal,
    /// Zuletzt gefundenes Update (CS9), bis der Nutzer es installiert.
    update: Mutex<Option<tauri_plugin_updater::Update>>,
    /// Kommandozeile (#791): `--vollbild`, `--fenster`, `--monitor N`.
    argumente: vollbild::Argumente,
    /// Letztes Umschalten — F11 kann Menü-Accelerator und Seitentaste
    /// zugleich auslösen; das zweite Signal fällt dann weg.
    vollbild_seit: Mutex<Option<Instant>>,
    /// Ergebnis der letzten Update-Prüfung (für die Einstellungsseite).
    update_info: Mutex<Option<aktualisierung::UpdateInfo>>,
    /// Letztes Laden einer Server-Seite: (Adresse, Unix-Sekunden). Dort hat
    /// das Initialisierungsskript `window.__LEADLAB_CLIENT__` gesetzt.
    zuletzt_geladen: Mutex<Option<(String, u64)>>,
}

fn jetzt() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn konfig_ordner(app: &AppHandle) -> Result<PathBuf, Fehler> {
    app.path().app_config_dir().map_err(|e| Fehler::Eingabe(e.to_string()))
}

fn standard_datenordner(app: &AppHandle) -> PathBuf {
    app.path()
        .app_local_data_dir()
        .map(|p| p.join("daten"))
        .unwrap_or_else(|_| PathBuf::from("leadlab-daten"))
}

fn aktive_konfig(z: &AppZustand) -> Result<ClientKonfig, Fehler> {
    if let Some((_, k)) = z.geprueft.lock().unwrap().clone() {
        return Ok(k);
    }
    z.einstellungen
        .lock()
        .unwrap()
        .konfig
        .clone()
        .ok_or_else(|| Fehler::Eingabe("Noch kein Server eingerichtet.".into()))
}

#[derive(Serialize)]
struct StartInfo {
    einstellungen: Einstellungen,
    standard_datenordner: PathBuf,
    version: &'static str,
    plattform: &'static str,
}

#[tauri::command]
fn start_info(app: AppHandle, z: State<'_, AppZustand>) -> StartInfo {
    StartInfo {
        einstellungen: z.einstellungen.lock().unwrap().clone(),
        standard_datenordner: standard_datenordner(&app),
        version: env!("CARGO_PKG_VERSION"),
        plattform: std::env::consts::OS,
    }
}

#[tauri::command]
async fn konfig_pruefen(eingabe: String, z: State<'_, AppZustand>) -> Result<ClientKonfig, Fehler> {
    let k = konfig::lade(&z.client, &eingabe).await?;
    *z.geprueft.lock().unwrap() = Some((eingabe.trim().to_string(), k.clone()));
    *z.katalog.lock().unwrap() = None;
    Ok(k)
}

/// Token für Katalog und Pakete: bei `zugang = anmeldung` Pflicht.
async fn daten_token(z: &AppZustand, k: &ClientKonfig) -> Result<Option<String>, Fehler> {
    if k.daten.zugang != konfig::Zugang::Anmeldung {
        return Ok(None);
    }
    let a = k.anmeldung.as_ref().ok_or_else(|| Fehler::Server("Der Server nennt keinen Anmeldedienst.".into()))?;
    match z.sitzung.token(&z.client, a).await? {
        Some(t) => Ok(Some(t)),
        None => Err(Fehler::Eingabe("Für die Daten dieses Servers bitte zuerst anmelden.".into())),
    }
}

#[derive(Serialize)]
struct AnmeldeAnsicht {
    /// Der Server verlangt eine Anmeldung für die Daten.
    noetig: bool,
    angemeldet: bool,
    /// Kontoname aus dem ID-Token, sobald in diesem Lauf ein Token vorlag.
    konto: Option<String>,
}

fn anmelde_ansicht(z: &AppZustand, k: &ClientKonfig) -> AnmeldeAnsicht {
    let noetig = k.daten.zugang == konfig::Zugang::Anmeldung;
    let s = k.anmeldung.as_ref().map(|a| z.sitzung.status(a));
    AnmeldeAnsicht {
        noetig,
        angemeldet: s.as_ref().map(|s| s.angemeldet).unwrap_or(false),
        konto: s.and_then(|s| s.konto),
    }
}

#[tauri::command]
fn anmeldung_status(z: State<'_, AppZustand>) -> Result<AnmeldeAnsicht, Fehler> {
    let k = aktive_konfig(&z)?;
    Ok(anmelde_ansicht(&z, &k))
}

/// Öffnet den Systembrowser zur Anmeldung und wartet auf den Rückruf.
#[tauri::command]
async fn anmelden(z: State<'_, AppZustand>) -> Result<AnmeldeAnsicht, Fehler> {
    let k = aktive_konfig(&z)?;
    let a = k.anmeldung.clone().ok_or_else(|| Fehler::Server("Der Server nennt keinen Anmeldedienst.".into()))?;
    let oeffne = |u: &Url| -> Result<(), Fehler> {
        open::that_detached(u.as_str()).map_err(|e| Fehler::Eingabe(format!("Browser lässt sich nicht öffnen ({e}).")))
    };
    z.sitzung.anmelden(&z.client, &a, &oeffne, anmeldung::FRIST).await?;
    Ok(anmelde_ansicht(&z, &k))
}

/// Holt ein Token (per Refresh-Token), damit der Kontoname bekannt ist.
/// Ein Netzfehler ändert nichts am Status.
#[tauri::command]
async fn anmeldung_pruefen(z: State<'_, AppZustand>) -> Result<AnmeldeAnsicht, Fehler> {
    let k = aktive_konfig(&z)?;
    if let Some(a) = &k.anmeldung {
        if z.sitzung.status(a).angemeldet {
            let _ = z.sitzung.token(&z.client, a).await;
        }
    }
    Ok(anmelde_ansicht(&z, &k))
}

#[tauri::command]
fn abmelden(z: State<'_, AppZustand>) -> Result<AnmeldeAnsicht, Fehler> {
    let k = aktive_konfig(&z)?;
    if let Some(a) = &k.anmeldung {
        z.sitzung.abmelden(a)?;
    }
    Ok(anmelde_ansicht(&z, &k))
}

/// Fragt das Update-Manifest des Servers (`client_update`) ab.
#[tauri::command]
async fn update_pruefen(app: AppHandle, z: State<'_, AppZustand>) -> Result<aktualisierung::UpdateInfo, Fehler> {
    let k = aktive_konfig(&z)?;
    let (info, update) = aktualisierung::pruefen(&app, k.client_update.as_ref()).await?;
    *z.update.lock().unwrap() = update;
    *z.update_info.lock().unwrap() = Some(info.clone());
    Ok(info)
}

/// Lädt das gefundene Update (Signatur prüft das Plugin), installiert und
/// startet neu.
#[tauri::command]
async fn update_installieren(app: AppHandle, z: State<'_, AppZustand>) -> Result<(), Fehler> {
    let update = z.update.lock().unwrap().take().ok_or_else(|| Fehler::Eingabe("Kein Update gefunden.".into()))?;
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| Fehler::Netz(format!("Update nicht installiert: {e}")))?;
    app.restart();
}

#[derive(Serialize)]
struct KatalogAnsicht {
    katalog: Katalog,
    /// Aus der Ablage: bisherige Auswahl und Bestand.
    auswahl: Auswahl,
    vorhanden: BTreeMap<String, String>,
}

#[tauri::command]
async fn katalog_laden(ordner: PathBuf, z: State<'_, AppZustand>) -> Result<KatalogAnsicht, Fehler> {
    let k = aktive_konfig(&z)?;
    let token = daten_token(&z, &k).await?;
    let kat = katalog::lade(&z.client, &k.daten.katalog, token.as_deref()).await?;
    *z.katalog.lock().unwrap() = Some(kat.clone());
    let a = Ablage::neu(&ordner);
    Ok(KatalogAnsicht { katalog: kat, auswahl: a.zustand_lesen().auswahl, vorhanden: a.vorhanden() })
}

#[derive(Serialize)]
struct PlanAnsicht {
    plan: Plan,
    frei: Option<u64>,
}

#[tauri::command]
fn plan_berechnen(auswahl: Auswahl, ordner: PathBuf, z: State<'_, AppZustand>) -> Result<PlanAnsicht, Fehler> {
    let kat = z.katalog.lock().unwrap().clone().ok_or_else(|| Fehler::Eingabe("Katalog nicht geladen.".into()))?;
    let a = Ablage::neu(&ordner);
    Ok(PlanAnsicht { plan: katalog::plan(&kat, &auswahl, &a.vorhanden()), frei: ablage::freier_speicher(&ordner) })
}

#[derive(Serialize, Clone)]
struct LadeEnde {
    ok: bool,
    meldung: Option<String>,
}

#[tauri::command]
fn laden_starten(app: AppHandle, auswahl: Auswahl, ordner: PathBuf, z: State<'_, AppZustand>) -> Result<(), Fehler> {
    if z.laeuft.swap(true, Ordering::SeqCst) {
        return Err(Fehler::Eingabe("Es läuft bereits ein Download.".into()));
    }
    let kat = match z.katalog.lock().unwrap().clone() {
        Some(k) => k,
        None => {
            z.laeuft.store(false, Ordering::SeqCst);
            return Err(Fehler::Eingabe("Katalog nicht geladen.".into()));
        }
    };
    let ablage = Arc::new(Ablage::neu(&ordner));
    let plan = katalog::plan(&kat, &auswahl, &ablage.vorhanden());
    // Für die Einstellungsseite (#792): welche Pakete danach lokal liegen.
    let ids = plan.pakete.clone();
    let geladen: BTreeSet<String> = plan.laden.iter().map(|d| d.pfad.clone()).collect();
    let kat_stand = kat.clone();
    // Vor jeder Datei ein frisches Token (Access-Token leben Minuten).
    let token: Option<laden::TokenQuelle> = match aktive_konfig(&z) {
        Ok(k) if k.daten.zugang == konfig::Zugang::Anmeldung => {
            let (client, sitzung) = (z.client.clone(), z.sitzung.clone());
            let a = k.anmeldung.clone();
            Some(Arc::new(move || {
                let (client, sitzung, a) = (client.clone(), sitzung.clone(), a.clone());
                Box::pin(async move {
                    let a = a.ok_or_else(|| Fehler::Server("Der Server nennt keinen Anmeldedienst.".into()))?;
                    match sitzung.token(&client, &a).await? {
                        Some(t) => Ok(Some(t)),
                        None => Err(Fehler::Eingabe("Anmeldung abgelaufen. Bitte neu anmelden.".into())),
                    }
                })
            }))
        }
        _ => None,
    };
    z.abbruch.store(false, Ordering::SeqCst);
    let (client, abbruch, laeuft) = (z.client.clone(), z.abbruch.clone(), z.laeuft.clone());
    let melder = app.clone();
    tauri::async_runtime::spawn(async move {
        let melde = Arc::new(move |f: laden::Fortschritt| {
            let _ = melder.emit("laden-fortschritt", f);
        });
        let r = laden::ausfuehren(client, kat.basis.clone(), token, ablage.clone(), plan, auswahl, kat.erzeugt.clone(), abbruch, melde)
            .await;
        let r = r.and_then(|()| {
            let mut zst = ablage.zustand_lesen();
            zst.pakete = uebersicht::stand_nach_laden(&zst.pakete, &kat_stand, &ids, &geladen, jetzt());
            ablage.zustand_schreiben(&zst)
        });
        laeuft.store(false, Ordering::SeqCst);
        let ende = match r {
            Ok(()) => LadeEnde { ok: true, meldung: None },
            Err(e) => LadeEnde { ok: false, meldung: Some(e.to_string()) },
        };
        let _ = app.emit("laden-ende", ende);
    });
    Ok(())
}

#[tauri::command]
fn laden_abbrechen(z: State<'_, AppZustand>) {
    z.abbruch.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn freier_speicher(ordner: PathBuf) -> Option<u64> {
    ablage::freier_speicher(&ordner)
}

/// Übernimmt geprüften Server und Datenordner und schreibt die Einstellungen.
#[tauri::command]
fn einrichtung_speichern(app: AppHandle, ordner: PathBuf, z: State<'_, AppZustand>) -> Result<(), Fehler> {
    let mut e = z.einstellungen.lock().unwrap();
    if let Some((eingabe, k)) = z.geprueft.lock().unwrap().take() {
        e.konfig_adresse = Some(eingabe);
        e.konfig = Some(k);
        e.konfig_geprueft = Some(jetzt());
    }
    if e.konfig.is_none() {
        return Err(Fehler::Eingabe("Noch kein Server geprüft.".into()));
    }
    z.lokal.ordner_setzen(ordner.clone());
    e.datenordner = Some(ordner);
    e.eingerichtet = true;
    e.schreiben(&konfig_ordner(&app)?)
}

// ── Konfigurationsmodus (#792) ─────────────────────────────────────────────

#[derive(Serialize)]
struct LokalAnsicht {
    /// `http://127.0.0.1:<port>` ohne Pfad-Token.
    adresse: String,
    status: String,
    pakete: usize,
    /// Ausgelieferte Kacheln/LoD2-Antworten seit dem Start.
    geliefert: u64,
    /// Letztes Laden einer Server-Seite mit `window.__LEADLAB_CLIENT__`.
    zuletzt_geladen: Option<String>,
    zuletzt_geladen_um: Option<u64>,
}

#[derive(Serialize)]
struct ProgrammAnsicht {
    version: &'static str,
    plattform: &'static str,
    update: Option<aktualisierung::UpdateInfo>,
    vollbild_einstellung: bool,
    argumente: vollbild::Argumente,
    einstellungen_datei: Option<PathBuf>,
    zustand_datei: PathBuf,
}

#[derive(Serialize)]
struct Uebersicht {
    einstellungen: Einstellungen,
    anmeldung: Option<AnmeldeAnsicht>,
    datenordner: PathBuf,
    frei: Option<u64>,
    belegt: u64,
    pakete: Vec<uebersicht::PaketZeile>,
    auswahl: Auswahl,
    katalog_erzeugt: Option<String>,
    lokal: LokalAnsicht,
    programm: ProgrammAnsicht,
    /// Ein Download läuft — Entfernen ist dann gesperrt.
    laeuft: bool,
}

fn datenordner(app: &AppHandle, z: &AppZustand) -> PathBuf {
    z.einstellungen.lock().unwrap().datenordner.clone().unwrap_or_else(|| standard_datenordner(app))
}

/// Alles, was die Einstellungsseite zeigt, in einer Struktur — ohne Netz.
#[tauri::command]
fn zustand_uebersicht(app: AppHandle, z: State<'_, AppZustand>) -> Uebersicht {
    let einstellungen = z.einstellungen.lock().unwrap().clone();
    let ordner = datenordner(&app, &z);
    let a = Ablage::neu(&ordner);
    let zst = a.zustand_lesen();
    let vorhanden = a.vorhanden();
    let lokale = z.lokal.pakete();
    let zoom: BTreeMap<String, [u8; 2]> = lokale.iter().filter_map(|p| p.zoom.map(|b| (p.pfad.clone(), b))).collect();
    let groesse = |f: &str| a.ziel(f).ok().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len()).unwrap_or(0);
    let pakete = uebersicht::zeilen(&zst, &vorhanden, &zoom, &groesse);
    let belegt = vorhanden.keys().map(|f| groesse(f)).sum();
    let anmeldung = einstellungen.konfig.as_ref().map(|k| anmelde_ansicht(&z, k));
    let geladen = z.zuletzt_geladen.lock().unwrap().clone();
    Uebersicht {
        anmeldung,
        frei: ablage::freier_speicher(&ordner),
        belegt,
        pakete,
        auswahl: zst.auswahl.clone(),
        katalog_erzeugt: zst.katalog_erzeugt.clone(),
        lokal: LokalAnsicht {
            adresse: format!("http://127.0.0.1:{}", z.lokal.port),
            status: format!("{}/status", z.lokal.url()),
            pakete: lokale.len(),
            geliefert: z.lokal.geliefert(),
            zuletzt_geladen: geladen.as_ref().map(|g| g.0.clone()),
            zuletzt_geladen_um: geladen.map(|g| g.1),
        },
        programm: ProgrammAnsicht {
            version: env!("CARGO_PKG_VERSION"),
            plattform: std::env::consts::OS,
            update: z.update_info.lock().unwrap().clone(),
            vollbild_einstellung: einstellungen.vollbild,
            argumente: z.argumente.clone(),
            einstellungen_datei: konfig_ordner(&app).ok().map(|o| o.join(einstellungen::DATEI)),
            zustand_datei: ordner.join(ablage::ZUSTAND),
        },
        laeuft: z.laeuft.load(Ordering::SeqCst),
        datenordner: ordner,
        einstellungen,
    }
}

/// Die gespeicherte Konfigurationsadresse neu lesen und übernehmen.
#[tauri::command]
async fn konfig_neu_pruefen(app: AppHandle, z: State<'_, AppZustand>) -> Result<ClientKonfig, Fehler> {
    let adresse = z
        .einstellungen
        .lock()
        .unwrap()
        .konfig_adresse
        .clone()
        .ok_or_else(|| Fehler::Eingabe("Noch kein Server eingerichtet.".into()))?;
    let k = konfig::lade(&z.client, &adresse).await?;
    let mut e = z.einstellungen.lock().unwrap();
    e.konfig = Some(k.clone());
    e.konfig_geprueft = Some(jetzt());
    e.schreiben(&konfig_ordner(&app)?)?;
    *z.katalog.lock().unwrap() = None;
    Ok(k)
}

/// Ein lokales Paket löschen (Dateien und Eintrag in `zustand.json`).
#[tauri::command]
fn paket_entfernen(app: AppHandle, id: String, z: State<'_, AppZustand>) -> Result<(), Fehler> {
    if z.laeuft.load(Ordering::SeqCst) {
        return Err(Fehler::Eingabe("Während eines Downloads lässt sich kein Paket entfernen.".into()));
    }
    let ordner = datenordner(&app, &z);
    let a = Ablage::neu(&ordner);
    let (neu, weg) = uebersicht::entfernen(&a.zustand_lesen(), &id)?;
    // Erst den Zustand schreiben: eine Datei ohne Eintrag gilt als nicht da.
    a.zustand_schreiben(&neu)?;
    // Offene PMTiles des Loopback-Servers loslassen (Windows sperrt sie sonst).
    z.lokal.ordner_setzen(ordner);
    for f in &weg {
        a.loeschen(f)?;
    }
    Ok(())
}

/// Daten- oder Einstellungsordner im Dateimanager des Systems öffnen.
#[tauri::command]
fn ordner_oeffnen(app: AppHandle, welcher: String, z: State<'_, AppZustand>) -> Result<(), Fehler> {
    let ordner = match welcher.as_str() {
        "daten" => datenordner(&app, &z),
        "einstellungen" => konfig_ordner(&app)?,
        _ => return Err(Fehler::Eingabe("Unbekannter Ordner.".into())),
    };
    std::fs::create_dir_all(&ordner)?;
    open::that_detached(&ordner).map_err(|e| Fehler::Eingabe(format!("Ordner lässt sich nicht öffnen ({e}).")))
}

#[tauri::command]
fn oberflaeche_oeffnen(app: AppHandle, z: State<'_, AppZustand>) -> Result<(), Fehler> {
    let (ziel, einstellung) = {
        let e = z.einstellungen.lock().unwrap();
        let ziel = e
            .konfig
            .as_ref()
            .map(|k| k.oberflaeche.clone())
            .ok_or_else(|| Fehler::Eingabe("Noch kein Server eingerichtet.".into()))?;
        (ziel, e.vollbild)
    };
    // Nur der Übergang Assistent → Oberfläche schaltet ins Vollbild; wer auf
    // der Oberfläche „LeadLab öffnen“ wählt, behält seinen Fensterzustand.
    let vom_assistenten = auf_startseite(&app, &z);
    navigiere(&app, ziel)?;
    if vom_assistenten && vollbild::im_vollbild(&z.argumente, einstellung) {
        vollbild_setzen(&app, true);
    }
    Ok(())
}

/// Zeigt das Hauptfenster gerade die lokale Assistentenseite?
fn auf_startseite(app: &AppHandle, z: &AppZustand) -> bool {
    let start = z.startseite.lock().unwrap().clone();
    let jetzt = app.get_webview_window("main").and_then(|w| w.url().ok());
    match (start, jetzt) {
        (Some(s), Some(j)) => s.origin() == j.origin(),
        _ => true,
    }
}

/// Vollbild ein (auf dem gewählten Monitor, ohne Menüleiste) oder aus (mit
/// Menüleiste). Die Seite erfährt den Zustand für Schwebeknopf und Tasten.
fn vollbild_setzen(app: &AppHandle, an: bool) {
    let Some(w) = app.get_webview_window("main") else { return };
    let z = app.state::<AppZustand>();
    *z.vollbild_seit.lock().unwrap() = Some(Instant::now());
    if an {
        let schirme = w.available_monitors().unwrap_or_default();
        // Unter Wayland kennt der Client die Zeigerposition oft nicht; dann
        // bleibt es beim ersten Monitor bzw. dem des Fensters (#165).
        let zeiger = w.cursor_position().ok().and_then(|p| w.monitor_from_point(p.x, p.y).ok().flatten());
        match vollbild::waehle_monitor(&schirme, z.argumente.monitor, zeiger) {
            Some(m) => {
                let p = m.position();
                let _ = w.set_fullscreen_on_monitor(PhysicalPosition::new(p.x as f64, p.y as f64));
            }
            None => {
                let _ = w.set_fullscreen(true);
            }
        }
        let _ = w.hide_menu();
    } else {
        let _ = w.set_fullscreen(false);
        let _ = w.show_menu();
    }
    let _ = w.eval(&vollbild::zustand_skript(an));
}

/// F11 aus dem Menü: umschalten, außer es wurde gerade erst umgeschaltet.
fn vollbild_umschalten(app: &AppHandle) {
    let z = app.state::<AppZustand>();
    let frisch = z.vollbild_seit.lock().unwrap().map(|t| t.elapsed() < Duration::from_millis(400)).unwrap_or(false);
    if frisch {
        return;
    }
    if let Some(w) = app.get_webview_window("main") {
        let an = w.is_fullscreen().unwrap_or(false);
        vollbild_setzen(app, !an);
    }
}

/// Menü „Beim Start im Vollbild“: Einstellung umdrehen und speichern. Das
/// Häkchen schaltet das Menü selbst um.
fn vollbild_einstellung_umschalten(app: &AppHandle) {
    let z = app.state::<AppZustand>();
    let mut e = z.einstellungen.lock().unwrap();
    e.vollbild = !e.vollbild;
    if let Ok(o) = konfig_ordner(app) {
        let _ = e.schreiben(&o);
    }
}

fn navigiere(app: &AppHandle, ziel: Url) -> Result<(), Fehler> {
    let w = app.get_webview_window("main").ok_or_else(|| Fehler::Eingabe("Hauptfenster fehlt.".into()))?;
    w.navigate(ziel).map_err(|e| Fehler::Eingabe(e.to_string()))
}

fn zurueck_zum_assistenten(app: &AppHandle, schritt: &str) {
    // Ein laufender Download läuft im Rust-Kern weiter; die Assistentenseite
    // zeigt ihn nach der Rückkehr über die Fortschritt-Events wieder an.
    // Der Assistent läuft im Fenster (#791).
    if app.get_webview_window("main").and_then(|w| w.is_fullscreen().ok()).unwrap_or(false) {
        vollbild_setzen(app, false);
    }
    let z = app.state::<AppZustand>();
    let start = z.startseite.lock().unwrap().clone();
    if let Some(mut start) = start {
        start.set_fragment(Some(schritt));
        let _ = navigiere(app, start);
    }
}

/// Hauptfenster aus tauri.conf.json (dort `create: false`), mit dem
/// Initialisierungsskript `window.__LEADLAB_CLIENT__` für die
/// Server-Oberfläche. Die lokale Assistentenseite bleibt ausgenommen; IPC hat
/// die entfernte Seite ohnehin nicht (capabilities ohne `remote`).
fn hauptfenster(app: &tauri::App, lokal: &lokal::Lokal) -> tauri::Result<()> {
    let cfg = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")
        .cloned()
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    let mut ausgenommen = vec![
        "tauri://localhost".to_string(),
        "http://tauri.localhost".to_string(),
        "https://tauri.localhost".to_string(),
    ];
    if let Some(dev) = &app.config().build.dev_url {
        ausgenommen.push(dev.origin().ascii_serialization());
    }
    let handle = app.handle().clone();
    let lokale_seiten = ausgenommen.clone();
    tauri::WebviewWindowBuilder::from_config(app.handle(), &cfg)?
        .initialization_script(&lokal::initialisierung(lokal, &ausgenommen))
        .initialization_script(&vollbild::skript(&ausgenommen))
        // Ausstieg aus dem Vollbild per Pseudo-Adresse: Navigation abbrechen,
        // Umschalten außerhalb des Navigationsrückrufs.
        .on_navigation(move |u| {
            if vollbild::ist_ausstieg(u) {
                let h = handle.clone();
                tauri::async_runtime::spawn(async move { vollbild_setzen(&h, false) });
                return false;
            }
            true
        })
        // Nach jedem Laden erfährt die Seite, ob sie im Vollbild steht.
        .on_page_load(move |w, p| {
            if p.event() == PageLoadEvent::Finished {
                let an = w.is_fullscreen().unwrap_or(false);
                let _ = w.eval(&vollbild::zustand_skript(an));
                // Für die Einstellungsseite (#792): wann die Oberfläche mit
                // `__LEADLAB_CLIENT__` zuletzt geladen wurde.
                let herkunft = p.url().origin().ascii_serialization();
                if !lokale_seiten.contains(&herkunft) && p.url().scheme() != "about" {
                    if let Some(z) = w.try_state::<AppZustand>() {
                        *z.zuletzt_geladen.lock().unwrap() = Some((herkunft, jetzt()));
                    }
                }
            }
        })
        .build()?;
    Ok(())
}

fn menue(app: &AppHandle, vollbild_start: bool) -> tauri::Result<Menu<tauri::Wry>> {
    let daten = MenuItem::with_id(app, "daten", "Daten verwalten …", true, None::<&str>)?;
    let einstellungen = MenuItem::with_id(app, "einstellungen", "Einstellungen …", true, Some("CmdOrCtrl+,"))?;
    let server = MenuItem::with_id(app, "server", "Server wechseln …", true, None::<&str>)?;
    let oberflaeche = MenuItem::with_id(app, "oberflaeche", "LeadLab öffnen", true, None::<&str>)?;
    let vollbild = MenuItem::with_id(app, "vollbild", "Vollbild ein/aus", true, Some("F11"))?;
    let beim_start =
        CheckMenuItem::with_id(app, "vollbild_start", "Beim Start im Vollbild", true, vollbild_start, None::<&str>)?;
    let neu = MenuItem::with_id(app, "neu_laden", "Neu laden", true, Some("F5"))?;
    let beenden = PredefinedMenuItem::quit(app, Some("Beenden"))?;
    let leadlab = Submenu::with_items(
        app,
        "LeadLab",
        true,
        &[&oberflaeche, &neu, &vollbild, &beim_start, &PredefinedMenuItem::separator(app)?, &einstellungen, &daten, &server,
          &PredefinedMenuItem::separator(app)?, &beenden],
    )?;
    Menu::with_items(app, &[&leadlab])
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let client = reqwest::Client::builder()
        .user_agent(concat!("LeadLab-Client/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .expect("HTTP-Client");

    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            let einstellungen = konfig_ordner(app.handle()).map(|o| Einstellungen::lesen(&o)).unwrap_or_default();
            let vollbild_start = einstellungen.vollbild;
            // Loopback-Server zuerst: sein Token steht im Initialisierungsskript
            // des Hauptfensters (Konzept §8.3).
            let ordner = einstellungen.datenordner.clone().unwrap_or_else(|| standard_datenordner(app.handle()));
            let lokal = tauri::async_runtime::block_on(lokal::Lokal::starten(ordner))?;
            hauptfenster(app, &lokal)?;
            app.manage(AppZustand {
                client: client.clone(),
                einstellungen: Mutex::new(einstellungen),
                geprueft: Mutex::new(None),
                katalog: Mutex::new(None),
                abbruch: Arc::new(AtomicBool::new(false)),
                laeuft: Arc::new(AtomicBool::new(false)),
                startseite: Mutex::new(None),
                sitzung: Arc::new(anmeldung::Sitzung::neu(Arc::new(anmeldung::Schluesselbund))),
                lokal,
                update: Mutex::new(None),
                argumente: vollbild::argumente(std::env::args().skip(1)),
                vollbild_seit: Mutex::new(None),
                update_info: Mutex::new(None),
                zuletzt_geladen: Mutex::new(None),
            });
            if let Some(w) = app.get_webview_window("main") {
                *app.state::<AppZustand>().startseite.lock().unwrap() = w.url().ok();
            }
            app.set_menu(menue(app.handle(), vollbild_start)?)?;
            app.on_menu_event(|app, ev| match ev.id().as_ref() {
                "daten" => zurueck_zum_assistenten(app, "daten"),
                "einstellungen" => zurueck_zum_assistenten(app, "einstellungen"),
                "server" => zurueck_zum_assistenten(app, "server"),
                "oberflaeche" => {
                    let _ = oberflaeche_oeffnen(app.clone(), app.state());
                }
                "neu_laden" => {
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.eval("location.reload()");
                    }
                }
                "vollbild" => vollbild_umschalten(app),
                "vollbild_start" => vollbild_einstellung_umschalten(app),
                _ => {}
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_info,
            konfig_pruefen,
            katalog_laden,
            plan_berechnen,
            laden_starten,
            laden_abbrechen,
            freier_speicher,
            einrichtung_speichern,
            oberflaeche_oeffnen,
            anmeldung_status,
            anmelden,
            abmelden,
            update_pruefen,
            update_installieren,
            anmeldung_pruefen,
            zustand_uebersicht,
            konfig_neu_pruefen,
            paket_entfernen,
            ordner_oeffnen
        ])
        .run(tauri::generate_context!())
        .expect("LeadLab-Client konnte nicht starten");
}
