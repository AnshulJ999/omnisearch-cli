//! `github` and `ghissues`: GitHub search through the logged-in `gh` CLI.
//! `github` finds repositories, `ghissues` issues and pull requests.

use super::spec::{FilterSupport, Price};
use super::{Adapter, Call, Output, ProviderSpec};
use crate::errors::SearchError;
use crate::tools::{self, Tool};
use crate::types::{FailureCategory, Mode, SearchResult, Transport};
use async_trait::async_trait;
use std::time::Duration;

const FILTERS: FilterSupport = FilterSupport {
    freshness: true,
    domains: false,
    country: false,
    lang: false,
};
const FREE: Price = Price {
    per_call: 0,
    per_extra_result: 0,
    included_results: 0,
};

pub static SPEC: ProviderSpec = ProviderSpec {
    name: "github",
    env_keys: &[],
    modes: &[Mode::Code, Mode::Deep],
    filters: FILTERS,
    max_count: 30,
    timeout: Duration::from_secs(20),
    billed_on_send: false,
    price: FREE,
};

pub static ISSUES_SPEC: ProviderSpec = ProviderSpec {
    name: "ghissues",
    env_keys: &[],
    modes: &[Mode::Code, Mode::Community],
    filters: FILTERS,
    max_count: 30,
    timeout: Duration::from_secs(20),
    billed_on_send: false,
    price: FREE,
};

pub struct GitHub {
    issues: bool,
}

impl GitHub {
    pub fn repos() -> Self {
        Self { issues: false }
    }
    pub fn issues() -> Self {
        Self { issues: true }
    }
}

fn fail(provider: &'static str, category: FailureCategory, message: String) -> SearchError {
    SearchError::Upstream {
        provider,
        status: 0,
        category,
        message,
    }
}

fn since(freshness: Option<&str>) -> Option<String> {
    let days = super::freshness_days(freshness?)?;
    Some(super::date_days_ago(days))
}

#[async_trait]
impl Adapter for GitHub {
    fn spec(&self) -> &'static ProviderSpec {
        if self.issues {
            &ISSUES_SPEC
        } else {
            &SPEC
        }
    }

    fn is_configured(&self) -> bool {
        tools::locate(Tool::Gh).is_some()
    }

    fn transport(&self) -> Transport {
        Transport::Local
    }

    async fn call(&self, call: &Call<'_>) -> Result<Output, SearchError> {
        let out = self.search(call, call.query).await?;
        // GitHub ANDs every word: a sentence-like query often finds nothing,
        // so retry once with its key words.
        if out.results.is_empty() {
            let short = key_words(call.query);
            if !short.is_empty() && short != call.query {
                return self.search(call, &short).await;
            }
        }
        Ok(out)
    }
}

/// The query without filler words, at most four words.
fn key_words(query: &str) -> String {
    const FILLER: &[&str] = &[
        "a", "an", "the", "in", "on", "for", "with", "of", "to", "and", "or", "written", "using",
        "based", "that", "which", "best", "good", "how", "what", "is", "are", "library", "tool",
        "tools", "alternative", "alternatives", "vs",
    ];
    query
        .split_whitespace()
        .filter(|w| !FILLER.contains(&w.to_lowercase().as_str()))
        .take(4)
        .collect::<Vec<_>>()
        .join(" ")
}

/// A bare year leaves the query: GitHub matches a number like "2026" against
/// issue and PR numbers. It comes back as an updated-since date.
fn split_year(query: &str) -> (String, Option<u32>) {
    let mut year = None;
    let words: Vec<&str> = query
        .split_whitespace()
        .filter(|w| {
            let y = w.len() == 4 && w.chars().all(|c| c.is_ascii_digit());
            match w.parse::<u32>() {
                Ok(n) if y && (1990..=2100).contains(&n) => {
                    year = year.max(Some(n));
                    false
                }
                _ => true,
            }
        })
        .collect();
    if words.is_empty() {
        return (query.to_string(), None);
    }
    (words.join(" "), year)
}

impl GitHub {
    async fn search(&self, call: &Call<'_>, query: &str) -> Result<Output, SearchError> {
        let name = self.spec().name;
        let (query, year) = split_year(query);
        let query = query.as_str();
        let limit = call.count.min(self.spec().max_count).to_string();
        // Each word its own argument: gh searches one quoted argument as a phrase.
        let words: Vec<String> = query.split_whitespace().map(str::to_string).collect();
        let mut args: Vec<String> = if self.issues {
            vec!["search".into(), "issues".into()]
        } else {
            vec!["search".into(), "repos".into()]
        };
        args.extend(words);
        args.extend::<Vec<String>>(if self.issues {
            vec![
                "--include-prs".into(),
                "--json".into(),
                "title,url,body,repository,state,commentsCount,updatedAt,isPullRequest".into(),
                "--sort".into(),
                "comments".into(),
            ]
        } else {
            vec![
                "--json".into(),
                "fullName,url,description,stargazersCount,updatedAt,pushedAt,isArchived,language".into(),
                "--sort".into(),
                "stars".into(),
            ]
        });
        args.push("--limit".into());
        args.push(limit);
        if let Some(d) = since(call.opts.freshness.as_deref()) {
            args.push("--updated".into());
            args.push(format!(">={d}"));
        } else if let Some(y) = year {
            args.push("--updated".into());
            args.push(format!(">={y}-01-01"));
        }
        let ran = tools::run(Tool::Gh, &args, call.timeout)
            .await
            .map_err(|e| fail(name, FailureCategory::Config, e))?;
        if ran.timed_out {
            return Err(fail(name, FailureCategory::Timeout, "gh timed out".into()));
        }
        if ran.code != Some(0) {
            let err = ran.stderr.trim().to_string();
            let category = if err.contains("auth") || err.contains("login") {
                FailureCategory::Auth
            } else if err.contains("rate limit") {
                FailureCategory::RateLimit
            } else {
                FailureCategory::Server
            };
            return Err(fail(name, category, err));
        }
        let list: Vec<serde_json::Value> = serde_json::from_slice(&ran.stdout).map_err(|e| {
            tools::contract_warning("gh search", &format!("output is not a JSON array: {e}"));
            fail(name, FailureCategory::Parse, e.to_string())
        })?;
        let results = list
            .iter()
            .filter_map(|v| {
                let url = v["url"].as_str()?.to_string();
                Some(if self.issues {
                    let repo = v["repository"]["nameWithOwner"].as_str().unwrap_or("");
                    let kind = if v["isPullRequest"].as_bool() == Some(true) {
                        "PR"
                    } else {
                        "issue"
                    };
                    let body = v["body"].as_str().unwrap_or("");
                    SearchResult {
                        title: format!("{repo} {kind}: {}", v["title"].as_str().unwrap_or("")),
                        url,
                        snippet: super::thread::clip(body, 400),
                        source: name.into(),
                        published: v["updatedAt"].as_str().map(str::to_string),
                        image_url: None,
                        extra: Some(serde_json::json!({
                            "state": v["state"],
                            "comments": v["commentsCount"],
                        })),
                    }
                } else {
                    SearchResult {
                        title: v["fullName"].as_str().unwrap_or("").to_string(),
                        url,
                        snippet: v["description"].as_str().unwrap_or("").to_string(),
                        source: name.into(),
                        published: v["updatedAt"].as_str().map(str::to_string),
                        image_url: None,
                        extra: Some(serde_json::json!({
                            "stars": v["stargazersCount"],
                            "language": v["language"],
                            "pushed": v["pushedAt"],
                            "archived": v["isArchived"],
                        })),
                    }
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
