# Security policy

CleanerX operates on private local Agent data, so destructive behavior is treated as a security boundary rather than a convenience feature.

## Invariants

- Source directories are never recursive scan or mutation roots.
- `auth.json`, `config.toml`, MCP credentials, rules, skills, plugins, browser account data, cookies and source code are never cleanup targets.
- Important data is not deleted until an encrypted backup is complete and atomically committed.
- Session deletion uses Codex App Server `thread/delete`; unknown versions and unavailable methods are report-only.
- Symbolic links, lexical traversal, paths outside allowlisted roots and changed file identities are rejected.
- CleanerX does not force-quit Codex or another writer.
- There is no telemetry, crash upload, cloud connection, updater, background daemon or general shell/filesystem command exposed to the GUI.

## Reporting a vulnerability

Please open a private security advisory in the source repository. Include the CleanerX version, platform, Codex version, affected data class, reproduction steps and whether any protected file was changed. Do not attach real transcripts, credentials or memory databases.

Until a report is resolved, preserve the operation journal and `.cxb` archive but do not publish them: both may contain sensitive paths or Agent data.

