# Open-source release policy

Status: active policy document

Last reviewed: 2026-08-30

This document defines the release states and gates for publishing CleanerX as an open-source project. It contains no implementation backlog. Every unfinished release, packaging, pilot, and maintenance task is tracked only in the [development roadmap](roadmap.md).

Signing and notarization are not release prerequisites. Source publication, mutation safety, and binary publisher identity are separate concerns with separate gates.

`SECURITY.md` is the normative product-safety policy and `AGENTS.md` is the normative development policy. This document does not redefine or weaken either one.

## Release principles

- Publishing source does not imply that cleanup mutations are production-ready.
- An unsigned build must meet the same data-safety requirements as a signed build.
- Checksums and build metadata provide artifact-integrity evidence, but they do not replace publisher identity or notarization.
- Unsupported Agent capabilities, schemas, and platforms degrade to explicit read-only behavior.
- Release documentation describes observed behavior and tested compatibility; it never presents roadmap work as implemented.
- CleanerX remains local-first. It has no telemetry, data upload, background daemon, or cloud synchronization. The explicitly approved updater contacts only the fixed GitHub Releases endpoint after a user action and sends no Agent data.
- Source trees, credentials, configuration, rules, skills, plugins, browser accounts, and cookies remain outside the cleanup boundary.

## Release states

Advancing the source repository does not automatically advance the downloadable application.

| State | Distribution | Mutation status | Purpose |
| --- | --- | --- | --- |
| Source preview | Public default branch, no promoted binary | Experimental; build from source at the user's own risk | Make the design and implementation reviewable and begin accepting engineering feedback. |
| Alpha | Tagged, unsigned macOS, Linux, and Windows artifacts | Enabled only after the mutation-safety gate passes | Validate packaging, compatibility, and real installations with a small group. |
| Beta | Tagged, unsigned macOS, Linux, and Windows artifacts | Enabled with documented compatibility and recovery limits | Collect broader compatibility evidence and close release-blocking defects. |
| `v0.1.0` | Repeatable unsigned cross-platform release | Supported within the published compatibility matrix | Establish the first maintained public release. |

A future signed or notarized release may replace unsigned artifacts without changing the safety gates. Signing remains optional until a suitable release identity is available.

Current state: [`v0.1.0`](https://github.com/BeaCox/CleanerX/releases/tag/v0.1.0) was published as the first stable release on 2026-08-28 UTC. Its applications remain unsigned and non-notarized, and support is limited to the capability-gated compatibility matrix.

## Source-preview gate

The repository may be public before mutation hardening is complete only when:

- the tracked tree and reviewed history contain no credentials or private Agent data;
- a clean checkout follows the documented setup, validation, and build path using committed lockfiles;
- licensing, security reporting, contribution rules, architecture boundaries, and product limitations are visible;
- documentation calls the project an engineering preview and explains that cleanup without backup is irreversible; and
- no downloadable application is described as supported or trusted merely because the source is public.

## Mutation-capable binary gate

No alpha, beta, or supported binary may promote cleanup mutations until M1 in the [development roadmap](roadmap.md#m1--mutation-safety-and-crash-recovery) passes. In particular:

- every direct-file and official-API mutation revalidates capability, path, identity, ownership, protected descendants, and active-writer state;
- a selected backup is independently reopened and verified before mutation;
- restore preflights all destinations and is all-or-nothing;
- backup creation, restore commits, and journal persistence around adapter mutations have deterministic boundary coverage;
- interrupted operations have an explainable, recoverable journal state; and
- unknown capabilities or schemas remain visible but read-only.

The shared M1 gate is implemented for the routes recorded in the [mutation compatibility matrix](compatibility.md). A route absent from that matrix must remain read-only until it is qualified; broader version and native-host evidence remains part of the alpha/beta release process.

## Unsigned-artifact policy

Every unsigned artifact name, release page, and installation section must state that:

- the application is unsigned and not notarized;
- macOS cannot verify the publisher identity, and Windows SmartScreen may warn about the unknown publisher;
- a checksum verifies that a download matches the published asset but does not substitute for signing;
- the supported opening path is Finder **Open** or approval in **System Settings → Privacy & Security**;
- users must not be told to disable Gatekeeper globally or run broad quarantine-removal commands; and
- building from source remains available for users who do not accept an unsigned binary.

An artifact is released from its exact tested tag. An earlier build is never promoted by renaming it, and the application does not silently update itself. Update checks are manual, present the target version before installation, and require a second explicit install action.

## Updater-signing policy

- Tauri updater signing is required for every in-app update artifact and is separate from optional Apple/Microsoft operating-system signing.
- The public key is pinned in `src-tauri/tauri.conf.json`. Release automation reads the private key only from the `TAURI_SIGNING_PRIVATE_KEY` GitHub Actions secret, while the maintainer keeps a recovery copy in the native credential store. It must never be committed, logged, or copied into a release artifact.
- Losing or rotating the private key without a signed transition strands installed builds on their pinned key. Key rotation therefore requires a release signed by the old key that embeds the new trust path before subsequent releases use the new key.
- The release job fails if any platform artifact or `.sig` file is absent. It creates `latest.json` only after all architecture-specific artifacts have been collected, and checksums include the manifest, signatures, and updater payloads.
- macOS arm64/x86_64, Windows x86_64 NSIS, and Linux x86_64 AppImage are the in-app update targets. MSI, `.deb`, DMG, and application ZIP artifacts remain manual distribution formats.

## Pilot and defect policy

- The alpha and beta are bounded compatibility exercises, not broad product launches.
- Feedback is collected through explicit repository channels without telemetry.
- Reports request sanitized version, platform, transport, and capability metadata, never transcripts, credentials, memory contents, logs, journals, or backup archives.
- Possible source-tree, credential, configuration, backup, restore, or cross-item deletion defects are release blockers.
- When a safety regression cannot be bounded immediately, the affected mutation category is visibly disabled.
- Storage-compatibility fixes require a fixture and regression test before closure.

## `v0.1.0` support boundary

`v0.1.0` is an engineering support commitment within a narrow published compatibility matrix, not a claim of universal Agent compatibility. It requires the source-preview, mutation, artifact, and pilot gates to pass on the final tag.

Maintenance releases must preserve the fixed cleanup boundaries. A patch release may fix compatibility but must not silently widen an allowlist. New schemas, categories, and mutation routes require fixtures, negative-path tests, documentation, and the same release gates. Capabilities that can no longer be validated are explicitly downgraded to read-only.

## Release evidence

Each tag has a separate checklist recording version synchronization, private-data scanning, dependency/license review, `make check`, safety and fault-injection results, native smoke tests, artifact/checksum verification, compatibility limits, unsigned-build disclosure, and release-note review. Tag automation reruns product CI, rejects unsynchronized versions and tags outside `main`, and creates a draft GitHub Release containing platform-specific build metadata, committed lockfiles, and SHA-256 checksums. A maintainer publishes that draft only after completing its artifact, signature, checksum, installation, and release-note checks. The checklist records the remaining human evidence for that tag; it does not become another backlog.
