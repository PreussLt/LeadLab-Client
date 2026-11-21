//! Vollbildmodus wie die Desktop-App (Hauptrepo #791, Konzept §8.5).
//!
//! Der Arbeitsplatz im Stabsraum zeigt nur die Oberfläche des Servers: kein
//! Fensterrahmen, keine Menüleiste. Hier stehen die reinen Teile, die sich
//! ohne Fenster prüfen lassen — Kommandozeile, Monitorwahl, die
//! Pseudo-Adresse für den Ausstieg und das Skript für den Schwebeknopf. Das
//! Umschalten selbst macht `lib.rs`.
//!
//! Die Server-Oberfläche hat bewusst keine IPC-Rechte. Der Schwebeknopf
//! „✕ Vollbild verlassen“ und die Tasten F11/Esc in der Seite navigieren
//! deshalb auf [`ADRESSE_AUS`]; der Client fängt das in `on_navigation` ab
//! und bricht die Navigation ab. Mehr als „Vollbild aus“ kann die Seite
//! darüber nicht auslösen.

use serde::Serialize;
use url::Url;

/// Pseudo-Adresse, mit der die Seite den Ausstieg anfordert.
pub const ADRESSE_AUS: &str = "leadlab://vollbild-aus";

/// Was auf der Kommandozeile stand. `None` = nichts angegeben.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Argumente {
    /// `--vollbild` → `Some(true)`, `--fenster` → `Some(false)`; das letzte gewinnt.
    pub vollbild: Option<bool>,
    /// `--monitor N` bzw. `--monitor=N` (0 = erster).
    pub monitor: Option<usize>,
}

/// Liest die Argumente (ohne Programmnamen). Unbekanntes bleibt liegen —
/// Tauri und das Betriebssystem reichen gelegentlich eigene Schalter durch.
pub fn argumente<I, S>(args: I) -> Argumente
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut a = Argumente::default();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_ref() {
            "--vollbild" => a.vollbild = Some(true),
            "--fenster" => a.vollbild = Some(false),
            "--monitor" => {
                if let Some(n) = it.next() {
                    a.monitor = n.as_ref().trim().parse().ok();
                }
            }
            s => {
                if let Some(n) = s.strip_prefix("--monitor=") {
                    a.monitor = n.trim().parse().ok();
                }
            }
        }
    }
    a
}

/// Startet die Oberfläche im Vollbild? Die Kommandozeile gilt für diesen
/// Start und schlägt die gespeicherte Einstellung, ohne sie zu ändern.
pub fn im_vollbild(a: &Argumente, einstellung: bool) -> bool {
    a.vollbild.unwrap_or(einstellung)
}

/// Wohin das Vollbild geht.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ziel {
    /// Auf diesen Monitor (Index in `available_monitors`).
    Monitor(usize),
    /// Ohne Output: der Compositor nimmt den Monitor des Fensters.
    Fenster,
}

/// Monitor fürs Vollbild wie `waehle_schirm` der Desktop-App:
/// `--monitor N`, sonst der mit dem Mauszeiger, sonst der des Fensters.
///
/// Unter Wayland kennt der Client den Mauszeiger nicht — tao meldet dort fest
/// `(0, 0)`, also immer den Monitor oben links (#1352). Dort gilt darum ohne
/// `--monitor` der Monitor des Fensters.
pub fn vollbild_ziel(anzahl: usize, wunsch: Option<usize>, zeiger: Option<usize>, wayland: bool) -> Ziel {
    if let Some(n) = wunsch.filter(|&n| n < anzahl) {
        return Ziel::Monitor(n);
    }
    match zeiger.filter(|&z| !wayland && z < anzahl) {
        Some(z) => Ziel::Monitor(z),
        None => Ziel::Fenster,
    }
}

/// Läuft GTK unter Wayland? Wie GDK selbst: `GDK_BACKEND` (erster Eintrag
/// der Liste) entscheidet, sonst ein gesetztes `WAYLAND_DISPLAY`.
pub fn ist_wayland(gdk_backend: Option<&str>, wayland_display: Option<&str>) -> bool {
    let erstes = gdk_backend.and_then(|b| b.split(',').map(str::trim).find(|t| !t.is_empty() && *t != "*"));
    match erstes {
        Some(b) => b == "wayland",
        None => wayland_display.map(|d| !d.trim().is_empty()).unwrap_or(false),
    }
}

/// Physisches Rechteck eines Monitors (`Monitor::position`/`size`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rechteck {
    pub x: i32,
    pub y: i32,
    pub breite: u32,
    pub hoehe: u32,
}

impl Rechteck {
    fn enthaelt(&self, x: i32, y: i32) -> bool {
        self.x <= x && x < self.x + self.breite as i32 && self.y <= y && y < self.y + self.hoehe as i32
    }
}

/// Punkt für `set_fullscreen_on_monitor`, unter dem Tauri den Zielmonitor
/// findet.
///
/// tauri-runtime-wry nimmt den **ersten** Monitor, dessen physisches Rechteck
/// den Punkt enthält. tao rechnet diese Rechtecke als logische Geometrie mal
/// eigener Skalierung; bei gemischter Skalierung (Laptop 150 % links, Monitor
/// 100 % rechts daneben) überdeckt das Rechteck des ersten den Ursprung des
/// zweiten, und `Monitor::position()` landet auf dem falschen Schirm (#1352).
/// `None`, wenn frühere Rechtecke den Zielmonitor ganz verdecken.
pub fn sondierpunkt(rechtecke: &[Rechteck], ziel: usize) -> Option<(i32, i32)> {
    let r = *rechtecke.get(ziel)?;
    // Jede freie Teilfläche des Ziels beginnt an einer Kante: der eigenen
    // oder der eines anderen Rechtecks. Diese Kanten genügen als Kandidaten.
    let kanten = |eigen: i32, laenge: u32, fremd: &dyn Fn(&Rechteck) -> (i32, u32)| {
        let ende = eigen + laenge as i32;
        let mut k = vec![eigen, eigen + laenge as i32 / 2];
        for o in rechtecke {
            let (a, l) = fremd(o);
            k.extend([a, a + l as i32]);
        }
        k.retain(|&v| eigen <= v && v < ende);
        k.sort_unstable();
        k.dedup();
        k
    };
    let xs = kanten(r.x, r.breite, &|o| (o.x, o.breite));
    let ys = kanten(r.y, r.hoehe, &|o| (o.y, o.hoehe));
    // Mitte zuerst, dann die Kanten.
    let mitte = (r.x + r.breite as i32 / 2, r.y + r.hoehe as i32 / 2);
    std::iter::once(mitte)
        .chain(xs.iter().flat_map(|&x| ys.iter().map(move |&y| (x, y))))
        .find(|&(x, y)| rechtecke.iter().position(|o| o.enthaelt(x, y)) == Some(ziel))
}

/// Zeigt `jetzt` dieselbe Herkunft wie die Startseite (Assistent)?
///
/// Nicht über `Url::origin()`: Für `tauri://localhost` (Assistent unter
/// Linux und macOS) ist der Ursprung opak, und zwei opake Ursprünge sind nie
/// gleich — der Übergang Assistent → Oberfläche schaltete dort darum nie ins
/// Vollbild (#1352). Verglichen werden Schema, Host und Port.
pub fn gleiche_herkunft(start: &Url, jetzt: &Url) -> bool {
    start.scheme() == jetzt.scheme()
        && start.host_str() == jetzt.host_str()
        && start.port_or_known_default() == jetzt.port_or_known_default()
}

/// Ist das eine Ausstiegs-Anforderung der Seite?
pub fn ist_ausstieg(u: &Url) -> bool {
    u.scheme() == "leadlab" && u.host_str() == Some("vollbild-aus")
}

/// Initialisierungsskript für die Server-Oberfläche: Schwebeknopf unten
/// rechts (wie „✕ Beenden“ der Desktop-App — oben rechts säße er auf dem
/// Abmelden-Knopf der Startzentrale) und F11/Esc. Sichtbar und scharf erst,
/// wenn der Client `window.__LEADLAB_VOLLBILD__(true)` ruft
/// ([`zustand_skript`]). Lokale Seiten (`ausgenommen`) bleiben unberührt.
///
/// Esc gehört in der Übung zuerst den Dialogen. Es verlässt das Vollbild
/// darum nur, wenn die Seite die Taste nicht selbst behandelt hat
/// (`defaultPrevented`), kein Dialog offen ist und der Fokus nicht in einem
/// Eingabefeld steht.
pub fn skript(ausgenommen: &[String]) -> String {
    let aus = serde_json::to_string(ausgenommen).unwrap_or_else(|_| "[]".into());
    let ziel = serde_json::to_string(ADRESSE_AUS).expect("Zeichenkette");
    format!(
        r#"(function(){{try{{var aus={aus};if(aus.indexOf(location.origin)>=0)return;
var an=false,knopf=null;
function raus(){{location.href={ziel};}}
var STIL='position:fixed;right:16px;bottom:14px;z-index:2147483647;min-height:44px;padding:6px 14px;border-radius:8px;'+
'font:500 13px "Open Sans",system-ui,sans-serif;cursor:pointer;';
function farbe(h){{knopf.style.background=h?'#cc0000':'rgba(17,24,39,0.88)';knopf.style.color=h?'#ffffff':'#9ca3af';
knopf.style.border='1px solid '+(h?'#cc0000':'#1f2937');}}
function zeige(){{if(!document.body){{document.addEventListener('DOMContentLoaded',zeige,{{once:true}});return;}}
if(!knopf){{knopf=document.createElement('button');knopf.type='button';knopf.textContent='✕  Vollbild verlassen';
knopf.setAttribute('data-leadlab-client','vollbild');knopf.setAttribute('aria-label','Vollbild verlassen (F11)');
knopf.style.cssText=STIL;farbe(false);
knopf.addEventListener('mouseenter',function(){{farbe(true);}});knopf.addEventListener('mouseleave',function(){{farbe(false);}});
knopf.addEventListener('click',raus);}}
knopf.style.display=an?'':'none';if(an&&!knopf.isConnected)document.body.appendChild(knopf);}}
function dialogOffen(){{return !!document.querySelector('dialog[open],[role="dialog"],[role="alertdialog"],[aria-modal="true"]');}}
function eingabe(t){{return !!(t&&t.closest&&t.closest('input,textarea,select,[contenteditable=""],[contenteditable="true"]'));}}
Object.defineProperty(window,'__LEADLAB_VOLLBILD__',{{value:function(v){{an=!!v;zeige();}},writable:false,configurable:false}});
window.addEventListener('keydown',function(e){{if(!an)return;
if(e.key==='F11'){{e.preventDefault();raus();return;}}
if(e.key==='Escape'&&!e.defaultPrevented&&!dialogOffen()&&!eingabe(e.target))raus();}});
}}catch(e){{}}}})();"#
    )
}

/// Teilt der Seite den Zustand mit (nach dem Umschalten und nach jedem Laden).
pub fn zustand_skript(an: bool) -> String {
    format!("window.__LEADLAB_VOLLBILD__&&window.__LEADLAB_VOLLBILD__({an});")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kommandozeile() {
        assert_eq!(argumente::<_, &str>([]), Argumente::default());
        assert_eq!(argumente(["--vollbild"]).vollbild, Some(true));
        assert_eq!(argumente(["--fenster"]).vollbild, Some(false));
        assert_eq!(argumente(["--vollbild", "--fenster"]).vollbild, Some(false), "das letzte gewinnt");
        assert_eq!(argumente(["--monitor", "2"]).monitor, Some(2));
        assert_eq!(argumente(["--monitor=1", "--vollbild"]), Argumente { vollbild: Some(true), monitor: Some(1) });
        assert_eq!(argumente(["--monitor", "zwei"]).monitor, None);
        assert_eq!(argumente(["--monitor"]).monitor, None);
        assert_eq!(argumente(["-psn_0_123", "--unbekannt", "--fenster"]).vollbild, Some(false));
    }

    #[test]
    fn kommandozeile_schlaegt_einstellung() {
        let leer = Argumente::default();
        assert!(!im_vollbild(&leer, false));
        assert!(im_vollbild(&leer, true));
        assert!(!im_vollbild(&argumente(["--fenster"]), true));
        assert!(im_vollbild(&argumente(["--vollbild"]), false));
    }

    #[test]
    fn monitorwahl_wie_waehle_schirm() {
        assert_eq!(vollbild_ziel(3, Some(2), Some(0), false), Ziel::Monitor(2), "--monitor gewinnt");
        assert_eq!(vollbild_ziel(3, Some(0), Some(1), false), Ziel::Monitor(0));
        assert_eq!(vollbild_ziel(3, None, Some(1), false), Ziel::Monitor(1), "sonst der Mauszeiger");
        assert_eq!(vollbild_ziel(3, Some(7), Some(1), false), Ziel::Monitor(1), "ungültige Nummer: Mauszeiger");
        assert_eq!(vollbild_ziel(3, Some(7), None, false), Ziel::Fenster, "sonst der des Fensters");
        assert_eq!(vollbild_ziel(0, Some(0), None, false), Ziel::Fenster);
    }

    /// #1352: Unter Wayland meldet tao den Zeiger fest bei (0, 0) — das ist
    /// immer der Monitor oben links, nicht der mit dem Mauszeiger.
    #[test]
    fn wayland_ohne_mauszeiger() {
        assert_eq!(vollbild_ziel(2, None, Some(0), true), Ziel::Fenster, "Zeiger zählt unter Wayland nicht");
        assert_eq!(vollbild_ziel(2, Some(1), Some(0), true), Ziel::Monitor(1), "--monitor gilt weiter");
        assert_eq!(vollbild_ziel(2, Some(5), None, true), Ziel::Fenster);
    }

    #[test]
    fn wayland_erkennen_wie_gdk() {
        assert!(ist_wayland(None, Some("wayland-1")));
        assert!(!ist_wayland(None, None));
        assert!(!ist_wayland(None, Some("")));
        assert!(!ist_wayland(Some("x11"), Some("wayland-1")), "GDK_BACKEND erzwingt X11");
        assert!(ist_wayland(Some("wayland,x11"), None));
        assert!(!ist_wayland(Some("x11,wayland"), Some("wayland-1")));
        assert!(ist_wayland(Some("*"), Some("wayland-0")), "* = Standardreihenfolge");
    }

    /// Nachbau von `find_monitor_for_position` aus tauri-runtime-wry 2.12:
    /// erster Monitor, dessen Rechteck den Punkt enthält.
    fn tauri_findet(rechtecke: &[Rechteck], p: (i32, i32)) -> Option<usize> {
        rechtecke.iter().position(|r| r.enthaelt(p.0, p.1))
    }

    /// #1352, Regieplatz: eDP-1 2560×1600 bei 150 % (GDK rundet auf 2) an
    /// 0,0, logisch 1707 breit; DP-6 3440×1440 bei 100 % an 1707,0.
    #[test]
    fn sondierpunkt_bei_gemischter_skalierung() {
        let schirme = [
            Rechteck { x: 0, y: 0, breite: 3414, hoehe: 2134 },
            Rechteck { x: 1707, y: 0, breite: 3440, hoehe: 1440 },
        ];
        for ziel in 0..schirme.len() {
            let p = sondierpunkt(&schirme, ziel).expect("Punkt");
            assert_eq!(tauri_findet(&schirme, p), Some(ziel), "Monitor {ziel} über {p:?}");
        }
        // Mit GDK-Skalierung 1 (kein Überlappen) ebenso.
        let schirme = [Rechteck { x: 0, y: 0, breite: 1707, hoehe: 1067 }, schirme[1]];
        assert_eq!(tauri_findet(&schirme, sondierpunkt(&schirme, 1).unwrap()), Some(1));
    }

    #[test]
    fn sondierpunkt_findet_freie_ecken() {
        // Zielmonitor rechts unten nur teilweise verdeckt, links davon ein großer.
        let schirme = [
            Rechteck { x: 0, y: 0, breite: 4000, hoehe: 1000 },
            Rechteck { x: 3000, y: 500, breite: 1920, hoehe: 1080 },
            Rechteck { x: -1920, y: 0, breite: 1920, hoehe: 1080 },
        ];
        for ziel in 0..schirme.len() {
            let p = sondierpunkt(&schirme, ziel).expect("Punkt");
            assert_eq!(tauri_findet(&schirme, p), Some(ziel));
        }
        // Ganz verdeckt: kein Punkt, der Client nimmt dann das Fenster.
        let verdeckt = [Rechteck { x: 0, y: 0, breite: 4000, hoehe: 2000 }, Rechteck { x: 10, y: 10, breite: 100, hoehe: 100 }];
        assert_eq!(sondierpunkt(&verdeckt, 1), None);
        assert_eq!(sondierpunkt(&verdeckt, 5), None);
    }

    /// #1352: Unter Linux läuft der Assistent auf tauri://localhost.
    #[test]
    fn startseite_auch_unter_tauri_schema() {
        let u = |s: &str| Url::parse(s).unwrap();
        assert!(gleiche_herkunft(&u("tauri://localhost/"), &u("tauri://localhost/#start")), "Linux/macOS");
        assert!(gleiche_herkunft(&u("http://tauri.localhost/"), &u("http://tauri.localhost/#anmeldung")), "Windows");
        assert!(gleiche_herkunft(&u("http://localhost:1420/"), &u("http://localhost:1420/index.html")), "Entwicklung");
        assert!(!gleiche_herkunft(&u("tauri://localhost/"), &u("http://uebung.lead.lab/")));
        assert!(!gleiche_herkunft(&u("http://tauri.localhost/"), &u("http://uebung.lead.lab/")));
        assert!(!gleiche_herkunft(&u("http://localhost:1420/"), &u("http://localhost:11201/")), "Port zählt");
    }

    #[test]
    fn ausstieg_nur_ueber_die_pseudo_adresse() {
        assert!(ist_ausstieg(&Url::parse(ADRESSE_AUS).unwrap()));
        assert!(!ist_ausstieg(&Url::parse("leadlab://anderes").unwrap()));
        assert!(!ist_ausstieg(&Url::parse("https://vollbild-aus/").unwrap()));
        assert!(!ist_ausstieg(&Url::parse("https://lead.lab/").unwrap()));
    }

    #[test]
    fn schwebeknopf_skript() {
        let js = skript(&["tauri://localhost".into()]);
        assert!(js.contains(r#"var aus=["tauri://localhost"]"#), "lokale Seiten ausgenommen");
        assert!(js.contains(r#"location.href="leadlab://vollbild-aus""#));
        assert!(js.contains("Vollbild verlassen"));
        assert!(js.contains("'F11'") && js.contains("'Escape'") && js.contains("defaultPrevented"));
        assert!(js.contains("__LEADLAB_VOLLBILD__") && js.contains("configurable:false"));
        assert!(js.contains("min-height:44px"), "Bedienziel ≥ 44 px");
        assert!(js.contains("right:16px;bottom:14px"), "unten rechts wie in der Desktop-App");
        assert_eq!(zustand_skript(true), "window.__LEADLAB_VOLLBILD__&&window.__LEADLAB_VOLLBILD__(true);");
    }
}
