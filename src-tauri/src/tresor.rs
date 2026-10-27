//! Token-Tresor des Clients (Anmeldedienst AD2, LeadLab #798).
//!
//! Access-, Refresh-(Offline-) und ID-Token liegen mit ihren Ablaufzeiten in
//! einer verschlüsselten Datei `tresor.bin` im **Konfigurationsordner**
//! (neben `einstellungen.json`) — nicht im Datenordner, der auf einem
//! Zweitlaufwerk liegen und kopiert werden kann.
//!
//! - Verschlüsselung: ChaCha20-Poly1305 (AEAD), 12 Byte Zufalls-Nonce je
//!   Schreibvorgang, Dateikopf `LLT1` als zusätzliche Daten.
//! - Geräteschlüssel (32 Byte) in `tresor.schluessel` daneben: unter Windows
//!   per DPAPI (`CryptProtectData`, Nutzerbereich) geschützt, sonst roh mit
//!   Modus 0600. Ohne Geräte- und Nutzerzugang ist die Datei wertlos; mit
//!   beidem entspricht sie dem früheren Schlüsselbund-Eintrag.
//! - Atomar schreiben (`.neu` + rename) wie `einstellungen.rs`.
//! - Kaputte, fremde oder mit einem anderen Schlüssel verschlüsselte Datei:
//!   gilt als „nicht angemeldet“, kein Absturz.

use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde::{Deserialize, Serialize};

use crate::fehler::Fehler;

pub const DATEI: &str = "tresor.bin";
pub const SCHLUESSEL_DATEI: &str = "tresor.schluessel";
/// Dateikopf und Fassung; geht als zusätzliche Daten in die Prüfsumme ein.
const KOPF: &[u8; 4] = b"LLT1";
const NONCE_LAENGE: usize = 12;

/// Inhalt des Tresors: eine Anmeldung an genau einem Anmeldedienst.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Eintrag {
    pub issuer: String,
    pub client_id: String,
    /// Kontoname aus dem ID-Token (nur zur Anzeige).
    #[serde(default)]
    pub konto: Option<String>,
    #[serde(default)]
    pub access_token: String,
    /// Ablauf des Access-Tokens (Unix-Sekunden); 0 = sofort erneuern.
    #[serde(default)]
    pub access_bis: u64,
    pub refresh_token: String,
    /// Offline-Token (`typ: Offline`, Scope `offline_access`).
    #[serde(default)]
    pub refresh_offline: bool,
    /// Ablauf des Refresh-Tokens, sofern aus dem Token lesbar (bei
    /// Offline-Tokens die Leerlaufgrenze, sie rückt mit jeder Erneuerung vor).
    #[serde(default)]
    pub refresh_bis: Option<u64>,
    #[serde(default)]
    pub id_token: Option<String>,
    /// Zeitpunkt der Anmeldung im Browser (Unix-Sekunden).
    #[serde(default)]
    pub angemeldet_seit: u64,
}

/// Woher der Dateischlüssel kommt.
pub enum Schluessel {
    /// Geräteschlüssel aus `tresor.schluessel` (wird beim ersten Schreiben angelegt).
    Geraet,
    /// Fester Schlüssel (Tests).
    Fest([u8; 32]),
}

/// Der Tresor im Konfigurationsordner.
pub struct Tresor {
    ordner: PathBuf,
    schluessel: Schluessel,
}

impl Tresor {
    pub fn neu(ordner: impl Into<PathBuf>, schluessel: Schluessel) -> Self {
        Tresor { ordner: ordner.into(), schluessel }
    }

    pub fn pfad(&self) -> PathBuf {
        self.ordner.join(DATEI)
    }

    fn schluessel(&self, anlegen: bool) -> Result<Option<[u8; 32]>, Fehler> {
        match &self.schluessel {
            Schluessel::Fest(k) => Ok(Some(*k)),
            Schluessel::Geraet => geraeteschluessel(&self.ordner.join(SCHLUESSEL_DATEI), anlegen),
        }
    }

    /// Liest den Eintrag. `None`: keine Datei, unlesbar, falscher Schlüssel
    /// oder fremder Inhalt — dann gilt der Arbeitsplatz als nicht angemeldet.
    pub fn lesen(&self) -> Option<Eintrag> {
        let roh = std::fs::read(self.pfad()).ok()?;
        let k = self.schluessel(false).ok().flatten()?;
        entschluesseln(&k, &roh)
    }

    pub fn schreiben(&self, e: &Eintrag) -> Result<(), Fehler> {
        std::fs::create_dir_all(&self.ordner)?;
        let k = self.schluessel(true)?.ok_or_else(|| Fehler::Eingabe("Tresor: kein Geräteschlüssel.".into()))?;
        let roh = verschluesseln(&k, e)?;
        let neu = self.ordner.join(format!("{DATEI}.neu"));
        datei_privat_schreiben(&neu, &roh)?;
        std::fs::rename(neu, self.pfad())?;
        Ok(())
    }

    /// Löscht den Tresor (Abmelden, abgelehnte Erneuerung). Der
    /// Geräteschlüssel bleibt für die nächste Anmeldung liegen.
    pub fn loeschen(&self) -> Result<(), Fehler> {
        match std::fs::remove_file(self.pfad()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

pub fn verschluesseln(k: &[u8; 32], e: &Eintrag) -> Result<Vec<u8>, Fehler> {
    let klar = serde_json::to_vec(e).expect("serialisierbar");
    let mut nonce = [0u8; NONCE_LAENGE];
    getrandom::fill(&mut nonce).map_err(|e| Fehler::Eingabe(format!("Zufallsquelle: {e}")))?;
    let c = ChaCha20Poly1305::new(Key::from_slice(k));
    let geheim = c
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: &klar, aad: KOPF })
        .map_err(|_| Fehler::Eingabe("Tresor: Verschlüsselung gescheitert.".into()))?;
    let mut roh = Vec::with_capacity(KOPF.len() + NONCE_LAENGE + geheim.len());
    roh.extend_from_slice(KOPF);
    roh.extend_from_slice(&nonce);
    roh.extend_from_slice(&geheim);
    Ok(roh)
}

pub fn entschluesseln(k: &[u8; 32], roh: &[u8]) -> Option<Eintrag> {
    if roh.len() < KOPF.len() + NONCE_LAENGE || &roh[..KOPF.len()] != KOPF {
        return None;
    }
    let (nonce, geheim) = roh[KOPF.len()..].split_at(NONCE_LAENGE);
    let c = ChaCha20Poly1305::new(Key::from_slice(k));
    let klar = c.decrypt(Nonce::from_slice(nonce), Payload { msg: geheim, aad: KOPF }).ok()?;
    serde_json::from_slice(&klar).ok()
}

// ── Datei mit engen Rechten ────────────────────────────────────────────────

#[cfg(unix)]
fn datei_privat_schreiben(pfad: &Path, inhalt: &[u8]) -> Result<(), Fehler> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(pfad)?;
    // Eine schon vorhandene Datei behält sonst ihre alten Rechte.
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(inhalt)?;
    f.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn datei_privat_schreiben(pfad: &Path, inhalt: &[u8]) -> Result<(), Fehler> {
    // Unter Windows liegt der Konfigurationsordner im Profil des Nutzers
    // (%APPDATA%), dessen ACL schon nur ihn zulässt; der Schlüssel ist
    // zusätzlich per DPAPI an das Nutzerkonto gebunden.
    std::fs::write(pfad, inhalt)?;
    Ok(())
}

// ── Geräteschlüssel ────────────────────────────────────────────────────────

/// Liest den Geräteschlüssel; fehlt er (oder ist er kaputt) und `anlegen`
/// ist gesetzt, wird ein neuer erzeugt. Ein neuer Schlüssel macht einen
/// alten Tresor unlesbar — der gilt dann als „nicht angemeldet“.
fn geraeteschluessel(pfad: &Path, anlegen: bool) -> Result<Option<[u8; 32]>, Fehler> {
    if let Ok(roh) = std::fs::read(pfad) {
        if let Some(k) = schluessel_auspacken(&roh) {
            return Ok(Some(k));
        }
    }
    if !anlegen {
        return Ok(None);
    }
    let mut k = [0u8; 32];
    getrandom::fill(&mut k).map_err(|e| Fehler::Eingabe(format!("Zufallsquelle: {e}")))?;
    if let Some(o) = pfad.parent() {
        std::fs::create_dir_all(o)?;
    }
    let neu = pfad.with_extension("neu");
    datei_privat_schreiben(&neu, &schluessel_einpacken(&k)?)?;
    std::fs::rename(neu, pfad)?;
    Ok(Some(k))
}

#[cfg(not(windows))]
fn schluessel_einpacken(k: &[u8; 32]) -> Result<Vec<u8>, Fehler> {
    Ok(k.to_vec())
}

#[cfg(not(windows))]
fn schluessel_auspacken(roh: &[u8]) -> Option<[u8; 32]> {
    roh.try_into().ok()
}

#[cfg(windows)]
fn schluessel_einpacken(k: &[u8; 32]) -> Result<Vec<u8>, Fehler> {
    dpapi::schuetzen(k).map_err(|e| Fehler::Eingabe(format!("Tresor: DPAPI ({e}).")))
}

#[cfg(windows)]
fn schluessel_auspacken(roh: &[u8]) -> Option<[u8; 32]> {
    dpapi::freigeben(roh).ok()?.as_slice().try_into().ok()
}

/// DPAPI im Nutzerbereich: Nur dasselbe Windows-Konto entschlüsselt.
#[cfg(windows)]
mod dpapi {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    fn blob(daten: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: daten.len() as u32, pbData: daten.as_ptr() as *mut u8 }
    }

    unsafe fn uebernehmen(aus: &CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let v = std::slice::from_raw_parts(aus.pbData, aus.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(aus.pbData as *mut core::ffi::c_void)));
        v
    }

    pub fn schuetzen(daten: &[u8]) -> windows::core::Result<Vec<u8>> {
        let ein = blob(daten);
        let mut aus = CRYPT_INTEGER_BLOB::default();
        unsafe {
            CryptProtectData(&ein, windows::core::w!("LeadLab-Client Tresor"), None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut aus)?;
            Ok(uebernehmen(&aus))
        }
    }

    pub fn freigeben(daten: &[u8]) -> windows::core::Result<Vec<u8>> {
        let ein = blob(daten);
        let mut aus = CRYPT_INTEGER_BLOB::default();
        unsafe {
            CryptUnprotectData(&ein, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut aus)?;
            Ok(uebernehmen(&aus))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eintrag() -> Eintrag {
        Eintrag {
            issuer: "https://auth.x.de/realms/leadlab".into(),
            client_id: "leadlab-client".into(),
            konto: Some("kbm.mueller".into()),
            access_token: "a1".into(),
            access_bis: 1_800_000_300,
            refresh_token: "r1".into(),
            refresh_offline: true,
            refresh_bis: Some(1_802_592_000),
            id_token: Some("i1".into()),
            angemeldet_seit: 1_800_000_000,
        }
    }

    #[test]
    fn rundreise_mit_geraeteschluessel() {
        let d = tempfile::tempdir().unwrap();
        let t = Tresor::neu(d.path(), Schluessel::Geraet);
        assert_eq!(t.lesen(), None);
        t.schreiben(&eintrag()).unwrap();
        assert_eq!(t.lesen(), Some(eintrag()));
        // Ein zweiter Tresor auf demselben Ordner (Neustart) liest denselben Eintrag.
        assert_eq!(Tresor::neu(d.path(), Schluessel::Geraet).lesen(), Some(eintrag()));
        // Kein Klartext in der Datei.
        let roh = std::fs::read(t.pfad()).unwrap();
        assert!(roh.starts_with(KOPF));
        assert!(!String::from_utf8_lossy(&roh).contains("kbm.mueller"));
        assert!(!d.path().join(format!("{DATEI}.neu")).exists());
        t.loeschen().unwrap();
        assert_eq!(t.lesen(), None);
        t.loeschen().unwrap(); // zweimal ist kein Fehler
    }

    #[test]
    fn kaputte_oder_fremde_datei_heisst_nicht_angemeldet() {
        let d = tempfile::tempdir().unwrap();
        let t = Tresor::neu(d.path(), Schluessel::Fest([7; 32]));
        std::fs::write(t.pfad(), b"").unwrap();
        assert_eq!(t.lesen(), None);
        std::fs::write(t.pfad(), b"LLT1kurz").unwrap();
        assert_eq!(t.lesen(), None);
        std::fs::write(t.pfad(), br#"{"refresh_token":"klartext"}"#).unwrap();
        assert_eq!(t.lesen(), None);
        // Gültig verschlüsselt, aber ein Bit gekippt → Prüfsumme schlägt an.
        let mut roh = verschluesseln(&[7; 32], &eintrag()).unwrap();
        let n = roh.len() - 1;
        roh[n] ^= 1;
        std::fs::write(t.pfad(), &roh).unwrap();
        assert_eq!(t.lesen(), None);
        // Und danach lässt er sich normal neu schreiben.
        t.schreiben(&eintrag()).unwrap();
        assert_eq!(t.lesen(), Some(eintrag()));
    }

    #[test]
    fn falscher_schluessel() {
        let d = tempfile::tempdir().unwrap();
        Tresor::neu(d.path(), Schluessel::Fest([1; 32])).schreiben(&eintrag()).unwrap();
        assert_eq!(Tresor::neu(d.path(), Schluessel::Fest([2; 32])).lesen(), None);
        // Geräteschlüssel fehlt → nicht anlegen beim Lesen, nur „nicht angemeldet“.
        assert_eq!(Tresor::neu(d.path(), Schluessel::Geraet).lesen(), None);
        assert!(!d.path().join(SCHLUESSEL_DATEI).exists());
    }

    #[test]
    fn kaputter_geraeteschluessel_wird_ersetzt() {
        let d = tempfile::tempdir().unwrap();
        let t = Tresor::neu(d.path(), Schluessel::Geraet);
        t.schreiben(&eintrag()).unwrap();
        std::fs::write(d.path().join(SCHLUESSEL_DATEI), b"zu kurz").unwrap();
        assert_eq!(t.lesen(), None);
        t.schreiben(&eintrag()).unwrap();
        assert_eq!(t.lesen(), Some(eintrag()));
    }

    #[cfg(unix)]
    #[test]
    fn rechte_0600() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let t = Tresor::neu(d.path(), Schluessel::Geraet);
        t.schreiben(&eintrag()).unwrap();
        for name in [DATEI, SCHLUESSEL_DATEI] {
            let modus = std::fs::metadata(d.path().join(name)).unwrap().permissions().mode() & 0o777;
            assert_eq!(modus, 0o600, "{name}");
        }
        assert_eq!(std::fs::read(d.path().join(SCHLUESSEL_DATEI)).unwrap().len(), 32);
    }

    #[test]
    fn alte_felder_fehlen() {
        // Ein Eintrag aus der Migration (nur Refresh-Token) bleibt lesbar.
        let k = [3; 32];
        let e = Eintrag { issuer: "i".into(), client_id: "c".into(), refresh_token: "r".into(), ..Default::default() };
        assert_eq!(entschluesseln(&k, &verschluesseln(&k, &e).unwrap()), Some(e));
    }
}
