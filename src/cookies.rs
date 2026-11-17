//! OmniSearch's private cookie store: the sessions `osearch login` saved.
//! The sites logged into form the allow-list: only their cookies are ever
//! sent, at every fetch tier. Google and banking sites are never stored.
//! On Windows the file holds the store encrypted with DPAPI (this Windows
//! user only), the way Chromium protects its own cookies.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    #[serde(default = "root_path")]
    pub path: String,
    /// Unix seconds; 0 or negative = a session cookie.
    #[serde(default)]
    pub expires: f64,
    #[serde(default)]
    pub secure: bool,
    #[serde(default)]
    pub http_only: bool,
}

fn root_path() -> String {
    "/".to_string()
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Store {
    /// Allow-listed sites (`reddit.com`), lowercase, no `www.`.
    #[serde(default)]
    pub sites: Vec<String>,
    /// Each site's main login.
    #[serde(default)]
    pub cookies: Vec<Cookie>,
    /// Further logins under a name (`osearch login reddit.com --as alt`).
    #[serde(default)]
    pub accounts: Vec<Account>,
}

/// A named login of a site, beside its main one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub site: String,
    pub name: String,
    #[serde(default)]
    pub cookies: Vec<Cookie>,
}

tokio::task_local! {
    /// The named account a task's fetches send (`with_account`).
    static ACCOUNT: String;
}

/// Run `f` with `name`'s login for the sites that have one under that name;
/// other sites keep their main login. An empty name changes nothing.
pub async fn with_account<F: std::future::Future>(name: &str, f: F) -> F::Output {
    let name = if name.is_empty() { "main" } else { name };
    // "main" is scoped too, so it overrides an outer named account.
    ACCOUNT.scope(name.to_string(), f).await
}

/// The named account the current task sends (`main` outside `with_account`).
pub fn current_account() -> String {
    ACCOUNT.try_with(|a| a.clone()).unwrap_or_else(|_| "main".to_string())
}

/// Whether a login is saved for `site` under `name` (empty or "main": the
/// site's main login).
pub fn has_account(site: &str, name: &str) -> bool {
    let store = cached();
    if name.is_empty() || name == "main" {
        store.cookies.iter().any(|c| under(c.domain.trim_start_matches('.'), site) && login_like(c))
    } else {
        store.accounts.iter().any(|a| a.site == site && a.name == name)
    }
}

fn active_account() -> Option<String> {
    ACCOUNT.try_with(|a| a.clone()).ok()
}

pub fn store_path() -> PathBuf {
    crate::config::data_dir().join("cookies.json")
}

/// The store for reading: an unreadable one counts as empty, with a warning.
pub fn load() -> Store {
    read().unwrap_or_else(|e| {
        tracing::warn!("{e}: logins are empty");
        Store::default()
    })
}

/// The store, or why it cannot be read. Writers use this so an unreadable
/// store is never overwritten with only the site being saved.
fn read() -> Result<Store, String> {
    let path = store_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Store::default()),
        Err(e) => return Err(format!("cookie store {}: {e}", path.display())),
    };
    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|_| format!("cookie store {} is not readable JSON", path.display()))?;
    if let Some(hex) = v.get("dpapi").and_then(|h| h.as_str()) {
        let plain = from_hex(hex).and_then(|b| dpapi::unprotect(&b)).ok_or_else(|| {
            format!("cookie store {} cannot be decrypted (another Windows user or machine)", path.display())
        })?;
        return serde_json::from_slice(&plain)
            .map_err(|e| format!("cookie store {} is damaged: {e}", path.display()));
    }
    let store: Store = serde_json::from_value(v)
        .map_err(|e| format!("cookie store {} is damaged: {e}", path.display()))?;
    // A store written before encryption: encrypt it in place.
    if cfg!(windows) && !store.sites.is_empty() {
        let _ = save(&store);
    }
    Ok(store)
}

fn read_for_write() -> std::io::Result<Store> {
    read().map_err(|e| std::io::Error::other(format!("{e}; nothing was saved, so the logins in it are kept")))
}

pub fn save(store: &Store) -> std::io::Result<()> {
    let path = store_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let plain = serde_json::to_vec_pretty(store)?;
    let body = match dpapi::protect(&plain) {
        Some(sealed) => serde_json::to_vec(&serde_json::json!({ "dpapi": to_hex(&sealed) }))?,
        None => {
            if cfg!(windows) {
                tracing::warn!("DPAPI encryption failed: cookie store written in plain text");
            }
            plain
        }
    };
    let tmp = path.with_extension("json.tmp");
    write_private(&tmp, &body)?;
    std::fs::rename(tmp, path)
}

/// Off Windows the store is plain JSON: readable by its owner only.
#[cfg(unix)]
fn write_private(path: &std::path::Path, body: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(body)
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, body: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, body)
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    (0..s.len())
        .step_by(2)
        .map(|i| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()))
        .collect()
}

#[cfg(windows)]
mod dpapi {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    fn run(data: &[u8], seal: bool) -> Option<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(data.len()).ok()?,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // SAFETY: input points at `data` for the call; DPAPI allocates `out`,
        // which is copied and then released with LocalFree.
        let ok = unsafe {
            if seal {
                CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            } else {
                CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            }
        };
        if ok == 0 || out.pbData.is_null() {
            return None;
        }
        let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
        unsafe { LocalFree(out.pbData as *mut core::ffi::c_void) };
        Some(bytes)
    }

    pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
        run(data, true)
    }

    pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
        run(data, false)
    }
}

#[cfg(not(windows))]
mod dpapi {
    pub fn protect(_: &[u8]) -> Option<Vec<u8>> {
        None
    }

    pub fn unprotect(_: &[u8]) -> Option<Vec<u8>> {
        None
    }
}

static STORE: OnceLock<Store> = OnceLock::new();

fn cached() -> &'static Store {
    STORE.get_or_init(load)
}

/// A site name from a URL or a bare domain: lowercase host without `www.`.
pub fn site_of(input: &str) -> Option<String> {
    let input = input.trim();
    let host = if input.contains("://") {
        url::Url::parse(input).ok()?.host_str()?.to_string()
    } else {
        input.split('/').next()?.to_string()
    };
    let host = host.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    (!host.is_empty() && host.contains('.')).then_some(host)
}

/// Sites whose cookies are never stored or sent: Google accounts and banks.
pub fn refused(site: &str) -> bool {
    let s = site.to_ascii_lowercase();
    let google = ["google.", "gmail.com", "youtube.com", "googleusercontent.com"]
        .iter()
        .any(|g| s.starts_with(g) || s.contains(&format!(".{g}")) || s == *g);
    google || s.contains("bank") || s.ends_with(".bank")
}

/// `host` is `site` or one of its subdomains.
pub fn under(host: &str, site: &str) -> bool {
    host == site || host.ends_with(&format!(".{site}"))
}

/// A cookie that carries a login session, judged by its name: `session`,
/// `auth`, `token`, `login` in it, or a known session name (Amazon's
/// `at-acbin`, LinkedIn's `li_at`, Facebook's `c_user`). CSRF tokens and
/// "logged out" values are not sessions.
pub fn is_session_cookie(c: &Cookie) -> bool {
    let n = c.name.to_ascii_lowercase();
    let v = c.value.trim().to_ascii_lowercase();
    if v.is_empty() || ["no", "false", "0", "deleted", "null"].contains(&v.as_str()) {
        return false;
    }
    if n.contains("csrf") || n.contains("xsrf") {
        return false;
    }
    let exact = ["sid", "ssid", "li_at", "c_user", "xs", "at-main", "sess-at-main", "x-main", "at-acbin", "sess-at-acbin", "x-acbin"];
    exact.contains(&n.as_str()) || ["session", "auth", "token", "login", "jwt", "remember"].iter().any(|w| n.contains(w))
}

/// Anti-bot and device cookies (Cloudflare, Akamai, DataDome): HttpOnly but
/// set for every visitor.
fn anti_bot(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("__cf") || n.starts_with("cf_") || n.starts_with("_cf") || n.contains("datadome")
        || ["ak_bmsc", "bm_sv", "bm_sz", "bm_mi", "_abck", "_device_id"].contains(&n.as_str())
}

/// A cookie a login may have set: a session name, or (for sites with short
/// names such as Flipkart's `at`/`rt`) any HttpOnly cookie that is not an
/// anti-bot one.
pub fn login_like(c: &Cookie) -> bool {
    let v = c.value.trim().to_ascii_lowercase();
    let empty = v.is_empty() || ["no", "false", "0", "deleted", "null"].contains(&v.as_str());
    is_session_cookie(c) || (c.http_only && !empty && !anti_bot(&c.name))
}

/// When a site's saved login runs out.
pub enum Expiry {
    /// Unix seconds; `guessed` when no cookie has a session name and the
    /// date comes from the site's HttpOnly cookies.
    At { t: f64, guessed: bool },
    /// Its session cookies end with the browser session (no date).
    Session,
    /// No cookie that looks like a session.
    Unknown,
}

pub fn login_expiry(store: &Store, site: &str) -> Expiry {
    expiry_of(&store.cookies, site)
}

/// When a login made of `cookies` runs out.
pub fn expiry_of(cookies: &[Cookie], site: &str) -> Expiry {
    let mine: Vec<&Cookie> = cookies
        .iter()
        .filter(|c| under(&c.domain.trim_start_matches('.').to_ascii_lowercase(), site))
        .collect();
    let named: Vec<&&Cookie> = mine.iter().filter(|c| is_session_cookie(c)).collect();
    if !named.is_empty() {
        // The longest-lived one: short-lived visitor tokens (Reddit's
        // token_v2) also carry "token" in their names.
        let latest = named.iter().map(|c| c.expires).fold(0.0_f64, f64::max);
        return if latest > 0.0 {
            Expiry::At { t: latest, guessed: false }
        } else {
            Expiry::Session
        };
    }
    // No session name: the earliest-ending HttpOnly cookie, to warn early.
    let dated: Vec<f64> = mine
        .iter()
        .filter(|c| login_like(c) && c.expires > 0.0)
        .map(|c| c.expires)
        .collect();
    match dated.iter().copied().reduce(f64::min) {
        Some(t) => Expiry::At { t, guessed: true },
        None => Expiry::Unknown,
    }
}

/// "expires 2026-12-01 (in 23 days)", "expired 2026-11-01", ...
pub fn describe_expiry(e: &Expiry) -> String {
    match e {
        Expiry::At { t, guessed } => {
            let date = chrono::DateTime::from_timestamp(*t as i64, 0)
                .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string())
                .unwrap_or_default();
            let days = (*t - now()) / 86_400.0;
            if *guessed {
                // No session-named cookie: tracking cookies look the same.
                return format!("unclear (no session cookie; its HttpOnly cookies last until {date})");
            }
            if days < 0.0 {
                format!("expired {date}")
            } else {
                format!("expires {date} (in {:.0} days)", days.round())
            }
        }
        Expiry::Session => "session cookies only (end when a browser closes; may still work)".into(),
        Expiry::Unknown => "no session cookie found (the login may not have been saved)".into(),
    }
}

/// Logins expired or ending within `days`: (site, description).
pub fn expiring(days: f64) -> Vec<(String, String)> {
    let store = load();
    let limit = now() + days * 86_400.0;
    store
        .sites
        .iter()
        .filter_map(|s| {
            let e = login_expiry(&store, s);
            matches!(e, Expiry::At { t, .. } if t < limit).then(|| (s.clone(), describe_expiry(&e)))
        })
        .collect()
}

/// The site a cookie domain belongs to: its last two labels, or three under
/// a two-part suffix (`amazon.co.uk`, `amazon.in`, `old.reddit.com` -> `reddit.com`).
pub fn base_site(domain: &str) -> String {
    let d = domain.trim_start_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = d.split('.').collect();
    let two_part = ["co.uk", "org.uk", "ac.uk", "co.in", "net.in", "org.in", "gov.in", "com.au", "co.jp", "com.br", "co.nz", "com.sg", "com.mx"];
    let n = labels.len();
    let keep = if n >= 3 && two_part.contains(&labels[n - 2..].join(".").as_str()) { 3 } else { 2 };
    labels[n.saturating_sub(keep)..].join(".")
}

/// Cookies as a Netscape `cookies.txt`.
pub fn to_netscape(list: &[Cookie]) -> String {
    let mut out = String::from("# Netscape HTTP Cookie File\n");
    for c in list {
        let domain = &c.domain;
        let sub = if domain.starts_with('.') { "TRUE" } else { "FALSE" };
        let prefix = if c.http_only { "#HttpOnly_" } else { "" };
        let expires = if c.expires > 0.0 { c.expires as i64 } else { 0 };
        out.push_str(&format!(
            "{prefix}{domain}\t{sub}\t{}\t{}\t{expires}\t{}\t{}\n",
            c.path,
            if c.secure { "TRUE" } else { "FALSE" },
            c.name,
            c.value
        ));
    }
    out
}

/// Cookies from a Netscape `cookies.txt` (what browser extensions export).
pub fn parse_netscape(text: &str) -> Vec<Cookie> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim_end_matches('\r');
            let (line, http_only) = match line.strip_prefix("#HttpOnly_") {
                Some(rest) => (rest, true),
                None => (line, false),
            };
            if line.trim().is_empty() || line.starts_with('#') {
                return None;
            }
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 7 {
                return None;
            }
            Some(Cookie {
                domain: f[0].to_string(),
                path: f[2].to_string(),
                secure: f[3].eq_ignore_ascii_case("true"),
                expires: f[4].trim().parse::<f64>().unwrap_or(0.0),
                name: f[5].to_string(),
                value: f[6..].join("\t"),
                http_only,
            })
        })
        .collect()
}

/// The allow-listed site `url` belongs to, if any.
pub fn allowed_site(url: &str) -> Option<String> {
    let host = url::Url::parse(url).ok()?.host_str()?.to_ascii_lowercase();
    cached()
        .sites
        .iter()
        .find(|s| under(&host, s))
        .cloned()
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Cookies to send for `url`: only for an allow-listed site, matching the
/// cookie's domain, path and secure flag, unexpired.
pub fn for_url(url: &str) -> Vec<Cookie> {
    let Ok(u) = url::Url::parse(url) else {
        return Vec::new();
    };
    let Some(host) = u.host_str().map(str::to_ascii_lowercase) else {
        return Vec::new();
    };
    if allowed_site(url).is_none() {
        return Vec::new();
    }
    let https = u.scheme() == "https";
    let path = u.path();
    let t = now();
    let store = cached();
    let site = allowed_site(url).unwrap_or_default();
    let named = active_account().and_then(|name| {
        store.accounts.iter().find(|a| a.site == site && a.name == name)
    });
    named
        .map_or(&store.cookies, |a| &a.cookies)
        .iter()
        .filter(|c| {
            let d = c.domain.trim_start_matches('.').to_ascii_lowercase();
            under(&host, &d)
                && path.starts_with(c.path.as_str())
                && (!c.secure || https)
                && (c.expires <= 0.0 || c.expires > t)
        })
        .cloned()
        .collect()
}

/// The `Cookie:` header for `url`, or `None` when nothing applies.
pub fn header_for(url: &str) -> Option<String> {
    let list = for_url(url);
    (!list.is_empty()).then(|| {
        list.iter()
            .map(|c| format!("{}={}", c.name, c.value))
            .collect::<Vec<_>>()
            .join("; ")
    })
}

/// Replace `site`'s cookies with `cookies` (those under the site only) and
/// put the site on the allow-list. Returns how many were kept.
pub fn store_site(site: &str, cookies: &[Cookie]) -> std::io::Result<usize> {
    let mut store = read_for_write()?;
    store.cookies.retain(|c| {
        let d = c.domain.trim_start_matches('.').to_ascii_lowercase();
        !under(&d, site)
    });
    let mine: Vec<Cookie> = cookies
        .iter()
        .filter(|c| under(&c.domain.trim_start_matches('.').to_ascii_lowercase(), site))
        .cloned()
        .collect();
    let n = mine.len();
    store.cookies.extend(mine);
    if !store.sites.iter().any(|s| s == site) {
        store.sites.push(site.to_string());
        store.sites.sort();
    }
    save(&store)?;
    Ok(n)
}

/// Save `cookies` as `site`'s login named `name` ("main" or empty: the
/// site's main login). The site joins the allow-list.
pub fn store_site_as(site: &str, name: &str, cookies: &[Cookie]) -> std::io::Result<usize> {
    if name.is_empty() || name == "main" {
        return store_site(site, cookies);
    }
    let mut store = read_for_write()?;
    let mine: Vec<Cookie> = cookies
        .iter()
        .filter(|c| under(&c.domain.trim_start_matches('.').to_ascii_lowercase(), site))
        .cloned()
        .collect();
    let n = mine.len();
    store.accounts.retain(|a| !(a.site == site && a.name == name));
    store.accounts.push(Account {
        site: site.to_string(),
        name: name.to_string(),
        cookies: mine,
    });
    if !store.sites.iter().any(|s| s == site) {
        store.sites.push(site.to_string());
        store.sites.sort();
    }
    save(&store)?;
    Ok(n)
}

/// Remove one named login of a site.
pub fn forget_account(site: &str, name: &str) -> std::io::Result<bool> {
    let mut store = read_for_write()?;
    let before = store.accounts.len();
    store.accounts.retain(|a| !(a.site == site && a.name == name));
    save(&store)?;
    Ok(store.accounts.len() != before)
}

/// Remove a site and its cookies.
pub fn forget_site(site: &str) -> std::io::Result<bool> {
    let mut store = read_for_write()?;
    let before = store.sites.len();
    store.sites.retain(|s| s != site);
    store.cookies.retain(|c| {
        let d = c.domain.trim_start_matches('.').to_ascii_lowercase();
        !under(&d, site)
    });
    store.accounts.retain(|a| a.site != site);
    save(&store)?;
    Ok(store.sites.len() != before)
}

/// A sign-in wall rather than content: a redirect to a sign-in page, or a
/// short page with sign-in wording. A page whose own address is about
/// logins (auth docs, even a renamed one) is content.
pub fn looks_like_login_wall(requested: &str, final_url: &str, text: &str) -> bool {
    let place = |u: &str| {
        url::Url::parse(u)
            .map(|p| {
                let host = p.host_str().unwrap_or("").trim_start_matches("www.").to_ascii_lowercase();
                (host, p.path().trim_end_matches('/').to_ascii_lowercase())
            })
            .unwrap_or_default()
    };
    let redirected = place(requested) != place(final_url);
    // A sign-in page's address: its last segment is a login word, or a login
    // segment plus a "return to" parameter (accounts.google.com/v3/signin/identifier?continue=).
    let path_hit = url::Url::parse(final_url).is_ok_and(|u| {
        let words = ["login", "log-in", "signin", "sign-in", "sign_in", "sso", "auth", "authorize", "oauth"];
        let segs: Vec<String> = u
            .path_segments()
            .map(|s| s.filter(|x| !x.is_empty()).map(str::to_ascii_lowercase).collect())
            .unwrap_or_default();
        let last_is_login = segs.last().is_some_and(|l| words.contains(&l.as_str()));
        let returns = u.query_pairs().any(|(k, _)| {
            let k = k.to_ascii_lowercase();
            ["return", "redirect", "next", "continue", "dest"].iter().any(|r| k.contains(r))
        });
        last_is_login || (returns && segs.iter().any(|s| words.contains(&s.as_str())))
    });
    let lower: String = text.chars().take(6000).collect::<String>().to_lowercase();
    let words = ["sign in to continue", "log in to continue", "please log in", "please sign in", "you must be logged in", "login required", "sign in to view", "log in to view"]
        .iter()
        .any(|w| lower.contains(w));
    (redirected && path_hit) || (words && text.len() < 6000)
}

#[cfg(test)]
mod tests {
    use super::looks_like_login_wall;

    #[test]
    fn auth_docs_are_content_sign_in_redirects_are_walls() {
        let page = "A long docs page. ".repeat(500);
        // Supabase renamed this page; the redirect lands on another docs page.
        assert!(!looks_like_login_wall(
            "https://supabase.com/docs/guides/auth/social-login",
            "https://supabase.com/docs/guides/auth/social-sign-in",
            &page
        ));
        assert!(looks_like_login_wall(
            "https://github.com/settings/tokens",
            "https://github.com/login?return_to=%2Fsettings%2Ftokens",
            &page
        ));
        assert!(looks_like_login_wall(
            "https://www.youtube.com/feed/history",
            "https://accounts.google.com/v3/signin/identifier?continue=https%3A%2F%2Fwww.youtube.com",
            &page
        ));
    }
}
