// Betriebsmodus (Hauptrepo #1377): Rollenfenster und Übersicht.
//
// Eine Seite für zwei Fenster: Der Kern sagt über `betrieb_ansicht`, was das
// aufrufende Fenster zeigt (`Ansicht` in betriebsmodus.rs). Hier steht nur
// das Bauen der Ansicht. Takt, Tasten und Befehle stehen in betrieb.ts.

export interface Rolle { name: string; zweck: string }
export interface Platz { nummer: number; rolle: Rolle; schirm?: string | null; steht: boolean }

/** Spiegelt `betriebsmodus::Ansicht`. */
export type Ansicht =
  | { art: 'rolle'; nummer: number; rolle: Rolle; schirm?: string | null; steht: boolean; bereit: boolean }
  | { art: 'einrichtung'; plaetze: Platz[]; klage: string }
  | { art: 'aus' }

export interface Aktionen {
  ziehen(): void
  festlegen(): void
  bestaetigen(): void
  abbrechen(): void
}

const ZAHLWORT: Record<number, string> = { 3: 'drei', 4: 'vier' }

function el(tag: string, klasse: string, text?: string): HTMLElement {
  const e = document.createElement(tag)
  if (klasse) e.className = klasse
  if (text !== undefined) e.textContent = text
  return e
}

function knopf(text: string, klasse: string, wirkung: () => void, an = true): HTMLButtonElement {
  const b = el('button', klasse, text) as HTMLButtonElement
  b.type = 'button'
  b.disabled = !an
  b.addEventListener('click', wirkung)
  return b
}

/** Ein Rollenfenster ohne Inhalt: erst Nummer zum Ziehen, dann bereit. */
function rolle(a: Extract<Ansicht, { art: 'rolle' }>, tun: Aktionen): HTMLElement {
  const seite = el('section', 'betrieb-rolle')
  seite.dataset.zustand = a.bereit ? 'bereit' : a.steht ? 'steht' : 'frei'
  // Rahmenloses Fenster: Irgendwo hineinklicken und ziehen. Knöpfe ausgenommen,
  // sonst schluckt der Fensterzug den Klick.
  seite.addEventListener('mousedown', (e) => {
    if (e.button === 0 && !(e.target as HTMLElement).closest('button')) tun.ziehen()
  })
  if (a.bereit) {
    seite.append(
      el('div', 'betrieb-marke', 'LeadLab'),
      el('div', 'betrieb-rollenzeile', `${a.rolle.name} – bereit`),
      el('div', 'betrieb-lage', `Bildschirm ${a.schirm ?? '?'}`),
    )
    return seite
  }
  seite.append(
    el('div', 'betrieb-nummer', String(a.nummer)),
    el('div', 'betrieb-rollenzeile', `${a.rolle.name} — ${a.rolle.zweck}`),
    el('div', 'betrieb-lage', `Bildschirm ${a.schirm ?? '?'} · ${a.steht ? 'steht' : 'noch frei beweglich'}`),
    el('p', 'betrieb-hinweis', a.steht
      ? 'Steht. Zum Umsetzen wieder hineinklicken und ziehen, dann erneut Leertaste.'
      : 'Dieses Fenster auf seinen Bildschirm ziehen — irgendwo hineinklicken und bewegen. Dann Leertaste.'),
  )
  if (!a.steht) seite.append(knopf('Hier bleiben  (Leertaste)', 'betrieb-primaer', tun.festlegen))
  return seite
}

/** Die Übersicht: wo steht welche Rolle, starten erst ohne Klage. */
function einrichtung(a: Extract<Ansicht, { art: 'einrichtung' }>, tun: Aktionen): HTMLElement {
  const seite = el('section', 'betrieb-einrichtung')
  const namen = a.plaetze.slice(1).map((p) => p.rolle.name)
  const liste = el('ol', 'betrieb-plaetze')
  for (const p of a.plaetze) {
    const zeile = el('li', 'betrieb-platz')
    zeile.append(
      el('b', '', String(p.nummer)),
      el('span', '', p.rolle.name),
      el('span', 'betrieb-matt', `${p.schirm ?? '—'} · ${p.steht ? 'steht' : 'noch nicht bestätigt'}`),
    )
    liste.append(zeile)
  }
  const anzahl = a.plaetze.length
  const fertig = a.klage === ''
  const reihe = el('div', 'betrieb-knoepfe')
  reihe.append(
    knopf('Abbrechen', 'betrieb-zweitrangig', tun.abbrechen),
    knopf('Betriebsmodus starten', 'betrieb-primaer', tun.bestaetigen, fertig),
  )
  seite.append(
    el('h1', '', 'Bildschirme einrichten'),
    el('p', 'betrieb-matt', `Für ${namen.length > 1 ? namen.slice(0, -1).join(', ') + ' und ' + namen[namen.length - 1] : namen.join('')} `
      + 'ist je ein Fenster mit großer Nummer aufgegangen. Jedes auf seinen Bildschirm ziehen — hineinklicken und bewegen — '
      + 'und dort mit der Leertaste bestätigen.'),
    liste,
    el('p', 'betrieb-klage', fertig ? `Alle ${ZAHLWORT[anzahl] ?? anzahl} Rollen stehen.` : a.klage),
    reihe,
  )
  return seite
}

export function baue(a: Ansicht, tun: Aktionen): HTMLElement {
  if (a.art === 'rolle') return rolle(a, tun)
  if (a.art === 'einrichtung') return einrichtung(a, tun)
  return el('section', 'betrieb-aus', 'Betriebsmodus beendet.')
}

/** Tasten: Leertaste/Enter legen den Platz fest, Esc bricht die Einrichtung ab. */
export function taste(a: Ansicht | null, e: { key: string }, tun: Aktionen): boolean {
  if (!a) return false
  if (a.art === 'rolle' && !a.bereit && (e.key === ' ' || e.key === 'Enter')) { tun.festlegen(); return true }
  if (a.art === 'einrichtung' && e.key === 'Escape') { tun.abbrechen(); return true }
  return false
}
