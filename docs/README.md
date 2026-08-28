# CleanerX documentation

- [Development roadmap](roadmap.md): the single source of truth for all unfinished work, milestones, priorities, and exit criteria.
- [Mutation compatibility matrix](compatibility.md): enabled mutation routes, runtime gates, backup/restore support, and current test evidence for each adapter.
- [Open-source release policy](open-source-release-plan.md): source publication states, mutation-safety gates, unsigned-artifact disclosure, and maintenance policy; it does not carry a separate backlog.
- [Storage and transaction model](storage-model.md): discovery, data categories, cleanup transactions, and restore rules.
- [Agent session hierarchy](agent-session-hierarchy.md): the cross-Agent project/session tree model and supporting evidence.
- [Agent memory capability and safety model](memory-management.md): native memory capabilities, editing permissions, safety boundaries, and acceptance criteria.
- [Security policy](../SECURITY.md): product safety invariants and vulnerability reporting.
- [Contributor guide](../CONTRIBUTING.md): contribution and validation requirements.
- [Agent instructions](../AGENTS.md): repository-wide constraints for coding agents.

Keep implementation-specific decisions close to the owning document, but put every unfinished task only in `roadmap.md`. Other documents describe current behavior, decisions, policies, and acceptance criteria and link to the roadmap instead of maintaining parallel phase lists. Avoid duplicating safety requirements with different wording; `SECURITY.md` and `AGENTS.md` are normative for product and development behavior respectively.
