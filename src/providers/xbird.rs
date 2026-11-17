//! `xbird`: X (Twitter) search through X's own web API with the saved
//! x.com login, by the vendored `bird` client (`vendor/bird-search`, MIT,
//! Peter Steinberger, via last30days) on Node. It uses the account's real
//! session, so it is off unless `research.xbird` is on.

use super::spec::{FilterSupport, Price};
use super::{Adapter, Call, Output, ProviderSpec};
use crate::errors::SearchError;
use crate::types::{FailureCategory, Mode, SearchResult, Transport};
use async_trait::async_trait;
use std::path::PathBuf;
use std::time::Duration;

pub static SPEC: ProviderSpec = ProviderSpec {
    name: "xbird",
    env_keys: &[],
    modes: &[Mode::Social],
    filters: FilterSupport {
        freshness: true,
        domains: false,
        country: false,
        lang: false,
    },
    max_count: 40,
    timeout: Duration::from_secs(40),
    billed_on_send: false,
    price: Price {
        per_call: 0,
        per_extra_result: 0,
        included_results: 0,
    },
};

pub struct XBird;

fn fail(category: FailureCategory, message: String) -> SearchError {
    SearchError::Upstream {
        provider: "xbird",
        status: 0,
        category,
        message,
    }
}

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vendor").join("bird-search").join("bird-search.mjs")
}

/// `auth_token` and `ct0` from the saved x.com login.
fn credentials() -> Option<(String, String)> {
    let jar = crate::cookies::for_url("https://x.com/");
    let get = |n: &str| jar.iter().find(|c| c.name == n).map(|c| c.value.clone());
    Some((get("auth_token")?, get("ct0")?))
}

#[async_trait]
impl Adapter for XBird {
    fn spec(&self) -> &'static ProviderSpec {
        &SPEC
    }

    fn is_configured(&self) -> bool {
        crate::config::get().research.xbird && script().is_file() && credentials().is_some()
    }

    fn transport(&self) -> Transport {
        Transport::Local
    }

    async fn call(&self, call: &Call<'_>) -> Result<Output, SearchError> {
        let (token, ct0) = credentials().ok_or_else(|| fail(FailureCategory::Config, "no saved x.com login (`osearch login x.com`)".into()))?;
        let n = call.count.min(SPEC.max_count);
        // `-f`: X's own date filter on top posts.
        let days = match call.opts.freshness.as_deref() {
            Some("day") => Some(1),
            Some("week") => Some(7),
            Some("month") => Some(30),
            Some("year") => Some(365),
            _ => None,
        };
        let query = match days {
            Some(d) => {
                let since = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64
                    - d * 86_400;
                let date = crate::providers::thread::iso_from_unix(since);
                format!("{} since:{}", call.query, date.get(..10).unwrap_or_default())
            }
            None => call.query.to_string(),
        };
        let mut cmd = tokio::process::Command::new("node");
        cmd.arg(script())
            .arg(&query)
            .args(["--count", &n.to_string(), "--json"])
            .env("AUTH_TOKEN", token)
            .env("CT0", ct0)
            .env("BIRD_DISABLE_BROWSER_COOKIES", "1")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        let out = tokio::time::timeout(call.timeout, cmd.output())
            .await
            .map_err(|_| fail(FailureCategory::Timeout, "X search timed out".into()))?
            .map_err(|e| fail(FailureCategory::Config, format!("could not run node: {e}")))?;
        let text = String::from_utf8_lossy(&out.stdout);
        crate::run::raw::record_page("xbird", &format!("x search: {query}"), if out.status.success() { 200 } else { 500 }, Duration::ZERO, &text);
        let v: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| fail(FailureCategory::Parse, format!("{e}; {}", String::from_utf8_lossy(&out.stderr).trim())))?;
        if let Some(e) = v["error"].as_str() {
            return Err(fail(FailureCategory::Rejected, e.to_string()));
        }
        let results: Vec<SearchResult> = v
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| {
                let id = t["id"].as_str()?;
                let user = t["author"]["username"].as_str().unwrap_or("i");
                let body = t["text"].as_str().unwrap_or("").to_string();
                let likes = t["likeCount"].as_i64().unwrap_or(0);
                let replies = t["replyCount"].as_i64().unwrap_or(0);
                Some(SearchResult {
                    title: format!("@{user}: {}", crate::providers::thread::clip(&crate::providers::thread::one_line(&body), 90)),
                    url: format!("https://x.com/{user}/status/{id}"),
                    snippet: format!("{body} ({likes} likes, {replies} replies)"),
                    source: "xbird".into(),
                    published: t["createdAt"].as_str().map(str::to_string),
                    image_url: None,
                    extra: Some(serde_json::json!({ "author": user, "likes": likes, "replies": replies })),
                })
            })
            .collect();
        // X's matching is loose: keep posts that carry the question's words.
        let results = crate::relevance::keep(results, call.query);
        Ok(Output {
            results,
            answers: Vec::new(),
            cost_micros: Some(0),
        })
    }
}
