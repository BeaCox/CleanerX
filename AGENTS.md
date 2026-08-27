# CleanerX agent instructions

These instructions apply to the entire repository. CleanerX deletes private local Agent data, so safety requirements take precedence over convenience, compatibility, or UI polish.

## Product invariants

- CleanerX is local-only. Do not add telemetry, crash upload, cloud synchronization, an updater, or a background daemon without an explicit product decision.
- Never recursively scan or mutate a source/project directory. A project path is grouping metadata, not a cleanup root.
- Never target authentication, configuration, MCP credentials, rules, skills, plugins, cookies, browser accounts, or source code.
- Never expose a general shell or unrestricted filesystem API to the webview.
- Never force-quit Codex or another process. Detect active writers, explain the blocker, and let the user retry.
- A backup is an explicit user choice. When selected, it must be verified and atomically committed before mutation starts; without one, the review must clearly state that cleanup is irreversible.

## Architecture boundaries

- `crates/cleanerx-core` owns domain types, cleanup planning, path validation, backup/restore, hashing, and transaction invariants. It must not depend on Tauri or a concrete Agent.
- `crates/adapter-codex` owns Codex discovery, capability probing, App Server transport, Codex storage classification, and read-only compatibility fallbacks.
- Future adapters implement the compile-time `AgentAdapter` trait. Do not add a dynamic plugin ABI for the MVP.
- `src-tauri` is the narrow application boundary. Expose only purpose-specific commands and validate all identifiers and paths again on the Rust side.
- `src` owns presentation and interaction. It must not infer that an unavailable control is safe merely because a path was discovered.

## Codex integration rules

- Prefer public, documented Codex App Server methods for session mutations. Complete the `initialize` request and `initialized` notification handshake.
- A stale or unresponsive control socket should fall back to an isolated stdio App Server. Failure of both transports makes session operations read-only.
- `thread/delete` is the supported session deletion route. Never repair or delete sessions through private SQLite writes.
- Treat `memory/reset` as an independently probed capability. Its absence disables memory reset only; it must not disable otherwise supported session cleanup.
- State databases may supplement scans only when their schema is recognized. Unknown schemas are read-only and must not block a filesystem inventory.
- Inventory scans must not load or retain transcript, memory, or log bodies. A recognized adapter may retain one normalized, strictly bounded first-user-message excerpt only when the Agent itself uses that excerpt as the unnamed session's display title; it must not retain any additional body content. Other content is loaded only after an explicit detail action, through a purpose-specific read-only command scoped to an item in the current snapshot. The media gallery may request one bounded image thumbnail for each visible attachment/generated item through its own narrow command; it must never load text, logs, or additional files for the card. Enforce recognized schemas, fixed roots, symlink rejection, bounded previews, and clear detail content when the detail view closes.

## Mutation and path safety

- Every direct file operation must resolve beneath a fixed, category-specific allowlisted root.
- Reject symbolic links, lexical traversal, ownership anomalies, protected descendants, and file identity changes between planning and execution.
- Session cleanup must expand descendants, show them in the review plan, and call the official deletion operation only for the minimal root set. If backup is selected, back up the full expanded set together.
- Delete allowlisted attachments only after the owning session deletion succeeds. Rescan to verify the result.
- Restoration is all-or-nothing: verify manifest hashes and preflight every destination before the first move. Never overwrite an existing ID or path.
- Keep transaction journal transitions atomic and recoverable. New mutation steps require fault-injection coverage for every boundary before and after irreversible work.

## UI and information architecture

- “Sessions” is the only navigation entry for project-associated session data. In its default tree, project roots group root sessions and descendants; do not reintroduce a duplicate Projects page.
- Keep the filtered flat list as an alternate session view.
- Filtering a child must retain its ancestors as non-selectable context.
- Bulk selection applies only to the visible/current scope and must skip blocked or protected items. Present one left-aligned select-all/deselect-all toggle, not separate buttons. Preserve `Cmd/Ctrl+A` inside text inputs for normal text selection.
- In Sessions, keep search and filters in one aligned row, then place bulk selection, view switching, and expand/collapse controls in one action row. Project group rows must use the same table columns as session rows so source, updated time, and size never drift. Long names and metadata must ellipsize within their cell rather than overlap adjacent columns.
- No cleanup item is automatically selected. Pinned and loaded sessions remain unavailable for selection where required by the mutation safety rules.
- Backup is optional and off by default. An unchecked backup control must not block cleanup by itself; show one concise irreversible-deletion warning instead.
- Open item details by activating the row or card itself; do not add a redundant view-details icon or button. Embedded selection and disclosure controls must not open details.
- Tree expansion uses one expand-all/collapse-all toggle in the existing filter toolbar; never add a separate tree-controls row. Media without a supported thumbnail uses an icon-only placeholder without visible explanatory copy.
- Keep Chinese and English translations in sync. Use system theme, keyboard-accessible controls, visible focus states, and reduced-motion preferences.
- Locale and theme choices preview immediately, persist only after a successful settings write, apply before the first useful paint from a local preference cache, and follow OS changes while set to system. Unsupported persisted values must be rejected at the Rust boundary.
- Avoid microtype for operational data. Body/table/control text should normally be at least 12 px; auxiliary monospace metadata may be 10–11 px when contrast remains accessible.

## Design system: field-manual console

CleanerX uses a flat technical-instrument aesthetic ("field manual console") because it performs irreversible deletion of private data. Preserve it; do not reintroduce consumer-SaaS/dashboard styling.

### Window frame (fixed, do not regress)

- No sidebar and no page hero titles. The frame is a 3-part chrome shell: a 46 px top toolbar (brand block, horizontal view tabs with a 2 px ink underline for the active tab, scan action), a full-bleed scrolling content region, and a 30 px bottom status deck.
- The active tab is the view title; keep exactly one visually hidden `h1` per view for assistive tech (`.visually-hidden`). Never render a visible in-page heading that repeats the active tab label.
- The status deck always shows the agent environment (connection dot + Codex version) on the left. The right side shows inventory totals + last-scan time, and swaps to the selection summary + clear/review actions while a selection exists. Selection UI must never float above content or appear as a separate bar.
- Content is full-width with modest gutters; no centered max-width column for data views. Settings use the available width as responsive columns and collapse to one column on narrow windows.

### Skin

- Warm paper/ink palette only (light: paper `#f4f1ea`, ink `#23201a`; dark: warm lamp-black). Primary actions are solid ink buttons (inverted in dark mode); destructive actions are solid red. The cobalt `--focus` is reserved for focus rings and inline status accents. No brand-colored CTAs and no colored button shadows.
- No gradients, glows, glassmorphism/backdrop blur, decorative orbs, or floating pills. Surfaces are separated by 1 px hairline borders; shadows (`--shadow-overlay`) only on transient overlays (dialogs, drawers, toasts).
- Corner radii stay sharp: 3 px badges/checkboxes, 6 px controls, 8 px cards, 10 px dialogs (`--radius-s/m/l/xl`). No pill-shaped buttons.
- Operational data — paths, sizes, counts, IDs, timestamps, durations, source labels, badges, table headers, status-deck text — is set in `ui-monospace` (`--mono`) at 10–12 px with tabular numerals. Prose and control labels use the system sans at ≥12 px.
- Category colors are muted earthy mid-tone hues that stay legible on both light and dark themes; they never appear as pastel fills for emphasis. Status is conveyed by text plus a small dot or icon, never by color alone.
- Group content with hairline separators and small monospace group labels (`.settings-group-label`, `.panel-heading`). Do not wrap forms or simple lists in boxed cards with large padding; a settings page is a flat stack of hairline rows.
- Summaries are flat stat strips (`.stat-strip`): divided cells in one bordered row, no per-stat icon cards.
- Theme tokens live at the top of `src/styles.css` for light, dark, and `prefers-color-scheme` fallback. New components must consume the existing custom properties instead of inventing hex values.

## Development workflow

- Use the root `Makefile` as the stable entry point. Tool-specific commands remain implemented by Cargo, pnpm, and Tauri underneath it.
- Run `make check` before handing off a code change. It covers Rust formatting, Clippy, Rust tests, frontend tests, and the frontend production build.
- Use `make dev` for hot-reload development, `make app` for a local unsigned `.app`, and `make bundles` for `.app` plus DMG release artifacts.
- Do not edit generated content in `dist/`, `target/`, or `node_modules/`. Change sources and regenerate.
- Keep lockfiles committed and avoid unrelated dependency upgrades.
- Preserve user changes in a dirty worktree and keep edits scoped to the requested task.

## Test requirements

- New storage schemas, categories, or mutation paths require temporary fixtures and negative-path tests.
- Safety tests must prove protected fixture bytes and source trees remain unchanged.
- App Server changes require transport failure, capability downgrade, pagination, active/archived/pinned, and descendant coverage.
- Frontend behavior changes require Testing Library coverage for selection, blockers, confirmation, errors, i18n, and keyboard accessibility as applicable.
- Platform-specific behavior must have an abstraction-level test runnable on Windows and Linux, plus a native smoke test on the affected platform.
- Live tests must use an isolated temporary `CODEX_HOME` unless they are explicitly ignored, read-only diagnostics.

## Documentation ownership

- Update `docs/storage-model.md` when discovery, classification, mutation, backup, journal, or restore behavior changes.
- Update `docs/agent-session-hierarchy.md` when an adapter changes hierarchy semantics.
- Update `docs/roadmap.md` when milestone scope or status changes.
- Update `SECURITY.md` for any safety-boundary or threat-model change.
- Link compatibility decisions to official Agent documentation whenever available, and clearly label reverse-engineered behavior as read-only.

## Definition of done

A change is complete only when its behavior is implemented, relevant tests pass, `make check` passes, safety degradation is explicit in the GUI, documentation is current, and any requested artifact is rebuilt. Do not mark destructive functionality complete if its backup, crash recovery, or post-operation verification path is missing.
