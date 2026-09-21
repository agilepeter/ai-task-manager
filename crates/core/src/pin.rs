//! Pinning an unpinned MCP server: turn the "runs an unpinned package"
//! finding into a fix.
//!
//! This is the only code in the app that writes to a file another tool owns
//! (Claude Code's or another app's MCP config), so it is built to be boring:
//!
//! - The version comes from what is already installed in the local package
//!   cache. No registry is asked, so nothing leaves the machine and the pin
//!   is a version that has actually run here.
//! - The edit is textual and minimal: the one JSON string holding the package
//!   spec is replaced, byte for byte, everything else untouched. Re-encoding
//!   the file would reorder keys and rewrite formatting in a config that the
//!   owning tool rewrites constantly.
//! - Before anything is written the result is parsed and compared with the
//!   original: the ONLY permitted difference is that string, inside an `args`
//!   array. Anything else aborts.
//! - The user sees the exact before and after first. Applying re-checks the
//!   file's size and modification time against the preview; if the owning
//!   tool wrote to it in between, the apply is refused.
//! - A backup of the original is saved beside the file.

use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PinPlan {
    pub file: String,
    pub package: String,
    pub installed_version: String,
    pub from: String,
    pub to: String,
    /// How many places in the file hold this spec (user and project scope can both).
    pub occurrences: usize,
    /// The file's size and modification time when the plan was made.
    pub file_len: u64,
    pub file_mtime_ms: i64,
}

/// The parts of a plan the UI sends back to say "this is what I was shown".
#[derive(serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PinPlanSeen {
    pub file: String,
    pub to: String,
    pub file_len: u64,
    pub file_mtime_ms: i64,
}

// ---------------------------------------------------------------------------
// Version discovery (local caches only)
// ---------------------------------------------------------------------------

/// "pkg@latest" → "pkg"; "@scope/pkg" → "@scope/pkg".
pub fn bare(spec: &str) -> &str {
    let at = if let Some(rest) = spec.strip_prefix('@') { rest.find('@').map(|i| i + 1) } else { spec.find('@') };
    at.map_or(spec, |i| &spec[..i])
}

fn version_key(v: &str) -> Vec<u64> {
    v.split(['.', '-', '+']).map(|p| p.parse::<u64>().unwrap_or(0)).collect()
}

/// A plain release version: digits and dots. Pre-releases and odd strings
/// are not pinned to.
fn is_release(v: &str) -> bool {
    !v.is_empty() && v.len() <= 32 && v.split('.').count() >= 2 && v.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Highest version of `package` in an npx cache (`<cache>/_npx/*/node_modules`).
pub fn npx_cached_version(npm_cache: &Path, package: &str) -> Option<String> {
    let mut best: Option<String> = None;
    for entry in std::fs::read_dir(npm_cache.join("_npx")).ok()?.flatten().take(2_000) {
        let manifest = entry.path().join("node_modules").join(package).join("package.json");
        let Some(v) = std::fs::read_to_string(&manifest)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|doc| doc.get("version").and_then(Value::as_str).map(str::to_string))
            .filter(|v| is_release(v))
        else {
            continue;
        };
        if best.as_deref().is_none_or(|b| version_key(&v) > version_key(b)) {
            best = Some(v);
        }
    }
    best
}

/// Highest version of `package` in a uv cache (`archive-v0/*/lib/python*/site-packages/<name>-<ver>.dist-info`).
pub fn uv_cached_version(uv_cache: &Path, package: &str) -> Option<String> {
    let wanted = package.to_ascii_lowercase().replace(['-', '.'], "_");
    let mut best: Option<String> = None;
    for archive in std::fs::read_dir(uv_cache.join("archive-v0")).ok()?.flatten().take(5_000) {
        let Ok(libs) = std::fs::read_dir(archive.path().join("lib")) else { continue };
        for py in libs.flatten() {
            let Ok(pkgs) = std::fs::read_dir(py.path().join("site-packages")) else { continue };
            for p in pkgs.flatten() {
                let name = p.file_name().to_string_lossy().to_ascii_lowercase();
                let Some(stem) = name.strip_suffix(".dist-info") else { continue };
                let Some((n, v)) = stem.rsplit_once('-') else { continue };
                if n == wanted && is_release(v) && best.as_deref().is_none_or(|b| version_key(v) > version_key(b)) {
                    best = Some(v.to_string());
                }
            }
        }
    }
    best
}

/// The spec to pin to. npm runners get the major (`pkg@1`): patch and minor
/// updates still flow, a breaking or hijacked major does not. uvx has no
/// short form for a range, so it gets the exact version.
pub fn pin_spec(launcher: &str, package: &str, version: &str) -> Option<String> {
    let name = bare(package);
    match launcher {
        "npx" | "bunx" | "pnpx" => Some(format!("{name}@{}", version.split('.').next()?)),
        "uvx" | "pipx" => Some(format!("{name}@{version}")),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The edit
// ---------------------------------------------------------------------------

/// Every difference between `a` and `b` must be the string `from` → `to`
/// sitting directly in an array under an `args` key.
fn only_args_changed(a: &Value, b: &Value, from: &str, to: &str, in_args: bool) -> bool {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| only_args_changed(v, w, from, to, k == "args")))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(v, w)| match (v, w) {
                (Value::String(s), Value::String(t)) if s != t => in_args && s == from && t == to,
                _ => only_args_changed(v, w, from, to, false),
            })
        }
        _ => a == b,
    }
}

/// The new file text, or why not. Pure: no filesystem.
pub fn rewrite(raw: &str, from: &str, to: &str) -> Result<(String, usize), String> {
    if from == to {
        return Err("already pinned".into());
    }
    let before: Value = serde_json::from_str(raw).map_err(|_| "the config is not valid JSON; leaving it alone")?;
    // The spec exactly as JSON writes it, quotes and escapes included.
    let token = serde_json::to_string(from).map_err(|e| e.to_string())?;
    let replacement = serde_json::to_string(to).map_err(|e| e.to_string())?;
    let occurrences = raw.matches(&token).count();
    if occurrences == 0 {
        return Err("that package spec is not in this file any more".into());
    }
    let next = raw.replace(&token, &replacement);
    let after: Value = serde_json::from_str(&next).map_err(|_| "the edit would break the file; nothing was written")?;
    if !only_args_changed(&before, &after, from, to, false) {
        return Err("the same text appears outside a server's arguments; this needs a manual edit".into());
    }
    Ok((next, occurrences))
}

fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

pub fn plan(file: &Path, launcher: &str, package: &str, installed: &str) -> Result<PinPlan, String> {
    let meta = std::fs::metadata(file).map_err(|e| format!("read {}: {e}", file.display()))?;
    if meta.len() > MAX_CONFIG_BYTES {
        return Err("that config file is too large to edit safely".into());
    }
    let raw = std::fs::read_to_string(file).map_err(|e| format!("read {}: {e}", file.display()))?;
    let to = pin_spec(launcher, package, installed).ok_or("this launcher has no pin syntax the app knows")?;
    let (_, occurrences) = rewrite(&raw, package, &to)?;
    Ok(PinPlan {
        file: file.display().to_string(),
        package: bare(package).to_string(),
        installed_version: installed.to_string(),
        from: package.to_string(),
        to,
        occurrences,
        file_len: meta.len(),
        file_mtime_ms: mtime_ms(&meta),
    })
}

/// Applies a plan the user has seen. Returns the backup's path.
pub fn apply(plan: &PinPlan) -> Result<String, String> {
    let file = PathBuf::from(&plan.file);
    let meta = std::fs::metadata(&file).map_err(|e| format!("read {}: {e}", file.display()))?;
    if meta.len() != plan.file_len || mtime_ms(&meta) != plan.file_mtime_ms {
        return Err("the file changed since the preview (its owner wrote to it). Preview again.".into());
    }
    let raw = std::fs::read_to_string(&file).map_err(|e| format!("read {}: {e}", file.display()))?;
    let (next, _) = rewrite(&raw, &plan.from, &plan.to)?;
    let backup = PathBuf::from(format!("{}.aitm-backup-{}", plan.file, crate::providers::unique_stamp()));
    std::fs::copy(&file, &backup).map_err(|e| format!("save a backup: {e}"))?;
    // Same folder, so the rename is atomic; permissions carried over first.
    let tmp = PathBuf::from(format!("{}.aitm-tmp-{}", plan.file, crate::providers::unique_stamp()));
    std::fs::write(&tmp, next).map_err(|e| format!("write the new file: {e}"))?;
    let _ = std::fs::set_permissions(&tmp, meta.permissions());
    std::fs::rename(&tmp, &file).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("replace the file: {e}")
    })?;
    Ok(backup.display().to_string())
}

/// Default cache locations. Environment overrides first, as the tools do.
pub fn npm_cache_dir() -> Option<PathBuf> {
    std::env::var_os("npm_config_cache").map(PathBuf::from).or_else(|| {
        if cfg!(windows) {
            std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("npm-cache"))
        } else {
            dirs::home_dir().map(|h| h.join(".npm"))
        }
    })
}

pub fn uv_cache_dir() -> Option<PathBuf> {
    std::env::var_os("UV_CACHE_DIR").map(PathBuf::from).or_else(|| {
        if cfg!(windows) {
            std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("uv").join("cache"))
        } else {
            dirs::cache_dir().map(|c| c.join("uv")).filter(|p| p.is_dir()).or_else(|| dirs::home_dir().map(|h| h.join(".cache").join("uv")))
        }
    })
}

pub fn installed_version(launcher: &str, package: &str) -> Option<String> {
    match launcher {
        "npx" | "bunx" | "pnpx" => npx_cached_version(&npm_cache_dir()?, bare(package)),
        "uvx" | "pipx" => uv_cached_version(&uv_cache_dir()?, bare(package)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"{
  "numStartups": 412,
  "mcpServers": {
    "chrome-devtools": { "type": "stdio", "command": "npx", "args": ["-y", "chrome-devtools-mcp@latest"], "env": { "K": "v" } },
    "blender":   {"command":"uvx","args":["blender-mcp"]}
  },
  "projects": {
    "/w": { "mcpServers": { "cd2": { "command": "npx", "args": ["chrome-devtools-mcp@latest", "--flag"] } },
            "history": ["please run chrome-devtools-mcp for me"] }
  },
  "zLast": true
}"#;

    #[test]
    fn only_the_spec_changes_and_every_other_byte_survives() {
        let (next, n) = rewrite(CONFIG, "chrome-devtools-mcp@latest", "chrome-devtools-mcp@1").unwrap();
        assert_eq!(n, 2, "user scope and project scope both held it");
        assert_eq!(next, CONFIG.replace("chrome-devtools-mcp@latest", "chrome-devtools-mcp@1"));
        assert!(next.contains("\"numStartups\": 412") && next.contains("\"blender\":   {\"command\""), "formatting untouched");
        assert!(next.contains("please run chrome-devtools-mcp for me"), "prose that merely mentions the package is untouched");
    }

    #[test]
    fn it_refuses_when_the_same_text_lives_outside_a_servers_arguments() {
        let tricky = r#"{"mcpServers":{"b":{"command":"uvx","args":["blender-mcp"]}},"notes":["blender-mcp"]}"#;
        let err = rewrite(tricky, "blender-mcp", "blender-mcp@1.6.4").unwrap_err();
        assert!(err.contains("manual edit"), "{err}");
        let as_key = r#"{"mcpServers":{"blender-mcp":{"command":"uvx","args":["blender-mcp"]}}}"#;
        assert!(rewrite(as_key, "blender-mcp", "blender-mcp@1.6.4").is_err(), "a server NAMED like its package");
        assert!(rewrite("{not json", "a", "a@1").is_err());
        assert!(rewrite(CONFIG, "gone-mcp", "gone-mcp@1").unwrap_err().contains("not in this file"));
        assert!(rewrite(CONFIG, "x@1", "x@1").is_err());
    }

    #[test]
    fn npm_pins_the_major_and_uvx_the_exact_version() {
        assert_eq!(pin_spec("npx", "chrome-devtools-mcp@latest", "1.9.0").as_deref(), Some("chrome-devtools-mcp@1"));
        assert_eq!(pin_spec("npx", "@scope/thing", "12.0.3").as_deref(), Some("@scope/thing@12"));
        assert_eq!(pin_spec("uvx", "blender-mcp", "1.6.4").as_deref(), Some("blender-mcp@1.6.4"));
        assert_eq!(pin_spec("python", "x", "1.0.0"), None);
        assert_eq!(bare("@scope/thing@^2"), "@scope/thing");
        assert_eq!(bare("plain"), "plain");
    }

    #[test]
    fn versions_come_from_the_local_caches_highest_release_wins() {
        let root = std::env::temp_dir().join(format!("aitm-pin-{}", crate::providers::unique_stamp()));
        for (hash, ver) in [("aaa", "1.2.0"), ("bbb", "1.10.0"), ("ccc", "2.0.0-beta.1")] {
            let dir = root.join("npm/_npx").join(hash).join("node_modules/some-mcp");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("package.json"), format!(r#"{{"name":"some-mcp","version":"{ver}"}}"#)).unwrap();
        }
        assert_eq!(npx_cached_version(&root.join("npm"), "some-mcp").as_deref(), Some("1.10.0"), "1.10 beats 1.2; the beta is ignored");
        assert_eq!(npx_cached_version(&root.join("npm"), "other-mcp"), None);

        for (hash, dist) in [("x", "blender_mcp-1.5.6.dist-info"), ("y", "Blender_MCP-1.6.4.dist-info"), ("z", "blender_mcp_extras-9.9.9.dist-info")] {
            std::fs::create_dir_all(root.join("uv/archive-v0").join(hash).join("lib/python3.12/site-packages").join(dist)).unwrap();
        }
        assert_eq!(uv_cached_version(&root.join("uv"), "blender-mcp").as_deref(), Some("1.6.4"), "name normalised, a longer name is not a match");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Previews every pin this machine's real configs would get. READS ONLY.
    #[test]
    #[ignore]
    fn live_preview() {
        for s in crate::inventory::scan().mcp_servers.iter().filter(|s| s.pin_to.is_some()) {
            let package = s.package.as_deref().unwrap();
            let installed = installed_version(&s.target, package).unwrap();
            match plan(Path::new(s.source_file.as_deref().unwrap()), &s.target, package, &installed) {
                Ok(p) => println!("{} ({}): {} -> {}  [installed {}, {} place(s), file …/{}]", s.name, s.client, p.from, p.to,
                    p.installed_version, p.occurrences, Path::new(&p.file).file_name().unwrap().to_string_lossy()),
                Err(e) => println!("{} ({}): REFUSED: {e}", s.name, s.client),
            }
        }
    }

    #[test]
    fn apply_backs_up_writes_once_and_refuses_a_file_that_moved_on() {
        let dir = std::env::temp_dir().join(format!("aitm-pin-apply-{}", crate::providers::unique_stamp()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("claude.json");
        std::fs::write(&file, CONFIG).unwrap();
        let p = plan(&file, "npx", "chrome-devtools-mcp@latest", "1.9.0").unwrap();
        assert_eq!((p.to.as_str(), p.occurrences), ("chrome-devtools-mcp@1", 2));

        let backup = apply(&p).unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), CONFIG, "the backup is the original");
        let now = std::fs::read_to_string(&file).unwrap();
        assert!(now.contains("chrome-devtools-mcp@1\"") && !now.contains("@latest"));
        assert!(std::fs::read_dir(&dir).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().contains("aitm-tmp")));

        // The same plan again: the file is no longer what was previewed.
        assert!(apply(&p).unwrap_err().contains("changed since the preview"));
        // A fresh plan on the already pinned file has nothing to do.
        assert!(plan(&file, "npx", "chrome-devtools-mcp@latest", "1.9.0").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
