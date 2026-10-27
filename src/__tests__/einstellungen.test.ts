import { describe, expect, it } from 'vitest'
import { type PaketZeile, aktualisierungen, gruppenTitel, paketTabelle, startverhalten, summeBytes, zeitpunkt, zoomText } from '../logik'

const zeile = (z: Partial<PaketZeile>): PaketZeile => ({
  id: 'x', region: 'Landkreis Sigmaringen', gruppe: 'karte', titel: 'Straßenkarte', version: '2026-10',
  bytes: 0, dateien: 1, vollstaendig: true, zoom: null, geprueft: null, sperre: null, ...z,
})

describe('paketTabelle', () => {
  it('ordnet nach Region, Gruppenreihenfolge des Datenbaums und Titel; ohne Region zuletzt', () => {
    const t = paketTabelle([
      zeile({ id: null, region: null, gruppe: 'gebaeude', titel: 'gebaeude/lod2/alt' }),
      zeile({ id: 'r-osm', region: 'Landkreis Rottweil' }),
      zeile({ id: 's-dop', gruppe: 'luftbild', titel: 'Luftbild' }),
      zeile({ id: 's-dgm', gruppe: 'gelaende', titel: 'Gelände' }),
      zeile({ id: 's-x', gruppe: 'sonstiges', titel: 'A' }),
      zeile({ id: 's-osm' }),
    ])
    expect(t.map((z) => z.id)).toEqual(['r-osm', 's-dgm', 's-osm', 's-dop', 's-x', null])
  })
  it('summiert die Größen', () => {
    expect(summeBytes([zeile({ bytes: 10 }), zeile({ bytes: 32 })])).toBe(42)
    expect(summeBytes([])).toBe(0)
  })
  it('benennt Gruppen wie der Datenbaum', () => {
    expect(gruppenTitel('gebaeude')).toBe('3D-Gebäude')
    expect(gruppenTitel('sonstiges')).toBe('sonstiges')
  })
})

describe('Anzeigehilfen', () => {
  it('zoomText', () => {
    expect(zoomText([5, 14])).toBe('z5–14')
    expect(zoomText([14, 14])).toBe('z14')
    expect(zoomText(null)).toBe('–')
  })
  it('zeitpunkt in Ortszeit, ohne Wert ein Strich', () => {
    expect(zeitpunkt(new Date(2026, 9, 27, 14, 5).getTime() / 1000)).toBe('27.10.2026, 14:05')
    expect(zeitpunkt(null)).toBe('–')
    expect(zeitpunkt(0)).toBe('–')
  })
})

describe('aktualisierungen', () => {
  it('meldet zu ladende und zu entfernende Dateien', () => {
    expect(aktualisierungen({ laden: [{ bytes: 1 }, { bytes: 2 }], loeschen: ['a'], bytes_zu_laden: 210_000_000 }))
      .toEqual({ offen: true, text: 'Aktualisierungen verfügbar: 2 Dateien zu laden (210 MB), 1 Datei zu entfernen.' })
    expect(aktualisierungen({ laden: [], loeschen: [], bytes_zu_laden: 0 }).offen).toBe(false)
  })
})

describe('startverhalten', () => {
  it('Kommandozeile schlägt die Einstellung und sagt es dazu', () => {
    expect(startverhalten(true, {})).toEqual({ modus: 'Vollbild', monitor: 'Monitor mit dem Mauszeiger' })
    expect(startverhalten(true, { vollbild: false })).toEqual({ modus: 'Fenster (diesmal --fenster)', monitor: 'Monitor mit dem Mauszeiger' })
    expect(startverhalten(false, { vollbild: true, monitor: 1 })).toEqual({ modus: 'Vollbild (diesmal --vollbild)', monitor: 'Monitor 2 (--monitor 1)' })
    expect(startverhalten(false, { vollbild: null, monitor: null }).modus).toBe('Fenster')
  })
})
