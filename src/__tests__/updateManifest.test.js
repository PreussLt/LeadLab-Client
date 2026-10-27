// Update-Manifest für den eigenen Server (CS9, Hauptrepo #764).
import { describe, expect, it } from 'vitest'
import { benoetigteDateien, fuerServer } from '../../werkzeug/update-manifest.mjs'

// So legt tauri-action latest.json an (includeUpdaterJson).
const LATEST = {
  version: '0.2.0',
  notes: 'Anmeldung und lokale Pakete',
  pub_date: '2026-10-26T20:00:00Z',
  platforms: {
    'windows-x86_64': { signature: 'c2lnLXdpbg==', url: 'https://github.com/PreussLt/LeadLab-Client/releases/download/v0.2.0/LeadLab_0.2.0_x64-setup.exe' },
    'linux-x86_64': { signature: 'c2lnLWxpbg==', url: 'https://github.com/PreussLt/LeadLab-Client/releases/download/v0.2.0/LeadLab_0.2.0_amd64.AppImage' },
  },
}

describe('update-manifest', () => {
  it('schreibt die Adressen auf den Server um, Signaturen bleiben', () => {
    const m = fuerServer(LATEST, 'https://leadlab.example.de/daten/client')
    expect(m.version).toBe('0.2.0')
    expect(m.notes).toBe(LATEST.notes)
    expect(m.platforms['windows-x86_64']).toEqual({
      signature: 'c2lnLXdpbg==', url: 'https://leadlab.example.de/daten/client/LeadLab_0.2.0_x64-setup.exe' })
    expect(m.platforms['linux-x86_64'].url).toBe('https://leadlab.example.de/daten/client/LeadLab_0.2.0_amd64.AppImage')
    expect(benoetigteDateien(m)).toEqual(['LeadLab_0.2.0_x64-setup.exe', 'LeadLab_0.2.0_amd64.AppImage'])
  })

  it('http nur im LAN, kaputtes Manifest wird abgewiesen', () => {
    expect(() => fuerServer(LATEST, 'http://lead.lab/daten/client/')).not.toThrow()
    expect(() => fuerServer(LATEST, 'http://leadlab.example.de/daten/client/')).toThrow(/https/)
    expect(() => fuerServer({ version: '1' }, 'https://x.de/')).toThrow(/latest.json/)
  })

  it('http auch für eigene LAN-Namen und private Adressen wie im Client (#793)', () => {
    for (const b of ['http://start.ffw.lan/daten/client/', 'http://ffw.lan:8080/daten/client', 'http://leadlab.local/',
      'http://leadlab.internal/', 'http://leadlab.ffw.home.arpa/', 'http://uebung.lead.lab/', 'http://localhost:11214/',
      'http://192.168.1.20/daten/client/', 'http://10.0.0.5:8080/', 'http://127.0.0.1/']) {
      expect(() => fuerServer(LATEST, b), b).not.toThrow()
    }
    for (const b of ['http://lan.example.de/', 'http://ffw-lan.de/', 'http://leadlab/', 'http://8.8.8.8/',
      'http://evillead.lab/', 'ftp://ffw.lan/']) {
      expect(() => fuerServer(LATEST, b), b).toThrow(/https/)
    }
    expect(() => fuerServer({ version: '1', platforms: { a: { url: 'https://x/y' } } }, 'https://x.de/')).toThrow(/signature/)
  })
})
