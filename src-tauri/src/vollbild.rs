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

/// Monitor fürs Vollbild wie `waehle_schirm` der Desktop-App:
/// `--monitor N`, sonst der mit dem Mauszeiger, sonst der erste.
pub fn waehle_monitor<T: Clone>(schirme: &[T], wunsch: Option<usize>, zeiger: Option<T>) -> Option<T> {
    if let Some(n) = wunsch {
        if n < schirme.len() {
            return Some(schirme[n].clone());
        }
    }
    zeiger.or_else(|| schirme.first().cloned())
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
'font:500 13px Jost,system-ui,sans-serif;cursor:pointer;';
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
        let schirme = ["links", "mitte", "rechts"];
        assert_eq!(waehle_monitor(&schirme, Some(2), Some("links")), Some("rechts"), "--monitor gewinnt");
        assert_eq!(waehle_monitor(&schirme, Some(0), Some("mitte")), Some("links"));
        assert_eq!(waehle_monitor(&schirme, None, Some("mitte")), Some("mitte"), "sonst der Mauszeiger");
        assert_eq!(waehle_monitor(&schirme, Some(7), Some("mitte")), Some("mitte"), "ungültige Nummer: Mauszeiger");
        assert_eq!(waehle_monitor(&schirme, Some(7), None), Some("links"), "sonst der erste");
        assert_eq!(waehle_monitor::<&str>(&[], Some(0), None), None);
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
