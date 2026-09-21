//! Optional trust ratings for the MCP servers found on this machine, from the
//! public MCP Trust Index.
//!
//! This is the one place the app talks to a server that is not an AI
//! provider's own API, so it is **off unless the user turns it on**, and it
//! is built to tell that server nothing: the whole public list is downloaded
//! with a plain GET (no parameters, no body, no identifying header) at most
//! once a day, cached, and matched against the local inventory *here*. Which
//! servers a user runs never leaves the machine; the only thing the index's
//! host can see is that someone fetched the list.

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const INDEX_URL: &str = "https://staas.fund/mcp/scanner/servers.json";
pub const INDEX_PAGE: &str = "https://staas.fund/mcp/";
const MAX_INDEX_BYTES: usize = 1024 * 1024;
const MAX_ENTRIES: usize = 2_000;
const MAX_TEXT: usize = 200;
/// The list changes about monthly; a day keeps it fresh without chatter.
const REFRESH_AFTER_MS: i64 = 24 * 3_600_000;

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub tier: String,
    pub score: f64,
    #[serde(default)]
    pub npm_package: Option<String>,
    #[serde(default)]
    pub pypi_package: Option<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct CacheFile {
    fetched_at: i64,
    entries: Vec<Entry>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Rating {
    /// The package as configured locally, version and all.
    pub package: String,
    /// The index's name for it, when listed.
    pub listed_as: Option<String>,
    pub tier: Option<String>,
    pub score: Option<f64>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct TrustView {
    pub enabled: bool,
    pub fetched_at: Option<i64>,
    pub listed: usize,
    pub ratings: Vec<Rating>,
    /// Set when the list could not be fetched and there is no cache.
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Pure
// ---------------------------------------------------------------------------

/// The list as published. Oversized or odd input yields what is usable, not
/// an error: a rating is a convenience and must never break the Inventory tab.
pub fn parse_index(raw: &str) -> Vec<Entry> {
    if raw.len() > MAX_INDEX_BYTES {
        return Vec::new();
    }
    serde_json::from_str::<Vec<serde_json::Value>>(raw)
        .unwrap_or_default()
        .into_iter()
        .take(MAX_ENTRIES)
        .filter_map(|v| serde_json::from_value::<Entry>(v).ok())
        .filter(|e| {
            e.score.is_finite()
                && !e.name.is_empty()
                && [Some(&e.name), Some(&e.tier), e.npm_package.as_ref(), e.pypi_package.as_ref()]
                    .into_iter()
                    .flatten()
                    .all(|s| s.len() <= MAX_TEXT)
        })
        .collect()
}

/// "pkg@1.2" → "pkg", "@scope/pkg@^2" → "@scope/pkg", lowercased.
pub fn package_name(spec: &str) -> String {
    let spec = spec.trim();
    let name = match spec.strip_prefix('@') {
        Some(rest) => match rest.split_once('@') {
            Some((n, _)) => format!("@{n}"),
            None => spec.to_string(),
        },
        None => spec.split_once('@').map_or(spec, |(n, _)| n).to_string(),
    };
    name.to_ascii_lowercase()
}

/// One rating per distinct local package, in the order given. A package not
/// on the list gets a row with no tier: "not rated" is itself the finding.
pub fn rate(packages: &[String], index: &[Entry]) -> Vec<Rating> {
    let mut seen = std::collections::HashSet::new();
    packages
        .iter()
        .filter(|p| seen.insert(package_name(p)))
        .map(|package| {
            let want = package_name(package);
            let hit = index.iter().find(|e| {
                [e.npm_package.as_deref(), e.pypi_package.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|listed| listed.eq_ignore_ascii_case(&want))
            });
            Rating {
                package: package.clone(),
                listed_as: hit.map(|e| e.name.clone()),
                tier: hit.map(|e| e.tier.clone()),
                score: hit.map(|e| e.score),
            }
        })
        .collect()
}

fn is_fresh(fetched_at: i64, now: i64) -> bool {
    fetched_at > 0 && now >= fetched_at && now - fetched_at < REFRESH_AFTER_MS
}

// ---------------------------------------------------------------------------
// Cache + fetch
// ---------------------------------------------------------------------------

fn load_cache(path: &Path) -> Option<CacheFile> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn save_cache(path: &Path, cache: &CacheFile) {
    if let (Some(dir), Ok(body)) = (path.parent(), serde_json::to_string(cache)) {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(path, body);
    }
}

pub fn cache_path() -> std::path::PathBuf {
    crate::providers::config_dir().join("trust_index.json")
}

/// A plain GET of the public list. Nothing about this machine goes with it.
async fn fetch() -> Result<Vec<Entry>, String> {
    let resp = crate::providers::http()
        .get(INDEX_URL)
        .send()
        .await
        .map_err(|e| format!("fetch the trust index: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("trust index: HTTP {}", resp.status()));
    }
    if resp.content_length().is_some_and(|n| n > MAX_INDEX_BYTES as u64) {
        return Err("trust index: response too large".into());
    }
    let raw = resp.text().await.map_err(|e| format!("read the trust index: {e}"))?;
    let entries = parse_index(&raw);
    if entries.is_empty() {
        return Err("trust index: nothing usable in the response".into());
    }
    Ok(entries)
}

/// Ratings for `packages`. With `enabled` false this does nothing at all: no
/// request, no cache read. A failed refresh falls back to the last good list.
pub async fn view(enabled: bool, packages: &[String], now: i64) -> TrustView {
    if !enabled {
        return TrustView::default();
    }
    let path = cache_path();
    let cached = load_cache(&path);
    let (entries, fetched_at, error) = match cached {
        Some(c) if is_fresh(c.fetched_at, now) => (c.entries, Some(c.fetched_at), None),
        stale => match fetch().await {
            Ok(entries) => {
                save_cache(&path, &CacheFile { fetched_at: now, entries: entries.clone() });
                (entries, Some(now), None)
            }
            Err(e) => match stale {
                Some(c) => (c.entries, Some(c.fetched_at), None),
                None => (Vec::new(), None, Some(e)),
            },
        },
    };
    TrustView {
        enabled: true,
        fetched_at,
        listed: entries.len(),
        ratings: if entries.is_empty() { Vec::new() } else { rate(packages, &entries) },
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"[
      {"name":"Chrome DevTools","tier":"recommended","score":89,"package_type":"npm",
       "npm_package":"chrome-devtools-mcp","pypi_package":null,"github_repo":"x/y","min_weekly_downloads":1},
      {"name":"Fetch","tier":"enterprise-verified","score":90,"package_type":"pypi",
       "npm_package":null,"pypi_package":"mcp-server-fetch","github_repo":"a/b","min_weekly_downloads":1},
      {"name":"Scoped","tier":"emerging","score":50,"npm_package":"@Scope/Thing","pypi_package":null},
      {"name":"","tier":"x","score":1},
      {"name":"NaN","tier":"x","score":"high"},
      "not an object"
    ]"#;

    #[test]
    fn the_published_list_parses_and_junk_is_dropped_not_fatal() {
        let index = parse_index(LIST);
        assert_eq!(index.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["Chrome DevTools", "Fetch", "Scoped"]);
        assert!(parse_index("{not json").is_empty());
        assert!(parse_index(&"x".repeat(MAX_INDEX_BYTES + 1)).is_empty());
        let long = format!(r#"[{{"name":"{}","tier":"t","score":1}}]"#, "n".repeat(MAX_TEXT + 1));
        assert!(parse_index(&long).is_empty());
    }

    #[test]
    fn versions_and_case_do_not_get_in_the_way_of_a_match() {
        assert_eq!(package_name("chrome-devtools-mcp@latest"), "chrome-devtools-mcp");
        assert_eq!(package_name("@Scope/Thing@^2.1.0"), "@scope/thing");
        assert_eq!(package_name("@scope/thing"), "@scope/thing");
        assert_eq!(package_name("  plain  "), "plain");
    }

    #[test]
    fn unlisted_packages_are_reported_as_not_rated_and_duplicates_once() {
        let index = parse_index(LIST);
        let packages = [
            "chrome-devtools-mcp@latest".to_string(),
            "chrome-devtools-mcp@1".to_string(), // same package in another app
            "mcp-server-fetch".to_string(),
            "@scope/thing@2".to_string(),
            "fetch-mcp".to_string(), // the look-alike the index warns about is NOT the listed one
        ];
        let got = rate(&packages, &index);
        assert_eq!(got.len(), 4);
        assert_eq!((got[0].listed_as.as_deref(), got[0].tier.as_deref(), got[0].score),
                   (Some("Chrome DevTools"), Some("recommended"), Some(89.0)));
        assert_eq!(got[1].listed_as.as_deref(), Some("Fetch"));
        assert_eq!(got[2].listed_as.as_deref(), Some("Scoped"));
        assert_eq!(got[3], Rating { package: "fetch-mcp".into(), listed_as: None, tier: None, score: None });
    }

    #[test]
    fn the_list_is_refreshed_daily_and_a_clock_set_back_does_not_pin_it() {
        let now = 1_000 * REFRESH_AFTER_MS;
        assert!(is_fresh(now - 3_600_000, now));
        assert!(!is_fresh(now - REFRESH_AFTER_MS, now));
        assert!(!is_fresh(0, now), "never fetched");
        assert!(!is_fresh(now + 5, now), "a cache from the future is not trusted as fresh");
    }

    /// Rates this machine's real packages against a saved copy of the real
    /// list (AITM_INDEX_FILE). No network. `--ignored --nocapture`
    #[test]
    #[ignore]
    fn live_rate() {
        let raw = std::fs::read_to_string(std::env::var("AITM_INDEX_FILE").unwrap()).unwrap();
        let index = parse_index(&raw);
        println!("listed: {}", index.len());
        let packages: Vec<String> =
            crate::inventory::scan().mcp_servers.into_iter().filter_map(|s| s.package).collect();
        for r in rate(&packages, &index) {
            println!("  {:40} {:?} {:?}", r.package, r.tier, r.score);
        }
    }

    /// The real request, with the app's real HTTP client. `--ignored --nocapture`
    #[test]
    #[ignore]
    fn live_fetch() {
        match crate::rt::block_on(fetch()) {
            Ok(entries) => println!("fetched {} entries, first: {}", entries.len(), entries[0].name),
            Err(e) => println!("FETCH FAILED: {e}"),
        }
    }

    #[test]
    fn switched_off_it_does_nothing_at_all() {
        let view = crate::rt::block_on(view(false, &["anything".to_string()], 1));
        assert!(!view.enabled);
        assert!(view.ratings.is_empty() && view.fetched_at.is_none() && view.error.is_none());
    }

    #[test]
    fn the_request_carries_nothing_about_the_machine() {
        // The URL is a constant with no query, fragment or userinfo; there is
        // nowhere for a package name to ride along.
        assert!(INDEX_URL.starts_with("https://"));
        assert!(!INDEX_URL.contains(['?', '#', '@', '{']));
    }
}
