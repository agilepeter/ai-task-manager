# Security Policy

AI Task Manager reads the credential files that AI CLIs and editors keep on
your computer. That is a serious responsibility, and this page explains how
the app handles it and how to reach the maintainer when something looks
wrong.

## Reporting a vulnerability

**Please do not open a public issue for security problems.**

Use GitHub's private vulnerability reporting: go to the
[Security tab](https://github.com/agilepeter/ai-task-manager/security) ->
*Report a vulnerability*. This is a small project maintained by one person,
so please allow a few days for a response.

Please include: what you found, how to reproduce it, and what an attacker
could do with it.

## Security properties you can verify

All of this is auditable in the source; links go to the exact code.

- **Tokens never leave their lane.** Each provider's credential is sent only
  to that provider's own API over HTTPS
  ([`crates/core/src/providers/`](crates/core/src/providers/) -- one module
  per provider; see [docs/providers.md](docs/providers.md) for the exact
  files read and endpoints called).
- **OS credential stores are read-only.** On macOS the Claude provider reads
  the login Keychain and never refreshes the token, because refreshing
  rotates it and would sign Claude Code out from under you. Where a CLI
  keeps its login in a flat file instead (Codex's `~/.codex/auth.json`, for
  example), the app writes a refreshed OAuth token back to that same file so
  the CLI stays signed in -- the same thing the CLI itself would do -- and
  never to anywhere else.
- **No analytics SDK, no crash reporter, no event streams.** Upstream's
  PostHog telemetry module was removed, not disabled: there is no
  `telemetry.rs` in this codebase and no daily statistic, random install id,
  or autocapture. `grep -ri posthog crates/ src-tauri/ src/` returns
  nothing. The app's entire self-reporting surface is two anonymous,
  **opt-in and off by default** exceptions, both documented field by field
  in [docs/privacy.md](docs/privacy.md): the MCP Trust Index lookup
  (`trustLookup`) and the update check (`updateChecks`). Neither sends
  anything about your machine; each is one parameterless request, at most a
  few times a day, only while its switch is on.
- **The local HTTP API is loopback-only, CORS-locked, and Host-checked.**
  It binds `127.0.0.1:6736`, serves usage numbers only, never credentials,
  sends no `Access-Control-Allow-Origin` header, and refuses requests whose
  `Host` header is not a loopback spelling -- so a web page you visit cannot
  read it from a browser, not even via DNS rebinding
  ([`crates/core/src/httpapi.rs`](crates/core/src/httpapi.rs)). Its extra
  feeds (work areas, clients, subscriptions) are opt-in and off by default;
  while off, those paths return 404 rather than an empty result, so nothing
  about your folders or customers is published at all.
- **The app changes your machine in exactly two places, both behind an
  explicit confirmed click.** Ending a running MCP server: you name a
  server, never a process id: the ids come from a snapshot taken inside that
  same call, so a recycled id cannot be hit, and only a process already
  matched as an MCP server is reachable; it asks politely (`SIGTERM`, or
  `taskkill` with no `/F`) and never escalates. Pinning an unpinned MCP
  package to a version: the version comes from your local package cache,
  never a registry call; it replaces one JSON string byte for byte,
  re-parses to prove that string is the only difference, refuses if the
  file's size or modification time moved since the preview, and leaves a
  backup beside it.
- **Reading your MCP configuration never copies a value out of it.** Those
  files routinely hold API keys in `env`, `args`, `headers`, URL paths and
  query strings. A test plants secrets in every one of those fields and
  asserts that none of them reach any output.
- **Config writes are schema-bound.** The UI can only write config keys the
  app already knows about; anything else is dropped and logged rather than
  written.
- **Links open in your default browser and nowhere else.** `open_link`
  refuses anything that is not an `http://` or `https://` URL, so a crafted
  link cannot launch a local program.
- **The webview is locked down.** A strict Content-Security-Policy (no
  remote scripts, no eval, no frames: `default-src 'self'`, `script-src
  'self'`, `object-src 'none'`, `frame-src 'none'`) plus a minimal Tauri
  capability set -- the frontend is granted exactly three permissions
  (listening for Rust events, unlistening, and reading the app version) and
  explicitly not the broader default set, because that would additionally
  expose arbitrary local file reads
  ([`src-tauri/tauri.conf.json`](src-tauri/tauri.conf.json),
  [`src-tauri/capabilities/default.json`](src-tauri/capabilities/default.json)).
- **API keys you paste** are stored in this app's own config directory
  (`~/Library/Application Support/AITaskManager` on macOS,
  `%APPDATA%\AITaskManager` on Windows -- never upstream's directory, so it
  can never collide with a separate OpenUsage install), written owner-only
  (Unix `0600`, a protected DACL on Windows) and sent only to their own
  vendor. One/New API and Sub2API sites and keys use a nested owner-only
  store; Settings returns labels and origins after save, never a secret
  value or a fragment of one.

## Known limitations (honesty section)

- **The installer is not yet code-signed** on either platform: macOS
  Gatekeeper reports "no usable signature" and Windows SmartScreen warns on
  first run. A certificate for each platform is the last step before a
  public release; see [SHIPPING.md](SHIPPING.md) for exactly what each one
  needs. Until then, building from source (the README's supported path) is
  also the more verifiable one: you can read what you are running before you
  run it.
- **Release binaries are built by GitHub Actions from the pushed tag** --
  see [.github/workflows/release.yml](.github/workflows/release.yml); once a
  release exists, its build logs are public and show the binary came from
  the tagged source.
- **Self-update is opt-in and off by default** (`updateChecks`, a Settings >
  Network toggle). While it is off, the app makes no update request at all.
  Once it is on, updates are verified with this project's own minisign key
  before they install; the public half is baked into the app and was
  checked byte for byte against the key file when it was set up. The
  private half is password-protected and never in this repository.
- The app refreshes OAuth tokens and writes them back to a CLI's own
  credential file, keeping that CLI signed in. This means the app has the
  same access to that account as the CLI itself does -- that is inherent to
  what the feature does, for every tool that works this way, not specific to
  this app.
- Model prices (LiteLLM, models.dev, and a small OpenUsage-derived
  supplement) are fetched without signatures; tampered pricing data could at
  worst show the wrong *display* dollar amount. Inputs are size-capped and
  never touch credentials or spend logs.

## Supported versions

This project has not cut a release yet (see [SHIPPING.md](SHIPPING.md)).
Once it has, only the
[latest release](https://github.com/agilepeter/ai-task-manager/releases/latest)
is supported, and the auto-updater (once you opt in) keeps an install
current.

---

This page's shape is adapted from [Pane](https://github.com/ItsJazii/pane)'s
own SECURITY.md (MIT). Full credits are in the README's
[Credits](README.md#credits) section and in [LICENSE](LICENSE).
