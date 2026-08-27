# CleanerX documentation

- [Development plan](roadmap.md): milestones, priorities, exit criteria, and non-goals.
- [Storage and transaction model](storage-model.md): discovery, data categories, cleanup transactions, and restore rules.
- [Agent session hierarchy](agent-session-hierarchy.md): the cross-Agent project/session tree model and supporting evidence.
- [Agent memory research](memory-management.md): native memory capabilities, editing boundaries, and the multi-Agent delivery plan.
- [Security policy](../SECURITY.md): product safety invariants and vulnerability reporting.
- [Contributor guide](../CONTRIBUTING.md): contribution and validation requirements.
- [Agent instructions](../AGENTS.md): repository-wide constraints for coding agents.

Keep implementation-specific decisions close to the owning document. Avoid duplicating safety requirements with different wording; `SECURITY.md` and `AGENTS.md` are normative for product and development behavior respectively.
