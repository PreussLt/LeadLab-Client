import { describe, expect, it, vi } from 'vitest'
import { type Aktionen, type Ansicht, baue, taste } from '../betriebAnsicht'

const LAGE = { name: 'Lagekarte', zweck: 'Dauerbild der Lage, direkt neben dem Cockpit' }
const COCKPIT = { name: 'Regie-Cockpit', zweck: 'Bedienbildschirm' }
const SIM = { name: 'Simulation', zweck: 'Nur lesend' }

function aktionen(): Aktionen {
  return { ziehen: vi.fn(), festlegen: vi.fn(), bestaetigen: vi.fn(), abbrechen: vi.fn() }
}

describe('Rollenfenster (#1377)', () => {
  it('zeigt große Nummer, Rolle und Bildschirm, Knopf legt fest', () => {
    const tun = aktionen()
    const e = baue({ art: 'rolle', nummer: 2, rolle: LAGE, schirm: 'DP-8', steht: false, bereit: false }, tun)
    expect(e.querySelector('.betrieb-nummer')?.textContent).toBe('2')
    expect(e.textContent).toContain('Lagekarte — Dauerbild')
    expect(e.textContent).toContain('Bildschirm DP-8 · noch frei beweglich')
    ;(e.querySelector('button') as HTMLButtonElement).click()
    expect(tun.festlegen).toHaveBeenCalled()
  })

  it('zieht beim Klick ins Fenster, nicht beim Klick auf den Knopf', () => {
    const tun = aktionen()
    const e = baue({ art: 'rolle', nummer: 2, rolle: LAGE, steht: false, bereit: false }, tun)
    e.querySelector('.betrieb-nummer')!.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, button: 0 }))
    expect(tun.ziehen).toHaveBeenCalledTimes(1)
    e.querySelector('button')!.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, button: 0 }))
    expect(tun.ziehen).toHaveBeenCalledTimes(1)
  })

  it('bereit: Marke und Rolle statt Nummer', () => {
    const e = baue({ art: 'rolle', nummer: 3, rolle: SIM, schirm: 'DP-6', steht: true, bereit: true }, aktionen())
    expect(e.querySelector('.betrieb-nummer')).toBeNull()
    expect(e.textContent).toContain('Simulation – bereit')
    expect(e.querySelector('button')).toBeNull()
  })

  it('Leertaste legt fest, nach dem Bestätigen nicht mehr', () => {
    const tun = aktionen()
    const frei: Ansicht = { art: 'rolle', nummer: 2, rolle: LAGE, steht: false, bereit: false }
    expect(taste(frei, { key: ' ' }, tun)).toBe(true)
    expect(tun.festlegen).toHaveBeenCalled()
    expect(taste({ ...frei, bereit: true }, { key: ' ' }, aktionen())).toBe(false)
  })
})

describe('Übersicht (#1377)', () => {
  const plaetze = [
    { nummer: 1, rolle: COCKPIT, schirm: 'eDP-1', steht: true },
    { nummer: 2, rolle: LAGE, schirm: 'DP-6', steht: true },
    { nummer: 3, rolle: SIM, schirm: 'DP-8', steht: false },
  ]

  it('starten erst ohne Klage', () => {
    const tun = aktionen()
    const e = baue({ art: 'einrichtung', plaetze, klage: 'Es fehlt noch: Simulation' }, tun)
    expect(e.textContent).toContain('Für Lagekarte und Simulation ist je ein Fenster')
    expect(e.querySelectorAll('li')).toHaveLength(3)
    expect(e.textContent).toContain('DP-8 · noch nicht bestätigt')
    const start = [...e.querySelectorAll('button')].find((b) => b.textContent === 'Betriebsmodus starten')!
    expect(start.disabled).toBe(true)
    expect(e.textContent).toContain('Es fehlt noch: Simulation')
  })

  it('ohne Klage: alle drei stehen, Start geht', () => {
    const tun = aktionen()
    const e = baue({ art: 'einrichtung', plaetze, klage: '' }, tun)
    expect(e.textContent).toContain('Alle drei Rollen stehen.')
    const start = [...e.querySelectorAll('button')].find((b) => b.textContent === 'Betriebsmodus starten')!
    start.click()
    expect(tun.bestaetigen).toHaveBeenCalled()
  })

  it('Esc bricht ab', () => {
    const tun = aktionen()
    expect(taste({ art: 'einrichtung', plaetze, klage: '' }, { key: 'Escape' }, tun)).toBe(true)
    expect(tun.abbrechen).toHaveBeenCalled()
  })
})
