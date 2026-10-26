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

/** latest.json → update.json mit Dateien unter `basis`. */
export function fuerServer(latest, basis) {
  if (!latest || typeof latest.version !== 'string' || typeof latest.platforms !== 'object') {
    throw new Error('kein latest.json der tauri-action (version, platforms fehlen)')
  }
  const b = basis.endsWith('/') ? basis : `${basis}/`
  if (!/^https:\/\//.test(b) && !/^http:\/\/(localhost|127\.0\.0\.1|([a-z0-9-]+\.)*lead\.lab)(:\d+)?\//.test(b)) {
    throw new Error(`Basis ${basis} muss https sein (http nur für lead.lab/localhost)`)
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
