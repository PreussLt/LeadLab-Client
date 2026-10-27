// Reine Logik des Assistenten — ohne DOM und ohne Tauri, damit sie im Test
// läuft. Die Typen spiegeln die Rust-Strukturen (katalog.rs, konfig.rs).

export interface Datei { pfad: string; bytes: number; sha256: string }
export interface Paket {
  id: string; gruppe: string; titel: string; region: string; version: string
  pflicht: boolean; lizenz?: string | null; dateien: Datei[]; braucht: string[]
}
export interface Region { id: string; name: string; bbox?: [number, number, number, number] | null }
export interface Katalog { schema: string; erzeugt?: string | null; basis: string[]; regionen: Region[]; pakete: Paket[] }
export interface Auswahl { regionen: string[]; pakete: string[] }
export interface ClientKonfig {
  schema: string; name: string; server_version?: string | null; oberflaeche: string
  daten: { katalog: string; zugang: 'anmeldung' | 'offen' }
  marke?: { landkreis?: string | null; logo?: string | null }
  startzentrale?: string | null
  anmeldung?: { issuer: string; client_id: string; scopes?: string[] } | null
  client_update?: string | null
}

/** Braucht dieser Server eine Anmeldung, bevor der Katalog lädt (Konzept §8.2 Schritt 2)? */
export function anmeldungNoetig(k: ClientKonfig | null): boolean {
  return k?.daten.zugang === 'anmeldung'
}

/** Schritt nach „Server“: Anmeldung, wenn der Server sie verlangt, sonst gleich die Daten. */
export function nachServer(k: ClientKonfig | null): 'anmeldung' | 'daten' {
  return anmeldungNoetig(k) ? 'anmeldung' : 'daten'
}

/** Reihenfolge und Namen der Gruppen im Datenbaum. */
export const GRUPPEN: { id: string; titel: string }[] = [
  { id: 'gelaende', titel: 'Gelände' },
  { id: 'karte', titel: 'Karte' },
  { id: 'luftbild', titel: 'Luftbild' },
  { id: 'gebaeude', titel: '3D-Gebäude' },
  { id: 'stammdaten', titel: 'Stammdaten' },
]

const ZAHL = new Intl.NumberFormat('de-DE', { maximumFractionDigits: 1, minimumFractionDigits: 0 })

/** 1 536 000 000 → „1,5 GB“ (Dezimal-Einheiten wie Datei-Manager unter Windows/Linux-Desktops). */
export function groesse(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '–'
  const stufen = ['B', 'kB', 'MB', 'GB', 'TB']
  let w = bytes
  let i = 0
  while (w >= 1000 && i < stufen.length - 1) { w /= 1000; i++ }
  return `${i === 0 ? Math.round(w) : ZAHL.format(w)} ${stufen[i]}`
}

export function paketBytes(p: Paket): number {
  return p.dateien.reduce((s, d) => s + d.bytes, 0)
}

export interface GruppenKnoten { id: string; titel: string; pakete: Paket[] }
export interface RegionKnoten { region: Region; gruppen: GruppenKnoten[] }

/** Katalog → Baum Region › Gruppe › Paket; unbekannte Gruppen hinten. */
export function baum(k: Katalog): RegionKnoten[] {
  const regionen = [...k.regionen]
  for (const p of k.pakete) {
    if (!regionen.some((r) => r.id === p.region)) regionen.push({ id: p.region, name: p.region })
  }
  return regionen.map((region) => {
    const eigene = k.pakete.filter((p) => p.region === region.id)
    const bekannte = GRUPPEN.map((g) => ({ ...g, pakete: eigene.filter((p) => p.gruppe === g.id) }))
    const fremde = [...new Set(eigene.map((p) => p.gruppe))]
      .filter((g) => !GRUPPEN.some((x) => x.id === g))
      .map((g) => ({ id: g, titel: g, pakete: eigene.filter((p) => p.gruppe === g) }))
    return { region, gruppen: [...bekannte, ...fremde].filter((g) => g.pakete.length > 0) }
  })
}

/** Ids, die durch Auswahl zwingend dabei sind: Pflicht der gewählten Regionen + Abhängigkeiten. */
export function aufgeloest(k: Katalog, a: Auswahl): Set<string> {
  const nachId = new Map(k.pakete.map((p) => [p.id, p]))
  const offen = [
    ...k.pakete.filter((p) => p.pflicht && a.regionen.includes(p.region)).map((p) => p.id),
    ...a.pakete,
  ]
  const fertig = new Set<string>()
  while (offen.length) {
    const id = offen.pop()!
    const p = nachId.get(id)
    if (!p || fertig.has(id)) continue
    fertig.add(id)
    offen.push(...p.braucht)
  }
  return fertig
}

/** Ist ein Paket gesperrt (nicht abwählbar), weil Pflicht oder von einem gewählten gebraucht? */
export function gesperrt(k: Katalog, a: Auswahl, id: string): boolean {
  const p = k.pakete.find((x) => x.id === id)
  if (!p) return false
  if (p.pflicht && a.regionen.includes(p.region)) return true
  const ohne = { ...a, pakete: a.pakete.filter((x) => x !== id) }
  return aufgeloest(k, ohne).has(id)
}

/**
 * Vorauswahl beim ersten Mal: die Region des Server-Landkreises (Name
 * enthält die Marke), sonst die erste Region. Eine gespeicherte Auswahl
 * gewinnt, soweit der Katalog sie noch kennt.
 */
export function vorauswahl(k: Katalog, konfig: ClientKonfig | null, gespeichert: Auswahl | null): Auswahl {
  const ids = new Set(k.pakete.map((p) => p.id))
  const regionIds = new Set(k.regionen.map((r) => r.id))
  if (gespeichert && (gespeichert.regionen.length || gespeichert.pakete.length)) {
    return {
      regionen: gespeichert.regionen.filter((r) => regionIds.has(r)),
      pakete: gespeichert.pakete.filter((p) => ids.has(p)),
    }
  }
  const kreis = konfig?.marke?.landkreis?.toLowerCase()
  const treffer = kreis ? k.regionen.find((r) => r.name.toLowerCase().includes(kreis)) : undefined
  const r = treffer ?? k.regionen[0]
  return { regionen: r ? [r.id] : [], pakete: [] }
}

export function umschalten<T>(liste: T[], wert: T, an: boolean): T[] {
  const ohne = liste.filter((x) => x !== wert)
  return an ? [...ohne, wert] : ohne
}

/** Gesamtfortschritt aus den Einzelmeldungen. */
export function gesamt(stand: Map<string, { geladen: number; gesamt: number }>): { geladen: number; gesamt: number; anteil: number } {
  let g = 0
  let s = 0
  for (const v of stand.values()) { g += v.geladen; s += v.gesamt }
  return { geladen: g, gesamt: s, anteil: s > 0 ? Math.min(1, g / s) : 0 }
}

// ── Konfigurationsmodus (#792) ─────────────────────────────────────────────
// Spiegelt uebersicht::PaketZeile und die Übersicht aus lib.rs.

export interface PaketZeile {
  id: string | null; region: string | null; gruppe: string; titel: string; version: string | null
  bytes: number; dateien: number; vollstaendig: boolean; zoom: [number, number] | null
  geprueft: number | null; sperre: string | null
}

/** Titel einer Gruppe, unbekannte bleiben, wie sie heißen. */
export function gruppenTitel(id: string): string {
  return GRUPPEN.find((g) => g.id === id)?.titel ?? id
}

/** Pakettabelle: nach Region (ohne Region zuletzt), dann Gruppenreihenfolge des Datenbaums, dann Titel. */
export function paketTabelle(zeilen: PaketZeile[]): PaketZeile[] {
  const rang = (g: string) => { const i = GRUPPEN.findIndex((x) => x.id === g); return i < 0 ? GRUPPEN.length : i }
  return [...zeilen].sort((a, b) =>
    (a.region === null ? 1 : 0) - (b.region === null ? 1 : 0)
    || (a.region ?? '').localeCompare(b.region ?? '', 'de')
    || rang(a.gruppe) - rang(b.gruppe)
    || a.titel.localeCompare(b.titel, 'de'))
}

export function summeBytes(zeilen: { bytes: number }[]): number {
  return zeilen.reduce((s, z) => s + z.bytes, 0)
}

/** [5, 14] → „z5–14“, [14, 14] → „z14“. */
export function zoomText(z: [number, number] | null | undefined): string {
  if (!z) return '–'
  return z[0] === z[1] ? `z${z[0]}` : `z${z[0]}–${z[1]}`
}

const ZEIT = new Intl.DateTimeFormat('de-DE', { day: '2-digit', month: '2-digit', year: 'numeric', hour: '2-digit', minute: '2-digit' })

/** Unix-Sekunden → „27.10.2026, 14:05“ (Ortszeit). */
export function zeitpunkt(unix: number | null | undefined): string {
  if (unix === null || unix === undefined || !Number.isFinite(unix) || unix <= 0) return '–'
  return ZEIT.format(new Date(unix * 1000))
}

/** Plan gegen die gespeicherte Auswahl → Hinweis auf Aktualisierungen. */
export function aktualisierungen(p: { laden: { bytes: number }[]; loeschen: string[]; bytes_zu_laden: number }): { offen: boolean; text: string } {
  const n = p.laden.length
  const weg = p.loeschen.length
  if (n === 0 && weg === 0) return { offen: false, text: 'Alle lokalen Daten sind auf dem Stand des Katalogs.' }
  const teile: string[] = []
  if (n > 0) teile.push(`${n} ${n === 1 ? 'Datei' : 'Dateien'} zu laden (${groesse(p.bytes_zu_laden)})`)
  if (weg > 0) teile.push(`${weg} ${weg === 1 ? 'Datei' : 'Dateien'} zu entfernen`)
  return { offen: true, text: `Aktualisierungen verfügbar: ${teile.join(', ')}.` }
}

/** Startverhalten aus Einstellung und Kommandozeile (#791) in Worten. */
export function startverhalten(einstellung: boolean, args: { vollbild?: boolean | null; monitor?: number | null }): { modus: string; monitor: string } {
  const an = args.vollbild ?? einstellung
  const quelle = args.vollbild === true ? ' (diesmal --vollbild)' : args.vollbild === false ? ' (diesmal --fenster)' : ''
  const modus = `${an ? 'Vollbild' : 'Fenster'}${quelle}`
  const monitor = args.monitor !== null && args.monitor !== undefined
    ? `Monitor ${args.monitor + 1} (--monitor ${args.monitor})`
    : 'Monitor mit dem Mauszeiger'
  return { modus, monitor }
}
