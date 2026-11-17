//! Tier 1: `donsetch fetch --json`. The contract OmniSearch relies on:
//! exit 0 ok, 1 permanent, 2 transient, 3 walled; a JSON body with `ok`,
//! `content` when ok, `meta.code` (`wall.challenge`, `wall.auth`,
//! `content.notfound`, `content.incomplete`, ...) and `error.message` when
//! not. Anything else is a contract warning and the page falls through.

use super::{Got, Miss, TierResult};
use crate::tools::{self, Tool};
use serde_json::Value;
use std::time::Duration;

const DEADLINE_MS: u64 = 12_000;

pub async fn fetch(url: &str, links: bool) -> TierResult {
    let mut args: Vec<String> = vec![
        "fetch".into(),
        url.into(),
        "--json".into(),
        "-q".into(),
        "--max-chars".into(),
        "1048576".into(),
        "--deadline-ms".into(),
        DEADLINE_MS.to_string(),
    ];
    if links {
        args.push("--links=urls".into());
    }
    let ran = tools::run(
        Tool::Donsetch,
        &args,
        Duration::from_millis(DEADLINE_MS + 5_000),
    )
    .await
    .map_err(|e| (Miss::Other, e))?;
    if ran.timed_out {
        return Err((Miss::Other, "donsetch timed out".into()));
    }
    if !matches!(ran.code, Some(0..=3)) {
        tools::contract_warning(
            "donsetch fetch",
            &format!("unexpected exit {:?}: {}", ran.code, ran.stderr.trim()),
        );
        return Err((Miss::Other, format!("exit {:?}", ran.code)));
    }
    let body: Value = match serde_json::from_slice(&ran.stdout) {
        Ok(v) => v,
        Err(e) => {
            tools::contract_warning("donsetch fetch", &format!("output is not JSON: {e}"));
            return Err((Miss::Other, format!("not JSON: {e}")));
        }
    };
    let Some(ok) = body.get("ok").and_then(Value::as_bool) else {
        tools::contract_warning("donsetch fetch", "no `ok` field in the JSON");
        return Err((Miss::Other, "no ok field".into()));
    };
    let meta = &body["meta"];
    let final_url = meta["url"].as_str().unwrap_or(url).to_string();
    if ok {
        let Some(content) = body["content"].as_str() else {
            tools::contract_warning("donsetch fetch", "ok without a `content` string");
            return Err((Miss::Other, "no content".into()));
        };
        if ran.code != Some(0) {
            tools::contract_warning("donsetch fetch", &format!("ok with exit {:?}", ran.code));
        }
        return Ok(to_got(&final_url, content));
    }
    let code = meta["code"].as_str().unwrap_or("");
    let message = body
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or(code)
        .lines()
        .next()
        .unwrap_or("")
        .to_string();
    let status = meta["status"].as_u64().unwrap_or(0);
    let miss = if code == "network.dns" && meta["errorKind"].as_str() == Some("permanent") {
        Miss::NoHost
    } else if code == "wall.auth" {
        Miss::Login
    } else if code.starts_with("wall.") || ran.code == Some(3) {
        Miss::Wall
    } else if code == "content.notfound" || matches!(status, 404 | 410) {
        Miss::NotFound
    } else if status == 402 {
        Miss::Paywall
    } else if code == "content.incomplete" || message.contains("client-side") {
        Miss::JsPage
    } else {
        if code.is_empty() {
            tools::contract_warning("donsetch fetch", "failure without `meta.code`");
        }
        Miss::Other
    };
    Err((miss, format!("{code}: {message}")))
}

/// donsetch's markdown starts with `# Title` and the URL below it; a page
/// with no title (a plain `.md` file) starts with the URL alone, and its own
/// first heading becomes the title.
fn to_got(final_url: &str, content: &str) -> Got {
    let lines: Vec<&str> = content.lines().collect();
    let mut title = final_url.to_string();
    let mut next = 0;
    if let Some(t) = lines.first().and_then(|l| l.trim().strip_prefix("# ")) {
        title = t.trim().to_string();
        next = 1;
    }
    let mut skip = next;
    while lines.get(next).is_some_and(|l| l.trim().is_empty()) {
        next += 1;
    }
    if lines.get(next).is_some_and(|l| l.trim().starts_with("http")) {
        skip = next + 1;
    }
    let body = lines[skip.min(lines.len())..].join("\n");
    let body = body.trim();
    let own = body
        .lines()
        .find(|l| !l.trim().is_empty())
        .and_then(|l| l.trim().strip_prefix("# "))
        .map(|h| h.trim().to_string());
    let (title, markdown) = match own {
        Some(h) if title.starts_with("http") && !h.is_empty() => (h, body.to_string()),
        _ => (title.clone(), format!("# {title}\n\n{body}")),
    };
    Got {
        stamp: None,
        note: None,
        published: None,
        final_url: final_url.to_string(),
        markdown,
        title,
        loss: None,
        cost_micros: 0,
    }
}
