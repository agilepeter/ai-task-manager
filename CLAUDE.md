# CLAUDE.md — ai-task-manager

Cross-platform (macOS + Windows) tray app that shows AI subscription usage,
limits, spend, and the local AI setup. Cargo workspace:

- `crates/core` (`aitm-core`): providers, spend, pricing, inventory, alerts, i18n, the
  local HTTP API, and `rt` (a tokio shim). **No Tauri, no UI.** A future headless
  per-seat agent is meant to reuse it as is. `cargo tree -p aitm-core` must never list tauri.
- `src-tauri` (`ai-task-manager`): the Tauri v2 tray app. Re-exports the core modules at
  its root so `providers::…` / `alerts::…` paths work unchanged.
- `crates/agent` (`aitm-agent`): headless. `report` prints this machine's seat report,
  `push --to URL` sends it to a collector (token only from `AITM_COLLECTOR_TOKEN`, https
  required off-machine). No daemon: cron / launchd / Task Scheduler decides when.
- `crates/collector` (`aitm-collector`): self-hosted receiver + team dashboard. One process,
  one folder of JSON, token on every request, binds to localhost by default.
- `src/`: vanilla TS UI.

The collector reads an optional `policy.json` from its data folder on every request
(`crates/core/src/policy.rs`: allowed / blocked packages with `*`, require pinned, allow remote,
minimum deny rules, allowed tools). **Conformance is computed on the collector from what a
seat reported**; seats never receive the policy and cannot claim to conform. A broken policy
file means "no policy", never an error page.

**The seat report (`crates/core/src/seat.rs`) is the privacy boundary of the enterprise half.**
It has no field that could hold a prompt, path, folder, work area, client name, session id or
credential, and `never_carries_*` plants those in its inputs. Add a field only with a reason
an organisation needs it, and extend that test. Finding *titles* go out; *details* do not. Live limits go out as provider family, plan,
metric label, percent and reset time only: never the card name or a metric's detail text,
which can hold an account email. `aitm-agent --no-limits` skips the vendor calls entirely.

## Provenance

Clean copy of github.com/ItsJazii/pane (MIT). Commit 1 is the pristine
upstream import; see `UPSTREAM.md` for the exact upstream commit. Keep both
copyright notices in `LICENSE`. To take an upstream provider fix, diff that one
file under `src-tauri/src/providers/` and port it by hand.

## Hard rules

- **No telemetry, no phone-home.** Upstream's PostHog module was removed. Outbound traffic
  is each provider's own vendor API, plus exactly one documented exception the owner approved
  on 2026-09-21: the **Trust Index lookup** (`crates/core/src/trust.rs`). It is OFF by default
  (`trustLookup`), and when on it sends NOTHING about the machine: one parameterless GET of
  the public list, at most daily, matched locally. Never turn that into a per-package query.
  Anything else that would leave the machine needs the same explicit, documented decision.
- **The local API's extra feeds are opt-in** (`apiFeeds`, Settings > Advanced): `/v1/spend`,
  `/v1/spend/areas`, `/v1/spend/clients`, `/v1/subscriptions`. Areas and clients are folder
  and customer names, so while the switch is off nothing is published at all (the paths are
  404, not hidden). Loopback only, and the DNS-rebinding host check covers them. This
  supersedes the earlier "never on the API" note for areas, by the owner's decision.
- **Refreshing is driven from Rust, not the webview** (`spawn_background_refresh`). macOS
  suspends JavaScript timers in a hidden webview, and a tray app is hidden nearly always:
  history, alerts, reminders and feeds stalled for hours. The loop yields to an open window
  through shared last-fetch timestamps, so providers are never fetched twice. Do not move
  scheduling back into the frontend. Note `tauri dev` only watches `src-tauri/`: a change
  under `crates/` needs the app restarted by hand.
- The HTTP user-agent is `ai-task-manager/<version>`: the same for every install, never an id.
- **Self-update is off** (`UPDATES_ENABLED` in `src-tauri/src/lib.rs`). The pubkey in
  `tauri.conf.json` is now OURS (minisign `6CDFF96385CA8101`; private half in `~/.tauri/` and
  repo secrets, never in the repo). The endpoints in `updater_endpoint_strings` are still
  upstream's and must be replaced before it is ever enabled. `SHIPPING.md` lists what
  distribution still needs.
- **OS credential stores are read-only.** `read_os_credential` never writes.
  On macOS the Claude provider reads the Keychain and never refreshes the
  token, because a refresh rotates it and would sign Claude Code out.
- **The app writes to another tool's file in exactly one place: `crates/core/src/pin.rs`**
  (pinning an unpinned MCP package), and only on an explicit click after a before/after
  preview. The rules there are the contract: version from the LOCAL package cache (no
  registry call), a byte-for-byte replacement of the one JSON string, a re-parse proving the
  only difference is that string inside an `args` array, refusal if the file's size or mtime
  moved since the preview, and a backup beside the file. Never "fix" a config by re-encoding
  it: that reorders keys in a file its owner rewrites constantly.
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
- **About** (`src/about.ts`): maker, links to staas.fund (Library, Trust Index, Classroom,
  workshop), upstream credits, and the HalperBot mascot with an easter egg (poke him; every
  tenth poke is a dance). Opened from the sidebar or the footer build stamp. **Everything
  about the maker lives in `src/brand.ts`, so a fork re-brands by editing one file. `CREDITS`
  in that file is the MIT attribution for Pane and OpenUsage: it stays in any fork.**
- **Browser demo** (`npm run build:demo` -> `dist-demo/`, entry `demo.html`): the real UI on
  `src/demo/mock.ts`. Fictional data only, generated by the real engine from a fictional
  machine (`npm run fixture:demo`); never paste real numbers, areas or clients into it. The
  entry uses ordered static imports because the app boots on DOMContentLoaded.
  `scripts/sync-demo-to-site.sh` rebuilds it into the staas.fund product page
  (`staasfund/task-manager/demo/`); it never commits or pushes, because a push there is a deploy.
- **Audit** (`crates/core/src/audit.rs`, `src/audit.ts`): a scored, sectioned read of the whole
  setup; opens itself once on first run (`auditSeen`), then from Inventory > Audit; exports
  Markdown. It never judges anything a second time: each existing finding becomes a check,
  its absence a pass. **Only "tighten" findings count against the score; "learn" findings
  show as "worth a look" and are unscored** (a remote server or an unused capability is not
  a failing). The score is checks passed over checks that apply, arithmetic only, and what
  cannot be judged (under $50 of usage, no tools found) is left out instead of counted.
- **Your week** (detail page; `history::burn_profile_from`): a 7 x 24 heatmap of when a limit
  gets used, in local time, with the next reset outlined. A rise is booked to an hour only
  when the two readings are within 90 minutes; across a longer gap nobody knows when the
  usage happened, so it is skipped rather than guessed. One hue, more of it for more burn.
- **Usage coaching** (`crates/core/src/coaching.rs`): Opportunities from how the tools were
  used (top-heavy model mix, long-lived costly sessions, a large unsorted share). Same
  contract as the setup ones: computed, states its own numbers, absent when there is nothing
  to say. Thresholds are named constants. A model whose tier the name does not reveal is left
  out of the mix, never guessed.
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
- **The macOS tray is a template glyph plus native title text** (`design/tray-template.svg`,
  `menu_bar_title`). Never draw digits into the icon on macOS: a Retina menu bar scales a
  32 px bitmap into a blur, and a coloured icon ignores the light / dark menu bar. The glyph
  must stay pure black on transparent. The app runs as an Accessory (no Dock icon). Windows
  keeps upstream's drawn-number icon. Icons are original vectors in the HalperBot family
  (silver helmet, dark screen, cyan eyes, antenna): sources in `design/`, never generated art.
- **Popover anchoring is per platform** (`popover_origin`): above the click for a bottom
  taskbar, below it for the macOS menu bar.
- **Inventory covers every MCP-capable app it knows**, not just Claude Code: Claude Desktop,
  VS Code (`servers` key), Cursor, Windsurf, Gemini CLI and Codex (TOML), all through
  `mcp_from_map_for`, so the never-copy-a-value rule and its leak test cover them. AI tools
  are detected by a settings folder or a command on the PATH; commands are looked for, never
  run. Only Claude Code and Claude Desktop have been checked against real files; the other
  formats are fixture-tested.
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

- **Client budgets** (`monthly_budget` on a client rule): one alert per client per calendar
  month when month to date passes it. **Weekly digest** (`crates/core/src/digest.rs`,
  `weeklyDigest` = off or a weekday): once per ISO week from 9:00, catches up if the machine
  slept through its day, and sends nothing for an empty week. Both fire from the spend scan
  (so from the Rust loop) and remember what they have said in `alert_marks.json`.
- **Export CSV** (`export_table`): saves what a view is showing to Downloads. Cells are made
  spreadsheet-safe and the file name file-safe on the Rust side, whatever the frontend sent.

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
