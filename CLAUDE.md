# CLAUDE.md — ai-task-manager

Cross-platform (macOS + Windows) tray app that shows AI subscription usage,
limits, spend, and the local AI setup. Cargo workspace:

- `crates/core` (`aitm-core`): providers, spend, pricing, inventory, alerts, i18n, the
  local HTTP API, and `rt` (a tokio shim). **No Tauri, no UI.** A future headless
  per-seat agent is meant to reuse it as is. `cargo tree -p aitm-core` must never list tauri.
- `src-tauri` (`ai-task-manager`): the Tauri v2 tray app. Re-exports the core modules at
  its root so `providers::…` / `alerts::…` paths work unchanged.
- `src/`: vanilla TS UI.

## Provenance

Clean copy of github.com/ItsJazii/pane (MIT). Commit 1 is the pristine
upstream import; see `UPSTREAM.md` for the exact upstream commit. Keep both
copyright notices in `LICENSE`. To take an upstream provider fix, diff that one
file under `src-tauri/src/providers/` and port it by hand.

## Hard rules

- **No telemetry, no phone-home.** Upstream's PostHog module was removed. The
  only outbound traffic allowed is each provider's own vendor API. Anything
  new that leaves the machine needs an explicit, documented decision.
- **Self-update is off** (`UPDATES_ENABLED` in `src-tauri/src/lib.rs`). The
  pubkey left in `tauri.conf.json` is upstream's. Never enable updates without
  replacing both the endpoints and the pubkey with our own.
- **OS credential stores are read-only.** `read_os_credential` never writes.
  On macOS the Claude provider reads the Keychain and never refreshes the
  token, because a refresh rotates it and would sign Claude Code out.
- **Never log or print a credential**, including in tests and error strings.
- **Both platforms must keep compiling.** Windows-only code sits behind
  `#[cfg(windows)]` with a non-Windows counterpart beside it; Windows crates
  live under `[target.'cfg(windows)'.dependencies]`.

## Known gaps (macOS)

- Antigravity discovery shells out to PowerShell/netstat: compiles, finds nothing on macOS.
- Claude extra accounts via `CLAUDE_CONFIG_DIR` (hash-suffixed Keychain service) are not mapped.
- Log prefix is still `[pane]`. About 10 user-facing strings (x3 languages in `src/i18n.ts`,
  plus Rust hints) still say "this PC" or show the old `%APPDATA%\Pane` path, which is now
  wrong on both platforms (the dir is `AITaskManager`). One copy pass, with the core split.
- Spend scan skips any single CLI log over 512 MiB, so a huge Claude Code session is
  left out of spend totals (seen on a 723 MiB session file).
- Only Claude and Copilot are verified end to end on macOS. Codex, Cursor, OpenRouter
  and ElevenLabs credential paths on macOS are unverified.

## Views

One product, tabs at the top: **Usage** (default; limits, pace, spend) and **Inventory**
(`src/inventory.ts` + `src-tauri/src/inventory.rs`: MCP servers, agents, skills, guardrails,
and computed Opportunities). Usage stays the default view.

- **Inventory reads names, shapes and counts only.** Claude config files hold API keys in
  `env`, `args`, `headers`, URL paths and query strings. Nothing may copy a value out of
  those. The `never_leaks_*` test plants secrets in every such field; extend it when adding
  a field.
- **Opportunities are computed from the inventory, never generic advice.** Each pairs a fact
  about this machine with why it matters and a Learn more link. A tidy setup shows none.
- **macOS WebKit gets opaque bars** (`body.no-svg-lens`): it does not paint SVG `url()`
  backdrop filters, and translucent bars let scrolling rows read through.
- **Check UI in WebKit, not Chromium**: the macOS webview is WebKit. Playwright's `webkit`
  with a mocked `window.__TAURI_INTERNALS__.invoke` renders the real frontend from the vite
  dev server (port 1420). Feed it real data, and give `get_config` a full config or boot aborts.

## Own config dir, never upstream's

`config_dir()` is `AITaskManager`. Do not reintroduce upstream's "OpenUsage -> Pane"
directory migration: on macOS OpenUsage is a separate real app, and sharing "Pane"
would let this app and upstream Pane overwrite each other.

## Build

Needs Rust (rustup, official installer: Homebrew has no bottle for Intel macOS 26 and
builds LLVM from source) and Node. `npm install`, then `npm run tauri dev`.
Local API for checking real numbers: `curl http://127.0.0.1:6736/v1/usage`.
Rust tests: `cargo test --workspace` from the repo root (468 on macOS).

## Tests share process-wide state: serialize, never assume order

Several app tests mutate global caches (`last_ok()`, `fail_state()`). `SnapCacheGuard` holds
a lock for its lifetime so tests sharing an id cannot overlap; use it for any new test that
touches those caches. Name temp paths with `providers::unique_stamp()`, never the clock alone
(macOS reports whole microseconds). A test that fails 1 run in 10 is a real bug: run the
suite 20 to 30 times before calling a concurrency fix done.

## CI

`.github/workflows/ci.yml` is Windows only, on purpose (see the comment in the file):
macOS runners bill at 10x on private repos and Mac is built locally. It runs on pushes
touching Rust or manifests, and on manual dispatch.
