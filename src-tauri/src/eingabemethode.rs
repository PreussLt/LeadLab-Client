//! Eingabemethode unter Linux (Hauptrepo #811).
//!
//! Viele Desktop-Sitzungen setzen `GTK_IM_MODULE=ibus` (bzw. `fcitx`) und
//! `XMODIFIERS=@im=ibus`, auch wenn kein Daemon der Eingabemethode läuft.
//! GTK-Programme fallen dann still auf die einfache Eingabe zurück, der
//! WebKitGTK-WebView nicht: Klicks setzen den Fokus, Tastendrücke gehen
//! verloren — die Anmeldemaske und die Konfigurationsadresse lassen sich
//! nicht ausfüllen.
//!
//! Der Kern prüft deshalb vor dem Start von GTK (aus `main.rs`), ob der
//! Daemon zur eingestellten Eingabemethode läuft. Läuft er nicht, weicht der
//! Client auf `gtk-im-context-simple` aus und entfernt `XMODIFIERS`. Läuft
//! er, bleibt alles, wie es ist — wer mit einer echten Eingabemethode
//! schreibt (z. B. Japanisch), verliert nichts.
//!
//! [`entscheide`] ist die reine Entscheidung (Umgebung + Prüfung → neue
//! Werte), [`prozess_laeuft`] der getrennte Blick nach `/proc`.

use std::path::Path;

/// Die einfache, eingebaute Eingabe von GTK.
pub const EINFACH: &str = "gtk-im-context-simple";

/// Daemon hinter einem `GTK_IM_MODULE`-Wert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Daemon {
    Ibus,
    Fcitx,
}

impl Daemon {
    /// Prozessnamen (`/proc/<pid>/comm`, höchstens 15 Zeichen), an denen
    /// der Daemon zu erkennen ist.
    pub fn prozessnamen(self) -> &'static [&'static str] {
        match self {
            Daemon::Ibus => &["ibus-daemon"],
            Daemon::Fcitx => &["fcitx5", "fcitx"],
        }
    }
}

/// Welcher Daemon gehört zum Wert von `GTK_IM_MODULE`? Eine Liste
/// (`ibus:gtk-im-context-simple`) zählt mit ihrem ersten Eintrag.
pub fn daemon_fuer(gtk_im_module: &str) -> Option<Daemon> {
    let erster = gtk_im_module.split(':').next().unwrap_or("").trim().to_ascii_lowercase();
    match erster.as_str() {
        "ibus" => Some(Daemon::Ibus),
        "fcitx" | "fcitx5" => Some(Daemon::Fcitx),
        _ => None,
    }
}

/// Neue Werte für die Umgebung, wenn ausgewichen werden muss.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anpassung {
    /// Neuer Wert für `GTK_IM_MODULE`.
    pub gtk_im_module: &'static str,
    /// `XMODIFIERS` entfernen.
    pub xmodifiers_entfernen: bool,
    /// Daemon, der nicht lief (für die Log-Zeile).
    pub fehlender_daemon: Daemon,
}

/// Reine Entscheidung: `None` = Umgebung bleibt unverändert.
///
/// `laeuft` wird nur gefragt, wenn `GTK_IM_MODULE` auf ibus oder fcitx
/// steht.
pub fn entscheide(gtk_im_module: Option<&str>, laeuft: impl Fn(Daemon) -> bool) -> Option<Anpassung> {
    let daemon = daemon_fuer(gtk_im_module?)?;
    if laeuft(daemon) {
        return None;
    }
    Some(Anpassung { gtk_im_module: EINFACH, xmodifiers_entfernen: true, fehlender_daemon: daemon })
}

/// Läuft ein Prozess mit einem der Namen unter der Nutzerkennung `uid`?
/// `proc` ist normalerweise `/proc`; Prozesse anderer Nutzer zählen nicht,
/// deren Daemon bedient diese Sitzung nicht.
#[cfg(unix)]
pub fn prozess_laeuft(proc: &Path, namen: &[&str], uid: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(eintraege) = std::fs::read_dir(proc) else { return false };
    eintraege.flatten().any(|e| {
        let name = e.file_name();
        let Some(name) = name.to_str() else { return false };
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        let pfad = e.path();
        match std::fs::metadata(&pfad) {
            Ok(m) if m.uid() == uid => {}
            _ => return false,
        }
        std::fs::read_to_string(pfad.join("comm"))
            .map(|c| namen.contains(&c.trim_end()))
            .unwrap_or(false)
    })
}

/// Prüft die Umgebung des laufenden Prozesses und passt sie an. Muss vor
/// dem Start von GTK und vor jedem weiteren Thread laufen (`main.rs`).
#[cfg(target_os = "linux")]
pub fn anpassen() {
    use std::os::unix::fs::MetadataExt;
    let proc = Path::new("/proc");
    let wert = std::env::var("GTK_IM_MODULE").ok();
    let uid = std::fs::metadata("/proc/self").map(|m| m.uid()).ok();
    let laeuft = |d: Daemon| match uid {
        Some(uid) => prozess_laeuft(proc, d.prozessnamen(), uid),
        // Ohne lesbares /proc lässt sich nichts sagen — dann nichts ändern.
        None => true,
    };
    match entscheide(wert.as_deref(), laeuft) {
        Some(a) => {
            std::env::set_var("GTK_IM_MODULE", a.gtk_im_module);
            if a.xmodifiers_entfernen {
                std::env::remove_var("XMODIFIERS");
            }
            eprintln!(
                "LeadLab-Client: GTK_IM_MODULE={} ohne laufenden {:?}-Daemon — Eingabe über {} (#811)",
                wert.as_deref().unwrap_or(""),
                a.fehlender_daemon,
                a.gtk_im_module
            );
        }
        None => {
            if cfg!(debug_assertions) {
                eprintln!(
                    "LeadLab-Client: Eingabemethode unverändert (GTK_IM_MODULE={:?}, XMODIFIERS={:?})",
                    wert,
                    std::env::var("XMODIFIERS").ok()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nie(_: Daemon) -> bool {
        false
    }
    fn immer(_: Daemon) -> bool {
        true
    }

    #[test]
    fn ibus_ohne_daemon_weicht_aus() {
        let a = entscheide(Some("ibus"), nie).expect("ausweichen");
        assert_eq!(a.gtk_im_module, EINFACH);
        assert!(a.xmodifiers_entfernen);
        assert_eq!(a.fehlender_daemon, Daemon::Ibus);
    }

    #[test]
    fn fcitx_ohne_daemon_weicht_aus() {
        for w in ["fcitx", "fcitx5", "FCITX"] {
            let a = entscheide(Some(w), nie).expect(w);
            assert_eq!(a.gtk_im_module, EINFACH);
            assert_eq!(a.fehlender_daemon, Daemon::Fcitx);
        }
    }

    #[test]
    fn laufender_daemon_bleibt_unveraendert() {
        assert_eq!(entscheide(Some("ibus"), immer), None);
        assert_eq!(entscheide(Some("fcitx"), immer), None);
    }

    #[test]
    fn geprueft_wird_der_passende_daemon() {
        // Nur fcitx läuft: ibus muss trotzdem ausweichen, fcitx nicht.
        let nur_fcitx = |d: Daemon| d == Daemon::Fcitx;
        assert!(entscheide(Some("ibus"), nur_fcitx).is_some());
        assert_eq!(entscheide(Some("fcitx"), nur_fcitx), None);
    }

    #[test]
    fn andere_werte_und_fehlende_variable_bleiben() {
        let fragt_nicht = |_: Daemon| -> bool { panic!("darf nicht gefragt werden") };
        assert_eq!(entscheide(None, fragt_nicht), None);
        assert_eq!(entscheide(Some(""), fragt_nicht), None);
        assert_eq!(entscheide(Some(EINFACH), fragt_nicht), None);
        assert_eq!(entscheide(Some("xim"), fragt_nicht), None);
        assert_eq!(entscheide(Some("wayland"), fragt_nicht), None);
    }

    #[test]
    fn liste_zaehlt_mit_dem_ersten_eintrag() {
        assert!(entscheide(Some(" ibus:gtk-im-context-simple"), nie).is_some());
        assert_eq!(entscheide(Some("xim:ibus"), nie), None);
    }

    #[cfg(unix)]
    #[test]
    fn prozesssuche_in_proc() {
        use std::os::unix::fs::MetadataExt;
        let d = tempfile::tempdir().unwrap();
        let uid = std::fs::metadata(d.path()).unwrap().uid();
        let prozess = |pid: &str, comm: &str| {
            let p = d.path().join(pid);
            std::fs::create_dir(&p).unwrap();
            std::fs::write(p.join("comm"), format!("{comm}\n")).unwrap();
        };
        prozess("17", "bash");
        prozess("42", "fcitx5");
        // Kein Prozessverzeichnis: zählt nicht, auch wenn comm passt.
        prozess("self", "ibus-daemon");

        assert!(!prozess_laeuft(d.path(), Daemon::Ibus.prozessnamen(), uid));
        assert!(prozess_laeuft(d.path(), Daemon::Fcitx.prozessnamen(), uid));
        // Fremde Nutzerkennung: der Daemon bedient diese Sitzung nicht.
        assert!(!prozess_laeuft(d.path(), Daemon::Fcitx.prozessnamen(), uid.wrapping_add(1)));

        prozess("4711", "ibus-daemon");
        assert!(prozess_laeuft(d.path(), Daemon::Ibus.prozessnamen(), uid));
        // Ohne lesbares Verzeichnis: nichts gefunden.
        assert!(!prozess_laeuft(&d.path().join("fehlt"), &["ibus-daemon"], uid));
    }
}
