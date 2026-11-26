//! The JSON API under `/api`. Reads the run archive and the config the CLI
//! writes; changes go through the CLI's own functions (`config_set`) or a
//! spawned osearch (`jobs`).

use super::UiState;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

type Answer = Result<Json<Value>, (StatusCode, String)>;

fn bad(msg: impl Into<String>) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, msg.into())
}

fn missing(msg: impl Into<String>) -> (StatusCode, String) {
    (StatusCode::NOT_FOUND, msg.into())
}

pub fn routes() -> Router<Arc<UiState>> {
    Router::new()
        .route("/info", get(info))
        .route("/config", get(config).post(set_config))
        .route("/runs", get(runs))
        .route("/runs/{id}", get(run))
        .route("/runs/{id}/calls", get(calls))
        .route("/runs/{id}/results", get(results))
        .route("/runs/{id}/pages/{pid}", get(page))
        .route("/runs/{id}/raw/{file}", get(raw))
        .route("/insights", get(insights))
        .route("/stats", get(stats))
        .route("/engines", get(engines))
        .route("/usage", post(usage))
        .route("/jobs", get(super::jobs::list).post(super::jobs::start))
        .route("/jobs/{id}", get(super::jobs::status))
        .route("/jobs/{id}/stop", post(super::jobs::stop))
}

/// A name from the URL that may only be a plain file or folder name.
fn plain(name: &str) -> Result<&str, (StatusCode, String)> {
    let ok = !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(name)
    } else {
        Err(bad(format!("not a plain name: {name}")))
    }
}

pub(super) fn run_dir(id: &str) -> Result<PathBuf, (StatusCode, String)> {
    let dir = crate::run::runs_root().join(plain(id)?);
    if dir.is_dir() {
        Ok(dir)
    } else {
        Err(missing(format!("no run {id}")))
    }
}

fn text(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn json_file(path: &Path) -> Value {
    text(path).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
}

pub(super) fn jsonl(path: &Path) -> Vec<Value> {
    crate::run::read_text(path)
        .ok()
        .map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
        .unwrap_or_default()
}

fn clip(v: &mut Value, field: &str, max: usize) {
    if let Some(s) = v.get(field).and_then(Value::as_str) {
        if s.len() > max {
            let mut cut = max;
            while !s.is_char_boundary(cut) {
                cut -= 1;
            }
            v[field] = json!(format!("{}…", &s[..cut]));
        }
    }
}

async fn info(State(state): State<Arc<UiState>>) -> Json<Value> {
    let c = crate::config::load_config().unwrap_or_else(|_| state.app.config.clone());
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "config_file": crate::config::config_path(),
        "data_folder": crate::config::data_dir(),
        "archive": c.paths.archive,
        "synth_configured": !c.synth.url.is_empty() && !c.synth.model.is_empty(),
        "notes": c.ui.notes,
    }))
}

/// The live config file's values (keys masked) and the setting catalogue.
async fn config() -> Answer {
    let c = crate::config::load_config().map_err(|e| bad(e.to_string()))?;
    let mut v = serde_json::to_value(&c).map_err(|e| bad(e.to_string()))?;
    if let Some(keys) = v.get_mut("keys").and_then(Value::as_object_mut) {
        for val in keys.values_mut() {
            *val = json!(crate::config::mask_key(val.as_str().unwrap_or("")));
        }
    }
    if let Some(k) = v.pointer_mut("/synth/key") {
        *k = json!(crate::config::mask_key(k.as_str().unwrap_or("")));
    }
    let mut defaults = serde_json::to_value(crate::config::AppConfig::default()).map_err(|e| bad(e.to_string()))?;
    if let Some(k) = defaults.pointer_mut("/synth/key") {
        *k = json!("");
    }
    Ok(Json(json!({
        "path": crate::config::config_path(),
        "values": v,
        "defaults": defaults,
        "settings": super::settings::SETTINGS,
    })))
}

#[derive(Deserialize)]
struct SetBody {
    key: String,
    value: String,
}

async fn set_config(Json(body): Json<SetBody>) -> Answer {
    if !super::settings::SETTINGS.iter().any(|s| s.key == body.key) {
        return Err(bad(format!("{} is not a setting the UI changes", body.key)));
    }
    // A secret is never shown, so its masked form coming back is not a new value.
    if body.key == "synth.key" {
        let current = crate::config::get().synth.key.clone();
        if body.value.trim().is_empty() || body.value == crate::config::mask_key(&current) || body.value == "(not set)" {
            return Err(bad("paste a new key to change it; the saved one stays as it is"));
        }
    }
    crate::config::config_set(&body.key, &body.value).map_err(|e| bad(e.to_string()))?;
    config().await
}

/// Every run, newest first, its rounds folded into one row.
async fn runs() -> Json<Value> {
    let indexed = crate::run::all_runs();
    let mut lines = indexed.clone();
    lines.extend(crate::run::unfinished_runs(&indexed));
    let root = crate::run::runs_root();
    let mut by_id: BTreeMap<String, Value> = BTreeMap::new();
    for l in lines {
        let row = by_id.entry(l.id.clone()).or_insert_with(|| {
            json!({
                "id": l.id, "topic": l.topic, "kind": l.kind, "ts": l.ts, "rounds": 0,
                "queries": [], "urls": 0, "pages": 0, "cost_usd": 0.0,
            })
        });
        row["rounds"] = json!(row["rounds"].as_u64().unwrap_or(0) + 1);
        row["urls"] = json!(row["urls"].as_u64().unwrap_or(0).max(l.urls as u64));
        row["pages"] = json!(row["pages"].as_u64().unwrap_or(0) + l.pages as u64);
        row["cost_usd"] = json!(row["cost_usd"].as_f64().unwrap_or(0.0) + l.cost_usd);
        if let Some(q) = row["queries"].as_array_mut() {
            q.extend(l.queries.into_iter().map(Value::from));
        }
    }
    let mut out: Vec<Value> = by_id
        .into_values()
        .filter(|r| root.join(r["id"].as_str().unwrap_or("")).is_dir())
        .map(|mut r| {
            let dir = root.join(r["id"].as_str().unwrap_or(""));
            r["synthesis"] = json!(dir.join("synthesis.md").is_file());
            r
        })
        .collect();
    out.sort_by(|a, b| b["id"].as_str().cmp(&a["id"].as_str()));
    Json(json!(out))
}

async fn run(UrlPath(id): UrlPath<String>) -> Answer {
    let dir = run_dir(&id)?;
    let mut rounds = Vec::new();
    for n in 1.. {
        let req = dir.join(format!("request.r{n}.json"));
        if !req.is_file() {
            break;
        }
        rounds.push(json!({
            "round": n,
            "request": json_file(&req),
            "summary": text(&dir.join(format!("summary.r{n}.md"))),
            "picks": json_file(&dir.join(format!("picks.r{n}.json"))),
            "use": json_file(&dir.join(format!("use.r{n}.json"))),
            "fetching": dir.join(format!("fetch.r{n}.running")).exists(),
        }));
    }
    let index: Vec<Value> = crate::run::all_runs()
        .into_iter()
        .filter(|l| l.id == id)
        .filter_map(|l| serde_json::to_value(l).ok())
        .collect();
    // Each round's synthesis and the whole-run one, with what each was sent.
    let mut syntheses: Vec<Value> = Vec::new();
    let tags = (1..=rounds.len()).map(|n| (format!("r{n}"), format!("Round {n}"))).chain([("all".to_string(), "Whole run".to_string())]);
    for (tag, label) in tags {
        let file = dir.join(format!("synthesis.{tag}.md"));
        if file.is_file() {
            syntheses.push(json!({
                "tag": tag,
                "label": label,
                "text": text(&file),
                "figures": json_file(&dir.join(format!("synthesis.{tag}.json"))),
            }));
        }
    }
    Ok(Json(json!({
        "id": id,
        "rounds": rounds,
        "index": index,
        "summary": text(&dir.join("summary.md")),
        "synthesis": text(&dir.join("synthesis.md")),
        "syntheses": syntheses,
        "synthesis_running": dir.join(crate::synth::RUNNING).exists(),
        "jina_running": dir.join("jina-deepsearch.running").exists(),
        "pages": crate::run::load_pages(&dir),
    })))
}

async fn calls(UrlPath(id): UrlPath<String>) -> Answer {
    let dir = run_dir(&id)?;
    let mut rows = jsonl(&dir.join("calls.jsonl"));
    for r in rows.iter_mut() {
        clip(r, "request", 4000);
    }
    Ok(Json(json!(rows)))
}

async fn results(UrlPath(id): UrlPath<String>) -> Answer {
    let dir = run_dir(&id)?;
    let mut rows = jsonl(&dir.join("results.jsonl"));
    for r in rows.iter_mut() {
        if let Some(seen) = r.get_mut("found_by").and_then(Value::as_array_mut) {
            for s in seen.iter_mut() {
                clip(s, "snippet", 700);
            }
        }
    }
    Ok(Json(json!(rows)))
}

async fn page(UrlPath((id, pid)): UrlPath<(String, String)>) -> Answer {
    let dir = run_dir(&id)?;
    if !(pid.starts_with('p') && pid.len() > 1 && pid[1..].chars().all(|c| c.is_ascii_digit())) {
        return Err(bad(format!("not a page id: {pid}")));
    }
    let md = text(&dir.join("pages").join(format!("{pid}.md"))).ok_or_else(|| missing(format!("{pid} has no saved text (it failed or is still fetching)")))?;
    Ok(Json(json!({ "id": pid, "text": md })))
}

async fn raw(UrlPath((id, file)): UrlPath<(String, String)>) -> Answer {
    let dir = run_dir(&id)?;
    let path = dir.join("raw").join(plain(&file)?);
    let body = std::fs::read(&path)
        .or_else(|_| crate::run::read_text(&path).map(String::into_bytes))
        .map_err(|_| missing(format!("no raw file {file}")))?;
    let cap = 2_000_000;
    let cut = body.len() > cap;
    let text = String::from_utf8_lossy(&body[..body.len().min(cap)]).into_owned();
    Ok(Json(json!({ "file": file, "text": text, "cut": cut, "bytes": body.len() })))
}

#[derive(Deserialize)]
struct Days {
    days: Option<u64>,
}

async fn stats(Query(q): Query<Days>) -> Json<Value> {
    let days = q.days.unwrap_or(30).clamp(1, 3650);
    let report = tokio::task::spawn_blocking(move || crate::stats::compute(days)).await.ok();
    Json(serde_json::to_value(report).unwrap_or(Value::Null))
}

async fn insights(State(state): State<Arc<UiState>>) -> Json<Value> {
    let state = state.clone();
    let v = tokio::task::spawn_blocking(move || state.insights.all()).await.unwrap_or(Value::Null);
    Json(v)
}

/// Engines and fetchers, whether each is set up, and every balance.
async fn engines(State(state): State<Arc<UiState>>) -> Json<Value> {
    let engines: Vec<Value> = crate::providers::build_providers(&state.app)
        .into_iter()
        .map(|p| {
            let s = p.spec();
            json!({
                "name": s.name,
                "configured": p.is_configured(),
                "keyless": s.env_keys.is_empty(),
                "per_call_usd": s.price.per_call as f64 / 1e6,
                "modes": p.capabilities(),
                "optional": crate::providers::SOCIAL_PLATFORMS.contains(&s.name),
                // The ScrapeCreators platforms draw on one shared balance.
                "balance": if crate::providers::social_api::PLATFORMS.contains(&s.name) { "scrapecreators" } else { s.name },
            })
        })
        .collect();
    let balances = tokio::task::spawn_blocking(crate::budget::statuses).await.unwrap_or_default();
    Json(json!({ "engines": engines, "balances": balances }))
}

/// Live balances from each provider (Brave's check costs one request).
async fn usage(State(state): State<Arc<UiState>>) -> Json<Value> {
    let report = crate::usage::report(state.app.clone()).await;
    Json(serde_json::to_value(report).unwrap_or(Value::Null))
}
