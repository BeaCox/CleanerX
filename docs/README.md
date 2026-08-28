# CleanerX documentation

This directory contains the detailed product, compatibility, architecture, and release documents that would make the root README too difficult to scan. CleanerX deletes private local data, so documentation distinguishes current verified behavior from planned work and never treats discovery alone as permission to mutate.

## Choose a document

| If you want to understand… | Read |
| --- | --- |
| what CleanerX can currently delete, how, and with what recovery support | [Mutation compatibility matrix](compatibility.md) |
| how data is discovered, classified, planned, backed up, deleted, restored, and recovered | [Storage and transaction model](storage-model.md) |
| how projects, root sessions, descendants, forks, and unlinked sessions map across Agents | [Agent session hierarchy](agent-session-hierarchy.md) |
| what “memory” means for each Agent and which content remains protected | [Agent memory capability and safety model](memory-management.md) |
| what remains unfinished and what blocks a supported release | [Development roadmap](roadmap.md) |
| how source preview, alpha, beta, unsigned artifacts, and `v0.1.0` are gated | [Open-source release policy](open-source-release-plan.md) |
| how application update checks, signatures, platform packages, and release feeds work | [Application update strategy](update-strategy.md) |
| what changed in the first alpha and which release checks remain | [`v0.1.0-alpha.1` release checklist](releases/v0.1.0-alpha.1.md) and the [changelog](../CHANGELOG.md) |
| the security boundary or how to report a vulnerability | [Security policy](../SECURITY.md) |
| how to prepare and verify a contribution | [Contributor guide](../CONTRIBUTING.md) |
| repository-wide constraints for coding agents | [Agent instructions](../AGENTS.md) |

## Document authority

- [SECURITY.md](../SECURITY.md) is normative for the product threat model and protected-data boundary.
- [AGENTS.md](../AGENTS.md) is normative for development, architecture, UI, testing, and definition-of-done constraints.
- [compatibility.md](compatibility.md) records mutation routes that are implemented and qualified by current automated evidence. It is not a promise that every Agent version works.
- [roadmap.md](roadmap.md) is the only source of unfinished tasks, priorities, milestones, and exit criteria.
- Other documents describe current behavior and decisions. They link to the roadmap instead of carrying parallel implementation plans.

When documents appear to conflict, use the stricter safety boundary and correct the inconsistency in the same change.

## Maintenance rules

- Link compatibility decisions to an official Agent interface whenever one is available.
- Clearly label reverse-engineered schemas or behavior as read-only.
- Describe capability downgrade and user-visible blockers alongside the happy path.
- Keep examples synthetic; never include real transcripts, memories, logs, credentials, journals, backups, state databases, IDs, or local machine paths.
- Update the owning document in the same pull request as a behavior change.
- Update the `Last reviewed` date only when the document's claims were actually checked, not for unrelated formatting edits.
