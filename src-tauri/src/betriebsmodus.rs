//! Betriebsmodus: den Regieplatz auf drei Bildschirme einrichten (Hauptrepo
//! #1377, Vorbild `apps/desktop/leadlab_desktop/betriebsmodus.py`, Konzept
//! Übungseinsatz und Rollen §5.1).
//!
//! Der Regieplatz braucht drei Bildschirme: Cockpit zum Bedienen, Lagekarte als
//! Dauerbild, Simulation zum Mitlesen. Unter Wayland darf ein Programm seine
//! Fenster nicht selbst auf einen Monitor legen, und Vollbild, die einzige
//! Betriebsart mit Monitorangabe, wird auf dem Regieplatz schwarz, sobald der
//! Mauszeiger den Monitor verlässt (#165). Darum gilt wie in der Desktop-App:
//! **platzieren tut der Mensch, zuordnen der Client.**
//!
//! Für Lagekarte und Simulation geht je ein rahmenloses Fenster mit großer
//! Nummer auf. Die Übungsleitung zieht es auf seinen Monitor und bestätigt mit
//! der Leertaste, dann maximiert es sich dort. Die Übersicht lässt erst starten,
//! wenn jede Rolle auf einem eigenen Bildschirm steht. Danach warten die Fenster
//! als Bereitschirme. Was ausgegliedert wird, wandert **in** sie hinein, erst
//! Lagekarte, dann Simulation. „✕ Freigeben“ bzw. Strg+W oder `window.close()`
//! der Seite (Übungsende, #181) gibt den Bildschirm zurück.
//!
//! Hier stehen die reinen Teile: Rollen, Prüfungen, Verteilung, Zuweisung und
//! das Skript für entfernte Inhalte im Rollenfenster. Die Fenster baut `lib.rs`.

use serde::Serialize;
use url::Url;

/// Startzentrale fordert den Betriebsmodus an (`BETRIEBSMODUS_PFAD` in
/// `apps/gemeinsam/src/desktopApp.js`). Der Client fängt die Navigation ab.
pub const PFAD: &str = "/betriebsmodus";
/// Pseudo-Adresse: Inhalt des Rollenfensters freigeben (Knopf, Strg+W, `window.close()`).
pub const ADRESSE_FREIGEBEN: &str = "leadlab://rolle-freigeben";
/// Präfix der Rollenfenster; dahinter steht die Nummer (2, 3).
pub const PRAEFIX: &str = "rolle-";
/// Die Übersicht auf dem Bedienbildschirm.
pub const EINRICHTUNG: &str = "betrieb-einrichtung";
/// Lokale Seite für Rollenfenster und Übersicht.
pub const SEITE: &str = "betrieb.html";

/// Was auf einem Bildschirm des Regieplatzes steht (§5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Rolle {
    pub name: &'static str,
    pub zweck: &'static str,
}

/// Der Grundaufbau, immer diese drei in dieser Reihenfolge, Nummer = Stelle + 1.
pub const ROLLEN: [Rolle; 3] = [
    Rolle { name: "Regie-Cockpit", zweck: "Bedienbildschirm – Maus und Tastatur gehören hierher" },
    Rolle { name: "Lagekarte", zweck: "Dauerbild der Lage, direkt neben dem Cockpit" },
    Rolle { name: "Simulation", zweck: "Nur lesend – Funker und Übungsleitung" },
];

pub fn noetige_bildschirme() -> usize {
    ROLLEN.len()
}

/// `None` heißt in Ordnung, sonst steht hier der Satz für den Nutzer. Mehr
/// Bildschirme als nötig sind kein Fehler (Beamer, §5.1).
pub fn fehlt_ein_bildschirm(anzahl: usize) -> Option<String> {
    let noetig = noetige_bildschirme();
    if anzahl >= noetig {
        return None;
    }
    Some(format!(
        "Der Betriebsmodus braucht {noetig} Bildschirme – angeschlossen {} {anzahl}. \
         Bitte die fehlenden anschließen und erneut starten.",
        if anzahl == 1 { "ist" } else { "sind" }
    ))
}

/// Ist das die Anforderung aus der Startzentrale? Nur auf den eigenen Hosts,
/// eine fremde Seite mit demselben Pfad richtet nichts ein.
pub fn ist_anforderung(u: &Url, hosts: &[Url]) -> bool {
    matches!(u.scheme(), "http" | "https")
        && u.path().trim_end_matches('/') == PFAD
        && u.host_str().is_some()
        && hosts.iter().any(|h| h.host_str() == u.host_str())
}

/// Adressen, die auf ein eigenes Gerät gehören und nie einen Bereitschirm
/// belegen (`hauptfenster.py`, `EIGENES_GERAET`, BS4): Beamer-Ansicht des
/// Stabsraums und Einspiel-Bildschirm. Öffnete die Regie die zuerst, nähmen sie
/// der Lagekarte den Monitor weg.
pub fn gehoert_auf_eigenes_geraet(u: &Url) -> bool {
    let pfad = u.path().trim_end_matches('/');
    pfad.ends_with("/stabssicht") || pfad.ends_with("/einspiel")
}

/// Darf diese Adresse in ein Rollenfenster wandern? Nur echte Seiten der
/// eigenen Oberfläche: Druckfenster (`about:blank`) und `blob:` befüllt die
/// aufrufende Seite selbst, die brauchen das verwandte Webview.
pub fn passt_in_rollenfenster(u: &Url) -> bool {
    matches!(u.scheme(), "http" | "https") && !gehoert_auf_eigenes_geraet(u)
}

/// Phase des Betriebsmodus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    #[default]
    Aus,
    /// Die Übersicht steht, die Fenster werden gezogen.
    Einrichten,
    /// Bestätigt: die Bereitschirme warten auf Inhalte.
    Bereit,
}

/// Ein Rollenfenster, wie der Kern es sieht.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Traeger {
    /// Fensterkennung (`rolle-2`).
    pub label: String,
    /// 2 = Lagekarte, 3 = Simulation.
    pub nummer: usize,
    /// Platz bestätigt (Leertaste), Fenster maximiert.
    pub steht: bool,
    /// Ein ausgegliederter Inhalt steht darin.
    pub belegt: bool,
}

impl Traeger {
    pub fn neu(nummer: usize) -> Self {
        Traeger { label: format!("{PRAEFIX}{nummer}"), nummer, steht: false, belegt: false }
    }

    pub fn rolle(&self) -> Rolle {
        ROLLEN[self.nummer - 1]
    }
}

/// Zustand des Betriebsmodus im Kern.
#[derive(Debug, Clone, Default)]
pub struct Betrieb {
    pub phase: Phase,
    pub traeger: Vec<Traeger>,
}

impl Betrieb {
    /// Einrichtung beginnen: je ein Träger für jede Rolle außer dem Cockpit.
    pub fn einrichten() -> Self {
        Betrieb { phase: Phase::Einrichten, traeger: (2..=ROLLEN.len()).map(Traeger::neu).collect() }
    }

    pub fn traeger_mut(&mut self, label: &str) -> Option<&mut Traeger> {
        self.traeger.iter_mut().find(|t| t.label == label)
    }

    /// Nächstes freies Rollenfenster für einen ausgegliederten Inhalt: der
    /// Reihe nach, erst Lagekarte, dann Simulation. `None` ohne bestätigten
    /// Betriebsmodus oder wenn alle belegt sind; dann entsteht wie bisher ein
    /// Nebenfenster.
    pub fn freier_traeger(&self) -> Option<&Traeger> {
        if self.phase != Phase::Bereit {
            return None;
        }
        self.traeger.iter().find(|t| !t.belegt)
    }
}

/// Pseudo-Adresse der Freigabe (`ADRESSE_FREIGEBEN`)?
pub fn ist_freigabe(u: &Url) -> bool {
    u.scheme() == "leadlab" && u.host_str() == Some("rolle-freigeben")
}

/// Eine Zeile der Übersicht.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Platz {
    pub nummer: usize,
    pub rolle: Rolle,
    /// Name des Bildschirms, auf dem das Fenster gerade steht.
    pub schirm: Option<String>,
    pub steht: bool,
}

/// Leerer Text heißt „kann bestätigt werden“. Zwei Rollen auf demselben
/// Bildschirm sind der Fehler, der am Regieplatz wirklich passiert: Ein
/// Fenster wurde nicht gezogen und dort bestätigt, wo es aufging.
pub fn klage(plaetze: &[Platz]) -> String {
    let offen: Vec<&str> = plaetze.iter().filter(|p| !p.steht).map(|p| p.rolle.name).collect();
    if !offen.is_empty() {
        return format!("Es fehlt noch: {} — Fenster auf seinen Bildschirm ziehen und bestätigen.", offen.join(", "));
    }
    let namen: Vec<&str> = plaetze.iter().filter_map(|p| p.schirm.as_deref()).collect();
    let mut doppelt: Vec<&str> = namen.iter().copied().filter(|n| namen.iter().filter(|m| *m == n).count() > 1).collect();
    doppelt.sort();
    doppelt.dedup();
    if !doppelt.is_empty() {
        return format!("Zwei Rollen stehen auf demselben Bildschirm ({}) — eines noch umsetzen.", doppelt.join(", "));
    }
    if plaetze.iter().any(|p| p.schirm.is_none()) {
        return "Ein Bildschirm ist noch nicht erkannt — Fenster kurz anfassen.".into();
    }
    String::new()
}

/// Was die lokale Seite `betrieb.html` anzeigt (spiegelt `src/betrieb.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "art", rename_all = "kebab-case")]
pub enum Ansicht {
    /// Ein Rollenfenster ohne Inhalt.
    Rolle { nummer: usize, rolle: Rolle, schirm: Option<String>, steht: bool, bereit: bool },
    /// Die Übersicht auf dem Bedienbildschirm.
    Einrichtung { plaetze: Vec<Platz>, klage: String },
    /// Nichts mehr zu zeigen (abgebrochen).
    Aus,
}

/// Skript für entfernte Inhalte im Rollenfenster: „✕ Freigeben“ unten rechts,
/// Strg+W und `window.close()` geben den Bildschirm zurück, statt das Fenster zu
/// schließen. F11 bleibt aus, Vollbild würde auf dem Regieplatz schwarz (#165).
/// Die lokale Bereitseite ist ausgenommen.
pub fn skript(ausgenommen: &[String]) -> String {
    let aus = serde_json::to_string(ausgenommen).unwrap_or_else(|_| "[]".into());
    let frei = serde_json::to_string(ADRESSE_FREIGEBEN).expect("Zeichenkette");
    format!(
        r#"(function(){{try{{var aus={aus};if(aus.indexOf(location.origin)>=0||location.protocol==='about:')return;
function frei(){{location.href={frei};}}
try{{window.close=frei;}}catch(e){{}}
var knopf=null;
var STIL='position:fixed;right:16px;bottom:14px;z-index:2147483647;min-height:44px;padding:6px 14px;border-radius:8px;'+
'font:500 13px "Open Sans",system-ui,sans-serif;cursor:pointer;';
function farbe(h){{knopf.style.background=h?'#cc0000':'rgba(17,24,39,0.88)';knopf.style.color=h?'#ffffff':'#9ca3af';
knopf.style.border='1px solid '+(h?'#cc0000':'#1f2937');}}
function zeige(){{if(!document.body){{document.addEventListener('DOMContentLoaded',zeige,{{once:true}});return;}}
if(!knopf){{knopf=document.createElement('button');knopf.type='button';knopf.textContent='✕  Freigeben';
knopf.setAttribute('data-leadlab-client','freigeben');
knopf.title='Diese Anzeige schließen (Strg+W) – der Bildschirm bleibt eingerichtet und wartet wieder';
knopf.setAttribute('aria-label',knopf.title);
knopf.style.cssText=STIL;farbe(false);
knopf.addEventListener('mouseenter',function(){{farbe(true);}});knopf.addEventListener('mouseleave',function(){{farbe(false);}});
knopf.addEventListener('click',frei);}}
if(!knopf.isConnected)document.body.appendChild(knopf);}}
zeige();
window.addEventListener('keydown',function(e){{
if(e.key==='F11'){{e.preventDefault();return;}}
if((e.ctrlKey||e.metaKey)&&!e.altKey&&(e.key==='w'||e.key==='W')){{e.preventDefault();frei();}}}});
}}catch(e){{}}}})();"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn platz(nummer: usize, schirm: Option<&str>, steht: bool) -> Platz {
        Platz { nummer, rolle: ROLLEN[nummer - 1], schirm: schirm.map(String::from), steht }
    }

    #[test]
    fn drei_bildschirme_noetig() {
        assert_eq!(fehlt_ein_bildschirm(3), None);
        assert_eq!(fehlt_ein_bildschirm(4), None, "Beamer darf hängen");
        let t = fehlt_ein_bildschirm(2).unwrap();
        assert!(t.contains("braucht 3 Bildschirme") && t.contains("angeschlossen sind 2"), "{t}");
        assert!(fehlt_ein_bildschirm(1).unwrap().contains("angeschlossen ist 1"));
    }

    #[test]
    fn anforderung_nur_von_eigenen_hosts() {
        let hosts = [u("http://lead.lab/"), u("http://uebung.lead.lab/")];
        assert!(ist_anforderung(&u("http://lead.lab/betriebsmodus"), &hosts));
        assert!(ist_anforderung(&u("https://lead.lab/betriebsmodus/"), &hosts));
        assert!(!ist_anforderung(&u("http://fremd.example.com/betriebsmodus"), &hosts));
        assert!(!ist_anforderung(&u("http://lead.lab/betriebsmodus-hilfe"), &hosts));
        assert!(!ist_anforderung(&u("http://lead.lab/"), &hosts));
        assert!(!ist_anforderung(&u("http://lead.lab/betriebsmodus"), &[]));
    }

    #[test]
    fn eigene_geraete_belegen_keinen_bereitschirm() {
        assert!(!passt_in_rollenfenster(&u("http://uebung.lead.lab/session/7/stabssicht")));
        assert!(!passt_in_rollenfenster(&u("http://uebung.lead.lab/session/7/einspiel")));
        assert!(!passt_in_rollenfenster(&u("about:blank")), "Druckfenster braucht das verwandte Webview");
        assert!(!passt_in_rollenfenster(&u("blob:http://uebung.lead.lab/0b1c")));
        assert!(passt_in_rollenfenster(&u("http://uebung.lead.lab/session/7?tab=karte&popout=1")));
        assert!(passt_in_rollenfenster(&u("http://uebung.lead.lab/session/7/sichtbild")));
    }

    #[test]
    fn inhalte_der_reihe_nach_erst_lagekarte() {
        let mut b = Betrieb::einrichten();
        assert_eq!(b.traeger.iter().map(|t| t.label.as_str()).collect::<Vec<_>>(), ["rolle-2", "rolle-3"]);
        assert_eq!(b.traeger[0].rolle().name, "Lagekarte");
        assert_eq!(b.traeger[1].rolle().name, "Simulation");
        assert_eq!(b.freier_traeger(), None, "vor dem Bestätigen nichts zuweisen");
        b.phase = Phase::Bereit;
        assert_eq!(b.freier_traeger().unwrap().nummer, 2);
        b.traeger_mut("rolle-2").unwrap().belegt = true;
        assert_eq!(b.freier_traeger().unwrap().nummer, 3);
        b.traeger_mut("rolle-3").unwrap().belegt = true;
        assert_eq!(b.freier_traeger(), None, "alles belegt: wieder ein Nebenfenster");
        b.traeger_mut("rolle-2").unwrap().belegt = false;
        assert_eq!(b.freier_traeger().unwrap().nummer, 2, "freigegeben: wieder frei");
    }

    #[test]
    fn klage_wie_die_desktop_app() {
        let alle = [platz(1, Some("DP-6"), true), platz(2, Some("DP-8"), true), platz(3, Some("eDP-1"), true)];
        assert_eq!(klage(&alle), "");
        let offen = [platz(1, Some("DP-6"), true), platz(2, Some("DP-6"), false), platz(3, Some("DP-6"), false)];
        assert!(klage(&offen).starts_with("Es fehlt noch: Lagekarte, Simulation"), "{}", klage(&offen));
        let doppelt = [platz(1, Some("DP-6"), true), platz(2, Some("DP-6"), true), platz(3, Some("DP-8"), true)];
        assert!(klage(&doppelt).contains("demselben Bildschirm (DP-6)"), "{}", klage(&doppelt));
        let unbekannt = [platz(1, Some("DP-6"), true), platz(2, None, true), platz(3, Some("DP-8"), true)];
        assert!(!klage(&unbekannt).is_empty(), "ohne erkannten Schirm nicht bestätigen");
    }

    #[test]
    fn ansicht_als_json_fuer_die_seite() {
        let a = Ansicht::Rolle { nummer: 2, rolle: ROLLEN[1], schirm: Some("DP-8".into()), steht: false, bereit: false };
        let j = serde_json::to_value(&a).unwrap();
        assert_eq!(j["art"], "rolle");
        assert_eq!(j["rolle"]["name"], "Lagekarte");
        assert_eq!(serde_json::to_value(Ansicht::Aus).unwrap()["art"], "aus");
    }

    #[test]
    fn freigabe_adresse() {
        assert!(ist_freigabe(&u(ADRESSE_FREIGEBEN)));
        assert!(!ist_freigabe(&u("leadlab://fenster-schliessen")));
        assert!(!ist_freigabe(&u("https://rolle-freigeben/")));
    }

    #[test]
    fn freigabe_skript() {
        let js = skript(&["tauri://localhost".into()]);
        assert!(js.contains(r#"var aus=["tauri://localhost"]"#), "Bereitseite ausgenommen");
        assert!(js.contains(r#"location.href="leadlab://rolle-freigeben""#));
        assert!(js.contains("window.close=frei"), "Übungsende gibt den Bildschirm frei (#181)");
        assert!(js.contains("Freigeben") && js.contains("Strg+W"));
        assert!(js.contains("e.key==='F11'){e.preventDefault();return;}"), "kein Vollbild im Rollenfenster (#165)");
        assert!(js.contains("min-height:44px"));
    }

    /// Wächter: Rollenfenster und Hauptfenster tragen die Handler, ohne die der
    /// Betriebsmodus still ins Leere läuft.
    #[test]
    fn fenster_tragen_die_handler() {
        let lib = include_str!("lib.rs");
        let block = |name: &str| {
            let start = lib.find(name).unwrap_or_else(|| panic!("{name} fehlt"));
            let ende = lib[start..].find("\nfn ").map(|e| start + e).unwrap_or(lib.len());
            &lib[start..ende]
        };
        assert!(block("fn hauptfenster(").contains("betriebsmodus::ist_anforderung"), "Hauptfenster fängt /betriebsmodus nicht ab");
        let rolle = block("fn rollenfenster_bauen(");
        assert!(rolle.contains(".on_new_window("), "Rollenfenster ohne Handler für neue Fenster");
        assert!(rolle.contains("betriebsmodus::skript"), "Rollenfenster ohne Freigabe-Skript");
        assert!(rolle.contains(".decorations(false)"), "Rollenfenster mit Rahmen");
        assert!(block("fn neues_fenster(").contains("freier_traeger"), "Ausgliedern ignoriert die Bereitschirme");
    }
}
