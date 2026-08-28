# Application update strategy

Status: implemented for tagged desktop releases

Last reviewed: 2026-08-28

## Research summary

Current Tauri v2 applications generally use one of three distribution patterns:

1. The official `tauri-plugin-updater` with a static signed JSON feed hosted as a GitHub Release asset. Tauri documents `createUpdaterArtifacts`, a pinned public key, HTTPS endpoints, the `latest.json` platform map, download progress, and mandatory signature verification. `tauri-apps/tauri-action` can assemble this feed automatically when it owns the release upload.
2. The same plugin with a dynamic update service for release channels, staged rollout, authenticated feeds, fallbacks, or downgrade policy. Atuin Desktop is a representative implementation: it chooses endpoints in Rust, retains the plugin's verified update resource, and presents progress in the renderer.
3. Check-and-notify without in-place installation on every platform. Entracte exposes a narrow Rust check command and an opt-in startup notification, while its settings hook keeps manual checks user initiated. Atuin installs in-app on macOS but sends other platforms to downloads, demonstrating that package behavior often needs a platform-specific fallback.

GitButler adds a configurable periodic check interval and stable/nightly channels, which is useful for a continuously shipped developer tool but would introduce background traffic and channel state that CleanerX does not currently need.

Primary references:

- [Tauri v2 updater documentation](https://v2.tauri.app/plugin/updater/)
- [`tauri-apps/tauri-action` updater manifest support](https://github.com/tauri-apps/tauri-action)
- [Entracte updater command](https://github.com/drmowinckels/entracte/blob/main/src-tauri/src/updater.rs) and [configuration](https://github.com/drmowinckels/entracte/blob/main/src-tauri/tauri.conf.json)
- [Atuin Desktop endpoint selection](https://github.com/atuinsh/desktop/blob/main/backend/src/commands/updates.rs) and [update UI](https://github.com/atuinsh/desktop/blob/main/src/routes/root/UpdateNotifier.tsx)
- [GitButler update interval defaults](https://github.com/gitbutlerapp/gitbutler/blob/master/crates/but-settings/assets/defaults.jsonc) and [platform release matrix](https://github.com/gitbutlerapp/gitbutler-docs/blob/main/content/docs/releases.mdx)
- [Tauri static-feed limitation for multiple Linux bundle types](https://github.com/tauri-apps/tauri-action/issues/1055)

## CleanerX decision

CleanerX uses the official Rust updater plugin, the stable GitHub Releases `latest.json` endpoint, and a public key pinned in the application configuration. The webview receives only purpose-specific status, check, and install commands. It cannot change the endpoint, signature key, target, headers, proxy, version comparator, or installer mode.

Checks are manual. Loading Settings reads the installed version and platform support locally; it does not perform a request. Clicking **Check for updates** performs one HTTPS request. If the manifest reports a newer SemVer version, CleanerX retains the verified update resource in Rust, displays bounded release notes, and waits for the user to click install. Download progress is sent over a Tauri IPC channel. macOS and Linux restart CleanerX after installation; Windows hands off to the passive NSIS installer, which exits the application as required by Tauri.

The fixed stable endpoint intentionally excludes GitHub prereleases. A future beta/nightly channel would require a separate explicit product decision and endpoint policy rather than allowing the renderer to choose arbitrary feeds.

## Platform and package policy

| Platform | In-app target | Manual formats | Reason |
| --- | --- | --- | --- |
| macOS arm64 | `.app.tar.gz` | DMG, application ZIP | Tauri replaces the installed application bundle; the DMG remains a manual installer. |
| macOS x86_64 | `.app.tar.gz` | DMG, application ZIP | Separate static-feed target preserves architecture matching. |
| Windows x86_64 | NSIS `.exe` | MSI | One static target cannot select both installer families; CleanerX standardizes the updater on NSIS. |
| Linux x86_64 | AppImage | `.deb` | A static feed has one `linux-x86_64` entry and cannot distinguish AppImage from `.deb`; non-AppImage runs do not contact the feed. |

## Release construction

Normal developer bundle commands remain unsigned and do not require the updater private key. Tagged releases add `src-tauri/tauri.updater.conf.json`, set `TAURI_SIGNING_PRIVATE_KEY` from GitHub Actions secrets, and ask Tauri to create update artifacts and `.sig` files. The publish job gathers every architecture, runs `scripts/generate-update-manifest.mjs`, and fails closed on a missing/empty signature before publishing `latest.json` and checksums.

Tauri's signature establishes continuity with the public key embedded in an installed CleanerX build. It does not establish operating-system publisher identity. Artifact names and release warnings therefore continue to say that current binaries are unsigned and not notarized.
