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
//!
//! Ein Login für alles (AD4, #800): Vor dem Wechsel zur Oberfläche holt der
//! Kern aus der Anmeldung des Clients eine BFF-Sitzung und setzt ihr Cookie
//! ins Fenster (oberflaeche.rs), verlängert sie im Hintergrund und fängt
//! Login-Umleitungen der Oberfläche ab.

pub mod ablage;
pub mod aktualisierung;
pub mod anmeldung;
#[cfg(target_os = "linux")]
pub mod eingabemethode;
pub mod einstellungen;
pub mod fehler;
pub mod katalog;
pub mod konfig;
pub mod laden;
pub mod lokal;
pub mod oberflaeche;
pub mod pmtiles;
pub mod tresor;
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
    /// Anmeldedienst (CS4, AD2): alle Tokens im Tresor des Konfigurationsordners.
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
    /// Sitzung der Oberfläche aus der Anmeldung des Clients (AD4).
    bff: Mutex<Option<oberflaeche::BffSitzung>>,
    /// Die Verlängerung im Hintergrund läuft.
    verlaengerung: AtomicBool,
    /// Zuletzt abgefangene Login-Umleitungen (gegen Kreisläufe).
    umleitungen: Mutex<Vec<Instant>>,
    /// Der Server hat die Sitzung beendet (Abmelden in der Oberfläche): die
    /// nächste Login-Umleitung führt in den Assistenten.
    serverseitig_beendet: AtomicBool,
    /// Laufende Browser-Anmeldung (#805): Adresse, Abbruch, Verbindung.
    warten: Mutex<Option<Arc<anmeldung::Warten>>>,
    /// Die Anmeldung läuft im Client-Fenster: Assistentenschritt, in den es
    /// danach zurückgeht (#805).
    im_fenster: Mutex<Option<String>>,
    /// Ergebnis einer Anmeldung im Client-Fenster, das die neu geladene
    /// Assistentenseite einmal abholt.
    anmelde_meldung: Mutex<Option<String>>,
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
    /// Kontoname aus dem ID-Token.
    konto: Option<String>,
    /// Offline-Token: Die Anmeldung hält wochenlang (AD2).
    offline: bool,
    angemeldet_seit: Option<u64>,
    refresh_bis: Option<u64>,
    /// Ablageort des Tresors (Konfigurationsordner).
    tresor: PathBuf,
    /// Sitzung der Oberfläche im Fenster läuft bis (Unix-Sekunden, AD4).
    oberflaeche_bis: Option<u64>,
    /// Eine Browser-Anmeldung läuft (#805).
    warten: Option<WarteAnsicht>,
    /// Fehler einer Anmeldung im Client-Fenster (nur in `anmeldung_status`).
    meldung: Option<String>,
    /// Der Server erlaubt die Anmeldemaske (`anmeldung.formular`, #804).
    formular: bool,
}

/// Spiegelt `anmeldung::Warten` für die Ansicht (#805).
#[derive(Serialize)]
struct WarteAnsicht {
    adresse: Option<String>,
    seit: u64,
    /// Ein Browser hat sich am Loopback gemeldet.
    verbunden: bool,
    /// Startfehler des Browsers.
    browser_fehler: Option<String>,
    /// Die Anmeldung läuft im Client-Fenster.
    im_fenster: bool,
    /// Ab so vielen Sekunden ohne Verbindung nennt die Ansicht die Ausweichwege.
    still_sek: u64,
}

fn anmelde_ansicht(z: &AppZustand, k: &ClientKonfig) -> AnmeldeAnsicht {
    let noetig = k.daten.zugang == konfig::Zugang::Anmeldung;
    let s = k.anmeldung.as_ref().map(|a| z.sitzung.status(a)).unwrap_or_default();
    AnmeldeAnsicht {
        noetig,
        angemeldet: s.angemeldet,
        konto: s.konto,
        offline: s.offline,
        angemeldet_seit: s.angemeldet_seit,
        refresh_bis: s.refresh_bis,
        tresor: z.sitzung.tresor_pfad(),
        oberflaeche_bis: z.bff.lock().unwrap().as_ref().map(|b| b.laeuft_bis),
        warten: z.warten.lock().unwrap().as_ref().map(|w| WarteAnsicht {
            adresse: w.adresse().map(|u| u.to_string()),
            seit: w.seit(),
            verbunden: w.verbunden(),
            browser_fehler: w.browser_fehler(),
            im_fenster: z.im_fenster.lock().unwrap().is_some(),
            still_sek: anmeldung::STILL_SEK,
        }),
        meldung: None,
        formular: k.anmeldung.as_ref().map(|a| a.formular).unwrap_or(false),
    }
}

/// Stand der Anmeldung; holt einmal das Ergebnis einer Anmeldung im
/// Client-Fenster ab (die Seite war währenddessen nicht geladen).
#[tauri::command]
fn anmeldung_status(z: State<'_, AppZustand>) -> Result<AnmeldeAnsicht, Fehler> {
    let k = aktive_konfig(&z)?;
    let mut a = anmelde_ansicht(&z, &k);
    a.meldung = z.anmelde_meldung.lock().unwrap().take();
    Ok(a)
}

fn browser_oeffnen(u: &Url) -> Result<(), Fehler> {
    open::that_detached(u.as_str()).map_err(|e| Fehler::Eingabe(format!("Browser lässt sich nicht öffnen ({e}).")))
}

/// Öffnet den Systembrowser zur Anmeldung und wartet auf den Rückruf. Die
/// Ansicht fragt währenddessen `anmeldung_status` (Adresse, Verbindung) und
/// kann abbrechen oder ins Client-Fenster wechseln (#805).
#[tauri::command]
async fn anmelden(app: AppHandle) -> Result<AnmeldeAnsicht, Fehler> {
    let z = app.state::<AppZustand>();
    let k = aktive_konfig(&z)?;
    let a = k.anmeldung.clone().ok_or_else(|| Fehler::Server("Der Server nennt keinen Anmeldedienst.".into()))?;
    let w = Arc::new(anmeldung::Warten::neu());
    // Eine ältere, noch wartende Anmeldung endet; es gilt die neue.
    if let Some(alt) = z.warten.lock().unwrap().replace(w.clone()) {
        alt.abbrechen();
    }
    *z.im_fenster.lock().unwrap() = None;
    let ergebnis = z.sitzung.anmelden(&z.client, &a, &browser_oeffnen, anmeldung::FRIST, &w).await;
    {
        let mut aktuell = z.warten.lock().unwrap();
        if aktuell.as_ref().map(|x| Arc::ptr_eq(x, &w)).unwrap_or(false) {
            *aktuell = None;
        } else {
            // Abgelöst von einer neueren Anmeldung: die räumt selbst auf.
            return ergebnis.map(|_| anmelde_ansicht(&z, &k));
        }
    }
    // Lief die Anmeldung im Client-Fenster, steht dort jetzt die Rückrufseite:
    // zurück in den Assistenten, das Ergebnis holt die Seite dort ab.
    if let Some(schritt) = z.im_fenster.lock().unwrap().take() {
        if let Err(e) = &ergebnis {
            if !matches!(e, Fehler::Abgebrochen) {
                *z.anmelde_meldung.lock().unwrap() = Some(e.to_string());
            }
        }
        if !auf_startseite(&app, &z) {
            zurueck_zum_assistenten(&app, &schritt);
        }
    }
    ergebnis?;
    Ok(anmelde_ansicht(&z, &k))
}

/// Anmeldemaske (#804): Benutzername/Passwort direkt am Token-Endpunkt. Das
/// Passwort liegt nur in diesem Aufruf und wird danach überschrieben; es
/// erscheint in keiner Meldung und keinem Protokoll.
#[tauri::command]
async fn anmelden_formular(z: State<'_, AppZustand>, benutzer: String, passwort: String) -> Result<AnmeldeAnsicht, Fehler> {
    let passwort = zeroize::Zeroizing::new(passwort);
    let k = aktive_konfig(&z)?;
    let a = k.anmeldung.clone().ok_or_else(|| Fehler::Server("Der Server nennt keinen Anmeldedienst.".into()))?;
    // Wartet noch eine Browser-Anmeldung, endet sie: Es gilt das Formular.
    if let Some(w) = z.warten.lock().unwrap().take() {
        w.abbrechen();
    }
    z.sitzung.anmelden_mit_passwort(&z.client, &a, &benutzer, &passwort).await?;
    Ok(anmelde_ansicht(&z, &k))
}

/// Laufende Browser-Anmeldung abbrechen: Loopback zu, Zustand zurück (#805).
#[tauri::command]
fn anmeldung_abbrechen(z: State<'_, AppZustand>) {
    if let Some(w) = z.warten.lock().unwrap().as_ref() {
        w.abbrechen();
    }
}

/// Dieselbe Anmeldeadresse noch einmal an den Systembrowser geben.
#[tauri::command]
fn anmeldung_browser_oeffnen(z: State<'_, AppZustand>) -> Result<(), Fehler> {
    let u = z.warten.lock().unwrap().as_ref().and_then(|w| w.adresse());
    browser_oeffnen(&u.ok_or_else(|| Fehler::Eingabe("Es läuft keine Anmeldung.".into()))?)
}

/// Ausweich, wenn sich kein Browser öffnet (#805): Das Hauptfenster geht auf
/// die Anmeldeseite des Anmeldedienstes; deren Rückruf auf den Loopback
/// funktioniert dort ebenso. Die fremde Seite bekommt keine IPC-Rechte
/// (capabilities ohne `remote`). Danach geht es zurück zu `zurueck`.
#[tauri::command]
fn anmeldung_im_fenster(app: AppHandle, zurueck: String) -> Result<(), Fehler> {
    let z = app.state::<AppZustand>();
    let schritt = if zurueck == "einstellungen" { "einstellungen" } else { "anmeldung" };
    let u = z.warten.lock().unwrap().as_ref().and_then(|w| w.adresse());
    let u = u.ok_or_else(|| Fehler::Eingabe("Es läuft keine Anmeldung.".into()))?;
    *z.im_fenster.lock().unwrap() = Some(schritt.to_string());
    navigiere(&app, u)
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

/// Abmelden auf beiden Seiten (AD4): BFF-Sitzung beenden, Cookies im
/// Fenster löschen, Widerruf beim Anmeldedienst, Tresor löschen.
#[tauri::command]
async fn abmelden(app: AppHandle) -> Result<AnmeldeAnsicht, Fehler> {
    abmelden_ueberall(&app).await?;
    let z = app.state::<AppZustand>();
    let k = aktive_konfig(&z)?;
    Ok(anmelde_ansicht(&z, &k))
}

async fn abmelden_ueberall(app: &AppHandle) -> Result<bool, Fehler> {
    let z = app.state::<AppZustand>();
    let k = aktive_konfig(&z).ok();
    let bff = z.bff.lock().unwrap().take();
    let oberflaeche = k.as_ref().map(|k| k.oberflaeche.clone());
    let fenster = app.get_webview_window("main");
    let hosts = k.as_ref().map(oberflaechen_hosts).unwrap_or_default();
    let loeschen = move || {
        if let Some(w) = &fenster {
            cookies_loeschen(w, &hosts);
        }
    };
    let bff_ziel = match (&oberflaeche, &bff) {
        (Some(o), Some(b)) => Some((o, b.wert.as_str())),
        _ => None,
    };
    oberflaeche::abmelden_beidseitig(&z.client, bff_ziel, loeschen, &z.sitzung, k.as_ref().and_then(|k| k.anmeldung.as_ref())).await
}

/// Oberfläche und Startzentrale des Servers (für Cookies und das Abfangen).
fn oberflaechen_hosts(k: &ClientKonfig) -> Vec<Url> {
    let mut v = vec![k.oberflaeche.clone()];
    if let Some(s) = &k.startzentrale {
        v.push(s.clone());
    }
    v
}

fn cookies_loeschen(w: &tauri::WebviewWindow, hosts: &[Url]) {
    for h in hosts {
        for c in w.cookies_for_url(oberflaeche::wurzel(h)).unwrap_or_default() {
            if c.name() == oberflaeche::SITZUNGS_COOKIE {
                let _ = w.delete_cookie(c);
            }
        }
    }
}

/// Was die Übernahme der Anmeldung ergab.
enum Uebernahme {
    /// Cookie steht im Fenster.
    Gesetzt,
    /// `set_cookie` griff nicht: über den Einmalcode navigieren.
    Einmalcode(String),
    /// Der Server nennt keinen Anmeldedienst: die Oberfläche meldet sich selbst an.
    OhneAnmeldedienst,
    /// Der Client ist nicht (mehr) angemeldet.
    NichtAngemeldet,
}

/// Anmeldung des Clients in die Oberfläche übernehmen: frisches Token, BFF-
/// Sitzung holen (die bestehende verlängern), Cookie ins Fenster setzen und
/// nachsehen, ob es dort angekommen ist.
async fn sitzung_uebernehmen(app: &AppHandle) -> Result<Uebernahme, Fehler> {
    let z = app.state::<AppZustand>();
    let k = aktive_konfig(&z)?;
    let Some(a) = k.anmeldung.clone() else { return Ok(Uebernahme::OhneAnmeldedienst) };
    let Some(token) = z.sitzung.token(&z.client, &a).await? else {
        *z.bff.lock().unwrap() = None;
        return Ok(Uebernahme::NichtAngemeldet);
    };
    let alt = z.bff.lock().unwrap().as_ref().map(|b| b.wert.clone());
    let s = oberflaeche::sitzung_holen(&z.client, &k.oberflaeche, &token, alt.as_deref()).await?;
    let hosts = oberflaechen_hosts(&k);
    let w = app.get_webview_window("main").ok_or_else(|| Fehler::Eingabe("Hauptfenster fehlt.".into()))?;
    let host_refs: Vec<&Url> = hosts.iter().collect();
    for c in oberflaeche::cookies_fuer(&s.cookie, &host_refs) {
        if let Err(e) = w.set_cookie(c) {
            eprintln!("LeadLab-Client: Cookie nicht gesetzt ({e})");
        }
    }
    let angekommen = w
        .cookies_for_url(oberflaeche::wurzel(&k.oberflaeche))
        .map(|alle| alle.iter().any(|c| c.name() == oberflaeche::SITZUNGS_COOKIE && c.value() == s.wert))
        .unwrap_or(false);
    let code = s.einmalcode.clone();
    *z.bff.lock().unwrap() = Some(s);
    verlaengerung_starten(app);
    Ok(if angekommen { Uebernahme::Gesetzt } else { Uebernahme::Einmalcode(code) })
}

/// Hält die BFF-Sitzung frisch, solange es eine gibt (AD4): kurz vor
/// `laeuft_bis` mit einem frischen Token und dem Cookie verlängern. Endet,
/// wenn der Client abgemeldet ist oder der Server die Sitzung beendet hat
/// (Abmelden in der Oberfläche) — dann bleibt das Fenster, wie es ist, und
/// die nächste Login-Umleitung führt in den Assistenten.
fn verlaengerung_starten(app: &AppHandle) {
    let z = app.state::<AppZustand>();
    if z.verlaengerung.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let bis = app.state::<AppZustand>().bff.lock().unwrap().as_ref().map(|b| b.laeuft_bis);
            let Some(bis) = bis else { break };
            let warten = oberflaeche::verlaengern_um(bis, jetzt()).saturating_sub(jetzt());
            tokio::time::sleep(Duration::from_secs(warten)).await;
            if !verlaengern(&app).await {
                break;
            }
        }
        app.state::<AppZustand>().verlaengerung.store(false, Ordering::SeqCst);
    });
}

/// Ein Verlängerungsschritt; `false` beendet die Schleife.
async fn verlaengern(app: &AppHandle) -> bool {
    let z = app.state::<AppZustand>();
    let Some(alt) = z.bff.lock().unwrap().clone() else { return false };
    let Ok(k) = aktive_konfig(&z) else { return false };
    let Some(a) = k.anmeldung.clone() else { return false };
    let token = match z.sitzung.token(&z.client, &a).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            *z.bff.lock().unwrap() = None;
            return false;
        }
        Err(_) => return true, // Netz weg: gleich noch einmal
    };
    match oberflaeche::sitzung_holen(&z.client, &k.oberflaeche, &token, Some(&alt.wert)).await {
        Ok(neu) if oberflaeche::vom_server_beendet(&alt, &neu, jetzt()) => {
            // Abgemeldet in der Oberfläche: die eben angelegte Sitzung gleich wieder weg.
            let _ = oberflaeche::bff_abmelden(&z.client, &k.oberflaeche, &neu.wert).await;
            *z.bff.lock().unwrap() = None;
            z.serverseitig_beendet.store(true, Ordering::SeqCst);
            false
        }
        Ok(neu) => {
            if neu.wert != alt.wert {
                // Abgelaufen (Ruhezustand): das neue Cookie ins Fenster.
                if let Some(w) = app.get_webview_window("main") {
                    let hosts = oberflaechen_hosts(&k);
                    let refs: Vec<&Url> = hosts.iter().collect();
                    for c in oberflaeche::cookies_fuer(&neu.cookie, &refs) {
                        let _ = w.set_cookie(c);
                    }
                }
            }
            *z.bff.lock().unwrap() = Some(neu);
            true
        }
        Err(Fehler::Netz(_)) => true,
        Err(_) => {
            *z.bff.lock().unwrap() = None;
            false
        }
    }
}

/// „Sitzung in der Oberfläche erneuern“ auf der Einstellungsseite.
#[tauri::command]
async fn oberflaeche_sitzung_erneuern(app: AppHandle) -> Result<AnmeldeAnsicht, Fehler> {
    match sitzung_uebernehmen(&app).await? {
        Uebernahme::NichtAngemeldet => return Err(Fehler::Eingabe("Der Client ist nicht angemeldet.".into())),
        Uebernahme::OhneAnmeldedienst => return Err(Fehler::Eingabe("Der Server nennt keinen Anmeldedienst.".into())),
        Uebernahme::Gesetzt | Uebernahme::Einmalcode(_) => {}
    }
    let z = app.state::<AppZustand>();
    let k = aktive_konfig(&z)?;
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
    let anmeldung = einstellungen.konfig.as_ref().map(|k| {
        let mut a = anmelde_ansicht(&z, k);
        // Rückkehr aus der Anmeldung im Client-Fenster auf diese Seite (#805).
        a.meldung = z.anmelde_meldung.lock().unwrap().take();
        a
    });
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

/// Zur Oberfläche — mit der Anmeldung des Clients (AD4). Async: Cookies
/// lesen und setzen darf unter WebView2 nicht im Hauptthread warten.
#[tauri::command]
async fn oberflaeche_oeffnen(app: AppHandle) -> Result<(), Fehler> {
    oeffnen_nach(&app, None).await
}

async fn oeffnen_nach(app: &AppHandle, ziel: Option<Url>) -> Result<(), Fehler> {
    let z = app.state::<AppZustand>();
    let (start, einstellung) = {
        let e = z.einstellungen.lock().unwrap();
        let start = e
            .konfig
            .as_ref()
            .map(|k| k.oberflaeche.clone())
            .ok_or_else(|| Fehler::Eingabe("Noch kein Server eingerichtet.".into()))?;
        (start, e.vollbild)
    };
    let ziel = ziel.unwrap_or(start.clone());
    // Nur der Übergang Assistent → Oberfläche schaltet ins Vollbild; wer auf
    // der Oberfläche „LeadLab öffnen“ wählt, behält seinen Fensterzustand.
    let vom_assistenten = auf_startseite(app, &z);
    // Ausdrücklich geöffnet: eine frühere Abmeldung in der Oberfläche ist erledigt.
    z.serverseitig_beendet.store(false, Ordering::SeqCst);
    match sitzung_uebernehmen(app).await {
        Ok(Uebernahme::Gesetzt | Uebernahme::OhneAnmeldedienst) => navigiere(app, ziel)?,
        Ok(Uebernahme::Einmalcode(code)) => navigiere(app, oberflaeche::client_login_url(&start, &code, &ziel))?,
        Ok(Uebernahme::NichtAngemeldet) => {
            // Statt der Keycloak-Seite im Fenster: die Anmeldung des Clients.
            zurueck_zum_assistenten(app, "anmeldung");
            return Ok(());
        }
        Err(e) => {
            if vom_assistenten {
                return Err(e);
            }
            zurueck_zum_assistenten(app, "anmeldung");
            return Ok(());
        }
    }
    if vom_assistenten && vollbild::im_vollbild(&z.argumente, einstellung) {
        vollbild_setzen(app, true);
    }
    Ok(())
}

/// Die Oberfläche wollte auf `/api/auth/login` (AD4): Sitzung erneuern und
/// zum Ziel.
///
/// - Lief die alte Sitzung noch und der Server gibt trotzdem eine neue aus,
///   hat er sie beendet (Abmelden in der Oberfläche, Admin): zurück in den
///   Assistenten, nicht still neu anmelden.
/// - Kommt dasselbe Cookie zurück, lebt die Sitzung — dann hat das Fenster
///   das Cookie nicht angenommen, und es geht über den Einmalcode.
/// - Mehr als drei Umleitungen in 30 s führen in den Assistenten statt im Kreis.
async fn login_abgefangen(app: AppHandle, ziel: Url) {
    let z = app.state::<AppZustand>();
    let anlaeufe = {
        let mut u = z.umleitungen.lock().unwrap();
        u.retain(|t| t.elapsed() < Duration::from_secs(30));
        u.push(Instant::now());
        u.len()
    };
    let Ok(k) = aktive_konfig(&z) else { return };
    if anlaeufe > 3 || z.serverseitig_beendet.swap(false, Ordering::SeqCst) {
        zurueck_zum_assistenten(&app, "anmeldung");
        return;
    }
    let alt = z.bff.lock().unwrap().clone();
    let ergebnis = sitzung_uebernehmen(&app).await;
    let neu = z.bff.lock().unwrap().clone();
    match (ergebnis, neu) {
        (Ok(u @ (Uebernahme::Gesetzt | Uebernahme::Einmalcode(_))), Some(neu)) => {
            if alt.as_ref().map(|a| oberflaeche::vom_server_beendet(a, &neu, jetzt())).unwrap_or(false) {
                let _ = oberflaeche::bff_abmelden(&z.client, &k.oberflaeche, &neu.wert).await;
                *z.bff.lock().unwrap() = None;
                if let Some(w) = app.get_webview_window("main") {
                    cookies_loeschen(&w, &oberflaechen_hosts(&k));
                }
                zurueck_zum_assistenten(&app, "anmeldung");
                return;
            }
            let ueber_code = matches!(u, Uebernahme::Einmalcode(_))
                || alt.map(|a| a.wert == neu.wert).unwrap_or(false)
                || anlaeufe >= 2;
            let adresse = if ueber_code { oberflaeche::client_login_url(&k.oberflaeche, &neu.einmalcode, &ziel) } else { ziel };
            if navigiere(&app, adresse).is_err() {
                zurueck_zum_assistenten(&app, "anmeldung");
            }
        }
        _ => zurueck_zum_assistenten(&app, "anmeldung"),
    }
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
            // Login-Umleitung der eigenen Oberfläche (AD4): Der Client meldet
            // an, nicht Keycloak im Fenster. Ohne Anmeldedienst läuft sie normal.
            if let Some(z) = handle.try_state::<AppZustand>() {
                let k = z.einstellungen.lock().unwrap().konfig.clone();
                if let Some(k) = k.filter(|k| k.anmeldung.is_some()) {
                    let hosts = oberflaechen_hosts(&k);
                    let refs: Vec<&Url> = hosts.iter().collect();
                    if let Some(ziel) = oberflaeche::login_ziel(u, &refs) {
                        tauri::async_runtime::spawn(login_abgefangen(handle.clone(), ziel));
                        return false;
                    }
                }
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
    let abmelden = MenuItem::with_id(app, "abmelden", "Abmelden", true, None::<&str>)?;
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
          &abmelden, &PredefinedMenuItem::separator(app)?, &beenden],
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
                sitzung: Arc::new(anmeldung::Sitzung::neu(
                    tresor::Tresor::neu(konfig_ordner(app.handle())?, tresor::Schluessel::Geraet),
                    Arc::new(anmeldung::Schluesselbund),
                )),
                lokal,
                update: Mutex::new(None),
                argumente: vollbild::argumente(std::env::args().skip(1)),
                vollbild_seit: Mutex::new(None),
                update_info: Mutex::new(None),
                zuletzt_geladen: Mutex::new(None),
                bff: Mutex::new(None),
                verlaengerung: AtomicBool::new(false),
                umleitungen: Mutex::new(Vec::new()),
                serverseitig_beendet: AtomicBool::new(false),
                warten: Mutex::new(None),
                im_fenster: Mutex::new(None),
                anmelde_meldung: Mutex::new(None),
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
                    let h = app.clone();
                    tauri::async_runtime::spawn(async move { let _ = oeffnen_nach(&h, None).await; });
                }
                "abmelden" => {
                    let h = app.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = abmelden_ueberall(&h).await;
                        zurueck_zum_assistenten(&h, "anmeldung");
                    });
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
            anmelden_formular,
            anmeldung_abbrechen,
            anmeldung_browser_oeffnen,
            anmeldung_im_fenster,
            abmelden,
            update_pruefen,
            update_installieren,
            anmeldung_pruefen,
            zustand_uebersicht,
            konfig_neu_pruefen,
            paket_entfernen,
            ordner_oeffnen,
            oberflaeche_sitzung_erneuern
        ])
        .run(tauri::generate_context!())
        .expect("LeadLab-Client konnte nicht starten");
}
