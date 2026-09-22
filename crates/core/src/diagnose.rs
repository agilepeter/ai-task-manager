//! "Why is this card empty?"
//!
//! A provider that cannot sign in says so, but not *where it looked*. On this
//! machine only Claude and Copilot have been checked end to end; the rest came
//! across from the Windows original, so "sign-in not found" can equally mean
//! "you are not signed in" or "we looked in the wrong place". Those two
//! deserve different reactions from the user, and only this module can tell
//! them apart: it lists each place the provider reads and whether it is there.
//!
//! The table below is written by hand beside the providers rather than
//! extracted from them. That is a real cost — it can drift — so `paths_exist`
//! keeps the two honest for the providers whose layout is settled, and every
//! row carries whether this platform has actually been verified.
//!
//! Paths are shown with the home directory as `~`. They stay in the UI: the
//! seat report has no field for them, and `never_carries_*` in `seat.rs`
//! keeps it that way.

use std::path::PathBuf;

use serde::Serialize;

/// One place a provider looks for its sign-in.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    /// "file" | "keychain" | "folder"
    pub kind: String,
    /// Where, with the home directory shortened to `~`.
    pub location: String,
    /// Whether it is there right now. A keychain entry is never opened to
    /// find out: reading it would prompt, and this view must stay silent.
    pub found: Option<bool>,
}

/// What the app knows about one provider's sign-in on this machine.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Diagnosis {
    pub id: String,
    pub name: String,
    pub probes: Vec<Probe>,
    /// Has this provider been checked end to end on this operating system?
    pub verified_here: bool,
    /// What to do, when nothing was found.
    pub hint: String,
}

impl Diagnosis {
    /// Nothing the provider reads is present.
    pub fn nothing_found(&self) -> bool {
        !self.probes.is_empty() && self.probes.iter().all(|p| p.found == Some(false))
    }
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

/// `/Users/dana/.codex/auth.json` → `~/.codex/auth.json`.
pub fn tilde(path: &std::path::Path, home: &std::path::Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

fn file(path: PathBuf) -> Probe {
    Probe { kind: "file".into(), found: Some(path.exists()), location: tilde(&path, &home()) }
}

/// Every place each locally-signed-in provider reads, in the order it reads
/// them. Providers that need a key pasted into the app are left out: there is
/// nothing to look for and nothing to explain.
pub fn all() -> Vec<Diagnosis> {
    let h = home();
    let claude_dir = std::env::var("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|_| h.join(".claude"));
    let codex_dir = std::env::var("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|_| h.join(".codex"));

    let mut claude = vec![file(claude_dir.join(".credentials.json"))];
    if cfg!(target_os = "macos") {
        // Claude Code keeps the sign-in in the login Keychain on macOS. Whether
        // it is there is deliberately not tested: `security find-generic-password`
        // can prompt, and a diagnostics panel must never pop a dialog.
        claude.push(Probe {
            kind: "keychain".into(),
            location: "login Keychain, \"Claude Code-credentials\"".into(),
            found: None,
        });
    }

    let mut copilot = Vec::new();
    if cfg!(windows) {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            copilot.push(file(PathBuf::from(&local).join("github-copilot").join("apps.json")));
            copilot.push(file(PathBuf::from(&local).join("github-copilot").join("hosts.json")));
        }
    } else {
        copilot.push(file(h.join(".config").join("github-copilot").join("apps.json")));
        copilot.push(file(h.join(".config").join("github-copilot").join("hosts.json")));
    }

    let cursor_db = if cfg!(target_os = "macos") {
        h.join("Library").join("Application Support").join("Cursor").join("User").join("globalStorage").join("state.vscdb")
    } else if cfg!(windows) {
        std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Cursor")
            .join("User")
            .join("globalStorage")
            .join("state.vscdb")
    } else {
        h.join(".config").join("Cursor").join("User").join("globalStorage").join("state.vscdb")
    };

    vec![
        Diagnosis {
            id: "claude".into(),
            name: "Claude".into(),
            probes: claude,
            verified_here: true,
            hint: "Run `claude` in a terminal and sign in.".into(),
        },
        Diagnosis {
            id: "codex".into(),
            name: "Codex".into(),
            probes: vec![file(codex_dir.join("auth.json"))],
            verified_here: cfg!(windows),
            hint: "Run `codex login` in a terminal.".into(),
        },
        Diagnosis {
            id: "copilot".into(),
            name: "GitHub Copilot".into(),
            probes: copilot,
            verified_here: true,
            hint: "Sign in to Copilot in your editor, or run `gh auth login`.".into(),
        },
        Diagnosis {
            id: "cursor".into(),
            name: "Cursor".into(),
            probes: vec![file(cursor_db)],
            verified_here: cfg!(windows),
            hint: "Open Cursor and sign in; the app reads its local database.".into(),
        },
    ]
}

/// Prints what this machine actually shows. Ignored: it reads the filesystem.
#[test]
#[ignore]
fn live_diagnose() {
    for d in all() {
        println!("{} ({}){}", d.name, d.id, if d.verified_here { "" } else { "  [unverified on this OS]" });
        for p in &d.probes {
            let mark = match p.found {
                Some(true) => "found",
                Some(false) => "missing",
                None => "not opened",
            };
            println!("    {:<10} {}", mark, p.location);
        }
        if d.nothing_found() {
            println!("    -> {}", d.hint);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortens_the_home_directory() {
        let h = PathBuf::from("/Users/dana");
        assert_eq!(tilde(&h.join(".codex/auth.json"), &h), "~/.codex/auth.json");
        assert_eq!(tilde(&PathBuf::from("/opt/x"), &h), "/opt/x", "outside home, shown whole");
    }

    #[test]
    fn every_provider_says_where_it_looks_and_what_to_do() {
        for d in all() {
            assert!(!d.probes.is_empty(), "{} lists nowhere to look", d.id);
            assert!(!d.hint.is_empty(), "{} has no hint", d.id);
            for p in &d.probes {
                assert!(!p.location.is_empty());
                assert!(matches!(p.kind.as_str(), "file" | "keychain" | "folder"), "{}", p.kind);
            }
        }
    }

    #[test]
    fn the_keychain_is_described_but_never_opened() {
        let claude = all().into_iter().find(|d| d.id == "claude").expect("claude");
        if cfg!(target_os = "macos") {
            let k = claude.probes.iter().find(|p| p.kind == "keychain").expect("a keychain probe on macOS");
            assert_eq!(k.found, None, "opening it could prompt, so it is never tested");
        }
    }

    #[test]
    fn honest_about_what_this_platform_has_proven() {
        let by = |id: &str| all().into_iter().find(|d| d.id == id).expect("present");
        // CLAUDE.md: only Claude and Copilot are verified end to end on macOS.
        assert!(by("claude").verified_here);
        assert!(by("copilot").verified_here);
        if cfg!(target_os = "macos") {
            assert!(!by("codex").verified_here, "unverified on macOS, and it must say so");
            assert!(!by("cursor").verified_here);
        }
    }

    #[test]
    fn paths_exist_reports_a_real_answer_for_files() {
        for d in all() {
            for p in d.probes.iter().filter(|p| p.kind == "file") {
                assert!(p.found.is_some(), "a file probe must answer yes or no");
            }
        }
    }

    #[test]
    fn nothing_found_needs_every_probe_missing() {
        let mut d = Diagnosis {
            id: "x".into(),
            name: "X".into(),
            probes: vec![
                Probe { kind: "file".into(), location: "a".into(), found: Some(false) },
                Probe { kind: "file".into(), location: "b".into(), found: Some(true) },
            ],
            verified_here: true,
            hint: "h".into(),
        };
        assert!(!d.nothing_found(), "one hit is enough to be signed in");
        d.probes[1].found = Some(false);
        assert!(d.nothing_found());
        // An untested keychain entry is not a miss.
        d.probes.push(Probe { kind: "keychain".into(), location: "k".into(), found: None });
        assert!(!d.nothing_found(), "we cannot claim nothing is there when one was never opened");
    }
}
