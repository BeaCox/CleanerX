# Agent memory research and implementation plan

Status: shared capability model and project-level Claude Code cleanup implemented; entry editing remains planned. Researched and updated 2026-08-27.

## Terminology

CleanerX must keep three kinds of persistent context separate:

1. **Automatic memory** is Agent-generated recall carried between sessions. This is the only category that belongs on the Memory page.
2. **Persistent instructions** are user- or team-authored rules such as `AGENTS.md`, `CLAUDE.md`, OpenCode rules, and Pi context files. They remain protected configuration and are never changed by memory cleanup.
3. **Session history** is the transcript and its descendants. It remains on the Sessions page even when an Agent later derives memory from it.

Deleting a session does not imply that already consolidated memory is forgotten. Project cleanup also never selects global memory automatically.

## Capability matrix

| Agent | Native automatic memory | Storage and scope | Supported control surface | CleanerX decision |
| --- | --- | --- | --- | --- |
| Codex | Yes | Local Codex Home, primarily `memories/`; consolidated state is global to the local host | `/memories` controls use/generation per chat. The documented App Server has no entry-level memory CRUD API. The locally observed Codex 0.145.0 schema exposes capability-probed `memory/reset` and chat memory-mode control, but no list/get/update/delete methods. | Scan metadata, load bounded details on demand, and offer global reset only when the runtime reports the capability. Do not edit generated files or private SQLite. |
| Claude Code | Yes | `~/.claude/projects/<project>/memory/`, one repository-scoped directory shared by its worktrees; `MEMORY.md` indexes topic Markdown files | Official documentation says auto-memory Markdown files may be edited or deleted at any time and `/memory` opens them. | Scan bounded metadata/content and delete a selected project's complete auto-memory directory through the verified file transaction. Entry editing remains disabled. Treat `CLAUDE.md`, `CLAUDE.local.md`, and `.claude/rules/` as protected instructions, not memory. |
| OpenCode | No native automatic-memory surface documented | Official persistence is instruction files such as project/global `AGENTS.md`; sessions are separate | The open native auto-memory proposal explicitly describes cross-session learning as absent today. | Do not present rules as memory or add a reset/editor. Detect a future native capability before enabling one. |
| Pi | No native automatic-memory surface documented | Official persistent context is `AGENTS.md`/`CLAUDE.md`, system-prompt files, sessions, and extension-owned data | Extensions can implement arbitrary storage and UI, so there is no single core memory schema to manage safely. | Keep core instruction files protected. Support memory only through a future adapter for a specific, recognized extension and schema. |

The OpenCode and Pi “no native automatic memory” entries are inferences from their current official feature documentation, reinforced for OpenCode by its open native-memory proposal. They must be rechecked when implementing those adapters.

## Evidence and compatibility notes

### Codex

Official Codex documentation distinguishes its local store from ChatGPT web memory, locates local memories beneath Codex Home, and describes the contents as generated state that may be inspected but should not be manually edited as the primary control surface. Required team guidance belongs in `AGENTS.md`, not memory.

The stable [Codex App Server API overview](https://learn.chatgpt.com/docs/app-server) documents thread operations but currently does not document memory CRUD or reset. CleanerX therefore treats the locally observed `memory/reset` method as an optional runtime capability, not a version guarantee. Its absence disables only reset; scanning, bounded detail reads, and session cleanup continue independently.

Sources:

- [Codex memories](https://learn.chatgpt.com/docs/customization/memories)
- [Codex App Server](https://learn.chatgpt.com/docs/app-server)
- [Codex AGENTS.md guidance](https://learn.chatgpt.com/docs/agent-configuration/agents-md)

### Claude Code

Claude Code explicitly separates user-authored instructions from auto memory. Auto memory uses a per-repository directory containing a concise `MEMORY.md` index plus topic files. Topic files can carry `type` and `modified` frontmatter; recognized types currently include `user`, `feedback`, `project`, and `reference`. The official documentation permits users to edit or delete these Markdown files.

This makes Claude Code suitable for an editor, but not for blind file manipulation. CleanerX must preserve unknown frontmatter, keep the index and topic files consistent, and avoid all instruction/configuration paths.

Source: [Claude Code memory documentation](https://code.claude.com/docs/en/memory)

### OpenCode

OpenCode documents `AGENTS.md` and optional instruction files as rules. These are user-authored configuration, not automatic memory, and remain protected. CleanerX should not infer a memory feature from the presence of a file named `MEMORY.md` because it may be user or plugin data.

Sources:

- [OpenCode rules](https://opencode.ai/docs/rules/)
- [OpenCode native auto-memory proposal](https://github.com/anomalyco/opencode/issues/20322)

### Pi

Pi documents context files, system-prompt files, sessions, compaction, and an extension mechanism. Since an extension can define arbitrary memory storage, ownership and mutation semantics cannot be inferred from the Pi home directory alone.

Source: [Pi coding-agent README](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/README.md)

## Core capability model

The former `memory_reset` boolean has been replaced with this explicit compile-time capability description:

```rust
pub struct MemoryCapabilities {
    pub can_scan: bool,
    pub can_read_content: bool,
    pub can_reset_all: bool,
    pub can_reset_scope: bool,
    pub can_edit_entries: bool,
    pub can_delete_entries: bool,
    pub can_toggle_use: bool,
    pub can_toggle_generation: bool,
    pub scope: MemoryScope,
}

pub enum MemoryScope {
    Global,
    Project,
    Mixed,
}
```

An inventory entry also needs a stable adapter-owned ID, scope, optional project association, recognized kind, modified time, byte size, source revision/hash, and mutation route. The webview receives IDs, never arbitrary paths.

## Product behavior

- The Memory page renders only capabilities the detected adapter actually supports.
- Content is loaded only after an explicit detail action and cleared when the detail view closes. Inventory scanning retains metadata, not memory bodies.
- Codex shows a global memory object with inspect/reset behavior. It must explain that reliable per-project deletion is unavailable after consolidation.
- Claude Code currently shows one cleanup item per project memory directory with bounded Markdown details. Deletion shows and backs up the complete affected index/topic-file set. A future entry editor will add individual topic entries and a Markdown-aware before/after diff.
- OpenCode and Pi show no native memory editor until a recognized native or extension-specific capability exists.
- Memory use/generation toggles are settings, not cleanup selections. They must never be silently changed as a side effect of deletion.
- Instructions remain in a separate protected category and do not appear as editable memory even when an Agent's own UI calls them “memory files.”

## Mutation safety

All memory mutations follow the repository safety invariants:

1. Resolve the item from the current snapshot beneath an Agent-specific memory root; reject user-supplied paths, symlinks, traversal, ownership anomalies, and changed file identities.
2. Require the Agent and other detected writers to exit. CleanerX never force-quits them.
3. Create and verify an atomically committed encrypted backup before changing recoverable memory data.
4. Revalidate the source revision immediately before mutation. A stale editor must fail rather than overwrite newer Agent output.
5. For file-backed memory, write to a sibling temporary file, fsync, validate Markdown/frontmatter, and atomically rename. A multi-file index/topic change is journaled and all-or-nothing.
6. Rescan after mutation and verify the expected entry/hash state. Unknown schemas or capabilities degrade to read-only.

Codex never receives a direct file or SQLite write fallback. Claude Code editing is allowed only because the Agent's official documentation defines those auto-memory files as user-editable plain Markdown.

## Delivery plan

### Phase 1 — Codex memory hardening

- Keep bounded content rendering and separate `memory/reset` capability probing.
- Add versioned fixtures for memory directories, recognized databases, unknown schemas, concurrent writers, and reset failures.
- Complete backup, journal, reset, rescan, and restore fault-injection coverage.
- Keep per-project editing disabled.

### Phase 2 — Shared memory domain model (complete)

- Add `MemoryCapabilities`, `MemoryScope`, `MemoryEntry`, and purpose-specific adapter methods.
- Keep Tauri commands ID-based: list from the current snapshot, read one entry, prepare an edit/delete plan, execute, and verify.
- Add capability-driven GUI states and synchronized Chinese/English copy.

### Phase 3 — Claude Code project cleanup complete; entry editor pending

- Detect the default auto-memory root beneath Claude Code Home without treating general Claude configuration as cleanup data. External `autoMemoryDirectory` targets remain unsupported because they are outside the fixed cleanup root.
- Parse `MEMORY.md` plus topic Markdown only for bounded, explicit detail reads; project cleanup preserves bytes in the optional encrypted backup.
- Implement entry edit/delete and project reset with optimistic concurrency, encrypted backup, atomic multi-file journaling, and post-write verification.
- Add fixtures for repository/worktree sharing, custom roots, missing indexes, duplicate links, malformed frontmatter, symlinks, permission failures, and concurrent edits.

### Phase 4 — Re-evaluate OpenCode and Pi

- Recheck official releases for a native memory API/schema.
- If Pi support depends on an extension, identify the extension and exact version at compile time; unknown extension data remains untouched.
- Never reinterpret rules, prompts, or system files as automatic memory merely to fill the UI.

## Acceptance criteria for editing

- Codex memory remains read/reset-only unless a public entry-level API becomes available.
- Claude edits round-trip without losing unknown frontmatter or unrelated topic files.
- A stale source hash, running writer, unverified backup, unknown schema, or path-safety failure prevents the first mutation.
- Restore reproduces every original byte and the Agent can load the restored memory afterward.
- Rules, instructions, auth, configuration, skills, plugins, sessions, and source directories remain byte-identical during memory-only operations.
