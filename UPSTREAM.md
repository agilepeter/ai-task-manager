# Upstream provenance

Clean copy (not a GitHub fork) of https://github.com/ItsJazii/pane

- Upstream commit: b00834d5babc822c60694b1fece6ae8bb729b149
- Upstream commit date: 2026-09-18T12:59:57-07:00
- Imported: 2026-09-20
- License: MIT. The original copyright notices are preserved in LICENSE, word
  for word, each on one line:
  - Jazii, 2026: Pane for Windows, the tray app this is a clean copy of.
  - Robin Ebers, 2025: OpenUsage for macOS, provider research and original
    concept, https://github.com/robinebers/openusage
  - Peter Steinberger, 2025: CodexBar for macOS, provider research for
    Codebuff, Kilo, Kiro, Amp, Vertex AI, Bedrock, Poe, Chutes, Warp and Crof,
    https://github.com/steipete/CodexBar

  Each notice is kept on a single line in LICENSE on purpose. A notice that
  wraps onto a second line stops GitHub from recognising the file as MIT, and
  the repository is then listed as "Other".

Upstream's own release history, up to the imported commit, is kept verbatim in
[docs/upstream-changelog.md](docs/upstream-changelog.md). This app's history
restarts at 0.1.0 in [CHANGELOG.md](CHANGELOG.md).

To pull a provider fix from upstream, diff the single provider file under
`crates/core/src/providers/` against this commit and port it by hand. (The
provider code lived in `src-tauri/src/providers/` at import time; it moved when
the Tauri-free core crate was split out.)
