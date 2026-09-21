//! Opportunities computed from how the tools were actually used: which
//! models did the work, how sessions were run, where the spend went.
//!
//! Same contract as the setup opportunities in `inventory.rs`: every one is
//! derived from this machine's numbers, states those numbers, and is absent
//! when there is nothing to say. Thresholds are constants so they are easy
//! to argue with.

use crate::inventory::Opportunity;
use crate::spend::{area_top, ProviderSpend, SessionSpend};

const CLASSROOM: &str = "https://staas.fund/classroom/";

/// Below this much 30-day spend the mix is noise, not a pattern.
const MIN_SPEND_FOR_MIX: f64 = 50.0;
/// Share of spend on the largest tier at which the mix is worth a look.
const TOP_TIER_SHARE: f64 = 0.70;
/// A session this long and this costly is carrying a very large history.
const LONG_SESSION_DAYS: f64 = 7.0;
const LONG_SESSION_COST: f64 = 50.0;
/// Share of spend with no work area at which attribution is getting blurry.
const UNSORTED_SHARE: f64 = 0.25;

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

pub fn opportunities(claude: Option<&ProviderSpend>, sessions: &[SessionSpend]) -> Vec<Opportunity> {
    let mut out = Vec::new();
    let mut push = |id: &str, kind: &str, title: String, detail: String| {
        out.push(Opportunity {
            id: id.into(),
            kind: kind.into(),
            title,
            detail,
            learn_url: Some(CLASSROOM.into()),
        });
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
                format!("{:.0}% of spend is on the largest models", 100.0 * top / known),
                format!(
                    "{} of {} in 30 days went to the top tier. Searching files, reading logs and summarising do as well a tier down; giving those jobs to subagents on a lighter model is the usual way to cut this without touching the hard work.",
                    money(top),
                    money(known)
                ),
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
                format!("{:.0}% of spend has no work area", 100.0 * unsorted / all),
                format!(
                    "{} in 30 days came from sessions that started at the top of a workspace and never moved. Starting Claude Code inside the folder you are working on makes the split by area, and by client, exact.",
                    money(unsorted)
                ),
            );
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
        push(
            "session-long-lived",
            "learn",
            format!("One session has been open {:.0} days", days),
            format!(
                "It cost {} in the last 30 days{}. A session re-sends its growing history on every turn, so an old one pays more for each answer and gets slower to steer. Starting fresh for new work, and keeping what matters in project memory, is cheaper.",
                money(worst.cost),
                match others {
                    0 => String::new(),
                    1 => ", and one other session is past a week too".to_string(),
                    n => format!(", and {n} other sessions are past a week too"),
                }
            ),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spend::{AreaSpend, ModelSpend, ProjectSpend, Window};

    fn w(cost: f64) -> Window {
        Window { cost, tokens: cost * 1000.0, models: Vec::new() }
    }

    fn claude(models: &[(&str, f64)], areas: &[(&str, f64)]) -> ProviderSpend {
        let total: f64 = models.iter().map(|(_, c)| c).sum();
        let mut last30 = w(total);
        last30.models =
            models.iter().map(|(m, c)| ModelSpend { model: m.to_string(), cost: *c, tokens: c * 1000.0 }).collect();
        ProviderSpend {
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
            top_model: None,
            areas: vec![],
            day_cost: None,
        }
    }

    fn ids(found: &[Opportunity]) -> Vec<&str> {
        found.iter().map(|o| o.id.as_str()).collect()
    }

    #[test]
    fn a_balanced_well_sorted_setup_earns_nothing() {
        let sp = claude(&[("claude-opus-5", 60.0), ("claude-sonnet-5", 40.0)], &[("acme", 95.0), ("(unsorted)", 5.0)]);
        assert!(opportunities(Some(&sp), &[session(0.2, 30.0), session(10.0, 5.0)]).is_empty());
        assert!(opportunities(None, &[]).is_empty());
    }

    #[test]
    fn a_top_heavy_mix_is_stated_with_its_own_numbers() {
        let sp = claude(
            &[("claude-fable-5-1", 800.0), ("claude-opus-5", 100.0), ("claude-sonnet-5", 100.0), ("mystery", 500.0)],
            &[("acme", 1500.0)],
        );
        let found = opportunities(Some(&sp), &[]);
        assert_eq!(ids(&found), ["mix-top-heavy"]);
        assert_eq!(found[0].title, "90% of spend is on the largest models", "the unknown model is left out, not guessed");
        assert!(found[0].detail.contains("$900 of $1000"), "{}", found[0].detail);
    }

    #[test]
    fn small_spend_is_not_a_pattern() {
        let sp = claude(&[("claude-opus-5", 20.0)], &[("(unsorted)", 20.0)]);
        assert!(opportunities(Some(&sp), &[]).is_empty());
    }

    #[test]
    fn unsorted_counts_at_either_depth_and_is_a_gap() {
        let sp = claude(&[("claude-sonnet-5", 100.0)], &[("(unsorted)", 30.0), ("acme/web", 70.0)]);
        let found = opportunities(Some(&sp), &[]);
        assert_eq!(ids(&found), ["areas-unsorted"]);
        assert_eq!(found[0].kind, "tighten");
        assert!(found[0].title.starts_with("30%"));
    }

    #[test]
    fn the_costliest_long_session_speaks_for_the_rest() {
        let found = opportunities(None, &[session(56.0, 419.0), session(9.0, 60.0), session(30.0, 10.0), session(1.0, 900.0)]);
        assert_eq!(ids(&found), ["session-long-lived"]);
        assert_eq!(found[0].title, "One session has been open 56 days");
        assert!(found[0].detail.contains("$419"));
        assert!(found[0].detail.contains("one other session is past a week too"), "{}", found[0].detail);
    }

    /// Prints what this machine's real usage earns. `--ignored --nocapture`
    #[test]
    #[ignore]
    fn live_coaching() {
        let spend = crate::spend::collect(None);
        let claude = spend.iter().find(|p| p.id == "claude");
        for o in opportunities(claude, &crate::spend::claude_sessions(None, None, 500)) {
            println!("[{}] {}\n    {}", o.kind, o.title, o.detail);
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
