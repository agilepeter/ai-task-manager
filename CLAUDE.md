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
- UI copy is platform-neutral; keep it that way ("this computer", "system notifications",
  no `%APPDATA%` paths, shortcut hints say Cmd on macOS). Log prefix is `[aitm]`.
- Only Claude and Copilot are verified end to end on macOS. Codex, Cursor, OpenRouter
  and ElevenLabs credential paths on macOS are unverified.

## Views

One product, tabs at the top: **Usage** (default; limits, pace, spend), **Subscriptions**
(`src/ledger.ts` + `crates/core/src/ledger.rs`) and **Inventory**
(`src/inventory.ts` + `src-tauri/src/inventory.rs`: MCP servers, agents, skills, guardrails,
and computed Opportunities). Usage stays the default view.

- **Detail view** (`src/detail.ts`): click a card's name. Same window, slide-in page like
  Settings; Esc backs out before it hides the window. Limits over time come from
  `crates/core/src/history.rs` (local SQLite, readings only, 90 days); spend groups by
  Model / Project / Day. Charts follow the dataviz rules noted at the top of the file;
  the `--viz-*` colours were validated per theme, so re-run the validator if they change.
  Project = the folder Claude Code was started in, matched against paths it still knows
  (folder names are lossy and cannot be decoded).
- **Clients** (`crates/core/src/clients.rs`): the user's rules in `clients.json` map work
  areas to a client name (plain folder = itself and below, `*` wildcard, first match wins).
  Every dollar lands in exactly one row; no match is `Unassigned`, sorted last. Areas are
  kept two folders deep so `site/client-a` and `site/client-b` can differ; the Work area view
  rolls up to one level by default. CSV export defuses formula cells (`= + - @`). **Never put
  a real client name in code, tests, fixtures or commit messages**: use "Acme"/"Client A".
  The detail page redraws on data refresh; `render(true)` reads the rules editor back first
  so typing survives, but click handlers must call plain `render()`.
- **Forecast** (`crates/core/src/forecast.rs`): "when does this run out at the rate I have
  been going?" Recent rate from the history store (lookback = a seventh of the period, 1 h to
  24 h, readings before a reset ignored), period average only when history is too thin, and
  the text always says which. Idle lately forecasts FLAT, not the old average. A wall is only
  reported when it comes before the reset. Separate from the pace rules in `alerts.rs`, which
  are straight lines from the period start.
- **Session drill-down**: click a Work area row or a Day bar for the sessions behind it
  (`spend::claude_sessions`, read from the scan cache, never a rescan). Times, totals, top
  model and areas only. Claude Code keeps conversation titles in the same logs; they come
  from prompts and are deliberately never read. A span is first to last message, not time
  worked: sessions stay open for weeks.
- **Subscriptions ledger.** The user's own numbers in `ledger.json`. **Never guess a price**:
  a detected plan names a tier, not what someone pays, so suggestions pre-fill the name and
  the linked tool only. Renewals step from the anchor date (Jan 31 -> Feb 28 -> Mar 31, no
  drift). "Idle" needs a MEASURED low usage; a tool with no spend data is "no data". One
  reminder per renewal, persisted as sent (`renewalReminderDays`, 0 = off). Value = the
  linked card's 30-day API-equivalent spend over the monthly cost; the frontend passes that
  map in so the command does not rescan.
- **Work areas** (`claude_area` in `crates/core/src/spend.rs`): spend inside a project, by
  top-level folder. Signals in order: the log line's `cwd` relative to the session's first
  `cwd`, then the first path under that root in the message's tool calls (path inputs, or
  paths inside a Bash command); otherwise the last area stands. Runs BEFORE the duplicate
  check, because Claude Code logs one content block per line and a tool call arrives on a
  later "duplicate" line. `_` and `.` folders never switch the area from a tool path. Areas
  are a second view of the same dollars and must sum to the project total. Names are bounded
  like model names. Any change to this logic needs a `PERSIST_VERSION` bump. Folder names
  stay on the machine: never add them to the local HTTP API or a share card.
- **Card expander extras** (`cardExtras` in `src/detail.ts`): every live card's chevron opens
  a 24-hour sparkline, today's spend with the leading model, and a Details button. The card
  renderer is synchronous, so extras draw from a cache and patch themselves in place when the
  background load lands. Never trigger a full re-render from that load: it would loop.
- **Wide mode** (`wideMode`, the Wide / Narrow button): the SAME window grown from 380 to
  760, list left, detail right. Never a second window. `set_wide` keeps the right edge fixed
  (the tray side). Full-window panels keep to the left column, and a closed Settings parks
  off the LEFT in wide mode, because its usual park position lands inside a 760 window.
  The saved choice is applied by `applySavedWide()` after the config loads, not in setup.
- **Popover anchoring is per platform** (`popover_origin`): above the click for a bottom
  taskbar, below it for the macOS menu bar.
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

## Budget guard

Two alert rules in `crates/core/src/alerts.rs`, configured by dropdowns in Settings > Notifications:

- **Burning Fast** (`burnAlertPoints`, default 15, 0 = off): a weekly-or-longer limit rose
  that many points inside 30 minutes. Exists because the pace rules are straight lines from
  the period start, so an agent fan-out early in a week looks fine to them until much later.
  Short windows are excluded on purpose: a busy 5-hour session is not an incident.
- **Daily Spend** (`dailySpendAlert` dollars, default 0 = off): today's local spend total
  crossed the mark. Fires once per local day. On a flat-rate plan the figure is
  API-equivalent value, not a charge; the tooltip says so, keep it honest.

Rules take the clock as a parameter (`evaluate_at`) so windows are tested exactly.

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

`.github/workflows/ci.yml` has no macOS job, on purpose: macOS runners bill at 10x on
private repos and Mac is built locally. A small `changes` job routes each push: Rust or
manifest changes run the Windows build and tests (about 15 minutes, 2x billing); UI changes
run a one-minute Linux type-check and build. Do not widen the Windows job to UI paths.
