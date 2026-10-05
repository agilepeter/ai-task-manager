# Roadmap

What is planned for AI Task Manager, what is being considered, and what will
not be built. What has shipped is in [CHANGELOG.md](CHANGELOG.md). This file
states intent, not dates.

Every item is held to the rule the app's findings follow: computed on your
machine from what the app already reads, stated with its numbers, and absent
when there is nothing to say. Nothing here adds telemetry, and nothing here
reads prompt text.

## Next

- **A runaway alert and a budget per agent.** A warning when one agent's pace
  would pass a figure you set, or when it has run past a duration you set.
  Off until you set a figure.
- **Time spent at the limit.** How often a limit reached 100 percent in the
  last 30 days, and for how long.
- **Unused MCP servers.** Servers with no tool calls over a stretch the app
  itself watched. It first needs the app to keep its own daily count of calls
  per server: working it out from the logs that are still on disk can name a
  server that is in use. Claude Code only, because that is where call counts
  exist.

## Being considered

- Context compactions per session, as a finding.
- One session that cost several times your usual session.
- The same MCP server pinned in one client and unpinned in another.
- A monthly statement per client, as an export.
- Gemini CLI spend in the 30-day totals. Live pace already reads its logs.

## Needs the owner, not code

- **Code signing.** macOS needs an Apple Developer ID certificate and Windows
  a code-signing certificate. Until then macOS blocks the first launch and
  Windows shows a SmartScreen warning. [SHIPPING.md](SHIPPING.md) has the
  steps and the install notes say how to open the app today.

## Will not be built

- **Anything that acts on an agent.** The app tells you; you decide. The one
  action it has, asking a running MCP server to stop, stays the only one.
- **Traces, replays or anything that stores what an agent read or wrote.**
  That needs prompt and tool content, which this app never keeps.
- **Telemetry, accounts or a hosted service.** The team collector is a
  program you run yourself, and a seat sends it counts, never names of your
  own agents, paths, prompts or credentials.

## Asking for something

Open an issue and say which question you cannot answer today. A provider
request is easiest to act on with the file the tool keeps its login in and
the endpoint its own app calls for usage.

Upstream Pane's roadmap, as it stood when this app was copied from it, is
kept in [docs/upstream-roadmap.md](docs/upstream-roadmap.md).
