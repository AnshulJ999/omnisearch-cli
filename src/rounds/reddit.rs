//! Community mode's Reddit routes: web engines scoped to reddit.com and
//! Reddit's RSS search (no account), search inside the question's
//! subreddits, and the logged-in old-Reddit search.

use super::{carrying_words, local_response, plan_side, results_of, Responses, RoundOut, SAME};
use crate::context::AppContext;
use crate::research::{search_cached, Planned, QuerySpec, Request};
use crate::run::raw;
use crate::types::{Mode, SearchOpts};
use std::sync::Arc;

/// Brave plus You.com scoped to reddit.com (Serper when either cannot run).
/// Exa returns nothing for reddit.com.
pub fn web(
    app: &Arc<AppContext>,
    req: &Request,
    statuses: &[crate::budget::Status],
    p: &Planned,
) -> Option<Planned> {
    let configured: Vec<String> = crate::providers::build_providers(app)
        .into_iter()
        .filter(|a| a.spec().modes.contains(&Mode::General) && a.is_configured())
        .map(|a| a.name().to_string())
        .collect();
    let wanted: Vec<&str> = ["brave", "youcom", "serper"]
        .into_iter()
        .filter(|e| configured.iter().any(|c| c == e))
        .collect();
    let picked = crate::budget::pick(Mode::General, crate::budget::Width::Wide, &wanted, statuses);
    let mut engines: Vec<String> = ["brave", "youcom"]
        .into_iter()
        .filter(|k| picked.engines.iter().any(|e| e == k))
        .map(String::from)
        .collect();
    if engines.len() < 2 && picked.engines.iter().any(|e| e == "serper") {
        engines.push("serper".into());
    }
    if engines.is_empty() {
        return None;
    }
    let spec = QuerySpec {
        q: p.query.clone(),
        mode: Some("general".into()),
        engines,
        count: Some(20),
        sites: vec!["reddit.com".into()],
        since: p.opts.freshness.clone(),
        ..QuerySpec::default()
    };
    plan_side(app, req, statuses, p, "r", spec, SAME)
}


/// Subreddits searched inside per community question, at most.
fn max_subs() -> usize {
    crate::config::get().research.redditsubs
}

/// With side-by-side searches on, every other logged-in search goes
/// through the main login.
static NEXT_MAIN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Reddit threads (subreddit, URL) among results.
fn threads<'a>(items: impl Iterator<Item = &'a crate::types::SearchResult>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for it in items {
        if let Some((sub, _)) = crate::providers::reddit::thread_ref(&it.url) {
            let key = crate::engine::normalize_url(&it.url);
            if !out.iter().any(|(_, u)| *u == key) {
                out.push((sub, key));
            }
        }
    }
    out
}

/// A logged-in search through the run account; when that fails, through
/// the main login within the run's allowance (`calls` pages), with a note.
/// Side by side (`research.redditsidebyside`), every other search goes
/// through the main login first, within the same allowance.
async fn logged_in<F, Fut>(search: F, calls: usize) -> (Result<Vec<crate::types::SearchResult>, String>, Option<String>)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<crate::types::SearchResult>, String>>,
{
    use crate::providers::reddit::{has_login, relogin_hint, run_account, take_main_calls};
    let run = run_account();
    let side = crate::config::get().research.redditsidebyside && run != "main" && has_login("main") && has_login(&run);
    if side && NEXT_MAIN.fetch_xor(true, std::sync::atomic::Ordering::Relaxed) && take_main_calls(calls) {
        return match crate::cookies::with_account("main", search()).await {
            Ok(v) => (Ok(v), None),
            Err(e) => {
                let r = crate::cookies::with_account(&run, search()).await;
                (r, Some(format!("a side-by-side search through the main Reddit login failed ({e}); the {run} login took it")))
            }
        };
    }
    let first = if has_login(&run) {
        crate::cookies::with_account(&run, search()).await
    } else {
        Err(format!("no {run} login saved"))
    };
    match first {
        Ok(v) => (Ok(v), None),
        Err(e) if run != "main" && has_login("main") && take_main_calls(calls) => {
            let r = crate::cookies::with_account("main", search()).await;
            (r, Some(format!("the {run} Reddit login failed ({e}); the main login stepped in; {}", relogin_hint())))
        }
        Err(e) => (Err(e), None),
    }
}

/// Whether a subreddit's name is one of `words` or made of one: its words
/// (split at underscores and capitals: `GooglePixel` is google, pixel) or the
/// whole name equal a word, or two neighbouring words run together.
fn name_has_word(name: &str, words: &[String]) -> bool {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if c == '_' || (c.is_uppercase() && prev_lower) {
            if !cur.is_empty() {
                parts.push(std::mem::take(&mut cur).to_lowercase());
            }
        }
        if c != '_' {
            cur.push(c);
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
    }
    if !cur.is_empty() {
        parts.push(cur.to_lowercase());
    }
    let whole = super::squash(name);
    words.iter().any(|w| *w == whole || parts.contains(w))
        || words.windows(2).any(|pair| format!("{}{}", pair[0], pair[1]) == whole)
}

#[cfg(test)]
mod tests {
    use super::name_has_word;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn subreddit_names_match_whole_words_only() {
        assert!(!name_has_word("programming", &words("pro plan")));
        assert!(name_has_word("GooglePixel", &words("pixel 9 rom")));
        assert!(name_has_word("selfhosted", &words("self hosted")));
        assert!(name_has_word("GrapheneOS", &words("graphene os phone")));
    }
}

/// Subreddits a free web search finds for the question by name: kept when
/// the name shares a word with the question or turns up in more than one
/// result, most often seen first.
async fn discover(app: Arc<AppContext>, query: &str, no_cache: bool) -> Vec<String> {
    static SUB: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?:^|[^A-Za-z0-9_])/?r/([A-Za-z0-9_]{2,21})\b").expect("regex")
    });
    let configured = |name: &str| {
        crate::providers::find(name).is_some_and(|s| !crate::providers::key_for(&app.config, s).is_empty())
    };
    let Some(engine) = ["jina", "brave"].into_iter().find(|e| configured(e)) else {
        return Vec::new();
    };
    let q = format!("{} subreddit", carrying_words(query, 6));
    let engines = Some(vec![engine.to_string()]);
    let Ok(resp) = search_cached(app.clone(), &q, Mode::General, 20, &engines, &SearchOpts::default(), no_cache).await else {
        return Vec::new();
    };
    let words: Vec<String> = carrying_words(query, 8)
        .split_whitespace()
        .map(super::squash)
        .filter(|w| w.len() >= 3)
        .collect();
    let mut seen: Vec<(String, usize)> = Vec::new();
    for r in &resp.results {
        let text = format!("{} {} {}", r.title, r.snippet, r.url);
        let mut here: Vec<String> = Vec::new();
        for c in SUB.captures_iter(&text) {
            let name = c[1].to_string();
            if ["all", "popular", "subreddit", "subreddits"].contains(&name.to_lowercase().as_str()) {
                continue;
            }
            if !here.iter().any(|h| h.eq_ignore_ascii_case(&name)) {
                here.push(name);
            }
        }
        for name in here {
            match seen.iter_mut().find(|(s, _)| s.eq_ignore_ascii_case(&name)) {
                Some(e) => e.1 += 1,
                None => seen.push((name, 1)),
            }
        }
    }
    seen.retain(|(s, n)| *n > 1 || name_has_word(s, &words));
    seen.sort_by(|a, b| b.1.cmp(&a.1));
    seen.into_iter().map(|(s, _)| s).collect()
}

/// Search inside each of the question's subreddits: those the request
/// named, the two its threads came from most, then ones found by name, at
/// most `research.redditsubs`. With the login, Reddit's own search there (top of the
/// year, or the most relevant of the `-f` period); without it, each one's
/// top threads of the year, on topic only.
#[allow(clippy::too_many_arguments)]
async fn subreddits(
    app: Arc<AppContext>,
    recorder: raw::Recorder,
    p: Planned,
    named: Vec<String>,
    from_threads: Vec<String>,
    login: bool,
    no_cache: bool,
) -> RoundOut {
    let mut out = RoundOut::default();
    let max = max_subs();
    if max == 0 {
        return out;
    }
    let mut subs: Vec<(String, &'static str)> = Vec::new();
    let add = |subs: &mut Vec<(String, &'static str)>, s: &str, why: &'static str| {
        let s = s.trim().trim_start_matches('/').trim_start_matches("r/").trim_end_matches('/');
        if !s.is_empty() && subs.len() < max && !subs.iter().any(|(x, _)| x.eq_ignore_ascii_case(s)) {
            subs.push((s.to_string(), why));
        }
    };
    for s in &named {
        add(&mut subs, s, "named");
    }
    for s in &from_threads {
        add(&mut subs, s, "threads");
    }
    if subs.len() < max {
        let rec = Some(recorder.labeled(&format!("{}s", p.label)));
        for s in raw::scope(rec, discover(app.clone(), &p.query, no_cache)).await {
            add(&mut subs, &s, "found");
        }
    }
    if subs.is_empty() {
        return out;
    }
    let list: Vec<String> = subs.iter().map(|(s, why)| format!("r/{s} ({why})")).collect();
    out.coverage.push(format!("{}: searched inside {}", p.label, list.join(", ")));
    let freshness = p.opts.freshness.clone();
    let mut set = tokio::task::JoinSet::new();
    for (i, (sub, _)) in subs.into_iter().enumerate() {
        let label = format!("{}a{}", p.label, i + 1);
        let rec = Some(recorder.labeled(&label));
        if login {
            let period = match &freshness {
                Some(f) => format!("most relevant of the past {f}"),
                None => "top of the year".to_string(),
            };
            let q = format!("r/{sub} search, {period}: {}", p.query);
            out.planned.push(Planned::local(&p, label.clone(), q.clone(), Mode::Community, "reddit-login", SAME));
            let (query, freshness) = (p.query.clone(), freshness.clone());
            set.spawn(raw::scope(rec, async move {
                let (r, note) = logged_in(
                    || crate::providers::reddit::login_subreddit_search(&sub, &query, 25, freshness.as_deref()),
                    1,
                )
                .await;
                let r = r
                    .map(|list| local_response("reddit-login", &q, Mode::Community, list))
                    .map_err(|e| crate::errors::SearchError::Config(format!("logged-in r/{sub} search failed: {e}")));
                (label, r, note)
            }));
            continue;
        }
        let q = format!("r/{sub} top threads of the year on: {}", p.query);
        out.planned.push(Planned::local(&p, label.clone(), q.clone(), Mode::Community, "reddit", SAME));
        let (app, topic) = (app.clone(), p.query.clone());
        set.spawn(raw::scope(rec, async move {
            let r = crate::providers::reddit::subreddit_top(&app, &sub, std::time::Duration::from_secs(30))
                .await
                .map(|list| local_response("reddit", &q, Mode::Community, crate::relevance::keep(list, &topic)))
                .map_err(crate::errors::SearchError::Config);
            (label, r, None)
        }));
    }
    while let Some(joined) = set.join_next().await {
        if let Ok((label, r, note)) = joined {
            out.responses.push((label, r));
            out.notes.extend(note);
        }
    }
    out
}

/// After the first searches of a community query, all at once: Reddit's
/// search inside the question's subreddits (`subreddits`; the archive
/// inside the top three when `research.redditarchive` is on), and the
/// logged-in search (`research.redditlogin` always, auto = fewer than
/// `research.redditmin` threads, never; a run's `reddit_login` forces it,
/// deeper). Logged-in searches go through the run account
/// (`research.redditaccount`); when it fails, the main login steps in for
/// at most `research.redditmainmax` calls a run.
pub async fn round(
    app: &Arc<AppContext>,
    recorder: &raw::Recorder,
    planned: &[Planned],
    responses: &Responses,
    req: &Request,
    no_cache: bool,
) -> RoundOut {
    use crate::providers::reddit::{has_login, run_account};
    let mut out = RoundOut::default();
    let parents: Vec<&Planned> = planned
        .iter()
        .filter(|p| p.mode == Mode::Community && !p.side)
        .collect();
    let rc = crate::config::get().research.clone();
    let can_log_in = has_login(&run_account()) || has_login("main");
    let mut set: tokio::task::JoinSet<RoundOut> = tokio::task::JoinSet::new();
    for p in &parents {
        let first = threads(results_of(&p.label, responses).into_iter());
        let mut counts: Vec<(String, usize)> = Vec::new();
        for (sub, _) in &first {
            match counts.iter_mut().find(|(s, _)| s.eq_ignore_ascii_case(sub)) {
                Some(c) => c.1 += 1,
                None => counts.push((sub.clone(), 1)),
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1));
        // The archive's title search needs every word: the one that carries it.
        let word = carrying_words(&p.query, 1);
        if rc.redditarchive && !word.is_empty() {
            for (i, (sub, _)) in counts.iter().take(3).enumerate() {
                let a = Planned::local(p, format!("{}a{}", p.label, i + 1), format!("r/{sub} {word}"), Mode::Community, "reddit", SAME);
                let app = app.clone();
                let rec = Some(recorder.labeled(&a.label));
                let (label, query) = (a.label.clone(), a.query.clone());
                let opts = SearchOpts { freshness: p.opts.freshness.clone(), ..SearchOpts::default() };
                out.planned.push(a);
                set.spawn(raw::scope(rec, async move {
                    let engines = Some(vec!["reddit".to_string()]);
                    let r = search_cached(app, &query, Mode::Community, 25, &engines, &opts, no_cache).await;
                    RoundOut { responses: vec![(label, r)], ..RoundOut::default() }
                }));
            }
        }
        let want = req.reddit_login
            || match rc.redditlogin.as_str() {
                "always" => true,
                "never" => false,
                _ => first.len() < rc.redditmin,
            };
        if !rc.redditarchive {
            let from_threads: Vec<String> = counts.iter().take(2).map(|(s, _)| s.clone()).collect();
            set.spawn(subreddits(
                app.clone(),
                recorder.clone(),
                (*p).clone(),
                req.subreddits.clone(),
                from_threads,
                can_log_in && want,
                no_cache,
            ));
        }
        if !want {
            continue;
        }
        if !can_log_in {
            out.notes.push(format!(
                "{}: {} Reddit threads found; the logged-in Reddit search would add more (run `osearch login reddit.com`)",
                p.label,
                first.len()
            ));
            continue;
        }
        let label = format!("{}l", p.label);
        out.planned.push(Planned::local(p, label.clone(), p.query.clone(), Mode::Community, "reddit-login", SAME));
        let (query, deeper, freshness) = (p.query.clone(), req.reddit_login, p.opts.freshness.clone());
        let rec = Some(recorder.labeled(&label));
        set.spawn(raw::scope(rec, async move {
            let calls = if deeper { 3 } else { 1 };
            let (r, note) = logged_in(
                || crate::providers::reddit::login_search(&query, 25, deeper, freshness.as_deref()),
                calls,
            )
            .await;
            let r = r
                .map(|results| local_response("reddit-login", &query, Mode::Community, results))
                .map_err(|e| crate::errors::SearchError::Config(format!("logged-in Reddit search failed: {e}")));
            RoundOut { responses: vec![(label, r)], notes: note.into_iter().collect(), ..RoundOut::default() }
        }));
        // Reddit's comment search: threads whose answer is in a reply.
        if rc.redditcomments {
            let label = format!("{}e", p.label);
            let q = format!("comments: {}", p.query);
            out.planned.push(Planned::local(p, label.clone(), q.clone(), Mode::Community, "reddit-login", SAME));
            let (query, freshness) = (p.query.clone(), p.opts.freshness.clone());
            let rec = Some(recorder.labeled(&label));
            set.spawn(raw::scope(rec, async move {
                let (r, note) = logged_in(
                    || crate::providers::reddit::login_comment_search(&query, 25, freshness.as_deref()),
                    1,
                )
                .await;
                let r = r
                    .map(|results| local_response("reddit-login", &q, Mode::Community, results))
                    .map_err(|e| crate::errors::SearchError::Config(format!("logged-in Reddit comment search failed: {e}")));
                RoundOut { responses: vec![(label, r)], notes: note.into_iter().collect(), ..RoundOut::default() }
            }));
        }
    }
    while let Some(joined) = set.join_next().await {
        if let Ok(part) = joined {
            out.merge(part);
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.notes.retain(|n| seen.insert(n.clone()));
    out
}
