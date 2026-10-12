# Betrieb

Wie Zunder auf dem eigenen Server läuft: Container, Konfiguration, Login, Tokens, MCP, Backups und Updates. Verbindlich für Schnittstellen und Variablen ist der [Implementierungsvertrag](superpowers/plans/2026-10-11-zunder-implementation.md), Abschnitt 3.

## Überblick

- **Ein Container** `ghcr.io/rubenvitt/zunder` (amd64 und arm64) enthält `zunder-server`, die gebaute Web-UI und `zunder-import`.
- **Daten** liegen im Volume unter `/data`: SQLite-Datenbank `zunder.db` (WAL-Modus) und nächtliche Backups in `/data/backups/`.
- **Port** 8080 im Container, nach außen nur über Traefik (keine Port-Mappings).
- **Login** über Pocket-ID (OIDC), für Agenten und MCP gibt es API-Tokens.
- **Stack-Dateien** liegen in [`deploy/`](../deploy): `compose.yaml`, `stack.env` (Vorlage) und `generate-secrets.sh`.

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

### 2. Geheimnisse erzeugen

Auf einem Rechner mit Repo-Checkout:

```sh
cd deploy
./generate-secrets.sh
```

Das Skript

- erzeugt `ZUNDER_SESSION_SECRET` (64 Zeichen, zufällig),
- fragt Issuer, Client ID, Client Secret und die erlaubte E-Mail ab,
- fragt optional Timing-Token und Timing-Projekt ab (leer lassen = Timing-Brücke aus),
- schreibt das Ergebnis nach `deploy/stack.env.local` (Rechte 600, von Git ignoriert) und gibt es zum Kopieren aus.

`deploy/stack.env` selbst bleibt unverändert: Es ist die eingecheckte Vorlage mit den Platzhaltern `WILL_BE_GENERATED`, `CHANGEME_MANUAL` und `CHANGEME_OPTIONAL`. Nach dem Kopieren nach Portainer `deploy/stack.env.local` löschen und `history -c` ausführen.

### 3. Stack in Portainer anlegen

1. **Stacks → Add stack**, Name `zunder`.
2. Inhalt von `deploy/compose.yaml` einfügen (oder per Git-Repository auf `deploy/compose.yaml` zeigen).
3. Unter **Environment variables → Advanced mode** den Inhalt aus `stack.env.local` einfügen.
4. **Deploy the stack.**

Voraussetzungen auf dem Server: das externe Netzwerk `infrastructure` mit Traefik (Entrypoint `websecure`) und ein DNS-Eintrag für `zunder.rubeen.dev`. Das Volume `zunder_data` legt Docker beim ersten Start selbst an (in Portainer heißt es dann `zunder_zunder_data`).

Ist das GHCR-Paket privat, braucht Portainer eine Registry-Anmeldung für `ghcr.io` (GitHub-Nutzer plus Personal Access Token mit `read:packages`). Alternativ das Paket in den GitHub-Einstellungen auf öffentlich stellen; es enthält keine Geheimnisse.

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
| `ZUNDER_OIDC_ISSUER` | Pocket-ID-URL | `stack.env`, manuell |
| `ZUNDER_OIDC_CLIENT_ID` | aus Pocket-ID | `stack.env`, manuell |
| `ZUNDER_OIDC_CLIENT_SECRET` | aus Pocket-ID | `stack.env`, manuell |
| `ZUNDER_OIDC_ALLOWED_EMAIL` | E-Mail des eigenen Pocket-ID-Kontos | `stack.env`, manuell |
| `ZUNDER_OIDC_ALLOWED_SUBJECT` | Alternative zur E-Mail: `sub` des Kontos (in Pocket-ID die Benutzer-ID) | nicht gesetzt; bei Bedarf in `compose.yaml` ergänzen |
| `ZUNDER_SESSION_SECRET` | zufällig | `stack.env`, generiert |
| `ZUNDER_TIMING_TOKEN` | Timing-API-Token | `stack.env`, optional (leer = aus) |
| `ZUNDER_TIMING_PROJECT` | Timing-Projekt für Flammen: Self-Link (z. B. `/projects/1`) oder Projekttitel, wird unverändert an Timing gegeben (eine reine Zahl liest Timing als Titel) | `stack.env`, optional |
| `ZUNDER_BOOTSTRAP_TOKEN` | fester API-Token `zt_…` | nicht gesetzt; nur für Tests oder eine Inbetriebnahme ohne Web-Login |
| `ZUNDER_DEV_AUTH` | `true` = Login ohne OIDC | **nie in Produktion**, nur lokal |

Leere Werte gelten als nicht gesetzt. Wer das Session-Secret ändert, meldet alle Browser-Sessions ab; API-Tokens bleiben gültig.

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
- Das Image hat einen Docker-`HEALTHCHECK` (`zunder-server healthcheck` fragt `/healthz` auf dem Port aus `ZUNDER_BIND` ab, alle 30 s). Status abfragen:

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

Die Backups liegen im selben Volume wie die Datenbank und schützen damit nur vor Bedienfehlern und kaputten Updates, nicht vor einem Plattenausfall. Das Volume (oder nur `backups/`) gehört deshalb in die übliche Server-Sicherung. Pfad auf dem Host:

```sh
docker volume inspect --format '{{.Mountpoint}}' zunder_zunder_data
```

### Manuelles Backup (z. B. vor einem Update)

```sh
docker exec zunder sqlite3 /data/zunder.db "VACUUM INTO '/data/manuell-$(date +%F-%H%M).db'"
```

Manuelle Kopien bewusst nicht nach `/data/backups/` legen, damit die Rotation sie nicht wegräumt.

### Wiederherstellen

1. Stack in Portainer stoppen (oder `docker stop zunder`).
2. Gewünschtes Backup auswählen und an die Stelle der Datenbank kopieren. Die alte Datenbank wird zur Sicherheit beiseitegelegt, WAL- und SHM-Dateien müssen weg:

   ```sh
   docker run --rm -it -v zunder_zunder_data:/data --entrypoint sh ghcr.io/rubenvitt/zunder:latest
   # im Container:
   ls -l /data/backups/
   mkdir -p /data/vor-wiederherstellung
   mv /data/zunder.db /data/zunder.db-wal /data/zunder.db-shm /data/vor-wiederherstellung/ 2>/dev/null
   cp /data/backups/<DATEI> /data/zunder.db
   sqlite3 /data/zunder.db "PRAGMA integrity_check"   # muss "ok" ausgeben
   exit
   ```

3. Stack wieder starten und `/healthz` prüfen.
4. Alle offenen Browser-Tabs neu laden. Änderungen nach dem Zeitpunkt des Backups sind weg, außer ein Client hat sie noch ungesendet in seiner Warteschlange und schickt sie erneut.
5. Wenn alles passt, `/data/vor-wiederherstellung/` löschen.

## Updates

- Jeder Push auf `main` baut `ghcr.io/rubenvitt/zunder:latest` neu (dazu `sha-<commit>`). Ein Git-Tag `v1.2.3` erzeugt zusätzlich `1.2.3`, `1.2` und `1`.
- Einspielen: in Portainer beim Stack **Update the stack** mit **Re-pull image** (bzw. „Pull and redeploy“).
- Vorher ein manuelles Backup ziehen (siehe oben). Datenbank-Migrationen laufen beim Start des Servers.
- **Zurückrollen:** in `compose.yaml` statt `latest` den vorherigen Tag eintragen (`sha-<commit>` oder Versionsnummer) und neu deployen. Hat die neue Version das Schema migriert, zusätzlich das Backup von vor dem Update wiederherstellen.
- Wer Updates bewusst steuern will, pinnt in `compose.yaml` eine Version statt `latest`.

## Fehlersuche

| Symptom | Prüfen |
|---|---|
| 404 von Traefik / keine Verbindung | Container im Netzwerk `infrastructure`? Labels korrekt? Traefik-Dashboard |
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
