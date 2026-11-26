//! `donsetch`: donsetch's keyless search (10+ engines merged by consensus),
//! run as an outside program. A different result set from the keyed engines,
//! at no cost.

use super::spec::{FilterSupport, Price};
use super::{Adapter, Call, Output, ProviderSpec};
use crate::errors::SearchError;
use crate::tools::{self, Tool};
use crate::types::{FailureCategory, Mode, SearchResult, Transport};
use async_trait::async_trait;
use std::time::Duration;

pub static SPEC: ProviderSpec = ProviderSpec {
    name: "donsetch",
    env_keys: &[],
    modes: &[Mode::General, Mode::News, Mode::Academic, Mode::Deep, Mode::Code],
    filters: FilterSupport {
        freshness: false,
        domains: true,
        country: false,
        lang: false,
    },
    max_count: 12,
    timeout: Duration::from_secs(25),
    billed_on_send: false,
    price: Price {
        per_call: 0,
        per_extra_result: 0,
        included_results: 0,
    },
};

pub struct Donsetch;

fn intent(mode: Mode) -> &'static str {
    match mode {
        Mode::News => "news",
        Mode::Academic => "paper",
        Mode::Code => "code",
        _ => "auto",
    }
}

/// donsetch's keyless Exa and Parallel sources, switched off when osearch
/// asks those engines itself: one engine's answer reached two ways would
/// otherwise count as two engines agreeing.
fn own_engine_overlap() -> Vec<(&'static str, &'static str)> {
    let config = crate::config::get();
    let mut env = Vec::new();
    if !super::key_for(config, &super::exa::SPEC).is_empty() {
        env.push(("DONSETCH_SEARCH__EXA_MCP", "false"));
    }
    if !super::key_for(config, &super::parallel::SPEC).is_empty() {
        env.push(("DONSETCH_SEARCH__PARALLEL_MCP", "false"));
    }
    env
}

fn fail(category: FailureCategory, message: String) -> SearchError {
    SearchError::Upstream {
        provider: "donsetch",
        status: 0,
        category,
        message,
    }
}

/// The result list of `donsetch search --json`: `meta.results[]` with
/// title, url, snippet; score, consensus and engines when present.
pub fn parse(body: &serde_json::Value, source: &str) -> Result<Vec<SearchResult>, String> {
    let list = body
        .pointer("/meta/results")
        .and_then(|v| v.as_array())
        .ok_or("no meta.results array")?;
    Ok(list
        .iter()
        .filter_map(|r| {
            let url = r.get("url")?.as_str()?.to_string();
            url.starts_with("http").then_some(())?;
            let mut extra = serde_json::Map::new();
            for k in ["score", "consensus", "engines"] {
                if let Some(v) = r.get(k) {
                    extra.insert(k.to_string(), v.clone());
                }
            }
            Some(SearchResult {
                title: r["title"].as_str().unwrap_or_default().to_string(),
                url,
                snippet: r["snippet"].as_str().unwrap_or_default().to_string(),
                source: source.to_string(),
                published: r["published"].as_str().map(str::to_string),
                image_url: None,
                extra: (!extra.is_empty()).then(|| serde_json::Value::Object(extra)),
            })
        })
        .collect())
}

#[async_trait]
impl Adapter for Donsetch {
    fn spec(&self) -> &'static ProviderSpec {
        &SPEC
    }

    fn is_configured(&self) -> bool {
        tools::locate(Tool::Donsetch).is_some()
    }

    fn transport(&self) -> Transport {
        Transport::Local
    }

    async fn call(&self, call: &Call<'_>) -> Result<Output, SearchError> {
        let query = super::augment_query(call.query, call.opts);
        let deadline_ms = call.timeout.as_millis().clamp(500, 600_000) as u64;
        let args: Vec<String> = vec![
            "search".into(),
            query,
            "--json".into(),
            "-q".into(),
            "--max-results".into(),
            call.count.min(SPEC.max_count).to_string(),
            "--intent".into(),
            intent(call.mode).into(),
            "--deadline-ms".into(),
            deadline_ms.saturating_sub(500).max(500).to_string(),
        ];
        let ran = tools::run_env(Tool::Donsetch, &args, call.timeout + Duration::from_secs(2), &own_engine_overlap())
            .await
            .map_err(|e| fail(FailureCategory::Config, e))?;
        if ran.timed_out {
            return Err(fail(FailureCategory::Timeout, "donsetch search timed out".into()));
        }
        let text = ran.stdout_text();
        let body: serde_json::Value = match serde_json::from_str(text.trim()) {
            Ok(v) => v,
            Err(e) => {
                tools::contract_warning(
                    "donsetch search",
                    &format!("output is not JSON (exit {:?}): {e}", ran.code),
                );
                return Err(fail(FailureCategory::Parse, format!("not JSON: {e}")));
            }
        };
        if body["ok"].as_bool() == Some(false) {
            let msg = body
                .pointer("/error/message")
                .and_then(|v| v.as_str())
                .unwrap_or("search failed")
                .to_string();
            return Err(fail(
                match ran.code {
                    Some(2) => FailureCategory::Timeout,
                    _ => FailureCategory::Server,
                },
                msg,
            ));
        }
        let source = if call.mode == Mode::News {
            "donsetch_news"
        } else {
            "donsetch"
        };
        let results = parse(&body, source).map_err(|e| {
            tools::contract_warning("donsetch search", &e);
            fail(FailureCategory::Parse, e)
        })?;
        Ok(Output {
            results,
            answers: Vec::new(),
            cost_micros: Some(0),
        })
    }
}
