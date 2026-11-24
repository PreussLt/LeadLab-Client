//! Zoom-Geste auf dem Touchpad (Hauptrepo #1387).
//!
//! WebKitGTK macht aus zwei auseinandergezogenen Fingern selbst eine
//! Seitenvergrößerung: Über einer Karte wuchs die ganze Oberfläche, die Karte
//! bekam nichts mit. Chromium (Qt-Desktop-App, Browser) liefert dieselbe Geste
//! als `wheel` mit `ctrlKey` an die Seite, und darauf zoomt Leaflet die Karte.
//! Eine Einstellung zum Abschalten gibt es in WebKitGTK nicht.
//!
//! Darum fängt der Client das `event`-Signal des Webviews ab, bevor WebKits
//! eigene Behandlung läuft (Klassenhandler, `RUN_LAST`), und gibt die
//! Skalenänderung wie Chromium als `wheel`-Ereignis mit `ctrlKey` an die
//! Stelle unter den Fingern weiter – gesammelt, bis sie für eine Kartenstufe
//! reicht (`SCHWELLE`).

/// Pixel Radweg je Faktor e der Skala. Leaflet zoomt bei kleinen Wegen um etwa
/// `delta / 166` Stufen (`wheelPxPerZoomLevel` 60, Glättung über 4 · ln 2);
/// so entspricht eine Verdopplung der Fingerspanne ungefähr einer Zoomstufe.
const PIXEL_JE_EINHEIT: f64 = 240.0;

/// Radweg, ab dem der gesammelte Weg als ein `wheel` an die Seite geht (etwa
/// 1,4-fache Fingerspanne). Die Karten des Frontends rasten auf ganze Stufen
/// (Leaflet `zoomSnap` 1): Jedes `wheel`-Bündel (Leaflet sammelt 40 ms) springt
/// mindestens eine Stufe. Jedes Pinch-Ereignis einzeln weitergegeben – das
/// Touchpad liefert sie mit 60–140 Hz – raste die Karte so durch alle Stufen.
const SCHWELLE: f64 = 80.0;

/// `deltaY` für eine Skalenänderung um `verhaeltnis` (neu / vorher).
/// Auseinander (> 1) heißt hineinzoomen, also negatives `deltaY`.
pub fn rad_delta(verhaeltnis: f64) -> f64 {
    if !verhaeltnis.is_finite() || verhaeltnis <= 0.0 {
        return 0.0;
    }
    -PIXEL_JE_EINHEIT * verhaeltnis.ln()
}

/// Gesammelter Radweg: `Some(weg)` zum Weitergeben, sobald er die Schwelle
/// erreicht (danach beginnt das Sammeln von vorn), sonst `None`.
pub fn abgeben(summe: f64) -> Option<f64> {
    (summe.abs() >= SCHWELLE).then_some(summe)
}

/// Skript, das das `wheel`-Ereignis am Element unter den Fingern auslöst.
/// `x`/`y` sind Fensterkoordinaten in CSS-Pixeln.
pub fn rad_skript(x: f64, y: f64, delta: f64) -> String {
    format!(
        "(function(){{try{{var x={x:.1},y={y:.1};var z=document.elementFromPoint(x,y)||document.body;if(!z)return;\
z.dispatchEvent(new WheelEvent('wheel',{{clientX:x,clientY:y,deltaY:{delta:.3},deltaMode:0,ctrlKey:true,bubbles:true,cancelable:true,composed:true}}));}}catch(e){{}}}})();"
    )
}

/// Hängt die Umleitung an das Webview des Fensters. Außerhalb von Linux nichts:
/// WebView2 und WKWebView geben die Geste selbst an die Seite.
pub fn umleiten(w: &tauri::WebviewWindow) {
    #[cfg(target_os = "linux")]
    {
        let fenster = w.clone();
        let _ = w.with_webview(move |wv| linux::anhaengen(wv.inner(), fenster));
    }
    #[cfg(not(target_os = "linux"))]
    let _ = w;
}

#[cfg(target_os = "linux")]
mod linux {
    use gtk::gdk;
    use gtk::glib::Propagation;
    use gtk::prelude::*;
    use std::cell::Cell;

    pub fn anhaengen<W: IsA<gtk::Widget>>(view: W, fenster: tauri::WebviewWindow) {
        // Skala ist kumulativ seit Beginn der Geste (Beginn = 1); gesammelt
        // wird die Änderung seit dem letzten Ereignis.
        let zuletzt = Cell::new(1.0f64);
        let gesammelt = Cell::new(0.0f64);
        view.connect_event(move |_, ev| {
            if ev.event_type() != gdk::EventType::TouchpadPinch {
                return Propagation::Proceed;
            }
            if let Some(p) = ev.downcast_ref::<gdk::EventTouchpadPinch>() {
                // Die Phase direkt aus der C-Struktur: gtk-rs 0.18 liest sie in
                // `is_phase()` als bool, Beginn (0) und Ende (2) wären so nicht
                // zu unterscheiden.
                let roh: &gdk::ffi::GdkEventTouchpadPinch = p.as_ref();
                if roh.phase as i32 == gdk::ffi::GDK_TOUCHPAD_GESTURE_PHASE_UPDATE {
                    let skala = p.scale();
                    let summe = gesammelt.get() + super::rad_delta(skala / zuletzt.get());
                    zuletzt.set(skala);
                    if let Some(weg) = super::abgeben(summe) {
                        gesammelt.set(0.0);
                        let (x, y) = p.position();
                        let _ = fenster.eval(&super::rad_skript(x, y, weg));
                    } else {
                        gesammelt.set(summe);
                    }
                } else {
                    // Beginn, Ende, Abbruch: neue Geste fängt bei Skala 1 an,
                    // ein Rest unter der Schwelle verfällt.
                    zuletzt.set(1.0);
                    gesammelt.set(0.0);
                }
            }
            // WebKit bekommt die Geste nie: keine Seitenvergrößerung.
            Propagation::Stop
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auseinander_zoomt_hinein() {
        assert!(rad_delta(1.1) < 0.0, "Finger auseinander: deltaY negativ wie Chromium");
        assert!(rad_delta(0.9) > 0.0);
        assert_eq!(rad_delta(1.0), 0.0);
        // Verdoppeln und Halbieren heben sich auf.
        assert!((rad_delta(2.0) + rad_delta(0.5)).abs() < 1e-9);
        assert!((rad_delta(2.0) + 166.4).abs() < 0.1, "Verdopplung ≈ eine Leaflet-Stufe");
        assert_eq!(rad_delta(0.0), 0.0);
        assert_eq!(rad_delta(f64::NAN), 0.0);
    }

    #[test]
    fn kleine_schritte_werden_gesammelt() {
        // Ein Pinch-Ereignis (Skala +1 %) allein geht nicht an die Seite …
        assert_eq!(abgeben(rad_delta(1.01)), None);
        // … gesammelt bis etwa zur 1,4-fachen Spanne dann als ein `wheel`.
        let mut summe = 0.0;
        let mut abgegeben = Vec::new();
        for _ in 0..40 {
            summe += rad_delta(1.01);
            if let Some(weg) = abgeben(summe) {
                abgegeben.push(weg);
                summe = 0.0;
            }
        }
        assert_eq!(abgegeben.len(), 1, "1,01^40 ≈ 1,49: genau eine Stufe");
        assert!(abgegeben[0] <= -SCHWELLE);
        assert!(abgeben(rad_delta(0.7)).is_some_and(|w| w > 0.0), "Zusammenziehen zoomt hinaus");
    }

    #[test]
    fn skript_loest_wheel_mit_ctrl_aus() {
        let js = rad_skript(120.0, 80.5, -22.871);
        assert!(js.contains("document.elementFromPoint(x,y)"));
        assert!(js.contains("var x=120.0,y=80.5"));
        assert!(js.contains("new WheelEvent('wheel'"));
        assert!(js.contains("deltaY:-22.871"));
        assert!(js.contains("ctrlKey:true") && js.contains("bubbles:true"));
    }

    /// Wächter: Haupt-, Neben- und Rollenfenster leiten die Geste um.
    #[test]
    fn alle_fenster_leiten_um() {
        let lib = include_str!("lib.rs");
        for name in ["fn hauptfenster(", "fn nebenfenster_bauen(", "fn rollenfenster_bauen("] {
            let start = lib.find(name).unwrap_or_else(|| panic!("{name} fehlt"));
            let ende = lib[start..].find("\nfn ").map(|e| start + e).unwrap_or(lib.len());
            assert!(lib[start..ende].contains("geste::umleiten("), "{name} ohne Umleitung der Zoom-Geste");
        }
    }
}
