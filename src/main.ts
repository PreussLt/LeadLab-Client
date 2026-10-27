// Einrichtungsassistent des LeadLab-Clients (Konzept §8.2).
//
// Schritte: Server → Anmeldung (nur bei zugang=anmeldung) → Daten →
// Speicherort → Download → Fertig. Der Rust-Kern
// macht alles mit Netz und Platte; diese Seite zeigt nur an und sammelt die
// Auswahl. Einstieg über das Fenstermenü: #server bzw. #daten.
//
// Daneben der Konfigurationsmodus (#792, Fragment #einstellungen, Menü
// „LeadLab › Einstellungen …“, Strg+,): Zustand von Server, Anmeldung,
// lokalen Daten, Loopback-Dienst und Programm auf einer Seite.

import './stil.css'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import {
  type Auswahl, type ClientKonfig, type Katalog,
  type PaketZeile, type WarteStand,
  FORMULAR_FALSCH, aktualisierungen, anmeldeFehler, anmeldungAngeboten, browserStill, formularFehler, formularZeigen, aufgeloest, baum, gesamt, gesperrt, groesse, gruppenTitel, nachServer, paketBytes,
  paketTabelle, restdauer, startverhalten, summeBytes, umschalten, vorauswahl, zeitpunkt, zoomText,
} from './logik'

type Schritt = 'start' | 'server' | 'anmeldung' | 'daten' | 'ort' | 'laden' | 'fertig' | 'einstellungen'

interface StartInfo {
  einstellungen: { konfig_adresse?: string | null; konfig?: ClientKonfig | null; datenordner?: string | null; eingerichtet: boolean }
  standard_datenordner: string
  version: string
  plattform: string
}
interface Plan { pakete: string[]; laden: { pfad: string; bytes: number }[]; loeschen: string[]; bytes_gesamt: number; bytes_zu_laden: number; unbekannt: string[] }
interface UpdateInfo { verfuegbar: boolean; version?: string | null; notizen?: string | null; hinweis?: string | null }
/** Spiegelt `AnmeldeAnsicht` in lib.rs (AD2/AD4). */
interface Anmeldestand {
  noetig: boolean; angemeldet: boolean; konto?: string | null
  offline?: boolean; angemeldet_seit?: number | null; refresh_bis?: number | null
  tresor?: string | null; oberflaeche_bis?: number | null
  /** Laufende Browser-Anmeldung (#805). */
  warten?: WarteStand | null
  /** Fehler einer Anmeldung im Client-Fenster, einmal abgeholt. */
  meldung?: string | null
  /** Der Server erlaubt die Anmeldemaske (#804). */
  formular?: boolean
}
/** Spiegelt `Uebersicht` in lib.rs. */
interface Uebersicht {
  einstellungen: StartInfo['einstellungen'] & { konfig_geprueft?: number | null; vollbild?: boolean }
  anmeldung: Anmeldestand | null
  datenordner: string
  frei: number | null
  belegt: number
  pakete: PaketZeile[]
  auswahl: Auswahl
  katalog_erzeugt: string | null
  lokal: { adresse: string; status: string; pakete: number; geliefert: number; zuletzt_geladen: string | null; zuletzt_geladen_um: number | null }
  programm: {
    version: string; plattform: string; update: UpdateInfo | null; vollbild_einstellung: boolean
    argumente: { vollbild: boolean | null; monitor: number | null }
    einstellungen_datei: string | null; zustand_datei: string
  }
  laeuft: boolean
}
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
  /** Anmeldung des Clients (CS4, AD4): null = noch nicht abgefragt. */
  anmeldung: null as Anmeldestand | null,
  /** Update des Clients (CS9): Ergebnis der letzten Prüfung. */
  update: null as UpdateInfo | null,
  /** Konfigurationsmodus (#792). */
  uebersicht: null as Uebersicht | null,
  /** Paket-Id, deren Entfernen gerade bestätigt werden soll. */
  entfernen: null as string | null,
  /** Ergebnis von „Sitzung in der Oberfläche erneuern“ (AD4). */
  sitzungHinweis: '' as string,
  /** Ergebnis von „Auf Aktualisierungen prüfen“. */
  aktualisierung: null as { offen: boolean; text: string } | null,
  /** Laufende Browser-Anmeldung (#805): Adresse, Verbindung, Ausweichwege. */
  warten: null as WarteStand | null,
  /** „Link kopieren“ hat geklappt. */
  kopiert: false,
  /** Anmeldemaske (#804). Das Passwort steht NICHT hier, sondern nur im
   *  Eingabefeld `passwortFeld`. */
  formular: { benutzer: '', fehlversuche: 0, fehler: '' },
}

/** Nach erfolgreicher Anmeldung so lange den Kontonamen zeigen, dann weiter. */
const WEITER_MS = 1200

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
  if (s === 'einstellungen') await einstellungenLaden()
}

async function anmeldungAbfragen() {
  try {
    z.anmeldung = await invoke<Anmeldestand>('anmeldung_status')
    warteUebernehmen(z.anmeldung)
  } catch (e) {
    z.fehler = fehlerText(e)
  }
  zeichne()
}

// ── Browser-Anmeldung (#805) ───────────────────────────────────────────────
// `anmelden` wartet im Rust-Kern bis zum Rückruf. Die Seite fragt solange
// jede Sekunde den Stand ab: Anmeldeadresse zum Kopieren, ob sich ein Browser
// am Loopback gemeldet hat, und nach 20 s ohne Verbindung die Ausweichwege.

/** `invoke('anmelden')` läuft aus dieser Seite (nach der Anmeldung im
 *  Client-Fenster ist die Seite neu geladen und kennt den Aufruf nicht). */
let anmeldungOffen = false
let warteUhr: ReturnType<typeof setInterval> | null = null
let warteSchluessel = ''

function warteBeobachten() {
  if (warteUhr) return
  warteUhr = setInterval(() => { void warteStandHolen() }, 1000)
  setTimeout(() => { void warteStandHolen() }, 300)
}

function warteBeenden() {
  if (warteUhr) clearInterval(warteUhr)
  warteUhr = null; z.warten = null; z.kopiert = false; warteSchluessel = ''
}

/** Stand aus dem Kern übernehmen: wartet eine Anmeldung, die Abfrage starten;
 *  eine Meldung aus dem Client-Fenster als Fehler zeigen. */
function warteUebernehmen(a: Anmeldestand | null | undefined) {
  if (a?.meldung) z.fehler = anmeldeFehler(a.meldung)
  if (a?.warten) { z.warten = a.warten; warteBeobachten() }
}

async function warteStandHolen() {
  let a: Anmeldestand
  try {
    a = await invoke<Anmeldestand>('anmeldung_status')
  } catch {
    return // nächster Takt versucht es wieder
  }
  if (a.warten) {
    const schluessel = JSON.stringify(a.warten) + browserStill(a.warten)
    z.warten = a.warten
    if (schluessel !== warteSchluessel) { warteSchluessel = schluessel; zeichne() }
    return
  }
  if (anmeldungOffen) return // das Ergebnis bringt der laufende Aufruf
  // Die Anmeldung endete außerhalb dieser Seite (Client-Fenster, Frist).
  warteBeenden()
  z.beschaeftigt = false
  if (a.meldung) z.fehler = anmeldeFehler(a.meldung)
  if (z.schritt === 'einstellungen') await einstellungenLaden()
  else { z.anmeldung = a; zeichne() }
}

async function anmelden() {
  z.beschaeftigt = true; z.fehler = ''; anmeldungOffen = true
  // Vorläufig, bis der Kern die Adresse nennt.
  z.warten = { seit: Date.now() / 1000, verbunden: false }
  warteBeobachten(); zeichne()
  try {
    const a = await invoke<Anmeldestand>('anmelden')
    if (z.schritt === 'anmeldung') z.anmeldung = a
    z.katalog = null
  } catch (e) {
    z.fehler = anmeldeFehler(fehlerText(e))
  } finally {
    anmeldungOffen = false; z.beschaeftigt = false
    warteBeenden()
  }
  if (z.schritt === 'einstellungen') await einstellungenLaden()
  else zeichne()
}

// ── Anmeldemaske (#804) ────────────────────────────────────────────────────
// Benutzername/Passwort gehen an den Rust-Kern, der sie per Passwort-Grant an
// Keycloak gibt. Das Passwort lebt nur im Eingabefeld: Es ist über alle
// Neuzeichnungen dasselbe Element (zeichne() baut sonst alles neu), wird nach
// Erfolg geleert und weder gespeichert noch protokolliert.

let passwortFeld: HTMLInputElement | null = null
function passwortEingabe(): HTMLInputElement {
  passwortFeld ??= h('input', { type: 'password', id: 'passwort', autocomplete: 'current-password', spellcheck: 'false' }) as HTMLInputElement
  return passwortFeld
}

function weiterNachAnmeldung() {
  return z.info?.einstellungen.eingerichtet ? oeffnen() : schritt('daten')
}

async function formularAnmelden() {
  const pw = passwortEingabe()
  const benutzer = z.formular.benutzer.trim()
  if (!benutzer || !pw.value) {
    z.formular.fehler = 'Bitte Benutzername und Passwort eingeben.'; zeichne()
    ;(benutzer ? pw : document.getElementById('benutzer'))?.focus()
    return
  }
  z.beschaeftigt = true; z.formular.fehler = ''; z.fehler = ''; zeichne()
  try {
    const a = await invoke<Anmeldestand>('anmelden_formular', { benutzer, passwort: pw.value })
    pw.value = ''
    z.formular.fehlversuche = 0
    z.katalog = null
    z.beschaeftigt = false
    if (z.schritt === 'einstellungen') { await einstellungenLaden(); return }
    z.anmeldung = a
    zeichne()
    // Kontoname kurz zeigen, dann weiter (Daten bzw. Oberfläche).
    setTimeout(() => { if (z.schritt === 'anmeldung' && z.anmeldung?.angemeldet) void weiterNachAnmeldung() }, WEITER_MS)
  } catch (e) {
    const t = fehlerText(e)
    if (t === FORMULAR_FALSCH) z.formular.fehlversuche++
    z.formular.fehler = formularFehler(t, z.formular.fehlversuche)
    z.beschaeftigt = false; zeichne()
    pw.focus(); pw.select()
  }
}

async function anmeldungAbbrechen() {
  try {
    await invoke('anmeldung_abbrechen')
  } catch (e) {
    z.fehler = fehlerText(e); zeichne()
  }
  await warteStandHolen()
}

async function browserErneut() {
  try {
    await invoke('anmeldung_browser_oeffnen')
  } catch (e) {
    z.fehler = fehlerText(e); zeichne()
  }
}

async function imFensterAnmelden() {
  try {
    // Das Fenster geht auf die Anmeldeseite; danach kommt der Kern hierher zurück.
    await invoke('anmeldung_im_fenster', { zurueck: z.schritt === 'einstellungen' ? 'einstellungen' : 'anmeldung' })
  } catch (e) {
    z.fehler = fehlerText(e); zeichne()
  }
}

async function linkKopieren() {
  const adr = z.warten?.adresse
  if (!adr) return
  try {
    await navigator.clipboard.writeText(adr)
    z.kopiert = true; zeichne()
  } catch {
    // Ohne Zwischenablage: Adresse markieren, Strg+C bleibt.
    z.kopiert = false; zeichne()
    const f = document.getElementById('anmeldeadresse') as HTMLInputElement | null
    f?.focus(); f?.select()
  }
}

async function abmelden() {
  try {
    z.anmeldung = await invoke<Anmeldestand>('abmelden')
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

// ── Konfigurationsmodus (#792) ─────────────────────────────────────────────
async function einstellungenLaden() {
  try {
    z.uebersicht = await invoke<Uebersicht>('zustand_uebersicht')
    warteUebernehmen(z.uebersicht.anmeldung)
    z.ordner = z.uebersicht.datenordner
    if (z.uebersicht.programm.update) z.update = z.uebersicht.programm.update
  } catch (e) {
    z.fehler = fehlerText(e)
  }
  zeichne()
  // Nachladen, was Netz braucht — die Seite steht schon.
  const u = z.uebersicht
  if (u?.anmeldung?.angemeldet && !u.anmeldung.konto) {
    try {
      u.anmeldung = await invoke<Anmeldestand>('anmeldung_pruefen')
      zeichne()
    } catch { /* Kontoname bleibt offen; der Status steht schon da */ }
  }
  if (u && z.update === null) { await updatePruefen(); zeichne() }
}

/** Fehler zeigen, ohne die Seite zu verlassen. */
async function versuche(f: () => Promise<unknown>) {
  z.beschaeftigt = true; z.fehler = ''; zeichne()
  try {
    await f()
  } catch (e) {
    z.fehler = fehlerText(e)
  } finally {
    z.beschaeftigt = false
  }
  await einstellungenLaden()
}

const konfigNeuPruefen = () => versuche(async () => {
  z.konfig = await invoke<ClientKonfig>('konfig_neu_pruefen')
  z.katalog = null
})
const einstellungAbmelden = () => versuche(async () => { await invoke('abmelden'); z.katalog = null })
const sitzungErneuern = () => versuche(async () => {
  z.sitzungHinweis = ''
  await invoke('oberflaeche_sitzung_erneuern')
  z.sitzungHinweis = 'Die Sitzung in der Oberfläche ist erneuert.'
})
const ordnerOeffnen = (welcher: 'daten' | 'einstellungen') => versuche(() => invoke('ordner_oeffnen', { welcher }))
const paketEntfernen = (id: string) => versuche(async () => {
  z.entfernen = null
  await invoke('paket_entfernen', { id })
  z.katalog = null; z.aktualisierung = null
})
const nachUpdateSuchen = () => versuche(async () => { z.update = null; await updatePruefen() })

async function aktualisierungPruefen() {
  await katalogLaden()
  z.aktualisierung = z.katalog && z.plan ? aktualisierungen(z.plan) : null
  zeichne()
}

async function speicherortAendern() {
  await schritt('ort')
  if (!z.katalog) await katalogLaden()
}

// ── Ansichten ──────────────────────────────────────────────────────────────
const ABSCHNITTE: { id: string; titel: string }[] = [
  { id: 'e-server', titel: 'Server' },
  { id: 'e-anmeldung', titel: 'Anmeldung' },
  { id: 'e-daten', titel: 'Daten' },
  { id: 'e-dienst', titel: 'Lokaler Dienst' },
  { id: 'e-programm', titel: 'Programm' },
]

function leiste(): HTMLElement {
  if (z.schritt === 'einstellungen') {
    return h('nav', { class: 'leiste', 'aria-label': 'Abschnitte' },
      h('div', { class: 'marke' }, h('img', { src: '/leadlab.png', alt: '' }), h('strong', {}, 'LeadLab')),
      ...ABSCHNITTE.map((a) => h('button', {
        type: 'button', class: 'schritt sprung',
        onclick: () => document.getElementById(a.id)?.scrollIntoView({ block: 'start' }),
      }, a.titel)),
      h('div', { class: 'fuss nebentext' }, 'Client ', h('span', { class: 'mono' }, z.info?.version ?? '')),
    )
  }
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

/** Während der Browser-Anmeldung (#805): nie stumm warten — Adresse zum
 *  Kopieren, Browser erneut öffnen, Abbrechen; ohne Browser nach 20 s die
 *  Anmeldung im Client-Fenster. */
function wartePanel(): HTMLElement {
  const w = z.warten!
  const adresse = w.adresse ?? ''
  const still = browserStill(w)
  return h('div', { class: 'panel warten' },
    h('p', { role: 'status' }, h('span', { class: 'dreht', 'aria-hidden': 'true' }),
      w.im_fenster
        ? ' Die Anmeldung läuft im Client-Fenster …'
        : adresse
          ? ' Die Anmeldung wird im Browserfenster fortgesetzt. Melden Sie sich dort an; danach geht es hier von selbst weiter.'
          : ' Anmeldeseite wird vorbereitet …'),
    adresse && h('div', {},
      h('label', { class: 'feldname', for: 'anmeldeadresse' }, 'Anmeldeadresse'),
      h('div', { class: 'zeile' },
        h('input', { type: 'text', id: 'anmeldeadresse', class: 'mono', readonly: true, value: adresse, spellcheck: 'false' }),
        h('button', { class: 'sekundaer', onclick: linkKopieren }, z.kopiert ? 'Kopiert' : 'Link kopieren')),
      h('p', { class: 'nebentext' }, 'Die Adresse gilt nur für diese Anmeldung und in jedem Browser auf diesem Rechner.')),
    still && h('div', { class: 'meldung hinweis', role: 'alert' },
      h('p', {}, h('strong', {}, 'Kein Browser hat sich gemeldet. '),
        w.browser_fehler ? `${w.browser_fehler} ` : '',
        'Kopieren Sie den Link in einen Browser, oder melden Sie sich hier im Client-Fenster an.'),
      h('div', { class: 'knoepfe' },
        h('button', { class: 'sekundaer', disabled: !adresse, onclick: imFensterAnmelden }, 'Im Client-Fenster anmelden'))),
    h('div', { class: 'knoepfe' },
      h('button', { class: 'sekundaer links', onclick: anmeldungAbbrechen }, 'Abbrechen'),
      h('button', { class: 'sekundaer', disabled: !adresse || !!w.im_fenster, onclick: browserErneut }, 'Browser erneut öffnen')),
  )
}

/** Benutzername, Passwort, „Anmelden“; Enter schickt ab (Formular-Submit). */
function anmeldeFormular(): HTMLElement {
  const benutzer = h('input', {
    type: 'text', id: 'benutzer', value: z.formular.benutzer, autocomplete: 'username', spellcheck: 'false',
    autocapitalize: 'off', disabled: z.beschaeftigt,
    oninput: (e: Event) => { z.formular.benutzer = (e.target as HTMLInputElement).value },
  }) as HTMLInputElement
  const pw = passwortEingabe()
  pw.disabled = z.beschaeftigt
  // Fokus ins erste leere Feld, aber keinem anderen Feld wegnehmen.
  queueMicrotask(() => {
    const f = document.activeElement
    if (!z.beschaeftigt && (!f || f === document.body)) (z.formular.benutzer ? pw : benutzer).focus()
  })
  return h('form', {
    class: 'panel formular', 'aria-label': 'Mit Benutzername und Passwort anmelden', novalidate: true,
    onsubmit: (e: Event) => { e.preventDefault(); void formularAnmelden() },
  },
    h('div', {}, h('label', { class: 'feldname', for: 'benutzer' }, 'Benutzername'), benutzer),
    h('div', {}, h('label', { class: 'feldname', for: 'passwort' }, 'Passwort'), pw),
    z.formular.fehler ? h('div', { class: 'meldung fehler', role: 'alert' }, z.formular.fehler) : null,
    h('div', { class: 'knoepfe' },
      h('button', { type: 'submit', class: 'primaer', disabled: z.beschaeftigt },
        z.beschaeftigt ? h('span', {}, h('span', { class: 'dreht', 'aria-hidden': 'true' }), ' Anmelden …') : 'Anmelden')),
    h('p', { class: 'nebentext' },
      'Das Passwort geht nur an den Anmeldedienst Ihres Servers und wird auf diesem Rechner nicht gespeichert. Konten mit zweitem Faktor melden sich über „Im Browser anmelden“ an.'),
  )
}

function ansichtAnmeldung(): HTMLElement {
  const a = z.anmeldung
  const eingerichtet = !!z.info?.einstellungen.eingerichtet
  return h('section', { class: 'inhalt' },
    h('h1', {}, 'Anmelden'),
    h('p', { class: 'einleitung' },
      'Melden Sie sich einmal im Client an. Die Anmeldung gilt für die Datenpakete und für LeadLab im Fenster — ein zweites Login gibt es nicht. ',
      'Sie hält wochenlang, bis jemand hier abmeldet oder die Verwaltung den Arbeitsplatz in der Anmeldung sperrt.',
      formularZeigen(a) ? '' : ' Der Knopf öffnet die Anmeldeseite im Browser; danach geht es hier weiter.'),
    !a?.angemeldet && z.warten
      ? wartePanel()
      : formularZeigen(a)
        ? anmeldeFormular()
        : h('div', { class: 'panel' },
          a?.angemeldet
            ? h('p', { class: 'meldung ok', role: 'status' },
              a.konto ? `Angemeldet als ${a.konto}. ` : 'Angemeldet. ',
              'Die Anmeldung liegt verschlüsselt auf diesem Rechner.',
              a.offline === false ? ' Dieses Konto darf keine Daueranmeldung; sie endet mit der Anmeldesitzung des Servers.' : null)
            : h('p', { class: 'nebentext' }, 'Noch nicht angemeldet.'),
        ),
    fehlerFeld(),
    h('div', { class: 'knoepfe' },
      h('button', { class: 'sekundaer links', disabled: !!z.warten, onclick: () => schritt('server') }, 'Zurück'),
      a?.angemeldet
        ? h('button', { class: 'sekundaer', onclick: abmelden }, 'Abmelden')
        : !z.warten && h('button', { class: 'sekundaer', disabled: z.beschaeftigt, onclick: anmelden }, 'Im Browser anmelden'),
      // Mit der Maske ist „Anmelden“ die Hauptaktion; nach dem Login geht es von selbst weiter.
      formularZeigen(a) && !z.warten ? null
        : eingerichtet
          ? h('button', { class: 'primaer', disabled: !a?.angemeldet, onclick: oeffnen }, 'LeadLab öffnen')
          : h('button', { class: 'primaer', disabled: !a?.angemeldet, onclick: () => schritt('daten') }, 'Weiter'),
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
        h('button', { class: 'sekundaer links', onclick: () => schritt(anmeldungAngeboten(z.konfig) ? 'anmeldung' : 'server') }, 'Zurück'),
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
      h('button', { class: 'sekundaer links', onclick: () => schritt(anmeldungAngeboten(z.konfig) ? 'anmeldung' : 'server') }, 'Zurück'),
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

function eintrag(name: string, wert: Kind, mono = false): HTMLElement[] {
  return [h('dt', {}, name), h('dd', { class: mono ? 'mono' : undefined }, wert || '–')]
}

function ansichtEinstellungen(): HTMLElement {
  const u = z.uebersicht
  if (!u) {
    return h('section', { class: 'inhalt' }, h('h1', {}, 'Einstellungen'),
      fehlerFeld() ?? h('p', {}, h('span', { class: 'dreht', 'aria-hidden': 'true' }), ' Zustand wird gelesen'))
  }
  const e = u.einstellungen
  const k = e.konfig ?? null
  const a = u.anmeldung
  const tabelle = paketTabelle(u.pakete)
  const sv = startverhalten(u.programm.vollbild_einstellung, u.programm.argumente)
  const upd = z.update
  return h('section', { class: 'inhalt' },
    h('h1', {}, 'Einstellungen'),
    h('p', { class: 'einleitung' }, 'Der Zustand dieses Arbeitsplatzes. Die Konfiguration selbst kommt vom Server und lässt sich hier nicht bearbeiten.'),
    fehlerFeld(),

    // 1. Server
    h('div', { class: 'panel abschnitt', id: 'e-server' },
      h('h2', {}, 'Server'),
      h('dl', { class: 'server-karte' },
        ...eintrag('Konfigurationsadresse', e.konfig_adresse, true),
        ...eintrag('Server', k?.name),
        ...eintrag('Landkreis', k?.marke?.landkreis),
        ...eintrag('Version', k?.server_version, true),
        ...eintrag('Oberfläche', k?.oberflaeche, true),
        ...eintrag('Startzentrale', k?.startzentrale, true),
        ...eintrag('Datenzugang', k ? (k.daten.zugang === 'offen' ? 'offen' : 'mit Anmeldung') : null),
        ...eintrag('Zuletzt geprüft', zeitpunkt(e.konfig_geprueft), true),
      ),
      k && h('details', {}, h('summary', {}, `Konfiguration (${k.schema})`),
        h('pre', { class: 'mono roh' }, JSON.stringify(k, null, 2))),
      h('div', { class: 'knoepfe' },
        h('button', { class: 'sekundaer', onclick: () => { z.eingabe = e.konfig_adresse ?? ''; void schritt('server') } }, 'Server wechseln'),
        h('button', { class: 'sekundaer', disabled: z.beschaeftigt || !e.konfig_adresse, onclick: konfigNeuPruefen }, 'Neu prüfen'),
      ),
    ),

    // 2. Anmeldung
    h('div', { class: 'panel abschnitt', id: 'e-anmeldung' },
      h('h2', {}, 'Anmeldung'),
      !a || (!a.noetig && !k?.anmeldung)
        ? h('p', { class: 'nebentext' }, 'Dieser Server gibt seine Daten ohne Anmeldung heraus.')
        : h('div', {},
          h('p', { class: 'nebentext' }, 'Eine Anmeldung für Daten und Oberfläche. Sie liegt verschlüsselt im Tresor dieses Rechners.'),
          h('dl', { class: 'server-karte' },
            ...eintrag('Angemeldet', a.angemeldet ? 'ja' : 'nein'),
            ...eintrag('Konto', a.angemeldet ? (a.konto ?? 'wird beim nächsten Datenabruf bekannt') : null),
            ...eintrag('Angemeldet seit', a.angemeldet ? zeitpunkt(a.angemeldet_seit) : null, true),
            ...eintrag('Daueranmeldung', a.angemeldet ? (a.offline ? 'ja (Offline-Token)' : 'nein') : null),
            ...eintrag('Läuft ab', a.angemeldet && a.refresh_bis
              ? `${zeitpunkt(a.refresh_bis)} · ${restdauer(a.refresh_bis)}${a.offline ? ', rückt mit jeder Nutzung vor' : ''}` : null, true),
            ...eintrag('Sitzung in der Oberfläche', a.oberflaeche_bis ? `bis ${zeitpunkt(a.oberflaeche_bis)}, verlängert sich selbst` : (a.angemeldet ? 'beim Öffnen von LeadLab' : null), !!a.oberflaeche_bis),
            ...eintrag('Für die Daten nötig', a.noetig ? 'ja' : 'nein'),
            ...eintrag('Tresor', a.tresor, true),
          ),
          z.sitzungHinweis ? h('p', { class: 'meldung ok', role: 'status' }, z.sitzungHinweis) : null,
          !a.angemeldet && z.warten ? wartePanel() : formularZeigen(a) ? anmeldeFormular() : null,
          h('div', { class: 'knoepfe' },
            a.angemeldet
              ? h('button', { class: 'sekundaer', disabled: z.beschaeftigt, onclick: einstellungAbmelden }, 'Abmelden')
              : !z.warten && h('button', { class: 'sekundaer', disabled: z.beschaeftigt, onclick: anmelden }, 'Im Browser anmelden'),
            a.angemeldet && h('button', { class: 'sekundaer', disabled: z.beschaeftigt || !k, onclick: sitzungErneuern }, 'Sitzung in der Oberfläche erneuern'),
          )),
    ),

    // 3. Daten
    h('div', { class: 'panel abschnitt', id: 'e-daten' },
      h('h2', {}, 'Daten'),
      h('dl', { class: 'server-karte' }, ...eintrag('Speicherort', u.datenordner, true)),
      h('div', { class: 'summe' },
        h('div', {}, h('span', { class: 'nebentext' }, 'Belegt'), h('span', { class: 'wert mono' }, groesse(u.belegt))),
        h('div', {}, h('span', { class: 'nebentext' }, 'Frei am Speicherort'), h('span', { class: 'wert mono' }, u.frei === null ? '–' : groesse(u.frei))),
        h('div', {}, h('span', { class: 'nebentext' }, 'Katalogstand'), h('span', { class: 'wert mono' }, u.katalog_erzeugt ?? '–')),
      ),
      tabelle.length === 0
        ? h('p', { class: 'nebentext' }, 'Keine lokalen Daten. Karten und Gelände kommen vom Server.')
        : h('div', { class: 'tabelle-rahmen' }, h('table', { class: 'tabelle' },
          h('thead', {}, h('tr', {}, ...['Region', 'Gruppe', 'Paket', 'Version', 'Größe', 'Zoom', 'Geprüft', ''].map((t) => h('th', { scope: 'col' }, t)))),
          h('tbody', {}, ...tabelle.map((p) => h('tr', {},
            h('td', {}, p.region ?? '–'),
            h('td', {}, gruppenTitel(p.gruppe)),
            h('td', {}, p.titel, !p.vollstaendig ? h('span', { class: 'chip kritisch' }, 'unvollständig') : null),
            h('td', { class: 'mono' }, p.version ?? '–'),
            h('td', { class: 'mono zahl' }, groesse(p.bytes)),
            h('td', { class: 'mono' }, zoomText(p.zoom)),
            h('td', { class: 'mono' }, zeitpunkt(p.geprueft)),
            h('td', { class: 'aktion' }, entfernenKnopf(p, u.laeuft)),
          ))),
          h('tfoot', {}, h('tr', {}, h('td', { colspan: 4 }, `${tabelle.length} ${tabelle.length === 1 ? 'Eintrag' : 'Einträge'}`),
            h('td', { class: 'mono zahl' }, groesse(summeBytes(tabelle))), h('td', { colspan: 3 }))),
        )),
      z.aktualisierung && h('div', { class: `meldung ${z.aktualisierung.offen ? 'hinweis' : 'ok'}`, role: 'status' }, z.aktualisierung.text),
      h('div', { class: 'knoepfe' },
        h('button', { class: 'sekundaer links', disabled: z.beschaeftigt || !k, onclick: aktualisierungPruefen }, 'Auf Aktualisierungen prüfen'),
        h('button', { class: 'sekundaer', onclick: () => ordnerOeffnen('daten') }, 'Ordner öffnen'),
        h('button', { class: 'sekundaer', disabled: !k || u.laeuft, onclick: speicherortAendern }, 'Speicherort ändern'),
        h('button', { class: 'sekundaer', disabled: !k, onclick: () => schritt(u.laeuft ? 'laden' : 'daten') }, u.laeuft ? 'Download ansehen' : 'Daten verwalten'),
      ),
    ),

    // 4. Lokaler Dienst
    h('div', { class: 'panel abschnitt', id: 'e-dienst' },
      h('h2', {}, 'Lokaler Dienst'),
      h('p', { class: 'nebentext' }, 'Der Client liefert die lokalen Pakete über eine Adresse auf diesem Rechner aus. Die Oberfläche bekommt sie beim Laden über window.__LEADLAB_CLIENT__.'),
      h('dl', { class: 'server-karte' },
        ...eintrag('Adresse', u.lokal.adresse, true),
        ...eintrag('Status', u.lokal.status, true),
        ...eintrag('Pakete im Dienst', String(u.lokal.pakete), true),
        ...eintrag('Ausgeliefert seit Start', `${u.lokal.geliefert} Antworten`, true),
        ...eintrag('Oberfläche zuletzt geladen', u.lokal.zuletzt_geladen
          ? `${u.lokal.zuletzt_geladen} · ${zeitpunkt(u.lokal.zuletzt_geladen_um)}`
          : 'noch nicht in diesem Lauf', !!u.lokal.zuletzt_geladen),
      ),
    ),

    // 5. Programm
    h('div', { class: 'panel abschnitt', id: 'e-programm' },
      h('h2', {}, 'Programm'),
      h('dl', { class: 'server-karte' },
        ...eintrag('Client-Version', u.programm.version, true),
        ...eintrag('Plattform', u.programm.plattform, true),
        ...eintrag('Update', upd === null ? 'wird geprüft …'
          : upd.verfuegbar ? `Version ${upd.version ?? ''} verfügbar` : (upd.hinweis ?? 'Dieser Client ist aktuell.')),
        ...eintrag('Oberfläche öffnen im', sv.modus),
        ...eintrag('Monitor', sv.monitor),
        ...eintrag('Einstellungsdatei', u.programm.einstellungen_datei, true),
        ...eintrag('Zustandsdatei', u.programm.zustand_datei, true),
      ),
      h('p', { class: 'nebentext' }, 'Das Startverhalten stellt „LeadLab › Beim Start im Vollbild“ im Menü um; --vollbild, --fenster und --monitor N gelten für einen Start.'),
      h('div', { class: 'knoepfe' },
        h('button', { class: 'sekundaer links', onclick: () => ordnerOeffnen('einstellungen') }, 'Einstellungsordner öffnen'),
        upd?.verfuegbar
          ? h('button', { class: 'sekundaer', disabled: z.beschaeftigt, onclick: updateInstallieren }, 'Jetzt aktualisieren')
          : h('button', { class: 'sekundaer', disabled: z.beschaeftigt || !k, onclick: nachUpdateSuchen }, 'Nach Update suchen'),
      ),
    ),

    // 6. Zurück
    h('div', { class: 'knoepfe' },
      h('button', { class: 'primaer', disabled: !k, onclick: oeffnen }, 'LeadLab öffnen'),
    ),
  )
}

/** „Entfernen“ mit Rückfrage in der Zeile (kein Systemdialog). */
function entfernenKnopf(p: PaketZeile, laeuft: boolean): Kind {
  if (!p.id) return h('span', { class: 'nebentext klein' }, p.sperre ?? '')
  if (p.sperre) return h('span', { class: 'nebentext klein' }, p.sperre)
  if (z.entfernen === p.id) {
    const id = p.id
    return h('span', { class: 'rueckfrage' },
      h('button', { class: 'sekundaer klein', onclick: () => { z.entfernen = null; zeichne() } }, 'Behalten'),
      h('button', { class: 'gefahr klein', disabled: z.beschaeftigt, onclick: () => paketEntfernen(id) }, `${groesse(p.bytes)} löschen`))
  }
  return h('button', {
    class: 'sekundaer klein', disabled: laeuft || z.beschaeftigt,
    onclick: () => { z.entfernen = p.id; zeichne() },
  }, 'Entfernen')
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
    ort: ansichtOrt, laden: ansichtLaden, fertig: ansichtFertig, einstellungen: ansichtEinstellungen,
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

  // Das Menü navigiert auf dieselbe Seite mit neuem Fragment; steht der
  // Assistent schon da, lädt sie nicht neu.
  window.addEventListener('hashchange', () => {
    const f = location.hash.replace('#', '')
    if (f === 'server' || f === 'einstellungen' || ((f === 'daten' || f === 'anmeldung') && z.konfig)) void schritt(f as Schritt)
  })

  const ziel = location.hash.replace('#', '')
  if (ziel === 'server') return schritt('server')
  if (ziel === 'einstellungen' && e.konfig) return schritt('einstellungen')
  if (ziel === 'daten' && e.konfig) return schritt('daten')
  // Aus der Oberfläche zurück, weil die Anmeldung fehlt (AD4) oder über „Abmelden“ im Menü.
  if (ziel === 'anmeldung' && e.konfig) return schritt('anmeldung')
  if (e.eingerichtet && e.konfig_adresse) { z.schritt = 'start'; return automatischStarten() }
  return schritt('server')
}

start().catch((e) => { z.fehler = fehlerText(e); z.schritt = 'server'; zeichne() })
