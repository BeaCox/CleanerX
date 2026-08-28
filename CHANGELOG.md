# Changelog

All notable CleanerX changes are recorded here. CleanerX follows Semantic Versioning, but pre-1.0 releases may intentionally narrow or disable a cleanup capability when its safety boundary can no longer be verified.

## [Unreleased]

## [0.1.0-alpha.1] - 2026-08-28

Initial engineering-preview release for bounded cross-platform and Agent compatibility testing.

### Added

- Metadata-first inventory and cleanup review for Codex, Claude Code, OpenCode, and pi.
- Project-grouped session trees, filtered flat sessions, bounded item details, and supported media thumbnails.
- Capability-gated session, memory, log, cache, temporary-data, and recognized direct-file cleanup routes.
- Optional age-encrypted backups for complete plans with a supported restore route.
- Atomic operation journal, startup recovery, post-mutation rescans, and all-or-nothing restore preflight and rollback.
- Unsigned macOS, Linux, and Windows packaging with SHA-256 checksums and build metadata.
- User-initiated Tauri updates with a pinned public key and signed updater artifacts.

### Safety boundary

- Project and source paths are grouping metadata only and are never recursive cleanup roots.
- Authentication, configuration, MCP credentials, rules, skills, plugins, browser accounts, cookies, and source code remain protected.
- Unsupported capabilities, unknown schemas, active writers, redirects, ownership anomalies, and changed file identities fail closed.
- Nothing is selected automatically; cleanup without an available and selected backup is irreversible.

### Known limitations

- This is an unsigned, non-notarized alpha. Gatekeeper and Windows SmartScreen may warn because operating-system publisher identity is not established.
- Codex session deletion and global memory reset are irreversible because Codex exposes no supported import route.
- Orphaned Codex media is inspect-only; session-owned media is removed only after the owning official session deletion succeeds.
- Agent and storage compatibility is capability-gated, not guaranteed for every historical or future Agent version.
- The stable updater feed excludes GitHub prereleases, so `v0.1.0-alpha.1` is installed manually and is not offered by CleanerX's in-app update check.
- Linux in-app updates support AppImage only; `.deb` installations update through their original distribution channel or a manual release download.
- Broader native disposable mutation/restore cycles, accessibility acceptance, and external pilot evidence remain prerequisites for `v0.1.0`.

[Unreleased]: https://github.com/BeaCox/CleanerX/compare/v0.1.0-alpha.1...HEAD
[0.1.0-alpha.1]: https://github.com/BeaCox/CleanerX/releases/tag/v0.1.0-alpha.1
