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

use std::collections::HashMap;

use serde::Serialize;

use crate::clients::{self, ClientRule};
use crate::i18n::Msg;
use crate::inventory::{McpServer, Opportunity};
use crate::spend::{self, LivePace};

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
    /// Percent CPU, when the `ps` line carried a column for it that parsed as
    /// a number. Always `None` on Windows: `Win32_Process` has no such field.
    pub cpu_percent: Option<f32>,
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

/// Parse `ps -axo pid=,ppid=,rss=,etime=,pcpu=,args=`. `rss` is in kilobytes.
/// `pcpu` is peeked rather than required: the token right after `etime` is
/// taken as it only when it parses as a number, so a `ps` build with no such
/// column -- or with something unreadable in its place, such as a zombie's
/// blank field -- leaves `cpu_percent: None` and that token starts `args`
/// instead of being silently dropped. This is also how Windows, whose
/// process table never carries a %cpu column at all, ends up with `None`
/// through the same, single, unbranched parser.
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
        // Try the next token as %cpu; only consume it from `words` if it is
        // actually numeric, so a command line's own first word is never eaten.
        let mut after_cpu = words.clone();
        let cpu_percent = after_cpu.next().and_then(|t| t.parse::<f32>().ok());
        if cpu_percent.is_some() {
            words = after_cpu;
        }
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
            cpu_percent,
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
        // An agent host (Claude Code, Codex, ...) is a row in the agents
        // list, never a server here. Without this, an npm-installed host
        // -- matched by its extracted package the same conservative way a
        // real MCP server is -- would also show up as an "unconfigured"
        // server sitting right next to its own agent row.
        if agent_host_of(p).is_some() {
            continue;
        }
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
        .args(["-axo", "pid=,ppid=,rss=,etime=,pcpu=,args="])
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
// Agent hosts
// ---------------------------------------------------------------------------

/// CLIs that host an AI coding session directly, rather than an MCP server
/// one of them might spawn. Each entry is (display name, binary file names,
/// npm package names). Presence in the process table is matched on
/// `RawProc.binary` or `RawProc.package`, the same conservative fields
/// `claims` uses for MCP servers -- never on argv text -- which is how an
/// npm-installed host run through a runner (`npx @anthropic-ai/claude-code`)
/// is still recognised even though its binary is `npx`.
const AGENT_HOSTS: &[(&str, &[&str], &[&str])] = &[
    ("Claude Code", &["claude"], &["@anthropic-ai/claude-code"]),
    ("Codex", &["codex"], &["@openai/codex"]),
    ("Gemini CLI", &["gemini"], &["@google/gemini-cli"]),
    ("Cursor Agent", &["cursor-agent"], &[]),
    ("Aider", &["aider"], &[]),
    ("OpenCode", &["opencode"], &[]),
    ("Goose", &["goose"], &[]),
    ("GitHub Copilot CLI", &["copilot"], &["@github/copilot"]),
];

/// Which agent host, if any, this process is.
fn agent_host_of(proc: &RawProc) -> Option<&'static str> {
    AGENT_HOSTS.iter().find_map(|entry| {
        let (name, bins, pkgs) = *entry;
        let matched = bins.contains(&proc.binary.as_str())
            || proc.package.as_deref().is_some_and(|p| pkgs.contains(&p));
        matched.then_some(name)
    })
}

/// One agent host process running right now, folded together with whatever
/// plain subprocesses it spawned.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunningAgent {
    /// The `AGENT_HOSTS` display name.
    pub tool: String,
    /// Never shown in the UI: there is no End task for an agent, only for
    /// the MCP servers it starts.
    pub pid: u32,
    /// The host process's own uptime. Folding a child never changes this.
    pub elapsed_secs: u64,
    /// The host plus every folded child.
    pub rss_bytes: u64,
    /// The host plus every folded child whose own reading was known; `None`
    /// only when nothing in the group ever reported one (which is every
    /// group on Windows, where `RawProc::cpu_percent` is always `None`).
    pub cpu_percent: Option<f32>,
    /// `None` on Windows, or when `lsof` had nothing for this pid.
    pub cwd: Option<String>,
    /// The most recent work area this agent's live session's tool calls
    /// touched. `None` when `cwd` is unknown, there is no live session for
    /// it, or none of its recent activity named an area.
    pub area: Option<String>,
    /// `clients::client_of(area, rules)` for `area` above, when a rule
    /// matches it. Never guessed when there is no area to match against.
    pub client: Option<String>,
    /// Tokens, cost and idle time from the newest live session in `cwd`.
    /// Agents that share a folder share this: it names the newest session
    /// in that folder, not a session specific to this one process.
    pub pace: Option<LivePace>,
}

/// The pid of the nearest agent-host ancestor `proc` folds into, or `None`
/// when it does not fold at all.
///
/// A process folds only when it is not itself an agent host -- a host whose
/// own parent chain reaches another host, such as `claude` running
/// `claude -p`, is always its own row, never folded -- and it carries no
/// extracted package of its own: a process with a package is an MCP
/// server's root and belongs to `group`'s output instead, so the walk up
/// also stops the moment it meets one, and that subtree's memory is never
/// folded into the agent and counted twice.
fn host_ancestor_pid<'a>(by_pid: &HashMap<u32, &'a RawProc>, proc: &'a RawProc) -> Option<u32> {
    if agent_host_of(proc).is_some() || proc.package.is_some() {
        return None;
    }
    let mut current = proc;
    // A generous bound against a cyclic ppid chain in adversarial input;
    // real process trees on either platform are nowhere near this deep.
    for _ in 0..256 {
        let parent: &RawProc = *by_pid.get(&current.ppid)?;
        if agent_host_of(parent).is_some() {
            return Some(parent.pid);
        }
        if parent.package.is_some() {
            return None;
        }
        current = parent;
    }
    None
}

/// Parse `lsof -a -p <pids> -d cwd -Fn`: one field per line. `p<pid>` opens a
/// process; the `n<path>` that follows (with `-d cwd` there is at most one
/// per process) is its cwd. Every other prefix (`f`, `t`, ...) is ignored,
/// and a line that is neither is skipped rather than guessed at.
pub fn parse_lsof_cwd(out: &str) -> HashMap<u32, String> {
    let mut map = HashMap::new();
    let mut current: Option<u32> = None;
    for line in out.lines() {
        let mut chars = line.trim_end().chars();
        let Some(tag) = chars.next() else { continue };
        let rest = chars.as_str();
        match tag {
            'p' => current = rest.parse::<u32>().ok(),
            'n' if !rest.is_empty() => {
                if let Some(pid) = current {
                    map.insert(pid, rest.to_string());
                }
            }
            _ => {}
        }
    }
    map
}

/// Working directories for these pids, one `lsof` call for the whole batch.
/// Stdout is parsed and stderr ignored, so a pid that exited between the
/// snapshot and this call, or one owned by another user, does not stop
/// `lsof` reporting the rest; failing to run it at all, or asking for zero
/// pids, gives an empty map without a spawn.
///
/// A pid can also be REUSED by an unrelated process in that same gap:
/// `lsof` then reports the new process's cwd under the old pid, so a
/// folder can name the wrong place for one refresh. Cosmetic, not acted on
/// -- nothing here does more than display a path, and the next snapshot
/// reads a fresh process table and corrects it.
#[cfg(not(windows))]
fn cwd_of_pids(pids: &[u32]) -> HashMap<u32, String> {
    if pids.is_empty() {
        return HashMap::new();
    }
    let list = pids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    std::process::Command::new("/usr/sbin/lsof")
        .args(["-a", "-p", &list, "-d", "cwd", "-Fn"])
        .output()
        .ok()
        .map(|out| parse_lsof_cwd(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default()
}

#[cfg(windows)]
fn cwd_of_pids(_pids: &[u32]) -> HashMap<u32, String> {
    // Win32_Process carries no cwd for another process, and reading one
    // through its PEB needs native calls this app does not make yet -- a gap,
    // not a guess.
    HashMap::new()
}

/// Every agent host running right now, its plain subprocesses folded in and
/// sorted by how long the host itself has been up. Pure and tested; the live
/// process table and cwd map are read by `live_agents` below.
pub fn agents_from(raw: &[RawProc], cwds: &HashMap<u32, String>) -> Vec<RunningAgent> {
    let by_pid: HashMap<u32, &RawProc> = raw.iter().map(|p| (p.pid, p)).collect();
    let mut out: Vec<RunningAgent> = raw
        .iter()
        .filter_map(|p| {
            agent_host_of(p).map(|tool| RunningAgent {
                tool: tool.to_string(),
                pid: p.pid,
                elapsed_secs: p.elapsed_secs,
                rss_bytes: p.rss_bytes,
                cpu_percent: p.cpu_percent,
                cwd: cwds.get(&p.pid).cloned(),
                area: None,
                client: None,
                pace: None,
            })
        })
        .collect();
    for p in raw {
        let Some(host_pid) = host_ancestor_pid(&by_pid, p) else { continue };
        let Some(row) = out.iter_mut().find(|a| a.pid == host_pid) else { continue };
        row.rss_bytes += p.rss_bytes;
        row.cpu_percent = match (row.cpu_percent, p.cpu_percent) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
        };
    }
    out.sort_by(|a, b| b.elapsed_secs.cmp(&a.elapsed_secs));
    out
}

/// Fills in each agent's area, client and live pace from its folder alone.
/// Pure: `lookup` (a fresh read of that folder's newest session file) and
/// the client rules both come from the caller, so this needs no I/O of its
/// own to test. An agent with no known `cwd` is left exactly as `agents_from`
/// built it -- there is no folder to ask a session or a client rule about.
/// Agents that share a `cwd` share one `lookup` call: the result is cached
/// by folder for this pass, so two hosts backed by the same session file
/// never make `lookup` read it twice.
pub fn attach_context(agents: &mut [RunningAgent], rules: &[ClientRule], lookup: &dyn Fn(&str) -> Option<LivePace>) {
    let mut cache: HashMap<String, Option<LivePace>> = HashMap::new();
    for agent in agents.iter_mut() {
        let Some(cwd) = agent.cwd.as_deref() else { continue };
        let pace = cache.entry(cwd.to_string()).or_insert_with(|| lookup(cwd)).clone();
        agent.area = pace.as_ref().and_then(|p| p.area.clone());
        agent.client = agent.area.as_deref().and_then(|a| clients::client_of(a, rules)).map(str::to_string);
        agent.pace = pace;
    }
}

/// The pids worth asking `lsof` for a cwd: agent hosts only. `agents_from`
/// gives its own `RunningAgent` row to a host alone -- a plain child folds
/// into its host's totals and an MCP server's own tree belongs to `group`
/// instead -- so a cwd fetched for any other pid would sit unread: nothing
/// in the output ever looks it up.
fn agent_host_pids(raw: &[RawProc]) -> Vec<u32> {
    raw.iter().filter(|p| agent_host_of(p).is_some()).map(|p| p.pid).collect()
}

/// The live picture: every running agent host, folded and sorted like
/// `agents_from`, with its work area, client and live pace attached. Agents
/// that share a cwd share a pace -- the newest live session in that folder.
pub fn agents_snapshot(rules: &[ClientRule]) -> Vec<RunningAgent> {
    let Some(table) = process_table() else { return Vec::new() };
    let raw = parse_ps(&table);
    let pids = agent_host_pids(&raw);
    let cwds = cwd_of_pids(&pids);
    let mut agents = agents_from(&raw, &cwds);
    attach_context(&mut agents, rules, &|cwd| spend::live_session_for_cwd(cwd, crate::pricing::now_ms()));
    agents
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
pub fn end_task(name: &str, servers: &[McpServer]) -> Result<usize, Msg> {
    let live = snapshot(servers);
    let target = live
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| Msg::new("error.procs.notRunning").var("name", name))?;
    if target.pids.is_empty() {
        return Err(Msg::new("error.procs.noProcesses").var("name", name));
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
        return Err(Msg::new("error.procs.refused").var("name", name));
    }
    Ok(ended)
}

/// The test-side key registry: every `error.procs.*` key this module can
/// emit. Only `i18n.rs`'s test module reads this, so it does not exist in a
/// release build at all.
#[cfg(test)]
pub(crate) const ERROR_KEYS: &[&str] =
    &["error.procs.notRunning", "error.procs.noProcesses", "error.procs.refused"];

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

/// The test-side key registry (see inventory::FINDING_IDS): every finding id
/// this module can emit. Only `i18n.rs`'s test module reads this, so it does
/// not exist in a release build at all.
#[cfg(test)]
pub(crate) const FINDING_IDS: &[&str] = &["mcp-duplicate-processes", "mcp-running-unconfigured", "mcp-memory"];

/// What the running picture suggests. Same contract as the setup ones: every
/// finding states this machine's own numbers, and a quiet machine shows none.
pub fn opportunities(running: &[RunningServer]) -> Vec<Opportunity> {
    let mut out = Vec::new();
    let mut push = |id: &str, kind: &str, title_msg: Msg, detail_msg: Msg| {
        out.push(Opportunity::from_msgs(id, kind, title_msg, Some(detail_msg), Some(MCP_LEARN)));
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

/// Real snapshot from this machine. Ignored: run it once by hand
/// (`cargo test -p aitm-core live_agents -- --ignored --nocapture`) while a
/// real agent host is open, to read back what this machine's own `ps` prints
/// for it and confirm -- or correct -- `AGENT_HOSTS` against it, and to see
/// the area/client/pace fields `agents_snapshot` attaches for real.
#[test]
#[ignore]
fn live_agents() {
    let Some(table) = process_table() else {
        println!("no process table available");
        return;
    };
    let raw = parse_ps(&table);
    let pids: Vec<u32> = raw.iter().map(|p| p.pid).collect();
    let cwds = cwd_of_pids(&pids);
    for a in agents_from(&raw, &cwds) {
        println!(
            "{:<20} pid={:<7} up {:>6}s  {:>6} MB  cpu={:?}  cwd={:?}",
            a.tool,
            a.pid,
            a.elapsed_secs,
            a.rss_bytes / 1_048_576,
            a.cpu_percent,
            a.cwd
        );
    }
    println!("--- agents_snapshot: area / client / pace ---");
    for a in agents_snapshot(&clients::load_from(&clients::path())) {
        println!(
            "{:<20} pid={:<7} up {:>6}s  area={:?}  client={:?}  pace={:?}",
            a.tool, a.pid, a.elapsed_secs, a.area, a.client, a.pace
        );
    }
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
        assert!(crate::i18n::render("en", &err).contains("not running"), "{err:?}");
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

    #[test]
    fn cpu_column_is_optional() {
        // The column is there and numeric.
        let with_cpu = parse_ps("100 1 2048 05:00 12.3 npx claude\n");
        assert_eq!(with_cpu[0].cpu_percent, Some(12.3));
        assert_eq!(with_cpu[0].binary, "npx", "the real command is untouched");

        // No %cpu column at all -- an older `ps`, or the Windows table,
        // which never has one: the token that would have been it is simply
        // the start of the command instead of being dropped.
        let without_cpu = parse_ps("100 1 2048 05:00 npx claude\n");
        assert_eq!(without_cpu[0].cpu_percent, None);
        assert_eq!(without_cpu[0].binary, "npx");

        // Present but not a number (e.g. a zombie's blank field rendered as
        // something unreadable): same result, nothing panics or is dropped.
        let garbled = parse_ps("100 1 2048 05:00 -- npx claude\n");
        assert_eq!(garbled[0].cpu_percent, None);
    }

    #[test]
    fn parse_lsof_reads_pid_then_cwd_records() {
        let out = "p500\nfcwd\ntDIR\nn/Users/dana/project\n\
                   pnot-a-pid\nn/should/not/be/kept\n\
                   p600\nfcwd\nn/Users/dana/other\n";
        let got = parse_lsof_cwd(out);
        assert_eq!(got.len(), 2, "the malformed pid record contributes nothing");
        assert_eq!(got.get(&500).map(String::as_str), Some("/Users/dana/project"));
        assert_eq!(got.get(&600).map(String::as_str), Some("/Users/dana/other"));
    }

    #[test]
    fn a_bare_claude_binary_is_an_agent_not_a_server() {
        let table = "500 1 51200 10:00 1.2 claude\n";
        let got = agents_from(&parse_ps(table), &HashMap::new());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].tool, "Claude Code");
        assert_eq!(got[0].pid, 500);
        assert_eq!(got[0].cpu_percent, Some(1.2));
        assert_eq!(got[0].cwd, None, "no lsof data was supplied");
    }

    #[test]
    fn an_npm_installed_claude_is_matched_by_package() {
        // Launched via a runner, so the binary is "npx", not "claude"; only
        // the extracted package identifies it.
        let table = "600 1 61440 02:00 0.4 npx @anthropic-ai/claude-code\n";
        let got = agents_from(&parse_ps(table), &HashMap::new());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].tool, "Claude Code");
        assert_eq!(got[0].pid, 600);
    }

    #[test]
    fn an_agent_hosts_child_is_folded_into_the_parent_row() {
        // claude (500) spawns a plain subshell (600) with no package of its
        // own: it folds in rather than becoming a second row.
        let table = "500 1 51200 10:00 1.0 claude\n\
                     600 500 10240 09:59 0.5 /bin/zsh -c some-helper\n";
        let got = agents_from(&parse_ps(table), &HashMap::new());
        assert_eq!(got.len(), 1, "the child is folded, not a row of its own");
        assert_eq!(got[0].tool, "Claude Code");
        assert_eq!(got[0].pid, 500, "the pid shown is the host's own");
        assert_eq!(got[0].elapsed_secs, 600, "the host's own uptime, untouched by folding");
        assert_eq!(got[0].rss_bytes, (51200 + 10240) * 1024, "host plus the folded child");
        assert_eq!(got[0].cpu_percent, Some(1.5), "summed while both readings are known");
    }

    #[test]
    fn an_mcp_servers_child_is_never_folded_into_the_agent() {
        // claude (500) spawns an MCP server via npx (600), which spawns the
        // server's own binary (700). Neither belongs to the agent's memory:
        // that subtree is `group`'s to count, never counted twice here.
        let table = "500 1 51200 10:00 1.0 claude\n\
                     600 500 8192 09:59 0.1 npx some-mcp@latest\n\
                     700 600 4096 09:58 0.1 some-mcp\n";
        let got = agents_from(&parse_ps(table), &HashMap::new());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].rss_bytes, 51200 * 1024, "the MCP server subtree is excluded");
    }

    #[test]
    fn a_host_spawned_by_another_host_is_its_own_row() {
        // claude (500) runs `claude -p` (600): still its own row, never
        // folded into its parent, however deep an agent's own chain goes.
        let table = "500 1 51200 10:00 1.0 claude\n\
                     600 500 20480 05:00 0.3 claude -p do-a-thing\n";
        let got = agents_from(&parse_ps(table), &HashMap::new());
        assert_eq!(got.len(), 2);
        assert!(got.iter().any(|a| a.pid == 500 && a.rss_bytes == 51200 * 1024));
        assert!(got.iter().any(|a| a.pid == 600 && a.rss_bytes == 20480 * 1024));
    }

    #[test]
    fn agents_are_sorted_by_elapsed_descending() {
        let table = "100 1 1024 01:00 0.1 aider\n200 1 1024 10:00 0.1 goose\n300 1 1024 05:00 0.1 codex\n";
        let got = agents_from(&parse_ps(table), &HashMap::new());
        assert_eq!(got.iter().map(|a| a.tool.as_str()).collect::<Vec<_>>(), ["Goose", "Codex", "Aider"]);
    }

    #[test]
    fn command_lines_never_reach_running_agent() {
        let table = "700 1 20480 03:00 2.0 claude --api-key sk-ant-PLANTED-SECRET \
                     --vault /Users/dana/Secret Plans\n";
        let got = agents_from(&parse_ps(table), &HashMap::new());
        let json = serde_json::to_string(&got).expect("serializes");
        for secret in ["sk-ant-PLANTED-SECRET", "/Users/dana/Secret", "Plans", "--vault"] {
            assert!(!json.contains(secret), "{secret} reached the output: {json}");
        }
        assert_eq!(got[0].tool, "Claude Code");
    }

    #[test]
    fn an_agent_host_is_never_listed_as_a_server() {
        // An npm-installed Claude Code is launched through npx, so it also
        // carries an extracted package -- exactly what would otherwise
        // match it as an unconfigured MCP server sitting right next to its
        // own agent row.
        let table = "600 1 61440 02:00 0.4 npx @anthropic-ai/claude-code\n";
        assert!(group(&parse_ps(table), &[]).is_empty(), "an agent host is never a server, configured or not");
        let agents = agents_from(&parse_ps(table), &HashMap::new());
        assert_eq!(agents.len(), 1, "it is still an agent");
        assert_eq!(agents[0].tool, "Claude Code");
    }

    #[test]
    fn a_cyclic_parent_chain_terminates() {
        // A's ppid is B and B's ppid is A -- an impossible but adversarial
        // process table. `host_ancestor_pid`'s walk is already bounded
        // (see its own comment), so this returns None instead of hanging.
        let raw = vec![
            RawProc { pid: 1, ppid: 2, rss_bytes: 0, elapsed_secs: 0, package: None, binary: "sh".into(), cpu_percent: None },
            RawProc { pid: 2, ppid: 1, rss_bytes: 0, elapsed_secs: 0, package: None, binary: "sh".into(), cpu_percent: None },
        ];
        let by_pid: HashMap<u32, &RawProc> = raw.iter().map(|p| (p.pid, p)).collect();
        assert_eq!(host_ancestor_pid(&by_pid, &raw[0]), None, "a cyclic ppid chain must terminate, not hang");
    }

    #[test]
    fn agent_host_pids_excludes_folded_children_and_mcp_servers() {
        // claude (500) with a plain folded child (600); an mcp server's
        // runner and its own binary (700, 800), never folded into an agent
        // and never a host themselves; and an npm-installed host (900)
        // matched by package rather than binary.
        let table = "500 1 51200 10:00 1.0 claude\n\
                     600 500 10240 09:59 0.5 /bin/zsh -c some-helper\n\
                     700 1 8192 09:59 0.1 npx some-mcp@latest\n\
                     800 700 4096 09:58 0.1 some-mcp\n\
                     900 1 61440 02:00 0.4 npx @anthropic-ai/claude-code\n";
        let raw = parse_ps(table);
        assert_eq!(
            agent_host_pids(&raw),
            vec![500, 900],
            "lsof is asked about agent hosts only -- never a folded child or an mcp server's own tree"
        );
    }

    #[test]
    fn attach_context_maps_the_pace_area_to_a_client() {
        let table = "500 1 51200 10:00 1.0 claude\n";
        let mut cwds = HashMap::new();
        cwds.insert(500, "/w/acme".to_string());
        let mut agents = agents_from(&parse_ps(table), &cwds);
        let pace = LivePace {
            session_id: "sess-1".into(),
            tokens_10m: 42,
            cost_10m: 0.1,
            priced: true,
            idle_secs: 5,
            model: Some("claude-sonnet-5".into()),
            area: Some("acme".into()),
        };
        let rules = vec![ClientRule { client: "Acme".into(), patterns: vec!["acme".into()], monthly_budget: None }];
        attach_context(&mut agents, &rules, &|cwd| {
            assert_eq!(cwd, "/w/acme", "the lookup is asked about the agent's own folder");
            Some(pace.clone())
        });
        assert_eq!(agents[0].area.as_deref(), Some("acme"));
        assert_eq!(agents[0].client.as_deref(), Some("Acme"));
        assert_eq!(agents[0].pace, Some(pace));
    }

    #[test]
    fn an_agent_without_a_live_session_keeps_only_its_folder() {
        let table = "500 1 51200 10:00 1.0 claude\n";
        let mut cwds = HashMap::new();
        cwds.insert(500, "/w/quiet".to_string());
        let mut agents = agents_from(&parse_ps(table), &cwds);
        // A rule that would match the bare folder name, to prove client
        // resolution goes through the pace's own area and never the raw
        // cwd directly.
        let rules = vec![ClientRule { client: "Acme".into(), patterns: vec!["quiet".into()], monthly_budget: None }];
        attach_context(&mut agents, &rules, &|_cwd| None);
        assert_eq!(agents[0].cwd.as_deref(), Some("/w/quiet"), "the folder itself is untouched");
        assert_eq!(agents[0].area, None, "no live session means no area to report");
        assert_eq!(agents[0].client, None, "so no client, even though a rule would match the bare folder name");
        assert_eq!(agents[0].pace, None);
    }

    #[test]
    fn attach_context_memoises_the_lookup_per_folder() {
        // Two different hosts, same cwd: the folder's live session should be
        // read once and shared, not re-read per agent.
        let table = "500 1 51200 10:00 1.0 claude\n600 1 20480 09:00 0.5 codex\n";
        let mut cwds = HashMap::new();
        cwds.insert(500, "/w/acme".to_string());
        cwds.insert(600, "/w/acme".to_string());
        let mut agents = agents_from(&parse_ps(table), &cwds);
        assert_eq!(agents.len(), 2, "both hosts get their own row");
        let calls = std::cell::Cell::new(0u32);
        let pace = LivePace {
            session_id: "sess-1".into(),
            tokens_10m: 1,
            cost_10m: 0.0,
            priced: true,
            idle_secs: 0,
            model: None,
            area: Some("acme".into()),
        };
        attach_context(&mut agents, &[], &|cwd| {
            calls.set(calls.get() + 1);
            assert_eq!(cwd, "/w/acme");
            Some(pace.clone())
        });
        assert_eq!(calls.get(), 1, "two agents sharing a folder must trigger one lookup");
        assert!(agents.iter().all(|a| a.pace == Some(pace.clone())), "both still get the shared result");
    }
}
