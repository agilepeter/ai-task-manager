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

use crate::changes::{self, Snapshot};
use crate::i18n::Msg;
use crate::inventory::{McpServer, Opportunity};

/// Below this many calls in the 30-day window, a server's numbers are too
/// thin to mean anything -- a handful of one-off calls says nothing about
/// how the server is actually used.
pub const MIN_CALLS: u64 = 20;
/// 30 days of tool-result bytes at which a server's context cost is worth a
/// look.
pub const HEAVY_RESULT_BYTES: u64 = 2 * 1024 * 1024;

/// The window a "no tool calls" verdict is about: the span the call counts
/// cover (`spend::TREND_DAYS`, which `mcp_usage_for_window` sums), the span a
/// server must have been configured for, and the number the finding's strings
/// say in words. The assertion below keeps the first from drifting away from
/// this one; `the_unused_strings_say_the_window_the_rule_uses` keeps the
/// strings from drifting away from it.
pub const UNUSED_NEEDS_DAYS: i64 = 30;
const _: () = assert!(UNUSED_NEEDS_DAYS == crate::spend::TREND_DAYS as i64);

/// How far back the logs must reach before zero calls means anything. Not the
/// whole window: Claude Code deletes logs older than its `cleanupPeriodDays`
/// (30 by default) and this app scans only recently changed files, so a log
/// line from exactly 30 days ago is there only if something happened to be
/// written on that very day, and the finding would come and go with the day
/// of the week. Three weeks shows that old logs are really being kept; the
/// retention setting says whether they are kept for the whole window.
pub const UNUSED_NEEDS_LOG_DAYS: i64 = 21;

/// The client whose servers have call counts at all. The one place its name is
/// written for this module's logic.
const CLAUDE_CODE: &str = "Claude Code";

/// Where calls from a file that names too many distinct servers are folded
/// (see `spend::mcp_key_for`): one shared bucket instead of one key per
/// server. A configured server's key can never be this, since a logged key
/// holds only `[A-Za-z0-9_-]`. The value is stored in users' scan caches, so
/// it must not change.
pub const OTHER_MCP_SERVER: &str = "(other)";

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
/// `normalized(name)` itself contains `__`, or ends in `_`, reads identically
/// to a shorter server name followed by a tool part, and nothing in the tool
/// name says which one Claude Code meant -- `figure_keys` below refuses a
/// figure to any such server rather than guess. `None` for anything not
/// shaped like an MCP tool call at all, for a name with no tool part, and for
/// a server part that is empty, longer than `MAX_SERVER_KEY`, or holds a
/// character `normalized` would never have produced -- a hostile or
/// corrupted log line must not mint a server name this app goes on to
/// display or persist.
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

/// `normalized` with each character outside the Basic Multilingual Plane
/// (an emoji, say) replaced twice, once per UTF-16 unit it takes, the way a
/// JavaScript regex without the `u` flag walks a string. `normalized` replaces
/// such a character once. Which of the two Claude Code does is not known here,
/// so a name where they differ (`my`, one such character, `server` is
/// `my_server` or `my__server`) is read both ways, and both are kept.
fn normalized_per_utf16_unit(config_name: &str) -> String {
    let by_unit: String = config_name.chars().flat_map(|c| std::iter::repeat_n(c, c.len_utf16())).collect();
    normalized(&by_unit)
}

/// A tool name's tool part for the probe below: any non-empty one.
const PROBE_TOOL: &str = "t";

/// Every key the calls of a server called `config_name` could be logged
/// under: for each way Claude Code might spell the name, what `server_of`
/// reads back from the tool name it would build (`mcp__<spelling>__<tool>`),
/// `None` when `server_of` rejects it and the call is never counted at all. A
/// spelling that ends in `_`, holds `__`, is empty or is too long reads back
/// as something other than itself, or as nothing.
fn logged_keys(config_name: &str) -> BTreeSet<Option<String>> {
    [normalized(config_name), normalized_per_utf16_unit(config_name)]
        .iter()
        .map(|spelling| server_of(&format!("mcp__{spelling}__{PROBE_TOOL}")))
        .collect()
}

/// For each configured server, in order, the key its calls sit under in the
/// logs, or `None` when that key is not known to be its alone. This is the one
/// place those refusals are stated, for `attach` and `unused` alike.
///
/// A key is known only when the calls are logged under exactly the name the
/// config gives it: every spelling Claude Code might use (`logged_keys`) reads
/// back as `normalized(name)`, none is rejected, and no other configured
/// Claude Code server's calls could be logged under it too. So `My Server!`
/// (its calls land under `My_Server`, not `My_Server_`), `a__b` (under `a`), a
/// name that is empty or longer than `MAX_SERVER_KEY`, and a name whose
/// spelling depends on how a character outside the Basic Multilingual Plane
/// is replaced all have no figure: reading the config key would show no calls
/// for a server that was called, or credit it with another server's calls.
/// `foo` and `foo!` have none either, since `foo!`'s calls are logged under
/// `foo`. A number there would be a guess dressed up as a fact. Only Claude
/// Code servers have figures.
fn figure_keys(servers: &[McpServer]) -> Vec<Option<String>> {
    let candidates: Vec<Option<BTreeSet<Option<String>>>> =
        servers.iter().map(|s| (s.client == CLAUDE_CODE).then(|| logged_keys(&s.name))).collect();
    let mut claims: HashMap<&str, usize> = HashMap::new();
    for key in candidates.iter().flatten().flatten().flatten() {
        *claims.entry(key.as_str()).or_insert(0) += 1;
    }
    servers
        .iter()
        .zip(&candidates)
        .map(|(s, logged)| {
            let logged = logged.as_ref()?;
            let own = normalized(&s.name);
            let read_back_as_itself = logged.len() == 1 && logged.contains(&Some(own.clone()));
            (read_back_as_itself && claims.get(own.as_str()) == Some(&1)).then_some(own)
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

/// Names of the Claude Code servers that can be said to have had no tool
/// calls in the last `UNUSED_NEEDS_DAYS`, sorted. It says so only when it is
/// known; any doubt returns nothing:
/// - the logs reach back `UNUSED_NEEDS_LOG_DAYS` (`days_covered`), which shows
///   that old logs are really being kept;
/// - Claude Code's own retention keeps logs for the whole window
///   (`retention_covers_window`), or the oldest calls may have been deleted;
/// - the server was configured the whole window (`configured_throughout`,
///   from the setup history; `None` when no snapshot is old enough), or zero
///   calls also describes a server added yesterday;
/// - no call sits in the shared overflow bucket, or some server's calls are
///   not attributed to it;
/// - some configured Claude Code server with a figure was called, or zero may
///   only mean the logs are not being read;
/// - the server can have a figure at all (`figure_keys`).
pub(crate) fn unused(
    servers: &[McpServer],
    by_server: &HashMap<String, McpUsage>,
    days_covered: i64,
    retention_covers_window: bool,
    configured_throughout: Option<&HashSet<String>>,
) -> Vec<String> {
    let Some(throughout) = configured_throughout else { return Vec::new() };
    if days_covered < UNUSED_NEEDS_LOG_DAYS
        || !retention_covers_window
        || by_server.get(OTHER_MCP_SERVER).is_some_and(|u| u.calls > 0)
    {
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

/// `unused` from the setup's own history: the servers `history` (the dated
/// snapshots of the setup's shape, as `changes::load_from` reads them) holds
/// for the whole `UNUSED_NEEDS_DAYS` ending `today` (`YYYY-MM-DD`, local).
/// The one function that states the whole rule, so the caller has nothing to
/// get wrong: it only supplies what was measured.
pub fn unused_from_history(
    servers: &[McpServer],
    by_server: &HashMap<String, McpUsage>,
    days_covered: i64,
    retention_covers_window: bool,
    history: &[Snapshot],
    today: &str,
) -> Vec<String> {
    let throughout = changes::configured_throughout(history, today, UNUSED_NEEDS_DAYS, CLAUDE_CODE);
    unused(servers, by_server, days_covered, retention_covers_window, throughout.as_ref())
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

    /// The tool name Claude Code builds for a server's tool, under each spelling
    /// of the server's name it might use.
    fn tool_names(config_name: &str) -> Vec<String> {
        let mut spellings = vec![normalized(config_name), normalized_per_utf16_unit(config_name)];
        spellings.dedup();
        spellings.iter().map(|spelling| format!("mcp__{spelling}__search")).collect()
    }

    /// Every string of up to `max_len` of the given symbols, the empty one included.
    fn all_names(symbols: &[&str], max_len: usize) -> Vec<String> {
        let mut all = vec![String::new()];
        let mut level = all.clone();
        for _ in 0..max_len {
            level = level.iter().flat_map(|n| symbols.iter().map(move |sym| format!("{n}{sym}"))).collect();
            all.extend(level.iter().cloned());
        }
        all
    }

    #[test]
    fn a_called_server_is_not_unused() {
        let servers = cc(&["busy", "idle"]);
        let all = throughout(&["busy", "idle"]);
        let got = unused(&servers, &usage(&[("busy", 3)]), DAYS, true, Some(&all));
        assert_eq!(got, vec!["idle".to_string()], "only the server with no calls is named");
    }

    #[test]
    fn a_server_without_a_figure_is_never_called_unused() {
        let all = throughout(&["busy", "acme search", "acme!search", "my__server"]);
        let servers = cc(&["busy", "acme search", "acme!search", "my__server"]);
        let got = unused(&servers, &usage(&[("busy", 3)]), DAYS, true, Some(&all));
        assert!(got.is_empty(), "two servers with one normalized name, and a name holding `__`, have no figure: {got:?}");
    }

    #[test]
    fn a_figure_key_is_always_where_the_calls_land() {
        let long = "x".repeat(70);
        // (config name, the key its calls are logged under, when that is its own).
        let table: [(&str, Option<&str>); 12] = [
            ("busy", Some("busy")),
            ("My Server!", None),         // normalizes to `My_Server_`; the calls land under `My_Server`
            ("x.", None),                 // `x_`: the calls land under `x`
            ("a__b", None),               // the calls land under `a`
            ("_x", Some("_x")),           // a leading underscore reads back whole
            ("@scope/pkg", Some("_scope_pkg")),
            ("a.b", Some("a_b")),
            ("claude.ai Google Drive", Some("claude_ai_Google_Drive")),
            (long.as_str(), None),        // longer than a key can be: the calls are never counted
            ("my\u{1F600}server", None),   // `my_server` or `my__server`, depending on the replacement
            ("\u{1F600}rest", None),       // `_rest`, or `__rest`, which `server_of` rejects: its calls may never be counted
            ("", None),                   // nothing to read back
        ];
        for (name, expected) in table {
            assert_eq!(figure_keys(&cc(&[name]))[0].as_deref(), expected, "{name:?}");
        }
        // Two servers whose calls land under one key have no figure each; one of them is not even the key's own.
        assert_eq!(figure_keys(&cc(&["foo", "foo!"])), [None, None]);
        assert_eq!(figure_keys(&cc(&["foo", "bar"])), [Some("foo".to_string()), Some("bar".to_string())]);
        // Another client's server of the same name does not make a Claude Code one ambiguous.
        assert_eq!(figure_keys(&[server("foo", "Claude Code"), server("foo!", "Cursor")])[0].as_deref(), Some("foo"));

        // Whatever the names, a figure key is where that server's own calls land, under either spelling of
        // the name, and where no other configured server's calls land.
        let landing = |name: &str| tool_names(name).iter().map(|tool| server_of(tool)).collect::<Vec<_>>();
        let mut singles = all_names(&["a", "_", ".", "-", " ", "\u{1F600}"], 4);
        singles.extend(all_names(&["a", "_", ".", "\u{1F600}"], 3).iter().map(|n| format!("claude.ai {n}")));
        for name in &singles {
            if let Some(key) = &figure_keys(&cc(&[name]))[0] {
                let lands = landing(name);
                assert!(lands.iter().all(|l| l.as_deref() == Some(key.as_str())), "{name:?} is read as {key}, but lands under {lands:?}");
            }
        }
        let pairs = all_names(&["a", "_", ".", "\u{1F600}"], 3);
        for first in &pairs {
            for second in &pairs {
                let keys = figure_keys(&cc(&[first, second]));
                for (own, other, key) in [(first, second, &keys[0]), (second, first, &keys[1])] {
                    let Some(key) = key else { continue };
                    let lands = landing(other);
                    assert!(lands.iter().all(|l| l.as_deref() != Some(key.as_str())), "{own:?} is read as {key}, but {other:?} lands there too");
                }
            }
        }
    }

    #[test]
    fn unused_needs_three_weeks_of_logs() {
        let servers = cc(&["busy", "idle"]);
        let all = throughout(&["busy", "idle"]);
        let by = usage(&[("busy", 3)]);
        assert!(unused(&servers, &by, 20, true, Some(&all)).is_empty(), "20 days of logs is not enough");
        assert_eq!(unused(&servers, &by, 21, true, Some(&all)), vec!["idle".to_string()], "21 days is three weeks");
        assert!(unused(&servers, &by, 0, true, Some(&all)).is_empty());
    }

    #[test]
    fn a_short_log_retention_setting_means_no_verdict() {
        let servers = cc(&["busy", "idle"]);
        let all = throughout(&["busy", "idle"]);
        let by = usage(&[("busy", 3)]);
        let verdict = |settings: &str| {
            let retention = crate::inventory::log_retention_from(&[settings]);
            unused(&servers, &by, DAYS, retention.covers(UNUSED_NEEDS_DAYS), Some(&all))
        };
        let idle = vec!["idle".to_string()];
        // No key means Claude Code's own default, 30 days, which covers the window.
        assert_eq!(verdict("{}"), idle);
        assert_eq!(verdict(r#"{"cleanupPeriodDays": 30}"#), idle);
        assert_eq!(verdict(r#"{"cleanupPeriodDays": 90}"#), idle);
        for days in ["29", "7", "0", "-1"] {
            assert!(verdict(&format!(r#"{{"cleanupPeriodDays": {days}}}"#)).is_empty(), "{days} days keeps logs for less than the window");
        }
        // A value that is not a whole number of days is not known, whatever it looks like.
        for odd in [r#""90""#, "29.5", "30.5", "null", "true", "[90]"] {
            assert!(verdict(&format!(r#"{{"cleanupPeriodDays": {odd}}}"#)).is_empty(), "{odd} is not a whole number of days");
        }
        // A settings file that cannot be read as an object says nothing about the key.
        assert!(verdict("not json").is_empty());
        assert!(verdict("[]").is_empty());
    }

    #[test]
    fn unused_needs_the_server_to_have_been_configured_throughout() {
        let servers = cc(&["busy", "idle"]);
        let by = usage(&[("busy", 3)]);
        // Added inside the window, or removed and re-added inside it: not in the set the history returns.
        assert!(unused(&servers, &by, DAYS, true, Some(&throughout(&["busy"]))).is_empty());
        assert!(unused(&servers, &by, DAYS, true, Some(&throughout(&[]))).is_empty());
        // No snapshot old enough: no verdict for anyone.
        assert!(unused(&servers, &by, DAYS, true, None).is_empty());
    }

    #[test]
    fn unused_needs_another_server_to_have_been_called() {
        let servers = cc(&["a", "b"]);
        let all = throughout(&["a", "b"]);
        assert!(unused(&servers, &HashMap::new(), DAYS, true, Some(&all)).is_empty(), "no call anywhere: the logs may not be read");
        // A call to a server nothing configures says nothing about the configured ones.
        assert!(unused(&servers, &usage(&[("gone", 9)]), DAYS, true, Some(&all)).is_empty());
    }

    #[test]
    fn calls_in_the_overflow_bucket_mean_no_verdict() {
        let servers = cc(&["busy", "idle"]);
        let all = throughout(&["busy", "idle"]);
        let by = usage(&[("busy", 3), (OTHER_MCP_SERVER, 1)]);
        assert!(unused(&servers, &by, DAYS, true, Some(&all)).is_empty(), "some server's calls are not attributed");
        let by = usage(&[("busy", 3), (OTHER_MCP_SERVER, 0)]);
        assert_eq!(unused(&servers, &by, DAYS, true, Some(&all)), vec!["idle".to_string()], "an empty bucket hides nothing");
    }

    #[test]
    fn the_overflow_key_is_the_one_scan_caches_already_hold() {
        assert_eq!(OTHER_MCP_SERVER, "(other)", "stored in users' scan caches: changing it would orphan what they hold");
    }

    #[test]
    fn a_server_of_another_client_is_never_judged() {
        let servers = vec![server("busy", "Claude Code"), server("idle", "Claude Desktop"), server("idle2", "Cursor")];
        let all = throughout(&["busy", "idle", "idle2"]);
        assert!(unused(&servers, &usage(&[("busy", 3)]), DAYS, true, Some(&all)).is_empty());
        // And a call figure for the name does not make another client's server count as the called one.
        let servers = vec![server("busy", "Claude Desktop"), server("idle", "Claude Code")];
        assert!(unused(&servers, &usage(&[("busy", 3)]), DAYS, true, Some(&all)).is_empty());
    }

    /// One snapshot per day for the 31 days ending `2026-10-05`, oldest first, each
    /// holding the Claude Code servers `holds(days_ago)` returns.
    fn thirty_one_days(holds: impl Fn(i64) -> Vec<&'static str>) -> Vec<Snapshot> {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        (0..=30)
            .rev()
            .map(|ago| Snapshot {
                taken: (today - chrono::Duration::days(ago)).format("%Y-%m-%d").to_string(),
                servers: holds(ago)
                    .into_iter()
                    .map(|name| changes::SnapServer {
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

    #[test]
    fn the_whole_rule_from_snapshots_and_logs() {
        let today = "2026-10-05";
        let servers = cc(&["busy", "idle"]);
        let by = usage(&[("busy", 3)]);
        let idle = vec!["idle".to_string()];
        let both = |_ago: i64| vec!["busy", "idle"];
        let history = thirty_one_days(both);
        assert_eq!(history.len(), 31);
        // Configured for 31 days, zero calls, logs and retention reach the window: named.
        assert_eq!(unused_from_history(&servers, &by, 30, true, &history, today), idle);

        // The snapshot 30 days old does not hold it: it may have been added since, so it is not named.
        let added_later = thirty_one_days(|ago| if ago == 30 { vec!["busy"] } else { vec!["busy", "idle"] });
        assert!(unused_from_history(&servers, &by, 30, true, &added_later, today).is_empty());
        // Nor when it is missing from any snapshot since.
        let removed_and_re_added = thirty_one_days(|ago| if ago == 12 { vec!["busy"] } else { vec!["busy", "idle"] });
        assert!(unused_from_history(&servers, &by, 30, true, &removed_and_re_added, today).is_empty());
        // A history only 30 days long has no snapshot old enough: no verdict for anyone.
        assert!(unused_from_history(&servers, &by, 30, true, &history[1..], today).is_empty());

        // Everything else in order but the retention setting, or the log coverage: nobody.
        assert!(unused_from_history(&servers, &by, 30, false, &history, today).is_empty(), "short retention");
        assert!(unused_from_history(&servers, &by, 20, true, &history, today).is_empty(), "logs reach back 20 days");
        // The whole rule judges only Claude Code's servers.
        let mixed = vec![server("busy", "Claude Code"), server("idle", "Claude Desktop")];
        assert!(unused_from_history(&mixed, &by, 30, true, &history, today).is_empty());
    }

    #[test]
    fn the_unused_finding_counts_what_it_names() {
        assert!(unused_opportunities(&[]).is_empty(), "absent when there is nothing to say");
        let names = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
        let found = unused_opportunities(&names);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].id.as_str(), found[0].kind.as_str()), ("mcp-unused", "learn"));
        assert_eq!(found[0].title, "3 MCP servers had no tool calls in the last 30 days");
        assert_eq!(found[0].title_msg.count, Some(3));
        assert!(found[0].detail.contains("alpha, beta, gamma"), "{}", found[0].detail);
        assert_eq!(unused_opportunities(&names[..1])[0].title, "1 MCP server had no tool calls in the last 30 days");
        assert_eq!(found[0].learn_url.as_deref(), Some(MCP_LEARN));
    }

    #[test]
    fn the_unused_strings_say_the_window_the_rule_uses() {
        let found = unused_opportunities(&["alpha".to_string()]);
        let days = format!("{UNUSED_NEEDS_DAYS} days");
        assert!(found[0].title.contains(&days), "{}", found[0].title);
        assert!(found[0].detail.contains(&days), "{}", found[0].detail);
    }

    #[test]
    fn the_unused_finding_reads_whole_in_every_language_and_size() {
        use crate::i18n::{render, LOCALES};
        for locale in LOCALES {
            for count in [1usize, 2, 5, 21] {
                let names: Vec<String> = (1..=count).map(|i| format!("srv{i}")).collect();
                let found = &unused_opportunities(&names)[0];
                let detail_msg = found.detail_msg.as_ref().expect("the finding has a detail");
                let (title, detail) = (render(locale, &found.title_msg), render(locale, detail_msg));
                for text in [&title, &detail] {
                    assert!(!text.contains('{') && !text.contains('}') && !text.contains("finding."), "{locale} {count}: {text}");
                    assert!(text.contains(&UNUSED_NEEDS_DAYS.to_string()), "{locale} {count} does not say the window: {text}");
                }
                assert!(title.contains(&count.to_string()), "{locale} {count}: {title}");
                assert!(detail.contains("srv1") && detail.contains(&format!("srv{count}")), "{locale} {count}: {detail}");
                if *locale != "en" {
                    assert_ne!(title, render("en", &found.title_msg), "{locale} {count} title still reads in English");
                    assert_ne!(detail, render("en", detail_msg), "{locale} {count} detail still reads in English");
                }
            }
        }
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
