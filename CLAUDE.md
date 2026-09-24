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
file under `crates/core/src/providers/` and port it by hand.

## Hard rules

- **No telemetry, no phone-home.** Upstream's PostHog module was removed. Outbound traffic
  is each provider's own vendor API, plus two documented, owner-approved exceptions, both OFF
  by default and both described in full below: the **Trust Index lookup** (`trustLookup`,
  approved 2026-09-21) and **update checks** (`updateChecks`, approved 2026-09-24). Never turn
  the Trust Index into a per-package query. Anything else that would leave the machine needs
  the same explicit, documented decision.
- **The local API's extra feeds are opt-in** (`apiFeeds`, Settings > Advanced): `/v1/spend`,
  `/v1/spend/areas`, `/v1/spend/clients`, `/v1/subscriptions`. Areas and clients are folder
  and customer names, so while the switch is off nothing is published at all (the paths are
  404, not hidden). Loopback only, and the DNS-rebinding host check covers them. This
  supersedes the earlier "never on the API" note for areas, by the owner's decision.
- **Refreshing is driven from Rust, not the webview** (`spawn_background_refresh`). macOS
  suspends JavaScript timers in a hidden webview, and a tray app is hidden nearly always:
  history, alerts, reminders and feeds stalled for hours. The loop yields to an open window
  through shared last-fetch timestamps, so providers are never fetched twice. Do not move
  scheduling back into the frontend. Note: `tauri dev` is documented as watching `src-tauri/` only, but on
  2026-09-22 a write under `crates/core` relaunched the app within a second. Do not rely on
  either: after a change under `crates/`, check the binary (`strings target/debug/ai-task-manager |
  grep <new text>`) and, if it did not rebuild, `touch src-tauri/src/lib.rs`.
- The HTTP user-agent is `ai-task-manager/<version>`: the same for every install, never an id.
- **Self-update is opt-in and off by default** (`updateChecks` in config.json, a Settings >
  Network toggle; `update_checks_enabled` in `src-tauri/src/lib.rs` is the one function every
  call site asks, reading the config fresh each time so a toggle needs no restart). A missing or
  malformed key reads as off, so no existing install starts checking on its own. The pubkey in
  `tauri.conf.json` is OURS (minisign `6CDFF96385CA8101`; private half in `~/.tauri/` and repo
  secrets, never in the repo) and `updater_endpoint_strings` already points at this project's own
  release feed — what the setting gates is not which feed, but whether the app ever asks it
  anything. `SHIPPING.md` lists what distribution still needs.
- **OS credential stores are read-only.** `read_os_credential` never writes.
  On macOS the Claude provider reads the Keychain and never refreshes the
  token, because a refresh rotates it and would sign Claude Code out.
- **The app changes this machine in exactly two places, both on an explicit confirmed click.**
  `procs::end_task` is the second. It stops a running MCP server and its rules are the
  contract: the caller names a **server**, never a pid; the pids come from a snapshot taken
  inside that call, so no number crosses the command boundary and a recycled pid cannot be
  hit; only a process this app already matched as an MCP server is reachable; it asks
  (SIGTERM, `taskkill` with no `/F`) and **never escalates** — a server that ignores it keeps
  running and the next refresh says so. Children are signalled before their runner. The UI
  asks twice and the button only appears on hover. Ending one is safe by design: every client
  starts these on demand, so the next request spawns a fresh copy.
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

- **Nine languages, one JSON per locale** (`src/locales/{en,zh,ru,es,fr,de,ja,pt-BR,ko}.json`):
  flat files, the single source for both halves -- TypeScript imports them directly, Rust reads
  the identical bytes via `include_str!`. `src/i18n.ts` is logic only; no dictionary lives inline
  in code. Adding a language is a fixed checklist, not a judgment call: a new
  `src/locales/<xx>.json`, one line each in TS's `LOCALES` / `DICTS` / `LOCALE_TAGS` /
  `PLURAL_FORMS` and Rust's `LOCALES` / `locale_source` / `WINDOWS_LANGIDS` (only if it needs a
  non-English Windows mapping) / `ENV_PREFIXES`, a Windows LANGID, an env prefix, the `<option>`
  in `index.html` (then `npm run build:demo`), and the endonym key added to every other locale
  file. The comments on `LOCALES` in `src/i18n.ts` and `crates/core/src/i18n.rs` spell out the
  exact list; both sides are deliberately loud on a gap (`locale_source` has no catch-all) rather
  than silently falling back to English.
- **Metric labels key under `label.*`** (`label.<English label>`, e.g. `label.Session`), read by
  `displayMetricLabel` with an English fall-through like every `t()` call; `label.weeklySuffix` is
  the "{model} weekly" pattern. The Rust tray paints the same keys through `i18n::metric_label`,
  so a card's label and its tray line can never drift onto two different words for the same thing.
- **Rust-authored prose stays data until something paints it.** A sentence a Rust module produces
  -- a finding, an audit check, a sign-in hint, a pin-preview note, an alert -- leaves Rust as a
  `Msg` (a key plus vars plus an optional count), never a rendered string: the popover paints it in
  the active language, notifications render at fire time, the seat report keeps the English.
  Concretely: the popover calls `tm()` on a painted `titleMsg`/`detailMsg`; a notification renders
  in Rust the moment it fires, in whatever locale is configured then rather than whenever the app
  started; the seat report, the collector and the local HTTP API keep the English render
  (`title`/`detail` beside `title_msg`/`detail_msg`), so an enterprise consumer never sees a
  translated string. `render` (mirrored on both sides) is the one candidate search everything
  funnels through: `key.<plural form>` -> `key.other` -> `key`, each tried in the target locale
  then English before falling to the literal key.
- **Plural forms are a hand-rolled CLDR subset for exactly the nine shipped locales**, mirrored by
  a test that keeps Rust's `plural_form` and TypeScript's `pluralForm` identical: en/es/de and
  fr/pt-BR use one/other (fr and pt-BR both resolve 0 *and* 1 to "one" -- CLDR, not a typo; only
  European Portuguese, not shipped here, would need a third row); ru uses one/few/many; zh/ja/ko
  use other only. **A base key is either a bare value or a set of plural forms, never both** -- a
  sentence with a zero-count variant gets its own key (`…detail` vs `…detailOthers.<forms>`),
  chosen in Rust by which branch applies, so the forms-completeness test can't wave a stray bare
  key through.
- **Two checks to run after touching `index.html`.** `npm test` includes the Settings panel
  coverage test (`scripts/settings-i18n-coverage.mjs`), which walks the Settings DOM itself rather
  than the key list, so a label or hint that ships with no `data-i18n*` fails loudly instead of
  silently staying English; its exemption list is short and commented, and holds only brand names
  and bare currency options. Separately -- not part of `npm test`, since it needs a browser --
  `scripts/layout-check.mjs` is the 380 px layout harness: Playwright WebKit against the demo
  build, all nine locales across all seven views, asserting no container overflows its own window.
  A native `<select>`'s own clipped, selected-option text does not move its
  `scrollWidth`/`clientWidth` in WebKit, so the harness cannot see that one class of clipping; the
  human screenshot pass stays the real check for a `<select>`. Run it per the comment at the top of
  the file (`npm run build:demo`, serve `dist-demo`, then `node scripts/layout-check.mjs`).
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
  about the maker lives in `src/brand.ts`. `CREDITS` in that file is the MIT attribution for
  Pane and OpenUsage: it stays in any fork.** That file is not quite the whole rebrand, and
  claiming it was hid four places for months: the tray tooltip (`TOOLTIP_TITLE` in
  `src-tauri/src/tray_projection.rs`), the Quit item (`i18n::quit_label`), the installer's
  `publisher` / `copyright` / `longDescription` in `tauri.conf.json`, and provider sign-in
  messages. Those shipped as "Pane" until 2026-09-22. The real set is those four plus
  `brand.ts`.
- **`demo.html` is GENERATED from `index.html`** (`scripts/make-demo-html.py`, run by
  `npm run build:demo`). They were hand-maintained copies and drifted: the public demo was
  still telling people their keys live in `%APPDATA%\Pane` long after the app stopped saying
  it, because every markup fix had to be made twice. Never edit `demo.html`; edit `index.html`.
  The only two differences the script applies are a `noindex` meta and the demo boot entry.
- **Browser demo** (`npm run build:demo` -> `dist-demo/`, entry `demo.html`): the real UI on
  `src/demo/mock.ts`. Fictional data only, generated by the real engine from a fictional
  machine (`npm run fixture:demo`); never paste real numbers, areas or clients into it. The
  entry uses ordered static imports because the app boots on DOMContentLoaded.
  `scripts/sync-demo-to-site.sh` rebuilds it into the staas.fund product page
  (`staasfund/task-manager/demo/`); it never commits or pushes, because a push there is a deploy.
- **A new finding is invisible to the audit until it is listed there.** `audit.rs` does not
  iterate opportunities; it asks for specific ids (`from_finding`, or `only_if_present` for a
  finding with no meaningful "pass"). The running and pricing findings were computed and shown
  in the tab but scored nothing for a day because of this. `enriched_inventory()` in
  `src-tauri/src/lib.rs` is now the single place both the tab and the audit get their findings
  from, so the two lists cannot drift apart again.
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
- **Model-specific limits are named from a FIXED vocabulary** (`providers::claude::model_family`).
  The server sends an arbitrary `display_name`, which must never be echoed: it can reach the
  telemetry boundary through starred metrics. It is mapped onto Opus / Sonnet / Haiku / Fable /
  Mythos, and anything unknown degrades to "Model weekly". **That degradation is silent**, which
  is how a Fable user ended up staring at "Model weekly 100%" with no way to tell which model
  hit the wall. Add each new family here as it ships; the test lists the exact strings seen.
- **Extra usage is reported whenever money was spent, switch or no switch**
  (`providers::claude::extra_usage_metric`). Anthropic turns pay-as-you-go OFF once the cap is
  reached, and the old code returned early unless `is_enabled` — so the single state that
  matters most, "you went past the cap and it is now off", rendered as nothing at all. Real
  example 2026-09-22: $21.87 against a $20 cap, switch off, app silent. `disabled_reason` is
  server text and is never echoed; it only picks between two fixed phrases. `spend` in the same
  payload is the newer shape of `extra_usage` and carries no extra facts: do not add a second card.
- **The vendor publishes a limit for ONE model at a time.** Verified against the real
  endpoint 2026-09-22: `limits[]` carried `session`, `weekly_all` and a single
  `weekly_scoped` (Fable); `seven_day_opus` and `seven_day_sonnet` were null, and
  `seven_day_breakdown` splits the week by SURFACE (Claude Code / Chats / Cowork / Other),
  not by model. So "usage per model" cannot come from the API. The **local logs** know every
  model, which is why the card expander carries `modelSplit` ("Models today", share of
  today's cost). Do not try to synthesise per-model limits from the API; it does not have
  them. `live_usage_shape` prints the payload's shape (keys and percentages only) when this
  needs checking again.
- **The in-app marks are ours** (`src/assets/aitm-mark.svg`, `aitm-icon.png`). Upstream's
  `pane-logo.png` / `pane-icon.png` shipped in the sidebar and the share card until 2026-09-22.
  The sidebar takes the flat mark as raw SVG so `currentColor` tints it like the provider
  icons; the share card needs a data URI, because a rasterized SVG snapshot cannot load
  external resources. If a fork rebrands, these two files and `src/brand.ts` are the set.
- **The macOS tray is a template glyph plus native title text** (`design/tray-template.svg`,
  `menu_bar_title`). Never draw digits into the icon on macOS: a Retina menu bar scales a
  32 px bitmap into a blur, and a coloured icon ignores the light / dark menu bar. The glyph
  must stay pure black on transparent. The app runs as an Accessory (no Dock icon). Windows
  keeps upstream's drawn-number icon. Icons are original vectors in the HalperBot family
  (silver helmet, dark screen, cyan eyes, antenna): sources in `design/`, never generated art.
- **The rail pushes, never covers.** The auto-hiding sidebar slides out over the cards, but
  over an open panel (Settings, Customize, Detail, Audit, About) the panel and its pinned
  header step right by the rail's width (`body:has(#side-zone:hover)` rules). In wide mode
  the narrow panels shrink by the same amount so they never cross the detail column.
- **The tray tooltip has a hard 127 UTF-16 budget** (Windows truncates
  `NOTIFYICONDATA.szTip` past it). `TOOLTIP_TITLE` and `TOOLTIP_UTF16_CAPACITY` in
  `tray_projection.rs` are the only copies of the name and the cap: the builder held the
  string while the length arithmetic held a bare `4` for it, so renaming the app overran the
  cap by 11 on every tooltip. `title_length_matches_the_builder` pins them together. The
  longer name costs one provider line on Windows (five, not six) -- that is the price of the
  correct name, and capacity tests size their fixtures off the constants so a future rename
  re-tunes them instead of rotting.
- **Popover anchoring is per platform** (`popover_origin`): above the click for a bottom
  taskbar, below it for the macOS menu bar.
- **Running now** (`crates/core/src/procs.rs`, `get_running`): the Task Manager half of the
  name. Reads the process table (`ps` on Unix, `Get-CimInstance` on Windows, same column
  order so the parser has no platform branch), matches each process to a configured MCP
  server, and groups a runner with everything it spawned into one copy. **A command line
  never leaves that module**: arguments carry API keys, vault paths and project directories,
  so the only string lifted out is the package specifier right after a known runner, and only
  when it matches a conservative package charset. `npm run <script>` runs a LOCAL SCRIPT and
  is deliberately not read (it once turned `npm run tauri dev` into a server called "tauri").
  A process that matches nothing MCP-shaped is dropped rather than described. `never_leaks_*`
  plants a key, a token and a path in a command line and asserts none reach the output.
  Findings: duplicate copies and unconfigured servers score as "tighten", total memory is
  "learn" (a resting cost is not a failing).
- **Sign-ins** (`crates/core/src/diagnose.rs`): why a card is empty. Lists every place each
  locally-signed-in provider reads and whether it is there, so "you are not signed in" and
  "we looked in the wrong place" stop looking the same — on macOS Peter has no
  `.credentials.json` at all and the Keychain is the real source. **A keychain entry is named
  and never opened**: `security find-generic-password` can prompt, and a diagnostics panel
  must never pop a dialog, so its `found` is `None` rather than a guess. Each row carries
  `verified_here`, false for Codex and Cursor on macOS, and the test asserts that against
  what "Known gaps" above admits. The table is hand-written beside the providers, so it can
  drift: keep it in step when a provider's path changes. These paths are UI-only and have no
  field in the seat report.
- **Cost per commit** (`crates/core/src/effort.rs`): 30 days of spend in a work area against
  30 days of commits in that folder (`git log --since -- .`, scoped so an area inside a
  larger repo counts only its own changes). git is read, never written. Zero commits is a
  real answer and never a divisor; a folder outside git says so instead of guessing; buckets
  like `(unsorted)` are never probed because they are not folders. **It is a ratio, not a
  verdict** — one commit can be a day's refactor — and the copy says so rather than scoring.
- **Pricing check** (`crates/core/src/drift.rs`): Claude Code writes a `cost-state` line
  carrying, per model, its tokens **and the vendor's own `costUSD`**. That is a price
  reference already on disk, so the app checks its catalogue against it. No network. Samples
  that used web search are skipped (per-search billing is not a token rate), and a line whose
  own `hasUnknownModelCost` is true is skipped (the vendor's total was partial). A gap is only
  reported at 8%+, over $0.50, across 2+ samples. **When the gap equals the cache-write tokens
  times (1-hour rate minus 5-minute rate) to within 1%, it says so precisely** instead of
  hedging: verified on real logs 2026-09-21, where haiku matched to +0.0% and Fable was 22.7%
  low, every sample explained exactly by 1-hour cache writes. Spend figures are a floor.
- **Week over week** (`spend::week_over_week`): the last 7 days against the 7 before, from
  `daily_cost`. Needs a full fortnight or it returns None, and `change_percent` is absent when
  last week was zero, because "up from nothing" is not a percentage. Derived after the scan,
  so it never enters the persisted cache.
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
Rust tests: `cargo test --workspace` from the repo root; read the total off the run's own
summary line rather than off this file, which will not track it going forward.
Frontend tests: `npm test` (`node --test scripts/*.test.mjs`), also run by CI; same.

## Tests share process-wide state: serialize, never assume order

Several app tests mutate global caches (`last_ok()`, `fail_state()`). `SnapCacheGuard` holds
a lock for its lifetime so tests sharing an id cannot overlap; use it for any new test that
touches those caches. Name temp paths with `providers::unique_stamp()`, never the clock alone
(macOS reports whole microseconds). A test that fails 1 run in 10 is a real bug: run the
suite 20 to 30 times before calling a concurrency fix done.

## CI

`.github/workflows/ci.yml` has no macOS job, on purpose: macOS is the daily driver, built
and tested locally on every change, and `release.yml` already builds a macOS bundle for
tagged releases -- a per-push job here would duplicate both. A small `changes` job routes
each push: Rust or manifest changes run the Windows build and tests (about 15 minutes, 2x
billing); UI changes run a one-minute Linux type-check, build and `npm test`. Do not widen
the Windows job to UI paths. `scripts/**` routes to the frontend job, because the frontend
tests live there and went unrun for weeks when nothing referenced them.
