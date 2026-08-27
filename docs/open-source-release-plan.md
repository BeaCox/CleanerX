# Open-source release policy

Status: active policy document

Last reviewed: 2026-08-27

This document defines the release states and gates for publishing CleanerX as an open-source project. It contains no implementation backlog. Every unfinished release, packaging, pilot, and maintenance task is tracked only in the [development roadmap](roadmap.md).

Signing and notarization are not release prerequisites. Source publication, mutation safety, and binary publisher identity are separate concerns with separate gates.

`SECURITY.md` is the normative product-safety policy and `AGENTS.md` is the normative development policy. This document does not redefine or weaken either one.

## Release principles

- Publishing source does not imply that cleanup mutations are production-ready.
- An unsigned build must meet the same data-safety requirements as a signed build.
- Checksums and build metadata provide artifact-integrity evidence, but they do not replace publisher identity or notarization.
- Unsupported Agent capabilities, schemas, and platforms degrade to explicit read-only behavior.
- Release documentation describes observed behavior and tested compatibility; it never presents roadmap work as implemented.
- CleanerX remains local-only. A release does not add telemetry, an updater, a background daemon, or cloud services without an explicit product decision.
- Source trees, credentials, configuration, rules, skills, plugins, browser accounts, and cookies remain outside the cleanup boundary.

## Release states

Advancing the source repository does not automatically advance the downloadable application.

| State | Distribution | Mutation status | Purpose |
| --- | --- | --- | --- |
| Source preview | Public default branch, no promoted binary | Experimental; build from source at the user's own risk | Make the design and implementation reviewable and begin accepting engineering feedback. |
| Alpha | Tagged, unsigned macOS artifacts | Enabled only after the mutation-safety gate passes | Validate packaging, compatibility, and real installations with a small group. |
| Beta | Tagged, unsigned macOS artifacts | Enabled with documented compatibility and recovery limits | Collect broader compatibility evidence and close release-blocking defects. |
| `v0.1.0` | Repeatable unsigned macOS release | Supported within the published compatibility matrix | Establish the first maintained public release. |

A future signed or notarized release may replace unsigned artifacts without changing the safety gates. Signing remains optional until a suitable release identity is available.

## Source-preview gate

The repository may be public before mutation hardening is complete only when:

- the tracked tree and reviewed history contain no credentials or private Agent data;
- a clean checkout follows the documented setup, validation, and build path using committed lockfiles;
- licensing, security reporting, contribution rules, architecture boundaries, and product limitations are visible;
- documentation calls the project an engineering preview and explains that cleanup without backup is irreversible; and
- no downloadable application is described as supported or trusted merely because the source is public.

## Mutation-capable binary gate

No alpha, beta, or supported binary may promote cleanup mutations until M1 in the [development roadmap](roadmap.md#m1--codex-safety-hardening) passes. In particular:

- every direct-file and official-API mutation revalidates capability, path, identity, ownership, protected descendants, and active-writer state;
- a selected backup is independently reopened and verified before mutation;
- restore preflights all destinations and is all-or-nothing;
- every irreversible boundary has deterministic fault-injection coverage;
- interrupted operations have an explainable, recoverable journal state; and
- unknown capabilities or schemas remain visible but read-only.

If this gate is incomplete, CleanerX may remain a source preview but must not promote a mutation-capable download.

## Unsigned-artifact policy

Every unsigned artifact name, release page, and installation section must state that:

- the application is unsigned and not notarized;
- macOS cannot verify the publisher identity;
- a checksum verifies that a download matches the published asset but does not substitute for signing;
- the supported opening path is Finder **Open** or approval in **System Settings → Privacy & Security**;
- users must not be told to disable Gatekeeper globally or run broad quarantine-removal commands; and
- building from source remains available for users who do not accept an unsigned binary.

An artifact is released from its exact tested tag. An earlier build is never promoted by renaming it, and the application does not silently update itself.

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

Each tag has a separate checklist recording version synchronization, private-data scanning, dependency/license review, `make check`, safety and fault-injection results, native smoke tests, artifact/checksum verification, compatibility limits, unsigned-build disclosure, and release-note review. The checklist records evidence for that tag; it does not become another backlog.
