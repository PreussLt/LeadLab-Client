// Browser-Anmeldung wartet nicht stumm (LeadLab #805): Während `anmelden`
// im Kern auf den Rückruf wartet, zeigt der Schritt „Anmeldung“ die
// Anmeldeadresse mit „Link kopieren“, „Browser erneut öffnen“ und
// „Abbrechen“; nach 20 s ohne Verbindung am Loopback die Ausweichwege bis
// zur Anmeldung im Client-Fenster.
import { beforeAll, describe, expect, it, vi } from 'vitest'
import konfigJson from '../../schema/beispiel-leadlab-client.json'
import { anmeldeFehler, browserStill } from '../logik'

vi.mock('../stil.css', () => ({}))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }))

const ADRESSE = 'https://auth.leadlab.example.de/realms/leadlab/protocol/openid-connect/auth?client_id=leadlab-client&state=s'
const jetzt = () => Math.floor(Date.now() / 1000)
const aus = { noetig: true, angemeldet: false, konto: null }
const kern = {
  warten: null as null | { adresse: string | null; seit: number; verbunden: boolean; still_sek: number; im_fenster?: boolean },
  anmelden: null as null | { ok: (v: unknown) => void; fehler: (e: unknown) => void },
  aufrufe: [] as { cmd: string; args?: unknown }[],
}

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, args?: unknown) => {
    kern.aufrufe.push({ cmd, args })
    if (cmd === 'start_info') {
      return { einstellungen: { konfig_adresse: 'lead.lab', konfig: konfigJson, datenordner: '/daten', eingerichtet: false }, standard_datenordner: '/daten', version: '0.1.0', plattform: 'linux' }
    }
    if (cmd === 'anmeldung_status') return { ...aus, warten: kern.warten }
    if (cmd === 'anmelden') {
      kern.warten = { adresse: ADRESSE, seit: jetzt(), verbunden: false, still_sek: 20 }
      return new Promise((ok, fehler) => { kern.anmelden = { ok, fehler } })
    }
    if (cmd === 'anmeldung_abbrechen') {
      kern.warten = null
      kern.anmelden?.fehler('Abgebrochen.')
      return null
    }
    return null
  }),
}))

const warte = (ms = 0) => new Promise((r) => setTimeout(r, ms))
const knopf = (text: string) => [...document.querySelectorAll('button')].find((b) => b.textContent === text) as HTMLButtonElement | undefined
const text = () => document.getElementById('app')!.textContent ?? ''
const schreiben = vi.fn(async () => {})

beforeAll(async () => {
  Object.defineProperty(navigator, 'clipboard', { value: { writeText: schreiben }, configurable: true })
  document.body.innerHTML = '<main id="app"></main>'
  location.hash = '#anmeldung'
  await import('../main')
  for (let i = 0; i < 5; i++) await warte()
})

describe('Logik', () => {
  it('browserStill: erst nach der Frist ohne Verbindung, sofort bei Startfehler', () => {
    const w = { seit: 1000, verbunden: false, still_sek: 20 }
    expect(browserStill(w, 1019)).toBe(false)
    expect(browserStill(w, 1020)).toBe(true)
    expect(browserStill({ ...w, verbunden: true }, 2000)).toBe(false)
    expect(browserStill({ ...w, im_fenster: true }, 2000)).toBe(false)
    expect(browserStill({ ...w, browser_fehler: 'xdg-open fehlt' }, 1001)).toBe(true)
    expect(browserStill(null, 2000)).toBe(false)
  })
  it('anmeldeFehler: Abbruch ist kein Fehler', () => {
    expect(anmeldeFehler('Abgebrochen.')).toBe('')
    expect(anmeldeFehler('Innerhalb von 5 Minuten kam keine Antwort vom Browser.')).toContain('5 Minuten')
  })
})

describe('Schritt Anmeldung: Warten im Browser', () => {
  it('zeigt Anmeldeadresse, Link kopieren, Browser erneut öffnen und Abbrechen', async () => {
    knopf('Im Browser anmelden')!.click()
    await warte(450)
    const feld = document.getElementById('anmeldeadresse') as HTMLInputElement
    expect(feld.value).toBe(ADRESSE)
    expect(feld.readOnly).toBe(true)
    expect(text()).toContain('im Browserfenster fortgesetzt')
    expect(knopf('Link kopieren')).toBeTruthy()
    expect(knopf('Browser erneut öffnen')!.disabled).toBe(false)
    expect(knopf('Abbrechen')).toBeTruthy()
    expect(knopf('Im Browser anmelden')).toBeUndefined()
    // Vor der Frist noch kein Hinweis.
    expect(text()).not.toContain('Kein Browser hat sich gemeldet')
  })

  it('kopiert den Link in die Zwischenablage', async () => {
    knopf('Link kopieren')!.click()
    await warte()
    expect(schreiben).toHaveBeenCalledWith(ADRESSE)
    expect(knopf('Kopiert')).toBeTruthy()
  })

  it('öffnet den Browser erneut über den Kern', async () => {
    knopf('Browser erneut öffnen')!.click()
    await warte()
    expect(kern.aufrufe.some((a) => a.cmd === 'anmeldung_browser_oeffnen')).toBe(true)
  })

  it('nennt nach 20 s ohne Verbindung die Ausweichwege bis zum Client-Fenster', async () => {
    kern.warten = { ...kern.warten!, seit: jetzt() - 25 }
    await warte(1100)
    expect(text()).toContain('Kein Browser hat sich gemeldet')
    knopf('Im Client-Fenster anmelden')!.click()
    await warte()
    expect(kern.aufrufe.find((a) => a.cmd === 'anmeldung_im_fenster')?.args).toEqual({ zurueck: 'anmeldung' })
  })

  it('Abbrechen beendet das Warten ohne Fehlermeldung', async () => {
    knopf('Abbrechen')!.click()
    for (let i = 0; i < 5; i++) await warte()
    expect(kern.aufrufe.some((a) => a.cmd === 'anmeldung_abbrechen')).toBe(true)
    expect(document.getElementById('anmeldeadresse')).toBeNull()
    expect(knopf('Im Browser anmelden')).toBeTruthy()
    expect(document.querySelector('.meldung.fehler')).toBeNull()
  })

  it('zeigt die Ursache, wenn die Frist abläuft', async () => {
    knopf('Im Browser anmelden')!.click()
    await warte()
    kern.warten = null
    kern.anmelden!.fehler('Innerhalb von 5 Minuten kam keine Antwort vom Browser. Die Anmeldung ist abgebrochen; bitte erneut anmelden.')
    for (let i = 0; i < 5; i++) await warte()
    expect(document.querySelector('.meldung.fehler')?.textContent).toContain('Innerhalb von 5 Minuten kam keine Antwort vom Browser')
    expect(knopf('Im Browser anmelden')).toBeTruthy()
  })
})
