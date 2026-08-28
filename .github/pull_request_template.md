## Summary

Describe the user-visible or safety behavior changed by this pull request.

## Safety boundary

- [ ] This change does not scan or mutate project/source directories.
- [ ] Authentication, configuration, MCP credentials, rules, skills, plugins, browser accounts, cookies, and source code remain protected.
- [ ] The webview receives only purpose-specific commands and no general shell or filesystem access.
- [ ] Active writers are blocked without force-quitting another process.
- [ ] Any new or changed mutation route includes preflight, backup/restore, journal, fault-injection, and post-operation verification coverage.

## Verification

- [ ] `make check` passes.
- [ ] New schemas, categories, or mutation behavior include temporary fixtures and negative-path tests.
- [ ] Safety tests prove protected fixture bytes and source trees remain unchanged.
- [ ] Frontend behavior changes include applicable blocker, confirmation, error, i18n, selection, and keyboard-accessibility coverage.
- [ ] Platform behavior includes an abstraction-level test and any required native smoke evidence.

## Documentation

- [ ] Relevant storage, hierarchy, roadmap, security, and user-facing documentation is updated.
- [ ] No private transcripts, credentials, memory data, logs, journals, `.cxb` archives, databases, or real local paths are included.
