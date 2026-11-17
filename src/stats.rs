//! `osearch stats` — local analytics over the JSONL search logs: volume,
//! modes, per-provider calls/failures, estimated spend, cache-hit rate, and
//! an extract-click-through table (which providers' results actually get
//! read). Everything is computed from disk; nothing leaves the machine.

use crate::logging;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Flat per-call estimates in USD (2026-07), for log lines written before
/// searches recorded `cost` (0.10). Newer lines carry their own per-provider
/// cost. `search usage` balance snapshots are ground truth where available.
fn legacy_cost_per_call(provider: &str) -> f64 {
    match provider {
        "serper" => 0.001,
        "jina" => 0.0005,
        "firecrawl" => 0.002,
        "brave" => 0.005,
        "parallel" => 0.005,
        "linkup" => 0.0055,
        "exa" => 0.007,
        "tavily" => 0.010,
        "serpapi" => 0.010,
        "perplexity" => 0.012,
        "browserless" => 0.004,
        "xai" => 0.05,
        _ => 0.0, // stealth and unknowns
    }
}

/// "brave_llm_context" bills as brave; "perplexity_sonar-pro" as perplexity.
fn base_provider(name: &str) -> &str {
    name.split('_').next().unwrap_or(name)
}

#[derive(Debug, Default, Serialize)]
pub struct ProviderStats {
    pub calls: u64,
    pub failures: u64,
    pub cancelled: u64,
    pub results_contributed: u64,
    /// All spend: list-price estimates for your own keys plus what
    /// metasearch.sh charged for hosted calls.
    pub estimated_spend_usd: f64,
    /// The part of the spend metasearch.sh charged (real money, not an estimate).
    pub charged_spend_usd: f64,
    /// Times an extract/scrape later ran on a URL this provider returned —
    /// the closest local proxy for "an agent found this result useful".
    pub result_read_through: u64,
    /// Links in run results that only this engine found (from each run's
    /// results.jsonl): what the engine adds that others miss.
    pub solo_links: u64,
    /// Run pages fetched from a link this engine found (shared credit).
    pub pages_fetched: u64,
}

#[derive(Debug, Serialize)]
pub struct StatsReport {
    pub window_days: u64,
    pub searches: u64,
    pub cache_hits: u64,
    pub cache_hit_rate: f64,
    pub by_mode: BTreeMap<String, u64>,
    pub by_provider: BTreeMap<String, ProviderStats>,
    pub estimated_total_spend_usd: f64,
    /// Charged by metasearch.sh across the window.
    pub charged_total_usd: f64,
    pub top_queries: Vec<(String, u64)>,
    /// Latest known balance per provider from `search usage` snapshots,
    /// with the change across the window (negative = burn).
    pub balances: BTreeMap<String, BalanceTrend>,
    pub log_files_read: u64,
}

#[derive(Debug, Serialize)]
pub struct BalanceTrend {
    pub latest: f64,
    pub unit: String,
    pub change_in_window: Option<f64>,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Every log line in the window, oldest first.
fn read_entries(days: u64) -> (Vec<serde_json::Value>, u64) {
    let cutoff = now_secs().saturating_sub(days * 86400);
    let mut entries = Vec::new();
    let mut files = 0u64;
    let Ok(dir) = std::fs::read_dir(logging::log_dir()) else {
        return (entries, files);
    };
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("searches_") || !name.ends_with(".jsonl") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        files += 1;
        for line in content.lines() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                if v.get("ts").and_then(|t| t.as_u64()).unwrap_or(0) >= cutoff {
                    entries.push(v);
                }
            }
        }
    }
    entries.sort_by_key(|v| v.get("ts").and_then(|t| t.as_u64()).unwrap_or(0));
    (entries, files)
}

pub fn compute(days: u64) -> StatsReport {
    let (entries, log_files_read) = read_entries(days);
    let mut report = summarize(&entries, days, log_files_read);
    report.balances = read_balance_trends(days);
    add_run_credit(&mut report, days);
    report
}

/// Per engine, from the run folders of the window: links only it found and
/// pages fetched from its links.
fn add_run_credit(report: &mut StatsReport, days: u64) {
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86400);
    let Ok(dir) = std::fs::read_dir(crate::run::runs_root()) else {
        return;
    };
    for entry in dir.flatten() {
        let path = entry.path().join("results.jsonl");
        let fresh = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .is_ok_and(|t| t >= cutoff);
        if !fresh {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let rows: Vec<serde_json::Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        // A link is "solo" only where two or more engines searched its query:
        // a query that asked one engine (a GitHub topic search) proves nothing.
        let mut asked: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
        for row in &rows {
            for f in row.get("found_by").and_then(|f| f.as_array()).into_iter().flatten() {
                if let (Some(q), Some(e)) = (f["q"].as_str(), f["engine"].as_str()) {
                    asked.entry(q.to_string()).or_default().insert(base_provider(e).to_string());
                }
            }
        }
        for row in &rows {
            let engines: Vec<String> = row
                .get("engines")
                .and_then(|e| e.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).map(|s| base_provider(s).to_string()).collect())
                .unwrap_or_default();
            if engines.iter().any(|e| e == "cache") {
                continue;
            }
            let contested = row
                .get("queries")
                .and_then(|q| q.as_array())
                .into_iter()
                .flatten()
                .filter_map(|q| q.as_str())
                .any(|q| asked.get(q).is_some_and(|set| set.len() >= 2));
            if engines.len() == 1 && contested {
                report.by_provider.entry(engines[0].clone()).or_default().solo_links += 1;
            }
            if row.get("page").is_some_and(|p| !p.is_null()) {
                for e in &engines {
                    report.by_provider.entry(e.clone()).or_default().pages_fetched += 1;
                }
            }
        }
    }
}

fn summarize(entries: &[serde_json::Value], days: u64, log_files_read: u64) -> StatsReport {
    let mut searches = 0u64;
    let mut cache_hits = 0u64;
    let mut by_mode: BTreeMap<String, u64> = BTreeMap::new();
    let mut by_provider: BTreeMap<String, ProviderStats> = BTreeMap::new();
    let mut query_counts: HashMap<String, u64> = HashMap::new();
    // url -> providers that returned it, for read-through attribution.
    let mut url_sources: HashMap<String, HashSet<String>> = HashMap::new();

    for e in entries {
        searches += 1;
        let cached = e.get("cached").and_then(|c| c.as_bool()).unwrap_or(false);
        if cached {
            cache_hits += 1;
        }
        let mode = e.get("mode").and_then(|m| m.as_str()).unwrap_or("?");
        *by_mode.entry(mode.to_string()).or_default() += 1;

        let query = e.get("query").and_then(|q| q.as_str()).unwrap_or("");
        *query_counts.entry(query.to_lowercase()).or_default() += 1;

        // Read-through: an extract/scrape whose target URL was previously
        // returned by a search credits every provider that returned it.
        if matches!(mode, "extract" | "scrape") {
            if let Some(sources) = url_sources.get(&normalize(query)) {
                for s in sources.clone() {
                    by_provider.entry(s).or_default().result_read_through += 1;
                }
            }
        }

        // Billing/contribution only counts fresh runs.
        if !cached {
            let failed: HashSet<String> = str_array(e, "providers_failed")
                .iter()
                .map(|p| base_provider(p).to_string())
                .collect();
            let logged_cost = e.pointer("/cost/by_provider").and_then(|c| c.as_object());
            let transport = e.get("transport").and_then(|t| t.as_object());
            for p in str_array(e, "providers_queried") {
                let base = base_provider(&p).to_string();
                let spend = match logged_cost {
                    // The engine's own figure, timeouts of billed calls included.
                    Some(costs) => {
                        costs.get(&p).and_then(|v| v.as_u64()).unwrap_or(0) as f64 / 1_000_000.0
                    }
                    // A failed call (bad key, no credit) was not billed.
                    None if failed.contains(&base) => 0.0,
                    None => legacy_cost_per_call(&base),
                };
                let hosted = transport
                    .and_then(|t| t.get(&p))
                    .is_some_and(|t| t == "hosted");
                let stat = by_provider.entry(base).or_default();
                stat.calls += 1;
                stat.estimated_spend_usd += spend;
                if hosted {
                    stat.charged_spend_usd += spend;
                }
            }
            for base in failed {
                by_provider.entry(base).or_default().failures += 1;
            }
            for p in str_array(e, "providers_cancelled") {
                by_provider
                    .entry(base_provider(&p).to_string())
                    .or_default()
                    .cancelled += 1;
            }
            if let Some(obj) = e.get("provider_results").and_then(|o| o.as_object()) {
                for (p, n) in obj {
                    by_provider
                        .entry(base_provider(p).to_string())
                        .or_default()
                        .results_contributed += n.as_u64().unwrap_or(0);
                }
            }
        }

        for url in str_array(e, "urls") {
            let sources: HashSet<String> = str_array(e, "sources")
                .iter()
                .map(|s| base_provider(s).to_string())
                .collect();
            url_sources
                .entry(normalize(&url))
                .or_default()
                .extend(sources);
        }
    }

    let estimated_total_spend_usd = by_provider.values().map(|p| p.estimated_spend_usd).sum();
    let charged_total_usd = by_provider.values().map(|p| p.charged_spend_usd).sum();

    let mut top_queries: Vec<(String, u64)> = query_counts.into_iter().collect();
    top_queries.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    top_queries.truncate(10);
    top_queries.retain(|(_, n)| *n > 1);

    StatsReport {
        window_days: days,
        searches,
        cache_hits,
        cache_hit_rate: if searches > 0 {
            cache_hits as f64 / searches as f64
        } else {
            0.0
        },
        by_mode,
        by_provider,
        estimated_total_spend_usd,
        charged_total_usd,
        top_queries,
        balances: BTreeMap::new(),
        log_files_read,
    }
}

fn str_array(e: &serde_json::Value, key: &str) -> Vec<String> {
    e.get(key)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn normalize(url: &str) -> String {
    url.trim_end_matches('/').to_lowercase()
}

/// Latest balance per provider plus the delta across the window — actual
/// burn, measured, not estimated. Fed by `search usage` snapshots.
fn read_balance_trends(days: u64) -> BTreeMap<String, BalanceTrend> {
    let cutoff = now_secs().saturating_sub(days * 86400);
    let path = logging::log_dir().join("balances.jsonl");
    let Ok(content) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    // provider -> (first_in_window, latest, unit)
    let mut trends: BTreeMap<String, (Option<f64>, f64, String)> = BTreeMap::new();
    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let ts = v.get("ts").and_then(|t| t.as_u64()).unwrap_or(0);
        let Some(balances) = v.get("balances").and_then(|b| b.as_object()) else {
            continue;
        };
        for (provider, info) in balances {
            let Some(remaining) = info.get("credits_remaining").and_then(|c| c.as_f64()) else {
                continue;
            };
            let unit = info
                .get("unit")
                .and_then(|u| u.as_str())
                .unwrap_or("credits")
                .to_string();
            let entry = trends
                .entry(provider.clone())
                .or_insert((None, remaining, unit.clone()));
            if ts >= cutoff && entry.0.is_none() {
                entry.0 = Some(remaining);
            }
            entry.1 = remaining;
            entry.2 = unit;
        }
    }
    trends
        .into_iter()
        .map(|(p, (first, latest, unit))| {
            (
                p,
                BalanceTrend {
                    latest,
                    unit,
                    change_in_window: first.map(|f| latest - f),
                },
            )
        })
        .collect()
}

/// Spend per provider (micro-USD) of fresh calls logged at or after `since`
/// (Unix seconds), from each line's `cost.by_provider`.
pub fn spend_since(since: u64) -> BTreeMap<String, u64> {
    let mut out: BTreeMap<String, u64> = BTreeMap::new();
    for (_, p, m) in spend_lines(since) {
        *out.entry(p).or_default() += m;
    }
    out
}

/// Every fresh logged cost at or after `since`: (when, provider, micro-USD).
pub fn spend_lines(since: u64) -> Vec<(u64, String, u64)> {
    let days = now_secs().saturating_sub(since) / 86_400 + 1;
    let (entries, _) = read_entries(days);
    let mut out = Vec::new();
    for e in entries {
        let ts = e.get("ts").and_then(|t| t.as_u64()).unwrap_or(0);
        if ts < since || e.get("cached").and_then(|c| c.as_bool()).unwrap_or(false) {
            continue;
        }
        if let Some(costs) = e.pointer("/cost/by_provider").and_then(|c| c.as_object()) {
            for (p, v) in costs {
                out.push((ts, base_provider(p).to_string(), v.as_u64().unwrap_or(0)));
            }
        }
    }
    out
}



/// Unix seconds of the first day of this month (local time).
pub fn month_start() -> u64 {
    use chrono::{Datelike, Local, TimeZone};
    let now = Local::now();
    Local
        .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
        .single()
        .map(|t| t.timestamp().max(0) as u64)
        .unwrap_or(0)
}

/// Balances that are low, held back or expired (`budget`).
pub fn quota_warnings() -> Vec<String> {
    crate::budget::warnings(&crate::budget::statuses())
}

/// Delete log files older than `days`. Returns files removed.
pub fn prune_logs(days: u64) -> usize {
    let cutoff = now_secs().saturating_sub(days * 86400);
    let Ok(dir) = std::fs::read_dir(logging::log_dir()) else {
        return 0;
    };
    let mut removed = 0;
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("searches_") || !name.ends_with(".jsonl") {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .is_some_and(|d| d.as_secs() < cutoff);
        if old && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

pub fn render_human(r: &StatsReport) {
    use crate::output::style::Paint;
    eprintln!(
        "\n{}  last {} days — {} searches, {:.0}% cache hits\n",
        "stats".bold().cyan(),
        r.window_days,
        r.searches,
        r.cache_hit_rate * 100.0
    );
    println!("modes:");
    for (mode, n) in &r.by_mode {
        println!("  {mode:<10} {n}");
    }
    println!("\nproviders (fresh calls only):");
    println!(
        "  {:<12} {:>6} {:>6} {:>8} {:>10} {:>6} {:>8}",
        "provider", "calls", "fail", "results", "~spend", "solo", "fetched"
    );
    for (p, s) in &r.by_provider {
        println!(
            "  {:<12} {:>6} {:>6} {:>8} {:>9.2}$ {:>6} {:>8}",
            p,
            s.calls,
            s.failures,
            s.results_contributed,
            s.estimated_spend_usd,
            s.solo_links,
            s.pages_fetched
        );
    }
    println!("  solo = links only this engine found, in searches where two or more engines looked; fetched = run pages from its links");
    println!(
        "\ntotal spend: ${:.2} (list-price estimates; `osearch usage` shows real balances)",
        r.estimated_total_spend_usd
    );
    let month = spend_since(month_start());
    if !month.is_empty() {
        println!("\nthis month (list-price estimates; `osearch usage` shows balances):");
        for (p, m) in &month {
            println!("  {p:<12} ${:.4}", *m as f64 / 1e6);
        }
    }
    for w in quota_warnings() {
        println!("  ! {w}");
    }
    if !r.balances.is_empty() {
        println!("\nbalances (from usage snapshots):");
        for (p, b) in &r.balances {
            let delta = b
                .change_in_window
                .map(|d| format!(" ({d:+.1} this window)"))
                .unwrap_or_default();
            println!("  {:<12} {} {}{}", p, b.latest, b.unit, delta);
        }
    }
    if !r.top_queries.is_empty() {
        println!("\nrepeated queries:");
        for (q, n) in &r.top_queries {
            println!("  {n}x  {q}");
        }
    }
    fetch_view(r.window_days);
    println!();
}

fn pct(v: &mut [u64], p: usize) -> u64 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    v[(v.len() - 1) * p / 100]
}

/// How each fetcher does over the window's runs, the pages finished in the
/// background, and the sites even the paid readers fail on: the evidence
/// the per-fetcher waits and the run budgets are tuned from.
fn fetch_view(days: u64) {
    let cutoff = chrono::Local::now() - chrono::Duration::days(days as i64);
    let cutoff = cutoff.format("%Y-%m-%d_%H%M%S").to_string();
    let mut ok_ms: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    let mut fails: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let (mut late, mut late_ok, mut late_ms) = (0usize, 0usize, Vec::new());
    let Ok(dirs) = std::fs::read_dir(crate::run::runs_root()) else { return };
    for d in dirs.flatten() {
        let id = d.file_name().to_string_lossy().into_owned();
        if id < cutoff {
            continue;
        }
        for p in crate::run::load_pages(&d.path()) {
            let mut total = 0u64;
            for a in &p.attempts {
                let fetcher = a["fetcher"].as_str().unwrap_or("?").to_string();
                let outcome = a["outcome"].as_str().unwrap_or("");
                let ms = a["ms"].as_u64().unwrap_or(0);
                total += ms;
                if outcome.starts_with("ok") {
                    ok_ms.entry(fetcher).or_default().push(ms);
                } else {
                    let kind = outcome.split(':').next().unwrap_or("other").to_string();
                    *fails.entry(fetcher).or_default().entry(kind).or_default() += 1;
                }
            }
            if p.late {
                late += 1;
                if p.ok {
                    late_ok += 1;
                    late_ms.push(total);
                }
            }
        }
    }
    if ok_ms.is_empty() && fails.is_empty() {
        return;
    }
    println!("\nfetching (runs of the window):");
    println!("  {:<10} {:>6} {:>8} {:>8} {:>8}  failures", "fetcher", "ok", "p50", "p90", "slowest");
    let names: std::collections::BTreeSet<&String> = ok_ms.keys().chain(fails.keys()).collect();
    for n in names {
        let mut v = ok_ms.get(n).cloned().unwrap_or_default();
        let f = fails
            .get(n)
            .map(|m| m.iter().map(|(k, c)| format!("{k} {c}")).collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        let (p50, p90, max) = (pct(&mut v, 50), pct(&mut v, 90), v.iter().copied().max().unwrap_or(0));
        println!(
            "  {:<10} {:>6} {:>7.1}s {:>7.1}s {:>7.1}s  {f}",
            n,
            v.len(),
            p50 as f64 / 1000.0,
            p90 as f64 / 1000.0,
            max as f64 / 1000.0
        );
    }
    if late > 0 {
        println!(
            "  finished in the background: {late_ok} of {late} pages, median {:.1} s of fetching each",
            pct(&mut late_ms, 50) as f64 / 1000.0
        );
    }
    let hard = crate::fetch::memory::hard_sites();
    if !hard.is_empty() {
        let list: Vec<String> = hard.iter().map(|(h, n)| format!("{h} ({n} pages)")).collect();
        println!("  sites no fetcher gets past, paid readers included: {}", list.join(", "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spend_uses_logged_cost_and_skips_failed_calls() {
        let entries = vec![
            // New line: the engine's own per-provider cost.
            serde_json::json!({
                "ts": 1, "mode": "general", "query": "a", "cached": false,
                "providers_queried": ["exa", "serper"],
                "providers_failed": ["serper"],
                "cost": {"by_provider": {"exa": 9000}},
            }),
            // Old line: flat table, the failed provider not billed.
            serde_json::json!({
                "ts": 2, "mode": "general", "query": "b", "cached": false,
                "providers_queried": ["serper", "brave"],
                "providers_failed": ["brave"],
            }),
        ];
        let r = summarize(&entries, 7, 1);
        assert!((r.by_provider["exa"].estimated_spend_usd - 0.009).abs() < 1e-9);
        assert!((r.by_provider["serper"].estimated_spend_usd - 0.001).abs() < 1e-9);
        assert_eq!(r.by_provider["brave"].estimated_spend_usd, 0.0);
        assert_eq!(r.by_provider["brave"].failures, 1);
        assert_eq!(r.by_provider["serper"].calls, 2);
        assert_eq!(r.charged_total_usd, 0.0);
    }

    #[test]
    fn hosted_calls_count_as_charged() {
        let entries = vec![serde_json::json!({
            "ts": 1, "mode": "general", "query": "a", "cached": false,
            "providers_queried": ["exa", "serper"],
            "providers_failed": [],
            "transport": {"exa": "hosted", "serper": "direct"},
            "cost": {"by_provider": {"exa": 10500, "serper": 1000}},
        })];
        let r = summarize(&entries, 7, 1);
        assert!((r.by_provider["exa"].charged_spend_usd - 0.0105).abs() < 1e-9);
        assert_eq!(r.by_provider["serper"].charged_spend_usd, 0.0);
        assert!((r.charged_total_usd - 0.0105).abs() < 1e-9);
        assert!((r.estimated_total_spend_usd - 0.0115).abs() < 1e-9);
    }
}
