//! Balances and balance-aware engine picking.
//!
//! Every paid service has an allowance in config (`[balance.<service>]`):
//! monthly (refills on a day of the month), onetime, or expiring (a date).
//! What is left comes from the newest live reading (`osearch usage`, or the
//! automatic refresh before a run) minus the spend logged since, else from
//! the configured starting amount minus the spend since its date.
//!
//! Picking: free engines always; monthly refills, expiring credits and
//! plentiful balances by default (monthly ones paced so they never run dry
//! mid-month); finite caps only when needed in standard, and in wide and
//! deep only above 20% left. An engine with no allowance is used freely.

use chrono::{Datelike, Local, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::types::Mode;

/// Finite credits stop being used below this share.
const FINITE_FLOOR: f64 = 0.20;
/// Every other paid balance stops below this share.
const LOW_FLOOR: f64 = 0.05;
/// A monthly allowance may run this far ahead of the month before it is held back.
const PACE_SLACK: f64 = 0.15;
/// A live reading older than this is refreshed before a run.
pub const STALE_SECS: u64 = 12 * 3600;
/// Standard width: how many engines.
const STANDARD_SIZE: usize = 6;
/// Standard width: below this many usable engines, finite ones fill in.
const STANDARD_MIN: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Refills to `amount` on `resetday` every month.
    Monthly,
    /// `amount` as of `checked`, never refilled.
    Onetime,
    /// `amount` as of `checked`, gone after `expires`.
    Expiring,
}

/// One service's allowance, as configured.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Allowance {
    pub kind: Kind,
    /// Monthly: the refill. Onetime and expiring: the balance on `checked`.
    pub amount: f64,
    /// usd, credits, requests, searches, tokens.
    pub unit: String,
    /// USD per unit: turns the logged list-price spend into units.
    pub unitprice: f64,
    /// Monthly: the day of the month it refills.
    #[serde(default = "first_day")]
    pub resetday: u32,
    /// Expiring: the last day the credits work (YYYY-MM-DD).
    #[serde(default)]
    pub expires: String,
    /// The day `amount` was read (YYYY-MM-DD).
    #[serde(default)]
    pub checked: String,
    /// A onetime balance large enough to use freely.
    #[serde(default)]
    pub plentiful: bool,
}

fn first_day() -> u32 {
    1
}

/// No allowances are built in: each user's plans and credits differ, so
/// they live in the config (`[balance.<service>]`, see config.example.toml).
pub fn defaults() -> BTreeMap<String, Allowance> {
    BTreeMap::new()
}

/// How the picker treats a service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Monthly, expiring or plentiful: used by default.
    Refill,
    /// A finite cap: only when needed.
    Finite,
}

/// What is known about one service's balance now.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub service: String,
    pub kind: Kind,
    pub unit: String,
    pub remaining: f64,
    /// The allowance (monthly) or the reference balance (onetime, expiring).
    pub total: f64,
    /// From a live reading in the last 12 hours.
    pub live: bool,
    /// The day the figure it starts from was read.
    pub as_of: String,
    /// Monthly: the next refill. Expiring: the last day.
    pub until: Option<String>,
    pub expired: bool,
    /// Monthly: share of the allowance used, and share of the month gone.
    pub used_share: Option<f64>,
    pub month_share: Option<f64>,
    pub plentiful: bool,
}

impl Status {
    pub fn share_left(&self) -> f64 {
        if self.total > 0.0 {
            self.remaining / self.total
        } else {
            1.0
        }
    }

    pub fn class(&self) -> Class {
        match self.kind {
            Kind::Onetime if !self.plentiful => Class::Finite,
            _ => Class::Refill,
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn day_start(d: NaiveDate) -> u64 {
    d.and_hms_opt(0, 0, 0)
        .and_then(|t| Local.from_local_datetime(&t).single())
        .map(|t| t.timestamp().max(0) as u64)
        .unwrap_or(0)
}

fn date_of(secs: u64) -> String {
    Local
        .timestamp_opt(secs as i64, 0)
        .single()
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

fn reset_in(year: i32, month: u32, day: u32) -> NaiveDate {
    let mut d = day.clamp(1, 31);
    loop {
        if let Some(date) = NaiveDate::from_ymd_opt(year, month, d) {
            return date;
        }
        d -= 1;
    }
}

/// The last refill on or before today and the next one.
fn cycle(resetday: u32) -> (NaiveDate, NaiveDate) {
    let today = Local::now().date_naive();
    let (y, m) = (today.year(), today.month());
    let this = reset_in(y, m, resetday);
    let (py, pm) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    if this <= today {
        (this, reset_in(ny, nm, resetday))
    } else {
        (reset_in(py, pm, resetday), this)
    }
}

/// Every engine of a service, as named in the cost log.
fn engines_of(service: &str) -> Vec<&'static str> {
    if service == "scrapecreators" {
        return crate::providers::social_api::PLATFORMS.to_vec();
    }
    crate::providers::all()
        .iter()
        .map(|s| s.name)
        .filter(|n| *n == service)
        .collect()
}

/// The newest live reading per service: (when, remaining).
fn latest_readings() -> BTreeMap<String, (u64, f64)> {
    let path = crate::logging::log_dir().join("balances.jsonl");
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let ts = v.get("ts").and_then(|t| t.as_u64()).unwrap_or(0);
        let Some(b) = v.get("balances").and_then(|b| b.as_object()) else { continue };
        for (service, info) in b {
            // Only readings taken in the service's own unit count.
            if info.get("budget_unit").is_none() {
                continue;
            }
            if let Some(r) = info.get("credits_remaining").and_then(|c| c.as_f64()) {
                out.insert(service.clone(), (ts, r));
            }
        }
    }
    out
}

/// Every configured service's status, computed from one read of the logs.
pub fn statuses() -> Vec<Status> {
    let table = &crate::config::get().balance;
    let readings = latest_readings();
    // Every logged cost of the last ~70 days (the oldest start a status needs).
    let lines = crate::stats::spend_lines(now_secs().saturating_sub(70 * 86_400));
    let since = |service: &str, from: u64| -> f64 {
        let engines = engines_of(service);
        lines
            .iter()
            .filter(|(ts, p, _)| *ts >= from && engines.contains(&p.as_str()))
            .map(|(_, _, m)| *m)
            .sum::<u64>() as f64
            / 1e6
    };
    let now = now_secs();
    let mut out = Vec::new();
    for (service, a) in table {
        if a.unitprice <= 0.0 {
            continue;
        }
        let units = |from: u64| since(service, from) / a.unitprice;
        let reading = readings.get(service).copied();
        let checked = NaiveDate::parse_from_str(&a.checked, "%Y-%m-%d")
            .map(day_start)
            .unwrap_or(0);
        let mut st = Status {
            service: service.clone(),
            kind: a.kind,
            unit: a.unit.clone(),
            remaining: 0.0,
            total: a.amount,
            live: reading.is_some_and(|(ts, _)| now.saturating_sub(ts) < STALE_SECS),
            as_of: String::new(),
            until: None,
            expired: false,
            used_share: None,
            month_share: None,
            plentiful: a.plentiful,
        };
        match a.kind {
            Kind::Monthly => {
                let (last, next) = cycle(a.resetday);
                let (from, base) = match reading {
                    Some((ts, r)) if ts >= day_start(last) => (ts, r),
                    _ => (day_start(last), a.amount),
                };
                st.remaining = (base - units(from)).max(0.0);
                st.as_of = date_of(from);
                st.until = Some(next.format("%Y-%m-%d").to_string());
                let len = (day_start(next) - day_start(last)).max(1) as f64;
                st.month_share = Some((now.saturating_sub(day_start(last)) as f64 / len).min(1.0));
                st.used_share = Some(if a.amount > 0.0 { 1.0 - st.remaining / a.amount } else { 0.0 });
            }
            Kind::Onetime | Kind::Expiring => {
                let (from, base) = match reading {
                    Some((ts, r)) if ts >= checked => (ts, r),
                    _ => (checked, a.amount),
                };
                st.remaining = (base - units(from)).max(0.0);
                st.as_of = date_of(from);
                if a.kind == Kind::Expiring {
                    st.until = Some(a.expires.clone());
                    if let Ok(last) = NaiveDate::parse_from_str(&a.expires, "%Y-%m-%d") {
                        st.expired = Local::now().date_naive() > last;
                    }
                }
            }
        }
        out.push(st);
    }
    out
}

/// Why a service should sit this one out, or None when it may run.
/// `finite_ok`: whether a finite cap may join (above its floor).
pub fn hold(st: &Status, finite_ok: bool) -> Option<String> {
    let pct = |x: f64| format!("{:.0}%", x * 100.0);
    if st.expired {
        return Some(format!("its credits expired on {}", st.until.clone().unwrap_or_default()));
    }
    let left = st.share_left();
    match st.class() {
        Class::Finite if !finite_ok => return Some("a finite balance, kept for when it is needed".into()),
        Class::Finite if left <= FINITE_FLOOR => {
            return Some(format!("finite credits down to {} (it stops at 20%)", pct(left)))
        }
        _ if left <= LOW_FLOOR => return Some(format!("nearly out ({} left)", pct(left))),
        _ => {}
    }
    if let (Some(used), Some(month)) = (st.used_share, st.month_share) {
        if used > month + PACE_SLACK {
            return Some(format!(
                "ahead of its monthly pace ({} used, {} of the month gone; it refills {})",
                pct(used),
                pct(month),
                st.until.clone().unwrap_or_default()
            ));
        }
    }
    None
}

/// How many engines a search uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Width {
    /// About six engines, picked by kind and balance.
    Standard,
    /// Every web engine with balance to spare.
    #[default]
    Wide,
}

impl Width {
    pub fn as_str(self) -> &'static str {
        match self {
            Width::Standard => "standard",
            Width::Wide => "wide",
        }
    }
}

/// The default standard set, in order: free multi-engine, Exa (semantic,
/// paced), You.com (plentiful), Parallel (expiring), Brave (own index),
/// Tavily; Jina and Linkup stand in when one is held back.
pub const STANDARD: &[&str] = &["donsetch", "exa", "youcom", "parallel", "brave", "tavily", "jina", "linkup"];

/// The engines picked for one query, and why the others were left out.
pub struct Picked {
    pub engines: Vec<String>,
    pub held: Vec<String>,
}

/// Balance-aware picking for a fused web mode. `candidates` are the mode's
/// configured engines in registry order.
pub fn pick(mode: Mode, width: Width, candidates: &[&str], statuses: &[Status]) -> Picked {
    // ScrapeCreators platforms share one balance.
    let status = |e: &str| {
        let service = if crate::providers::social_api::PLATFORMS.contains(&e) { "scrapecreators" } else { e };
        statuses.iter().find(|s| s.service == service)
    };
    let standard = mode == Mode::General && width == Width::Standard;
    let mut held = Vec::new();
    let mut ok: Vec<&str> = Vec::new();
    let mut finite_spare: Vec<&str> = Vec::new();
    for e in candidates {
        match status(e) {
            None => ok.push(e),
            Some(st) => match hold(st, !standard) {
                None => ok.push(e),
                // Standard keeps a healthy finite engine in reserve.
                Some(_) if standard && hold(st, true).is_none() => finite_spare.push(e),
                Some(why) => held.push(format!("{e}: {why}")),
            },
        }
    }
    if !standard {
        return Picked {
            engines: ok.iter().map(|e| e.to_string()).collect(),
            held,
        };
    }
    let order = crate::config::get().research.standard.clone();
    let order: Vec<String> = if order.is_empty() {
        STANDARD.iter().map(|s| s.to_string()).collect()
    } else {
        order
    };
    let mut engines: Vec<String> = order
        .iter()
        .filter(|e| ok.contains(&e.as_str()))
        .take(STANDARD_SIZE)
        .cloned()
        .collect();
    for e in &finite_spare {
        if engines.len() >= STANDARD_MIN {
            break;
        }
        engines.push(e.to_string());
    }
    Picked { engines, held }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn onetime(service: &str, remaining: f64, total: f64) -> Status {
        Status {
            service: service.into(),
            kind: Kind::Onetime,
            unit: "credits".into(),
            remaining,
            total,
            live: true,
            as_of: String::new(),
            until: None,
            expired: false,
            used_share: None,
            month_share: None,
            plentiful: false,
        }
    }

    /// ScrapeCreators platforms are held by their shared balance's floor
    /// (they once slipped past it: the balance is stored as `scrapecreators`).
    #[test]
    fn platforms_share_the_scrapecreators_floor() {
        let low = [onetime("scrapecreators", 100.0, 1767.0)];
        let p = pick(Mode::Social, Width::Wide, &["tiktok", "threads"], &low);
        assert!(p.engines.is_empty());
        assert_eq!(p.held.len(), 2);
        let fine = [onetime("scrapecreators", 1500.0, 1767.0)];
        assert_eq!(pick(Mode::Social, Width::Wide, &["tiktok"], &fine).engines, ["tiktok"]);
    }
}

/// Results asked of each engine when the request names no count: 20 where
/// more results cost the same, fewer where each extra result is billed.
pub fn preferred_count(engine: &str) -> u16 {
    match engine {
        // $1/1k per result past 10: 20 makes a $0.007 call $0.017.
        "exa" => 20,
        // Google pages hold 10; more is a second billed page.
        "serper" => 10,
        "donsetch" => 12,
        "youtube" | "xai" | "perplexity" => 10,
        // Billed per call whatever the count (Parallel: expiring credits).
        "tavily" | "brave" | "youcom" | "linkup" | "jina" | "parallel" | "serpapi" => 20,
        "github" | "ghissues" | "reddit" | "hackernews" => 20,
        _ => 10,
    }
}

/// One line per service for `osearch usage` and doctor: warnings only.
pub fn warnings(statuses: &[Status]) -> Vec<String> {
    statuses
        .iter()
        .filter_map(|st| {
            let left = st.share_left();
            let low = match st.class() {
                Class::Finite => left <= FINITE_FLOOR + 0.1,
                _ => left <= 0.2,
            };
            if st.expired {
                Some(format!("{}: credits expired on {}", st.service, st.until.clone().unwrap_or_default()))
            } else if let Some(why) = hold(st, true) {
                Some(format!("{}: {why}", st.service))
            } else if low {
                Some(format!("{}: {:.0}% left", st.service, left * 100.0))
            } else {
                None
            }
        })
        .collect()
}
