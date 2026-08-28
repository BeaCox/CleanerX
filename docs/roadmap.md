# Development roadmap

Status: active roadmap

Last updated: 2026-08-28

This is the repository's single source of truth for unfinished work. Design, security, storage, and release documents describe current behavior and binding constraints; they link here instead of maintaining separate phase or task lists.

Work is ordered by data-safety risk. A later milestone may be explored early, but it must not delay an unresolved safety requirement in an earlier milestone. The [open-source release policy](open-source-release-plan.md) separates public source availability from mutation-capable binary releases. The repository may be published as a source preview before M1 is complete, but any promoted binary with cleanup mutations enabled must pass the M1 safety gate. Signing and notarization are explicitly optional and do not replace that gate.

## Current baseline

CleanerX currently has:

- a Rust workspace with `cleanerx-core`, compile-time Codex, Claude Code, OpenCode, and pi adapters, and a narrow Tauri command layer;
- a React/TypeScript GUI for Agent-specific inventory, cleanup planning, backup listing, settings, and a persisted target-Agent switcher in the bottom status deck;
- a project-rooted session tree with a filtered list alternative and scoped bulk selection;
- a presentation-only “No project” session root plus updated-time filtering for recent sessions;
- a full-width desktop layout with centered navigation, overview storage charts, bounded media thumbnails, and an explicitly confirmed permanent-backup-delete flow;
- persisted Chinese/English and system-aware light/dark appearance preferences with immediate preview;
- branded cross-platform application icons, a custom macOS DMG layout, and a native macOS About panel with version and BeaCox copyright metadata;
- Codex App Server capability probing, control-socket timeout handling, and stdio fallback;
- Claude Code Home/binary/writer detection, metadata-only session inventory, project auto-memory inventory, guarded local-data cleanup, and Agent-bound backup/restore;
- OpenCode XDG data/cache and binary/writer detection, recognized-SQLite metadata inventory, official CLI session deletion, export/import backup and restore, and protected legacy/source-managed storage;
- pi agent-directory and binary/writer detection, metadata-only JSONL session inventory with fork lineage, writer blocking, guarded session-file deletion through the documented removal route, model-catalog cache cleanup, and protected configuration/extension storage;
- encrypted `.cxb` backup/restore primitives, path guards, and an operation journal;
- optional, off-by-default cleanup backups with an explicit irreversible-deletion warning;
- macOS Apple Silicon and Intel `.app`/DMG builds;
- unsigned x86_64 Linux `.deb` and AppImage builds on Ubuntu 22.04, with a native Xvfb launch smoke test and an isolated Secret Service backup/restore round trip;
- unsigned x86_64 Windows MSI and NSIS builds, with a native launch smoke test and an isolated Credential Manager backup/restore round trip;
- Windows application-data and package-manager launcher discovery, stdio-only Codex control transport, writer-process recognition, write-through atomic replacement, volume/file identity checks, owner validation, and junction/reparse-point rejection;
- reusable product CI with the complete Linux quality gate, cross-platform Rust tests, and native Linux/Windows launch smoke tests;
- one SemVer tag workflow that reruns product CI, validates synchronized versions and `main` ancestry, builds every supported platform, and publishes a GitHub Release with explicitly unsigned assets, build metadata, committed lockfiles, and SHA-256 checksums.

This is an engineering MVP, not yet a promise that every Codex storage revision or crash boundary has production-grade coverage.

## M0 — Publishable source baseline

Priority: required before making the repository public as a source preview. It does not enable or endorse mutation-capable binaries.

### Repository hygiene

- Scan the tracked tree and Git history for credentials, private transcripts, memory databases, journals, `.cxb` files, local machine identifiers, and non-demo absolute paths.
- Confirm that branding assets and third-party notices can be redistributed under the repository license.
- Confirm version and metadata consistency across the Cargo workspace, frontend package, Tauri configuration, About panel, and documentation.
- Run `make setup` and `make check` from a clean checkout using committed lockfiles, and verify generated output remains untracked.

### Public project controls

- Keep the root documentation explicit about engineering-preview status, irreversible cleanup without backup, supported platforms, build prerequisites, read-only degradation, and unsigned-build policy.
- Enable private vulnerability reporting when the repository becomes public.
- Label only bounded, non-mutation work for first-time contributors.

### Exit criteria

- The tracked tree and reviewed history contain no secret or private user data.
- A clean checkout passes the documented setup and validation path.
- License, security reporting, contribution rules, architecture boundaries, and product limitations are visible from the repository root or documentation index.
- No downloadable application is described as supported or trusted merely because the source repository is public.

## M1 — Mutation safety and crash recovery

Priority: required before promoting a public mutation-capable binary. Source code may be published earlier under the source-preview conditions in the [open-source release plan](open-source-release-plan.md).

**Status: implemented for the mutation routes listed in the [compatibility matrix](compatibility.md).**

M1 is a bounded capability gate, not an Agent-wide allowlist or a promise to cover every historical Agent version. A route is enabled only when the current adapter recognizes its storage or control surface and the shared transaction boundary can account for it. Unsupported routes remain visibly read-only without disabling independently available routes.

### Cross-adapter transaction recovery

- Journal format v2 retains the immutable cleanup plan, selected Agent and snapshot ID, expanded scope, item categories, backup commit phases, and per-operation progress. Every pre/post mutation and verification transition is atomically persisted.
- Selected backups are durably committed, reopened, decrypted, and hash-verified before mutation. Partial archives, incomplete verification, and catalog failures are not accepted as restorable backups.
- Startup inventories all recognized v2 journals before allowing another cleanup for the same Agent. The dismissible recovery dialog rescans the owning Agent and can accept a verified result, restore a catalog-bound backup, or safely close the recovery workflow without retrying an Agent mutation. Strictly recognized pre-v2 status-only journals are removed as obsolete metadata.
- Backup creation and restore commit boundaries have deterministic fault injection. Direct-file routes revalidate source revisions and file identities, while official-API routes recheck runtime capability and writer state before execution.
- Protected fixtures and source/project trees remain outside mutation policies and are asserted byte-identical in the shared and adapter tests.

### Qualified mutation scope

| Adapter | M1-qualified routes | Important limit |
| --- | --- | --- |
| Codex | App Server session deletion, independently probed global memory reset, recognized-schema log retention, cache/temporary cleanup, post-session media cleanup | Codex exposes no supported session or memory import route, so these deletions remain enabled but are explicitly irreversible and are not presented as restorable backups. |
| Claude Code | Session files, selected project auto-memory, recognized history/cache/temporary paths | Any recognized Claude writer blocks mutation; source and project directories never become cleanup roots. |
| OpenCode | Offline official CLI deletion, verified idle loopback Server deletion, export/import session backup, offline log/cache cleanup | Unknown/changed SQLite state and unverified or busy writers fail closed. |
| pi | Documented session-file deletion and `models-store.json` cleanup | Any recognized pi writer blocks mutation; fork lineage is display-only and never creates a deletion cascade. |

The detailed route, evidence, and backup status is maintained in [compatibility.md](compatibility.md). Codex `thread/delete` and `memory/reset` are probed independently; absence of one does not disable the other. Orphaned Codex media remains inspect-only, and session-owned media is removed only after the owning `thread/delete` succeeds.

### Exit criteria

- An interrupted operation cannot be mistaken for success or bypassed by starting another cleanup for the same Agent.
- Every recognized recovery begins with an Agent rescan; journal progress alone never proves the mutation result.
- A selected backup is independently verified before deletion begins, and restore either completes fully or returns the destination tree to its pre-restore state.
- Recovery restoration is marked complete only after the selected adapter rediscovers every planned restored item.
- Unknown committed journals block mutation while leaving browsing available; unknown capabilities, schemas, transports, redirects, changed identities, and active writers fail closed for the affected route.
- `make check` passes with journal, backup/restore, adapter mutation, protected-data, i18n, and startup-recovery coverage.

Broader version-by-version fixtures, native disposable mutation cycles, and pilot evidence remain part of M2 release readiness. They extend the compatibility matrix without reopening the shared M1 transaction design.

## M2 — Cross-platform release readiness

Priority: repeatable unsigned artifacts and a bounded cross-platform path from alpha to `v0.1.0`. The release states and mandatory disclosures are defined in the [open-source release policy](open-source-release-plan.md).

### Alpha artifacts

- Add Tauri smoke tests for launch, scan, read-only degradation, detail loading, review dialog, backup listing, and settings persistence on macOS 13+.
- Launch each architecture artifact on a clean supported environment and run an isolated mutation/backup/restore cycle with disposable Agent data.
- Confirm bundles contain no development URLs, private source-map paths, local preferences, test data, journals, or backup identities.
- Verify accessibility: keyboard-only navigation, focus order, contrast, system Chinese/English switching, dark mode, and reduced motion.
- Document reproducible local commands for both architectures and the supported Finder/System Settings opening path. Never instruct users to disable Gatekeeper globally or run broad quarantine-removal commands.
- Verify the manual signed updater on every supported updater package after draft artifacts exist: no startup/background request, invalid signatures fail closed, stable feeds exclude prereleases, and installation still requires two explicit user actions.

### Bounded pilot and beta

- Recruit a small pilot audience that understands the unsigned engineering-preview status and begins with read-only inventory/review before mutation testing.
- Collect sanitized compatibility reports only through repository issues or discussions; do not add telemetry or request transcripts, credentials, memory contents, logs, journals, or backups.
- Publish observed Agent versions, transports, capabilities, supported mutations, and read-only degradation in the compatibility table.
- Treat possible source-tree, credential, configuration, backup, restore, or cross-item deletion defects as release blockers; visibly disable an affected mutation category when the regression cannot be bounded immediately.
- Add a fixture and regression test before closing each storage-compatibility defect.

### `v0.1.0` release and maintenance

- Complete the source, mutation-safety, unsigned-artifact, and pilot exit criteria; freeze the tested compatibility matrix and list unsupported operations by capability.
- Synchronize all version surfaces and publish a changelog, migration notes where needed, checksums, build metadata, compatibility limits, and the exact source commit.
- Repeat native smoke tests on the final tag instead of promoting an earlier build by renaming it.
- Keep `main` releasable; use fixtures and negative-path tests for every new schema/category/mutation route; never silently widen a cleanup allowlist in a patch release.
- Define patch releases for compatibility fixes and minor releases for backward-compatible capability work, and explicitly mark capabilities read-only when the maintained release line can no longer validate them.

### Exit criteria

- A clean checkout can run `make setup`, `make check`, and build the native artifact for its architecture.
- Tagged CI publishes every supported platform/architecture artifact and its checksums.
- Packaged apps pass native launch, core read-only scan, and disposable mutation/restore smoke tests.
- Beta evidence comes from installations beyond the maintainer, and no unresolved defect can mutate protected data or leave an unexplained unrecoverable operation.
- Release notes link the exact compatibility matrix, known limitations, security policy, and source commit.

## M3 — Windows and Linux foundations

**Status: implemented.** The Windows-only core and adapter tests, Credential Manager round trip, debug and release launch smoke tests, and unsigned MSI/NSIS packaging all pass on the native Windows CI host. Installation and interactive acceptance on a supported Windows 10/11 desktop remain part of the broader pilot and release gates rather than this foundation milestone.

Linux provides XDG/home data resolution, process probing, common desktop-launch executable locations, Unix ownership and same-device mount-boundary checks, Secret Service-backed `.cxb` encryption, `.deb`/AppImage packages, and a native Xvfb smoke test.

Windows provides roaming/local application-data resolution, `.exe`/`.cmd`/`.bat` package-manager discovery, native writer recognition, direct Codex stdio App Server transport, current-token default-owner and same-volume/file-index validation, junction/reparse-point rejection, write-through atomic replacement, Credential Manager-backed `.cxb` encryption, MSI/NSIS packages, and a native launch smoke test. The platform-independent path/process tests run on every host; Windows CI additionally exercises native path semantics, builds the Tauri application, and launches it as an ongoing product regression check.

### Exit criteria

- Core and Codex inventory run on all three platforms with identical protected-path invariants.
- Backup/restore round trips use the native key store and reject overwrite/path escape cases.
- Unsupported platform operations are visibly disabled rather than silently approximated.

## M4 — Additional Agent adapters

Priority order: Claude Code, OpenCode, then pi, subject to documented interface stability.

Claude Code, OpenCode, and pi are implemented additional adapters. Claude Code includes installation detection, read-only inventory, protected-path specification, project memory deletion capability, encrypted backup/restore, writer blocking, guarded cleanup, and post-operation rescan. OpenCode includes recognized-SQLite read-only inventory, offline official CLI deletion, verified loopback Server API deletion for inactive sessions, official export/import recovery, writer blocking, descendant expansion, and protected data/cache roots; it intentionally exposes no memory item because no supported automatic-memory capability was found. pi includes installation and writer detection, metadata-only session inventory from the documented JSONL layout with fork lineage display, writer blocking, guarded deletion of session files through the documented file-removal route, `models-store.json` cache cleanup, and protected configuration, trust, rules, skills, extension, and package storage; it exposes no memory item because no supported automatic-memory capability was found. Claude Code's documented auto-memory Markdown is user-editable; CleanerX currently reports entry editing as **not yet supported**, not prohibited. Instructions and rules remain protected. See the [Agent memory capability and safety model](memory-management.md).

Each adapter progresses through the same gates:

1. installation and home-directory detection;
2. read-only inventory and project/session hierarchy mapping;
3. category ownership and protected-data specification;
4. official mutation capability discovery;
5. encrypted backup/restore round trip;
6. guarded cleanup with fixtures, fault injection, and native smoke tests.

Adapters remain compile-time implementations of `AgentAdapter`. Reverse-engineered databases may support read-only reporting, but never serve as an undocumented mutation fallback.

### Claude Code memory entry editing

- Represent recognized topic entries with stable adapter-owned IDs while keeping all webview commands ID-based and bounded to the current snapshot.
- Round-trip `MEMORY.md` and topic files without losing unknown frontmatter, links, ordering, or unrelated files; show a Markdown-aware before/after diff.
- Reject external `autoMemoryDirectory` targets until they have their own fixed-root policy; never visit a source tree or project `.claude/` directory.
- Require Claude Code and other detected writers to exit, capture and revalidate source hashes/identities, and fail stale edits instead of overwriting newer Agent output.
- Write through sibling temporary files with `fsync` and atomic rename; journal multi-file index/topic changes as all-or-nothing and verify the expected entry/hash state by rescan.
- Apply the normal optional-backup contract: verify a selected encrypted backup before mutation, otherwise show the concise irreversible-change warning.
- Add fixtures for repository/worktree sharing, missing indexes, duplicate links, malformed or unknown frontmatter, symlinks, permission failures, concurrent edits, every journal boundary, and byte-identical protected/source trees.
- Enable `canEditEntries` and `canDeleteEntries` only after edit, delete, recovery, restore, i18n, confirmation, error, and keyboard-accessibility tests pass.

### Later adapter memory decisions

- Recheck OpenCode and pi official releases for a native memory API or recognized schema before implementing either adapter's memory surface. OpenCode session support must not be treated as evidence of a memory capability.
- If pi memory depends on an extension, identify the extension and exact version at compile time; leave unknown extension data untouched.
- Never reinterpret rules, prompts, system files, or arbitrary `MEMORY.md` files as automatic memory to populate the UI.

### Exit criteria

- The adapter publishes a tested version/capability matrix and official source references.
- Project/session inventory trees preserve roots, descendants, forks, and unlinked sessions without loading message bodies during scanning.
- One Agent's cleanup plan cannot select another Agent's paths or protected data.

## M5 — Product polish

- Improve large-inventory performance through streaming, bounded concurrency, cancellation, and incremental progress reporting.
- Add storage-change comparisons based only on local snapshots; do not introduce telemetry.
- Refine backup lifecycle UX, explicit expiry reminders, restore conflict explanations, and export/import where platform key handling remains safe.
- Evaluate signed/notarized macOS releases and automatic updates as separate, explicit security decisions.
- Evaluate Homebrew distribution only after the supported release and recovery path is established.

## Deliberate non-goals

- Cloud-side ChatGPT/Codex data deletion.
- Dynamic third-party cleanup plugins or arbitrary cleanup scripts.
- Background transcript/content indexing or retaining content in inventory snapshots. Explicit bounded, read-only item previews are supported.
- Background monitoring, unattended destructive cleanup, forced process termination, or silent backup purging.
- General shell/filesystem privileges in the webview.
- A public website or growth campaign as an engineering milestone.

## Planning rules

- Prefer capability detection over hard-coded version gates.
- Land read-only support before mutation support for a new schema, platform, or Agent.
- Every mutation feature is planned together with backup, crash recovery, verification, and restore—not as follow-up work.
- Add every unfinished task to this document instead of creating another implementation plan or phase checklist.
- Update this document when milestone scope or exit criteria change; implementation details, current behavior, and binding policies belong in their owning documents.
