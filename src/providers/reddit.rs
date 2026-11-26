//! `reddit`: Reddit search and threads, keyless.
//!
//! - **Live:** Reddit's own server-rendered partials (`/svc/shreddit/...`),
//!   fetched from this machine with a Chrome TLS fingerprint (wreq), then a
//!   Safari one when the first is refused. A 200 that is a captcha page is a
//!   failure, never content. Requests are paced by a token bucket shared by
//!   every `search` process on the machine (about 10 a minute); after a
//!   captcha the live route rests for 7 minutes.
//! - **Archive:** Arctic Shift (`arctic-shift.photon-reddit.com`), which
//!   keeps every thread's full comment tree. It saves a post seconds after
//!   it is posted and refreshes the post later; comments can take longer
//!   than a day to arrive. Archive search needs a subreddit (`r/<name>`).
//! - **Threads:** live is Reddit's own JSON of the thread through a saved
//!   login (`old.reddit.com/.../.json`). A thread an agent asked for is read
//!   live first (main login); a thread a run picked is read from the archive,
//!   and live only when the archive's copy is young or lacks the comments
//!   (`research.redditlivemax` a run, the run account). The archive fills
//!   what live lacks; a copy not read live is stamped.
//!
//! - **RSS:** Reddit's RSS search (`/search.rss`, `/r/<sub>/search.rss`)
//!   answers without a login, one request a minute per IP (its headers:
//!   `x-ratelimit-remaining: 0` after one); shared by every osearch process.
//!   Subreddit listings (`/svc/shreddit/community-more-posts/`) allow about
//!   200 per window and carry vote and comment counts.
//!
//! The archive's title search is off by default (`research.redditarchive`):
//! it times out on its own server for rare words in big subreddits.
//! `SEARCH_REDDIT_ROUTE=archive` skips the live route; `live` skips the
//! archive.

use super::spec::{FilterSupport, Price};
use super::thread::{self, Comment, Post};
use super::{Adapter, Call, Output, ProviderSpec};
use crate::context::AppContext;
use crate::errors::SearchError;
use crate::types::{FailureCategory, Mode, SearchResult};
use async_trait::async_trait;
use dom_query::Document;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const LIVE: &str = "https://www.reddit.com";
const ARCHIVE: &str = "https://arctic-shift.photon-reddit.com/api";

pub static SPEC: ProviderSpec = ProviderSpec {
    name: "reddit",
    env_keys: &[],
    modes: &[Mode::Community, Mode::Extract],
    filters: FilterSupport {
        freshness: true,
        domains: false,
        country: false,
        lang: false,
    },
    max_count: 25,
    timeout: Duration::from_secs(20),
    billed_on_send: false,
    price: Price {
        per_call: 0,
        per_extra_result: 0,
        included_results: 0,
    },
};

pub struct Reddit {
    ctx: Arc<AppContext>,
}

impl Reddit {
    pub fn new(ctx: Arc<AppContext>) -> Self {
        Self { ctx }
    }
}

// ---------------------------------------------------------------------------
// Thread URLs

/// `(subreddit, post id)` of a Reddit thread URL:
/// `reddit.com/r/<sub>/comments/<id>/...` on www, old, new, np or m.
pub fn thread_ref(url: &str) -> Option<(String, String)> {
    let u = url::Url::parse(url.trim()).ok()?;
    let host = u.host_str()?.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let host = ["old.", "new.", "np.", "m."]
        .iter()
        .find_map(|p| host.strip_prefix(p))
        .unwrap_or(host);
    if host != "reddit.com" {
        return None;
    }
    let parts: Vec<&str> = u.path().split('/').filter(|s| !s.is_empty()).collect();
    match parts.as_slice() {
        ["r", sub, "comments", id, ..] if valid_sub(sub) && valid_id(id) => {
            Some((sub.to_string(), id.to_ascii_lowercase()))
        }
        _ => None,
    }
}

fn valid_sub(s: &str) -> bool {
    (2..=21).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn valid_id(s: &str) -> bool {
    (1..=12).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// An `r/<name>` named in a search query, and the query without it.
fn subreddit_in(query: &str) -> Option<(String, String)> {
    let words: Vec<&str> = query.split_whitespace().collect();
    let at = words.iter().position(|w| {
        let w = w.trim_start_matches('/');
        w.len() > 2
            && w[..2].eq_ignore_ascii_case("r/")
            && valid_sub(w[2..].trim_end_matches(['/', ',', '.']))
    })?;
    let sub = words[at].trim_start_matches('/')[2..]
        .trim_end_matches(['/', ',', '.'])
        .to_string();
    let rest: Vec<&str> = words
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != at)
        .map(|(_, w)| *w)
        .collect();
    Some((sub, rest.join(" ")))
}

// ---------------------------------------------------------------------------
// Who reads a thread, and with which login

/// Who wants a thread: an agent that named it, or a run that picked it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Read {
    /// Live first, through the main login.
    Asked,
    /// The archive first, through the run account.
    Picked,
}

/// Per-process counts, used only outside a run (the run's own counts are in
/// its folder, per round).
static LIVE_READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static MAIN_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn take(kind: &str, counter: &std::sync::atomic::AtomicUsize, n: usize, max: usize) -> bool {
    use std::sync::atomic::Ordering::SeqCst;
    match crate::run::raw::current() {
        Some(rec) => rec.take_use(kind, n, max),
        None => counter.try_update(SeqCst, SeqCst, |c| (c + n <= max).then_some(c + n)).is_ok(),
    }
}

/// Count `n` calls the main login makes because the run account failed;
/// false once this round's allowance (`research.redditmainmax`) is spent.
pub fn take_main_calls(n: usize) -> bool {
    take("main_calls", &MAIN_CALLS, n, crate::config::get().research.redditmainmax)
}

fn take_live_read() -> bool {
    take("live_reads", &LIVE_READS, 1, crate::config::get().research.redditlivemax)
}

/// The account a run's own Reddit use goes through (`research.redditaccount`).
pub fn run_account() -> String {
    let a = crate::config::get().research.redditaccount.trim().to_string();
    if a.is_empty() {
        "main".into()
    } else {
        a
    }
}

/// Whether a Reddit login is saved under `name`.
pub fn has_login(name: &str) -> bool {
    crate::cookies::has_account("reddit.com", name)
}

/// The logins a live read tries in order, each with whether it spends the
/// run's main-login allowance.
fn read_accounts(read: Read) -> Vec<(String, bool)> {
    let run = run_account();
    let mut out: Vec<(String, bool)> = match read {
        Read::Asked => vec![("main".into(), false), (run.clone(), false)],
        Read::Picked => vec![(run.clone(), false), ("main".into(), run != "main")],
    };
    out.dedup_by(|a, b| a.0 == b.0);
    out.retain(|(a, _)| has_login(a));
    out
}

/// The command that logs the run account in again, when it is not main.
pub fn relogin_hint() -> String {
    let run = run_account();
    if run == "main" {
        "run `osearch login reddit.com`".into()
    } else {
        format!("log the {run} account in again: `osearch login reddit.com --as {run}`")
    }
}

/// Reddit's time window (`t=`) for `-f`, else `default`.
fn window(freshness: Option<&str>, default: &'static str) -> &'static str {
    match freshness {
        Some("day") => "day",
        Some("week") => "week",
        Some("month") => "month",
        Some("year") => "year",
        _ => default,
    }
}

/// A thread for the fetch tiers: who asked decides live or archive first.
pub async fn read_thread(ctx: Arc<AppContext>, url: &str, read: Read) -> Result<SearchResult, SearchError> {
    let out = Reddit::new(ctx).thread(url, read, Duration::from_secs(30)).await?;
    out.results.into_iter().next().ok_or_else(|| SearchError::Api {
        provider: "reddit",
        code: "no_thread",
        message: "no thread returned".into(),
        status: None,
    })
}

// ---------------------------------------------------------------------------
// Pacing: a token bucket on disk, shared by every search process.

/// Burst allowance; Reddit served a captcha after 14 requests in 10 s.
const BURST: f64 = 5.0;
/// How long the live route rests after a captcha (it cleared in about 7 min).
const COOLDOWN_SECS: f64 = 420.0;

/// The sustained rate in requests a second (`research.redditpace` a minute).
fn per_sec() -> f64 {
    crate::config::get().research.redditpace.max(1) as f64 / 60.0
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, serde::Serialize)]
pub struct Bucket {
    pub tokens: f64,
    /// Unix seconds of the last update.
    pub at: f64,
    /// Unix seconds until which the live route rests (after a captcha).
    #[serde(default)]
    pub cooldown_until: f64,
}

impl Bucket {
    pub fn full(now: f64) -> Self {
        Self {
            tokens: BURST,
            at: now,
            cooldown_until: 0.0,
        }
    }

    /// Take one token at `now`, refilling at `per_sec`, or say how long
    /// until one is free.
    pub fn take(&mut self, now: f64, per_sec: f64) -> Result<(), Duration> {
        let elapsed = (now - self.at).max(0.0);
        self.tokens = (self.tokens + elapsed * per_sec).min(BURST);
        self.at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - self.tokens) / per_sec))
        }
    }

    pub fn resting(&self, now: f64) -> Option<Duration> {
        (self.cooldown_until > now).then(|| Duration::from_secs_f64(self.cooldown_until - now))
    }
}

static PACE: Mutex<()> = Mutex::new(());

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// The shared bucket (the pace, and the rest after a captcha, which is per
/// address); with side-by-side searches each account also paces its own
/// requests in a bucket of its own.
fn pace_path(account: Option<&str>) -> std::path::PathBuf {
    let dir = crate::cache::cache_dir();
    match account {
        Some(a) => {
            let name: String = a.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
            dir.join(format!("reddit-pace-{name}.json"))
        }
        None => dir.join("reddit-pace.json"),
    }
}

fn load_bucket(path: &std::path::Path, now: f64) -> Bucket {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| Bucket::full(now))
}

fn save_bucket(path: &std::path::Path, b: &Bucket) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("pace");
    if let Ok(text) = serde_json::to_string(b) {
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// Wait for a live-request token, at most `max_wait`. `Err` says why not:
/// resting after a captcha, or the next token is too far off.
async fn acquire(max_wait: Duration) -> Result<(), String> {
    let rc = &crate::config::get().research;
    let rate = per_sec();
    loop {
        let wait = {
            let _guard = PACE.lock().unwrap_or_else(|e| e.into_inner());
            let now = now_secs();
            let shared = pace_path(None);
            let mut b = load_bucket(&shared, now);
            if let Some(rest) = b.resting(now) {
                return Err(format!(
                    "reddit served a captcha recently; live pages rest for {} more s",
                    rest.as_secs()
                ));
            }
            let own = rc.redditsidebyside.then(|| pace_path(Some(&crate::cookies::current_account())));
            let path = match &own {
                Some(p) => {
                    b = load_bucket(p, now);
                    p.clone()
                }
                None => shared,
            };
            match b.take(now, rate) {
                Ok(()) => {
                    save_bucket(&path, &b);
                    return Ok(());
                }
                Err(wait) => wait,
            }
        };
        if wait > max_wait {
            return Err(format!(
                "paced: about {} Reddit requests a minute; the next is free in {} s",
                rc.redditpace.max(1),
                wait.as_secs() + 1
            ));
        }
        tokio::time::sleep(wait).await;
    }
}

const BROWSER_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";

/// Keyless Reddit pages and their spacing, shared by every osearch process.
#[derive(Clone, Copy)]
enum Keyless {
    Rss,
    Listing,
}

impl Keyless {
    fn gap(self) -> f64 {
        match self {
            Keyless::Rss => 61.0,
            Keyless::Listing => 2.0,
        }
    }
    fn file(self) -> &'static str {
        match self {
            Keyless::Rss => "reddit-rss-pace.json",
            Keyless::Listing => "reddit-listing-pace.json",
        }
    }
}

/// Reserve the next slot of `kind`, waiting at most `max_wait` for it.
async fn rss_slot(kind: Keyless, max_wait: Duration) -> Result<(), String> {
    let wait = {
        let _guard = PACE.lock().unwrap_or_else(|e| e.into_inner());
        let now = now_secs();
        let path = crate::cache::cache_dir().join(kind.file());
        let next: f64 = std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0.0);
        let at = next.max(now);
        if at - now > max_wait.as_secs_f64() {
            return Err(format!(
                "paced: Reddit's RSS search answers once a minute without a login; the next is free in {} s",
                (at - now).ceil()
            ));
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, format!("{}", at + kind.gap()));
        at - now
    };
    if wait > 0.0 {
        tokio::time::sleep(Duration::from_secs_f64(wait)).await;
    }
    Ok(())
}

/// A Reddit page without a login, paced by its kind.
async fn rss_get(ctx: &AppContext, kind: Keyless, url: &str, timeout: Duration) -> Result<String, String> {
    let route = match kind {
        Keyless::Rss => "rss",
        Keyless::Listing => "listing",
    };
    if let Some(note) = refused_note(route) {
        return Err(format!("reddit {route}: {note}"));
    }
    rss_slot(kind, timeout.saturating_sub(Duration::from_secs(4))).await?;
    let started = std::time::Instant::now();
    let resp = ctx
        .client
        .get(url)
        .header("User-Agent", BROWSER_UA)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("reddit rss: {}", e.without_url()))?;
    let status = resp.status().as_u16();
    let body = resp.text().await.map_err(|e| format!("reddit rss: {e}"))?;
    crate::run::raw::record_page("reddit", url, status, started.elapsed(), &body);
    match status {
        200..=299 => Ok(body),
        429 => Err("reddit rss: HTTP 429 (rate limit)".into()),
        s => {
            if s == 403 {
                mark_refused(route);
            }
            Err(format!("reddit rss: HTTP {s}"))
        }
    }
}

/// Reddit's own search through RSS: everywhere, or inside one subreddit;
/// `freshness` limits it to that period.
pub async fn rss_search(
    ctx: &AppContext,
    query: &str,
    sub: Option<&str>,
    timeout: Duration,
    freshness: Option<&str>,
) -> Result<Vec<SearchResult>, String> {
    let q: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    let t = window(freshness, "all");
    let url = match sub {
        Some(s) => format!("{LIVE}/r/{s}/search.rss?q={q}&restrict_sr=on&sort=relevance&t={t}&limit=25"),
        None => format!("{LIVE}/search.rss?q={q}&sort=relevance&t={t}&limit=25"),
    };
    let body = rss_get(ctx, Keyless::Rss, &url, timeout).await?;
    // Reddit's own matching is loose ("selling AI chat logs" brought WoW
    // addon threads): keep what is on topic.
    Ok(crate::relevance::keep(parse_rss(&body), query))
}

/// Threads of a search feed (Atom): title, address, date, subreddit and
/// the post's opening text. RSS carries no vote counts.
pub fn parse_rss(xml: &str) -> Vec<SearchResult> {
    static ENTRY: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?s)<entry>(.*?)</entry>").expect("regex"));
    static TAG: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?s)<title>(.*?)</title>|<link href="([^"]+)"|<updated>([^<]+)</updated>|<category term="([^"]+)"|<content type="html">(.*?)</content>|<name>/u/([^<]+)</name>"#).expect("regex")
    });
    let mut out = Vec::new();
    for e in ENTRY.captures_iter(xml) {
        let mut f: [String; 6] = Default::default();
        for c in TAG.captures_iter(&e[1]) {
            for (i, slot) in f.iter_mut().enumerate() {
                if let Some(m) = c.get(i + 1) {
                    *slot = unescape(m.as_str());
                }
            }
        }
        let [title, link, date, sub, content, author] = f;
        if thread_ref(&link).is_none() {
            continue;
        }
        let text = thread::html_to_text(&content);
        let text = text.trim_start_matches("submitted by").trim();
        out.push(SearchResult {
            title,
            url: link,
            snippet: if text.is_empty() { format!("r/{sub}") } else { thread::clip(text, 500) },
            source: "reddit".into(),
            published: (!date.is_empty()).then_some(date),
            image_url: None,
            extra: Some(serde_json::json!({ "subreddit": sub, "author": author, "route": "rss" })),
        });
    }
    out
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&")
}

/// A subreddit's top threads of the past year with their vote and comment
/// counts (the listing partial serves them without a login).
pub async fn subreddit_top(ctx: &AppContext, sub: &str, timeout: Duration) -> Result<Vec<SearchResult>, String> {
    let url = format!("{LIVE}/svc/shreddit/community-more-posts/top/?name={sub}&t=year");
    let body = rss_get(ctx, Keyless::Listing, &url, timeout).await?;
    let doc = Document::from(body.as_str());
    let mut out = Vec::new();
    for node in doc.select("shreddit-post").iter() {
        let attr = |n: &str| node.attr(n).map(|v| v.to_string()).unwrap_or_default();
        let link = format!("{LIVE}{}", attr("permalink"));
        if thread_ref(&link).is_none() {
            continue;
        }
        let score: i64 = attr("score").parse().unwrap_or(0);
        let comments: i64 = attr("comment-count").parse().unwrap_or(0);
        out.push(SearchResult {
            title: attr("post-title"),
            url: link,
            snippet: format!("r/{sub} · {score} votes · {comments} comments"),
            source: "reddit".into(),
            published: Some(attr("created-timestamp")).filter(|d| !d.is_empty()),
            image_url: None,
            extra: Some(serde_json::json!({ "subreddit": sub, "score": score, "num_comments": comments, "author": attr("author"), "route": "listing" })),
        });
    }
    Ok(out)
}

fn start_cooldown() {
    let _guard = PACE.lock().unwrap_or_else(|e| e.into_inner());
    let now = now_secs();
    let path = pace_path(None);
    let mut b = load_bucket(&path, now);
    b.cooldown_until = now + COOLDOWN_SECS;
    b.tokens = 0.0;
    b.at = now;
    save_bucket(&path, &b);
}

/// Routes Reddit refused (HTTP 403), remembered across runs for
/// `research.redditrefused` hours: route name to unix seconds.
fn refused_path() -> std::path::PathBuf {
    crate::cache::cache_dir().join("reddit-refused.json")
}

fn refused_map() -> HashMap<String, f64> {
    std::fs::read_to_string(refused_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Why `route` is skipped now, when Reddit refused it on a recent run.
fn refused_note(route: &str) -> Option<String> {
    if crate::config::get().research.redditrefused == 0 {
        return None;
    }
    let until = refused_map().get(route).copied().filter(|t| *t > now_secs())?;
    Some(format!(
        "skipped: Reddit refused it (HTTP 403) on a recent run; tried again in {} (research.redditrefused)",
        age(until - now_secs())
    ))
}

fn mark_refused(route: &str) {
    let hours = crate::config::get().research.redditrefused;
    if hours == 0 {
        return;
    }
    let _guard = PACE.lock().unwrap_or_else(|e| e.into_inner());
    let mut map = refused_map();
    map.insert(route.to_string(), now_secs() + hours as f64 * 3600.0);
    let path = refused_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string(&map) {
        let _ = std::fs::write(path, text);
    }
}

// ---------------------------------------------------------------------------
// Live fetches

/// The TLS fingerprints the live route tries, in order. Reddit's edge
/// blocks some fingerprints outright (403) and not others; a captcha is per
/// IP, so only a 403 moves on to the next profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    #[cfg_attr(not(feature = "stealth"), allow(dead_code))]
    Chrome,
    #[cfg_attr(not(feature = "stealth"), allow(dead_code))]
    Safari,
    /// Built without the `stealth` feature: the plain HTTP client.
    #[cfg_attr(feature = "stealth", allow(dead_code))]
    Plain,
}

pub fn live_profiles() -> &'static [Profile] {
    #[cfg(feature = "stealth")]
    {
        &[Profile::Chrome, Profile::Safari]
    }
    #[cfg(not(feature = "stealth"))]
    {
        &[Profile::Plain]
    }
}

/// What a live response turned out to be.
#[derive(Debug, PartialEq, Eq)]
pub enum Live {
    /// The page holds the expected elements.
    Content(String),
    /// A real page with none of them (no comments, no hits).
    Empty,
    /// A captcha or interstitial, whatever the status.
    Challenge,
    /// Refused outright by status (403, 429, ...).
    Refused(u16),
}

/// Judge a live response. `marker` is an element every real page of this
/// kind contains when it has anything to show.
pub fn judge(status: u16, body: &str, marker: &str) -> Live {
    if status == 200 && body.contains(marker) {
        return Live::Content(body.to_string());
    }
    let lower = body.to_ascii_lowercase();
    let captcha = [
        "recaptcha",
        "g-recaptcha",
        "hcaptcha",
        "blocked by network security",
        "whoa there, pardner",
        "verify you are human",
        "prove your humanity",
    ]
    .iter()
    .any(|m| lower.contains(m));
    match status {
        200 if captcha => Live::Challenge,
        200 => Live::Empty,
        s => Live::Refused(s),
    }
}

async fn get_live(
    ctx: &AppContext,
    profile: Profile,
    url: &str,
    timeout: Duration,
) -> Result<(u16, String), String> {
    match profile {
        #[cfg(feature = "stealth")]
        Profile::Chrome | Profile::Safari => {
            let emulation = match profile {
                Profile::Safari => wreq_util::Profile::Safari26,
                _ => wreq_util::Profile::Chrome149,
            };
            let client = wreq::Client::builder()
                .emulation(emulation)
                .cookie_store(true)
                .connect_timeout(Duration::from_secs(5))
                .build()
                .map_err(|e| e.to_string())?;
            let fut = async {
                let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
                let status = resp.status().as_u16();
                let body = resp.text().await.map_err(|e| e.to_string())?;
                Ok::<_, String>((status, body))
            };
            tokio::time::timeout(timeout, fut)
                .await
                .map_err(|_| "timed out".to_string())?
        }
        _ => {
            let resp = ctx
                .client
                .get(url)
                .timeout(timeout)
                .send()
                .await
                .map_err(|e| e.without_url().to_string())?;
            let status = resp.status().as_u16();
            let body = resp.text().await.map_err(|e| e.to_string())?;
            Ok((status, body))
        }
    }
}

/// One live page through the profiles, paced. `Ok(None)` is a real page
/// with nothing on it.
/// Set once Reddit refuses the live route (HTTP 403 on every fingerprint):
/// later calls in this process skip it instead of spending the pace budget.
static LIVE_REFUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

async fn live_page(
    ctx: &AppContext,
    url: &str,
    marker: &str,
    timeout: Duration,
) -> Result<Option<String>, String> {
    if LIVE_REFUSED.load(std::sync::atomic::Ordering::Relaxed) {
        return Err("live route skipped: Reddit refused it earlier in this run (HTTP 403)".into());
    }
    if let Some(note) = refused_note("live") {
        return Err(format!("live route {note}"));
    }
    let deadline = tokio::time::Instant::now() + timeout;
    let mut why = String::new();
    for profile in live_profiles() {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        acquire(
            left.saturating_sub(Duration::from_secs(3))
                .min(Duration::from_secs(2)),
        )
        .await?;
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return Err("timed out".into());
        }
        let (status, body) = get_live(ctx, *profile, url, left).await?;
        match judge(status, &body, marker) {
            Live::Content(body) => return Ok(Some(body)),
            Live::Empty => return Ok(None),
            Live::Challenge => {
                start_cooldown();
                return Err(
                    "challenge: reddit served a captcha page; live pages rest for 7 min".into(),
                );
            }
            Live::Refused(s) => why = format!("HTTP {s} from reddit ({profile:?} fingerprint)"),
        }
        if !matches!(why.as_str(), w if w.starts_with("HTTP 403")) {
            break;
        }
    }
    if why.starts_with("HTTP 403") {
        LIVE_REFUSED.store(true, std::sync::atomic::Ordering::Relaxed);
        mark_refused("live");
    }
    Err(why)
}

/// Which routes a call may use, from `SEARCH_REDDIT_ROUTE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Routes {
    pub live: bool,
    pub archive: bool,
}

pub fn routes_from(value: Option<&str>) -> Routes {
    match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("archive") => Routes {
            live: false,
            archive: true,
        },
        Some("live") => Routes {
            live: true,
            archive: false,
        },
        _ => Routes {
            live: true,
            archive: true,
        },
    }
}

fn routes() -> Routes {
    match std::env::var("SEARCH_REDDIT_ROUTE").ok() {
        Some(v) => routes_from(Some(&v)),
        None if !crate::config::get().research.redditlive => routes_from(Some("archive")),
        None => routes_from(None),
    }
}

// ---------------------------------------------------------------------------
// Parsing the live partials

/// Every `<shreddit-comment>` of a comments partial, in page order.
pub fn parse_live_comments(html: &str) -> Vec<Comment> {
    let doc = Document::from(html);
    let mut out = Vec::new();
    for node in doc.select("shreddit-comment").iter() {
        let attr = |name: &str| node.attr(name).map(|v| v.to_string()).unwrap_or_default();
        let thing = attr("thingid");
        let Some(id) = thing.strip_prefix("t1_").map(str::to_string) else {
            continue;
        };
        let parent = node
            .attr("parentid")
            .and_then(|p| p.strip_prefix("t1_").map(str::to_string));
        let body_sel = format!("div[id=\"{thing}-comment-rtjson-content\"]");
        let body_node = doc.select(&body_sel);
        let body = if body_node.exists() {
            thread::html_to_text(&body_node.html())
        } else {
            "[deleted]".to_string()
        };
        out.push(Comment {
            id,
            parent,
            author: attr("author"),
            score: attr("score").parse().ok(),
            depth: attr("depth").parse().unwrap_or(0),
            created: attr("created").replace("+0000", "Z"),
            body,
        });
    }
    out
}

#[derive(Debug, Deserialize, Default)]
struct Tracking {
    #[serde(default)]
    post: TrackPost,
    #[serde(default)]
    profile: TrackName,
    #[serde(default)]
    subreddit: TrackName,
    #[serde(default)]
    search: TrackSearch,
}

#[derive(Debug, Deserialize, Default)]
struct TrackPost {
    #[serde(default)]
    id: String,
}

#[derive(Debug, Deserialize, Default)]
struct TrackName {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize, Default)]
struct TrackSearch {
    #[serde(default)]
    snippet: String,
}

/// Posts of a search partial.
pub fn parse_live_search(html: &str) -> Vec<SearchResult> {
    let doc = Document::from(html);
    // The tracking context names the author, subreddit and snippet per post.
    let mut tracking: HashMap<String, Tracking> = HashMap::new();
    for node in doc.select("search-telemetry-tracker").iter() {
        let Some(raw) = node.attr("data-faceplate-tracking-context") else {
            continue;
        };
        if let Ok(t) = serde_json::from_str::<Tracking>(&raw) {
            if !t.post.id.is_empty() {
                let entry = tracking.entry(t.post.id.clone()).or_default();
                if entry.search.snippet.is_empty() {
                    entry.search.snippet = t.search.snippet;
                }
                if entry.profile.name.is_empty() {
                    entry.profile.name = t.profile.name;
                }
                if entry.subreddit.name.is_empty() {
                    entry.subreddit.name = t.subreddit.name;
                }
            }
        }
    }
    let mut out = Vec::new();
    for unit in doc.select("[data-testid=\"sdui-post-unit\"]").iter() {
        let title = unit.select("a[data-testid=\"post-title-text\"]");
        let Some(href) = title.attr("href") else {
            continue;
        };
        let Some((sub, id)) = thread_ref(&format!("{LIVE}{href}")) else {
            continue;
        };
        let numbers: Vec<i64> = unit
            .select("[data-testid=\"search-counter-row\"] faceplate-number")
            .iter()
            .filter_map(|n| n.attr("number").and_then(|v| v.parse().ok()))
            .collect();
        let published = unit
            .select("faceplate-timeago")
            .attr("ts")
            .map(|t| t.replace("+0000", "Z"));
        let t = tracking.remove(&format!("t3_{id}")).unwrap_or_default();
        let subreddit = if t.subreddit.name.is_empty() {
            sub
        } else {
            t.subreddit.name
        };
        let mut extra = serde_json::json!({ "subreddit": subreddit });
        let obj = extra.as_object_mut().expect("object");
        if let Some(score) = numbers.first() {
            obj.insert("score".into(), (*score).into());
        }
        if let Some(n) = numbers.get(1) {
            obj.insert("num_comments".into(), (*n).into());
        }
        if !t.profile.name.is_empty() {
            obj.insert("author".into(), t.profile.name.into());
        }
        let snippet = if t.search.snippet.is_empty() {
            format!("r/{subreddit}")
        } else {
            t.search.snippet
        };
        out.push(SearchResult {
            title: title.text().trim().to_string(),
            url: format!("{LIVE}{href}"),
            snippet: thread::clip(&snippet, 500),
            source: "reddit".into(),
            published,
            image_url: None,
            extra: Some(extra),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Parsing the archive

#[derive(Debug, Deserialize)]
struct ArchiveList<T> {
    #[serde(default = "Vec::new")]
    data: Vec<T>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ArchivePost {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    score: Option<i64>,
    #[serde(default)]
    num_comments: Option<u64>,
    #[serde(default)]
    subreddit: String,
    #[serde(default)]
    created_utc: Option<f64>,
    #[serde(default)]
    permalink: String,
    #[serde(default)]
    selftext: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    is_self: Option<bool>,
    /// When the archive first saved it (absent in Reddit's own answer).
    #[serde(default)]
    retrieved_on: Option<f64>,
}

/// What the archive's copy says about its own age.
#[derive(Debug, Clone, Copy, Default)]
struct PostMeta {
    created: Option<f64>,
    retrieved: Option<f64>,
    num_comments: Option<u64>,
}

fn archive_post(p: ArchivePost) -> Post {
    let url = format!("{LIVE}{}", p.permalink);
    let link = p
        .url
        .filter(|u| p.is_self != Some(true) && !u.is_empty() && *u != url);
    Post {
        title: p.title,
        url,
        author: p.author,
        score: p.score,
        num_comments: p.num_comments,
        community: format!("r/{}", p.subreddit),
        created: p
            .created_utc
            .map(|t| thread::iso_from_unix(t as i64))
            .unwrap_or_default(),
        body: if matches!(p.selftext.as_str(), "[removed]" | "[deleted]") {
            p.selftext
        } else {
            thread::html_to_text(&p.selftext)
        },
        link,
    }
}

fn archive_error(e: impl std::fmt::Display) -> String {
    format!("archive: {e}")
}

fn parse_archive_post(body: &str) -> Result<(Post, PostMeta), String> {
    let list: ArchiveList<ArchivePost> = serde_json::from_str(body).map_err(archive_error)?;
    if let Some(e) = list.error {
        return Err(archive_error(e));
    }
    list.data
        .into_iter()
        .next()
        .map(|p| {
            let meta = PostMeta { created: p.created_utc, retrieved: p.retrieved_on, num_comments: p.num_comments };
            (archive_post(p), meta)
        })
        .ok_or_else(|| archive_error("no such post"))
}

#[derive(Debug, Deserialize)]
struct ArchiveNode {
    kind: String,
    data: serde_json::Value,
}

/// Every comment of an archive tree, with parent links; `more` stubs (the
/// rest of a long thread the archive did not send) are counted, not kept.
fn parse_archive_tree(body: &str) -> Result<(Vec<Comment>, u64), String> {
    let list: ArchiveList<ArchiveNode> = serde_json::from_str(body).map_err(archive_error)?;
    if let Some(e) = list.error {
        return Err(archive_error(e));
    }
    Ok(tree_from(list.data))
}

/// Reddit's own JSON of a thread (`[post listing, comment listing]`, the
/// same comment shape as the archive's tree): the post, its comments and the
/// count the answer left out.
pub fn parse_live_json(body: &str) -> Result<(Post, Vec<Comment>, u64), String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("not JSON ({e})"))?;
    let post = v
        .pointer("/0/data/children/0/data")
        .cloned()
        .ok_or("no post in the answer")?;
    let post: ArchivePost = serde_json::from_value(post).map_err(|e| e.to_string())?;
    let nodes: Vec<ArchiveNode> = v
        .pointer("/1/data/children")
        .and_then(|c| serde_json::from_value(c.clone()).ok())
        .unwrap_or_default();
    let (comments, more) = tree_from(nodes);
    Ok((archive_post(post), comments, more))
}

/// One thread live from Reddit, through whichever login the task carries.
async fn live_json(sub: &str, id: &str, timeout: Duration) -> Result<(Post, Vec<Comment>, u64), String> {
    acquire(Duration::from_secs(12)).await?;
    let url = format!("https://old.reddit.com/r/{sub}/comments/{id}/.json?raw_json=1&limit=500");
    let client = crate::page::client().map_err(|e| e.to_string())?;
    let started = std::time::Instant::now();
    let p = crate::page::fetch_direct_with(&client, &url, timeout, true)
        .await
        .map_err(|e| e.to_string())?;
    let account = format!("account: {}", crate::cookies::current_account());
    crate::run::raw::record_page_as("reddit-live", &url, p.status, started.elapsed(), &p.body, Some(account));
    if !(200..300).contains(&p.status) {
        return Err(format!("HTTP {}", p.status));
    }
    parse_live_json(&p.body)
}

/// Comments of a listing's children, with parent links; `more` stubs (the
/// rest of a long thread the answer did not send) are counted, not kept.
fn tree_from(nodes: Vec<ArchiveNode>) -> (Vec<Comment>, u64) {
    let mut out = Vec::new();
    let mut more = 0u64;
    let mut stack: Vec<(ArchiveNode, usize)> =
        nodes.into_iter().rev().map(|n| (n, 0)).collect();
    while let Some((node, depth)) = stack.pop() {
        if node.kind != "t1" {
            more += node.data["count"].as_u64().unwrap_or(0);
            continue;
        }
        let d = node.data;
        let s = |k: &str| d[k].as_str().unwrap_or("").to_string();
        let parent = d["parent_id"]
            .as_str()
            .and_then(|p| p.strip_prefix("t1_"))
            .map(str::to_string);
        out.push(Comment {
            id: s("id"),
            parent,
            author: s("author"),
            score: d["score"].as_i64(),
            depth,
            created: d["created_utc"]
                .as_f64()
                .map(|t| thread::iso_from_unix(t as i64))
                .unwrap_or_default(),
            body: thread::html_to_text(&s("body")),
        });
        if let Some(children) = d["replies"]["data"]["children"].as_array() {
            for child in children.iter().rev() {
                if let Ok(n) = serde_json::from_value::<ArchiveNode>(child.clone()) {
                    stack.push((n, depth + 1));
                }
            }
        }
    }
    (out, more)
}

/// Why the archive's copy of a thread a run picked needs a live read, if it does.
fn stale(post: &Result<(Post, PostMeta), String>, tree: &Result<(Vec<Comment>, u64), String>) -> Option<String> {
    let Ok((_, meta)) = post else {
        return Some("the archive has no copy".into());
    };
    if let Some(created) = meta.created {
        let hours = (now_secs() - created) / 3600.0;
        if hours < 72.0 {
            return Some(format!("the post is {} old", age(hours * 3600.0)));
        }
    }
    let n = meta.num_comments.unwrap_or(0);
    let have = tree.as_ref().map_or(0, |t| t.0.len());
    (n > 0 && have == 0).then(|| format!("the archive has none of its {n} comments"))
}

/// `8 s`, `3 h`, `2 days`.
fn age(secs: f64) -> String {
    let s = secs.max(0.0);
    if s < 90.0 {
        format!("{} s", s.round())
    } else if s < 5400.0 {
        format!("{} min", (s / 60.0).round())
    } else if s < 172_800.0 {
        format!("{} h", (s / 3600.0).round())
    } else if s < 60.0 * 86_400.0 {
        format!("{} days", (s / 86_400.0).round())
    } else if s < 730.0 * 86_400.0 {
        format!("{} months", (s / (30.4 * 86_400.0)).round())
    } else {
        format!("{} years", (s / (365.25 * 86_400.0)).round())
    }
}

/// The stamp of a thread served by the archive alone.
fn archive_stamp(meta: &PostMeta, not_live: Option<&str>) -> String {
    // A young post: how soon the archive saved it matters; an old one: its age.
    let mut s = match (meta.created, meta.retrieved) {
        (Some(c), Some(r)) if now_secs() - c < 3.0 * 86_400.0 => format!(
            "Reddit archive copy (Arctic Shift) of a post {} old, saved {} after posting; {}comments may be missing",
            age(now_secs() - c),
            age(r - c),
            // Under a day and a half old, the archive's score is a snapshot.
            if now_secs() - c < 36.0 * 3600.0 { "its score is still pending and " } else { "" }
        ),
        (Some(c), _) => format!(
            "Reddit archive copy (Arctic Shift) of a post {} old; scores as the archive last saved them",
            age(now_secs() - c)
        ),
        _ => "Reddit archive copy (Arctic Shift); scores as the archive last saved them".to_string(),
    };
    if let Some(why) = not_live.filter(|w| !w.is_empty()) {
        s.push_str(&format!(" (not read live: {why})"));
    }
    s
}

/// What a live read brought.
struct LiveThread {
    post: Option<Post>,
    comments: Vec<Comment>,
    more: u64,
    note: Option<String>,
}

fn parse_archive_search(body: &str) -> Result<Vec<SearchResult>, String> {
    let list: ArchiveList<ArchivePost> = serde_json::from_str(body).map_err(archive_error)?;
    if let Some(e) = list.error {
        return Err(archive_error(e));
    }
    Ok(list
        .data
        .into_iter()
        .map(|p| {
            let post = archive_post(p);
            let snippet = if post.body.is_empty() {
                post.community.clone()
            } else {
                thread::clip(&post.body, 500)
            };
            SearchResult {
                title: post.title.clone(),
                url: post.url.clone(),
                snippet,
                source: "reddit".into(),
                published: (!post.created.is_empty()).then(|| post.created.clone()),
                image_url: None,
                extra: Some(serde_json::json!({
                    "subreddit": post.community.trim_start_matches("r/"),
                    "score": post.score,
                    "num_comments": post.num_comments,
                    "author": post.author,
                    "route": "archive",
                })),
            }
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Merging a thread

/// The thread from whatever answered: the archive tree as the skeleton, the
/// live page's current scores and bodies over it, live comments the archive
/// lacks added. Returns the ordered comments and the route that served it.
pub fn merge(
    live: Option<Vec<Comment>>,
    archive: Option<Vec<Comment>>,
) -> Option<(Vec<Comment>, &'static str)> {
    let route = match (&live, &archive) {
        (Some(_), Some(_)) => "live+archive",
        (Some(_), None) => "live",
        (None, Some(_)) => "archive",
        (None, None) => return None,
    };
    let mut by_id: HashMap<String, Comment> = HashMap::new();
    for c in archive.into_iter().flatten() {
        by_id.insert(c.id.clone(), c);
    }
    for c in live.into_iter().flatten() {
        by_id.insert(c.id.clone(), c);
    }
    Some((thread::order(by_id.into_values().collect()), route))
}

fn slug_title(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| {
            u.path_segments()
                .and_then(|mut s| s.nth(4).map(|t| t.replace('_', " ")))
        })
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "Reddit thread".to_string())
}

// ---------------------------------------------------------------------------
// Calls

impl Reddit {
    /// Creation dates (YYYY-MM-DD) of threads by post id, from the archive,
    /// at most 100 ids a call.
    pub async fn archive_dates(&self, ids: &[String]) -> HashMap<String, String> {
        let mut out = HashMap::new();
        if !routes().archive {
            return out;
        }
        for chunk in ids.chunks(100) {
            let path = format!("posts/ids?ids={}", chunk.join(","));
            let Ok(body) = self.archive_get(&path, Duration::from_secs(15)).await else { continue };
            let Ok(list) = serde_json::from_str::<ArchiveList<ArchivePost>>(&body) else { continue };
            for p in list.data {
                if let Some(t) = p.created_utc {
                    let date: String = thread::iso_from_unix(t as i64).chars().take(10).collect();
                    out.insert(p.id.to_ascii_lowercase(), date);
                }
            }
        }
        out
    }

    async fn archive_get(&self, path: &str, timeout: Duration) -> Result<String, String> {
        let url = format!("{ARCHIVE}/{path}");
        // The archive refuses bursts (HTTP 429, "Too many requests"): wait
        // and try again within the call's time, staggered so the calls of
        // one burst do not all come back together.
        let started = std::time::Instant::now();
        let stagger = Duration::from_millis(url.bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64)) % 900);
        let mut wait = Duration::from_millis(1200) + stagger;
        loop {
            match super::send(self, timeout, || self.ctx.client.get(&url)).await {
                Ok(resp) => return resp.text().await.map_err(archive_error),
                Err(e) if e.to_string().contains("429") && started.elapsed() + wait < timeout => {
                    tokio::time::sleep(wait).await;
                    wait *= 2;
                }
                Err(e) => return Err(archive_error(e)),
            }
        }
    }

    /// The archive's post and comment tree, at once.
    async fn archive_half(
        &self,
        id: &str,
        timeout: Duration,
    ) -> (Result<(Post, PostMeta), String>, Result<(Vec<Comment>, u64), String>) {
        if !routes().archive {
            let off = "archive off (SEARCH_REDDIT_ROUTE=live)".to_string();
            return (Err(off.clone()), Err(off));
        }
        let post = async {
            self.archive_get(&format!("posts/ids?ids={id}"), timeout)
                .await
                .and_then(|b| parse_archive_post(&b))
        };
        let tree = async {
            self.archive_get(&format!("comments/tree?link_id=t3_{id}&limit=9999"), timeout)
                .await
                .and_then(|b| parse_archive_tree(&b))
        };
        tokio::join!(post, tree)
    }

    /// The thread live: Reddit's JSON through the saved logins in `read`'s
    /// order, else Reddit's logged-out page when that route is on.
    async fn live_half(&self, sub: &str, id: &str, read: Read, timeout: Duration) -> Result<LiveThread, String> {
        let run = run_account();
        let mut why: Vec<String> = Vec::new();
        for (acct, spends_main) in read_accounts(read) {
            if spends_main && !take_main_calls(1) {
                why.push(format!(
                    "main login: this run's {} calls are used",
                    crate::config::get().research.redditmainmax
                ));
                continue;
            }
            match crate::cookies::with_account(&acct, live_json(sub, id, timeout)).await {
                Ok((post, comments, more)) => {
                    let note = (!why.is_empty()).then(|| {
                        let mut n = format!("read live with the {acct} login after: {}", why.join("; "));
                        if acct != run {
                            n.push_str(&format!("; {}", relogin_hint()));
                        }
                        n
                    });
                    return Ok(LiveThread { post: Some(post), comments, more, note });
                }
                Err(e) => why.push(format!("{acct} login: {e}")),
            }
        }
        if routes().live {
            let live_url = format!("{LIVE}/svc/shreddit/comments/r/{sub}/t3_{id}");
            match live_page(&self.ctx, &live_url, "<shreddit-comment", timeout).await {
                Ok(page) => {
                    let comments = page.map(|p| parse_live_comments(&p)).unwrap_or_default();
                    return Ok(LiveThread { post: None, comments, more: 0, note: None });
                }
                Err(e) => why.push(format!("logged out: {e}")),
            }
        }
        Err(if why.is_empty() { "no saved Reddit login".into() } else { why.join("; ") })
    }

    async fn thread(&self, url: &str, read: Read, timeout: Duration) -> Result<Output, SearchError> {
        let Some((sub, id)) = thread_ref(url) else {
            return Err(SearchError::Api {
                provider: "reddit",
                code: "not_a_thread",
                message: "reddit extracts only reddit.com/r/<sub>/comments/<id> thread URLs".into(),
                status: None,
            });
        };
        let mut notes: Vec<String> = Vec::new();
        let ((post, tree), live) = match read {
            // Asked for: live; the archive only when live failed or left
            // comments out (it can take 10 s to answer).
            Read::Asked => {
                let live = self.live_half(&sub, &id, read, timeout).await;
                let archive = match &live {
                    Ok(l) if l.more == 0 => (Err(String::new()), Err(String::new())),
                    _ => self.archive_half(&id, timeout).await,
                };
                (archive, live)
            }
            // Picked by a run: the archive; live only for a young or incomplete copy.
            Read::Picked => {
                let (post, tree) = self.archive_half(&id, timeout).await;
                let live = match stale(&post, &tree) {
                    None => Err(String::new()),
                    Some(why) if take_live_read() => self
                        .live_half(&sub, &id, read, timeout)
                        .await
                        .map_err(|e| format!("{why}, and the live read failed: {e}")),
                    Some(why) => Err(format!(
                        "{why}, but this run's {} live reads are used",
                        crate::config::get().research.redditlivemax
                    )),
                };
                ((post, tree), live)
            }
        };
        let (live, live_err) = match live {
            Ok(l) => (Some(l), None),
            Err(e) => (None, Some(e)),
        };
        let (tree, archive_more) = match tree {
            Ok((c, more)) => (Some(c), more),
            Err(e) => {
                if routes().archive && !e.is_empty() {
                    notes.push(format!("archive tree unavailable: {e}"));
                }
                (None, 0)
            }
        };
        let (archive_post, meta) = match post {
            Ok((p, m)) => (Some(p), Some(m)),
            Err(_) => (None, None),
        };
        let (live_post, live_comments, more) = match live {
            Some(l) => {
                notes.extend(l.note);
                (l.post, Some(l.comments), l.more)
            }
            None => (None, None, archive_more),
        };
        if more > 0 {
            notes.push(format!("{more} more comments exist that the answer did not include"));
        }
        let Some((comments, route)) = merge(live_comments, tree) else {
            let message = live_err.filter(|e| !e.is_empty()).unwrap_or_else(|| "no source answered".into());
            let category = if message.starts_with("challenge") {
                FailureCategory::Rejected
            } else if message.starts_with("paced") {
                FailureCategory::RateLimit
            } else {
                FailureCategory::Network
            };
            return Err(SearchError::Upstream {
                provider: "reddit",
                status: 0,
                category,
                message: format!("{message}; {}", notes.join("; ")),
            });
        };
        let post = match live_post.or(archive_post) {
            Some(p) => p,
            None => {
                notes.push("post details unavailable".into());
                let canonical = format!("{LIVE}/r/{sub}/comments/{id}/");
                Post {
                    title: slug_title(url),
                    url: canonical,
                    community: format!("r/{sub}"),
                    ..Post::default()
                }
            }
        };
        let mut result = thread::result("reddit", &post, &comments, route, &notes);
        if route == "archive" {
            let stamp = archive_stamp(&meta.unwrap_or_default(), live_err.as_deref());
            if let Some(extra) = result.extra.as_mut() {
                extra["stamp"] = serde_json::json!(stamp);
            }
        }
        Ok(Output {
            results: vec![result],
            ..Output::default()
        })
    }

    async fn search(&self, call: &Call<'_>) -> Result<Output, SearchError> {
        let routes = routes();
        let archive_search = routes.archive && crate::config::get().research.redditarchive;
        let mut why = Vec::new();
        // A subreddit named: the archive first (the live search is often refused).
        let mut archive_tried = false;
        if archive_search {
            if let Some((sub, rest)) = subreddit_in(call.query) {
                archive_tried = true;
                let title: String = url::form_urlencoded::byte_serialize(rest.as_bytes()).collect();
                let path = format!(
                    "posts/search?subreddit={sub}&title={title}&limit={}&sort=desc",
                    call.count
                );
                match self.archive_get(&path, call.timeout).await.and_then(|b| parse_archive_search(&b)) {
                    Ok(results) if !results.is_empty() => {
                        return Ok(Output {
                            results,
                            ..Output::default()
                        })
                    }
                    Ok(_) => why.push(format!("archive: no r/{sub} threads match")),
                    Err(e) => why.push(e),
                }
            }
        }
        if routes.live {
            let q: String = url::form_urlencoded::byte_serialize(call.query.as_bytes()).collect();
            let url = format!("{LIVE}/svc/shreddit/search/?q={q}&type=posts");
            match live_page(&self.ctx, &url, "post-title-text", call.timeout).await {
                Ok(page) => {
                    let mut results = page.map(|p| parse_live_search(&p)).unwrap_or_default();
                    results.truncate(call.count as usize);
                    return Ok(Output {
                        results,
                        ..Output::default()
                    });
                }
                Err(e) => why.push(e),
            }
        }
        // Reddit's own search through RSS, without a login.
        let (sub, rest) = match subreddit_in(call.query) {
            Some((s, r)) => (Some(s), r),
            None => (None, call.query.to_string()),
        };
        match rss_search(&self.ctx, &rest, sub.as_deref(), call.timeout, call.opts.freshness.as_deref()).await {
            Ok(mut results) => {
                results.truncate(call.count as usize);
                return Ok(Output {
                    results,
                    ..Output::default()
                });
            }
            Err(e) => why.push(e),
        }
        if archive_search && !archive_tried {
            match subreddit_in(call.query) {
                Some((sub, rest)) => {
                    let title: String =
                        url::form_urlencoded::byte_serialize(rest.as_bytes()).collect();
                    let path = format!(
                        "posts/search?subreddit={sub}&title={title}&limit={}&sort=desc",
                        call.count
                    );
                    match self
                        .archive_get(&path, call.timeout)
                        .await
                        .and_then(|b| parse_archive_search(&b))
                    {
                        Ok(results) => {
                            return Ok(Output {
                                results,
                                ..Output::default()
                            })
                        }
                        Err(e) => why.push(e),
                    }
                }
                None => why.push(
                    "archive search needs a subreddit: add r/<name> to the query".to_string(),
                ),
            }
        }
        // Reddit refused and the query names no subreddit: a keyless web
        // search scoped to reddit.com threads.
        if subreddit_in(call.query).is_none() {
            match web_fallback(call).await {
                Ok(out) => return Ok(out),
                Err(e) => why.push(e),
            }
        }
        let message = why.join("; ");
        let category = if message.starts_with("challenge") {
            FailureCategory::Rejected
        } else if message.starts_with("paced") || message.starts_with("reddit served a captcha") {
            FailureCategory::RateLimit
        } else {
            FailureCategory::Network
        };
        Err(SearchError::Upstream {
            provider: "reddit",
            status: 0,
            category,
            message,
        })
    }
}

/// Old Reddit's search page read with the saved Reddit login (the plain
/// fetcher, then the browser-like one): thread links with points and
/// comment counts. Used by community mode only when the keyless routes
/// found few threads (`research.redditlogin`). `freshness` limits it to that
/// period, still by relevance.
pub async fn login_search(
    query: &str,
    count: usize,
    deeper: bool,
    freshness: Option<&str>,
) -> Result<Vec<SearchResult>, String> {
    let q: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    let first = format!("https://old.reddit.com/search?q={q}&sort=relevance&t={}", window(freshness, "all"));
    let (mut out, next) = login_page(&first, count).await?;
    if deeper {
        // The run asked for more: the second page, and the year's top threads.
        let mut more = Vec::new();
        if let Some(n) = next {
            more.push(n);
        }
        more.push(format!("https://old.reddit.com/search?q={q}&sort=top&t={}", window(freshness, "year")));
        for url in more {
            match login_page(&url, count).await {
                Ok((list, _)) => {
                    for r in list {
                        if !out.iter().any(|o| o.url == r.url) {
                            out.push(r);
                        }
                    }
                }
                Err(e) => tracing::debug!("logged-in Reddit search page: {e}"),
            }
        }
    }
    Ok(out)
}

/// Old Reddit's search inside one subreddit with the saved login: top of
/// the past year (the threads there about the question, best-voted first),
/// or with `freshness` the most relevant of that period.
pub async fn login_subreddit_search(
    sub: &str,
    query: &str,
    count: usize,
    freshness: Option<&str>,
) -> Result<Vec<SearchResult>, String> {
    let q: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    let order = match freshness {
        Some(_) => format!("sort=relevance&t={}", window(freshness, "year")),
        None => "sort=top&t=year".to_string(),
    };
    let url = format!("https://old.reddit.com/r/{sub}/search?q={q}&restrict_sr=on&{order}");
    login_page(&url, count).await.map(|(list, _)| list)
}

/// A comment-search hit on Reddit's search page (one result unit).
const COMMENT_UNIT: &str = r#"view-events="search/view/comment""#;
/// Reddit's own "no results" unit on a search page.
const NO_RESULTS_UNIT: &str = r#"view-events="search/view/no_results""#;

/// Reddit's comment search with the saved login (the search page's
/// fragment, as the site loads it): threads whose answer is in a reply.
/// Reddit applies a time window to comment search only when it is sorted
/// by date, so `freshness` sorts by new; without it, by relevance.
pub async fn login_comment_search(
    query: &str,
    count: usize,
    freshness: Option<&str>,
) -> Result<Vec<SearchResult>, String> {
    let q: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    let order = match freshness {
        Some(_) => format!("sort=new&t={}", window(freshness, "all")),
        None => "sort=relevance&t=all".to_string(),
    };
    let url = format!("{LIVE}/svc/shreddit/search/?q={q}&type=comments&{order}");
    let mut last = String::new();
    for via in ["direct", "stealth"] {
        acquire(Duration::from_secs(12)).await?;
        let started = std::time::Instant::now();
        let page = login_get(&url, via).await;
        if let Ok((status, body)) = &page {
            let account = format!("account: {}", crate::cookies::current_account());
            crate::run::raw::record_page_as("reddit-login-comments", &url, *status, started.elapsed(), body, Some(account));
        }
        match page {
            Ok((status, body)) if (200..300).contains(&status) => {
                if body.contains(COMMENT_UNIT) {
                    let hits = parse_comment_search(&body, count);
                    if !hits.is_empty() {
                        return Ok(hits);
                    }
                    last = format!("{via}: Reddit's comment results no longer parse");
                } else if body.contains(NO_RESULTS_UNIT) {
                    return Ok(Vec::new());
                } else {
                    last = format!("{via}: not a comment search page (the login may have lapsed)");
                }
            }
            Ok((status, _)) => last = format!("{via}: HTTP {status}"),
            Err(e) => last = format!("{via}: {e}"),
        }
    }
    Err(last)
}

/// Threads of Reddit's comment-search page. Each result unit's tracker
/// carries its thread (id, title, subreddit) as JSON; the reply's text and
/// date follow in the unit. Adult and quarantined communities are dropped.
pub fn parse_comment_search(html: &str, count: usize) -> Vec<SearchResult> {
    static TAG: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"<search-telemetry-tracker\b[^>]*>").expect("regex"));
    static CONTEXT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"\bdata-faceplate-tracking-context="([^"]*)""#).expect("regex")
    });
    static TIMEAGO: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"<faceplate-timeago\b[^>]*?\bts="([^"]+)""#).expect("regex")
    });
    static TAGS: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"<[^>]+>").expect("regex"));
    let units: Vec<(usize, serde_json::Value)> = TAG
        .find_iter(html)
        .filter(|m| m.as_str().contains(COMMENT_UNIT))
        .filter_map(|m| {
            let c = CONTEXT.captures(m.as_str())?;
            let v: serde_json::Value = serde_json::from_str(&unescape(&c[1])).ok()?;
            Some((m.start(), v))
        })
        .collect();
    let text = |v: &serde_json::Value| v.as_str().unwrap_or("").to_string();
    let mut out: Vec<SearchResult> = Vec::new();
    for (i, (start, ctx)) in units.iter().enumerate() {
        let (post, sub, comment) = (&ctx["post"], &ctx["subreddit"], &ctx["comment"]);
        if post["nsfw"].as_bool() == Some(true)
            || sub["nsfw"].as_bool() == Some(true)
            || sub["quarantined"].as_bool() == Some(true)
        {
            continue;
        }
        let mut post_id = text(&post["id"]).trim_start_matches("t3_").to_ascii_lowercase();
        if post_id.is_empty() {
            post_id = text(&comment["post_id"]).trim_start_matches("t3_").to_ascii_lowercase();
        }
        let name = text(&sub["name"]);
        if !valid_id(&post_id) || !valid_sub(&name) {
            continue;
        }
        let url = format!("{LIVE}/r/{name}/comments/{post_id}/");
        if out.iter().any(|r| r.url == url) {
            continue;
        }
        let end = units.get(i + 1).map_or(html.len(), |u| u.0);
        let chunk = &html[*start..end];
        let cid = text(&comment["id"]).trim_start_matches("t1_").to_ascii_lowercase();
        let reply = chunk
            .find(&format!("id=\"search-comment-t1_{cid}-post-rtjson-content\""))
            .map(|at| {
                let from = chunk[at..].find('>').map_or(chunk.len(), |g| at + g + 1);
                let to = chunk[from..].find("</shreddit-stop-propagation>").map_or(chunk.len(), |e| from + e);
                let plain = unescape(&TAGS.replace_all(&chunk[from..to], " "));
                plain.split_whitespace().collect::<Vec<_>>().join(" ")
            })
            .unwrap_or_default();
        let date = chunk
            .find("data-testid=\"search-comment-content\"")
            .and_then(|at| TIMEAGO.captures(&chunk[at..]))
            .map(|c| c[1].chars().take(10).collect::<String>());
        let title = match text(&post["title"]) {
            t if t.trim().is_empty() => format!("r/{name} thread {post_id}"),
            t => unescape(t.trim()),
        };
        out.push(SearchResult {
            title,
            url,
            snippet: format!("r/{name} · a reply: {}", thread::clip(&reply, 300)),
            source: "reddit".into(),
            published: date,
            image_url: None,
            extra: Some(serde_json::json!({ "subreddit": name, "route": "login-comments" })),
        });
        if out.len() >= count {
            break;
        }
    }
    out
}

/// One page through the saved login: the plain fetcher, or the
/// browser-like one. (status, body)
async fn login_get(url: &str, via: &str) -> Result<(u16, String), String> {
    match via {
        "direct" => match crate::page::client() {
            Ok(client) => crate::page::fetch_direct_with(&client, url, Duration::from_secs(10), true)
                .await
                .map(|p| (p.status, p.body))
                .map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        },
        #[cfg(feature = "stealth")]
        _ => crate::providers::stealth::fetch_page(url, Duration::from_secs(10), true)
            .await
            .map(|p| (p.status, p.body))
            .map_err(|e| e.to_string()),
        #[cfg(not(feature = "stealth"))]
        _ => Err("stealth fetcher not built".to_string()),
    }
}

/// One old-Reddit search page through the saved login, paced with every
/// other Reddit request: its threads and the next page's address.
async fn login_page(
    url: &str,
    count: usize,
) -> Result<(Vec<SearchResult>, Option<String>), String> {
    static NEXT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\[next[^\]]*\]\((https://old\.reddit\.com/search\?[^)\s]+)\)").expect("regex")
    });
    let mut last = String::new();
    for via in ["direct", "stealth"] {
        acquire(Duration::from_secs(12)).await?;
        let started = std::time::Instant::now();
        // The raw page, converted whole: article extraction can keep one
        // result's post text and drop the list of results.
        let page = login_get(url, via).await;
        if let Ok((status, body)) = &page {
            let account = format!("account: {}", crate::cookies::current_account());
            crate::run::raw::record_page_as("reddit-login-search", url, *status, started.elapsed(), body, Some(account));
        }
        match page {
            Ok((status, body)) if (200..300).contains(&status) => {
                let (_, md) = crate::page::whole_page_markdown(&body, url);
                let results = parse_login_search(&md, count);
                if !results.is_empty() {
                    let next = NEXT.captures(&md).map(|c| c[1].replace("&amp;", "&"));
                    return Ok((results, next));
                }
                // A logged-in page that lists nothing is an empty result, not a failed login.
                if body.to_ascii_lowercase().contains("logout") {
                    return Ok((Vec::new(), None));
                }
                last = format!("{via}: no threads on the page");
            }
            Ok((status, _)) => last = format!("{via}: HTTP {status}"),
            Err(e) => last = format!("{via}: {e}"),
        }
    }
    Err(last)
}

/// Threads in old Reddit's search page as markdown: a `[title](thread url)`
/// line (a flair label may come before it), then "N points [M comments](...)
/// submitted ... to [r/sub]".
pub fn parse_login_search(md: &str, count: usize) -> Vec<SearchResult> {
    static LINK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\[([^\]]+)\]\((https://old\.reddit\.com/r/[^/]+/comments/[^)\s]+)\)").expect("regex")
    });
    static META: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^(-?[\d,]+) points?\s*\[([\d,]+) comments?\]\((https://old\.reddit\.com(/r/[^/]+/comments/[^)\s]+))\)").expect("regex")
    });
    let lines: Vec<&str> = md.lines().map(str::trim).collect();
    let mut out: Vec<SearchResult> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        // Every result has a points line; its title is the last link to the
        // same thread on the line above (a flair label or a thumbnail may
        // come before it).
        let Some(c) = META.captures(line) else { continue };
        let url = format!("{LIVE}{}", &c[4]);
        let Some((sub, _)) = thread_ref(&url) else { continue };
        if out.iter().any(|r| r.url == url) {
            continue;
        }
        let thread = &c[3];
        let title = lines[..i]
            .iter()
            .rev()
            .find(|l| !l.is_empty())
            .and_then(|l| LINK.captures_iter(l).filter(|m| &m[2] == thread).last())
            .map(|m| m[1].trim().to_string())
            .filter(|t| !t.is_empty());
        let Some(title) = title else { continue };
        let mut extra = serde_json::json!({ "subreddit": sub, "route": "login" });
        let obj = extra.as_object_mut().expect("object");
        if let Ok(s) = c[1].replace(',', "").parse::<i64>() {
            obj.insert("score".into(), s.into());
        }
        if let Ok(n) = c[2].replace(',', "").parse::<u64>() {
            obj.insert("num_comments".into(), n.into());
        }
        out.push(SearchResult {
            title,
            url,
            snippet: format!("r/{sub}"),
            source: "reddit".into(),
            published: submitted_date(line),
            image_url: None,
            extra: Some(extra),
        });
        if out.len() >= count {
            break;
        }
    }
    out
}

/// The date in old Reddit's "submitted 16 days ago" text (YYYY-MM-DD).
fn submitted_date(line: &str) -> Option<String> {
    static AGO: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"submitted (\d+|an?) (minute|hour|day|month|year)s? ago").expect("regex")
    });
    let c = AGO.captures(line)?;
    let n: i64 = c[1].parse().unwrap_or(1);
    let days = match &c[2] {
        "minute" | "hour" => 0,
        "day" => n,
        "month" => n * 30,
        _ => n * 365,
    };
    Some((chrono::Local::now() - chrono::Duration::days(days)).format("%Y-%m-%d").to_string())
}

/// Reddit threads found by donsetch's keyless web search (`site:reddit.com`).
async fn web_fallback(call: &Call<'_>) -> Result<Output, String> {
    let donsetch = super::donsetch::Donsetch;
    if !donsetch.is_configured() {
        return Err("web fallback: donsetch is not installed".into());
    }
    let q = format!("{} site:reddit.com", call.query);
    let opts = crate::types::SearchOpts::default();
    let c = Call {
        mode: Mode::General,
        query: &q,
        count: call.count.min(12),
        opts: &opts,
        timeout: call.timeout,
    };
    let out = donsetch
        .call(&c)
        .await
        .map_err(|e| format!("web fallback: {e}"))?;
    let results: Vec<SearchResult> = out
        .results
        .into_iter()
        .filter(|r| thread_ref(&r.url).is_some())
        .map(|mut r| {
            r.source = "reddit".into();
            r.extra = Some(serde_json::json!({ "route": "web" }));
            r
        })
        .collect();
    if results.is_empty() {
        return Err("web fallback: no reddit threads found".into());
    }
    Ok(Output {
        results,
        ..Output::default()
    })
}

#[async_trait]
impl Adapter for Reddit {
    fn spec(&self) -> &'static ProviderSpec {
        &SPEC
    }

    fn is_configured(&self) -> bool {
        true // keyless
    }

    async fn call(&self, call: &Call<'_>) -> Result<Output, SearchError> {
        match call.mode {
            Mode::Community => self.search(call).await,
            Mode::Extract | Mode::Scrape => self.thread(call.query, Read::Asked, call.timeout).await,
            other => Err(super::direct::unsupported("reddit", other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMENTS: &str = include_str!("../../tests/fixtures/reddit/comments.html");
    const SEARCH: &str = include_str!("../../tests/fixtures/reddit/search.html");
    const ARCHIVE_POST: &str = include_str!("../../tests/fixtures/reddit/arctic_post.json");
    const ARCHIVE_TREE: &str = include_str!("../../tests/fixtures/reddit/arctic_tree.json");
    const CAPTCHA: &str = include_str!("../../tests/fixtures/reddit/captcha.html");

    #[test]
    fn live_comments_carry_author_score_depth_parent_and_time() {
        let comments = parse_live_comments(COMMENTS);
        assert!(comments.len() >= 8, "{}", comments.len());
        let first = &comments[0];
        assert_eq!(first.id, "kcxj4ur");
        assert_eq!(first.author, "Vova_xX");
        assert_eq!(first.score, Some(66));
        assert_eq!(first.depth, 0);
        assert_eq!(first.parent, None);
        assert!(first.created.starts_with("2023-12-11T18:59:35"));
        assert!(first.body.contains("keychron V3"), "{}", first.body);
        // A reply's body is its own, not its parent's.
        let reply = &comments[1];
        assert_eq!(reply.parent.as_deref(), Some("kcxj4ur"));
        assert_eq!(reply.depth, 1);
        assert!(
            !reply.body.contains("keychron V3, as 60%"),
            "{}",
            reply.body
        );
    }

    #[test]
    fn live_search_reads_posts_with_votes_comments_and_subreddit() {
        let results = parse_live_search(SEARCH);
        assert!(results.len() >= 3, "{}", results.len());
        let r = &results[0];
        assert!(r.url.starts_with("https://www.reddit.com/r/"), "{}", r.url);
        assert!(!r.title.is_empty());
        let extra = r.extra.as_ref().unwrap();
        assert!(extra["score"].is_i64(), "{extra}");
        assert!(extra["num_comments"].is_i64(), "{extra}");
        assert!(extra["subreddit"].is_string());
        assert!(r.published.is_some());
    }

    #[test]
    fn a_captcha_page_is_a_challenge_never_content() {
        assert_eq!(judge(200, CAPTCHA, "<shreddit-comment"), Live::Challenge);
        assert_eq!(judge(403, CAPTCHA, "<shreddit-comment"), Live::Refused(403));
        assert!(matches!(
            judge(200, COMMENTS, "<shreddit-comment"),
            Live::Content(_)
        ));
        // A real page with nothing on it is empty, not a challenge.
        assert_eq!(
            judge(200, "<html><body>ok</body></html>", "<shreddit-comment"),
            Live::Empty
        );
        // A thread that mentions recaptcha in a comment is still content.
        let talk = format!("{COMMENTS}<p>how do I add reCAPTCHA?</p>");
        assert!(matches!(
            judge(200, &talk, "<shreddit-comment"),
            Live::Content(_)
        ));
    }

    #[test]
    fn the_bucket_allows_a_burst_then_about_ten_a_minute() {
        const TEN: f64 = 10.0 / 60.0;
        let mut b = Bucket::full(1000.0);
        for _ in 0..5 {
            assert!(b.take(1000.0, TEN).is_ok());
        }
        let wait = b.take(1000.0, TEN).unwrap_err();
        assert!((5.5..=6.5).contains(&wait.as_secs_f64()), "{wait:?}");
        // Six seconds later one more token is free, and only one.
        assert!(b.take(1006.1, TEN).is_ok());
        assert!(b.take(1006.2, TEN).is_err());
        // A minute of quiet refills about ten, capped at the burst.
        let mut taken = 0;
        let mut b = Bucket {
            tokens: 0.0,
            at: 0.0,
            cooldown_until: 0.0,
        };
        let mut t = 0.0;
        while t < 60.0 {
            if b.take(t, TEN).is_ok() {
                taken += 1;
            }
            t += 0.5;
        }
        assert!((9..=11).contains(&taken), "{taken}");
        let resting = Bucket {
            cooldown_until: 500.0,
            ..Bucket::full(100.0)
        };
        assert!(resting.resting(100.0).is_some());
        assert!(resting.resting(501.0).is_none());
    }

    #[test]
    fn archive_post_and_tree_parse() {
        let (post, _) = parse_archive_post(ARCHIVE_POST).unwrap();
        assert_eq!(post.title, "Keyboards that you recommend for programming?");
        // Reddit's own thread JSON (saved 2026-11-10): post, every comment, tree links.
        let (live, comments, more) = parse_live_json(include_str!("../../tests/fixtures/reddit/live_thread.json")).unwrap();
        assert_eq!((live.score, live.num_comments, comments.len(), more), (Some(13), Some(9), 9, 0));
        assert!(comments.iter().any(|c| c.depth == 1 && c.parent.is_some()));
        assert_eq!(post.community, "r/webdev");
        assert_eq!(post.num_comments, Some(222));
        let (tree, _) = parse_archive_tree(ARCHIVE_TREE).unwrap();
        assert!(tree.len() >= 10, "{}", tree.len());
        assert_eq!(tree[0].id, "kcxj4ur");
        assert!(tree.iter().any(|c| c.depth == 1 && c.parent.is_some()));
        assert!(parse_archive_post("{\"data\":null,\"error\":\"Timeout\"}").is_err());
    }

    /// The live route first, then the archive; the merged thread keeps
    /// live scores, and the archive alone serves it when live fails.
    #[test]
    fn fallback_order_and_merge() {
        #[cfg(feature = "stealth")]
        assert_eq!(live_profiles(), &[Profile::Chrome, Profile::Safari]);
        #[cfg(not(feature = "stealth"))]
        assert_eq!(live_profiles(), &[Profile::Plain]);

        let live = parse_live_comments(COMMENTS);
        let (archive, _) = parse_archive_tree(ARCHIVE_TREE).unwrap();
        let live_score = live[0].score;
        let (both, route) = merge(Some(live.clone()), Some(archive.clone())).unwrap();
        assert_eq!(route, "live+archive");
        assert!(both.len() >= live.len().max(archive.len()));
        let first = both.iter().find(|c| c.id == "kcxj4ur").unwrap();
        assert_eq!(first.score, live_score);

        let (only_archive, route) = merge(None, Some(archive.clone())).unwrap();
        assert_eq!(route, "archive");
        assert_eq!(only_archive.len(), archive.len());
        assert_eq!(merge(Some(live), None).unwrap().1, "live");
        assert!(merge(None, None).is_none());

        assert_eq!(
            routes_from(None),
            Routes {
                live: true,
                archive: true
            }
        );
        assert_eq!(
            routes_from(Some("archive")),
            Routes {
                live: false,
                archive: true
            }
        );
        assert_eq!(
            routes_from(Some("LIVE")),
            Routes {
                live: true,
                archive: false
            }
        );
    }

    #[test]
    fn thread_urls_and_subreddit_cues() {
        let want = Some(("webdev".to_string(), "18g0wcw".to_string()));
        for u in [
            "https://www.reddit.com/r/webdev/comments/18g0wcw/keyboards_that_you_recommend_for_programming/",
            "https://old.reddit.com/r/webdev/comments/18g0wcw/",
            "https://reddit.com/r/webdev/comments/18g0wcw",
        ] {
            assert_eq!(thread_ref(u), want, "{u}");
        }
        assert_eq!(thread_ref("https://www.reddit.com/r/webdev/"), None);
        assert_eq!(thread_ref("https://evilreddit.com/r/a/comments/b/"), None);
        assert_eq!(
            subreddit_in("r/rust async runtimes"),
            Some(("rust".into(), "async runtimes".into()))
        );
        assert_eq!(subreddit_in("rust async"), None);
        assert_eq!(
            slug_title("https://www.reddit.com/r/webdev/comments/18g0wcw/keyboards_that_you/"),
            "keyboards that you"
        );
    }

    #[test]
    fn login_search_reads_threads_with_a_flair_label_before_the_title() {
        let md = "Need Help [How do you handle backups](https://old.reddit.com/r/selfhosted/comments/1xfqpq5/how_do/)\n\n\
                  15 points [35 comments](https://old.reddit.com/r/selfhosted/comments/1xfqpq5/how_do/) submitted 16 days ago\n\n\
                  [![](https://preview.redd.it/x.jpeg)](https://old.reddit.com/r/selfhosted/comments/1vr3ukj/selfhosting_everything/)\n\n\
                  Meta Post[self-hosting everything](https://old.reddit.com/r/selfhosted/comments/1vr3ukj/selfhosting_everything/)\n\n\
                  18,650 points [403 comments](https://old.reddit.com/r/selfhosted/comments/1vr3ukj/selfhosting_everything/) submitted 2 months ago\n\n\
                  [Plain title](https://old.reddit.com/r/restic/comments/abc123/plain/)\n\n\
                  1 point [2 comments](https://old.reddit.com/r/restic/comments/abc123/plain/) submitted today\n";
        let found = parse_login_search(md, 10);
        let titles: Vec<&str> = found.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["How do you handle backups", "self-hosting everything", "Plain title"]);
        assert_eq!(found[1].extra.as_ref().unwrap()["score"], 18650);
        let days = |r: &SearchResult| {
            let d = chrono::NaiveDate::parse_from_str(r.published.as_deref().unwrap(), "%Y-%m-%d").unwrap();
            (chrono::Local::now().date_naive() - d).num_days()
        };
        assert_eq!(days(&found[0]), 16);
        assert_eq!(days(&found[1]), 60);
    }
}
