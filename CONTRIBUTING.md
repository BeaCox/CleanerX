# Contributing to CleanerX

Thank you for helping improve CleanerX. Contributions are accepted under the repository's [Apache License 2.0](LICENSE).

CleanerX deletes private local Agent data, so a change is judged first by whether its cleanup boundary can be understood, tested, and recovered—not by how much functionality it adds. The repository-wide [AGENTS.md](AGENTS.md) rules are normative for both human- and agent-assisted changes.

## Before you start

- Search existing issues and the [development roadmap](docs/roadmap.md) before opening parallel work.
- Use the product bug, compatibility, or bounded read-only improvement issue template and include sanitized metadata only.
- Open an issue for design discussion before implementing a new Agent, storage schema, cleanup category, mutation route, background behavior, or safety-boundary change.
- Report possible protected-data mutation, path escape, credential exposure, arbitrary command execution, or backup/restore compromise through a [private security advisory](https://github.com/BeaCox/CleanerX/security/advisories/new), not a public issue.

Good first contributions are usually bounded read-only inventory fixes, accessibility and keyboard improvements, localization corrections, documentation, or tests that do not widen a mutation path.

## Development setup

Install the prerequisites listed in [README.md](README.md), then use the root `Makefile` as the stable entry point:

```bash
make setup
make dev
```

Run the complete repository gate before submitting a pull request:

```bash
make check
```

`make check` runs Rust formatting checks, Clippy with warnings denied, Rust tests, frontend tests, and the production frontend build. Platform-specific changes also need the relevant native smoke test described in the README and roadmap.

Do not edit generated files in `dist/`, `target/`, or `node_modules/`, and do not include real Agent data or machine-specific paths in fixtures.

## Change requirements

| Change | Required evidence |
| --- | --- |
| Storage schema or category | Temporary recognized and unrecognized fixtures, bounded parsing, and negative-path coverage |
| Mutation route | Preflight, optional backup or an explicit irreversible state, restore where supported, journal boundaries, fault injection, and post-operation rescan |
| Direct file operation | Fixed category root, traversal/link/ownership/identity checks, and proof that protected bytes and source trees remain unchanged |
| App Server or Agent transport | Initialization/capability downgrade, transport failure, pagination, active/archived/pinned state, and descendant coverage as applicable |
| Frontend behavior | Testing Library coverage for selection, blockers, confirmation, errors, i18n, and keyboard accessibility as applicable |
| Platform behavior | An abstraction-level test runnable across hosts plus a native smoke test on the affected platform |
| Documentation or compatibility claim | Link to the public Agent interface when available; label reverse-engineered behavior read-only |

Private SQLite writes, guessed deletion paths, dynamic cleanup plugins, general shell/filesystem access, recursive project scanning, forced process termination, and mutation without a complete verification path are not accepted as compatibility fallbacks.

## Architecture boundaries

- `crates/cleanerx-core` owns domain types, plans, path safety, backup/restore, hashing, and transaction invariants. It stays independent of Tauri and concrete Agents.
- Adapter crates own Agent discovery, recognized storage, public mutation routes, bounded read-only fallbacks, and protected-path classification.
- `src-tauri` exposes purpose-specific commands, accepts opaque item identifiers, and revalidates paths and snapshot ownership on the Rust side.
- `src` owns presentation and interaction. It must not infer write safety from a discovered path or disabled backend gate.

New Agents implement the compile-time `AgentAdapter` trait. CleanerX does not provide a dynamic adapter ABI for cleanup code.

## Documentation ownership

Keep the owning document current in the same pull request:

- discovery, classification, cleanup, backup, journal, or restore behavior → [docs/storage-model.md](docs/storage-model.md)
- mutation routes and evidence → [docs/compatibility.md](docs/compatibility.md)
- hierarchy semantics → [docs/agent-session-hierarchy.md](docs/agent-session-hierarchy.md)
- memory semantics → [docs/memory-management.md](docs/memory-management.md)
- unfinished work or milestone status → [docs/roadmap.md](docs/roadmap.md)
- threat-model or safety-boundary changes → [SECURITY.md](SECURITY.md)

The roadmap is the only backlog. Do not create parallel phase lists in design or policy documents.

## Pull requests

Keep each pull request narrowly scoped and explain the user-visible behavior and safety consequences. Complete the repository pull request template, including:

- the exact cleanup boundary affected, or confirmation that no mutation boundary changed;
- tests and native evidence run;
- read-only degradation and error behavior;
- documentation updated; and
- confirmation that no transcript, memory, log, credential, journal, backup, database, real path, or other private material is included.

A change is complete only when relevant behavior is implemented, the required tests and `make check` pass, safety degradation is explicit in the UI, documentation is current, and any requested artifact has been rebuilt.
