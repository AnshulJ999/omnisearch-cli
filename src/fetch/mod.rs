//! Getting one page, cheapest way first. Each tier runs only when the one
//! before failed, and every result is checked before it counts (status,
//! length, challenge text, the outside tool's own labels):
//!
//! - **0, the site's own data door:** Reddit and HN threads, GitHub through
//!   `gh`, StackExchange, npm / PyPI / crates.io, arXiv, YouTube transcripts
//!   (yt-dlp), the optional social platforms when the request names them.
//! - **1, no browser, Chrome's network fingerprint:** donsetch; the built-in
//!   direct and stealth fetchers as the safety net (and first, with saved
//!   cookies, since donsetch sends no sessions).
//! - **2, a browser, only for pages built by JavaScript:** moli (or Obscura)
//!   and headless Chromium over CDP, raced: the first good page wins.
//! - **3, hard bot walls:** crawl4ai with its browser profile, then the
//!   hosted readers (Jina, Firecrawl); otherwise an honest "bot wall".
//! - **Last, the Wayback Machine:** its newest saved copy when the page is
//!   gone, walled at every tier or behind a paywall, stamped with its date.
//!
//! A page not read live carries a stamp (`Fetched::stamp`): where it came
//! from and how old it is.
//!
//! Every outcome goes into the per-site memory (`memory`); a site where a
//! fetcher keeps failing starts after it, and the skipped fetchers are tried
//! last only if the rest fail too.

pub mod browser;
mod convert;
mod donsetch;
pub mod memory;
mod sites;
mod wayback;

use crate::context::AppContext;
use crate::page::{self, Verdict};
use crate::tools::{self, Tool};
use regex::Regex;
use serde::Serialize;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

pub use convert::Loss;

/// What the caller wants from a fetch.
#[derive(Debug, Clone, Default)]
pub struct FetchOpts {
    /// Optional ScrapeCreators platforms the request named: their URLs are
    /// read through them at tier 0.
    pub platforms: Vec<String>,
    /// Allow the paid hosted readers at tier 3.
    pub no_paid: bool,
    /// Keep the page's links (for snowballing and crawling).
    pub links: bool,
    /// One named fetcher only (`wayback` included).
    pub via: Option<String>,
    /// Fetchers not to try (already tried before a hand-off).
    pub skip: Vec<String>,
    /// An agent asked for this page by address (not a page a run picked).
    pub asked: bool,
    /// Readers that keep a cache read the page fresh.
    pub fresh: bool,
    /// Attempts are copied here as they finish, so a fetch stopped midway
    /// still says what it tried.
    pub progress: Option<Arc<std::sync::Mutex<Vec<Attempt>>>>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct Attempt {
    pub tier: u8,
    pub fetcher: String,
    pub outcome: String,
    pub ms: u128,
}

/// One page, fetched or not.
#[derive(Debug, Clone, Serialize)]
pub struct Fetched {
    pub url: String,
    pub final_url: String,
    pub title: String,
    #[serde(skip)]
    pub markdown: String,
    pub ok: bool,
    pub tier: u8,
    pub fetcher: String,
    /// Why it failed: bot_wall, login_wall, not_found, paywall, js_page,
    /// no_such_host, unreachable, empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    pub message: String,
    pub attempts: Vec<Attempt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loss: Option<Loss>,
    pub cost_micros: u64,
    /// Where the copy came from: `read live <time>`, or an archive, reader
    /// cache or Wayback copy with how old it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stamp: Option<String>,
    /// What the fetcher noted about the page (missing comments, a login that
    /// stepped in).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The page's own date, when its metadata gives one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
}

/// The start of a stamp for a page read live.
pub const LIVE: &str = "read live ";

/// A tier's content, before the final check.
struct Got {
    final_url: String,
    title: String,
    markdown: String,
    loss: Option<Loss>,
    cost_micros: u64,
    stamp: Option<String>,
    note: Option<String>,
    published: Option<String>,
}

/// How a tier failed: decides where the next attempt goes.
#[derive(Debug, Clone, PartialEq)]
enum Miss {
    /// Built by JavaScript: try a browser.
    JsPage,
    /// A bot wall or challenge: tier 3.
    Wall,
    /// A sign-in wall.
    Login,
    /// 404/410: nothing to escalate to.
    NotFound,
    /// A paywall (402): never fetched around.
    Paywall,
    /// Too little text.
    Thin,
    /// The domain does not exist (DNS says so): final, no tier can help.
    NoHost,
    /// No answer, a crash, a tool missing or answering out of contract.
    Other,
}

impl Miss {
    fn code(&self) -> &'static str {
        match self {
            Miss::JsPage => "js_page",
            Miss::Wall => "bot_wall",
            Miss::Login => "login_wall",
            Miss::NotFound => "not_found",
            Miss::Paywall => "paywall",
            Miss::Thin => "empty",
            Miss::NoHost => "no_such_host",
            Miss::Other => "unreachable",
        }
    }

    fn from_verdict(v: &Verdict) -> Self {
        match v {
            Verdict::JsShell => Miss::JsPage,
            Verdict::Challenge => Miss::Wall,
            Verdict::Thin => Miss::Thin,
            Verdict::PaymentRequired { .. } => Miss::Paywall,
            Verdict::HttpError(404 | 410) => Miss::NotFound,
            Verdict::HttpError(401) => Miss::Login,
            Verdict::HttpError(403 | 429 | 503) => Miss::Wall,
            _ => Miss::Other,
        }
    }
}

type TierResult = Result<Got, (Miss, String)>;

struct Trail {
    url: String,
    attempts: Vec<Attempt>,
    shared: Option<Arc<std::sync::Mutex<Vec<Attempt>>>>,
}

impl Trail {
    fn new(url: &str, opts: &FetchOpts) -> Self {
        Self {
            url: url.to_string(),
            attempts: Vec::new(),
            shared: opts.progress.clone(),
        }
    }

    fn note(&mut self, tier: u8, fetcher: &str, started: Instant, outcome: &TierResult) {
        let text = match outcome {
            Ok(g) => format!("ok ({} chars)", g.markdown.len()),
            Err((m, msg)) => format!("{}: {}", m.code(), crate::providers::thread::clip(msg, 160)),
        };
        let a = Attempt {
            tier,
            fetcher: fetcher.to_string(),
            outcome: text,
            ms: started.elapsed().as_millis(),
        };
        if let Some(s) = &self.shared {
            s.lock().unwrap_or_else(|e| e.into_inner()).push(a.clone());
        }
        self.attempts.push(a);
    }

    fn done(self, tier: u8, fetcher: &str, got: Got) -> Fetched {
        Fetched {
            url: self.url,
            final_url: got.final_url,
            title: got.title,
            markdown: got.markdown,
            ok: true,
            tier,
            fetcher: fetcher.to_string(),
            failure: None,
            message: String::new(),
            attempts: self.attempts,
            loss: got.loss,
            cost_micros: got.cost_micros,
            stamp: got.stamp,
            note: got.note,
            published: got.published,
        }
    }

    fn failed(self, miss: &Miss, message: String) -> Fetched {
        Fetched {
            stamp: None,
            note: None,
            published: None,
            final_url: self.url.clone(),
            url: self.url,
            title: String::new(),
            markdown: String::new(),
            ok: false,
            tier: 0,
            fetcher: String::new(),
            failure: Some(miss.code().to_string()),
            message,
            attempts: self.attempts,
            loss: None,
            cost_micros: 0,
        }
    }
}

/// A network error saying the domain does not exist (not a passing failure).
pub(super) fn no_such_host(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    ["no such host", "name or service not known", "nodename nor servname", "nxdomain", "host could not be resolved"]
        .iter()
        .any(|p| m.contains(p))
}

fn net_miss(msg: String) -> (Miss, String) {
    if no_such_host(&msg) {
        (Miss::NoHost, msg)
    } else {
        (Miss::Other, msg)
    }
}

/// Inline `data:` images and links (base64 pictures): they carry no text
/// and inflate token counts. A link keeps its text.
static DATA_URI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(!?)\[([^\]]*)\]\(\s*data:[^)]*\)|data:[a-zA-Z]+/[a-zA-Z0-9.+-]+;base64,[A-Za-z0-9+/=]+")
        .expect("data uri pattern")
});

fn strip_inline_data(markdown: String) -> String {
    if !markdown.contains("data:") {
        return markdown;
    }
    DATA_URI
        .replace_all(&markdown, |c: &regex::Captures| match (c.get(1), c.get(2)) {
            (Some(bang), Some(text)) if bang.as_str().is_empty() => text.as_str().to_string(),
            _ => String::new(),
        })
        .into_owned()
}

/// The final check every tier's content passes: real text, not a challenge,
/// not a sign-in page.
fn check(requested: &str, mut got: Got) -> TierResult {
    got.markdown = strip_inline_data(got.markdown);
    let len = got.markdown.trim().chars().count();
    if len < page::verdict_min_chars() {
        return Err((Miss::Thin, format!("only {len} characters of text")));
    }
    match page::verdict(&got.markdown, Some("text/markdown"), 200, None) {
        Verdict::Ok | Verdict::Thin => {}
        v => return Err((Miss::from_verdict(&v), format!("content judged {}", v.code()))),
    }
    // XenForo's error page for refused clients comes with the whole forum
    // layout, too long for the challenge check.
    let title = got.title.trim().to_ascii_lowercase();
    if title.starts_with("oops! we ran into some problems") {
        return Err((Miss::Wall, "the forum served its error page".into()));
    }
    // A reader's copy of a challenge page keeps its title, not its markup.
    if title.starts_with("just a moment") || (title.starts_with("attention required") && title.contains("cloudflare")) {
        return Err((Miss::Wall, format!("a challenge page (\"{}\")", got.title.trim())));
    }
    if crate::cookies::looks_like_login_wall(requested, &got.final_url, &got.markdown) {
        return Err((Miss::Login, format!("a sign-in page at {}", got.final_url)));
    }
    if moved_to_front(requested, &got.final_url) {
        return Err((Miss::NotFound, format!("redirected to the site's front page ({})", got.final_url)));
    }
    Ok(got)
}

/// A deep address that lands on its site's front page: the page is gone
/// (removed articles often redirect home), so the Wayback copy is tried.
fn moved_to_front(requested: &str, final_url: &str) -> bool {
    let (Ok(a), Ok(b)) = (url::Url::parse(requested), url::Url::parse(final_url)) else {
        return false;
    };
    let base = |u: &url::Url| {
        let h = u.host_str().unwrap_or("").to_ascii_lowercase();
        let parts: Vec<&str> = h.split('.').collect();
        parts[parts.len().saturating_sub(2)..].join(".")
    };
    let segments = |u: &url::Url| u.path().split('/').filter(|s| !s.is_empty()).count();
    let front = segments(&b) == 0 || (segments(&b) == 1 && b.path().trim_matches('/').len() <= 5);
    base(&a) == base(&b) && segments(&a) >= 1 && front && b.query().is_none() && a.path().trim_matches('/').len() > 5
}

macro_rules! attempt {
    ($trail:expr, $tier:expr, $name:expr, $fut:expr) => {{
        let started = Instant::now();
        let r: TierResult = match $fut.await {
            Ok(g) => check(&$trail.url, g),
            Err(e) => Err(e),
        };
        $trail.note($tier, $name, started, &r);
        r
    }};
}

/// Fetch `url` through the tiers and record the outcome in the site memory.
/// Fetchers the memory says keep failing on this site run last, only when
/// the others fail too: a wrong judgement costs time, never the page.
pub async fn fetch(app: &Arc<AppContext>, url: &str, opts: &FetchOpts) -> Fetched {
    let remembered: Vec<String> = memory::skip_for(url).into_iter().filter(|s| !opts.skip.contains(s)).collect();
    let mut first = opts.clone();
    first.skip.extend(remembered.iter().cloned());
    let use_wayback = !opts.skip.iter().any(|s| s == "wayback");
    // A site that walls every fetcher: the Wayback lookup starts beside them.
    let early = use_wayback && memory::walls_us(url);
    let (mut f, mut snap) = if early {
        let (f, s) = tokio::join!(chain(app, url, &first), wayback::lookup(app, url));
        (f, Some(s))
    } else {
        (chain(app, url, &first).await, None)
    };
    if !f.ok && !remembered.is_empty() && !matches!(f.failure.as_deref(), Some("not_found" | "no_such_host" | "paywall")) {
        let mut again = opts.clone();
        again.skip.extend(f.attempts.iter().map(|a| a.fetcher.clone()));
        let g = chain(app, url, &again).await;
        let mut attempts = std::mem::take(&mut f.attempts);
        attempts.extend(g.attempts.clone());
        f = if g.ok { g } else { f };
        f.attempts = attempts;
    }
    if !f.ok && use_wayback && !f.message.starts_with("blocked:") {
        let started = Instant::now();
        let ts = match snap.take() {
            Some(s) => s,
            None => wayback::lookup(app, url).await,
        };
        let why = wayback::reason(f.failure.as_deref());
        let r = match ts {
            Ok(ts) => wayback::snapshot(url, &ts, why).await.and_then(|g| check(url, g)),
            Err(e) => Err((Miss::Other, e)),
        };
        let mut trail = Trail::new(url, opts);
        trail.attempts = std::mem::take(&mut f.attempts);
        trail.note(4, "wayback", started, &r);
        match r {
            Ok(g) => f = trail.done(4, "wayback", g),
            Err(_) => f.attempts = trail.attempts,
        }
    }
    memory::record(&f, true);
    live_stamp(&mut f);
    f
}

/// A page with no stamp of its own was read live: say when.
fn live_stamp(f: &mut Fetched) {
    if f.ok && f.stamp.is_none() {
        f.stamp = Some(format!("{LIVE}{}", chrono::Local::now().format("%Y-%m-%d %H:%M")));
    }
}

async fn chain(app: &Arc<AppContext>, url: &str, opts: &FetchOpts) -> Fetched {
    let mut trail = Trail::new(url, opts);
    let skip = |name: &str| opts.skip.iter().any(|s| s == name);
    if !crate::guard::allow_private() {
        if let Err(e) = crate::guard::assert_public_url(url).await {
            return trail.failed(&Miss::Other, format!("blocked: {e}"));
        }
    }

    // Tier 0: the site's own data door.
    if let Some((name, fut)) = sites::door(app, url, opts).filter(|(n, _)| !skip(n)) {
        match attempt!(trail, 0, name, fut) {
            Ok(g) => return trail.done(0, name, g),
            Err((m @ (Miss::NotFound | Miss::NoHost), msg)) => return trail.failed(&m, msg),
            Err(_) => {}
        }
    }

    let cookies = crate::cookies::allowed_site(url).is_some();
    let mut last = (Miss::Other, String::from("no fetcher answered"));
    let mut want_browser = false;
    let mut walled = false;

    // Tier 1. With saved cookies the built-in fetchers go first: donsetch
    // sends no sessions. Reddit threads are the exception: donsetch reads
    // them whole without a login, the logged-in page is thinner.
    let mut builtin_done = false;
    if cookies && crate::providers::reddit::thread_ref(url).is_none() {
        match builtin(&mut trail, url, true, &opts.skip).await {
            Ok((name, g)) => return trail.done(1, name, g),
            Err(m) if m.0 == Miss::NoHost => return trail.failed(&m.0, m.1),
            Err(m) => {
                builtin_done = true;
                want_browser |= m.0 == Miss::JsPage;
                walled |= m.0 == Miss::Wall;
                last = m;
            }
        }
    }
    // Forum threads: donsetch and the built-in fetchers run together and the
    // longer copy is kept (each loses replies on some forum software).
    let mut donsetch_done = false;
    if !builtin_done
        && !skip("donsetch")
        && tools::locate(Tool::Donsetch).is_some()
        && crate::config::get().research.forumboth
        && crate::rounds::forums::thread_like(url)
    {
        let started = Instant::now();
        let (d, b) = tokio::join!(donsetch::fetch(url, opts.links), builtin(&mut trail, url, cookies, &opts.skip));
        let d: TierResult = d.and_then(|g| check(&trail.url, g));
        trail.note(1, "donsetch", started, &d);
        builtin_done = true;
        donsetch_done = true;
        match (d, b) {
            (Ok(dg), Ok((name, bg))) => {
                let words = |g: &Got| g.markdown.split_whitespace().count();
                return if words(&bg) > words(&dg) {
                    trail.done(1, name, bg)
                } else {
                    trail.done(1, "donsetch", dg)
                };
            }
            (Ok(dg), Err(_)) => return trail.done(1, "donsetch", dg),
            (Err(_), Ok((name, bg))) => return trail.done(1, name, bg),
            (Err(dm), Err(bm)) => {
                if dm.0 == Miss::Login && !cookies {
                    return trail.failed(&Miss::Login, login_message(url, &dm.1));
                }
                for m in [&dm, &bm] {
                    match m.0 {
                        Miss::NotFound | Miss::Paywall | Miss::NoHost => return trail.failed(&m.0, m.1.clone()),
                        Miss::JsPage => want_browser = true,
                        Miss::Wall => walled = true,
                        _ => {}
                    }
                }
                last = bm;
            }
        }
    }
    if !donsetch_done && tools::locate(Tool::Donsetch).is_some() && !skip("donsetch") {
        match attempt!(trail, 1, "donsetch", donsetch::fetch(url, opts.links)) {
            Ok(g) => return trail.done(1, "donsetch", g),
            Err((miss, msg)) => {
                match miss {
                    Miss::NotFound | Miss::Paywall | Miss::NoHost => return trail.failed(&miss, msg),
                    Miss::Login if !cookies => {
                        return trail.failed(&Miss::Login, login_message(url, &msg))
                    }
                    Miss::JsPage => want_browser = true,
                    Miss::Wall => walled = true,
                    _ => {}
                }
                last = (miss, msg);
            }
        }
    }
    if !builtin_done && !want_browser {
        match builtin(&mut trail, url, cookies, &opts.skip).await {
            Ok((name, g)) => return trail.done(1, name, g),
            Err(m) => {
                match m.0 {
                    Miss::NotFound | Miss::Paywall | Miss::NoHost => return trail.failed(&m.0, m.1),
                    Miss::Wall => walled = true,
                    _ => {}
                }
                last = m;
            }
        }
    }

    // The Jina reader races the browsers (or, on a wall, crawl4ai): billed
    // in Jina tokens only, it never slows a page down.
    let mut jina = !opts.no_paid && reader_configured(app, "jina") && !skip("jina");

    // Tier 2: a browser, for pages JavaScript builds (and anything tier 1
    // could not judge). A wall goes straight to tier 3.
    if !walled {
        // moli takes the saved login; Obscura cannot, so it skips logged-in sites.
        let light = match browser::light() {
            Some(Tool::Obscura) if cookies => None,
            other => other,
        }
        .filter(|t| !skip(t.name()));
        let mut racers: Vec<Racer<'_>> = Vec::new();
        match light {
            Some(Tool::Obscura) => racers.push(("obscura", 2, Box::pin(browser::obscura(url)))),
            Some(t) => racers.push((t.name(), 2, Box::pin(browser::moli(url, cookies)))),
            None => {}
        }
        if browser::chromium_path().is_some() && !skip("chromium") {
            racers.push(("chromium", 2, Box::pin(browser::chromium(url, cookies))));
        }
        if jina && !racers.is_empty() {
            racers.push(("jina", 3, Box::pin(hosted_reader(app, url, "jina", opts.fresh))));
            jina = false;
        }
        match race(&mut trail, url, racers).await {
            Ok((name, tier, g)) => return trail.done(tier, name, g),
            Err(errors) => {
                if let Some(e) = errors.iter().find(|e| e.0 == Miss::NoHost) {
                    return trail.failed(&e.0, e.1.clone());
                }
                walled |= errors.iter().any(|e| e.0 == Miss::Wall);
                if let Some(e) = errors.into_iter().last() {
                    last = e;
                }
            }
        }
    }

    // Tier 3: hard walls.
    let mut racers: Vec<Racer<'_>> = Vec::new();
    if tools::locate(Tool::Crwl).is_some() && !skip("crawl4ai") {
        racers.push(("crawl4ai", 3, Box::pin(crawl4ai(url))));
    }
    if jina {
        racers.push(("jina", 3, Box::pin(hosted_reader(app, url, "jina", opts.fresh))));
    }
    match race(&mut trail, url, racers).await {
        Ok((name, tier, g)) => return trail.done(tier, name, g),
        Err(errors) => {
            if let Some(e) = errors.into_iter().last() {
                last = e;
            }
        }
    }
    if !opts.no_paid {
        for reader in ["firecrawl"] {
            if !reader_configured(app, reader) || skip(reader) {
                continue;
            }
            match attempt!(trail, 3, reader, hosted_reader(app, url, reader, opts.fresh)) {
                Ok(g) => return trail.done(3, reader, g),
                Err(e) => last = e,
            }
        }
    }

    let (miss, msg) = last;
    let message = match miss {
        Miss::Wall | Miss::Thin | Miss::Other if walled => format!(
            "bot wall on {}: no free tier got past it ({msg}); try another source for this page",
            host(url)
        ),
        Miss::Login => login_message(url, &msg),
        _ => msg,
    };
    let miss = if walled { Miss::Wall } else { miss };
    trail.failed(&miss, message)
}

type Racer<'a> = (&'static str, u8, std::pin::Pin<Box<dyn std::future::Future<Output = TierResult> + Send + 'a>>);

/// Fetchers at once: the first good page wins and the rest are stopped.
/// Every finished attempt is noted; `Err` lists their failures in order.
async fn race(trail: &mut Trail, url: &str, racers: Vec<Racer<'_>>) -> Result<(&'static str, u8, Got), Vec<(Miss, String)>> {
    use futures_util::stream::{FuturesUnordered, StreamExt};
    let mut set: FuturesUnordered<_> = racers
        .into_iter()
        .map(|(name, tier, fut)| async move {
            let started = Instant::now();
            (name, tier, started, fut.await.and_then(|g| check(url, g)))
        })
        .collect();
    let mut errors = Vec::new();
    while let Some((name, tier, started, r)) = set.next().await {
        trail.note(tier, name, started, &r);
        match r {
            Ok(g) => return Ok((name, tier, g)),
            Err(e) => errors.push(e),
        }
    }
    Err(errors)
}

/// Fetch `url` with one named fetcher only (`fetch --via`): for a page known
/// to need a browser, or to test one tier.
pub async fn fetch_via(app: &Arc<AppContext>, url: &str, via: &str, opts: &FetchOpts) -> Fetched {
    let mut trail = Trail::new(url, opts);
    if !crate::guard::allow_private() {
        if let Err(e) = crate::guard::assert_public_url(url).await {
            return trail.failed(&Miss::Other, format!("blocked: {e}"));
        }
    }
    let cookies = crate::cookies::allowed_site(url).is_some();
    let r = match via {
        "donsetch" => attempt!(trail, 1, "donsetch", donsetch::fetch(url, opts.links)),
        "direct" => attempt!(trail, 1, "direct", direct(url, cookies)),
        #[cfg(feature = "stealth")]
        "stealth" => attempt!(trail, 1, "stealth", stealth(url, cookies)),
        "moli" => attempt!(trail, 2, "moli", browser::moli(url, cookies)),
        "obscura" => attempt!(trail, 2, "obscura", browser::obscura(url)),
        "chromium" => attempt!(trail, 2, "chromium", browser::chromium(url, cookies)),
        "crawl4ai" => attempt!(trail, 3, "crawl4ai", crawl4ai(url)),
        "jina" | "firecrawl" => attempt!(trail, 3, via, hosted_reader(app, url, via, opts.fresh)),
        "wayback" => {
            let started = Instant::now();
            let r = match wayback::lookup(app, url).await {
                Ok(ts) => wayback::snapshot(url, &ts, "asked for").await.and_then(|g| check(url, g)),
                Err(e) => Err((Miss::Other, e)),
            };
            trail.note(4, "wayback", started, &r);
            r
        }
        "site" => match sites::door(app, url, opts) {
            Some((name, fut)) => attempt!(trail, 0, name, fut),
            None => Err((Miss::Other, "no site door for this URL".to_string())),
        },
        other => Err((
            Miss::Other,
            format!("unknown fetcher '{other}': site, donsetch, direct, stealth, moli, obscura, chromium, crawl4ai, jina, firecrawl, wayback"),
        )),
    };
    let tier = trail.attempts.last().map_or(0, |a| a.tier);
    let mut f = match r {
        Ok(g) => trail.done(tier, via, g),
        Err((m, msg)) => trail.failed(&m, msg),
    };
    live_stamp(&mut f);
    f
}

fn host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string())
}

fn login_message(url: &str, msg: &str) -> String {
    let site = crate::cookies::site_of(url).unwrap_or_else(|| host(url));
    format!("login wall on {site}: run `osearch login {site}` ({msg})")
}

/// Tier 1's safety net: the honest direct GET, then the stealth GET when
/// direct was refused. Cookies go along for allow-listed sites.
async fn builtin(
    trail: &mut Trail,
    url: &str,
    cookies: bool,
    skip: &[String],
) -> Result<(&'static str, Got), (Miss, String)> {
    let skipped = |n: &str| skip.iter().any(|s| s == n);
    let miss = if skipped("direct") {
        (Miss::Other, "direct skipped".to_string())
    } else {
        match attempt!(trail, 1, "direct", direct(url, cookies)) {
            Ok(g) => return Ok(("direct", g)),
            Err(e) => e,
        }
    };
    let refused = matches!(miss.0, Miss::Wall | Miss::Login | Miss::Other);
    if !refused || !cfg!(feature = "stealth") || skipped("stealth") {
        return Err(miss);
    }
    #[cfg(feature = "stealth")]
    {
        let r = attempt!(trail, 1, "stealth", stealth(url, cookies));
        return r.map(|g| ("stealth", g));
    }
    #[allow(unreachable_code)]
    Err(miss)
}

async fn direct(url: &str, cookies: bool) -> TierResult {
    let client = if crate::guard::allow_private() {
        page::client_allow_private()
    } else {
        page::client()
    }
    .map_err(|e| (Miss::Other, e.to_string()))?;
    let started = Instant::now();
    let p = page::fetch_direct_with(&client, url, Duration::from_secs(10), cookies)
        .await
        .map_err(|e| net_miss(e.to_string()))?;
    crate::run::raw::record_page("direct", url, p.status, started.elapsed(), &p.body);
    convert::from_page(p).await
}

#[cfg(feature = "stealth")]
async fn stealth(url: &str, cookies: bool) -> TierResult {
    let started = Instant::now();
    let p = crate::providers::stealth::fetch_page(url, Duration::from_secs(10), cookies)
        .await
        .map_err(|e| net_miss(e.to_string()))?;
    crate::run::raw::record_page("stealth", url, p.status, started.elapsed(), &p.body);
    convert::from_page(p).await
}

/// crawl4ai through `crwl`, with its browser profile, scrolled to the end so
/// content loaded on scroll is in.
async fn crawl4ai(url: &str) -> TierResult {
    let profile = crate::config::get().tools.crwlprofile.clone();
    let mut args = vec![url.to_string(), "-o".into(), "markdown".into(), "-c".into(), "scan_full_page=true".into()];
    if !profile.is_empty() {
        args.push("-p".into());
        args.push(profile);
    }
    let ran = tools::run(Tool::Crwl, &args, Duration::from_secs(30))
        .await
        .map_err(|e| (Miss::Other, e))?;
    if ran.timed_out {
        return Err((Miss::Other, "crwl timed out".into()));
    }
    if ran.code != Some(0) {
        let err = crate::providers::thread::clip(ran.stderr.trim(), 300);
        if ran.code.is_none() || ran.code.is_some_and(|c| c > 1) {
            tools::contract_warning("crwl", &format!("exit {:?}: {err}", ran.code));
        }
        return Err((Miss::Other, format!("crwl exit {:?}: {err}", ran.code)));
    }
    let markdown = strip_crwl_noise(&ran.stdout_text());
    let title = markdown
        .lines()
        .find_map(|l| l.trim().strip_prefix("# "))
        .map(str::to_string)
        .unwrap_or_else(|| url.to_string());
    Ok(Got {
        stamp: None,
        note: None,
        published: None,
        final_url: url.to_string(),
        title,
        markdown,
        loss: None,
        cost_micros: 0,
    })
}

/// crwl prints progress lines before the markdown on some versions.
fn strip_crwl_noise(out: &str) -> String {
    out.lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("[INIT]")
                || t.starts_with("[FETCH]")
                || t.starts_with("[SCRAPE]")
                || t.starts_with("[COMPLETE]")
                || t.starts_with("[INFO]"))
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn reader_configured(app: &Arc<AppContext>, name: &str) -> bool {
    crate::providers::find(name)
        .map(|s| !crate::providers::key_for(&app.config, s).is_empty())
        .unwrap_or(false)
}

/// A paid hosted reader through the extract mode, so its cost lands in the
/// search log like any provider call. `fresh`: no cached copy.
async fn hosted_reader(app: &Arc<AppContext>, url: &str, reader: &str, fresh: bool) -> TierResult {
    let only = Some(vec![reader.to_string()]);
    let opts = crate::types::SearchOpts { fresh, ..Default::default() };
    let resp = crate::engine::run(app.clone(), url, crate::types::Mode::Extract, 1, &only, &opts).await;
    match resp {
        Ok(r) => {
            crate::logging::log_search(&r);
            let cost = r.metadata.cost.as_ref().map_or(0, |c| c.micros);
            let Some(first) = r.results.into_iter().next() else {
                let why = r
                    .metadata
                    .provider_failures
                    .first()
                    .map(|f| f.reason.clone())
                    .unwrap_or_else(|| "no content".into());
                return Err((Miss::Other, why));
            };
            let mut stamp = first.extra.as_ref().and_then(|e| e["stamp"].as_str()).map(str::to_string);
            // Jina's reader may answer from its cache (only `fresh` turns it
            // off), so its copy is never "read live".
            if reader == "jina" && !fresh && stamp.is_none() {
                stamp = Some("Jina reader copy (its cache may have served it; age unknown)".to_string());
            }
            Ok(Got {
                stamp,
                note: None,
                published: None,
                final_url: first.url,
                title: first.title,
                markdown: first.snippet,
                loss: None,
                cost_micros: cost,
            })
        }
        Err(e) => {
            if let crate::errors::SearchError::AllProvidersFailed { failed, cost } = &e {
                crate::logging::log_failed_search(url, "extract", 0, failed, cost.as_ref());
            }
            Err((Miss::Other, crate::errors::redact_secrets(&e.to_string())))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::moved_to_front;

    #[test]
    fn a_removed_article_sent_to_the_front_page_counts_as_gone() {
        assert!(moved_to_front("https://www.anandtech.com/show/21000/some-review", "https://forums.anandtech.com/"));
        assert!(moved_to_front("https://example.com/blog/2024/post-title", "https://example.com/en/"));
        assert!(!moved_to_front("https://example.com/blog/post", "https://example.com/blog/post-renamed"));
        assert!(!moved_to_front("https://example.com/", "https://example.com/"));
        assert!(!moved_to_front("https://old.com/page-here", "https://new.org/"));
    }
}
