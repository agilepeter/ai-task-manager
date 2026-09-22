# Privacy

One rule: **your data is nobody's business, including ours.**

There is no account, no backend, and no analytics. Your quotas, spend, folder
names, client names, prompts and provider data never leave your computer. This
page lists every network call the app can make, and how to check the claim
yourself.

> **If you are comparing this against the upstream project:** the PostHog
> telemetry module that shipped in [Pane](https://github.com/ItsJazii/pane) was
> **removed**, not disabled. There is no analytics SDK, no daily statistic, no
> random install id and no `telemetry.rs`. An earlier version of this very
> document described that pipeline in detail; it was inherited copy and it was
> wrong about this app. `grep -ri posthog crates/ src-tauri/ src/` returns nothing.

## Every network call the app can make

This is the complete list. Anything not listed here does not happen.

| Destination | When | What is sent |
|---|---|---|
| Each provider's own API (Anthropic, OpenAI/ChatGPT, cursor.com, GitHub, x.ai, opencode.ai, Devin, MiniMax, OpenRouter, Z.ai, Google, DeepSeek, Moonshot, Kimi, ElevenLabs, Codebuff, Kilo, AihubMix, Alibaba Model Studio) | Every refresh, for providers you have enabled | That provider's own token or key, exactly as its official tool would send it. Per-provider detail: [providers.md](providers.md) |
| One/New API and Sub2API origins **you configured** | Their scheduled refresh; a status probe when you save or change a site | That site's key as Bearer, **only to that origin**, with redirects disabled and no fallback endpoints |
| `raw.githubusercontent.com` (LiteLLM), `models.dev`, `robinebers.github.io` | About daily, hourly while unknown models are around | Anonymous GET for public model price tables. Nothing identifying, no key, no usage |
| `staas.fund/mcp/scanner/servers.json` | **Only if you switch on the MCP Trust Index** (`trustLookup`, off by default). At most once a day | One parameterless GET of a public list. Nothing about your machine, your servers or your identity is sent; the matching happens locally after the list arrives |
| `127.0.0.1:11434` (your own machine) | Every refresh, if Ollama is enabled | A local query of your own Ollama server |

Notably absent: analytics, session recording, event streams, A/B flags,
autocapture, crash reporting, an install counter, and an update check. None of
it exists in the codebase.

## Updates do not phone home either

**Self-update is off** (`UPDATES_ENABLED` in `src-tauri/src/lib.rs`). The
updater is not registered, so the app makes no update request at all, not at
launch and not in the background.

The endpoint strings still in the source belong to the upstream project and are
the reason the feature stays off: pointing an updater at someone else's feed
would replace this app with theirs. Before updates are ever switched on, those
endpoints have to be replaced with our own. See `SHIPPING.md`.

## What stays on your machine

Settings, saved keys and caches live in one directory:

- macOS: `~/Library/Application Support/AITaskManager`
- Windows: `%APPDATA%\AITaskManager`

That is deliberately **not** upstream's directory. On macOS, OpenUsage is a
separate real application whose settings must never be touched, and sharing a
directory with upstream Pane would let the two apps overwrite each other.

- **Credentials** are read from the files the official CLIs already maintain
  (see [providers.md](providers.md)). Keys you paste live in
  `<config dir>/<provider>.json`, written owner-only (Unix `0600`, a protected
  DACL on Windows) so a permissive parent directory cannot expose them.
  Settings lists names, URLs and key labels only, never a secret or a fragment
  of one.
- **OS credential stores are read-only.** On macOS the Claude provider reads
  the login Keychain and deliberately never refreshes the token: refreshing
  rotates it, which would sign Claude Code out from under you.
- **Refreshed OAuth tokens** are written back to the CLIs' own credential files
  so your tools stay signed in, which is what the CLIs do themselves.
- **Usage snapshots and the spend cache** are stored locally so the app opens
  instantly. They are never uploaded.
- **Spend is computed by reading your CLIs' local logs.** The logs never leave
  your machine; only the public price tables are downloaded. Claude Code keeps
  conversation titles derived from your prompts in those same logs, and the
  scanner skips them on purpose.
- **Folder and client names stay local.** Work areas are your directory names
  and clients are your customers' names. Neither is published anywhere by
  default.

## What the app writes outside its own folder

Exactly two things, each behind an explicit confirmed click:

1. **Ending a running MCP server.** You name a server, never a process id. The
   process ids come from a snapshot taken inside that same call, so a recycled
   id cannot be hit, and only a process already matched as an MCP server is
   reachable. It asks politely (`SIGTERM`, or `taskkill` with no `/F`) and
   **never escalates**: a server that ignores it keeps running and the next
   refresh says so.
2. **Pinning an unpinned MCP package to a version.** The version comes from
   your local package cache, never a registry call. It replaces one JSON string
   byte for byte, re-parses to prove that string inside `args` is the only
   difference, refuses if the file's size or modification time moved since the
   preview, and leaves a backup beside it.

Reading your MCP configuration never copies a value out of it. Those files
routinely hold API keys in `env`, `args`, `headers`, URL paths and query
strings. A test plants secrets in every one of those fields and asserts that
none of them reach any output.

## The local HTTP API

`http://127.0.0.1:6736/v1/usage` exists so your own scripts, widgets and
overlays can read your usage.

- **Loopback only.** Nothing on your network can reach it.
- **No CORS headers**, and non-loopback `Host` headers are refused, so a
  website you visit cannot read it through your browser, not even via DNS
  rebinding.
- **Usage numbers only.** Never credentials, keys, dashboard URLs or origins.
- **The extra feeds are opt-in and off by default** (`apiFeeds`, in Settings >
  Advanced): `/v1/spend`, `/v1/spend/areas`, `/v1/spend/clients` and
  `/v1/subscriptions`. Work areas are your folder names and clients are your
  customers', so while that switch is off those paths return **404 rather than
  an empty result**: nothing is published at all.

Details: [local-http-api.md](local-http-api.md).

## Verifying all of this

This repository is the entire codebase. Every outbound HTTP call site lives in
one of four places, and there are no others:

- a provider module, [`crates/core/src/providers/`](../crates/core/src/providers/)
- the pricing engine, [`crates/core/src/pricing.rs`](../crates/core/src/pricing.rs)
- the MCP Trust Index lookup, [`crates/core/src/trust.rs`](../crates/core/src/trust.rs)
- the updater registration, [`src-tauri/src/lib.rs`](../src-tauri/src/lib.rs), which is switched off

Useful greps:

```sh
# No analytics, anywhere:
grep -ri posthog crates/ src-tauri/ src/

# Every destination that is not a provider's own vendor API:
grep -rhoE '"https://[a-zA-Z0-9./_-]+' --include="*.rs" \
  crates/core/src/pricing.rs crates/core/src/trust.rs src-tauri/src/lib.rs | sort -u
```

The second command prints seven URLs: three public price tables, the Trust
Index list and its human-readable page, and **two updater endpoints belonging
to the upstream project**. Those last two are exactly why self-update is off,
and they are left visible here rather than quietly deleted so that what the
source contains and what this page says stay the same thing.

The enterprise half has its own boundary. The per-seat report
([`crates/core/src/seat.rs`](../crates/core/src/seat.rs)) has no field that can
hold a prompt, path, folder, work area, client name, session id or credential,
and a test plants each of those in its inputs to prove they cannot appear. It
is sent only to a collector you run yourself, and `aitm-agent --no-limits`
skips the vendor calls entirely.
