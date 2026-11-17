//! `youcom`: You.com Search API (`ydc-index.io/v1/search`), web and news hits
//! with several snippets each.

use super::spec::{FilterSupport, Price};
use super::{Adapter, Call, Output, ProviderSpec};
use crate::context::AppContext;
use crate::errors::SearchError;
use crate::types::{Mode, SearchResult};
use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

pub static SPEC: ProviderSpec = ProviderSpec {
    name: "youcom",
    env_keys: &["YDC_API_KEY", "YOU_COM_API_KEY"],
    modes: &[Mode::General, Mode::News, Mode::Deep],
    filters: FilterSupport {
        freshness: true,
        domains: true,
        country: true,
        lang: false,
    },
    max_count: 20,
    timeout: Duration::from_secs(15),
    billed_on_send: true,
    price: Price {
        per_call: crate::config::YOUCOM_PER_CALL_MICROS,
        per_extra_result: 0,
        included_results: 0,
    },
};

pub struct YouCom {
    ctx: Arc<AppContext>,
}

impl YouCom {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }

    fn api_key(&self) -> String {
        SPEC.env_keys
            .iter()
            .map(|env| super::resolve_key(self.ctx.config.keys.get("youcom").unwrap_or(""), env))
            .find(|k| !k.is_empty())
            .unwrap_or_default()
    }
}

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    results: Results,
}

#[derive(Deserialize, Default)]
struct Results {
    #[serde(default)]
    web: Vec<Hit>,
    #[serde(default)]
    news: Vec<Hit>,
}

#[derive(Deserialize)]
struct Hit {
    url: Option<String>,
    title: Option<String>,
    description: Option<String>,
    #[serde(default)]
    snippets: Vec<String>,
    page_age: Option<String>,
    thumbnail_url: Option<String>,
}

fn to_result(h: Hit, source: &str) -> Option<SearchResult> {
    let url = h.url.filter(|u| u.starts_with("http"))?;
    let mut snippet = h.description.unwrap_or_default();
    for s in h.snippets {
        if !snippet.contains(s.trim()) {
            if !snippet.is_empty() {
                snippet.push_str(" … ");
            }
            snippet.push_str(s.trim());
        }
    }
    Some(SearchResult {
        title: h.title.unwrap_or_default(),
        url,
        snippet,
        source: source.to_string(),
        published: h.page_age.filter(|d| !d.is_empty()),
        image_url: h.thumbnail_url,
        extra: None,
    })
}

#[async_trait]
impl Adapter for YouCom {
    fn spec(&self) -> &'static ProviderSpec {
        &SPEC
    }

    fn is_configured(&self) -> bool {
        !self.api_key().is_empty()
    }

    async fn call(&self, call: &Call<'_>) -> Result<Output, SearchError> {
        let key = self.api_key();
        if key.is_empty() {
            return Err(SearchError::AuthMissing { provider: "youcom" });
        }
        let query = super::augment_query(call.query, call.opts);
        let count = call.count.min(SPEC.max_count).to_string();
        let mut params: Vec<(&str, &str)> = vec![("query", &query), ("count", &count)];
        if let Some(f) = call.opts.freshness.as_deref() {
            params.push(("freshness", f));
        }
        let country = call.opts.country.as_deref().map(str::to_ascii_uppercase);
        if let Some(c) = country.as_deref() {
            params.push(("country", c));
        }
        let client = &self.ctx.client;
        let resp = super::send(self, call.timeout, || {
            client
                .get("https://ydc-index.io/v1/search")
                .header("X-API-Key", &key)
                .query(&params)
        })
        .await?;
        let body: Response = resp.json().await?;
        let (first, second, source) = if call.mode == Mode::News {
            (body.results.news, body.results.web, "youcom_news")
        } else {
            (body.results.web, body.results.news, "youcom")
        };
        let results = first
            .into_iter()
            .chain(second)
            .filter_map(|h| to_result(h, source))
            .take(call.count as usize)
            .collect();
        Ok(Output {
            results,
            answers: Vec::new(),
            cost_micros: None,
        })
    }
}
