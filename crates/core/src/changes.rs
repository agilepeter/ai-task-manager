//! A dated history of the setup's own SHAPE (which servers, agents, skills,
//! hook events exist, and the size of the deny/allow lists), kept locally so
//! the Inventory tab can say what changed since about a week ago, and a
//! weakened guardrail can be flagged the moment it shows up.
//!
//! Only shapes and counts ever go in a snapshot -- the same rule
//! `inventory.rs` already holds for a live scan, so a snapshot is exactly as
//! safe to keep on disk for weeks as a single scan is to hold in memory for
//! a few seconds. Nothing here calls out, and nothing here is read by the
//! seat report.

use crate::i18n::{self, Msg};
use crate::inventory::{Inventory, Opportunity};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Snapshots older than this, measured from the one just recorded, are
/// dropped on the next write. Local disk only; nothing this old is worth
/// keeping around forever.
pub const KEEP_DAYS: i64 = 35;
/// How far back "about a week ago" reaches when picking which stored
/// snapshot to compare today against.
pub const COMPARE_DAYS: i64 = 7;

/// One MCP server's shape on the day a snapshot was taken. Never the
/// target host, the project path, or an env count: those can change for
/// reasons that have nothing to do with what this file is for, and the
/// project path in particular is exactly the kind of thing that must never
/// sit in a file kept around for weeks.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SnapServer {
    pub name: String,
    pub client: String,
    pub transport: String,
    pub package: Option<String>,
    /// `None` when there is no package to have a version at all (a remote
    /// server, or a stdio command that is not a package runner).
    pub pinned: Option<bool>,
}

/// The whole setup's shape on one day, local time.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// `YYYY-MM-DD`, local. One snapshot per day: recording again on the
    /// same day replaces it rather than adding a second one.
    pub taken: String,
    pub servers: Vec<SnapServer>,
    pub agents: Vec<String>,
    pub skills: Vec<String>,
    /// (event name, how many hooks are registered for it).
    pub hook_events: Vec<(String, usize)>,
    pub deny: usize,
    pub allow: usize,
    pub deny_covers_shell: bool,
}

/// One thing that is different between two snapshots.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    /// "added" | "removed" | "changed"
    pub kind: String,
    /// "server" | "agent" | "skill" | "hook" | "deny" | "allow" | "shell"
    pub what: String,
    /// The server, agent, skill or hook event's own name. Empty for the
    /// setup-wide deny/allow/shell rows, which name no single thing.
    pub name: String,
    /// The key + vars the Changes section paints in the active locale.
    pub msg: Msg,
    /// English, produced by `render("en", &msg)` of the same Msg -- never a
    /// second literal, so the two can never disagree.
    pub text: String,
}

/// One day's worth of history plus the changes found against it.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SetupChanges {
    /// The date being compared against. `None` when there is no earlier
    /// snapshot to compare with at all -- the popover reads that as "history
    /// starts today", not as an error.
    pub since: Option<String>,
    pub changes: Vec<Change>,
    /// How many days `since` actually is before today. Normally close to
    /// `COMPARE_DAYS`; honestly smaller when the app has not been recording
    /// long enough yet.
    pub days_of_history: i64,
}

/// Every finding id this module can emit, for the same test-side registry
/// `inventory::FINDING_IDS` documents. Only `i18n.rs`'s test module reads
/// this, so it does not exist in a release build at all.
#[cfg(test)]
pub(crate) const FINDING_IDS: &[&str] = &["guardrail-removed", "setup-changed"];

const STORE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Store {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    snapshots: Vec<Snapshot>,
}

fn store_path(dir: &Path) -> std::path::PathBuf {
    dir.join("inventory_snapshots.json")
}

/// Whole calendar days from `from` to `to` (`YYYY-MM-DD`, local). `None`
/// when either string does not parse -- a caller treats that exactly like
/// "does not qualify" rather than guessing a distance.
fn days_between(from: &str, to: &str) -> Option<i64> {
    let a = chrono::NaiveDate::parse_from_str(from, "%Y-%m-%d").ok()?;
    let b = chrono::NaiveDate::parse_from_str(to, "%Y-%m-%d").ok()?;
    Some((b - a).num_days())
}

/// Today's setup, reduced to the shape a snapshot keeps. Every field here
/// already went through `inventory::scan()`'s own no-values rule, so there
/// is nothing left to strip -- this only picks which fields matter for
/// spotting a change, and sorts each list so two runs over an identical
/// setup always produce byte-identical snapshots.
pub fn snapshot_of(inv: &Inventory, today: &str) -> Snapshot {
    let mut servers: Vec<SnapServer> = inv
        .mcp_servers
        .iter()
        .map(|s| SnapServer {
            name: s.name.clone(),
            client: s.client.clone(),
            transport: s.transport.clone(),
            package: s.package.clone(),
            pinned: s.package.as_deref().map(crate::inventory::is_pinned),
        })
        .collect();
    servers.sort_by(|a, b| (a.name.as_str(), a.client.as_str()).cmp(&(b.name.as_str(), b.client.as_str())));

    let mut agents: Vec<String> = inv.agents.iter().map(|a| a.name.clone()).collect();
    agents.sort();
    let mut skills: Vec<String> = inv.skills.iter().map(|s| s.name.clone()).collect();
    skills.sort();
    let mut hook_events: Vec<(String, usize)> = inv.hooks.iter().map(|h| (h.event.clone(), h.count)).collect();
    hook_events.sort_by(|a, b| a.0.cmp(&b.0));

    Snapshot {
        taken: today.to_string(),
        servers,
        agents,
        skills,
        hook_events,
        deny: inv.permissions.deny,
        allow: inv.permissions.allow,
        deny_covers_shell: inv.permissions.deny_covers_shell,
    }
}

/// The one place a `Change` is built: `text` is always this Msg's English
/// rendering, so it can never drift from what the popover shows in another
/// locale.
fn change(kind: &str, what: &str, name: &str, msg: Msg) -> Change {
    Change { kind: kind.into(), what: what.into(), name: name.into(), text: i18n::render("en", &msg), msg }
}

fn server_key(s: &SnapServer) -> (&str, &str) {
    (s.name.as_str(), s.client.as_str())
}

fn diff_servers(old: &[SnapServer], new: &[SnapServer], out: &mut Vec<Change>) {
    for s in new {
        if !old.iter().any(|o| server_key(o) == server_key(s)) {
            out.push(change("added", "server", &s.name, Msg::new("changes.server.added").var("name", &s.name)));
        }
    }
    for s in old {
        if !new.iter().any(|n| server_key(n) == server_key(s)) {
            out.push(change("removed", "server", &s.name, Msg::new("changes.server.removed").var("name", &s.name)));
        }
    }
    for s in new {
        let Some(o) = old.iter().find(|o| server_key(o) == server_key(s)) else { continue };
        if o.transport != s.transport || o.package != s.package || o.pinned != s.pinned {
            out.push(change("changed", "server", &s.name, Msg::new("changes.server.changed").var("name", &s.name)));
        }
    }
}

/// Agents and skills are both a plain list of user-chosen names, so one walk
/// serves both -- only the `what` tag and which two message keys apply
/// differ between the two callers below.
fn diff_names(old: &[String], new: &[String], what: &'static str, added_key: &'static str, removed_key: &'static str, out: &mut Vec<Change>) {
    for name in new {
        if !old.contains(name) {
            out.push(change("added", what, name, Msg::new(added_key).var("name", name)));
        }
    }
    for name in old {
        if !new.contains(name) {
            out.push(change("removed", what, name, Msg::new(removed_key).var("name", name)));
        }
    }
}

/// A hook event can appear, disappear, or keep existing with a different
/// number of hooks registered under it. Fewer hooks reads the same as the
/// event disappearing entirely for the guardrail check below -- both mean
/// less of the setup is watching that event than before.
fn diff_hooks(old: &[(String, usize)], new: &[(String, usize)], out: &mut Vec<Change>) {
    for (event, new_count) in new {
        match old.iter().find(|(e, _)| e == event) {
            None => out.push(change("added", "hook", event, Msg::new("changes.hook.added").var("event", event))),
            Some((_, old_count)) if new_count < old_count => {
                out.push(change("removed", "hook", event, Msg::new("changes.hook.fewer").var("event", event)))
            }
            Some((_, old_count)) if new_count > old_count => {
                out.push(change("added", "hook", event, Msg::new("changes.hook.more").var("event", event)))
            }
            _ => {}
        }
    }
    for (event, _) in old {
        if !new.iter().any(|(e, _)| e == event) {
            out.push(change("removed", "hook", event, Msg::new("changes.hook.removed").var("event", event)));
        }
    }
}

/// Deny going down, or shell coverage being lost, is a guardrail weakening
/// (`kind == "removed"`); either moving the other way is just informational.
/// Allow is tracked for the Changes list but never counts as a guardrail on
/// its own -- more or fewer allow rules is a convenience change, not a
/// safety one.
fn diff_counts(old: &Snapshot, new: &Snapshot, out: &mut Vec<Change>) {
    if old.deny != new.deny {
        let kind = if new.deny < old.deny { "removed" } else { "added" };
        out.push(change(
            kind,
            "deny",
            "",
            Msg::new("changes.deny.count").var("old", old.deny as i64).var("new", new.deny as i64),
        ));
    }
    if old.allow != new.allow {
        let kind = if new.allow < old.allow { "removed" } else { "added" };
        out.push(change(
            kind,
            "allow",
            "",
            Msg::new("changes.allow.count").var("old", old.allow as i64).var("new", new.allow as i64),
        ));
    }
    if old.deny_covers_shell != new.deny_covers_shell {
        if new.deny_covers_shell {
            out.push(change("added", "shell", "", Msg::new("changes.shell.added")));
        } else {
            out.push(change("removed", "shell", "", Msg::new("changes.shell.removed")));
        }
    }
}

/// Every difference between two snapshots. Order is servers, then agents,
/// then skills, then hooks, then the deny/allow/shell counts -- stable so
/// two calls over the same pair always list changes in the same order.
pub fn diff(old: &Snapshot, new: &Snapshot) -> Vec<Change> {
    let mut out = Vec::new();
    diff_servers(&old.servers, &new.servers, &mut out);
    diff_names(&old.agents, &new.agents, "agent", "changes.agent.added", "changes.agent.removed", &mut out);
    diff_names(&old.skills, &new.skills, "skill", "changes.skill.added", "changes.skill.removed", &mut out);
    diff_hooks(&old.hook_events, &new.hook_events, &mut out);
    diff_counts(old, new, &mut out);
    out
}

/// Adds today's snapshot, replacing anything already on file for the same
/// day -- so a refresh loop that happens to record twice in one day never
/// grows the file -- then drops anything older than `KEEP_DAYS`, measured
/// from the day just recorded. Written with the same owner-only atomic swap
/// every other local settings file in this app uses, so a crash mid-write
/// can never leave a half-written history behind.
pub fn record_at(dir: &Path, snap: Snapshot) -> std::io::Result<()> {
    let path = store_path(dir);
    let mut store: Store = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    store.snapshots.retain(|s| s.taken != snap.taken);
    let taken = snap.taken.clone();
    store.snapshots.push(snap);
    store.snapshots.retain(|s| days_between(&s.taken, &taken).is_some_and(|d| (0..=KEEP_DAYS).contains(&d)));
    store.snapshots.sort_by(|a, b| a.taken.cmp(&b.taken));
    store.version = STORE_VERSION;
    let body = serde_json::to_string_pretty(&store).map_err(std::io::Error::other)?;
    std::fs::create_dir_all(dir)?;
    crate::providers::onenewapi::store::atomic_write(&path, &body).map_err(std::io::Error::other)
}

/// Every snapshot on file, oldest first. A missing or unreadable file (a
/// fresh install, or one hand-edited into garbage) reads as no history at
/// all, never as an error -- there is nothing here worth failing a scan
/// over.
pub fn load_from(dir: &Path) -> Vec<Snapshot> {
    std::fs::read_to_string(store_path(dir))
        .ok()
        .and_then(|raw| serde_json::from_str::<Store>(&raw).ok())
        .map(|s| s.snapshots)
        .unwrap_or_default()
}

/// Today's setup against whatever history is on file. Picks the newest
/// stored snapshot that is still at least `COMPARE_DAYS` old; when nothing
/// qualifies (the app has not been running that long yet), falls back to
/// the oldest snapshot on file and reports `days_of_history` honestly
/// instead of pretending it found a full week. With no snapshot from any
/// day before today at all, there is nothing to compare against: `since` is
/// `None` and `changes` is empty, which the popover reads as "history
/// starts today" rather than as a quiet setup.
pub fn changes_at(dir: &Path, inv: &Inventory, today: &str) -> SetupChanges {
    let new_snap = snapshot_of(inv, today);
    let history = load_from(dir);
    let mut candidates: Vec<&Snapshot> = history.iter().filter(|s| s.taken.as_str() != today).collect();
    if candidates.is_empty() {
        return SetupChanges { since: None, changes: Vec::new(), days_of_history: 0 };
    }
    candidates.sort_by(|a, b| a.taken.cmp(&b.taken));
    let old = candidates
        .iter()
        .rev()
        .find(|s| days_between(&s.taken, today).is_some_and(|d| d >= COMPARE_DAYS))
        .copied()
        .unwrap_or(candidates[0]);
    let days = days_between(&old.taken, today).unwrap_or(0);
    SetupChanges { since: Some(old.taken.clone()), changes: diff(old, &new_snap), days_of_history: days }
}

/// What the setup's own history is worth flagging. A deny rule dropping, a
/// hook event thinning out or disappearing, or shell coverage being lost is
/// a guardrail going away -- worth a look on its own, regardless of what
/// else changed. Everything else that changed becomes one more finding
/// naming only how many things, never which agent or skill: those names are
/// user-chosen and stay on the machine in the Changes list itself, which is
/// local-only, but a finding is the one sentence that could plausibly be
/// screenshotted or read aloud, so it stays to counts.
pub fn opportunities(c: &SetupChanges) -> Vec<Opportunity> {
    if c.changes.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let guardrail =
        c.changes.iter().filter(|ch| ch.kind == "removed" && matches!(ch.what.as_str(), "deny" | "shell" | "hook")).count();
    if guardrail > 0 {
        out.push(Opportunity::from_msgs(
            "guardrail-removed",
            "tighten",
            Msg::new("finding.guardrail-removed.title").count(guardrail as i64),
            Some(Msg::new("finding.guardrail-removed.detail").count(guardrail as i64)),
            None,
        ));
    }
    let other = c.changes.len() - guardrail;
    if other > 0 {
        out.push(Opportunity::from_msgs(
            "setup-changed",
            "learn",
            Msg::new("finding.setup-changed.title").count(other as i64),
            Some(Msg::new("finding.setup-changed.detail").count(other as i64)),
            None,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::{Inventory, Permissions};

    fn empty_snapshot(taken: &str) -> Snapshot {
        Snapshot {
            taken: taken.into(),
            servers: Vec::new(),
            agents: Vec::new(),
            skills: Vec::new(),
            hook_events: Vec::new(),
            deny: 0,
            allow: 0,
            deny_covers_shell: false,
        }
    }

    fn server(name: &str, package: Option<&str>, pinned: Option<bool>) -> SnapServer {
        SnapServer { name: name.into(), client: "Claude Code".into(), transport: "stdio".into(), package: package.map(str::to_string), pinned }
    }

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("aitm-changes-{tag}-{}", crate::providers::unique_stamp()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn diff_names_a_server_that_appeared() {
        let old = empty_snapshot("2026-09-01");
        let new = Snapshot { servers: vec![server("docs", Some("docs-mcp@1"), Some(true))], ..empty_snapshot("2026-09-08") };
        let found = diff(&old, &new);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "added");
        assert_eq!(found[0].what, "server");
        assert_eq!(found[0].name, "docs");
        assert!(found[0].text.contains("docs"), "{}", found[0].text);
    }

    #[test]
    fn diff_names_a_server_that_disappeared() {
        let old = Snapshot { servers: vec![server("docs", Some("docs-mcp@1"), Some(true))], ..empty_snapshot("2026-09-01") };
        let new = empty_snapshot("2026-09-08");
        let found = diff(&old, &new);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "removed");
        assert_eq!(found[0].what, "server");
        assert_eq!(found[0].name, "docs");
    }

    #[test]
    fn a_pin_lost_is_a_change() {
        let old = Snapshot { servers: vec![server("docs", Some("docs-mcp@1"), Some(true))], ..empty_snapshot("2026-09-01") };
        let new = Snapshot { servers: vec![server("docs", Some("docs-mcp"), Some(false))], ..empty_snapshot("2026-09-08") };
        let found = diff(&old, &new);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "changed");
        assert_eq!(found[0].what, "server");
        assert_eq!(found[0].name, "docs");
    }

    #[test]
    fn a_deny_rule_removed_is_a_guardrail() {
        let old = Snapshot { deny: 5, ..empty_snapshot("2026-09-01") };
        let new = Snapshot { deny: 3, ..empty_snapshot("2026-09-08") };
        let found = diff(&old, &new);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "removed");
        assert_eq!(found[0].what, "deny");
        let sc = SetupChanges { since: Some(old.taken.clone()), changes: found, days_of_history: 7 };
        let opps = opportunities(&sc);
        assert_eq!(opps.len(), 1);
        assert_eq!(opps[0].id, "guardrail-removed");
        assert_eq!(opps[0].kind, "tighten");
    }

    #[test]
    fn a_hook_event_gone_is_a_guardrail() {
        let old = Snapshot { hook_events: vec![("PreToolUse".into(), 1)], ..empty_snapshot("2026-09-01") };
        let new = empty_snapshot("2026-09-08");
        let found = diff(&old, &new);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "removed");
        assert_eq!(found[0].what, "hook");
        let sc = SetupChanges { since: Some(old.taken.clone()), changes: found, days_of_history: 7 };
        let opps = opportunities(&sc);
        assert_eq!(opps.len(), 1);
        assert_eq!(opps[0].id, "guardrail-removed");
        assert_eq!(opps[0].kind, "tighten");
    }

    #[test]
    fn nothing_changed_says_nothing() {
        let a = Snapshot {
            servers: vec![server("docs", Some("docs-mcp@1"), Some(true))],
            agents: vec!["reviewer".into()],
            skills: vec!["deploy".into()],
            hook_events: vec![("SessionEnd".into(), 1)],
            deny: 3,
            allow: 1,
            deny_covers_shell: true,
            taken: "2026-09-01".into(),
        };
        let b = Snapshot { taken: "2026-09-08".into(), ..a.clone() };
        assert!(diff(&a, &b).is_empty());

        let dir = tmp_dir("nothing");
        record_at(&dir, a).unwrap();
        let inv = Inventory {
            mcp_servers: vec![crate::inventory::McpServer {
                name: "docs".into(),
                client: "Claude Code".into(),
                scope: "user".into(),
                project: None,
                transport: "stdio".into(),
                target: "npx".into(),
                package: Some("docs-mcp@1".into()),
                env_count: 0,
                pin_to: None,
                source_file: None,
            }],
            agents: vec![crate::inventory::Definition { name: "reviewer".into(), scope: "user".into(), project: None, model: None, tools: None }],
            skills: vec![crate::inventory::Definition { name: "deploy".into(), scope: "user".into(), project: None, model: None, tools: None }],
            hooks: vec![crate::inventory::HookEvent { event: "SessionEnd".into(), count: 1 }],
            permissions: Permissions { default_mode: None, allow: 1, ask: 0, deny: 3, deny_covers_shell: true },
            ..Inventory::default()
        };
        let sc = changes_at(&dir, &inv, "2026-09-08");
        assert_eq!(sc.since.as_deref(), Some("2026-09-01"));
        assert!(sc.changes.is_empty(), "{:?}", sc.changes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_snapshot_a_day() {
        let dir = tmp_dir("oneaday");
        record_at(&dir, Snapshot { deny: 1, ..empty_snapshot("2026-09-20") }).unwrap();
        record_at(&dir, Snapshot { deny: 2, ..empty_snapshot("2026-09-20") }).unwrap();
        let stored = load_from(&dir);
        assert_eq!(stored.len(), 1, "recording twice on the same day never grows the file");
        assert_eq!(stored[0].deny, 2, "the later write for the same day wins");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_snapshots_are_pruned() {
        let dir = tmp_dir("prune");
        record_at(&dir, empty_snapshot("2026-01-01")).unwrap();
        record_at(&dir, empty_snapshot("2026-09-20")).unwrap();
        let stored = load_from(&dir);
        assert_eq!(stored.len(), 1, "a snapshot far past KEEP_DAYS is dropped once a newer one is recorded");
        assert_eq!(stored[0].taken, "2026-09-20");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreadable_store_is_empty_not_an_error() {
        let dir = tmp_dir("unreadable");
        std::fs::write(dir.join("inventory_snapshots.json"), "not json at all").unwrap();
        assert!(load_from(&dir).is_empty());
        let sc = changes_at(&dir, &Inventory::default(), "2026-09-08");
        assert_eq!(sc.since, None, "no readable history reads the same as no history at all");
        assert!(sc.changes.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compares_with_the_newest_one_a_week_old() {
        let dir = tmp_dir("compare");
        record_at(&dir, Snapshot { deny: 9, ..empty_snapshot("2026-09-10") }).unwrap(); // 15 days before "today"
        record_at(&dir, Snapshot { deny: 5, ..empty_snapshot("2026-09-18") }).unwrap(); // exactly 7 days before
        let today = "2026-09-25";
        let inv = Inventory { permissions: Permissions { deny: 3, ..Permissions::default() }, ..Inventory::default() };
        let sc = changes_at(&dir, &inv, today);
        assert_eq!(sc.since.as_deref(), Some("2026-09-18"), "the newest snapshot old enough to qualify, not the oldest on file");
        assert_eq!(sc.days_of_history, 7);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn less_than_a_week_of_history_says_so() {
        let dir = tmp_dir("thin");
        record_at(&dir, Snapshot { deny: 5, ..empty_snapshot("2026-09-23") }).unwrap(); // 2 days before "today"
        let today = "2026-09-25";
        let inv = Inventory { permissions: Permissions { deny: 3, ..Permissions::default() }, ..Inventory::default() };
        let sc = changes_at(&dir, &inv, today);
        assert_eq!(sc.since.as_deref(), Some("2026-09-23"), "nothing older on file, so this is what there is");
        assert_eq!(sc.days_of_history, 2, "honest about how little history exists, not rounded up to a week");
        assert!(!sc.changes.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_finding_never_names_an_agent_or_a_skill() {
        let old = empty_snapshot("2026-09-01");
        let new = Snapshot {
            agents: vec!["super-secret-reviewer".into()],
            skills: vec!["confidential-deploy-flow".into()],
            ..empty_snapshot("2026-09-08")
        };
        let found = diff(&old, &new);
        // The Changes list itself may legitimately name them: that is a
        // local-only, per-machine list, not a sentence handed to a finding.
        assert!(found.iter().any(|c| c.text.contains("super-secret-reviewer")));
        assert!(found.iter().any(|c| c.text.contains("confidential-deploy-flow")));

        let sc = SetupChanges { since: Some(old.taken.clone()), changes: found, days_of_history: 7 };
        for o in opportunities(&sc) {
            assert!(!o.title.contains("super-secret-reviewer") && !o.detail.contains("super-secret-reviewer"));
            assert!(!o.title.contains("confidential-deploy-flow") && !o.detail.contains("confidential-deploy-flow"));
        }
    }

    /// Plants a secret in every field `inventory.rs`'s own
    /// `never_leaks_secret_values_from_any_field` test uses (env, args,
    /// headers, and a URL's user/pass/path/query), takes a snapshot of the
    /// inventory that config produces, diffs it against an empty one, and
    /// checks neither the snapshot nor the diff carries any of them.
    #[test]
    fn never_leaks_secret_values_into_a_snapshot() {
        let doc = serde_json::json!({
            "mcpServers": {
                "with-env": {
                    "command": "/usr/local/bin/npx",
                    "args": ["-y", "@scope/server-thing@1.2.3", "--token", "ghp_SECRETARGTOKEN"],
                    "env": { "API_KEY": "sk-live-SECRETENVVALUE", "OTHER": "hunter2" }
                },
                "remote": {
                    "type": "http",
                    "url": "https://user:hunter2@mcp.example.com:8443/v1/SECRETPATHKEY/mcp?key=SECRETQUERYKEY",
                    "headers": { "Authorization": "Bearer SECRETHEADER" }
                }
            }
        });
        let servers = crate::inventory::mcp_from_claude_json(&doc);
        let inv = Inventory { mcp_servers: servers, ..Inventory::default() };
        let snap = snapshot_of(&inv, "2026-09-08");
        let found = diff(&empty_snapshot("2026-09-01"), &snap);
        let wire = format!("{}{}", serde_json::to_string(&snap).unwrap(), serde_json::to_string(&found).unwrap());
        for secret in ["sk-live-SECRETENVVALUE", "ghp_SECRETARGTOKEN", "SECRETPATHKEY", "SECRETQUERYKEY", "hunter2", "SECRETHEADER"] {
            assert!(!wire.contains(secret), "{secret} leaked into {wire}");
        }
    }

    /// This machine's real setup changes as JSON, against whatever the
    /// fictional demo home's own history file (planted by
    /// scripts/make-demo-fixture.py) says. `cargo test live_setup_changes --
    /// -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_setup_changes() {
        let inv = crate::inventory::scan();
        let today = crate::spend::today_naive_date().format("%Y-%m-%d").to_string();
        let sc = changes_at(&crate::providers::config_dir(), &inv, &today);
        println!("{}", serde_json::to_string(&sc).unwrap());
    }
}
