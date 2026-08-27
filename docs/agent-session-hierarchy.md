# Agent session hierarchy

CleanerX uses a common **forest** model for storage navigation:

```text
Agent
└── Verified project / repository root
    ├── Root session
    │   ├── Branch, fork, or child session
    │   └── Subagent session
    └── Another root session
```

A project can contain multiple independent root sessions, so this is a forest rather than a strict single tree. Sessions with no known parent stay at the project root. Unknown or missing project associations are kept under an explicit “No project” virtual root instead of being discarded. This node is presentation-only: it is not written to the Agent's project registry and selecting it selects only visible, eligible session data.

## Evidence from supported and planned agents

| Agent | Official hierarchy signals | CleanerX mapping |
| --- | --- | --- |
| Codex | App Server `thread/list` exposes `parentThreadId` / `ancestorThreadId`; `thread/delete` removes a thread and its spawned descendants. | Project root → root thread → child/subagent threads. |
| Claude Code | Sessions are associated with a project bucket. Forked sessions have independent IDs; subagent transcripts are stored beneath their parent session directory. | Project bucket → independent root sessions. Forks remain sibling roots because the documented transcript metadata does not expose a stable parent-session ID; subagent files are included in the owning session's cleanup impact rather than invented as selectable sessions. |
| OpenCode | The recognized official SQLite schema records `project_id` and `parent_id`; the public server exposes child-session and fork routes, and official deletion recursively removes children. | Official project record → root session → child/fork sessions. The project `worktree` and session `directory` are grouping metadata only and are never scanned. |
| Pi | Session JSONL entries use `id` and `parentId`, and session files can reference a `parentSession`; `/tree`, `/fork`, and `/clone` expose branching. | Project root → session file lineage. Message-entry branching is intentionally not shown in CleanerX MVP. |

Official references:

- [Codex App Server protocol](https://learn.chatgpt.com/docs/app-server)
- [Codex projects and chats](https://learn.chatgpt.com/docs/projects)
- [Claude Code sessions](https://code.claude.com/docs/en/sessions)
- [Claude Code subagents](https://code.claude.com/docs/en/sub-agents)
- [OpenCode agents](https://opencode.ai/docs/agents/)
- [OpenCode server session APIs](https://dev.opencode.ai/docs/server/)
- [Pi session format](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/session-format.md)
- [Pi session navigation](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/sessions.md)

## Product decisions

- Tree view is the default on the Sessions page. Project roots are grouping nodes there; CleanerX does not maintain a duplicate Projects page.
- Sessions page retains a flat list toggle for sorting, comparison, and large-result workflows.
- “Recent” is an updated-time filter (currently last 7 or 30 days), not a durable tree node. Its membership changes over time and never creates or changes a project association.
- Filtering a child keeps its ancestor rows as non-selectable context.
- Project selection operates only on associated Agent data; it never selects or traverses the source directory.
- Session title, `cwd`, and project association remain independent. A title is never synthesized from the `cwd` directory name, and a title does not establish project membership.
- A session `cwd` is shown as recognition metadata but does not by itself create a project association. CleanerX requires a recognized Codex project root or an ancestor Git marker; standalone desktop chat workspaces therefore remain under “No project.”
- For Claude Code, the documented `projects/<project>/` bucket is positive association evidence. CleanerX groups every recognized UUID transcript in that bucket together, records absolute transcript `cwd` values as display roots, and never traverses or mutates those roots. Sessions without a usable `cwd` may still inherit the verified bucket group.
- For OpenCode, a recognized `session.project_id` joined to the official `project` table is positive association evidence. CleanerX uses `parent_id` transitively, detects cycles, and includes every known descendant in the review and encrypted export set while invoking official deletion only for the minimal roots. A verified loopback Server API status map applies to the full expanded tree, so an active descendant blocks deletion of its ancestor.
- The “No project” virtual root sorts sessions by most recent update. A known `cwd` may be shown only as recognition metadata; an absent `cwd` is displayed explicitly and is never inferred from CleanerX's working directory.
- A parent cleanup remains explicit. The confirmation plan expands and displays every descendant that the Agent's official delete operation will also remove.
- CleanerX does not inspect message bodies while building the hierarchy. Codex content can be loaded on demand in a bounded detail view; Agent-specific entry-level branching, such as Pi's internal message tree, remains out of scope for the MVP hierarchy.
