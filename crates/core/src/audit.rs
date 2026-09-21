//! The audit: one structured read of a machine's AI setup, for a first run
//! and for a baseline to measure against later.
//!
//! It does not judge anything a second time. Every finding the app already
//! computes (the setup opportunities in `inventory.rs`, the usage ones in
//! `coaching.rs`) becomes a check that needs attention; its absence becomes a
//! pass. On top of that sit a few "is this set up at all" checks. The score
//! is arithmetic: checks passed over checks that apply. Nothing is graded by
//! feel, and a check that cannot be evaluated is left out of the score
//! instead of counting either way.

use crate::inventory::Inventory;
use crate::ledger::LedgerView;
use serde::Serialize;

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub id: String,
    /// "pass" | "attention" (a gap; scored) | "consider" (worth knowing or
    /// trying; not scored) | "info" (a fact; not scored)
    pub status: String,
    pub title: String,
    pub detail: String,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    pub name: String,
    pub checks: Vec<Check>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuditReport {
    pub generated_at: i64,
    pub passed: usize,
    pub attention: usize,
    /// 0 to 100, or None when nothing could be scored.
    pub score: Option<u32>,
    pub sections: Vec<Section>,
}

/// What the audit needs beyond the inventory (which carries every finding).
pub struct Inputs<'a> {
    pub inventory: &'a Inventory,
    pub ledger: &'a LedgerView,
    /// Total 30-day API-equivalent spend seen in local logs.
    pub spend30: f64,
    pub client_rules: usize,
    pub work_areas: usize,
}

fn check(id: &str, status: &str, title: impl Into<String>, detail: impl Into<String>) -> Check {
    Check { id: id.into(), status: status.into(), title: title.into(), detail: detail.into() }
}

/// A finding present → shown in its own words; absent → pass. Findings come
/// in two kinds and the audit keeps them apart: a "tighten" is a gap and
/// counts against the score; a "learn" is something worth knowing or trying
/// (a remote server, a capability not in use) and is not a failing, so it is
/// shown as "consider" and left out of the score.
fn from_finding(inv: &Inventory, id: &str, pass_title: &str, pass_detail: &str) -> Check {
    match inv.opportunities.iter().find(|o| o.id == id) {
        Some(o) if o.kind == "tighten" => check(id, "attention", o.title.clone(), o.detail.clone()),
        Some(o) => check(id, "consider", o.title.clone(), o.detail.clone()),
        None => check(id, "pass", pass_title, pass_detail),
    }
}

fn plural(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

pub fn run(i: &Inputs, now: i64) -> AuditReport {
    let inv = i.inventory;
    let packaged = inv.mcp_servers.iter().filter(|s| s.package.is_some()).count();

    let mut setup = vec![check(
        "tools",
        "info",
        format!("{} found", plural(inv.tools.len(), "AI tool")),
        if inv.tools.is_empty() {
            "None of the AI tools this app knows were found.".to_string()
        } else {
            inv.tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")
        },
    )];
    if inv.mcp_servers.is_empty() {
        setup.push(check("mcp", "info", "No MCP servers configured", "Nothing to check here yet."));
    } else {
        setup.push(check(
            "mcp",
            "info",
            format!("{} configured", plural(inv.mcp_servers.len(), "MCP server")),
            inv.mcp_servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "),
        ));
        if packaged > 0 {
            setup.push(from_finding(inv, "mcp-unpinned", "Every packaged MCP server is pinned", "A new release cannot start running with your tools' access on its own."));
        }
        setup.push(from_finding(inv, "mcp-env-secrets", "No MCP server is handed credentials", "No API keys are sitting in an MCP config."));
        setup.push(from_finding(inv, "mcp-remote", "No remote MCP servers", "Everything configured runs on this computer."));
    }

    let p = &inv.permissions;
    let guardrails = vec![
        if p.allow + p.ask + p.deny == 0 {
            from_finding(inv, "perm-none", "Permission rules are set", "")
        } else {
            check("perm-rules", "pass", "Permission rules are set", format!("{} allow, {} ask, {} deny.", p.allow, p.ask, p.deny))
        },
        if p.deny > 0 {
            check("perm-deny", "pass", format!("{} in place", plural(p.deny, "deny rule")), "There is a floor under what an agent may do.")
        } else {
            check("perm-deny", "attention", "No deny rules", "Nothing is ruled out. A short deny list (secrets, force-push, destructive deletes) is the cheapest guardrail there is.")
        },
        from_finding(inv, "hooks-none", "Hooks are configured", "Some rules run automatically instead of relying on the model to remember them."),
        check(
            "model",
            "info",
            match &inv.model {
                Some(m) => format!("Default model pinned: {m}"),
                None => "No default model pinned".to_string(),
            },
            "Informational: a pinned default keeps sessions predictable.",
        ),
    ];

    let mut usage = Vec::new();
    if i.spend30 >= 50.0 {
        usage.push(from_finding(inv, "mix-top-heavy", "The model mix is balanced", "Less than 70% of spend is on the largest models."));
        usage.push(from_finding(inv, "session-long-lived", "No costly long-running sessions", "No session has been open a week or more while costing real money."));
        if i.work_areas > 0 {
            usage.push(from_finding(inv, "areas-unsorted", "Spend is attributable", "Less than a quarter of spend is without a work area."));
        }
    } else {
        usage.push(check("usage-thin", "info", "Not enough local usage to judge", "Under $50 of usage in 30 days: shares would be noise, so nothing is scored here."));
    }
    usage.push(from_finding(inv, "agents-none", "Custom agents are in use", "Specialist agents keep long jobs out of the main context."));

    let l = i.ledger;
    let mut money = Vec::new();
    if l.items.is_empty() {
        money.push(check(
            "ledger",
            if inv.tools.is_empty() { "info" } else { "attention" },
            "No subscriptions recorded",
            "The app cannot say what the tools cost, when they renew, or whether each plan earns its price until the Subscriptions tab has your numbers.",
        ));
    } else {
        money.push(check("ledger", "pass", format!("{} tracked", plural(l.items.len(), "subscription")), format!("${:.0} a month, ${:.0} a year.", l.monthly, l.yearly)));
        let idle = l.items.iter().filter(|x| x.idle).count();
        money.push(if idle > 0 {
            check("ledger-idle", "attention", format!("{} with no measured usage", plural(idle, "plan")), format!("${:.0} a month is going to tools that showed no usage in 30 days.", l.idle_monthly))
        } else {
            check("ledger-idle", "pass", "No idle plans", "Every linked plan showed usage in the last 30 days.")
        });
        let undated = l.items.iter().filter(|x| x.next_renewal.is_none()).count();
        money.push(if undated > 0 {
            check("ledger-dates", "attention", format!("{} without a renewal date", plural(undated, "plan")), "Without a date there is no renewal reminder.")
        } else {
            check("ledger-dates", "pass", "Every plan has a renewal date", "Renewal reminders can fire.")
        });
    }
    if i.work_areas > 1 {
        money.push(if i.client_rules == 0 {
            check("clients", "attention", "Work areas are not mapped to clients", format!("{} exist. Mapping them turns spend into a per-client figure you can bill or justify.", plural(i.work_areas, "work area")))
        } else {
            check("clients", "pass", format!("{} set", plural(i.client_rules, "client rule")), "Spend rolls up to who the work was for.")
        });
    }

    let sections = vec![
        Section { name: "Setup".into(), checks: setup },
        Section { name: "Guardrails".into(), checks: guardrails },
        Section { name: "Usage".into(), checks: usage },
        Section { name: "Money".into(), checks: money },
    ];
    let count = |status: &str| sections.iter().flat_map(|s| &s.checks).filter(|c| c.status == status).count();
    let (passed, attention) = (count("pass"), count("attention"));
    AuditReport {
        generated_at: now,
        passed,
        attention,
        score: (passed + attention > 0).then(|| ((passed as f64 / (passed + attention) as f64) * 100.0).round() as u32),
        sections,
    }
}

/// The report as Markdown, for the file a person keeps or sends. Statuses
/// are written as words, so the file reads without colour.
pub fn to_markdown(r: &AuditReport, date: &str) -> String {
    let mut out = format!("# AI setup audit\n\n{date}\n\n");
    match r.score {
        Some(s) => out.push_str(&format!("**{s} of 100.** {} of {} scored checks pass; {} need attention.\n", r.passed, r.passed + r.attention, r.attention)),
        None => out.push_str("Nothing could be scored yet.\n"),
    }
    for section in &r.sections {
        out.push_str(&format!("\n## {}\n\n", section.name));
        for c in &section.checks {
            let word = match c.status.as_str() {
                "pass" => "PASS",
                "attention" => "NEEDS ATTENTION",
                "consider" => "WORTH A LOOK",
                _ => "NOTE",
            };
            out.push_str(&format!("- **{word}: {}**", c.title));
            if !c.detail.is_empty() {
                out.push_str(&format!("  \n  {}", c.detail));
            }
            out.push('\n');
        }
    }
    out.push_str("\n---\nComputed on this computer from its own configuration and logs. The score is checks passed over checks that apply. Only gaps count against it: \"worth a look\" items and notes are not scored.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::{AiTool, HookEvent, McpServer, Opportunity, Permissions};
    use crate::ledger::{view, Cycle, Subscription};
    use std::collections::HashMap;

    fn finding(id: &str, title: &str) -> Opportunity {
        Opportunity { id: id.into(), kind: "tighten".into(), title: title.into(), detail: format!("detail of {id}"), learn_url: None }
    }

    fn server(name: &str, package: Option<&str>) -> McpServer {
        McpServer {
            name: name.into(), client: "Claude Code".into(), scope: "user".into(), project: None, transport: "stdio".into(),
            target: "npx".into(), package: package.map(str::to_string), env_count: 0, pin_to: None, source_file: None,
        }
    }

    fn ledger(items: &[Subscription], usage: &[(&str, f64)]) -> LedgerView {
        let usage: HashMap<String, f64> = usage.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        view(items, chrono::NaiveDate::from_ymd_opt(2026, 9, 21).unwrap(), &usage)
    }

    fn sub(name: &str, renews: Option<&str>, provider: Option<&str>) -> Subscription {
        Subscription {
            id: name.into(), name: name.into(), price: 100.0, cycle: Cycle::Monthly,
            renews_on: renews.map(str::to_string), provider: provider.map(str::to_string), notes: None, reminded_for: None,
        }
    }

    fn statuses(r: &AuditReport) -> Vec<(String, String)> {
        r.sections.iter().flat_map(|s| &s.checks).map(|c| (c.id.clone(), c.status.clone())).collect()
    }

    #[test]
    fn a_tidy_setup_scores_100_and_every_pass_says_why() {
        let inv = Inventory {
            tools: vec![AiTool { name: "Claude Code".into(), kind: "app".into(), mcp_servers: 1 }],
            mcp_servers: vec![server("docs", Some("docs-mcp@2"))],
            permissions: Permissions { default_mode: None, allow: 3, ask: 0, deny: 9 },
            hooks: vec![HookEvent { event: "SessionEnd".into(), count: 1 }],
            model: Some("claude-sonnet-5".into()),
            ..Inventory::default()
        };
        let l = ledger(&[sub("Claude", Some("2026-10-01"), Some("claude"))], &[("claude", 400.0)]);
        let r = run(&Inputs { inventory: &inv, ledger: &l, spend30: 400.0, client_rules: 2, work_areas: 6 }, 1);
        assert_eq!(r.attention, 0, "{:?}", statuses(&r));
        assert_eq!(r.score, Some(100));
        assert!(r.sections.iter().flat_map(|s| &s.checks).filter(|c| c.status == "pass").all(|c| !c.title.is_empty()));
    }

    #[test]
    fn findings_become_attention_with_their_own_words_and_the_score_is_arithmetic() {
        let inv = Inventory {
            tools: vec![AiTool { name: "Claude Code".into(), kind: "app".into(), mcp_servers: 2 }],
            mcp_servers: vec![server("loose", Some("loose-mcp")), server("bin", None)],
            permissions: Permissions { default_mode: None, allow: 0, ask: 0, deny: 23 },
            opportunities: vec![
                finding("mcp-unpinned", "1 MCP server runs an unpinned package"),
                finding("mix-top-heavy", "92% of spend is on the largest models"),
                finding("agents-none", "No custom agents defined"),
                finding("hooks-none", "No hooks configured"),
            ],
            ..Inventory::default()
        };
        let l = ledger(&[], &[]);
        let r = run(&Inputs { inventory: &inv, ledger: &l, spend30: 2000.0, client_rules: 0, work_areas: 40 }, 1);
        let got = statuses(&r);
        let status = |id: &str| got.iter().find(|(i, _)| i == id).map(|(_, s)| s.as_str());
        assert_eq!(status("mcp-unpinned"), Some("attention"));
        assert_eq!(status("mcp-remote"), Some("pass"));
        assert_eq!(status("perm-deny"), Some("pass"));
        assert_eq!(status("mix-top-heavy"), Some("attention"));
        assert_eq!(status("session-long-lived"), Some("pass"));
        assert_eq!(status("ledger"), Some("attention"));
        assert_eq!(status("clients"), Some("attention"));
        assert_eq!(status("tools"), Some("info"));
        let unpinned = r.sections[0].checks.iter().find(|c| c.id == "mcp-unpinned").unwrap();
        assert_eq!(unpinned.title, "1 MCP server runs an unpinned package", "the finding's own words, not a second verdict");
        assert_eq!(r.score, Some(((r.passed as f64 / (r.passed + r.attention) as f64) * 100.0).round() as u32));
        assert!(r.attention >= 6 && r.passed >= 5, "{} pass, {} attention", r.passed, r.attention);
    }

    #[test]
    fn something_to_learn_is_not_a_failing() {
        let mut remote = finding("mcp-remote", "1 remote MCP server");
        remote.kind = "learn".into();
        let mut agents = finding("agents-none", "No custom agents defined");
        agents.kind = "learn".into();
        let inv = Inventory {
            mcp_servers: vec![server("cloud", None)],
            permissions: Permissions { default_mode: None, allow: 1, ask: 0, deny: 5 },
            hooks: vec![HookEvent { event: "SessionEnd".into(), count: 1 }],
            opportunities: vec![remote, agents],
            ..Inventory::default()
        };
        let l = ledger(&[], &[]);
        let r = run(&Inputs { inventory: &inv, ledger: &l, spend30: 0.0, client_rules: 0, work_areas: 0 }, 1);
        let got = statuses(&r);
        assert!(got.contains(&("mcp-remote".into(), "consider".into())));
        assert!(got.contains(&("agents-none".into(), "consider".into())));
        assert_eq!(r.attention, 0);
        assert_eq!(r.score, Some(100), "two things to consider cost nothing");
    }

    #[test]
    fn what_cannot_be_judged_is_left_out_of_the_score() {
        let inv = Inventory::default();
        let l = ledger(&[], &[]);
        let r = run(&Inputs { inventory: &inv, ledger: &l, spend30: 3.0, client_rules: 0, work_areas: 0 }, 1);
        let got = statuses(&r);
        assert!(got.contains(&("usage-thin".into(), "info".into())), "thin usage is a note, not a pass");
        assert!(!got.iter().any(|(id, _)| id == "mix-top-heavy" || id == "clients" || id == "mcp-unpinned"));
        assert!(got.contains(&("ledger".into(), "info".into())), "no tools found: an empty ledger is not a failing");
    }

    #[test]
    fn idle_and_undated_plans_are_called_out() {
        let inv = Inventory { tools: vec![AiTool { name: "Cursor".into(), kind: "app".into(), mcp_servers: 0 }], ..Inventory::default() };
        let l = ledger(&[sub("Cursor", None, Some("cursor")), sub("Claude", Some("2026-10-01"), Some("claude"))], &[("cursor", 0.0), ("claude", 900.0)]);
        let r = run(&Inputs { inventory: &inv, ledger: &l, spend30: 900.0, client_rules: 0, work_areas: 0 }, 1);
        let by = |id: &str| r.sections[3].checks.iter().find(|c| c.id == id).unwrap().clone();
        assert_eq!(by("ledger-idle").status, "attention");
        assert!(by("ledger-idle").detail.contains("$100 a month"));
        assert_eq!(by("ledger-dates").title, "1 plan without a renewal date");
    }

    /// This machine's real audit as JSON. `--ignored --nocapture`
    #[test]
    #[ignore]
    fn live_audit() {
        let mut inv = crate::inventory::scan();
        let spend = crate::spend::collect(None);
        let claude = spend.iter().find(|p| p.id == "claude");
        inv.opportunities.extend(crate::coaching::opportunities(claude, &crate::spend::claude_sessions(None, None, 500)));
        let l = view(&crate::ledger::load_from(&crate::ledger::path()), chrono::Local::now().date_naive(), &HashMap::new());
        let areas: std::collections::HashSet<&str> = spend.iter().flat_map(|p| p.projects.iter())
            .flat_map(|pr| pr.areas.iter()).map(|a| crate::spend::area_top(&a.area)).filter(|a| !a.starts_with('(')).collect();
        let r = run(&Inputs { inventory: &inv, ledger: &l, spend30: spend.iter().map(|p| p.last30.cost).sum(),
            client_rules: crate::clients::load_from(&crate::clients::path()).len(), work_areas: areas.len() }, 1);
        println!("{}", serde_json::to_string(&r).unwrap());
    }

    #[test]
    fn the_markdown_reads_without_colour() {
        let inv = Inventory { opportunities: vec![finding("hooks-none", "No hooks configured")], ..Inventory::default() };
        let l = ledger(&[], &[]);
        let md = to_markdown(&run(&Inputs { inventory: &inv, ledger: &l, spend30: 0.0, client_rules: 0, work_areas: 0 }, 1), "21 September 2026");
        assert!(md.starts_with("# AI setup audit\n\n21 September 2026"));
        assert!(md.contains("- **NEEDS ATTENTION: No hooks configured**  \n  detail of hooks-none"));
        assert!(md.contains("- **NOTE: 0 AI tools found**"));
        assert!(md.contains("## Guardrails") && md.contains("checks passed over checks that apply"));
    }
}
