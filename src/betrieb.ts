// Lokale Seite der Betriebsmodus-Fenster (Hauptrepo #1377). Fragt den Kern im
// Takt, was das Fenster zeigt: Ein Ziehen erzeugt kein Ereignis, an dem sich
// der neue Bildschirm ablesen ließe, der Compositor teilt ihn erst danach mit.

import './stil.css'
import './betrieb.css'
import { invoke } from '@tauri-apps/api/core'
import { type Aktionen, type Ansicht, baue, taste } from './betriebAnsicht'

const TAKT_MS = 400
const wurzel = document.getElementById('betrieb')!
let jetzt: Ansicht | null = null
let zuletzt = ''

const tun: Aktionen = {
  ziehen: () => { void invoke('betrieb_ziehen') },
  festlegen: () => { void invoke('betrieb_platz_festlegen').then(auffrischen) },
  bestaetigen: () => { void invoke('betrieb_bestaetigen').then(auffrischen) },
  abbrechen: () => { void invoke('betrieb_abbrechen') },
}

async function auffrischen(): Promise<void> {
  try {
    const a = await invoke<Ansicht>('betrieb_ansicht')
    const text = JSON.stringify(a)
    jetzt = a
    // Nur bei Änderung neu bauen: sonst springt der Fokus im Takt.
    if (text === zuletzt) return
    zuletzt = text
    wurzel.replaceChildren(baue(a, tun))
  } catch (e) {
    console.error(e)
  }
}

window.addEventListener('keydown', (e) => {
  if (taste(jetzt, e, tun)) e.preventDefault()
})

void auffrischen()
setInterval(() => { void auffrischen() }, TAKT_MS)
