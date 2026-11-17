//! The Wayback Machine's newest saved copy of a page: for a page that is
//! gone, walled at every tier, or behind a paywall. Never used in place of a
//! page that loaded.

use super::{convert, Miss, TierResult};
use crate::context::AppContext;
use std::sync::Arc;
use std::time::{Duration, Instant};

const AVAILABLE: &str = "https://archive.org/wayback/available";

/// The newest good snapshot's timestamp (`YYYYMMDDhhmmss`).
pub async fn lookup(app: &Arc<AppContext>, url: &str) -> Result<String, String> {
    let started = Instant::now();
    let resp = app
        .client
        .get(AVAILABLE)
        .query(&[("url", url)])
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("wayback lookup: {}", e.without_url()))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(|e| format!("wayback lookup: {e}"))?;
    crate::run::raw::record_page("wayback", &format!("wayback lookup: {url}"), status, started.elapsed(), &text);
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("wayback lookup: {e}"))?;
    let snap = &v["archived_snapshots"]["closest"];
    match (snap["available"].as_bool(), snap["status"].as_str(), snap["timestamp"].as_str()) {
        (Some(true), Some("200"), Some(ts)) if ts.len() >= 8 => Ok(ts.to_string()),
        _ => Err("the Wayback Machine has no saved copy".into()),
    }
}

/// The snapshot `ts` of `url`, as it was saved (`id_`: no Wayback toolbar).
pub async fn snapshot(url: &str, ts: &str, why: &str) -> TierResult {
    let raw = format!("https://web.archive.org/web/{ts}id_/{url}");
    let client = crate::page::client().map_err(|e| (Miss::Other, e.to_string()))?;
    let started = Instant::now();
    let mut p = crate::page::fetch_direct_with(&client, &raw, Duration::from_secs(20), false)
        .await
        .map_err(|e| (Miss::Other, format!("wayback: {e}")))?;
    crate::run::raw::record_page("wayback", &raw, p.status, started.elapsed(), &p.body);
    if !(200..300).contains(&p.status) {
        return Err((Miss::Other, format!("wayback: HTTP {}", p.status)));
    }
    // Converted against the original address, so its relative links point
    // at the site, not at web.archive.org.
    p.final_url = url.to_string();
    let mut g = convert::from_page(p).await?;
    g.final_url = format!("https://web.archive.org/web/{ts}/{url}");
    g.stamp = Some(format!("Wayback Machine copy saved {} ({why})", date(ts)));
    Ok(g)
}

/// `20251130231336` as `2025-11-30`.
fn date(ts: &str) -> String {
    format!("{}-{}-{}", &ts[..4], &ts[4..6], &ts[6..8])
}

/// Why the live page was not used, for the stamp.
pub fn reason(failure: Option<&str>) -> &'static str {
    match failure {
        Some("not_found") => "the live page is gone",
        Some("no_such_host") => "the site no longer exists",
        Some("paywall") => "the live page is behind a paywall",
        Some("login_wall") => "the live page needs a login",
        _ => "every fetcher was refused or failed on the live page",
    }
}
