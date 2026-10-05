//! Per-MCP-server context cost: how many tool calls a server answered and
//! how many bytes of result came back, over the rolling 30 days
//! `spend::mcp_usage_30d` already tallies from the lines Claude Code writes
//! to its own session logs. Counts and sizes only -- this module never sees
//! a call's arguments or a result's content, only their lengths.
//!
//! Covered today: Claude Code only. `attach` is the one place a figure from
//! the logs is stamped onto a configured server, and it only ever looks at
//! servers this app already knows are loaded by Claude Code.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::i18n::Msg;
use crate::inventory::{McpServer, Opportunity};
use crate::spend::OTHER_MCP_SERVER;

/// Below this many calls in the 30-day window, a server's numbers are too
/// thin to mean anything -- a handful of one-off calls says nothing about
/// how the server is actually used.
pub const MIN_CALLS: u64 = 20;
/// 30 days of tool-result bytes at which a server's context cost is worth a
/// look.
pub const HEAVY_RESULT_BYTES: u64 = 2 * 1024 * 1024;

/// Days of Claude Code logs, and of unbroken configuration, a "no calls in 30
/// days" verdict needs. The strings say 30 days in words.
pub const UNUSED_NEEDS_DAYS: i64 = 30;

const MCP_LEARN: &str = "https://staas.fund/mcp/";

/// One server's 30-day figures. Never anything about what was asked or
/// returned -- only how many times and how much came back.
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpUsage {
    pub calls: u64,
    pub result_bytes: u64,
}

/// The test-side key registry (see inventory::FINDING_IDS): every finding id
/// this module can emit. Only i18n.rs's test module reads this, so it does
/// not exist in a release build at all.
#[cfg(test)]
pub(crate) const FINDING_IDS: &[&str] = &["mcp-context-heavy", "mcp-unused"];

/// Longest server name admitted out of a tool name -- a real configured
/// server name is short; anything past this in a log line is either
/// corrupt or hostile and is refused rather than displayed.
const MAX_SERVER_KEY: usize = 64;

/// The server named inside an MCP tool's own name: Claude Code builds a
/// tool's name as `"mcp__" + normalized(server) + "__" + tool`, and reads it
/// back by splitting on the FIRST `__` after the `mcp__` prefix -- server is
/// everything before it, tool is everything after (including any further
/// `__` runs the tool name itself carries). `"mcp__my_server__get__thing"`
/// is server `"my_server"`, tool `"get__thing"`; `"mcp__a__b__c"` is server
/// `"a"`. The one case this cannot get right: a configured server whose own
/// `normalized(name)` itself contains `__` reads identically to a shorter
/// server name followed by a tool part, and nothing in the tool name says
/// which one Claude Code meant -- `attach` below refuses a figure to any
/// such server rather than guess. `None` for anything not shaped like an
/// MCP tool call at all, for a name with no tool part, and for a server
/// part that is empty, longer than `MAX_SERVER_KEY`, or holds a character
/// `normalized` would never have produced -- a hostile or corrupted log
/// line must not mint a server name this app goes on to display or persist.
pub fn server_of(tool_name: &str) -> Option<String> {
    let rest = tool_name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    if server.is_empty() || tool.is_empty() || server.len() > MAX_SERVER_KEY {
        return None;
    }
    server
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        .then(|| server.to_string())
}

/// The form Claude Code gives a configured server name inside a tool name:
/// every character outside `[A-Za-z0-9_-]` becomes `_`. Lossy on purpose --
/// this is only ever used to match a configured name against a log's own
/// key, never to recover the original spelling from one. Claude Code's own
/// connectors are named `"claude.ai <name>"` (Google Drive, Gmail, and so
/// on) and get one further step: once a name that started with
/// `"claude.ai "` has gone through the same character replacement, every
/// run of consecutive `_` collapses to a single `_`, and any `_` that
/// collapsing leaves at either end is trimmed off.
pub fn normalized(config_name: &str) -> String {
    let replaced: String = config_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    if !config_name.starts_with("claude.ai ") {
        return replaced;
    }
    let mut collapsed = String::with_capacity(replaced.len());
    let mut prev_underscore = false;
    for c in replaced.chars() {
        if c == '_' {
            if prev_underscore {
                continue;
            }
            prev_underscore = true;
        } else {
            prev_underscore = false;
        }
        collapsed.push(c);
    }
    collapsed.trim_matches('_').to_string()
}

/// For each configured server, in order, the key its calls would sit under in
/// the logs, or `None` when the logs cannot hold a figure for it. This is the
/// one place those refusals are stated, for `attach` and `unused` alike.
/// Only Claude Code servers have figures. Two configured Claude Code servers
/// that normalize to the same string cannot be told apart in the logs, so
/// neither gets one: showing either one's number on both would be a guess
/// dressed up as a fact. Same for a server whose own normalized name contains
/// `__`: `server_of` splits a tool name at the FIRST `__` after the `mcp__`
/// prefix, so such a name reads identically to a shorter server name plus a
/// tool part, and nothing says which one a log line meant.
fn figure_keys(servers: &[McpServer]) -> Vec<Option<String>> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for s in servers.iter().filter(|s| s.client == "Claude Code") {
        *counts.entry(normalized(&s.name)).or_insert(0) += 1;
    }
    servers
        .iter()
        .map(|s| {
            if s.client != "Claude Code" {
                return None;
            }
            let key = normalized(&s.name);
            (!key.contains("__") && counts.get(&key).copied() == Some(1)).then_some(key)
        })
        .collect()
}

/// Stamps each Claude Code server with its 30-day figures, matched by
/// `normalized(name)` against the log's own keys (see `figure_keys` for the
/// servers that get none). A server the logs know about that nothing here
/// configures anymore is simply left alone: it is not this app's to report
/// on. Every other client is untouched -- this figure is measured for Claude
/// Code only.
pub fn attach(servers: &mut [McpServer], by_server: &HashMap<String, McpUsage>) {
    let keys = figure_keys(servers);
    for (s, key) in servers.iter_mut().zip(keys) {
        if let Some(u) = key.and_then(|k| by_server.get(&k)) {
            s.usage = Some(*u);
        }
    }
}

/// Names of the Claude Code servers that can be said to have had no calls in
/// the last 30 days, sorted. It says so only when it is known; any doubt
/// returns nothing:
/// - the logs reach back `UNUSED_NEEDS_DAYS` days (`days_covered`), or zero
///   calls may just mean the logs are young;
/// - the server was configured the whole window (`configured_throughout`,
///   from the setup history; `None` when no snapshot is old enough), or zero
///   calls also describes a server added yesterday;
/// - no call sits in the shared overflow bucket, or some server's calls are
///   not attributed to it;
/// - some configured Claude Code server with a figure was called, or zero may
///   only mean the logs are not being read;
/// - the server can have a figure at all (`figure_keys`).
pub fn unused(
    servers: &[McpServer],
    by_server: &HashMap<String, McpUsage>,
    days_covered: i64,
    configured_throughout: Option<&HashSet<String>>,
) -> Vec<String> {
    let Some(throughout) = configured_throughout else { return Vec::new() };
    if days_covered < UNUSED_NEEDS_DAYS || by_server.get(OTHER_MCP_SERVER).is_some_and(|u| u.calls > 0) {
        return Vec::new();
    }
    let keys = figure_keys(servers);
    let calls = |key: &Option<String>| key.as_ref().and_then(|k| by_server.get(k)).map_or(0, |u| u.calls);
    if !keys.iter().any(|k| calls(k) > 0) {
        return Vec::new();
    }
    let names: BTreeSet<String> = servers
        .iter()
        .zip(&keys)
        .filter(|(s, k)| k.is_some() && calls(k) == 0 && throughout.contains(&s.name))
        .map(|(s, _)| s.name.clone())
        .collect();
    names.into_iter().collect()
}

/// MB under 1 GB, then GB -- every heavy server is already past
/// `HEAVY_RESULT_BYTES` (2 MB), so this never has to spell out KB.
fn format_bytes(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{:.1} MB", mb)
    }
}

/// Same untranslated-data convention the other MCP findings' `names()`
/// helper uses (inventory.rs): server names are proper nouns, so this list
/// -- sizes baked in -- travels as one opaque var rather than being torn
/// apart and rebuilt per locale.
fn named_with_sizes(list: &[(&McpServer, McpUsage)]) -> String {
    list.iter()
        .map(|(s, u)| format!("{} ({})", s.name, format_bytes(u.result_bytes)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The servers heavy enough to be worth a look: at least `MIN_CALLS` calls
/// and at least `HEAVY_RESULT_BYTES` of results in the last 30 days,
/// heaviest first. A quiet setup earns nothing -- this is a "learn"
/// finding, not a gap, so it never counts against the audit score.
pub fn opportunities(servers: &[McpServer]) -> Vec<Opportunity> {
    let mut heavy: Vec<(&McpServer, McpUsage)> = servers
        .iter()
        .filter_map(|s| s.usage.map(|u| (s, u)))
        .filter(|(_, u)| u.calls >= MIN_CALLS && u.result_bytes >= HEAVY_RESULT_BYTES)
        .collect();
    if heavy.is_empty() {
        return Vec::new();
    }
    heavy.sort_by_key(|(_, u)| std::cmp::Reverse(u.result_bytes));
    let n = heavy.len() as i64;
    let names = named_with_sizes(&heavy);
    vec![Opportunity::from_msgs(
        "mcp-context-heavy",
        "learn",
        Msg::new("finding.mcp-context-heavy.title").count(n),
        Some(Msg::new("finding.mcp-context-heavy.detail").var("names", names).count(n)),
        Some(MCP_LEARN),
    )]
}

/// The finding for `unused`'s names: absent when there are none. The title is
/// a count and nothing else, because titles leave the machine in the seat
/// report and a server's name is the user's own; the names travel in the
/// detail as one opaque variable, as every other MCP finding's do.
pub fn unused_opportunities(names: &[String]) -> Vec<Opportunity> {
    if names.is_empty() {
        return Vec::new();
    }
    vec![Opportunity::from_msgs(
        "mcp-unused",
        "learn",
        Msg::new("finding.mcp-unused.title").count(names.len() as i64),
        Some(Msg::new("finding.mcp-unused.detail").var("names", names.join(", "))),
        Some(MCP_LEARN),
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str, client: &str) -> McpServer {
        McpServer {
            name: name.into(),
            client: client.into(),
            scope: "user".into(),
            project: None,
            transport: "stdio".into(),
            target: "npx".into(),
            package: None,
            env_count: 0,
            pin_to: None,
            source_file: None,
            usage: None,
        }
    }

    #[test]
    fn server_of_reads_a_well_formed_name() {
        assert_eq!(server_of("mcp__a_b__tool"), Some("a_b".to_string()));
    }

    #[test]
    fn server_of_refuses_a_name_with_no_tool_part() {
        assert_eq!(server_of("mcp__serveronly"), None);
        assert_eq!(server_of("mcp__"), None);
    }

    #[test]
    fn server_of_refuses_a_hostile_length() {
        let long_server = "a".repeat(65);
        assert_eq!(server_of(&format!("mcp__{long_server}__tool")), None);
        let ok_server = "a".repeat(64);
        assert_eq!(server_of(&format!("mcp__{ok_server}__tool")), Some(ok_server));
    }

    #[test]
    fn server_of_refuses_a_tool_that_is_not_mcp() {
        assert_eq!(server_of("Bash"), None);
        assert_eq!(server_of("str_replace_editor"), None);
        assert_eq!(server_of(""), None);
    }

    #[test]
    fn normalized_replaces_what_a_tool_name_cannot_hold() {
        assert_eq!(normalized("my server!!"), "my_server__");
        assert_eq!(normalized("acme-search_1"), "acme-search_1");
        assert_eq!(normalized("a/b\\c"), "a_b_c");
    }

    #[test]
    fn normalized_collapses_underscores_for_a_claude_ai_connector() {
        assert_eq!(normalized("claude.ai Google Drive"), "claude_ai_Google_Drive");
        assert_eq!(normalized("claude.ai  A  B"), "claude_ai_A_B");
        // The replacement pass turns the trailing "!" into an underscore right
        // at the end of the string; trim_matches('_') is what cuts it back off.
        assert_eq!(normalized("claude.ai Drive!"), "claude_ai_Drive");
    }

    #[test]
    fn server_of_splits_at_the_first_separator() {
        assert_eq!(server_of("mcp__my_server__get__thing"), Some("my_server".to_string()));
        assert_eq!(server_of("mcp__a__b__c"), Some("a".to_string()));
    }

    #[test]
    fn a_server_whose_name_holds_the_separator_gets_no_figure() {
        let mut servers = vec![server("my__server", "Claude Code")];
        let mut by_server = HashMap::new();
        by_server.insert("my__server".to_string(), McpUsage { calls: 40, result_bytes: 1000 });
        attach(&mut servers, &by_server);
        assert_eq!(
            servers[0].usage, None,
            "a normalized server name that itself holds \"__\" can't be told apart from a shorter server plus a tool part"
        );
    }

    #[test]
    fn attach_matches_only_claude_code_servers() {
        let mut servers = vec![server("acme", "Claude Code"), server("acme", "Claude Desktop")];
        let mut by_server = HashMap::new();
        by_server.insert("acme".to_string(), McpUsage { calls: 40, result_bytes: 1000 });
        attach(&mut servers, &by_server);
        assert_eq!(servers[0].usage, Some(McpUsage { calls: 40, result_bytes: 1000 }));
        assert_eq!(servers[1].usage, None, "not Claude Code -- gets nothing");
    }

    #[test]
    fn two_servers_with_one_normalized_name_get_no_figure() {
        let mut servers = vec![server("acme search", "Claude Code"), server("acme!search", "Claude Code")];
        let mut by_server = HashMap::new();
        by_server.insert("acme_search".to_string(), McpUsage { calls: 40, result_bytes: 1000 });
        attach(&mut servers, &by_server);
        assert!(servers.iter().all(|s| s.usage.is_none()), "an ambiguous match gets no figure on either side");
    }

    #[test]
    fn a_server_no_longer_configured_is_ignored() {
        let mut servers = vec![server("acme", "Claude Code")];
        let mut by_server = HashMap::new();
        by_server.insert("gone".to_string(), McpUsage { calls: 999, result_bytes: 999_999 });
        attach(&mut servers, &by_server);
        assert_eq!(servers[0].usage, None);
    }

    #[test]
    fn below_min_calls_no_finding() {
        let mut s = server("acme", "Claude Code");
        s.usage = Some(McpUsage { calls: MIN_CALLS - 1, result_bytes: HEAVY_RESULT_BYTES * 2 });
        assert!(opportunities(&[s]).is_empty());

        let mut light = server("acme", "Claude Code");
        light.usage = Some(McpUsage { calls: MIN_CALLS * 2, result_bytes: HEAVY_RESULT_BYTES - 1 });
        assert!(opportunities(&[light]).is_empty(), "enough calls but too little data is still not a finding");
    }

    #[test]
    fn the_thresholds_are_inclusive() {
        let mut at_both_thresholds = server("acme", "Claude Code");
        at_both_thresholds.usage = Some(McpUsage { calls: MIN_CALLS, result_bytes: HEAVY_RESULT_BYTES });
        let found = opportunities(&[at_both_thresholds]);
        assert_eq!(found.len(), 1, "exactly MIN_CALLS calls and exactly HEAVY_RESULT_BYTES bytes must already qualify");
        assert!(found[0].detail.contains("acme"), "{}", found[0].detail);

        let mut one_call_short = server("acme", "Claude Code");
        one_call_short.usage = Some(McpUsage { calls: MIN_CALLS - 1, result_bytes: HEAVY_RESULT_BYTES * 4 });
        assert!(opportunities(&[one_call_short]).is_empty(), "one call under MIN_CALLS, however heavy, must not qualify");

        let mut one_byte_short = server("acme", "Claude Code");
        one_byte_short.usage = Some(McpUsage { calls: MIN_CALLS * 4, result_bytes: HEAVY_RESULT_BYTES - 1 });
        assert!(opportunities(&[one_byte_short]).is_empty(), "one byte under HEAVY_RESULT_BYTES, however called, must not qualify");
    }

    #[test]
    fn a_heavy_server_is_named_with_its_size() {
        let mut s = server("acme-search", "Claude Code");
        s.usage = Some(McpUsage { calls: 40, result_bytes: 3 * 1024 * 1024 });
        let found = opportunities(&[s]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "mcp-context-heavy");
        assert_eq!(found[0].kind, "learn");
        assert!(found[0].detail.contains("acme-search"), "{}", found[0].detail);
        assert!(found[0].detail.contains("3.0 MB"), "{}", found[0].detail);
        assert!(!found[0].title.starts_with("finding."), "{}", found[0].title);
    }

    // ---- unused ----

    const DAYS: i64 = UNUSED_NEEDS_DAYS;

    fn called(n: u64) -> McpUsage {
        McpUsage { calls: n, result_bytes: 10 }
    }

    fn usage(pairs: &[(&str, u64)]) -> HashMap<String, McpUsage> {
        pairs.iter().map(|(k, n)| (k.to_string(), called(*n))).collect()
    }

    fn cc(names: &[&str]) -> Vec<McpServer> {
        names.iter().map(|n| server(n, "Claude Code")).collect()
    }

    fn throughout(names: &[&str]) -> HashSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn a_called_server_is_not_unused() {
        let servers = cc(&["busy", "idle"]);
        let all = throughout(&["busy", "idle"]);
        let got = unused(&servers, &usage(&[("busy", 3)]), DAYS, Some(&all));
        assert_eq!(got, vec!["idle".to_string()], "only the server with no calls is named");
    }

    #[test]
    fn a_server_without_a_figure_is_never_called_unused() {
        let all = throughout(&["busy", "acme search", "acme!search", "my__server"]);
        let servers = cc(&["busy", "acme search", "acme!search", "my__server"]);
        let got = unused(&servers, &usage(&[("busy", 3)]), DAYS, Some(&all));
        assert!(got.is_empty(), "two servers with one normalized name, and a name holding `__`, have no figure: {got:?}");
    }

    #[test]
    fn unused_needs_thirty_days_of_logs() {
        let servers = cc(&["busy", "idle"]);
        let all = throughout(&["busy", "idle"]);
        let by = usage(&[("busy", 3)]);
        assert!(unused(&servers, &by, DAYS - 1, Some(&all)).is_empty(), "29 days of logs is not enough");
        assert_eq!(unused(&servers, &by, DAYS, Some(&all)), vec!["idle".to_string()]);
        assert!(unused(&servers, &by, 0, Some(&all)).is_empty());
    }

    #[test]
    fn unused_needs_the_server_to_have_been_configured_throughout() {
        let servers = cc(&["busy", "idle"]);
        let by = usage(&[("busy", 3)]);
        // Added inside the window, or removed and re-added inside it: not in the set the history returns.
        assert!(unused(&servers, &by, DAYS, Some(&throughout(&["busy"]))).is_empty());
        assert!(unused(&servers, &by, DAYS, Some(&throughout(&[]))).is_empty());
        // No snapshot old enough: no verdict for anyone.
        assert!(unused(&servers, &by, DAYS, None).is_empty());
    }

    #[test]
    fn unused_needs_another_server_to_have_been_called() {
        let servers = cc(&["a", "b"]);
        let all = throughout(&["a", "b"]);
        assert!(unused(&servers, &HashMap::new(), DAYS, Some(&all)).is_empty(), "no call anywhere: the logs may not be read");
        // A call to a server nothing configures says nothing about the configured ones.
        assert!(unused(&servers, &usage(&[("gone", 9)]), DAYS, Some(&all)).is_empty());
    }

    #[test]
    fn calls_in_the_overflow_bucket_mean_no_verdict() {
        let servers = cc(&["busy", "idle"]);
        let all = throughout(&["busy", "idle"]);
        let by = usage(&[("busy", 3), (OTHER_MCP_SERVER, 1)]);
        assert!(unused(&servers, &by, DAYS, Some(&all)).is_empty(), "some server's calls are not attributed");
        let by = usage(&[("busy", 3), (OTHER_MCP_SERVER, 0)]);
        assert_eq!(unused(&servers, &by, DAYS, Some(&all)), vec!["idle".to_string()], "an empty bucket hides nothing");
    }

    #[test]
    fn a_server_of_another_client_is_never_judged() {
        let servers = vec![server("busy", "Claude Code"), server("idle", "Claude Desktop"), server("idle2", "Cursor")];
        let all = throughout(&["busy", "idle", "idle2"]);
        assert!(unused(&servers, &usage(&[("busy", 3)]), DAYS, Some(&all)).is_empty());
        // And a call figure for the name does not make another client's server count as the called one.
        let servers = vec![server("busy", "Claude Desktop"), server("idle", "Claude Code")];
        assert!(unused(&servers, &usage(&[("busy", 3)]), DAYS, Some(&all)).is_empty());
    }

    #[test]
    fn the_unused_finding_counts_what_it_names() {
        assert!(unused_opportunities(&[]).is_empty(), "absent when there is nothing to say");
        let names = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
        let found = unused_opportunities(&names);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].id.as_str(), found[0].kind.as_str()), ("mcp-unused", "learn"));
        assert_eq!(found[0].title, "3 MCP servers had no calls in the last 30 days");
        assert_eq!(found[0].title_msg.count, Some(3));
        assert!(found[0].detail.contains("alpha, beta, gamma"), "{}", found[0].detail);
        assert_eq!(unused_opportunities(&names[..1])[0].title, "1 MCP server had no calls in the last 30 days");
        assert_eq!(found[0].learn_url.as_deref(), Some(MCP_LEARN));
    }

    #[test]
    fn the_unused_title_carries_a_count_only() {
        use crate::inventory::Inventory;
        const MARKER: &str = "northwind-ledger-mcp";
        let found = unused_opportunities(&[MARKER.to_string()]);
        assert_eq!(found.len(), 1, "the finding has to exist, or this proves nothing");
        assert!(found[0].title_msg.vars.is_empty(), "a title carries a count only");
        assert!(!found[0].title.contains(MARKER), "{}", found[0].title);
        assert!(found[0].detail.contains(MARKER), "the name belongs in the detail");
        let inv = Inventory { opportunities: found, ..Inventory::default() };
        let report = crate::seat::build_with("seat-abcdefgh", "Dana's MacBook", 1, &inv, &[], &[]);
        assert_eq!(report.findings.len(), 1, "the finding does reach the report, as its title");
        let wire = serde_json::to_string(&report).unwrap();
        assert!(!wire.contains(MARKER) && !wire.contains("northwind"), "{wire}");
    }
}
