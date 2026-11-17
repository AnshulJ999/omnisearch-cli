//! `youtube`: YouTube video search through yt-dlp (`ytsearchN:`), keyless.
//! Transcripts are read by the fetch tiers (tier 0), not here.

use super::spec::{FilterSupport, Price};
use super::{Adapter, Call, Output, ProviderSpec};
use crate::errors::SearchError;
use crate::tools::{self, Tool};
use crate::types::{FailureCategory, Mode, SearchResult, Transport};
use async_trait::async_trait;
use std::time::Duration;

pub static SPEC: ProviderSpec = ProviderSpec {
    name: "youtube",
    env_keys: &[],
    modes: &[Mode::Social],
    filters: FilterSupport {
        freshness: false,
        domains: false,
        country: false,
        lang: false,
    },
    max_count: 20,
    timeout: Duration::from_secs(30),
    billed_on_send: false,
    price: Price {
        per_call: 0,
        per_extra_result: 0,
        included_results: 0,
    },
};

pub struct YouTube;

fn fail(category: FailureCategory, message: String) -> SearchError {
    SearchError::Upstream {
        provider: "youtube",
        status: 0,
        category,
        message,
    }
}

#[async_trait]
impl Adapter for YouTube {
    fn spec(&self) -> &'static ProviderSpec {
        &SPEC
    }

    fn is_configured(&self) -> bool {
        tools::locate(Tool::YtDlp).is_some()
    }

    fn transport(&self) -> Transport {
        Transport::Local
    }

    async fn call(&self, call: &Call<'_>) -> Result<Output, SearchError> {
        let n = call.count.min(SPEC.max_count);
        let args: Vec<String> = vec![
            format!("ytsearch{n}:{}", call.query),
            "--flat-playlist".into(),
            "-J".into(),
            "--no-warnings".into(),
        ];
        let ran = tools::run(Tool::YtDlp, &args, call.timeout)
            .await
            .map_err(|e| fail(FailureCategory::Config, e))?;
        if ran.timed_out {
            return Err(fail(FailureCategory::Timeout, "yt-dlp timed out".into()));
        }
        let body: serde_json::Value = serde_json::from_slice(&ran.stdout).map_err(|e| {
            tools::contract_warning("yt-dlp search", &format!("output is not JSON: {e}"));
            fail(FailureCategory::Parse, format!("{e}; {}", ran.stderr.trim()))
        })?;
        let entries = body["entries"].as_array().ok_or_else(|| {
            tools::contract_warning("yt-dlp search", "no entries array");
            fail(FailureCategory::Parse, "no entries".into())
        })?;
        let results = entries
            .iter()
            .filter_map(|e| {
                let id = e["id"].as_str()?;
                let url = e["url"]
                    .as_str()
                    .filter(|u| u.starts_with("http"))
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("https://www.youtube.com/watch?v={id}"));
                let channel = e["channel"].as_str().or(e["uploader"].as_str()).unwrap_or("");
                let mut snippet = e["description"].as_str().unwrap_or("").to_string();
                if snippet.is_empty() {
                    snippet = format!("YouTube video by {channel}");
                }
                Some(SearchResult {
                    title: e["title"].as_str().unwrap_or("").to_string(),
                    url,
                    snippet,
                    source: "youtube".into(),
                    published: None,
                    image_url: None,
                    extra: Some(serde_json::json!({
                        "channel": channel,
                        "duration_s": e["duration"],
                        "views": e["view_count"],
                    })),
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
