#!/usr/bin/env node
// Update-Manifest für den eigenen Server (CS9, LeadLab-Hauptrepo #764).
//
// Die Release-CI (tauri-action) legt `latest.json` mit Download-Adressen auf
// GitHub ins Release. Arbeitsplätze laden aber vom LeadLab-Server
// (Konfigurationsadresse → client_update, nginx /daten/client/ ohne
// Anmeldung). Dieses Skript schreibt die Adressen auf eine Basis um; die
// Signaturen bleiben unverändert (sie gelten der Datei, nicht der Adresse).
//
//   node werkzeug/update-manifest.mjs latest.json https://leadlab.example.de/daten/client/ > update.json
//
// Danach update.json und die genannten Dateien (Installer + .sig) nach
// <volume daten-auslieferung>/client/ legen.

import { readFileSync } from 'node:fs'
import { pathToFileURL } from 'node:url'

/**
 * Namen, die im Internet nicht vergeben werden — Spiegel von `LAN_ENDUNGEN`
 * in src-tauri/src/konfig.rs (#793). Dort steht die Begründung je Eintrag.
 */
export const LAN_ENDUNGEN = ['lead.lab', 'localhost', 'localdomain', 'lan', 'home', 'corp', 'local', 'home.arpa', 'internal']

/** Darf `adresse` ohne TLS angesprochen werden? Gleiche Regel wie `http_erlaubt` im Client. */
export function httpErlaubt(adresse) {
  let host
  try {
    host = new URL(adresse).hostname.toLowerCase().replace(/\.+$/, '')
  } catch {
    return false
  }
  const v4 = host.match(/^(\d+)\.(\d+)\.(\d+)\.(\d+)$/)
  if (v4) {
    const [a, b] = v4.slice(1).map(Number)
    // Loopback, privat (RFC 1918), link-local
    return a === 127 || a === 10 || (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168) || (a === 169 && b === 254)
  }
  if (host === '[::1]') return true
  return LAN_ENDUNGEN.some((e) => host === e || host.endsWith(`.${e}`))
}

/** latest.json → update.json mit Dateien unter `basis`. */
export function fuerServer(latest, basis) {
  if (!latest || typeof latest.version !== 'string' || typeof latest.platforms !== 'object') {
    throw new Error('kein latest.json der tauri-action (version, platforms fehlen)')
  }
  const b = basis.endsWith('/') ? basis : `${basis}/`
  if (!/^https:\/\//.test(b) && !(/^http:\/\//.test(b) && httpErlaubt(b))) {
    throw new Error(`Basis ${basis} muss https sein (http nur im eigenen Netz: private Adressen, lead.lab, .lan, .local, .internal, .home.arpa)`)
  }
  const platforms = {}
  for (const [ziel, p] of Object.entries(latest.platforms)) {
    if (!p?.url || !p?.signature) throw new Error(`Plattform ${ziel} ohne url/signature`)
    const datei = decodeURIComponent(new URL(p.url).pathname.split('/').pop())
    platforms[ziel] = { ...p, url: b + encodeURIComponent(datei) }
  }
  return { ...latest, platforms }
}

/** Die Dateien, die neben update.json auf den Server gehören. */
export function benoetigteDateien(manifest) {
  return [...new Set(Object.values(manifest.platforms).map((p) => decodeURIComponent(new URL(p.url).pathname.split('/').pop())))]
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  const [datei, basis] = process.argv.slice(2)
  if (!datei || !basis) {
    console.error('Aufruf: node werkzeug/update-manifest.mjs <latest.json> <basis-url>')
    process.exit(2)
  }
  const m = fuerServer(JSON.parse(readFileSync(datei, 'utf8')), basis)
  process.stdout.write(JSON.stringify(m, null, 2) + '\n')
  console.error(`Dateien nach …/client/: ${benoetigteDateien(m).join(', ')}`)
}
