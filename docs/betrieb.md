# Betrieb

Wie Zunder auf dem eigenen Server läuft: Container, Konfiguration, Login, Tokens, MCP, Backups und Updates. Verbindlich für Schnittstellen und Variablen ist der [Implementierungsvertrag](superpowers/plans/2026-10-11-zunder-implementation.md), Abschnitt 3.

## Überblick

- **Ein Container** `ghcr.io/rubenvitt/zunder` (amd64 und arm64) enthält `zunder-server`, die gebaute Web-UI und `zunder-import`.
- **Daten** liegen unter `/data`, auf dem Server gemountet aus `~/server/zunder/data`: SQLite-Datenbank `zunder.db` (WAL-Modus) und nächtliche Backups in `/data/backups/`.
- **Port** 8080 im Container, nach außen nur über Traefik (keine Port-Mappings).
- **Login** über Pocket-ID (OIDC), für Agenten und MCP gibt es API-Tokens.
- **Stack-Dateien** liegen in [`deploy/`](../deploy): `compose.yaml`, `.env.example` (Vorlage) und `generate-secrets.sh`. Auf dem Server liegen sie unter `~/server/zunder/` und werden von Hand mit `docker compose` betrieben (kein Portainer).
- **Server** ist `r-server` (Hetzner, amd64); Traefik hängt am externen Netzwerk `proxy`, Entrypoint `https`, Zertifikate über den Resolver `letsencrypt`.

### Warum eine öffentliche Domain

Zunder läuft unter **`zunder.rubeen.dev`** (öffentlich) und nicht unter `*.rubeen.home` (Tailscale). Der Grund ist der MCP-Endpoint `/mcp`: claude.ai ruft Connectors aus der Cloud von Anthropic auf und kommt nicht ins Tailnet. Auch Claude Code auf fremden Rechnern und Agenten in CI-Läufen erreichen so dieselbe Instanz.

Was das für die Sicherheit heißt:

- Ohne Anmeldung erreichbar sind nur `/healthz`, die Login-Strecke unter `/auth/*`, die statischen Dateien der Web-UI und der OAuth-Autorisierungsserver für claude.ai (siehe [claude.ai](#claudeai)):
  - Metadaten unter `/.well-known/oauth-protected-resource`, `/.well-known/oauth-protected-resource/mcp` und `/.well-known/oauth-authorization-server`,
  - die offene dynamische Client-Registrierung `POST /oauth/register` (höchstens 30 Registrierungen pro Stunde und 500 Clients insgesamt; Clients ohne gültige Freigabe verschwinden nach einem Tag),
  - `GET /oauth/authorize` (prüft die Anfrage und leitet ohne Session auf den Login um; die Freigabe `POST /oauth/authorize` verlangt eine Session),
  - `POST /oauth/token` und `POST /oauth/revoke` (wirken nur mit einem Code, Refresh- oder Access-Token aus einer erteilten Freigabe).

  Alles unter `/api/*` verlangt eine Session oder einen API-Token, `/mcp` einen API-Token oder ein OAuth-Access-Token. Wer vor Traefik eine IP-Allowlist oder Forward-Auth schaltet, muss `/.well-known/*` und `/oauth/*` (wie `/mcp`) davon ausnehmen, sonst funktioniert der claude.ai-Connector nicht.
- Anmelden darf sich genau eine Person (`ZUNDER_OIDC_ALLOWED_EMAIL` bzw. `ZUNDER_OIDC_ALLOWED_SUBJECT`).
- API-Tokens liegen in der Datenbank nur als SHA-256-Hash und sind einzeln widerrufbar.
- TLS übernimmt Traefik.

## Erstinbetriebnahme

### 1. OIDC-Client in Pocket-ID anlegen

In der Pocket-ID-Administration unter **OIDC Clients → Add OIDC Client**:

| Feld | Wert |
|---|---|
| Name | `Zunder` |
| Callback URLs | `https://zunder.rubeen.dev/auth/callback` |
| Logout Callback URLs | `https://zunder.rubeen.dev/` (optional) |
| Public Client | aus (Zunder ist ein vertraulicher Client mit Secret) |
| PKCE | an (Zunder nutzt Authorization Code + PKCE) |

Danach notieren:

- **Client ID** → `ZUNDER_OIDC_CLIENT_ID`
- **Client Secret** (wird nur einmal angezeigt) → `ZUNDER_OIDC_CLIENT_SECRET`
- **Issuer** = Basis-URL von Pocket-ID, z. B. `https://id.example.com` → `ZUNDER_OIDC_ISSUER`. Zunder liest daraus `/.well-known/openid-configuration`.

Optional kann der Client in Pocket-ID auf eine Benutzergruppe beschränkt werden. Zunder prüft unabhängig davon selbst, wer sich anmeldet.

### 2. Ordner auf dem Server anlegen

```sh
mkdir -p ~/server/zunder/data
cd ~/server/zunder
# compose.yaml, .env.example und generate-secrets.sh aus deploy/ hierher kopieren, z. B.:
for f in compose.yaml .env.example generate-secrets.sh; do
  curl -fsSLO "https://raw.githubusercontent.com/rubenvitt/zunder/main/deploy/$f"
done
chmod +x generate-secrets.sh
sudo chown 10001:10001 data   # der Container läuft als uid/gid 10001
```

### 3. Geheimnisse erzeugen und starten

```sh
cd ~/server/zunder
./generate-secrets.sh
docker compose pull
docker compose up -d
docker compose logs -f zunder
```

Das Skript

- erzeugt `ZUNDER_SESSION_SECRET` (64 Zeichen, zufällig),
- fragt Issuer, Client ID, Client Secret und die erlaubte E-Mail ab,
- fragt optional Timing-Token und Timing-Projekt ab (leer lassen = Timing-Brücke aus; beides lässt sich auch später in der Web-UI unter **Einstellungen** eintragen),
- schreibt das Ergebnis nach `.env` neben `compose.yaml` (Rechte 600). `compose.yaml` lädt sie per `env_file`.

`.env.example` bleibt unverändert: Es ist die Vorlage mit den Platzhaltern `WILL_BE_GENERATED`, `CHANGEME_MANUAL` und `CHANGEME_OPTIONAL`. Im Repo ist `deploy/.env` von Git ignoriert.

Voraussetzungen auf dem Server: das externe Netzwerk `proxy` mit Traefik (Entrypoint `https`, Zertifikats-Resolver `letsencrypt`) und ein DNS-Eintrag für `zunder.rubeen.dev`. Heißen Netzwerk, Entrypoint oder Resolver dort anders, die drei Werte in `compose.yaml` anpassen (`docker network ls`, Traefik-Konfiguration unter `~/server/traefik/`).

Ist das GHCR-Paket privat, braucht der Server eine Anmeldung: `docker login ghcr.io` mit GitHub-Nutzer und Personal Access Token (`read:packages`). Alternativ das Paket in den GitHub-Einstellungen auf öffentlich stellen; es enthält keine Geheimnisse.

### Automatisches Ausrollen (optional)

Nach jedem grünen Push auf `main` legt die CI ein GitHub-Deployment (Umgebung `production`, `payload.tag` = voller Commit-SHA) an, wie bei den anderen Projekten auf r-server. Der `~/server/deploy-agent` holt es ab, zieht `ghcr.io/rubenvitt/zunder:<sha>`, startet neu, wartet auf `healthy` und meldet Erfolg oder Fehler zurück. Damit das greift, muss Zunder im deploy-agent eingetragen sein. Ohne Eintrag bleibt das Deployment auf GitHub einfach in der Warteschlange und es passiert nichts.

### 4. Prüfen

```sh
curl -fsS https://zunder.rubeen.dev/healthz
# {"status":"ok","version":"0.1.0"}
```

Dann `https://zunder.rubeen.dev` im Browser öffnen, über Pocket-ID anmelden und unter **Tokens** den ersten API-Token anlegen.

## Umgebungsvariablen

| Variable | Wert im Stack | Herkunft |
|---|---|---|
| `ZUNDER_PUBLIC_URL` | `https://zunder.rubeen.dev` | statisch in `compose.yaml` |
| `ZUNDER_BIND` | `0.0.0.0:8080` | statisch (auch Image-Default) |
| `ZUNDER_DATA_DIR` | `/data` | statisch (auch Image-Default) |
| `ZUNDER_WEB_DIR` | `/app/web` | statisch (auch Image-Default) |
| `ZUNDER_BACKUP_HOUR` | `3` | statisch; Stunde in der Zeitzone `TZ` |
| `TZ` | `Europe/Berlin` | statisch (auch Image-Default, `tzdata` ist im Image); bestimmt, was „3 Uhr“ heißt |
| `ZUNDER_OIDC_ISSUER` | Pocket-ID-URL | `.env`, manuell |
| `ZUNDER_OIDC_CLIENT_ID` | aus Pocket-ID | `.env`, manuell |
| `ZUNDER_OIDC_CLIENT_SECRET` | aus Pocket-ID | `.env`, manuell |
| `ZUNDER_OIDC_ALLOWED_EMAIL` | E-Mail des eigenen Pocket-ID-Kontos | `.env`, manuell |
| `ZUNDER_OIDC_ALLOWED_SUBJECT` | Alternative zur E-Mail: `sub` des Kontos (in Pocket-ID die Benutzer-ID) | nicht gesetzt; bei Bedarf in `compose.yaml` ergänzen |
| `ZUNDER_SESSION_SECRET` | zufällig | `.env`, generiert |
| `ZUNDER_TIMING_TOKEN` | Timing-API-Token | `.env`, optional (leer = aus); ein Token aus den **Einstellungen** der Web-UI hat Vorrang |
| `ZUNDER_TIMING_PROJECT` | Timing-Projekt für Flammen: Self-Link (z. B. `/projects/1`) oder Projekttitel, wird unverändert an Timing gegeben (eine reine Zahl liest Timing als Titel) | `.env`, optional; ein Projekt aus den **Einstellungen** hat Vorrang |
| `ZUNDER_BOOTSTRAP_TOKEN` | fester API-Token `zt_…` | nicht gesetzt; nur für Tests oder eine Inbetriebnahme ohne Web-Login |
| `ZUNDER_DEV_AUTH` | `true` = Login ohne OIDC | **nie in Produktion**, nur lokal |
| `ZUNDER_DEV_AUTH_ALLOW_REMOTE` | `true` = Dev-Auth auch mit nicht lokaler `ZUNDER_BIND` (nur für einen lokalen Container mit `-p 127.0.0.1:…`) | **nie in Produktion** |

Leere Werte gelten als nicht gesetzt. Timing lässt sich statt über `.env` auch in der Web-UI unter **Einstellungen** einrichten: Die Werte liegen dann in der Datenbank (Tabelle `settings`, der Token im Klartext, also auch in den Backups), gelten sofort ohne Neustart und überschreiben die beiden Variablen; „Zurücksetzen“ dort lässt wieder die Variablen gelten. Wer das Session-Secret ändert, meldet alle Browser-Sessions ab; API-Tokens bleiben gültig.

## API-Tokens

- Anlegen und widerrufen in der Web-UI unter **Tokens** (nur mit Browser-Session, nicht per Token).
- Ein Token hat die Form `zt_<43 Zeichen>` und wird **nur einmal** angezeigt. Gespeichert wird nur der SHA-256-Hash.
- Je Verwendung ein eigener Token mit sprechendem Namen, z. B. `claude-code-laptop`, `import`, `funke-iphone` (claude.ai braucht keinen Token, siehe [claude.ai](#claudeai)). Der Name erscheint im Änderungsprotokoll als Quelle (`mcp:<name>`), und ein verlorenes Gerät lässt sich einzeln sperren.
- Die Token-Liste zeigt `last_used_at`. Tokens, die lange nicht benutzt wurden, widerrufen.
- Verwendung: Header `Authorization: Bearer zt_…`, sowohl für `/api/*` als auch für `/mcp`.
- Darunter stehen unter **Verbundene Apps** die per OAuth verbundenen Clients (z. B. claude.ai) mit Ziel-Host und letzter Nutzung. **Trennen** widerruft die Verbindung sofort samt allen Tokens.

## MCP verbinden

Endpoint: `https://zunder.rubeen.dev/mcp` (Streamable HTTP, JSON-RPC 2.0, Protokollversion `2025-06-18`). Werkzeuge: siehe Vertrag, Abschnitt 4, und [`umstieg.md`](umstieg.md) für die Zuordnung zu den ClickUp-Werkzeugen.

### Claude Code

Für alle Projekte auf diesem Rechner:

```sh
claude mcp add --transport http --scope user zunder https://zunder.rubeen.dev/mcp \
  --header "Authorization: Bearer zt_…"
```

Oder projektbezogen in `.mcp.json`, mit dem Token aus einer Umgebungsvariable statt im Klartext:

```json
{
  "mcpServers": {
    "zunder": {
      "type": "http",
      "url": "https://zunder.rubeen.dev/mcp",
      "headers": { "Authorization": "Bearer ${ZUNDER_TOKEN}" }
    }
  }
}
```

Prüfen mit `claude mcp list` bzw. `/mcp` in einer Sitzung.

### claude.ai

claude.ai verbindet sich per OAuth (MCP-Authorization); Zunder bringt dafür einen eigenen kleinen Authorization-Server mit, die Anmeldung läuft über den normalen Pocket-ID-Login. Einen Token muss man dafür nicht anlegen.

1. In claude.ai **Einstellungen → Connectors → Benutzerdefinierten Connector hinzufügen**.
2. Name `Zunder`, URL **`https://zunder.rubeen.dev/mcp`**. Die erweiterten Einstellungen (OAuth-Client-ID und -Secret) leer lassen: claude.ai registriert sich selbst.
3. **Verbinden** öffnet Zunder im Browser. Ohne aktive Sitzung kommt zuerst der Pocket-ID-Login, danach die Freigabeseite „Zugriff erlauben?“ mit dem Namen des Clients (`Claude`) und dem Ziel `claude.ai`. Steht dort ein anderes Ziel, **Ablehnen**.
4. Nach **Erlauben** ist der Connector verbunden. Änderungen erscheinen im Änderungsprotokoll als „Agent · Claude (OAuth)“ (Quelle `mcp:oauth:Claude`).

Gut zu wissen:

- claude.ai erneuert den Zugriff selbst (Access-Token 1 Stunde, Refresh-Token 30 Tage, bei jeder Erneuerung neu). Wer den Connector 30 Tage nicht nutzt, verbindet ihn neu.
- Trennen: in Zunder unter **Tokens → Verbundene Apps → Trennen** oder in claude.ai den Connector entfernen. Bei einer erneuten Verbindung registriert sich claude.ai neu und Zunder fragt wieder nach der Freigabe.
- Voraussetzungen: `ZUNDER_PUBLIC_URL` ist genau `https://zunder.rubeen.dev` (daraus entstehen Issuer, Ressource und alle Endpunkte), und Traefik leitet auch `/.well-known/*` und `/oauth/*` an den Container (die Regel `Host(...)` tut das). Ändert sich die öffentliche URL, werden alle OAuth-Tokens ungültig und der Connector muss neu verbunden werden.
- Prüfen ohne claude.ai: `curl -s https://zunder.rubeen.dev/.well-known/oauth-protected-resource` muss `"resource":"https://zunder.rubeen.dev/mcp"` liefern, und `curl -si -X POST https://zunder.rubeen.dev/mcp` einen `401` mit `WWW-Authenticate: Bearer resource_metadata=…`.
- Zeigt claude.ai „Authorization with the MCP server failed“, im Log von Zunder nach `OAuth` suchen (Registrierung, Freigabe, Wiederverwendung eines Refresh-Tokens werden geloggt, Tokens nie).

Claude Code und eigene Agenten nutzen weiter API-Tokens per Header (siehe oben); das bleibt unverändert.

### Andere Agenten und Handtest

```sh
curl -sS https://zunder.rubeen.dev/mcp \
  -H "Authorization: Bearer $ZUNDER_TOKEN" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"handtest","version":"0"}}}'
```

Ohne gültigen Token antwortet der Endpoint mit `401`.

## Health-Endpoint

- `GET /healthz` ohne Anmeldung → `{"status":"ok","version":"…"}` mit HTTP 200, sobald die Datenbank offen ist.
- Das Image hat einen Docker-`HEALTHCHECK` (`zunder-server healthcheck` fragt `/healthz` auf dem Port aus `ZUNDER_BIND` ab, alle 30 s, in den ersten 20 s nach dem Start alle 2 s). Traefik leitet erst auf einen Container mit Status `healthy` weiter; nach einem Neustart oder Stack-Update ist Zunder dadurch nach etwa 2 s wieder erreichbar. Status abfragen:

  ```sh
  docker inspect --format '{{.State.Health.Status}}' zunder
  ```

- Für ein externes Monitoring (z. B. Uptime Kuma) `https://zunder.rubeen.dev/healthz` prüfen und auf das Schlüsselwort `"ok"` achten.
- Der Healthcheck liest `ZUNDER_BIND` und folgt damit einem geänderten Port; ein eigener Healthcheck in der Compose-Datei ist nicht nötig.

## Backups

### Automatisch

- Jede Nacht zur Stunde `ZUNDER_BACKUP_HOUR` (Default 3, Zeitzone `TZ`) schreibt der Server per `VACUUM INTO` eine konsistente Kopie der Datenbank nach `/data/backups/`.
- Die **14** neuesten Backups bleiben liegen, ältere löscht der Server.
- `VACUUM INTO` läuft bei laufendem Betrieb und erzeugt eine kompakte, in sich geschlossene Datei ohne WAL.

### Vom Server wegkopieren

Die Backups liegen im selben Ordner wie die Datenbank und schützen damit nur vor Bedienfehlern und kaputten Updates, nicht vor einem Plattenausfall. Auf dem Host ist das `~/server/zunder/data/` (Datenbank) bzw. `~/server/zunder/data/backups/`; beides liegt unter `~/server` und damit in der normalen Server-Sicherung.

### Manuelles Backup (z. B. vor einem Update)

```sh
docker exec zunder sqlite3 /data/zunder.db "VACUUM INTO '/data/manuell-$(date +%F-%H%M).db'"
```

Manuelle Kopien bewusst nicht nach `/data/backups/` legen, damit die Rotation sie nicht wegräumt.

### Wiederherstellen

1. Zunder stoppen: `cd ~/server/zunder && docker compose stop zunder`.
2. Gewünschtes Backup auswählen und an die Stelle der Datenbank kopieren. Die alte Datenbank wird zur Sicherheit beiseitegelegt, WAL- und SHM-Dateien müssen weg:

   ```sh
   docker compose run --rm --no-deps --entrypoint sh zunder
   # im Container:
   ls -l /data/backups/
   mkdir -p /data/vor-wiederherstellung
   mv /data/zunder.db /data/zunder.db-wal /data/zunder.db-shm /data/vor-wiederherstellung/ 2>/dev/null
   cp /data/backups/<DATEI> /data/zunder.db
   sqlite3 /data/zunder.db "PRAGMA integrity_check"   # muss "ok" ausgeben
   exit
   ```

3. `docker compose start zunder` und `/healthz` prüfen.
4. Alle offenen Browser-Tabs neu laden. Änderungen nach dem Zeitpunkt des Backups sind weg, außer ein Client hat sie noch ungesendet in seiner Warteschlange und schickt sie erneut. Die Tauri-App merkt beim nächsten Abgleich selbst, dass das Operations-Log des Servers nicht mehr zu ihrem passt, verwirft ihre bestätigten Daten, behält ihre noch nicht gesendeten Änderungen und lädt den Snapshot neu; dazu muss sie nur einmal mit dem Server abgleichen.
5. Wenn alles passt, `/data/vor-wiederherstellung/` löschen.

## Updates

- Jeder Push auf `main` baut `ghcr.io/rubenvitt/zunder:latest` neu (dazu `sha-<kurz>` und den vollen Commit-SHA als Tag). Jedes Release (automatisch nach einem grünen Push auf `main` mit `feat:`/`fix:`-Commits, siehe [`apps/zunder-app/README.md`](../apps/zunder-app/README.md#updates-und-releases)) hängt zusätzlich die Versions-Tags an, etwa `1.2.3`, `1.2` und `1` (bei `0.x` ohne den Major-Tag).
- Einspielen von Hand: `cd ~/server/zunder && docker compose pull && docker compose up -d` (oder `~/server/server.sh update` für alle Stacks). Mit eingetragenem deploy-agent passiert das nach jedem grünen Push auf `main` automatisch (siehe [Automatisches Ausrollen](#automatisches-ausrollen-optional)).
- Vorher ein manuelles Backup ziehen (siehe oben). Datenbank-Migrationen laufen beim Start des Servers.
- **Zurückrollen:** in `compose.yaml` statt `latest` den vorherigen Tag eintragen (`sha-<commit>` oder Versionsnummer) und `docker compose up -d`. Hat die neue Version das Schema migriert, zusätzlich das Backup von vor dem Update wiederherstellen.
- Wer Updates bewusst steuern will, pinnt in `compose.yaml` eine Version statt `latest`.

### Umstellung der IDs auf CUID2 (einmalig)

Die erste Version mit Schema 5 stellt beim Start alle IDs von UUID auf CUID2 um, ohne Zutun:

1. Sie schreibt `backups/vor-cuid2-<UTC-Zeit>.db` (wird nie automatisch gelöscht).
2. Sie rechnet Oplog und Bezüge in einer Transaktion um und baut den Zustand neu auf. Im Log steht `IDs switched to CUID2` mit der Zahl der Operationen.
3. Die Zuordnung alt → neu liegt in `<Datenverzeichnis>/id-zuordnung-<UTC-Zeit>.json` (`entity`, `old_id`, `new_id`, `name`).

Vorher bzw. danach:

- **Apps zuerst aktualisieren.** Web-UI und App (ab dieser Version) rechnen ihre gespeicherten Daten und noch nicht abgeglichene Änderungen beim Start selbst um. Eine ältere App-Version kann mit dem umgestellten Server nicht mehr abgleichen.
- **Alte IDs funktionieren weiter** als Eingabe: im MCP (`task_id`, `area_id`, `project_id`), in Links `#/aufgabe/<uuid>`, beim Import (`existing_id`) und in Pushes von Clients, die noch UUIDs erzeugen (Funke). Die Umrechnung ist fest, deshalb muss nichts sofort angepasst werden.
- **Vault:** Die `zunder_id`s in den Notizen sind noch die alten. Mit der Zuordnungsliste lassen sie sich ersetzen; bis dahin findet das MCP die Bereiche und Projekte auch über die alte ID.
- **Zurückrollen:** Eine ältere Version startet nicht mit Schema 5. Dann `vor-cuid2-….db` wie unter [Wiederherstellen](#wiederherstellen) einspielen; alles, was seit der Umstellung geändert wurde, ist danach weg.

### Kommentare (Schema 6)

Die erste Version mit Schema 6 baut beim Start die Tabellen `ops` und `entities` neu auf, damit sie Kommentare aufnehmen (eine Transaktion; Cursor und Daten bleiben). Vorher ein manuelles Backup ziehen. Die Zunder-App muss danach mindestens diese Version haben, sonst scheitert ihr Abgleich, sobald es den ersten Kommentar gibt. **Zurückrollen:** Eine ältere Version startet nicht mit Schema 6; dann das Backup von vor dem Update einspielen.

## Fehlersuche

| Symptom | Prüfen |
|---|---|
| 404 von Traefik / keine Verbindung | Container im Netzwerk `proxy`? Entrypoint `https` und Resolver `letsencrypt` passen zur Traefik-Konfiguration? `docker logs` von Traefik |
| Container startet neu | `docker logs zunder`: meist fehlende oder falsche OIDC-Variablen |
| Login-Schleife oder Fehler nach Pocket-ID | Callback-URL in Pocket-ID exakt `https://zunder.rubeen.dev/auth/callback`? `ZUNDER_PUBLIC_URL` korrekt? Uhrzeit des Servers korrekt? |
| „Nicht berechtigt“ nach erfolgreichem Pocket-ID-Login | `ZUNDER_OIDC_ALLOWED_EMAIL` stimmt nicht mit der E-Mail im Pocket-ID-Konto überein |
| MCP `401` | Token widerrufen oder falsch kopiert; Header `Authorization: Bearer zt_…` |
| Backup fehlt | Logs um `ZUNDER_BACKUP_HOUR`, Zeitzone `TZ`, freier Platz im Volume |
| `unhealthy` | `docker exec zunder zunder-server healthcheck` (Exit-Code und Meldung), dazu `docker logs zunder` |

## Image selbst bauen

```sh
docker build -t zunder .
# beide Architekturen (braucht buildx; QEMU nur für die kleine Runtime-Stufe)
docker buildx build --platform linux/amd64,linux/arm64 -t zunder .
```

Das `Dockerfile` baut Web-UI und Rust auf der Plattform des Build-Rechners und kompiliert für die Zielarchitektur über Kreuz (Debian-Cross-GCC für das gebündelte SQLite). Das ist deutlich schneller und stabiler als `rustc` unter QEMU. Die Abhängigkeiten werden mit `cargo-chef` in einer eigenen Schicht zwischengespeichert. Details stehen als Kommentar im `Dockerfile`.
