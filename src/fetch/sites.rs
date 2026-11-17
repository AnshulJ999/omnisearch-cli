//! Tier 0: sites read through their own data door instead of their web
//! page. Each door returns markdown; a door that fails hands the page to
//! the next tier (a 404 stops the fetch).

use super::{Got, Miss, TierResult};
use crate::context::AppContext;
use crate::providers::{self, Adapter, Call};
use crate::tools::{self, Tool};
use crate::types::{Mode, SearchOpts};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

type Door<'a> = Pin<Box<dyn Future<Output = TierResult> + Send + 'a>>;

/// The door for `url`, if a site has one.
pub fn door<'a>(
    app: &'a Arc<AppContext>,
    url: &'a str,
    opts: &'a super::FetchOpts,
) -> Option<(&'static str, Door<'a>)> {
    let u = url::Url::parse(url).ok()?;
    let host = u.host_str()?.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    let segs: Vec<String> = u
        .path_segments()
        .map(|s| s.filter(|x| !x.is_empty()).map(str::to_string).collect())
        .unwrap_or_default();

    if let Some(platform) = providers::social_api::platform_of_url(url) {
        let name = if platform == "youtube" { "youtubesc" } else { platform };
        if opts.platforms.iter().any(|p| p.eq_ignore_ascii_case(name)) {
            let name: &'static str = providers::find(name)?.name;
            return Some((name, Box::pin(adapter_extract(app, name, url))));
        }
    }
    if providers::reddit::thread_ref(url).is_some() {
        let read = if opts.asked { providers::reddit::Read::Asked } else { providers::reddit::Read::Picked };
        return Some(("reddit", Box::pin(reddit(app, url, read))));
    }
    if providers::hackernews::item_id(url).is_some() {
        return Some(("hackernews", Box::pin(adapter_extract(app, "hackernews", url))));
    }
    if let Some((channel, post)) = providers::telegram::channel_of(url) {
        return Some(("telegram", Box::pin(telegram(app, url, channel, post))));
    }
    if let Some((handle, rkey)) = bluesky_post(&host, &segs) {
        return Some(("bluesky", Box::pin(bluesky(app, url, handle, rkey))));
    }
    if youtube_id(&u).is_some() && tools::locate(Tool::YtDlp).is_some() {
        return Some(("yt-dlp", Box::pin(youtube(app, url))));
    }
    if host == "github.com" && segs.len() >= 2 && tools::locate(Tool::Gh).is_some() {
        return Some(("gh", Box::pin(github(url.to_string(), segs))));
    }
    if let Some((site, id)) = stackexchange(&host, &segs) {
        return Some(("stackexchange", Box::pin(stack(app, url, site, id))));
    }
    if let Some(id) = discourse_topic(&host, &segs) {
        return Some(("discourse", Box::pin(discourse(app, url, host.clone(), id))));
    }
    match (host.as_str(), segs.as_slice()) {
        ("npmjs.com", [p, rest @ ..]) if p == "package" && !rest.is_empty() => {
            let name = rest.join("/");
            Some(("npm", Box::pin(npm(app, url, name))))
        }
        ("pypi.org", [p, name, ..]) if p == "project" => {
            Some(("pypi", Box::pin(pypi(app, url, name.clone()))))
        }
        ("crates.io", [c, name, ..]) if c == "crates" => {
            Some(("crates.io", Box::pin(crate_io(app, url, name.clone()))))
        }
        ("arxiv.org", [kind, id, ..]) if kind == "abs" || kind == "pdf" => {
            let id = id.trim_end_matches(".pdf").to_string();
            Some(("arxiv", Box::pin(arxiv(app, url, id))))
        }
        _ => None,
    }
}

fn other(e: impl std::fmt::Display) -> (Miss, String) {
    (Miss::Other, e.to_string())
}

fn got(final_url: &str, title: &str, markdown: String) -> Got {
    Got {
        stamp: None,
        note: None,
        published: None,
        final_url: final_url.to_string(),
        title: title.to_string(),
        markdown,
        loss: None,
        cost_micros: 0,
    }
}

/// A provider's own extract (a whole Reddit or HN thread, a social post).
async fn adapter_extract(app: &Arc<AppContext>, name: &'static str, url: &str) -> TierResult {
    let adapter: Box<dyn Adapter> = providers::build_providers(app)
        .into_iter()
        .find(|a| a.name() == name)
        .ok_or_else(|| other(format!("{name} is not built in")))?;
    if !adapter.is_configured() {
        return Err(other(format!("{name} is not configured")));
    }
    let opts = SearchOpts::default();
    let call = Call {
        mode: Mode::Extract,
        query: url,
        count: 1,
        opts: &opts,
        timeout: Duration::from_secs(30),
    };
    let out = adapter.call(&call).await.map_err(|e| {
        let notfound = e.to_string().contains("404");
        (if notfound { Miss::NotFound } else { Miss::Other }, e.to_string())
    })?;
    let r = out
        .results
        .into_iter()
        .next()
        .ok_or_else(|| other("no thread returned"))?;
    let mut g = from_result(r);
    g.cost_micros = out.cost_micros.unwrap_or(0);
    Ok(g)
}

/// A Reddit thread: live or from the archive, by who asked for it.
async fn reddit(app: &Arc<AppContext>, url: &str, read: providers::reddit::Read) -> TierResult {
    let r = providers::reddit::read_thread(app.clone(), url, read).await.map_err(|e| {
        let notfound = e.to_string().contains("404");
        (if notfound { Miss::NotFound } else { Miss::Other }, e.to_string())
    })?;
    Ok(from_result(r))
}

/// A provider's result as a page; its stamp and notes (`extra`) go along.
fn from_result(r: crate::types::SearchResult) -> Got {
    let mut g = got(&r.url, &r.title, r.snippet);
    if let Some(extra) = &r.extra {
        g.stamp = extra["stamp"].as_str().map(str::to_string);
        let notes: Vec<String> = extra["notes"]
            .as_array()
            .map(|a| a.iter().filter_map(|n| n.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        if !notes.is_empty() {
            g.note = Some(notes.join("; "));
        }
    }
    if g.published.is_none() {
        g.published = r.published.clone();
    }
    g
}

async fn get_json(app: &Arc<AppContext>, url: &str) -> Result<Value, (Miss, String)> {
    let started = Instant::now();
    let resp = app
        .client
        .get(url)
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| other(e.without_url()))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(other)?;
    crate::run::raw::record_page("site-api", url, status, started.elapsed(), &text);
    if status == 404 {
        return Err((Miss::NotFound, format!("HTTP 404 from {url}")));
    }
    if status >= 400 {
        return Err(other(format!("HTTP {status} from {url}")));
    }
    serde_json::from_str(&text).map_err(other)
}

async fn get_text(app: &Arc<AppContext>, url: &str) -> Result<String, (Miss, String)> {
    let started = Instant::now();
    let resp = app
        .client
        .get(url)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| other(e.without_url()))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(other)?;
    crate::run::raw::record_page("site-api", url, status, started.elapsed(), &text);
    if status >= 400 {
        return Err(other(format!("HTTP {status} from {url}")));
    }
    Ok(text)
}

// Bluesky: a post and its thread through the public API (no login).

fn bluesky_post(host: &str, segs: &[String]) -> Option<(String, String)> {
    match (host, segs) {
        ("bsky.app", [p, handle, k, rkey, ..]) if p == "profile" && k == "post" => Some((handle.clone(), rkey.clone())),
        _ => None,
    }
}

async fn bluesky(app: &Arc<AppContext>, url: &str, handle: String, rkey: String) -> TierResult {
    let api = format!(
        "https://public.api.bsky.app/xrpc/app.bsky.feed.getPostThread?uri=at://{handle}/app.bsky.feed.post/{rkey}&depth=6&parentHeight=5"
    );
    let v = get_json(app, &api).await?;
    let t = &v["thread"];
    let post = &t["post"];
    if !post.is_object() {
        return Err((Miss::NotFound, "Bluesky has no such post".into()));
    }
    let title = format!(
        "@{}: {}",
        post["author"]["handle"].as_str().unwrap_or(&handle),
        providers::thread::clip(&providers::thread::one_line(post["record"]["text"].as_str().unwrap_or("")), 80)
    );
    let mut md = format!("# {title}\n\n{url}\n\n");
    let mut parents = Vec::new();
    let mut p = &t["parent"];
    while p["post"].is_object() {
        parents.push(&p["post"]);
        p = &p["parent"];
    }
    for p in parents.iter().rev() {
        md.push_str(&bsky_post("Earlier in the thread", p));
    }
    md.push_str(&bsky_post("Post", post));
    bsky_replies(&mut md, t, 0);
    Ok(got(url, &title, md))
}

fn bsky_post(label: &str, p: &Value) -> String {
    let date = p["record"]["createdAt"].as_str().unwrap_or("");
    format!(
        "## {label}: @{} · {} · {} likes, {} replies\n\n{}\n\n",
        p["author"]["handle"].as_str().unwrap_or("?"),
        date.get(..10).unwrap_or(date),
        p["likeCount"].as_i64().unwrap_or(0),
        p["replyCount"].as_i64().unwrap_or(0),
        p["record"]["text"].as_str().unwrap_or("").trim()
    )
}

fn bsky_replies(md: &mut String, node: &Value, depth: usize) {
    for r in node["replies"].as_array().into_iter().flatten() {
        if !r["post"].is_object() {
            continue;
        }
        let label = if depth == 0 {
            "Reply".to_string()
        } else {
            format!("Reply to @{}", node["post"]["author"]["handle"].as_str().unwrap_or("?"))
        };
        md.push_str(&bsky_post(&label, &r["post"]));
        bsky_replies(md, r, depth + 1);
    }
}

// Telegram: a public channel's preview page.

async fn telegram(app: &Arc<AppContext>, url: &str, channel: String, post: Option<String>) -> TierResult {
    let page = match &post {
        Some(id) => format!("https://t.me/s/{channel}/{id}"),
        None => format!("https://t.me/s/{channel}"),
    };
    let html = providers::telegram::page(app, &page).await.map_err(other)?;
    let (title, posts) = providers::telegram::parse(&html);
    if posts.is_empty() {
        return Err((Miss::Other, "no public posts (a group or a private channel)".into()));
    }
    let title = if title.is_empty() { channel } else { title };
    Ok(got(url, &title, providers::telegram::markdown(&page, &title, &posts)))
}

/// An HTML fragment (a StackExchange body, a crates.io readme) as markdown.
fn fragment_md(html: &str, base: &str) -> String {
    let (_, md) =
        crate::page::whole_page_markdown(&format!("<html><body>{html}</body></html>"), base);
    md
}

// ---------------------------------------------------------------------------
// YouTube: metadata and transcript through yt-dlp

fn youtube_id(u: &url::Url) -> Option<String> {
    let host = u.host_str()?.to_ascii_lowercase();
    let host = host
        .trim_start_matches("www.")
        .trim_start_matches("m.")
        .to_string();
    match host.as_str() {
        "youtube.com" | "music.youtube.com" => {
            if u.path() == "/watch" {
                return u
                    .query_pairs()
                    .find(|(k, _)| k == "v")
                    .map(|(_, v)| v.to_string());
            }
            let segs: Vec<&str> = u.path_segments()?.collect();
            match segs.as_slice() {
                ["shorts" | "live" | "embed", id, ..] => Some(id.to_string()),
                _ => None,
            }
        }
        "youtu.be" => u.path_segments()?.next().map(str::to_string),
        _ => None,
    }
}

/// The English caption track's json3 URL: manual subtitles first, then the
/// automatic ones.
/// Caption tracks to try, best first: uploaded English subtitles, then
/// automatic English (the original-speech track first), then the original
/// speech in another language. A translated track is often refused (HTTP
/// 429), so the next one is tried.
fn caption_urls(info: &Value) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for (field, auto) in [("subtitles", false), ("automatic_captions", true)] {
        let Some(tracks) = info[field].as_object() else {
            continue;
        };
        let mut langs: Vec<&String> = tracks
            .keys()
            .filter(|k| *k == "en" || k.starts_with("en-") || k.starts_with("en_") || (auto && k.ends_with("-orig")))
            .collect();
        let english = |k: &str| k == "en" || k.starts_with("en-") || k.starts_with("en_");
        langs.sort_by_key(|k| (!english(k), k.as_str() != "en-orig", k.as_str() != "en", k.len()));
        for lang in langs {
            if let Some(list) = tracks[lang.as_str()].as_array() {
                if let Some(t) = list.iter().find(|t| t["ext"] == "json3") {
                    if let Some(u) = t["url"].as_str() {
                        out.push((u.to_string(), auto));
                    }
                }
            }
        }
    }
    out.truncate(4);
    out
}

fn json3_text(v: &Value) -> String {
    let mut out = String::new();
    for ev in v["events"].as_array().into_iter().flatten() {
        for seg in ev["segs"].as_array().into_iter().flatten() {
            if let Some(t) = seg["utf8"].as_str() {
                out.push_str(t);
            }
        }
        if !out.ends_with(' ') && !out.ends_with('\n') {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

async fn youtube(app: &Arc<AppContext>, url: &str) -> TierResult {
    let args: Vec<String> = vec![
        "-J".into(),
        "--skip-download".into(),
        "--no-warnings".into(),
        url.into(),
    ];
    let ran = tools::run(Tool::YtDlp, &args, Duration::from_secs(60))
        .await
        .map_err(other)?;
    if ran.code != Some(0) {
        let err = ran.stderr.trim().to_string();
        let miss = if err.contains("unavailable") || err.contains("private") {
            Miss::NotFound
        } else {
            Miss::Other
        };
        return Err((miss, crate::providers::thread::clip(&err, 300)));
    }
    let info: Value = serde_json::from_slice(&ran.stdout).map_err(|e| {
        tools::contract_warning("yt-dlp -J", &format!("output is not JSON: {e}"));
        other(e)
    })?;
    let title = info["title"].as_str().unwrap_or("YouTube video").to_string();
    let mut md = format!("# {title}\n\n{url}\n\n");
    let channel = info["channel"].as_str().or(info["uploader"].as_str()).unwrap_or("");
    let date = info["upload_date"].as_str().unwrap_or("");
    let mut line = vec![channel.to_string()];
    if date.len() == 8 {
        line.push(format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..]));
    }
    if let Some(d) = info["duration"].as_f64() {
        line.push(format!("{}:{:02}", (d as u64) / 60, (d as u64) % 60));
    }
    if let Some(v) = info["view_count"].as_u64() {
        line.push(format!("{v} views"));
    }
    md.push_str(&line.join(" · "));
    md.push_str("\n\n## Description\n\n");
    md.push_str(info["description"].as_str().unwrap_or("").trim());
    md.push('\n');
    if let Some(chapters) = info["chapters"].as_array().filter(|c| !c.is_empty()) {
        md.push_str("\n## Chapters\n\n");
        for c in chapters {
            let t = c["start_time"].as_f64().unwrap_or(0.0) as u64;
            md.push_str(&format!(
                "- {}:{:02} {}\n",
                t / 60,
                t % 60,
                c["title"].as_str().unwrap_or("")
            ));
        }
    }
    let tracks = caption_urls(&info);
    let mut last_err = String::new();
    let mut got_one = false;
    for (cap, auto) in &tracks {
        match get_json(app, cap).await {
            Ok(v) => {
                let text = json3_text(&v);
                md.push_str(&format!(
                    "\n## Transcript{}\n\n{text}\n",
                    if *auto { " (automatic captions)" } else { "" }
                ));
                got_one = true;
                break;
            }
            Err((_, e)) => last_err = e,
        }
    }
    match (got_one, tracks.is_empty()) {
        (true, _) => {}
        (false, false) => md.push_str(&format!("\n(transcript unavailable: {last_err})\n")),
        (false, true) => md.push_str("\n(no English captions on this video)\n"),
    }
    Ok(got(url, &title, md))
}

// ---------------------------------------------------------------------------
// GitHub through `gh`

async fn gh_api(path: &str, raw: bool) -> Result<String, (Miss, String)> {
    let mut args = vec!["api".to_string(), path.to_string()];
    if raw {
        args.push("-H".into());
        args.push("Accept: application/vnd.github.raw".into());
    }
    let ran = tools::run(Tool::Gh, &args, Duration::from_secs(30))
        .await
        .map_err(other)?;
    if ran.code != Some(0) {
        let err = ran.stderr.trim().to_string();
        let miss = if err.contains("404") || err.contains("Not Found") {
            Miss::NotFound
        } else {
            Miss::Other
        };
        return Err((miss, err));
    }
    Ok(ran.stdout_text())
}

async fn github(url: String, segs: Vec<String>) -> TierResult {
    let (owner, repo) = (&segs[0], &segs[1]);
    let base = format!("repos/{owner}/{repo}");
    match segs.get(2).map(String::as_str) {
        None => {
            let meta: Value = serde_json::from_str(&gh_api(&base, false).await?).map_err(other)?;
            let readme = gh_api(&format!("{base}/readme"), true)
                .await
                .unwrap_or_default();
            let title = meta["full_name"].as_str().unwrap_or(repo).to_string();
            let md = format!(
                "# {title}\n\n{url}\n\n{}\n\nstars {} · forks {} · open issues {} · language {} · license {} · updated {}\n\n---\n\n{}",
                meta["description"].as_str().unwrap_or(""),
                meta["stargazers_count"],
                meta["forks_count"],
                meta["open_issues_count"],
                meta["language"].as_str().unwrap_or("-"),
                meta["license"]["spdx_id"].as_str().unwrap_or("-"),
                meta["pushed_at"].as_str().unwrap_or(""),
                readme.trim()
            );
            Ok(got(&url, &title, md))
        }
        Some("issues" | "pull") => {
            let n = segs.get(3).ok_or_else(|| other("no issue number"))?;
            let issue: Value = serde_json::from_str(&gh_api(&format!("{base}/issues/{n}"), false).await?)
                .map_err(other)?;
            let comments: Value = serde_json::from_str(
                &gh_api(&format!("{base}/issues/{n}/comments?per_page=100"), false).await?,
            )
            .unwrap_or(Value::Null);
            let title = format!(
                "{owner}/{repo}#{n}: {}",
                issue["title"].as_str().unwrap_or("")
            );
            let mut md = format!(
                "# {title}\n\n{url}\n\n{} by {} · {} · {} comments\n\n{}\n",
                issue["state"].as_str().unwrap_or(""),
                issue["user"]["login"].as_str().unwrap_or(""),
                issue["created_at"].as_str().unwrap_or(""),
                issue["comments"],
                issue["body"].as_str().unwrap_or("").trim()
            );
            for c in comments.as_array().into_iter().flatten() {
                md.push_str(&format!(
                    "\n## {} · {}\n\n{}\n",
                    c["user"]["login"].as_str().unwrap_or(""),
                    c["created_at"].as_str().unwrap_or(""),
                    c["body"].as_str().unwrap_or("").trim()
                ));
            }
            Ok(got(&url, &title, md))
        }
        Some("blob") if segs.len() >= 5 => {
            let reference = &segs[3];
            let path = segs[4..].join("/");
            let text = gh_api(&format!("{base}/contents/{path}?ref={reference}"), true).await?;
            let title = format!("{owner}/{repo}: {path}");
            let fence = if path.ends_with(".md") { "" } else { "```" };
            let md = format!("# {title}\n\n{url}\n\n{fence}\n{}\n{fence}\n", text.trim_end());
            Ok(got(&url, &title, md))
        }
        _ => Err(other("not a repo, issue, PR or file page")),
    }
}

// ---------------------------------------------------------------------------
// StackExchange API

fn stackexchange(host: &str, segs: &[String]) -> Option<(String, String)> {
    let site = match host {
        "stackoverflow.com" => "stackoverflow".to_string(),
        "superuser.com" => "superuser".to_string(),
        "serverfault.com" => "serverfault".to_string(),
        "askubuntu.com" => "askubuntu".to_string(),
        h if h.ends_with(".stackexchange.com") => h.trim_end_matches(".stackexchange.com").to_string(),
        _ => return None,
    };
    match segs {
        [q, id, ..] if (q == "questions" || q == "q") && id.chars().all(|c| c.is_ascii_digit()) => {
            Some((site, id.clone()))
        }
        _ => None,
    }
}

/// A Discourse-style topic address: `/t/<slug>/<id>` or `/t/<id>`, on a
/// host that is not one of the sites read another way.
fn discourse_topic(host: &str, segs: &[String]) -> Option<String> {
    if ["github.com", "reddit.com", "news.ycombinator.com"].iter().any(|h| crate::cookies::under(host, h)) {
        return None;
    }
    let numeric = |s: &String| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    match segs {
        [t, _slug, id, ..] if t == "t" && numeric(id) => Some(id.clone()),
        [t, id, ..] if t == "t" && numeric(id) => Some(id.clone()),
        _ => None,
    }
}

/// A whole Discourse thread from its own data (`/t/<id>.json?print=true`):
/// every post with its author, date and likes. Not Discourse: next tier.
async fn discourse(app: &Arc<AppContext>, url: &str, host: String, id: String) -> TierResult {
    // Any failure here means "not this door": other sites use /t/<id> too
    // (v2ex.com), so a 404 must not count as the page being gone.
    let v = get_json(app, &format!("https://{host}/t/{id}.json?print=true"))
        .await
        .map_err(|(_, e)| other(format!("not a Discourse thread ({e})")))?;
    let posts = v["post_stream"]["posts"]
        .as_array()
        .ok_or_else(|| other("not a Discourse thread"))?;
    let title = v["title"].as_str().unwrap_or("").to_string();
    let mut md = format!(
        "# {title}\n\n{url}\n\n{host} · {} posts · {} views · started {}\n",
        v["posts_count"].as_u64().unwrap_or(posts.len() as u64),
        v["views"].as_u64().unwrap_or(0),
        v["created_at"].as_str().and_then(|d| d.get(..10)).unwrap_or("?")
    );
    for p in posts {
        let reply = p["reply_to_post_number"]
            .as_u64()
            .map(|n| format!(" · reply to #{n}"))
            .unwrap_or_default();
        let likes = p["actions_summary"]
            .as_array()
            .and_then(|a| a.iter().find(|x| x["id"] == 2))
            .and_then(|x| x["count"].as_u64())
            .map(|n| format!(" · {n} likes"))
            .unwrap_or_default();
        md.push_str(&format!(
            "\n## #{} {} · {}{likes}{reply}\n\n{}\n",
            p["post_number"].as_u64().unwrap_or(0),
            p["username"].as_str().unwrap_or("?"),
            p["created_at"].as_str().and_then(|d| d.get(..10)).unwrap_or("?"),
            fragment_md(p["cooked"].as_str().unwrap_or(""), url)
        ));
    }
    Ok(got(url, &title, md))
}

async fn stack(app: &Arc<AppContext>, url: &str, site: String, id: String) -> TierResult {
    let api = "https://api.stackexchange.com/2.3";
    let q = get_json(app, &format!("{api}/questions/{id}?site={site}&filter=withbody")).await?;
    let item = q["items"]
        .as_array()
        .and_then(|a| a.first())
        .ok_or((Miss::NotFound, "no such question".to_string()))?;
    let answers = get_json(
        app,
        &format!("{api}/questions/{id}/answers?site={site}&filter=withbody&sort=votes&pagesize=15"),
    )
    .await
    .unwrap_or(Value::Null);
    let title = crate::providers::thread::html_to_text(item["title"].as_str().unwrap_or(""));
    let tags: Vec<&str> = item["tags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut md = format!(
        "# {title}\n\n{url}\n\nscore {} · {} answers · tags: {}\n\n{}\n",
        item["score"],
        item["answer_count"],
        tags.join(", "),
        fragment_md(item["body"].as_str().unwrap_or(""), url)
    );
    for a in answers["items"].as_array().into_iter().flatten() {
        let accepted = if a["is_accepted"].as_bool() == Some(true) {
            " · accepted"
        } else {
            ""
        };
        md.push_str(&format!(
            "\n## Answer · score {}{accepted}\n\n{}\n",
            a["score"],
            fragment_md(a["body"].as_str().unwrap_or(""), url)
        ));
    }
    Ok(got(url, &title, md))
}

// ---------------------------------------------------------------------------
// Package registries

async fn npm(app: &Arc<AppContext>, url: &str, name: String) -> TierResult {
    let v = get_json(app, &format!("https://registry.npmjs.org/{name}")).await?;
    let latest = v["dist-tags"]["latest"].as_str().unwrap_or("");
    let md = format!(
        "# {name}\n\n{url}\n\n{}\n\nlatest {latest} · homepage {} · repository {}\n\n---\n\n{}",
        v["description"].as_str().unwrap_or(""),
        v["homepage"].as_str().unwrap_or("-"),
        v["repository"]["url"].as_str().unwrap_or("-"),
        v["readme"].as_str().unwrap_or("(no readme in the registry)").trim()
    );
    Ok(got(url, &name, md))
}

async fn pypi(app: &Arc<AppContext>, url: &str, name: String) -> TierResult {
    let v = get_json(app, &format!("https://pypi.org/pypi/{name}/json")).await?;
    let info = &v["info"];
    let links: Vec<String> = info["project_urls"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, u)| format!("{k}: {}", u.as_str().unwrap_or("")))
        .collect();
    let md = format!(
        "# {name}\n\n{url}\n\n{}\n\nversion {} · requires python {} · license {}\n{}\n\n---\n\n{}",
        info["summary"].as_str().unwrap_or(""),
        info["version"].as_str().unwrap_or(""),
        info["requires_python"].as_str().unwrap_or("-"),
        info["license"].as_str().unwrap_or("-"),
        links.join("\n"),
        info["description"].as_str().unwrap_or("").trim()
    );
    Ok(got(url, &name, md))
}

async fn crate_io(app: &Arc<AppContext>, url: &str, name: String) -> TierResult {
    let v = get_json(app, &format!("https://crates.io/api/v1/crates/{name}")).await?;
    let c = &v["crate"];
    let version = c["max_stable_version"]
        .as_str()
        .or(c["max_version"].as_str())
        .unwrap_or("")
        .to_string();
    let readme = get_text(app, &format!("https://crates.io/api/v1/crates/{name}/{version}/readme"))
        .await
        .map(|html| fragment_md(&html, url))
        .unwrap_or_default();
    let md = format!(
        "# {name}\n\n{url}\n\n{}\n\nversion {version} · downloads {} · repository {} · docs {}\n\n---\n\n{}",
        c["description"].as_str().unwrap_or(""),
        c["downloads"],
        c["repository"].as_str().unwrap_or("-"),
        c["documentation"].as_str().unwrap_or("-"),
        readme.trim()
    );
    Ok(got(url, &name, md))
}

// ---------------------------------------------------------------------------
// arXiv: the HTML paper when there is one, else the abstract

async fn arxiv(app: &Arc<AppContext>, url: &str, id: String) -> TierResult {
    let html_url = format!("https://arxiv.org/html/{id}");
    if let Ok(client) = crate::page::client() {
        if let Ok(p) = crate::page::fetch_direct(&client, &html_url, Duration::from_secs(20)).await {
            if p.status == 200 {
                if let Ok(g) = super::convert::from_page(p).await {
                    return Ok(g);
                }
            }
        }
    }
    let xml = get_text(app, &format!("https://export.arxiv.org/api/query?id_list={id}")).await?;
    let entry = xml.split("<entry>").nth(1).ok_or((Miss::NotFound, "no such paper".to_string()))?;
    let tag = |t: &str| -> String {
        entry
            .split(&format!("<{t}>"))
            .nth(1)
            .and_then(|s| s.split(&format!("</{t}>")).next())
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default()
    };
    let authors: Vec<String> = entry
        .split("<name>")
        .skip(1)
        .filter_map(|s| s.split("</name>").next())
        .map(str::to_string)
        .collect();
    let title = tag("title");
    let md = format!(
        "# {title}\n\n{url}\n\n{} · published {}\n\n## Abstract\n\n{}\n\n(full text: no HTML version; the PDF is at https://arxiv.org/pdf/{id})\n",
        authors.join(", "),
        tag("published"),
        tag("summary")
    );
    Ok(got(url, &title, md))
}
