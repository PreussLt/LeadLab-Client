//! Neue Fenster aus der Oberfläche (Hauptrepo #859, Konzept Desktop-App §4.4).
//!
//! Die Frontends gliedern Ansichten mit `window.open(…, '_blank')` bzw.
//! `target="_blank"` aus: Einspiel-Bildschirm, Stabssicht, ausgelagerter
//! Reiter, Beamer, Hydrantenkarte. Am Übungsplatz gehört das auf Zweitschirm
//! oder Beamer. Ohne Handler für neue Fenster verwirft WebKitGTK die Anfrage
//! still — genau das war der Fehler.
//!
//! Regeln wie in der Desktop-App:
//! - internes Ziel (Host der Oberfläche oder der Startzentrale, `about:blank`
//!   für Druckfenster) → **Nebenfenster** derselben Sitzung, auf dem zweiten
//!   Bildschirm, mit „✕ Schließen“, Strg+W und F11;
//! - externes Ziel (`http`, `https`, `mailto`) → Systembrowser;
//! - alles andere (`leadlab://`, `file:`, `javascript:` …) → abgelehnt.
//!
//! Hier stehen die reinen Teile: Zielentscheidung, Bildschirmwahl, Platz und
//! das Skript für Schließknopf und Tasten. Das Fenster baut `lib.rs`.

use serde::Serialize;
use url::Url;

/// Pseudo-Adresse: das Nebenfenster schließen (Knopf, Strg+W, `window.close()`).
pub const ADRESSE_SCHLIESSEN: &str = "leadlab://fenster-schliessen";
/// Pseudo-Adresse: Vollbild des Nebenfensters umschalten (F11).
pub const ADRESSE_VOLLBILD: &str = "leadlab://vollbild-umschalten";
/// Präfix der Fensterkennungen; das Hauptfenster heißt `main`.
pub const PRAEFIX: &str = "neben-";

/// Wohin eine Anfrage nach einem neuen Fenster geht.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Ziel {
    /// Eigenes Client-Fenster derselben Sitzung.
    Nebenfenster,
    /// Standardbrowser des Systems.
    Browser,
    /// Gar nicht öffnen.
    Ablehnen,
}

/// Entscheidet über eine Anfrage aus `window.open` / `target="_blank"`.
/// `hosts` sind Oberfläche und Startzentrale aus der Client-Konfiguration.
pub fn entscheide(u: &Url, hosts: &[Url]) -> Ziel {
    match u.scheme() {
        // Druckfenster (`window.open('', '_blank')`) befüllt die Seite selbst.
        "about" if u.path() == "blank" => Ziel::Nebenfenster,
        "blob" => match Url::parse(u.path()) {
            Ok(innen) if intern(&innen, hosts) => Ziel::Nebenfenster,
            _ => Ziel::Ablehnen,
        },
        "http" | "https" if intern(u, hosts) => Ziel::Nebenfenster,
        "http" | "https" | "mailto" => Ziel::Browser,
        _ => Ziel::Ablehnen,
    }
}

/// Gleicher Host wie Oberfläche oder Startzentrale (Schema und Port egal:
/// ein Wechsel http → https bleibt dieselbe Oberfläche).
fn intern(u: &Url, hosts: &[Url]) -> bool {
    matches!(u.scheme(), "http" | "https")
        && u.host_str().is_some()
        && hosts.iter().any(|h| h.host_str() == u.host_str())
}

/// Was die Seite im Nebenfenster über eine Pseudo-Adresse anfordert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aktion {
    Schliessen,
    VollbildUmschalten,
}

pub fn aktion(u: &Url) -> Option<Aktion> {
    if u.scheme() != "leadlab" {
        return None;
    }
    match u.host_str() {
        Some("fenster-schliessen") => Some(Aktion::Schliessen),
        Some("vollbild-umschalten") => Some(Aktion::VollbildUmschalten),
        _ => None,
    }
}

/// Bildschirm für ein Nebenfenster wie `Nebenfenster.zeige` der Desktop-App:
/// der erste, der nicht der des Hauptfensters ist; gibt es keinen anderen,
/// der des Hauptfensters (bzw. der erste).
pub fn zweitschirm<T: Clone>(schirme: &[T], haupt: Option<usize>) -> Option<T> {
    schirme
        .iter()
        .enumerate()
        .find(|(i, _)| Some(*i) != haupt)
        .map(|(_, s)| s.clone())
        .or_else(|| haupt.and_then(|i| schirme.get(i).cloned()))
        .or_else(|| schirme.first().cloned())
}

/// Logische Lage (x, y, Breite, Höhe) eines Nebenfensters auf einer
/// Arbeitsfläche in physischen Pixeln: vier Fünftel groß, mittig.
pub fn platz(x: i32, y: i32, breite: u32, hoehe: u32, skala: f64) -> (f64, f64, f64, f64) {
    let s = if skala > 0.0 { skala } else { 1.0 };
    let (b, h) = (breite as f64 * 0.8 / s, hoehe as f64 * 0.8 / s);
    let (lx, ly) = (x as f64 / s, y as f64 / s);
    (lx + (breite as f64 / s - b) / 2.0, ly + (hoehe as f64 / s - h) / 2.0, b, h)
}

/// Initialisierungsskript des Nebenfensters: „✕ Schließen“ unten rechts
/// (immer sichtbar — im Vollbild gibt es keinen Rahmen und kein Kreuz),
/// Strg+W, F11 und `window.close()` (die Übung schließt ihre ausgegliederten
/// Ansichten beim Beenden selbst, #181). Alles läuft über Pseudo-Adressen, die
/// der Client in `on_navigation` abfängt; IPC hat die Seite nicht.
pub fn skript(ausgenommen: &[String]) -> String {
    let aus = serde_json::to_string(ausgenommen).unwrap_or_else(|_| "[]".into());
    let zu = serde_json::to_string(ADRESSE_SCHLIESSEN).expect("Zeichenkette");
    let vb = serde_json::to_string(ADRESSE_VOLLBILD).expect("Zeichenkette");
    format!(
        r#"(function(){{try{{var aus={aus};if(aus.indexOf(location.origin)>=0)return;
function zu(){{location.href={zu};}}
try{{window.close=zu;}}catch(e){{}}
var knopf=null;
var STIL='position:fixed;right:16px;bottom:14px;z-index:2147483647;min-height:44px;padding:6px 14px;border-radius:8px;'+
'font:500 13px "Open Sans",system-ui,sans-serif;cursor:pointer;';
function farbe(h){{knopf.style.background=h?'#cc0000':'rgba(17,24,39,0.88)';knopf.style.color=h?'#ffffff':'#9ca3af';
knopf.style.border='1px solid '+(h?'#cc0000':'#1f2937');}}
function zeige(){{if(!document.body){{document.addEventListener('DOMContentLoaded',zeige,{{once:true}});return;}}
if(!knopf){{knopf=document.createElement('button');knopf.type='button';knopf.textContent='✕  Schließen';
knopf.setAttribute('data-leadlab-client','schliessen');knopf.setAttribute('aria-label','Dieses Fenster schließen (Strg+W) – die Übung läuft weiter');
knopf.title='Dieses Fenster schließen (Strg+W) – die Übung läuft weiter';
knopf.style.cssText=STIL;farbe(false);
knopf.addEventListener('mouseenter',function(){{farbe(true);}});knopf.addEventListener('mouseleave',function(){{farbe(false);}});
knopf.addEventListener('click',zu);}}
if(!knopf.isConnected)document.body.appendChild(knopf);}}
zeige();
window.addEventListener('keydown',function(e){{
if(e.key==='F11'){{e.preventDefault();location.href={vb};return;}}
if((e.ctrlKey||e.metaKey)&&!e.altKey&&(e.key==='w'||e.key==='W')){{e.preventDefault();zu();}}}});
}}catch(e){{}}}})();"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn hosts() -> Vec<Url> {
        vec![u("http://uebung.lead.lab/"), u("http://lead.lab/")]
    }

    #[test]
    fn internes_ziel_wird_nebenfenster() {
        let h = hosts();
        assert_eq!(entscheide(&u("http://uebung.lead.lab/session/7/einspiel"), &h), Ziel::Nebenfenster);
        assert_eq!(entscheide(&u("http://uebung.lead.lab/session/7?tab=karte&popout=1"), &h), Ziel::Nebenfenster);
        assert_eq!(entscheide(&u("http://lead.lab/hydrantenkarte"), &h), Ziel::Nebenfenster, "Startzentrale");
        assert_eq!(entscheide(&u("https://uebung.lead.lab/x"), &h), Ziel::Nebenfenster, "http → https bleibt intern");
        assert_eq!(entscheide(&u("about:blank"), &h), Ziel::Nebenfenster, "Druckfenster");
        assert_eq!(entscheide(&u("blob:http://uebung.lead.lab/0b1c-2d"), &h), Ziel::Nebenfenster);
    }

    #[test]
    fn externes_ziel_geht_in_den_browser() {
        let h = hosts();
        assert_eq!(entscheide(&u("https://www.mapillary.com/app/?lat=48"), &h), Ziel::Browser);
        assert_eq!(entscheide(&u("https://www.openstreetmap.org/copyright"), &h), Ziel::Browser);
        assert_eq!(entscheide(&u("http://lead.lab.example.com/"), &h), Ziel::Browser, "nur exakt gleicher Host");
        assert_eq!(entscheide(&u("mailto:leitstelle@example.org"), &h), Ziel::Browser);
    }

    #[test]
    fn alles_andere_wird_abgelehnt() {
        let h = hosts();
        assert_eq!(entscheide(&u("leadlab://vollbild-aus"), &h), Ziel::Ablehnen);
        assert_eq!(entscheide(&u(ADRESSE_SCHLIESSEN), &h), Ziel::Ablehnen);
        assert_eq!(entscheide(&u("file:///etc/passwd"), &h), Ziel::Ablehnen);
        assert_eq!(entscheide(&u("javascript:alert(1)"), &h), Ziel::Ablehnen);
        assert_eq!(entscheide(&u("about:config"), &h), Ziel::Ablehnen);
        assert_eq!(entscheide(&u("blob:https://fremd.example.com/0b1c"), &h), Ziel::Ablehnen);
        assert_eq!(entscheide(&u("http://uebung.lead.lab/"), &[]), Ziel::Browser, "ohne Konfiguration nichts intern");
    }

    #[test]
    fn pseudo_adressen() {
        assert_eq!(aktion(&u(ADRESSE_SCHLIESSEN)), Some(Aktion::Schliessen));
        assert_eq!(aktion(&u(ADRESSE_VOLLBILD)), Some(Aktion::VollbildUmschalten));
        assert_eq!(aktion(&u("leadlab://vollbild-aus")), None, "gehört vollbild.rs");
        assert_eq!(aktion(&u("https://fenster-schliessen/")), None);
    }

    #[test]
    fn zweitschirm_wie_die_desktop_app() {
        let schirme = ["links", "mitte", "rechts"];
        assert_eq!(zweitschirm(&schirme, Some(0)), Some("mitte"));
        assert_eq!(zweitschirm(&schirme, Some(1)), Some("links"));
        assert_eq!(zweitschirm(&["allein"], Some(0)), Some("allein"), "ein Schirm: dort");
        assert_eq!(zweitschirm(&["a", "b"], None), Some("a"));
        assert_eq!(zweitschirm::<&str>(&[], Some(0)), None);
    }

    #[test]
    fn platz_vier_fuenftel_mittig() {
        assert_eq!(platz(1920, 0, 1920, 1080, 1.0), (2112.0, 108.0, 1536.0, 864.0));
        // HiDPI: logische Pixel.
        assert_eq!(platz(0, 0, 3840, 2160, 2.0), (192.0, 108.0, 1536.0, 864.0));
        assert_eq!(platz(0, 0, 1000, 500, 0.0), (100.0, 50.0, 800.0, 400.0));
    }

    #[test]
    fn schliessknopf_skript() {
        let js = skript(&["tauri://localhost".into()]);
        assert!(js.contains(r#"var aus=["tauri://localhost"]"#), "lokale Seiten ausgenommen");
        assert!(js.contains(r#"location.href="leadlab://fenster-schliessen""#));
        assert!(js.contains(r#"location.href="leadlab://vollbild-umschalten""#));
        assert!(js.contains("window.close=zu"), "window.close() schließt das Nebenfenster (#181)");
        assert!(js.contains("Schließen") && js.contains("Strg+W"));
        assert!(js.contains("'F11'") && js.contains("e.ctrlKey"));
        assert!(js.contains("min-height:44px"), "Bedienziel ≥ 44 px");
        assert!(js.contains("right:16px;bottom:14px"), "unten rechts wie in der Desktop-App");
    }

    /// Wächter (#859): Ohne `on_new_window` am Fenster verwirft WebKitGTK
    /// `window.open`/`target=_blank` still. Haupt- und Nebenfenster müssen
    /// den Handler tragen.
    #[test]
    fn fenster_tragen_handler_fuer_neue_fenster() {
        let lib = include_str!("lib.rs");
        let start = lib.find("fn hauptfenster(").expect("hauptfenster");
        let ende = lib[start..].find("\nfn ").map(|e| start + e).unwrap_or(lib.len());
        assert!(lib[start..ende].contains(".on_new_window("), "Hauptfenster ohne Handler für neue Fenster");
        let start = lib.find("fn nebenfenster_bauen(").expect("nebenfenster_bauen");
        let ende = lib[start..].find("\nfn ").map(|e| start + e).unwrap_or(lib.len());
        assert!(lib[start..ende].contains(".on_new_window("), "Nebenfenster ohne Handler für neue Fenster");
    }
}
