<div align="center">

# CC Switch — Remote Server Edition

### An independent fork of [farion1231/cc-switch](https://github.com/farion1231/cc-switch)

[![Version](https://img.shields.io/github/v/release/xh20220630/cc-switch-agent?color=blue&label=version)](https://github.com/xh20220630/cc-switch-agent/releases)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux-lightgrey.svg)](https://github.com/xh20220630/cc-switch-agent/releases)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-orange.svg)](https://tauri.app/)

English | [中文](README_ZH.md) | [日本語](README_JA.md) | [Deutsch](README_DE.md)

</div>

---

## What Is This

This repository is an independent fork of [CC Switch](https://github.com/farion1231/cc-switch) (MIT). Upstream is a Tauri 2 desktop app that manages multi-provider API configurations for Claude Code, Claude Desktop, Codex, Gemini CLI, Grok Build, OpenCode, OpenClaw and Hermes in one place — one-click switching, MCP/Prompts/Skills management, proxy & failover, usage tracking, cloud sync, and more.

**For upstream's full feature set and documentation, see the upstream repository directly:**

- Project home & docs: <https://github.com/farion1231/cc-switch>
- User manual: <https://github.com/farion1231/cc-switch/tree/main/docs/user-manual>
- Changelog: <https://github.com/farion1231/cc-switch/blob/main/CHANGELOG.md>

This fork **keeps all upstream capabilities and adds a "Remote Server Mode"**: connect to a remote Linux server over SSH, run an ephemeral Agent on the remote side, read/write the remote `~/.cc-switch` database directly, import and display session usage from Claude/Gemini/OpenCode/Grok/Codex/Kimi running on the remote machine, and sync providers between local and remote.

---

## What This Fork Adds

### Remote Server Mode (Remote Runtime)

Many users run Claude Code / Codex / Gemini on a **remote Linux server** (cloud hosts, containers, dev boxes) while the desktop app only manages local config. This fork lets the desktop connect straight to the remote side and "ship the compute core" over:

- **SSH target management** — auto-discover targets from `~/.ssh/config` or add manually; connection test, edit, delete
- **Ephemeral Agent, in and out** — the desktop bundles a musl statically-linked Linux Agent (x86_64 / aarch64). On connect it's scp'd to `/tmp`, run with `--stdio`, and self-deletes on exit (trap cleanup + desktop-side fallback)
- **Remote DB read/write directly** — the Agent opens the remote user's own `~/.cc-switch/cc-switch.db`, behaving exactly like local (the same Core importer) — not a "pulled copy"
- **Remote usage sync** — syncs automatically once on successful connect, once more when entering the Usage page, plus manual sync anytime; imports session logs from all six sources (Claude / Gemini / OpenCode / Grok / Codex / Kimi) into the remote database
- **Provider sync** — sync providers between local and remote via a dialog with search, dedup-skip and retry
- **Runtime switcher** — switch between "local / remote target" in the top bar, with live status (connecting / online / offline / reconnecting / incompatible)

```
┌─ Desktop (Windows / Linux) ─┐      SSH      ┌─ Remote server (Linux) ────────┐
│ Tauri frontend UI           │   scp/ssh   │ Ephemeral Agent (cc-switch-agent)│
│  appInvoke ─► remote_invoke │ ──────────► │  │ opens remote ~/.cc-switch/    │
│  RemoteRuntimeState         │ ◄────────── │  │  cc-switch.db                 │
│  embedded Agent bytes       │             │  │ session logs: ~/.claude ...   │
└─────────────────────────────┘             │  │ ~/.gemini ~/.grok ~/.codex    │
                                            └──────────────────────────────────┘
```

### Security Design

| Risk | Countermeasure |
|---|---|
| Command injection | Remote commands are assembled from controlled templates (hex paths / decimal lengths / SHA-256); user input never reaches the shell; SSH args pass as an `OsString` array, never through a local shell |
| Escalated commands | Central capability allowlist (`cc-switch-protocol`, 27 commands), echoed back at handshake, enforced by `require()` on the desktop; unregistered local commands are refused remotely |
| Cross-talk (stale responses after target switch) | Double-checked generation token + frontend query-cache scope isolation; late responses during a switch are rejected |
| Credential leak | Password is an in-memory input for the current connection only, never written to `remote-targets.json`; optional DPAPI (CryptProtectData) encrypted storage on Windows, SSH_ASKPASS for TTY-less auth |
| Host key | First connect shows the fingerprint via `ssh-keyscan`; `known_hosts` is written only after user confirmation (XShell-style) |
| Remote residue | Agent self-deletes via trap + desktop SSH cleanup on drop |
| stdout log pollution | Agent writes diagnostics to stderr only; protocol frames carry a magic marker to detect stream mix |
| Timeout hangs | Per-command allowlist timeouts (30s–300s); Cancel sent on timeout (with requestId + operationId to avoid mis-cancel) |

### Remote Scope

- **Remote server**: Linux only (`uname -s` preflight), x86_64 or aarch64
- **Desktop**: Windows (x86_64 / ARM64), Linux (x86_64 / ARM64)
- Remote data read: session files under the remote `$HOME` — `~/.claude`, `~/.gemini`, `~/.local/share/opencode`, `~/.grok`, `~/.codex`, `~/.kimi-code` — and the `~/.cc-switch/` database; desktop local files are never uploaded or read

### Differences vs Upstream

| Item | Upstream | This fork |
|---|---|---|
| Remote server mode | No | ✅ Added (this repo's core value) |
| Provider sync | No | ✅ Added |
| Build platforms | Windows / macOS / Linux | Windows x64/ARM64 + Linux x64/ARM64 |
| macOS signing/notarization | Apple Developer ID + notarization | None (fork has no Apple cert, no macOS build) |
| Auto-update signing/endpoints | Upstream keys & repo | Fork's own signing key and this repo |
| Version | 3.x following upstream | 0.0.1 independent release |

---

## Install & Download

Get the latest release from [Releases](https://github.com/xh20220630/cc-switch-agent/releases):

| Platform | Files |
|---|---|
| Windows (x86_64) | `CC-Switch-{version}-Windows.msi` (installer) / `Windows-Portable.zip` (portable) |
| Windows (ARM64) | `CC-Switch-{version}-Windows-arm64.msi` / `Windows-arm64-Portable.zip` |
| Linux (x86_64) | `CC-Switch-{version}-Linux-x86_64.AppImage` / `.deb` / `.rpm` |
| Linux (ARM64) | `CC-Switch-{version}-Linux-arm64.AppImage` / `.deb` / `.rpm` |

> This fork does not ship macOS builds (no Apple signing certificate). macOS users should use the upstream release.
> `.tar.gz` is reserved for the Tauri updater; no manual download needed.

## Quick Start: Connect to a Remote Server

1. **Prepare the remote** — a Linux server (x86_64 / aarch64), SSH enabled, with access to the remote user's `~/.cc-switch` and session directories
2. **Add a target** — Settings → Remote Targets; auto-scan `~/.ssh/config` or fill in host / user / port manually
3. **Connect** — first connect shows the server public-key fingerprint via `ssh-keyscan`; `known_hosts` is written only after confirmation; the password is used for this connection only (optional encrypted save on Windows)
4. **Switch runtime** — pick the target in the top-bar runtime switcher; remote usage syncs once on connect, once more on entering the Usage page, and anytime via "Sync Sessions"
5. **Provider sync** — push local providers to the remote (or pull from it) in the Provider Sync dialog, with search and dedup

> No 60-second daemon on the remote: the Agent is an ephemeral process born with the connection, so sync happens at "connect + page enter + manual".

## FAQ

<details>
<summary><strong>When does remote usage data appear after connecting?</strong></summary>

Backend syncs once the moment the connection succeeds, then once more when entering the Usage page; afterwards use the "Sync Sessions" button anytime. No 60-second daemon on the remote.

</details>

<details>
<summary><strong>Will the remote read my local files?</strong></summary>

No. The remote Agent only opens session files and the `~/.cc-switch/` database under the remote `$HOME`; desktop local files are never uploaded or read.

</details>

<details>
<summary><strong>Where is the password stored?</strong></summary>

The connection password is an in-memory input for that connection only, never written to `remote-targets.json`. On Windows it may optionally be DPAPI-encrypted to `~/.cc-switch/remote-credentials.json`; non-Windows platforms do not offer credential saving yet.

</details>

<details>
<summary><strong>Why is there no macOS build?</strong></summary>

This fork has no Apple Developer ID certificate, so it cannot sign/notarize macOS apps. For macOS, use upstream: <https://github.com/farion1231/cc-switch/releases>

</details>

## Documentation

- **Remote sync architecture** (zh): `docs/remote-sync-architecture-zh.md`
- **Local/remote request isolation design** (zh): `docs/remote-route-isolation-design.md`
- Upstream features' manual, FAQ and changelog live in the upstream repo: <https://github.com/farion1231/cc-switch>

## Development

### Requirements

- Node.js 18+, pnpm 8+
- Rust 1.85+, Tauri CLI 2.8+

### Commands

```bash
pnpm install        # install dependencies
pnpm dev            # dev mode (hot reload)
pnpm typecheck      # frontend type check
pnpm test:unit      # frontend unit tests
cd src-tauri
cargo test          # backend tests
pnpm tauri build    # build app
```

> The remote Agent is a musl statically-linked Linux binary (x86_64 / aarch64) embedded into the desktop bundle by `src-tauri/build.rs`; the GitHub Actions release workflow cross-compiles and packages it.

## License

MIT © Jason Young (upstream) — this fork is distributed under the [MIT License](LICENSE).
