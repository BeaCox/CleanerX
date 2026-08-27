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
| Claude Code | Sessions are associated with a project directory. Forked sessions have independent IDs; subagent transcripts are stored beneath their parent session directory. | Project root → session family → fork/subagent transcript. Independent sessions remain sibling roots. |
| OpenCode | Sessions accept a `parentID`, expose a children endpoint, and support forking. The UI provides parent/child/sibling navigation for subagent sessions. | Project root → root session → child/fork sessions. |
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
- The “No project” virtual root sorts sessions by most recent update. A known `cwd` may be shown only as recognition metadata; an absent `cwd` is displayed explicitly and is never inferred from CleanerX's working directory.
- A parent cleanup remains explicit. The confirmation plan expands and displays every descendant that the Agent's official delete operation will also remove.
- CleanerX does not inspect message bodies while building the hierarchy. Codex content can be loaded on demand in a bounded detail view; Agent-specific entry-level branching, such as Pi's internal message tree, remains out of scope for the MVP hierarchy.
