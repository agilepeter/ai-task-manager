# Shipping AI Task Manager

Where distribution stands, and what each remaining step needs. Checked 2026-09-21.

## Works today

- `npm run tauri build -- --bundles app,dmg` produces `AI Task Manager.app` (26 MB) and a
  `.dmg` (9.8 MB) under `target/release/bundle/`. Verified on an Intel Mac.
- `.github/workflows/release.yml` builds macOS (universal) and Windows installers for a
  `v*` tag and attaches them to a **draft** release. It has never been run: a macOS runner
  bills at 10x on a private repo, and without signing the result would be the same unsigned
  bundle the local build already proves.
- Update signing key: ours (`minisign 6CDFF96385CA8101`). Public half in
  `src-tauri/tauri.conf.json`; private half and its password in `~/.tauri/` (owner-only) and
  in the repo secrets `TAURI_SIGNING_PRIVATE_KEY` / `_PASSWORD`. Upstream's key is gone.
  **Back the two `~/.tauri/ai-task-manager.key*` files up somewhere safe.** Losing them
  means no installed copy can ever accept an update again.

## Blocked, and on what

1. **macOS Gatekeeper rejects the app** (`spctl`: "no usable signature"). Needs an Apple
   Developer ID Application certificate. There is none on the build Mac
   (`security find-identity -v -p codesigning` lists 0). With one, add these secrets and the
   release workflow signs and notarizes with no edits: `APPLE_CERTIFICATE` (base64 .p12),
   `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`
   (app-specific), `APPLE_TEAM_ID`. Until then a user has to right-click > Open, or run
   `xattr -dr com.apple.quarantine "/Applications/AI Task Manager.app"`.
2. **Windows SmartScreen** warns on unsigned installers. Needs a code-signing certificate
   (OV/EV) or Azure Trusted Signing. Nothing is set up.
3. **Auto-update has nowhere to look.** The updater needs a public URL serving `latest.json`
   and the installers. This repo is private, so its releases are not downloadable without
   auth. Pick one: make the repo (or a releases-only repo) public, or host the files on a
   site you control. Then, together: set the endpoints in `updater_endpoint_strings`
   (`src-tauri/src/lib.rs`), set `"createUpdaterArtifacts": true` and the same endpoints in
   `tauri.conf.json`, and flip `UPDATES_ENABLED` to `true`. Do not flip it alone.

## Still upstream's

- ~~The app icon~~ is ours now: an original vector in the HalperBot family
  (`design/app-icon.svg`; regenerate every size with `npx tauri icon design/app-icon-1024.png`).
- `README.md`, `CHANGELOG.md`, `ROADMAP.md`, `docs/` describe Pane for Windows.
