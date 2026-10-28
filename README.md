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
   `http://` gilt nur im eigenen Netz: für private Adressen und für Namen,
   die es im Internet nicht gibt — `localhost`, `lead.lab`, `.lan`,
   `.localdomain`, `.home`, `.corp`, `.local`, `.home.arpa`, `.internal`
   (`LAN_ENDUNGEN` in `src-tauri/src/konfig.rs`, Hauptrepo #793). Ein
   Feuerwehrhaus mit eigenen Hosts ohne TLS (`LEADLAB_DOMAIN=ffw.lan`) gibt
   also `start.ffw.lan` ein. Eine öffentliche Domain ohne TLS nimmt der
   Client nicht an, auch wenn sie per `/etc/hosts` ins LAN zeigt: Er kann
   das nicht vom Internet unterscheiden, und Passwort und Token liefen
   unverschlüsselt. Dann die IP-Adresse oder einen der Namen oben nehmen.
2. **Anmeldung** (wenn der Server einen Anmeldedienst nennt): Ein Login gilt
   für die Datenpakete **und** für die Oberfläche im Fenster. Die
   **Anmeldemaske** fragt Benutzername und Passwort ab und tauscht sie direkt
   am Token-Endpunkt von Keycloak (`grant_type=password`, Client
   `leadlab-client`, `scope=openid offline_access`; Hauptrepo #804). Das
   Passwort wird nie gespeichert oder protokolliert und im Kern nach dem
   Aufruf überschrieben (`zeroize`). Falsches Passwort, Sperre nach fünf
   Fehlversuchen, Konten mit OTP und ein Server ohne Formular bekommen
   eigene Meldungen; die letzten verweisen auf den Browser. Die Maske lässt sich serverseitig abschalten
   (`anmeldung.formular: false` in der Konfigurationsadresse). Der zweite
   Knopf *Im Browser anmelden* öffnet den Systembrowser auf die Keycloak-Anmeldung des Servers
   (öffentlicher Client `leadlab-client`, Authorization Code + PKCE S256), der
   Client wartet auf `http://127.0.0.1:<zufallsport>/rueckruf` (RFC 8252).
   Währenddessen zeigt der Schritt die Anmeldeadresse mit *Link kopieren*,
   *Browser erneut öffnen* und *Abbrechen*. Meldet sich nach 20 s kein
   Browser am Loopback, folgt ein Hinweis mit *Im Client-Fenster anmelden*:
   Das Fenster öffnet die Keycloak-Seite selbst, der Rückruf kommt dort
   genauso an (#805). Nach 5 Minuten ohne Antwort bricht der Client ab und
   sagt das.
   Angefragt wird zusätzlich `offline_access`: Die Anmeldung hält dann
   wochenlang (Offline-Token, 30 Tage ohne Nutzung), bis jemand abmeldet oder
   ein Admin die Offline-Sitzung in Keycloak widerruft. Access-, Refresh- und
   ID-Token liegen mit ihren Ablaufzeiten verschlüsselt im **Tresor**
   `tresor.bin` im Konfigurationsordner (`~/.config/de.leadlab.client`, unter
   Windows `%APPDATA%\de.leadlab.client`), nicht im Datenordner:
   ChaCha20-Poly1305 mit einem Geräteschlüssel in `tresor.schluessel`
   (Windows: per DPAPI an das Nutzerkonto gebunden; Linux: Modus 0600). Eine
   kaputte oder fremde Datei gilt als „nicht angemeldet“. Vor Ablauf erneuert
   der Client das Access-Token per Refresh-Token und schreibt das Ergebnis
   (auch ein rotiertes Refresh-Token) zurück; lehnt Keycloak ab, wird der
   Tresor geleert. *Abmelden* widerruft das Refresh-Token beim Anmeldedienst
   (RFC 7009) und löscht den Tresor. Ein Refresh-Token aus dem Schlüsselbund
   älterer Fassungen wird beim ersten Start übernommen und dort gelöscht.
   Vor jeder Datei holt der Downloader ein frisches Access-Token.
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

Nach der Einrichtung öffnet der Client den Server direkt.

## Lokale Pakete in der Oberfläche

Der Client startet einen **Loopback-Server** auf `127.0.0.1:<zufallsport>`,
erreichbar nur unter einem zufälligen Pfad-Token (`/t/<token>/…`, je Start
neu). Er liest die geladenen Pakete per Dateizugriff:
`/kacheln/osm/{z}/{x}/{y}.png`, `/kacheln/dop/{z}/{x}/{y}.jpg`,
`/gelaende/{z}/{x}/{y}.png` (Terrarium) aus den PMTiles, und
`/lod2/gebaeude?lat_min=…` aus den LoD2-Manifesten (Antwort wie
`GET /api/lod2/gebaeude` des Servers). Was fehlt, ist 404 — dann fragt die
Oberfläche den Server. `/status` nennt die vorhandenen Pakete.

In jede Seite außer der Assistentenseite spritzt der Client
`window.__LEADLAB_CLIENT__ = { version, lokal, pakete }` ein (eingefroren).
Die Frontend-Weiche der Oberfläche liest daraus die lokale Adresse
(Hauptrepo CS8 #763). IPC-Rechte bekommt die entfernte Seite weiterhin nicht. Das Menü
**LeadLab** bietet *Daten verwalten*, *Server wechseln*, *Abmelden*, *Neu
laden* (F5), *Vollbild ein/aus* (F11), *Beim Start im Vollbild* und
*Einstellungen …* (Strg+,).

## Ein Login für Daten und Oberfläche

Die Oberfläche im Fenster übernimmt die Anmeldung des Clients
(`oberflaeche.rs`, Hauptrepo AD4 #800), es erscheint kein zweites Login:

1. Vor dem Öffnen (auch beim automatischen Start) holt der Kern ein frisches
   Access-Token aus dem Tresor und ruft `POST <oberfläche>/api/auth/client-sitzung`
   auf. Der Server tauscht es bei Keycloak und antwortet mit dem Cookie
   `leadlab_sitzung`.
2. Das Cookie setzt der Kern per `set_cookie` ins Fenster (mit Domain aus der
   Antwort, sonst je Host für Oberfläche und Startzentrale) und prüft mit
   `cookies_for_url`, dass es angekommen ist. Erst dann navigiert er. Kommt
   es nicht an, geht es über den Einmalcode:
   `GET /api/auth/client-login?code=…&ziel=…` setzt das Cookie in einer
   gewöhnlichen Antwort.
3. Die Sitzung der Oberfläche läuft fünf Minuten; der Kern verlängert sie
   90 s vor Ablauf mit dem Cookie (dieselbe Sitzung).
4. Navigiert die Oberfläche auf `/api/auth/login` (Sitzung abgelaufen, etwa
   nach dem Ruhezustand), fängt `on_navigation` das ab, erneuert die Sitzung
   und geht zum ursprünglichen Ziel. Ist der Client nicht mehr angemeldet
   (Offline-Token widerrufen), kommt der Assistentenschritt *Anmeldung* statt
   der Keycloak-Seite. Mehr als drei Umleitungen in 30 s führen ebenfalls
   dorthin, statt im Kreis.
5. *Abmelden* (Menü, Einstellungen, Assistent) beendet zuerst die Sitzung beim
   Server (`POST /api/auth/abmelden` mit dem Cookie), löscht dann die Cookies
   im Fenster und widerruft zuletzt das Refresh-Token beim Anmeldedienst
   (Tresor gelöscht). Wer in der Oberfläche selbst abmeldet, landet beim
   nächsten Login-Versuch im Assistenten, nicht still wieder angemeldet.

Cookies lesen und setzen läuft in async-Befehlen bzw. Tasks; WebView2 darf
dabei nicht im Hauptthread warten.

## Einstellungen

*LeadLab › Einstellungen …* zeigt den Zustand des Arbeitsplatzes auf einer
lokalen Seite (Hauptrepo #792, Konzept §8.6):

- **Server:** Konfigurationsadresse, Name, Landkreis, Oberfläche,
  Startzentrale, Datenzugang, letzte Prüfung, die rohe Konfiguration;
  *Neu prüfen*, *Server wechseln*.
- **Anmeldung:** Konto, angemeldet seit, Daueranmeldung (Offline-Token) ja
  oder nein, Ablauf, Sitzung der Oberfläche, Tresor-Pfad; Anmeldemaske,
  *Im Browser anmelden*, *Abmelden*, *Sitzung in der Oberfläche erneuern*.
- **Daten:** Speicherort mit belegtem und freiem Platz, Tabelle der lokalen
  Pakete (Region, Gruppe, Paket, Version, Größe, Zoom, geprüft);
  *Auf Aktualisierungen prüfen*, *Ordner öffnen*, *Speicherort ändern*,
  *Daten verwalten*, *Entfernen* (mit Rückfrage in der Zeile).
- **Lokaler Dienst:** Adresse des Loopback-Servers, ausgelieferte Antworten,
  letztes Laden der Oberfläche.
- **Programm:** Version, Plattform, Update-Stand, Startverhalten, Pfade der
  Einstellungs- und Zustandsdatei.

Die Pakettabelle braucht kein Netz: Nach jedem Download stehen die Pakete mit
ihren Katalogangaben in `zustand.json` (Feld `pakete`).

## Vollbild

Für den Arbeitsplatz im Stabsraum öffnet der Client die Oberfläche im
Vollbild, ohne Fensterrahmen und ohne Menüleiste, wie die Desktop-App
(Hauptrepo #791, Konzept §8.5). Der Assistent läuft weiter im Fenster; erst
„LeadLab öffnen“ bzw. der automatische Start wechselt ins Vollbild.

```bash
leadlab-client --vollbild      # diesmal im Vollbild (Einstellung bleibt)
leadlab-client --fenster       # diesmal im Fenster
leadlab-client --monitor 1     # Vollbild auf dem zweiten Monitor
```

Ohne `--vollbild`/`--fenster` gilt *Beim Start im Vollbild* aus dem Menü
(`einstellungen.json`, Feld `vollbild`). Ohne `--monitor` nimmt der Client den
Monitor, auf dem der Mauszeiger steht. Hinaus geht es mit F11, Esc (nur wenn
kein Dialog offen ist und kein Eingabefeld den Fokus hat) oder dem
Schwebeknopf **✕ Vollbild verlassen** unten rechts; dann kommt die
Menüleiste zurück. Knopf und Tasten navigieren auf `leadlab://vollbild-aus`,
das der Client abfängt. Die Server-Oberfläche bekommt dafür keine IPC-Rechte.

## Nebenfenster und externe Links

Die Oberfläche gliedert Ansichten mit `window.open(…, '_blank')` aus
(Reiter ausgliedern, Einspiel-Bildschirm, Stabssicht, Beamer,
Hydrantenkarte). Der Client öffnet dafür ein **Nebenfenster** derselben
Sitzung wie die Desktop-App (Hauptrepo #859, Konzept Desktop-App §4.4): auf
dem zweiten Bildschirm, wenn es einen gibt (unter Wayland setzt der
Compositor das Fenster selbst), ohne Menüleiste, mit **✕ Schließen** unten
rechts, Strg+W und F11 für Vollbild. `window.close()` der Seite schließt es;
mit dem Hauptfenster gehen alle Nebenfenster zu. Intern ist, was auf dem Host
der Oberfläche oder der Startzentrale liegt, dazu `about:blank`
(Druckfenster). Externe Ziele (`http`, `https`, `mailto`) öffnet der
Systembrowser, alles andere wird verworfen. Knopf und Tasten laufen über
`leadlab://fenster-schliessen` und `leadlab://vollbild-umschalten`; IPC hat
auch das Nebenfenster nicht.

## Linux: keine Tastatureingabe

Setzt die Desktop-Sitzung `GTK_IM_MODULE=ibus` (oder `fcitx`) und
`XMODIFIERS=@im=ibus`, ohne dass der Daemon der Eingabemethode läuft, nimmt
WebKitGTK in Eingabefeldern keine Tastendrücke an: Der Klick setzt den Fokus,
getippt wird nichts (Hauptrepo #811). Der Client prüft das beim Start
(`src-tauri/src/eingabemethode.rs`): Läuft unter der eigenen Nutzerkennung
kein `ibus-daemon` bzw. `fcitx5`/`fcitx`, setzt er
`GTK_IM_MODULE=gtk-im-context-simple` und entfernt `XMODIFIERS`; auf stderr
steht dann eine Zeile mit `#811`. Läuft der Daemon, bleibt alles unverändert
(der Debug-Build schreibt das ebenfalls mit). Das gilt für deb, AppImage und
den Start aus dem Terminal gleichermaßen.

Greift das nicht (etwa bei einer anderen Eingabemethode), hilft der Start mit
der einfachen Eingabe von GTK:

```bash
env -u XMODIFIERS GTK_IM_MODULE=gtk-im-context-simple leadlab-client
```

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
npm test                 # Vitest: Assistentenlogik, Einstellungsseite
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

## Release und Updates

Die Release-CI ([`.github/workflows/release.yml`](.github/workflows/release.yml))
baut bei einem Tag `v<version>` (gleich `version` in `package.json`) auf
Windows NSIS und MSI, auf Linux AppImage und deb, signiert die
Update-Artefakte und legt alles mit `latest.json` als **Entwurf** ins
GitHub-Release.

Einmalig einrichten:

1. Schlüssel erzeugen: `npm run tauri signer generate -- -w ~/.tauri/leadlab-client.key`.
   Die private Datei bleibt außerhalb des Repos.
2. Inhalt von `leadlab-client.key.pub` in `src-tauri/tauri.conf.json` unter
   `plugins.updater.pubkey` statt des Platzhalters eintragen und committen.
   Solange dort der Platzhalter steht, prüft der Client keine Updates, und die
   Release-CI bricht ab.
3. Im Repo die Secrets `TAURI_SIGNING_PRIVATE_KEY` (Inhalt der privaten Datei)
   und `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` anlegen.

Ausliefern: Die Arbeitsplätze laden das Update vom eigenen Server
(`client_update` der Konfigurationsadresse, nginx `/daten/client/` ohne
Anmeldung). Dafür `latest.json` umschreiben und mit den Dateien in das Volume
`daten-auslieferung` legen:

```bash
node werkzeug/update-manifest.mjs latest.json https://<domain>/daten/client/ > update.json
# update.json + die genannten Installer nach <daten-auslieferung>/client/
```

Lokal geht `npx tauri build --bundles deb,appimage` ohne Schlüssel. Die
Update-Artefakte schaltet erst `--config src-tauri/tauri.release.conf.json`
ein.

## Aufbau

| Datei | Aufgabe |
| --- | --- |
| `src/main.ts` | Assistent (Schritte, Anzeige, Fortschritt) und Einstellungsseite |
| `src/logik.ts` | Reine Logik: Baum, Auswahl, Sperren, Größen, Vorauswahl, Pakettabelle |
| `src-tauri/src/konfig.rs` | Konfigurationsadresse auflösen und prüfen |
| `src-tauri/src/katalog.rs` | Katalog, Auswahl auflösen, Plan gegen die Ablage |
| `src-tauri/src/ablage.rs` | Datenordner, sichere Pfade, Zustand, freier Speicher |
| `src-tauri/src/laden.rs` | Downloader mit Range, SHA-256 und Spiegeln |
| `src-tauri/src/anmeldung.rs` | OIDC PKCE mit Loopback-Rückruf, `offline_access`, Erneuerung, Widerruf, Übernahme aus dem Schlüsselbund |
| `src-tauri/src/oberflaeche.rs` | Sitzung der Oberfläche aus der Anmeldung: Cookie, Einmalcode, Login-Umleitung, beidseitiges Abmelden |
| `src-tauri/src/tresor.rs` | Verschlüsselter Token-Tresor im Konfigurationsordner (ChaCha20-Poly1305, DPAPI/0600) |
| `src-tauri/src/aktualisierung.rs` | Update-Prüfung gegen `client_update` (tauri-plugin-updater) |
| `werkzeug/update-manifest.mjs` | `latest.json` → `update.json` mit Adressen des eigenen Servers |
| `src-tauri/src/lokal.rs` | Loopback-Server für lokale Pakete, Initialisierungsskript |
| `src-tauri/src/pmtiles.rs` | PMTiles v3 lesen (Hilbert-Ids, Blattverzeichnisse) |
| `src-tauri/src/vollbild.rs` | Kommandozeile, Monitorwahl, Schwebeknopf-Skript |
| `src-tauri/src/nebenfenster.rs` | Neue Fenster: Zielentscheidung, Zweitschirm, Schließknopf-Skript |
| `src-tauri/src/eingabemethode.rs` | Linux: ohne laufenden ibus-/fcitx-Daemon auf `gtk-im-context-simple` ausweichen |
| `src-tauri/src/uebersicht.rs` | Pakettabelle, Paketstand nach dem Download, Paket entfernen |
| `src-tauri/src/lib.rs` | Tauri-Befehle, Menü, Navigation zur Server-Oberfläche, Vollbild |

**Sicherheit:** IPC-Rechte hat nur die lokale Assistentenseite
(`capabilities/default.json` ohne `remote`). Die Server-Oberfläche im selben
Fenster kann keine Befehle aufrufen. Ein Katalog kann nicht außerhalb des
Datenordners schreiben (`ablage::sicherer_pfad`).

## Offen

- Ende-zu-Ende im echten Fenster: Server-Oberfläche per https mit lokalen
  Kacheln vom Loopback (WebView2 unter Windows, WebKitGTK unter Linux).
- Ebenso nur im echten Fenster prüfbar: ob `set_cookie` das Sitzungs-Cookie
  unter WebView2 und WebKitGTK annimmt (sonst greift der Einmalcode), das
  Abfangen von `/api/auth/login` und das Löschen der Cookies beim Abmelden.
- Prüfung der optionalen Katalog-Signatur (`daten.signatur_schluessel`,
  `katalog.json.sig`, Ed25519). Der Server legt sie mit `--signieren` an, der
  Client prüft sie noch nicht.
