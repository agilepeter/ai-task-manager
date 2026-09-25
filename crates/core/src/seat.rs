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
use crate::spend::ProviderSpend;
use serde::{Deserialize, Serialize};

pub const SCHEMA: u32 = 1;

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
    }
}

/// A report from the wire: bounded, the right schema, a sane shape.
pub fn parse(raw: &str) -> Result<SeatReport, String> {
    if raw.len() > 512 * 1024 {
        return Err("report too large".into());
    }
    let report: SeatReport = serde_json::from_str(raw).map_err(|e| format!("not a seat report: {e}"))?;
    if report.schema != SCHEMA {
        return Err(format!("unsupported schema {}", report.schema));
    }
    let id_ok = (8..=64).contains(&report.seat_id.len())
        && report.seat_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if !id_ok {
        return Err("bad seat id".into());
    }
    if report.servers.len() > 500 || report.tools.len() > 100 || report.findings.len() > 100 || report.limits.len() > 100 {
        return Err("report has too many entries".into());
    }
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
    use crate::inventory::{AiTool, HookEvent, McpServer, Opportunity, Permissions};
    use crate::spend::{AreaSpend, ModelSpend, ProjectSpend, Window};

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
