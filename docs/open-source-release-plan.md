# Open-source release plan

Status: active planning document

Last reviewed: 2026-08-27

This plan defines the engineering path for publishing CleanerX as an open-source project. It does not optimize for a large launch or a specific star count. The goal is a repository that strangers can inspect, build, test, and contribute to without overstating the maturity of destructive operations.

Signing and notarization are not release prerequisites. Source publication, mutation safety, and binary publisher identity are separate concerns and have separate gates below.

`SECURITY.md` remains the normative product-safety policy and `AGENTS.md` remains the normative development policy. This document schedules evidence for those requirements; it does not redefine or weaken them.

## Release principles

- Publishing source does not imply that cleanup mutations are production-ready.
- An unsigned build must meet the same data-safety requirements as a signed build.
- Checksums and build metadata provide artifact-integrity evidence, but they do not replace publisher identity or notarization.
- Unsupported Codex capabilities, schemas, and platforms degrade to read-only behavior.
- Release documentation describes observed behavior and tested compatibility; planned guarantees are not presented as implemented.
- CleanerX remains local-only. The open-source release does not add telemetry, an updater, a background daemon, or cloud services.
- The source tree, credentials, configuration, rules, skills, plugins, browser accounts, and cookies remain outside the cleanup boundary.

## Release tracks

CleanerX uses four release states. Advancing the source repository does not automatically advance the downloadable application.

| State | Distribution | Mutation status | Purpose |
| --- | --- | --- | --- |
| Source preview | Public default branch, no promoted binary | Experimental; build from source at the user's own risk | Make the design and implementation reviewable and begin accepting engineering feedback. |
| Alpha | Tagged, unsigned macOS artifacts | Enabled only after the mutation safety gate passes | Validate packaging, compatibility, and real installations with a small group. |
| Beta | Tagged, unsigned macOS artifacts | Enabled with documented compatibility and recovery limits | Collect broader compatibility evidence and close release-blocking defects. |
| `v0.1.0` | Repeatable unsigned macOS release | Supported within the published compatibility matrix | Establish the first maintained public release. |

A future signed/notarized release may replace the unsigned artifacts without changing these safety gates. Signing work stays optional until a suitable release identity is available.

## Phase 0 — Establish a releasable source baseline

This phase is the gate for making the repository public. It does not require completion of every M1 mutation item and does not publish a downloadable application.

### Repository hygiene

- Scan the tracked tree and Git history for credentials, private transcripts, memory databases, journals, `.cxb` files, local machine identifiers, and non-demo absolute paths.
- Confirm that branding assets and third-party notices can be redistributed under the repository license.
- Keep generated output out of Git: `dist/`, `target/`, `node_modules/`, coverage output, application archives, and local preference data.
- Confirm version and metadata consistency across the Cargo workspace, frontend package, Tauri configuration, About panel, and documentation.
- Run `make check` from a clean checkout using the committed lockfiles.

### Public documentation

- Keep `README.md` explicit that CleanerX is an engineering preview and that cleanup without a backup is irreversible.
- Separate implemented behavior, known limitations, and planned work.
- Document the supported host platform, Codex requirements, build prerequisites, read-only degradation, and unsigned-build policy.
- Keep `SECURITY.md`, `CONTRIBUTING.md`, the storage model, and this plan linked from the documentation index.
- Add a compatibility report template that asks for CleanerX version, macOS version, Codex version, transport used, capability state, and a sanitized reproduction. It must warn users not to attach private Agent data.

### Repository controls

- Require the existing CI workflow on the default branch before merge.
- Enable private vulnerability reporting in the source host.
- Add focused issue forms for defects, compatibility reports, and feature proposals.
- Add a pull-request template containing safety, fixture, documentation, and `make check` checkboxes.
- Label suitable, bounded tasks for first-time contributors; mutation routes are not `good first issue` work.

### Source-publication exit criteria

- The tracked tree and available history contain no secret or private user data.
- A clean checkout passes `make setup` and `make check` using documented toolchain versions.
- License, security reporting, contribution rules, architecture boundaries, and product limitations are visible from the repository root or documentation index.
- No downloadable application is described as supported or trusted merely because the source repository is public.

## Phase 1 — Close the mutation safety gate

This phase is required before promoting any mutation-capable binary, including an unsigned alpha. It implements the relevant M1 work in the [development plan](roadmap.md).

### Backup commit integrity

- After writing an encrypted archive, reopen it from disk, decrypt it, validate the manifest identity and format, and verify every archived entry hash.
- Do not rename `.cxb.partial` to `.cxb` or enter the deletion state until verification succeeds.
- Flush the completed archive and its containing directory where the platform provides the required durability primitives.
- Make catalog commit failures recoverable and ensure an orphan archive cannot be mistaken for a committed backup.

### All-or-nothing restore

- Extract and hash the full archive into an isolated staging directory.
- Preflight every root, destination, parent directory, ownership rule, path boundary, and conflict before the first destination mutation.
- Implement rollback for every committed destination or use an atomic directory-swap strategy where category roots make that possible.
- Treat cross-filesystem copy fallback as an explicit journaled transaction, not as an untracked substitute for rename.
- Add fault injection before and after every destination commit and rollback step.

### Operation recovery

- Preserve the committed `backup_id` when an operation fails after backup creation.
- Record enough immutable plan and progress information to explain which mutations did and did not complete.
- On startup, detect incomplete journals and offer only purpose-specific actions: resume verification, restore a committed backup, or safely terminate an operation that performed no mutation.
- Never infer completion solely from journal state; rescan and verify the affected inventory.

### Capability and compatibility validation

- Probe `thread/delete` without deleting a real session and record it independently from `thread/list` and `memory/reset`.
- Add versioned temporary `CODEX_HOME` fixtures for active, archived, pinned, loaded, parent, child, subagent, compressed rollout, and unlinked-session cases.
- Add recognized, missing-column, WAL, corrupt, and unknown state/log database fixtures.
- Add isolated live App Server tests for pagination, control-socket failure, stdio fallback, capability downgrade, minimal-root deletion, and descendant expansion.
- Verify that direct-file cleanup rejects symlinks, traversal, changed file identity, ownership anomalies, protected descendants, and active writers.

### Mutation-safety exit criteria

- Every destructive boundary has a deterministic failure test before and after irreversible work.
- Protected fixtures and project source trees are byte-identical after every positive and negative cleanup test.
- A selected backup is independently verified before deletion begins.
- Restore either completes fully or returns the destination tree to its pre-restore state.
- Interrupted operations have an explainable and recoverable startup state.
- Unknown capabilities and schemas remain visible but read-only.

If this gate is incomplete, CleanerX may remain public as source preview, but the project must not promote a mutation-capable download.

## Phase 2 — Produce repeatable unsigned alpha artifacts

This phase turns a tagged commit into inspectable, long-lived artifacts without implying code-signing trust.

### Release automation

- Extend the tag workflow to create a GitHub Release instead of uploading only short-lived workflow artifacts.
- Build Apple Silicon and Intel `.app` and DMG artifacts from the same tag.
- Name every unsigned artifact explicitly, for example `CleanerX_0.1.0-alpha.1_aarch64_unsigned.dmg`.
- Publish SHA-256 checksums, the source commit, Rust/Node/pnpm versions, runner image, dependency lockfiles, and build timestamp.
- Retain the workflow logs and build metadata needed to investigate a packaging discrepancy.
- Keep automatic updating out of the application; users update manually from an explicit release.

### Unsigned-build disclosure

Every release page and installation section must state:

- the application is unsigned and not notarized;
- macOS cannot verify the publisher identity;
- the checksum can verify that a download matches the GitHub Release asset, but is not a substitute for signing;
- the supported opening path is Finder **Open** or approval in **System Settings → Privacy & Security**;
- users should not disable Gatekeeper globally or run broad quarantine-removal commands;
- building from source remains available for users who do not accept an unsigned binary.

### Packaged application validation

- Launch each architecture artifact on a clean supported macOS environment.
- Smoke-test first launch, inventory scan, read-only fallback, detail loading, cleanup review, backup listing, and settings persistence.
- Run one isolated mutation/backup/restore cycle with disposable fixture data on each architecture.
- Confirm the bundle contains no development server URLs, source maps containing private paths, local preferences, test data, operation journals, or backup identities.

### Alpha exit criteria

- A tag creates both architecture releases and checksums without manual file replacement.
- A fresh machine can follow the documented unsigned installation flow and complete a read-only scan.
- Packaged smoke tests pass and the result is recorded in the release checklist.
- Release notes link the exact compatibility matrix, known limitations, security policy, and source commit.

## Phase 3 — Run a bounded public pilot

The alpha is a compatibility exercise, not a broad product launch.

### Pilot scope

- Recruit a small set of users who understand that the application is an unsigned preview.
- Ask users to begin with read-only inventory and inspect the cleanup review before testing mutations.
- Collect compatibility reports through GitHub issues or discussions only; do not add telemetry.
- Request sanitized metadata and capability results, never transcripts, credentials, memory contents, logs, journals, or backup archives.
- Publish newly recognized Codex versions and degradation behavior in the compatibility table.

### Defect policy

- Treat possible source-tree, credential, configuration, backup, restore, or cross-item deletion defects as release blockers.
- Pause affected mutation categories when a safety regression cannot be immediately bounded.
- Prefer a visible capability downgrade over an undocumented compatibility workaround.
- Add a fixture and regression test before closing every storage-compatibility defect.

### Beta exit criteria

- The documented clean-checkout and packaged-app paths have been exercised by people other than the maintainer.
- Supported Codex versions have recorded inventory and mutation results; unsupported versions have a clear read-only outcome.
- No unresolved defect can mutate protected data, leave an unrecoverable backup-enabled operation, or partially restore without explanation.
- Installation, troubleshooting, and compatibility documentation reflect the pilot findings.

## Phase 4 — Publish and maintain `v0.1.0`

`v0.1.0` is an engineering support commitment within a narrow matrix, not a claim of universal Codex compatibility.

### Release requirements

- Complete the source, mutation, unsigned-artifact, and pilot exit criteria.
- Freeze a tested compatibility matrix and list unsupported operations by capability.
- Synchronize versions across all manifests and UI surfaces.
- Publish a changelog, migration notes when storage formats change, checksums, and build metadata.
- Repeat native smoke tests on the final tag rather than promoting an earlier build by renaming it.

### Maintenance policy

- Keep `main` releasable and use short-lived branches for scoped changes.
- Require fixtures and negative-path tests for every new schema, category, or mutation route.
- Release compatibility fixes as patch versions and new backward-compatible capability work as minor versions.
- Do not silently widen a cleanup allowlist in a patch release.
- Maintain the current release line while it can be validated against available Codex versions; explicitly mark abandoned capabilities read-only.
- Review dependencies and third-party licenses during each release, avoiding unrelated upgrades.

## Deferred work

The following work is valuable but does not block the initial open-source release:

- Developer ID signing and Apple notarization;
- Homebrew distribution;
- Windows and Linux application bundles;
- additional Agent adapters;
- an automatic updater;
- a public website or growth campaign.

Signing may be added later as its own release project. It must not be used to claim that an artifact is safe, and its absence must not be used to waive mutation, recovery, or verification requirements.

## Release checklist ownership

Before the first alpha tag, create a repeatable checklist under `docs/` or `.github/` covering:

1. version synchronization;
2. secret and private-data scan;
3. dependency and license review;
4. `make check`;
5. safety fixture and fault-injection results;
6. native packaged smoke tests;
7. artifact and checksum verification;
8. compatibility and known-limit documentation;
9. unsigned-build disclosure;
10. final release-note review.

The release checklist records evidence for a particular tag. This plan defines policy and should not be copied into each release.
