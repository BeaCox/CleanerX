<p align="center">
  <img src="assets/branding/cleanerx-wordmark.png" width="900" alt="CleanerX — Clean storage. Clear mind. Better code.">
</p>

# CleanerX

CleanerX is a local-first desktop application for inspecting and safely cleaning storage created by coding agents. The current engineering MVP supports Codex, Claude Code, OpenCode, and pi data on macOS 13+ and x86_64 Linux, and is built with Rust, Tauri 2, React, and TypeScript.

CleanerX itself does not connect to cloud services, upload data, or collect telemetry. Project paths are used only to organize sessions: CleanerX never recursively scans or modifies source directories.

> [!CAUTION]
> CleanerX performs destructive operations on private local data. This repository is an engineering MVP and is not yet intended for broad production use. Review every cleanup plan before running it. Backups are optional and disabled by default, so cleanup without a backup is irreversible.

## Features

- Discovers current, archived, root, and child Codex sessions through the Codex App Server.
- Organizes sessions by project root in a tree, with a searchable flat-list alternative.
- Reports the real disk usage of session rollouts, attachments, generated media, visualizations, logs, caches, and temporary files.
- Loads transcript, memory, log, and media details only after an explicit user action, using bounded read-only requests. Content bodies are not retained in inventory snapshots.
- Deletes session trees through the official `thread/delete` operation and shows all affected descendants before confirmation.
- Uses OpenCode's official CLI while offline and a strictly verified loopback Server API for inactive-session deletion while OpenCode is running, while keeping its SQLite database read-only to CleanerX.
- Probes `memory/reset` independently, so an unsupported memory operation does not disable otherwise supported session cleanup.
- Supports optional encrypted `.cxb` backups and all-or-nothing restore without overwriting existing data.
- Protects active and pinned sessions, authentication, configuration, MCP credentials, rules, skills, plugins, browser data, cookies, and source code.
- Provides English and Chinese localization, system-aware light and dark themes, keyboard navigation, and reduced-motion support.

## Safety model

CleanerX treats deletion as a security boundary:

| Area | Guarantee |
| --- | --- |
| Source projects | Project paths are grouping metadata only and are never recursive scan or cleanup roots. |
| Session deletion | Codex uses App Server `thread/delete`; OpenCode uses its documented CLI while offline or a verified loopback Server API for inactive sessions; CleanerX never writes private session databases. |
| Capability failure | Missing or unavailable mutation methods degrade the affected operation to read-only reporting. |
| Direct file cleanup | Every path must remain under a category-specific allowlisted root. Symlinks, traversal, protected descendants, ownership anomalies, and identity changes are rejected. |
| Active writers | CleanerX never force-quits Codex or another process. It reports the blocker and lets the user retry. |
| Backups | Backups are opt-in. When selected, the encrypted archive is verified and atomically committed before mutation starts. |
| Restore | Manifest hashes and every destination are checked before the first move. Existing IDs and paths are never overwritten. |
| Privacy | There is no telemetry, crash upload, cloud synchronization, updater, background daemon, or unrestricted shell/filesystem API. |

Backups use tar + zstd and [age](https://age-encryption.org/) X25519 encryption. The private identity is stored in macOS Keychain or the Linux desktop Secret Service. Archives are first written as `.partial` files, verified, and then atomically committed as `.cxb` files. If the native credential store is unavailable, backup creation fails before cleanup begins.

For the complete threat model and vulnerability-reporting process, see [SECURITY.md](SECURITY.md).

## Project status

CleanerX is currently an engineering MVP. There is no signed or notarized public build yet. The repository builds unsigned Apple Silicon and Intel `.app`/DMG artifacts and unsigned x86_64 Linux `.deb`/AppImage artifacts. Linux packages are built and smoke-tested on Ubuntu 22.04 in GitHub Actions; Windows and further adapter hardening remain planned work.

See the [development roadmap](docs/roadmap.md) for current milestones, release gates, and deliberate non-goals.

## Requirements

- macOS 13 or later, or x86_64 Linux with a WebKitGTK 4.1 desktop environment (CI uses Ubuntu 22.04)
- A supported local Codex, Claude Code, OpenCode, or pi installation for that Agent's inventory and mutations
- [Rust](https://www.rust-lang.org/tools/install) 1.88 or later
- [Node.js](https://nodejs.org/) 22 or later (CI uses Node.js 24)
- [pnpm](https://pnpm.io/installation) 11.3.0 or later
- Xcode Command Line Tools for native macOS builds, or the Tauri system packages listed below for Linux builds

Install the Xcode Command Line Tools if needed:

```bash
xcode-select --install
```

On Debian or Ubuntu, install the Linux build prerequisites:

```bash
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev patchelf
```

Encrypted backups on Linux also require a desktop Secret Service provider such as GNOME Keyring or KDE Wallet.

## Getting started

From the repository root, install the locked frontend dependencies and start the development application:

```bash
make setup
make dev
```

To build an unsigned macOS application:

```bash
make app
open target/release/bundle/macos/CleanerX.app
```

To build both the `.app` and DMG artifacts:

```bash
make bundles
```

To build the Linux packages on an x86_64 Linux host:

```bash
make linux
```

The packages are written beneath `target/release/bundle/deb/` and `target/release/bundle/appimage/`. The Linux CI job also launches the release binary under Xvfb and uploads both packages as the `CleanerX-linux-x86_64-unsigned` workflow artifact.

Builds are unsigned. On first launch, macOS may block the application. Use Finder to right-click CleanerX and choose **Open**, or approve it in **System Settings → Privacy & Security**. Do not bypass Gatekeeper for binaries from an untrusted source.

## Usage

1. Launch CleanerX, select the target Agent, and scan its detected installation.
2. Inspect the overview, session tree, media, memory, logs, caches, and temporary data.
3. Select individual cleanable items or use scoped bulk selection. Nothing is selected by default.
4. Review the cleanup plan, including expanded session descendants and any blocked items.
5. Choose whether to create an encrypted backup. This option is off by default.
6. Confirm the operation. CleanerX performs the cleanup and rescans to verify the result.

You can set custom absolute data roots in Settings. Codex resolves `CODEX_HOME` then `~/.codex`; Claude Code resolves `CLAUDE_CONFIG_DIR` then `~/.claude`; OpenCode resolves `XDG_DATA_HOME/opencode` then `~/.local/share/opencode`.

## Read-only mode

CleanerX first tries the active Codex control socket. If the socket is stale or unresponsive, it falls back to an isolated stdio App Server. If neither transport provides the required official capability, the affected session operation remains read-only.

If CleanerX reports read-only mode:

1. Use **Retry connection** and read the specific reason shown in the application.
2. Confirm that a Codex CLI or supported desktop application is installed. In a terminal, `codex --version` and `codex app-server --help` should succeed when using the CLI.
3. If your data lives in a custom location, set an absolute `CODEX_HOME` in Settings and scan again.
4. Restart CleanerX after upgrading Codex.

CleanerX will not bypass a missing official deletion capability by writing to private SQLite databases.

## Architecture

| Path | Responsibility |
| --- | --- |
| `crates/cleanerx-core` | Domain types, cleanup planning, path validation, backup and restore, hashing, and transaction invariants. |
| `crates/adapter-codex` | Codex discovery, capability probing, App Server transport, storage classification, and read-only compatibility fallbacks. |
| `crates/adapter-claude` | Claude Code discovery, documented local-storage classification, bounded previews, and guarded path cleanup. |
| `crates/adapter-opencode` | OpenCode discovery, recognized-SQLite read-only inventory, and official CLI delete/export/import routes. |
| `src-tauri` | Narrow application command boundary and cleanup transaction orchestration. |
| `src` | React and TypeScript presentation layer. |

Future agents integrate through the compile-time `AgentAdapter` trait. CleanerX does not load a dynamic cleanup-plugin ABI or expose general shell and filesystem access to the webview.

## Development

The root `Makefile` is the stable entry point for local development and CI:

| Command | Purpose |
| --- | --- |
| `make setup` | Install dependencies from the lockfile. |
| `make dev` | Run the Tauri application with hot reload. |
| `make format` | Format the Rust workspace. |
| `make check` | Run formatting checks, Clippy, Rust tests, frontend tests, and the production frontend build. |
| `make app` | Build an unsigned macOS `.app`. |
| `make dmg` | Build an unsigned macOS DMG. |
| `make bundles` | Build the `.app` and DMG in one Tauri invocation. |
| `make linux` | Build unsigned Linux `.deb` and AppImage packages. |
| `make smoke-linux` | Launch the built Linux release binary under Xvfb for a native smoke test. |

Run the complete validation pipeline before submitting a change:

```bash
make check
```

Use `TARGET=<rust-target-triple>` with the bundle commands when building for an explicit architecture.

## Documentation

- [Documentation index](docs/README.md)
- [Development roadmap](docs/roadmap.md)
- [Storage and transaction model](docs/storage-model.md)
- [Agent session hierarchy](docs/agent-session-hierarchy.md)
- [Agent memory capability and safety model](docs/memory-management.md)
- [Security policy](SECURITY.md)

CleanerX follows the public [Codex App Server protocol](https://learn.chatgpt.com/docs/app-server) for session operations. Runtime capabilities are negotiated because Codex evolves independently of CleanerX.

## Contributing

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) and the repository-wide [AGENTS.md](AGENTS.md) constraints before starting. New storage schemas, categories, and mutation paths require fixtures and negative-path tests, including proof that protected files and source trees remain unchanged.

## Security

Please report vulnerabilities through a private security advisory in the source repository. Do not include real transcripts, credentials, memory databases, operation journals, or `.cxb` archives in a public report. See [SECURITY.md](SECURITY.md) for the requested report details.

## License

Copyright © 2026 BeaCOx. Licensed under the [Apache License 2.0](LICENSE).
