# Contributing

Thanks for looking at AI Task Manager. A few ground rules keep this easy for
everyone.

## Issues first

Open an issue before writing code -- especially for a new provider or a
larger feature. It saves you building something that will not get merged.
Bug reports with reproduction steps are always welcome; this repository is
public, so anyone can file one.

## Security issues

Never in public issues -- see [SECURITY.md](SECURITY.md) (GitHub private
vulnerability reporting).

## Building

```sh
npm install
npm run tauri dev     # run with hot reload
npm run tauri build   # produce an installer
```

Full prerequisites and per-platform steps are in the README's
[Install](README.md#install) section. Rust changes need a rebuild and a
relaunch of the app: it is a long-lived tray process, and `tauri dev` only
watches `src-tauri/`.

## Running the gates

```sh
npm test                # frontend + i18n dictionary tests
cargo test --workspace  # Rust, every crate
```

Both must stay green. CI runs both suites too, routed by what changed: a
Rust or manifest change gets the Windows build and test run, a UI change
gets a fast Linux type-check, build and `npm test`. `npm test` already
includes the Settings panel's i18n coverage check, so a Settings row that
ships with no translation key fails there rather than shipping in English.
If your change touches `index.html`, also run the 380 px layout harness
(`scripts/layout-check.mjs`, instructions in its own header comment) and
look at the screenshots it writes -- it catches overflow the coverage check
cannot.

## The two-crate shape

`crates/core` (`aitm-core`) holds every provider, the spend engine, pricing,
i18n and the local HTTP API, and knows nothing about Tauri or the UI.
`src-tauri` (`ai-task-manager`) is the tray app itself: the webview, the
tray icon, and the Tauri commands the frontend calls. A headless agent and a
self-hosted collector (`crates/agent`, `crates/collector`) reuse `aitm-core`
the same way the tray app does. When new logic doesn't obviously belong to
one: if it needs a window, it belongs in `src-tauri`; almost everything else
belongs in `aitm-core` so the headless agent can use it too.

## Adding user-facing text

Every string a person reads goes through the translator, never a literal in
markup or a template string. Add one key to **all nine** files in
`src/locales/` (`en`, `zh`, `ru`, `es`, `fr`, `de`, `ja`, `pt-BR`, `ko`) --
English plus a real translation in the other eight, not a copy of the
English value. `npm test` checks that every locale has exactly the same set
of keys as English and that no value is empty, so a PR that adds a key to
only `en.json` fails the suite instead of shipping half-translated.
[CLAUDE.md](CLAUDE.md)'s Views section has the full convention, including
how plural forms work and how a Rust-authored sentence reaches the screen.

## Pull requests

- Keep PRs focused: one change per PR.
- Describe **what was happening** and **what this changes** in plain
  English; screenshots for anything visual.
- Provider PRs follow the house rules: credentials are read only from where
  the official tool already stores them, and are sent only to that vendor's
  own API. One/New API and Sub2API are explicit manual-key exceptions:
  users supply the key and site address, keys are stored in owner-only
  local files, and requests go only to that configured origin. These
  exceptions do not authorize other credential sources or destinations.
  Every new provider gets a section in
  [docs/providers.md](docs/providers.md) documenting exactly what it reads
  and calls.
- No telemetry, analytics SDKs, or "phone home" code -- PRs adding any will
  be declined regardless of intent. Two deliberate, documented exceptions
  exist, both off by default and both covered field by field in
  [docs/privacy.md](docs/privacy.md): the MCP Trust Index lookup and the
  opt-in update check. Neither is a precedent for a third: users' quotas,
  usage amounts, spend and error text never leave their PC, and a PR
  widening what either exception carries, or adding any analytics SDK, will
  be declined.
- No new dependencies without a stated reason.

---

Some of this file's structure is adapted from [Pane](https://github.com/ItsJazii/pane)'s
own CONTRIBUTING.md (MIT). Full credits are in the README's
[Credits](README.md#credits) section and in [LICENSE](LICENSE).
