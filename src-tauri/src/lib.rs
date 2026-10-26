//! LeadLab-Client — Tauri-Kern.
//!
//! Konzept: docs/konzepte/Konzept-Client-und-Datenauslieferung.md im
//! LeadLab-Hauptrepo (§5 Konfigurationsadresse, §6 Katalog, §8 Client).
//!
//! Das Hauptfenster zeigt zuerst den Einrichtungsassistenten (lokale Seite,
//! einzige Seite mit IPC-Rechten). Nach der Einrichtung navigiert es zur
//! Oberfläche des Servers; die entfernte Seite bekommt keine Befehle. Zurück
//! geht es über das Fenstermenü („Daten verwalten“, „Server wechseln“).

pub mod ablage;
pub mod anmeldung;
pub mod einstellungen;
pub mod fehler;
pub mod katalog;
pub mod konfig;
pub mod laden;
pub mod lokal;
pub mod pmtiles;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, State, Url};

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
}

fn anmelde_ansicht(z: &AppZustand, k: &ClientKonfig) -> AnmeldeAnsicht {
    let noetig = k.daten.zugang == konfig::Zugang::Anmeldung;
    let angemeldet = k.anmeldung.as_ref().map(|a| z.sitzung.status(a).angemeldet).unwrap_or(false);
    AnmeldeAnsicht { noetig, angemeldet }
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

#[tauri::command]
fn abmelden(z: State<'_, AppZustand>) -> Result<AnmeldeAnsicht, Fehler> {
    let k = aktive_konfig(&z)?;
    if let Some(a) = &k.anmeldung {
        z.sitzung.abmelden(a)?;
    }
    Ok(anmelde_ansicht(&z, &k))
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
        let r = laden::ausfuehren(client, kat.basis.clone(), token, ablage, plan, auswahl, kat.erzeugt.clone(), abbruch, melde)
            .await;
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
    }
    if e.konfig.is_none() {
        return Err(Fehler::Eingabe("Noch kein Server geprüft.".into()));
    }
    z.lokal.ordner_setzen(ordner.clone());
    e.datenordner = Some(ordner);
    e.eingerichtet = true;
    e.schreiben(&konfig_ordner(&app)?)
}

#[tauri::command]
fn oberflaeche_oeffnen(app: AppHandle, z: State<'_, AppZustand>) -> Result<(), Fehler> {
    let ziel = z
        .einstellungen
        .lock()
        .unwrap()
        .konfig
        .as_ref()
        .map(|k| k.oberflaeche.clone())
        .ok_or_else(|| Fehler::Eingabe("Noch kein Server eingerichtet.".into()))?;
    navigiere(&app, ziel)
}

fn navigiere(app: &AppHandle, ziel: Url) -> Result<(), Fehler> {
    let w = app.get_webview_window("main").ok_or_else(|| Fehler::Eingabe("Hauptfenster fehlt.".into()))?;
    w.navigate(ziel).map_err(|e| Fehler::Eingabe(e.to_string()))
}

fn zurueck_zum_assistenten(app: &AppHandle, schritt: &str) {
    // Ein laufender Download läuft im Rust-Kern weiter; die Assistentenseite
    // zeigt ihn nach der Rückkehr über die Fortschritt-Events wieder an.
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
    tauri::WebviewWindowBuilder::from_config(app.handle(), &cfg)?
        .initialization_script(&lokal::initialisierung(lokal, &ausgenommen))
        .build()?;
    Ok(())
}

fn menue(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let daten = MenuItem::with_id(app, "daten", "Daten verwalten …", true, None::<&str>)?;
    let server = MenuItem::with_id(app, "server", "Server wechseln …", true, None::<&str>)?;
    let oberflaeche = MenuItem::with_id(app, "oberflaeche", "LeadLab öffnen", true, None::<&str>)?;
    let vollbild = MenuItem::with_id(app, "vollbild", "Vollbild ein/aus", true, Some("F11"))?;
    let neu = MenuItem::with_id(app, "neu_laden", "Neu laden", true, Some("F5"))?;
    let beenden = PredefinedMenuItem::quit(app, Some("Beenden"))?;
    let leadlab = Submenu::with_items(
        app,
        "LeadLab",
        true,
        &[&oberflaeche, &neu, &vollbild, &PredefinedMenuItem::separator(app)?, &daten, &server,
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
        .setup(move |app| {
            let einstellungen = konfig_ordner(app.handle()).map(|o| Einstellungen::lesen(&o)).unwrap_or_default();
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
            });
            if let Some(w) = app.get_webview_window("main") {
                *app.state::<AppZustand>().startseite.lock().unwrap() = w.url().ok();
            }
            app.set_menu(menue(app.handle())?)?;
            app.on_menu_event(|app, ev| match ev.id().as_ref() {
                "daten" => zurueck_zum_assistenten(app, "daten"),
                "server" => zurueck_zum_assistenten(app, "server"),
                "oberflaeche" => {
                    let _ = oberflaeche_oeffnen(app.clone(), app.state());
                }
                "neu_laden" => {
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.eval("location.reload()");
                    }
                }
                "vollbild" => {
                    if let Some(w) = app.get_webview_window("main") {
                        let an = w.is_fullscreen().unwrap_or(false);
                        let _ = w.set_fullscreen(!an);
                    }
                }
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
            abmelden
        ])
        .run(tauri::generate_context!())
        .expect("LeadLab-Client konnte nicht starten");
}
