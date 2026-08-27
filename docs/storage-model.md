# Storage and transaction model

## Discovery

Codex Home resolution order is user override, `CODEX_HOME`, then `~/.codex`. The executable is resolved from `PATH`, known macOS application resources, and common Homebrew/NVM/Volta/asdf/Bun/pnpm user locations so Finder launches do not depend on an interactive shell PATH. Application Support is a separate optional allowlisted root.

CleanerX initializes an App Server JSON-RPC connection and opts into capability discovery. It prefers the existing Unix control socket through `codex app-server proxy`; a missing, stale, or unresponsive socket falls back to a temporary stdio server. Failure of both transports disables session mutations. It calls `thread/list` for active and archived pages and `thread/loaded/list` for runtime blockers. No message preview or turn body is retained in the inventory model. After an explicit detail action, `thread/read` with `includeTurns: true` loads a bounded read-only preview without resuming the thread; a recognized rollout parser is the read-only fallback.

When App Server listing fails, CleanerX may query a recognized state database read-only. Required columns are checked before queries. This fallback always disables mutations.

## Categories

| Category | Initial selection | Backup | Mutation route |
| --- | --- | --- | --- |
| Current/archived session | Off | Optional | App Server `thread/delete` |
| Global memory | Off | Optional | App Server `memory/reset` |
| Attachment/generated content | Off | Optional | Allowlisted path removal |
| Logs older than retention | Off | No | Validated SQLite transaction |
| Regenerable cache | Off | No | Allowlisted path removal |
| Temporary data older than retention | Off | No | Allowlisted path removal |
| Auth/config/rules/skills/plugins | Never | N/A | Protected |

Projects are associations derived from a non-empty absolute session `cwd`, known project roots and ancestor `.git` markers. A child with no recorded `cwd` may inherit an already resolved parent association; CleanerX never resolves an empty or relative `cwd` against its own process directory. Sessions that still have no association remain outside `projects` and are displayed under the UI-only “No project” virtual root. Project cleanup selects linked Agent records only. It never makes a project directory an allowed mutation root.

Content preview is separate from inventory. The Tauri boundary accepts only an item ID from the current snapshot, resolves its known paths on the Rust side, rejects symlinks and paths outside Codex Home/Application Support, and applies block/byte limits. Recognized memory and log schemas are queried read-only; unknown SQLite schemas and unsupported binaries produce an explicit unavailable preview instead of a guessed query or raw dump. The media gallery uses a separate thumbnail command that reads at most the first supported image (PNG, JPEG, GIF, or WebP) up to 5 MB from a visible attachment/generated item; it cannot return text or enumerate content into the frontend. Protected authentication and configuration content is never opened.

Memory semantics and future entry-editing routes are specified in [Agent memory research and implementation plan](memory-management.md). Instructions and rules remain protected even when an Agent presents them alongside memory in its own UI.

## Transaction states

```text
planned ──┬────────────────────→ deleting → verified → complete
          └→ backupWritten ────↗
any in-progress state ────────────────────────────→ failed
```

Each transition is written to an atomic JSON operation journal. Backup is an explicit option and defaults off; the review warns that direct cleanup is irreversible. When backup is selected, eligible items are collected relative to a named fixed root, hashed with SHA-256, archived as tar + zstd, encrypted with age X25519, written to `.partial`, finalized, and entered in the backup catalog before mutation starts. With backup disabled, the journal proceeds directly from `planned` to `deleting`; path, capability, writer, and post-operation verification guards are unchanged.

After cleanup, CleanerX scans again. A session still returned by Codex is a failed verification; CleanerX does not try to repair the private database by writing it directly.

## Restore

An archive is decrypted into a private staging directory. Every entry must be a regular relative path and every SHA-256 must match the manifest. All destination paths are preflighted before the first move. Any existing destination rejects the entire restore. A post-restore App Server scan lets Codex rebuild recognized indexes through its supported scan-and-repair path.

## Permanent backup deletion

Backup deletion is an explicit, separately confirmed action. CleanerX accepts only a backup ID from the current catalog and derives the sole valid archive path as `<backup-directory>/<id>.cxb`; altered paths, symlinks, and non-file entries are rejected. The encrypted archive is removed before its catalog entry. If an earlier interruption already removed the archive, retrying the action removes the stale catalog entry without touching any other path.
