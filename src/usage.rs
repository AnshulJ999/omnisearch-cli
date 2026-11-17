//! `osearch usage`: every engine's balance in one table. Live readings come
//! from the vendors' own balance endpoints; the rest is counted from the
//! allowance in config (`budget`). Every live reading is logged
//! (balances.jsonl), so later estimates start from it.

use crate::context::AppContext;
use crate::providers;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;
use tokio::time::timeout;

const USAGE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Serialize)]
pub struct Reading {
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

async fn get_json(
    ctx: &AppContext,
    url: &str,
    bearer: Option<&str>,
    header: Option<(&str, &str)>,
) -> Result<serde_json::Value, String> {
    let mut req = ctx.client.get(url);
    if let Some(token) = bearer {
        req = req.bearer_auth(token);
    }
    if let Some((k, v)) = header {
        req = req.header(k, v);
    }
    let resp = req
        .timeout(USAGE_TIMEOUT)
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let excerpt: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        let excerpt: String = excerpt.chars().take(160).collect();
        return Err(format!("HTTP {status}: {excerpt}"));
    }
    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| e.to_string())
}

/// A reading in the service's own budget unit: `budget` starts its estimates
/// from these.
fn reading(remaining: Option<f64>, unit: &str, extra: serde_json::Value) -> Result<serde_json::Value, String> {
    let r = remaining.ok_or("no balance in the answer")?;
    let mut v = serde_json::json!({ "credits_remaining": r, "unit": unit, "budget_unit": unit });
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        o.extend(e.clone());
    }
    Ok(v)
}

fn num(v: Option<&serde_json::Value>) -> Option<f64> {
    v.and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok())))
}

/// SerpApi: GET https://serpapi.com/account.json?api_key=...
async fn serpapi_usage(ctx: &AppContext, key: String) -> Result<serde_json::Value, String> {
    let url = format!("https://serpapi.com/account.json?api_key={key}");
    let v = get_json(ctx, &url, None, None).await?;
    reading(
        num(v.get("total_searches_left").or(v.get("plan_searches_left"))),
        "searches",
        serde_json::json!({ "searches_per_month": v.get("searches_per_month").cloned() }),
    )
}

/// Brave has no balance endpoint; every search answer carries
/// X-RateLimit-Remaining, whose second value is the monthly quota left. So
/// the check costs one request, and only `osearch usage` makes it.
async fn brave_usage(ctx: &AppContext, key: String) -> Result<serde_json::Value, String> {
    let resp = ctx
        .client
        .get("https://api.search.brave.com/res/v1/web/search?q=a&count=1")
        .header("X-Subscription-Token", key)
        .header("Accept", "application/json")
        .timeout(USAGE_TIMEOUT)
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    let remaining = resp
        .headers()
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    let monthly = remaining
        .as_deref()
        .and_then(|s| s.split(',').nth(1))
        .and_then(|s| s.trim().parse::<f64>().ok());
    reading(monthly, "requests", serde_json::json!({ "note": "this check used one request" }))
}

/// xAI: GET https://management-api.x.ai/v1/billing/teams/{team_id}/prepaid/balance.
/// Needs a management key and a team id (not the xai- inference key).
async fn xai_usage(ctx: &AppContext, _key: String) -> Result<serde_json::Value, String> {
    let mgmt_key = std::env::var("XAI_MANAGEMENT_API_KEY").unwrap_or_default();
    let team_id = std::env::var("XAI_TEAM_ID").unwrap_or_default();
    if mgmt_key.is_empty() || team_id.is_empty() {
        return Err("set XAI_MANAGEMENT_API_KEY and XAI_TEAM_ID (management key differs from the xai- inference key)".into());
    }
    let url = format!("https://management-api.x.ai/v1/billing/teams/{team_id}/prepaid/balance");
    let v = get_json(ctx, &url, Some(&mgmt_key), None).await?;
    let cents = num(v.get("total").and_then(|t| t.get("val")));
    reading(cents.map(|c| c / 100.0), "usd", serde_json::json!({}))
}

/// Firecrawl: GET https://api.firecrawl.dev/v2/team/credit-usage (Bearer).
async fn firecrawl_usage(ctx: &AppContext, key: String) -> Result<serde_json::Value, String> {
    let v = get_json(ctx, "https://api.firecrawl.dev/v2/team/credit-usage", Some(&key), None).await?;
    let data = v.get("data").unwrap_or(&v);
    reading(
        num(data.get("remainingCredits")),
        "credits",
        serde_json::json!({
            "plan_credits": data.get("planCredits").cloned(),
            "period_end": data.get("billingPeriodEnd").cloned(),
        }),
    )
}

/// Tavily: GET https://api.tavily.com/usage (Bearer): limit minus usage.
async fn tavily_usage(ctx: &AppContext, key: String) -> Result<serde_json::Value, String> {
    let v = get_json(ctx, "https://api.tavily.com/usage", Some(&key), None).await?;
    let account = v.get("account").unwrap_or(&v);
    let left = match (num(account.get("plan_usage")), num(account.get("plan_limit"))) {
        (Some(u), Some(l)) => Some(l - u),
        _ => None,
    };
    reading(left, "credits", serde_json::json!({ "plan_limit": account.get("plan_limit").cloned() }))
}

/// Linkup: GET https://api.linkup.so/v1/credits/balance (Bearer) -> { balance } in USD.
async fn linkup_usage(ctx: &AppContext, key: String) -> Result<serde_json::Value, String> {
    let v = get_json(ctx, "https://api.linkup.so/v1/credits/balance", Some(&key), None).await?;
    reading(num(v.get("balance")), "usd", serde_json::json!({}))
}

/// You.com: GET https://api.you.com/v1/billing/account_balance -> data.attributes.balance in cents.
async fn youcom_usage(ctx: &AppContext, key: String) -> Result<serde_json::Value, String> {
    let v = get_json(ctx, "https://api.you.com/v1/billing/account_balance", None, Some(("X-API-Key", &key))).await?;
    let cents = num(v.pointer("/data/attributes/balance"));
    reading(cents.map(|c| c / 100.0), "usd", serde_json::json!({}))
}

/// Jina: the dashboard's own endpoint (undocumented, so a failure is only a
/// missing reading). Only the wallet balance is kept: the answer also holds
/// the account's name and address.
async fn jina_usage(ctx: &AppContext, key: String) -> Result<serde_json::Value, String> {
    let url = format!("https://embeddings-dashboard-api.jina.ai/api/v1/api_key/fe_user?api_key={key}");
    let v = get_json(ctx, &url, None, None).await?;
    reading(num(v.pointer("/wallet/total_balance")), "tokens", serde_json::json!({}))
}

/// Serper: GET https://google.serper.dev/account -> { balance, rateLimit }
/// (not in Serper's docs; seen working 2026-11-08).
async fn serper_usage(ctx: &AppContext, key: String) -> Result<serde_json::Value, String> {
    let v = get_json(ctx, "https://google.serper.dev/account", None, Some(("X-API-KEY", &key))).await?;
    reading(num(v.get("balance")), "credits", serde_json::json!({ "rate_limit": v.get("rateLimit").cloned() }))
}

type UsageFetch = fn(
    &AppContext,
    String,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send + '_>>;

/// The providers with a balance endpoint, and whether reading it costs a request.
fn fetcher(name: &str) -> Option<(UsageFetch, bool)> {
    let f: UsageFetch = match name {
        "serpapi" => |ctx, key| Box::pin(serpapi_usage(ctx, key)),
        "firecrawl" => |ctx, key| Box::pin(firecrawl_usage(ctx, key)),
        "brave" => |ctx, key| Box::pin(brave_usage(ctx, key)),
        "xai" => |ctx, key| Box::pin(xai_usage(ctx, key)),
        "tavily" => |ctx, key| Box::pin(tavily_usage(ctx, key)),
        "linkup" => |ctx, key| Box::pin(linkup_usage(ctx, key)),
        "youcom" => |ctx, key| Box::pin(youcom_usage(ctx, key)),
        "jina" => |ctx, key| Box::pin(jina_usage(ctx, key)),
        "serper" => |ctx, key| Box::pin(serper_usage(ctx, key)),
        _ => return None,
    };
    Some((f, name == "brave"))
}

/// Whether a provider has a free live balance check.
pub fn has_free_check(name: &str) -> bool {
    fetcher(name).is_some_and(|(_, paid)| !paid)
}

/// Read the live balances of `only` (every configured provider with a check
/// when None); `paid_checks` also runs checks that cost a request (Brave).
/// The readings are logged.
pub async fn collect(ctx: Arc<AppContext>, only: Option<&[String]>, paid_checks: bool) -> Vec<Reading> {
    let mut set: JoinSet<Reading> = JoinSet::new();
    for p in providers::build_providers(&ctx) {
        let name = p.name();
        if !p.is_configured() || only.is_some_and(|o| !o.iter().any(|x| x == name)) {
            continue;
        }
        let Some((fetch, paid)) = fetcher(name) else { continue };
        if paid && !paid_checks {
            continue;
        }
        let key = providers::find(name)
            .map(|spec| providers::key_for(&ctx.config, spec))
            .unwrap_or_default();
        let ctx2 = ctx.clone();
        let name = name.to_string();
        set.spawn(async move {
            match timeout(USAGE_TIMEOUT, fetch(&ctx2, key)).await {
                Ok(Ok(data)) => Reading { provider: name, data: Some(data), error: None },
                Ok(Err(e)) => Reading { provider: name, data: None, error: Some(crate::errors::redact_secrets(&e)) },
                Err(_) => Reading { provider: name, data: None, error: Some("timed out".into()) },
            }
        });
    }
    let mut out = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok(r) = joined {
            out.push(r);
        }
    }
    out.sort_by(|a, b| a.provider.cmp(&b.provider));
    let snapshot: serde_json::Map<String, serde_json::Value> = out
        .iter()
        .filter_map(|u| u.data.clone().map(|d| (u.provider.clone(), d)))
        .collect();
    if !snapshot.is_empty() {
        crate::logging::log_balances(&serde_json::Value::Object(snapshot));
    }
    out
}

/// Before a run: refresh the free live readings older than `budget::STALE_SECS`,
/// waiting at most a few seconds.
pub async fn refresh_stale(ctx: Arc<AppContext>) {
    let stale: Vec<String> = crate::budget::statuses()
        .into_iter()
        .filter(|s| !s.live && has_free_check(&s.service))
        .map(|s| s.service)
        .collect();
    if stale.is_empty() {
        return;
    }
    let _ = timeout(Duration::from_secs(5), collect(ctx, Some(&stale), false)).await;
}

/// The whole table as data (`--json`).
#[derive(Serialize)]
pub struct Report {
    pub balances: Vec<crate::budget::Status>,
    pub readings: Vec<Reading>,
    pub free: Vec<String>,
    pub no_key: Vec<String>,
    pub warnings: Vec<String>,
}

pub async fn report(ctx: Arc<AppContext>) -> Report {
    let readings = collect(ctx.clone(), None, true).await;
    let balances = crate::budget::statuses();
    let mut free = Vec::new();
    let mut no_key = Vec::new();
    for p in providers::build_providers(&ctx) {
        let name = p.name().to_string();
        let spec = p.spec();
        let searches = spec.modes.iter().any(|m| !matches!(m, crate::types::Mode::Extract | crate::types::Mode::Scrape));
        if providers::SOCIAL_PLATFORMS.contains(&name.as_str()) || !searches {
            continue;
        }
        if spec.env_keys.is_empty() {
            free.push(name);
        } else if !p.is_configured() {
            no_key.push(name);
        }
    }
    let warnings = crate::budget::warnings(&balances);
    Report { balances, readings, free, no_key, warnings }
}

fn amount(v: f64, unit: &str) -> String {
    match unit {
        "usd" => format!("${v:.2}"),
        _ if v >= 1e6 => format!("{:.0}M {unit}", v / 1e6),
        _ => format!("{v:.0} {unit}"),
    }
}

pub fn render_human(r: &Report) {
    println!("{:<15} {:>14}  {:<32} {:<26} {}", "service", "left", "allowance", "figure", "refills / expires");
    for st in &r.balances {
        let allowance = match st.kind {
            crate::budget::Kind::Monthly => format!("{} a month", amount(st.total, &st.unit)),
            crate::budget::Kind::Onetime if st.plentiful => format!("onetime {} (plentiful)", amount(st.total, &st.unit)),
            crate::budget::Kind::Onetime => format!("onetime {}", amount(st.total, &st.unit)),
            crate::budget::Kind::Expiring => format!("expiring {}", amount(st.total, &st.unit)),
        };
        let figure = if st.live {
            format!("live {}", st.as_of)
        } else {
            format!("estimated from {}", st.as_of)
        };
        println!(
            "{:<15} {:>14}  {:<32} {:<26} {}",
            st.service,
            amount(st.remaining, &st.unit),
            allowance,
            figure,
            st.until.clone().unwrap_or_default()
        );
    }
    for e in r.readings.iter().filter(|u| u.error.is_some()) {
        println!("  {} live check failed: {}", e.provider, e.error.clone().unwrap_or_default());
    }
    if !r.free.is_empty() {
        println!("free (no key, no balance): {}", r.free.join(", "));
    }
    if !r.no_key.is_empty() {
        println!("no key: {}", r.no_key.join(", "));
    }
    for w in &r.warnings {
        println!("! {w}");
    }
    println!("Estimates count the logged list-price spend since the figure's date; set a balance with `osearch config set balance.<service>.amount <n>`.");
}
