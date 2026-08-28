# Security policy

CleanerX operates on private local Agent data and performs irreversible deletion, so destructive behavior is treated as a security boundary rather than a convenience feature.

## Supported versions

CleanerX is currently an engineering source preview. There is no promoted supported binary release yet. Report issues against the latest default-branch commit when possible and include the exact commit or tag you tested. The [open-source release policy](docs/open-source-release-plan.md) defines when a tagged build becomes an alpha, beta, or supported release.

## What to report privately

Use a private security report for behavior that could cross a trust or cleanup boundary, including:

- mutation of source code, project trees, credentials, configuration, MCP credentials, rules, skills, plugins, browser accounts, cookies, or another protected category;
- path traversal, link or redirect following, mount/volume escape, ownership bypass, or file-identity race;
- deletion of an unselected item, another Agent's data, or a descendant that was not disclosed in the review plan;
- unrestricted shell or filesystem access from the webview, arbitrary command execution, or unsafe Agent transport use;
- backup plaintext exposure, credential-store bypass, archive substitution, hash-verification failure, overwrite during restore, or non-atomic partial restore;
- a journal or recovery state that permits mutation to continue without reconciling an incomplete operation; or
- inventory or preview behavior that exposes private content outside an explicit, bounded detail action.

Ordinary UI defects, sanitized compatibility failures, expected read-only degradation, and documentation errors can use the public issue templates unless they reveal private data or one of the boundaries above.

## Reporting a vulnerability

Open a [private GitHub security advisory](https://github.com/BeaCox/CleanerX/security/advisories/new). Include only the minimum sanitized information needed to reproduce the problem:

- CleanerX version or commit and operating system;
- Agent name and version;
- affected data class and mutation or preview route;
- isolated reproduction steps using a temporary Agent home;
- expected and observed behavior; and
- whether any protected or unselected file changed.

Do not attach real transcripts, credentials, memory contents, logs, state databases, operation journals, `.cxb` archives, screenshots with private paths, or a copy of your real Agent home. Preserve relevant journals and backups locally until the report is resolved; they may contain sensitive paths or Agent data even when encrypted.

The maintainer will validate the report privately, bound the affected route, and disable mutation visibly when a regression cannot be safely contained. A fix is not considered complete until it has a regression fixture, negative-path coverage, protected-byte checks, and the relevant transaction or recovery evidence.

## Safe research

- Use an isolated temporary Agent home and synthetic content. Live tests must never point at a real user profile.
- Begin with inventory and planning. Do not run mutation until the expected plan and protected fixtures have been recorded.
- Do not test against accounts, machines, or data you do not own or have permission to use.
- Stop after proving the boundary failure; do not expand access, publish private artifacts, or use the issue to reach unrelated files.
- Keep vulnerability details private until a fix and disclosure plan are agreed.

## Product safety invariants

- Source directories are never recursive scan or mutation roots.
- `auth.json`, `config.toml`, MCP credentials, rules, skills, plugins, browser account data, cookies, and source code are never cleanup targets.
- Backups are optional and off by default. Direct cleanup is irreversible and must be stated in the review. If the user selects a backup, deletion cannot begin until the encrypted archive is verified and atomically committed.
- Codex session deletion uses App Server `thread/delete`. OpenCode session deletion uses its documented CLI command while offline, or a verified loopback Server API only when every related writer reports the full deletion scope idle; backup and restore use the documented export/import commands and remain offline-only. pi session deletion removes only documented per-session JSONL files through CleanerX's preflighted path transaction while no pi process is running. Unknown schemas, ambiguous databases, unverified writers, authentication challenges, and unavailable routes fail closed.
- CleanerX never writes OpenCode SQLite rows, deletes its database/WAL files, or treats OpenCode project/worktree paths as cleanup roots. OpenCode pre-mutation revisions cover the durable database and WAL metadata but exclude SQLite's non-durable shared-memory reader-lock file, so CleanerX's own read-only scan cannot invalidate its snapshot.
- Symbolic links, Windows reparse points and junctions, lexical traversal, paths outside allowlisted roots, nested Unix mount or Windows volume boundaries, ownership anomalies, and changed file identities are rejected. On Windows, ownership is checked against the current process token's default owner SID, which is the SID Windows applies to newly created objects and may be an owner-capable group for an elevated account.
- Atomic journal, catalog, and backup replacement uses same-directory native primitives. Windows replacement is write-through, and a selected encrypted backup is reopened and fully hash-verified after commit before mutation can begin.
- Operation journal format v2 durably records the immutable plan, selected Agent and snapshot, expanded scope, backup phases, and per-mutation progress. A recognized non-terminal journal blocks mutation only for its owning Agent; an unknown committed journal blocks mutation globally. Recovery prompts may be dismissed for read-only browsing, but mutation commands enforce the block again. Strictly recognized pre-v2 status-only journals are removed as obsolete metadata. Recovery first rescans the owning Agent and never infers success from journal state alone.
- Restore verifies the exact manifest/payload set and preflights fixed roots, ownership, volume/device boundaries, redirects, conflicts, and every destination. Payloads are staged as sibling files and committed with no-replace native renames; an in-process failure revalidates identities and rolls back every committed destination without touching pre-existing paths. Startup recovery marks restoration complete only after the owning adapter rediscovers every planned item.
- CleanerX offers backup only for data with a supported restore path. Codex session deletion and global memory reset remain enabled but explicitly irreversible because restoring copied files cannot recreate private App Server state and CleanerX never repairs private Codex SQLite records.
- Backup identities stay in the native platform credential store: macOS Keychain, Linux Secret Service, or Windows Credential Manager. A missing or failing credential backend disables backup creation rather than weakening encryption.
- CleanerX does not force-quit Codex or another writer.
- Inventory may retain one normalized, 96-character first-user-message excerpt only as an unnamed pi session title, matching pi's own selector; no additional transcript content is retained by the scan.
- There is no telemetry, crash upload, cloud synchronization, background daemon, or general shell/filesystem command exposed to the GUI. The sole built-in network action is a user-initiated HTTPS update check against the fixed CleanerX GitHub Releases endpoint; it sends no Agent data and never runs on a timer or at startup.
- Application updates use Tauri's official updater through three purpose-specific Rust commands. The webview cannot choose an endpoint, public key, target file, or shell command. Update installation requires a manifest-selected artifact whose Tauri signature verifies against the public key embedded in the installed build.
- The updater signing private key is absent from the repository and release artifacts. Release automation receives it only through GitHub Actions secrets; the maintainer's recovery copy stays in the native credential store. Operating-system signing remains a separate trust layer: the updater signature does not make an artifact Apple-notarized or establish a Microsoft publisher identity.

Implementation details and qualified mutation routes are maintained in the [storage and transaction model](docs/storage-model.md) and [compatibility matrix](docs/compatibility.md).
