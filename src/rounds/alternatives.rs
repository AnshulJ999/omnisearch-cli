//! "Alternatives to X": GitHub repos sharing X's topics (sorted by stars),
//! and pages like X's own site (Exa) when X is a product rather than a repo.
//! Skipped, with a note, when X's page can't be identified.

use super::{squash, Answer, Responses, RoundOut, Slots};
use crate::context::AppContext;
use crate::research::{search_cached, Planned};
use crate::run::raw;
use crate::types::{Mode, SearchOpts, SearchResponse};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Alternatives share at most a third of a run's pages with the question.
const SLOTS: Slots = Slots { extra: true, cap: None, guaranteed: false };

/// The X of "alternatives to X", "X alternatives", "tools like X",
/// "similar to X", "replacement for X".
pub fn subject(query: &str) -> Option<String> {
    static PATTERNS: std::sync::LazyLock<Vec<regex::Regex>> = std::sync::LazyLock::new(|| {
        [
            r"(?i)\balternatives?\s+(?:to|for)\s+(.+)",
            r"(?i)\b(?:replacements?|substitutes?)\s+(?:for|to)\s+(.+)",
            r"(?i)\b(?:tools?|librar(?:y|ies)|apps?|projects?|software|frameworks?|services?|sites?|crates?|packages?)\s+(?:like|similar to)\s+(.+)",
            r"(?i)\bsimilar\s+to\s+(.+)",
            r"(?i)^(.+?)\s+alternatives?\b",
        ]
        .iter()
        .map(|p| regex::Regex::new(p).expect("regex"))
        .collect()
    });
    const LEAD: &[&str] = &["best", "top", "free", "good", "open", "source", "open-source", "self-hosted", "cheaper", "cheap", "the", "a", "any"];
    for re in PATTERNS.iter() {
        let Some(c) = re.captures(query) else { continue };
        let mut x = c[1].to_lowercase();
        for cut in [" for ", " in ", " with ", " that ", " which ", " on ", " vs ", ",", "?", "("] {
            if let Some(i) = x.find(cut) {
                x.truncate(i);
            }
        }
        let words: Vec<&str> = x
            .split_whitespace()
            .map(|w| w.trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '-' && ch != '.'))
            .filter(|w| !w.is_empty() && !LEAD.contains(w))
            .take(3)
            .collect();
        let x = words.join(" ");
        if x.chars().filter(|c| c.is_alphanumeric()).count() >= 2 {
            return Some(x);
        }
    }
    None
}

/// X's own page among the results: its GitHub repo (preferred), else a site
/// whose name is X. Returns (url, Some((owner, repo)) for a repo).
fn subject_page(x: &str, responses: &Responses) -> Option<(String, Option<(String, String)>)> {
    let want = squash(x);
    let urls: Vec<&str> = responses
        .values()
        .filter_map(|r| r.as_ref().ok())
        .flat_map(|r| r.per_provider.iter().flat_map(|(_, v)| v.iter()).chain(r.results.iter()))
        .map(|it| it.url.as_str())
        .collect();
    for u in &urls {
        if let Some((owner, repo)) = super::github::repo_of(u) {
            if squash(&repo) == want {
                return Some((format!("https://github.com/{owner}/{repo}"), Some((owner, repo))));
            }
        }
    }
    for u in &urls {
        let Ok(parsed) = url::Url::parse(u) else { continue };
        let Some(host) = parsed.host_str() else { continue };
        let host = host.trim_start_matches("www.");
        let name = host.split('.').next().unwrap_or("");
        if !name.is_empty() && squash(name) == want {
            return Some((format!("https://{host}/"), None));
        }
    }
    None
}

/// What X's site says about itself: the longest snippet among the results on
/// its host that is not a table (the title when none is), without its name or links.
fn site_description(x: &str, url: &str, responses: &Responses) -> Option<String> {
    let host = url::Url::parse(url).ok()?.host_str()?.trim_start_matches("www.").to_string();
    let name = squash(x);
    let rows: Vec<&crate::types::SearchResult> = responses
        .values()
        .filter_map(|r| r.as_ref().ok())
        .flat_map(|r| r.per_provider.iter().flat_map(|(_, v)| v.iter()).chain(r.results.iter()))
        .filter(|r| url::Url::parse(&r.url).ok().and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.") == host)).unwrap_or(false))
        .collect();
    let text = rows
        .iter()
        .map(|r| r.snippet.as_str())
        .filter(|s| !s.contains('|'))
        .max_by_key(|s| s.len())
        .filter(|s| !s.is_empty())
        .or_else(|| rows.first().map(|r| r.title.as_str()))?;
    let words: Vec<&str> = text
        .split_whitespace()
        .filter(|w| !w.contains('/') && (name.len() < 3 || !squash(w).contains(&name)))
        .take(30)
        .collect();
    (words.len() >= 4).then(|| words.join(" "))
}

/// Results that are X itself (its repo, forks named after it, mirrors,
/// its docs) leave the alternatives searches.
fn drop_subject(resp: &mut SearchResponse, x: &str) {
    let x = squash(x);
    if x.len() < 4 {
        return;
    }
    let keep = |it: &crate::types::SearchResult| !squash(&it.url).contains(&x) && !squash(&it.title).contains(&x);
    resp.results.retain(keep);
    for (_, items) in resp.per_provider.iter_mut() {
        items.retain(keep);
    }
}

pub async fn round(
    app: &Arc<AppContext>,
    recorder: &raw::Recorder,
    planned: &[Planned],
    responses: &Responses,
    no_cache: bool,
) -> RoundOut {
    let mut out = RoundOut::default();
    let exa = crate::providers::build_providers(app)
        .into_iter()
        .any(|a| a.name() == "exa" && a.is_configured());
    for p in planned.iter().filter(|p| !p.side) {
        let Some(x) = subject(&p.query) else { continue };
        let mut page = subject_page(&x, responses);
        if page.is_none() {
            let engines = Some(vec!["github".to_string()]);
            if let Ok(resp) = search_cached(app.clone(), &x, Mode::Code, 5, &engines, &SearchOpts::default(), no_cache).await {
                let one: Responses = BTreeMap::from([("gh".to_string(), Ok(resp))]);
                page = subject_page(&x, &one);
            }
        }
        let Some((url, repo)) = page else {
            out.notes.push(format!(
                "{}: alternatives searches skipped: couldn't identify the page of '{x}' (try `osearch search -q <its url> -m similar`)",
                p.label
            ));
            continue;
        };
        let mut jobs: Vec<(String, String, Mode, &str)> = Vec::new();
        let mut how = "pages like it";
        match &repo {
            // A repo: Exa's find-similar returns the repo itself; its topics
            // work, else the words of its own description.
            Some((owner, name)) => {
                for (i, t) in super::github::topics(owner, name).await.into_iter().enumerate() {
                    jobs.push((format!("{}xt{}", p.label, i + 1), format!("topic:{t}"), Mode::Code, "github"));
                }
                how = "GitHub repos sharing its topics";
                if jobs.is_empty() {
                    // The repo's own description (the github engine's row), in plain words.
                    let rows: Vec<&crate::types::SearchResult> = responses
                        .values()
                        .filter_map(|r| r.as_ref().ok())
                        .flat_map(|r| r.per_provider.iter().flat_map(|(_, v)| v.iter()).chain(r.results.iter()))
                        .filter(|r| super::github::repo_of(&r.url).is_some_and(|(o, n)| o == *owner && n == *name) && !r.snippet.is_empty())
                        .collect();
                    let described = rows
                        .iter()
                        .find(|r| r.source == "github")
                        .or_else(|| rows.first())
                        .map(|r| {
                            let plain: String = r
                                .snippet
                                .split_whitespace()
                                .filter(|w| !w.contains('/') && w.chars().any(|c| c.is_alphanumeric()))
                                .collect::<Vec<_>>()
                                .join(" ");
                            super::carrying_words(&plain, 4)
                        })
                        .filter(|w| !w.is_empty());
                    if let Some(words) = described {
                        jobs.push((format!("{}xd", p.label), words, Mode::Code, "github"));
                        how = "GitHub repos described like it";
                    }
                }
            }
            None if exa => {
                jobs.push((format!("{}xs", p.label), url.clone(), Mode::Similar, "exa"));
                // Find-similar on a product's own site returns pages about the
                // product, so its description is searched too.
                if let Some(desc) = site_description(&x, &url, responses) {
                    jobs.push((format!("{}xd", p.label), format!("tools similar to one described as: {desc}"), Mode::General, "exa"));
                    how = "pages like it and pages described like it";
                }
            }
            None => {}
        }
        if jobs.is_empty() {
            out.notes.push(format!("{}: alternatives of '{x}' ({url}): nothing to search by (no topics or description)", p.label));
            continue;
        }
        let mut set = tokio::task::JoinSet::new();
        for (label, query, mode, engine) in &jobs {
            let app = app.clone();
            let rec = Some(recorder.labeled(label));
            let (label, query, mode, engines) = (label.clone(), query.clone(), *mode, Some(vec![engine.to_string()]));
            set.spawn(raw::scope(rec, async move {
                let r: Answer = search_cached(app, &query, mode, 15, &engines, &SearchOpts::default(), no_cache).await;
                (label, r)
            }));
        }
        while let Some(joined) = set.join_next().await {
            if let Ok((label, mut r)) = joined {
                if let Ok(resp) = r.as_mut() {
                    drop_subject(resp, &x);
                }
                out.responses.push((label, r));
            }
        }
        for (label, query, mode, engine) in jobs {
            out.planned.push(Planned::local(p, label, query, mode, engine, SLOTS));
        }
        out.coverage.push(format!("{}: alternatives of '{x}' ({url}): also searched {how}", p.label));
    }
    out
}
