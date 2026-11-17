//! `osearch doctor`: keys, outside tools (with versions), the browser, the
//! cookie store, the archive. `--probe` also test-fires every configured
//! engine with one small real (billed) request.

use crate::context::AppContext;
use crate::errors::redact_secrets;
use crate::providers::{self, Call};
use crate::tools::{self, Tool};
use crate::types::{Mode, SearchOpts};
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinSet;

#[derive(Debug, Serialize)]
pub struct Check {
    pub item: String,
    pub ok: bool,
    pub detail: String,
}

fn check(item: &str, ok: bool, detail: impl Into<String>) -> Check {
    Check {
        item: item.to_string(),
        ok,
        detail: detail.into(),
    }
}

pub async fn run(app: Arc<AppContext>, probe: bool, json: bool) -> bool {
    let mut checks: Vec<Check> = Vec::new();
    let cfg = &app.config;
    checks.push(check(
        "keys file",
        std::path::Path::new(&cfg.paths.envfile).is_file(),
        cfg.paths.envfile.clone(),
    ));
    let archive = std::path::PathBuf::from(&cfg.paths.archive);
    let writable = std::fs::create_dir_all(archive.join("runs")).is_ok();
    checks.push(check("archive", writable, archive.display().to_string()));
    for t in Tool::ALL {
        let v = tools::version(t).await;
        let path = tools::locate(t).map(|p| p.display().to_string()).unwrap_or_default();
        checks.push(check(
            t.name(),
            v.is_some(),
            match v {
                Some(v) => format!("{v} [{path}]"),
                None => "not installed (osearch update installs donsetch and obscura)".into(),
            },
        ));
    }
    let chromium = crate::fetch::browser::chromium_path();
    checks.push(check(
        "chromium",
        chromium.is_some(),
        chromium
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "no Playwright Chromium found (tools.chromium sets a path)".into()),
    ));
    let store = crate::cookies::load();
    checks.push(check(
        "logins",
        true,
        if store.sites.is_empty() {
            "none (osearch login <site>)".to_string()
        } else {
            store.sites.join(", ")
        },
    ));
    for (site, when) in crate::cookies::expiring(14.0) {
        checks.push(check(
            &format!("login {site}"),
            false,
            format!("{when}: refresh with `osearch login {site}`"),
        ));
    }
    for p in providers::build_providers(&app) {
        let s = p.spec();
        let optional = providers::SOCIAL_PLATFORMS.contains(&s.name);
        let detail = if p.is_configured() {
            "configured".to_string()
        } else if s.env_keys.is_empty() {
            "its outside tool is missing".to_string()
        } else {
            format!("no key ({})", s.env_keys.join(" or "))
        };
        // A missing key is a choice, not a fault.
        checks.push(check(
            &format!("engine {}{}", s.name, if optional { " (optional)" } else { "" }),
            true,
            detail,
        ));
    }
    for w in crate::stats::quota_warnings() {
        checks.push(check("quota", false, w));
    }
    if probe {
        checks.extend(probe_engines(&app).await);
    }
    let healthy = checks.iter().all(|c| c.ok);
    if json {
        crate::output::json::render_value(&serde_json::json!({
            "status": if healthy { "success" } else { "partial_success" },
            "checks": checks,
        }));
    } else {
        for c in &checks {
            println!("{} {:<26} {}", if c.ok { "ok  " } else { "FAIL" }, c.item, c.detail);
        }
    }
    healthy
}

/// One minimal real call per configured engine (billed where the engine bills).
async fn probe_engines(app: &Arc<AppContext>) -> Vec<Check> {
    let mut set: JoinSet<Check> = JoinSet::new();
    for p in providers::build_providers(app) {
        if !p.is_configured() || providers::SOCIAL_PLATFORMS.contains(&p.name()) {
            continue;
        }
        set.spawn(async move {
            let s = p.spec();
            let mode = if s.modes.contains(&Mode::General) {
                Mode::General
            } else {
                s.modes[0]
            };
            let url_mode = crate::registry::spec(mode).input == crate::registry::InputKind::Url;
            let opts = SearchOpts::default();
            let call = Call {
                mode,
                // A plain server-rendered page: the direct and stealth readers fail on JS-only ones by design.
                query: if url_mode { "https://en.wikipedia.org/wiki/Web_search_engine" } else { "health" },
                count: 1,
                opts: &opts,
                timeout: s.timeout,
            };
            let start = Instant::now();
            let r = tokio::time::timeout(s.timeout + Duration::from_secs(2), p.call(&call)).await;
            let ms = start.elapsed().as_millis();
            match r {
                Ok(Ok(out)) => check(&format!("probe {}", s.name), true, format!("{ms} ms, {} results", out.results.len())),
                Ok(Err(e)) => check(
                    &format!("probe {}", s.name),
                    false,
                    format!("[{}] {}", e.category().as_str(), redact_secrets(&e.to_string())),
                ),
                Err(_) => check(&format!("probe {}", s.name), false, "timed out"),
            }
        });
    }
    let mut out = Vec::new();
    while let Some(Ok(c)) = set.join_next().await {
        out.push(c);
    }
    out.sort_by(|a, b| a.item.cmp(&b.item));
    out
}
