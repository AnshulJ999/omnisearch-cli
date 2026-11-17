//! `devto`: developer articles on dev.to (Forem's public API, keyless).
//! Its text search matches loosely, so the engine asks for the question's
//! likely tags (the main words joined, then each alone) and keeps only
//! articles on topic, most reacted first.

use super::spec::{FilterSupport, Price};
use super::{Adapter, Call, Output, ProviderSpec};
use crate::context::AppContext;
use crate::errors::SearchError;
use crate::types::{Mode, SearchResult};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

pub static SPEC: ProviderSpec = ProviderSpec {
    name: "devto",
    env_keys: &[],
    modes: &[Mode::Community],
    filters: FilterSupport {
        freshness: false,
        domains: false,
        country: false,
        lang: false,
    },
    max_count: 30,
    timeout: Duration::from_secs(15),
    billed_on_send: false,
    price: Price {
        per_call: 0,
        per_extra_result: 0,
        included_results: 0,
    },
};

pub struct DevTo {
    ctx: Arc<AppContext>,
}

impl DevTo {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }
}

/// Candidate tags: the first two main words joined, then each main word.
fn tags(query: &str) -> Vec<String> {
    let words: Vec<String> = crate::rounds::carrying_words(query, 3)
        .split_whitespace()
        .map(|w| w.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>())
        .filter(|w| w.len() >= 3)
        .collect();
    let mut out = Vec::new();
    if words.len() >= 2 {
        out.push(format!("{}{}", words[0], words[1]));
    }
    out.extend(words.into_iter().take(2));
    out.dedup();
    out
}

#[async_trait]
impl Adapter for DevTo {
    fn spec(&self) -> &'static ProviderSpec {
        &SPEC
    }

    fn is_configured(&self) -> bool {
        true
    }

    async fn call(&self, call: &Call<'_>) -> Result<Output, SearchError> {
        let mut found: Vec<(i64, SearchResult)> = Vec::new();
        for tag in tags(call.query) {
            let resp = super::send(self, call.timeout, || {
                self.ctx
                    .client
                    .get("https://dev.to/api/articles")
                    .query(&[("tag", tag.as_str()), ("top", "365"), ("per_page", "30")])
            })
            .await?;
            let list: serde_json::Value = resp.json().await.unwrap_or_default();
            for a in list.as_array().into_iter().flatten() {
                let Some(url) = a["url"].as_str() else { continue };
                if found.iter().any(|(_, r)| r.url == url) {
                    continue;
                }
                let reactions = a["positive_reactions_count"].as_i64().unwrap_or(0);
                let comments = a["comments_count"].as_i64().unwrap_or(0);
                found.push((
                    reactions + 3 * comments,
                    SearchResult {
                        title: a["title"].as_str().unwrap_or("").to_string(),
                        url: url.to_string(),
                        snippet: format!(
                            "{} ({reactions} reactions, {comments} comments; tags: {})",
                            a["description"].as_str().unwrap_or(""),
                            a["tag_list"].as_array().map(|t| t.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default()
                        ),
                        source: "devto".into(),
                        published: a["published_at"].as_str().map(str::to_string),
                        image_url: None,
                        extra: Some(serde_json::json!({ "reactions": reactions, "comments": comments })),
                    },
                ));
            }
            if found.len() >= call.count as usize {
                break;
            }
        }
        let q = crate::relevance::Query::new(call.query);
        found.retain(|(_, r)| q.score(&format!("{} {}", r.title, r.snippet)) >= crate::relevance::FLOOR);
        found.sort_by(|a, b| b.0.cmp(&a.0));
        Ok(Output {
            results: found.into_iter().take(call.count as usize).map(|(_, r)| r).collect(),
            answers: Vec::new(),
            cost_micros: Some(0),
        })
    }
}
