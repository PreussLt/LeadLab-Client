// Konfigurationsmodus (#792) als Ansicht: main.ts mit nachgebildetem
// Tauri-Kern, Einstieg über #einstellungen wie aus dem Menü.
import { beforeAll, describe, expect, it, vi } from 'vitest'
import konfigJson from '../../schema/beispiel-leadlab-client.json'

vi.mock('../stil.css', () => ({}))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }))

const aufrufe: { cmd: string; args?: unknown }[] = []
const uebersicht = {
  einstellungen: { konfig_adresse: 'lead.lab', konfig: konfigJson, datenordner: '/daten', eingerichtet: true, konfig_geprueft: 1_790_000_000, vollbild: true },
  anmeldung: { noetig: false, angemeldet: false, konto: null },
  datenordner: '/daten', frei: 200_000_000_000, belegt: 1_500_000_000,
  pakete: [
    { id: 'osm', region: 'Landkreis Sigmaringen', gruppe: 'karte', titel: 'Straßenkarte', version: '2026-10', bytes: 500_000_000, dateien: 2, vollstaendig: true, zoom: [5, 16], geprueft: 1_790_000_000, sperre: null },
    { id: 'dgm', region: 'Landkreis Sigmaringen', gruppe: 'gelaende', titel: 'Gelände', version: '2026-09', bytes: 1_000_000_000, dateien: 1, vollstaendig: false, zoom: [8, 12], geprueft: 1_790_000_000, sperre: 'Grundpaket der Region Landkreis Sigmaringen.' },
  ],
  auswahl: { regionen: ['lk_sigmaringen'], pakete: ['osm'] }, katalog_erzeugt: '2026-10-27T10:00:00Z',
  lokal: { adresse: 'http://127.0.0.1:4711', status: 'http://127.0.0.1:4711/t/abc/status', pakete: 2, geliefert: 17, zuletzt_geladen: 'https://lead.lab', zuletzt_geladen_um: 1_790_000_100 },
  programm: {
    version: '0.1.0', plattform: 'windows', update: { verfuegbar: false, hinweis: 'Dieser Client ist ohne Update-Schlüssel gebaut.' },
    vollbild_einstellung: true, argumente: { vollbild: null, monitor: 1 },
    einstellungen_datei: 'C:/cfg/einstellungen.json', zustand_datei: '/daten/zustand.json',
  },
  laeuft: false,
}

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, args?: unknown) => {
    aufrufe.push({ cmd, args })
    if (cmd === 'start_info') return { einstellungen: uebersicht.einstellungen, standard_datenordner: '/daten', version: '0.1.0', plattform: 'windows' }
    if (cmd === 'zustand_uebersicht') return structuredClone(uebersicht)
    return null
  }),
}))

const warte = () => new Promise((r) => setTimeout(r, 0))
const knopf = (text: string) => [...document.querySelectorAll('button')].find((b) => b.textContent === text) as HTMLButtonElement

beforeAll(async () => {
  document.body.innerHTML = '<main id="app"></main>'
  location.hash = '#einstellungen'
  await import('../main')
  for (let i = 0; i < 5; i++) await warte()
})

describe('Einstellungsseite', () => {
  it('zeigt die fünf Abschnitte mit den Werten aus der Übersicht', () => {
    const t = document.body.textContent ?? ''
    expect([...document.querySelectorAll('h2')].map((x) => x.textContent)).toEqual(['Server', 'Anmeldung', 'Daten', 'Lokaler Dienst', 'Programm'])
    expect(t).toContain('lead.lab')
    expect(t).toContain('Angemeldetnein') // Beispielserver nennt einen Anmeldedienst
    expect(knopf('Im Browser anmelden')).toBeTruthy()
    expect(t).toContain('1,5 GB') // belegt
    expect(t).toContain('http://127.0.0.1:4711')
    expect(t).toContain('17 Antworten')
    expect(t).toContain('ohne Update-Schlüssel')
    expect(t).toContain('Vollbild')
    expect(t).toContain('Monitor 2 (--monitor 1)')
    expect(document.querySelector('details pre')?.textContent).toContain('"schema"')
  })

  it('ordnet die Pakettabelle nach Gruppen und sperrt Grundpakete', () => {
    const zeilen = [...document.querySelectorAll('tbody tr')]
    expect(zeilen.map((z) => z.children[2].textContent)).toEqual(['Gelände' + 'unvollständig', 'Straßenkarte'])
    expect(zeilen[0].textContent).toContain('Grundpaket')
    expect(zeilen[1].children[5].textContent).toBe('z5–16')
  })

  it('fragt vor dem Entfernen nach und ruft dann paket_entfernen', async () => {
    knopf('Entfernen').click()
    expect(aufrufe.some((a) => a.cmd === 'paket_entfernen')).toBe(false)
    knopf('Behalten').click()
    expect(knopf('Entfernen')).toBeTruthy()
    knopf('Entfernen').click()
    knopf('500 MB löschen').click()
    await warte()
    expect(aufrufe.find((a) => a.cmd === 'paket_entfernen')?.args).toEqual({ id: 'osm' })
  })

  it('öffnet den Datenordner und die Oberfläche über die vorhandenen Befehle', async () => {
    knopf('Ordner öffnen').click()
    await warte()
    expect(aufrufe.find((a) => a.cmd === 'ordner_oeffnen')?.args).toEqual({ welcher: 'daten' })
    knopf('LeadLab öffnen').click()
    await warte()
    expect(aufrufe.some((a) => a.cmd === 'oberflaeche_oeffnen')).toBe(true)
  })
})
