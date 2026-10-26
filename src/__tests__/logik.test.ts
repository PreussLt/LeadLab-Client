import { describe, expect, it } from 'vitest'
import katalogJson from '../../schema/beispiel-katalog.json'
import konfigJson from '../../schema/beispiel-leadlab-client.json'
import { type ClientKonfig, type Katalog, anmeldungNoetig, aufgeloest, baum, gesamt, gesperrt, groesse, nachServer, umschalten, vorauswahl } from '../logik'

const k = katalogJson as unknown as Katalog
const konfig = konfigJson as unknown as ClientKonfig

describe('groesse', () => {
  it('formatiert deutsch mit Dezimal-Einheiten', () => {
    expect(groesse(0)).toBe('0 B')
    expect(groesse(999)).toBe('999 B')
    expect(groesse(48_213_004)).toBe('48,2 MB')
    expect(groesse(1_530_000_000)).toBe('1,5 GB')
    expect(groesse(-1)).toBe('–')
  })
})

describe('baum', () => {
  it('ordnet Region › Gruppe in fester Gruppenreihenfolge', () => {
    const b = baum(k)
    expect(b.map((r) => r.region.id)).toEqual(['lk_sigmaringen', 'lk_rottweil'])
    expect(b[0].gruppen.map((g) => g.id)).toEqual(['gelaende', 'karte', 'luftbild', 'gebaeude'])
  })
  it('nimmt Pakete ohne Regionseintrag und unbekannte Gruppen mit', () => {
    const extra: Katalog = { ...k, pakete: [...k.pakete, { ...k.pakete[0], id: 'x', region: 'neu', gruppe: 'sonstiges' }] }
    const r = baum(extra).find((x) => x.region.id === 'neu')!
    expect(r.gruppen.map((g) => g.id)).toEqual(['sonstiges'])
  })
})

describe('Auswahl', () => {
  it('Region bringt Grundpakete, Detail zieht die Übersicht nach', () => {
    const a = { regionen: ['lk_sigmaringen'], pakete: ['karte-osm.lk_sigmaringen.z15-17'] }
    expect([...aufgeloest(k, a)].sort()).toEqual([
      'gelaende-dgm.lk_sigmaringen', 'karte-osm.lk_sigmaringen.z15-17', 'karte-osm.lk_sigmaringen.z5-14',
    ])
  })
  it('Grundpakete und gebrauchte Pakete sind gesperrt, freie nicht', () => {
    const a = { regionen: ['lk_sigmaringen'], pakete: ['karte-osm.lk_sigmaringen.z15-17'] }
    expect(gesperrt(k, a, 'gelaende-dgm.lk_sigmaringen')).toBe(true)
    expect(gesperrt(k, a, 'karte-osm.lk_sigmaringen.z5-14')).toBe(true)
    expect(gesperrt(k, a, 'karte-osm.lk_sigmaringen.z15-17')).toBe(false)
    expect(gesperrt(k, a, 'luftbild-dop.lk_sigmaringen.z10-16')).toBe(false)
  })
  it('Vorauswahl trifft den Landkreis des Servers', () => {
    expect(vorauswahl(k, konfig, null)).toEqual({ regionen: ['lk_sigmaringen'], pakete: [] })
    const rw = { ...konfig, marke: { landkreis: 'Rottweil' } }
    expect(vorauswahl(k, rw, null).regionen).toEqual(['lk_rottweil'])
  })
  it('gespeicherte Auswahl gewinnt, Verschwundenes fällt heraus', () => {
    const a = vorauswahl(k, konfig, { regionen: ['lk_rottweil', 'weg'], pakete: ['gibt-es-nicht', 'gebaeude-lod2.lk_sigmaringen'] })
    expect(a).toEqual({ regionen: ['lk_rottweil'], pakete: ['gebaeude-lod2.lk_sigmaringen'] })
  })
  it('umschalten fügt ohne Doppel hinzu und entfernt', () => {
    expect(umschalten(['a'], 'a', true)).toEqual(['a'])
    expect(umschalten(['a', 'b'], 'a', false)).toEqual(['b'])
  })
})

describe('gesamt', () => {
  it('summiert und begrenzt den Anteil', () => {
    const m = new Map([['a', { geladen: 50, gesamt: 100 }], ['b', { geladen: 0, gesamt: 100 }]])
    expect(gesamt(m)).toEqual({ geladen: 50, gesamt: 200, anteil: 0.25 })
    expect(gesamt(new Map()).anteil).toBe(0)
  })
})

describe('Anmeldung (CS4)', () => {
  const k = konfigJson as unknown as ClientKonfig
  it('verlangt sie bei zugang=anmeldung', () => {
    expect(anmeldungNoetig(k)).toBe(true)
    expect(nachServer(k)).toBe('anmeldung')
  })
  it('überspringt sie bei zugang=offen und ohne Server', () => {
    const offen = { ...k, daten: { ...k.daten, zugang: 'offen' as const } }
    expect(anmeldungNoetig(offen)).toBe(false)
    expect(nachServer(offen)).toBe('daten')
    expect(nachServer(null)).toBe('daten')
  })
})
