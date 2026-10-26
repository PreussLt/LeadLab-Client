# LeadLab-Client

Der Arbeitsplatz-Client für **Vanguard LeadLab** unter Windows und Linux. Er
verbindet sich mit einem LeadLab-Server und zeigt dessen Oberfläche als eigenes
Programm. Gelände-, Karten-, Luftbild- und Gebäudedaten hält er auf Wunsch lokal
vor, damit sie nicht bei jeder Übung über das Netz kommen.

Konzept und Stufenplan stehen im Hauptrepo:
`docs/konzepte/Konzept-Client-und-Datenauslieferung.md` (Eltern-Issue #766,
dieses Grundgerüst ist CS1 #756). Dort ist dieses Repo als Submodul
`apps/client` eingebunden.

## Ablauf

1. **Server:** Der Assistent fragt nach der *Konfigurationsadresse*, z. B.
   `leadlab.landkreis-beispiel.de`. Er lädt
   `https://<adresse>/.well-known/leadlab-client.json` (oder direkt eine
   angegebene `.json`) und zeigt Name, Landkreis, Version und Datenzugang.
   `http://` gilt nur für `localhost`, `lead.lab` und private Adressen.
2. **Anmeldung** (nur wenn der Server `zugang: anmeldung` meldet): Der Knopf
   öffnet den Systembrowser auf die Keycloak-Anmeldung des Servers
   (öffentlicher Client `leadlab-client`, Authorization Code + PKCE S256), der
   Client wartet auf `http://127.0.0.1:<zufallsport>/rueckruf` (RFC 8252). Das
   Refresh-Token liegt im Schlüsselbund des Betriebssystems (Windows
   Credential Manager, Secret Service unter Linux), das Access-Token nur im
   Speicher; vor jeder Datei holt der Downloader ein frisches. Das Token
   öffnet nur `/daten/` des Servers, nicht die Übung.
3. **Daten:** Der Baum zeigt Region › Gruppe (Gelände, Karte, Luftbild,
   3D-Gebäude, Stammdaten) › Paket, jeweils mit Größe und Lizenz. Grundpakete
   einer gewählten Region sind gesetzt, Abhängigkeiten werden mitgenommen.
4. **Speicherort:** Vorgabe ist `%LOCALAPPDATA%\de.leadlab.client\daten`
   bzw. `~/.local/share/de.leadlab.client/daten`. Angezeigt wird der freie
   Platz am Speicherort.
5. **Download:** höchstens drei Ströme, Wiederaufnahme per HTTP-Range,
   SHA-256-Prüfung jeder Datei, Spiegel-Rückfall. Abbruch und Neustart
   verlieren nichts.
6. **Fertig:** Das Fenster wechselt zur Oberfläche des Servers.

Nach der Einrichtung öffnet der Client den Server direkt. Das Menü
**LeadLab** bietet *Daten verwalten*, *Server wechseln*, *Neu laden* (F5) und
*Vollbild* (F11).

## Formate

Die Schemata `leadlab-client/1` (Konfigurationsadresse) und `leadlab-daten/1`
(Katalog) sind im Konzept §5 und §6.2 beschrieben. Beispiele, die auch die
Tests verwenden, liegen in [`schema/`](schema/).

Die Blobs liegen auf dem Server unter `<basis>sha256/<ab>/<hash>`. Im
Datenordner heißen sie wie ihr Katalog-`pfad`. Den Bestand hält
`zustand.json`, laufende Downloads liegen als `*.teil` daneben.

## Entwickeln

Benötigt Node 20+, Rust stable und unter Linux die WebKitGTK-Pakete (siehe
[`werkzeug/Dockerfile.linux`](werkzeug/Dockerfile.linux)).

```bash
npm install
npm test                 # Vitest: Assistentenlogik
npm run tauri dev        # Client mit Hot-Reload
(cd src-tauri && cargo test)   # Rust: Konfig, Katalog/Plan, Ablage, Downloader
npm run tauri build      # Pakete: NSIS/MSI (Windows), AppImage/deb (Linux)
```

Ohne lokale Rust-Installation läuft alles über den Bau-Container:

```bash
docker build -t leadlab-client-bau -f werkzeug/Dockerfile.linux werkzeug
docker run --rm -v "$PWD:/c" -v leadlab-client-cargo:/usr/local/cargo/registry \
  -v leadlab-client-target:/c/src-tauri/target -w /c/src-tauri leadlab-client-bau cargo test
```

## Aufbau

| Datei | Aufgabe |
| --- | --- |
| `src/main.ts` | Assistent (Schritte, Anzeige, Fortschritt) |
| `src/logik.ts` | Reine Logik: Baum, Auswahl, Sperren, Größen, Vorauswahl |
| `src-tauri/src/konfig.rs` | Konfigurationsadresse auflösen und prüfen |
| `src-tauri/src/katalog.rs` | Katalog, Auswahl auflösen, Plan gegen die Ablage |
| `src-tauri/src/ablage.rs` | Datenordner, sichere Pfade, Zustand, freier Speicher |
| `src-tauri/src/laden.rs` | Downloader mit Range, SHA-256 und Spiegeln |
| `src-tauri/src/anmeldung.rs` | OIDC PKCE mit Loopback-Rückruf, Token im Schlüsselbund |
| `src-tauri/src/lib.rs` | Tauri-Befehle, Menü, Navigation zur Server-Oberfläche |

**Sicherheit:** IPC-Rechte hat nur die lokale Assistentenseite
(`capabilities/default.json` ohne `remote`). Die Server-Oberfläche im selben
Fenster kann keine Befehle aufrufen. Ein Katalog kann nicht außerhalb des
Datenordners schreiben (`ablage::sicherer_pfad`).

## Offen (Folgepakete im Hauptrepo)

- **CS7 #762** Loopback-Server, der lokale Pakete an die Oberfläche liefert.
- **CS9 #764** Release-CI und Auto-Update.
