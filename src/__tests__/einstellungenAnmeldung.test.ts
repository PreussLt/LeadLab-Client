// Einstellungsseite, Abschnitt Anmeldung (AD4, LeadLab #800): ein Login für
// Daten und Oberfläche — Konto, angemeldet seit, Daueranmeldung, Ablauf,
// Sitzung der Oberfläche, Tresor-Pfad; „Abmelden“ und „Sitzung in der
// Oberfläche erneuern“ rufen den Rust-Kern.
import { beforeAll, describe, expect, it, vi } from 'vitest'
import konfigJson from '../../schema/beispiel-leadlab-client.json'

vi.mock('../stil.css', () => ({}))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }))

const jetzt = Math.floor(Date.now() / 1000)
const anmeldung = {
  noetig: true, angemeldet: true, konto: 'kbm.mueller', offline: true,
  angemeldet_seit: jetzt - 3 * 86400, refresh_bis: jetzt + 30 * 86400 + 600,
  tresor: '/home/x/.config/de.leadlab.client/tresor.bin', oberflaeche_bis: jetzt + 240,
}
const aufrufe: string[] = []
const uebersicht = {
  einstellungen: { konfig_adresse: 'lead.lab', konfig: konfigJson, datenordner: '/daten', eingerichtet: true, konfig_geprueft: jetzt, vollbild: false },
  anmeldung,
  datenordner: '/daten', frei: null, belegt: 0, pakete: [], auswahl: { regionen: [], pakete: [] }, katalog_erzeugt: null,
  lokal: { adresse: 'http://127.0.0.1:4711', status: 'http://127.0.0.1:4711/t/abc/status', pakete: 0, geliefert: 0, zuletzt_geladen: null, zuletzt_geladen_um: null },
  programm: {
    version: '0.1.0', plattform: 'linux', update: { verfuegbar: false, hinweis: 'aktuell' },
    vollbild_einstellung: false, argumente: { vollbild: null, monitor: null },
    einstellungen_datei: '/home/x/.config/de.leadlab.client/einstellungen.json', zustand_datei: '/daten/zustand.json',
  },
  laeuft: false,
}

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    aufrufe.push(cmd)
    if (cmd === 'start_info') return { einstellungen: uebersicht.einstellungen, standard_datenordner: '/daten', version: '0.1.0', plattform: 'linux' }
    if (cmd === 'zustand_uebersicht') return structuredClone(uebersicht)
    if (cmd === 'abmelden') { uebersicht.anmeldung = { ...anmeldung, angemeldet: false, oberflaeche_bis: null }; return uebersicht.anmeldung }
    if (cmd === 'oberflaeche_sitzung_erneuern') return anmeldung
    return null
  }),
}))

const warte = () => new Promise((r) => setTimeout(r, 0))
const knopf = (text: string) => [...document.querySelectorAll('button')].find((b) => b.textContent === text) as HTMLButtonElement | undefined
const abschnitt = () => document.getElementById('e-anmeldung')!
const wert = (name: string) => {
  const dt = [...abschnitt().querySelectorAll('dt')].find((d) => d.textContent === name)
  return dt?.nextElementSibling?.textContent ?? null
}

beforeAll(async () => {
  document.body.innerHTML = '<main id="app"></main>'
  location.hash = '#einstellungen'
  await import('../main')
  for (let i = 0; i < 5; i++) await warte()
})

describe('Einstellungsseite: Anmeldung', () => {
  it('zeigt Konto, Daueranmeldung, Ablauf, Sitzung der Oberfläche und Tresor', () => {
    expect(abschnitt().textContent).toContain('Eine Anmeldung für Daten und Oberfläche')
    expect(wert('Angemeldet')).toBe('ja')
    expect(wert('Konto')).toBe('kbm.mueller')
    expect(wert('Angemeldet seit')).toMatch(/\d{2}\.\d{2}\.\d{4}/)
    expect(wert('Daueranmeldung')).toBe('ja (Offline-Token)')
    expect(wert('Läuft ab')).toContain('noch 30 Tage')
    expect(wert('Läuft ab')).toContain('rückt mit jeder Nutzung vor')
    expect(wert('Sitzung in der Oberfläche')).toContain('verlängert sich selbst')
    expect(wert('Tresor')).toBe('/home/x/.config/de.leadlab.client/tresor.bin')
  })

  it('erneuert die Sitzung in der Oberfläche über den Rust-Kern', async () => {
    knopf('Sitzung in der Oberfläche erneuern')!.click()
    for (let i = 0; i < 3; i++) await warte()
    expect(aufrufe).toContain('oberflaeche_sitzung_erneuern')
    expect(abschnitt().textContent).toContain('Die Sitzung in der Oberfläche ist erneuert.')
  })

  it('meldet ab und bietet danach die Anmeldung an', async () => {
    knopf('Abmelden')!.click()
    for (let i = 0; i < 3; i++) await warte()
    expect(aufrufe).toContain('abmelden')
    expect(wert('Angemeldet')).toBe('nein')
    expect(knopf('Im Browser anmelden')).toBeTruthy()
    expect(knopf('Sitzung in der Oberfläche erneuern')).toBeUndefined()
  })
})
