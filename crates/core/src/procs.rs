//! What is actually running right now: the Task Manager half of the name.
//!
//! `inventory.rs` reads what the machine is *configured* to run. This reads
//! what it *is* running, and matches the two. A server configured but not
//! running is normal (they start on demand); a server running twice, or one
//! running that nothing configured, is worth seeing.
//!
//! **Command lines never leave this module.** A server's arguments routinely
//! hold API keys, vault paths and project directories (the same fields
//! `inventory.rs` refuses to copy). Nothing here reports an argument. The one
//! string lifted out of a command line is the package specifier straight after
//! a known runner, and only when it matches a conservative package charset, so
//! `--api-key sk-…` and `--vault /Users/…` can never be mistaken for one.
//! `never_leaks_*` plants those in the input and asserts they never appear.

use serde::Serialize;

use crate::i18n::{self, Msg};
use crate::inventory::{McpServer, Opportunity};

/// Runners that take a package directly, after optional flags ("npx -y pkg").
const DIRECT_RUNNERS: [&str; 3] = ["npx", "uvx", "bunx"];
/// Runners that need a subcommand first, and which subcommand fetches a
/// package. `npm run` and `pnpm run` execute a LOCAL SCRIPT, never a package:
/// reading them turned `npm run tauri dev` into a server called "tauri".
const VERB_RUNNERS: [(&str, &[&str]); 3] =
    [("npm", &["exec"]), ("pnpm", &["dlx", "exec"]), ("yarn", &["dlx"])];

/// One MCP server as it exists in memory right now. A server is often a small
/// process tree (the runner, then what it launched); those are one entry.
#[derive(Serialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunningServer {
    /// The configured server's name when it matched, else its package name.
    pub name: String,
    /// Whether an MCP server in the inventory claims this process.
    pub configured: bool,
    /// The app that configured it, when known.
    pub client: Option<String>,
    /// The package it runs, when it was launched through a runner.
    pub package: Option<String>,
    /// How many independent copies are running.
    pub instances: usize,
    /// Resident memory across every process in the group.
    pub rss_bytes: u64,
    /// How long the oldest process in the group has been up.
    pub elapsed_secs: u64,
    /// Processes in the group. Ids only: useful for the user, not identifying.
    pub pids: Vec<u32>,
}

/// One line of `ps`, already stripped of everything but what we may keep.
#[derive(Debug, Clone, PartialEq)]
pub struct RawProc {
    pub pid: u32,
    pub ppid: u32,
    pub rss_bytes: u64,
    pub elapsed_secs: u64,
    /// The package specifier, when the command line began with a known runner.
    pub package: Option<String>,
    /// The executable's file name. Never a path, never an argument.
    pub binary: String,
}

/// `DD-HH:MM:SS`, `HH:MM:SS` or `MM:SS`, which is what `ps -o etime` prints.
pub fn parse_etime(s: &str) -> u64 {
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.trim().parse::<u64>().unwrap_or(0), r),
        None => (0, s.trim()),
    };
    let mut secs = 0u64;
    for part in rest.split(':') {
        secs = secs * 60 + part.trim().parse::<u64>().unwrap_or(0);
    }
    days * 86_400 + secs
}

/// A package specifier is `name`, `@scope/name`, either with an optional
/// `@version`. Anything with a slash that is not a scope, anything starting
/// with `-`, and anything holding a character a package name cannot contain is
/// rejected. This is what keeps flags, values and paths out of the output.
fn package_like(token: &str) -> Option<String> {
    if token.is_empty() || token.starts_with('-') || token.len() > 128 {
        return None;
    }
    // Strip the version: "pkg@1.2.3" and "@scope/pkg@latest".
    let body = match token.rfind('@') {
        Some(i) if i > 0 => &token[..i],
        _ => token,
    };
    let (scope, name) = match body.strip_prefix('@') {
        Some(rest) => match rest.split_once('/') {
            Some((s, n)) => (Some(s), n),
            None => return None,
        },
        None => (None, body),
    };
    if name.is_empty() || name.contains('/') {
        return None;
    }
    let ok = |s: &str| {
        !s.is_empty()
            && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
    };
    if !ok(name) || scope.is_some_and(|s| !ok(s)) {
        return None;
    }
    Some(match scope {
        Some(s) => format!("@{s}/{name}"),
        None => name.to_string(),
    })
}

/// The file name of the executable, with no directory and no arguments.
fn binary_of(args: &str) -> String {
    let first = args.split_whitespace().next().unwrap_or_default();
    first.rsplit('/').next().unwrap_or(first).to_string()
}

/// The package a command line runs, when it starts with a known runner.
/// Only the token right after the runner (and its verb) is ever considered.
pub fn package_in(args: &str) -> Option<String> {
    let mut words = args.split_whitespace();
    let runner = binary_of(words.next()?);
    if let Some((_, verbs)) = VERB_RUNNERS.iter().find(|(r, _)| *r == runner) {
        let verb = words.next()?;
        if !verbs.contains(&verb) {
            return None;
        }
    } else if !DIRECT_RUNNERS.contains(&runner.as_str()) {
        return None;
    }
    // Skip the runner's own flags, then read exactly one token.
    for word in words.by_ref().take(4) {
        if word.starts_with('-') {
            continue;
        }
        return package_like(word);
    }
    None
}

/// Parse `ps -axo pid=,ppid=,rss=,etime=,args=`. `rss` is in kilobytes.
pub fn parse_ps(output: &str) -> Vec<RawProc> {
    let mut out = Vec::new();
    for line in output.lines() {
        let line = line.trim_start();
        if line.is_empty() {
            continue;
        }
        // Four numeric columns, then the command line as the remainder.
        let mut words = line.split_whitespace();
        let (pid, ppid, rss, etime) = match (words.next(), words.next(), words.next(), words.next()) {
            (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
            _ => continue,
        };
        let (pid, ppid, rss) = match (pid.parse::<u32>(), ppid.parse::<u32>(), rss.parse::<u64>()) {
            (Ok(a), Ok(b), Ok(c)) => (a, b, c),
            _ => continue,
        };
        let args = words.collect::<Vec<_>>().join(" ");
        if args.is_empty() {
            continue;
        }
        out.push(RawProc {
            pid,
            ppid,
            rss_bytes: rss * 1024,
            elapsed_secs: parse_etime(etime),
            package: package_in(&args),
            binary: binary_of(&args),
        });
    }
    out
}

/// A configured server's package with its version specifier removed, so
/// `@playwright/mcp@latest` in the config matches `@playwright/mcp` running.
fn base_package(spec: &str) -> String {
    match spec.rfind('@') {
        Some(i) if i > 0 => spec[..i].to_string(),
        _ => spec.to_string(),
    }
}

/// Does this process belong to that configured server?
fn claims(server: &McpServer, proc: &RawProc) -> bool {
    if let (Some(want), Some(got)) = (server.package.as_ref(), proc.package.as_ref()) {
        if base_package(want) == *got {
            return true;
        }
    }
    // A package that spawns a like-named binary ("chrome-devtools-mcp").
    if let Some(want) = server.package.as_ref() {
        let base = base_package(want);
        let leaf = base.rsplit('/').next().unwrap_or(&base);
        if !leaf.is_empty() && proc.binary == leaf {
            return true;
        }
    }
    false
}

/// Group running processes into servers, matching the inventory where it can.
///
/// Processes that match nothing MCP-shaped are dropped: this is a view of the
/// AI setup, not a system process list, and every dropped line is one that
/// could otherwise carry an argument into the UI.
pub fn group(procs: &[RawProc], servers: &[McpServer]) -> Vec<RunningServer> {
    let mut out: Vec<RunningServer> = Vec::new();
    // A runner and the binary it spawned are one instance, not two.
    let mut roots: Vec<(String, &RawProc, Option<&McpServer>)> = Vec::new();
    for p in procs {
        let matched = servers.iter().find(|s| claims(s, p));
        let key = match (matched, p.package.as_ref()) {
            (Some(s), _) => s.name.clone(),
            (None, Some(pkg)) => pkg.clone(),
            (None, None) => continue,
        };
        roots.push((key, p, matched));
    }
    for (key, p, matched) in &roots {
        // A copy is a process whose parent is not already in the same group,
        // so a runner and everything it spawned count once however deep the
        // chain goes. Checked against the whole group, not the part seen so
        // far, because `ps` does not order children after their parents.
        let parent_in_group = roots.iter().any(|(k, q, _)| k == key && q.pid == p.ppid);
        let entry = match out.iter_mut().find(|e| e.name == *key) {
            Some(e) => e,
            None => {
                out.push(RunningServer {
                    name: key.clone(),
                    configured: matched.is_some(),
                    client: matched.map(|s| s.client.clone()),
                    package: matched
                        .and_then(|s| s.package.clone())
                        .map(|s| base_package(&s))
                        .or_else(|| p.package.clone()),
                    ..Default::default()
                });
                out.last_mut().expect("just pushed")
            }
        };
        entry.rss_bytes += p.rss_bytes;
        entry.elapsed_secs = entry.elapsed_secs.max(p.elapsed_secs);
        entry.pids.push(p.pid);
        if !parent_in_group {
            entry.instances += 1;
        }
    }
    for e in out.iter_mut() {
        e.pids.sort_unstable();
        e.instances = e.instances.max(1);
    }
    out.sort_by(|a, b| b.rss_bytes.cmp(&a.rss_bytes).then_with(|| a.name.cmp(&b.name)));
    out
}

/// Read the process table. Unix asks `ps`; Windows asks PowerShell for the
/// same four fields plus the command line.
#[cfg(not(windows))]
fn process_table() -> Option<String> {
    let out = std::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,rss=,etime=,args="])
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(windows)]
fn process_table() -> Option<String> {
    // Same column order as ps, with elapsed seconds rendered as HH:MM:SS and
    // working set in kilobytes, so `parse_ps` needs no platform branch.
    const SCRIPT: &str = "Get-CimInstance Win32_Process | ForEach-Object { \
        $s = if ($_.CreationDate) { [int]((Get-Date) - $_.CreationDate).TotalSeconds } else { 0 }; \
        '{0} {1} {2} {3}:{4}:{5} {6}' -f $_.ProcessId, $_.ParentProcessId, \
        [int]($_.WorkingSetSize/1kb), [int]($s/3600), [int](($s%3600)/60), [int]($s%60), $_.CommandLine }";
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// What is running right now, matched against the configured servers.
pub fn snapshot(servers: &[McpServer]) -> Vec<RunningServer> {
    match process_table() {
        Some(table) => group(&parse_ps(&table), servers),
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Ending a task
// ---------------------------------------------------------------------------

/// Stop a running MCP server, by name, after the caller has confirmed it.
///
/// This is the second and last thing the app does *to* the machine (the first
/// is `pin.rs`), so it carries the same kind of contract:
///
/// * the caller names a **server**, never a pid, and the pids are taken from a
///   snapshot read inside this call, so a number cannot be smuggled in and a
///   recycled pid cannot be hit;
/// * only a process this module already matched as an MCP server can be named,
///   so nothing else on the machine is reachable through it;
/// * it asks politely. SIGTERM on Unix, `taskkill` without `/F` on Windows.
///   A server that ignores it keeps running, and the next refresh will say so.
///   Nothing here escalates to SIGKILL;
/// * the server is meant to come back: every client starts these on demand, so
///   ending one frees the memory and the next request spawns a fresh copy.
pub fn end_task(name: &str, servers: &[McpServer]) -> Result<usize, String> {
    let live = snapshot(servers);
    let target = live.iter().find(|s| s.name == name).ok_or_else(|| format!("{name} is not running any more"))?;
    if target.pids.is_empty() {
        return Err(format!("{name} has no processes to end"));
    }
    // Children first: ending a runner first can orphan what it spawned.
    let mut pids = target.pids.clone();
    pids.sort_unstable_by(|a, b| b.cmp(a));
    let mut ended = 0usize;
    for pid in pids {
        if signal(pid) {
            ended += 1;
        }
    }
    if ended == 0 {
        return Err(format!("could not end {name}: the system refused every process"));
    }
    Ok(ended)
}

/// Ask one process to stop. Never forces.
#[cfg(not(windows))]
fn signal(pid: u32) -> bool {
    std::process::Command::new("/bin/kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn signal(pid: u32) -> bool {
    // No /F: this is a request, not a kill.
    std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Opportunities
// ---------------------------------------------------------------------------

/// A server running this many copies is worth a look: each one is a full
/// process tree, and they are usually one per client app rather than one need.
const MANY_COPIES: usize = 2;
/// Total resident memory across MCP servers at which the number is worth
/// saying out loud. Below this it is noise on any modern machine.
const HEAVY_TOTAL_BYTES: u64 = 1_073_741_824; // 1 GiB

const MCP_LEARN: &str = "https://staas.fund/mcp/";

fn mb(bytes: u64) -> u64 {
    bytes / 1_048_576
}

/// Every finding id this module can emit (see inventory::FINDING_IDS).
pub const FINDING_IDS: &[&str] = &["mcp-duplicate-processes", "mcp-running-unconfigured", "mcp-memory"];

/// What the running picture suggests. Same contract as the setup ones: every
/// finding states this machine's own numbers, and a quiet machine shows none.
pub fn opportunities(running: &[RunningServer]) -> Vec<Opportunity> {
    let mut out = Vec::new();
    let mut push = |id: &str, kind: &str, title_msg: Msg, detail_msg: Msg| {
        out.push(Opportunity {
            id: id.into(),
            kind: kind.into(),
            title: i18n::render("en", &title_msg),
            detail: i18n::render("en", &detail_msg),
            title_msg,
            detail_msg: Some(detail_msg),
            learn_url: Some(MCP_LEARN.into()),
        });
    };

    let mut dupes: Vec<&RunningServer> = running.iter().filter(|s| s.instances >= MANY_COPIES).collect();
    dupes.sort_by(|a, b| b.rss_bytes.cmp(&a.rss_bytes));
    if let Some(worst) = dupes.first() {
        let names = dupes.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ");
        let wasted: u64 = dupes.iter().map(|s| s.rss_bytes - s.rss_bytes / s.instances as u64).sum();
        push(
            "mcp-duplicate-processes",
            "tighten",
            Msg::new("finding.mcp-duplicate-processes.title").count(dupes.len() as i64),
            // The outer sentence's own grammar does not depend on dupes.len()
            // (it always reads "{names} each have…", however many there
            // are), so only the nested "running N times" clause carries a
            // count -- worst.instances, independent of dupes.len().
            Msg::new("finding.mcp-duplicate-processes.detail")
                .var("names", &names)
                .var("worstName", &worst.name)
                .sub("times", Msg::new("unit.times").count(worst.instances as i64))
                .var("mb", mb(worst.rss_bytes))
                .var("wasted", mb(wasted)),
        );
    }

    let unconfigured: Vec<&RunningServer> = running.iter().filter(|s| !s.configured).collect();
    if !unconfigured.is_empty() {
        let names = unconfigured.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ");
        let n = unconfigured.len() as i64;
        push(
            "mcp-running-unconfigured",
            "tighten",
            Msg::new("finding.mcp-running-unconfigured.title").count(n),
            Msg::new("finding.mcp-running-unconfigured.detail").var("names", &names).count(n),
        );
    }

    let total: u64 = running.iter().map(|s| s.rss_bytes).sum();
    if total >= HEAVY_TOTAL_BYTES {
        let mut by_size: Vec<&RunningServer> = running.iter().collect();
        by_size.sort_by(|a, b| b.rss_bytes.cmp(&a.rss_bytes));
        let top = by_size.first().expect("non-empty: total is above the floor");
        let process_total = running.iter().map(|s| s.pids.len()).sum::<usize>() as i64;
        push(
            "mcp-memory",
            "learn",
            // The title never inflects on the server count ("MCP servers"
            // stays plural however many there are); only the detail's
            // leading clause agrees with it.
            Msg::new("finding.mcp-memory.title").var("totalMb", mb(total)),
            Msg::new("finding.mcp-memory.detail")
                .sub("processes", Msg::new("unit.process").count(process_total))
                .var("topName", &top.name)
                .var("topMb", mb(top.rss_bytes))
                .count(running.len() as i64),
        );
    }
    out
}

/// Real snapshot from this machine. Ignored: it depends on what is running.
#[test]
#[ignore]
fn live_procs() {
    let inv = crate::inventory::scan();
    let got = snapshot(&inv.mcp_servers);
    println!("configured servers: {}", inv.mcp_servers.len());
    for s in &got {
        println!(
            "{:<28} {:>7} MB  x{}  up {:>6}s  {}  pkg={:?}",
            s.name,
            s.rss_bytes / 1_048_576,
            s.instances,
            s.elapsed_secs,
            if s.configured { "configured" } else { "NOT CONFIGURED" },
            s.package
        );
    }
    println!("total resident: {} MB", got.iter().map(|s| s.rss_bytes).sum::<u64>() / 1_048_576);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str, package: Option<&str>) -> McpServer {
        McpServer {
            name: name.into(),
            client: "Claude Code".into(),
            scope: "user".into(),
            project: None,
            transport: "stdio".into(),
            target: "npx".into(),
            package: package.map(Into::into),
            env_count: 0,
            pin_to: None,
            source_file: None,
        }
    }

    #[test]
    fn etime_covers_every_ps_shape() {
        assert_eq!(parse_etime("12:34"), 754);
        assert_eq!(parse_etime("01:02:03"), 3723);
        assert_eq!(parse_etime("2-03:00:00"), 2 * 86_400 + 10_800);
        assert_eq!(parse_etime("nonsense"), 0);
    }

    #[test]
    fn reads_the_package_after_a_runner() {
        assert_eq!(package_in("npm exec chrome-devtools-mcp@latest --flag").as_deref(), Some("chrome-devtools-mcp"));
        assert_eq!(package_in("npx -y @playwright/mcp@1.2.3").as_deref(), Some("@playwright/mcp"));
        assert_eq!(package_in("/opt/homebrew/bin/uvx blender-mcp").as_deref(), Some("blender-mcp"));
        assert_eq!(package_in("node /Users/dana/server.js"), None, "only runners are read");
    }

    #[test]
    fn a_local_script_is_not_a_package() {
        // `npm run <script>` runs something from package.json, not a package.
        assert_eq!(package_in("npm run tauri dev"), None);
        assert_eq!(package_in("npm run dev"), None);
        assert_eq!(package_in("pnpm run build"), None);
        assert_eq!(package_in("npm exec chrome-devtools-mcp").as_deref(), Some("chrome-devtools-mcp"));
        assert_eq!(package_in("pnpm dlx some-mcp").as_deref(), Some("some-mcp"));
    }

    #[test]
    fn a_grandchild_is_still_one_copy() {
        // npx -> node -> the binary, with the middle process unmatched by name
        // but carrying the package, and listed out of order.
        let table = "300 200 4096 05:00 chrome-devtools-mcp\n\
                     100 1 1024 05:02 npx chrome-devtools-mcp@latest\n\
                     200 100 2048 05:01 npx chrome-devtools-mcp@latest --inner\n";
        let got = group(&parse_ps(table), &[server("chrome-devtools", Some("chrome-devtools-mcp"))]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].instances, 1, "one tree, however deep");
        assert_eq!(got[0].pids, vec![100, 200, 300]);
    }

    #[test]
    fn a_flag_or_a_path_is_never_taken_for_a_package() {
        // The token after the runner is not always a package; when it is not,
        // nothing is lifted out of the command line at all.
        assert_eq!(package_in("npx --api-key sk-ant-SECRET"), None);
        assert_eq!(package_in("npx /Users/dana/Secret Vault/server.js"), None);
        assert_eq!(package_in("npx @scope-only"), None);
        assert_eq!(package_in("npx UPPER-Case"), None);
    }

    #[test]
    fn parses_a_ps_table() {
        let table = "  1130  1129  93552 01-11:31:43 npm exec chrome-devtools-mcp@latest --autoConnect\n\
                     \x20 1210  1130 168440    31:19 chrome-devtools-mcp\n";
        let got = parse_ps(table);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].pid, 1130);
        assert_eq!(got[0].ppid, 1129);
        assert_eq!(got[0].rss_bytes, 93_552 * 1024);
        assert_eq!(got[0].elapsed_secs, 86_400 + 11 * 3600 + 31 * 60 + 43);
        assert_eq!(got[0].package.as_deref(), Some("chrome-devtools-mcp"));
        assert_eq!(got[1].binary, "chrome-devtools-mcp");
        assert_eq!(got[1].package, None);
    }

    #[test]
    fn a_runner_and_its_child_are_one_instance() {
        let table = "1130 1 93552 05:00 npm exec chrome-devtools-mcp@latest\n\
                     1210 1130 168440 04:59 chrome-devtools-mcp\n";
        let got = group(&parse_ps(table), &[server("chrome-devtools", Some("chrome-devtools-mcp@latest"))]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "chrome-devtools", "the configured name wins over the package");
        assert!(got[0].configured);
        assert_eq!(got[0].instances, 1, "a runner plus the binary it spawned is one copy");
        assert_eq!(got[0].rss_bytes, (93_552 + 168_440) * 1024);
        assert_eq!(got[0].pids, vec![1130, 1210]);
        assert_eq!(got[0].elapsed_secs, 300);
    }

    #[test]
    fn two_independent_copies_count_as_two() {
        let table = "1130 1 93552 05:00 npm exec chrome-devtools-mcp@latest\n\
                     1210 1130 168440 04:59 chrome-devtools-mcp\n\
                     1539 1 95900 03:00 npm exec chrome-devtools-mcp@latest\n\
                     1893 1539 166684 02:59 chrome-devtools-mcp\n";
        let got = group(&parse_ps(table), &[server("chrome-devtools", Some("chrome-devtools-mcp@latest"))]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].instances, 2);
        assert_eq!(got[0].pids.len(), 4);
    }

    #[test]
    fn a_running_server_nothing_configured_is_still_reported() {
        let got = group(&parse_ps("900 1 1024 01:00 npx some-other-mcp@2\n"), &[]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "some-other-mcp");
        assert!(!got[0].configured, "nothing in the inventory claims it");
        assert_eq!(got[0].client, None);
    }

    #[test]
    fn processes_that_are_not_mcp_shaped_are_dropped() {
        let table = "1 0 38292 99:00 /sbin/launchd\n\
                     2 1 90968 99:00 /usr/libexec/logd --mode boot\n";
        assert!(group(&parse_ps(table), &[]).is_empty());
    }

    #[test]
    fn never_leaks_arguments_into_the_output() {
        // Everything a real command line can carry, planted at once.
        let table = "900 1 2048 01:00 npx obsidian-mcp@2 serve --vault /Users/dana/Private Vault \
                     --api-key sk-ant-SECRET --token ghp_SECRET2 --url https://user:pw@example.com/x\n";
        let got = group(&parse_ps(&table), &[server("notes", Some("obsidian-mcp"))]);
        let json = serde_json::to_string(&got).expect("serializes");
        for secret in
            ["sk-ant-SECRET", "ghp_SECRET2", "Private Vault", "/Users/dana", "user:pw", "example.com", "--vault"]
        {
            assert!(!json.contains(secret), "{secret} reached the output: {json}");
        }
        assert_eq!(got[0].name, "notes");
        assert_eq!(got[0].package.as_deref(), Some("obsidian-mcp"));
    }

    #[test]
    fn never_leaks_when_nothing_matches_either() {
        let table = "900 1 2048 01:00 npx --api-key sk-ant-SECRET /Users/dana/x.js\n";
        let json = serde_json::to_string(&group(&parse_ps(table), &[])).expect("serializes");
        assert_eq!(json, "[]", "an unreadable command line is dropped, not guessed at");
    }

    fn running(name: &str, instances: usize, mb: u64, configured: bool) -> RunningServer {
        RunningServer {
            name: name.into(),
            configured,
            instances,
            rss_bytes: mb * 1_048_576,
            pids: (0..instances as u32).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_tidy_machine_gets_no_findings() {
        assert!(opportunities(&[running("notes", 1, 80, true)]).is_empty());
    }

    #[test]
    fn duplicates_are_reported_with_this_machines_numbers() {
        let got = opportunities(&[running("chrome-devtools", 5, 1275, true), running("notes", 1, 80, true)]);
        let dupe = got.iter().find(|o| o.id == "mcp-duplicate-processes").expect("duplicate finding");
        assert_eq!(dupe.kind, "tighten");
        assert!(dupe.title.starts_with("1 MCP server is running"), "{}", dupe.title);
        assert!(dupe.detail.contains("running 5 times"), "{}", dupe.detail);
        assert!(dupe.detail.contains("1275 MB"), "{}", dupe.detail);
        assert!(dupe.detail.contains("1020 MB is duplicate"), "four of five copies: {}", dupe.detail);
        assert!(!dupe.detail.contains("  "), "no collapsed line continuations");
    }

    #[test]
    fn a_server_no_config_claims_is_reported() {
        let got = opportunities(&[running("mystery-mcp", 1, 40, false)]);
        let o = got.iter().find(|o| o.id == "mcp-running-unconfigured").expect("unconfigured finding");
        assert!(o.title.contains("1 MCP server is running"), "{}", o.title);
        assert!(o.detail.contains("mystery-mcp"));
    }

    #[test]
    fn memory_is_only_mentioned_once_it_is_large() {
        assert!(opportunities(&[running("a", 1, 500, true)]).iter().all(|o| o.id != "mcp-memory"));
        let big = opportunities(&[running("a", 1, 1500, true)]);
        let o = big.iter().find(|o| o.id == "mcp-memory").expect("memory finding");
        assert_eq!(o.kind, "learn", "a resting cost is not a failing");
        assert!(o.detail.contains("1500 MB"), "{}", o.detail);
    }

    #[test]
    fn a_name_that_is_not_running_is_refused() {
        // No inventory, so the snapshot cannot contain it whatever is live.
        let err = end_task("definitely-not-running-xyz", &[]).expect_err("must refuse");
        assert!(err.contains("not running"), "{err}");
    }

    /// A missing translation key renders as its own literal key text instead
    /// of failing -- that takes a real fixture run to catch. One fixture
    /// clears all three findings' thresholds at once, exercising the nested
    /// unit.times (5 copies) and unit.process Msgs alongside their outer
    /// sentences.
    #[test]
    fn opportunities_never_render_a_raw_key() {
        let found = opportunities(&[running("chrome-devtools", 5, 1275, true), running("mystery-mcp", 1, 40, false)]);
        assert_eq!(
            found.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(),
            ["mcp-duplicate-processes", "mcp-running-unconfigured", "mcp-memory"]
        );
        for o in found {
            assert!(!o.title.starts_with("finding.") && !o.title.starts_with("unit."), "{}: raw key in title: {}", o.id, o.title);
            assert!(!o.detail.starts_with("finding.") && !o.detail.contains("unit."), "{}: raw key in detail: {}", o.id, o.detail);
        }
    }

    #[test]
    fn heaviest_first() {
        let table = "1 0 1000 01:00 npx small-mcp\n2 0 9000 01:00 npx big-mcp\n";
        let got = group(&parse_ps(table), &[]);
        assert_eq!(got[0].name, "big-mcp");
    }
}
