// Einrichtungsassistent des LeadLab-Clients (Konzept §8.2).
//
// Schritte: Server → Anmeldung (nur bei zugang=anmeldung) → Daten →
// Speicherort → Download → Fertig. Der Rust-Kern
// macht alles mit Netz und Platte; diese Seite zeigt nur an und sammelt die
// Auswahl. Einstieg über das Fenstermenü: #server bzw. #daten.

import './stil.css'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import {
  type Auswahl, type ClientKonfig, type Katalog,
  anmeldungNoetig, aufgeloest, baum, gesamt, gesperrt, groesse, nachServer, paketBytes, umschalten, vorauswahl,
} from './logik'

type Schritt = 'start' | 'server' | 'anmeldung' | 'daten' | 'ort' | 'laden' | 'fertig'

interface StartInfo {
  einstellungen: { konfig_adresse?: string | null; konfig?: ClientKonfig | null; datenordner?: string | null; eingerichtet: boolean }
  standard_datenordner: string
  version: string
  plattform: string
}
interface Plan { pakete: string[]; laden: { pfad: string; bytes: number }[]; loeschen: string[]; bytes_gesamt: number; bytes_zu_laden: number; unbekannt: string[] }
interface UpdateInfo { verfuegbar: boolean; version?: string | null; notizen?: string | null; hinweis?: string | null }
interface Fortschritt { pfad: string; geladen: number; gesamt: number; stand: 'laeuft' | 'fertig' | 'fehler'; meldung?: string | null }

const z = {
  schritt: 'start' as Schritt,
  info: null as StartInfo | null,
  eingabe: '',
  konfig: null as ClientKonfig | null,
  katalog: null as Katalog | null,
  auswahl: { regionen: [], pakete: [] } as Auswahl,
  ordner: '',
  plan: null as Plan | null,
  frei: null as number | null,
  dateien: new Map<string, Fortschritt>(),
  laeuft: false,
  beschaeftigt: false,
  fehler: '' as string,
  hinweis: '' as string,
  /** Anmeldung für die Daten (CS4): null = noch nicht abgefragt. */
  anmeldung: null as { noetig: boolean; angemeldet: boolean } | null,
  /** Update des Clients (CS9): Ergebnis der letzten Prüfung. */
  update: null as UpdateInfo | null,
}

const SCHRITTE: { id: Schritt; titel: string }[] = [
  { id: 'server', titel: 'Server' },
  { id: 'anmeldung', titel: 'Anmeldung' },
  { id: 'daten', titel: 'Daten' },
  { id: 'ort', titel: 'Speicherort' },
  { id: 'laden', titel: 'Download' },
  { id: 'fertig', titel: 'Fertig' },
]

// ── kleine DOM-Hilfe ───────────────────────────────────────────────────────
type Kind = Node | string | null | undefined | false
function h(tag: string, attr: Record<string, unknown> = {}, ...kinder: Kind[]): HTMLElement {
  const el = document.createElement(tag)
  for (const [k, v] of Object.entries(attr)) {
    if (v === undefined || v === null || v === false) continue
    if (k.startsWith('on') && typeof v === 'function') el.addEventListener(k.slice(2), v as EventListener)
    else if (k === 'class') el.className = String(v)
    else if (v === true) el.setAttribute(k, '')
    else el.setAttribute(k, String(v))
  }
  for (const c of kinder) if (c) el.append(c)
  return el
}

function fehlerText(e: unknown): string {
  return typeof e === 'string' ? e : e instanceof Error ? e.message : String(e)
}

async function schritt(s: Schritt) {
  z.schritt = s
  z.fehler = ''
  zeichne()
  if (s === 'anmeldung') await anmeldungAbfragen()
  if (s === 'daten' && !z.katalog) await katalogLaden()
}

async function anmeldungAbfragen() {
  try {
    z.anmeldung = await invoke<{ noetig: boolean; angemeldet: boolean }>('anmeldung_status')
  } catch (e) {
    z.fehler = fehlerText(e)
  }
  zeichne()
}

async function anmelden() {
  z.beschaeftigt = true; z.fehler = ''; zeichne()
  try {
    z.anmeldung = await invoke<{ noetig: boolean; angemeldet: boolean }>('anmelden')
    z.katalog = null
  } catch (e) {
    z.fehler = fehlerText(e)
  } finally {
    z.beschaeftigt = false; zeichne()
  }
}

async function abmelden() {
  try {
    z.anmeldung = await invoke<{ noetig: boolean; angemeldet: boolean }>('abmelden')
    z.katalog = null
  } catch (e) {
    z.fehler = fehlerText(e)
  }
  zeichne()
}

// ── Aktionen ───────────────────────────────────────────────────────────────
async function serverPruefen() {
  z.beschaeftigt = true; z.fehler = ''; z.konfig = null; z.katalog = null; zeichne()
  try {
    z.konfig = await invoke<ClientKonfig>('konfig_pruefen', { eingabe: z.eingabe })
  } catch (e) {
    z.fehler = fehlerText(e)
  } finally {
    z.beschaeftigt = false; zeichne()
  }
}

async function katalogLaden() {
  z.beschaeftigt = true; z.fehler = ''; zeichne()
  try {
    const a = await invoke<{ katalog: Katalog; auswahl: Auswahl }>('katalog_laden', { ordner: z.ordner })
    z.katalog = a.katalog
    z.auswahl = vorauswahl(a.katalog, z.konfig, a.auswahl)
    await planNeu()
  } catch (e) {
    z.fehler = fehlerText(e)
  } finally {
    z.beschaeftigt = false; zeichne()
  }
}

async function planNeu() {
  if (!z.katalog) return
  try {
    const p = await invoke<{ plan: Plan; frei: number | null }>('plan_berechnen', { auswahl: z.auswahl, ordner: z.ordner })
    z.plan = p.plan; z.frei = p.frei
  } catch (e) {
    z.fehler = fehlerText(e)
  }
  zeichne()
}

async function ladenStarten() {
  z.fehler = ''
  try {
    await invoke('einrichtung_speichern', { ordner: z.ordner })
    z.dateien = new Map((z.plan?.laden ?? []).map((d) => [d.pfad, { pfad: d.pfad, geladen: 0, gesamt: d.bytes, stand: 'laeuft' as const }]))
    if (!z.plan || z.plan.laden.length === 0) {
      // nichts zu laden — trotzdem aufräumen lassen (verwaiste Dateien)
      await invoke('laden_starten', { auswahl: z.auswahl, ordner: z.ordner })
      z.hinweis = 'Alle gewählten Daten sind schon aktuell.'
      return schritt('fertig')
    }
    z.laeuft = true
    await invoke('laden_starten', { auswahl: z.auswahl, ordner: z.ordner })
    await schritt('laden')
  } catch (e) {
    z.laeuft = false
    z.fehler = fehlerText(e); zeichne()
  }
}

async function ohneDatenWeiter() {
  try {
    await invoke('einrichtung_speichern', { ordner: z.ordner })
    z.hinweis = 'Ohne lokale Daten eingerichtet. Karten und Gelände kommen vom Server; über „LeadLab › Daten verwalten“ lässt sich das jederzeit nachholen.'
    await schritt('fertig')
  } catch (e) {
    z.fehler = fehlerText(e); zeichne()
  }
}

async function oeffnen() {
  try {
    await invoke('oberflaeche_oeffnen')
  } catch (e) {
    z.fehler = fehlerText(e); zeichne()
  }
}

/** Update-Manifest des Servers fragen; ein Fehler hält den Start nicht auf. */
let updateLaeuft = false
async function updatePruefen() {
  if (updateLaeuft) return
  updateLaeuft = true
  try {
    z.update = await invoke<UpdateInfo>('update_pruefen')
  } catch (e) {
    z.update = { verfuegbar: false, hinweis: fehlerText(e) }
  } finally {
    updateLaeuft = false
  }
}

async function updateInstallieren() {
  z.beschaeftigt = true; z.fehler = ''; zeichne()
  try {
    await invoke('update_installieren') // startet den Client danach neu
  } catch (e) {
    z.fehler = fehlerText(e)
    z.beschaeftigt = false; zeichne()
  }
}

/** Gespeicherten Server neu prüfen, dann direkt zur Oberfläche — außer es
 *  wartet ein Update, dann fragt die Startseite. */
async function automatischStarten() {
  const e = z.info!.einstellungen
  z.beschaeftigt = true; zeichne()
  try {
    z.konfig = await invoke<ClientKonfig>('konfig_pruefen', { eingabe: e.konfig_adresse ?? '' })
    await invoke('einrichtung_speichern', { ordner: z.ordner })
    await updatePruefen()
    if (z.update?.verfuegbar) {
      z.beschaeftigt = false; zeichne()
      return
    }
    await invoke('oberflaeche_oeffnen')
  } catch (err) {
    z.fehler = fehlerText(err)
    z.beschaeftigt = false; zeichne()
  }
}

// ── Ansichten ──────────────────────────────────────────────────────────────
function leiste(): HTMLElement {
  const idx = SCHRITTE.findIndex((s) => s.id === z.schritt)
  return h('nav', { class: 'leiste', 'aria-label': 'Schritte' },
    h('div', { class: 'marke' }, h('img', { src: '/leadlab.png', alt: '' }), h('strong', {}, 'LeadLab')),
    ...SCHRITTE.map((s, i) => h('div', {
      class: `schritt ${i === idx ? 'aktiv' : ''} ${idx > i ? 'erledigt' : ''}`,
      'aria-current': i === idx ? 'step' : undefined,
    }, h('span', { class: 'nr' }, idx > i ? '✓' : String(i + 1)), s.titel)),
    h('div', { class: 'fuss nebentext' }, 'Client ', h('span', { class: 'mono' }, z.info?.version ?? '')),
  )
}

function fehlerFeld(): Kind {
  return z.fehler ? h('div', { class: 'meldung fehler', role: 'alert' }, z.fehler) : null
}

function ansichtUpdate(): HTMLElement {
  const u = z.update!
  return h('section', { class: 'inhalt' },
    h('h1', {}, `Neue Version ${u.version ?? ''} verfügbar`),
    h('p', { class: 'einleitung' },
      'Der Server bietet eine neue Version des LeadLab-Clients an. Das Update ist signiert; der Client startet danach neu.'),
    u.notizen ? h('div', { class: 'panel' }, h('p', {}, u.notizen)) : null,
    z.beschaeftigt ? h('p', {}, h('span', { class: 'dreht', 'aria-hidden': 'true' }), ' Update wird geladen …') : null,
    fehlerFeld(),
    !z.beschaeftigt && h('div', { class: 'knoepfe' },
      h('button', { class: 'sekundaer links', onclick: oeffnen }, 'Später, LeadLab öffnen'),
      h('button', { class: 'primaer', onclick: updateInstallieren }, 'Jetzt aktualisieren'),
    ),
  )
}

function ansichtStart(): HTMLElement {
  if (z.update?.verfuegbar) return ansichtUpdate()
  const adr = z.info?.einstellungen.konfig_adresse ?? ''
  return h('section', { class: 'inhalt' },
    h('h1', {}, z.beschaeftigt ? 'Verbinde mit dem Server …' : 'Server nicht erreichbar'),
    h('p', { class: 'einleitung' }, h('span', { class: 'mono' }, adr)),
    z.beschaeftigt ? h('p', {}, h('span', { class: 'dreht', 'aria-hidden': 'true' }), ' Konfiguration wird geprüft') : null,
    fehlerFeld(),
    !z.beschaeftigt && h('div', { class: 'knoepfe' },
      h('button', { class: 'sekundaer links', onclick: () => { z.eingabe = adr; void schritt('server') } }, 'Server wechseln'),
      z.info?.einstellungen.konfig && h('button', { class: 'sekundaer', onclick: oeffnen }, 'Trotzdem öffnen'),
      h('button', { class: 'primaer', onclick: automatischStarten }, 'Erneut versuchen'),
    ),
  )
}

function ansichtServer(): HTMLElement {
  const eingabe = h('input', {
    type: 'text', id: 'adresse', value: z.eingabe, spellcheck: 'false', autocomplete: 'url',
    placeholder: 'leadlab.landkreis-beispiel.de', 'aria-describedby': 'adresse-hilfe',
    oninput: (e: Event) => { z.eingabe = (e.target as HTMLInputElement).value },
    onkeydown: (e: KeyboardEvent) => { if (e.key === 'Enter') void serverPruefen() },
  }) as HTMLInputElement
  queueMicrotask(() => eingabe.focus())
  const k = z.konfig
  return h('section', { class: 'inhalt' },
    h('h1', {}, 'Mit welchem Server verbinden?'),
    h('p', { class: 'einleitung' },
      'Die Konfigurationsadresse bekommen Sie vom Betreiber Ihres LeadLab-Servers. Meist reicht der Servername; eine vollständige Adresse auf eine .json-Datei geht auch.'),
    h('div', { class: 'panel' },
      h('label', { class: 'feldname', for: 'adresse' }, 'Konfigurationsadresse'),
      h('div', { class: 'zeile' }, eingabe,
        h('button', { class: 'sekundaer', onclick: serverPruefen, disabled: z.beschaeftigt || !z.eingabe.trim() },
          z.beschaeftigt ? h('span', { class: 'dreht', 'aria-label': 'Prüfe' }) : 'Prüfen')),
      h('p', { id: 'adresse-hilfe', class: 'nebentext' }, 'Im Feuerwehrhaus-Netz geht auch ', h('span', { class: 'mono' }, 'lead.lab'), '.'),
    ),
    fehlerFeld(),
    k && h('div', { class: 'panel meldung ok' },
      h('dl', { class: 'server-karte' },
        h('dt', {}, 'Server'), h('dd', {}, k.name),
        k.marke?.landkreis ? h('dt', {}, 'Landkreis') : null, k.marke?.landkreis ? h('dd', {}, k.marke.landkreis) : null,
        h('dt', {}, 'Version'), h('dd', { class: 'mono' }, k.server_version ?? '–'),
        h('dt', {}, 'Oberfläche'), h('dd', { class: 'mono' }, k.oberflaeche),
        h('dt', {}, 'Datenzugang'), h('dd', {}, k.daten.zugang === 'offen' ? 'offen' : 'mit Anmeldung'),
      )),
    h('div', { class: 'knoepfe' },
      h('button', { class: 'primaer', disabled: !k, onclick: () => schritt(nachServer(k)) }, 'Weiter'),
    ),
  )
}

function ansichtAnmeldung(): HTMLElement {
  const a = z.anmeldung
  return h('section', { class: 'inhalt' },
    h('h1', {}, 'Anmelden für die Daten'),
    h('p', { class: 'einleitung' },
      'Dieser Server gibt seine Datenpakete nur nach einer Anmeldung heraus. Der Knopf öffnet die Anmeldeseite im Browser; danach geht es hier weiter. ',
      'Die Anmeldung gilt nur für die Daten — in LeadLab selbst melden Sie sich wie gewohnt an.'),
    h('div', { class: 'panel' },
      a?.angemeldet
        ? h('p', { class: 'meldung ok' }, 'Angemeldet. Die Anmeldung bleibt im Schlüsselbund dieses Rechners gespeichert.')
        : z.beschaeftigt
          ? h('p', {}, h('span', { class: 'dreht', 'aria-hidden': 'true' }), ' Warte auf die Anmeldung im Browser …')
          : h('p', { class: 'nebentext' }, 'Noch nicht angemeldet.'),
    ),
    fehlerFeld(),
    h('div', { class: 'knoepfe' },
      h('button', { class: 'sekundaer links', onclick: () => schritt('server') }, 'Zurück'),
      a?.angemeldet
        ? h('button', { class: 'sekundaer', onclick: abmelden }, 'Abmelden')
        : h('button', { class: 'sekundaer', disabled: z.beschaeftigt, onclick: anmelden }, 'Im Browser anmelden'),
      h('button', { class: 'primaer', disabled: !a?.angemeldet, onclick: () => schritt('daten') }, 'Weiter'),
    ),
  )
}

function ansichtDaten(): HTMLElement {
  const k = z.katalog
  if (!k) {
    return h('section', { class: 'inhalt' },
      h('h1', {}, 'Welche Daten sollen auf diesen Rechner?'),
      z.beschaeftigt ? h('p', {}, h('span', { class: 'dreht', 'aria-hidden': 'true' }), ' Datenkatalog wird geladen') : null,
      fehlerFeld(),
      !z.beschaeftigt && h('div', { class: 'knoepfe' },
        h('button', { class: 'sekundaer links', onclick: () => schritt(anmeldungNoetig(z.konfig) ? 'anmeldung' : 'server') }, 'Zurück'),
        h('button', { class: 'sekundaer', onclick: ohneDatenWeiter }, 'Ohne lokale Daten weiter'),
        h('button', { class: 'primaer', onclick: katalogLaden }, 'Erneut versuchen'),
      ))
  }
  const drin = aufgeloest(k, z.auswahl)
  const setze = (a: Auswahl) => { z.auswahl = a; void planNeu() }
  return h('section', { class: 'inhalt' },
    h('h1', {}, 'Welche Daten sollen auf diesen Rechner?'),
    h('p', { class: 'einleitung' },
      'Was hier liegt, lädt LeadLab von der Platte statt über das Netz. Grundpakete einer Region sind immer dabei. Nicht gewählte Daten kommen weiter vom Server.'),
    h('div', {},
      ...baum(k).map(({ region, gruppen }) => {
        const an = z.auswahl.regionen.includes(region.id)
        return h('div', { class: 'region' },
          h('label', {},
            h('input', {
              type: 'checkbox', checked: an,
              onchange: (e: Event) => {
                const ja = (e.target as HTMLInputElement).checked
                const pakete = ja ? z.auswahl.pakete : z.auswahl.pakete.filter((id) => k.pakete.find((p) => p.id === id)?.region !== region.id)
                setze({ regionen: umschalten(z.auswahl.regionen, region.id, ja), pakete })
              },
            }),
            region.name),
          an && h('div', {}, ...gruppen.map((g) => h('div', { class: 'gruppe' },
            h('h3', {}, g.titel),
            ...g.pakete.map((p) => {
              const sperre = gesperrt(k, z.auswahl, p.id)
              return h('label', { class: `paket ${sperre ? 'aus' : ''}` },
                h('input', {
                  type: 'checkbox', checked: drin.has(p.id), disabled: sperre,
                  onchange: (e: Event) => setze({ ...z.auswahl, pakete: umschalten(z.auswahl.pakete, p.id, (e.target as HTMLInputElement).checked) }),
                }),
                h('span', {},
                  h('span', { class: 'titel' }, p.titel),
                  p.pflicht ? h('span', { class: 'chip' }, 'Grundpaket') : null,
                  p.lizenz ? h('span', { class: 'lizenz' }, p.lizenz) : null),
                h('span', { class: 'mono nebentext' }, groesse(paketBytes(p))))
            })))))
      })),
    summe(),
    fehlerFeld(),
    h('div', { class: 'knoepfe' },
      h('button', { class: 'sekundaer links', onclick: () => schritt(anmeldungNoetig(z.konfig) ? 'anmeldung' : 'server') }, 'Zurück'),
      h('button', { class: 'primaer', onclick: () => schritt('ort') }, 'Weiter'),
    ),
  )
}

function summe(): HTMLElement {
  const p = z.plan
  const knapp = p && z.frei !== null && p.bytes_zu_laden > z.frei
  return h('div', { class: 'panel summe' },
    h('div', {}, h('span', { class: 'nebentext' }, 'Gewählt'), h('span', { class: 'wert mono' }, p ? groesse(p.bytes_gesamt) : '–')),
    h('div', {}, h('span', { class: 'nebentext' }, 'Noch zu laden'), h('span', { class: 'wert mono' }, p ? groesse(p.bytes_zu_laden) : '–')),
    h('div', {}, h('span', { class: 'nebentext' }, 'Frei am Speicherort'),
      h('span', { class: `wert mono ${knapp ? 'kritisch' : ''}` }, z.frei === null ? '–' : groesse(z.frei))),
    p && p.loeschen.length > 0 ? h('div', {}, h('span', { class: 'nebentext' }, 'Wird entfernt'), h('span', { class: 'wert mono' }, `${p.loeschen.length} Datei(en)`)) : null,
  )
}

function ansichtOrt(): HTMLElement {
  const feld = h('input', {
    type: 'text', id: 'ordner', value: z.ordner, spellcheck: 'false',
    onchange: (e: Event) => { z.ordner = (e.target as HTMLInputElement).value.trim(); void planNeu() },
  })
  const knapp = z.plan && z.frei !== null && z.plan.bytes_zu_laden > z.frei
  return h('section', { class: 'inhalt' },
    h('h1', {}, 'Wo sollen die Daten liegen?'),
    h('p', { class: 'einleitung' }, 'Große Luftbildpakete brauchen viel Platz — ein zweites Laufwerk ist dafür gut geeignet.'),
    h('div', { class: 'panel' },
      h('label', { class: 'feldname', for: 'ordner' }, 'Datenordner'),
      h('div', { class: 'zeile' }, feld,
        h('button', { class: 'sekundaer', onclick: () => { z.ordner = z.info!.standard_datenordner; void planNeu() } }, 'Standard')),
    ),
    summe(),
    knapp ? h('div', { class: 'meldung fehler', role: 'alert' }, 'Am gewählten Ort ist nicht genug Platz. Weniger Pakete wählen oder einen anderen Ordner nehmen.') : null,
    fehlerFeld(),
    h('div', { class: 'knoepfe' },
      h('button', { class: 'sekundaer links', onclick: () => schritt('daten') }, 'Zurück'),
      h('button', { class: 'primaer', disabled: !z.ordner || !!knapp, onclick: ladenStarten },
        z.plan && z.plan.bytes_zu_laden > 0 ? `${groesse(z.plan.bytes_zu_laden)} laden` : 'Übernehmen'),
    ),
  )
}

function ansichtLaden(): HTMLElement {
  const g = gesamt(z.dateien)
  const fehler = [...z.dateien.values()].filter((d) => d.stand === 'fehler')
  return h('section', { class: 'inhalt' },
    h('h1', {}, z.laeuft ? 'Daten werden geladen' : fehler.length ? 'Download unvollständig' : 'Download abgeschlossen'),
    h('p', { class: 'einleitung' }, 'Das Fenster darf geschlossen werden — beim nächsten Start geht es an derselben Stelle weiter.'),
    h('div', { class: 'panel' },
      h('div', { class: 'summe' },
        h('div', {}, h('span', { class: 'nebentext' }, 'Geladen'), h('span', { class: 'wert mono' }, `${groesse(g.geladen)} / ${groesse(g.gesamt)}`)),
        h('div', {}, h('span', { class: 'nebentext' }, 'Fortschritt'), h('span', { class: 'wert mono' }, `${Math.floor(g.anteil * 100)} %`)),
      ),
      h('div', { class: 'balken', role: 'progressbar', 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': Math.floor(g.anteil * 100) },
        h('span', { style: `width:${(g.anteil * 100).toFixed(1)}%` })),
    ),
    h('div', { class: 'panel' }, ...[...z.dateien.values()].map((d) => h('div', { class: 'datei' },
      h('span', { class: 'mono' }, d.pfad),
      h('span', { class: `mono ${d.stand === 'fehler' ? 'kritisch' : d.stand === 'fertig' ? 'erledigt' : 'nebentext'}` },
        d.stand === 'fertig' ? 'fertig' : d.stand === 'fehler' ? 'Fehler' : `${groesse(d.geladen)} / ${groesse(d.gesamt)}`),
      h('div', { class: 'balken' }, h('span', { style: `width:${d.gesamt ? (100 * d.geladen / d.gesamt).toFixed(1) : 0}%` })),
      d.meldung ? h('span', { class: 'kritisch', style: 'grid-column:1/-1' }, d.meldung) : null,
    ))),
    fehlerFeld(),
    h('div', { class: 'knoepfe' },
      z.laeuft
        ? h('button', { class: 'sekundaer', onclick: () => invoke('laden_abbrechen') }, 'Anhalten')
        : h('button', { class: 'sekundaer links', onclick: () => schritt('daten') }, 'Auswahl ändern'),
      !z.laeuft && fehler.length > 0 ? h('button', { class: 'sekundaer', onclick: ladenStarten }, 'Fortsetzen') : null,
      h('button', { class: 'primaer', disabled: z.laeuft, onclick: () => schritt('fertig') }, 'Weiter'),
    ),
  )
}

function ansichtFertig(): HTMLElement {
  if (z.update === null) void updatePruefen().then(zeichne)
  return h('section', { class: 'inhalt' },
    h('h1', {}, 'LeadLab ist eingerichtet'),
    z.update?.verfuegbar
      ? h('div', { class: 'panel meldung ok' }, `Neue Client-Version ${z.update.version ?? ''} verfügbar. `,
          h('button', { class: 'sekundaer', onclick: updateInstallieren, disabled: z.beschaeftigt }, 'Jetzt aktualisieren'))
      : null,
    h('p', { class: 'einleitung' }, z.hinweis || 'Die gewählten Daten liegen auf diesem Rechner. Updates holt der Client über „LeadLab › Daten verwalten“.'),
    fehlerFeld(),
    h('div', { class: 'knoepfe' },
      h('button', { class: 'sekundaer links', onclick: () => schritt('daten') }, 'Daten verwalten'),
      h('button', { class: 'primaer', onclick: oeffnen }, 'LeadLab öffnen'),
    ),
  )
}

function zeichne() {
  const app = document.getElementById('app')!
  const ansicht = {
    start: ansichtStart, server: ansichtServer, anmeldung: ansichtAnmeldung, daten: ansichtDaten,
    ort: ansichtOrt, laden: ansichtLaden, fertig: ansichtFertig,
  }[z.schritt]()
  app.replaceChildren(leiste(), ansicht)
}

// ── Start ──────────────────────────────────────────────────────────────────
async function start() {
  await listen<Fortschritt>('laden-fortschritt', (e) => {
    z.dateien.set(e.payload.pfad, e.payload)
    if (z.schritt === 'laden') zeichne()
  })
  await listen<{ ok: boolean; meldung?: string | null }>('laden-ende', (e) => {
    z.laeuft = false
    z.fehler = e.payload.ok || e.payload.meldung === 'Abgebrochen.' ? '' : (e.payload.meldung ?? '')
    if (z.schritt === 'laden') zeichne()
  })

  z.info = await invoke<StartInfo>('start_info')
  const e = z.info.einstellungen
  z.eingabe = e.konfig_adresse ?? ''
  z.konfig = e.konfig ?? null
  z.ordner = e.datenordner ?? z.info.standard_datenordner

  const ziel = location.hash.replace('#', '')
  if (ziel === 'server') return schritt('server')
  if (ziel === 'daten' && e.konfig) return schritt('daten')
  if (e.eingerichtet && e.konfig_adresse) { z.schritt = 'start'; return automatischStarten() }
  return schritt('server')
}

start().catch((e) => { z.fehler = fehlerText(e); z.schritt = 'server'; zeichne() })
