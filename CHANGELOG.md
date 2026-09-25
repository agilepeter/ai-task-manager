# Changelog

This app's history. It began as a clean copy of
[Pane](https://github.com/ItsJazii/pane) 0.4.52 and restarted its version
numbering at 0.1.0, so the numbers here do not continue upstream's. Pane's own
release history is kept verbatim in
[docs/upstream-changelog.md](docs/upstream-changelog.md).

## 0.1.1 — 2026-09-25

### Added

- **Agent processes in Running now.** Every agent host it recognizes (Claude
  Code, Codex, Gemini CLI, Cursor Agent, Aider, OpenCode, Goose, GitHub
  Copilot CLI) is listed alongside MCP servers: which tool, how long it has
  run, its working folder (via `lsof` on macOS; Windows has no way yet to
  read another process's folder), the client that folder maps to, and live
  token pace from the newest session writing there, subagent fan-outs
  included. Matching is by binary or package, never argv, and there is no
  End task for an agent -- only for the MCP servers it starts.
- **Cost per agent, in Inventory.** Thirty days of subagent spend broken out
  by agent name, for your own custom definitions and Claude Code's built-in
  agents alike, with an unattributed row for a transcript that never named
  one. A custom agent nobody has run in the window surfaces as a Learn
  opportunity rather than sitting unnoticed. A session's own row shows how
  much of its cost its subagents accounted for.
- **Agent guardrails, locally and for a team.** Audit gains three checks: a
  custom agent with no tools allowlist, a deny list that never names the
  shell, and a custom agent with no model pinned (the first two score
  against the setup; the model check is a "consider"). The same three
  conditions are now policy rules a collector can require (`requireAgentTools`,
  `requireShellDeny`, `requireHooks`), backed by four additive seat-report
  fields -- two counts, a boolean, and a capped list of hook event names --
  that never carry an agent's name, its model, or a hook's command or
  matcher. The collector's dashboard gains a Guardrails column.
- **A deterministic browser demo.** `npm run fixture:demo` pins the
  fictional machine's "today" (`AITM_TODAY`) to a fixed instant, so two runs
  of the fixture, and two builds of the demo from it, are byte-identical
  until that instant is bumped by hand.

### Changed

- **A session's cost now includes its subagents'.** Folding a subagent
  transcript into the session that spawned it (see Fixed, below) means that
  session's own cost and token totals now include what its subagents spent;
  project and day totals do not move, since they already counted this
  spend. The new agent-spend view above breaks the subagent share back out
  by which agent ran it.
- **The local scan cache re-scans once.** Recognizing subagent transcripts
  nested a folder deeper, under a workflow run's own
  `subagents/workflows/<workflow-id>/`, bumped the cache format, so the
  first launch after updating re-reads session logs once, to reclassify
  transcripts an older build had already cached as their own sessions.

### Fixed

- **Subagent transcripts no longer appear as sessions.** Claude Code writes
  each subagent (Task-tool) run to its own log file under
  `<session>/subagents/`, and a workflow run nests its own transcripts a
  folder deeper still, under `subagents/workflows/<workflow-id>/`; the
  scanner was listing every one of those as its own phantom session instead
  of recognizing it as part of the session that spawned it. Recognizing it
  now covers both nesting depths, and the Running-now pace follows the same
  writes, including workflow transcripts, so a session waiting on a
  subagent no longer reads as idle.

## 0.1.0 — 2026-09-24

First release of AI Task Manager. Everything below is relative to the imported
upstream copy.

### Added

- **macOS support.** The app runs on macOS as a menu bar Accessory (no Dock
  icon) with a template glyph and native title text, alongside the Windows
  tray. Copy throughout is platform-neutral.
- **Inventory tab.** MCP servers, agents, skills and guardrails across every
  MCP-capable app it knows (Claude Code, Claude Desktop, VS Code, Cursor,
  Windsurf, Gemini CLI, Codex), plus the AI tools installed on the machine.
  Names, shapes and counts only: a test plants secrets in every field those
  config files can hold and asserts none reach the output.
- **Running now.** Reads the process table and shows which configured MCP
  servers are actually running, grouped by runner, with their memory. **End
  task** stops one, after two confirmations, by name and never by process id.
  It asks and never escalates.
- **Audit.** A scored, sectioned read of the whole setup, which opens itself
  once on first run and exports Markdown. Only "tighten" findings count against
  the score; what cannot be judged is left out rather than counted.
- **Detail view.** Click a card's name for limits over time from a local 90-day
  history, spend grouped by model, project or day, and a 24-hour sparkline on
  every card expander.
- **Your week.** A 7x24 heatmap of when a limit actually gets used, in local
  time, with the next reset outlined. A rise is only booked to an hour when
  two readings are close enough to know.
- **Forecast.** When a limit runs out at the recent rate, saying which basis it
  used. Idle forecasts flat rather than replaying an old average.
- **Subscriptions tab.** Your own ledger of what you pay, with renewals that
  step from an anchor date without drift, one reminder per renewal, what-if
  pricing, and each subscription's 30-day API-equivalent value. It never
  guesses a price.
- **Spend by work area, and clients.** Where inside a project the dollars went,
  rolled up to the customer the work was for, with CSV export that defuses
  formula cells. Folder and client names stay on the machine.
- **Session drill-down.** The sessions behind a work area or a day, read from
  the scan cache. Conversation titles are deliberately never read.
- **Usage coaching.** Opportunities computed from how the tools were actually
  used: model mix, long-lived costly sessions, a large unsorted share.
- **Budget guard.** *Burning Fast* catches a weekly-or-longer limit jumping
  many points inside thirty minutes, which pace rules miss. *Daily Spend*
  fires once per local day at a figure you set. Plus per-client monthly
  budgets and an optional weekly digest.
- **Long-session nudge.** One notification a week about an old session still
  in use.
- **Pricing check.** Cross-checks the app's own catalogue against the vendor's
  recorded cost in local logs, with no network call, and names the cause when
  the arithmetic explains the gap.
- **Week-over-week deltas** on spend.
- **Cost per commit.** Thirty days of spend in a work area against thirty days
  of commits in that folder. git is read, never written, and it is presented as
  a ratio rather than a verdict.
- **Sign-ins diagnostics.** Every location each provider checks and whether it
  is there, so "not signed in" and "we looked in the wrong place" stop looking
  alike. A keychain entry is named and never opened, because reading it can
  prompt.
- **One-click pin.** Turns the unpinned-MCP-package finding into a fix: version
  from the local package cache, one JSON string replaced byte for byte, a
  re-parse proving nothing else moved, and a backup beside the file.
- **MCP Trust Index lookup.** Off by default. Switched on it sends nothing
  about the machine: one parameterless GET of a public list, at most daily,
  matched locally.
- **Opt-in update checks.** Off by default (`updateChecks` in Settings >
  Network). Switched on, the app asks GitHub's own release feed for
  `latest.json` on launch, whenever the popover opens, and every 4 hours in
  the background, sending nothing about the machine; a 404 (no release yet) or being offline are both logged and
  otherwise ignored. The footer's "Update available" button only ever
  appears from a real answer, and turning the setting off hides a pending
  one immediately.
- **Enterprise half.** A headless agent (`aitm-agent`) that reports a per-seat
  inventory, a self-hosted collector with a team dashboard, and a team policy
  whose conformance is computed on the collector rather than claimed by the
  seat. The seat report has no field that can carry a prompt, path, folder,
  work area, client name, session id or credential.
- **Wide mode.** The same window grown from 380 to 760, list left and detail
  right. Never a second window.
- **Browser demo.** The real interface on a fictional machine, generated by the
  real engine, with nothing installed.
- **About panel**, and opt-in extra feeds on the local HTTP API.
- **Sessions.** Group Spend by Session for every Claude Code session of the last 30
  days, heaviest first, with when it last wrote a line and how big its log is. Each
  row has a Reveal button that shows the file so you can archive it yourself. The
  app never deletes a session: those logs are also where its spend figures come from.
- **Extra usage names its period.** The row reads "$21.87 of $20.00 monthly cap (109%)"
  rather than "limit reached" three times with no period, after a weekly rollover made
  it look like a stuck weekly limit. The bar still clamps at 100; the words carry the
  real figure.
- **Never two vendor calls inside a minute.** A refresh, whether from the background loop, a
  click or a reset timer, reuses any answer younger than sixty seconds instead of asking again.
  A click seconds after the loop's own poll used to be a second call in the same minute, which
  Anthropic answers with a 429; that benched the provider for minutes and froze the card. A
  held-back refresh now names the time its numbers are from, with the reason on hover.
- **A manual refresh admits what it could not refresh.** When a provider's fetch fails
  and its last good numbers are shown instead, a refresh you asked for says so in the
  footer instead of "Updated".
- **Nine languages.** English, 中文, Русский, Español, Français, Deutsch, 日本語,
  Português (Brasil) and 한국어, following the system locale by default and
  changeable in Settings. Detail, Inventory, Audit, About and Subscriptions --
  the five views that had stayed English-only -- now translate alongside Usage
  and Settings. A sentence Rust authors (a finding, an audit check, a sign-in
  hint, a notification) leaves Rust as a key plus variables; the popover paints
  that key in the active language, and a notification renders at the moment it
  fires. The seat report and the local HTTP API keep the English render, so an
  enterprise collector or a script reading `/v1/usage` never sees a translated
  string. The browser demo translates the same way the app does, generated by
  the real engine rather than hand-edited.
- **Settings panel fully keyed, checked by a coverage test.** Every label,
  hint and tooltip in the Settings panel now carries a translation key; the
  test walks the panel's markup rather than the key list, so a new row that
  ships with no key fails loudly instead of shipping in English.
- **380 px layout harness.** A Playwright pass renders all nine languages
  across every view at the app's real 380 px window width; the screenshots it
  writes were reviewed by hand for clipping and awkward wraps.

### Changed

- **Split a Tauri-free core crate** (`aitm-core`) out of the app, so the
  headless agent reuses the same provider, spend and pricing code.
- **Refreshing is driven from Rust**, not the webview. macOS suspends
  JavaScript timers in a hidden window, and a tray app is hidden nearly always,
  so history, alerts, reminders and feeds had been stalling for hours.
- **Its own config directory** (`AITaskManager`), never upstream's. On macOS
  OpenUsage is a separate real app whose settings must not be touched.
- **Model-specific limits are named from a fixed vocabulary**, so an arbitrary
  server string is never echoed, and an unknown family degrades to a generic
  label rather than a guess.
- **Extra usage is reported whenever money was spent**, switch or no switch.
  The state that matters most, "you went past the cap and it is now off", used
  to render as nothing at all.
- **The sidebar rail pushes an open panel aside** instead of covering its first
  column and its Done button.
- Renamed throughout: the Quit item, tray tooltip, installer metadata, provider
  sign-in messages and the HTTP user-agent had all still said Pane.
- Documentation rewritten for both platforms, including a privacy page that
  matches what the app actually does.

### Removed

- **Telemetry.** Upstream's PostHog module is gone rather than disabled: no
  analytics SDK, no daily statistic, no random install id.
- **Growth machinery.** The GitHub star prompt and its five config keys.
- **Always-on self-update.** Upstream checked its own feed on every launch
  with no way to say no. The updater now points at this project's release
  feed and stays off until you switch it on (see "Opt-in update checks"
  above).
- **Upstream's release, winget and installer automation**, and the legacy
  OpenUsage directory migration.
- **Share cards**, which published card contents to the clipboard.
