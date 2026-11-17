//! Jina DeepSearch beside a deep run: Jina searches, reads and reasons on its
//! own, in a separate process that nothing waits for. Its answer lands as a
//! page of the run, the pages it visited in the results index.

use crate::errors::SearchError;
use crate::research::{JinaAsk, Planned, Request};
use crate::run::Run;
use crate::types::Mode;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{Duration, Instant};

pub const RUNNING: &str = "jina-deepsearch.running";
const FAILED: &str = "jina-deepsearch.failed";
const URL: &str = "https://deepsearch.jina.ai/v1/chat/completions";
const FETCHER: &str = "jina-deepsearch";

#[derive(Serialize, Deserialize, Default)]
struct Job {
    question: String,
    #[serde(default)]
    page: Option<String>,
}

/// The question a run's Jina DeepSearch is asked, if it gets one: the
/// request's own question, else its first deep query.
pub fn question(req: &Request, planned: &[Planned]) -> Option<String> {
    let own: Vec<&Planned> = planned.iter().filter(|p| !p.side).collect();
    let first_deep = own.iter().find(|p| p.mode == Mode::Deep).map(|p| p.query.clone());
    match &req.jina_deepsearch {
        Some(JinaAsk::On(false)) => None,
        Some(JinaAsk::Question(q)) if !q.trim().is_empty() => Some(q.trim().to_string()),
        Some(JinaAsk::On(true)) => first_deep.or_else(|| own.first().map(|p| p.query.clone())),
        _ => crate::config::get().jinadeepsearch.modes.iter().any(|m| m == "deep").then_some(first_deep).flatten(),
    }
}

/// Start the job in its own process. Its page number comes later
/// (`assign`), once the run has numbered its own pages.
pub fn start(run: &Run, question: &str) -> Result<(), String> {
    if crate::providers::key_for(crate::config::get(), &crate::providers::jina::SPEC).is_empty() {
        return Err("no Jina key".into());
    }
    // One at a time per run: a later round starts its own once the earlier one landed.
    if running(&run.dir) {
        return Err(format!(
            "an earlier round's Jina DeepSearch is still running; `osearch wait {} --jina-deepsearch` prints it",
            run.id
        ));
    }
    let job = Job { question: question.to_string(), page: None };
    let text = serde_json::to_string(&job).map_err(|e| e.to_string())?;
    std::fs::write(run.dir.join(RUNNING), text).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(run.dir.join(FAILED));
    crate::synth::spawn_self(&["jina-deepsearch", "--job", &run.id]).inspect_err(|_| {
        let _ = std::fs::remove_file(run.dir.join(RUNNING));
    })
}

/// A job is running: its marker exists and is younger than the job's own
/// time limit (an older one was left by a killed job).
fn running(dir: &Path) -> bool {
    let limit = Duration::from_secs(crate::config::get().jinadeepsearch.stop + 60);
    std::fs::metadata(dir.join(RUNNING))
        .and_then(|m| m.modified())
        .is_ok_and(|t| t.elapsed().unwrap_or_default() < limit)
}

/// Give the job its page number; it waits for one before saving.
pub fn assign(run: &Run, page: &str) {
    let path = run.dir.join(RUNNING);
    if let Some(mut job) = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Job>(&t).ok()) {
        job.page = Some(page.to_string());
        let _ = std::fs::write(&path, serde_json::to_string(&job).unwrap_or_default());
    }
}

/// The summary's section for a started job.
pub fn summary(id: &str, question: &str, page: &str) -> String {
    format!(
        "\n## Jina DeepSearch\n\nRunning in the background on \"{}\" (usually 3-4 minutes). Start `osearch wait {id} --jina-deepsearch` as a background command to be told when it lands (it waits up to {} minutes; the job itself keeps going). Its answer becomes {page}, and the pages it visited join results.jsonl.\n",
        crate::providers::thread::clip(question, 160),
        crate::config::get().jinadeepsearch.wait.div_ceil(60)
    )
}

/// `osearch jina-deepsearch --job RUN`: the background process.
pub async fn job(run_id: &str) -> Result<i32, SearchError> {
    let mut run = Run::attach(run_id)?;
    let job: Job = serde_json::from_str(&std::fs::read_to_string(run.dir.join(RUNNING))?)?;
    let result = ask(&run, &job.question).await;
    let code = match result {
        Ok(answer) => {
            let page = page_number(&mut run).await;
            save(&mut run, &job.question, &page, &answer);
            0
        }
        Err(e) => {
            let _ = std::fs::write(run.dir.join(FAILED), &e);
            1
        }
    };
    let _ = std::fs::remove_file(run.dir.join(RUNNING));
    Ok(code)
}

struct Answer {
    content: String,
    quotes: Vec<(String, String, String)>,
    visited: Vec<String>,
    read: Vec<String>,
    tokens: u64,
    seconds: u64,
}

async fn ask(run: &Run, question: &str) -> Result<Answer, String> {
    let s = crate::config::get().jinadeepsearch.clone();
    let key = crate::providers::key_for(crate::config::get(), &crate::providers::jina::SPEC);
    // Its own client: the shared one gives up after 90 s without data.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(s.stop))
        .build()
        .map_err(|e| e.to_string())?;
    let body = serde_json::json!({
        "model": "jina-deepsearch-v1",
        "messages": [{ "role": "user", "content": question }],
        "stream": false,
        "reasoning_effort": s.effort,
        "budget_tokens": s.budget,
    });
    let sent = body.to_string();
    let started = Instant::now();
    let resp = client
        .post(URL)
        .bearer_auth(&key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Jina DeepSearch: {}", e.without_url()))?;
    let status = resp.status().as_u16();
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let elapsed = started.elapsed();
    crate::run::raw::scope(Some(run.recorder.labeled("jds")), async {
        crate::run::raw::record_http(FETCHER, "POST", URL, Some(sent.as_bytes()), status, elapsed, Some("application/json"), &bytes);
    })
    .await;
    if status >= 400 {
        return Err(format!("Jina DeepSearch: HTTP {status}: {}", crate::providers::thread::clip(&String::from_utf8_lossy(&bytes), 300)));
    }
    let v: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| format!("Jina DeepSearch answer: {e}"))?;
    let msg = &v["choices"][0]["message"];
    // Its markdown carries HTML code tags.
    let content = msg["content"].as_str().unwrap_or("").trim().replace("<code>", "`").replace("</code>", "`");
    if content.is_empty() {
        return Err("Jina DeepSearch returned no answer".into());
    }
    let quotes = msg["annotations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let c = &a["url_citation"];
            Some((c["exactQuote"].as_str()?.to_string(), c["title"].as_str().unwrap_or("").to_string(), c["url"].as_str()?.to_string()))
        })
        .collect();
    let list = |k: &str| -> Vec<String> { v[k].as_array().into_iter().flatten().filter_map(|u| u.as_str().map(str::to_string)).collect() };
    let tokens = v["usage"]["total_tokens"].as_u64().unwrap_or(0);
    crate::logging::log_spend("jina", tokens.div_ceil(20), question, FETCHER);
    Ok(Answer { content, quotes, visited: list("visitedURLs"), read: list("readURLs"), tokens, seconds: elapsed.as_secs() })
}

/// The page number the run gave the job, else (the run never assigned one)
/// the next free one.
async fn page_number(run: &mut Run) -> String {
    let started = Instant::now();
    loop {
        let assigned = std::fs::read_to_string(run.dir.join(RUNNING))
            .ok()
            .and_then(|t| serde_json::from_str::<Job>(&t).ok())
            .and_then(|j| j.page);
        if let Some(p) = assigned {
            return p;
        }
        if started.elapsed() > Duration::from_secs(120) {
            // The run never gave one: number it after the pages saved by now.
            if let Ok(fresh) = Run::attach(&run.id) {
                *run = fresh;
            }
            return run.reserve(1).remove(0);
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

fn save(run: &mut Run, question: &str, page: &str, a: &Answer) {
    let s = &crate::config::get().jinadeepsearch;
    let mut md = format!(
        "# Jina DeepSearch answer\n\nQuestion: {question}\n\nJina DeepSearch (jina-deepsearch-v1, effort {}): {} s, {} tokens, visited {} pages, read {}. Another model's answer: check its key claims against the pages.\n\n## Answer\n\n{}\n",
        s.effort,
        a.seconds,
        a.tokens,
        a.visited.len(),
        a.read.len(),
        a.content
    );
    if !a.quotes.is_empty() {
        md.push_str("\n## Quotes it cites\n\n");
        for (q, title, url) in &a.quotes {
            md.push_str(&format!("- \"{q}\" - [{}]({url})\n", if title.is_empty() { url } else { title }));
        }
    }
    if !a.read.is_empty() {
        md.push_str("\n## Pages it read\n\n");
        for u in &a.read {
            md.push_str(&format!("- {u}\n"));
        }
    }
    let f = crate::fetch::Fetched {
        stamp: None,
        note: None,
        published: None,
        url: "https://deepsearch.jina.ai/".into(),
        final_url: "https://deepsearch.jina.ai/".into(),
        title: "Jina DeepSearch answer".into(),
        markdown: md,
        ok: true,
        tier: 0,
        fetcher: FETCHER.into(),
        failure: None,
        message: String::new(),
        attempts: Vec::new(),
        loss: None,
        cost_micros: a.tokens.div_ceil(20),
    };
    crate::research::save_as(run, f, FETCHER, page, true);
    add_results(run, a);
}

/// The pages it visited, as rows of the results index (new ones only).
fn add_results(run: &Run, a: &Answer) {
    let text = std::fs::read_to_string(run.dir.join("results.jsonl")).unwrap_or_default();
    let mut known = std::collections::HashSet::new();
    let mut rank = 0u64;
    for v in text.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()) {
        if let Some(u) = v["url"].as_str() {
            known.insert(crate::engine::normalize_url(u));
        }
        rank = rank.max(v["rank"].as_u64().unwrap_or(0));
    }
    let read: std::collections::HashSet<&str> = a.read.iter().map(String::as_str).collect();
    let ordered = a.read.iter().chain(a.visited.iter().filter(|u| !read.contains(u.as_str())));
    for (i, u) in ordered.enumerate() {
        if !known.insert(crate::engine::normalize_url(u)) {
            continue;
        }
        rank += 1;
        let how = if read.contains(u.as_str()) { "read by Jina DeepSearch" } else { "visited by Jina DeepSearch" };
        run.append_jsonl(
            "results.jsonl",
            &serde_json::json!({
                "rank": rank,
                "url": u,
                "title": "",
                "score": 0.0,
                "engines": [FETCHER],
                "queries": ["jds"],
                "found_by": [{ "q": "jds", "engine": FETCHER, "rank": i + 1, "title": "", "snippet": how }],
            }),
        );
    }
}

/// `osearch wait RUN --jina-deepsearch`: wait for the job, then print its answer.
pub async fn wait(dir: &Path) -> String {
    let id = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let limit = Duration::from_secs(crate::config::get().jinadeepsearch.wait);
    let started = Instant::now();
    while running(dir) && started.elapsed() < limit {
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let mut out = format!("run {id}\n");
    if !running(dir) && dir.join(RUNNING).exists() {
        out.push_str("Jina DeepSearch stopped without an answer (its job ended early).\n");
        return out;
    }
    if running(dir) {
        out.push_str(&format!(
            "Jina DeepSearch is still running after {} minutes; it keeps going and lands in this run. Run `osearch wait {id} --jina-deepsearch` again later.\n",
            limit.as_secs().div_ceil(60)
        ));
        return out;
    }
    if let Ok(e) = std::fs::read_to_string(dir.join(FAILED)) {
        out.push_str(&format!("Jina DeepSearch failed: {e}\n"));
        return out;
    }
    let pages = crate::run::load_pages(dir);
    match pages.iter().rev().find(|p| p.fetcher == FETCHER && p.ok) {
        Some(p) => {
            let text = std::fs::read_to_string(dir.join(format!("pages/{}.md", p.id))).unwrap_or_default();
            out.push_str(&format!("{} Jina DeepSearch answer ({} tok)\n\n{text}\n", p.id, p.tokens));
        }
        None => out.push_str("no Jina DeepSearch in this run\n"),
    }
    out
}
