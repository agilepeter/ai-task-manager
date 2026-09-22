# Upstream provenance

Clean copy (not a GitHub fork) of https://github.com/ItsJazii/pane

- Upstream commit: b00834d5babc822c60694b1fece6ae8bb729b149
- Upstream commit date: 2026-09-18T12:59:57-07:00
- Imported: 2026-09-20
- License: MIT. Original copyright notices are preserved in LICENSE
  (Jazii, Pane for Windows; Robin Ebers, OpenUsage for macOS).

Upstream's own release history, up to the imported commit, is kept verbatim in
[docs/upstream-changelog.md](docs/upstream-changelog.md). This app's history
restarts at 0.1.0 in [CHANGELOG.md](CHANGELOG.md).

To pull a provider fix from upstream, diff the single provider file under
`crates/core/src/providers/` against this commit and port it by hand. (The
provider code lived in `src-tauri/src/providers/` at import time; it moved when
the Tauri-free core crate was split out.)
