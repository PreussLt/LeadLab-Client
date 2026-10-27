// Unter Windows ohne zusätzliches Konsolenfenster starten.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Vor GTK und jedem weiteren Thread: ohne laufenden ibus-/fcitx-Daemon
    // nimmt WebKitGTK sonst keine Tastatureingabe an (#811).
    #[cfg(target_os = "linux")]
    leadlab_client_lib::eingabemethode::anpassen();
    leadlab_client_lib::run()
}
