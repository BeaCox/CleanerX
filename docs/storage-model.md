# Storage and transaction model

## Discovery

CleanerX persists one active target Agent and exposes the selector in the left side of the bottom status deck. Switching targets atomically saves the preference, clears the old selection/detail state, and creates a fresh Agent-specific snapshot. A cleanup plan is bound to that snapshot, and execution dispatches from the snapshot's `AgentKind`; paths and backups from different Agents cannot be mixed.

The complete metadata-only inventory snapshot remains behind the Rust boundary. The initial scan response contains non-session cleanup items, aggregate session counts, source values, and per-project count/size summaries, but no session records or session cleanup items. It also carries a compact selection index containing only each selectable session item's ID, thread ID, project association, and size; this lets current-scope and project selection include unloaded rows without transferring their display metadata. The Sessions view requests pages of at most 100 direct matches only for an expanded visible project or the visible flat-list range. Tree pages may additionally include the already-inventoried ancestors required to preserve hierarchy, while separately identifying the actual filtered matches so context rows remain non-selectable. Automatic pages are prefetched ahead of the viewport without inserting visible loading rows; only a failed page adds an inline retry row. Every page and project-summary request is bound to the current snapshot ID and validated filters; cleanup planning still resolves selected IDs against the complete backend snapshot.

### Codex

Codex Home resolution order is user override, `CODEX_HOME`, then `~/.codex`. The executable is resolved from `PATH`, known macOS application resources, and common Homebrew/NVM/Volta/asdf/Bun/pnpm user locations so Finder launches do not depend on an interactive shell PATH. Application Support is a separate optional allowlisted root.

CleanerX initializes an App Server JSON-RPC connection and opts into capability discovery. It prefers the existing Unix control socket through `codex app-server proxy`; a missing, stale, or unresponsive socket falls back to a temporary stdio server. Failure of both transports disables session mutations. It calls `thread/list` for active and archived pages and `thread/loaded/list` for runtime blockers. No message preview or turn body is retained in the inventory model. After an explicit detail action, `thread/read` with `includeTurns: true` loads a bounded read-only preview without resuming the thread; a recognized rollout parser is the read-only fallback.

A session display title is resolved from read-only metadata in this exact order: non-empty `threads.name`, non-empty `session_index.thread_name`, non-empty `threads.title`, non-empty `threads.first_user_message`, then `未命名会话`. `session_index.jsonl` and recognized state-database columns may supplement App Server results without enabling private database writes. Title resolution trims blank metadata, never uses the final component of `cwd`, and retains only the chosen display string rather than an additional message body.

When App Server listing fails, CleanerX may query a recognized state database read-only. Required columns are checked before queries. This fallback always disables mutations.

### Claude Code

Claude Code Home resolution order is user override, `CLAUDE_CONFIG_DIR`, then `~/.claude`. CleanerX recognizes only the application-data layout documented by Claude Code: UUID-named transcripts directly beneath `projects/<project>/`, the matching session directory and per-session `tasks`, `file-history`, `image-cache`, `uploads`, `session-env`, and `debug` entries, project `memory/`, and the documented cache/temporary roots. It never scans a source directory; transcript `cwd` values are grouping metadata only.

Inventory reads transcript top-level metadata with a 2 MiB bound and retains only the session ID, title, absolute working directory, entrypoint, and timestamps. Unknown JSON fields, including message/tool bodies, are skipped by the typed metadata decoder and never enter the snapshot. An explicit detail action may read a bounded transcript or project-memory Markdown preview. UUID/schema mismatches, symbolic links, foreign ownership, and unknown paths are skipped or made read-only.

Claude Code currently exposes project-wide `claude project purge`, but not a per-session deletion command. Its official application-data documentation also explicitly permits deleting the listed local paths by hand. CleanerX therefore performs per-session and per-project-memory deletion through its existing fixed-root file transaction: capture a metadata-only source revision, reject active writers, optionally commit and verify an encrypted backup, revalidate the revision and filesystem identity, remove only the snapshot-owned paths, then rescan. If any Claude Code process or running-session marker is present, every writable Claude item is blocked. CleanerX never invokes `project purge` because its project-wide scope would silently include unselected history and configuration state.

Protected Claude Code data includes `~/.claude.json` (outside the cleanup root), credentials, settings, `CLAUDE.md`, keybindings, rules, commands, agents, skills, plugins, themes, hooks, policy caches, and configuration backups. Project `.claude/` directories and source trees are never visited.

### OpenCode

OpenCode data resolution uses the user override, then `XDG_DATA_HOME/opencode`, then `~/.local/share/opencode`. The executable is resolved from `PATH` and the same bounded set of common user package-manager locations used by the other CLI adapters. The regenerable cache root is resolved separately from `XDG_CACHE_HOME/opencode` or `~/.cache/opencode`; it never expands the data-directory allowlist into a project directory.

CleanerX recognizes the official `project` and `session` SQLite columns before reading metadata from `opencode.db` or one unambiguous channel database. Session title, directory, parent ID, project ID, archive time, and timestamps are read without loading transcript bodies. Logical per-session size is estimated from recognized session-owned projection and event rows; official deletion can make database pages reusable without immediately shrinking the SQLite file, and CleanerX never runs a private `VACUUM` or edits SQLite rows. Unknown or ambiguous schemas stay read-only while logs, cache metadata, and protected paths remain inventoryable.

OpenCode session deletion uses the documented `opencode session delete <sessionID>` command with the recognized database selected through the official `OPENCODE_DB` override. CleanerX requires every OpenCode process to exit, rechecks the database metadata revision, expands descendants in the review, and calls the official command only for the minimal root set. It never calls the documented HTTP endpoint on an undiscovered server, writes SQL, or deletes the database file.

When backup is selected, CleanerX runs the documented `opencode export <sessionID>` route for the full expanded set into a private temporary directory. The verified JSON exports are encrypted into the `.cxb` archive before deletion and the plaintext staging directory is removed. Restore extracts exports only into private staging, rejects duplicate or existing session IDs and unavailable original working directories before import, then uses `opencode import <file>` from the recorded directory. A failed multi-session import compensates by deleting sessions already imported through the same official CLI route. OpenCode does not expose a supported automatic-memory capability, so CleanerX shows no OpenCode memory cleanup item.

Protected OpenCode data includes authentication, configuration, plugins, skills, rules, commands, agents, source-managed worktrees/repositories, legacy or unmigrated JSON storage, plans, and the SQLite database/WAL files themselves. These may be shown as protected metadata but are never opened or selected. An explicit session detail action may query bounded message/part projections read-only; an explicit log detail action may read bounded lines and warns that logs can contain prompts, paths, and tool output.

## Categories

| Category | Initial selection | Backup | Codex route | Claude Code route | OpenCode route |
| --- | --- | --- | --- | --- | --- |
| Current/archived session | Off | Optional | App Server `thread/delete` | Snapshot-owned session paths beneath Claude Code Home | Official `session delete`; backup/restore through `export` / `import` |
| Automatic memory | Off | Optional where supported | App Server `memory/reset` (global) | Selected project `memory/` directory | Unsupported; no item shown |
| Attachment/generated content | Off | Optional | Allowlisted path removal | Session-owned documented attachment/cache paths only | Not independently targeted |
| Logs/history | Off | Agent-specific | Validated SQLite transaction | Recognized `history.jsonl` path removal | Recognized log-directory path removal while offline |
| Regenerable cache | Off | No | Allowlisted path removal | Documented cache roots beneath Claude Code Home | Fixed XDG cache root only |
| Temporary data | Off | No | Allowlisted path removal | Documented temporary roots beneath Claude Code Home | Not currently targeted |
| Auth/config/rules/skills/plugins/source-managed data | Never | N/A | Protected | Protected | Protected |

Title, working directory, and project association are three independent fields. Projects require positive association evidence: a non-empty absolute session `cwd` must fall beneath a root in Codex's recognized project registry or beneath an ancestor with a `.git` marker. An arbitrary absolute working directory is recognition metadata, not a project root; in particular, a standalone desktop chat workspace such as `Documents/Codex/<date>/<name>` stays unassigned unless it independently satisfies one of those checks. A child with no recorded `cwd` may inherit an already resolved parent association; CleanerX never resolves an empty or relative `cwd` against its own process directory. Sessions that still have no association remain outside `projects` and are displayed under the UI-only “No project” virtual root. If the project registry schema is unavailable or unrecognized, CleanerX keeps the inventory available and falls back only to verifiable Git roots. Project cleanup selects linked Agent records only. It never makes a project directory an allowed mutation root.

Content preview is separate from inventory. The Tauri boundary accepts only an item ID from the current snapshot, selects the adapter from that snapshot, resolves its known paths on the Rust side, rejects symlinks and paths outside the selected Agent's fixed roots, and applies block/byte limits. Recognized memory and log schemas are queried read-only; unknown SQLite/JSONL schemas and unsupported binaries produce an explicit unavailable preview instead of a guessed query or raw dump. The Codex media gallery uses a separate thumbnail command that reads at most the first supported image (PNG, JPEG, GIF, or WebP) up to 5 MB from a visible attachment/generated item; it cannot return text or enumerate content into the frontend. Protected authentication and configuration content is never opened.

Memory semantics and entry-editing safety requirements are specified in the [Agent memory capability and safety model](memory-management.md). Unfinished implementation work is tracked only in the [development roadmap](roadmap.md). Instructions and rules remain protected even when an Agent presents them alongside memory in its own UI.

## Transaction states

```text
planned ──┬────────────────────→ deleting → verified → complete
          └→ backupWritten ────↗
any in-progress state ────────────────────────────→ failed
```

Each transition is written to an atomic JSON operation journal. Backup is an explicit option and defaults off; the review warns that direct cleanup is irreversible. When backup is selected, eligible items are collected relative to a named fixed root, hashed with SHA-256, archived as tar + zstd, encrypted with age X25519, written to `.partial`, finalized, and entered in the backup catalog before mutation starts. With backup disabled, the journal proceeds directly from `planned` to `deleting`; path, capability, writer, and post-operation verification guards are unchanged.

After cleanup, CleanerX scans the same Agent again. A selected session that remains is a failed verification; CleanerX does not try to repair a private database or broaden the deletion set.

## Restore

An archive records its `AgentKind` and is decrypted into a private staging directory. Every entry must be a regular relative path and every SHA-256 must match the manifest. The catalog Agent, manifest Agent, and restore adapter must match. All destination paths are preflighted before the first move. Any existing destination rejects the entire restore. OpenCode session exports use private import staging and official ID/directory preflight instead of becoming permanent filesystem destinations. A post-restore scan verifies that the selected Agent can rediscover the restored paths or imported sessions.

Official Claude Code references: [application data and project purge](https://code.claude.com/docs/en/claude-directory), [sessions and transcript location](https://code.claude.com/docs/en/sessions), and [project auto memory](https://code.claude.com/docs/en/memory).

Official OpenCode references: [storage locations](https://opencode.ai/docs/troubleshooting/), [CLI session/delete/export/import commands](https://opencode.ai/docs/cli/), [server session APIs](https://opencode.ai/docs/server/), and the [official session schema](https://github.com/anomalyco/opencode/blob/dev/packages/core/src/session/sql.ts).

## Permanent backup deletion

Backup deletion is an explicit, separately confirmed action. CleanerX accepts only a backup ID from the current catalog and derives the sole valid archive path as `<backup-directory>/<id>.cxb`; altered paths, symlinks, and non-file entries are rejected. The encrypted archive is removed before its catalog entry. If an earlier interruption already removed the archive, retrying the action removes the stale catalog entry without touching any other path.
