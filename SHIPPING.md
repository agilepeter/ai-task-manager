# Shipping AI Task Manager

Where distribution stands, and what each remaining step needs. Checked 2026-09-24.

## Works today

- `npm run tauri build -- --bundles app,dmg` produces `AI Task Manager.app` and a `.dmg` under
  `target/release/bundle/`. Verified on an Intel Mac.
- `.github/workflows/release.yml` builds macOS (universal) and Windows installers for a `v*` tag
  and attaches them to a **draft** release, together with the updater artifacts (`.sig` files
  and `latest.json`) now that `createUpdaterArtifacts` is on. The repository is public, so the
  runners cost nothing; a `workflow_dispatch` run builds without creating a release and is the
  way to rehearse.
  Both legs have been rehearsed green (2026-09-24). macOS produces `AI Task Manager_0.1.0_universal.dmg`
  plus the updater pair `AI Task Manager.app.tar.gz` / `.sig`; Windows produces the `.msi` and the
  NSIS `-setup.exe`, each with its `.sig`. `latest.json` is written when a tag creates the draft release.
- Update signing key: ours (`minisign 6CDFF96385CA8101`). The public half is in
  `src-tauri/tauri.conf.json` (verified byte-for-byte against the key file). The private half is
  password-encrypted and lives in `~/.tauri/` on the build Mac and in the repository secrets
  `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (write-only). It is backed up
  in two places on purpose, neither holding both halves: the encrypted key and its public half in
  the owner's iCloud Drive (`Backups/ai-task-manager-signing/`), the password in the owner's
  private R2 bucket (`empire-git-backups/ai-task-manager-signing/`). Losing all of these means no
  installed copy can ever accept an update again.
- The update feed is wired: `updater_endpoint_strings` and `tauri.conf.json` both point at
  `https://github.com/agilepeter/ai-task-manager/releases/latest/download/latest.json`, which
  exists as soon as the first release is published.

## Not yet on, and why

1. **Update checks are opt-in and off by default.** A Settings toggle (`updateChecks`) turns them
   on; until a user does, the app makes no update request at all, which is what the privacy
   notes promise. The switch replaces the old build-time constant. Enabling it before a release
   exists is harmless: a 404 from the feed is a logged non-event.
2. **macOS Gatekeeper rejects an unsigned app** (`spctl`: "no usable signature"). Needs an Apple
   Developer ID Application certificate, which only the owner can obtain: enroll in the Apple
   Developer Program (developer.apple.com, yearly fee, identity check with the owner's Apple ID),
   create a "Developer ID Application" certificate, export it as a `.p12`, then add six repository
   secrets — `APPLE_CERTIFICATE` (the `.p12`, base64), `APPLE_CERTIFICATE_PASSWORD`,
   `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD` (an app-specific password),
   `APPLE_TEAM_ID`. The release workflow already reads all six and signs and notarizes with no
   edits. Until then a user right-clicks > Open once, or runs
   `xattr -dr com.apple.quarantine "/Applications/AI Task Manager.app"`.
3. **Windows SmartScreen** warns on unsigned installers. Needs a code-signing certificate (an OV
   certificate from a CA, or Azure Trusted Signing) and the matching `bundle.windows` settings in
   `tauri.conf.json`. Nothing is set up; nothing in the code blocks it.

## Cutting a release

1. Bump `version` in `src-tauri/tauri.conf.json` and `src-tauri/Cargo.toml`, note it in
   `CHANGELOG.md`, commit.
2. `git tag vX.Y.Z && git push --tags`. The workflow builds both platforms and opens a draft
   release with the installers, the `.sig` files and `latest.json`.
3. Check the draft's assets, then publish it. Installs with update checks on see it within
   four hours or at their next launch.

## Still upstream's

- `docs/upstream-changelog.md` is Pane's history, kept for attribution.
