//! What each site does with each fetcher, kept forever in `sites.json`
//! (data folder): attempts, successes and failure kinds, plus the recent
//! outcomes decisions are made from. A decision needs repeated failures on
//! at least two days, never one bad run; and nothing is ever dropped
//! because of it: a page from a hard site only starts later in the chain or
//! goes straight to the background.

use super::Fetched;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;

const RECENT: usize = 6;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Outcome {
    pub ok: bool,
    pub day: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Rec {
    pub tries: u64,
    pub ok: u64,
    pub walls: u64,
    pub timeouts: u64,
    pub other: u64,
    /// Milliseconds over all successes.
    pub ok_ms: u64,
    #[serde(default)]
    pub last_ok: String,
    #[serde(default)]
    pub recent: Vec<Outcome>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Site {
    #[serde(default)]
    pub fetchers: BTreeMap<String, Rec>,
    /// Whole pages: fetched by any fetcher or not.
    #[serde(default)]
    pub pages: Vec<Outcome>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub sites: BTreeMap<String, Site>,
}

static LOCK: Mutex<()> = Mutex::new(());

fn path() -> std::path::PathBuf {
    crate::config::data_dir().join("sites.json")
}

pub fn load() -> Store {
    std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(s: &Store) {
    let p = path();
    let tmp = p.with_extension("json.tmp");
    if let Ok(text) = serde_json::to_string(s) {
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

/// The site a URL belongs to: its host without `www.`.
pub fn site_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_ascii_lowercase()))
        .unwrap_or_default()
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

fn push(list: &mut Vec<Outcome>, ok: bool, cap: usize) {
    list.push(Outcome { ok, day: today() });
    if list.len() > cap {
        list.remove(0);
    }
}

/// Record one finished fetch: every fetcher it tried, and the page's outcome.
/// A page cut off mid-fetch (no final outcome) records only its attempts.
pub fn record(f: &Fetched, finished: bool) {
    let site = site_of(&f.url);
    if site.is_empty() || f.attempts.is_empty() {
        return;
    }
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = load();
    let s = store.sites.entry(site).or_default();
    for a in &f.attempts {
        let r = s.fetchers.entry(a.fetcher.clone()).or_default();
        r.tries += 1;
        let ok = a.outcome.starts_with("ok");
        if ok {
            r.ok += 1;
            r.ok_ms += a.ms as u64;
            r.last_ok = today();
        } else if a.outcome.starts_with("bot_wall") {
            r.walls += 1;
        } else if a.outcome.contains("timed out") || a.outcome.contains("timeout") {
            r.timeouts += 1;
        } else {
            r.other += 1;
        }
        push(&mut r.recent, ok, RECENT);
    }
    if finished {
        push(&mut s.pages, f.ok, 10);
    }
    save(&store);
}

fn days(list: &[Outcome]) -> usize {
    let mut d: Vec<&str> = list.iter().map(|o| o.day.as_str()).collect();
    d.sort_unstable();
    d.dedup();
    d.len()
}

/// Repeated failures: at least three recent tries, none good, on two days or more.
fn failing(list: &[Outcome]) -> bool {
    list.len() >= 3 && !list.iter().any(|o| o.ok) && days(list) >= 2
}

/// Fetchers to start after for this site: those that keep failing there,
/// when another fetcher has worked for it recently.
pub fn skip_for(url: &str) -> Vec<String> {
    let store = load();
    let Some(s) = store.sites.get(&site_of(url)) else { return Vec::new() };
    let something_works = s.fetchers.values().any(|r| r.recent.iter().any(|o| o.ok));
    if !something_works {
        return Vec::new();
    }
    s.fetchers.iter().filter(|(_, r)| failing(&r.recent)).map(|(n, _)| n.clone()).collect()
}

/// This site's pages have kept failing at every fetcher: its pages go to
/// the background at once rather than hold up a run.
pub fn walls_us(url: &str) -> bool {
    load().sites.get(&site_of(url)).is_some_and(|s| failing(&s.pages))
}

/// Sites whose pages fail even at the paid readers, with their page tries.
pub fn hard_sites() -> Vec<(String, usize)> {
    let paid = ["jina", "firecrawl"];
    let mut out: Vec<(String, usize)> = load()
        .sites
        .into_iter()
        .filter(|(_, s)| {
            failing(&s.pages) && paid.iter().any(|p| s.fetchers.get(*p).is_some_and(|r| r.tries > 0 && r.ok == 0))
        })
        .map(|(h, s)| (h, s.pages.len()))
        .collect();
    out.sort();
    out
}
