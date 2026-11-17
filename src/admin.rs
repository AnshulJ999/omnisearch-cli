//! `login`, `update` and `providers`.

use crate::cli::LoginArgs;
use crate::context::AppContext;
use crate::cookies;
use crate::errors::SearchError;
use crate::tools::{self, Tool};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

fn bad(message: impl Into<String>) -> SearchError {
    SearchError::InvalidInput {
        message: message.into(),
    }
}

pub async fn login(a: LoginArgs) -> Result<i32, SearchError> {
    let account = a.account.clone().unwrap_or_default();
    let named = if account.is_empty() || account == "main" { String::new() } else { format!(" as '{account}'") };
    if a.list {
        let store = cookies::load();
        if store.sites.is_empty() {
            println!("no sites logged in ({})", cookies::store_path().display());
        }
        for s in &store.sites {
            let n = store
                .cookies
                .iter()
                .filter(|c| cookies::under(c.domain.trim_start_matches('.'), s))
                .count();
            if n > 0 {
                let e = cookies::describe_expiry(&cookies::login_expiry(&store, s));
                println!("{s} ({n} cookies) · login {e}");
            }
            for acc in store.accounts.iter().filter(|x| &x.site == s) {
                let e = cookies::describe_expiry(&cookies::expiry_of(&acc.cookies, s));
                println!("{s} as '{}' ({} cookies) · login {e}", acc.name, acc.cookies.len());
            }
        }
        return Ok(0);
    }
    if let Some(file) = a.import_file {
        return import_file(&file, a.site.as_deref(), &account);
    }
    if let Some(site) = a.forget {
        let site = cookies::site_of(&site).ok_or_else(|| bad("not a site name"))?;
        let removed = if named.is_empty() { cookies::forget_site(&site)? } else { cookies::forget_account(&site, &account)? };
        println!(
            "{}",
            if removed { format!("forgot {site}{named}") } else { format!("{site}{named} was not logged in") }
        );
        return Ok(0);
    }
    if let Some(sites) = a.import_crwl {
        return import_crwl(sites, &account).await;
    }
    let site = a
        .site
        .as_deref()
        .and_then(cookies::site_of)
        .ok_or_else(|| bad("name a site: osearch login reddit.com"))?;
    match crate::fetch::browser::login(&site, &account).await.map_err(bad)? {
        crate::fetch::browser::LoginOutcome::Saved { cookies: n, sessions } => {
            let store = cookies::load();
            let e = match store.accounts.iter().find(|x| x.site == site && x.name == account) {
                Some(acc) => cookies::describe_expiry(&cookies::expiry_of(&acc.cookies, &site)),
                None => cookies::describe_expiry(&cookies::login_expiry(&store, &site)),
            };
            println!(
                "saved {n} cookies for {site}{named} (session cookies: {}); login {e}. It is on the allow-list and every fetch tier sends them",
                sessions.join(", ")
            );
            Ok(0)
        }
        crate::fetch::browser::LoginOutcome::NoLogin { cookies_seen } => {
            println!(
                "no login detected for {site}, nothing saved ({cookies_seen} cookies seen, none a new session). Log in fully before closing the window; a login saved earlier stays as it was"
            );
            Ok(1)
        }
    }
}

/// `login --import-file cookies.txt [site]`: a Netscape cookies.txt exported
/// from a browser extension. Every site in it is imported, or only `site`.
fn import_file(path: &str, only: Option<&str>, account: &str) -> Result<i32, SearchError> {
    let text = std::fs::read_to_string(path).map_err(|e| bad(format!("{path}: {e}")))?;
    let all = cookies::parse_netscape(&text);
    if all.is_empty() {
        return Err(bad(format!("{path}: no cookies in Netscape cookies.txt format (tab-separated, 7 fields)")));
    }
    let sites: Vec<String> = match only {
        Some(s) => vec![cookies::site_of(s).ok_or_else(|| bad("not a site name"))?],
        None => {
            let mut v: Vec<String> = all.iter().map(|c| cookies::base_site(&c.domain)).collect();
            v.sort();
            v.dedup();
            v
        }
    };
    for site in sites {
        if cookies::refused(&site) {
            println!("{site}: skipped (Google accounts and banks are never stored)");
            continue;
        }
        let n = cookies::store_site_as(&site, account, &all)?;
        if n == 0 {
            println!("{site}: no cookies for it in the file");
            continue;
        }
        let e = cookies::describe_expiry(&cookies::login_expiry(&cookies::load(), &site));
        println!("{site}: {n} cookies imported; login {e}");
    }
    Ok(0)
}

fn crwl_profile() -> PathBuf {
    let name = crate::config::get().tools.crwlprofile.clone();
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".crawl4ai").join("profiles").join(name)
}

/// Copy the crwl profile's cookie files to a temporary profile and read
/// them there, so a crwl run holding the real profile does not block us.
fn copy_profile(from: &Path, to: &Path) -> std::io::Result<()> {
    for rel in ["Default/Network/Cookies", "Default/Cookies", "Local State"] {
        let src = from.join(rel);
        if src.is_file() {
            let dst = to.join(rel);
            if let Some(d) = dst.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

async fn import_crwl(sites: Vec<String>, account: &str) -> Result<i32, SearchError> {
    let profile = crwl_profile();
    if !profile.is_dir() {
        return Err(bad(format!("no crwl profile at {}", profile.display())));
    }
    let tmp = crate::config::data_dir()
        .join("browser-tmp")
        .join(format!("crwl-import-{}", uuid::Uuid::new_v4()));
    copy_profile(&profile, &tmp)?;
    let all = crate::fetch::browser::profile_cookies(&tmp).await;
    crate::fetch::browser::remove_profile(&tmp).await;
    let all = all.map_err(bad)?;
    if sites.is_empty() {
        let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
        for c in &all {
            let d = c.domain.trim_start_matches('.').to_string();
            let d = d.strip_prefix("www.").unwrap_or(&d).to_string();
            if !cookies::refused(&d) {
                *counts.entry(d).or_default() += 1;
            }
        }
        let mut list: Vec<(String, usize)> = counts.into_iter().collect();
        list.sort_by(|a, b| b.1.cmp(&a.1));
        println!("sites in the crwl profile (cookies); import with --import-crwl site1,site2:");
        for (d, n) in list.iter().take(60) {
            println!("  {d} ({n})");
        }
        return Ok(0);
    }
    for s in sites {
        let Some(site) = cookies::site_of(&s) else { continue };
        if cookies::refused(&site) {
            println!("{site}: skipped (Google accounts and banks are never stored)");
            continue;
        }
        let n = cookies::store_site_as(&site, account, &all)?;
        println!("{site}: {n} cookies imported");
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// update

const DONSETCH_REPO: &str = "dondai44423/donsetch";
const OBSCURA_REPO: &str = "h4ckf0r0day/obscura";
const MOLI_REPO: &str = "lexmount/moli";
const MOLI_ASSET: &str = "moli-x86_64-pc-windows-msvc.zip";
/// The Obscura build tested on this machine: no rendering, stealth on.
const OBSCURA_ASSET: &str = "obscura-x86_64-windows-no-render-stealth.zip";
const DONSETCH_ASSET: &str = "donsetch-win32-x64.tar.gz";

async fn newest_release(app: &AppContext, repo: &str) -> Result<serde_json::Value, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page=5");
    let resp = app
        .client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let list: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    list.as_array()
        .and_then(|a| a.iter().find(|r| r["draft"].as_bool() != Some(true)).cloned())
        .ok_or_else(|| format!("no releases listed for {repo}"))
}

async fn download(app: &AppContext, url: &str, to: &Path) -> Result<(), String> {
    let bytes = app
        .client
        .get(url)
        .timeout(Duration::from_secs(600))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;
    std::fs::write(to, &bytes).map_err(|e| e.to_string())
}

fn asset_url(release: &serde_json::Value, name: &str) -> Option<String> {
    release["assets"]
        .as_array()?
        .iter()
        .find(|a| a["name"] == name)?["browser_download_url"]
        .as_str()
        .map(str::to_string)
}

/// Unpack an archive with the system `tar` (bsdtar on Windows reads zip).
async fn unpack(archive: &Path, into: &Path) -> Result<(), String> {
    std::fs::create_dir_all(into).map_err(|e| e.to_string())?;
    let out = tokio::process::Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .output()
        .await
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(())
}

/// Move every file of `from` (searched to two levels) into `to`, keeping a
/// `.bak` of each file replaced.
fn install_files(from: &Path, to: &Path) -> Result<Vec<String>, String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    let mut stack = vec![(from.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        for e in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
            let p = e.path();
            if p.is_dir() && depth < 2 {
                stack.push((p, depth + 1));
            } else if p.is_file() {
                let name = e.file_name();
                let dst = to.join(&name);
                if dst.exists() {
                    let _ = std::fs::rename(&dst, dst.with_extension("bak"));
                }
                std::fs::rename(&p, &dst)
                    .or_else(|_| std::fs::copy(&p, &dst).map(|_| ()))
                    .map_err(|e| e.to_string())?;
                files.push(name.to_string_lossy().into_owned());
            }
        }
    }
    Ok(files)
}

async fn install(app: &AppContext, repo: &str, asset: &str, tool: &str) -> Result<String, String> {
    let release = newest_release(app, repo).await?;
    let tag = release["tag_name"].as_str().unwrap_or("?").to_string();
    let url = asset_url(&release, asset).ok_or_else(|| format!("{tag} has no {asset}"))?;
    let work = crate::config::data_dir()
        .join("browser-tmp")
        .join(format!("dl-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let archive = work.join(asset);
    download(app, &url, &archive).await?;
    // donsetch publishes a SHA256 next to the archive: check it.
    if let Some(sum_url) = asset_url(&release, &format!("{asset}.sha256")) {
        let sum_file = work.join("sum.sha256");
        download(app, &sum_url, &sum_file).await?;
        let want = std::fs::read_to_string(&sum_file)
            .unwrap_or_default()
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let got = sha256_of(&archive).await?;
        if want.is_empty() || want != got {
            let _ = std::fs::remove_dir_all(&work);
            return Err(format!("SHA256 mismatch for {asset}: published {want}, downloaded {got}"));
        }
    }
    let unpacked = work.join("x");
    unpack(&archive, &unpacked).await?;
    let files = install_files(&unpacked, &tools::tools_dir().join(tool))?;
    let _ = std::fs::remove_dir_all(&work);
    Ok(format!("{tag} ({} files)", files.len()))
}

/// SHA256 through the system's `certutil` (Windows) or `sha256sum`.
async fn sha256_of(path: &Path) -> Result<String, String> {
    let out = if cfg!(windows) {
        tokio::process::Command::new("certutil")
            .args(["-hashfile"])
            .arg(path)
            .arg("SHA256")
            .output()
            .await
    } else {
        tokio::process::Command::new("sha256sum").arg(path).output().await
    }
    .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .map(|l| l.trim().replace(' ', "").to_ascii_lowercase())
        .find(|l| l.len() == 64 && l.chars().all(|c| c.is_ascii_hexdigit()))
        .or_else(|| {
            text.split_whitespace()
                .next()
                .filter(|w| w.len() == 64)
                .map(str::to_ascii_lowercase)
        })
        .ok_or_else(|| format!("cannot hash {}", path.display()))
}

pub async fn update(app: Arc<AppContext>, check: bool) -> Result<i32, SearchError> {
    for t in Tool::ALL {
        let where_ = tools::locate(t)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not installed".into());
        let v = tools::version(t).await.unwrap_or_default();
        println!("{:<9} {v}  [{where_}]", t.name());
    }
    if check {
        for (repo, name) in [(DONSETCH_REPO, "donsetch"), (MOLI_REPO, "moli"), (OBSCURA_REPO, "obscura")] {
            match newest_release(&app, repo).await {
                Ok(r) => println!("newest {name} release: {}", r["tag_name"].as_str().unwrap_or("?")),
                Err(e) => println!("{name}: cannot check ({e})"),
            }
        }
        return Ok(0);
    }
    let mut failed = false;
    // donsetch updates itself (SHA256 check, backup, `donsetch rollback`);
    // installed here first when missing.
    match tools::locate(Tool::Donsetch) {
        Some(_) => match tools::run(Tool::Donsetch, &["update".to_string()], Duration::from_secs(600)).await {
            Ok(r) => {
                let text = r.stdout_text() + &r.stderr;
                println!("donsetch update: {}", text.trim().lines().last().unwrap_or("done"));
                failed |= r.code != Some(0);
            }
            Err(e) => {
                println!("donsetch update failed: {e}");
                failed = true;
            }
        },
        None => match install(&app, DONSETCH_REPO, DONSETCH_ASSET, "donsetch").await {
            Ok(v) => println!("donsetch installed: {v}"),
            Err(e) => {
                println!("donsetch install failed: {e}");
                failed = true;
            }
        },
    }
    // moli and Obscura: replaced when a newer release exists (a checksum is
    // checked when the release publishes one).
    for (tool, repo, asset) in [(Tool::Moli, MOLI_REPO, MOLI_ASSET), (Tool::Obscura, OBSCURA_REPO, OBSCURA_ASSET)] {
        let name = tool.name();
        let installed = tools::version(tool).await.unwrap_or_default();
        match newest_release(&app, repo).await {
            Ok(r) => {
                let tag = r["tag_name"].as_str().unwrap_or("").trim_start_matches('v').to_string();
                let own = tools::tools_dir().join(name);
                if !installed.contains(&tag) || !own.is_dir() {
                    match install(&app, repo, asset, name).await {
                        Ok(v) => println!("{name} installed: {v}"),
                        Err(e) => {
                            println!("{name} install failed: {e}");
                            failed = true;
                        }
                    }
                } else {
                    println!("{name} is current ({tag})");
                }
            }
            Err(e) => {
                println!("{name}: cannot check ({e})");
                failed = true;
            }
        }
    }
    println!("crawl4ai, yt-dlp and gh are updated by their own installers (pip, yt-dlp -U, winget)");
    Ok(if failed { 1 } else { 0 })
}

pub fn providers(app: &Arc<AppContext>) {
    println!("{:<12} {:<11} {:<6} modes", "engine", "configured", "cost");
    for p in crate::providers::build_providers(app) {
        let s = p.spec();
        let cost = if s.price.per_call == 0 {
            "free".to_string()
        } else {
            format!("${:.4}", s.price.per_call as f64 / 1e6)
        };
        let optional = if crate::providers::SOCIAL_PLATFORMS.contains(&s.name) {
            " (optional: only when a request names it)"
        } else {
            ""
        };
        println!(
            "{:<12} {:<11} {:<6} {}{optional}",
            s.name,
            if p.is_configured() { "yes" } else { "no" },
            cost,
            p.capabilities().join(",")
        );
    }
    println!("\nmodes:");
    for m in crate::registry::MODES {
        if m.mode == crate::types::Mode::Scrape {
            continue;
        }
        println!(
            "  {:<10} {} ({} input, {}): {}",
            m.mode.to_string(),
            m.description,
            m.input.as_str(),
            m.merge.as_str(),
            m.when_to_use
        );
    }
    println!(
        "\nfetch tiers: 0 site doors · 1 donsetch {} / direct / stealth · 2 {} {} / chromium {} · 3 crawl4ai {} / jina / firecrawl",
        mark(tools::locate(Tool::Donsetch).is_some()),
        crate::fetch::browser::light().map_or("light browser off", |t| t.name()),
        mark(crate::fetch::browser::light().is_some()),
        mark(crate::fetch::browser::chromium_path().is_some()),
        mark(tools::locate(Tool::Crwl).is_some()),
    );
}

fn mark(b: bool) -> &'static str {
    if b {
        "(ok)"
    } else {
        "(missing)"
    }
}
