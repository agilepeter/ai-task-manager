# CLAUDE.md — ai-task-manager

Cross-platform (macOS + Windows) tray app that shows AI subscription usage,
limits, and spend. Tauri v2: Rust core in `src-tauri/`, vanilla TS UI in `src/`.

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

- Internal crate is still named `pane` / `pane_lib`; rename during the core split.
- Antigravity discovery shells out to PowerShell/netstat: compiles, finds nothing on macOS.
- Claude extra accounts via `CLAUDE_CONFIG_DIR` (hash-suffixed Keychain service) are not mapped.
- Log prefix is still `[pane]`, and some user-facing strings still say "Pane" or "this PC".
- Spend scan skips any single CLI log over 512 MiB, so a huge Claude Code session is
  left out of spend totals (seen on a 723 MiB session file).
- Only Claude and Copilot are verified end to end on macOS. Codex, Cursor, OpenRouter
  and ElevenLabs credential paths on macOS are unverified.

## Own config dir, never upstream's

`config_dir()` is `AITaskManager`. Do not reintroduce upstream's "OpenUsage -> Pane"
directory migration: on macOS OpenUsage is a separate real app, and sharing "Pane"
would let this app and upstream Pane overwrite each other.

## Build

Needs Rust (rustup, official installer: Homebrew has no bottle for Intel macOS 26 and
builds LLVM from source) and Node. `npm install`, then `npm run tauri dev`.
Local API for checking real numbers: `curl http://127.0.0.1:6736/v1/usage`.
Rust tests: `cd src-tauri && cargo test`.
