//! `osearch replay RUN`: a run's saved results ranked again with the current
//! ranking (Jina's reranker blended in), beside the ranking it had, with no
//! engine calls. For judging a ranking change on past runs.

use crate::context::AppContext;
use crate::errors::SearchError;
use crate::providers::thread::clip;
use serde_json::Value;
use std::fmt::Write as _;

/// Rows sent to the reranker, as in a run.
const RERANK: usize = 100;

pub async fn command(app: &AppContext, which: Option<String>, top: usize) -> Result<i32, SearchError> {
    let dir = crate::run::resolve(which.as_deref().unwrap_or("last"))?;
    let id = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let rows: Vec<Value> = crate::run::read_text(&dir.join("results.jsonl"))?
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .filter(|v: &Value| v["engines"].as_array().is_none_or(|e| !e.iter().any(|x| x == "jina-deepsearch")))
        .collect();
    // Several rounds: the last one only (each round's rows restart at rank 1).
    let start = rows.iter().rposition(|r| r["rank"].as_u64() == Some(1)).unwrap_or(0);
    let rows: Vec<Value> = rows.into_iter().skip(start).collect();
    if rows.is_empty() {
        return Err(SearchError::InvalidInput { message: format!("run {id} has no results to replay") });
    }
    let question = question(&dir);
    let n = rows.len().min(RERANK);
    let docs: Vec<String> = rows[..n].iter().map(row_text).collect();
    let rel = crate::rerank::scores(app, &question, &docs).await.map_err(SearchError::Config)?;
    let best = rows.iter().filter_map(|r| r["score"].as_f64()).fold(0.0, f64::max);
    let mut now: Vec<(usize, f64)> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| (i, crate::rerank::blend(r["score"].as_f64().unwrap_or(0.0), best, rel.get(i).copied().unwrap_or(0.0))))
        .collect();
    now.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let fetched: Vec<usize> = (0..rows.len()).filter(|&i| rows[i]["page"].is_string()).collect();
    let k = fetched.len().max(1);

    let mut out = format!("run {id}\nquestion: {question}\nthen = the run's ranking · now = with Jina's reranker blended in (research.rerank)\n\n");
    out.push_str(" now then page  rel   title · engines\n");
    for (pos, (i, _)) in now.iter().take(top).enumerate() {
        let r = &rows[*i];
        let engines: Vec<&str> = r["engines"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        let _ = writeln!(
            out,
            "{:>4} {:>4} {:<5} {:.2}  {} · {}",
            pos + 1,
            i + 1,
            r["page"].as_str().unwrap_or(""),
            rel.get(*i).copied().unwrap_or(0.0),
            clip(r["title"].as_str().unwrap_or(""), 70),
            engines.join(", ")
        );
    }
    let top_now: Vec<usize> = now.iter().take(k).map(|(i, _)| *i).collect();
    let into: Vec<String> = top_now.iter().filter(|i| !fetched.contains(i)).map(|&i| line(&rows[i], i)).collect();
    let out_of: Vec<String> = fetched.iter().filter(|i| !top_now.contains(i)).map(|&i| line(&rows[i], i)).collect();
    let _ = writeln!(out, "\nThe run fetched {} pages. In the top {k} now but not fetched then:", fetched.len());
    for l in &into {
        let _ = writeln!(out, "+ {l}");
    }
    let _ = writeln!(out, "Fetched then but outside the top {k} now:");
    for l in &out_of {
        let _ = writeln!(out, "- {l}");
    }
    out.push_str("A run picks pages per query first, so the top list is a guide to what would change, not the exact pick.\n");
    print!("{out}");
    Ok(0)
}

fn row_text(r: &Value) -> String {
    let snippet = r["found_by"][0]["snippet"].as_str().unwrap_or("");
    format!("{} {}", r["title"].as_str().unwrap_or(""), clip(snippet, 400))
}

fn line(r: &Value, i: usize) -> String {
    format!("#{} {} {}", i + 1, clip(r["title"].as_str().unwrap_or(""), 70), r["url"].as_str().unwrap_or(""))
}

/// The last round's topic, else its queries.
fn question(dir: &std::path::Path) -> String {
    let latest = (1..100).rev().map(|n| dir.join(format!("request.r{n}.json"))).find(|p| p.exists());
    let req: Value = latest
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    if let Some(t) = req["topic"].as_str().filter(|t| !t.trim().is_empty()) {
        return t.to_string();
    }
    req["queries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|q| q.as_str().or(q["q"].as_str()))
        .collect::<Vec<_>>()
        .join(" / ")
}
