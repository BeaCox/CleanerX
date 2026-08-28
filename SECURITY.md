# Security policy

CleanerX operates on private local Agent data, so destructive behavior is treated as a security boundary rather than a convenience feature.

## Invariants

- Source directories are never recursive scan or mutation roots.
- `auth.json`, `config.toml`, MCP credentials, rules, skills, plugins, browser account data, cookies and source code are never cleanup targets.
- Backups are optional and off by default. Direct cleanup is irreversible and must be stated in the review. If the user selects a backup, deletion cannot begin until the encrypted archive is verified and atomically committed.
- Codex session deletion uses App Server `thread/delete`. OpenCode session deletion uses its documented CLI command while offline, or a verified loopback Server API only when every related writer reports the full deletion scope idle; backup and restore use the documented export/import commands and remain offline-only. pi session deletion removes only the documented per-session JSONL files through CleanerX's preflighted path transaction while no pi process is running. Unknown schemas, ambiguous databases, unverified writers, authentication challenges, and unavailable routes fail closed.
- CleanerX never writes OpenCode SQLite rows, deletes its database/WAL files, or treats OpenCode project/worktree paths as cleanup roots.
- Symbolic links, Windows reparse points and junctions, lexical traversal, paths outside allowlisted roots, nested Unix mount or Windows volume boundaries, ownership anomalies and changed file identities are rejected. On Windows, ownership is checked against the current process token's default owner SID, which is the SID Windows applies to newly created objects and may be an owner-capable group for an elevated account.
- Atomic journal, catalog, and backup replacement uses same-directory native primitives. Windows replacement is write-through, and a selected encrypted backup is reopened and fully hash-verified after commit before mutation can begin.
- Restore verifies the exact manifest/payload set and preflights fixed roots, ownership, volume/device boundaries, redirects, conflicts, and every destination. Payloads are staged as sibling files and committed with no-replace native renames; an in-process failure revalidates identities and rolls back every committed destination without touching pre-existing paths. Process-crash startup recovery remains a release gate and is not claimed as complete in the source preview.
- Backup identities stay in the native platform credential store: macOS Keychain, Linux Secret Service, or Windows Credential Manager. A missing or failing credential backend disables backup creation rather than weakening encryption.
- CleanerX does not force-quit Codex or another writer.
- Inventory may retain one normalized, 96-character first-user-message excerpt only as an unnamed Pi session title, matching Pi's own selector; no additional transcript content is retained by the scan.
- There is no telemetry, crash upload, cloud connection, updater, background daemon or general shell/filesystem command exposed to the GUI.

## Reporting a vulnerability

Please open a private security advisory in the source repository. Include the CleanerX version, platform, Codex version, affected data class, reproduction steps and whether any protected file was changed. Do not attach real transcripts, credentials or memory databases.

Until a report is resolved, preserve the operation journal and `.cxb` archive but do not publish them: both may contain sensitive paths or Agent data.
