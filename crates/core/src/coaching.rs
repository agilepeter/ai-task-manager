//! Opportunities computed from how the tools were actually used: which
//! models did the work, how sessions were run, where the spend went.
//!
//! Same contract as the setup opportunities in `inventory.rs`: every one is
//! derived from this machine's numbers, states those numbers, and is absent
//! when there is nothing to say. Thresholds are constants so they are easy
//! to argue with.

use crate::i18n::Msg;
use crate::inventory::Opportunity;
use crate::spend::{area_top, AgentSpend, ProviderSpend, SessionSpend};

const CLASSROOM: &str = "https://staas.fund/classroom/";

/// The test-side key registry (see inventory::FINDING_IDS): every finding id
/// this module can emit. Only `i18n.rs`'s test module reads this, so it does
/// not exist in a release build at all.
#[cfg(test)]
pub(crate) const FINDING_IDS: &[&str] =
    &["mix-top-heavy", "areas-unsorted", "session-long-lived", "cache-read-share", "subagent-share"];

/// Below this much 30-day spend the mix is noise, not a pattern.
const MIN_SPEND_FOR_MIX: f64 = 50.0;
/// Share of spend on the largest tier at which the mix is worth a look.
const TOP_TIER_SHARE: f64 = 0.70;
/// A session this long and this costly is carrying a very large history.
const LONG_SESSION_DAYS: f64 = 7.0;
const LONG_SESSION_COST: f64 = 50.0;
/// Share of spend with no work area at which attribution is getting blurry.
const UNSORTED_SHARE: f64 = 0.25;
/// Below this many tokens in the 30-day window, a cache-read share is noise:
/// a handful of requests can swing the ratio wildly either way.
const MIN_TOKENS_FOR_CACHE_SHARE: f64 = 1_000_000.0;
/// Share of all tokens in the window that must be cache re-reads before the
/// pattern is worth a look. First guess, tuned against a real Mac's own
/// 30-day numbers running Claude Code daily: that machine's real share sat
/// close to this, comfortably above ordinary single-digit-percent re-use and
/// comfortably below what a runaway loop re-sending the same huge context
/// every turn would show.
const CACHE_READ_SHARE: f64 = 0.60;
/// Below this much subagent spend in the window, the share is noise, not a
/// pattern -- a single one-off fan-out shouldn't earn a callout.
const MIN_SUBAGENT_COST: f64 = 5.0;
/// Share of 30-day spend subagents must cross before it is worth a look.
/// First guess: subagents are a deliberate choice (fan out search/summarise
/// work to keep the main context small), so this sits well above what an
/// occasional Explore/Plan call would cost and flags only a setup that has
/// come to lean on them heavily.
const SUBAGENT_SHARE: f64 = 0.10;

/// 3 = the largest models, 2 = mid, 1 = small. None when the name gives no
/// clue: an unknown model is left out of the mix rather than guessed at.
fn tier(model: &str) -> Option<u8> {
    let m = model.to_ascii_lowercase();
    if !m.starts_with("claude") {
        return None;
    }
    if m.contains("haiku") {
        Some(1)
    } else if m.contains("sonnet") {
        Some(2)
    } else if m.contains("opus") || m.contains("fable") || m.contains("mythos") {
        Some(3)
    } else {
        None
    }
}

fn money(n: f64) -> String {
    format!("${:.0}", n)
}

/// A session counts as "in use" if its last message is this recent.
const ACTIVE_WITHIN_MS: i64 = 24 * 3_600_000;

/// Sessions worth a nudge right now: still in use, open at least `min_days`,
/// and costly enough to matter. Costliest first. `already` holds marks of the
/// form "session|<id>|<week>" and is updated, so each session nudges at most
/// once a week however often this runs.
pub fn sessions_to_nudge(
    sessions: &[SessionSpend],
    now_ms: i64,
    min_days: i64,
    week: &str,
    already: &mut Vec<String>,
) -> Vec<(String, f64, f64)> {
    if min_days <= 0 {
        return Vec::new();
    }
    // Marks from earlier weeks have done their job.
    already.retain(|m| !m.starts_with("session|") || m.ends_with(&format!("|{week}")));
    let mut due: Vec<(String, f64, f64)> = sessions
        .iter()
        .filter_map(|s| {
            let (start, end) = (s.started_ms?, s.ended_ms?);
            let days = (end - start) as f64 / 86_400_000.0;
            let in_use = now_ms - end <= ACTIVE_WITHIN_MS && now_ms >= end;
            (in_use && days >= min_days as f64 && s.cost >= LONG_SESSION_COST).then(|| (s.id.clone(), days, s.cost))
        })
        .filter(|(id, _, _)| !already.contains(&format!("session|{id}|{week}")))
        .collect();
    due.sort_by(|a, b| b.2.total_cmp(&a.2));
    for (id, _, _) in &due {
        already.push(format!("session|{id}|{week}"));
    }
    due
}

/// `agent_spend` is the same 30-day `spend::agent_spend(30)` view
/// `enriched_inventory` already computes for the agent-usage opportunity, so
/// this never triggers a second scan.
pub fn opportunities(claude: Option<&ProviderSpend>, sessions: &[SessionSpend], agent_spend: &[AgentSpend]) -> Vec<Opportunity> {
    let mut out = Vec::new();
    let mut push = |id: &str, kind: &str, title_msg: Msg, detail_msg: Msg| {
        out.push(Opportunity::from_msgs(id, kind, title_msg, Some(detail_msg), Some(CLASSROOM)));
    };

    if let Some(sp) = claude {
        // --- model mix
        let tiered: Vec<(u8, f64)> =
            sp.last30.models.iter().filter_map(|m| tier(&m.model).map(|t| (t, m.cost))).collect();
        let known: f64 = tiered.iter().map(|(_, c)| c).sum();
        let top: f64 = tiered.iter().filter(|(t, _)| *t == 3).map(|(_, c)| c).sum();
        if known >= MIN_SPEND_FOR_MIX && top / known >= TOP_TIER_SHARE {
            push(
                "mix-top-heavy",
                "learn",
                // A percentage, not a count: {pct} is a var, never Msg::count().
                Msg::new("finding.mix-top-heavy.title").var("pct", format!("{:.0}", 100.0 * top / known)),
                Msg::new("finding.mix-top-heavy.detail").var("top", money(top)).var("known", money(known)),
            );
        }

        // --- unsorted share
        let areas: Vec<(&str, f64)> = sp
            .projects
            .iter()
            .flat_map(|p| p.areas.iter().map(|a| (a.area.as_str(), a.last30.cost)))
            .collect();
        let all: f64 = areas.iter().map(|(_, c)| c).sum();
        let unsorted: f64 = areas.iter().filter(|(a, _)| area_top(a) == "(unsorted)").map(|(_, c)| c).sum();
        if all >= MIN_SPEND_FOR_MIX && unsorted / all >= UNSORTED_SHARE {
            push(
                "areas-unsorted",
                "tighten",
                Msg::new("finding.areas-unsorted.title").var("pct", format!("{:.0}", 100.0 * unsorted / all)),
                Msg::new("finding.areas-unsorted.detail").var("unsorted", money(unsorted)),
            );
        }

        // --- cache-read share: how much of the window is a re-read, not
        // fresh input. The scanner already parses this per line
        // (`ClaudeTokens.cache_read` in spend.rs); the window total comes
        // from the same persisted per-day accumulator `last30.tokens` does.
        let total_tokens = sp.last30.tokens;
        if total_tokens >= MIN_TOKENS_FOR_CACHE_SHARE {
            let share = sp.last30.cache_read / total_tokens;
            if share >= CACHE_READ_SHARE {
                push(
                    "cache-read-share",
                    "learn",
                    Msg::new("finding.cache-read-share.title").var("percent", format!("{:.0}", 100.0 * share)),
                    Msg::new("finding.cache-read-share.detail"),
                );
            }
        }

        // --- subagent share of spend
        let subagent_total: f64 = agent_spend.iter().map(|a| a.cost).sum();
        if sp.last30.cost > 0.0 && subagent_total >= MIN_SUBAGENT_COST && subagent_total / sp.last30.cost >= SUBAGENT_SHARE {
            // agent_spend sorts named agents first by cost, the empty
            // "unattributed" name always last regardless of its own cost
            // (see agent_spend_from's own sort) -- the first named row is
            // the highest-cost one actually worth pointing at.
            if let Some(top) = agent_spend.iter().find(|a| !a.name.is_empty()) {
                push(
                    "subagent-share",
                    "learn",
                    Msg::new("finding.subagent-share.title")
                        .var("cost", money(subagent_total))
                        .var("percent", format!("{:.0}", 100.0 * subagent_total / sp.last30.cost)),
                    Msg::new("finding.subagent-share.detail").var("agent", top.name.clone()),
                );
            }
        }
    }

    // --- long-lived sessions: the costliest one speaks for the pattern
    let long: Vec<(&SessionSpend, f64)> = sessions
        .iter()
        .filter_map(|s| {
            let days = (s.ended_ms? - s.started_ms?) as f64 / 86_400_000.0;
            (days >= LONG_SESSION_DAYS && s.cost >= LONG_SESSION_COST).then_some((s, days))
        })
        .collect();
    if let Some((worst, days)) = long.iter().max_by(|a, b| a.0.cost.total_cmp(&b.0.cost)) {
        let others = long.len() - 1;
        // `others` picks which of two whole sentences this is, not a
        // trailing clause bolted onto one: a base key is either a bare
        // value or a set of plural forms, never both, so "no other session"
        // and "N other sessions too" are two separate keys rather than one
        // key that sometimes has a count and sometimes does not. The count
        // itself is meaningless to the no-others sentence, so it is only
        // ever set on the Msg that names the other-sessions key.
        let detail_msg = if others > 0 {
            Msg::new("finding.session-long-lived.detailOthers").var("cost", money(worst.cost)).count(others as i64)
        } else {
            Msg::new("finding.session-long-lived.detail").var("cost", money(worst.cost))
        };
        push(
            "session-long-lived",
            "learn",
            Msg::new("finding.session-long-lived.title").var("days", format!("{:.0}", days)),
            detail_msg,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spend::{AgentSpend, AreaSpend, ModelSpend, ProjectSpend, Window};

    fn w(cost: f64) -> Window {
        Window { cost, tokens: cost * 1000.0, cache_read: 0.0, models: Vec::new() }
    }

    /// A last30 window with tokens/cache-read chosen independently of cost --
    /// `w()`/`claude()` tie tokens to cost at a fixed ratio, which can't
    /// express "big window, high cache-read share" on its own.
    fn with_cache_read(mut sp: ProviderSpend, tokens: f64, cache_read: f64) -> ProviderSpend {
        sp.last30.tokens = tokens;
        sp.last30.cache_read = cache_read;
        sp
    }

    fn claude(models: &[(&str, f64)], areas: &[(&str, f64)]) -> ProviderSpend {
        let total: f64 = models.iter().map(|(_, c)| c).sum();
        let mut last30 = w(total);
        last30.models =
            models.iter().map(|(m, c)| ModelSpend { model: m.to_string(), cost: *c, tokens: c * 1000.0 }).collect();
        ProviderSpend {
            week: None,
            id: "claude".into(),
            name: "Claude".into(),
            today: w(0.0),
            yesterday: w(0.0),
            last30,
            trend: vec![],
            unpriced: 0,
            unpriced_models: vec![],
            daily_cost: vec![],
            projects: vec![ProjectSpend {
                project: "/w".into(),
                today: w(0.0),
                yesterday: w(0.0),
                last30: w(total),
                areas: areas
                    .iter()
                    .map(|(a, c)| AreaSpend {
                        week: None,
                        area: a.to_string(),
                        today: w(0.0),
                        yesterday: w(0.0),
                        last30: w(*c),
                        daily_cost: vec![],
                    })
                    .collect(),
            }],
        }
    }

    fn session(days: f64, cost: f64) -> SessionSpend {
        SessionSpend {
            id: format!("s-{days}-{cost}"),
            project: "/w".into(),
            started_ms: Some(0),
            ended_ms: Some((days * 86_400_000.0) as i64),
            cost,
            tokens: 0.0,
            bytes: 0,
            top_model: None,
            areas: vec![],
            day_cost: None,
            subagent_cost: 0.0,
            subagent_runs: 0,
        }
    }

    fn ids(found: &[Opportunity]) -> Vec<&str> {
        found.iter().map(|o| o.id.as_str()).collect()
    }

    fn agent(name: &str, cost: f64) -> AgentSpend {
        AgentSpend { name: name.into(), runs: 1, cost, tokens: 0, last_used_ms: 0, top_model: None, by_client: vec![] }
    }

    #[test]
    fn a_balanced_well_sorted_setup_earns_nothing() {
        let sp = claude(&[("claude-opus-5", 60.0), ("claude-sonnet-5", 40.0)], &[("acme", 95.0), ("(unsorted)", 5.0)]);
        assert!(opportunities(Some(&sp), &[session(0.2, 30.0), session(10.0, 5.0)], &[]).is_empty());
        assert!(opportunities(None, &[], &[]).is_empty());
    }

    #[test]
    fn a_top_heavy_mix_is_stated_with_its_own_numbers() {
        let sp = claude(
            &[("claude-fable-5-1", 800.0), ("claude-opus-5", 100.0), ("claude-sonnet-5", 100.0), ("mystery", 500.0)],
            &[("acme", 1500.0)],
        );
        let found = opportunities(Some(&sp), &[], &[]);
        assert_eq!(ids(&found), ["mix-top-heavy"]);
        assert_eq!(found[0].title, "90% of spend is on the largest models", "the unknown model is left out, not guessed");
        assert!(found[0].detail.contains("$900 of $1000"), "{}", found[0].detail);
    }

    #[test]
    fn small_spend_is_not_a_pattern() {
        let sp = claude(&[("claude-opus-5", 20.0)], &[("(unsorted)", 20.0)]);
        assert!(opportunities(Some(&sp), &[], &[]).is_empty());
    }

    #[test]
    fn unsorted_counts_at_either_depth_and_is_a_gap() {
        let sp = claude(&[("claude-sonnet-5", 100.0)], &[("(unsorted)", 30.0), ("acme/web", 70.0)]);
        let found = opportunities(Some(&sp), &[], &[]);
        assert_eq!(ids(&found), ["areas-unsorted"]);
        assert_eq!(found[0].kind, "tighten");
        assert!(found[0].title.starts_with("30%"));
    }

    #[test]
    fn the_costliest_long_session_speaks_for_the_rest() {
        let found =
            opportunities(None, &[session(56.0, 419.0), session(9.0, 60.0), session(30.0, 10.0), session(1.0, 900.0)], &[]);
        assert_eq!(ids(&found), ["session-long-lived"]);
        assert_eq!(found[0].title, "One session has been open 56 days");
        assert!(found[0].detail.contains("$419"));
        assert!(found[0].detail.contains("one other session is past a week too"), "{}", found[0].detail);
    }

    #[test]
    fn cache_share_finding_needs_a_real_window() {
        let base = claude(&[("claude-sonnet-5", 100.0)], &[("acme", 100.0)]);

        // High share, but the window itself is thin: a handful of requests
        // could swing that ratio either way.
        let thin = with_cache_read(base.clone(), 900_000.0, 800_000.0);
        assert!(opportunities(Some(&thin), &[], &[]).iter().all(|o| o.id != "cache-read-share"), "under the token floor");

        // Big enough window, but the share itself is unremarkable.
        let low_share = with_cache_read(base.clone(), 2_000_000.0, 400_000.0);
        assert!(
            opportunities(Some(&low_share), &[], &[]).iter().all(|o| o.id != "cache-read-share"),
            "under the share threshold"
        );

        // Both conditions cross: fires with its own percentage.
        let over = with_cache_read(base, 2_000_000.0, 1_400_000.0);
        let found = opportunities(Some(&over), &[], &[]);
        let o = found.iter().find(|o| o.id == "cache-read-share").expect("cache-read-share should fire");
        assert_eq!(o.kind, "learn");
        assert_eq!(o.title, "70% of your tokens were context re-reads");
    }

    #[test]
    fn subagent_share_finding_names_the_top_agent() {
        // $4 of a $30 window is 13%: over the share, under the $5 floor.
        let small = claude(&[("claude-sonnet-5", 30.0)], &[("acme", 30.0)]);
        assert!(
            opportunities(Some(&small), &[], &[agent("Explore", 4.0)]).iter().all(|o| o.id != "subagent-share"),
            "under the dollar floor"
        );

        // $8 of a $100 window is 8%: over the $5 floor, under the share.
        let sp = claude(&[("claude-sonnet-5", 100.0)], &[("acme", 100.0)]);
        assert!(
            opportunities(Some(&sp), &[], &[agent("Explore", 8.0)]).iter().all(|o| o.id != "subagent-share"),
            "under the share threshold"
        );

        // $12 of the same $100 window is 12%: over both, names the top agent.
        let over = [agent("Explore", 9.0), agent("Plan", 3.0)];
        let found = opportunities(Some(&sp), &[], &over);
        let o = found.iter().find(|o| o.id == "subagent-share").expect("subagent-share should fire");
        assert_eq!(o.kind, "learn");
        assert_eq!(o.title, "Subagents cost $12, 12% of the last 30 days");
        assert!(o.detail.contains("Explore"), "{}", o.detail);
    }

    /// Prints what this machine's real usage earns. `--ignored --nocapture`
    #[test]
    #[ignore]
    fn live_coaching() {
        let spend = crate::spend::collect(None);
        let claude = spend.iter().find(|p| p.id == "claude");
        let agent_spend = crate::spend::agent_spend(30);
        for o in opportunities(claude, &crate::spend::claude_sessions(None, None, 500), &agent_spend) {
            println!("[{}] {}\n    {}", o.kind, o.title, o.detail);
        }
    }

    #[test]
    fn a_nudge_is_for_old_costly_sessions_still_in_use_once_a_week() {
        let day = 86_400_000;
        let now = 100 * day;
        let at = |id: &str, age_days: i64, idle_days: i64, cost: f64| SessionSpend {
            id: id.into(),
            started_ms: Some(now - age_days * day),
            ended_ms: Some(now - idle_days * day),
            ..session(0.0, cost)
        };
        let sessions = [
            at("old-active", 20, 0, 300.0),
            at("old-abandoned", 40, 9, 900.0), // not in use: nothing to change
            at("old-cheap", 30, 0, 5.0),
            at("young", 2, 0, 400.0),
            at("older-active", 50, 0, 120.0),
        ];
        let mut marks = vec!["session|old-active|2026-W38".to_string(), "budget|x".to_string()];
        let due = sessions_to_nudge(&sessions, now, 7, "2026-W39", &mut marks);
        assert_eq!(due.iter().map(|d| d.0.as_str()).collect::<Vec<_>>(), ["old-active", "older-active"], "costliest first");
        assert!((due[0].1 - 20.0).abs() < 1e-9);
        assert!(marks.contains(&"budget|x".to_string()), "other marks are left alone");
        assert!(!marks.iter().any(|m| m.ends_with("W38")), "last week's mark is cleared");
        assert!(sessions_to_nudge(&sessions, now, 7, "2026-W39", &mut marks).is_empty(), "once a week");
        assert_eq!(sessions_to_nudge(&sessions, now, 7, "2026-W40", &mut marks).len(), 2, "and again next week");
        assert!(sessions_to_nudge(&sessions, now, 0, "2026-W41", &mut Vec::new()).is_empty(), "0 is off");
        assert_eq!(sessions_to_nudge(&sessions, now, 30, "2026-W41", &mut Vec::new()).len(), 1, "a longer threshold");
    }

    /// A missing translation key renders as its own literal key text instead
    /// of failing -- that takes a real fixture run to catch. Exercises every
    /// finding id, including session-long-lived's "others" clause at
    /// count=1 (the .one plural form) and count=2 (.other).
    #[test]
    fn opportunities_never_render_a_raw_key() {
        let mix = claude(
            &[("claude-fable-5-1", 800.0), ("claude-opus-5", 100.0), ("claude-sonnet-5", 100.0)],
            &[("acme", 1000.0)],
        );
        let unsorted = claude(&[("claude-sonnet-5", 100.0)], &[("(unsorted)", 30.0), ("acme/web", 70.0)]);
        let one_other = [session(56.0, 419.0), session(9.0, 60.0)];
        let two_others = [session(56.0, 419.0), session(9.0, 60.0), session(30.0, 500.0)];
        let cache_heavy = with_cache_read(claude(&[("claude-sonnet-5", 100.0)], &[("acme", 100.0)]), 2_000_000.0, 1_400_000.0);
        let subagent_heavy = claude(&[("claude-sonnet-5", 100.0)], &[("acme", 100.0)]);
        let fixtures: [Vec<Opportunity>; 6] = [
            opportunities(Some(&mix), &[], &[]),
            opportunities(Some(&unsorted), &[], &[]),
            opportunities(None, &one_other, &[]),
            opportunities(None, &two_others, &[]),
            opportunities(Some(&cache_heavy), &[], &[]),
            opportunities(Some(&subagent_heavy), &[], &[agent("Explore", 9.0), agent("Plan", 3.0)]),
        ];
        for found in fixtures {
            assert!(!found.is_empty());
            for o in found {
                assert!(!o.title.starts_with("finding."), "{}: raw key in title: {}", o.id, o.title);
                assert!(!o.detail.starts_with("finding."), "{}: raw key in detail: {}", o.id, o.detail);
            }
        }
    }

    #[test]
    fn tiers_come_from_the_name_or_not_at_all() {
        assert_eq!(tier("claude-haiku-4-5-20251001"), Some(1));
        assert_eq!(tier("claude-sonnet-5"), Some(2));
        assert_eq!(tier("claude-opus-5"), Some(3));
        assert_eq!(tier("claude-fable-5-1"), Some(3));
        assert_eq!(tier("claude-next"), None);
        assert_eq!(tier("gpt-5"), None);
    }
}
