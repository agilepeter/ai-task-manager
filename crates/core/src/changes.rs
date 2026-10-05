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
use std::collections::HashSet;
use std::path::Path;

/// A snapshot older than this, measured from today, is too old to be what
/// "since" means, and is never the one today is compared with.
pub const KEEP_DAYS: i64 = 35;
/// How many snapshots the file holds: the last ones written, one a day, a
/// few more than `KEEP_DAYS` of them.
pub const KEEP_SNAPSHOTS: usize = 40;
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
    /// Whether this row is a guardrail weakening -- exactly the rows
    /// `opportunities()` counts under "guardrail-removed" (`kind == "removed"`
    /// and `what` one of "deny" | "shell" | "hook"), computed by the same
    /// `is_guardrail_loss` both places call so the finding's count and the
    /// UI's marked rows can never drift apart. Losing a version pin is
    /// deliberately never `true` here: that weakening is already scored by
    /// the audit's own `mcp-unpinned` finding, so counting it again here
    /// under a different name would double-count it.
    pub guardrail: bool,
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

/// The shape of the history file. A file that states a HIGHER number was
/// written by a newer copy of the app, in a shape this one does not know:
/// it is neither read (its fields may mean something else) nor written over
/// (the history in it belongs to the copy that can read it).
const STORE_VERSION: u32 = 1;

/// A file that states a version this one does not know. Anything stated
/// that is not a whole number up to `STORE_VERSION` counts: this version
/// only ever writes such a number, so a larger one, a fraction or a word is
/// somebody else's. A file that states nothing, or is not JSON at all, is
/// not a newer app's: it reads as no history and is written over.
fn from_a_newer_app(raw: &str) -> bool {
    let Ok(body) = serde_json::from_str::<serde_json::Value>(raw) else { return false };
    match body.get("version") {
        None | Some(serde_json::Value::Null) => false,
        Some(stated) => !stated.as_u64().is_some_and(|v| v <= u64::from(STORE_VERSION)),
    }
}

#[cfg(test)]
pub(crate) const ERROR_KEYS: &[&str] = &["error.setupHistory.newer"];

/// What to tell the user when the history on file is a newer version's.
pub fn newer_history_msg() -> crate::i18n::Msg {
    crate::i18n::Msg::new("error.setupHistory.newer")
}

/// Whether the history on file belongs to a newer version of the app, so
/// that "no history" can be told apart from "history this version leaves
/// alone" by whoever shows it.
pub fn history_is_from_a_newer_app(dir: &Path) -> bool {
    std::fs::read_to_string(store_path(dir)).is_ok_and(|raw| from_a_newer_app(&raw))
}

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

/// The servers of `client` that the newest snapshot at least `days` old
/// holds, and that every snapshot taken after it holds too: the ones that
/// were in the setup the whole time. A snapshot keeps a server's name and
/// client but not its scope, so this cannot say which scope it had. `None`
/// when no snapshot is that old, or when a snapshot's date cannot be read --
/// the history cannot vouch for anything then, and a caller treats that as
/// "not known". Every snapshot dated on or after the start counts, in any
/// order: a second one on the same day, or one dated in the future by a clock
/// that ran ahead, can only remove servers from the answer. Only tests call it
/// while `mcp_usage::unused_from_history`, the one rule that needs it, is not
/// wired.
pub fn configured_throughout(snapshots: &[Snapshot], today: &str, days: i64, client: &str) -> Option<HashSet<String>> {
    let mut dated: Vec<(chrono::NaiveDate, &Snapshot)> = Vec::new();
    for s in snapshots {
        dated.push((chrono::NaiveDate::parse_from_str(&s.taken, "%Y-%m-%d").ok()?, s));
    }
    let today_date = chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d").ok()?;
    let start = dated.iter().map(|(d, _)| *d).filter(|d| (today_date - *d).num_days() >= days).max()?;
    let mut kept: Option<HashSet<String>> = None;
    for (_, s) in dated.iter().filter(|(d, _)| *d >= start) {
        let names: HashSet<String> = s.servers.iter().filter(|v| v.client == client).map(|v| v.name.clone()).collect();
        kept = Some(match kept {
            None => names,
            Some(k) => k.intersection(&names).cloned().collect(),
        });
    }
    kept
}

/// A history of one snapshot a day for the `days + 1` days ending `today`
/// (oldest first), each holding the Claude Code servers `holds` names for that
/// many days ago. For tests of anything that reads the history.
#[cfg(test)]
pub(crate) fn daily_history<S: Into<String>>(today: &str, days: i64, holds: impl Fn(i64) -> Vec<S>) -> Vec<Snapshot> {
    let today = chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d").expect("a test date");
    (0..=days)
        .rev()
        .map(|ago| Snapshot {
            taken: (today - chrono::Duration::days(ago)).format("%Y-%m-%d").to_string(),
            servers: holds(ago)
                .into_iter()
                .map(|name| SnapServer {
                    name: name.into(),
                    client: "Claude Code".into(),
                    transport: "stdio".into(),
                    package: None,
                    pinned: None,
                })
                .collect(),
            agents: Vec::new(),
            skills: Vec::new(),
            hook_events: Vec::new(),
            deny: 0,
            allow: 0,
            deny_covers_shell: false,
        })
        .collect()
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

/// Whether a Change of this `kind`/`what` is a guardrail weakening. The one
/// place this is decided, so `change()` (which stamps every `Change.guardrail`)
/// and `opportunities()` (which counts them for the "guardrail-removed"
/// finding) can never disagree about which rows those are.
fn is_guardrail_loss(kind: &str, what: &str) -> bool {
    kind == "removed" && matches!(what, "deny" | "shell" | "hook")
}

/// The one place a `Change` is built: `text` is always this Msg's English
/// rendering, so it can never drift from what the popover shows in another
/// locale.
fn change(kind: &str, what: &str, name: &str, msg: Msg) -> Change {
    Change { kind: kind.into(), what: what.into(), name: name.into(), guardrail: is_guardrail_loss(kind, what), text: i18n::render("en", &msg), msg }
}

fn server_key(s: &SnapServer) -> (&str, &str) {
    (s.name.as_str(), s.client.as_str())
}

/// A server that still exists can differ in up to three independent ways,
/// each its own row so the Changes list (and the guardrail marker on it) can
/// say exactly what changed rather than a single opaque "{name} changed" --
/// in this fixed order: transport, package, then pin.
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
        if o.transport != s.transport {
            out.push(change("changed", "server", &s.name, Msg::new("changes.server.transport").var("name", &s.name)));
        }
        if o.package != s.package {
            out.push(change("changed", "server", &s.name, Msg::new("changes.server.package").var("name", &s.name)));
        }
        if o.pinned != s.pinned {
            if s.pinned == Some(true) {
                // Went to pinned from anything else (unpinned or no package to pin at all).
                out.push(change("changed", "server", &s.name, Msg::new("changes.server.pinned").var("name", &s.name)));
            } else if o.pinned == Some(true) {
                // Was pinned, now is not (unpinned, or the package disappeared).
                out.push(change("changed", "server", &s.name, Msg::new("changes.server.unpinned").var("name", &s.name)));
            }
            // Neither side is `Some(true)` (e.g. Some(false) <-> None): neither state is a
            // pin, so nothing a user would call a change happened. Nothing is emitted --
            // there is deliberately no "changes.server.pinUnknown" key.
        }
    }
}

/// Agents and skills are both a plain list of user-chosen names, so one walk
/// serves both -- only the `what` tag and which two message keys apply
/// differ between the two callers below.
fn diff_names(old: &[String], new: &[String], what: &'static str, added_key: &'static str, removed_key: &'static str, out: &mut Vec<Change>) {
    // Counted, not just looked up: the same name can be defined in two projects, and a
    // second definition appearing is a change even though the name was already known.
    let times = |list: &[String], name: &String| list.iter().filter(|n| *n == name).count();
    let mut seen: std::collections::HashSet<&String> = std::collections::HashSet::new();
    for name in new.iter().chain(old.iter()) {
        if !seen.insert(name) {
            continue;
        }
        let (before, after) = (times(old, name), times(new, name));
        for _ in before..after {
            out.push(change("added", what, name, Msg::new(added_key).var("name", name)));
        }
        for _ in after..before {
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
/// day -- so a refresh loop that records twice in one day never grows the
/// file -- and keeps the last `KEEP_SNAPSHOTS` written.
///
/// Nothing is ever dropped for its DATE. A date is only as good as the
/// clock that wrote it, and a clock can be wrong in both directions: one
/// that steps back would make newer snapshots look impossible, and one that
/// jumps ahead for a day would make every real snapshot look ancient. No
/// comparison of dates can tell a wrong clock from a machine that was left
/// off for a month. So the file is kept by the order things were written
/// in, which no clock can change, and a snapshot with a date that makes no
/// sense today simply goes unused by `changes_at` until it is pushed out.
/// Written with the same owner-only atomic swap every other local settings
/// file in this app uses, so a crash mid-write never leaves half a history.
pub fn record_at(dir: &Path, snap: Snapshot) -> std::io::Result<()> {
    let path = store_path(dir);
    let raw = std::fs::read_to_string(&path).ok();
    if raw.as_deref().is_some_and(from_a_newer_app) {
        return Err(std::io::Error::other("the setup history was written by a newer version of the app and is left as it is"));
    }
    let mut store: Store = raw.and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default();
    store.snapshots.retain(|s| s.taken != snap.taken);
    store.snapshots.push(snap);
    let extra = store.snapshots.len().saturating_sub(KEEP_SNAPSHOTS);
    store.snapshots.drain(..extra);
    store.version = STORE_VERSION;
    let body = serde_json::to_string_pretty(&store).map_err(std::io::Error::other)?;
    std::fs::create_dir_all(dir)?;
    crate::providers::onenewapi::store::atomic_write(&path, &body).map_err(std::io::Error::other)
}

/// Every snapshot on file, oldest first. A missing or unreadable file (a
/// fresh install, or one hand-edited into garbage), or one a newer version
/// of the app wrote, reads as no history at all, never as an error -- there is nothing here worth failing a scan
/// over.
pub fn load_from(dir: &Path) -> Vec<Snapshot> {
    std::fs::read_to_string(store_path(dir))
        .ok()
        .filter(|raw| !from_a_newer_app(raw))
        .and_then(|raw| serde_json::from_str::<Store>(&raw).ok())
        .map(|s| {
            let mut snapshots = s.snapshots;
            snapshots.sort_by(|a, b| a.taken.cmp(&b.taken));
            snapshots
        })
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
    // Only a snapshot from before today and inside the window can be what today is
    // compared with: one dated today is today, one dated later came from a wrong clock,
    // and one older than the window is too old to be what "since" means.
    let mut candidates: Vec<&Snapshot> = history
        .iter()
        .filter(|s| days_between(&s.taken, today).is_some_and(|d| (1..=KEEP_DAYS).contains(&d)))
        .collect();
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
    // Both findings count the rows of the Changes list, which is where their text sends
    // the reader: a number that is not the number of rows there reads as a row gone
    // missing.
    let mut out = Vec::new();
    let guardrail = c.changes.iter().filter(|ch| ch.guardrail).count();
    if guardrail > 0 {
        // "learn", not "tighten": this is the one finding built from history
        // rather than from the present state of the machine, and a diff can
        // only say that a rule disappeared, never why. Someone may have
        // loosened a rule on purpose (a false positive that kept blocking a
        // real command); that is a legitimate choice this app cannot second-
        // guess from a before/after comparison alone. So it stays its own
        // finding, shown first among the changes, but scored as "worth a
        // look" rather than as a failing.
        out.push(Opportunity::from_msgs(
            "guardrail-removed",
            "learn",
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

    fn server_transport(name: &str, transport: &str, package: Option<&str>, pinned: Option<bool>) -> SnapServer {
        SnapServer { name: name.into(), client: "Claude Code".into(), transport: transport.into(), package: package.map(str::to_string), pinned }
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
    fn a_transport_change_is_its_own_row() {
        let old = Snapshot { servers: vec![server_transport("docs", "stdio", Some("docs-mcp@1"), Some(true))], ..empty_snapshot("2026-09-01") };
        let new = Snapshot { servers: vec![server_transport("docs", "http", Some("docs-mcp@1"), Some(true))], ..empty_snapshot("2026-09-08") };
        let found = diff(&old, &new);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "changed");
        assert_eq!(found[0].what, "server");
        assert_eq!(found[0].name, "docs");
        assert_eq!(found[0].msg.key, "changes.server.transport");
        assert!(found[0].text.contains("docs"), "{}", found[0].text);
        assert!(!found[0].text.contains("stdio") && !found[0].text.contains("http"), "the transport value itself must never reach the message: {}", found[0].text);
        assert!(!found[0].guardrail, "a server change is never a guardrail loss");
    }

    #[test]
    fn a_package_change_is_its_own_row() {
        let old = Snapshot { servers: vec![server("docs", Some("docs-mcp@1"), Some(true))], ..empty_snapshot("2026-09-01") };
        let new = Snapshot { servers: vec![server("docs", Some("other-mcp@1"), Some(true))], ..empty_snapshot("2026-09-08") };
        let found = diff(&old, &new);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "changed");
        assert_eq!(found[0].what, "server");
        assert_eq!(found[0].name, "docs");
        assert_eq!(found[0].msg.key, "changes.server.package");
        assert!(!found[0].text.contains("docs-mcp") && !found[0].text.contains("other-mcp"), "the package name must never reach the message: {}", found[0].text);
    }

    #[test]
    fn a_pin_gained_is_its_own_row() {
        let old = Snapshot { servers: vec![server("docs", Some("docs-mcp"), Some(false))], ..empty_snapshot("2026-09-01") };
        let new = Snapshot { servers: vec![server("docs", Some("docs-mcp@1"), Some(true))], ..empty_snapshot("2026-09-08") };
        let found = diff(&old, &new);
        assert_eq!(found.len(), 2, "package and pin both differ, so both get their own row");
        assert!(found.iter().any(|c| c.msg.key == "changes.server.package"));
        let pin_row = found.iter().find(|c| c.msg.key == "changes.server.pinned").expect("a pin gained row");
        assert_eq!(pin_row.kind, "changed");
        assert_eq!(pin_row.what, "server");
        assert_eq!(pin_row.name, "docs");
        assert!(!pin_row.guardrail);
    }

    #[test]
    fn a_pin_lost_is_a_change() {
        let old = Snapshot { servers: vec![server("docs", Some("docs-mcp@1"), Some(true))], ..empty_snapshot("2026-09-01") };
        let new = Snapshot { servers: vec![server("docs", Some("docs-mcp@1"), Some(false))], ..empty_snapshot("2026-09-08") };
        let found = diff(&old, &new);
        assert_eq!(found.len(), 1, "only pinned differs here, so only one row");
        assert_eq!(found[0].kind, "changed");
        assert_eq!(found[0].what, "server");
        assert_eq!(found[0].name, "docs");
        assert_eq!(found[0].msg.key, "changes.server.unpinned");
        assert!(
            !found[0].guardrail,
            "losing a pin is already scored by the audit's own mcp-unpinned finding; counting it again here under a different name would double-count it"
        );
    }

    #[test]
    fn a_pin_state_that_was_never_really_pinned_either_way_is_not_a_change() {
        // Some(false) <-> None: neither side is a pin (an unpinned package vs. no package to
        // have a version at all), so nothing a user would call a change happened -- and with
        // the package itself unchanged, this diff has nothing else to report either.
        let old = Snapshot { servers: vec![server("notes", Some("notes-mcp"), Some(false))], ..empty_snapshot("2026-09-01") };
        let new = Snapshot { servers: vec![server("notes", Some("notes-mcp"), None)], ..empty_snapshot("2026-09-08") };
        assert!(diff(&old, &new).is_empty(), "{:?}", diff(&old, &new));
    }

    #[test]
    fn a_deny_rule_removed_is_a_guardrail() {
        let old = Snapshot { deny: 5, ..empty_snapshot("2026-09-01") };
        let new = Snapshot { deny: 3, ..empty_snapshot("2026-09-08") };
        let found = diff(&old, &new);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "removed");
        assert_eq!(found[0].what, "deny");
        assert!(found[0].guardrail, "the UI's amber marker and the finding's count read this same field");
        let sc = SetupChanges { since: Some(old.taken.clone()), changes: found, days_of_history: 7 };
        let opps = opportunities(&sc);
        assert_eq!(opps.len(), 1);
        assert_eq!(opps[0].id, "guardrail-removed");
        assert_eq!(opps[0].kind, "learn", "whether a rule loss was on purpose cannot be judged from a diff");
    }

    #[test]
    fn a_hook_event_gone_is_a_guardrail() {
        let old = Snapshot { hook_events: vec![("PreToolUse".into(), 1)], ..empty_snapshot("2026-09-01") };
        let new = empty_snapshot("2026-09-08");
        let found = diff(&old, &new);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "removed");
        assert_eq!(found[0].what, "hook");
        assert!(found[0].guardrail);
        let sc = SetupChanges { since: Some(old.taken.clone()), changes: found, days_of_history: 7 };
        let opps = opportunities(&sc);
        assert_eq!(opps.len(), 1);
        assert_eq!(opps[0].id, "guardrail-removed");
        assert_eq!(opps[0].kind, "learn", "whether a rule loss was on purpose cannot be judged from a diff");
    }

    /// A guardrail loss is real and worth surfacing, but it is the one
    /// finding this module builds from history rather than from the present
    /// state of the machine -- a diff can say a rule disappeared, never why
    /// it did, so it must not count against the audit score the way a
    /// "tighten" finding does. It still comes first among the two possible
    /// findings this module emits, ahead of the generic "setup-changed" one,
    /// when both apply at once.
    #[test]
    fn a_guardrail_gone_is_worth_a_look_not_a_failing() {
        let old = Snapshot {
            agents: vec!["reviewer".into()],
            deny: 5,
            ..empty_snapshot("2026-09-01")
        };
        let new = Snapshot { agents: vec!["reviewer".into(), "auditor".into()], deny: 3, ..empty_snapshot("2026-09-08") };
        let sc = SetupChanges { since: Some(old.taken.clone()), changes: diff(&old, &new), days_of_history: 7 };
        let opps = opportunities(&sc);
        assert_eq!(opps.len(), 2, "one guardrail loss plus one ordinary change: two findings, not merged");
        assert_eq!(opps[0].id, "guardrail-removed", "the guardrail finding leads");
        assert_eq!(opps[0].kind, "learn");
        assert_eq!(opps[1].id, "setup-changed");
    }

    #[test]
    fn each_finding_counts_the_rows_the_list_shows() {
        let old = Snapshot { servers: vec![server_transport("docs", "stdio", Some("docs-mcp@1"), Some(true))], ..empty_snapshot("2026-09-01") };
        let new = Snapshot { servers: vec![server_transport("docs", "http", Some("other-mcp@2"), Some(true))], ..empty_snapshot("2026-09-08") };
        let found = diff(&old, &new);
        assert_eq!(found.len(), 2, "transport and package each get their own row");
        assert!(!found.iter().any(|c| c.guardrail));
        let sc = SetupChanges { since: Some(old.taken.clone()), changes: found, days_of_history: 7 };
        let opps = opportunities(&sc);
        assert_eq!(opps.len(), 1);
        assert_eq!(opps[0].id, "setup-changed");
        assert_eq!(opps[0].title, "2 other things changed in your setup", "two rows in the list, so two in the finding");

        // The same name defined a second time is a second row, and counts as one.
        let old = Snapshot { agents: vec!["reviewer".into()], hook_events: vec![("SessionEnd".into(), 2)], deny: 4, ..empty_snapshot("2026-09-01") };
        let new = Snapshot { agents: vec!["reviewer".into(); 3], hook_events: vec![("SessionEnd".into(), 1)], deny: 2, ..empty_snapshot("2026-09-08") };
        let found = diff(&old, &new);
        let (marked, plain) = (found.iter().filter(|c| c.guardrail).count(), found.iter().filter(|c| !c.guardrail).count());
        assert_eq!((marked, plain), (2, 2), "{found:?}");
        let opps = opportunities(&SetupChanges { since: Some(old.taken.clone()), changes: found, days_of_history: 7 });
        assert_eq!(opps[0].title, "2 guardrails were removed from your setup");
        assert_eq!(opps[1].title, "2 other things changed in your setup");
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
                switched_off_in_a_project: false,
                usage: None,
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
    fn a_clock_that_steps_back_loses_no_history() {
        let dir = tmp_dir("clock-back");
        record_at(&dir, empty_snapshot("2026-09-25")).unwrap();
        record_at(&dir, empty_snapshot("2026-09-26")).unwrap();
        // The clock on this machine jumps backward and a snapshot for an
        // already-passed day is recorded after two newer ones exist. None of
        // the three should be judged "too new to be real" and dropped: the
        // newest one on file after this write is still 2026-09-26, and every
        // one of the three is well within KEEP_DAYS of that.
        record_at(&dir, empty_snapshot("2026-09-20")).unwrap();
        let stored = load_from(&dir);
        let dates: Vec<&str> = stored.iter().map(|s| s.taken.as_str()).collect();
        assert_eq!(dates, vec!["2026-09-20", "2026-09-25", "2026-09-26"], "a clock stepping back must not lose history: {dates:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A history file as this version writes it, one snapshot in it, stating
    /// `version` (or none).
    fn history_stating(version: Option<u32>) -> String {
        let store = Store { version: STORE_VERSION, snapshots: vec![empty_snapshot("2026-09-20")] };
        let mut body = serde_json::to_value(&store).unwrap();
        match version {
            Some(v) => body["version"] = serde_json::json!(v),
            None => drop(body.as_object_mut().unwrap().remove("version")),
        }
        body.to_string()
    }

    #[test]
    fn a_history_from_a_newer_app_is_not_read_and_not_written_over() {
        let dir = tmp_dir("newer");
        // Every field this version knows, so it would parse: only the number
        // says the shape is one this version does not know.
        let theirs = history_stating(Some(STORE_VERSION + 1));
        std::fs::write(store_path(&dir), &theirs).unwrap();
        assert!(load_from(&dir).is_empty(), "a newer shape is not guessed at");
        assert!(record_at(&dir, empty_snapshot("2026-09-27")).is_err(), "and its history is not replaced");
        assert_eq!(std::fs::read_to_string(store_path(&dir)).unwrap(), theirs, "the file is left byte for byte");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn any_version_this_one_never_writes_is_somebody_elses() {
        for stated in ["2", "99999999999999999999999", "1.5", "-1", "\"2\"", "[1]"] {
            let dir = tmp_dir("odd");
            let theirs = history_stating(Some(STORE_VERSION)).replacen(&format!("\"version\":{STORE_VERSION}"), &format!("\"version\":{stated}"), 1);
            assert!(theirs.contains(stated), "the fixture has to state it: {theirs}");
            std::fs::write(store_path(&dir), &theirs).unwrap();
            assert!(history_is_from_a_newer_app(&dir), "{stated}");
            assert!(load_from(&dir).is_empty(), "{stated}");
            assert!(record_at(&dir, empty_snapshot("2026-09-27")).is_err(), "{stated}");
            assert_eq!(std::fs::read_to_string(store_path(&dir)).unwrap(), theirs, "{stated}");
            let _ = std::fs::remove_dir_all(&dir);
        }
        let dir = tmp_dir("none");
        assert!(!history_is_from_a_newer_app(&dir), "no file is nobody's");
        std::fs::write(store_path(&dir), "not json").unwrap();
        assert!(!history_is_from_a_newer_app(&dir), "garbage is nobody's");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_history_with_no_version_or_this_one_is_read() {
        for version in [None, Some(STORE_VERSION)] {
            let dir = tmp_dir("same");
            std::fs::write(store_path(&dir), history_stating(version)).unwrap();
            assert_eq!(load_from(&dir).len(), 1, "{version:?}");
            record_at(&dir, empty_snapshot("2026-09-27")).unwrap();
            assert_eq!(load_from(&dir).len(), 2, "{version:?}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn old_snapshots_are_pruned() {
        let dir = tmp_dir("prune");
        for day in 1..=(KEEP_SNAPSHOTS + 5) {
            let date = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap() + chrono::Duration::days(day as i64);
            record_at(&dir, empty_snapshot(&date.format("%Y-%m-%d").to_string())).unwrap();
        }
        let stored = load_from(&dir);
        assert_eq!(stored.len(), KEEP_SNAPSHOTS, "the file holds the last ones written and no more");
        assert_eq!(stored[0].taken, "2026-01-07", "the five written first are the ones let go");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clock_that_jumped_ahead_once_loses_no_history() {
        let dir = tmp_dir("clock-ahead");
        record_at(&dir, Snapshot { deny: 9, ..empty_snapshot("2026-09-20") }).unwrap();
        record_at(&dir, empty_snapshot("2026-09-27")).unwrap();
        record_at(&dir, empty_snapshot("2030-01-01")).unwrap(); // the clock was wrong for a day
        record_at(&dir, empty_snapshot("2026-09-28")).unwrap();
        let stored = load_from(&dir);
        let dates: Vec<&str> = stored.iter().map(|s| s.taken.as_str()).collect();
        assert_eq!(dates, vec!["2026-09-20", "2026-09-27", "2026-09-28", "2030-01-01"]);
        let inv = Inventory { permissions: Permissions { deny: 3, ..Permissions::default() }, ..Inventory::default() };
        let sc = changes_at(&dir, &inv, "2026-09-28");
        assert_eq!(sc.since.as_deref(), Some("2026-09-20"), "the day from the future is never the one compared with");
        assert_eq!(sc.days_of_history, 8);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_snapshot_older_than_the_window_is_not_compared_with() {
        let dir = tmp_dir("stale");
        record_at(&dir, Snapshot { deny: 9, ..empty_snapshot("2026-01-01") }).unwrap();
        let sc = changes_at(&dir, &Inventory::default(), "2026-09-20");
        assert_eq!(sc.since, None, "eight months ago is not what 'since' should mean");
        assert!(sc.changes.is_empty());
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
    fn a_snapshot_from_yesterday_does_not_stand_in_for_the_one_a_week_old() {
        let dir = tmp_dir("recent-and-old");
        record_at(&dir, Snapshot { deny: 9, ..empty_snapshot("2026-09-10") }).unwrap(); // 15 days before
        record_at(&dir, Snapshot { deny: 3, ..empty_snapshot("2026-09-24") }).unwrap(); // yesterday
        let inv = Inventory { permissions: Permissions { deny: 3, ..Permissions::default() }, ..Inventory::default() };
        let sc = changes_at(&dir, &inv, "2026-09-25");
        assert_eq!(sc.since.as_deref(), Some("2026-09-10"));
        assert_eq!(sc.changes.len(), 1, "against yesterday nothing changed; against the week it did");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_snapshot_dated_after_today_is_no_history() {
        let dir = tmp_dir("future");
        record_at(&dir, Snapshot { deny: 5, ..empty_snapshot("2027-01-01") }).unwrap();
        let sc = changes_at(&dir, &Inventory::default(), "2026-09-27");
        assert_eq!(sc.since, None);
        assert_eq!(sc.days_of_history, 0);
        assert!(sc.changes.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_definition_of_a_known_name_is_a_change() {
        let old = Snapshot { agents: vec!["reviewer".into()], ..empty_snapshot("2026-09-01") };
        let new = Snapshot { agents: vec!["reviewer".into(), "reviewer".into()], ..empty_snapshot("2026-09-08") };
        let found = diff(&old, &new);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].kind.as_str(), found[0].what.as_str()), ("added", "agent"));
        assert_eq!(diff(&new, &old)[0].kind, "removed");
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

        // The "added" path above (diffed against an empty snapshot) never exercises
        // diff_servers' "changed" branch at all. A real reconfiguration -- a bumped
        // package version, a rotated token, a different transport -- is exactly the
        // shape the transport/package/pin split was built for, so it needs its own
        // planted "old" snapshot to diff against, with its OWN, different secrets.
        let old_doc = serde_json::json!({
            "mcpServers": {
                "with-env": {
                    "command": "/usr/local/bin/npx",
                    "args": ["-y", "@scope/server-thing@1.0.0", "--token", "ghp_OLDARGTOKEN"],
                    "env": { "API_KEY": "sk-live-OLDENVVALUE", "OTHER": "hunter1" }
                },
                "remote": {
                    "type": "http",
                    "url": "https://user:hunter1@mcp.example.com:8443/v1/OLDPATHKEY/mcp?key=OLDQUERYKEY",
                    "headers": { "Authorization": "Bearer OLDHEADER" }
                }
            }
        });
        let old_servers = crate::inventory::mcp_from_claude_json(&old_doc);
        let old_inv = Inventory { mcp_servers: old_servers, ..Inventory::default() };
        let old_snap = snapshot_of(&old_inv, "2026-09-01");
        let changed = diff(&old_snap, &snap);
        // with-env's package went from @scope/server-thing@1.0.0 to @1.2.3: a real,
        // reportable change (still pinned both times, so no pin row).
        assert!(changed.iter().any(|c| c.msg.key == "changes.server.package" && c.name == "with-env"), "{changed:?}");
        let changed_wire = format!("{}{}", serde_json::to_string(&old_snap).unwrap(), serde_json::to_string(&changed).unwrap());
        for secret in [
            "sk-live-SECRETENVVALUE", "ghp_SECRETARGTOKEN", "SECRETPATHKEY", "SECRETQUERYKEY", "hunter2", "SECRETHEADER",
            "sk-live-OLDENVVALUE", "ghp_OLDARGTOKEN", "OLDPATHKEY", "OLDQUERYKEY", "hunter1", "OLDHEADER",
        ] {
            assert!(!changed_wire.contains(secret), "{secret} leaked into {changed_wire}");
        }
        // The package's own name and version are legitimate SnapServer fields (kept in the
        // snapshot on disk, same as `package` already is elsewhere) -- what must never
        // happen is either one reaching a Change's own message. Checked against the
        // Change list alone, not the whole wire above, which legitimately carries them.
        let changed_only = serde_json::to_string(&changed).unwrap();
        for value in ["@scope/server-thing", "1.0.0", "1.2.3"] {
            assert!(!changed_only.contains(value), "{value} leaked into a Change: {changed_only}");
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

    #[test]
    fn configured_throughout_needs_every_snapshot_since() {
        // The window the unused-server rule asks about, so the dates below follow it.
        let window = crate::mcp_usage::UNUSED_WINDOW_DAYS;
        let today = "2026-10-05";
        let ago = |days: i64| {
            let today = chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d").unwrap();
            (today - chrono::Duration::days(days)).format("%Y-%m-%d").to_string()
        };
        let with = |taken: &str, names: &[&str]| Snapshot {
            servers: names.iter().map(|n| server(n, None, None)).collect(),
            ..empty_snapshot(taken)
        };
        let set = |names: &[&str]| -> HashSet<String> { names.iter().map(|n| n.to_string()).collect() };

        // The start is the NEWEST snapshot at least `window` days old: one exactly that old, older ones before
        // it, and one a day too young.
        //   late   was added after the oldest snapshot but is in the start and every later one: counts.
        //   young  first appears one day inside the window: does not.
        //   gone   is in the oldest snapshot only: does not.
        //   b      is in the first and the last snapshot but missing from one between: does not.
        //   new    is in today's snapshot only: does not.
        let history = [
            with(&ago(window + 15), &["gone", "a", "b", "c"]),
            with(&ago(window), &["a", "b", "c", "late"]),
            with(&ago(window - 1), &["a", "b", "c", "late", "young"]),
            with(&ago(window - 11), &["a", "c", "late", "young"]),
            with(today, &["a", "b", "c", "late", "young", "new"]),
        ];
        assert_eq!(configured_throughout(&history, today, window, "Claude Code"), Some(set(&["a", "c", "late"])));

        // The order the snapshots come in does not matter.
        let reversed: Vec<Snapshot> = history.iter().rev().cloned().collect();
        assert_eq!(configured_throughout(&reversed, today, window, "Claude Code"), Some(set(&["a", "c", "late"])));

        // Two snapshots on one day are both applied, so a server only one of them holds is out.
        let twice = [with(&ago(window), &["a", "b"]), with(&ago(window), &["a"]), with(today, &["a", "b"])];
        assert_eq!(configured_throughout(&twice, today, window, "Claude Code"), Some(set(&["a"])));

        // A snapshot dated in the future (a clock that ran ahead) holding no servers leaves none: it can only remove.
        let ahead = [with(&ago(window), &["a", "b"]), with("2026-12-01", &[])];
        assert_eq!(configured_throughout(&ahead, today, window, "Claude Code"), Some(set(&[])));

        // A server of another client is not counted for this client.
        let mut other = with(&ago(window + 4), &["a"]);
        other.servers.push(SnapServer { client: "Cursor".into(), ..server("x", None, None) });
        assert_eq!(configured_throughout(&[other], today, window, "Claude Code"), Some(set(&["a"])));

        // No snapshot that old: no answer at all, not an empty set.
        let young = [with(&ago(window - 1), &["a"]), with(today, &["a"])];
        assert_eq!(configured_throughout(&young, today, window, "Claude Code"), None);
        assert_eq!(configured_throughout(&[], today, window, "Claude Code"), None);

        // A date that cannot be read means the history cannot vouch for anything.
        let broken = [with(&ago(window + 4), &["a"]), with("not a date", &["a"])];
        assert_eq!(configured_throughout(&broken, today, window, "Claude Code"), None);
        assert_eq!(configured_throughout(&history, "garbage", window, "Claude Code"), None);
    }
}
