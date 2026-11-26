//! Searches started from the page: each one is osearch itself, run with a
//! request file (the same path the CLI takes), its output kept for the page
//! and its run folder watched for progress.

use super::UiState;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};

#[derive(Default)]
pub struct Jobs {
    inner: Mutex<BTreeMap<u64, Arc<Mutex<Job>>>>,
    next: std::sync::atomic::AtomicU64,
}

struct Job {
    id: u64,
    kind: String,
    title: String,
    /// Local time stamp when it started (`YYYY-MM-DD_HHMMSS`): run folders
    /// named after it belong to it.
    stamp: String,
    started: u64,
    ended: Option<u64>,
    code: Option<i32>,
    lines: Vec<String>,
    run: Option<String>,
    pid: Option<u32>,
}

#[derive(Deserialize)]
pub struct Start {
    /// run, research, approve, synth
    kind: String,
    /// The request file's JSON, for `run`.
    request: Option<Value>,
    /// The question, for `research` and `approve`.
    question: Option<String>,
    /// A run id, for `synth` (and a follow-up round of `run`).
    run: Option<String>,
}

fn now() -> u64 {
    crate::run::raw::now()
}

pub async fn start(State(state): State<Arc<UiState>>, Json(body): Json<Start>) -> Result<Json<Value>, (StatusCode, String)> {
    let bad = |m: &str| (StatusCode::BAD_REQUEST, m.to_string());
    let (args, title): (Vec<String>, String) = match body.kind.as_str() {
        "run" => {
            let mut req = body.request.ok_or_else(|| bad("a run needs a request"))?;
            if let Some(parent) = body.run.as_deref() {
                super::api::run_dir(parent)?;
                req["parent"] = json!(parent);
            }
            let title = req["topic"].as_str().unwrap_or("search").to_string();
            let dir = crate::config::data_dir().join("ui").join("requests");
            std::fs::create_dir_all(&dir).map_err(|e| bad(&e.to_string()))?;
            let file = dir.join(format!("{}.json", crate::run::local_stamp()));
            std::fs::write(&file, serde_json::to_string_pretty(&req).unwrap_or_default()).map_err(|e| bad(&e.to_string()))?;
            (vec!["run".into(), file.to_string_lossy().into_owned()], title)
        }
        "research" | "approve" => {
            let q = body.question.filter(|q| !q.trim().is_empty()).ok_or_else(|| bad("research needs a question"))?;
            let mut a = vec!["research".to_string(), q.clone()];
            if body.kind == "approve" {
                a.push("--approved".into());
            }
            (a, q)
        }
        "synth" => {
            let id = body.run.clone().ok_or_else(|| bad("synthesis needs a run"))?;
            super::api::run_dir(&id)?;
            (vec!["synth".into(), id.clone()], format!("synthesis of {id}"))
        }
        other => return Err(bad(&format!("unknown job kind {other}"))),
    };
    let id = state.jobs.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let job = Arc::new(Mutex::new(Job {
        id,
        kind: body.kind.clone(),
        title,
        stamp: crate::run::local_stamp(),
        started: now(),
        ended: None,
        code: None,
        lines: Vec::new(),
        // Known up front for a synthesis or a follow-up round.
        run: body.run.clone(),
        pid: None,
    }));
    state.jobs.inner.lock().expect("jobs").insert(id, job.clone());
    let exe = std::env::current_exe().map_err(|e| bad(&e.to_string()))?;
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("NO_COLOR", "1")
        .kill_on_drop(false);
    #[cfg(windows)]
    {
        // No console window per job when the UI runs without one.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| bad(&format!("could not start osearch: {e}")))?;
    job.lock().expect("job").pid = child.id();
    let (out, err) = (child.stdout.take(), child.stderr.take());
    for stream in [out.map(|o| Box::new(o) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), err.map(|e| Box::new(e) as _)]
        .into_iter()
        .flatten()
    {
        let job = job.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut j = job.lock().expect("job");
                if j.run.is_none() {
                    j.run = run_id_in(&line);
                }
                j.lines.push(line);
            }
        });
    }
    let waiter = job.clone();
    tokio::spawn(async move {
        let code = child.wait().await.ok().and_then(|s| s.code());
        // The output readers finish just after the process.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let mut j = waiter.lock().expect("job");
        j.code = code;
        j.ended = Some(now());
    });
    Ok(Json(json!({ "id": id })))
}

/// A run id in a line of osearch's output (`run <id>`, `--run <id>`).
fn run_id_in(line: &str) -> Option<String> {
    static ID: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\b(\d{4}-\d{2}-\d{2}_\d{6}_[A-Za-z0-9-]+)").expect("regex"));
    ID.captures(line).map(|c| c[1].to_string())
}

/// The newest run folder made since the job started (before osearch prints its id).
fn run_since(stamp: &str) -> Option<String> {
    let mut ids: Vec<String> = std::fs::read_dir(crate::run::runs_root())
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.as_str() >= stamp)
        .collect();
    ids.sort();
    ids.pop()
}

fn view(j: &Job, with_lines: bool) -> Value {
    // The folder guess only while a new run has not printed its id yet.
    let run = j.run.clone().or_else(|| (j.kind == "run" && j.ended.is_none()).then(|| run_since(&j.stamp)).flatten());
    let mut progress = json!(null);
    if let Some(id) = &run {
        let dir = crate::run::runs_root().join(id);
        let pages = crate::run::load_pages(&dir);
        progress = json!({
            "calls": super::api::jsonl(&dir.join("calls.jsonl")).len(),
            "pages_ok": pages.iter().filter(|p| p.ok).count(),
            "pages_failed": pages.iter().filter(|p| !p.ok).count(),
            "synthesis_running": dir.join(crate::synth::RUNNING).exists(),
        });
    }
    let mut v = json!({
        "id": j.id, "kind": j.kind, "title": j.title, "started": j.started, "ended": j.ended,
        "code": j.code, "run": run, "progress": progress, "line_count": j.lines.len(),
    });
    if with_lines {
        v["lines"] = json!(j.lines);
    }
    v
}

pub async fn list(State(state): State<Arc<UiState>>) -> Json<Value> {
    let jobs: Vec<Value> = state.jobs.inner.lock().expect("jobs").values().rev().map(|j| view(&j.lock().expect("job"), false)).collect();
    Json(json!(jobs))
}

pub async fn status(State(state): State<Arc<UiState>>, UrlPath(id): UrlPath<u64>) -> Result<Json<Value>, (StatusCode, String)> {
    let job = state.jobs.inner.lock().expect("jobs").get(&id).cloned().ok_or((StatusCode::NOT_FOUND, format!("no job {id}")))?;
    let v = view(&job.lock().expect("job"), true);
    Ok(Json(v))
}

/// Stops the osearch process of a job. Pages already fetching in the
/// background run on in their own process, as they do from the CLI.
pub async fn stop(State(state): State<Arc<UiState>>, UrlPath(id): UrlPath<u64>) -> Result<Json<Value>, (StatusCode, String)> {
    let job = state.jobs.inner.lock().expect("jobs").get(&id).cloned().ok_or((StatusCode::NOT_FOUND, format!("no job {id}")))?;
    let pid = {
        let j = job.lock().expect("job");
        if j.ended.is_some() {
            return Ok(Json(json!({ "stopped": false, "why": "it had already finished" })));
        }
        j.pid
    };
    let Some(pid) = pid else { return Err((StatusCode::CONFLICT, "no process to stop".into())) };
    let ok = if cfg!(windows) {
        std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).output().is_ok_and(|o| o.status.success())
    } else {
        std::process::Command::new("kill").arg(pid.to_string()).output().is_ok_and(|o| o.status.success())
    };
    Ok(Json(json!({ "stopped": ok })))
}
