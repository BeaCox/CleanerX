# Development roadmap

This is the repository's single source of truth for unfinished work. Design, security, storage, and release documents describe current behavior and binding constraints; they link here instead of maintaining separate phase or task lists.

Work is ordered by data-safety risk. A later milestone may be explored early, but it must not delay an unresolved safety requirement in an earlier milestone. The [open-source release policy](open-source-release-plan.md) separates public source availability from mutation-capable binary releases. The repository may be published as a source preview before M1 is complete, but any promoted binary with cleanup mutations enabled must pass the M1 safety gate. Signing and notarization are explicitly optional and do not replace that gate.

## Current baseline

CleanerX currently has:

- a Rust workspace with `cleanerx-core`, compile-time Codex and Claude Code adapters, and a narrow Tauri command layer;
- a React/TypeScript GUI for Agent-specific inventory, cleanup planning, backup listing, settings, and a persisted target-Agent switcher in the bottom status deck;
- a project-rooted session tree with a filtered list alternative and scoped bulk selection;
- a presentation-only “No project” session root plus updated-time filtering for recent sessions;
- a full-width desktop layout with centered navigation, overview storage charts, bounded media thumbnails, and an explicitly confirmed permanent-backup-delete flow;
- persisted Chinese/English and system-aware light/dark appearance preferences with immediate preview;
- branded cross-platform application icons, a custom macOS DMG layout, and a native macOS About panel with version and BeaCOx copyright metadata;
- Codex App Server capability probing, control-socket timeout handling, and stdio fallback;
- Claude Code Home/binary/writer detection, metadata-only session inventory, project auto-memory inventory, guarded local-data cleanup, and Agent-bound backup/restore;
- encrypted `.cxb` backup/restore primitives, path guards, and an operation journal;
- optional, off-by-default cleanup backups with an explicit irreversible-deletion warning;
- macOS Apple Silicon `.app` and unsigned DMG builds, plus CI definitions for Apple Silicon and Intel artifacts;
- cross-platform Rust checks and frontend tests in CI.

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
- Add a sanitized compatibility-report template covering CleanerX, OS, Agent, transport, and capability versions while warning users not to attach private Agent data.
- Enable required CI, private vulnerability reporting, focused issue forms, and a pull-request template with safety, fixture, documentation, and `make check` checks.
- Label only bounded, non-mutation work for first-time contributors.

### Exit criteria

- The tracked tree and reviewed history contain no secret or private user data.
- A clean checkout passes the documented setup and validation path.
- License, security reporting, contribution rules, architecture boundaries, and product limitations are visible from the repository root or documentation index.
- No downloadable application is described as supported or trusted merely because the source repository is public.

## M1 — Codex safety hardening

Priority: required before promoting a public mutation-capable binary. Source code may be published earlier under the source-preview conditions in the [open-source release plan](open-source-release-plan.md).

### Storage compatibility

- Build versioned temporary `CODEX_HOME` fixtures for normal and compressed rollouts, archives, descendants, pinned sessions, attachments, generated images, logs, caches, and temporary files.
- Add recognized-schema fixtures for state and log databases, including WAL, corrupt databases, missing columns, and future/unknown schemas.
- Maintain a compatibility table for tested Codex CLI/Desktop versions and record capability-based degradation independently of version strings.
- Ensure filesystem-only fallback reports provenance and remains read-only for sessions.

### Transaction recovery

- Reopen every selected encrypted backup from disk before mutation, decrypt it, validate the manifest identity and format, verify every archived hash, and durably flush the archive and containing directory where supported.
- Prevent `.cxb.partial` files, incomplete verification, catalog failures, or orphan archives from being treated as committed backups.
- Stage and hash the full restore before mutation; preflight every root, destination, ownership rule, path boundary, and conflict; then complete all destinations or roll back every committed destination.
- Journal cross-filesystem restore fallback explicitly instead of silently substituting copy for rename.
- Exercise every journal transition with fault injection: before and after archive creation, archive verification, catalog commit, each destination commit or rollback, each cleanup mutation route, and rescan verification.
- Add startup recovery UI that can continue verification, restore a committed backup, or safely terminate an incomplete operation.
- Preserve an operation's committed `backup_id` after later failure and retain enough immutable plan/progress data to explain which mutations completed.
- Never infer completion from journal state alone; rescan and verify the affected inventory.
- Add concurrent-writer and file-identity-change fixtures for each direct file category.

### Mutation routes

- Validate session deletion against active, archived, pinned, loaded, parent, child, and subagent combinations using an isolated live App Server test environment.
- Probe `thread/delete` independently from `thread/list` and `memory/reset` without deleting a real session.
- Complete the memory-reset flow: require Codex exit, optionally create a consistent memory/database backup, call `memory/reset`, rescan, and expose capability-specific errors without disabling session cleanup.
- Finish versioned memory fixtures for recognized and unknown schemas, concurrent writers, reset failures, journal boundaries, rescan, and restore. Codex remains inspect/reset-only unless an official entry-level mutation API appears.
- Implement and validate log maintenance only for recognized schemas using transactions, WAL checkpointing, and compaction. Unknown schemas stay report-only.
- Remove attachments/generated content only after the owning session mutation succeeds, then verify references and residual files by rescan.

### Exit criteria

- Protected files and source fixtures are byte-identical after every cleanup test.
- All destructive fault-injection cases end in an explainable journal state; backup-enabled paths are recoverable.
- A selected backup is independently verified before deletion begins, and restore either completes fully or returns the destination tree to its pre-restore state.
- Restored file hashes match the backup manifest and Codex can rediscover restored sessions.
- Unknown Codex capabilities, schemas, and active writers produce a specific read-only/blocking reason in the GUI.

## M2 — macOS release readiness

Priority: repeatable unsigned artifacts and a bounded path from alpha to `v0.1.0`. The release states and mandatory disclosures are defined in the [open-source release policy](open-source-release-plan.md).

### Alpha artifacts

- Extend the tag workflow to create a GitHub Release with Apple Silicon and Intel `.app` and DMG artifacts from the same tag.
- Name every artifact explicitly as unsigned and publish SHA-256 checksums, the source commit, toolchain and runner versions, lockfiles, build timestamp, retained logs, and investigation metadata.
- Add Tauri smoke tests for launch, scan, read-only degradation, detail loading, review dialog, backup listing, and settings persistence on macOS 13+.
- Launch each architecture artifact on a clean supported environment and run an isolated mutation/backup/restore cycle with disposable Agent data.
- Confirm bundles contain no development URLs, private source-map paths, local preferences, test data, journals, or backup identities.
- Verify accessibility: keyboard-only navigation, focus order, contrast, system Chinese/English switching, dark mode, and reduced motion.
- Create a repeatable per-tag release checklist covering version synchronization, secret/private-data scanning, dependency and license review, `make check`, safety fixtures, native smoke tests, artifact/checksum verification, compatibility limits, unsigned-build disclosure, and final release-note review.
- Document reproducible local commands for both architectures and the supported Finder/System Settings opening path. Never instruct users to disable Gatekeeper globally or run broad quarantine-removal commands.
- Keep automatic updating out of the alpha; installation and release notes must explain that checksum verification does not establish publisher identity.

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
- Tagged CI publishes both architecture artifacts and checksums.
- Packaged apps pass native launch, core read-only scan, and disposable mutation/restore smoke tests.
- Beta evidence comes from installations beyond the maintainer, and no unresolved defect can mutate protected data or leave an unexplained unrecoverable operation.
- Release notes link the exact compatibility matrix, known limitations, security policy, and source commit.

## M3 — Windows and Linux foundations

Priority: portability before additional Agent adapters.

- Extract platform services for application data paths, process detection, control transport, file ownership checks, atomic replacement, and secure key storage.
- Use Windows Credential Manager and Linux Secret Service/keyring implementations without weakening `.cxb` encryption.
- Add Windows junction/reparse-point and Linux symlink/mount-boundary safety fixtures.
- Create Tauri installers/packages and native smoke tests while keeping platform-specific mutation categories disabled until validated.

### Exit criteria

- Core and Codex inventory run on all three platforms with identical protected-path invariants.
- Backup/restore round trips use the native key store and reject overwrite/path escape cases.
- Unsupported platform operations are visibly disabled rather than silently approximated.

## M4 — Additional Agent adapters

Priority order: Claude Code, OpenCode, then Pi, subject to documented interface stability.

Claude Code is the first additional adapter. Its installation detection, read-only inventory, protected-path specification, project memory deletion capability, encrypted backup/restore, writer blocking, guarded cleanup, and post-operation rescan are implemented. Its documented auto-memory Markdown is user-editable; CleanerX currently reports entry editing as **not yet supported**, not prohibited. `CLAUDE.md`, rules, and other instructions remain protected. See the [Agent memory capability and safety model](memory-management.md).

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

- Recheck OpenCode and Pi official releases for a native memory API or recognized schema before implementing either adapter's memory surface.
- If Pi memory depends on an extension, identify the extension and exact version at compile time; leave unknown extension data untouched.
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
