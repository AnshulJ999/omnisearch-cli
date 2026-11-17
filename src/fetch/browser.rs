//! Browsers: tier 2 (the light browser, moli or Obscura, then headless
//! Chromium over CDP) and the visible window `osearch login` opens.

use super::{convert, Miss, TierResult};
use crate::cookies::{self, Cookie};
use crate::tools::{self, Tool};
use chromiumoxide::cdp::browser_protocol::network::{Cookie as CdpCookie, CookieParam};
use chromiumoxide::{Browser, BrowserConfig, Handler};
use futures_util::StreamExt;
use std::path::PathBuf;
use std::time::Duration;

/// Below this many bytes a rendered page is nothing (Obscura returns one
/// byte on a bot wall, with no error).
const MIN_HTML_BYTES: usize = 512;

/// Scroll to the end in steps until the page stops growing (at most 12),
/// then back to the top.
const SCROLL_JS: &str = "(async () => { let last = 0; for (let i = 0; i < 12; i++) { window.scrollTo(0, document.body.scrollHeight); await new Promise(r => setTimeout(r, 350)); const h = document.body.scrollHeight; if (h === last) break; last = h; } window.scrollTo(0, 0); return last; })()";

/// The light browser of tier 2: `tools.lightbrowser` (moli by default,
/// obscura, or off), when installed.
pub fn light() -> Option<Tool> {
    let pick = crate::config::get().tools.lightbrowser.to_ascii_lowercase();
    let tool = match pick.as_str() {
        "obscura" => Tool::Obscura,
        "off" | "none" => return None,
        _ => Tool::Moli,
    };
    tools::locate(tool).map(|_| tool)
}

/// Tier 2a: `moli fetch <url> --dump html`, with the site's saved login as
/// a cookie file that lives only for the fetch.
pub(super) async fn moli(url: &str, with_cookies: bool) -> TierResult {
    let mut args: Vec<String> = vec![
        "fetch".into(),
        url.into(),
        "--dump".into(),
        "html".into(),
        "--timeout".into(),
        "12000".into(),
    ];
    let jar = if with_cookies {
        let list = cookies::for_url(url);
        if list.is_empty() {
            None
        } else {
            let path = temp_profile().with_extension("cookies.txt");
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let jar = TempFile(path.clone());
            std::fs::write(&path, cookies::to_netscape(&list)).map_err(|e| (Miss::Other, e.to_string()))?;
            args.push("--cookie".into());
            args.push(path.display().to_string());
            Some(jar)
        }
    } else {
        None
    };
    let ran = tools::run(Tool::Moli, &args, Duration::from_secs(15)).await;
    drop(jar);
    let ran = ran.map_err(|e| (Miss::Other, e))?;
    if ran.timed_out {
        return Err((Miss::Other, "moli timed out".into()));
    }
    if ran.code != Some(0) {
        let msg = format!("moli exit {:?}: {}", ran.code, crate::providers::thread::clip(ran.stderr.trim(), 300));
        let miss = if super::no_such_host(&msg) { Miss::NoHost } else { Miss::Other };
        return Err((miss, msg));
    }
    if ran.stdout.len() < MIN_HTML_BYTES {
        return Err((
            Miss::Wall,
            format!("moli returned {} bytes and no error (a bot wall or an empty render)", ran.stdout.len()),
        ));
    }
    convert::from_html(url, ran.stdout_text()).await
}

/// Tier 2a (when `tools.lightbrowser` is obscura): `obscura --stealth fetch <url> --dump html`.
pub(super) async fn obscura(url: &str) -> TierResult {
    let args: Vec<String> = vec![
        "--stealth".into(),
        "fetch".into(),
        url.into(),
        "--dump".into(),
        "html".into(),
        "--timeout".into(),
        "12".into(),
    ];
    let ran = tools::run(Tool::Obscura, &args, Duration::from_secs(15))
        .await
        .map_err(|e| (Miss::Other, e))?;
    if ran.timed_out {
        return Err((Miss::Other, "obscura timed out".into()));
    }
    if ran.code != Some(0) {
        let msg = format!("obscura exit {:?}: {}", ran.code, ran.stderr.trim());
        let miss = if super::no_such_host(&msg) { Miss::NoHost } else { Miss::Other };
        return Err((miss, msg));
    }
    if ran.stdout.len() < MIN_HTML_BYTES {
        return Err((
            Miss::Wall,
            format!(
                "obscura returned {} bytes and no error (a bot wall or an empty render)",
                ran.stdout.len()
            ),
        ));
    }
    convert::from_html(url, ran.stdout_text()).await
}

/// Chromium for tier 2 and `login`: the config path, else Playwright's
/// newest full Chromium.
pub fn chromium_path() -> Option<PathBuf> {
    let configured = &crate::config::get().tools.chromium;
    if !configured.is_empty() {
        let p = PathBuf::from(configured);
        return p.is_file().then_some(p);
    }
    let root = if cfg!(windows) {
        PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("ms-playwright")
    } else {
        PathBuf::from(std::env::var_os("HOME")?).join(".cache/ms-playwright")
    };
    let mut builds: Vec<(u32, PathBuf)> = std::fs::read_dir(&root)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let n: u32 = name.strip_prefix("chromium-")?.parse().ok()?;
            let candidates: &[&str] = if cfg!(windows) {
                &["chrome-win64/chrome.exe", "chrome-win/chrome.exe"]
            } else if cfg!(target_os = "macos") {
                &[
                    "chrome-mac-arm64/Chromium.app/Contents/MacOS/Chromium",
                    "chrome-mac/Chromium.app/Contents/MacOS/Chromium",
                ]
            } else {
                &["chrome-linux64/chrome", "chrome-linux/chrome"]
            };
            let exe = candidates.iter().map(|c| e.path().join(c)).find(|p| p.is_file())?;
            Some((n, exe))
        })
        .collect();
    builds.sort();
    builds.pop().map(|(_, p)| p)
}

fn drive(mut handler: Handler) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = handler.next().await {
            if event.is_err() {
                break;
            }
        }
    })
}

fn to_param(c: &Cookie) -> Option<CookieParam> {
    serde_json::from_value(serde_json::json!({
        "name": c.name,
        "value": c.value,
        "domain": c.domain,
        "path": c.path,
        "secure": c.secure,
        "httpOnly": c.http_only,
        "expires": if c.expires > 0.0 { Some(c.expires) } else { None },
    }))
    .ok()
}

fn from_cdp(c: &CdpCookie) -> Cookie {
    Cookie {
        name: c.name.clone(),
        value: c.value.clone(),
        domain: c.domain.clone(),
        path: c.path.clone(),
        expires: if c.session { 0.0 } else { c.expires },
        secure: c.secure,
        http_only: c.http_only,
    }
}

/// A throwaway profile folder for one headless launch, so parallel fetches
/// never share a locked profile.
fn temp_profile() -> PathBuf {
    crate::config::data_dir()
        .join("browser-tmp")
        .join(uuid::Uuid::new_v4().to_string())
}

/// A temporary file removed when dropped, also when its fetch is stopped.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Remove what stopped fetches left in `browser-tmp` (profiles, cookie
/// files) once it is older than ten minutes.
pub fn sweep_stale() {
    let Ok(entries) = std::fs::read_dir(crate::config::data_dir().join("browser-tmp")) else { return };
    let old = std::time::SystemTime::now() - Duration::from_secs(600);
    for e in entries.flatten() {
        let stale = e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t < old);
        if !stale {
            continue;
        }
        let p = e.path();
        let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
    }
}

/// Tier 2b: headless Chromium. Saved cookies are set first for an
/// allow-listed site.
/// Chromium windows open at once, across this process's fetches.
static WINDOWS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(3);

pub(super) async fn chromium(url: &str, with_cookies: bool) -> TierResult {
    let exe = chromium_path().ok_or((Miss::Other, "no Chromium found".to_string()))?;
    let _window = WINDOWS.acquire().await.map_err(|e| (Miss::Other, e.to_string()))?;
    let profile = temp_profile();
    let result = tokio::time::timeout(
        Duration::from_secs(25),
        render(&exe, &profile, url, with_cookies),
    )
    .await
    .unwrap_or_else(|_| Err((Miss::Other, "chromium timed out after 25 s".into())));
    remove_profile(&profile).await;
    result
}

/// Delete a temporary profile folder. Chromium keeps its files locked for a
/// moment after it closes, so the delete is retried for a few seconds; a
/// leftover copy of a cookie database must not stay on disk.
pub async fn remove_profile(dir: &std::path::Path) {
    for _ in 0..20 {
        if tokio::fs::remove_dir_all(dir).await.is_ok() || !dir.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    tracing::warn!("could not delete the temporary profile {}", dir.display());
}

async fn render(
    exe: &std::path::Path,
    profile: &std::path::Path,
    url: &str,
    with_cookies: bool,
) -> TierResult {
    let other = |e: String| (Miss::Other, e);
    let config = BrowserConfig::builder()
        .chrome_executable(exe)
        .new_headless_mode()
        .user_data_dir(profile)
        .arg("--disable-blink-features=AutomationControlled")
        .launch_timeout(Duration::from_secs(10))
        .request_timeout(Duration::from_secs(15))
        .build()
        .map_err(other)?;
    let started = std::time::Instant::now();
    let (browser, handler) = Browser::launch(config)
        .await
        .map_err(|e| other(format!("cannot start Chromium: {e}")))?;
    let task = drive(handler);
    let out = async {
        if with_cookies {
            let params: Vec<CookieParam> =
                cookies::for_url(url).iter().filter_map(to_param).collect();
            if !params.is_empty() {
                browser
                    .set_cookies(params)
                    .await
                    .map_err(|e| other(format!("set cookies: {e}")))?;
            }
        }
        let page = browser
            .new_page(url)
            .await
            .map_err(|e| other(format!("open page: {e}")))?;
        let _ = page.wait_for_navigation().await;
        let _ = page
            .wait_for_network_idle_with_timeout(Duration::from_secs(6))
            .await;
        // Content loaded on scroll (long threads, feeds): scroll to the end
        // until the page stops growing.
        if tokio::time::timeout(Duration::from_secs(6), page.evaluate(SCROLL_JS)).await.is_ok() {
            let _ = page
                .wait_for_network_idle_with_timeout(Duration::from_secs(3))
                .await;
        }
        let html = page
            .content()
            .await
            .map_err(|e| other(format!("read page: {e}")))?;
        let final_url = page
            .url()
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| url.to_string());
        Ok::<_, (Miss, String)>((final_url, html))
    }
    .await;
    let _ = browser.close().await;
    let _ = task.await;
    let (final_url, html) = out?;
    crate::run::raw::record_page("chromium", url, 200, started.elapsed(), &html);
    if final_url.starts_with("chrome-error://") {
        return Err((Miss::Other, "Chromium showed its own error page (the site can't be reached)".into()));
    }
    if html.len() < MIN_HTML_BYTES {
        return Err((Miss::Thin, format!("Chromium rendered {} bytes", html.len())));
    }
    convert::from_html(&final_url, html).await
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// An everyday browser for the visible login window (Edge, else Chrome), so
/// sites treat the login as a normal one.
fn everyday_browser() -> Option<PathBuf> {
    if cfg!(windows) {
        let roots: Vec<PathBuf> = ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"]
            .iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .collect();
        let apps = [
            ["Microsoft", "Edge", "Application", "msedge.exe"],
            ["Google", "Chrome", "Application", "chrome.exe"],
        ];
        return apps
            .iter()
            .flat_map(|a| roots.iter().map(move |r| r.join(a[0]).join(a[1]).join(a[2]).join(a[3])))
            .find(|p| p.is_file());
    }
    if cfg!(target_os = "macos") {
        return [
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file());
    }
    ["microsoft-edge", "microsoft-edge-stable", "google-chrome", "google-chrome-stable", "chromium", "chromium-browser"]
        .iter()
        .find_map(|exe| crate::tools::on_path(exe))
}

/// The login window's switches: chromey's defaults minus `--enable-automation`
/// (the "controlled by automated software" bar) and the headless-only ones.
const LOGIN_ARGS: &[&str] = &[
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-sync",
    "--disable-features=TranslateUI",
    "--disable-blink-features=AutomationControlled",
    "--disable-popup-blocking",
];

/// What `osearch login` ended with.
pub enum LoginOutcome {
    /// Cookies saved; the session cookies that showed the login.
    Saved { cookies: usize, sessions: Vec<String> },
    /// No session cookie appeared or changed: nothing saved.
    NoLogin { cookies_seen: usize },
}

/// `osearch login <site>`: a visible browser window (Edge, else Chromium)
/// with OmniSearch's own login profile. The person logs in and closes the
/// window. The site's cookies are saved only when a session cookie appeared
/// or changed after the page loaded, so closing without logging in saves
/// nothing.
pub async fn login(site: &str, account: &str) -> Result<LoginOutcome, String> {
    if cookies::refused(site) {
        return Err(format!(
            "{site} is never logged into (Google accounts and banks are kept out)"
        ));
    }
    let (exe, profile_name) = match everyday_browser() {
        Some(p) => (p, "login-profile-edge"),
        None => (
            chromium_path().ok_or("no Edge, Chrome or Chromium found (tools.chromium sets a path)")?,
            "login-profile",
        ),
    };
    // A named account logs in from its own profile, so the main login's
    // browser session is not the one it signs out of.
    let profile_name = if account.is_empty() || account == "main" {
        profile_name.to_string()
    } else {
        format!("{profile_name}-{account}")
    };
    let profile = crate::config::data_dir().join(profile_name);
    let config = BrowserConfig::builder()
        .chrome_executable(exe)
        .with_head()
        .user_data_dir(&profile)
        .window_size(1200, 900)
        .viewport(None::<chromiumoxide::handler::viewport::Viewport>)
        .disable_default_args()
        .args(LOGIN_ARGS.iter().copied())
        .launch_timeout(Duration::from_secs(30))
        .build()?;
    let (browser, handler) = Browser::launch(config)
        .await
        .map_err(|e| format!("cannot start the browser: {e}"))?;
    let task = drive(handler);
    let start_url = format!("https://{site}/");
    eprintln!("Opening a browser window at {start_url} ...");
    let page = browser
        .new_page(start_url.as_str())
        .await
        .map_err(|e| format!("open {start_url}: {e}"))?;
    let _ = page.wait_for_navigation().await;
    let _ = page
        .wait_for_network_idle_with_timeout(Duration::from_secs(8))
        .await;
    // The cookies a visit sets without logging in: the baseline.
    let baseline: Vec<Cookie> = browser
        .get_cookies()
        .await
        .map(|l| l.iter().map(from_cdp).collect())
        .unwrap_or_default();
    eprintln!("Log in there, then close the browser window to save the session.");
    // Snapshot the cookies until the window closes; the last good snapshot
    // is what gets saved.
    let mut snapshot: Vec<CdpCookie> = Vec::new();
    loop {
        if task.is_finished() {
            break;
        }
        match tokio::time::timeout(Duration::from_secs(5), browser.get_cookies()).await {
            Ok(Ok(list)) => snapshot = list,
            Ok(Err(_)) => break,
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let _ = task.await;
    let mine: Vec<Cookie> = snapshot
        .iter()
        .map(from_cdp)
        .filter(|c| cookies::under(&c.domain.trim_start_matches('.').to_ascii_lowercase(), site))
        .collect();
    let mut sessions: Vec<String> = mine
        .iter()
        .filter(|c| {
            // A login lasts weeks; visitor and tracking cookies set by the
            // page's scripts (Reddit's session_tracker, token_v2) last hours.
            let lasting = c.expires > now_secs() + 7.0 * 86_400.0;
            cookies::login_like(c)
                && lasting
                && !baseline
                    .iter()
                    .any(|b| b.name == c.name && b.domain == c.domain && b.value == c.value)
        })
        .map(|c| c.name.clone())
        .collect();
    sessions.dedup();
    if sessions.is_empty() {
        return Ok(LoginOutcome::NoLogin {
            cookies_seen: mine.len(),
        });
    }
    let n = cookies::store_site_as(site, account, &mine).map_err(|e| format!("save cookies: {e}"))?;
    Ok(LoginOutcome::Saved { cookies: n, sessions })
}

/// Cookies of a Chromium profile folder (the crwl `main` profile), read
/// headless. The profile must not be open in another browser.
pub async fn profile_cookies(profile: &std::path::Path) -> Result<Vec<Cookie>, String> {
    let exe = chromium_path().ok_or("no Chromium found")?;
    let config = BrowserConfig::builder()
        .chrome_executable(exe)
        .new_headless_mode()
        .user_data_dir(profile)
        .launch_timeout(Duration::from_secs(30))
        .build()?;
    let (browser, handler) = Browser::launch(config)
        .await
        .map_err(|e| format!("cannot open the profile (is it in use?): {e}"))?;
    let task = drive(handler);
    let list = browser.get_cookies().await.map_err(|e| e.to_string());
    let _ = browser.close().await;
    let _ = task.await;
    Ok(list?.iter().map(from_cdp).collect())
}
