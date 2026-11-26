//! Numbers across every run: what each engine found, found alone and got
//! read, how each fetcher did, and how long runs took. Each run's numbers
//! are worked out once and kept in `<data>/ui/insights.json`, redone when
//! the run's files change.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EngineNums {
    pub calls: u64,
    pub failed: u64,
    pub ms: u64,
    pub links: u64,
    /// Links no other engine returned in the run.
    pub alone: u64,
    /// Links of this engine that were picked to read.
    pub picked: u64,
    /// Of those, read successfully.
    pub read: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FetcherNums {
    pub tries: u64,
    pub ok: u64,
    pub ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunNums {
    pub id: String,
    /// Changes when the run's files change.
    pub mark: u64,
    pub modes: Vec<String>,
    pub questions: u64,
    pub links: u64,
    pub pages_ok: u64,
    pub pages_failed: u64,
    pub tokens: u64,
    pub seconds: u64,
    pub engines: BTreeMap<String, EngineNums>,
    pub fetchers: BTreeMap<String, FetcherNums>,
    /// Picks by reason (question, side, solo, rank, spare, url).
    pub why: BTreeMap<String, u64>,
    pub synthesis: bool,
}

#[derive(Deserialize)]
struct Row {
    #[serde(default)]
    engines: Vec<String>,
    #[serde(default)]
    page: Option<String>,
}

#[derive(Deserialize)]
struct Call {
    #[serde(default)]
    provider: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    status: Option<i32>,
    #[serde(default)]
    elapsed_ms: u64,
    #[serde(default)]
    ts: u64,
}

pub struct Cache {
    runs: Mutex<BTreeMap<String, RunNums>>,
}

fn cache_file() -> PathBuf {
    crate::config::data_dir().join("ui").join("insights.json")
}

/// Last-modified seconds of the files a run's numbers come from.
fn mark(dir: &Path) -> u64 {
    ["results.jsonl", "calls.jsonl", "pages/index.jsonl", "synthesis.md"]
        .iter()
        .filter_map(|f| std::fs::metadata(dir.join(f)).ok()?.modified().ok())
        .filter_map(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .max()
        .unwrap_or(0)
}

fn lines<T: for<'de> Deserialize<'de>>(path: &Path) -> Vec<T> {
    crate::run::read_text(path)
        .map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
        .unwrap_or_default()
}

fn work_out(id: &str, dir: &Path) -> RunNums {
    let mut n = RunNums { id: id.to_string(), mark: mark(dir), ..RunNums::default() };
    let pages = crate::run::load_pages(dir);
    let ok: HashSet<&str> = pages.iter().filter(|p| p.ok).map(|p| p.id.as_str()).collect();
    for p in &pages {
        if p.ok {
            n.pages_ok += 1;
            n.tokens += p.tokens as u64;
        } else {
            n.pages_failed += 1;
        }
        for a in &p.attempts {
            let fetcher = a["fetcher"].as_str().unwrap_or("?").to_string();
            let f = n.fetchers.entry(fetcher).or_default();
            f.tries += 1;
            f.ms += a["ms"].as_u64().unwrap_or(0);
            if a["outcome"].as_str().is_some_and(|o| o.starts_with("ok")) {
                f.ok += 1;
            }
        }
    }
    for r in lines::<Row>(&dir.join("results.jsonl")) {
        n.links += 1;
        let alone = r.engines.len() == 1;
        for e in &r.engines {
            let en = n.engines.entry(e.clone()).or_default();
            en.links += 1;
            if alone {
                en.alone += 1;
            }
            if let Some(p) = &r.page {
                en.picked += 1;
                if ok.contains(p.as_str()) {
                    en.read += 1;
                }
            }
        }
    }
    let calls = lines::<Call>(&dir.join("calls.jsonl"));
    let (mut first, mut last) = (u64::MAX, 0u64);
    for c in &calls {
        first = first.min(c.ts.saturating_sub(c.elapsed_ms / 1000));
        last = last.max(c.ts);
        if c.label.starts_with("fetch") {
            continue;
        }
        let en = n.engines.entry(c.provider.clone()).or_default();
        en.calls += 1;
        en.ms += c.elapsed_ms;
        if !c.status.is_some_and(|s| (200..300).contains(&s)) {
            en.failed += 1;
        }
    }
    if last >= first && first != u64::MAX {
        n.seconds = last - first;
    }
    for round in 1.. {
        let req = dir.join(format!("request.r{round}.json"));
        let Ok(t) = std::fs::read_to_string(&req) else { break };
        let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
        for q in v["queries"].as_array().into_iter().flatten() {
            n.questions += 1;
            let mode = q["mode"].as_str().unwrap_or("general").to_string();
            if !n.modes.contains(&mode) {
                n.modes.push(mode);
            }
        }
        let picks: Value = std::fs::read_to_string(dir.join(format!("picks.r{round}.json")))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        for p in picks.as_array().into_iter().flatten() {
            *n.why.entry(p["why"].as_str().unwrap_or("?").to_string()).or_default() += 1;
        }
    }
    n.synthesis = dir.join("synthesis.md").is_file();
    n
}

impl Cache {
    pub fn load() -> Self {
        let runs = std::fs::read_to_string(cache_file())
            .ok()
            .and_then(|t| serde_json::from_str::<Vec<RunNums>>(&t).ok())
            .map(|v| v.into_iter().map(|r| (r.id.clone(), r)).collect())
            .unwrap_or_default();
        Self { runs: Mutex::new(runs) }
    }

    /// Every run's numbers (worked out where missing or changed) and the totals.
    pub fn all(&self) -> Value {
        let root = crate::run::runs_root();
        let mut ids: Vec<String> = std::fs::read_dir(&root)
            .map(|d| d.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        ids.sort();
        let mut changed = false;
        let mut runs = self.runs.lock().expect("insights");
        for id in &ids {
            let dir = root.join(id);
            let fresh = runs.get(id).is_some_and(|r| r.mark == mark(&dir));
            if !fresh {
                runs.insert(id.clone(), work_out(id, &dir));
                changed = true;
            }
        }
        runs.retain(|id, _| ids.contains(id));
        if changed {
            let all: Vec<&RunNums> = runs.values().collect();
            if let Ok(text) = serde_json::to_string(&all) {
                let _ = std::fs::create_dir_all(cache_file().parent().expect("ui folder"));
                let _ = std::fs::write(cache_file(), text);
            }
        }
        let mut engines: BTreeMap<String, EngineNums> = BTreeMap::new();
        let mut fetchers: BTreeMap<String, FetcherNums> = BTreeMap::new();
        let mut why: BTreeMap<String, u64> = BTreeMap::new();
        for r in runs.values() {
            for (k, e) in &r.engines {
                let t = engines.entry(k.clone()).or_default();
                t.calls += e.calls;
                t.failed += e.failed;
                t.ms += e.ms;
                t.links += e.links;
                t.alone += e.alone;
                t.picked += e.picked;
                t.read += e.read;
            }
            for (k, f) in &r.fetchers {
                let t = fetchers.entry(k.clone()).or_default();
                t.tries += f.tries;
                t.ok += f.ok;
                t.ms += f.ms;
            }
            for (k, c) in &r.why {
                *why.entry(k.clone()).or_default() += c;
            }
        }
        json!({
            "runs": runs.values().collect::<Vec<_>>(),
            "engines": engines,
            "fetchers": fetchers,
            "why": why,
        })
    }
}
