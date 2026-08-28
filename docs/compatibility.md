# Mutation compatibility matrix

This table records the mutation routes CleanerX currently enables and the evidence used to qualify them. Runtime capability and storage recognition are authoritative; an Agent version string is informational and never enables a write route by itself.

All enabled routes share operation journal format v2, pre-mutation capability/writer checks, immutable cleanup plans, post-operation Agent rescans, and startup recovery. A selected backup is offered only when CleanerX has a supported restore route for every item in the expanded plan and the adapter can rediscover all of them afterward. Mixed restorable/irreversible selections must be cleaned separately to use backup.

## Current routes

| Agent | Data | Mutation route | Runtime gate | Backup/restore | Automated evidence |
| --- | --- | --- | --- | --- | --- |
| Codex | Sessions | App Server `thread/delete` for the minimal selected root set | Initialized App Server plus an independent invalid-parameter method probe; pinned and active/loaded sessions are blocked | Unavailable: the documented App Server has no session import route | Descendant/minimal-root planning, independent method-probe classification, pinned/active/loaded blocking, post-success media cleanup, and rescan verification |
| Codex | Global memory | Runtime-probed App Server `memory/reset` | Probed independently from session listing/deletion; Codex must be stopped | Unavailable: no supported memory import route | Independent capability downgrade, exit requirement, journal/rescan recovery behavior, and recognized read-only memory fixtures |
| Codex | Diagnostic logs | Transactional deletion from a recognized `logs(ts, estimated_bytes, …)` SQLite schema, followed by WAL checkpoint and compaction | Codex must be stopped; required columns must match | Not offered | Recognized WAL-schema retention test and unknown-schema no-mutation test |
| Codex | Cache and temporary data | Fixed-root guarded removal | Codex must be stopped where the scanned item says a writer can recreate it | Not offered; data is regenerable | Path policy, redirect/identity checks, and post-operation rescan |
| Codex | Attachments/generated media | Removed only as an owning-session artifact after successful `thread/delete` | Owning session must be selected and successfully deleted | Not independently offered | Planner rejects standalone media mutation; orphan cards remain inspect-only; artifact cleanup preserves the rollout and source tree |
| Claude Code | Sessions | Snapshot-owned documented application-data paths | Recognized storage and no Claude writer | Encrypted file backup/restore | Metadata-only fixtures, stale/active writer coverage, redirect rejection, source-revision checks, protected/source byte checks, and rescan |
| Claude Code | Project auto memory | Selected recognized project `memory/` directory | No Claude writer and unchanged source revision | Encrypted file backup/restore | Recognized project-memory fixtures, bounded detail reads, source-revision checks, and protected instruction separation |
| Claude Code | History/cache/temporary data | Recognized fixed-root path removal | No Claude writer and unchanged source revision | Offered only for recoverable history data | Fixed-root classification, redirect checks, and direct-file transaction tests |
| OpenCode | Sessions, offline | Official `opencode session delete` with `OPENCODE_DB` | Recognized official schema, matching database revision, and no writer | Official `export` / `import` inside encrypted backup staging | Exact CLI argument test, command-failure test, descendant planning, revision-change rejection, import preflight, and compensation tests |
| OpenCode | Sessions, online | Verified loopback Server `DELETE /session/:id` | Every related writer must expose the same verified loopback server and the full scope must be idle | Unavailable while OpenCode is running | Idle/busy/retrying, unverified writer, endpoint failure, and isolated server deletion tests |
| OpenCode | Logs/cache | Recognized log directory or fixed XDG cache root removal | OpenCode must be stopped | No session-style import; cache is regenerable | Fixed-root and symlink fixtures; protected SQLite/config/source paths remain excluded |
| pi | Sessions | Documented per-session JSONL file removal | Recognized session header, unchanged source revision, and no pi writer | Encrypted file backup/restore | Metadata-only session fixtures, writer blocking, symlink rejection, fork-without-cascade behavior, guarded deletion, and protected/source byte checks |
| pi | Provider catalog cache | Guarded removal of `models-store.json` | No pi writer and unchanged source revision | Not offered; data is regenerable | Fixed-file classification, path policy, and protected data checks |

## Fail-closed behavior

- An unrecognized or ambiguous database schema may supplement read-only inventory only; it never enables private database mutation.
- Missing methods, failed transports, active writers, changed source revisions or identities, redirects, ownership anomalies, and paths outside fixed roots block only the affected route.
- A recognized incomplete v2 journal blocks new cleanup only for its owning Agent until it is reconciled or safely closed; browsing and other Agents remain available. Strictly recognized pre-v2 status-only journals are removed as obsolete metadata. Unknown committed journals still block mutation globally because their ownership and scope cannot be proven.
- `.partial` archives, archives that did not pass verification, and backups not bound to the journal operation and Agent are never offered for recovery.
- Automatic restore is offered only when the rescan confirms every planned destination is absent. Partial or conflicting current data is never overwritten; the recovery UI reports that state and allows a safe close instead of guessing a merge.

## Evidence maintenance

The complete repository gate is `make check`. Live tests that require an installed Agent remain isolated/ignored diagnostics and must use disposable Agent homes. Native disposable mutation/restore cycles and broader released-version observations are release-readiness evidence tracked by [M2](roadmap.md#m2--cross-platform-release-readiness), not prerequisites for changing the shared M1 journal format.

Primary interface references:

- [Codex App Server](https://learn.chatgpt.com/docs/app-server)
- [Claude Code application data](https://code.claude.com/docs/en/claude-directory)
- [OpenCode CLI](https://opencode.ai/docs/cli/) and [Server API](https://opencode.ai/docs/server/)
- [pi session storage](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/sessions.md)
