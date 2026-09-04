<div align="center">

# CC Switch — Edition für Remote-Server

### Ein unabhängiger Fork von [farion1231/cc-switch](https://github.com/farion1231/cc-switch)

[![Version](https://img.shields.io/github/v/release/xh20220630/cc-switch-agent?color=blue&label=version)](https://github.com/xh20220630/cc-switch-agent/releases)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux-lightgrey.svg)](https://github.com/xh20220630/cc-switch-agent/releases)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-orange.svg)](https://tauri.app/)

[English](README.md) | [中文](README_ZH.md) | [日本語](README_JA.md) | Deutsch

</div>

---

## Was ist das

Dieses Repository ist ein unabhängiger Fork von [CC Switch](https://github.com/farion1231/cc-switch) (MIT). Upstream ist eine Desktop-App auf Basis von Tauri 2, die Provider-API-Konfigurationen für Claude Code, Claude Desktop, Codex, Gemini CLI, Grok Build, OpenCode, OpenClaw und Hermes zentral verwaltet — mit One-Click-Umschalten, MCP/Prompts/Skills-Verwaltung, Proxy & Failover, Nutzungsverfolgung, Cloud-Sync und mehr.

**Den vollständigen Funktionsumfang und die Dokumentation von Upstream finden Sie direkt im Upstream-Repository:**

- Projektseite & Dokumentation: <https://github.com/farion1231/cc-switch>
- Benutzerhandbuch: <https://github.com/farion1231/cc-switch/tree/main/docs/user-manual>
- Changelog: <https://github.com/farion1231/cc-switch/blob/main/CHANGELOG.md>

Dieser Fork **behält alle Upstream-Funktionen und fügt einen „Remote-Server-Modus" hinzu**: Verbinden Sie sich per SSH mit einem entfernten Linux-Server, führen Sie dort einen ephemeren Agenten aus, lesen und schreiben Sie direkt die entfernte Datenbank `~/.cc-switch`, importieren und zeigen Sie die Sitzungsnutzung von Claude/Gemini/OpenCode/Grok/Codex/Kimi auf dem Remote-Server an und synchronisieren Sie Provider zwischen lokal und remote.

---

## Was dieser Fork hinzufügt

### Remote-Server-Modus (Remote Runtime)

Viele Benutzer betreiben Claude Code / Codex / Gemini auf einem **entfernten Linux-Server** (Cloud-Hosts, Container, Entwicklungsrechner), während der Desktop nur die lokale Konfiguration verwaltet. Dieser Fork verbindet den Desktop direkt mit dem Remote-System und „transportiert den Rechenkern" dorthin:

- **SSH-Zielverwaltung** — Ziele automatisch aus `~/.ssh/config` erkennen oder manuell hinzufügen; Verbindungstest, Bearbeiten, Löschen
- **Ephemerer Agent, kommt und geht mit der Verbindung** — der Desktop bündelt einen musl-statisch gelinkten Linux-Agenten (x86_64 / aarch64). Beim Verbinden wird er per scp nach `/tmp` hochgeladen, mit `--stdio` ausgeführt und löscht sich beim Beenden selbst (trap-Cleanup + Fallback vom Desktop)
- **Remote-DB direkt lesen/schreiben** — der Agent öffnet die eigene Datenbank `~/.cc-switch/cc-switch.db` des Remote-Benutzers und verhält sich exakt wie lokal (derselbe Core-Importer) — keine „gezogene Kopie"
- **Remote-Nutzungssynchronisierung** — automatische Synchronisierung einmal bei erfolgreicher Verbindung, erneut beim Öffnen der Usage-Seite, plus manuelle Synchronisierung jederzeit; importiert Sitzungsprotokolle aus allen sechs Quellen (Claude / Gemini / OpenCode / Grok / Codex / Kimi) in die entfernte Datenbank
- **Provider-Synchronisierung** — Provider zwischen lokal und remote synchronisieren, über einen Dialog mit Suche, Duplikat-Überspringen und Retry
- **Runtime-Umschalter** — Umschalten zwischen „lokal / Remote-Ziel" in der oberen Leiste, mit Live-Status (connecting / online / offline / reconnecting / incompatible)

```
┌─ Desktop (Windows / Linux) ─┐      SSH      ┌─ Remote-Server (Linux) ────────┐
│ Tauri-Frontend-UI           │   scp/ssh   │ Ephemerer Agent (cc-switch-agent)│
│  appInvoke ─► remote_invoke │ ──────────► │  │ öffnet Remote-~/.cc-switch/   │
│  RemoteRuntimeState         │ ◄────────── │  │  cc-switch.db                 │
│  eingebettete Agent-Bytes   │             │  │ Sitzungslogs: ~/.claude …     │
└─────────────────────────────┘             │  │ ~/.gemini ~/.grok ~/.codex    │
                                            └──────────────────────────────────┘
```

### Sicherheitsdesign

| Risiko | Gegenmaßnahme |
|---|---|
| Command-Injection | Remote-Befehle werden aus kontrollierten Vorlagen zusammengesetzt (Hex-Pfade / Dezimallängen / SHA-256); Benutzereingaben erreichen nie die Shell; SSH-Argumente werden als `OsString`-Array direkt übergeben, nie durch eine lokale Shell |
| Unbefugte Befehle | Zentrale Fähigkeits-Whitelist (`cc-switch-protocol`, 27 Befehle), beim Handshake zurückgemeldet, vom Desktop per `require()` erzwungen; nicht registrierte lokale Befehle werden remote verweigert |
| Cross-Talk (alte Antworten nach Zielwechsel) | Doppelt geprüfter Generation-Token + Scope-Isolation des Frontend-Query-Cache; verspätete Antworten während eines Wechsels werden abgewiesen |
| Leck von Anmeldedaten | Das Passwort ist eine In-Memory-Eingabe nur für die aktuelle Verbindung, wird nie in `remote-targets.json` geschrieben; optional DPAPI-verschlüsselte Speicherung (CryptProtectData) unter Windows, SSH_ASKPASS für TTY-lose Authentifizierung |
| Host-Key | Beim ersten Verbinden zeigt `ssh-keyscan` den Fingerabdruck; `known_hosts` wird erst nach Bestätigung durch den Benutzer geschrieben (XShell-Stil) |
| Rückstände auf dem Remote | Agent löscht sich selbst per Trap + SSH-Cleanup vom Desktop beim Drop |
| stdout-Log-Verschmutzung | Agent schreibt Diagnosen nur nach stderr; Protokoll-Frames tragen ein Magic-Marker zur Erkennung von Stromvermischung |
| Timeout-Hänger | Pro Befehl eine Whitelist-Timeout (30s–300s); bei Timeout wird Cancel gesendet (mit requestId + operationId gegen Fehlabbruch) |

### Remote-Bereich

- **Remote-Server**: nur Linux (`uname -s` Preflight), x86_64 oder aarch64
- **Desktop**: Windows (x86_64 / ARM64), Linux (x86_64 / ARM64)
- Gelesene Remote-Daten: Sitzungsdateien unter dem Remote-`$HOME` — `~/.claude`, `~/.gemini`, `~/.local/share/opencode`, `~/.grok`, `~/.codex`, `~/.kimi-code` — sowie die Datenbank `~/.cc-switch/`; lokale Dateien des Desktops werden weder hochgeladen noch gelesen

### Unterschiede zu Upstream

| Punkt | Upstream | Dieser Fork |
|---|---|---|
| Remote-Server-Modus | Nein | ✅ Hinzugefügt (Kern dieses Repos) |
| Provider-Synchronisierung | Nein | ✅ Hinzugefügt |
| Build-Plattformen | Windows / macOS / Linux | Windows x64/ARM64 + Linux x64/ARM64 |
| macOS-Signierung / Notarisierung | Apple Developer ID + notarization | Keine (Fork ohne Apple-Zertifikat, kein macOS-Build) |
| Auto-Update-Signierung / Endpunkte | Upstream-Schlüssel & Repo | Eigener Signierschlüssel des Forks und dieses Repo |
| Version | 3.x folgend Upstream | 0.0.1 unabhängiges Release |

---

## Installation & Download

Die neueste Version erhalten Sie auf [Releases](https://github.com/xh20220630/cc-switch-agent/releases):

| Plattform | Dateien |
|---|---|
| Windows (x86_64) | `CC-Switch-{version}-Windows.msi` (Installationsprogramm) / `Windows-Portable.zip` (portabel) |
| Windows (ARM64) | `CC-Switch-{version}-Windows-arm64.msi` / `Windows-arm64-Portable.zip` |
| Linux (x86_64) | `CC-Switch-{version}-Linux-x86_64.AppImage` / `.deb` / `.rpm` |
| Linux (ARM64) | `CC-Switch-{version}-Linux-arm64.AppImage` / `.deb` / `.rpm` |

> Dieser Fork liefert keine macOS-Builds (kein Apple-Signierzertifikat). macOS-Benutzer sollten das Upstream-Release verwenden.
> `.tar.gz` ist ausschließlich für den Tauri-Updater reserviert; kein manueller Download nötig.

## Schnellstart: Verbindung zu einem Remote-Server

1. **Remote vorbereiten** — ein Linux-Server (x86_64 / aarch64), SSH aktiviert, mit Zugriff auf `~/.cc-switch` und die Sitzungsverzeichnisse des Remote-Benutzers
2. **Ziel hinzufügen** — Einstellungen → Remote Targets; `~/.ssh/config` automatisch scannen oder Host / Benutzer / Port manuell eintragen
3. **Verbinden** — beim ersten Verbinden zeigt `ssh-keyscan` den Fingerabdruck des öffentlichen Serverschlüssels; `known_hosts` wird erst nach Bestätigung geschrieben; das Passwort gilt nur für diese Verbindung (optional verschlüsseltes Speichern unter Windows)
4. **Runtime umschalten** — im Runtime-Umschalter oben das Ziel wählen; die Remote-Nutzung wird einmal beim Verbinden und einmal beim Öffnen der Usage-Seite synchronisiert, danach jederzeit über „Sitzungen synchronisieren"
5. **Provider-Synchronisierung** — lokale Provider im Provider-Sync-Dialog auf den Remote pushen (oder von dort ziehen), mit Suche und Duplikat-Überspringen

> Kein 60-Sekunden-Daemon auf dem Remote: Der Agent ist ein ephemerer Prozess, der mit der Verbindung entsteht. Synchronisiert wird bei „Verbinden + Seitenaufruf + manuell".

## FAQ

<details>
<summary><strong>Wann erscheinen die Remote-Nutzungsdaten nach dem Verbinden?</strong></summary>

Das Backend synchronisiert im Moment des Verbindungserfolgs einmal, dann erneut beim Öffnen der Usage-Seite; danach jederzeit über die Schaltfläche „Sitzungen synchronisieren". Kein 60-Sekunden-Daemon auf dem Remote.

</details>

<details>
<summary><strong>Liest der Remote meine lokalen Dateien?</strong></summary>

Nein. Der Remote-Agent öffnet nur Sitzungsdateien und die Datenbank `~/.cc-switch/` unter dem Remote-`$HOME`; lokale Dateien des Desktops werden weder hochgeladen noch gelesen.

</details>

<details>
<summary><strong>Wo wird das Passwort gespeichert?</strong></summary>

Das Verbindungspasswort ist eine In-Memory-Eingabe nur für diese Verbindung und wird nie in `remote-targets.json` geschrieben. Unter Windows kann es optional DPAPI-verschlüsselt in `~/.cc-switch/remote-credentials.json` gespeichert werden; Nicht-Windows-Plattformen bieten das Speichern von Anmeldedaten noch nicht an.

</details>

<details>
<summary><strong>Warum gibt es keinen macOS-Build?</strong></summary>

Dieser Fork besitzt kein Apple-Developer-ID-Zertifikat und kann daher keine macOS-Apps signieren/notarisieren. Für macOS verwenden Sie bitte Upstream: <https://github.com/farion1231/cc-switch/releases>

</details>

## Dokumentation

- **Remote-Sync-Architektur** (zh): `docs/remote-sync-architecture-zh.md`
- **Design der lokalen/remoten Anfrage-Isolation** (zh): `docs/remote-route-isolation-design.md`
- Handbuch, FAQ und Changelog der Upstream-Funktionen liegen im Upstream-Repository: <https://github.com/farion1231/cc-switch>

## Entwicklung

### Voraussetzungen

- Node.js 18+, pnpm 8+
- Rust 1.85+, Tauri CLI 2.8+

### Befehle

```bash
pnpm install        # Abhängigkeiten installieren
pnpm dev            # Entwicklungsmodus (Hot Reload)
pnpm typecheck      # Frontend-Typprüfung
pnpm test:unit      # Frontend-Unit-Tests
cd src-tauri
cargo test          # Backend-Tests
pnpm tauri build    # App bauen
```

> Der Remote-Agent ist ein musl-statisch gelinktes Linux-Binary (x86_64 / aarch64), das von `src-tauri/build.rs` in das Desktop-Paket eingebettet wird; der GitHub-Actions-Release-Workflow übernimmt Cross-Compiling und Packaging.

## Lizenz

MIT © Jason Young (Upstream) — dieser Fork wird unter der [MIT License](LICENSE) vertrieben.
