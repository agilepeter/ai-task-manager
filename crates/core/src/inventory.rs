//! Local AI inventory: what is installed and wired up on this seat — MCP
//! servers, agents, skills, hooks, and the permission posture of Claude Code.
//!
//! Read-only, local-only, and **names and shapes only**. Config files here
//! routinely hold API keys (`env` blocks, tokens in `args`, keys in URL paths
//! and query strings), so nothing in this module ever copies a value out of
//! those fields: an env block becomes a count, args become at most one
//! package name, a URL becomes its host. The `never_leaks_*` tests plant
//! secrets in every such field and assert they are absent from the output.

use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

use crate::i18n::{self, Msg};

/// Config files are small; anything past this is not one we should parse.
const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;
/// Agent / skill definition files: only the frontmatter is read.
const MAX_DEF_BYTES: u64 = 256 * 1024;

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpServer {
    pub name: String,
    /// The app that loads this server: "Claude Code", "Claude Desktop", …
    pub client: String,
    /// "user" | "project"
    pub scope: String,
    /// The project directory for project-scoped servers.
    pub project: Option<String>,
    /// "stdio" | "http" | "sse"
    pub transport: String,
    /// stdio: the launcher's file name ("npx"). http/sse: the URL host only.
    pub target: String,
    /// stdio launched through a package runner: the package it runs.
    pub package: Option<String>,
    /// How many env vars the server is given. Never their names' values.
    pub env_count: usize,
    /// When the package is unpinned and a version of it is in the local
    /// package cache: the spec a one-click pin would write.
    pub pin_to: Option<String>,
    /// The config file this entry was read from. Stays inside the process
    /// (the pin command needs it); never serialized to the UI or a report.
    #[serde(skip)]
    pub source_file: Option<String>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Definition {
    pub name: String,
    /// "user" | "project"
    pub scope: String,
    pub project: Option<String>,
    /// Agents only: the `model` frontmatter field, if set.
    pub model: Option<String>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HookEvent {
    pub event: String,
    pub count: usize,
}

#[derive(Serialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Permissions {
    pub default_mode: Option<String>,
    pub allow: usize,
    pub ask: usize,
    pub deny: usize,
}

#[derive(Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Inventory {
    pub mcp_servers: Vec<McpServer>,
    pub agents: Vec<Definition>,
    pub skills: Vec<Definition>,
    pub hooks: Vec<HookEvent>,
    pub permissions: Permissions,
    /// Default model from Claude Code settings, if pinned.
    pub model: Option<String>,
    /// Projects Claude Code knows about that still exist on disk.
    pub projects: usize,
    /// AI tools found on this machine, by name. Presence only.
    pub tools: Vec<AiTool>,
    /// What this setup suggests learning or tightening next.
    pub opportunities: Vec<Opportunity>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AiTool {
    pub name: String,
    /// "app" (a config folder exists) | "cli" (a command is on PATH)
    pub kind: String,
    /// MCP servers this tool is configured with, across every file read.
    pub mcp_servers: usize,
}

/// One observation about the setup, paired with why it matters. Every one is
/// computed from the inventory above; none is generic advice.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Opportunity {
    /// Stable id, e.g. "mcp-unpinned".
    pub id: String,
    /// "tighten" (a concrete gap) | "learn" (a capability not in use yet)
    pub kind: String,
    /// English, produced by `render("en", &title_msg)` of the same Msg --
    /// never a second literal, so the two can never disagree.
    pub title: String,
    pub detail: String,
    /// The key + vars the popover paints in the active locale.
    pub title_msg: Msg,
    /// `None` only when `detail` is empty; every finding here has a real one.
    pub detail_msg: Option<Msg>,
    /// Where to read more. Opened in the browser only when clicked.
    pub learn_url: Option<String>,
}

/// Every finding id this module can emit, so a test can walk the JSON files
/// and confirm each one has the keys it needs, and that nothing in the JSON
/// claims to be a finding this module never produces.
pub const FINDING_IDS: &[&str] = &[
    "mcp-unpinned",
    "mcp-env-secrets",
    "mcp-remote",
    "perm-none",
    "perm-deny-only",
    "agents-none",
    "agents-model-unset",
    "hooks-none",
];

// ---------------------------------------------------------------------------
// Pure parsers (unit-tested; no filesystem)
// ---------------------------------------------------------------------------

/// Package runners whose first positional argument is the package to run.
const PACKAGE_RUNNERS: &[&str] = &["npx", "bunx", "pnpx", "uvx", "pipx"];

/// A conservative package-spec shape: `name`, `@scope/name`, optional
/// `@version`. Lowercase registry names only, so an opaque token does not
/// pass as a package.
fn looks_like_package(arg: &str) -> bool {
    if arg.is_empty() || arg.len() > 80 || arg.starts_with('-') {
        return false;
    }
    let (name, version) = match arg.strip_prefix('@') {
        Some(rest) => match rest.split_once('@') {
            Some((n, v)) => (format!("@{n}"), Some(v)),
            None => (arg.to_string(), None),
        },
        None => match arg.split_once('@') {
            Some((n, v)) => (n.to_string(), Some(v)),
            None => (arg.to_string(), None),
        },
    };
    let name_ok = {
        let bare = name.strip_prefix('@').unwrap_or(&name);
        let parts: Vec<&str> = bare.split('/').collect();
        let part_ok = |p: &str| {
            !p.is_empty()
                && p.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.')
                })
                && p.bytes().any(|b| b.is_ascii_lowercase())
        };
        match (name.starts_with('@'), parts.as_slice()) {
            (true, [scope, pkg]) => part_ok(scope) && part_ok(pkg),
            (false, [pkg]) => part_ok(pkg),
            _ => false,
        }
    };
    let version_ok = version.is_none_or(|v| {
        !v.is_empty()
            && v.len() <= 24
            && v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'^' | b'~'))
    });
    name_ok && version_ok
}

/// The package a runner launches: its first positional argument, and only
/// when that argument has a registry-name shape.
fn package_of(command: &str, args: Option<&Vec<Value>>) -> Option<String> {
    let launcher = Path::new(command).file_name()?.to_str()?;
    if !PACKAGE_RUNNERS.contains(&launcher) {
        return None;
    }
    let first = args?
        .iter()
        .filter_map(Value::as_str)
        .find(|a| !a.starts_with('-'))?;
    looks_like_package(first).then(|| first.to_string())
}

/// Host of a URL, nothing else: paths and query strings can carry keys.
fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // Drop any `user:pass@` and the port.
    let host = authority.rsplit('@').next().unwrap_or_default();
    let host = host.split(':').next().unwrap_or_default();
    if host.is_empty() {
        "unknown".into()
    } else {
        host.to_ascii_lowercase()
    }
}

/// One `mcpServers` object → entries. `scope`/`project` are stamped on each.
fn mcp_from_map(map: &Value, scope: &str, project: Option<&str>) -> Vec<McpServer> {
    mcp_from_map_for(map, "Claude Code", scope, project)
}

/// Every MCP-capable app keeps the same entry shape (command/args/env, or
/// url/headers), so one reader serves them all and the same never-copy-a-
/// value rule covers them all.
fn mcp_from_map_for(map: &Value, client: &str, scope: &str, project: Option<&str>) -> Vec<McpServer> {
    let Some(map) = map.as_object() else {
        return Vec::new();
    };
    let mut out: Vec<McpServer> = map
        .iter()
        .map(|(name, cfg)| {
            let url = cfg.get("url").and_then(Value::as_str);
            let command = cfg.get("command").and_then(Value::as_str);
            let declared = cfg.get("type").and_then(Value::as_str);
            let transport = match (declared, url) {
                (Some(t @ ("http" | "sse" | "stdio")), _) => t.to_string(),
                (_, Some(_)) => "http".to_string(),
                _ => "stdio".to_string(),
            };
            let (target, package) = match (url, command) {
                (Some(u), _) if transport != "stdio" => (host_of(u), None),
                (_, Some(c)) => (
                    Path::new(c)
                        .file_name()
                        .and_then(|f| f.to_str())
                        .unwrap_or("unknown")
                        .to_string(),
                    package_of(c, cfg.get("args").and_then(Value::as_array)),
                ),
                _ => ("unknown".to_string(), None),
            };
            McpServer {
                name: name.clone(),
                client: client.to_string(),
                scope: scope.to_string(),
                project: project.map(str::to_string),
                transport,
                target,
                package,
                env_count: cfg.get("env").and_then(Value::as_object).map_or(0, |e| e.len()),
                pin_to: None,
                source_file: None,
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Every MCP server in a `~/.claude.json` document: user scope plus each
/// project's own.
pub fn mcp_from_claude_json(doc: &Value) -> Vec<McpServer> {
    let mut out = mcp_from_map(doc.get("mcpServers").unwrap_or(&Value::Null), "user", None);
    if let Some(projects) = doc.get("projects").and_then(Value::as_object) {
        let mut paths: Vec<&String> = projects.keys().collect();
        paths.sort();
        for path in paths {
            let servers = projects[path].get("mcpServers").unwrap_or(&Value::Null);
            out.extend(mcp_from_map(servers, "project", Some(path)));
        }
    }
    out
}

pub fn permissions_from(settings: &Value) -> Permissions {
    let node = settings.get("permissions");
    let count = |key: &str| {
        node.and_then(|n| n.get(key))
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    };
    Permissions {
        default_mode: node
            .and_then(|n| n.get("defaultMode"))
            .and_then(Value::as_str)
            .map(str::to_string),
        allow: count("allow"),
        ask: count("ask"),
        deny: count("deny"),
    }
}

pub fn hooks_from(settings: &Value) -> Vec<HookEvent> {
    let Some(map) = settings.get("hooks").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out: Vec<HookEvent> = map
        .iter()
        .map(|(event, entries)| HookEvent {
            event: event.clone(),
            count: entries.as_array().map_or(0, Vec::len),
        })
        .filter(|h| h.count > 0)
        .collect();
    out.sort_by(|a, b| a.event.cmp(&b.event));
    out
}

const MCP_TRUST_INDEX: &str = "https://staas.fund/mcp/";
const CLASSROOM: &str = "https://staas.fund/classroom/";

fn names(list: &[&McpServer]) -> String {
    let mut n: Vec<String> = list
        .iter()
        .map(|s| match s.client.as_str() {
            "Claude Code" => s.name.clone(),
            other => format!("{} ({other})", s.name),
        })
        .collect();
    n.dedup();
    n.join(", ")
}

/// True when a package spec carries an explicit version (`pkg@1`, `@s/p@^2`).
fn is_pinned(package: &str) -> bool {
    package.trim_start_matches('@').contains('@')
        && !package.ends_with("@latest")
}

/// Observations derived from the inventory. Ordered: gaps first, then
/// capabilities not in use yet.
pub fn opportunities_for(inv: &Inventory) -> Vec<Opportunity> {
    let mut out = Vec::new();
    // English is produced, never duplicated: title and detail are always
    // render("en", …) of the very Msg the popover gets, so the two can never
    // disagree with each other.
    let mut push = |id: &str, kind: &str, title_msg: Msg, detail_msg: Msg, url: Option<&str>| {
        out.push(Opportunity {
            id: id.into(),
            kind: kind.into(),
            title: i18n::render("en", &title_msg),
            detail: i18n::render("en", &detail_msg),
            title_msg,
            detail_msg: Some(detail_msg),
            learn_url: url.map(str::to_string),
        });
    };

    let unpinned: Vec<&McpServer> = inv
        .mcp_servers
        .iter()
        .filter(|s| s.package.as_deref().is_some_and(|p| !is_pinned(p)))
        .collect();
    if !unpinned.is_empty() {
        let n = unpinned.len() as i64;
        push(
            "mcp-unpinned",
            "tighten",
            Msg::new("finding.mcp-unpinned.title").count(n),
            Msg::new("finding.mcp-unpinned.detail").var("names", names(&unpinned)).count(n),
            Some(MCP_TRUST_INDEX),
        );
    }

    let with_env: Vec<&McpServer> = inv.mcp_servers.iter().filter(|s| s.env_count > 0).collect();
    if !with_env.is_empty() {
        let n = with_env.len() as i64;
        push(
            "mcp-env-secrets",
            "tighten",
            Msg::new("finding.mcp-env-secrets.title").count(n),
            Msg::new("finding.mcp-env-secrets.detail").var("names", names(&with_env)).count(n),
            Some(MCP_TRUST_INDEX),
        );
    }

    let remote: Vec<&McpServer> = inv.mcp_servers.iter().filter(|s| s.transport != "stdio").collect();
    if !remote.is_empty() {
        let n = remote.len() as i64;
        push(
            "mcp-remote",
            "learn",
            Msg::new("finding.mcp-remote.title").count(n),
            Msg::new("finding.mcp-remote.detail").var("names", names(&remote)).count(n),
            Some(MCP_TRUST_INDEX),
        );
    }

    let p = &inv.permissions;
    if p.allow == 0 && p.ask == 0 && p.deny == 0 {
        push(
            "perm-none",
            "tighten",
            Msg::new("finding.perm-none.title"),
            Msg::new("finding.perm-none.detail"),
            Some(CLASSROOM),
        );
    } else if p.deny > 0 && p.allow == 0 {
        push(
            "perm-deny-only",
            "learn",
            // Not a plural family: today's sentence always says "rules"
            // (plural) whatever the count, so this is a plain var, not a
            // Msg::count() -- deliberately, to keep the shipped English
            // exactly as it reads today.
            Msg::new("finding.perm-deny-only.title").var("count", p.deny),
            Msg::new("finding.perm-deny-only.detail"),
            Some(CLASSROOM),
        );
    }

    if inv.agents.is_empty() {
        push(
            "agents-none",
            "learn",
            Msg::new("finding.agents-none.title"),
            Msg::new("finding.agents-none.detail"),
            Some(CLASSROOM),
        );
    } else {
        let unset = inv.agents.iter().filter(|a| a.model.is_none()).count();
        if unset > 0 {
            push(
                "agents-model-unset",
                "learn",
                Msg::new("finding.agents-model-unset.title").count(unset as i64),
                Msg::new("finding.agents-model-unset.detail"),
                Some(CLASSROOM),
            );
        }
    }

    if inv.hooks.is_empty() {
        push(
            "hooks-none",
            "learn",
            Msg::new("finding.hooks-none.title"),
            Msg::new("finding.hooks-none.detail"),
            Some(CLASSROOM),
        );
    }
    out
}

/// A single top-level `key: value` from a leading `---` frontmatter block.
fn frontmatter_field(text: &str, key: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        // Top-level keys only: an indented line belongs to a nested value.
        if line.starts_with([' ', '\t']) {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.trim() == key {
                let v = v.trim().trim_matches(['"', '\'']).trim();
                return (!v.is_empty()).then(|| v.to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Filesystem scan
// ---------------------------------------------------------------------------

fn read_capped(path: &Path, cap: u64) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > cap {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&read_capped(path, MAX_CONFIG_BYTES)?).ok()
}

/// Agents are `<dir>/<name>.md`; skills are `<dir>/<name>/SKILL.md` (or a
/// bare `<name>.md`). The name falls back to the file or folder name.
fn definitions_in(dir: &Path, scope: &str, project: Option<&str>, skills: bool) -> Vec<Definition> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        if stem.is_empty() || stem.starts_with('.') {
            continue;
        }
        let file = if path.is_dir() {
            if !skills {
                continue;
            }
            path.join("SKILL.md")
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            path.clone()
        } else {
            continue;
        };
        let Some(text) = read_capped(&file, MAX_DEF_BYTES) else {
            continue;
        };
        out.push(Definition {
            name: frontmatter_field(&text, "name").unwrap_or_else(|| stem.to_string()),
            scope: scope.to_string(),
            project: project.map(str::to_string),
            model: if skills { None } else { frontmatter_field(&text, "model") },
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn claude_dir() -> PathBuf {
    std::env::var("CLAUDE_CONFIG_DIR")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".claude"))
}

/// Where another app keeps its MCP servers: (app name, file, key holding the
/// server map). Paths are relative to the home or app-data folder.
enum Base {
    Home,
    AppData,
}

const OTHER_MCP_FILES: &[(&str, Base, &str, &str)] = &[
    ("Claude Desktop", Base::AppData, "Claude/claude_desktop_config.json", "mcpServers"),
    ("VS Code", Base::AppData, "Code/User/mcp.json", "servers"),
    ("Cursor", Base::Home, ".cursor/mcp.json", "mcpServers"),
    ("Windsurf", Base::Home, ".codeium/windsurf/mcp_config.json", "mcpServers"),
    ("Gemini CLI", Base::Home, ".gemini/settings.json", "mcpServers"),
];

/// Codex keeps its servers in TOML (`[mcp_servers.<name>]`); same fields.
pub fn mcp_from_codex_toml(raw: &str) -> Vec<McpServer> {
    toml::from_str::<toml::Value>(raw)
        .ok()
        .and_then(|doc| serde_json::to_value(doc).ok())
        .map(|doc| mcp_from_map_for(doc.get("mcp_servers").unwrap_or(&Value::Null), "Codex", "user", None))
        .unwrap_or_default()
}

pub fn mcp_from_app_json(doc: &Value, client: &str, key: &str) -> Vec<McpServer> {
    mcp_from_map_for(doc.get(key).unwrap_or(&Value::Null), client, "user", None)
}

fn other_app_servers(home: &Path) -> Vec<McpServer> {
    let app_data = crate::providers::app_data_dir();
    let mut out = Vec::new();
    for (client, base, rel, key) in OTHER_MCP_FILES {
        let root = match base {
            Base::Home => Some(home.to_path_buf()),
            Base::AppData => app_data.clone(),
        };
        let Some(file) = root.map(|r| r.join(rel)) else { continue };
        if let Some(doc) = read_json(&file) {
            let from = Some(file.display().to_string());
            out.extend(mcp_from_app_json(&doc, client, key).into_iter().map(|mut s| {
                s.source_file = from.clone();
                s
            }));
        }
    }
    let codex_home = std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".codex"));
    if let Some(raw) = read_capped(&codex_home.join("config.toml"), MAX_CONFIG_BYTES) {
        out.extend(mcp_from_codex_toml(&raw));
    }
    out
}

/// Known AI tools: (name, config folder under home or app data, command).
const KNOWN_TOOLS: &[(&str, Option<(Base, &str)>, Option<&str>)] = &[
    ("Claude Code", Some((Base::Home, ".claude")), Some("claude")),
    ("Claude Desktop", Some((Base::AppData, "Claude")), None),
    ("Codex", Some((Base::Home, ".codex")), Some("codex")),
    ("Cursor", Some((Base::Home, ".cursor")), Some("cursor-agent")),
    ("Windsurf", Some((Base::Home, ".codeium/windsurf")), None),
    ("VS Code", Some((Base::AppData, "Code/User")), Some("code")),
    ("Gemini CLI", Some((Base::Home, ".gemini")), Some("gemini")),
    ("GitHub Copilot", Some((Base::Home, ".config/github-copilot")), None),
    ("Aider", None, Some("aider")),
    ("OpenCode", Some((Base::Home, ".config/opencode")), Some("opencode")),
    ("Ollama", Some((Base::Home, ".ollama")), Some("ollama")),
    ("Goose", Some((Base::Home, ".config/goose")), Some("goose")),
];

/// Is `command` an executable file in one of `dirs`? Looks, never runs.
fn on_path(command: &str, dirs: &[PathBuf]) -> bool {
    let names: Vec<String> = if cfg!(windows) {
        ["", ".exe", ".cmd", ".bat"].iter().map(|ext| format!("{command}{ext}")).collect()
    } else {
        vec![command.to_string()]
    };
    dirs.iter().any(|d| names.iter().any(|n| d.join(n).is_file()))
}

fn find_tools(home: &Path, path_dirs: &[PathBuf], servers: &[McpServer]) -> Vec<AiTool> {
    let app_data = crate::providers::app_data_dir();
    KNOWN_TOOLS
        .iter()
        .filter_map(|(name, folder, command)| {
            let has_folder = folder.as_ref().is_some_and(|(base, rel)| {
                match base {
                    Base::Home => Some(home.to_path_buf()),
                    Base::AppData => app_data.clone(),
                }
                .is_some_and(|r| r.join(rel).is_dir())
            });
            let has_cli = command.is_some_and(|c| on_path(c, path_dirs));
            (has_folder || has_cli).then(|| AiTool {
                name: name.to_string(),
                kind: if has_folder { "app" } else { "cli" }.to_string(),
                mcp_servers: servers.iter().filter(|s| s.client == *name).count(),
            })
        })
        .collect()
}

/// Project paths Claude Code has on record (`~/.claude.json`), whether or not
/// they still exist. Used to turn a log folder name back into a path.
pub fn known_project_paths() -> Vec<String> {
    let home = dirs::home_dir().unwrap_or_default();
    [claude_dir().join(".claude.json"), home.join(".claude.json")]
        .into_iter()
        .find_map(|p| read_json(&p))
        .and_then(|doc| {
            doc.get("projects")
                .and_then(Value::as_object)
                .map(|projects| projects.keys().cloned().collect())
        })
        .unwrap_or_default()
}

/// Scans this machine. Missing files are normal and simply contribute nothing.
pub fn scan() -> Inventory {
    let home = dirs::home_dir().unwrap_or_default();
    let claude = claude_dir();
    let mut inv = Inventory::default();

    // ~/.claude.json sits beside ~/.claude, or inside a custom config dir.
    let claude_json_path =
        [claude.join(".claude.json"), home.join(".claude.json")].into_iter().find(|p| read_json(p).is_some());
    let claude_json = claude_json_path.as_ref().and_then(|p| read_json(p));
    let mut project_dirs: Vec<String> = Vec::new();
    if let Some(doc) = &claude_json {
        inv.mcp_servers = mcp_from_claude_json(doc);
        let from = claude_json_path.as_ref().map(|p| p.display().to_string());
        inv.mcp_servers.iter_mut().for_each(|s| s.source_file = from.clone());
        if let Some(projects) = doc.get("projects").and_then(Value::as_object) {
            project_dirs = projects
                .keys()
                .filter(|p| Path::new(p).is_dir())
                .cloned()
                .collect();
            project_dirs.sort();
        }
    }
    inv.projects = project_dirs.len();

    if let Some(settings) = read_json(&claude.join("settings.json")) {
        inv.permissions = permissions_from(&settings);
        inv.hooks = hooks_from(&settings);
        inv.model = settings.get("model").and_then(Value::as_str).map(str::to_string);
    }

    inv.agents = definitions_in(&claude.join("agents"), "user", None, false);
    inv.skills = definitions_in(&claude.join("skills"), "user", None, true);
    for project in &project_dirs {
        let root = Path::new(project);
        // A project at $HOME would rescan the user scope.
        if root.join(".claude") == claude {
            continue;
        }
        let p = Some(project.as_str());
        inv.agents.extend(definitions_in(&root.join(".claude/agents"), "project", p, false));
        inv.skills.extend(definitions_in(&root.join(".claude/skills"), "project", p, true));
        if let Some(doc) = read_json(&root.join(".mcp.json")) {
            let servers = doc.get("mcpServers").unwrap_or(&Value::Null);
            let from = Some(root.join(".mcp.json").display().to_string());
            inv.mcp_servers.extend(mcp_from_map(servers, "project", p).into_iter().map(|mut s| {
                s.source_file = from.clone();
                s
            }));
        }
    }
    inv.mcp_servers.extend(other_app_servers(&home));
    for server in &mut inv.mcp_servers {
        let Some(package) = server.package.as_deref().filter(|p| !is_pinned(p)) else { continue };
        server.pin_to = crate::pin::installed_version(&server.target, package)
            .and_then(|v| crate::pin::pin_spec(&server.target, package, &v));
    }
    let path_dirs: Vec<PathBuf> =
        std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    inv.tools = find_tools(&home, &path_dirs, &inv.mcp_servers);
    inv.opportunities = opportunities_for(&inv);
    inv
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SECRETS: &[&str] = &[
        "sk-live-SECRETENVVALUE",
        "ghp_SECRETARGTOKEN",
        "SECRETPATHKEY",
        "SECRETQUERYKEY",
        "hunter2",
        "SECRETHEADER",
    ];

    fn hostile_doc() -> Value {
        json!({
            "mcpServers": {
                "with-env": {
                    "command": "/usr/local/bin/npx",
                    "args": ["-y", "@scope/server-thing@1.2.3", "--token", "ghp_SECRETARGTOKEN"],
                    "env": { "API_KEY": "sk-live-SECRETENVVALUE", "OTHER": "hunter2" }
                },
                "remote": {
                    "type": "http",
                    "url": "https://user:hunter2@mcp.example.com:8443/v1/SECRETPATHKEY/mcp?key=SECRETQUERYKEY",
                    "headers": { "Authorization": "Bearer SECRETHEADER" }
                },
                "token-as-first-arg": {
                    "command": "npx",
                    "args": ["ghp_SECRETARGTOKEN"]
                }
            },
            "projects": {
                "/work/acme": { "mcpServers": {
                    "proj": { "command": "python", "args": ["server.py", "--key=SECRETQUERYKEY"] }
                }}
            }
        })
    }

    #[test]
    fn never_leaks_secret_values_from_any_field() {
        let servers = mcp_from_claude_json(&hostile_doc());
        let wire = serde_json::to_string(&servers).unwrap();
        for secret in SECRETS {
            assert!(!wire.contains(secret), "{secret} leaked into {wire}");
        }
    }

    #[test]
    fn reports_names_shapes_and_counts() {
        let servers = mcp_from_claude_json(&hostile_doc());
        let by = |n: &str| servers.iter().find(|s| s.name == n).unwrap().clone();

        let env = by("with-env");
        assert_eq!(env.transport, "stdio");
        assert_eq!(env.target, "npx");
        assert_eq!(env.package.as_deref(), Some("@scope/server-thing@1.2.3"));
        assert_eq!(env.env_count, 2);
        assert_eq!(env.scope, "user");

        let remote = by("remote");
        assert_eq!(remote.transport, "http");
        assert_eq!(remote.target, "mcp.example.com");
        assert_eq!(remote.package, None);

        // An opaque token in the package slot is not reported as a package.
        assert_eq!(by("token-as-first-arg").package, None);

        let proj = by("proj");
        assert_eq!(proj.scope, "project");
        assert_eq!(proj.project.as_deref(), Some("/work/acme"));
        assert_eq!(proj.target, "python");
        assert_eq!(proj.package, None, "python is not a package runner");
    }

    #[test]
    fn package_shape_is_conservative() {
        for ok in ["mcp-server", "@scope/pkg", "pkg@latest", "@scope/pkg@^1.2.3", "blender-mcp"] {
            assert!(looks_like_package(ok), "{ok}");
        }
        for bad in ["", "-y", "ghp_ABCdef123", "UPPER", "a/b/c", "@scope", "12345", "pkg@", "with space"] {
            assert!(!looks_like_package(bad), "{bad}");
        }
    }

    #[test]
    fn host_only_from_urls() {
        assert_eq!(host_of("https://api.example.com/x/y?k=v"), "api.example.com");
        assert_eq!(host_of("http://u:p@Example.COM:9000"), "example.com");
        assert_eq!(host_of("example.com/path"), "example.com");
        assert_eq!(host_of(""), "unknown");
    }

    #[test]
    fn permissions_and_hooks_are_counts() {
        let settings = json!({
            "permissions": { "defaultMode": "acceptEdits", "deny": ["a", "b", "c"], "allow": ["x"] },
            "hooks": { "SessionEnd": [{}], "PreCompact": [{}, {}], "Empty": [] }
        });
        assert_eq!(
            permissions_from(&settings),
            Permissions { default_mode: Some("acceptEdits".into()), allow: 1, ask: 0, deny: 3 }
        );
        assert_eq!(
            hooks_from(&settings),
            vec![
                HookEvent { event: "PreCompact".into(), count: 2 },
                HookEvent { event: "SessionEnd".into(), count: 1 },
            ]
        );
        assert_eq!(permissions_from(&json!({})), Permissions::default());
    }

    #[test]
    fn frontmatter_reads_top_level_keys_only() {
        let text = "---\nname: \"reviewer\"\nmodel: opus\nmetadata:\n  name: nested\n---\nname: body\n";
        assert_eq!(frontmatter_field(text, "name").as_deref(), Some("reviewer"));
        assert_eq!(frontmatter_field(text, "model").as_deref(), Some("opus"));
        assert_eq!(frontmatter_field(text, "tools"), None);
        assert_eq!(frontmatter_field("no frontmatter\nname: x", "name"), None);
    }

    fn server(name: &str, package: Option<&str>, transport: &str, env_count: usize) -> McpServer {
        McpServer {
            name: name.into(),
            client: "Claude Code".into(),
            scope: "user".into(),
            project: None,
            transport: transport.into(),
            target: "npx".into(),
            package: package.map(str::to_string),
            env_count,
            pin_to: None,
            source_file: None,
        }
    }

    fn ids(inv: &Inventory) -> Vec<String> {
        opportunities_for(inv).into_iter().map(|o| o.id).collect()
    }

    #[test]
    fn other_apps_are_read_with_the_same_no_values_rule() {
        let desktop = json!({"mcpServers": {"files": {
            "command": "npx", "args": ["-y", "some-files-mcp", "--token", "ghp_SECRETARGTOKEN"],
            "env": {"KEY": "sk-live-SECRETENVVALUE"}}}});
        let vscode = json!({"servers": {"remote": {"type": "http",
            "url": "https://mcp.example.com/SECRETPATHKEY?key=SECRETQUERYKEY",
            "headers": {"Authorization": "Bearer SECRETHEADER"}}}});
        let codex = r#"
            model = "whatever"
            [mcp_servers.docs]
            command = "uvx"
            args = ["docs-mcp@2", "--api-key", "hunter2"]
            [mcp_servers.docs.env]
            TOKEN = "sk-live-SECRETENVVALUE"
        "#;
        let mut all = mcp_from_app_json(&desktop, "Claude Desktop", "mcpServers");
        all.extend(mcp_from_app_json(&vscode, "VS Code", "servers"));
        all.extend(mcp_from_codex_toml(codex));
        let wire = serde_json::to_string(&all).unwrap();
        for secret in SECRETS {
            assert!(!wire.contains(secret), "{secret} leaked into {wire}");
        }
        let by = |n: &str| all.iter().find(|s| s.name == n).unwrap().clone();
        assert_eq!((by("files").client.as_str(), by("files").package.as_deref(), by("files").env_count),
                   ("Claude Desktop", Some("some-files-mcp"), 1));
        assert_eq!((by("remote").client.as_str(), by("remote").target.as_str()), ("VS Code", "mcp.example.com"));
        assert_eq!((by("docs").client.as_str(), by("docs").package.as_deref(), by("docs").env_count),
                   ("Codex", Some("docs-mcp@2"), 1));
        assert!(mcp_from_codex_toml("this is [not toml").is_empty());
        assert!(mcp_from_app_json(&json!({}), "Cursor", "mcpServers").is_empty());
    }

    #[test]
    fn tools_are_found_by_a_folder_or_a_command_without_running_anything() {
        let root = std::env::temp_dir().join(format!("aitm-tools-{}", crate::providers::unique_stamp()));
        let home = root.join("home");
        let bin = root.join("bin");
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join(if cfg!(windows) { "aider.exe" } else { "aider" }), "").unwrap();
        std::fs::create_dir_all(bin.join("ollama")).unwrap(); // a folder is not a command
        let servers = vec![McpServer { client: "Codex".into(), ..server("docs", None, "stdio", 0) }];
        let found = find_tools(&home, &[bin.clone()], &servers);
        let by = |n: &str| found.iter().find(|t| t.name == n).cloned();
        assert_eq!(by("Codex"), Some(AiTool { name: "Codex".into(), kind: "app".into(), mcp_servers: 1 }));
        assert_eq!(by("Aider"), Some(AiTool { name: "Aider".into(), kind: "cli".into(), mcp_servers: 0 }));
        assert_eq!(by("Ollama"), None);
        assert_eq!(by("Cursor"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn findings_name_the_app_when_it_is_not_claude_code() {
        let inv = Inventory {
            mcp_servers: vec![
                server("local", Some("a-mcp"), "stdio", 0),
                McpServer { client: "Claude Desktop".into(), ..server("files", Some("b-mcp"), "stdio", 0) },
            ],
            ..Inventory::default()
        };
        let found = opportunities_for(&inv);
        assert!(found[0].detail.starts_with("local, files (Claude Desktop) fetch "), "{}", found[0].detail);
    }

    #[test]
    fn pinning_is_read_from_the_package_spec() {
        assert!(is_pinned("pkg@1"));
        assert!(is_pinned("@scope/pkg@^2.1.0"));
        assert!(!is_pinned("pkg"));
        assert!(!is_pinned("@scope/pkg"), "a scope is not a version");
        assert!(!is_pinned("pkg@latest"), "latest floats");
    }

    #[test]
    fn opportunities_come_from_the_inventory_not_from_a_script() {
        // A tidy setup earns no findings at all.
        let tidy = Inventory {
            mcp_servers: vec![server("a", Some("@s/a@1"), "stdio", 0)],
            agents: vec![Definition { name: "r".into(), scope: "user".into(), project: None, model: Some("haiku".into()) }],
            hooks: vec![HookEvent { event: "SessionEnd".into(), count: 1 }],
            permissions: Permissions { default_mode: None, allow: 4, ask: 0, deny: 9 },
            ..Inventory::default()
        };
        assert!(ids(&tidy).is_empty(), "{:?}", ids(&tidy));

        let loose = Inventory {
            mcp_servers: vec![
                server("floaty", Some("some-mcp"), "stdio", 2),
                server("cloud", None, "http", 0),
            ],
            ..Inventory::default()
        };
        assert_eq!(
            ids(&loose),
            ["mcp-unpinned", "mcp-env-secrets", "mcp-remote", "perm-none", "agents-none", "hooks-none"]
        );
        let unpinned = &opportunities_for(&loose)[0];
        assert!(unpinned.detail.contains("floaty"), "names the server: {}", unpinned.detail);
        assert!(!unpinned.detail.contains("cloud"));
    }

    /// A missing translation key renders as its own literal key text instead
    /// of failing -- that is the silent failure mode `render()` is built to
    /// have, so it takes a real fixture run to catch it. `loose` above
    /// already covers six of the eight finding ids; the other two
    /// (perm-deny-only, agents-model-unset) need their own inventory since
    /// each is mutually exclusive with the finding `loose` triggers in the
    /// same slot.
    #[test]
    fn opportunities_never_render_a_raw_key() {
        let loose = Inventory {
            mcp_servers: vec![server("floaty", Some("some-mcp"), "stdio", 2), server("cloud", None, "http", 0)],
            ..Inventory::default()
        };
        let other = Inventory {
            permissions: Permissions { default_mode: None, allow: 0, ask: 0, deny: 3 },
            agents: vec![Definition { name: "r".into(), scope: "user".into(), project: None, model: None }],
            hooks: vec![HookEvent { event: "SessionEnd".into(), count: 1 }],
            ..Inventory::default()
        };
        assert_eq!(ids(&other), ["perm-deny-only", "agents-model-unset"]);
        for inv in [&loose, &other] {
            for o in opportunities_for(inv) {
                assert!(!o.title.starts_with("finding."), "{}: raw key in title: {}", o.id, o.title);
                assert!(!o.detail.starts_with("finding."), "{}: raw key in detail: {}", o.id, o.detail);
            }
        }
    }

    #[test]
    fn wording_agrees_with_the_count() {
        let one = Inventory {
            mcp_servers: vec![server("solo", Some("some-mcp"), "stdio", 1)],
            ..Inventory::default()
        };
        let found = opportunities_for(&one);
        assert_eq!(found[0].title, "1 MCP server runs an unpinned package");
        assert!(found[0].detail.starts_with("solo fetches "), "{}", found[0].detail);
        assert_eq!(found[1].title, "1 MCP server is handed credentials");

        let two = Inventory {
            mcp_servers: vec![
                server("a", Some("a-mcp"), "stdio", 0),
                server("b", Some("b-mcp"), "stdio", 0),
            ],
            ..Inventory::default()
        };
        let found = opportunities_for(&two);
        assert_eq!(found[0].title, "2 MCP servers run an unpinned package");
        assert!(found[0].detail.starts_with("a, b fetch "), "{}", found[0].detail);
    }

    #[test]
    fn deny_only_posture_is_a_learning_note_not_a_gap() {
        let inv = Inventory {
            permissions: Permissions { default_mode: None, allow: 0, ask: 0, deny: 23 },
            ..Inventory::default()
        };
        let found = opportunities_for(&inv);
        let perm = found.iter().find(|o| o.id.starts_with("perm-")).unwrap();
        assert_eq!(perm.id, "perm-deny-only");
        assert_eq!(perm.kind, "learn");
        assert!(perm.title.contains("23"));
    }

    /// Prints this machine's real inventory. `cargo test live_scan -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_scan() {
        println!("{}", serde_json::to_string_pretty(&scan()).unwrap());
    }

    #[test]
    fn scans_agents_and_skills_from_disk() {
        let root = std::env::temp_dir().join(format!("aitm-inv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("agents")).unwrap();
        std::fs::create_dir_all(root.join("skills/deploy")).unwrap();
        std::fs::write(root.join("agents/cooper.md"), "---\nname: cooper\nmodel: sonnet\n---\nhi").unwrap();
        std::fs::write(root.join("agents/notes.txt"), "ignored").unwrap();
        std::fs::write(root.join("skills/deploy/SKILL.md"), "no frontmatter").unwrap();

        let agents = definitions_in(&root.join("agents"), "user", None, false);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "cooper");
        assert_eq!(agents[0].model.as_deref(), Some("sonnet"));

        let skills = definitions_in(&root.join("skills"), "project", Some("/p"), true);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "deploy", "falls back to the folder name");
        assert_eq!(skills[0].project.as_deref(), Some("/p"));

        let _ = std::fs::remove_dir_all(&root);
    }
}
