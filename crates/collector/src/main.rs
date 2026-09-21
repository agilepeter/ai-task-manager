//! `aitm-collector`: receives seat reports and shows a team dashboard.
//!
//!   AITM_COLLECTOR_TOKEN=… aitm-collector [--bind 127.0.0.1:8787] [--data ./aitm-data]
//!
//! A `policy.json` in the data folder (see `aitm_core::policy`) turns the
//! dashboard into a conformance view. It is read on each request, so editing
//! the file is all it takes; seats never receive it.
//!
//! Self-hosted and small on purpose: one process, one folder of JSON files
//! (the latest report per seat), no database, no accounts. Every request
//! needs the token, the dashboard included. It binds to this machine unless
//! told otherwise; put it behind the organisation's own TLS and access
//! controls before binding it wider. What a report can contain is fixed by
//! `aitm_core::seat`: no prompts, paths, folder or client names, credentials.

use aitm_core::policy::{self, Policy};
use aitm_core::seat::{self, SeatReport};
use std::path::{Path, PathBuf};

const MAX_BODY: usize = 512 * 1024;
const MAX_SEATS: usize = 5_000;
/// A seat that has not reported for this long is shown as stale.
const STALE_MS: i64 = 8 * 24 * 3_600_000;

pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
}

fn reply(status: u16, content_type: &'static str, body: impl Into<String>) -> Reply {
    Reply { status, content_type, body: body.into() }
}

/// Compares every byte whatever the input, so timing says nothing about how
/// much of a guessed token was right.
fn token_matches(given: &str, expected: &str) -> bool {
    let (a, b) = (given.as_bytes(), expected.as_bytes());
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= (a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0)) as usize;
    }
    diff == 0
}

fn authorised(header: Option<&str>, query: &str, token: &str) -> bool {
    let bearer = header.and_then(|h| h.strip_prefix("Bearer ")).map(str::trim);
    // The dashboard is opened in a browser, which cannot set a header.
    let from_query = query.split('&').find_map(|kv| kv.strip_prefix("token="));
    bearer.or(from_query).is_some_and(|given| token_matches(given, token))
}

fn load_all(dir: &Path) -> Vec<SeatReport> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<SeatReport> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|raw| seat::parse(&raw).ok())
        .collect();
    out.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    out
}

fn store(dir: &Path, report: &SeatReport) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create data folder: {e}"))?;
    // seat::parse has already limited the id to [A-Za-z0-9-], so it is safe
    // as a file name. A new seat past the cap is refused, an update is not.
    let path = dir.join(format!("{}.json", report.seat_id));
    if !path.exists() && load_all(dir).len() >= MAX_SEATS {
        return Err("seat limit reached".into());
    }
    let tmp = dir.join(format!("{}.tmp", report.seat_id));
    let body = serde_json::to_string(report).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| format!("write report: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("store report: {e}"))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// 2068.4 → "$2,068"
fn dollars(n: f64) -> String {
    let digits = format!("{:.0}", n.max(0.0));
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("${out}")
}

fn top_tier_share(report: &SeatReport) -> Option<f64> {
    let models = report.spend.iter().flat_map(|s| s.top_models.iter());
    let (mut top, mut all) = (0.0, 0.0);
    for (model, cost) in models {
        let m = model.to_ascii_lowercase();
        all += cost;
        if m.contains("opus") || m.contains("fable") || m.contains("mythos") {
            top += cost;
        }
    }
    (all > 0.0).then(|| top / all)
}

fn load_policy(dir: &Path) -> Option<Policy> {
    policy::parse(&std::fs::read_to_string(dir.join("policy.json")).ok()?)
}

/// The limit closest to its ceiling: what decides whether a seat is squeezed.
fn tightest(report: &SeatReport) -> Option<String> {
    report
        .limits
        .iter()
        .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent))
        .map(|l| format!("{} {} {:.0}%", l.provider, l.metric, l.used_percent))
}

pub fn dashboard(reports: &[SeatReport], now: i64) -> String {
    dashboard_with(reports, None, now)
}

pub fn dashboard_with(reports: &[SeatReport], policy: Option<&Policy>, now: i64) -> String {
    let spend: f64 = reports.iter().flat_map(|r| r.spend.iter()).map(|s| s.last30).sum();
    let servers: usize = reports.iter().map(|r| r.servers.len()).sum();
    let unpinned: usize =
        reports.iter().flat_map(|r| r.servers.iter()).filter(|s| s.pinned == Some(false)).count();
    let unguarded = reports.iter().filter(|r| r.allow_rules + r.ask_rules + r.deny_rules == 0).count();
    let out_of_policy = policy.map(|p| reports.iter().filter(|r| !policy::check(r, p).is_empty()).count());

    // Which servers are in use across the team, most common first. Counted
    // in seats: one machine running a server in two apps is still one seat.
    let mut fleet: std::collections::BTreeMap<String, (usize, bool)> = Default::default();
    for r in reports {
        let mut on_this_seat = std::collections::BTreeMap::new();
        for s in &r.servers {
            let key = s.package.clone().unwrap_or_else(|| format!("{} ({})", s.name, s.target));
            *on_this_seat.entry(key).or_insert(false) |= s.pinned == Some(false);
        }
        for (key, loose) in on_this_seat {
            let entry = fleet.entry(key).or_insert((0, false));
            entry.0 += 1;
            entry.1 |= loose;
        }
    }
    let mut fleet: Vec<_> = fleet.into_iter().collect();
    fleet.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then_with(|| a.0.cmp(&b.0)));

    let seat_rows: String = reports
        .iter()
        .map(|r| {
            let age_h = (now - r.generated_at).max(0) / 3_600_000;
            let seen = if age_h < 1 { "under an hour ago".to_string() } else if age_h < 48 { format!("{age_h} h ago") } else { format!("{} days ago", age_h / 24) };
            let stale = now - r.generated_at > STALE_MS;
            let spend: f64 = r.spend.iter().map(|s| s.last30).sum();
            let loose = r.servers.iter().filter(|s| s.pinned == Some(false)).count();
            let rules = r.allow_rules + r.ask_rules + r.deny_rules;
            let verdict = match policy.map(|p| policy::check(r, p)) {
                None => "–".to_string(),
                Some(v) if v.is_empty() => "Conforms".to_string(),
                Some(v) => format!(
                    "<b class=warn>{} issue{}</b><ul>{}</ul>",
                    v.len(),
                    if v.len() == 1 { "" } else { "s" },
                    v.iter().take(12).map(|x| format!("<li>{}</li>", esc(&x.detail))).collect::<String>()
                ),
            };
            format!(
                "<tr{}><td>{}</td><td>{}</td><td>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                if stale { " class=stale" } else { "" },
                esc(&r.label),
                esc(&seen),
                esc(&r.tools.join(", ")),
                r.servers.len(),
                if loose > 0 { format!("<b class=warn>{loose}</b>") } else { "0".into() },
                if rules == 0 { "<b class=warn>none</b>".to_string() } else { rules.to_string() },
                dollars(spend),
                top_tier_share(r).map_or("–".to_string(), |s| format!("{:.0}%", s * 100.0)),
                esc(&tightest(r).unwrap_or_else(|| "–".to_string())),
                verdict,
                esc(&r.findings.iter().map(|f| f.title.as_str()).collect::<Vec<_>>().join(" · ")),
            )
        })
        .collect();
    let fleet_rows: String = fleet
        .iter()
        .take(60)
        .map(|(name, (count, loose))| {
            format!(
                "<tr><td>{}</td><td class=n>{count}</td><td>{}</td></tr>",
                esc(name),
                if *loose { "<b class=warn>unpinned on at least one seat</b>" } else { "" }
            )
        })
        .collect();

    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="robots" content="noindex"><title>AI Task Manager · Team</title>
<style>
:root{{color-scheme:light dark;--bg:#fff;--fg:#0a0a0b;--mut:#6b6b76;--line:#e4e4e9;--card:#f7f7f9;--warn:#b45309}}
@media (prefers-color-scheme:dark){{:root{{--bg:#09090b;--fg:#fafafa;--mut:#a1a1aa;--line:#27272a;--card:#18181b;--warn:#eab308}}}}
body{{margin:0;padding:24px 16px 48px;background:var(--bg);color:var(--fg);font:14px/1.45 system-ui,-apple-system,Segoe UI,sans-serif}}
main{{max-width:1100px;margin:0 auto}} h1{{font-size:20px;margin:0 0 4px}} h2{{font-size:14px;margin:28px 0 8px}}
p.sub{{margin:0 0 20px;color:var(--mut)}}
.tiles{{display:grid;grid-template-columns:repeat(auto-fit,minmax(150px,1fr));gap:10px}}
.tile{{background:var(--card);border-radius:12px;padding:12px 14px}} .tile b{{display:block;font-size:22px;letter-spacing:-.02em}} .tile span{{color:var(--mut);font-size:12px}}
.scroll{{overflow-x:auto}} table{{border-collapse:collapse;width:100%;min-width:720px}}
th,td{{text-align:left;padding:8px 10px;border-top:1px solid var(--line);vertical-align:top}} th{{color:var(--mut);font-weight:500;font-size:12px;border-top:none}}
td:nth-child(-n+2){{white-space:nowrap}} td ul{{margin:4px 0 0;padding-left:16px;color:var(--mut);font-size:12px}} td.n,th.n{{text-align:right;font-variant-numeric:tabular-nums;white-space:nowrap}} .warn{{color:var(--warn)}} tr.stale td{{color:var(--mut)}}
footer{{margin-top:28px;color:var(--mut);font-size:12px}}
</style></head><body><main>
<h1>AI tooling across the team</h1>
<p class="sub">{} seat{} reporting. Inventory, guardrails and usage value only: no prompts, file or folder names, or credentials are ever collected.</p>
<div class="tiles">
<div class="tile"><b>{}</b><span>seats</span></div>
<div class="tile"><b>{}</b><span>MCP servers in use</span></div>
<div class="tile"><b>{}</b><span>of them unpinned</span></div>
<div class="tile"><b>{}</b><span>seats with no permission rules</span></div>
<div class="tile"><b>{}</b><span>{}</span></div>
<div class="tile"><b>{}</b><span>API-equivalent usage, 30 days</span></div>
</div>
<h2>Seats</h2><div class="scroll"><table><thead><tr><th>Seat</th><th>Last report</th><th>AI tools</th><th class=n>MCP servers</th><th class=n>Unpinned</th><th class=n>Permission rules</th><th class=n>30-day usage</th><th class=n>Largest models</th><th>Tightest limit</th><th>Policy</th><th>Findings</th></tr></thead><tbody>{}</tbody></table></div>
<h2>MCP servers across the team</h2><div class="scroll"><table><thead><tr><th>Server</th><th class=n>Seats</th><th></th></tr></thead><tbody>{}</tbody></table></div>
<footer>Usage is priced at API rates from each seat's local logs: on flat-rate plans it is equivalent value, not a charge. A greyed seat has not reported in over a week.</footer>
</main></body></html>"#,
        reports.len(),
        if reports.len() == 1 { "" } else { "s" },
        reports.len(),
        servers,
        unpinned,
        unguarded,
        out_of_policy.map_or("–".to_string(), |n| n.to_string()),
        if policy.is_some() { "seats out of policy" } else { "no policy.json set" },
        dollars(spend),
        if seat_rows.is_empty() { "<tr><td colspan=11>No seat has reported yet.</td></tr>".to_string() } else { seat_rows },
        if fleet_rows.is_empty() { "<tr><td colspan=3>None yet.</td></tr>".to_string() } else { fleet_rows },
    )
}

/// One request in, one reply out. No sockets here, so every branch is testable.
pub fn handle(method: &str, url: &str, auth: Option<&str>, body: &str, dir: &Path, token: &str, now: i64) -> Reply {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    if path == "/healthz" {
        return reply(200, "text/plain", "ok");
    }
    if !authorised(auth, query, token) {
        return reply(401, "text/plain", "a valid token is required");
    }
    match (method, path) {
        ("POST", "/v1/report") => {
            if body.len() > MAX_BODY {
                return reply(413, "text/plain", "report too large");
            }
            match seat::parse(body).and_then(|r| store(dir, &r).map(|_| r)) {
                Ok(r) => reply(200, "application/json", format!("{{\"stored\":\"{}\"}}", r.seat_id)),
                Err(e) => reply(400, "text/plain", e),
            }
        }
        ("GET", "/v1/seats") => match serde_json::to_string(&load_all(dir)) {
            Ok(json) => reply(200, "application/json", json),
            Err(e) => reply(500, "text/plain", e.to_string()),
        },
        ("GET", "/") => {
            reply(200, "text/html; charset=utf-8", dashboard_with(&load_all(dir), load_policy(dir).as_ref(), now))
        }
        // Conformance as data, for a team's own tooling.
        ("GET", "/v1/conformance") => {
            let Some(p) = load_policy(dir) else { return reply(404, "text/plain", "no policy.json in the data folder") };
            let rows: Vec<serde_json::Value> = load_all(dir)
                .iter()
                .map(|r| serde_json::json!({"seatId": r.seat_id, "label": r.label, "violations": policy::check(r, &p)}))
                .collect();
            reply(200, "application/json", serde_json::Value::Array(rows).to_string())
        }
        _ => reply(404, "text/plain", "not found"),
    }
}

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let token = match std::env::var("AITM_COLLECTOR_TOKEN") {
        Ok(t) if t.len() >= 16 => t,
        _ => {
            eprintln!("aitm-collector: set AITM_COLLECTOR_TOKEN to a secret of at least 16 characters");
            std::process::exit(1);
        }
    };
    let bind = arg(&args, "--bind").unwrap_or_else(|| "127.0.0.1:8787".to_string());
    let dir = PathBuf::from(arg(&args, "--data").unwrap_or_else(|| "aitm-data".to_string()));
    let server = match tiny_http::Server::http(&bind) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("aitm-collector: cannot listen on {bind}: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("aitm-collector: listening on http://{bind}, storing in {}", dir.display());
    for mut request in server.incoming_requests() {
        let auth = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Authorization"))
            .map(|h| h.value.as_str().to_string());
        let mut body = String::new();
        let too_big = request.body_length().is_some_and(|n| n > MAX_BODY);
        if !too_big {
            use std::io::Read;
            let _ = request.as_reader().take(MAX_BODY as u64 + 1).read_to_string(&mut body);
        }
        let method = request.method().as_str().to_string();
        let url = request.url().to_string();
        let out = if too_big {
            reply(413, "text/plain", "report too large")
        } else {
            handle(&method, &url, auth.as_deref(), &body, &dir, &token, chrono::Utc::now().timestamp_millis())
        };
        let header = |k: &str, v: &str| tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()).expect("static header");
        let response = tiny_http::Response::from_string(out.body)
            .with_status_code(out.status)
            .with_header(header("Content-Type", out.content_type))
            .with_header(header("Cache-Control", "no-store"))
            .with_header(header("X-Content-Type-Options", "nosniff"))
            .with_header(header("Referrer-Policy", "no-referrer"))
            .with_header(header("Content-Security-Policy", "default-src 'none'; style-src 'unsafe-inline'"));
        let _ = request.respond(response);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aitm_core::seat::{SeatFinding, SeatServer, SeatSpend};

    const TOKEN: &str = "correct-horse-battery";

    fn report(id: &str, label: &str, at: i64) -> SeatReport {
        SeatReport {
            schema: seat::SCHEMA,
            seat_id: id.into(),
            label: label.into(),
            generated_at: at,
            agent_version: "0.1.0".into(),
            os: "macos".into(),
            tools: vec!["Claude Code".into()],
            servers: vec![SeatServer {
                name: "docs".into(), client: "Claude Code".into(), scope: "user".into(), transport: "stdio".into(),
                target: "npx".into(), package: Some("docs-mcp".into()), pinned: Some(false), env_count: 1,
            }],
            agents: 0, skills: 3, hooks: 1, permission_mode: None, allow_rules: 0, ask_rules: 0, deny_rules: 0,
            spend: vec![SeatSpend { provider: "claude".into(), last30: 250.0, today: 4.0,
                top_models: vec![("claude-opus-5".into(), 200.0), ("claude-sonnet-5".into(), 50.0)] }],
            findings: vec![SeatFinding { id: "mcp-unpinned".into(), kind: "tighten".into(), title: "1 MCP server runs an unpinned package".into() }],
            limits: vec![
                seat::SeatLimit { provider: "claude".into(), plan: Some("max".into()), metric: "Session".into(), used_percent: 12.0, resets_at: None },
                seat::SeatLimit { provider: "claude".into(), plan: Some("max".into()), metric: "Weekly".into(), used_percent: 91.0, resets_at: None },
            ],
        }
    }

    fn tmp() -> PathBuf {
        std::env::temp_dir().join(format!("aitm-collector-{}", aitm_core::providers::unique_stamp()))
    }

    #[test]
    fn nothing_but_the_health_check_answers_without_the_token() {
        let dir = tmp();
        for (m, u) in [("GET", "/"), ("GET", "/v1/seats"), ("POST", "/v1/report")] {
            assert_eq!(handle(m, u, None, "", &dir, TOKEN, 0).status, 401, "{m} {u}");
            assert_eq!(handle(m, u, Some("Bearer wrong-token-entirely"), "", &dir, TOKEN, 0).status, 401);
        }
        assert_eq!(handle("GET", "/healthz", None, "", &dir, TOKEN, 0).status, 200);
        assert_eq!(handle("GET", "/?token=correct-horse-battery", None, "", &dir, TOKEN, 0).status, 200, "a browser uses the query");
        assert!(token_matches(TOKEN, TOKEN));
        assert!(!token_matches("correct-horse-batterX", TOKEN));
        assert!(!token_matches("", TOKEN) && !token_matches(TOKEN, ""));
    }

    #[test]
    fn a_report_is_stored_per_seat_and_a_newer_one_replaces_it() {
        let dir = tmp();
        let auth = Some("Bearer correct-horse-battery");
        let post = |r: &SeatReport| handle("POST", "/v1/report", auth, &serde_json::to_string(r).unwrap(), &dir, TOKEN, 0);
        assert_eq!(post(&report("seat-aaaaaaaa", "Dana", 1)).status, 200);
        assert_eq!(post(&report("seat-bbbbbbbb", "Eli", 1)).status, 200);
        assert_eq!(post(&report("seat-aaaaaaaa", "Dana (new laptop)", 2)).status, 200);
        let seats: Vec<SeatReport> =
            serde_json::from_str(&handle("GET", "/v1/seats", auth, "", &dir, TOKEN, 0).body).unwrap();
        assert_eq!(seats.iter().map(|s| s.label.as_str()).collect::<Vec<_>>(), ["Dana (new laptop)", "Eli"]);
        assert_eq!(handle("POST", "/v1/report", auth, "{}", &dir, TOKEN, 0).status, 400);
        assert_eq!(handle("POST", "/v1/report", auth, &"x".repeat(MAX_BODY + 1), &dir, TOKEN, 0).status, 413);
        assert_eq!(handle("GET", "/nope", auth, "", &dir, TOKEN, 0).status, 404);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_policy_file_turns_the_dashboard_into_a_conformance_view() {
        let dir = tmp();
        let auth = Some("Bearer correct-horse-battery");
        let body = serde_json::to_string(&report("seat-aaaaaaaa", "Dana", 1)).unwrap();
        assert_eq!(handle("POST", "/v1/report", auth, &body, &dir, TOKEN, 1).status, 200);
        // No policy yet: the column is a dash and the data endpoint says so.
        let html = handle("GET", "/", auth, "", &dir, TOKEN, 1).body;
        assert!(html.contains("no policy.json set") && html.contains("claude Weekly 91%"), "the tightest limit, not the first");
        assert_eq!(handle("GET", "/v1/conformance", auth, "", &dir, TOKEN, 1).status, 404);

        std::fs::write(dir.join("policy.json"), r#"{"requirePinned": true, "minDenyRules": 3}"#).unwrap();
        let html = handle("GET", "/", auth, "", &dir, TOKEN, 1).body;
        assert!(html.contains("<b>1</b><span>seats out of policy</span>"));
        assert!(html.contains("2 issues") && html.contains("docs-mcp without a pinned version"));
        let data = handle("GET", "/v1/conformance", auth, "", &dir, TOKEN, 1).body;
        assert!(data.contains("\"rule\":\"unpinned\"") && data.contains("\"rule\":\"deny-rules\""));
        assert_eq!(handle("GET", "/v1/conformance", None, "", &dir, TOKEN, 1).status, 401);

        std::fs::write(dir.join("policy.json"), "{broken").unwrap();
        assert!(handle("GET", "/", auth, "", &dir, TOKEN, 1).body.contains("no policy.json set"), "a broken file is no policy, not an error page");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_seat_id_cannot_climb_out_of_the_data_folder() {
        let dir = tmp();
        let evil = serde_json::to_string(&report("seat-aaaaaaaa", "x", 1)).unwrap().replace("seat-aaaaaaaa", "../../outside");
        assert_eq!(handle("POST", "/v1/report", Some("Bearer correct-horse-battery"), &evil, &dir, TOKEN, 0).status, 400);
        assert!(!dir.parent().unwrap().join("outside.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_dashboard_totals_the_team_and_a_label_cannot_inject_markup() {
        let now = 100 * 24 * 3_600_000;
        let mut old = report("seat-cccccccc", "<script>alert(1)</script>", now - 9 * 24 * 3_600_000);
        old.servers[0].pinned = Some(true);
        let html = dashboard(&[report("seat-aaaaaaaa", "Dana", now - 3_600_000), old], now);
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("<b>2</b><span>seats</span>"));
        assert!(html.contains("<b>1</b><span>of them unpinned</span>"));
        assert!(html.contains("<b>$500</b>"));
        assert_eq!(dollars(2068.4), "$2,068");
        assert_eq!(dollars(1_234_567.0), "$1,234,567");
        assert_eq!(dollars(12.0), "$12");
        // One seat running a server in two apps counts once.
        let mut twice = report("seat-dddddddd", "Two apps", now);
        twice.servers.push(SeatServer { client: "Claude Desktop".into(), ..twice.servers[0].clone() });
        assert!(dashboard(&[twice], now).contains("<td>docs-mcp</td><td class=n>1</td>"));
        assert!(html.contains("class=stale"), "nine days without a report greys the seat");
        assert!(html.contains("80%"), "200 of 250 on the largest models");
        assert!(dashboard(&[], now).contains("No seat has reported yet."));
    }
}
