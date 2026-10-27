// Anmeldemaske im Schritt „Anmeldung“ (LeadLab #804): Benutzername und
// Passwort gehen per `anmelden_formular` an den Rust-Kern (Passwort-Grant
// gegen Keycloak), der Browser-Weg bleibt als zweiter Knopf. Fehlertexte,
// Fehlversuche bis zur Sperre, Enter schickt ab, nach Erfolg Kontoname und
// automatisch weiter.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { beforeAll, describe, expect, it, vi } from 'vitest'
import konfigJson from '../../schema/beispiel-leadlab-client.json'
import { FORMULAR_FALSCH, SPERRE_NACH, formularFehler, formularZeigen } from '../logik'

vi.mock('../stil.css', () => ({}))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }))

const NUR_BROWSER = 'Dieses Konto meldet sich mit zweitem Faktor an. Bitte „Im Browser anmelden“.'
const aufrufe: { cmd: string; args?: Record<string, string> }[] = []
let stand: Record<string, unknown> = { noetig: true, angemeldet: false, konto: null, formular: true }

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, args?: Record<string, string>) => {
    aufrufe.push({ cmd, args })
    if (cmd === 'start_info') {
      return { einstellungen: { konfig_adresse: 'lead.lab', konfig: konfigJson, datenordner: '/daten', eingerichtet: false }, standard_datenordner: '/daten', version: '0.1.0', plattform: 'linux' }
    }
    if (cmd === 'anmeldung_status') return stand
    if (cmd === 'anmelden_formular') {
      if (args?.benutzer === 'verwaltung') throw NUR_BROWSER
      if (args?.benutzer !== 'kbm.mueller' || args?.passwort !== 'richtig') throw FORMULAR_FALSCH
      stand = { ...stand, angemeldet: true, konto: 'kbm.mueller', offline: true }
      return stand
    }
    if (cmd === 'katalog_laden') throw 'Katalog im Test nicht da.'
    return null
  }),
}))

const warte = (ms = 0) => new Promise((r) => setTimeout(r, ms))
const knopf = (text: string) => [...document.querySelectorAll('button')].find((b) => b.textContent === text) as HTMLButtonElement | undefined
const feld = (id: string) => document.getElementById(id) as HTMLInputElement
const formular = () => document.querySelector('form.formular') as HTMLFormElement
const fehler = () => formular()?.querySelector('.meldung.fehler')?.textContent ?? ''
function tippe(id: string, wert: string) {
  const f = feld(id)
  f.value = wert
  f.dispatchEvent(new Event('input'))
}
async function absenden() {
  // Enter in einem Feld löst im Browser den Submit des Formulars aus.
  formular().requestSubmit()
  for (let i = 0; i < 5; i++) await warte()
}

beforeAll(async () => {
  document.body.innerHTML = '<main id="app"></main>'
  location.hash = '#anmeldung'
  await import('../main')
  for (let i = 0; i < 5; i++) await warte()
})

describe('Logik', () => {
  it('Texte gleich mit anmeldung.rs', () => {
    const rust = readFileSync(resolve(process.cwd(), 'src-tauri/src/anmeldung.rs'), 'utf-8')
    expect(rust).toContain(`pub const FORMULAR_FALSCH: &str = "${FORMULAR_FALSCH}";`)
  })
  it('zählt Fehlversuche bis zur Sperre und lässt andere Fehler stehen', () => {
    expect(formularFehler(FORMULAR_FALSCH, 1)).toBe(FORMULAR_FALSCH)
    expect(formularFehler(FORMULAR_FALSCH, SPERRE_NACH - 2)).toContain('Noch 2 Versuche')
    expect(formularFehler(FORMULAR_FALSCH, SPERRE_NACH - 1)).toContain('Noch 1 Versuch,')
    const gesperrt = formularFehler(FORMULAR_FALSCH, SPERRE_NACH)
    expect(gesperrt).toContain('sperrt der Server das Konto vorübergehend')
    expect(gesperrt).toContain('etwa eine Minute warten')
    expect(formularFehler(NUR_BROWSER, 9)).toBe(NUR_BROWSER)
  })
  it('zeigt die Maske nur ohne Anmeldung und wenn der Server sie erlaubt', () => {
    expect(formularZeigen({ angemeldet: false, formular: true })).toBe(true)
    expect(formularZeigen({ angemeldet: false, formular: false })).toBe(false)
    expect(formularZeigen({ angemeldet: false })).toBe(false)
    expect(formularZeigen({ angemeldet: true, formular: true })).toBe(false)
    expect(formularZeigen(null)).toBe(false)
  })
})

describe('Schritt Anmeldung: Maske', () => {
  it('zeigt Benutzername, Passwort und Anmelden; der Browser-Weg bleibt', () => {
    expect(feld('benutzer').type).toBe('text')
    expect(feld('benutzer').getAttribute('autocomplete')).toBe('username')
    expect(feld('passwort').type).toBe('password')
    expect(feld('passwort').getAttribute('autocomplete')).toBe('current-password')
    expect(document.querySelector('label[for="benutzer"]')?.textContent).toBe('Benutzername')
    expect(document.querySelector('label[for="passwort"]')?.textContent).toBe('Passwort')
    const anmelden = knopf('Anmelden')!
    expect(anmelden.type).toBe('submit')
    expect(formular().contains(anmelden)).toBe(true)
    expect(knopf('Im Browser anmelden')).toBeTruthy()
    expect(knopf('Weiter')).toBeUndefined()
    // Fokus im ersten Feld, Tab-Reihenfolge Benutzername → Passwort → Anmelden.
    expect(document.activeElement).toBe(feld('benutzer'))
    const reihe = [...formular().querySelectorAll('input, button')]
    expect(reihe).toEqual([feld('benutzer'), feld('passwort'), anmelden])
  })

  it('leere Felder schicken nichts ab', async () => {
    await absenden()
    expect(aufrufe.some((a) => a.cmd === 'anmelden_formular')).toBe(false)
    expect(fehler()).toBe('Bitte Benutzername und Passwort eingeben.')
  })

  it('falsches Passwort: Meldung, Eingaben bleiben, ab dem dritten Versuch Warnung vor der Sperre', async () => {
    tippe('benutzer', 'kbm.mueller')
    tippe('passwort', 'falsch')
    await absenden()
    expect(aufrufe.at(-1)).toEqual({ cmd: 'anmelden_formular', args: { benutzer: 'kbm.mueller', passwort: 'falsch' } })
    expect(fehler()).toBe(FORMULAR_FALSCH)
    expect(feld('benutzer').value).toBe('kbm.mueller')
    expect(feld('passwort').value).toBe('falsch')
    expect(document.activeElement).toBe(feld('passwort'))
    await absenden()
    await absenden()
    expect(fehler()).toContain('Noch 2 Versuche')
    await absenden()
    await absenden()
    expect(fehler()).toContain('sperrt der Server das Konto vorübergehend')
  })

  it('Konto mit zweitem Faktor: Verweis auf den Browser-Weg', async () => {
    tippe('benutzer', 'verwaltung')
    await absenden()
    expect(fehler()).toBe(NUR_BROWSER)
  })

  it('Erfolg: Passwort geleert, Kontoname, dann automatisch weiter zu den Daten', async () => {
    tippe('benutzer', 'kbm.mueller')
    tippe('passwort', 'richtig')
    await absenden()
    expect(aufrufe.at(-1)).toEqual({ cmd: 'anmelden_formular', args: { benutzer: 'kbm.mueller', passwort: 'richtig' } })
    expect(feld('passwort')).toBeNull()
    expect(document.querySelector('.meldung.ok')?.textContent).toContain('Angemeldet als kbm.mueller.')
    // Das Passwort liegt nirgends sonst: kein Browser-Speicher, kein Text auf der Seite.
    expect(JSON.stringify({ ...localStorage, ...sessionStorage })).not.toContain('richtig')
    expect(document.body.innerHTML).not.toContain('richtig')
    await warte(1300)
    expect(aufrufe.some((a) => a.cmd === 'katalog_laden')).toBe(true)
    expect(document.querySelector('h1')?.textContent).toBe('Welche Daten sollen auf diesen Rechner?')
  })
})
