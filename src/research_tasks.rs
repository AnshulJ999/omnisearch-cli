//! `osearch research "<question>"`: the engines' own research agents
//! (Tavily Research, You.com Research, Exa's agent, Parallel's Task API) as
//! background tasks.
//! They cost far more than a search, so nothing is called without
//! `--approved`: the command prints each engine's cost estimate instead.
//! Task ids live in the run folder (`research-tasks.json`); `--collect`
//! picks finished reports up later, saves each as a page with its sources,
//! and puts the real cost in the ledger.

use crate::context::AppContext;
use crate::errors::SearchError;
use crate::run::{raw, Run, RunLine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const ENGINES: &[&str] = &["tavily", "youcom", "exa", "parallel"];
const TASKS_FILE: &str = "research-tasks.json";
/// Tavily's pay-as-you-go credit price; free monthly credits are used first.
const TAVILY_CREDIT_USD: f64 = 0.008;
const WAIT_LIMIT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Depth {
    Light,
    Standard,
    Deep,
}

/// What one engine is asked for at a depth, and what that costs.
struct Plan {
    engine: &'static str,
    label: &'static str,
    setting: &'static str,
    /// Exa's hard spending cap for the run.
    cap_usd: Option<f64>,
    low_usd: f64,
    high_usd: f64,
    estimate: String,
}

fn plan(engine: &str, depth: Depth) -> Option<Plan> {
    let p = match (engine, depth) {
        ("tavily", Depth::Deep) => Plan {
            engine: "tavily",
            label: "Tavily Research",
            setting: "pro",
            cap_usd: None,
            low_usd: 15.0 * TAVILY_CREDIT_USD,
            high_usd: 250.0 * TAVILY_CREDIT_USD,
            estimate: "model pro: 15-250 credits (~$0.12-$2.00 at $0.008 a credit; free monthly credits are used first)".into(),
        },
        ("tavily", _) => Plan {
            engine: "tavily",
            label: "Tavily Research",
            setting: "mini",
            cap_usd: None,
            low_usd: 4.0 * TAVILY_CREDIT_USD,
            high_usd: 110.0 * TAVILY_CREDIT_USD,
            estimate: "model mini: 4-110 credits (~$0.03-$0.88 at $0.008 a credit; free monthly credits are used first)".into(),
        },
        ("youcom", d) => {
            let (setting, usd) = match d {
                Depth::Light => ("lite", 0.012),
                Depth::Standard => ("standard", 0.05),
                Depth::Deep => ("deep", 0.10),
            };
            Plan {
                engine: "youcom",
                label: "You.com Research",
                setting,
                cap_usd: None,
                low_usd: usd,
                high_usd: usd,
                estimate: format!("effort {setting}: ${usd:.3} a task (fixed price)"),
            }
        }
        ("parallel", d) => {
            // Per completed run, whatever it reads; failed runs are not billed.
            let (setting, usd) = match d {
                Depth::Light => ("core", 0.025),
                Depth::Standard => ("pro", 0.10),
                Depth::Deep => ("ultra", 0.30),
            };
            Plan {
                engine: "parallel",
                label: "Parallel Task",
                setting,
                cap_usd: None,
                low_usd: usd,
                high_usd: usd,
                estimate: format!("processor {setting}: ${usd:.3} a completed run (fixed price, from the expiring credits)"),
            }
        }
        ("exa", d) => {
            let (setting, cap) = match d {
                Depth::Light => ("low", 0.10),
                Depth::Standard => ("medium", 0.30),
                Depth::Deep => ("high", 1.00),
            };
            Plan {
                engine: "exa",
                label: "Exa agent",
                setting,
                cap_usd: Some(cap),
                low_usd: 0.012,
                high_usd: cap,
                estimate: format!("effort {setting}: $0.012 up to a hard cap of ${cap:.2} (Exa bills the measured cost)"),
            }
        }
        _ => return None,
    };
    Some(p)
}

/// One background task, as kept in the run folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Task {
    engine: String,
    id: String,
    question: String,
    depth: Depth,
    setting: String,
    submitted: u64,
    /// running, completed, failed
    status: String,
    estimate: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    cost_note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    page: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub struct ResearchArgs {
    pub question: String,
    pub engines: Vec<String>,
    pub depth: Depth,
    pub approved: bool,
    pub parent: Option<String>,
    pub collect: Option<String>,
    pub wait: bool,
}

fn key(app: &Arc<AppContext>, engine: &str) -> String {
    crate::providers::find(engine)
        .map(|s| crate::providers::key_for(&app.config, s))
        .unwrap_or_default()
}

fn clip(s: &str, n: usize) -> String {
    crate::providers::thread::clip(&s.split_whitespace().collect::<Vec<_>>().join(" "), n)
}

/// One HTTP call to a research API, its answer kept raw in the run.
async fn call(
    app: &Arc<AppContext>,
    engine: &str,
    method: reqwest::Method,
    url: &str,
    body: Option<Value>,
) -> Result<(u16, Value, String), String> {
    let k = key(app, engine);
    let mut req = app.client.request(method.clone(), url).timeout(Duration::from_secs(30));
    req = match engine {
        "tavily" => req.bearer_auth(k),
        "youcom" => req.header("X-API-Key", k),
        _ => req.header("x-api-key", k),
    };
    let sent = body.as_ref().and_then(|b| serde_json::to_vec(b).ok());
    if let Some(b) = &body {
        req = req.json(b);
    }
    let started = Instant::now();
    let resp = req.send().await.map_err(|e| e.without_url().to_string())?;
    let status = resp.status().as_u16();
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = resp.bytes().await.map_err(|e| e.without_url().to_string())?;
    raw::record_http(engine, method.as_str(), url, sent.as_deref(), status, started.elapsed(), ct.as_deref(), &bytes);
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Ok((status, v, text))
}

/// Start one engine's task: its id.
async fn submit(app: &Arc<AppContext>, p: &Plan, question: &str) -> Result<String, String> {
    let (url, body, id_field) = match p.engine {
        "tavily" => (
            "https://api.tavily.com/research",
            json!({"input": question, "model": p.setting, "stream": false}),
            "/request_id",
        ),
        "youcom" => (
            "https://api.you.com/v1/research",
            json!({"input": question, "research_effort": p.setting, "background": true}),
            "/task_id",
        ),
        "parallel" => (
            "https://api.parallel.ai/v1/tasks/runs",
            json!({"input": question, "processor": p.setting, "task_spec": {"output_schema": {"type": "text"}}}),
            "/run_id",
        ),
        _ => (
            "https://api.exa.ai/agent/runs",
            json!({"query": question, "effort": p.setting, "budget": {"maxCostDollars": p.cap_usd}}),
            "/id",
        ),
    };
    let (status, v, text) = call(app, p.engine, reqwest::Method::POST, url, Some(body)).await?;
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status}: {}", clip(&text, 300)));
    }
    v.pointer(id_field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            crate::tools::contract_warning(&format!("{} research", p.engine), "no task id in the start answer");
            format!("no task id in the answer: {}", clip(&text, 300))
        })
}

enum Poll {
    Running(String),
    Done {
        content: String,
        sources: Vec<(String, String)>,
        cost_usd: Option<f64>,
        cost_note: String,
    },
    Failed {
        error: String,
        cost_usd: Option<f64>,
    },
}

fn content_text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

fn sources_of(list: Option<&Value>) -> Vec<(String, String)> {
    list.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| {
                    let url = s["url"].as_str()?.to_string();
                    Some((s["title"].as_str().unwrap_or("").to_string(), url))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parallel: the run's status, then its result (a markdown report and the
/// pages it cited) once completed.
async fn poll_parallel(app: &Arc<AppContext>, t: &Task) -> Result<Poll, String> {
    let base = format!("https://api.parallel.ai/v1/tasks/runs/{}", t.id);
    let (status, v, text) = call(app, "parallel", reqwest::Method::GET, &base, None).await?;
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status}: {}", clip(&text, 300)));
    }
    match v["status"].as_str().unwrap_or("") {
        "completed" => {}
        "failed" | "cancelled" => {
            return Ok(Poll::Failed {
                error: content_text(v.get("error")),
                cost_usd: None,
            })
        }
        other => return Ok(Poll::Running(other.to_string())),
    }
    let (status, v, text) = call(app, "parallel", reqwest::Method::GET, &format!("{base}/result"), None).await?;
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status}: {}", clip(&text, 300)));
    }
    let mut sources = Vec::new();
    for b in v.pointer("/output/basis").and_then(Value::as_array).into_iter().flatten() {
        for (title, url) in sources_of(b.get("citations")) {
            if !sources.iter().any(|(_, u)| *u == url) {
                sources.push((title, url));
            }
        }
    }
    Ok(Poll::Done {
        content: content_text(v.pointer("/output/content")),
        sources,
        cost_usd: plan("parallel", t.depth).map(|p| p.low_usd),
        cost_note: "Parallel's list price for this processor".into(),
    })
}

async fn poll(app: &Arc<AppContext>, t: &Task) -> Result<Poll, String> {
    if t.engine == "parallel" {
        return poll_parallel(app, t).await;
    }
    let url = match t.engine.as_str() {
        "tavily" => format!("https://api.tavily.com/research/{}?include_usage=true", t.id),
        "youcom" => format!("https://api.you.com/v1/research/{}", t.id),
        _ => format!("https://api.exa.ai/agent/runs/{}", t.id),
    };
    let (status, v, text) = call(app, &t.engine, reqwest::Method::GET, &url, None).await?;
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status}: {}", clip(&text, 300)));
    }
    let state = v["status"].as_str().unwrap_or("").to_string();
    match t.engine.as_str() {
        "tavily" => match state.as_str() {
            "completed" => {
                let credits = v.pointer("/usage/credits").and_then(Value::as_f64);
                Ok(Poll::Done {
                    content: content_text(v.get("content")),
                    sources: sources_of(v.get("sources")),
                    cost_usd: credits.map(|c| c * TAVILY_CREDIT_USD),
                    cost_note: credits
                        .map(|c| format!("{c} credits reported by Tavily"))
                        .unwrap_or_else(|| "Tavily reported no credit use".into()),
                })
            }
            "failed" => Ok(Poll::Failed {
                error: content_text(v.get("error")),
                cost_usd: None,
            }),
            other => Ok(Poll::Running(other.to_string())),
        },
        "youcom" => match state.as_str() {
            "completed" => {
                let out = v.pointer("/result/output");
                let usd = plan("youcom", t.depth).map(|p| p.low_usd);
                Ok(Poll::Done {
                    content: content_text(out.and_then(|o| o.get("content"))),
                    sources: sources_of(out.and_then(|o| o.get("sources"))),
                    cost_usd: usd,
                    cost_note: "You.com's list price for this effort".into(),
                })
            }
            "failed" | "cancelled" => Ok(Poll::Failed {
                error: content_text(v.get("error")),
                cost_usd: None,
            }),
            other => Ok(Poll::Running(other.to_string())),
        },
        _ => {
            let cost = v.pointer("/costDollars/total").and_then(Value::as_f64);
            match state.as_str() {
                "completed" => {
                    let mut sources = Vec::new();
                    for g in v.pointer("/output/grounding").and_then(Value::as_array).into_iter().flatten() {
                        for (title, url) in sources_of(g.get("citations")) {
                            if !sources.iter().any(|(_, u)| *u == url) {
                                sources.push((title, url));
                            }
                        }
                    }
                    let text = content_text(v.pointer("/output/text"));
                    let content = if text.trim().is_empty() {
                        content_text(v.pointer("/output/structured"))
                    } else {
                        text
                    };
                    Ok(Poll::Done {
                        content,
                        sources,
                        cost_usd: cost,
                        cost_note: if cost.is_some() {
                            "measured cost reported by Exa".into()
                        } else {
                            "Exa reported no cost".into()
                        },
                    })
                }
                "failed" | "cancelled" => Ok(Poll::Failed {
                    error: content_text(v.pointer("/error/message")),
                    cost_usd: cost,
                }),
                other => Ok(Poll::Running(other.to_string())),
            }
        }
    }
}

fn load_tasks(run: &Run) -> Vec<Task> {
    std::fs::read_to_string(run.dir.join(TASKS_FILE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn engine_label(engine: &str) -> &'static str {
    match engine {
        "tavily" => "Tavily Research",
        "youcom" => "You.com Research",
        "parallel" => "Parallel Task",
        _ => "Exa agent",
    }
}

/// What an engine calls its depth setting.
fn setting_word(engine: &str) -> &'static str {
    match engine {
        "tavily" => "model",
        "parallel" => "processor",
        _ => "effort",
    }
}

/// Poll every running task once; save finished reports as pages.
async fn collect_once(app: &Arc<AppContext>, run: &mut Run, tasks: &mut [Task]) -> Vec<String> {
    let mut notes = Vec::new();
    for t in tasks.iter_mut().filter(|t| t.status == "running") {
        let rec = Some(run.recorder.labeled(&format!("research {}", t.engine)));
        match raw::scope(rec, poll(app, t)).await {
            Ok(Poll::Running(state)) => notes.push(format!("{} still {state}", t.engine)),
            Ok(Poll::Done {
                content,
                sources,
                cost_usd,
                cost_note,
            }) => {
                let mut md = format!(
                    "# {} report: {}\n\nSynthesized by {} ({} {}): leads to verify, not findings.\n\n{}\n",
                    engine_label(&t.engine),
                    t.question,
                    engine_label(&t.engine),
                    setting_word(&t.engine),
                    t.setting,
                    content.trim()
                );
                if !sources.is_empty() {
                    md.push_str("\n## Sources\n\n");
                    for (title, url) in &sources {
                        md.push_str(&format!("- [{}]({url})\n", if title.is_empty() { url.as_str() } else { title.as_str() }));
                    }
                }
                let id = format!("research://{}/{}", t.engine, t.id);
                let f = crate::fetch::Fetched {
                    stamp: None,
                    note: None,
                    published: None,
                    url: id.clone(),
                    final_url: id,
                    title: format!("{} report: {}", engine_label(&t.engine), clip(&t.question, 70)),
                    markdown: md,
                    ok: true,
                    tier: 0,
                    fetcher: format!("{}-research", t.engine),
                    failure: None,
                    message: String::new(),
                    attempts: Vec::new(),
                    loss: None,
                    cost_micros: cost_usd.map_or(0, |c| (c * 1e6) as u64),
                };
                let entry = crate::research::save(run, f, "research");
                t.page = Some(entry.id.clone());
                t.status = "completed".into();
                t.cost_usd = cost_usd;
                t.cost_note = cost_note;
                if let Some(c) = cost_usd {
                    crate::logging::log_spend(&t.engine, (c * 1e6) as u64, &t.question, "research_task");
                }
                notes.push(format!(
                    "{} done: {} ({} sources, {})",
                    t.engine,
                    entry.id,
                    sources.len(),
                    cost_usd.map(|c| format!("${c:.4}")).unwrap_or_else(|| "cost unknown".into())
                ));
            }
            Ok(Poll::Failed { error, cost_usd }) => {
                t.status = "failed".into();
                t.error = Some(clip(&error, 300));
                t.cost_usd = cost_usd;
                if let Some(c) = cost_usd {
                    crate::logging::log_spend(&t.engine, (c * 1e6) as u64, &t.question, "research_task");
                }
                notes.push(format!("{} failed: {}", t.engine, clip(&error, 200)));
            }
            Err(e) => notes.push(format!("{} status check failed (will retry on the next collect): {e}", t.engine)),
        }
    }
    notes
}

fn render(run: &Run, tasks: &[Task], notes: &[String]) -> String {
    let mut out = format!(
        "# OmniSearch research tasks, run {}\n\nfolder: {}\n\n",
        run.id,
        run.dir.display()
    );
    for t in tasks {
        let cost = t
            .cost_usd
            .map(|c| format!("${c:.4} ({})", t.cost_note))
            .unwrap_or_else(|| format!("estimate: {}", t.estimate));
        let state = match (t.status.as_str(), &t.page, &t.error) {
            ("completed", Some(p), _) => format!("completed -> {p}"),
            ("failed", _, Some(e)) => format!("failed: {e}"),
            (s, _, _) => s.to_string(),
        };
        out.push_str(&format!("- {} ({} {}): {state} · {cost} · task {}\n", t.engine, setting_word(&t.engine), t.setting, t.id));
    }
    for n in notes {
        out.push_str(&format!("- {n}\n"));
    }
    if tasks.iter().any(|t| t.status == "running") {
        out.push_str(&format!(
            "\nStill running. Collect later: `osearch research --collect {}` (reports take from seconds to several minutes).\n",
            run.id
        ));
    }
    if tasks.iter().any(|t| t.page.is_some()) {
        out.push_str("\nRead a report: `osearch read pN` (outline) · `osearch read pN#K`. Engine reports are synthesized by the engine: leads to verify, not findings.\n");
    }
    out
}

fn save_state(run: &Run, tasks: &[Task], summary: &str) -> Result<(), SearchError> {
    run.write_json(TASKS_FILE, &tasks)?;
    run.write_text("research-tasks.md", summary)
}

pub async fn research(app: Arc<AppContext>, args: ResearchArgs) -> Result<crate::research::Outcome, SearchError> {
    // Picking up tasks started earlier.
    if let Some(which) = args.collect.as_deref() {
        let mut run = Run::follow(which)?;
        let mut tasks = load_tasks(&run);
        if tasks.is_empty() {
            return Err(SearchError::InvalidInput {
                message: format!("run {} has no research tasks", run.id),
            });
        }
        let notes = wait_loop(&app, &mut run, &mut tasks, args.wait).await;
        let summary = render(&run, &tasks, &notes);
        save_state(&run, &tasks, &summary)?;
        return Ok(crate::research::Outcome {
            summary,
            run_dir: run.dir.clone(),
        });
    }

    let question = args.question.trim().to_string();
    if question.is_empty() {
        return Err(SearchError::InvalidInput {
            message: "give a research question, or --collect <run>".into(),
        });
    }
    let wanted: Vec<String> = if args.engines.is_empty() {
        ENGINES.iter().map(|e| e.to_string()).collect()
    } else {
        args.engines.iter().map(|e| e.to_lowercase()).collect()
    };
    let mut plans = Vec::new();
    let mut skipped = Vec::new();
    for e in &wanted {
        match plan(e, args.depth) {
            None => skipped.push(format!("{e}: not a research engine ({})", ENGINES.join(", "))),
            Some(_) if key(&app, e).is_empty() => skipped.push(format!("{e}: no API key")),
            Some(p) => plans.push(p),
        }
    }
    if plans.is_empty() {
        return Err(SearchError::InvalidInput {
            message: format!("no research engine available: {}", skipped.join("; ")),
        });
    }
    let low: f64 = plans.iter().map(|p| p.low_usd).sum();
    let high: f64 = plans.iter().map(|p| p.high_usd).sum();
    let mut estimate = format!(
        "Cost estimate for \"{}\" (depth {}):\n",
        clip(&question, 120),
        format!("{:?}", args.depth).to_lowercase()
    );
    for p in &plans {
        estimate.push_str(&format!("- {:<7} {}, {}\n", p.engine, p.label, p.estimate));
    }
    for s in &skipped {
        estimate.push_str(&format!("- skipped: {s}\n"));
    }
    estimate.push_str(&format!("Total: about ${low:.2} to ${high:.2}.\n"));

    if !args.approved {
        let summary = format!(
            "Research mode was NOT run: it costs far more than a search and needs the user's approval first.\n\n{estimate}\nShow the user this estimate and ask. If they agree, run the same command with --approved.\n"
        );
        return Ok(crate::research::Outcome {
            summary,
            run_dir: crate::run::runs_root(),
        });
    }

    let mut run = match args.parent.as_deref() {
        Some(p) => Run::follow(p)?,
        None => Run::create(&format!("research {question}"))?,
    };
    run.write_json(
        &format!("request.r{}.json", run.round),
        &json!({"research": question, "engines": wanted, "depth": args.depth, "approved": true}),
    )?;
    let mut tasks = load_tasks(&run);
    let mut notes: Vec<String> = skipped.iter().map(|s| format!("skipped {s}")).collect();
    for p in &plans {
        let rec = Some(run.recorder.labeled(&format!("research {}", p.engine)));
        match raw::scope(rec, submit(&app, p, &question)).await {
            Ok(id) => tasks.push(Task {
                engine: p.engine.to_string(),
                id,
                question: question.clone(),
                depth: args.depth,
                setting: p.setting.to_string(),
                submitted: raw::now(),
                status: "running".into(),
                estimate: p.estimate.clone(),
                cost_usd: None,
                cost_note: String::new(),
                page: None,
                error: None,
            }),
            Err(e) => notes.push(format!("{} did not start: {e}", p.engine)),
        }
    }
    if crate::config::get().jinadeepsearch.modes.iter().any(|m| m == "research") {
        match crate::jinadeep::start(&run, &question) {
            Ok(()) => {
                let page = run.reserve(1).remove(0);
                crate::jinadeep::assign(&run, &page);
                notes.push(format!("Jina DeepSearch started beside them; `osearch wait {} --jina-deepsearch` prints it ({page})", run.id));
            }
            Err(e) => notes.push(format!("Jina DeepSearch did not start: {e}")),
        }
    }
    // Saved before any waiting, so a stopped command loses no task id.
    let started = render(&run, &tasks, &notes);
    save_state(&run, &tasks, &started)?;
    run.index(&RunLine {
        id: run.id.clone(),
        round: run.round,
        ts: raw::now(),
        topic: format!("research tasks: {question}"),
        queries: vec![question.clone()],
        urls: 0,
        pages: 0,
        cost_usd: 0.0,
        kind: "research-tasks".into(),
    });
    if args.wait {
        notes.extend(wait_loop(&app, &mut run, &mut tasks, true).await);
    }
    let summary = format!("{}\n{estimate}", render(&run, &tasks, &notes));
    save_state(&run, &tasks, &summary)?;
    Ok(crate::research::Outcome {
        summary,
        run_dir: run.dir.clone(),
    })
}

/// Collect once, or until every task ends (up to 15 minutes) with `wait`.
async fn wait_loop(app: &Arc<AppContext>, run: &mut Run, tasks: &mut [Task], wait: bool) -> Vec<String> {
    let started = Instant::now();
    loop {
        let notes = collect_once(app, run, tasks).await;
        let running = tasks.iter().filter(|t| t.status == "running").count();
        if !wait || running == 0 || started.elapsed() > WAIT_LIMIT {
            return notes;
        }
        eprintln!("[{:>4.0} s] {running} research task(s) still running", started.elapsed().as_secs_f64());
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}
