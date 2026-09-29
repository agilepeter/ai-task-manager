# Changelog

This app's history. It began as a clean copy of
[Pane](https://github.com/ItsJazii/pane) 0.4.52 and restarted its version
numbering at 0.1.0, so the numbers here do not continue upstream's. Pane's own
release history is kept verbatim in
[docs/upstream-changelog.md](docs/upstream-changelog.md).

## 0.1.5 — 2026-09-28

A small release for people who use the keyboard.

### Fixed

- **Every control can be reached with the Tab key.** In the macOS webview
  Tab used to skip buttons and links. It reaches them all, in order, with a
  ring around the one in focus. A provider's name on the Usage tab is a
  control too: Enter or Space opens its page. The three view tabs are one
  stop, and the arrow keys, Home and End move between them.
- **A panel returns focus to what opened it.** About, Settings, Customize,
  a provider's page, the client rules editor, the pin preview, the
  Subscriptions form and the confirm dialog all move focus in when they
  open and back when they close. In wide mode, where a provider's page
  stays open beside the list, Escape takes focus from the page back to its
  card.
- **A redraw keeps a keyboard user's place.** The cards redraw on a timer
  and Inventory redraws after a rescan. Focus stays on the control in the
  same place. End task keeps it through each of its steps, and so does
  drilling into a bar on a provider's page and coming back out.
- **Nothing is left armed.** After a click or a right-click, Space and
  Enter do not press the control that was clicked, or the button that
  opened a panel the click closed. A held Enter presses a control once.
- **Only the panel in front answers.** With a panel open, Tab stays inside
  it and the rail. When a second panel opens over a first, Tab, Escape and
  a click on the close button go to the one in front, and closing it brings
  the other back. Escape in a confirm dialog closes the dialog and leaves
  the panel beneath.
- **A confirmation for something that cannot be undone opens on Cancel.**
  That holds for the confirm dialog and for End task, so Return pressed
  without reading loses nothing.
- **The three figures in the Agents view read on one line each** in every
  language. The period has its own smaller line under the third.
- **The image at the top of the README showed version 0.1.0.** It is
  retaken from the demo, and the release steps name it.

On macOS before 12.3 a closed panel's controls stay in the Tab order, as
they were before this release. Everything else here works the same.

## 0.1.4 — 2026-09-28

### Added

- **An Agents view.** Opened from the Agents button in Inventory, or from the
  row at the top of it. It gathers what was spread over six places: three
  figures (your agents, running now, subagent spend over 30 days), the agent
  hosts running now with folder, client and pace, your own agents and the
  built-in ones with 30 days of runs and cost, and the findings about
  agents. When an agent guardrail check in the Audit needs attention it says
  how many and opens the Audit. It reads nothing new, and nothing in it
  starts, stops or edits an agent.

### Changed

- **Inventory no longer lists agents.** Running now holds MCP servers only,
  and the section that held agents, skills and guardrails holds skills and
  guardrails. One row at the top of Inventory states the three figures and
  opens the Agents view. Audit checks about agents open it too.
- **Wording in eight languages.** Strings added in 0.1.3 named a guardrail,
  a deny rule, an allow rule and a version pin with different words than the
  rest of the same language's file. They now use each language's own. German
  addresses the reader formally throughout, and Japanese names the tab and
  its section in Japanese.
- **Polish.** In Running now the End task button sits on the row's second
  line, so there is no empty line under every server. It shows on hover or
  keyboard focus, and always on a touch screen. The three figures at the top
  of the Agents view share one row in every language. The line about agent
  guardrail checks reads as a sentence with its button beneath. The Audit
  and the Agents view refresh when the popover is opened again, at most once
  a minute, and a rescan fills them from what it just read without scanning
  twice. A refresh that failed retries the moment the popover is opened
  again, rather than waiting out that minute. One Japanese string spaces a
  name as the rest of its file does, and the tooltip on the Agents button
  says spend, as the figure does.

### Fixed

- **Closing the Audit left the keyboard focus nowhere.** Closing the Audit,
  or the new Agents view, returns focus to the button that opened it.
- **A failed subagent-spend read showed as $0.00, Never run, and zero
  built-in agents, as if that were all true.** The stats figure and the
  Inventory door row show a question mark instead, Your agents shows no
  spend line at all rather than claiming one, and the built-in section names
  the error in place of its rows.
- **A link in this changelog to a file in the repository answered 404 on the
  changelog page.** It points at the repository now.
- **The product page stated an old version in two places** after a release:
  the sentence above the download links and the page's structured data. The
  script that refreshes the page after a release writes both now, along with
  the version cell and the download links, and refuses to write anything when
  the page is not in the shape it expects.
- **The security policy said no release had been cut**, and the roadmap was
  upstream's. Both describe this app now. Upstream's roadmap is kept in
  [docs/upstream-roadmap.md](docs/upstream-roadmap.md).

## 0.1.3 — 2026-09-28

### Added

- **Setup changes, in the Inventory tab.** Once a local day, the app writes
  down the shape of your setup: for each MCP server its name, client,
  transport, package and whether it is pinned; the names of your agents and
  skills; each hook event and how many hooks fire on it; and how many allow
  and deny rules there are, plus whether a deny rule covers the shell --
  never a value from env or headers, never a URL and never a path, and from
  args only a package name that has the plain shape of one. The file
  holds the last 40 snapshots written, and nothing is ever dropped for its
  own date, so a clock that runs backward or jumps ahead for a day cannot
  erase history. A new Changes section shows what moved since the newest
  snapshot that is at least a week old, or the oldest one on file, saying
  honestly how far back that reaches; a snapshot dated in the future or more
  than 35 days back is never the one compared against. A server that
  changed says what changed, a row each: how it connects, the package it
  runs, or a version pin gained or lost. Two new findings, both "learn" and unscored:
  `guardrail-removed`, when a deny rule, a hook or shell coverage went away
  and is worth confirming it was meant, and `setup-changed` for everything
  else that moved. Each counts the rows the Changes list shows. A row for a
  guardrail that went away carries an amber marker and a label a screen
  reader speaks. Losing a version pin does not, since `mcp-unpinned`
  already scores it. The weekly digest names the day it compares with.
- **MCP context cost, in Inventory.** For each MCP server Claude Code has
  configured, how many tool calls it answered and the total size, in bytes,
  of what came back over the last 30 days, shown as a fact on the server's
  own row. A new "learn" finding, `mcp-context-heavy`, flags a server with
  at least 20 calls and at least 2 MB of results in that window. The size
  is bytes as the session log holds them, not tokens, and it is a total
  over the 30 days, not the size of any one result. To take it, the scanner
  opens one kind of line it used to skip: the one that answers a call to an
  MCP server, from which it keeps a number and nothing else
  ([docs/privacy.md](docs/privacy.md) says exactly what). Claude Code
  only. A server whose name holds a double underscore gets no figure,
  because its calls cannot be told apart from a shorter server name followed
  by a tool part; the same goes for two configured servers whose names
  collapse to the same string once everything but letters, digits,
  underscores and hyphens turns into an underscore. A result whose call was
  made more than 256 MCP calls earlier without being answered is not sized.
  Counting a call is protected against the same replay a resumed session or
  a rescan could cause that the spend figures already guard against, no
  more and no less.

### Changed

- **The local scan cache re-scans once.** The MCP context-cost figures need
  a per-server, per-day count of calls and result bytes the persisted cache
  did not keep before, so the first launch after updating re-reads session
  logs once to back-fill it; every other total is unaffected.

### Fixed

- **The browser demo's footer answered "v0.1.0" no matter what it was built
  from.** It now reports the version it was actually built from, the same
  one in `package.json`.
- **The install notes told Mac users to right-click the app and choose
  Open**, which macOS 15 and later refuse for an app that is not notarized.
  They now give the step that works: System Settings > Privacy & Security >
  Open Anyway.
- **GitHub listed the licence as "Other"** because two of the copyright
  notices in `LICENSE` wrapped across more than one line each. Every notice
  is now on its own line, and GitHub reads the licence as MIT.

## 0.1.2 — 2026-09-26

### Added

- **Agent folders resolve on Windows too.** Running now reads a live agent's
  working folder through its own PEB, one process at a time, the way a
  debugger does, since Windows has no batch call like `lsof`. A process that
  refuses the read, and any 32-bit target, still reads "folder unknown"
  rather than a guess.
- **Agent spend names the client it mostly went to.** Inventory's thirty-day
  agent rows roll each agent's cost up to a client the same way the Clients
  tab does, and say so only when one client is a true majority -- never a
  mere plurality among several smaller ones.
- **An opt-in `/v1/agents` feed.** Thirty days of agent spend alongside who
  is running right now, behind the same loopback-only switch as the other
  local feeds. A running row carries a tool's pace, work area and client;
  the working folder, the process id and the session id never reach it.
- **Team-wide agent spend on the collector.** Built-in agent names (Explore,
  Plan and the rest) come through as themselves; anything a seat named
  itself is folded into one `custom` row before it ever leaves that seat, so
  a name you chose yourself never leaves the machine that reported it. The
  collector re-applies the same fold to whatever a client actually posts.
- **Two new usage findings, and a third promoted to the audit.**
  `cache-read-share` fires when at least 90% of the last 30 days' tokens
  were cache re-reads and at least $50 of Claude spend sits behind it -- the
  share alone is just how Claude Code works, so the dollar floor is what
  decides whether it is worth reading. `subagent-share` fires when
  subagents account for at least 10% of the last 30 days' spend, and at
  least $5. `perm-deny-only`, until now shown only in Inventory, also
  reaches the audit. All three are educational, not scored: unscored
  "consider" entries that stay quiet on a machine with nothing to say.

### Changed

- **The local scan cache re-scans once.** The new re-read finding needed a
  day-by-day cache-read count the persisted cache did not keep before, so
  the first launch after updating re-reads session logs once to backfill
  it; every other total is unaffected.
- **Live token pace now covers Codex and Gemini CLI, not only Claude Code.**
  Running now's pace reads each tool's own newest session file for that
  folder, cached by tool and folder together, so two different tools
  sharing one folder can never show each other's numbers.

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
