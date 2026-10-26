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
    expect(() => fuerServer({ version: '1', platforms: { a: { url: 'https://x/y' } } }, 'https://x.de/')).toThrow(/signature/)
  })
})
