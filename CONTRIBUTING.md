# Contributing

Contributions are welcome under Apache-2.0.

Before submitting a change:

1. Keep every adapter behind the compile-time `AgentAdapter` trait.
2. Do not add general Shell or filesystem access to the webview.
3. Add a fixture for every new storage schema, category or mutation path.
4. Prove protected fixture bytes remain unchanged after the operation.
5. Run `make check`, which covers Rust formatting, Clippy, Rust tests, frontend tests, and the frontend production build.

Protocol changes must link to an official Agent interface or be read-only. Private database writes are not accepted as a compatibility fallback.

Repository-wide coding-agent constraints are defined in [AGENTS.md](AGENTS.md). The current implementation sequence and milestone exit criteria are in [docs/roadmap.md](docs/roadmap.md).
