//! GitHub beside the web engines: a repo search on general, community and
//! deep questions (strict matches only, listed apart in the summary), and in
//! code mode the top repo's topics, recently active projects, awesome lists
//! and, for "examples of X" questions, code search.

use super::{carrying_words, local_response, plan_side, squash, Responses, RoundOut, Slots, SAME};
use crate::context::AppContext;
use crate::research::{search_cached, Planned, QuerySpec, Request};
use crate::run::raw;
use crate::tools::{self, Tool};
use crate::types::{Mode, SearchOpts, SearchResponse, SearchResult};
use std::sync::Arc;
use std::time::Duration;


/// The GitHub list beside a web question: at most two of its pages fetched.
const LIST: Slots = Slots { extra: true, cap: Some(2), guaranteed: false };

/// `owner/repo` of a repository's own page.
pub fn repo_of(url: &str) -> Option<(String, String)> {
    static REPO: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^https?://(?:www\.)?github\.com/([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+?)(?:\.git)?/?$").expect("regex")
    });
    let c = REPO.captures(url)?;
    let owner = c[1].to_string();
    let skip = ["topics", "sponsors", "orgs", "marketplace", "features", "collections", "search", "about", "settings"];
    (!skip.contains(&owner.to_ascii_lowercase().as_str())).then(|| (owner, c[2].to_string()))
}

/// The GitHub side query of a general, community or deep question
/// (`research.githubbeside`).
pub fn general(
    app: &Arc<AppContext>,
    req: &Request,
    statuses: &[crate::budget::Status],
    p: &Planned,
) -> Option<Planned> {
    let modes: &[Mode] = match crate::config::get().research.githubbeside.as_str() {
        "off" => &[],
        "general" => &[Mode::General, Mode::Deep],
        _ => &[Mode::General, Mode::Community, Mode::Deep],
    };
    if !modes.contains(&p.mode) || tools::locate(Tool::Gh).is_none() {
        return None;
    }
    let words = carrying_words(&p.query, 4);
    if words.is_empty() {
        return None;
    }
    let spec = QuerySpec {
        q: words,
        mode: Some("code".into()),
        engines: vec!["github".into()],
        count: Some(15),
        ..QuerySpec::default()
    };
    plan_side(app, req, statuses, p, "g", spec, LIST)
}

/// Keep only repos that clearly match the question: most of its carrying
/// words in the repo's name or description, and some stars.
pub fn keep_matches(resp: &mut SearchResponse, question: &str) {
    let words: Vec<String> = carrying_words(question, 6)
        .split_whitespace()
        .map(squash)
        .filter(|w| w.len() >= 3)
        .collect();
    // Repos with fewer stars stay out (`research.githubminstars`, 0 = off).
    let min_stars = crate::config::get().research.githubminstars;
    let keep = |r: &SearchResult| {
        let stars = r.extra.as_ref().and_then(|e| e["stars"].as_u64()).unwrap_or(0);
        let hay = squash(&format!("{} {}", r.title, r.snippet));
        let hits = words.iter().filter(|w| hay.contains(w.as_str())).count();
        stars >= min_stars && !words.is_empty() && hits * 2 >= words.len().max(1)
    };
    resp.results.retain(keep);
    for (_, items) in resp.per_provider.iter_mut() {
        items.retain(keep);
    }
    resp.metadata.result_count = resp.results.len();
}

/// One summary line per kept repo: stars, last push, archived.
pub fn listing(r: &SearchResult) -> String {
    let e = r.extra.clone().unwrap_or_default();
    let stars = e["stars"].as_u64().unwrap_or(0);
    let pushed = e["pushed"].as_str().map(|d| d.get(..10).unwrap_or(d).to_string());
    let archived = if e["archived"].as_bool() == Some(true) { " · archived" } else { "" };
    format!(
        "{} ★{stars}{}{archived} {}\n   {}",
        r.title,
        pushed.map(|d| format!(" · pushed {d}")).unwrap_or_default(),
        r.url,
        crate::providers::thread::clip(&r.snippet, 160)
    )
}

/// GitHub topics of a repo that say what it does: generic ones and ones
/// naming the repo itself dropped, two-word topics (web-scraping) first.
pub async fn topics(owner: &str, repo: &str) -> Vec<String> {
    const GENERIC: &[&str] = &[
        "python", "python3", "javascript", "typescript", "rust", "go", "golang", "java", "nodejs", "ai", "llm",
        "llms", "machine-learning", "open-source", "opensource", "api", "cli", "hacktoberfest", "library",
        "framework", "tool", "tools", "artificial-intelligence", "deep-learning", "openai", "chatgpt", "gpt",
        "ai-agents", "ai-agent", "agents", "agent", "agentic-ai", "generative-ai", "mcp", "rag", "markdown",
        "llm-agents", "chatbot", "self-hosted", "docker", "react", "nextjs", "playwright",
    ];
    let own = squash(repo);
    let args: Vec<String> = vec!["api".into(), format!("repos/{owner}/{repo}"), "--jq".into(), ".topics | join(\",\")".into()];
    let Ok(ran) = tools::run(Tool::Gh, &args, Duration::from_secs(15)).await else {
        return Vec::new();
    };
    let mut list: Vec<String> = String::from_utf8_lossy(&ran.stdout)
        .trim()
        .split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty() && !GENERIC.contains(&t.as_str()) && !squash(t).contains(&own))
        .collect();
    list.sort_by_key(|t| !t.contains('-'));
    list.truncate(3);
    list
}

/// The X of "examples of X", "projects using X", "who uses X", "X usage".
/// What a code search looks for: "examples of X" / "projects using X" in any
/// mode; "X examples" / "X usage" only in code mode, where it is safe.
fn code_subject(query: &str, code_mode: bool) -> Option<String> {
    static PATTERNS: std::sync::LazyLock<Vec<regex::Regex>> = std::sync::LazyLock::new(|| {
        [
            r"(?i)\b(?:examples?\s+of|usage\s+of|used\s+by|who\s+uses|(?:projects?|repos?|repositories|apps?)\s+(?:that\s+)?(?:use|uses|using))\s+(.+)",
            r"(?i)^(.+?)\s+(?:examples?|usage|use\s+cases?)\b",
        ]
        .iter()
        .map(|p| regex::Regex::new(p).expect("regex"))
        .collect()
    });
    let n = if code_mode { PATTERNS.len() } else { 1 };
    PATTERNS[..n].iter().find_map(|re| {
        let x = carrying_words(re.captures(query)?.get(1)?.as_str(), 3);
        (x.chars().filter(|c| c.is_alphanumeric()).count() >= 2).then_some(x)
    })
}

/// Repos from `gh search repos`, the rows the `github` engine gives.
async fn repos(words: &str, extra: &[&str], limit: usize) -> Result<Vec<SearchResult>, String> {
    let mut args: Vec<String> = vec!["search".into(), "repos".into()];
    args.extend(words.split_whitespace().map(str::to_string));
    args.extend::<Vec<String>>(vec![
        "--json".into(),
        "fullName,url,description,stargazersCount,pushedAt,isArchived".into(),
        "--sort".into(),
        "stars".into(),
        "--limit".into(),
        limit.to_string(),
    ]);
    args.extend(extra.iter().map(|s| s.to_string()));
    let list = gh_json(args).await?;
    Ok(list
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| {
            Some(SearchResult {
                title: v["fullName"].as_str()?.to_string(),
                url: v["url"].as_str()?.to_string(),
                snippet: v["description"].as_str().unwrap_or("").to_string(),
                source: "github".into(),
                published: v["pushedAt"].as_str().map(str::to_string),
                image_url: None,
                extra: Some(serde_json::json!({
                    "stars": v["stargazersCount"],
                    "pushed": v["pushedAt"],
                    "archived": v["isArchived"],
                })),
            })
        })
        .collect())
}

async fn gh_json(args: Vec<String>) -> Result<serde_json::Value, String> {
    let ran = tools::run(Tool::Gh, &args, Duration::from_secs(25)).await?;
    if ran.code != Some(0) {
        return Err(ran.stderr.trim().to_string());
    }
    serde_json::from_slice(&ran.stdout).map_err(|e| e.to_string())
}

/// Repos whose files mention `term` (code search), one row per repo.
async fn code_search(term: &str) -> Result<Vec<SearchResult>, String> {
    let mut args: Vec<String> = vec!["search".into(), "code".into()];
    args.extend(term.split_whitespace().map(str::to_string));
    args.extend(["--json", "repository,path", "--limit", "40"].map(String::from));
    let list = gh_json(args).await?;
    let mut rows: Vec<(String, Vec<String>)> = Vec::new();
    for v in list.as_array().into_iter().flatten() {
        let Some(repo) = v["repository"]["nameWithOwner"].as_str() else { continue };
        if v["repository"]["isFork"].as_bool() == Some(true) {
            continue;
        }
        let path = v["path"].as_str().unwrap_or("").to_string();
        match rows.iter_mut().find(|(r, _)| r == repo) {
            Some((_, paths)) => paths.push(path),
            None => rows.push((repo.to_string(), vec![path])),
        }
    }
    Ok(rows
        .into_iter()
        .take(20)
        .map(|(repo, paths)| SearchResult {
            title: repo.clone(),
            url: format!("https://github.com/{repo}"),
            snippet: format!("uses '{term}' in {}", paths.iter().take(3).cloned().collect::<Vec<_>>().join(", ")),
            source: "ghcode".into(),
            published: None,
            image_url: None,
            extra: None,
        })
        .collect())
}

/// Repos listed in the most-starred awesome lists for `words`, in the
/// order of how many lists name them.
async fn awesome(words: &str) -> Result<(Vec<SearchResult>, Vec<String>), String> {
    // One specific word at a time, in list names only: "awesome-markdown".
    // Generic words find lists about something else (awesome-html5: games).
    const GENERIC: &[&str] = &[
        "html", "web", "api", "app", "apps", "js", "javascript", "typescript", "python", "rust", "go", "java",
        "crate", "crates", "library", "libraries", "framework", "tool", "tools", "cli", "server", "client",
        "code", "open", "source", "free", "fast", "simple", "lib", "package", "plugin",
    ];
    let mut lists: Vec<String> = Vec::new();
    for w in words
        .split_whitespace()
        .filter(|w| !GENERIC.contains(&w.to_ascii_lowercase().as_str()))
        .take(3)
    {
        let found = gh_json(
            ["search", "repos", "awesome", w, "--match", "name", "--json", "fullName", "--sort", "stars", "--limit", "8"]
                .map(String::from)
                .to_vec(),
        )
        .await?;
        for n in found.as_array().into_iter().flatten().filter_map(|v| v["fullName"].as_str()) {
            let named = n.split('/').nth(1).is_some_and(|r| r.to_ascii_lowercase().starts_with(&format!("awesome-{}", w.to_ascii_lowercase())));
            if named && !lists.iter().any(|l| l == n) && lists.len() < 2 {
                lists.push(n.to_string());
            }
        }
        if lists.len() >= 2 {
            break;
        }
    }
    static LINK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"https?://github\.com/([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+)").expect("regex")
    });
    let mut seen: Vec<(String, usize, String)> = Vec::new();
    for list in &lists {
        let args: Vec<String> = vec![
            "api".into(),
            format!("repos/{list}/readme"),
            "-H".into(),
            "Accept: application/vnd.github.raw".into(),
        ];
        let Ok(ran) = tools::run(Tool::Gh, &args, Duration::from_secs(20)).await else { continue };
        let text = String::from_utf8_lossy(&ran.stdout);
        for c in LINK.captures_iter(&text) {
            let url = format!("https://github.com/{}/{}", &c[1], c[2].trim_end_matches(".git"));
            let Some((owner, repo)) = repo_of(&url) else { continue };
            let full = format!("{owner}/{repo}");
            if full.eq_ignore_ascii_case(list) || repo.to_ascii_lowercase().starts_with("awesome") {
                continue;
            }
            match seen.iter_mut().find(|(f, _, _)| f.eq_ignore_ascii_case(&full)) {
                Some(s) => s.1 += 1,
                None => seen.push((full, 1, list.clone())),
            }
        }
    }
    seen.sort_by(|a, b| b.1.cmp(&a.1));
    let results = seen
        .into_iter()
        .take(25)
        .map(|(full, n, list)| SearchResult {
            title: full.clone(),
            url: format!("https://github.com/{full}"),
            snippet: format!("listed in {list}{}", if n > 1 { format!(" ({n} mentions)") } else { String::new() }),
            source: "awesome".into(),
            published: None,
            image_url: None,
            extra: None,
        })
        .collect();
    Ok((results, lists))
}

/// Code mode's extra angles, and code search where the question asks for
/// examples or users of something (or the run asked for it).
pub async fn round(
    app: &Arc<AppContext>,
    recorder: &raw::Recorder,
    planned: &[Planned],
    responses: &Responses,
    req: &Request,
    no_cache: bool,
) -> RoundOut {
    let mut out = RoundOut::default();
    if tools::locate(Tool::Gh).is_none() {
        return out;
    }
    for p in planned.iter().filter(|p| !p.side) {
        let code = p.mode == Mode::Code;
        let words = carrying_words(&p.query, 4);
        let mut searches: Vec<(String, String)> = Vec::new();
        if code && !words.is_empty() {
            // The best-ranked repo the question found: its topics.
            let top = super::results_of(&p.label, responses)
                .into_iter()
                .find_map(|r| repo_of(&r.url));
            if let Some((owner, repo)) = top {
                for (i, t) in topics(&owner, &repo).await.into_iter().enumerate() {
                    searches.push((format!("{}t{}", p.label, i + 1), format!("topic:{t}")));
                }
            }
        }
        let mut set = tokio::task::JoinSet::new();
        for (label, query) in &searches {
            let app = app.clone();
            let rec = Some(recorder.labeled(label));
            let (label, query) = (label.clone(), query.clone());
            set.spawn(raw::scope(rec, async move {
                let engines = Some(vec!["github".to_string()]);
                let r = search_cached(app, &query, Mode::Code, 15, &engines, &SearchOpts::default(), no_cache).await;
                (label, r)
            }));
        }
        if code && !words.is_empty() {
            // Recently active projects: updated in the last four months, by stars.
            let label = format!("{}n", p.label);
            let since = format!(">={}", crate::providers::date_days_ago(120));
            match raw::scope(Some(recorder.labeled(&label)), repos(&words, &["--updated", &since], 15)).await {
                Ok(list) => {
                    let q = format!("{words} (updated {since})");
                    out.responses.push((label.clone(), Ok(local_response("github", &q, Mode::Code, list))));
                    out.planned.push(Planned::local(p, label, q, Mode::Code, "github", SAME));
                }
                Err(e) => out.notes.push(format!("{label}: recently active search failed: {e}")),
            }
            let label = format!("{}w", p.label);
            let rec = Some(recorder.labeled(&label));
            match raw::scope(rec, awesome(&words)).await {
                Ok((results, lists)) if !results.is_empty() => {
                    let q = format!("awesome lists: {}", lists.join(", "));
                    out.responses.push((label.clone(), Ok(local_response("awesome", &q, Mode::Code, results))));
                    out.planned.push(Planned::local(p, label, q, Mode::Code, "awesome", SAME));
                }
                Ok(_) => {}
                Err(e) => out.notes.push(format!("{label}: awesome lists: {e}")),
            }
        }
        let term = if req.code_search { Some(words.clone()).filter(|w| !w.is_empty()) } else { code_subject(&p.query, code) };
        if let Some(term) = term {
            let label = format!("{}c", p.label);
            let rec = Some(recorder.labeled(&label));
            match raw::scope(rec, code_search(&term)).await {
                Ok(results) => {
                    out.responses.push((label.clone(), Ok(local_response("ghcode", &term, Mode::Code, results))));
                    let slots = if code { SAME } else { LIST };
                    out.planned.push(Planned::local(p, label, format!("code search: {term}"), Mode::Code, "ghcode", slots));
                }
                Err(e) => out.notes.push(format!("{label}: GitHub code search failed: {e}")),
            }
        }
        while let Some(joined) = set.join_next().await {
            if let Ok(pair) = joined {
                out.responses.push(pair);
            }
        }
        for (label, query) in searches {
            out.planned.push(Planned::local(p, label, query, Mode::Code, "github", SAME));
        }
    }
    out
}
