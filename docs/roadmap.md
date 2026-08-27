# Development plan

This plan orders work by data-safety risk. A later milestone may be explored early, but it must not delay an unresolved safety requirement in an earlier milestone.

The [open-source release plan](open-source-release-plan.md) separates public source availability from mutation-capable binary releases. The repository may be published as a source preview before M1 is complete, but any promoted binary with cleanup mutations enabled must pass the M1 safety gate. Signing and notarization are explicitly optional and do not replace that gate.

## Current baseline

CleanerX currently has:

- a Rust workspace with `cleanerx-core`, `adapter-codex`, and a narrow Tauri command layer;
- a React/TypeScript GUI for Codex inventory, cleanup planning, backup listing, and settings;
- a project-rooted session tree with a filtered list alternative and scoped bulk selection;
- a presentation-only “No project” session root plus updated-time filtering for recent sessions;
- a full-width desktop layout with centered navigation, overview storage charts, bounded media thumbnails, and an explicitly confirmed permanent-backup-delete flow;
- persisted Chinese/English and system-aware light/dark appearance preferences with immediate preview;
- branded cross-platform application icons, a custom macOS DMG layout, and a native macOS About panel with version and BeaCOx copyright metadata;
- Codex App Server capability probing, control-socket timeout handling, and stdio fallback;
- encrypted `.cxb` backup/restore primitives, path guards, and an operation journal;
- optional, off-by-default cleanup backups with an explicit irreversible-deletion warning;
- macOS Apple Silicon `.app` and unsigned DMG builds, plus CI definitions for Apple Silicon and Intel artifacts;
- cross-platform Rust checks and frontend tests in CI.

This is an engineering MVP, not yet a promise that every Codex storage revision or crash boundary has production-grade coverage.

## M1 — Codex safety hardening

Priority: required before promoting a public mutation-capable binary. Source code may be published earlier under the source-preview conditions in the [open-source release plan](open-source-release-plan.md).

### Storage compatibility

- Build versioned temporary `CODEX_HOME` fixtures for normal and compressed rollouts, archives, descendants, pinned sessions, attachments, generated images, logs, caches, and temporary files.
- Add recognized-schema fixtures for state and log databases, including WAL, corrupt databases, missing columns, and future/unknown schemas.
- Maintain a compatibility table for tested Codex CLI/Desktop versions and record capability-based degradation independently of version strings.
- Ensure filesystem-only fallback reports provenance and remains read-only for sessions.

### Transaction recovery

- Exercise every journal transition with fault injection: before archive creation, during archive streaming, after verification, during each mutation route, and before/after rescan verification.
- Add startup recovery UI that can continue verification, restore a committed backup, or safely terminate an incomplete operation.
- When backup is selected, prove deletion cannot begin while a `.partial` archive exists or while manifest/hash verification is incomplete.
- Add concurrent-writer and file-identity-change fixtures for each direct file category.

### Mutation routes

- Validate session deletion against active, archived, pinned, loaded, parent, child, and subagent combinations using an isolated live App Server test environment.
- Complete the memory-reset flow: require Codex exit, optionally create a consistent memory/database backup, call `memory/reset`, rescan, and expose capability-specific errors without disabling session cleanup.
- Formalize the cross-Agent memory capability model described in [Agent memory research](memory-management.md); Codex remains inspect/reset-only unless an official entry-level mutation API appears.
- Implement and validate log maintenance only for recognized schemas using transactions, WAL checkpointing, and compaction. Unknown schemas stay report-only.
- Remove attachments/generated content only after the owning session mutation succeeds, then verify references and residual files by rescan.

### Exit criteria

- Protected files and source fixtures are byte-identical after every cleanup test.
- All destructive fault-injection cases end in an explainable journal state; backup-enabled paths are recoverable.
- Restored file hashes match the backup manifest and Codex can rediscover restored sessions.
- Unknown Codex capabilities, schemas, and active writers produce a specific read-only/blocking reason in the GUI.

## M2 — macOS release readiness

Priority: public source release and repeatable unsigned artifacts.

- Add Tauri smoke tests for launch, scan, read-only degradation, review dialog, and backup listing on macOS 13+.
- Produce Intel and Apple Silicon `.app` and DMG artifacts from tagged CI builds, with SHA-256 checksums and retained build metadata.
- Verify accessibility: keyboard-only navigation, focus order, contrast, system Chinese/English switching, dark mode, and reduced motion.
- Add a release checklist covering version synchronization, changelog, dependency/license review, fixtures, `make check`, bundle smoke tests, and Gatekeeper instructions.
- Document reproducible local commands for both architectures and keep signing/notarization optional until a release identity is available.
- Mark artifact names and release notes as unsigned, explain the limits of checksum verification, and never instruct users to disable Gatekeeper globally.
- Follow the staged source-preview, alpha, pilot, and `v0.1.0` gates in the [open-source release plan](open-source-release-plan.md).

### Exit criteria

- A clean checkout can run `make setup`, `make check`, and build the native artifact for its architecture.
- Tagged CI publishes both architecture artifacts and checksums.
- Packaged apps pass native launch and core read-only scan smoke tests.

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

Claude Code is the first planned entry-level memory editor because its official auto-memory format is user-editable Markdown. OpenCode and Pi do not receive a native memory editor until their official capabilities define one; rules and extension-owned data remain protected or unsupported. See [Agent memory research](memory-management.md).

Each adapter progresses through the same gates:

1. installation and home-directory detection;
2. read-only inventory and project/session hierarchy mapping;
3. category ownership and protected-data specification;
4. official mutation capability discovery;
5. encrypted backup/restore round trip;
6. guarded cleanup with fixtures, fault injection, and native smoke tests.

Adapters remain compile-time implementations of `AgentAdapter`. Reverse-engineered databases may support read-only reporting, but never serve as an undocumented mutation fallback.

### Exit criteria

- The adapter publishes a tested version/capability matrix and official source references.
- Project/session inventory trees preserve roots, descendants, forks, and unlinked sessions without loading message bodies during scanning.
- One Agent's cleanup plan cannot select another Agent's paths or protected data.

## M5 — Product polish

- Improve large-inventory performance through streaming, bounded concurrency, cancellation, and incremental progress reporting.
- Add storage-change comparisons based only on local snapshots; do not introduce telemetry.
- Refine backup lifecycle UX, explicit expiry reminders, restore conflict explanations, and export/import where platform key handling remains safe.
- Evaluate signed/notarized macOS releases and automatic updates as separate, explicit security decisions.

## Deliberate non-goals

- Cloud-side ChatGPT/Codex data deletion.
- Dynamic third-party cleanup plugins or arbitrary cleanup scripts.
- Background transcript/content indexing or retaining content in inventory snapshots. Explicit bounded, read-only item previews are supported.
- Background monitoring, unattended destructive cleanup, forced process termination, or silent backup purging.
- General shell/filesystem privileges in the webview.

## Planning rules

- Prefer capability detection over hard-coded version gates.
- Land read-only support before mutation support for a new schema, platform, or Agent.
- Every mutation feature is planned together with backup, crash recovery, verification, and restore—not as follow-up work.
- Update this document when milestone scope or exit criteria change; implementation details belong in the storage model or adapter-specific design documents.
