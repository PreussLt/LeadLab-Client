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
