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

use crate::i18n::{self, Msg};
use crate::inventory::{Inventory, Opportunity};
use crate::ledger::LedgerView;
use serde::Serialize;

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub id: String,
    /// "pass" | "attention" (a gap; scored) | "consider" (worth knowing or
    /// trying; not scored) | "info" (a fact; not scored)
    pub status: String,
    /// English, produced by render("en", &title_msg) of the same Msg.
    pub title: String,
    pub detail: String,
    pub title_msg: Msg,
    /// `None` when detail is empty, or is not a sentence at all (the joined
    /// tool / server names under the "tools" and "mcp" checks) -- there is
    /// nothing there to translate.
    pub detail_msg: Option<Msg>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    pub name: String,
    pub name_key: &'static str,
    pub checks: Vec<Check>,
}

/// Every `check.<id>.<variant>` key prefix this module can emit on its own
/// account, plus the `.pass` variant of every finding id it turns into a
/// check via `from_finding` (see below) -- together, every `check.*` prefix
/// that can ever reach the popover.
pub const CHECK_KEYS: &[&str] = &[
    "check.tools.info",
    "check.mcp.none",
    "check.mcp.configured",
    "check.perm-rules.pass",
    "check.perm-deny.pass",
    "check.perm-deny.attention",
    "check.model.info",
    "check.usage-thin.info",
    "check.ledger.none",
    "check.ledger.tracked",
    "check.ledger-idle.attention",
    "check.ledger-idle.pass",
    "check.ledger-dates.attention",
    "check.ledger-dates.pass",
    "check.clients.attention",
    "check.clients.pass",
    "check.mcp-unpinned.pass",
    "check.mcp-env-secrets.pass",
    "check.mcp-remote.pass",
    "check.mcp-duplicate-processes.pass",
    "check.mcp-running-unconfigured.pass",
    "check.perm-none.pass",
    "check.hooks-none.pass",
    "check.pricing-cache-ttl.pass",
    "check.mix-top-heavy.pass",
    "check.session-long-lived.pass",
    "check.areas-unsorted.pass",
    "check.agents-none.pass",
];

pub const SECTION_KEYS: &[&str] = &["section.setup", "section.guardrails", "section.usage", "section.money"];

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

/// A check whose title and detail are both real sentences.
fn check(id: &str, status: &str, title_msg: Msg, detail_msg: Msg) -> Check {
    Check {
        id: id.into(),
        status: status.into(),
        title: i18n::render("en", &title_msg),
        detail: i18n::render("en", &detail_msg),
        title_msg,
        detail_msg: Some(detail_msg),
    }
}

/// A check whose detail is not a sentence -- pure data (the joined tool or
/// server names), or deliberately empty -- so there is no Msg behind it: the
/// string is the same in every locale because there is nothing in it to
/// translate.
fn check_data(id: &str, status: &str, title_msg: Msg, detail: impl Into<String>) -> Check {
    Check {
        id: id.into(),
        status: status.into(),
        title: i18n::render("en", &title_msg),
        detail: detail.into(),
        title_msg,
        detail_msg: None,
    }
}

/// A finding's own Msgs, cloned onto a check with the given status -- never
/// re-keyed, so the check reads exactly as the finding does.
fn check_from_opportunity(id: &str, status: &str, o: &Opportunity) -> Check {
    Check {
        id: id.into(),
        status: status.into(),
        title: o.title.clone(),
        detail: o.detail.clone(),
        title_msg: o.title_msg.clone(),
        detail_msg: o.detail_msg.clone(),
    }
}

/// A finding present → shown in its own words; absent → pass. Findings come
/// in two kinds and the audit keeps them apart: a "tighten" is a gap and
/// counts against the score; a "learn" is something worth knowing or trying
/// (a remote server, a capability not in use) and is not a failing, so it is
/// shown as "consider" and left out of the score.
fn from_finding(inv: &Inventory, id: &str, pass_title_msg: Msg, pass_detail_msg: Option<Msg>) -> Check {
    match inv.opportunities.iter().find(|o| o.id == id) {
        Some(o) if o.kind == "tighten" => check_from_opportunity(id, "attention", o),
        Some(o) => check_from_opportunity(id, "consider", o),
        None => Check {
            id: id.into(),
            status: "pass".into(),
            title: i18n::render("en", &pass_title_msg),
            detail: pass_detail_msg.as_ref().map(|m| i18n::render("en", m)).unwrap_or_default(),
            title_msg: pass_title_msg,
            detail_msg: pass_detail_msg,
        },
    }
}

/// A finding that has no meaningful "pass": it either applies or it does not,
/// so its absence adds no row rather than an empty one.
fn only_if_present(inv: &Inventory, id: &str) -> Option<Check> {
    inv.opportunities
        .iter()
        .find(|o| o.id == id)
        .map(|o| check_from_opportunity(id, if o.kind == "tighten" { "attention" } else { "consider" }, o))
}

fn section(name_key: &'static str, checks: Vec<Check>) -> Section {
    Section { name: i18n::render("en", &Msg::new(name_key)), name_key, checks }
}

pub fn run(i: &Inputs, now: i64) -> AuditReport {
    let inv = i.inventory;
    let packaged = inv.mcp_servers.iter().filter(|s| s.package.is_some()).count();

    // The title's own count already reads fine at zero ("0 AI tools found"),
    // so only the detail needs a branch: a real sentence when the list is
    // empty, pure data (the joined names, untranslated -- there is nothing
    // in a name to translate) when it is not.
    let mut setup = vec![if inv.tools.is_empty() {
        check(
            "tools",
            "info",
            Msg::new("check.tools.info.title").count(0),
            Msg::new("check.tools.info.detail"),
        )
    } else {
        check_data(
            "tools",
            "info",
            Msg::new("check.tools.info.title").count(inv.tools.len() as i64),
            inv.tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", "),
        )
    }];
    if inv.mcp_servers.is_empty() {
        setup.push(check(
            "mcp",
            "info",
            Msg::new("check.mcp.none.title"),
            Msg::new("check.mcp.none.detail"),
        ));
    } else {
        setup.push(check_data(
            "mcp",
            "info",
            Msg::new("check.mcp.configured.title").count(inv.mcp_servers.len() as i64),
            inv.mcp_servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "),
        ));
        if packaged > 0 {
            setup.push(from_finding(
                inv,
                "mcp-unpinned",
                Msg::new("check.mcp-unpinned.pass.title"),
                Some(Msg::new("check.mcp-unpinned.pass.detail")),
            ));
        }
        setup.push(from_finding(
            inv,
            "mcp-env-secrets",
            Msg::new("check.mcp-env-secrets.pass.title"),
            Some(Msg::new("check.mcp-env-secrets.pass.detail")),
        ));
        setup.push(from_finding(
            inv,
            "mcp-remote",
            Msg::new("check.mcp-remote.pass.title"),
            Some(Msg::new("check.mcp-remote.pass.detail")),
        ));
        setup.push(from_finding(
            inv,
            "mcp-duplicate-processes",
            Msg::new("check.mcp-duplicate-processes.pass.title"),
            Some(Msg::new("check.mcp-duplicate-processes.pass.detail")),
        ));
        setup.push(from_finding(
            inv,
            "mcp-running-unconfigured",
            Msg::new("check.mcp-running-unconfigured.pass.title"),
            Some(Msg::new("check.mcp-running-unconfigured.pass.detail")),
        ));
        setup.extend(only_if_present(inv, "mcp-memory"));
    }

    let p = &inv.permissions;
    let guardrails = vec![
        if p.allow + p.ask + p.deny == 0 {
            from_finding(inv, "perm-none", Msg::new("check.perm-none.pass.title"), None)
        } else {
            check(
                "perm-rules",
                "pass",
                Msg::new("check.perm-rules.pass.title"),
                Msg::new("check.perm-rules.pass.detail").var("allow", p.allow).var("ask", p.ask).var("deny", p.deny),
            )
        },
        if p.deny > 0 {
            check(
                "perm-deny",
                "pass",
                Msg::new("check.perm-deny.pass.title").count(p.deny as i64),
                Msg::new("check.perm-deny.pass.detail"),
            )
        } else {
            check(
                "perm-deny",
                "attention",
                Msg::new("check.perm-deny.attention.title"),
                Msg::new("check.perm-deny.attention.detail"),
            )
        },
        from_finding(
            inv,
            "hooks-none",
            Msg::new("check.hooks-none.pass.title"),
            Some(Msg::new("check.hooks-none.pass.detail")),
        ),
        check(
            "model",
            "info",
            match &inv.model {
                Some(m) => Msg::new("check.model.info.title").var("model", m),
                None => Msg::new("check.model.info.titleUnset"),
            },
            Msg::new("check.model.info.detail"),
        ),
    ];

    let mut usage = Vec::new();
    // Whether the dollar figures can be trusted belongs before anything that
    // is measured in dollars.
    usage.push(from_finding(
        inv,
        "pricing-cache-ttl",
        Msg::new("check.pricing-cache-ttl.pass.title"),
        Some(Msg::new("check.pricing-cache-ttl.pass.detail")),
    ));
    usage.extend(only_if_present(inv, "pricing-drift"));
    if i.spend30 >= 50.0 {
        usage.push(from_finding(
            inv,
            "mix-top-heavy",
            Msg::new("check.mix-top-heavy.pass.title"),
            Some(Msg::new("check.mix-top-heavy.pass.detail")),
        ));
        usage.push(from_finding(
            inv,
            "session-long-lived",
            Msg::new("check.session-long-lived.pass.title"),
            Some(Msg::new("check.session-long-lived.pass.detail")),
        ));
        if i.work_areas > 0 {
            usage.push(from_finding(
                inv,
                "areas-unsorted",
                Msg::new("check.areas-unsorted.pass.title"),
                Some(Msg::new("check.areas-unsorted.pass.detail")),
            ));
        }
    } else {
        usage.push(check(
            "usage-thin",
            "info",
            Msg::new("check.usage-thin.info.title"),
            Msg::new("check.usage-thin.info.detail"),
        ));
    }
    usage.push(from_finding(
        inv,
        "agents-none",
        Msg::new("check.agents-none.pass.title"),
        Some(Msg::new("check.agents-none.pass.detail")),
    ));

    let l = i.ledger;
    let mut money = Vec::new();
    if l.items.is_empty() {
        money.push(check(
            "ledger",
            if inv.tools.is_empty() { "info" } else { "attention" },
            Msg::new("check.ledger.none.title"),
            Msg::new("check.ledger.none.detail"),
        ));
    } else {
        money.push(check(
            "ledger",
            "pass",
            Msg::new("check.ledger.tracked.title").count(l.items.len() as i64),
            Msg::new("check.ledger.tracked.detail")
                .var("monthly", format!("{:.0}", l.monthly))
                .var("yearly", format!("{:.0}", l.yearly)),
        ));
        let idle = l.items.iter().filter(|x| x.idle).count();
        money.push(if idle > 0 {
            check(
                "ledger-idle",
                "attention",
                Msg::new("check.ledger-idle.attention.title").count(idle as i64),
                Msg::new("check.ledger-idle.attention.detail").var("idleMonthly", format!("{:.0}", l.idle_monthly)),
            )
        } else {
            check(
                "ledger-idle",
                "pass",
                Msg::new("check.ledger-idle.pass.title"),
                Msg::new("check.ledger-idle.pass.detail"),
            )
        });
        let undated = l.items.iter().filter(|x| x.next_renewal.is_none()).count();
        money.push(if undated > 0 {
            check(
                "ledger-dates",
                "attention",
                Msg::new("check.ledger-dates.attention.title").count(undated as i64),
                Msg::new("check.ledger-dates.attention.detail"),
            )
        } else {
            check(
                "ledger-dates",
                "pass",
                Msg::new("check.ledger-dates.pass.title"),
                Msg::new("check.ledger-dates.pass.detail"),
            )
        });
    }
    if i.work_areas > 1 {
        money.push(if i.client_rules == 0 {
            check(
                "clients",
                "attention",
                Msg::new("check.clients.attention.title"),
                Msg::new("check.clients.attention.detail").count(i.work_areas as i64),
            )
        } else {
            check(
                "clients",
                "pass",
                Msg::new("check.clients.pass.title").count(i.client_rules as i64),
                Msg::new("check.clients.pass.detail"),
            )
        });
    }

    let sections = vec![
        section("section.setup", setup),
        section("section.guardrails", guardrails),
        section("section.usage", usage),
        section("section.money", money),
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
        Opportunity {
            id: id.into(),
            kind: "tighten".into(),
            title: title.into(),
            detail: format!("detail of {id}"),
            // A placeholder Msg: these tests check the rendered title/detail
            // strings a check carries, never the key behind them.
            title_msg: Msg::new("finding.test.title"),
            detail_msg: Some(Msg::new("finding.test.detail")),
            learn_url: None,
        }
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
    fn a_finding_with_no_pass_state_adds_no_row_when_absent() {
        let inv = Inventory::default();
        assert!(super::only_if_present(&inv, "mcp-memory").is_none());
        let inv = Inventory {
            opportunities: vec![crate::inventory::Opportunity {
                id: "mcp-memory".into(),
                kind: "learn".into(),
                title: "MCP servers are holding 2115 MB".into(),
                detail: "d".into(),
                title_msg: Msg::new("finding.test.title"),
                detail_msg: Some(Msg::new("finding.test.detail")),
                learn_url: None,
            }],
            ..Inventory::default()
        };
        let c = super::only_if_present(&inv, "mcp-memory").expect("present");
        assert_eq!(c.status, "consider", "a resting cost is not a failing");
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

    /// A missing translation key renders as its own literal key text instead
    /// of failing -- that takes a real fixture run to catch. Three inputs
    /// together clear most of the standalone-check and pass/attention
    /// branches: a tidy, pass-heavy setup; a gap-heavy one with real
    /// findings; and a bare-minimum one (empty MCP, thin spend, an idle and
    /// an undated subscription) for the branches neither of the other two
    /// reaches.
    #[test]
    fn checks_never_render_a_raw_key() {
        let tidy = Inventory {
            tools: vec![AiTool { name: "Claude Code".into(), kind: "app".into(), mcp_servers: 1 }],
            mcp_servers: vec![server("docs", Some("docs-mcp@2"))],
            permissions: Permissions { default_mode: None, allow: 3, ask: 0, deny: 9 },
            hooks: vec![HookEvent { event: "SessionEnd".into(), count: 1 }],
            model: Some("claude-sonnet-5".into()),
            ..Inventory::default()
        };
        let tidy_ledger = ledger(&[sub("Claude", Some("2026-10-01"), Some("claude"))], &[("claude", 400.0)]);
        let tidy_inputs =
            Inputs { inventory: &tidy, ledger: &tidy_ledger, spend30: 400.0, client_rules: 2, work_areas: 6 };

        let loose = Inventory {
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
        let loose_ledger = ledger(&[], &[]);
        let loose_inputs =
            Inputs { inventory: &loose, ledger: &loose_ledger, spend30: 2000.0, client_rules: 0, work_areas: 40 };

        let thin = Inventory { tools: vec![AiTool { name: "Cursor".into(), kind: "app".into(), mcp_servers: 0 }], ..Inventory::default() };
        let thin_ledger = ledger(
            &[sub("Cursor", None, Some("cursor")), sub("Claude", Some("2026-10-01"), Some("claude"))],
            &[("cursor", 0.0), ("claude", 900.0)],
        );
        let thin_inputs = Inputs { inventory: &thin, ledger: &thin_ledger, spend30: 3.0, client_rules: 0, work_areas: 0 };

        for inputs in [tidy_inputs, loose_inputs, thin_inputs] {
            let r = run(&inputs, 1);
            for s in &r.sections {
                assert!(!s.name.starts_with("section."), "{}: raw key in section name: {}", s.name_key, s.name);
                for c in &s.checks {
                    assert!(!c.title.starts_with("check."), "{}: raw key in title: {}", c.id, c.title);
                    assert!(!c.detail.starts_with("check."), "{}: raw key in detail: {}", c.id, c.detail);
                }
            }
        }
    }
}
