mod tray_projection;

// The data layer lives in the core crate; these keep the `alerts::…`,
// `providers::…` paths used throughout this file and by `tray_projection`.
pub(crate) use aitm_core::{alerts, audit, clients, coaching, diagnose, digest, drift, effort, forecast, history, httpapi, i18n, inventory, ledger, pin, pricing, procs, providers, spend, trust};
use aitm_core::{card_is_disabled, family_of, is_managed_key_card};

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, WindowEvent,
};

/// Last-good snapshots older than this are too misleading to show or to
/// use as a stand-in for a live Moonshot card.
const SNAPSHOT_CACHE_MS: i64 = 24 * 60 * 60 * 1000;
/// One failed cycle isn't "Outdated": vendors hiccup routinely.
const STALE_GRACE_MS: i64 = 3 * 60 * 1000;

// ---------------------------------------------------------------------------
// App settings, stored at %APPDATA%\AITaskManager\config.json
// ---------------------------------------------------------------------------

fn config_path_in(dir: &Path) -> PathBuf {
    dir.join("config.json")
}

/// A parse failure here once silently reset all settings to defaults, so
/// failures are now logged durably and the last good copy is used instead.
fn note_config_error(context: &str) {
    eprintln!("[aitm] {context}");
    // Tests run against temp dirs; they must never append into the
    // developer's real config-error.log.
    if cfg!(test) {
        return;
    }
    let line = format!(
        "{} {}\r\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        context
    );
    let path = providers::config_dir().join("config-error.log");
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

fn parse_config_file(path: &PathBuf) -> Result<Value, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("read: {e}"))?;
    // Tolerate a UTF-8 BOM (Notepad and PowerShell 5.1 both write one).
    let value: Value =
        serde_json::from_str(raw.trim_start_matches('\u{feff}')).map_err(|e| format!("parse: {e}"))?;
    // `[]` / `null` / `"x"` parse, but this app's contract is an object.
    // Treating those as unreadable lets load fall back to the backup
    // and stops a save from copying junk over the last good copy.
    if !value.is_object() {
        return Err("not an object".into());
    }
    Ok(value)
}

fn load_config() -> Value {
    load_config_from(&providers::config_dir())
}

fn load_config_from(dir: &Path) -> Value {
    let path = config_path_in(dir);
    if !path.exists() {
        return json!({});
    }
    match parse_config_file(&path) {
        Ok(cfg) => cfg,
        Err(e) => {
            note_config_error(&format!("config.json unreadable ({e}) — trying backup"));
            let backup = dir.join("config.json.bak");
            match parse_config_file(&backup) {
                Ok(cfg) => cfg,
                Err(e2) => {
                    note_config_error(&format!("config.json.bak also failed ({e2}) — defaults"));
                    json!({})
                }
            }
        }
    }
}

fn config_with_defaults(mut cfg: Value) -> Value {
    if !cfg.is_object() {
        cfg = json!({});
    }
    let obj = cfg.as_object_mut().unwrap();
    // Out-of-the-box experience: 1-min refresh, pacing always visible,
    // all three quota alerts on, dark + compact. (Autostart defaults on
    // in setup; tray icon defaults to Auto via pinned = null.)
    obj.entry("refreshMinutes").or_insert(json!(1));
    obj.entry("disabled").or_insert(json!([]));
    obj.entry("pinned").or_insert(Value::Null);
    obj.entry("trayProviders").or_insert(json!([]));
    obj.entry("pacingAlways").or_insert(json!(true));
    obj.entry("notifyAlmostOut").or_insert(json!(true));
    obj.entry("notifyCuttingClose").or_insert(json!(true));
    obj.entry("notifyWillRunOut").or_insert(json!(true));
    obj.entry("notifyReset").or_insert(json!(true));
    // Budget guard. Burn: points of a weekly-or-longer quota inside 30
    // minutes (0 = off). Spend: dollars in one local day (0 = off; there is
    // no universal default for what a day should cost).
    obj.entry("wideMode").or_insert(json!(false));
    // Off by default: the only request to a non-provider server.
    obj.entry("trustLookup").or_insert(json!(false));
    // Off by default: serves spend, work areas, clients and the ledger on
    // the loopback API for the user's own dashboards.
    obj.entry("apiFeeds").or_insert(json!(false));
    // "off" or a weekday ("mon" … "sun").
    obj.entry("weeklyDigest").or_insert(json!("mon"));
    // Days a still-used session may stay open before one weekly nudge (0 = off).
    obj.entry("sessionNudgeDays").or_insert(json!(7));
    // The audit opens by itself once, on the first run.
    obj.entry("auditSeen").or_insert(json!(false));
    // Days before a renewal to send its one reminder (0 = off).
    obj.entry("renewalReminderDays").or_insert(json!(3));
    obj.entry("burnAlertPoints").or_insert(json!(15));
    obj.entry("dailySpendAlert").or_insert(json!(0));
    obj.entry("spendTab").or_insert(json!("today"));
    obj.entry("spendMetric").or_insert(json!("cost"));
    obj.entry("showUsed").or_insert(json!(false));
    obj.entry("resetExact").or_insert(json!(false));
    obj.entry("timeFormat").or_insert(json!("auto"));
    obj.entry("layout").or_insert(Value::Null);
    obj.entry("appearance").or_insert(json!("dark"));
    obj.entry("density").or_insert(json!("compact"));
    obj.entry("minimal").or_insert(json!(false));
    obj.entry("glassEffects").or_insert(json!(true));
    obj.entry("shortcut").or_insert(json!(""));
    obj.entry("proxy")
        .or_insert(json!({ "enabled": false, "url": "" }));
    obj.entry("showTotalSpend").or_insert(json!(true));
    obj.entry("welcomeDismissed").or_insert(json!(false));
    // Empty = "never recorded": the frontend uses it to tell a fresh
    // install (no What's-new popup) from an update (popup with the notes).
    obj.entry("lastSeenVersion").or_insert(json!(""));
    obj.entry("reduceAnimations").or_insert(json!(false));
    obj.entry("locale").or_insert(json!("auto"));
    cfg
}

#[tauri::command]
fn system_ui_locale() -> &'static str {
    i18n::system_ui_locale()
}

/// The local AI inventory for the Inventory tab. Names, shapes and counts
/// only; see `inventory.rs` for what is deliberately never read out.
#[tauri::command]
async fn get_inventory() -> Result<inventory::Inventory, String> {
    tauri::async_runtime::spawn_blocking(|| enriched_inventory().0)
    .await
    .map_err(|e| format!("inventory scan: {e}"))
}

/// Renders a core `Msg` error in the resolved locale before it crosses the
/// command boundary. Every Tauri command backed by a core `Result<_, Msg>`
/// funnels its `Err` through this one function -- never a hand-written
/// closure per command -- so the popover only ever sees a plain, already-
/// localised string and no command has to re-derive the resolved locale.
fn user_error(cfg: &Value, m: &i18n::Msg) -> String {
    i18n::t(cfg, m)
}

/// Stop one running MCP server. The frontend sends a NAME it was shown and
/// the user confirmed; `procs::end_task` resolves that to pids from a fresh
/// snapshot, so no pid crosses this boundary and nothing outside the matched
/// MCP servers can be reached. It asks (SIGTERM), never forces.
#[tauri::command]
async fn end_task(name: String) -> Result<usize, String> {
    let cfg = config_with_defaults(load_config());
    tauri::async_runtime::spawn_blocking(move || {
        let inv = inventory::scan();
        procs::end_task(&name, &inv.mcp_servers)
    })
    .await
    .map_err(|e| format!("end task: {e}"))?
    .map_err(|m| user_error(&cfg, &m))
}

/// Where each provider looks for its sign-in, and whether it is there.
#[tauri::command]
async fn get_diagnosis() -> Result<Vec<diagnose::Diagnosis>, String> {
    tauri::async_runtime::spawn_blocking(diagnose::all).await.map_err(|e| format!("diagnose: {e}"))
}

/// 30 days of spend per work area against 30 days of commits in that folder.
#[tauri::command]
async fn get_effort() -> Result<Vec<effort::AreaEffort>, String> {
    tauri::async_runtime::spawn_blocking(|| effort::measure_spend(&spend::collect(None), 30))
        .await
        .map_err(|e| format!("effort: {e}"))
}

/// The live process view. Separate from `get_inventory` because it is cheap
/// and changes by the second, while a full scan reads every config file.
#[tauri::command]
async fn get_running() -> Result<Vec<procs::RunningServer>, String> {
    tauri::async_runtime::spawn_blocking(|| procs::snapshot(&inventory::scan().mcp_servers))
        .await
        .map_err(|e| format!("process scan: {e}"))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientView {
    rules: Vec<clients::ClientRule>,
    rows: Vec<clients::ClientSpend>,
}

/// Work areas rolled up to the user's clients. The frontend already holds
/// the areas from the spend scan, so they are passed in rather than rescanned.
#[tauri::command]
fn client_rollup(areas: Vec<spend::AreaSpend>) -> ClientView {
    let rules = clients::load_from(&clients::path());
    let rows = clients::rollup(&areas, &rules, chrono::Local::now().date_naive());
    ClientView { rules, rows }
}

#[tauri::command]
fn save_clients(rules: Vec<clients::ClientRule>) -> Result<Vec<clients::ClientRule>, String> {
    clients::save_to(&clients::path(), rules)
}

/// Writes the client rollup as CSV into the Downloads folder and shows it
/// there. Returns the file's path.
#[tauri::command]
fn export_clients_csv(app: tauri::AppHandle, areas: Vec<spend::AreaSpend>) -> Result<String, String> {
    let cfg = config_with_defaults(load_config());
    let today = chrono::Local::now().date_naive();
    let rules = clients::load_from(&clients::path());
    let body = clients::csv(&clients::rollup(&areas, &rules, today), today);
    let dir = app
        .path()
        .download_dir()
        .map_err(|e| user_error(&cfg, &i18n::Msg::new("error.export.downloadsDir").var("error", e)))?;
    let file = dir.join(format!("ai-cost-by-client-{}.csv", today.format("%Y-%m-%d")));
    std::fs::write(&file, body).map_err(|e| {
        user_error(&cfg, &i18n::Msg::new("error.export.write").var("path", file.display()).var("error", e))
    })?;
    use tauri_plugin_opener::OpenerExt;
    let _ = app.opener().reveal_item_in_dir(&file);
    Ok(file.display().to_string())
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LiveMetric {
    label: String,
    used: f64,
    resets_at: Option<i64>,
    period_ms: Option<i64>,
}

/// When each of a card's limits runs out at the recent rate of use. The live
/// readings come from the frontend; the rate comes from the history store.
#[tauri::command]
async fn get_forecast(provider_id: String, metrics: Vec<LiveMetric>) -> Result<Vec<forecast::Forecast>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let now = chrono::Utc::now().timestamp_millis();
        let series = history::series(&provider_id, now - 25 * 3_600_000);
        metrics
            .iter()
            .filter_map(|m| {
                let points: Vec<(i64, f64)> = series
                    .iter()
                    .find(|s| s.metric == m.label)
                    .map(|s| s.points.iter().map(|p| (p.at, p.used)).collect())
                    .unwrap_or_default();
                forecast::forecast(&m.label, &points, m.used, m.resets_at, m.period_ms, now)
            })
            .collect()
    })
    .await
    .map_err(|e| format!("forecast: {e}"))
}

/// Trust Index ratings for the given packages. Does nothing unless the user
/// turned `trustLookup` on; see `trust.rs` for what the request carries (nothing).
#[tauri::command]
async fn get_trust(packages: Vec<String>) -> trust::TrustView {
    let enabled = config_with_defaults(load_config())
        .get("trustLookup")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    trust::view(enabled, &packages, chrono::Utc::now().timestamp_millis()).await
}

/// Saves a table the UI is showing as CSV in the Downloads folder and
/// reveals it. Cells are made spreadsheet-safe and the name file-safe here,
/// whatever the frontend sent.
#[tauri::command]
fn export_table(app: tauri::AppHandle, name: String, headers: Vec<String>, rows: Vec<Vec<String>>) -> Result<String, String> {
    let cfg = config_with_defaults(load_config());
    // Unreachable from the shipped UI, which only ever sends one of the
    // handful of allowlisted table names it already knows the headers for;
    // kept as a real, translated refusal rather than a debug-only guard
    // because a command boundary is not a place to assume the caller.
    if headers.is_empty() || headers.len() > 40 || rows.iter().any(|r| r.len() > 40) {
        return Err(user_error(&cfg, &i18n::Msg::new("error.export.unsupported")));
    }
    let dir = app
        .path()
        .download_dir()
        .map_err(|e| user_error(&cfg, &i18n::Msg::new("error.export.downloadsDir").var("error", e)))?;
    let today = chrono::Local::now().format("%Y-%m-%d");
    let file = dir.join(format!("{}-{today}.csv", clients::safe_stem(&name)));
    std::fs::write(&file, clients::table_csv(&headers, &rows)).map_err(|e| {
        user_error(&cfg, &i18n::Msg::new("error.export.write").var("path", file.display()).var("error", e))
    })?;
    use tauri_plugin_opener::OpenerExt;
    let _ = app.opener().reveal_item_in_dir(&file);
    Ok(file.display().to_string())
}

fn pin_plan_for(name: &str, client: &str) -> Result<pin::PinPlan, i18n::Msg> {
    let inv = inventory::scan();
    let server = inv
        .mcp_servers
        .iter()
        .find(|s| s.name == name && s.client == client && s.pin_to.is_some())
        .ok_or_else(|| i18n::Msg::new("error.pin.notUnpinned"))?;
    let package = server.package.as_deref().ok_or_else(|| i18n::Msg::new("error.pin.noPackage"))?;
    let file = server.source_file.as_deref().ok_or_else(|| i18n::Msg::new("error.pin.unknownFile"))?;
    let installed =
        pin::installed_version(&server.target, package).ok_or_else(|| i18n::Msg::new("error.pin.notInstalled"))?;
    pin::plan(std::path::Path::new(file), &server.target, package, &installed)
}

/// What pinning a server would change. Reads only.
#[tauri::command]
async fn pin_preview(name: String, client: String) -> Result<pin::PinPlan, String> {
    let cfg = config_with_defaults(load_config());
    tauri::async_runtime::spawn_blocking(move || pin_plan_for(&name, &client))
        .await
        .map_err(|e| format!("pin preview: {e}"))?
        .map_err(|m| user_error(&cfg, &m))
}

/// Applies the pin the user was shown. The plan is rebuilt here and must
/// match what they saw, file size and modification time included.
#[tauri::command]
async fn pin_apply(name: String, client: String, seen: pin::PinPlanSeen) -> Result<String, String> {
    let cfg = config_with_defaults(load_config());
    tauri::async_runtime::spawn_blocking(move || {
        let plan = pin_plan_for(&name, &client)?;
        if plan.to != seen.to || plan.file != seen.file || plan.file_len != seen.file_len || plan.file_mtime_ms != seen.file_mtime_ms {
            return Err(i18n::Msg::new("error.pin.planChanged"));
        }
        pin::apply(&plan)
    })
    .await
    .map_err(|e| format!("pin: {e}"))?
    .map_err(|m| user_error(&cfg, &m))
}

/// When in the week each of a card's limits gets used, over the last 28
/// days, bucketed in local time.
#[tauri::command]
async fn get_burn_profile(provider_id: String) -> Result<Vec<history::BurnProfile>, String> {
    let offset_ms = i64::from(chrono::Local::now().offset().local_minus_utc()) * 1000;
    let since = chrono::Utc::now().timestamp_millis() - 28 * 24 * 3_600_000;
    tauri::async_runtime::spawn_blocking(move || history::burn_profiles(&provider_id, since, offset_ms))
        .await
        .map_err(|e| format!("burn profile: {e}"))
}

/// The inventory plus every computed finding, and the spend it was built
/// from. **Both the Inventory tab and the audit go through here**: they used
/// to assemble the list separately, so the audit silently scored a shorter
/// one than the tab displayed.
fn enriched_inventory() -> (inventory::Inventory, Vec<spend::ProviderSpend>) {
    let mut inv = inventory::scan();
    let spend = spend::collect(None);
    let claude = spend.iter().find(|p| p.id == "claude");
    inv.opportunities.extend(coaching::opportunities(claude, &spend::claude_sessions(None, None, 500)));
    inv.opportunities.extend(procs::opportunities(&procs::snapshot(&inv.mcp_servers)));
    inv.opportunities.extend(drift::opportunities(&drift::scan()));
    // Gaps first, then things to learn, each in the order found.
    inv.opportunities.sort_by_key(|o| o.kind != "tighten");
    (inv, spend)
}

fn build_audit() -> audit::AuditReport {
    let (inv, spend) = enriched_inventory();
    let usage30: std::collections::HashMap<String, f64> = spend.iter().map(|p| (p.id.clone(), p.last30.cost)).collect();
    let today = chrono::Local::now().date_naive();
    let ledger_view = ledger::view(&ledger::load_from(&ledger::path()), today, &usage30);
    let areas: std::collections::HashSet<&str> = spend
        .iter()
        .flat_map(|p| p.projects.iter())
        .flat_map(|pr| pr.areas.iter())
        .map(|a| spend::area_top(&a.area))
        .filter(|a| !a.starts_with('('))
        .collect();
    audit::run(
        &audit::Inputs {
            inventory: &inv,
            ledger: &ledger_view,
            spend30: spend.iter().map(|p| p.last30.cost).sum(),
            client_rules: clients::load_from(&clients::path()).len(),
            work_areas: areas.len(),
        },
        chrono::Utc::now().timestamp_millis(),
    )
}

/// The scored read of this machine's AI setup.
#[tauri::command]
async fn get_audit() -> Result<audit::AuditReport, String> {
    tauri::async_runtime::spawn_blocking(build_audit).await.map_err(|e| format!("audit: {e}"))
}

/// Saves the audit as Markdown in the Downloads folder and reveals it.
#[tauri::command]
async fn export_audit(app: tauri::AppHandle) -> Result<String, String> {
    let cfg = config_with_defaults(load_config());
    let report = tauri::async_runtime::spawn_blocking(build_audit).await.map_err(|e| format!("audit: {e}"))?;
    let now = chrono::Local::now();
    let dir = app
        .path()
        .download_dir()
        .map_err(|e| user_error(&cfg, &i18n::Msg::new("error.export.downloadsDir").var("error", e)))?;
    let file = dir.join(format!("ai-setup-audit-{}.md", now.format("%Y-%m-%d")));
    std::fs::write(&file, audit::to_markdown(&report, &now.format("%-d %B %Y").to_string())).map_err(|e| {
        user_error(&cfg, &i18n::Msg::new("error.export.write").var("path", file.display()).var("error", e))
    })?;
    use tauri_plugin_opener::OpenerExt;
    let _ = app.opener().reveal_item_in_dir(&file);
    Ok(file.display().to_string())
}

/// The sessions behind an area or a day, from the scan cache (no rescan).
#[tauri::command]
async fn get_sessions(area: Option<String>, day: Option<String>) -> Result<Vec<spend::SessionSpend>, String> {
    tauri::async_runtime::spawn_blocking(move || spend::claude_sessions(area.as_deref(), day.as_deref(), 40))
        .await
        .map_err(|e| format!("sessions: {e}"))
}

/// Shows a session's log file in the OS file manager. Read-only, and the
/// most the app will ever do about an old session: it never deletes one,
/// because these logs are also where its spend figures come from. The id is
/// matched against the scan cache and the path comes from there, so nothing
/// the popover sends can name a file.
#[tauri::command]
fn reveal_session(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let path = spend::session_path(&id)
        .ok_or_else(|| "that session is no longer in the scan cache; refresh and try again".to_string())?;
    use tauri_plugin_opener::OpenerExt;
    app.opener().reveal_item_in_dir(&path).map_err(|e| format!("could not show the file: {e}"))
}

/// The subscription ledger with totals, renewals and value against usage.
/// `usage30` is each card's 30-day API-equivalent spend, which the frontend
/// already holds from the spend scan; passing it in avoids a second scan.
#[tauri::command]
fn get_ledger(usage30: std::collections::HashMap<String, f64>) -> ledger::LedgerView {
    let today = chrono::Local::now().date_naive();
    ledger::view(&ledger::load_from(&ledger::path()), today, &usage30)
}

#[tauri::command]
fn save_subscription(subscription: ledger::Subscription) -> Result<ledger::Subscription, String> {
    let cfg = config_with_defaults(load_config());
    ledger::upsert_in(&ledger::path(), subscription).map_err(|m| user_error(&cfg, &m))
}

#[tauri::command]
fn delete_subscription(id: String) -> Result<(), String> {
    let cfg = config_with_defaults(load_config());
    ledger::delete_in(&ledger::path(), &id).map_err(|m| user_error(&cfg, &m))
}

/// Limit readings for one card over the last `hours`, for the detail view.
#[tauri::command]
async fn get_history(provider_id: String, hours: u32) -> Result<Vec<history::Series>, String> {
    let since = chrono::Utc::now().timestamp_millis() - i64::from(hours.min(24 * 90)) * 3_600_000;
    tauri::async_runtime::spawn_blocking(move || history::series(&provider_id, since))
        .await
        .map_err(|e| format!("history: {e}"))
}

#[tauri::command]
fn get_config() -> Value {
    config_with_defaults(load_config())
}

/// Every key config.json may hold — the same set config_with_defaults seeds.
/// set_config drops anything else so a compromised frontend can't stash
/// arbitrary data in the config file.
const CONFIG_KEYS: &[&str] = &[
    // Not seeded by config_with_defaults (the autostart plugin is the
    // source of truth at runtime) but persisted here so setup() can apply
    // the user's choice on launch.
    "autostart",
    "refreshMinutes",
    "disabled",
    "pinned",
    "trayProviders",
    "pacingAlways",
    "notifyAlmostOut",
    "notifyCuttingClose",
    "notifyWillRunOut",
    "notifyReset",
    "burnAlertPoints",
    "dailySpendAlert",
    "wideMode",
    "trustLookup",
    "apiFeeds",
    "weeklyDigest",
    "sessionNudgeDays",
    "auditSeen",
    "renewalReminderDays",
    "spendMetric",
    "spendTab",
    "showUsed",
    "resetExact",
    "timeFormat",
    "layout",
    "appearance",
    "density",
    "minimal",
    "glassEffects",
    "shortcut",
    "proxy",
    "showTotalSpend",
    "welcomeDismissed",
    "lastSeenVersion",
    "reduceAnimations",
    "locale",
];

static CONFIG_WRITE: Mutex<()> = Mutex::new(());
static CONFIG_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn apply_config_patch(cfg: &mut Value, patch: &Value) {
    if let (Some(target), Some(source)) = (cfg.as_object_mut(), patch.as_object()) {
        for (k, v) in source {
            if CONFIG_KEYS.contains(&k.as_str()) {
                if k == "locale" {
                    let ok = matches!(v.as_str(), Some(s) if s == "auto" || i18n::LOCALES.contains(&s));
                    target.insert(k.clone(), if ok { v.clone() } else { json!("auto") });
                } else {
                    target.insert(k.clone(), v.clone());
                }
            } else {
                eprintln!("[aitm] set_config: ignoring unknown key '{k}'");
            }
        }
    }
}

/// Injectable filesystem operations for config persistence, so failure
/// paths (disk full, locked file, failed replace) are repeatable tests
/// instead of best-effort permission games.
#[derive(Clone, Copy)]
struct ConfigPersistIo {
    write_tmp: fn(&Path, &str) -> std::io::Result<()>,
    replace: fn(&Path, &Path) -> std::io::Result<()>,
}

impl ConfigPersistIo {
    fn real() -> Self {
        Self {
            write_tmp: |path, raw| std::fs::write(path, raw),
            // std::fs::rename replaces an existing destination on Windows
            // (MoveFileEx REPLACE_EXISTING), so the swap is atomic-ish.
            replace: |tmp, path| std::fs::rename(tmp, path),
        }
    }
}

/// Commit order for one config save (callers hold CONFIG_WRITE):
///   1. write the new config to a unique temp file — failure cleans the
///      temp and leaves the main file and backup untouched;
///   2. refresh the backup from the main file, but ONLY while the main
///      file still parses — a corrupt main must never clobber the last
///      good backup, because that backup is the only thing a corrupt
///      main recovers from;
///   3. replace the main file with the temp — failure removes the temp
///      and both old files survive.
///
/// After every step at least one parseable config exists: the old main,
/// the backup, or (once step 1 succeeded) the temp itself.
fn persist_config_at(dir: &Path, cfg: &Value, io: ConfigPersistIo) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create config dir: {e}"))?;
    let path = config_path_in(dir);
    let backup = dir.join("config.json.bak");
    let tmp = dir.join(format!(
        "config.{}.{}.tmp",
        std::process::id(),
        CONFIG_TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let raw = serde_json::to_string_pretty(cfg).unwrap_or_default();
    if let Err(e) = (io.write_tmp)(&tmp, &raw) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("write config: {e}"));
    }
    if path.exists() && parse_config_file(&path).is_ok() {
        // Copy into a temp, then rename over the backup. A failed
        // mid-copy must not truncate the last good .bak in place.
        let bak_tmp = dir.join(format!(
            "config.bak.{}.{}.tmp",
            std::process::id(),
            CONFIG_TMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let refresh = (|| {
            std::fs::copy(&path, &bak_tmp)?;
            std::fs::rename(&bak_tmp, &backup)
        })();
        if let Err(e) = refresh {
            let _ = std::fs::remove_file(&bak_tmp);
            // Not fatal: the new config is already in `tmp`, and the
            // previous backup (if any) is still intact.
            note_config_error(&format!("config backup refresh failed ({e})"));
        }
    }
    if let Err(e) = (io.replace)(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("replace config: {e}"));
    }
    Ok(())
}

fn persist_config_in(dir: &Path, cfg: &Value) -> Result<(), String> {
    persist_config_at(dir, cfg, ConfigPersistIo::real())
}

fn set_config_in(dir: &Path, patch: Value) -> Result<Value, String> {
    let _guard = CONFIG_WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut cfg = config_with_defaults(load_config_from(dir));
    apply_config_patch(&mut cfg, &patch);
    persist_config_in(dir, &cfg)?;
    Ok(cfg)
}

fn set_config_inner(patch: Value) -> Result<Value, String> {
    let cfg = set_config_in(&providers::config_dir(), patch)?;
    let disabled = cfg.get("disabled").and_then(Value::as_array)
        .map(|ids| ids.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>())
        .unwrap_or_default();
    httpapi::forget_disabled_snapshots(&disabled);
    Ok(cfg)
}

#[tauri::command]
fn set_config(app: tauri::AppHandle, patch: Value) -> Result<Value, String> {
    let _publication = KEY_CARD_PUBLICATION.lock().unwrap_or_else(|e| e.into_inner());
    let cfg = set_config_inner(patch)?;
    apply_tray_locale(&app, &cfg);
    Ok(cfg)
}

fn apply_tray_locale(app: &tauri::AppHandle, cfg: &Value) {
    let next = i18n::resolved_locale(cfg);
    static LAST: Mutex<Option<&'static str>> = Mutex::new(None);
    let Ok(mut last) = LAST.lock() else {
        return;
    };
    if *last == Some(next) {
        return;
    }
    *last = Some(next);
    drop(last);
    let Ok(quit) = MenuItem::with_id(app, "quit", i18n::quit_label(cfg), true, None::<&str>) else {
        return;
    };
    let Ok(menu) = Menu::with_items(app, &[&quit]) else {
        return;
    };
    if let Some(tray) = app.tray_by_id("tray") {
        let _ = tray.set_menu(Some(menu));
    }
}

// ---------------------------------------------------------------------------
// Start with Windows
// ---------------------------------------------------------------------------

#[tauri::command]
fn get_autostart(app: tauri::AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    // Remember the choice so startup knows whether to re-assert it.
    let _ = set_config_inner(json!({ "autostart": enabled }));
    let manager = app.autolaunch();
    if enabled {
        manager.enable().map_err(|e| e.to_string())
    } else {
        manager.disable().map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Tray icon with the pinned metric drawn onto it
// ---------------------------------------------------------------------------

// 4x6 pixel digit font, one nibble per row (bit 3 = leftmost pixel).
const DIGIT_FONT: [[u8; 6]; 10] = [
    [0x6, 0x9, 0x9, 0x9, 0x9, 0x6], // 0
    [0x2, 0x6, 0x2, 0x2, 0x2, 0x7], // 1
    [0x6, 0x9, 0x1, 0x2, 0x4, 0xF], // 2
    [0xE, 0x1, 0x6, 0x1, 0x9, 0x6], // 3
    [0x2, 0x6, 0xA, 0xF, 0x2, 0x2], // 4
    [0xF, 0x8, 0xE, 0x1, 0x9, 0x6], // 5
    [0x6, 0x8, 0xE, 0x9, 0x9, 0x6], // 6
    [0xF, 0x1, 0x2, 0x2, 0x4, 0x4], // 7
    [0x6, 0x9, 0x6, 0x9, 0x9, 0x6], // 8
    [0x6, 0x9, 0x9, 0x7, 0x1, 0x6], // 9
];

/// Renders one or two numbers (0-100) stacked on a 32x32 RGBA tray icon —
/// two rows mimic the Mac menu bar's "100% / 36%" pair. White digits with a
/// black outline so they read on both light and dark taskbars.
fn draw_tray_numbers(values: &[u32]) -> Vec<u8> {
    const SIZE: usize = 32;
    let scale = 2usize;
    let glyph_w = 4 * scale;
    let _glyph_h = 6 * scale;
    let gap = scale;

    let mut mask = [false; SIZE * SIZE];
    let rows: &[usize] = if values.len() >= 2 { &[3, 17] } else { &[10] };

    for (value, y0) in values.iter().zip(rows) {
        let digits: Vec<usize> = value
            .to_string()
            .chars()
            .filter_map(|c| c.to_digit(10).map(|d| d as usize))
            .collect();
        let text_w = digits.len() * glyph_w + digits.len().saturating_sub(1) * gap;
        let x0 = (SIZE.saturating_sub(text_w)) / 2;

        for (i, d) in digits.iter().enumerate() {
            let gx = x0 + i * (glyph_w + gap);
            for (row, bits) in DIGIT_FONT[*d].iter().enumerate() {
                for col in 0..4 {
                    if bits & (0x8 >> col) != 0 {
                        for sy in 0..scale {
                            for sx in 0..scale {
                                let x = gx + col * scale + sx;
                                let y = y0 + row * scale + sy;
                                if x < SIZE && y < SIZE {
                                    mask[y * SIZE + x] = true;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    // Outline pass: black anywhere adjacent to a text pixel.
    for y in 0..SIZE {
        for x in 0..SIZE {
            if mask[y * SIZE + x] {
                continue;
            }
            let near = (-1i32..=1).any(|dy| {
                (-1i32..=1).any(|dx| {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    nx >= 0
                        && ny >= 0
                        && (nx as usize) < SIZE
                        && (ny as usize) < SIZE
                        && mask[ny as usize * SIZE + nx as usize]
                })
            });
            if near {
                let p = (y * SIZE + x) * 4;
                rgba[p..p + 4].copy_from_slice(&[0, 0, 0, 230]);
            }
        }
    }
    for y in 0..SIZE {
        for x in 0..SIZE {
            if mask[y * SIZE + x] {
                let p = (y * SIZE + x) * 4;
                rgba[p..p + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
    }
    rgba
}

/// The menu bar text for the starred metrics: "33 · 48". Percent left, the
/// same numbers the Windows tray draws into its icon.
fn menu_bar_title(remaining: &[u32]) -> Option<String> {
    let parts: Vec<String> = remaining.iter().take(3).map(|v| (*v).min(100).to_string()).collect();
    (!parts.is_empty()).then(|| parts.join(" \u{b7} "))
}

/// macOS: a template glyph plus native text. The Windows approach (digits
/// drawn into a 32 px bitmap) is scaled up by a Retina menu bar into a blur,
/// and a coloured icon ignores the light / dark menu bar. A template image
/// is tinted by the system, and the title is real text at any scale.
#[cfg(target_os = "macos")]
fn apply_main_tray_projection(
    app: &tauri::AppHandle,
    projection: &tray_projection::MainTrayProjection,
) -> Result<(), String> {
    let tray = app
        .tray_by_id("tray")
        .ok_or_else(|| "main tray icon is unavailable".to_string())?;
    tray.set_tooltip(Some(&projection.tooltip))
        .map_err(|error| format!("set main tray tooltip: {error}"))?;
    let title = match projection.icon_mode {
        tray_projection::MainTrayIconMode::Logo => None,
        tray_projection::MainTrayIconMode::Numbers => menu_bar_title(&projection.remaining_percentages),
    };
    tray.set_title(title.as_deref()).map_err(|error| format!("set menu bar title: {error}"))?;
    if let Ok(mut slot) = last_main_tray().lock() {
        slot.lefts = projection.remaining_percentages.clone();
        slot.tooltip = projection.tooltip.clone();
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn apply_main_tray_projection(
    app: &tauri::AppHandle,
    projection: &tray_projection::MainTrayProjection,
) -> Result<(), String> {
    let tray = app
        .tray_by_id("tray")
        .ok_or_else(|| "main tray icon is unavailable".to_string())?;
    tray.set_tooltip(Some(&projection.tooltip))
        .map_err(|error| format!("set main tray tooltip: {error}"))?;
    match projection.icon_mode {
        tray_projection::MainTrayIconMode::Logo => {
            let default = app
                .default_window_icon()
                .ok_or_else(|| "default AI Task Manager icon is unavailable".to_string())?;
            tray.set_icon(Some(default.clone()))
                .map_err(|error| format!("set main tray logo: {error}"))?;
        }
        tray_projection::MainTrayIconMode::Numbers => {
            let icon = tauri::image::Image::new_owned(
                draw_tray_numbers(&projection.remaining_percentages),
                32,
                32,
            );
            tray.set_icon(Some(icon))
                .map_err(|error| format!("set main tray numbers: {error}"))?;
        }
    }
    if let Ok(mut slot) = last_main_tray().lock() {
        slot.lefts = projection.remaining_percentages.clone();
        slot.tooltip = projection.tooltip.clone();
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Mac-style tray strip: a [provider logo][live numbers] icon pair per
// selected provider. The UI rasterizes each SVG logo to 32x32 RGBA (the
// webview already has the icons) and sends the pixels here.
// ---------------------------------------------------------------------------

struct LastMainTray {
    lefts: Vec<u32>,
    tooltip: String,
}

fn last_main_tray() -> &'static Mutex<LastMainTray> {
    static S: OnceLock<Mutex<LastMainTray>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(LastMainTray {
            lefts: Vec::new(),
            tooltip: String::from("AI Task Manager"),
        })
    })
}

fn last_strip() -> &'static Mutex<Vec<StripEntry>> {
    static S: OnceLock<Mutex<Vec<StripEntry>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Vec::new()))
}

fn tray_strip_apply_lock() -> &'static tauri::async_runtime::Mutex<()> {
    static LOCK: OnceLock<tauri::async_runtime::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tauri::async_runtime::Mutex::new(()))
}

#[derive(Clone, serde::Deserialize)]
struct StripEntry {
    id: String,
    logo: Vec<u8>, // 32x32 RGBA
    values: Vec<u32>,
    tooltip: String,
}

/// Every provider family that may appear in the tray strip. Frontend
/// strip ids are validated against this before becoming tray icon ids,
/// including `family@account` cards. Stale family-level strip icons are
/// removed for exactly this set.
const STRIP_PROVIDER_IDS: [&str; 23] = [
    "claude",
    "codex",
    "cursor",
    "opencode",
    "copilot",
    "grok",
    "devin",
    "minimax",
    "openrouter",
    "zai",
    "antigravity",
    "deepseek",
    "moonshot",
    "elevenlabs",
    "ollama",
    "codebuff",
    "kilo",
    "aihubmix",
    "qwen",
    "hermes",
    "kimi",
    "onenewapi",
    "sub2api",
];

async fn update_tray_strip(app: tauri::AppHandle, entries: Vec<StripEntry>) -> Result<(), String> {
    validate_strip_entries(&entries)?;
    let _guard = tray_strip_apply_lock().lock().await;
    let previous = last_strip()
        .lock()
        .map(|slot| slot.clone())
        .unwrap_or_default();
    let reset_ids = strip_reset_ids(&previous, &entries);
    let rebuild_order = !reset_ids.is_empty();
    let result = apply_tray_strip(
        app.clone(),
        entries.clone(),
        reset_ids,
        rebuild_order,
    )
    .await;
    if result.is_err() {
        if clear_tray_strip_icons(app, &previous, &entries)
            .await
            .is_ok()
        {
            if let Ok(mut slot) = last_strip().lock() {
                slot.clear();
            }
        }
        return result;
    }
    let Ok(mut slot) = last_strip().lock() else {
        return result;
    };
    commit_strip_state_after_apply(&mut slot, &entries, result)
}

fn commit_strip_state_after_apply(
    current: &mut Vec<StripEntry>,
    next: &[StripEntry],
    result: Result<(), String>,
) -> Result<(), String> {
    result?;
    *current = next.to_vec();
    Ok(())
}

fn strip_is_active(strip_ok: bool, entries: &[StripEntry]) -> bool {
    strip_ok && !entries.is_empty()
}

fn strip_icon_ids_to_clear(known: &[StripEntry], attempted: &[StripEntry]) -> Vec<String> {
    let mut ids: Vec<String> = STRIP_PROVIDER_IDS
        .iter()
        .map(|id| (*id).to_string())
        .collect();
    for entry in known.iter().chain(attempted) {
        if !ids.iter().any(|seen| seen == &entry.id) {
            ids.push(entry.id.clone());
        }
    }
    ids
}

#[tauri::command]
async fn sync_tray_surfaces(
    app: tauri::AppHandle,
    snapshots: Vec<providers::Snapshot>,
    projection: tray_projection::TrayProjectionConfig,
    entries: Vec<StripEntry>,
) -> Result<(), String> {
    let strip_result = update_tray_strip(app.clone(), entries.clone()).await;
    let main = tray_projection::project_main_tray(
        &snapshots,
        &projection,
        strip_is_active(strip_result.is_ok(), &entries),
    );
    let main_result = apply_main_tray_projection(&app, &main);
    match (main_result, strip_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(main_error), Err(strip_error)) => Err(format!("{main_error}; {strip_error}")),
    }
}

fn validate_strip_entries(entries: &[StripEntry]) -> Result<(), String> {
    if entries.len() > 4 {
        return Err("tray strip accepts at most 4 providers".into());
    }
    for (index, entry) in entries.iter().enumerate() {
        if !strip_provider_id_is_allowed(&entry.id) {
            return Err(format!("invalid tray strip provider id: {}", entry.id));
        }
        if entries[..index].iter().any(|seen| seen.id == entry.id) {
            return Err(format!("duplicate tray strip provider id: {}", entry.id));
        }
        if entry.logo.len() != 32 * 32 * 4 {
            return Err(format!("invalid tray strip logo for {}", entry.id));
        }
        if entry.values.is_empty() || entry.values.len() > 2 {
            return Err(format!("invalid tray strip values for {}", entry.id));
        }
    }
    Ok(())
}

fn strip_provider_id_is_allowed(id: &str) -> bool {
    match id.split_once('@') {
        None => STRIP_PROVIDER_IDS.contains(&id),
        Some((family, account)) => {
            STRIP_PROVIDER_IDS.contains(&family)
                && !account.is_empty()
                && account
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        }
    }
}

fn strip_tray_key(id: &str) -> String {
    id.replace('@', "--")
}

fn strip_reset_ids(previous: &[StripEntry], next: &[StripEntry]) -> Vec<String> {
    let same_order = previous.len() == next.len()
        && previous.iter().zip(next).all(|(old, new)| old.id == new.id);
    if same_order {
        return Vec::new();
    }

    let mut ids = Vec::new();
    for entry in previous.iter().chain(next) {
        if !ids.contains(&entry.id) {
            ids.push(entry.id.clone());
        }
    }
    ids
}

fn strip_entry_application_order(entries: &[StripEntry], rebuild_order: bool) -> Vec<&StripEntry> {
    let mut ordered: Vec<&StripEntry> = entries.iter().collect();
    if rebuild_order {
        // Windows inserts each new tray icon to the left. Rebuild Provider
        // pairs from right to left so their visible order matches providerOrder.
        ordered.reverse();
    }
    ordered
}

async fn clear_tray_strip_icons(
    app: tauri::AppHandle,
    known: &[StripEntry],
    attempted: &[StripEntry],
) -> Result<(), String> {
    let ids = strip_icon_ids_to_clear(known, attempted);
    let handle = app.clone();
    let (sender, mut receiver) = tauri::async_runtime::channel(1);
    app.run_on_main_thread(move || {
        for id in &ids {
            let key = strip_tray_key(id);
            handle.remove_tray_by_id(&format!("strip-logo-{key}"));
            handle.remove_tray_by_id(&format!("strip-num-{key}"));
        }
        let _ = sender.blocking_send(());
    })
    .map_err(|error| error.to_string())?;
    receiver
        .recv()
        .await
        .ok_or_else(|| "tray strip clear ended before reporting a result".to_string())
}

async fn apply_tray_strip(
    app: tauri::AppHandle,
    entries: Vec<StripEntry>,
    reset_ids: Vec<String>,
    rebuild_order: bool,
) -> Result<(), String> {
    let handle = app.clone();
    let (sender, mut receiver) = tauri::async_runtime::channel(1);
    app.run_on_main_thread(move || {
        let result = (|| -> Result<(), String> {
            // Removal returns None when an icon is already absent; that is
            // the desired end state rather than an update failure.
            for id in STRIP_PROVIDER_IDS {
                if !entries.iter().any(|entry| entry.id == id) {
                    handle.remove_tray_by_id(&format!("strip-logo-{id}"));
                    handle.remove_tray_by_id(&format!("strip-num-{id}"));
                }
            }
            for id in &reset_ids {
                let key = strip_tray_key(id);
                handle.remove_tray_by_id(&format!("strip-logo-{key}"));
                handle.remove_tray_by_id(&format!("strip-num-{key}"));
            }

            for entry in strip_entry_application_order(&entries, rebuild_order) {
                let tray_key = strip_tray_key(&entry.id);
                let logo_id = format!("strip-logo-{tray_key}");
                let num_id = format!("strip-num-{tray_key}");
                let logo_icon = tauri::image::Image::new_owned(entry.logo.clone(), 32, 32);
                let num_icon = tauri::image::Image::new_owned(
                    draw_tray_numbers(&entry.values),
                    32,
                    32,
                );
                let tooltip = entry.tooltip.clone();

                let new_trays = if let Some(tray) = handle.tray_by_id(&num_id) {
                    tray.set_icon(Some(num_icon))
                        .map_err(|error| format!("set {} strip numbers: {error}", entry.id))?;
                    tray.set_tooltip(Some(&tooltip))
                        .map_err(|error| format!("set {} strip tooltip: {error}", entry.id))?;
                    if let Some(logo_tray) = handle.tray_by_id(&logo_id) {
                        logo_tray.set_tooltip(Some(&tooltip)).map_err(|error| {
                            format!("set {} strip logo tooltip: {error}", entry.id)
                        })?;
                        Vec::new()
                    } else {
                        vec![(logo_id, logo_icon)]
                    }
                } else {
                    vec![(num_id, num_icon), (logo_id, logo_icon)]
                };

                // New pairs are numbers first: Windows inserts each new tray
                // icon to the left, yielding "logo | numbers" on screen.
                for (tray_id, icon) in new_trays {
                    TrayIconBuilder::with_id(tray_id)
                        .icon(icon)
                        .tooltip(&tooltip)
                        .show_menu_on_left_click(false)
                        .on_tray_icon_event(|tray, event| {
                            if let TrayIconEvent::Click {
                                button: MouseButton::Left,
                                button_state: MouseButtonState::Up,
                                position,
                                ..
                            } = event
                            {
                                toggle_popover(tray.app_handle(), position);
                            }
                        })
                        .build(&handle)
                        .map_err(|error| format!("build {} strip icon: {error}", entry.id))?;
                }
            }
            Ok(())
        })();
        let _ = sender.blocking_send(result);
    })
    .map_err(|error| error.to_string())?;
    receiver
        .recv()
        .await
        .ok_or_else(|| "tray strip update ended before reporting a result".to_string())?
}

// ---------------------------------------------------------------------------
// Usage fetching
// ---------------------------------------------------------------------------

/// A provider that just failed gets benched briefly instead of being
/// re-probed on every refresh: 60s for ordinary errors, 5 minutes for rate
/// limits (hammering a 429 makes it worse — learned that the hard way).
struct FailState {
    until_ms: i64,
    note: String,
}

fn fail_state() -> &'static Mutex<HashMap<String, FailState>> {
    static STATE: OnceLock<Mutex<HashMap<String, FailState>>> = OnceLock::new();
    STATE.get_or_init(Default::default)
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct CachedSnap {
    at: i64,
    snap: providers::Snapshot,
}

fn last_ok() -> &'static Mutex<HashMap<String, CachedSnap>> {
    static LAST_OK: OnceLock<Mutex<HashMap<String, CachedSnap>>> = OnceLock::new();
    LAST_OK.get_or_init(|| {
        let cache_file = providers::config_dir().join("last_snapshots.json");
        let loaded = std::fs::read_to_string(&cache_file)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        Mutex::new(loaded)
    })
}

fn persist_last_ok_at(
    path: &std::path::Path,
    map: &HashMap<String, CachedSnap>,
) -> Result<(), String> {
    let serialized =
        serde_json::to_string(map).map_err(|e| format!("serialize snapshot cache: {e}"))?;
    let parent = path
        .parent()
        .ok_or_else(|| "snapshot cache path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| format!("create snapshot cache dir: {e}"))?;
    std::fs::write(path, serialized).map_err(|e| format!("write snapshot cache: {e}"))
}

// Thread-local, not global: a process-wide one-shot flag gets stolen by
// whichever parallel test calls persist_last_ok inside the injecting
// test's store -> consume window, failing BOTH tests at once.
#[cfg(test)]
thread_local! {
    static TEST_PERSIST_LAST_OK_FAIL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
static SNAPSHOT_CACHE_NEEDS_FLUSH: AtomicBool = AtomicBool::new(false);

fn persist_last_ok(map: &HashMap<String, CachedSnap>) -> Result<(), String> {
    #[cfg(test)]
    if TEST_PERSIST_LAST_OK_FAIL.with(|fail| fail.replace(false)) {
        SNAPSHOT_CACHE_NEEDS_FLUSH.store(true, Ordering::Release);
        return Err("test: persist last_ok failed".into());
    }
    if cfg!(test) {
        SNAPSHOT_CACHE_NEEDS_FLUSH.store(false, Ordering::Release);
        return Ok(());
    }
    let cache_file = providers::config_dir().join("last_snapshots.json");
    let result = persist_last_ok_at(&cache_file, map);
    SNAPSHOT_CACHE_NEEDS_FLUSH.store(result.is_err(), Ordering::Release);
    result
}

fn forget_provider_snapshots(ids: &[String]) -> Result<(), String> {
    forget_provider_snapshots_inner(ids, false)
}

/// Drop cached snapshots for `ids`. Memory, fail-state, alerts, and the
/// local HTTP publication are always cleared so a failed disk write cannot
/// keep serving the old account. `persist_even_if_unchanged` rewrites the
/// cache file on retry after a persist failure (memory may already be clean).
fn forget_provider_snapshots_inner(
    ids: &[String],
    persist_even_if_unchanged: bool,
) -> Result<(), String> {
    let mut map = last_ok().lock().unwrap();
    let mut next = map.clone();
    let mut changed = false;
    for id in ids {
        changed |= next.remove(id).is_some();
    }
    *map = next.clone();
    drop(map);
    let mut failures = fail_state().lock().unwrap();
    for id in ids {
        failures.remove(id);
        alerts::forget_snapshot(id);
    }
    httpapi::forget_snapshots(ids);
    if changed || persist_even_if_unchanged {
        persist_last_ok(&next)?;
    }
    Ok(())
}

fn forget_provider_snapshot(id: &str) -> Result<(), String> {
    forget_provider_snapshots(&[id.to_string()])
}

fn forget_onenewapi_key_ids(key_ids: impl IntoIterator<Item = String>) -> Result<(), String> {
    let snapshot_ids: Vec<String> = key_ids
        .into_iter()
        .map(|key_id| format!("onenewapi@{key_id}"))
        .collect();
    forget_provider_snapshots(&snapshot_ids)
}

fn onenewapi_snapshot_ids(key_ids: &[String]) -> Vec<String> {
    key_ids.iter().map(|id| format!("onenewapi@{id}")).collect()
}

fn sub2api_snapshot_ids(key_ids: &[String]) -> Vec<String> {
    key_ids.iter().map(|id| format!("sub2api@{id}")).collect()
}

fn forget_sub2api_key_ids(key_ids: impl IntoIterator<Item = String>) -> Result<(), String> {
    forget_provider_snapshots(&sub2api_snapshot_ids(&key_ids.into_iter().collect::<Vec<_>>()))
}

fn purge_sub2api_cards(key_ids: &[String]) -> Result<(), String> {
    let ids = sub2api_snapshot_ids(key_ids);
    let restore = persist_key_cards_config_purge(&ids)?;
    if let Err(error) = forget_provider_snapshots(&ids) {
        return match restore_key_cards_config_purge(restore) {
            Ok(()) => Err(error),
            Err(restore_error) => Err(format!("{error}; restore card settings failed: {restore_error}")),
        };
    }
    Ok(())
}

fn sub2api_after_site_save(
    previous: &providers::sub2api::SiteDto,
    site: &providers::sub2api::SiteDto,
) -> Result<(), String> {
    if site.base_url == previous.base_url && site.name != previous.name {
        let renames = site.keys.iter().map(|key| (
            format!("sub2api@{}", key.id), format!("{} · {}", site.name, key.label),
        )).collect::<Vec<_>>();
        rename_cached_snapshots(&renames)?;
    }
    Ok(())
}

fn cached_onenewapi_id_is_configured(id: &str, configured: &HashSet<String>) -> bool {
    family_of(id) != "onenewapi" || configured.contains(id)
}

fn retain_current_key_card_results(
    all: &mut Vec<providers::Snapshot>,
    expected: &HashMap<String, u64>,
    current: &HashMap<String, u64>,
) -> Vec<String> {
    let stale: Vec<String> = all
        .iter()
        .filter(|snapshot| {
            is_credential_scoped_card(&snapshot.id)
                && expected.get(&snapshot.id) != current.get(&snapshot.id)
        })
        .map(|snapshot| snapshot.id.clone())
        .collect();
    let stale_set: HashSet<&str> = stale.iter().map(String::as_str).collect();
    all.retain(|snapshot| !stale_set.contains(snapshot.id.as_str()));
    stale
}

/// The "current" side of the generation check: one map covering every
/// credential-scoped snapshot in the batch. Built from the same id
/// universe as the expected side — if this ever narrows back to managed
/// cards only, every plain provider's `Some(0)` would compare unequal to
/// a missing entry and each refresh would silently drop its results.
fn current_credential_scoped_generations(all: &[providers::Snapshot]) -> HashMap<String, u64> {
    key_card_snapshot_generations(
        all.iter()
            .filter(|snapshot| is_credential_scoped_card(&snapshot.id))
            .map(|snapshot| snapshot.id.clone()),
    )
}

static KEY_CARD_MUTATION_GENERATION: AtomicU64 = AtomicU64::new(0);
static KEY_CARD_ACTIVE_MUTATIONS: AtomicU64 = AtomicU64::new(0);
static KEY_CARD_SNAPSHOT_GENERATIONS: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
// Serialize only cache/publication and local mutations, never network requests.
static KEY_CARD_PUBLICATION: Mutex<()> = Mutex::new(());

fn key_card_mutation_generation() -> u64 {
    KEY_CARD_MUTATION_GENERATION.load(Ordering::Acquire)
}

fn key_card_snapshot_generations(ids: impl IntoIterator<Item = String>) -> HashMap<String, u64> {
    let generations = KEY_CARD_SNAPSHOT_GENERATIONS
        .get_or_init(Default::default)
        .lock()
        .unwrap();
    ids.into_iter()
        .map(|id| {
            let generation = generations.get(&id).copied().unwrap_or(0);
            (id, generation)
        })
        .collect()
}

fn bump_key_card_snapshot_generations(ids: &[String]) {
    let mut generations = KEY_CARD_SNAPSHOT_GENERATIONS
        .get_or_init(Default::default)
        .lock()
        .unwrap();
    for id in ids {
        *generations.entry(id.clone()).or_default() += 1;
    }
}

struct KeyCardMutationGuard {
    snapshot_ids: Vec<String>,
    _publication: std::sync::MutexGuard<'static, ()>,
}

impl KeyCardMutationGuard {
    fn begin(snapshot_ids: Vec<String>) -> Self {
        let publication = KEY_CARD_PUBLICATION.lock().unwrap_or_else(|e| e.into_inner());
        KEY_CARD_ACTIVE_MUTATIONS.fetch_add(1, Ordering::AcqRel);
        bump_key_card_snapshot_generations(&snapshot_ids);
        KEY_CARD_MUTATION_GENERATION.fetch_add(1, Ordering::AcqRel);
        Self { snapshot_ids, _publication: publication }
    }

    fn track(&mut self, snapshot_ids: Vec<String>) {
        bump_key_card_snapshot_generations(&snapshot_ids);
        self.snapshot_ids.extend(snapshot_ids);
    }
}

impl Drop for KeyCardMutationGuard {
    fn drop(&mut self) {
        bump_key_card_snapshot_generations(&self.snapshot_ids);
        KEY_CARD_MUTATION_GENERATION.fetch_add(1, Ordering::AcqRel);
        KEY_CARD_ACTIVE_MUTATIONS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Strip deleted key cards from config, preserving their family choices.
/// Returns only changed config fields.
fn purge_key_cards_from_config(cfg: &mut Value, snapshot_ids: &[String]) -> Value {
    if snapshot_ids.is_empty() {
        return json!({});
    }
    let drop: HashSet<&str> = snapshot_ids.iter().map(String::as_str).collect();
    let mut patch = serde_json::Map::new();

    if let Some(arr) = cfg.get_mut("disabled").and_then(Value::as_array_mut) {
        let before = arr.len();
        arr.retain(|v| v.as_str().map(|s| !drop.contains(s)).unwrap_or(true));
        if arr.len() != before {
            patch.insert("disabled".into(), Value::Array(arr.clone()));
        }
    }

    let mut layout_changed = false;
    if let Some(layout) = cfg.get_mut("layout").and_then(Value::as_object_mut) {
        if let Some(order) = layout
            .get_mut("providerOrder")
            .and_then(Value::as_array_mut)
        {
            let before = order.len();
            order.retain(|v| v.as_str().map(|s| !drop.contains(s)).unwrap_or(true));
            layout_changed |= order.len() != before;
        }
        if let Some(providers) = layout.get_mut("providers").and_then(Value::as_object_mut) {
            for id in snapshot_ids {
                layout_changed |= providers.remove(id).is_some();
            }
        }
    }
    if layout_changed {
        if let Some(layout) = cfg.get("layout") {
            patch.insert("layout".into(), layout.clone());
        }
    }

    let pinned_hit = cfg
        .get("pinned")
        .and_then(|p| p.get("provider"))
        .and_then(Value::as_str)
        .is_some_and(|p| drop.contains(p));
    if pinned_hit {
        cfg["pinned"] = Value::Null;
        patch.insert("pinned".into(), Value::Null);
    }

    if let Some(arr) = cfg.get_mut("trayProviders").and_then(Value::as_array_mut) {
        let before = arr.len();
        arr.retain(|v| v.as_str().map(|s| !drop.contains(s)).unwrap_or(true));
        if arr.len() != before {
            patch.insert("trayProviders".into(), Value::Array(arr.clone()));
        }
    }

    Value::Object(patch)
}

fn key_cards_purge_restore_patch(original: &Value, purge_patch: &Value) -> Value {
    let mut restore = serde_json::Map::new();
    if let Some(obj) = purge_patch.as_object() {
        for key in obj.keys() {
            restore.insert(key.clone(), original.get(key).cloned().unwrap_or(Value::Null));
        }
    }
    Value::Object(restore)
}

fn persist_key_cards_config_purge(snapshot_ids: &[String]) -> Result<Value, String> {
    // Tests must not rewrite the developer's real config.json.
    if cfg!(test) {
        return Ok(json!({}));
    }
    let mut cfg = config_with_defaults(load_config());
    let original = cfg.clone();
    let patch = purge_key_cards_from_config(&mut cfg, snapshot_ids);
    let restore = key_cards_purge_restore_patch(&original, &patch);
    if patch.as_object().is_some_and(|o| !o.is_empty()) {
        set_config_inner(patch)?;
    }
    Ok(restore)
}

fn restore_key_cards_config_purge(restore: Value) -> Result<(), String> {
    if restore.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        return Ok(());
    }
    if cfg!(test) {
        return Ok(());
    }
    set_config_inner(restore).map(|_| ())
}

fn purge_onenewapi_cards(key_ids: &[String]) -> Result<(), String> {
    purge_onenewapi_cards_coordinated(
        key_ids,
        persist_key_cards_config_purge,
        |ids| forget_onenewapi_key_ids(ids.iter().cloned()),
        restore_key_cards_config_purge,
    )
}

#[cfg(test)]
fn purge_onenewapi_cards_with(
    key_ids: &[String],
    persist_config: impl FnOnce(&[String]) -> Result<(), String>,
) -> Result<(), String> {
    purge_onenewapi_cards_coordinated(
        key_ids,
        |ids| persist_config(ids).map(|()| json!({})),
        |ids| forget_onenewapi_key_ids(ids.iter().cloned()),
        |_| Ok(()),
    )
}

fn purge_onenewapi_cards_coordinated(
    key_ids: &[String],
    persist_config: impl FnOnce(&[String]) -> Result<Value, String>,
    forget: impl FnOnce(&[String]) -> Result<(), String>,
    restore_config: impl FnOnce(Value) -> Result<(), String>,
) -> Result<(), String> {
    if key_ids.is_empty() {
        return Ok(());
    }
    let restore = persist_config(&onenewapi_snapshot_ids(key_ids))?;
    if let Err(error) = forget(key_ids) {
        return match restore_config(restore) {
            Ok(()) => Err(error),
            Err(restore_error) => Err(format!(
                "{error}; restore card settings failed: {restore_error}"
            )),
        };
    }
    Ok(())
}

fn onenewapi_after_site_save(
    previous: &providers::onenewapi::SiteDto,
    site: &providers::onenewapi::SiteDto,
) -> Result<(), String> {
    if site.base_url != previous.base_url {
        return Ok(());
    }
    if site.name != previous.name {
        let renames: Vec<(String, String)> = site
            .keys
            .iter()
            .map(|key| {
                (
                    format!("onenewapi@{}", key.id),
                    format!("{} · {}", site.name, key.label),
                )
            })
            .collect();
        rename_cached_snapshots(&renames)?;
    }
    Ok(())
}

fn rename_cached_snapshot(id: &str, new_name: String) -> Result<(), String> {
    rename_cached_snapshots(&[(id.to_string(), new_name)])
}

fn rename_cached_snapshots(renames: &[(String, String)]) -> Result<(), String> {
    let mut map = last_ok().lock().unwrap();
    rename_cached_snapshots_in(&mut map, renames, persist_last_ok)?;
    httpapi::rename_snapshots(&renames.iter().cloned().collect());
    Ok(())
}

#[cfg(test)]
fn rename_cached_snapshot_in<Persist>(
    map: &mut HashMap<String, CachedSnap>,
    id: &str,
    new_name: String,
    persist: Persist,
) -> Result<(), String>
where
    Persist: FnOnce(&HashMap<String, CachedSnap>) -> Result<(), String>,
{
    rename_cached_snapshots_in(map, &[(id.to_string(), new_name)], persist)
}

fn rename_cached_snapshots_in<Persist>(
    map: &mut HashMap<String, CachedSnap>,
    renames: &[(String, String)],
    persist: Persist,
) -> Result<(), String>
where
    Persist: FnOnce(&HashMap<String, CachedSnap>) -> Result<(), String>,
{
    let mut next = map.clone();
    let mut changed = false;
    for (id, new_name) in renames {
        if let Some(entry) = next.get_mut(id) {
            if entry.snap.name != *new_name {
                entry.snap.name = new_name.clone();
                changed = true;
            }
        }
    }
    if changed {
        persist(&next)?;
        *map = next;
    }
    Ok(())
}

/// The plain API-key providers set_api_key accepts, in
/// %APPDATA%\AITaskManager\<provider>.json. Single source of truth for both the
/// save command's validation and the credential-context bookkeeping below.
const API_KEY_PROVIDERS: &[&str] = &[
    "openrouter",
    "zai",
    "minimax",
    "deepseek",
    "moonshot",
    "kimi",
    "elevenlabs",
    "codebuff",
    "kilo",
    "aihubmix",
    "qwen",
];

fn is_plain_api_key_provider(family: &str) -> bool {
    API_KEY_PROVIDERS.contains(&family)
}

/// Cards whose cached snapshots, cooldowns, and alerts belong to one
/// specific credential and must be dropped when that credential changes:
/// managed key cards plus the plain API-key providers. Everything else
/// (CLI-login families like claude/codex) is handled by the separate
/// cache-identity stamp, not by generations.
fn is_credential_scoped_card(id: &str) -> bool {
    let family = family_of(id);
    is_managed_key_card(id) || is_plain_api_key_provider(&family)
}

/// Snapshots to drop when this pasted key changes. Moonshot's wallet
/// folds into the Kimi card, so rotating Moonshot must also forget the
/// Kimi snapshot. Rotating only Kimi leaves the Moonshot snapshot —
/// that wallet key did not change.
fn api_key_snapshot_ids(provider: &str) -> Vec<String> {
    match provider {
        "moonshot" => vec!["moonshot".into(), "kimi".into()],
        _ => vec![provider.to_string()],
    }
}

/// Credit high-water marks belong to one pasted key. Kimi and Moonshot
/// do not share a pot — rotating Kimi must not zero Moonshot's meter.
fn api_key_baseline_ids(provider: &str) -> Vec<String> {
    vec![provider.to_string()]
}

// Owned id/name so dynamically discovered account cards (claude@<hash>)
// can ride the same guard as the static providers under a 'static spawn.
/// A snapshot younger than this is served as it is instead of being fetched
/// again. The background loop polls every minute; a refresh the user clicks
/// lands seconds after one of its ticks, and that second call inside the
/// same minute is what Anthropic answers with a 429 -- which then benched
/// the loop's own fetches for five minutes, so clicking Refresh made the
/// numbers *stop*. Two reports in two days (2026-09-22 and 23) were exactly
/// that. One minute is the app's own minimum polling interval, so the rule
/// this makes true for every caller -- the loop, a click, a reset timer --
/// is: never two vendor calls inside a minute. A minute-old number is
/// current; a 429 is not.
const FRESH_REUSE_MS: i64 = 60_000;

async fn guarded<F>(id: String, name: String, fut: F) -> providers::Snapshot
where
    F: std::future::Future<Output = providers::Snapshot>,
{
    let id = id.as_str();
    let name = name.as_str();
    // A credential rotation bumps this card's generation under the
    // publication lock. Capturing it before the request and comparing after
    // means a result that outlived its own key neither benches nor unbenches
    // the replacement's fail state (fetch_usage drops it separately).
    let expected_generation = key_card_snapshot_generations([id.to_string()])
        .get(id)
        .copied()
        .unwrap_or(0);
    let _credit_bind = CreditMeterBindGuard::begin(credit_meter_bind_ids(id));
    let now = now_ms() as i64;
    // The window yields to the loop, as the loop already yields to the
    // window: a good answer from a moment ago is the answer.
    let recent = {
        let map = last_ok().lock().unwrap();
        map.get(id)
            .filter(|c| c.snap.status == "ok" && now >= c.at && now - c.at <= FRESH_REUSE_MS)
            .map(|c| c.snap.clone())
    };
    if let Some(mut snap) = recent {
        snap.attempt_failed = false;
        snap.stale = false;
        return snap;
    }
    let benched = {
        let map = fail_state().lock().unwrap();
        map.get(id)
            .filter(|f| now < f.until_ms)
            .map(|f| f.note.clone())
    };
    if let Some(note) = benched {
        return providers::Snapshot::error(id, name, note);
    }
    let snap = fut.await;
    let generation_now = key_card_snapshot_generations([id.to_string()])
        .get(id)
        .copied()
        .unwrap_or(0);
    if generation_now != expected_generation {
        return snap;
    }
    let mut map = fail_state().lock().unwrap();
    if snap.status == "error" {
        let err = snap.error.clone().unwrap_or_default();
        let rate_limited = err.contains("429");
        // A vendor-stated Retry-After wins over our fixed backoff — bench
        // for exactly that long (capped at an hour) instead of knocking on
        // a door the server said stays shut.
        let retry_after_ms = err
            .split("retry_after_s=")
            .nth(1)
            .and_then(|rest| {
                rest.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse::<i64>()
                    .ok()
            })
            .map(|s| (s * 1000).min(3_600_000));
        let bench_ms = retry_after_ms.unwrap_or(if rate_limited { 300_000 } else { 60_000 });
        map.insert(
            id.to_string(),
            FailState {
                until_ms: now + bench_ms,
                note: if let Some(ms) = retry_after_ms {
                    format!(
                        "rate limited — the vendor asked to wait ~{}m",
                        (ms / 60_000).max(1)
                    )
                } else if rate_limited {
                    format!("rate limited — cooling down for a few minutes ({err})")
                } else {
                    err
                },
            },
        );
    } else {
        map.remove(id);
    }
    snap
}

/// Last-good Kimi snapshot on disk. Used to skip the leftover Moonshot
/// fetch only when that card has actually painted *recently* — a
/// credentials file, or a day-old cache entry, must not hide the wallet.
fn cached_kimi_ok() -> bool {
    let path = providers::config_dir().join("last_snapshots.json");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(doc) = serde_json::from_str::<Value>(&raw) else {
        return false;
    };
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    cached_kimi_ok_from(&doc, now_ms)
}

fn cached_kimi_ok_from(doc: &Value, now_ms: i64) -> bool {
    if doc.pointer("/kimi/snap/status").and_then(Value::as_str) != Some("ok") {
        return false;
    }
    let at = doc.pointer("/kimi/at").and_then(Value::as_i64).unwrap_or(0);
    at > 0 && now_ms.saturating_sub(at) <= SNAPSHOT_CACHE_MS
}

fn fold_moonshot_into_kimi(all: &mut Vec<providers::Snapshot>) {
    let Some(kimi) = all.iter().find(|s| s.id == "kimi" && s.status == "ok") else {
        return;
    };
    // Don't throw away a freshly fetched wallet just because the plan
    // card loaded. Fold only when Kimi already carries those rows, or
    // when Moonshot has nothing to show (plan-only / no_credentials).
    let kimi_has_wallet = kimi.metrics.iter().any(|m| is_kimi_wallet_label(&m.label));
    let moonshot_has_rows = all
        .iter()
        .any(|s| s.id == "moonshot" && !s.metrics.is_empty());
    if kimi_has_wallet || !moonshot_has_rows {
        all.retain(|s| s.id != "moonshot");
    }
}

fn is_kimi_wallet_label(label: &str) -> bool {
    matches!(
        label,
        "API" | "Credits used" | "Balance" | "Vouchers" | "Cash"
    )
}

fn restore_kimi_wallet_rows(current: &mut providers::Snapshot, previous: &providers::Snapshot) {
    if current.metrics.iter().any(|m| m.label == "API") {
        return;
    }
    for m in &previous.metrics {
        if is_kimi_wallet_label(&m.label) && !current.metrics.iter().any(|x| x.label == m.label) {
            current.metrics.push(m.clone());
        }
    }
}

fn restore_last_success_after_error(
    current: &mut providers::Snapshot,
    previous: &providers::Snapshot,
    age_ms: i64,
) -> bool {
    let sub2api = family_of(&current.id) == "sub2api";
    if current.status != "error" || (!sub2api && age_ms > SNAPSHOT_CACHE_MS) {
        return false;
    }
    let warning = current.error.clone();
    *current = previous.clone();
    current.attempt_failed = true;
    // The reason travels whatever the age. The card's Outdated chip is gated
    // on `stale`, so inside the grace window nothing new appears -- but the
    // footer of a refresh the user asked for, and the chip's tooltip once it
    // does show, can say WHY the numbers are held back instead of shrugging.
    current.warning = warning;
    if sub2api || age_ms > STALE_GRACE_MS {
        current.stale = true;
    }
    true
}

/// Old `last_snapshots.json` entries have no `fetched_at` on the snap
/// itself. The cache clock (`CachedSnap.at`) is the last success time.
fn hydrate_fetch_time(s: &mut providers::Snapshot, at: i64) {
    if s.fetched_at.is_none() {
        s.fetched_at = Some(at);
    }
}

/// When the last usage fetch and the last spend scan began (epoch ms), from
/// whichever side started them. The background loop reads these so it never
/// fetches on top of the window.
static LAST_USAGE_FETCH_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
static LAST_SPEND_SCAN_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
/// The spend scan reads local logs; ten minutes keeps feeds and the daily
/// spend alert current without walking the log tree every minute.
const BACKGROUND_SPEND_EVERY_MIN: i64 = 10;

/// Has `interval_min` passed since `last_ms`? A clock set backwards counts
/// as due, so a stale future timestamp cannot switch refreshing off.
fn refresh_due(last_ms: i64, now_ms: i64, interval_min: i64) -> bool {
    now_ms < last_ms || now_ms - last_ms >= interval_min.max(1) * 60_000
}

/// Keeps refreshing while the window is closed. The window's own refresh
/// runs on JavaScript timers, and macOS suspends those in a hidden webview:
/// a tray app is hidden nearly all the time, so history recording, the
/// budget alerts, renewal reminders and the API feeds all stalled for hours
/// (a 192-minute hole showed up in a real history file). This loop lives in
/// Rust, where nothing suspends it. While the window is open and refreshing
/// it stays quiet, because the timestamps above are shared.
fn spawn_background_refresh(app: &tauri::AppHandle) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let cfg = config_with_defaults(load_config());
            let every = cfg.get("refreshMinutes").and_then(Value::as_i64).unwrap_or(1);
            let now = chrono::Utc::now().timestamp_millis();
            let last_usage = LAST_USAGE_FETCH_MS.load(std::sync::atomic::Ordering::Relaxed);
            // Half a minute of grace lets an open window go first.
            if refresh_due(last_usage + 30_000, now, every) {
                let _ = fetch_usage(handle.clone(), None).await;
            }
            let last_spend = LAST_SPEND_SCAN_MS.load(std::sync::atomic::Ordering::Relaxed);
            if refresh_due(last_spend, now, BACKGROUND_SPEND_EVERY_MIN) {
                let _ = fetch_spend(handle.clone()).await;
            }
        }
    });
}

/// Called by the UI. Refreshes every enabled provider at the same time and
/// returns whatever each one found — data, "not signed in", or an error.
#[tauri::command]
async fn fetch_usage(
    app: tauri::AppHandle,
    disabled: Option<Vec<String>>,
) -> Vec<providers::Snapshot> {
    LAST_USAGE_FETCH_MS.store(chrono::Utc::now().timestamp_millis(), std::sync::atomic::Ordering::Relaxed);
    let cfg = config_with_defaults(load_config());
    let disabled = disabled.unwrap_or_else(|| {
        cfg.get("disabled")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    });

    // Bind the default OpenCode fingerprint to this refresh so a swap of
    // auth.json while the request is in flight cannot cache the old key's
    // numbers under the new identity.
    let opencode_identity_at_start = providers::opencode::default_identity();

    // Each provider future is boxed onto the heap and spawned as its own
    // task. A single tokio::join! over 28 inlined futures builds one huge
    // combined state machine on the calling thread's stack — at 28 providers
    // that overflowed the main thread's 1 MB stack and killed the app.
    type BoxedSnap =
        std::pin::Pin<Box<dyn std::future::Future<Output = providers::Snapshot> + Send>>;
    // Disabled providers are skipped BEFORE anything is spawned — a merely
    // post-filtered provider still did all its work invisibly: network
    // calls, file reads, and in Kiro's case spawning a CLI whose own
    // auto-updater downloaded a fresh installer to %TEMP% on every refresh
    // (gigabytes within days). Futures are lazy, so building and dropping
    // a disabled entry here runs none of its code.
    let base: Vec<(&str, BoxedSnap)> = vec![
        (
            "claude",
            Box::pin(guarded(
                "claude".into(),
                "Claude".into(),
                providers::claude::snapshot(),
            )),
        ),
        (
            "codex",
            Box::pin(guarded(
                "codex".into(),
                "Codex".into(),
                providers::codex::snapshot(),
            )),
        ),
        (
            "cursor",
            Box::pin(guarded(
                "cursor".into(),
                "Cursor".into(),
                providers::cursor::snapshot(),
            )),
        ),
        (
            "opencode",
            Box::pin(guarded(
                "opencode".into(),
                "OpenCode".into(),
                providers::opencode::snapshot(),
            )),
        ),
        (
            "copilot",
            Box::pin(guarded(
                "copilot".into(),
                "Copilot".into(),
                providers::copilot::snapshot(),
            )),
        ),
        (
            "grok",
            Box::pin(guarded(
                "grok".into(),
                "Grok".into(),
                providers::grok::snapshot(),
            )),
        ),
        (
            "devin",
            Box::pin(guarded(
                "devin".into(),
                "Devin".into(),
                providers::devin::snapshot(),
            )),
        ),
        (
            "minimax",
            Box::pin(guarded(
                "minimax".into(),
                "MiniMax".into(),
                providers::minimax::snapshot(),
            )),
        ),
        (
            "openrouter",
            Box::pin(guarded(
                "openrouter".into(),
                "OpenRouter".into(),
                providers::openrouter::snapshot(),
            )),
        ),
        (
            "zai",
            Box::pin(guarded(
                "zai".into(),
                "Z.ai".into(),
                providers::zai::snapshot(),
            )),
        ),
        (
            "antigravity",
            Box::pin(guarded(
                "antigravity".into(),
                "Antigravity".into(),
                providers::antigravity::snapshot(),
            )),
        ),
        (
            "deepseek",
            Box::pin(guarded(
                "deepseek".into(),
                "DeepSeek".into(),
                providers::deepseek::snapshot(),
            )),
        ),
        (
            "moonshot",
            Box::pin(guarded(
                "moonshot".into(),
                "Kimi API".into(),
                providers::moonshot::snapshot(),
            )),
        ),
        (
            "elevenlabs",
            Box::pin(guarded(
                "elevenlabs".into(),
                "ElevenLabs".into(),
                providers::elevenlabs::snapshot(),
            )),
        ),
        (
            "ollama",
            Box::pin(guarded(
                "ollama".into(),
                "Ollama".into(),
                providers::ollama::snapshot(),
            )),
        ),
        (
            "codebuff",
            Box::pin(guarded(
                "codebuff".into(),
                "Codebuff".into(),
                providers::codebuff::snapshot(),
            )),
        ),
        (
            "kilo",
            Box::pin(guarded(
                "kilo".into(),
                "Kilo".into(),
                providers::kilo::snapshot(),
            )),
        ),
        (
            "aihubmix",
            Box::pin(guarded(
                "aihubmix".into(),
                "AihubMix".into(),
                providers::aihubmix::snapshot(),
            )),
        ),
        (
            "qwen",
            Box::pin(guarded(
                "qwen".into(),
                "Qwen Code".into(),
                providers::qwen::snapshot(),
            )),
        ),
        (
            "hermes",
            Box::pin(guarded(
                "hermes".into(),
                "Hermes".into(),
                providers::hermes::snapshot(),
            )),
        ),
        (
            "kimi",
            Box::pin(guarded(
                "kimi".into(),
                "Kimi Code".into(),
                providers::kimi::snapshot(),
            )),
        ),
    ];
    // Skip the leftover Moonshot fetch only when the last Kimi card
    // actually painted — a credentials file alone is not enough (expired
    // login / network blip would otherwise hide the wallet with nothing
    // to fall back to). The post-fetch retain still drops it whenever
    // this cycle's Kimi snapshot is ok.
    let kimi_card_live = cached_kimi_ok();
    let mut futs: Vec<(String, BoxedSnap)> = base
        .into_iter()
        .filter(|(id, _)| {
            *id != "moonshot"
                || !providers::kimi::has_credentials()
                || disabled.iter().any(|d| d == "kimi")
                || !kimi_card_live
        })
        .map(|(id, fut)| (id.to_string(), fut))
        .collect();
    // Extra Claude accounts (multi-login machines): each discovered config
    // dir renders its own card under a claude@<hash8> id, running the same
    // provider flow scoped to its dir. The default login keeps the bare id.
    for acct in providers::claude::discover_extra_accounts() {
        let (id, name, dir) = (acct.id, acct.name, acct.dir);
        futs.push((
            id.clone(),
            Box::pin(guarded(
                id.clone(),
                name.clone(),
                providers::claude::snapshot_at(dir, id, name),
            )),
        ));
    }
    for acct in providers::codex::discover_extra_accounts() {
        let (id, name, dir) = (acct.id, acct.name, acct.dir);
        futs.push((
            id.clone(),
            Box::pin(guarded(
                id.clone(),
                name.clone(),
                providers::codex::snapshot_at(dir, id, name),
            )),
        ));
    }
    for acct in providers::opencode::discover_extra_accounts() {
        let (id, name, dir, fp) = (acct.id, acct.name, acct.dir, acct.fingerprint);
        futs.push((
            id.clone(),
            Box::pin(guarded(
                id.clone(),
                name.clone(),
                providers::opencode::snapshot_at(dir, id, name, Some(fp)),
            )),
        ));
    }
    // Plain API-key providers ride the same generation scheme as managed
    // key cards: set_api_key bumps a provider's generation when its stored
    // credential actually changes, so a request the old key started can be
    // refused at every write-back below (cache, cooldown, publication).
    let mut expected_key_card_generations = key_card_snapshot_generations(
        futs.iter()
            .map(|(id, _)| id.clone())
            .filter(|id| is_credential_scoped_card(id) && !is_managed_key_card(id)),
    );
    let onenewapi_generation_before = key_card_mutation_generation();
    let onenewapi_active_before = KEY_CARD_ACTIVE_MUTATIONS.load(Ordering::Acquire);
    if !disabled.iter().any(|d| d == "onenewapi") {
        if let Ok(cards) = providers::onenewapi::prepare_key_cards().await {
            for (id, generation) in
                key_card_snapshot_generations(cards.iter().map(|card| card.id.clone()))
            {
                expected_key_card_generations.insert(id, generation);
            }
            let onenewapi_generation_after = key_card_mutation_generation();
            let onenewapi_active_after = KEY_CARD_ACTIVE_MUTATIONS.load(Ordering::Acquire);
            let stable = onenewapi_active_before == 0
                && onenewapi_active_after == 0
                && onenewapi_generation_before == onenewapi_generation_after;
            if stable {
                let clients = providers::onenewapi::refresh_clients(&cards);
                for card in cards {
                    let client = clients
                        .get(&card.origin)
                        .cloned()
                        .unwrap_or_else(providers::http_no_redirect);
                    let (id, name) = (card.id.clone(), card.name.clone());
                    futs.push((
                        id.clone(),
                        Box::pin(guarded(
                            id,
                            name,
                            providers::onenewapi::snapshot_key_with_client(client, card),
                        )),
                    ));
                }
            } else {
                expected_key_card_generations.retain(|id, _| !is_managed_key_card(id));
            }
        }
    }
    if !disabled.iter().any(|d| d == "sub2api") {
        let before = key_card_mutation_generation();
        let active_before = KEY_CARD_ACTIVE_MUTATIONS.load(Ordering::Acquire);
        if let Ok(cards) = providers::sub2api::key_cards() {
            let generations = key_card_snapshot_generations(cards.iter().map(|card| card.id.clone()));
            if active_before == 0
                && KEY_CARD_ACTIVE_MUTATIONS.load(Ordering::Acquire) == 0
                && before == key_card_mutation_generation()
            {
                expected_key_card_generations.extend(generations);
                let clients = providers::sub2api::refresh_clients(&cards);
                for card in cards {
                    let client = clients.get(&card.origin).cloned()
                        .unwrap_or_else(providers::http_no_redirect);
                    let (id, name) = (card.id.clone(), card.name.clone());
                    futs.push((id.clone(), Box::pin(guarded(
                        id, name, providers::sub2api::snapshot_key_with_client(client, card),
                    ))));
                }
            }
        }
    }
    let futs: Vec<(String, BoxedSnap)> = futs
        .into_iter()
        .filter(|(id, _)| !card_is_disabled(id, &disabled))
        .collect();
    let handles: Vec<_> = futs
        .into_iter()
        .map(|(_, fut)| tauri::async_runtime::spawn(fut))
        .collect();
    let mut all = Vec::with_capacity(handles.len());
    for h in handles {
        if let Ok(mut snap) = h.await {
            // Stamp each provider as it lands — not once after the
            // slowest sibling finishes — so fetchedAt is that card's
            // last success, not the batch join clock.
            if snap.status == "ok" && snap.fetched_at.is_none() {
                snap.fetched_at = Some(now_ms() as i64);
            }
            all.push(snap);
        }
    }
    let _publication = KEY_CARD_PUBLICATION.lock().unwrap_or_else(|e| e.into_inner());
    let current_key_card_generations = current_credential_scoped_generations(&all);
    let stale_key_card_ids = retain_current_key_card_results(
        &mut all,
        &expected_key_card_generations,
        &current_key_card_generations,
    );
    if !stale_key_card_ids.is_empty() {
        let mut failures = fail_state().lock().unwrap();
        for id in stale_key_card_ids {
            failures.remove(&id);
        }
    }
    let opencode_identity_now = providers::opencode::default_identity();
    let opencode_swapped_mid_refresh = matches!(
        (&opencode_identity_at_start, &opencode_identity_now),
        (Some(old), Some(current)) if old != current
    );
    if opencode_swapped_mid_refresh {
        for s in &mut all {
            if s.id == "opencode" {
                *s = providers::Snapshot::error(
                    "opencode",
                    "OpenCode",
                    "OpenCode login changed during refresh.".into(),
                );
            }
        }
    }

    for s in &all {
        let log_family = family_of(&s.id);
        let log_id = if is_managed_key_card(&s.id) {
            log_family.as_str()
        } else {
            s.id.as_str()
        };
        eprintln!(
            "[aitm] {}: {} ({} metrics){}",
            log_id,
            s.status,
            s.metrics.len(),
            s.error
                .as_deref()
                .map(|e| format!(" — {e}"))
                .unwrap_or_default()
        );
    }

    // Transient server errors (a 503, a timeout) shouldn't blank a card the
    // user was just reading: fall back to the last good snapshot, marked
    // stale so the UI can say "Outdated" with the real error on hover. The
    // cache survives app restarts. Sub2API keeps explicitly stale history
    // until its credential context changes; other providers keep the
    // existing one-day limit.
    {
        let cache = last_ok();
        // Cache identity stamp (upstream's Phase 1): if a DIFFERENT account
        // signed into a default home since the cache was written, that
        // family's cached last-good snapshot belongs to the old account —
        // drop it instead of painting the wrong account's numbers under the
        // bare id. Extra-account cards are immune: their ids are derived
        // from the account identity itself.
        {
            let stamp_file = providers::config_dir().join("cache_identities.json");
            let current = json!({
                "claude": providers::claude::default_identity(),
                "codex": providers::codex::default_identity(),
                "opencode": providers::opencode::default_identity(),
            });
            let stored: Value = std::fs::read_to_string(&stamp_file)
                .ok()
                .and_then(|raw| serde_json::from_str(&raw).ok())
                .unwrap_or_else(|| json!({}));
            let mut map = cache.lock().unwrap();
            let mut removed = false;
            let mut to_store = serde_json::Map::new();
            for fam in ["claude", "codex", "opencode"] {
                let cur = current.get(fam).cloned().unwrap_or(Value::Null);
                let old = stored.get(fam).cloned().unwrap_or(Value::Null);
                // Only a KNOWN stored identity differing from a KNOWN
                // current one is evidence of an account swap. A missing
                // stamp (first launch after updating) or a momentarily
                // unreadable identity file must not dump the last-good
                // cache — that's the safety net, not a swap.
                if !old.is_null() && !cur.is_null() && old != cur && map.remove(fam).is_some() {
                    removed = true;
                } else if fam == "opencode"
                    && opencode_swapped_mid_refresh
                    && map.remove(fam).is_some()
                {
                    // Mid-refresh A→B with two known fingerprints: drop
                    // the last-good so error restore cannot paint A as B.
                    removed = true;
                } else if fam == "opencode"
                    && old.is_null()
                    && !cur.is_null()
                    && map.remove(fam).is_some()
                {
                    // First stamp after upgrade: the cached snapshot
                    // predates identity tracking and may belong to a
                    // previous login. Drop it rather than pin it to the
                    // current key.
                    removed = true;
                }
                // And a transient null never OVERWRITES a known identity:
                // erasing it would make a swap that happens before the next
                // launch undetectable.
                to_store.insert(
                    fam.to_string(),
                    if cur.is_null() && !old.is_null() {
                        old
                    } else {
                        cur
                    },
                );
            }
            // Persist the PRUNED cache before the new stamp: if this
            // refresh finds nothing ok (offline launch) the on-disk cache
            // would otherwise keep the old account's entry while the stamp
            // already claims the new one, resurrecting the wrong numbers
            // next launch. Stamp last, so a failed write just re-prunes.
            let cache_persisted = !removed || persist_last_ok(&map).is_ok();
            drop(map);
            let to_store = Value::Object(to_store);
            if cache_persisted && to_store != stored {
                let _ = std::fs::write(
                    &stamp_file,
                    serde_json::to_string_pretty(&to_store).unwrap_or_default(),
                );
            }
        }
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        if let Ok(mut map) = cache.lock() {
            let mut dirty = false;
            for s in all.iter_mut() {
                if is_credential_scoped_card(&s.id) {
                    let current = key_card_snapshot_generations([s.id.clone()]);
                    if expected_key_card_generations.get(&s.id) != current.get(&s.id) {
                        continue;
                    }
                }
                // Plan bars can succeed while the folded Moonshot wallet
                // call fails; keep last-known API/Balance rows so Almost
                // Out and the tray pin don't blink off for one timeout.
                // Do not re-cache the patched snapshot — that would reset
                // `at` and keep serving the same balance forever.
                let mut skip_cache = false;
                if s.id == "kimi" && s.status == "ok" && s.warning.is_some() {
                    if let Some(previous) = map.get("kimi") {
                        let age = now_ms - previous.at;
                        if age <= SNAPSHOT_CACHE_MS {
                            let n = s.metrics.len();
                            restore_kimi_wallet_rows(s, &previous.snap);
                            if s.metrics.len() > n {
                                skip_cache = true;
                                s.attempt_failed = true;
                                if age > STALE_GRACE_MS {
                                    s.stale = true;
                                }
                            }
                        }
                    }
                }
                if s.status == "ok" && !skip_cache {
                    let at = s.fetched_at.unwrap_or(now_ms);
                    s.fetched_at = Some(at);
                    s.attempt_failed = false;
                    map.insert(
                        s.id.clone(),
                        CachedSnap {
                            at,
                            snap: s.clone(),
                        },
                    );
                    dirty = true;
                } else if s.status == "error" {
                    if let Some(previous) = map.get(&s.id) {
                        let age = now_ms - previous.at;
                        let previous_at = previous.at;
                        if restore_last_success_after_error(s, &previous.snap, age) {
                            hydrate_fetch_time(s, previous_at);
                        }
                    }
                }
            }
            if dirty {
                if let Err(error) = persist_last_ok(&map) {
                    eprintln!("[aitm] snapshot cache refresh: {error}");
                }
            }
        }
    }

    // Recheck before publishing; the publication lock keeps local mutations
    // from interleaving cache updates, HTTP publication, and alerts.
    let current_key_card_generations = current_credential_scoped_generations(&all);
    let stale_key_card_ids = retain_current_key_card_results(
        &mut all,
        &expected_key_card_generations,
        &current_key_card_generations,
    );
    if !stale_key_card_ids.is_empty() {
        let mut failures = fail_state().lock().unwrap();
        for id in stale_key_card_ids {
            failures.remove(&id);
        }
    }

    // One Kimi card: Session / Weekly / API. Hide the leftover Moonshot
    // wallet card whenever the plan card is actually showing.
    fold_moonshot_into_kimi(&mut all);

    // A user may disable a family or key while its request is in flight.
    let publish_cfg = config_with_defaults(load_config());
    let publish_disabled = publish_cfg.get("disabled").and_then(Value::as_array)
        .map(|ids| ids.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>())
        .unwrap_or_default();
    all.retain(|snapshot| !card_is_disabled(&snapshot.id, &publish_disabled));
    httpapi::publish(&all);

    // Local history for the detail view's charts. Never fails a refresh.
    history::record(&all);

    // Renewal reminders: one per renewal, persisted as sent.
    let remind_days = cfg.get("renewalReminderDays").and_then(Value::as_i64).unwrap_or(0);
    for due in ledger::take_reminders_in(&ledger::path(), chrono::Local::now().date_naive(), remind_days) {
        use tauri_plugin_notification::NotificationExt;
        let price = format!("{:.2}", due.subscription.price);
        let cycle_key = match due.subscription.cycle {
            ledger::Cycle::Monthly => "notify.renewal.cycleMonthly",
            ledger::Cycle::Yearly => "notify.renewal.cycleYearly",
        };
        let cycle = i18n::Msg::new(cycle_key);
        let body = match due.days_left {
            Some(0) => i18n::Msg::new("notify.renewal.today"),
            Some(1) => i18n::Msg::new("notify.renewal.tomorrow"),
            // Only "one" is unreachable here (0 and 1 have their own keys
            // above); every n >= 2 the app can produce needs a real plural
            // form, so this is a genuine count.
            Some(n) => i18n::Msg::new("notify.renewal.inDays").count(n),
            None => continue,
        }
        .var("name", &due.subscription.name)
        .var("price", price)
        .sub("cycle", cycle);
        let _ = app
            .notification()
            .builder()
            .title(i18n::t(&cfg, &i18n::Msg::new("notify.renewal.title")))
            .body(i18n::t(&cfg, &body))
            .show();
    }

    for alert in alerts::evaluate(&all, &cfg) {
        use tauri_plugin_notification::NotificationExt;
        let _ = app
            .notification()
            .builder()
            .title(i18n::t(&cfg, &alert.title))
            .body(i18n::t(&cfg, &alert.body))
            .show();
    }

    all
}

/// The previous run's last-good snapshots, straight from the disk cache —
/// the instant first paint at launch. Cards show numbers in milliseconds
/// instead of a blank "Refreshing…" while the slowest provider answers
/// (at boot, with the network still coming up, that wait ran 30-40 s).
/// Everything is marked stale; the first live fetch replaces it.
#[tauri::command]
fn cached_usage() -> Vec<providers::Snapshot> {
    let _publication = KEY_CARD_PUBLICATION.lock().unwrap_or_else(|e| e.into_inner());
    #[derive(serde::Deserialize)]
    struct CachedSnap {
        at: i64,
        snap: providers::Snapshot,
    }
    const MAX_STALE_MS: i64 = SNAPSHOT_CACHE_MS;
    let Ok(raw) = std::fs::read_to_string(providers::config_dir().join("last_snapshots.json"))
    else {
        return Vec::new();
    };
    let Ok(map) = serde_json::from_str::<std::collections::HashMap<String, CachedSnap>>(&raw)
    else {
        return Vec::new();
    };

    let cfg = config_with_defaults(load_config());
    let disabled: Vec<String> = cfg
        .get("disabled")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let configured_onenewapi: HashSet<String> = providers::onenewapi::key_cards()
        .map(|cards| cards.into_iter().map(|card| card.id).collect())
        .unwrap_or_default();
    let configured_sub2api: HashSet<String> = providers::sub2api::key_cards()
        .map(|cards| cards.into_iter().map(|card| card.id).collect())
        .unwrap_or_default();

    // Same account-swap rule as the live path: if a different account
    // signed into a default home since the cache was written, that
    // family's bare-id entry belongs to the old account — never paint it,
    // not even for the seconds until the live fetch lands.
    let stored: Value =
        std::fs::read_to_string(providers::config_dir().join("cache_identities.json"))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_else(|| json!({}));
    let swapped: Vec<&str> = [
        ("claude", providers::claude::default_identity()),
        ("codex", providers::codex::default_identity()),
        ("opencode", providers::opencode::default_identity()),
    ]
    .into_iter()
    .filter(|(fam, current)| {
        let old = stored.get(fam).cloned().unwrap_or(Value::Null);
        matches!((current, &old), (Some(cur), Value::String(o)) if cur != o)
    })
    .map(|(fam, _)| fam)
    .collect();

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let mut out: Vec<providers::Snapshot> = map
        .into_iter()
        .filter(|(id, c)| {
            (family_of(id) == "sub2api" || now_ms - c.at <= MAX_STALE_MS)
                && !card_is_disabled(id, &disabled)
                && cached_onenewapi_id_is_configured(id, &configured_onenewapi)
                && (family_of(id) != "sub2api" || configured_sub2api.contains(id))
                && !swapped.iter().any(|f| f == id)
        })
        .map(|(_, c)| {
            let mut s = c.snap;
            s.stale = true;
            hydrate_fetch_time(&mut s, c.at);
            s
        })
        .collect();
    fold_moonshot_into_kimi(&mut out);
    out.sort_by(|a, b| a.id.cmp(&b.id));
    httpapi::publish_restored_sub2api(&out);
    out
}

/// Computes local spend (Today / Yesterday / Last 30 Days) from the CLIs'
/// own session logs. Heavy file IO, so it runs on a blocking thread.
#[tauri::command]
async fn fetch_spend(app: tauri::AppHandle) -> Vec<spend::ProviderSpend> {
    LAST_SPEND_SCAN_MS.store(chrono::Utc::now().timestamp_millis(), std::sync::atomic::Ordering::Relaxed);
    eprintln!("[aitm] spend: scan starting");
    let started = std::time::Instant::now();
    // Cursor's CSV export needs the async client; fetch it here and hand it
    // to the blocking scan. Unlike every other spend source it's an
    // authenticated NETWORK call, so it honors the disabled toggle the same
    // way fetch_usage does — a switched-off Cursor makes no requests.
    let cursor_disabled = config_with_defaults(load_config())
        .get("disabled")
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().any(|v| v.as_str() == Some("cursor")));
    // CSV is a network call. Don't make the local log walk sit behind it —
    // Codex/Claude appends are the slow part, and they don't need Cursor.
    let csv_task = async {
        if cursor_disabled {
            None
        } else {
            providers::cursor::fetch_usage_csv().await
        }
    };
    let scan_task = tauri::async_runtime::spawn_blocking(|| spend::collect(None));
    let (cursor_csv, local) = tokio::join!(csv_task, scan_task);
    let mut result = local.unwrap_or_default();
    if let Some(csv) = cursor_csv {
        let cursor = spend::cursor_from_csv(&csv);
        if spend::provider_spend_has_data(&cursor) {
            if cursor.unpriced > 0 {
                pricing::note_unpriced();
            }
            result.push(cursor);
        }
    }
    eprintln!(
        "[aitm] spend: {} providers in {:?}",
        result.len(),
        started.elapsed()
    );
    // Optional dashboard feeds on the loopback API. Off by default; when off
    // nothing is published, so the paths do not exist.
    {
        let cfg = config_with_defaults(load_config());
        let on = cfg.get("apiFeeds").and_then(Value::as_bool).unwrap_or(false);
        let today = chrono::Local::now().date_naive();
        let areas: Vec<spend::AreaSpend> = result
            .iter()
            .flat_map(|p| p.projects.iter())
            .flat_map(|pr| pr.areas.iter().cloned())
            .collect();
        let usage30: std::collections::HashMap<String, f64> =
            result.iter().map(|p| (p.id.clone(), p.last30.cost)).collect();
        httpapi::publish_feeds(
            on,
            vec![
                ("/v1/spend", httpapi::spend_feed(&result)),
                ("/v1/spend/areas", httpapi::areas_feed(&result)),
                (
                    "/v1/spend/clients",
                    serde_json::to_value(clients::rollup(&areas, &clients::load_from(&clients::path()), today))
                        .unwrap_or(Value::Null),
                ),
                (
                    "/v1/subscriptions",
                    serde_json::to_value(ledger::view(&ledger::load_from(&ledger::path()), today, &usage30))
                        .unwrap_or(Value::Null),
                ),
            ],
        );
    }

    // Client budgets and the weekly digest. Both remember what they have
    // already said in a small state file, so a restart does not repeat them.
    {
        use tauri_plugin_notification::NotificationExt;
        let cfg = config_with_defaults(load_config());
        let now = chrono::Local::now();
        let today = now.date_naive();
        let marks_path = providers::config_dir().join("alert_marks.json");
        let mut marks: Value = std::fs::read_to_string(&marks_path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_else(|| json!({}));
        let mut changed = false;

        let rules = clients::load_from(&clients::path());
        if rules.iter().any(|r| r.monthly_budget.is_some()) {
            let areas: Vec<spend::AreaSpend> = result
                .iter()
                .flat_map(|p| p.projects.iter())
                .flat_map(|pr| pr.areas.iter().cloned())
                .collect();
            let rows = clients::rollup(&areas, &rules, today);
            let mut fired: Vec<String> = marks
                .get("budgetFired")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default();
            for (client, spent, budget) in clients::over_budget(&rows, &rules, today, &mut fired) {
                let body = i18n::Msg::new("notify.clientBudget.body")
                    .var("client", &client)
                    .var("spent", format!("{spent:.0}"))
                    .var("budget", format!("{budget:.0}"));
                let _ = app
                    .notification()
                    .builder()
                    .title(i18n::t(&cfg, &i18n::Msg::new("notify.clientBudget.title")))
                    .body(i18n::t(&cfg, &body))
                    .show();
                changed = true;
            }
            marks["budgetFired"] = json!(fired);
        }

        // Session hygiene: an old, costly session that is still being used.
        let nudge_days = cfg.get("sessionNudgeDays").and_then(Value::as_i64).unwrap_or(0);
        if nudge_days > 0 {
            let mut fired: Vec<String> = marks
                .get("sessionNudged")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default();
            let due = coaching::sessions_to_nudge(
                &spend::claude_sessions(None, None, 500),
                chrono::Utc::now().timestamp_millis(),
                nudge_days,
                &digest::week_mark(today),
                &mut fired,
            );
            // One notification, about the costliest: several at once is noise.
            if let Some((_, days, cost)) = due.first() {
                let mut body = i18n::t(
                    &cfg,
                    &i18n::Msg::new("notify.longSession.body")
                        .var("days", format!("{days:.0}"))
                        .var("cost", format!("{cost:.0}")),
                );
                // The tail is only rendered and appended when there IS one
                // (n > 0), so its "one" plural form is genuinely reachable.
                let others = due.len() - 1;
                if others > 0 {
                    body.push(' ');
                    body.push_str(&i18n::t(&cfg, &i18n::Msg::new("notify.longSession.others").count(others as i64)));
                }
                let _ = app
                    .notification()
                    .builder()
                    .title(i18n::t(&cfg, &i18n::Msg::new("notify.longSession.title")))
                    .body(body)
                    .show();
                changed = true;
            }
            marks["sessionNudged"] = json!(fired);
        }

        let setting = cfg.get("weeklyDigest").and_then(Value::as_str).unwrap_or("off");
        let last = marks.get("digestWeek").and_then(Value::as_str).map(str::to_string);
        use chrono::Timelike;
        if digest::due(setting, today, now.hour(), last.as_deref()) {
            let usage30: std::collections::HashMap<String, f64> =
                result.iter().map(|p| (p.id.clone(), p.last30.cost)).collect();
            let ledger_view = ledger::view(&ledger::load_from(&ledger::path()), today, &usage30);
            if let Some(d) = digest::build(&result, &ledger_view) {
                let body = d.body.iter().map(|m| i18n::t(&cfg, m)).collect::<Vec<_>>().join(" ");
                let _ = app.notification().builder().title(i18n::t(&cfg, &d.title)).body(body).show();
            }
            // Marked even when there was nothing to say: an empty week is
            // not retried every ten minutes.
            marks["digestWeek"] = json!(digest::week_mark(today));
            changed = true;
        }
        if changed {
            let _ = std::fs::write(&marks_path, marks.to_string());
        }
    }

    // Budget guard: today's total across every provider against the user's
    // daily mark. `today` windows are already cut at local midnight.
    let today_cost: f64 = result.iter().map(|p| p.today.cost).sum();
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let cfg = config_with_defaults(load_config());
    if let Some(alert) = alerts::evaluate_spend(today_cost, &today, &cfg) {
        use tauri_plugin_notification::NotificationExt;
        let _ = app
            .notification()
            .builder()
            .title(i18n::t(&cfg, &alert.title))
            .body(i18n::t(&cfg, &alert.body))
            .show();
    }
    result
}

/// The key a provider's credential file currently holds (None when the
/// file is absent or unreadable). Used to tell a real credential change
/// from a re-save of the identical key, so an unchanged save doesn't dump
/// the cached snapshot. The key never leaves this comparison — not
/// logged, not returned, not published.
fn stored_pane_api_key(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let doc = serde_json::from_str::<Value>(&raw).ok()?;
    doc.get("apiKey")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_string)
}

struct CreditMeterBindGuard {
    ids: Vec<String>,
}

impl CreditMeterBindGuard {
    fn begin(ids: Vec<String>) -> Self {
        for id in &ids {
            providers::bind_credit_meter_generation(
                id,
                providers::credit_baseline_generation(id),
            );
        }
        Self { ids }
    }
}

impl Drop for CreditMeterBindGuard {
    fn drop(&mut self) {
        for id in &self.ids {
            providers::unbind_credit_meter_generation(id);
        }
    }
}

fn credit_meter_bind_ids(id: &str) -> Vec<String> {
    match id {
        "kimi" => vec!["kimi".into(), "moonshot".into()],
        _ => vec![id.to_string()],
    }
}

fn api_key_context_is_dirty(dir: &Path, provider: &str) -> bool {
    let snapshot_ids = api_key_snapshot_ids(provider);
    let baseline_ids = api_key_baseline_ids(provider);
    if SNAPSHOT_CACHE_NEEDS_FLUSH.load(Ordering::Acquire) {
        return true;
    }
    {
        let cache = last_ok().lock().unwrap();
        if snapshot_ids.iter().any(|id| cache.contains_key(id)) {
            return true;
        }
    }
    {
        let failures = fail_state().lock().unwrap();
        if snapshot_ids.iter().any(|id| failures.contains_key(id)) {
            return true;
        }
    }
    providers::credit_baselines_contain(dir, &baseline_ids)
}

fn context_cleanup_error(error: String) -> String {
    format!("the key change is saved, but clearing the previous key's cached data failed: {error}")
}

/// A credential actually changed: everything the old key produced — the
/// last-good snapshot (memory + disk), the local HTTP publication, the
/// fail-state cooldown, the alert history, and the credit high-water
/// mark — belongs to the old account. Moonshot rotation also drops the
/// folded Kimi snapshot; each key's credit baseline is forgotten alone.
fn invalidate_api_key_context(dir: &Path, provider: &str) -> Result<(), String> {
    let snapshot_ids = api_key_snapshot_ids(provider);
    let baseline_ids = api_key_baseline_ids(provider);
    let _mutation = KeyCardMutationGuard::begin(snapshot_ids.clone());
    let snap_err = forget_provider_snapshots_inner(&snapshot_ids, true)
        .err()
        .map(context_cleanup_error);
    let base_err = providers::forget_credit_baselines_in(dir, &baseline_ids)
        .err()
        .map(context_cleanup_error);
    // A MiniMax key change also forgets the remembered mcode plan tier —
    // a pasted key must never inherit another account's tier.
    let tier_err = if provider == "minimax" {
        providers::minimax::forget_remembered_tier_in(dir)
            .err()
            .map(context_cleanup_error)
    } else {
        None
    };
    [snap_err, base_err, tier_err]
        .into_iter()
        .flatten()
        .reduce(|left, right| format!("{left}; {right}"))
        .map_or(Ok(()), Err)
}

fn set_api_key_in(dir: &Path, provider: &str, key: &str) -> Result<(), String> {
    if !is_plain_api_key_provider(provider) {
        return Err(format!("unknown provider: {provider}"));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("create config dir: {e}"))?;
    let path = dir.join(format!("{provider}.json"));
    let previous_key = stored_pane_api_key(&path);
    let key = key.trim();
    if key.is_empty() {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("remove key file: {e}")),
        }
        if previous_key.is_some()
            || api_key_context_is_dirty(dir, provider)
            || (provider == "minimax" && providers::minimax::remembered_tier_exists_in(dir))
        {
            invalidate_api_key_context(dir, provider)?;
        }
        return Ok(());
    }
    // A MiniMax key change stashes the remembered mcode tier BEFORE the
    // new key lands: if the stash fails the old key stays in place and a
    // retry still sees a differing key; if the key write then fails the
    // stash goes back so the old key keeps its tier. (The invalidation
    // below retries the same delete — NotFound is Ok — so a succeeded
    // change is idempotent.)
    let stashed = provider == "minimax"
        && previous_key.as_deref() != Some(key)
        && providers::minimax::stash_remembered_tier_in(dir).map_err(context_cleanup_error)?;
    let raw = serde_json::json!({ "apiKey": key }).to_string();
    if let Err(e) = providers::onenewapi::store::atomic_write(&path, &raw) {
        if stashed {
            providers::minimax::restore_stashed_tier_in(dir);
        }
        return Err(format!("write key file: {e}"));
    }
    if stashed {
        providers::minimax::discard_stashed_tier_in(dir);
    }
    // Same-key retries still invalidate when a previous cleanup left
    // snapshots, cooldowns, or credit baselines behind.
    if previous_key.as_deref() != Some(key) || api_key_context_is_dirty(dir, provider) {
        invalidate_api_key_context(dir, provider)?;
    }
    Ok(())
}

/// Saves (or clears, when `key` is empty) a user-pasted API key to
/// %APPDATA%\AITaskManager\<provider>.json.
#[tauri::command]
fn set_api_key(provider: String, key: String) -> Result<(), String> {
    set_api_key_in(&providers::config_dir(), &provider, &key)
}

#[tauri::command]
fn onenewapi_list_sites() -> Result<Vec<providers::onenewapi::SiteDto>, String> {
    providers::onenewapi::list_sites()
}

#[tauri::command]
async fn onenewapi_probe_site(base_url: String) -> Result<providers::onenewapi::ProbeDto, String> {
    providers::onenewapi::probe_site(base_url).await
}

#[tauri::command]
async fn onenewapi_create_site(
    name: String,
    base_url: String,
) -> Result<providers::onenewapi::CreateSiteResult, String> {
    providers::onenewapi::create_site(name, base_url).await
}

#[tauri::command]
async fn onenewapi_update_site(
    id: String,
    name: Option<String>,
    base_url: Option<String>,
) -> Result<providers::onenewapi::SiteDto, String> {
    let previous = providers::onenewapi::list_sites()?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| "site not found".to_string())?;
    let normalized_base_url = base_url
        .as_deref()
        .map(providers::onenewapi::normalize_site_url)
        .transpose()?;
    let url_changed = normalized_base_url
        .as_deref()
        .is_some_and(|candidate| candidate != previous.base_url);
    let (verified_base_url, display) = if url_changed {
        let raw = base_url
            .as_ref()
            .ok_or_else(|| "site URL is required".to_string())?;
        let (dto, display) = providers::onenewapi::probe_site_display(raw.clone()).await?;
        (Some(dto.base_url), Some(display))
    } else {
        (normalized_base_url, None)
    };
    let key_ids = previous
        .keys
        .iter()
        .map(|key| key.id.clone())
        .collect::<Vec<_>>();
    let affected_snapshot_ids = onenewapi_snapshot_ids(&key_ids);
    let _mutation = KeyCardMutationGuard::begin(affected_snapshot_ids);
    providers::onenewapi::update_site_consistently(id, name, verified_base_url, display, |site| {
        if url_changed {
            forget_onenewapi_key_ids(key_ids)?;
            Ok(())
        } else {
            onenewapi_after_site_save(&previous, site)
        }
    })
}

#[tauri::command]
fn onenewapi_delete_site(id: String) -> Result<(), String> {
    let key_ids = providers::onenewapi::list_sites()?
        .into_iter()
        .find(|s| s.id == id)
        .map(|s| s.keys.into_iter().map(|k| k.id).collect::<Vec<_>>())
        .ok_or_else(|| "site not found".to_string())?;
    let _mutation = KeyCardMutationGuard::begin(onenewapi_snapshot_ids(&key_ids));
    providers::onenewapi::delete_site_consistently(id, || purge_onenewapi_cards(&key_ids))
}

#[cfg(test)]
fn onenewapi_apply_zero_to_one_enable(disabled: &mut Vec<Value>, key_id: &str) {
    let snap_id = format!("onenewapi@{key_id}");
    disabled.retain(|v| match v.as_str() {
        Some("onenewapi") => false,
        Some(id) if id == snap_id => false,
        _ => true,
    });
}

#[tauri::command]
fn onenewapi_create_key(
    site_id: String,
    label: String,
    api_key: String,
) -> Result<providers::onenewapi::CreatedKey, String> {
    let _mutation = KeyCardMutationGuard::begin(Vec::new());
    providers::onenewapi::create_key(site_id, label, api_key)
}

#[tauri::command]
fn onenewapi_update_key(
    site_id: String,
    key_id: String,
    label: Option<String>,
    api_key: Option<String>,
) -> Result<providers::onenewapi::SiteDto, String> {
    let rotated = api_key
        .as_deref()
        .map(str::trim)
        .is_some_and(|s| !s.is_empty());
    let label_changed = label.is_some();
    let snap_id = format!("onenewapi@{key_id}");
    let _mutation = KeyCardMutationGuard::begin(vec![snap_id.clone()]);
    providers::onenewapi::update_key_consistently(site_id, key_id.clone(), label, api_key, |site| {
        if rotated {
            forget_provider_snapshot(&snap_id)?;
        } else if label_changed {
            if let Some(key) = site.keys.iter().find(|k| k.id == key_id) {
                rename_cached_snapshot(&snap_id, format!("{} · {}", site.name, key.label))?;
            }
        }
        Ok(())
    })
}

#[tauri::command]
fn onenewapi_delete_key(
    site_id: String,
    key_id: String,
) -> Result<providers::onenewapi::SiteDto, String> {
    let _mutation = KeyCardMutationGuard::begin(vec![format!("onenewapi@{key_id}")]);
    let cleanup_key_id = key_id.clone();
    providers::onenewapi::delete_key_consistently(site_id, key_id, || {
        purge_onenewapi_cards(&[cleanup_key_id])
    })
}

#[tauri::command]
fn sub2api_list_sites() -> Result<Vec<providers::sub2api::SiteDto>, String> {
    providers::sub2api::list_sites()
}

#[tauri::command]
async fn sub2api_create_site(
    name: String,
    base_url: String,
) -> Result<providers::sub2api::CreateSiteResult, String> {
    providers::sub2api::create_site(name, base_url).await
}

#[tauri::command]
async fn sub2api_update_site(
    id: String,
    name: Option<String>,
    base_url: Option<String>,
) -> Result<providers::sub2api::SiteDto, String> {
    let mut mutation = KeyCardMutationGuard::begin(Vec::new());
    let previous = providers::sub2api::list_sites()?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| "site not found".to_string())?;
    let normalized_base_url = base_url
        .as_deref()
        .map(providers::sub2api::normalize_site_url)
        .transpose()?;
    let url_changed = normalized_base_url
        .as_deref()
        .is_some_and(|candidate| candidate != previous.base_url);
    let key_ids = previous
        .keys
        .iter()
        .map(|key| key.id.clone())
        .collect::<Vec<_>>();
    let affected_snapshot_ids = sub2api_snapshot_ids(&key_ids);
    mutation.track(affected_snapshot_ids);
    providers::sub2api::update_site_consistently(id, name, normalized_base_url, |site| {
        if url_changed {
            forget_sub2api_key_ids(key_ids)?;
            Ok(())
        } else {
            sub2api_after_site_save(&previous, site)
        }
    })
}

#[tauri::command]
fn sub2api_delete_site(id: String) -> Result<(), String> {
    let mut mutation = KeyCardMutationGuard::begin(Vec::new());
    let key_ids = providers::sub2api::list_sites()?
        .into_iter()
        .find(|s| s.id == id)
        .map(|s| s.keys.into_iter().map(|k| k.id).collect::<Vec<_>>())
        .ok_or_else(|| "site not found".to_string())?;
    mutation.track(sub2api_snapshot_ids(&key_ids));
    providers::sub2api::delete_site_consistently(id, || purge_sub2api_cards(&key_ids))
}

#[tauri::command]
fn sub2api_create_key(
    site_id: String,
    label: String,
    api_key: String,
) -> Result<providers::sub2api::CreatedKey, String> {
    let _mutation = KeyCardMutationGuard::begin(Vec::new());
    providers::sub2api::create_key(site_id, label, api_key)
}

#[tauri::command]
fn sub2api_update_key(
    site_id: String,
    key_id: String,
    label: Option<String>,
    api_key: Option<String>,
) -> Result<providers::sub2api::SiteDto, String> {
    let rotated = api_key
        .as_deref()
        .map(str::trim)
        .is_some_and(|s| !s.is_empty());
    let label_changed = label.is_some();
    let snap_id = format!("sub2api@{key_id}");
    let _mutation = KeyCardMutationGuard::begin(vec![snap_id.clone()]);
    providers::sub2api::update_key_consistently(site_id, key_id.clone(), label, api_key, |site| {
        if rotated {
            forget_provider_snapshot(&snap_id)?;
        } else if label_changed {
            if let Some(key) = site.keys.iter().find(|k| k.id == key_id) {
                rename_cached_snapshot(&snap_id, format!("{} · {}", site.name, key.label))?;
            }
        }
        Ok(())
    })
}

#[tauri::command]
fn sub2api_delete_key(
    site_id: String,
    key_id: String,
) -> Result<providers::sub2api::SiteDto, String> {
    let _mutation = KeyCardMutationGuard::begin(vec![format!("sub2api@{key_id}")]);
    let cleanup_key_id = key_id.clone();
    providers::sub2api::delete_key_consistently(site_id, key_id, || {
        purge_sub2api_cards(&[cleanup_key_id])
    })
}

/// Opens a provider quick link in the default browser. Only plain web URLs —
/// nothing that could launch a program.
#[tauri::command]
fn open_link(app: tauri::AppHandle, url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only http(s) links allowed".into());
    }
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("open link: {e}"))
}

/// A share card is a few hundred KB of PNG at 2x scale; 8 MB of base64
/// (6 MB decoded) leaves generous headroom while bounding what any code
/// running in the WebView can hand us.
const MAX_SHARE_PNG_BASE64: usize = 8 * 1024 * 1024;
/// Raw RGBA is 4 bytes per pixel, so 16 M pixels caps the expansion at
/// 64 MB. Real cards are ~1200x2400 (≈3 M pixels).
const MAX_SHARE_PNG_PIXELS: u64 = 16_000_000;

/// Reads width/height out of a PNG's IHDR chunk, which is always the first
/// chunk right after the 8-byte signature. Checking the declared dimensions
/// *before* handing the bytes to a decoder is what keeps a decompression
/// bomb (tiny file, billions of pixels) from being expanded at all.
fn png_dimensions(bytes: &[u8]) -> Result<(u32, u32), String> {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.len() < 24 || bytes[..8] != SIG || &bytes[12..16] != b"IHDR" {
        return Err("not a PNG".into());
    }
    let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    if w == 0 || h == 0 {
        return Err("empty image".into());
    }
    Ok((w, h))
}

/// Puts a share-card PNG (rendered by the frontend on a canvas) onto the
/// Windows clipboard as a real image.
///
/// Every command is callable by whatever JavaScript runs in the WebView, so
/// the encoded size and the declared pixel count are both bounded before any
/// decoding happens — otherwise a crafted PNG could force a multi-gigabyte
/// RGBA allocation and take the tray process down.
#[tauri::command]
fn copy_share_image(png_base64: String) -> Result<(), String> {
    use base64::Engine;
    let png_base64 = png_base64.trim();
    if png_base64.len() > MAX_SHARE_PNG_BASE64 {
        return Err("share image too large".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png_base64)
        .map_err(|e| format!("decode png: {e}"))?;
    let (dw, dh) = png_dimensions(&bytes)?;
    if u64::from(dw) * u64::from(dh) > MAX_SHARE_PNG_PIXELS {
        return Err("share image too large".into());
    }
    let img = tauri::image::Image::from_bytes(&bytes).map_err(|e| format!("parse png: {e}"))?;
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w != dw as usize || h != dh as usize {
        return Err("share image dimensions mismatch".into());
    }
    let rgba = img.rgba().to_vec();
    let mut clipboard = arboard::Clipboard::new().map_err(|e| format!("clipboard: {e}"))?;
    clipboard
        .set_image(arboard::ImageData {
            width: w,
            height: h,
            bytes: rgba.into(),
        })
        .map_err(|e| format!("copy image: {e}"))
}

/// (Re-)registers the global toggle-popover shortcut. An empty string clears
/// it. The accelerator uses Tauri syntax, e.g. "Ctrl+Shift+U".
fn register_shortcut(app: &tauri::AppHandle, accel: &str) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let accel = accel.trim();
    if accel.is_empty() {
        return Ok(());
    }
    let shortcut: Shortcut = accel
        .parse()
        .map_err(|_| format!("could not parse shortcut \"{accel}\""))?;
    gs.on_shortcut(shortcut, |app, _shortcut, event| {
        if event.state() == ShortcutState::Pressed {
            let pos = app
                .cursor_position()
                .unwrap_or(tauri::PhysicalPosition::new(1200.0, 700.0));
            toggle_popover(app, pos);
        }
    })
    .map_err(|e| format!("register shortcut: {e}"))
}

#[tauri::command]
fn set_shortcut(app: tauri::AppHandle, shortcut: String) -> Result<(), String> {
    register_shortcut(&app, &shortcut)
}

/// Spends one banked Codex rate-limit reset credit. Irreversible — the
/// frontend shows a confirm dialog before calling this.
#[tauri::command]
async fn codex_redeem_credit(
    credit_id: String,
    provider_id: Option<String>,
    redeem_request_id: Option<String>,
) -> Result<providers::codex::RedeemOutcome, String> {
    // provider_id routes multi-account redeems; absent = the default card
    // (older frontend builds during an update overlap). redeem_request_id
    // is the frontend's per-credit idempotency key.
    let pid = provider_id.unwrap_or_else(|| "codex".into());
    providers::codex::redeem_credit(&pid, &credit_id, redeem_request_id).await
}

/// Self-update is OFF in this fork. The endpoints and the signing pubkey in
/// tauri.conf.json still belong to upstream Pane, so an enabled updater would
/// replace this app with Pane. Before flipping this on: point
/// `updater_endpoint_strings` at our own release feed AND replace the pubkey
/// in tauri.conf.json with our own minisign key.
const UPDATES_ENABLED: bool = false;

/// Updater with the app version stamped into the endpoint by us. Tauri's
/// `{{current_version}}` template arrives percent-encoded and never gets
/// substituted in query strings, so 0.4.17 installs literally reported
/// "?v={{current_version}}" — the version is now formatted in Rust.
/// GitHub stays as the automatic fallback; the pubkey comes from config.
fn updater_endpoint_strings(version: &str) -> [String; 2] {
    [
        format!("https://trypane.xyz/api/update?v={version}"),
        "https://github.com/ItsJazii/pane/releases/latest/download/latest.json".into(),
    ]
}

fn build_updater(app: &tauri::AppHandle) -> Result<tauri_plugin_updater::Updater, String> {
    build_updater_with(app, Some(std::time::Duration::from_secs(30)))
}

/// The install path skips the 30 s ceiling: that bound keeps a hung
/// manifest endpoint from stalling the background check loop, but applied
/// to download_and_install it would kill a slow connection mid-download.
fn build_updater_for_install(
    app: &tauri::AppHandle,
) -> Result<tauri_plugin_updater::Updater, String> {
    build_updater_with(app, None)
}

fn build_updater_with(
    app: &tauri::AppHandle,
    timeout: Option<std::time::Duration>,
) -> Result<tauri_plugin_updater::Updater, String> {
    use tauri_plugin_updater::UpdaterExt;
    let version = app.package_info().version.to_string();
    let endpoints = updater_endpoint_strings(&version)
        .into_iter()
        .map(|endpoint| endpoint.parse().map_err(|e| format!("endpoint parse: {e}")))
        .collect::<Result<Vec<_>, _>>()?;
    let builder = app
        .updater_builder()
        .endpoints(endpoints)
        .map_err(|e| e.to_string())?;
    let builder = match timeout {
        Some(t) => builder.timeout(t),
        None => builder,
    };
    builder.build().map_err(|e| e.to_string())
}

/// Downloads and installs a pending update, then restarts the app. Only
/// called from the frontend banner after check_for_update announced one.
#[tauri::command]
async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    if !UPDATES_ENABLED {
        return Err("self-update is disabled in this build".into());
    }
    let updater = build_updater_for_install(&app)?;
    match updater.check().await.map_err(|e| e.to_string())? {
        Some(update) => {
            update
                .download_and_install(|_, _| {}, || {})
                .await
                .map_err(|e| e.to_string())?;
            app.restart();
        }
        // The update the button promised is gone (yanked release, CDN
        // hiccup). Succeeding silently would strand the frontend in its
        // "Installing…" state — fail so the button can recover.
        None => Err("update no longer available — try again shortly".into()),
    }
}

async fn live_update_check(app: &tauri::AppHandle) -> Result<Option<String>, String> {
    if !UPDATES_ENABLED {
        return Ok(None);
    }
    Ok(build_updater(app)?
        .check()
        .await
        .map_err(|e| e.to_string())?
        .map(|u| u.version))
}

/// Update check for the footer. Launch and every popover open hit the
/// network — same as before the 0.4.49 4 h gate — so a just-published
/// release shows up the next time you open AI Task Manager.
#[tauri::command]
async fn check_update(app: tauri::AppHandle) -> Result<Option<String>, String> {
    live_update_check(&app).await
}

/// Quiet backup if the popover never opens: check at startup, then every
/// 4 h. A hit emits "update-available" so the footer can show the button.
/// 404 (no releases yet) and offline are non-events.
fn spawn_update_checker(app: &tauri::AppHandle) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            match live_update_check(&handle).await {
                Ok(Some(version)) => {
                    let _ = handle.emit("update-available", version);
                }
                Ok(None) => {}
                Err(e) => eprintln!("[aitm] update check: {e}"),
            }
            tokio::time::sleep(std::time::Duration::from_secs(4 * 3600)).await;
        }
    });
}

// ---------------------------------------------------------------------------
// Tray + popover window plumbing
// ---------------------------------------------------------------------------

// Clicking the tray icon while the popover is open first steals focus
// (which hides the window) and then delivers the click event. Without a
// guard, that click would instantly re-open the window the user just
// closed. We remember when the last auto-hide happened and ignore tray
// clicks that arrive right after it.
static LAST_AUTO_HIDE_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Tells WebView2 to release memory while the popover is hidden and return
/// to normal when it shows. Tauri doesn't expose wry's setter for this, so
/// we make the same COM calls wry does (SetMemoryUsageTargetLevel).
#[cfg(windows)]
fn set_webview_memory_level(window: &tauri::WebviewWindow, low: bool) {
    let _ = window.with_webview(move |webview| unsafe {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL,
        };
        use windows_core::Interface;
        if let Ok(core) = webview.controller().CoreWebView2() {
            if let Ok(wv19) = core.cast::<ICoreWebView2_19>() {
                let level = COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL(if low { 1 } else { 0 });
                let _ = wv19.SetMemoryUsageTargetLevel(level);
            }
        }
    });
}

/// WKWebView (macOS) and WebKitGTK expose no memory-target setter; the OS
/// reclaims a hidden webview's memory on its own.
#[cfg(not(windows))]
fn set_webview_memory_level(_window: &tauri::WebviewWindow, _low: bool) {}

/// Hide the dashboard without the tray-click reopen dance. Esc on the
/// dashboard (and the hide half of the tray toggle) both land here so
/// the webview still drops to the low-memory target.
#[tauri::command]
fn hide_popover(app: tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        set_webview_memory_level(&window, true);
    }
}

/// Where the popover's top-left corner goes for a tray click. Its right edge
/// lines up with the click either way. Windows and Linux trays sit in a bar
/// along the bottom, so the window hangs above the click; the macOS menu bar
/// is along the top, so it drops below (the click lands inside a bar about
/// 24 pt tall, hence the clearance).
fn popover_origin(
    click: tauri::PhysicalPosition<f64>,
    size: tauri::PhysicalSize<u32>,
    menu_bar_on_top: bool,
) -> (f64, f64) {
    let x = (click.x - f64::from(size.width)).max(0.0);
    let y = if menu_bar_on_top {
        click.y + 18.0
    } else {
        (click.y - f64::from(size.height) - 8.0).max(0.0)
    };
    (x, y)
}

/// Popover widths in logical pixels: the single column, and wide mode's list
/// plus detail side by side.
const NARROW_WIDTH: f64 = 380.0;
const WIDE_WIDTH: f64 = 760.0;
const POPOVER_HEIGHT: f64 = 600.0;

/// New left edge when the window changes width: the right edge stays where
/// it is (that is the side anchored to the tray), without leaving the screen.
fn resized_left(old_left: i32, old_width: u32, new_width: u32) -> i32 {
    (old_left + old_width as i32 - new_width as i32).max(0)
}

/// Wide mode: the same window grown to fit the detail view beside the list.
/// Not a second window. The choice is saved by the frontend as `wideMode`.
#[tauri::command]
fn set_wide(app: tauri::AppHandle, wide: bool) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or("no main window")?;
    let scale = window.scale_factor().unwrap_or(1.0);
    let width = if wide { WIDE_WIDTH } else { NARROW_WIDTH };
    let new_width = (width * scale).round() as u32;
    let old = window.outer_size().map_err(|e| e.to_string())?;
    if old.width == new_width {
        return Ok(());
    }
    let position = window.outer_position().map_err(|e| e.to_string())?;
    window
        .set_size(tauri::LogicalSize::new(width, POPOVER_HEIGHT))
        .map_err(|e| e.to_string())?;
    let left = resized_left(position.x, old.width, new_width);
    window
        .set_position(tauri::PhysicalPosition::new(left, position.y))
        .map_err(|e| e.to_string())
}

fn toggle_popover(app: &tauri::AppHandle, click: tauri::PhysicalPosition<f64>) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };

    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        set_webview_memory_level(&window, true);
        return;
    }

    if now_ms().saturating_sub(LAST_AUTO_HIDE_MS.load(Ordering::Relaxed)) < 300 {
        return;
    }

    set_webview_memory_level(&window, false);

    let size = window
        .outer_size()
        .unwrap_or(tauri::PhysicalSize::new(380, 600));
    let (x, y) = popover_origin(click, size, cfg!(target_os = "macos"));
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    let _ = window.show();
    let _ = window.set_focus();
    let _ = window.emit("popover-shown", ());
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Second launches just poke the existing instance's popover open
        // instead of spawning a duplicate tray icon (Mac parity).
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            let pos = app
                .cursor_position()
                .unwrap_or(tauri::PhysicalPosition::new(1200.0, 700.0));
            toggle_popover(app, pos);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .invoke_handler(tauri::generate_handler![
            fetch_usage,
            get_inventory,
            get_running,
            end_task,
            get_diagnosis,
            get_effort,
            get_history,
            get_ledger,
            get_audit,
            export_audit,
            get_burn_profile,
            pin_preview,
            pin_apply,
            export_table,
            get_trust,
            get_forecast,
            get_sessions,
            reveal_session,
            client_rollup,
            save_clients,
            export_clients_csv,
            save_subscription,
            delete_subscription,
            set_wide,
            cached_usage,
            fetch_spend,
            set_api_key,
            onenewapi_list_sites,
            onenewapi_probe_site,
            onenewapi_create_site,
            onenewapi_update_site,
            onenewapi_delete_site,
            onenewapi_create_key,
            onenewapi_update_key,
            onenewapi_delete_key,
            sub2api_list_sites,
            sub2api_create_site,
            sub2api_update_site,
            sub2api_delete_site,
            sub2api_create_key,
            sub2api_update_key,
            sub2api_delete_key,
            get_config,
            set_config,
            system_ui_locale,
            get_autostart,
            set_autostart,
            sync_tray_surfaces,
            open_link,
            copy_share_image,
            set_shortcut,
            codex_redeem_credit,
            install_update,
            check_update,
            hide_popover
        ])
        .setup(|app| {
            spawn_update_checker(app.handle());
            spawn_background_refresh(app.handle());
            let quit = MenuItem::with_id(
                app,
                "quit",
                i18n::quit_label(&config_with_defaults(load_config())),
                true,
                None::<&str>,
            )?;
            let menu = Menu::with_items(app, &[&quit])?;

            // macOS: a monochrome template glyph the system tints for a light
            // or dark menu bar. Elsewhere: the app icon, as before.
            #[cfg(target_os = "macos")]
            let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray-template.png"))?;
            #[cfg(not(target_os = "macos"))]
            let tray_icon = app.default_window_icon().unwrap().clone();
            // A menu bar app has no business in the Dock or the app switcher.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            TrayIconBuilder::with_id("tray")
                .icon(tray_icon)
                .icon_as_template(cfg!(target_os = "macos"))
                .tooltip("AI Task Manager")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    if event.id.as_ref() == "quit" {
                        app.exit(0);
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        position,
                        ..
                    } = event
                    {
                        toggle_popover(tray.app_handle(), position);
                    }
                })
                .build(app)?;

            // The popover starts hidden, so start the webview in low-memory
            // mode too; it flips to normal the first time it is shown.
            if let Some(wv) = app.get_webview_window("main") {
                set_webview_memory_level(&wv, true);
            }

            httpapi::start();

            let saved_shortcut = load_config()
                .get("shortcut")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if let Err(e) = register_shortcut(app.handle(), &saved_shortcut) {
                eprintln!("[aitm] shortcut: {e}");
            }

            // Start with Windows is on by default (like the Mac app's
            // launch-at-login) and re-asserted each launch so the registry
            // entry follows the exe if it moves — e.g. loose exe → installed.
            // Only an explicit "off" in Settings is respected. Skipped in dev
            // builds so the debug exe never registers itself.
            if !cfg!(debug_assertions) {
                let wants_autostart = load_config()
                    .get("autostart")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                if wants_autostart {
                    use tauri_plugin_autostart::ManagerExt;
                    let _ = app.autolaunch().enable();
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let WindowEvent::Focused(false) = event {
                    if window.hide().is_ok() {
                        LAST_AUTO_HIDE_MS.store(now_ms(), Ordering::Relaxed);
                        if let Some(wv) = window.app_handle().get_webview_window("main") {
                            set_webview_memory_level(&wv, true);
                        }
                    }
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_menu_bar_title_is_short_real_text() {
        assert_eq!(super::menu_bar_title(&[33, 48]).as_deref(), Some("33 \u{b7} 48"));
        assert_eq!(super::menu_bar_title(&[140]).as_deref(), Some("100"), "capped");
        assert_eq!(super::menu_bar_title(&[1, 2, 3, 4]).as_deref(), Some("1 \u{b7} 2 \u{b7} 3"), "three at most: the menu bar is narrow");
        assert_eq!(super::menu_bar_title(&[]), None);
    }

    #[test]
    fn the_background_loop_waits_its_turn_and_a_bad_clock_cannot_stop_it() {
        let min = 60_000;
        assert!(!super::refresh_due(10 * min, 10 * min + 59_000, 1), "the window fetched 59 s ago");
        assert!(super::refresh_due(10 * min, 11 * min, 1));
        assert!(!super::refresh_due(10 * min, 14 * min, 5));
        assert!(super::refresh_due(10 * min, 15 * min, 5));
        assert!(super::refresh_due(0, 2 * min, 1), "never fetched is due");
        assert!(super::refresh_due(99 * min, 10 * min, 1), "a timestamp from the future is due, not a permanent off");
        assert!(super::refresh_due(10 * min, 11 * min, 0), "an interval of 0 is treated as a minute");
    }

    #[test]
    fn the_shortcut_hints_shown_in_settings_are_strings_the_parser_accepts() {
        use tauri_plugin_global_shortcut::Shortcut;
        // Settings shows "Ctrl+Shift+U" on Windows and "Cmd+Shift+U" on macOS.
        for hint in ["Ctrl+Shift+U", "Cmd+Shift+U"] {
            assert!(hint.parse::<Shortcut>().is_ok(), "{hint} must parse");
        }
        assert!("Not+A+Key".parse::<Shortcut>().is_err());
    }

    #[test]
    fn popover_hangs_above_a_bottom_tray_and_drops_below_a_top_menu_bar() {
        let size = tauri::PhysicalSize::new(380, 600);
        let taskbar = tauri::PhysicalPosition::new(1900.0, 1060.0);
        assert_eq!(super::popover_origin(taskbar, size, false), (1520.0, 452.0));
        let menu_bar = tauri::PhysicalPosition::new(1400.0, 12.0);
        let (x, y) = super::popover_origin(menu_bar, size, true);
        assert_eq!(x, 1020.0);
        assert!(y > 24.0, "clears the menu bar, was {y}");
        // A click near the left edge never pushes the window off screen.
        assert_eq!(super::popover_origin(tauri::PhysicalPosition::new(100.0, 12.0), size, true).0, 0.0);
    }

    #[test]
    fn widening_keeps_the_right_edge_and_stays_on_screen() {
        assert_eq!(super::resized_left(1000, 380, 760), 620, "right edge 1380 before and after");
        assert_eq!(super::resized_left(620, 760, 380), 1000, "and back");
        assert_eq!(super::resized_left(200, 380, 760), 0, "clamped at the screen edge");
    }

    use super::{
        current_credential_scoped_generations, guarded, is_credential_scoped_card,
        is_plain_api_key_provider, set_api_key_in, stored_pane_api_key,
        cached_kimi_ok_from, cached_onenewapi_id_is_configured, card_is_disabled,
        commit_strip_state_after_apply, fail_state, load_config_from, set_config_in,
        fold_moonshot_into_kimi, forget_onenewapi_key_ids, forget_provider_snapshot,
        is_kimi_wallet_label, last_ok, onenewapi_after_site_save,
        onenewapi_apply_zero_to_one_enable, key_card_snapshot_generations, persist_last_ok_at,
        persist_config_at, ConfigPersistIo,
        key_cards_purge_restore_patch, purge_onenewapi_cards, purge_onenewapi_cards_coordinated,
        purge_onenewapi_cards_with, purge_key_cards_from_config,
        rename_cached_snapshot, rename_cached_snapshot_in, rename_cached_snapshots_in,
        restore_kimi_wallet_rows,
        hydrate_fetch_time, restore_last_success_after_error,
        retain_current_key_card_results, strip_entry_application_order, strip_icon_ids_to_clear,
        strip_is_active, strip_reset_ids,
        updater_endpoint_strings, CachedSnap, FailState,
        KeyCardMutationGuard, StripEntry, SNAPSHOT_CACHE_MS, SNAPSHOT_CACHE_NEEDS_FLUSH,
        STALE_GRACE_MS, TEST_PERSIST_LAST_OK_FAIL,
    };
    use crate::alerts;
    use crate::providers::{Metric, Snapshot};
    use serde_json::{json, Value};
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::Ordering;

    fn strip_entry(id: &str, value: u32) -> StripEntry {
        StripEntry {
            id: id.into(),
            logo: vec![0; 32 * 32 * 4],
            values: vec![value],
            tooltip: id.into(),
        }
    }

    #[test]
    fn updater_prefers_trypane_then_github() {
        assert_eq!(
            updater_endpoint_strings("0.4.46"),
            [
                "https://trypane.xyz/api/update?v=0.4.46".to_string(),
                "https://github.com/ItsJazii/pane/releases/latest/download/latest.json".to_string(),
            ]
        );
    }

    #[test]
    fn kimi_wallet_labels() {
        assert!(is_kimi_wallet_label("API"));
        assert!(is_kimi_wallet_label("Balance"));
        assert!(!is_kimi_wallet_label("Session"));
        assert!(!is_kimi_wallet_label("Weekly"));
    }

    #[test]
    fn restore_wallet_rows_when_api_missing() {
        let mut current = Snapshot::ok(
            "kimi",
            "Kimi Code",
            None,
            vec![Metric::progress("Session", 0.0, None)],
        );
        current.warning = Some("Moonshot API wallet couldn't refresh".into());
        let previous = Snapshot::ok(
            "kimi",
            "Kimi Code",
            None,
            vec![
                Metric::progress("Session", 10.0, None),
                Metric::progress("API", 24.0, None),
                Metric::text("Balance", "$152.00".into()),
            ],
        );
        restore_kimi_wallet_rows(&mut current, &previous);
        let labels: Vec<_> = current.metrics.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["Session", "API", "Balance"]);
    }

    #[test]
    fn restore_wallet_rows_skips_when_api_present() {
        let mut current = Snapshot::ok(
            "kimi",
            "Kimi Code",
            None,
            vec![
                Metric::progress("Session", 0.0, None),
                Metric::progress("API", 1.0, None),
            ],
        );
        let previous = Snapshot::ok(
            "kimi",
            "Kimi Code",
            None,
            vec![Metric::progress("API", 99.0, None)],
        );
        restore_kimi_wallet_rows(&mut current, &previous);
        let api = current.metrics.iter().find(|m| m.label == "API").unwrap();
        assert!((api.used_percent.unwrap() - 1.0).abs() < 0.01);
    }

    #[test]
    fn cached_kimi_ok_ignores_stale_or_missing_entries() {
        let now = 1_800_000_000_000i64;
        let fresh = json!({"kimi": {"at": now - 60_000, "snap": {"status": "ok"}}});
        assert!(cached_kimi_ok_from(&fresh, now));
        let old = json!({"kimi": {"at": now - SNAPSHOT_CACHE_MS - 1, "snap": {"status": "ok"}}});
        assert!(!cached_kimi_ok_from(&old, now));
        let err = json!({"kimi": {"at": now, "snap": {"status": "error"}}});
        assert!(!cached_kimi_ok_from(&err, now));
        assert!(!cached_kimi_ok_from(&json!({}), now));
    }

    #[test]
    fn tray_strip_order_change_rebuilds_all_pairs_right_to_left() {
        let previous = vec![strip_entry("claude", 50), strip_entry("codex", 60)];
        let next = vec![strip_entry("codex", 60), strip_entry("claude", 50)];

        let reset_ids = strip_reset_ids(&previous, &next);
        let application_ids: Vec<&str> = strip_entry_application_order(&next, true)
            .into_iter()
            .map(|entry| entry.id.as_str())
            .collect();

        assert_eq!(reset_ids, vec!["claude", "codex"]);
        assert_eq!(application_ids, vec!["claude", "codex"]);
    }

    #[test]
    fn tray_strip_value_change_keeps_existing_pairs() {
        let previous = vec![strip_entry("claude", 50), strip_entry("codex", 60)];
        let next = vec![strip_entry("claude", 40), strip_entry("codex", 30)];

        assert!(strip_reset_ids(&previous, &next).is_empty());
    }

    #[test]
    fn failed_tray_strip_clear_invalidates_cache_so_retry_rebuilds() {
        let previous = vec![strip_entry("claude", 50), strip_entry("codex", 60)];
        let same_order = previous.clone();
        let reordered = vec![strip_entry("codex", 60), strip_entry("claude", 50)];
        let mut cached = previous.clone();

        let result: Result<(), String> = Err("native tray update failed".into());
        assert!(commit_strip_state_after_apply(&mut cached, &same_order, result).is_err());
        cached.clear();

        assert!(!strip_reset_ids(&cached, &same_order).is_empty());
        assert!(!strip_reset_ids(&cached, &reordered).is_empty());
    }

    #[test]
    fn successful_tray_strip_apply_commits_the_new_state() {
        let previous = vec![strip_entry("claude", 50), strip_entry("codex", 60)];
        let next = vec![strip_entry("codex", 60), strip_entry("claude", 50)];
        let mut cached = previous;

        assert!(commit_strip_state_after_apply(&mut cached, &next, Ok(())).is_ok());
        assert!(strip_reset_ids(&cached, &next).is_empty());
    }

    #[test]
    fn strip_is_inactive_when_apply_failed() {
        assert!(!strip_is_active(false, &[strip_entry("claude", 50)]));
    }

    #[test]
    fn strip_is_inactive_when_entries_are_empty() {
        assert!(!strip_is_active(true, &[]));
        assert!(!strip_is_active(false, &[]));
    }

    #[test]
    fn strip_is_active_when_apply_succeeded_with_entries() {
        assert!(strip_is_active(true, &[strip_entry("claude", 50)]));
    }

    #[test]
    fn strip_clear_ids_include_family_and_account_cards() {
        let known = vec![strip_entry("claude@work", 50)];
        let attempted = vec![strip_entry("codex", 40)];
        let ids = strip_icon_ids_to_clear(&known, &attempted);
        assert!(ids.contains(&"claude".into()));
        assert!(ids.contains(&"claude@work".into()));
        assert!(ids.contains(&"codex".into()));
    }

    #[test]
    fn sub2api_refresh_failure_keeps_history_and_marks_it_stale_immediately() {
        let previous = Snapshot::ok(
            "sub2api@wallet", "Panel · Key 1", None,
            vec![Metric::progress("Total quota", 25.0, None)],
        );
        let mut current = Snapshot::error("sub2api@wallet", "Panel · Key 1", "HTTP 401".into());
        assert!(restore_last_success_after_error(&mut current, &previous, 1_000));
        assert!(current.stale);
        assert!(current.attempt_failed);
        assert_eq!(current.warning.as_deref(), Some("HTTP 401"));
        assert_eq!(current.metrics[0].used_percent, Some(25.0));
    }

    #[test]
    fn a_failure_inside_the_grace_window_keeps_its_reason_but_not_the_chip() {
        let previous = Snapshot::ok("claude", "Claude", Some("max".into()), vec![Metric::progress("Weekly", 5.0, None)]);
        let mut current = Snapshot::error("claude", "Claude", "usage endpoint: HTTP 429 (retry_after_s=30)".into());
        assert!(restore_last_success_after_error(&mut current, &previous, 1_000));
        assert!(!current.stale, "one hiccup inside the grace window is not Outdated");
        assert!(current.attempt_failed);
        assert_eq!(current.warning.as_deref(), Some("usage endpoint: HTTP 429 (retry_after_s=30)"), "but the reason is kept for the footer");
        assert_eq!(current.metrics[0].used_percent, Some(5.0), "the last good numbers stand in");
    }

    #[test]
    fn sub2api_history_remains_readable_after_a_day_offline() {
        let previous = Snapshot::ok("sub2api@offline", "Panel · Offline", None,
            vec![Metric::text("Balance", "$8.00".into())]);
        let mut current = Snapshot::error("sub2api@offline", "Panel · Offline", "Network error".into());
        assert!(restore_last_success_after_error(&mut current, &previous, SNAPSHOT_CACHE_MS + 1));
        assert!(current.stale);
        assert_eq!(current.metrics[0].value.as_deref(), Some("$8.00"));
        assert_eq!(current.warning.as_deref(), Some("Network error"));
    }

    #[test]
    fn sub2api_family_and_key_choices_are_independent() {
        let family = vec!["sub2api".into(), "sub2api@b".into()];
        assert!(card_is_disabled("sub2api@a", &family));
        assert!(card_is_disabled("sub2api@b", &family));
        assert!(!card_is_disabled("onenewapi@a", &family));
        let key_only = vec!["sub2api@b".into()];
        assert!(!card_is_disabled("sub2api@a", &key_only));
        assert!(card_is_disabled("sub2api@b", &key_only));
    }

    #[test]
    fn sub2api_changed_key_rejects_late_result_and_keeps_other_keys() {
        let ids = ["sub2api@changed".to_string(), "sub2api@unrelated".to_string()];
        let expected = key_card_snapshot_generations(ids.clone());
        drop(KeyCardMutationGuard::begin(vec![ids[0].clone()]));
        let current = key_card_snapshot_generations(ids.clone());
        let mut snapshots = ids.iter().map(|id| Snapshot::ok(id, "Panel · Key", None, vec![]))
            .collect::<Vec<_>>();
        retain_current_key_card_results(&mut snapshots, &expected, &current);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].id, "sub2api@unrelated");
    }

    #[test]
    fn sub2api_delete_removes_only_its_card_settings() {
        let mut cfg = json!({
            "disabled": ["sub2api", "sub2api@drop", "sub2api@keep", "onenewapi@drop"],
            "layout": {
                "providerOrder": ["sub2api@drop", "onenewapi@drop", "sub2api@keep"],
                "providers": {"sub2api@drop": {"hidden": ["Balance"]}, "sub2api@keep": {"starred": ["Balance"]}}
            },
            "pinned": {"provider": "sub2api@drop", "label": "Primary quota"},
            "trayProviders": ["sub2api@drop", "sub2api@keep"]
        });
        purge_key_cards_from_config(&mut cfg, &["sub2api@drop".into()]);
        assert_eq!(cfg["disabled"], json!(["sub2api", "sub2api@keep", "onenewapi@drop"]));
        assert_eq!(cfg["layout"]["providerOrder"], json!(["onenewapi@drop", "sub2api@keep"]));
        assert_eq!(cfg["layout"]["providers"], json!({"sub2api@keep": {"starred": ["Balance"]}}));
        assert!(cfg["pinned"].is_null());
        assert_eq!(cfg["trayProviders"], json!(["sub2api@keep"]));
    }

    #[test]
    fn sub2api_rename_preserves_usage_and_rotation_clears_only_that_key() {
        let changed = "sub2api@lifecycle-changed";
        let other = "sub2api@lifecycle-other";
        let _changed = SnapCacheGuard::new(changed);
        let _other = SnapCacheGuard::new(other);
        for id in [changed, other] {
            last_ok().lock().unwrap().insert(id.into(), CachedSnap {
                at: 42,
                snap: Snapshot::ok(id, "Original · Key", None,
                    vec![Metric::text("Balance", "$8.00".into())]),
            });
            fail_state().lock().unwrap().insert(id.into(), FailState {
                until_ms: i64::MAX, note: "HTTP 401".into(),
            });
        }
        rename_cached_snapshot(changed, "Renamed · Key".into()).unwrap();
        {
            let map = last_ok().lock().unwrap();
            let entry = &map[changed];
            assert_eq!(entry.at, 42);
            assert_eq!(entry.snap.id, changed);
            assert_eq!(entry.snap.name, "Renamed · Key");
            assert_eq!(entry.snap.metrics[0].value.as_deref(), Some("$8.00"));
            assert_eq!(map[other].snap.name, "Original · Key");
        }
        alerts::insert_state_for_test(&format!("{changed}:Total quota"));
        super::forget_sub2api_key_ids(["lifecycle-changed".into()]).unwrap();
        assert!(!last_ok().lock().unwrap().contains_key(changed));
        assert!(!fail_state().lock().unwrap().contains_key(changed));
        assert!(!alerts::has_state_for_test(&format!("{changed}:Total quota")));
        assert!(last_ok().lock().unwrap().contains_key(other));
        assert!(fail_state().lock().unwrap().contains_key(other));
    }

    #[test]
    fn recent_error_fallback_within_grace_is_not_marked_stale() {
        let previous = Snapshot::ok(
            "codex",
            "Codex",
            None,
            vec![Metric::progress("Weekly", 25.0, None)],
        );
        let mut current = Snapshot::error("codex", "Codex", "timeout".into());

        assert!(restore_last_success_after_error(
            &mut current,
            &previous,
            1_000
        ));
        assert_eq!(current.status, "ok");
        assert!(!current.stale, "one hiccup inside the grace window is not Outdated");
        assert!(current.attempt_failed);
        // The chip is gated on `stale`; the reason is kept regardless so a
        // manual refresh can say why the numbers are held back.
        assert_eq!(current.warning.as_deref(), Some("timeout"));
        assert_eq!(current.metrics[0].used_percent, Some(25.0));
    }

    #[test]
    fn recent_error_fallback_after_grace_is_marked_stale() {
        let previous = Snapshot::ok(
            "codex",
            "Codex",
            None,
            vec![Metric::progress("Weekly", 25.0, None)],
        );
        let mut current = Snapshot::error("codex", "Codex", "timeout".into());

        assert!(restore_last_success_after_error(
            &mut current,
            &previous,
            STALE_GRACE_MS + 1,
        ));
        assert_eq!(current.status, "ok");
        assert!(current.stale);
        assert!(current.attempt_failed);
        assert_eq!(current.warning.as_deref(), Some("timeout"));
        assert_eq!(current.metrics[0].used_percent, Some(25.0));
    }

    #[test]
    fn old_cache_without_fetched_at_publishes_the_cache_clock() {
        let raw = r#"{"codex":{"at":1800000000000,"snap":{"id":"codex","name":"Codex","plan":null,"status":"ok","error":null,"metrics":[],"stale":false,"warning":null}}}"#;
        let map: std::collections::HashMap<String, CachedSnap> =
            serde_json::from_str(raw).unwrap();
        let entry = &map["codex"];
        assert!(entry.snap.fetched_at.is_none());
        let mut s = entry.snap.clone();
        hydrate_fetch_time(&mut s, entry.at);
        let json = crate::httpapi::provider_json(&s, "2026-09-05T00:00:00Z");
        assert_eq!(json["fetchedAt"], "2027-01-15T08:00:00Z");
        assert_eq!(json["status"], "ok");
    }

    #[test]
    fn expired_error_fallback_is_not_restored() {
        let previous = Snapshot::ok(
            "codex",
            "Codex",
            None,
            vec![Metric::progress("Weekly", 25.0, None)],
        );
        let mut current = Snapshot::error("codex", "Codex", "timeout".into());

        assert!(!restore_last_success_after_error(
            &mut current,
            &previous,
            SNAPSHOT_CACHE_MS + 1,
        ));
        assert_eq!(current.status, "error");
        assert!(!current.stale);
    }

    #[test]
    fn fold_keeps_moonshot_when_kimi_has_no_wallet() {
        let mut all = vec![
            Snapshot::ok(
                "kimi",
                "Kimi Code",
                None,
                vec![Metric::progress("Session", 0.0, None)],
            ),
            Snapshot::ok(
                "moonshot",
                "Kimi API",
                None,
                vec![Metric::progress("Credits used", 24.0, None)],
            ),
        ];
        fold_moonshot_into_kimi(&mut all);
        assert!(all.iter().any(|s| s.id == "moonshot"));
    }

    #[test]
    fn fold_hides_moonshot_when_kimi_has_wallet_or_moonshot_is_empty() {
        let mut with_api = vec![
            Snapshot::ok(
                "kimi",
                "Kimi Code",
                None,
                vec![
                    Metric::progress("Session", 0.0, None),
                    Metric::progress("API", 24.0, None),
                ],
            ),
            Snapshot::ok(
                "moonshot",
                "Kimi API",
                None,
                vec![Metric::progress("Credits used", 24.0, None)],
            ),
        ];
        fold_moonshot_into_kimi(&mut with_api);
        assert!(!with_api.iter().any(|s| s.id == "moonshot"));

        let mut empty_moon = vec![
            Snapshot::ok(
                "kimi",
                "Kimi Code",
                None,
                vec![Metric::progress("Session", 0.0, None)],
            ),
            Snapshot::no_credentials("moonshot", "Kimi API", "paste a key"),
        ];
        fold_moonshot_into_kimi(&mut empty_moon);
        assert!(!empty_moon.iter().any(|s| s.id == "moonshot"));
    }

    #[test]
    fn card_is_disabled_onenewapi_family_gates_keys_not_claude() {
        let family = vec!["onenewapi".into()];
        assert!(card_is_disabled("onenewapi@abc", &family));
        assert!(card_is_disabled("onenewapi", &family));
        assert!(!card_is_disabled("claude@home", &family));
        let one_key = vec!["onenewapi@abc".into()];
        assert!(card_is_disabled("onenewapi@abc", &one_key));
        assert!(!card_is_disabled("onenewapi@def", &one_key));
        let claude = vec!["claude".into()];
        assert!(card_is_disabled("claude", &claude));
        assert!(!card_is_disabled("claude@home", &claude));
    }

    #[test]
    fn onenewapi_zero_to_one_auto_enable_clears_family_and_new_key() {
        let mut disabled = vec![
            json!("onenewapi"),
            json!("onenewapi@abc"),
            json!("onenewapi@other"),
            json!("claude"),
        ];
        onenewapi_apply_zero_to_one_enable(&mut disabled, "abc");
        assert_eq!(disabled, vec![json!("onenewapi@other"), json!("claude")]);
    }

    #[test]
    fn onenewapi_zero_to_one_does_not_add_the_new_key_to_disabled() {
        let mut disabled: Vec<Value> = vec![];
        onenewapi_apply_zero_to_one_enable(&mut disabled, "abc");
        assert!(disabled.is_empty());
    }

    struct TempConfig {
        dir: std::path::PathBuf,
    }

    impl TempConfig {
        fn new() -> Self {
            let stamp = crate::providers::unique_stamp();
            let dir = std::env::temp_dir().join(format!(
                "pane-config-{}-{stamp}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }
    }

    impl Drop for TempConfig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn concurrent_config_patches_keep_both_updates() {
        let tmp = TempConfig::new();
        std::fs::write(tmp.dir.join("config.json"), "{}").unwrap();
        for round in 0..8 {
            std::fs::write(tmp.dir.join("config.json"), "{}").unwrap();
            let dir_a = tmp.dir.clone();
            let dir_b = tmp.dir.clone();
            let t1 = std::thread::spawn(move || set_config_in(&dir_a, json!({ "disabled": ["claude"] })));
            let t2 = std::thread::spawn(move || set_config_in(&dir_b, json!({ "locale": "zh" })));
            t1.join()
                .expect("disabled patch thread")
                .unwrap_or_else(|e| panic!("round {round} disabled patch: {e}"));
            t2.join()
                .expect("locale patch thread")
                .unwrap_or_else(|e| panic!("round {round} locale patch: {e}"));
            let cfg = load_config_from(&tmp.dir);
            assert_eq!(
                cfg["disabled"],
                json!(["claude"]),
                "round {round} lost disabled patch"
            );
            assert_eq!(cfg["locale"], json!("zh"), "round {round} lost locale patch");
        }
    }

    fn no_tmp_leftovers(dir: &std::path::Path) -> bool {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .all(|e| !e.file_name().to_string_lossy().ends_with(".tmp"))
    }

    fn fail_write(_path: &std::path::Path, _raw: &str) -> std::io::Result<()> {
        Err(std::io::Error::other("disk full"))
    }

    fn partial_write(path: &std::path::Path, raw: &str) -> std::io::Result<()> {
        std::fs::write(path, &raw[..raw.len() / 2])?;
        Err(std::io::Error::other("disk full"))
    }

    fn fail_replace(_tmp: &std::path::Path, _path: &std::path::Path) -> std::io::Result<()> {
        Err(std::io::Error::other("file in use"))
    }

    #[test]
    fn non_object_main_config_does_not_clobber_or_lose_the_backup() {
        let tmp = TempConfig::new();
        let good = json!({"locale": "zh"});
        std::fs::write(
            tmp.dir.join("config.json.bak"),
            serde_json::to_string(&good).unwrap(),
        )
        .unwrap();
        std::fs::write(tmp.dir.join("config.json"), "[]").unwrap();

        set_config_in(&tmp.dir, json!({ "density": "compact" })).unwrap();

        let backup: Value = serde_json::from_str(
            &std::fs::read_to_string(tmp.dir.join("config.json.bak")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            backup["locale"], "zh",
            "valid non-object JSON must not overwrite the object backup"
        );
        let main = load_config_from(&tmp.dir);
        assert_eq!(main["density"], "compact");
        assert_eq!(
            main["locale"], "zh",
            "the save must patch the recovered backup, not empty defaults"
        );
    }

    #[test]
    fn corrupt_main_config_never_clobbers_the_good_backup() {
        let tmp = TempConfig::new();
        let good = json!({"locale": "zh"});
        std::fs::write(
            tmp.dir.join("config.json.bak"),
            serde_json::to_string(&good).unwrap(),
        )
        .unwrap();
        std::fs::write(tmp.dir.join("config.json"), "{corrupt").unwrap();

        set_config_in(&tmp.dir, json!({ "density": "compact" })).unwrap();

        let backup: Value = serde_json::from_str(
            &std::fs::read_to_string(tmp.dir.join("config.json.bak")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            backup, good,
            "a corrupt main file must never overwrite the good backup"
        );
        let main = load_config_from(&tmp.dir);
        assert_eq!(main["density"], "compact");
        assert_eq!(
            main["locale"], "zh",
            "the save patches on top of the recovered backup, not defaults"
        );
    }

    #[test]
    fn failed_temp_write_leaves_main_and_backup_untouched() {
        let tmp = TempConfig::new();
        std::fs::write(tmp.dir.join("config.json"), r#"{"locale":"zh"}"#).unwrap();
        let error = persist_config_at(
            &tmp.dir,
            &json!({"density": "compact"}),
            ConfigPersistIo {
                write_tmp: fail_write,
                ..ConfigPersistIo::real()
            },
        )
        .unwrap_err();
        assert!(error.contains("write config"), "{error}");
        assert_eq!(load_config_from(&tmp.dir)["locale"], "zh");
        assert!(!tmp.dir.join("config.json.bak").exists());
        assert!(no_tmp_leftovers(&tmp.dir));
    }

    #[test]
    fn partial_temp_write_is_cleaned_up() {
        let tmp = TempConfig::new();
        std::fs::write(tmp.dir.join("config.json"), "{}").unwrap();
        persist_config_at(
            &tmp.dir,
            &json!({"density": "compact"}),
            ConfigPersistIo {
                write_tmp: partial_write,
                ..ConfigPersistIo::real()
            },
        )
        .unwrap_err();
        assert!(
            no_tmp_leftovers(&tmp.dir),
            "a half-written temp file must be removed"
        );
    }

    #[test]
    fn failed_replace_keeps_a_recoverable_config() {
        let tmp = TempConfig::new();
        std::fs::write(tmp.dir.join("config.json"), r#"{"locale":"zh"}"#).unwrap();
        let error = persist_config_at(
            &tmp.dir,
            &json!({"density": "compact"}),
            ConfigPersistIo {
                replace: fail_replace,
                ..ConfigPersistIo::real()
            },
        )
        .unwrap_err();
        assert!(error.contains("replace config"), "{error}");
        assert_eq!(load_config_from(&tmp.dir)["locale"], "zh");
        let backup: Value = serde_json::from_str(
            &std::fs::read_to_string(tmp.dir.join("config.json.bak")).unwrap(),
        )
        .unwrap();
        assert_eq!(backup["locale"], "zh");
        assert!(no_tmp_leftovers(&tmp.dir));
    }

    #[test]
    fn backup_update_failure_does_not_block_the_save() {
        let tmp = TempConfig::new();
        std::fs::write(tmp.dir.join("config.json"), r#"{"locale":"zh"}"#).unwrap();
        // An unusable backup target (a directory) must not lose the save.
        std::fs::create_dir_all(tmp.dir.join("config.json.bak")).unwrap();
        set_config_in(&tmp.dir, json!({ "density": "compact" })).unwrap();
        let main = load_config_from(&tmp.dir);
        assert_eq!(main["density"], "compact");
        assert_eq!(main["locale"], "zh");
    }

    #[test]
    fn repeated_saves_keep_the_previous_valid_config_recoverable() {
        let tmp = TempConfig::new();
        set_config_in(&tmp.dir, json!({ "locale": "zh" })).unwrap();
        set_config_in(&tmp.dir, json!({ "density": "regular" })).unwrap();
        // Crash mid-write: the main file becomes garbage.
        std::fs::write(tmp.dir.join("config.json"), "{corrupt").unwrap();
        let recovered = load_config_from(&tmp.dir);
        assert_eq!(
            recovered["locale"], "zh",
            "the backup still loads the previous valid config"
        );
        // Saving from the recovered state heals the main file — without
        // letting the corrupt copy destroy the backup first.
        set_config_in(&tmp.dir, json!({ "spendTab": "week" })).unwrap();
        let healed = load_config_from(&tmp.dir);
        assert_eq!(healed["spendTab"], "week");
        assert_eq!(healed["locale"], "zh");
    }

    #[test]
    fn onenewapi_snapshot_cache_write_failure_is_reported() {
        let root =
            std::env::temp_dir().join(format!("pane-onenewapi-cache-fail-{}", std::process::id()));
        let _ = std::fs::remove_file(&root);
        let _ = std::fs::remove_dir_all(&root);
        std::fs::write(&root, "not a directory").unwrap();
        let result = persist_last_ok_at(&root.join("last_snapshots.json"), &HashMap::new());
        let _ = std::fs::remove_file(&root);
        assert!(
            result.is_err(),
            "cache persistence errors must reach deletion cleanup"
        );
    }

    #[test]
    fn onenewapi_cached_cards_require_a_configured_key() {
        let configured = HashSet::from(["onenewapi@keep".to_string()]);
        assert!(cached_onenewapi_id_is_configured(
            "onenewapi@keep",
            &configured
        ));
        assert!(!cached_onenewapi_id_is_configured(
            "onenewapi@deleted",
            &configured
        ));
        assert!(cached_onenewapi_id_is_configured("claude", &configured));
    }

    #[test]
    fn onenewapi_stale_refresh_results_are_discarded() {
        let expected = key_card_snapshot_generations(["onenewapi@old".into()]);
        let mutation = KeyCardMutationGuard::begin(vec!["onenewapi@old".into()]);
        drop(mutation);
        let current = key_card_snapshot_generations(["onenewapi@old".into()]);
        let mut snapshots = vec![
            Snapshot::ok("onenewapi@old", "Old · Key 1", None, vec![]),
            Snapshot::ok("claude", "Claude", None, vec![]),
        ];
        let stale = retain_current_key_card_results(&mut snapshots, &expected, &current);
        assert_eq!(stale, ["onenewapi@old"]);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].id, "claude");
    }

    /// Owns one id in the process-wide snapshot caches for a test: removes it
    /// on drop, and holds `SNAP_CACHE_SERIAL` so tests that share an id cannot
    /// run side by side. Without the lock, one test's drop deleted the entry
    /// another test was asserting on (about 3 runs in 10 failed).
    struct SnapCacheGuard {
        id: String,
        _serial: Option<std::sync::MutexGuard<'static, ()>>,
    }

    static SNAP_CACHE_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    thread_local! {
        /// Guards alive on this thread. Only the first takes the lock, so a
        /// test may hold several ids without deadlocking on itself.
        static SNAP_GUARD_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }

    impl SnapCacheGuard {
        fn new(id: &str) -> Self {
            let first = SNAP_GUARD_DEPTH.with(|d| {
                let depth = d.get();
                d.set(depth + 1);
                depth == 0
            });
            // A test that panicked while holding it must not fail the rest.
            let serial = first.then(|| SNAP_CACHE_SERIAL.lock().unwrap_or_else(|p| p.into_inner()));
            Self { id: id.to_string(), _serial: serial }
        }
    }

    impl Drop for SnapCacheGuard {
        fn drop(&mut self) {
            // into_inner: cleanup still has to run after a failed assertion
            // poisoned these locks, or the next test inherits the entry.
            fail_state().lock().unwrap_or_else(|p| p.into_inner()).remove(&self.id);
            last_ok().lock().unwrap_or_else(|p| p.into_inner()).remove(&self.id);
            SNAP_GUARD_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
        }
    }

    #[cfg(windows)]
    fn hold_no_delete(path: &std::path::Path) -> std::fs::File {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1) // FILE_SHARE_READ
            .open(path)
            .expect("open held key file")
    }

    fn seed_cached_ok(id: &str, name: &str) {
        last_ok().lock().unwrap().insert(
            id.into(),
            CachedSnap {
                at: 1_000,
                snap: Snapshot::ok(
                    id,
                    name,
                    None,
                    vec![Metric::progress("Credits used", 80.0, None)],
                ),
            },
        );
    }

    #[test]
    fn api_key_context_scopes_only_key_backed_families() {
        assert!(is_credential_scoped_card("deepseek"));
        assert!(is_credential_scoped_card("sub2api@k1"));
        assert!(is_credential_scoped_card("onenewapi@k1"));
        assert!(!is_credential_scoped_card("claude"));
        assert!(!is_credential_scoped_card("claude@abcd1234"));
        assert!(!is_credential_scoped_card("codex"));
        assert!(!is_credential_scoped_card("grok"));
        assert!(!is_plain_api_key_provider("claude"));
        assert!(is_plain_api_key_provider("kimi"));
    }

    #[test]
    fn current_generations_cover_plain_providers_like_the_expected_side() {
        let _deepseek = SnapCacheGuard::new("deepseek");
        let batch = vec![
            Snapshot::ok("deepseek", "DeepSeek", None, vec![]),
            Snapshot::ok("claude", "Claude", None, vec![]),
        ];
        let current = current_credential_scoped_generations(&batch);
        let expected = key_card_snapshot_generations(["deepseek".to_string()]);
        assert_eq!(
            current.get("deepseek"),
            expected.get("deepseek"),
            "no rotation must compare equal and survive the retain"
        );
        assert!(!current.contains_key("claude"));

        let mut all = batch;
        let stale = retain_current_key_card_results(&mut all, &expected, &current);
        assert!(stale.is_empty(), "nothing dropped without a rotation");
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn rotating_plain_api_key_clears_only_that_providers_old_state() {
        let tmp = TempConfig::new();
        let rotated = "deepseek";
        let bystander = "aihubmix";
        let _rotated = SnapCacheGuard::new(rotated);
        let _bystander = SnapCacheGuard::new(bystander);
        set_api_key_in(&tmp.dir, rotated, "key-a").unwrap();
        set_api_key_in(&tmp.dir, bystander, "key-z").unwrap();
        seed_cached_ok(rotated, "DeepSeek");
        fail_state().lock().unwrap().insert(
            rotated.into(),
            FailState {
                until_ms: i64::MAX,
                note: "HTTP 429 rate limited".into(),
            },
        );
        alerts::insert_state_for_test(&format!("{rotated}:Credits used"));
        seed_cached_ok(bystander, "AihubMix");

        set_api_key_in(&tmp.dir, rotated, "key-b").unwrap();

        {
            let cache = last_ok().lock().unwrap();
            assert!(
                !cache.contains_key(rotated),
                "the old account's success snapshot must not survive a rotation"
            );
            assert!(
                cache.contains_key(bystander),
                "an untouched provider keeps its cache"
            );
        }
        assert!(fail_state().lock().unwrap().get(rotated).is_none());
        assert!(!alerts::has_state_for_test(&format!(
            "{rotated}:Credits used"
        )));
        assert_eq!(
            stored_pane_api_key(&tmp.dir.join(format!("{rotated}.json"))).as_deref(),
            Some("key-b")
        );
        let before = key_card_snapshot_generations([rotated.to_string()]);
        set_api_key_in(&tmp.dir, rotated, "key-b").unwrap();
        let after = key_card_snapshot_generations([rotated.to_string()]);
        assert_eq!(before.get(rotated), after.get(rotated));
    }

    #[test]
    fn changing_minimax_key_forgets_remembered_tier() {
        let tmp = TempConfig::new();
        let _minimax = SnapCacheGuard::new("minimax");
        std::fs::write(
            tmp.dir.join("minimax-plan.json"),
            serde_json::json!({
                "tier": "Ultra Plan",
                "seen_ms": chrono::Utc::now().timestamp_millis(),
                "user_id": "1",
            })
            .to_string(),
        )
        .unwrap();

        set_api_key_in(&tmp.dir, "minimax", "sk-new-key-xxxxxxxx").unwrap();

        assert!(
            !tmp.dir.join("minimax-plan.json").exists(),
            "a pasted key must not inherit another account's remembered tier"
        );
    }

    #[test]
    fn resaving_same_minimax_key_keeps_remembered_tier() {
        let tmp = TempConfig::new();
        let _minimax = SnapCacheGuard::new("minimax");
        set_api_key_in(&tmp.dir, "minimax", "sk-same-key-xxxxxxxx").unwrap();
        std::fs::write(
            tmp.dir.join("minimax-plan.json"),
            serde_json::json!({
                "tier": "Ultra Plan",
                "seen_ms": chrono::Utc::now().timestamp_millis(),
                "user_id": "1",
            })
            .to_string(),
        )
        .unwrap();

        set_api_key_in(&tmp.dir, "minimax", "sk-same-key-xxxxxxxx").unwrap();

        assert!(
            tmp.dir.join("minimax-plan.json").exists(),
            "re-saving an unchanged key must not drop the remembered tier"
        );
    }

    #[test]
    fn clearing_minimax_key_forgets_remembered_tier() {
        let tmp = TempConfig::new();
        let _minimax = SnapCacheGuard::new("minimax");
        // No key file at all — the plan cache alone still triggers the
        // cleanup when the user hits Save with an empty field.
        std::fs::write(
            tmp.dir.join("minimax-plan.json"),
            serde_json::json!({
                "tier": "Ultra Plan",
                "seen_ms": chrono::Utc::now().timestamp_millis(),
                "user_id": "1",
            })
            .to_string(),
        )
        .unwrap();

        set_api_key_in(&tmp.dir, "minimax", "").unwrap();

        assert!(
            !tmp.dir.join("minimax-plan.json").exists(),
            "clearing the key must drop the remembered tier"
        );
    }

    #[test]
    fn minimax_key_change_is_refused_while_stale_tier_cannot_be_removed() {
        let tmp = TempConfig::new();
        let _minimax = SnapCacheGuard::new("minimax");
        set_api_key_in(&tmp.dir, "minimax", "sk-a-xxxxxxxxxx").unwrap();

        // A directory where the STASH file belongs: remove_file can't
        // clear it, then the rename onto it fails — so the new key must
        // not be saved while the old tier survives.
        std::fs::write(
            tmp.dir.join("minimax-plan.json"),
            serde_json::json!({
                "tier": "Ultra Plan",
                "seen_ms": chrono::Utc::now().timestamp_millis(),
                "user_id": "1",
            })
            .to_string(),
        )
        .unwrap();
        std::fs::create_dir(tmp.dir.join("minimax-plan.json.old")).unwrap();
        set_api_key_in(&tmp.dir, "minimax", "sk-b-xxxxxxxxxx")
            .expect_err("a failed tier stash must refuse the key change");
        assert_eq!(
            stored_pane_api_key(&tmp.dir.join("minimax.json")).as_deref(),
            Some("sk-a-xxxxxxxxxx"),
            "the old key stays so the retry still sees a rotation"
        );
        assert!(tmp.dir.join("minimax-plan.json").exists());

        std::fs::remove_dir(tmp.dir.join("minimax-plan.json.old")).unwrap();
        set_api_key_in(&tmp.dir, "minimax", "sk-b-xxxxxxxxxx").unwrap();
        assert_eq!(
            stored_pane_api_key(&tmp.dir.join("minimax.json")).as_deref(),
            Some("sk-b-xxxxxxxxxx")
        );
        assert!(!tmp.dir.join("minimax-plan.json").exists());
        assert!(!tmp.dir.join("minimax-plan.json.old").exists());
    }

    #[test]
    fn failed_minimax_key_write_restores_remembered_tier() {
        let tmp = TempConfig::new();
        let _minimax = SnapCacheGuard::new("minimax");
        set_api_key_in(&tmp.dir, "minimax", "sk-a-xxxxxxxxxx").unwrap();
        let plan_json = serde_json::json!({
            "tier": "Ultra Plan",
            "seen_ms": chrono::Utc::now().timestamp_millis(),
            "user_id": "1",
        })
        .to_string();
        std::fs::write(tmp.dir.join("minimax-plan.json"), &plan_json).unwrap();

        // A directory where the key file belongs makes atomic_write fail —
        // the stash must go back so the surviving old key keeps its tier.
        std::fs::remove_file(tmp.dir.join("minimax.json")).unwrap();
        std::fs::create_dir(tmp.dir.join("minimax.json")).unwrap();
        set_api_key_in(&tmp.dir, "minimax", "sk-b-xxxxxxxxxx")
            .expect_err("the key write must fail against a directory");
        assert_eq!(
            std::fs::read_to_string(tmp.dir.join("minimax-plan.json")).unwrap(),
            plan_json,
            "a failed key write restores the stashed tier"
        );
        assert!(!tmp.dir.join("minimax-plan.json.old").exists());
    }

    /// rotating_moonshot… and rotating_kimi… exercise the same two global
    /// ids; under a parallel test runner they'd evict each other's
    /// fixtures, so they serialize on this lock.
    fn kimi_moonshot_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap()
    }

    #[test]
    fn rotating_moonshot_also_forgets_folded_kimi_wallet() {
        let _km = kimi_moonshot_test_lock();
        let tmp = TempConfig::new();
        let _moonshot = SnapCacheGuard::new("moonshot");
        let _kimi = SnapCacheGuard::new("kimi");
        set_api_key_in(&tmp.dir, "moonshot", "key-a").unwrap();
        seed_cached_ok("moonshot", "Kimi API");
        last_ok().lock().unwrap().insert(
            "kimi".into(),
            CachedSnap {
                at: 1_000,
                snap: Snapshot::ok(
                    "kimi",
                    "Kimi Code",
                    None,
                    vec![
                        Metric::progress("Session", 10.0, None),
                        Metric::progress("Credits used", 80.0, None),
                    ],
                ),
            },
        );
        alerts::insert_state_for_test("kimi:Credits used");

        set_api_key_in(&tmp.dir, "moonshot", "key-b").unwrap();

        let cache = last_ok().lock().unwrap();
        assert!(
            !cache.contains_key("moonshot"),
            "rotated Moonshot snapshot must go"
        );
        assert!(
            !cache.contains_key("kimi"),
            "folded Kimi wallet rows must not survive a Moonshot rotation"
        );
        assert!(!alerts::has_state_for_test("kimi:Credits used"));
    }

    #[test]
    fn rotating_deepseek_drops_the_old_credit_baseline() {
        let tmp = TempConfig::new();
        let _guard = SnapCacheGuard::new("deepseek");
        set_api_key_in(&tmp.dir, "deepseek", "key-a").unwrap();
        std::fs::write(
            tmp.dir.join("credit_baselines.json"),
            r#"{"deepseek": 40.0, "moonshot": 12.0}"#,
        )
        .unwrap();

        set_api_key_in(&tmp.dir, "deepseek", "key-b").unwrap();

        let doc: Value = serde_json::from_str(
            &std::fs::read_to_string(tmp.dir.join("credit_baselines.json")).unwrap(),
        )
        .unwrap();
        assert!(
            doc.get("deepseek").is_none(),
            "the new key must not inherit the old high-water mark"
        );
        assert_eq!(
            doc["moonshot"], 12.0,
            "an untouched provider keeps its baseline"
        );
    }

    #[test]
    fn rotating_kimi_does_not_reset_moonshot_usage() {
        let _km = kimi_moonshot_test_lock();
        let tmp = TempConfig::new();
        let _kimi = SnapCacheGuard::new("kimi");
        let _moonshot = SnapCacheGuard::new("moonshot");
        set_api_key_in(&tmp.dir, "kimi", "kimi-a").unwrap();
        set_api_key_in(&tmp.dir, "moonshot", "ms-a").unwrap();
        seed_cached_ok("kimi", "Kimi Code");
        seed_cached_ok("moonshot", "Kimi API");
        std::fs::write(
            tmp.dir.join("credit_baselines.json"),
            r#"{"kimi": 5.0, "moonshot": 12.0}"#,
        )
        .unwrap();

        set_api_key_in(&tmp.dir, "kimi", "kimi-b").unwrap();

        let cache = last_ok().lock().unwrap();
        assert!(
            !cache.contains_key("kimi"),
            "rotated Kimi snapshot must go"
        );
        assert!(
            cache.contains_key("moonshot"),
            "Moonshot's key did not change, so its snapshot and meter stay"
        );
        let doc: Value = serde_json::from_str(
            &std::fs::read_to_string(tmp.dir.join("credit_baselines.json")).unwrap(),
        )
        .unwrap();
        assert!(doc.get("kimi").is_none());
        assert_eq!(doc["moonshot"], 12.0);
    }

    #[test]
    fn leftover_cache_is_cleared_when_retrying_the_same_key() {
        let tmp = TempConfig::new();
        let id = "openrouter";
        let _guard = SnapCacheGuard::new(id);
        set_api_key_in(&tmp.dir, id, "key-b").unwrap();
        seed_cached_ok(id, "OpenRouter");
        fail_state().lock().unwrap().insert(
            id.into(),
            FailState {
                until_ms: i64::MAX,
                note: "HTTP 429 rate limited".into(),
            },
        );

        set_api_key_in(&tmp.dir, id, "key-b").unwrap();

        assert!(
            !last_ok().lock().unwrap().contains_key(id),
            "retrying the already-saved key must finish a leftover cleanup"
        );
        assert!(fail_state().lock().unwrap().get(id).is_none());
    }

    #[test]
    fn blocked_credit_baselines_file_is_reported() {
        let tmp = TempConfig::new();
        // "qwen" belongs to clearing_missing_key…, which asserts that a
        // no-op clear leaves the generation untouched — this test bumps
        // its id's generation, so they must not share one.
        let id = "aihubmix";
        let _guard = SnapCacheGuard::new(id);
        set_api_key_in(&tmp.dir, id, "key-a").unwrap();
        std::fs::create_dir(tmp.dir.join("credit_baselines.json")).unwrap();
        let error = set_api_key_in(&tmp.dir, id, "key-b").expect_err("baseline IO must surface");
        assert!(
            error.contains("credit baselines") || error.contains("cached data"),
            "a baseline rewrite failure must not report success: {error}"
        );
        assert_eq!(
            stored_pane_api_key(&tmp.dir.join(format!("{id}.json"))).as_deref(),
            Some("key-b")
        );
    }

    #[test]
    fn failed_snapshot_persist_clears_memory_and_is_retryable() {
        let tmp = TempConfig::new();
        let id = "deepseek";
        let _guard = SnapCacheGuard::new(id);
        struct PersistFailGuard;
        impl Drop for PersistFailGuard {
            fn drop(&mut self) {
                TEST_PERSIST_LAST_OK_FAIL.with(|fail| fail.set(false));
                SNAPSHOT_CACHE_NEEDS_FLUSH.store(false, Ordering::Release);
            }
        }
        let _persist_guard = PersistFailGuard;
        set_api_key_in(&tmp.dir, id, "key-a").unwrap();
        seed_cached_ok(id, "DeepSeek");
        TEST_PERSIST_LAST_OK_FAIL.with(|fail| fail.set(true));
        let error = set_api_key_in(&tmp.dir, id, "key-b").expect_err("persist fail must surface");
        assert!(
            error.contains("cached data"),
            "cleanup failure must not look like a successful save: {error}"
        );
        assert_eq!(
            stored_pane_api_key(&tmp.dir.join(format!("{id}.json"))).as_deref(),
            Some("key-b")
        );
        assert!(
            !last_ok().lock().unwrap().contains_key(id),
            "memory must drop the old snapshot even when disk persist fails"
        );
        set_api_key_in(&tmp.dir, id, "key-b").unwrap();
        assert!(!last_ok().lock().unwrap().contains_key(id));
        assert!(!SNAPSHOT_CACHE_NEEDS_FLUSH.load(Ordering::Acquire));
    }

    #[test]
    fn rotated_key_late_result_is_refused_and_401_shows_no_old_data() {
        let tmp = TempConfig::new();
        let id = "openrouter";
        let _guard = SnapCacheGuard::new(id);
        set_api_key_in(&tmp.dir, id, "key-a").unwrap();
        let expected = key_card_snapshot_generations([id.to_string()]);
        set_api_key_in(&tmp.dir, id, "key-b").unwrap();
        assert!(!last_ok().lock().unwrap().contains_key(id));
        let mut publishable = vec![
            Snapshot::ok(id, "OpenRouter", None, vec![]),
            Snapshot::ok("claude", "Claude", None, vec![]),
        ];
        let current = current_credential_scoped_generations(&publishable);
        let stale = retain_current_key_card_results(&mut publishable, &expected, &current);
        assert_eq!(stale, vec![id.to_string()]);
        assert_eq!(publishable.len(), 1);
        assert_eq!(publishable[0].id, "claude");
    }

    #[test]
    fn late_failure_after_key_rotation_cannot_bench_the_new_context() {
        let id = "kilo";
        let _guard = SnapCacheGuard::new(id);
        let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let fut = async move {
            started_tx.send(()).expect("test gate open");
            release_rx.await.expect("test release");
            Snapshot::error(id, "Kilo", "HTTP 429 rate limited".into())
        };
        let handle = std::thread::spawn(move || {
            tauri::async_runtime::block_on(guarded(id.to_string(), "Kilo".into(), fut))
        });
        started_rx.recv().unwrap();
        drop(KeyCardMutationGuard::begin(vec![id.to_string()]));
        release_tx.send(()).unwrap();
        let snap = handle.join().expect("guarded thread");
        assert_eq!(snap.status, "error");
        assert!(
            fail_state().lock().unwrap().get(id).is_none(),
            "a late failure from the old key must not bench the new key"
        );
    }

    #[test]
    fn a_snapshot_younger_than_the_reuse_window_is_served_without_a_fetch() {
        let id = "kilo-fresh";
        let _guard = SnapCacheGuard::new(id);
        let now = super::now_ms() as i64;
        let mut fresh = Snapshot::ok(id, "Kilo", None, vec![]);
        fresh.fetched_at = Some(now - 10_000);
        last_ok().lock().unwrap().insert(id.into(), CachedSnap { at: now - 10_000, snap: fresh });
        let snap = tauri::async_runtime::block_on(guarded(id.to_string(), "Kilo".into(), async {
            panic!("a fetch ten seconds after a good one is the double call that gets rate limited");
            #[allow(unreachable_code)]
            Snapshot::ok(id, "Kilo", None, vec![])
        }));
        assert_eq!(snap.status, "ok");
        assert_eq!(snap.fetched_at, Some(now - 10_000), "the real fetch time survives, so the footer can say it");
        assert!(!snap.attempt_failed);
        assert!(!snap.stale);
    }

    #[test]
    fn a_snapshot_older_than_the_reuse_window_is_fetched_again() {
        let id = "kilo-old";
        let _guard = SnapCacheGuard::new(id);
        let now = super::now_ms() as i64;
        let mut old = Snapshot::ok(id, "Kilo", None, vec![]);
        old.fetched_at = Some(now - 120_000);
        last_ok().lock().unwrap().insert(id.into(), CachedSnap { at: now - 120_000, snap: old });
        let snap = tauri::async_runtime::block_on(guarded(id.to_string(), "Kilo".into(), async {
            let mut s = Snapshot::ok(id, "Kilo", None, vec![]);
            s.plan = Some("fetched".into());
            s
        }));
        assert_eq!(snap.plan.as_deref(), Some("fetched"), "two minutes old is worth a real fetch");
    }

    #[test]
    fn fresh_failure_without_rotation_still_benches() {
        let id = "kilo-control";
        let _guard = SnapCacheGuard::new(id);
        let snap = tauri::async_runtime::block_on(guarded(id.to_string(), "Kilo".into(), async {
            Snapshot::error(id, "Kilo", "HTTP 429 rate limited".into())
        }));
        assert_eq!(snap.status, "error");
        assert!(fail_state().lock().unwrap().contains_key(id));
    }

    #[test]
    fn clearing_missing_key_file_succeeds_and_changes_nothing() {
        let tmp = TempConfig::new();
        let id = "qwen";
        let _guard = SnapCacheGuard::new(id);
        let before = key_card_snapshot_generations([id.to_string()]);
        set_api_key_in(&tmp.dir, id, "").unwrap();
        let after = key_card_snapshot_generations([id.to_string()]);
        assert_eq!(before.get(id), after.get(id));
        assert!(!tmp.dir.join(format!("{id}.json")).exists());
    }

    #[cfg(windows)]
    #[test]
    fn clearing_blocked_key_file_reports_failure_and_keeps_state() {
        let tmp = TempConfig::new();
        let id = "zai";
        let _guard = SnapCacheGuard::new(id);
        set_api_key_in(&tmp.dir, id, "key-a").unwrap();
        seed_cached_ok(id, "Z.ai");
        let held = hold_no_delete(&tmp.dir.join(format!("{id}.json")));
        let result = set_api_key_in(&tmp.dir, id, "");
        drop(held);
        let error = result.expect_err("a locked key file must not report success");
        assert!(error.contains("remove key file"), "{error}");
        assert!(stored_pane_api_key(&tmp.dir.join(format!("{id}.json"))).is_some());
        assert!(last_ok().lock().unwrap().contains_key(id));
    }

    #[cfg(windows)]
    #[test]
    fn failed_key_save_keeps_the_old_credential_and_state() {
        let tmp = TempConfig::new();
        let id = "minimax";
        let _guard = SnapCacheGuard::new(id);
        set_api_key_in(&tmp.dir, id, "key-a").unwrap();
        seed_cached_ok(id, "MiniMax");
        let expected = key_card_snapshot_generations([id.to_string()]);
        let held = hold_no_delete(&tmp.dir.join(format!("{id}.json")));
        let result = set_api_key_in(&tmp.dir, id, "key-b");
        drop(held);
        assert!(result.is_err(), "a failed write must not report success");
        assert_eq!(
            stored_pane_api_key(&tmp.dir.join(format!("{id}.json"))).as_deref(),
            Some("key-a")
        );
        assert!(last_ok().lock().unwrap().contains_key(id));
        let after = key_card_snapshot_generations([id.to_string()]);
        assert_eq!(expected.get(id), after.get(id));
    }

    #[test]
    fn forget_provider_snapshot_clears_fail_state_and_last_ok() {
        let id = "onenewapi@ticket03-forget";
        let _guard = SnapCacheGuard::new(id);
        fail_state().lock().unwrap().insert(
            id.to_string(),
            FailState {
                until_ms: i64::MAX,
                note: "benched".into(),
            },
        );
        last_ok().lock().unwrap().insert(
            id.to_string(),
            CachedSnap {
                at: 1,
                snap: Snapshot::ok(
                    id,
                    "Panel · Old",
                    None,
                    vec![Metric::text("Limit", "$10.00".into())],
                ),
            },
        );
        forget_provider_snapshot(id).unwrap();
        assert!(!fail_state().lock().unwrap().contains_key(id));
        assert!(!last_ok().lock().unwrap().contains_key(id));
    }

    #[test]
    fn rename_cached_snapshot_updates_name_only() {
        let id = "onenewapi@ticket03-rename";
        let _guard = SnapCacheGuard::new(id);
        fail_state().lock().unwrap().insert(
            id.to_string(),
            FailState {
                until_ms: i64::MAX,
                note: "benched".into(),
            },
        );
        last_ok().lock().unwrap().insert(
            id.to_string(),
            CachedSnap {
                at: 42,
                snap: Snapshot::ok(
                    id,
                    "Panel · Old",
                    None,
                    vec![Metric::text("Limit", "$10.00".into())],
                ),
            },
        );
        rename_cached_snapshot(id, "Panel · New".into()).unwrap();
        let map = last_ok().lock().unwrap();
        let entry = map.get(id).unwrap();
        assert_eq!(entry.snap.name, "Panel · New");
        assert_eq!(entry.at, 42);
        assert_eq!(entry.snap.status, "ok");
        assert_eq!(entry.snap.metrics.len(), 1);
        assert_eq!(entry.snap.metrics[0].label, "Limit");
        assert_eq!(entry.snap.metrics[0].value.as_deref(), Some("$10.00"));
        drop(map);
        assert_eq!(
            fail_state().lock().unwrap().get(id).unwrap().note,
            "benched"
        );
    }

    #[test]
    fn onenewapi_cached_rename_write_failure_keeps_old_name() {
        let id = "onenewapi@ticket03-rename-fail";
        let mut map = HashMap::from([(
            id.to_string(),
            CachedSnap {
                at: 42,
                snap: Snapshot::ok(id, "Panel · Old", None, vec![]),
            },
        )]);
        let result = rename_cached_snapshot_in(&mut map, id, "Panel · New".into(), |_| {
            Err("snapshot cache locked".into())
        });
        assert_eq!(result.unwrap_err(), "snapshot cache locked");
        assert_eq!(map.get(id).unwrap().snap.name, "Panel · Old");
    }

    #[test]
    fn onenewapi_multi_key_rename_write_failure_keeps_all_old_names() {
        let a = "onenewapi@ticket06-rename-a";
        let b = "onenewapi@ticket06-rename-b";
        let mut map = HashMap::from([
            (
                a.to_string(),
                CachedSnap {
                    at: 1,
                    snap: Snapshot::ok(a, "Old · One", None, vec![]),
                },
            ),
            (
                b.to_string(),
                CachedSnap {
                    at: 2,
                    snap: Snapshot::ok(b, "Old · Two", None, vec![]),
                },
            ),
        ]);
        let result = rename_cached_snapshots_in(
            &mut map,
            &[
                (a.to_string(), "New · One".into()),
                (b.to_string(), "New · Two".into()),
            ],
            |_| Err("snapshot cache locked".into()),
        );
        assert_eq!(result.unwrap_err(), "snapshot cache locked");
        assert_eq!(map.get(a).unwrap().snap.name, "Old · One");
        assert_eq!(map.get(b).unwrap().snap.name, "Old · Two");
        assert_eq!(map.get(a).unwrap().at, 1);
        assert_eq!(map.get(b).unwrap().at, 2);
    }

    fn seed_onenewapi_cache(key_id: &str, name: &str) -> SnapCacheGuard {
        let id = format!("onenewapi@{key_id}");
        fail_state().lock().unwrap().insert(
            id.clone(),
            FailState {
                until_ms: i64::MAX,
                note: "benched".into(),
            },
        );
        last_ok().lock().unwrap().insert(
            id.clone(),
            CachedSnap {
                at: 42,
                snap: Snapshot::ok(
                    &id,
                    name,
                    None,
                    vec![Metric::text("Limit", "$10.00".into())],
                ),
            },
        );
        SnapCacheGuard::new(&id)
    }

    fn onenewapi_site(
        id: &str,
        name: &str,
        base_url: &str,
        keys: &[(&str, &str)],
    ) -> crate::providers::onenewapi::SiteDto {
        crate::providers::onenewapi::SiteDto {
            id: id.into(),
            name: name.into(),
            base_url: base_url.into(),
            keys: keys
                .iter()
                .map(|(kid, label)| crate::providers::onenewapi::KeyDto {
                    id: (*kid).into(),
                    label: (*label).into(),
                    has_api_key: true,
                })
                .collect(),
        }
    }

    #[test]
    fn forget_onenewapi_key_ids_clears_listed_keys_only() {
        let _a = seed_onenewapi_cache("keep-a", "Panel · A");
        let _b = seed_onenewapi_cache("drop-b", "Panel · B");
        let _c = seed_onenewapi_cache("drop-c", "Panel · C");
        forget_onenewapi_key_ids(["drop-b".into(), "drop-c".into()]).unwrap();
        assert!(last_ok().lock().unwrap().contains_key("onenewapi@keep-a"));
        assert!(fail_state()
            .lock()
            .unwrap()
            .contains_key("onenewapi@keep-a"));
        assert!(!last_ok().lock().unwrap().contains_key("onenewapi@drop-b"));
        assert!(!fail_state()
            .lock()
            .unwrap()
            .contains_key("onenewapi@drop-b"));
        assert!(!last_ok().lock().unwrap().contains_key("onenewapi@drop-c"));
        assert!(!fail_state()
            .lock()
            .unwrap()
            .contains_key("onenewapi@drop-c"));
    }

    #[test]
    fn onenewapi_url_change_forgets_that_sites_keys() {
        let _a = seed_onenewapi_cache("site-a1", "Panel · One");
        let _b = seed_onenewapi_cache("site-a2", "Panel · Two");
        let _other = seed_onenewapi_cache("other-1", "Other · One");
        let previous = onenewapi_site(
            "site-a",
            "Panel",
            "http://127.0.0.1:1",
            &[("site-a1", "One"), ("site-a2", "Two")],
        );
        let updated = onenewapi_site(
            "site-a",
            "Panel",
            "http://127.0.0.1:2",
            &[("site-a1", "One"), ("site-a2", "Two")],
        );
        forget_onenewapi_key_ids(updated.keys.iter().map(|key| key.id.clone())).unwrap();
        onenewapi_after_site_save(&previous, &updated).unwrap();
        assert!(!last_ok().lock().unwrap().contains_key("onenewapi@site-a1"));
        assert!(!last_ok().lock().unwrap().contains_key("onenewapi@site-a2"));
        assert!(!fail_state()
            .lock()
            .unwrap()
            .contains_key("onenewapi@site-a1"));
        assert!(last_ok().lock().unwrap().contains_key("onenewapi@other-1"));
        assert!(fail_state()
            .lock()
            .unwrap()
            .contains_key("onenewapi@other-1"));
    }

    #[test]
    fn onenewapi_name_change_renames_child_cache_without_clearing() {
        let _a = seed_onenewapi_cache("site-n1", "Old · One");
        let _b = seed_onenewapi_cache("site-n2", "Old · Two");
        let previous = onenewapi_site(
            "site-n",
            "Old",
            "http://127.0.0.1:1",
            &[("site-n1", "One"), ("site-n2", "Two")],
        );
        let updated = onenewapi_site(
            "site-n",
            "New",
            "http://127.0.0.1:1",
            &[("site-n1", "One"), ("site-n2", "Two")],
        );
        onenewapi_after_site_save(&previous, &updated).unwrap();
        let map = last_ok().lock().unwrap();
        assert_eq!(map.get("onenewapi@site-n1").unwrap().snap.name, "New · One");
        assert_eq!(map.get("onenewapi@site-n2").unwrap().snap.name, "New · Two");
        assert_eq!(map.get("onenewapi@site-n1").unwrap().at, 42);
        assert_eq!(map.get("onenewapi@site-n1").unwrap().snap.metrics.len(), 1);
        drop(map);
        assert_eq!(
            fail_state()
                .lock()
                .unwrap()
                .get("onenewapi@site-n1")
                .unwrap()
                .note,
            "benched"
        );
    }

    fn sample_card_layout() -> Value {
        json!({
            "metricOrder": ["Usage"],
            "onDemand": [],
            "hidden": [],
            "starred": ["Usage"],
            "expanded": false
        })
    }

    #[test]
    fn purge_onenewapi_from_config_drops_only_those_snapshot_ids() {
        let mut cfg = json!({
            "disabled": ["onenewapi", "onenewapi@drop", "onenewapi@keep", "aihubmix"],
            "layout": {
                "providerOrder": [
                    "aihubmix",
                    "onenewapi",
                    "onenewapi@drop",
                    "onenewapi@keep",
                    "onenewapi@other"
                ],
                "providers": {
                    "aihubmix": sample_card_layout(),
                    "onenewapi": sample_card_layout(),
                    "onenewapi@drop": sample_card_layout(),
                    "onenewapi@keep": sample_card_layout(),
                    "onenewapi@other": sample_card_layout()
                }
            },
            "pinned": {"provider": "onenewapi@drop", "label": "Usage"},
            "trayProviders": ["onenewapi@drop", "aihubmix", "onenewapi@keep"]
        });
        let patch = purge_key_cards_from_config(&mut cfg, &["onenewapi@drop".into()]);
        assert_eq!(
            cfg["disabled"],
            json!(["onenewapi", "onenewapi@keep", "aihubmix"])
        );
        assert_eq!(
            cfg["layout"]["providerOrder"],
            json!(["aihubmix", "onenewapi", "onenewapi@keep", "onenewapi@other"])
        );
        assert!(cfg["layout"]["providers"].get("onenewapi@drop").is_none());
        assert!(cfg["layout"]["providers"].get("onenewapi@keep").is_some());
        assert!(cfg["layout"]["providers"].get("onenewapi@other").is_some());
        assert!(cfg["layout"]["providers"].get("aihubmix").is_some());
        assert!(cfg["layout"]["providers"].get("onenewapi").is_some());
        assert_eq!(cfg["pinned"], Value::Null);
        assert_eq!(cfg["trayProviders"], json!(["aihubmix", "onenewapi@keep"]));
        assert!(patch.get("disabled").is_some());
        assert!(patch.get("layout").is_some());
        assert_eq!(patch["pinned"], Value::Null);
        assert!(patch.get("trayProviders").is_some());
    }

    #[test]
    fn purge_onenewapi_from_config_keeps_family_disabled_and_unrelated_pin() {
        let mut cfg = json!({
            "disabled": ["onenewapi", "onenewapi@drop"],
            "layout": {
                "providerOrder": ["aihubmix", "onenewapi", "onenewapi@drop"],
                "providers": {
                    "aihubmix": sample_card_layout(),
                    "onenewapi@drop": sample_card_layout()
                }
            },
            "pinned": {"provider": "aihubmix", "label": "Usage"},
            "trayProviders": ["aihubmix"]
        });
        let patch = purge_key_cards_from_config(&mut cfg, &["onenewapi@drop".into()]);
        assert_eq!(cfg["disabled"], json!(["onenewapi"]));
        assert_eq!(
            cfg["pinned"],
            json!({"provider": "aihubmix", "label": "Usage"})
        );
        assert_eq!(cfg["trayProviders"], json!(["aihubmix"]));
        assert_eq!(
            cfg["layout"]["providerOrder"],
            json!(["aihubmix", "onenewapi"])
        );
        assert!(patch.get("pinned").is_none());
        assert!(patch.get("trayProviders").is_none());
    }

    #[test]
    fn purge_onenewapi_from_config_drops_all_site_keys_keeps_other_sites() {
        let mut cfg = json!({
            "disabled": ["onenewapi@a1", "onenewapi@a2", "onenewapi@b1", "aihubmix"],
            "layout": {
                "providerOrder": [
                    "aihubmix",
                    "onenewapi@a1",
                    "onenewapi@a2",
                    "onenewapi@b1"
                ],
                "providers": {
                    "aihubmix": sample_card_layout(),
                    "onenewapi@a1": sample_card_layout(),
                    "onenewapi@a2": sample_card_layout(),
                    "onenewapi@b1": sample_card_layout()
                }
            },
            "pinned": {"provider": "onenewapi@a2", "label": "Usage"},
            "trayProviders": ["onenewapi@a1", "onenewapi@b1", "aihubmix"]
        });
        let patch =
            purge_key_cards_from_config(&mut cfg, &["onenewapi@a1".into(), "onenewapi@a2".into()]);
        assert_eq!(cfg["disabled"], json!(["onenewapi@b1", "aihubmix"]));
        assert_eq!(
            cfg["layout"]["providerOrder"],
            json!(["aihubmix", "onenewapi@b1"])
        );
        assert!(cfg["layout"]["providers"].get("onenewapi@a1").is_none());
        assert!(cfg["layout"]["providers"].get("onenewapi@a2").is_none());
        assert!(cfg["layout"]["providers"].get("onenewapi@b1").is_some());
        assert!(cfg["layout"]["providers"].get("aihubmix").is_some());
        assert_eq!(cfg["pinned"], Value::Null);
        assert_eq!(cfg["trayProviders"], json!(["onenewapi@b1", "aihubmix"]));
        assert!(patch.get("disabled").is_some());
    }

    #[test]
    fn purge_onenewapi_cards_drops_one_key_cache_and_alerts() {
        let _keep = seed_onenewapi_cache("ticket07-keep", "Panel · Keep");
        let _drop = seed_onenewapi_cache("ticket07-drop", "Panel · Drop");
        let _other = seed_onenewapi_cache("ticket07-other", "Other · One");
        alerts::insert_state_for_test("onenewapi@ticket07-drop:Usage");
        alerts::insert_state_for_test("onenewapi@ticket07-keep:Usage");
        alerts::insert_state_for_test("onenewapi@ticket07-other:Usage");
        purge_onenewapi_cards(&["ticket07-drop".into()]).unwrap();
        assert!(last_ok()
            .lock()
            .unwrap()
            .contains_key("onenewapi@ticket07-keep"));
        assert!(fail_state()
            .lock()
            .unwrap()
            .contains_key("onenewapi@ticket07-keep"));
        assert!(!last_ok()
            .lock()
            .unwrap()
            .contains_key("onenewapi@ticket07-drop"));
        assert!(!fail_state()
            .lock()
            .unwrap()
            .contains_key("onenewapi@ticket07-drop"));
        assert!(last_ok()
            .lock()
            .unwrap()
            .contains_key("onenewapi@ticket07-other"));
        assert!(fail_state()
            .lock()
            .unwrap()
            .contains_key("onenewapi@ticket07-other"));
        assert!(!alerts::has_state_for_test("onenewapi@ticket07-drop:Usage"));
        assert!(alerts::has_state_for_test("onenewapi@ticket07-keep:Usage"));
        assert!(alerts::has_state_for_test("onenewapi@ticket07-other:Usage"));
        alerts::forget_snapshot("onenewapi@ticket07-keep");
        alerts::forget_snapshot("onenewapi@ticket07-other");
    }

    #[test]
    fn purge_onenewapi_cards_config_save_failure_keeps_snapshots_and_alerts() {
        let _keep = seed_onenewapi_cache("ticket07-keep-cfg", "Panel · Keep");
        let _drop = seed_onenewapi_cache("ticket07-drop-cfg", "Panel · Drop");
        alerts::insert_state_for_test("onenewapi@ticket07-drop-cfg:Usage");
        alerts::insert_state_for_test("onenewapi@ticket07-keep-cfg:Usage");
        let result = purge_onenewapi_cards_with(&["ticket07-drop-cfg".into()], |_| {
            Err("config locked".into())
        });
        assert_eq!(result.unwrap_err(), "config locked");
        assert!(last_ok()
            .lock()
            .unwrap()
            .contains_key("onenewapi@ticket07-drop-cfg"));
        assert!(fail_state()
            .lock()
            .unwrap()
            .contains_key("onenewapi@ticket07-drop-cfg"));
        assert!(last_ok()
            .lock()
            .unwrap()
            .contains_key("onenewapi@ticket07-keep-cfg"));
        assert!(fail_state()
            .lock()
            .unwrap()
            .contains_key("onenewapi@ticket07-keep-cfg"));
        assert!(alerts::has_state_for_test(
            "onenewapi@ticket07-drop-cfg:Usage"
        ));
        assert!(alerts::has_state_for_test(
            "onenewapi@ticket07-keep-cfg:Usage"
        ));
        alerts::forget_snapshot("onenewapi@ticket07-drop-cfg");
        alerts::forget_snapshot("onenewapi@ticket07-keep-cfg");
    }

    #[test]
    fn purge_restores_card_settings_when_cache_cleanup_fails() {
        let cfg = std::cell::RefCell::new(json!({
            "disabled": ["onenewapi@drop", "aihubmix"],
            "layout": {
                "providerOrder": ["onenewapi@drop", "aihubmix"],
                "providers": {
                    "onenewapi@drop": {"starred": ["Usage"]},
                    "aihubmix": {"starred": ["Usage"]}
                }
            },
            "pinned": {"provider": "onenewapi@drop", "metric": "Usage"},
            "trayProviders": ["onenewapi@drop", "aihubmix"]
        }));
        let original = cfg.borrow().clone();
        let _keep = seed_onenewapi_cache("keep", "Panel · Keep");
        let _drop = seed_onenewapi_cache("drop", "Panel · Drop");
        alerts::insert_state_for_test("onenewapi@drop:Usage");
        alerts::insert_state_for_test("onenewapi@keep:Usage");
        let result = purge_onenewapi_cards_coordinated(
            &["drop".into()],
            |ids| {
                let mut cfg = cfg.borrow_mut();
                let before = cfg.clone();
                let patch = purge_key_cards_from_config(&mut cfg, ids);
                assert!(!patch.as_object().unwrap().is_empty());
                assert_ne!(*cfg, before);
                Ok(key_cards_purge_restore_patch(&before, &patch))
            },
            |_| Err("cache locked".into()),
            |restore| {
                let mut cfg = cfg.borrow_mut();
                if let Some(obj) = restore.as_object() {
                    for (k, v) in obj {
                        cfg[k.clone()] = v.clone();
                    }
                }
                Ok(())
            },
        );
        assert_eq!(result.unwrap_err(), "cache locked");
        assert_eq!(*cfg.borrow(), original);
        assert!(last_ok().lock().unwrap().contains_key("onenewapi@drop"));
        assert!(last_ok().lock().unwrap().contains_key("onenewapi@keep"));
        assert!(alerts::has_state_for_test("onenewapi@drop:Usage"));
        assert!(alerts::has_state_for_test("onenewapi@keep:Usage"));
        alerts::forget_snapshot("onenewapi@drop");
        alerts::forget_snapshot("onenewapi@keep");
    }

    #[test]
    fn purge_onenewapi_cards_drops_all_site_child_cache() {
        let _a1 = seed_onenewapi_cache("ticket07-a1", "Panel · One");
        let _a2 = seed_onenewapi_cache("ticket07-a2", "Panel · Two");
        let _b1 = seed_onenewapi_cache("ticket07-b1", "Other · One");
        alerts::insert_state_for_test("onenewapi@ticket07-a1:Usage");
        alerts::insert_state_for_test("onenewapi@ticket07-a2:Usage");
        alerts::insert_state_for_test("onenewapi@ticket07-b1:Usage");
        purge_onenewapi_cards(&["ticket07-a1".into(), "ticket07-a2".into()]).unwrap();
        assert!(!last_ok()
            .lock()
            .unwrap()
            .contains_key("onenewapi@ticket07-a1"));
        assert!(!last_ok()
            .lock()
            .unwrap()
            .contains_key("onenewapi@ticket07-a2"));
        assert!(last_ok()
            .lock()
            .unwrap()
            .contains_key("onenewapi@ticket07-b1"));
        assert!(!alerts::has_state_for_test("onenewapi@ticket07-a1:Usage"));
        assert!(!alerts::has_state_for_test("onenewapi@ticket07-a2:Usage"));
        assert!(alerts::has_state_for_test("onenewapi@ticket07-b1:Usage"));
        alerts::forget_snapshot("onenewapi@ticket07-b1");
    }

}
