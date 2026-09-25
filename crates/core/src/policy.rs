//! A team's AI tooling policy, checked against seat reports.
//!
//! The policy lives with the collector and is evaluated there. Seats never
//! receive it and cannot edit it, and a seat cannot claim to conform: the
//! verdict is computed from what the seat reported. Every rule is optional;
//! an empty policy passes everything.

use crate::seat::SeatReport;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Policy {
    /// If not empty, every MCP package must match one of these (`*` wildcard).
    pub allowed_packages: Vec<String>,
    /// Never allowed, whatever `allowed_packages` says.
    pub blocked_packages: Vec<String>,
    /// Every package-run server must carry a version.
    pub require_pinned: bool,
    /// Remote (http / sse) servers are refused when this is false.
    pub allow_remote: Option<bool>,
    /// A seat must have at least this many deny rules.
    pub min_deny_rules: usize,
    /// If not empty, only these AI tools may be present.
    pub allowed_tools: Vec<String>,
    /// Every custom agent must carry a `tools` allowlist: none may be left
    /// to use every tool the parent has, shell included.
    pub require_agent_tools: bool,
    /// At least one deny rule must target the shell.
    pub require_shell_deny: bool,
    /// Every one of these hook events must be present on a seat. Event
    /// names, case-sensitive ("PreToolUse", "Stop", …).
    pub require_hooks: Vec<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Violation {
    /// Stable id: "blocked-package", "unlisted-package", "unpinned",
    /// "remote-server", "deny-rules", "unlisted-tool", "agent-unrestricted",
    /// "shell-deny", "hook-missing".
    pub rule: String,
    pub detail: String,
}

fn matches_any(patterns: &[String], name: &str) -> bool {
    patterns.iter().any(|p| crate::clients::glob_match(p.trim(), name))
}

pub fn check(report: &SeatReport, policy: &Policy) -> Vec<Violation> {
    let mut out = Vec::new();
    let mut push = |rule: &str, detail: String| out.push(Violation { rule: rule.into(), detail });
    for s in &report.servers {
        let who = format!("{} ({})", s.name, s.client);
        if let Some(package) = s.package.as_deref() {
            let name = crate::pin::bare(package);
            if matches_any(&policy.blocked_packages, name) {
                push("blocked-package", format!("{who} runs {name}, which is blocked"));
            } else if !policy.allowed_packages.is_empty() && !matches_any(&policy.allowed_packages, name) {
                push("unlisted-package", format!("{who} runs {name}, which is not on the approved list"));
            }
            if policy.require_pinned && s.pinned == Some(false) {
                push("unpinned", format!("{who} runs {package} without a pinned version"));
            }
        }
        if policy.allow_remote == Some(false) && s.transport != "stdio" {
            push("remote-server", format!("{who} is a remote server ({})", s.target));
        }
    }
    if report.deny_rules < policy.min_deny_rules {
        push("deny-rules", format!("{} deny rules, the policy asks for {}", report.deny_rules, policy.min_deny_rules));
    }
    if !policy.allowed_tools.is_empty() {
        for tool in report.tools.iter().filter(|t| !policy.allowed_tools.iter().any(|a| a.eq_ignore_ascii_case(t))) {
            push("unlisted-tool", format!("{tool} is not an approved AI tool"));
        }
    }
    if policy.require_agent_tools && report.agents_unrestricted > 0 {
        push("agent-unrestricted", format!("{} agents can use every tool", report.agents_unrestricted));
    }
    if policy.require_shell_deny && !report.deny_covers_shell {
        push("shell-deny", "no deny rule limits the shell".to_string());
    }
    for event in &policy.require_hooks {
        if !report.hook_events.contains(event) {
            push("hook-missing", format!("no {event} hook"));
        }
    }
    out
}

/// A policy file as written by hand: unknown keys are ignored, a missing
/// file or broken JSON is no policy rather than an error page.
pub fn parse(raw: &str) -> Option<Policy> {
    serde_json::from_str(raw).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seat::SeatServer;

    fn server(name: &str, package: Option<&str>, pinned: Option<bool>, transport: &str) -> SeatServer {
        SeatServer {
            name: name.into(), client: "Claude Code".into(), scope: "user".into(), transport: transport.into(),
            target: if transport == "stdio" { "npx".into() } else { "mcp.example.com".into() },
            package: package.map(str::to_string), pinned, env_count: 0,
        }
    }

    fn report(servers: Vec<SeatServer>, deny: usize, tools: &[&str]) -> SeatReport {
        SeatReport {
            schema: crate::seat::SCHEMA, seat_id: "seat-abcdefgh".into(), label: "x".into(), generated_at: 0,
            agent_version: "0".into(), os: "macos".into(), tools: tools.iter().map(|t| t.to_string()).collect(),
            servers, agents: 0, skills: 0, hooks: 0, permission_mode: None, allow_rules: 0, ask_rules: 0,
            deny_rules: deny, spend: vec![], findings: vec![], limits: vec![],
            agents_unrestricted: 0, agents_model_unset: 0, deny_covers_shell: false, hook_events: vec![],
        }
    }

    fn rules(found: &[Violation]) -> Vec<&str> {
        found.iter().map(|v| v.rule.as_str()).collect()
    }

    #[test]
    fn an_empty_policy_passes_everything() {
        let r = report(vec![server("a", Some("any-mcp"), Some(false), "stdio"), server("b", None, None, "http")], 0, &["Aider"]);
        assert!(check(&r, &Policy::default()).is_empty());
        assert_eq!(parse("{}"), Some(Policy::default()));
        assert_eq!(parse("not json"), None);
    }

    #[test]
    fn each_rule_names_the_server_and_blocked_beats_allowed() {
        let policy = parse(r#"{
            "allowedPackages": ["@acme/*", "chrome-devtools-mcp", "sketchy-mcp"],
            "blockedPackages": ["sketchy-*"],
            "requirePinned": true, "allowRemote": false, "minDenyRules": 5,
            "allowedTools": ["claude code"], "somethingNew": 1
        }"#).unwrap();
        let r = report(
            vec![
                server("ok", Some("@acme/docs-mcp@2"), Some(true), "stdio"),
                server("loose", Some("chrome-devtools-mcp@latest"), Some(false), "stdio"),
                server("bad", Some("sketchy-mcp@1"), Some(true), "stdio"),
                server("unknown", Some("random-mcp@1"), Some(true), "stdio"),
                server("cloud", None, None, "http"),
                server("binary", None, None, "stdio"),
            ],
            2,
            &["Claude Code", "Cursor"],
        );
        let found = check(&r, &policy);
        assert_eq!(rules(&found), ["unpinned", "blocked-package", "unlisted-package", "remote-server", "deny-rules", "unlisted-tool"]);
        assert!(found[0].detail.starts_with("loose (Claude Code) runs chrome-devtools-mcp@latest"));
        assert!(found[1].detail.contains("sketchy-mcp, which is blocked"), "on the allow list too, still blocked");
        assert!(found[5].detail.starts_with("Cursor"), "tool names match without regard to case");
    }

    #[test]
    fn versions_do_not_defeat_a_package_rule() {
        let policy = Policy { blocked_packages: vec!["evil-mcp".into()], ..Policy::default() };
        let r = report(vec![server("x", Some("evil-mcp@9.9.9"), Some(true), "stdio")], 0, &[]);
        assert_eq!(rules(&check(&r, &policy)), ["blocked-package"]);
    }

    #[test]
    fn agent_rules_name_the_missing_hook_and_count_unrestricted_agents() {
        let mut r = report(vec![], 0, &[]);
        r.agents_unrestricted = 2;
        r.deny_covers_shell = false;
        r.hook_events = vec!["PreToolUse".into()];
        let policy = Policy {
            require_agent_tools: true,
            require_shell_deny: true,
            require_hooks: vec!["PreToolUse".into(), "Stop".into()],
            ..Policy::default()
        };
        let found = check(&r, &policy);
        assert_eq!(rules(&found), ["agent-unrestricted", "shell-deny", "hook-missing"]);
        assert_eq!(found[0].detail, "2 agents can use every tool");
        assert_eq!(found[1].detail, "no deny rule limits the shell");
        assert_eq!(found[2].detail, "no Stop hook", "case-sensitive and names the missing event");

        // A satisfied guardrail names nothing, and matching is case-sensitive.
        r.agents_unrestricted = 0;
        r.deny_covers_shell = true;
        r.hook_events = vec!["pretoolonly".into()];
        let policy = Policy { require_hooks: vec!["PreToolUse".into()], ..policy };
        assert_eq!(rules(&check(&r, &policy)), ["hook-missing"], "a differently-cased or unrelated event never satisfies the rule");

        // Neither required hook is present: each is named on its own
        // violation, in the order the policy lists them.
        r.hook_events = vec![];
        let policy = Policy { require_hooks: vec!["PreToolUse".into(), "Stop".into()], ..policy };
        let found = check(&r, &policy);
        assert_eq!(rules(&found), ["hook-missing", "hook-missing"]);
        assert_eq!(found[0].detail, "no PreToolUse hook");
        assert_eq!(found[1].detail, "no Stop hook");
    }
}
