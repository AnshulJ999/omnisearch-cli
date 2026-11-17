//! `bluesky`: Bluesky post search (AT Protocol `app.bsky.feed.searchPosts`)
//! with an app password (`BSKY_HANDLE`, `BSKY_APP_PASSWORD`). The public
//! mirror refuses search, so each call opens a session at bsky.social first.

use super::spec::{FilterSupport, Price};
use super::{Adapter, Call, Output, ProviderSpec};
use crate::context::AppContext;
use crate::errors::SearchError;
use crate::types::{FailureCategory, Mode, SearchResult};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

pub static SPEC: ProviderSpec = ProviderSpec {
    name: "bluesky",
    env_keys: &["BSKY_HANDLE", "BSKY_APP_PASSWORD"],
    modes: &[Mode::Social],
    filters: FilterSupport {
        freshness: false,
        domains: false,
        country: false,
        lang: false,
    },
    max_count: 50,
    timeout: Duration::from_secs(20),
    billed_on_send: false,
    price: Price {
        per_call: 0,
        per_extra_result: 0,
        included_results: 0,
    },
};

pub struct Bluesky {
    ctx: Arc<AppContext>,
}

impl Bluesky {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }
}

fn creds() -> Option<(String, String)> {
    let get = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    Some((get("BSKY_HANDLE")?, get("BSKY_APP_PASSWORD")?))
}

fn fail(category: FailureCategory, message: String) -> SearchError {
    SearchError::Upstream {
        provider: "bluesky",
        status: 0,
        category,
        message,
    }
}

#[async_trait]
impl Adapter for Bluesky {
    fn spec(&self) -> &'static ProviderSpec {
        &SPEC
    }

    fn is_configured(&self) -> bool {
        creds().is_some()
    }

    async fn call(&self, call: &Call<'_>) -> Result<Output, SearchError> {
        let (handle, password) = creds().ok_or_else(|| fail(FailureCategory::Config, "BSKY_HANDLE / BSKY_APP_PASSWORD not set".into()))?;
        // The session call stays out of the run archive: its answer holds tokens.
        let session = crate::run::raw::scope(
            None,
            super::send(self, call.timeout, || {
                self.ctx
                    .client
                    .post("https://bsky.social/xrpc/com.atproto.server.createSession")
                    .json(&serde_json::json!({ "identifier": handle, "password": password }))
            }),
        )
        .await?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| fail(FailureCategory::Parse, e.to_string()))?;
        let jwt = session["accessJwt"].as_str().ok_or_else(|| fail(FailureCategory::Auth, "Bluesky refused the app password".into()))?.to_string();
        let n = call.count.min(SPEC.max_count).to_string();
        let body = super::send(self, call.timeout, || {
            self.ctx
                .client
                .get("https://api.bsky.app/xrpc/app.bsky.feed.searchPosts")
                .bearer_auth(&jwt)
                .query(&[("q", call.query), ("limit", n.as_str()), ("sort", "top")])
        })
        .await?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| fail(FailureCategory::Parse, e.to_string()))?;
        let results = body["posts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                let author = p["author"]["handle"].as_str()?;
                let rkey = p["uri"].as_str()?.rsplit('/').next()?;
                let text = p["record"]["text"].as_str().unwrap_or("").to_string();
                let likes = p["likeCount"].as_i64().unwrap_or(0);
                let replies = p["replyCount"].as_i64().unwrap_or(0);
                Some(SearchResult {
                    title: format!("@{author}: {}", crate::providers::thread::clip(&crate::providers::thread::one_line(&text), 90)),
                    url: format!("https://bsky.app/profile/{author}/post/{rkey}"),
                    snippet: format!("{text} ({likes} likes, {replies} replies)"),
                    source: "bluesky".into(),
                    published: p["record"]["createdAt"].as_str().map(str::to_string),
                    image_url: None,
                    extra: Some(serde_json::json!({ "author": author, "likes": likes, "replies": replies })),
                })
            })
            .collect();
        Ok(Output {
            results,
            answers: Vec::new(),
            cost_micros: Some(0),
        })
    }
}
