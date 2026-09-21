//! `aitm-agent`: the headless half. It builds this machine's seat report
//! (see `aitm_core::seat` for exactly what that holds, and what it never
//! holds) and either prints it or sends it to a team's own collector.
//!
//!   aitm-agent report [--label "Dana's MacBook"]
//!   aitm-agent push --to https://collector.example.com [--label …]
//!
//! The collector token comes from the AITM_COLLECTOR_TOKEN environment
//! variable, never from an argument: arguments show up in process lists.
//! There is no daemon and no schedule in here on purpose. Run it from cron,
//! launchd or Task Scheduler, so IT decides when a seat reports.

use aitm_core::{coaching, inventory, providers, rt, seat, spend};

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

/// Plain http is allowed only to this machine: anywhere else the token and
/// the report would cross the network in the clear.
fn transport_ok(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    url.strip_prefix("http://").is_some_and(|rest| {
        let host = rest.split(['/', ':']).next().unwrap_or("");
        matches!(host, "127.0.0.1" | "localhost")
    })
}

fn build_report(label: Option<String>) -> Result<seat::SeatReport, String> {
    let mut inv = inventory::scan();
    let spend = spend::collect(None);
    let claude = spend.iter().find(|p| p.id == "claude");
    inv.opportunities.extend(coaching::opportunities(claude, &spend::claude_sessions(None, None, 500)));
    let seat_id = seat::seat_id_in(&providers::config_dir())?;
    let label = label
        .or_else(|| std::env::var("AITM_SEAT_LABEL").ok())
        .filter(|l| !l.trim().is_empty())
        .unwrap_or_else(|| "Unnamed seat".to_string());
    Ok(seat::build(&seat_id, label.trim(), chrono::Utc::now().timestamp_millis(), &inv, &spend))
}

fn run(args: &[String]) -> Result<String, String> {
    let label = arg(args, "--label");
    match args.first().map(String::as_str) {
        Some("report") => {
            serde_json::to_string_pretty(&build_report(label)?).map_err(|e| e.to_string())
        }
        Some("push") => {
            let to = arg(args, "--to").ok_or("push needs --to <collector url>")?;
            if !transport_ok(&to) {
                return Err("refusing to send over plain http to another machine; use https".into());
            }
            let token = std::env::var("AITM_COLLECTOR_TOKEN")
                .ok()
                .filter(|t| !t.is_empty())
                .ok_or("set AITM_COLLECTOR_TOKEN to the collector's token")?;
            let report = build_report(label)?;
            let url = format!("{}/v1/report", to.trim_end_matches('/'));
            rt::block_on(async {
                let resp = providers::http()
                    .post(&url)
                    .bearer_auth(&token)
                    .json(&report)
                    .send()
                    .await
                    .map_err(|e| format!("send report: {e}"))?;
                if resp.status().is_success() {
                    Ok(format!("reported as {} ({} servers, {} tools)", report.label, report.servers.len(), report.tools.len()))
                } else {
                    Err(format!("collector answered HTTP {}", resp.status()))
                }
            })
        }
        _ => Err("usage: aitm-agent report [--label NAME] | push --to URL [--label NAME]".into()),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(out) => println!("{out}"),
        Err(e) => {
            eprintln!("aitm-agent: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_never_travels_in_the_clear_to_another_machine() {
        assert!(transport_ok("https://collector.example.com"));
        assert!(transport_ok("http://127.0.0.1:8787"));
        assert!(transport_ok("http://localhost:8787/base"));
        assert!(!transport_ok("http://collector.example.com"));
        assert!(!transport_ok("http://127.0.0.1.evil.example"));
        assert!(!transport_ok("ftp://x"));
    }

    #[test]
    fn flags_are_read_by_name_and_a_missing_value_is_none() {
        let args: Vec<String> = ["push", "--to", "https://c", "--label"].iter().map(|s| s.to_string()).collect();
        assert_eq!(arg(&args, "--to").as_deref(), Some("https://c"));
        assert_eq!(arg(&args, "--label"), None);
        assert!(run(&["push".to_string()]).unwrap_err().contains("--to"));
        assert!(run(&[]).unwrap_err().starts_with("usage"));
    }
}
