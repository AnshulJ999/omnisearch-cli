use crate::errors::SearchError;
use directories::ProjectDirs;
use figment::{
    providers::{Env, Format, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;

/// You.com Search API list price per call, in micro-USD ($5 per 1,000 calls).
pub const YOUCOM_PER_CALL_MICROS: u32 = 5_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub keys: ApiKeys,
    pub settings: Settings,
    #[serde(default)]
    pub paths: Paths,
    #[serde(default)]
    pub tools: ToolPaths,
    #[serde(default)]
    pub research: Research,
    /// AI synthesis of a run (`synth`).
    #[serde(default)]
    pub synth: Synth,
    /// Jina DeepSearch beside a run (`jinadeepsearch`).
    #[serde(default)]
    pub jinadeepsearch: JinaDeepSearch,
    /// The web UI (`osearch ui`).
    #[serde(default)]
    pub ui: Ui,
    /// Each paid service's allowance (`budget`): what balance-aware picking
    /// and `osearch usage` count against.
    #[serde(default = "crate::budget::defaults")]
    pub balance: BTreeMap<String, crate::budget::Allowance>,
}

/// An integer, or a string holding one.
fn de_num<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Num {
        Int(u64),
        Text(String),
    }
    match Num::deserialize(d)? {
        Num::Int(n) => Ok(n),
        Num::Text(t) => t
            .trim()
            .parse()
            .map_err(|_| serde::de::Error::custom(format!("expected a number, got '{t}'"))),
    }
}

fn de_count<'de, D: serde::Deserializer<'de>>(d: D) -> Result<usize, D::Error> {
    de_num(d).map(|n| n as usize)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiKeys {
    #[serde(default)]
    pub parallel: String,
    #[serde(default)]
    pub brave: String,
    #[serde(default)]
    pub serper: String,
    #[serde(default)]
    pub exa: String,
    #[serde(default)]
    pub jina: String,
    #[serde(default)]
    pub linkup: String,
    #[serde(default)]
    pub firecrawl: String,
    #[serde(default)]
    pub tavily: String,
    #[serde(default)]
    pub serpapi: String,
    #[serde(default)]
    pub perplexity: String,
    #[serde(default)]
    pub browserless: String,
    #[serde(default)]
    pub xai: String,
    #[serde(default)]
    pub youcom: String,
}

impl ApiKeys {
    /// The file/`OSEARCH_KEYS_*` value for a provider, or None when the
    /// provider takes no key.
    pub fn get(&self, provider: &str) -> Option<&str> {
        let v = match provider {
            "parallel" => &self.parallel,
            "brave" => &self.brave,
            "serper" => &self.serper,
            "exa" => &self.exa,
            "jina" => &self.jina,
            "linkup" => &self.linkup,
            "firecrawl" => &self.firecrawl,
            "tavily" => &self.tavily,
            "serpapi" => &self.serpapi,
            "perplexity" => &self.perplexity,
            "browserless" => &self.browserless,
            "xai" => &self.xai,
            "youcom" => &self.youcom,
            _ => return None,
        };
        Some(v.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_count", deserialize_with = "de_count")]
    pub count: usize,
}

/// 0: each engine's own count (`budget::preferred_count`).
fn default_count() -> usize {
    0
}

/// Where runs are archived and where the keys file is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Paths {
    /// Run archive root: `<archive>/runs/<id>/`.
    #[serde(default = "default_archive")]
    pub archive: String,
    /// `KEY=value` file read at start; variables already set win.
    #[serde(default = "default_env_file")]
    pub envfile: String,
}

impl Default for Paths {
    fn default() -> Self {
        Self {
            archive: default_archive(),
            envfile: default_env_file(),
        }
    }
}

fn default_archive() -> String {
    data_dir().join("archive").to_string_lossy().into_owned()
}

fn default_env_file() -> String {
    String::new()
}

/// Paths of outside tools; empty = OmniSearch's tools folder, then PATH.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolPaths {
    #[serde(default)]
    pub donsetch: String,
    #[serde(default)]
    pub obscura: String,
    #[serde(default)]
    pub crwl: String,
    #[serde(default)]
    pub ytdlp: String,
    #[serde(default)]
    pub gh: String,
    #[serde(default)]
    pub moli: String,
    /// The light browser at fetch tier 2 before Chromium: moli, obscura or off.
    #[serde(default = "default_light_browser")]
    pub lightbrowser: String,
    /// Chromium for tier 2 and `login`; empty = Playwright's newest Chromium.
    #[serde(default)]
    pub chromium: String,
    /// crwl browser profile used at tier 3.
    #[serde(default = "default_crwl_profile")]
    pub crwlprofile: String,
}

fn default_crwl_profile() -> String {
    "main".to_string()
}

fn default_light_browser() -> String {
    "moli".to_string()
}

/// AI synthesis settings: when it runs, the endpoint and model, what it
/// reads and what the main agent is shown. Off until `url` and `model` are set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Synth {
    /// The endpoint's API style: anthropic (`/v1/messages`) or openai
    /// (`/v1/chat/completions`, also OpenRouter, Ollama, LM Studio).
    #[serde(default = "s_api")]
    pub api: String,
    /// auto, on or off. auto: `deep` and `requests` decide, quick searches use `quick`.
    #[serde(default = "s_auto")]
    pub enabled: String,
    #[serde(default)]
    pub quick: bool,
    #[serde(default = "s_true")]
    pub requests: bool,
    #[serde(default = "s_true")]
    pub deep: bool,
    /// wait (summary and synthesis together) or background (summary first).
    #[serde(default = "s_wait")]
    pub wait: String,
    #[serde(default = "s_url")]
    pub url: String,
    #[serde(default = "s_model")]
    pub model: String,
    /// Sent as x-api-key when set.
    #[serde(default)]
    pub key: String,
    /// Reasoning effort, when the model takes one (anthropic style:
    /// output_config.effort; openai style: reasoning_effort); empty = none.
    #[serde(default = "s_effort")]
    pub effort: String,
    #[serde(default = "s_max_tokens")]
    pub maxtokens: u64,
    /// Seconds to wait for the answer.
    #[serde(default = "s_timeout")]
    pub timeout: u64,
    /// What the main agent is shown: full, short or both.
    #[serde(default = "s_show")]
    pub show: String,
    #[serde(default = "s_short")]
    pub shorttag: String,
    #[serde(default = "s_full")]
    pub fulltag: String,
    /// full (whole pages) or sections (each page's best sections).
    #[serde(default = "s_pages")]
    pub pages: String,
    /// Engine-written answers (marked unverified) go in too.
    #[serde(default = "s_true")]
    pub answers: bool,
    /// Snippets of results that were not fetched go in too.
    #[serde(default = "s_true")]
    pub snippets: bool,
    /// Tokens of material at most: over it, top pages go whole and the rest
    /// as their best sections; pages beyond even that are left out.
    #[serde(default = "s_max_input")]
    pub maxinput: usize,
    /// Tokens per page at most: a longer page goes as its opening plus its
    /// best sections for the question.
    #[serde(default = "s_page_cap")]
    pub pagecap: usize,
    /// A prompt file read at every run; empty = the built-in prompt.
    #[serde(default)]
    pub prompt: String,
    /// USD per million input and output tokens, for the cost log; 0 = take
    /// them from the price list file (`prices`), when one is set.
    #[serde(default)]
    pub inprice: f64,
    #[serde(default)]
    pub outprice: f64,
    /// A price list file in OpenRouter's catalogue shape (USD per token); empty = none.
    #[serde(default = "s_prices")]
    pub prices: String,
}

impl Default for Synth {
    fn default() -> Self {
        toml::from_str("").expect("synth defaults")
    }
}

fn s_auto() -> String {
    "auto".into()
}
fn s_true() -> bool {
    true
}
fn s_wait() -> String {
    "wait".into()
}
fn s_api() -> String {
    "anthropic".into()
}
fn s_url() -> String {
    String::new()
}
fn s_model() -> String {
    String::new()
}
fn s_effort() -> String {
    String::new()
}
fn s_max_tokens() -> u64 {
    32_000
}
fn s_timeout() -> u64 {
    630
}
fn s_show() -> String {
    "full".into()
}
fn s_short() -> String {
    "short".into()
}
fn s_full() -> String {
    "full".into()
}
fn s_pages() -> String {
    "full".into()
}
fn s_max_input() -> usize {
    300_000
}
fn s_page_cap() -> usize {
    30_000
}
fn s_prices() -> String {
    String::new()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ui {
    /// The port `osearch ui` listens on (127.0.0.1 only).
    #[serde(default = "ui_port")]
    pub port: u16,
    /// A notes server endpoint for the review-notes widget; empty = no notes.
    #[serde(default)]
    pub notes: String,
}

impl Default for Ui {
    fn default() -> Self {
        Self { port: ui_port(), notes: String::new() }
    }
}

fn ui_port() -> u16 {
    7820
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JinaDeepSearch {
    /// Run types that start it on their own: deep (deep-mode runs), research
    /// (`osearch research`). Never quick searches.
    #[serde(default = "jd_modes")]
    pub modes: Vec<String>,
    /// Token budget per job (`budget_tokens`).
    #[serde(default = "jd_budget")]
    pub budget: u64,
    /// low, medium or high (`reasoning_effort`).
    #[serde(default = "jd_effort")]
    pub effort: String,
    /// Seconds `osearch wait --jina-deepsearch` waits; the job keeps going.
    #[serde(default = "jd_wait")]
    pub wait: u64,
    /// Seconds after which a job is given up (a hung connection).
    #[serde(default = "jd_stop")]
    pub stop: u64,
}

impl Default for JinaDeepSearch {
    fn default() -> Self {
        Self { modes: jd_modes(), budget: jd_budget(), effort: jd_effort(), wait: jd_wait(), stop: jd_stop() }
    }
}

fn jd_modes() -> Vec<String> {
    vec!["deep".into()]
}
fn jd_budget() -> u64 {
    500_000
}
fn jd_effort() -> String {
    "medium".into()
}
fn jd_wait() -> u64 {
    360
}
fn jd_stop() -> u64 {
    1200
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Research {
    /// Pages fetched per run from the top fused results (request files).
    #[serde(default = "default_fetch")]
    pub fetch: usize,
    /// The same for a quick search (`osearch "query"`, `osearch search`).
    #[serde(default = "default_quick_fetch")]
    pub quickfetch: usize,
    /// Threads read when every query of a run is in community mode (free
    /// tier-0 reads, so more than for web pages).
    #[serde(default = "default_community_fetch")]
    pub communityfetch: usize,
    /// Pages fetched at once.
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    /// Snowball candidates fetched per run (0 = list them only).
    #[serde(default)]
    pub snowball: usize,
    /// Engines for general research when a query names none; empty = picked
    /// by `width` and balance.
    #[serde(default)]
    pub engines: Vec<String>,
    /// standard (about six engines) or wide (every web engine with balance).
    #[serde(default)]
    pub width: crate::budget::Width,
    /// The standard set, in order of preference; empty = `budget::STANDARD`.
    #[serde(default)]
    pub standard: Vec<String>,
    /// Estimated spend allowed per run in USD; queries past it are skipped
    /// with a warning.
    #[serde(default = "default_cost_cap")]
    pub costcap: f64,
    /// Seconds the fetch step of a run may take; slower pages are stopped.
    #[serde(default = "default_fetch_time")]
    pub fetchtime: u64,
    /// The same for a quick search (`osearch "query"`, `osearch search`).
    #[serde(default = "default_quick_time")]
    pub quicktime: u64,
    /// Seconds a run waits for its pages before leaving the rest to the
    /// background (it returns sooner once enough good pages are in).
    #[serde(default = "default_soft_time")]
    pub softtime: u64,
    /// The same for a quick search.
    #[serde(default = "default_quick_soft")]
    pub quicksoft: u64,
    /// Logged-in old-Reddit search in community mode: auto (only when the
    /// keyless routes found fewer than `redditmin` threads), always, never.
    #[serde(default = "default_reddit_login")]
    pub redditlogin: String,
    #[serde(default = "default_reddit_min")]
    pub redditmin: usize,
    /// The saved Reddit account a run's own Reddit use goes through: its
    /// logged-in searches and live reads of the threads it picked
    /// (`osearch login reddit.com --as NAME`); empty = the main login.
    /// Pages an agent asks for go through the main login.
    #[serde(default)]
    pub redditaccount: String,
    /// Live reads of threads a run picked (young or incomplete archive
    /// copies), at most this many per run.
    #[serde(default = "default_reddit_live_max")]
    pub redditlivemax: usize,
    /// Calls the main login may make in one run when the run account fails.
    #[serde(default = "default_reddit_main_max")]
    pub redditmainmax: usize,
    /// Leave a fetched page out of synthesis when most of it is another
    /// page of the run (`dedupemin` percent). The overlap is recorded either way.
    #[serde(default)]
    pub dedupe: bool,
    #[serde(default = "default_dedupe_min")]
    pub dedupemin: u64,
    /// Reddit's own pages without a login (search and the live half of a
    /// thread). Off: Reddit refuses them (HTTP 403); the archive, donsetch and
    /// the saved login do the work. `SEARCH_REDDIT_ROUTE` still overrides.
    #[serde(default)]
    pub redditlive: bool,
    /// X search in social mode through the saved x.com login (`xbird`).
    /// Off by default: it runs as the real account.
    #[serde(default)]
    pub xbird: bool,
    /// The Reddit archive's title search inside subreddits (Arctic Shift).
    /// Off: it times out on its server; thread reads from the archive stay.
    #[serde(default)]
    pub redditarchive: bool,
    /// Repos with fewer stars stay out of the GitHub list beside a question
    /// (0 = no floor; stars still order the list).
    #[serde(default)]
    pub githubminstars: u64,
    /// Pages fetched when every query of a run is in deep mode.
    #[serde(default = "default_fetch")]
    pub deepfetch: usize,
    /// Jina's reranker blended into the ranking before pages are picked.
    #[serde(default)]
    pub rerank: bool,
    /// Pages added per question of a request past the third (the default
    /// page count is for up to three questions).
    #[serde(default = "default_fetch_grow")]
    pub fetchgrow: usize,
    /// The most pages the growth reaches. A run asked for more is not cut.
    #[serde(default = "default_fetch_max")]
    pub fetchmax: usize,
    /// A request's `fetch` below the run's default is raised to it (0 stays
    /// 0: search only).
    #[serde(default = "default_true")]
    pub fetchfloor: bool,
    /// On forum-thread addresses donsetch and the built-in fetchers run
    /// together and the longer copy is kept.
    #[serde(default = "default_true")]
    pub forumboth: bool,
    /// Telegram side searches get a guaranteed page like the other side
    /// searches; off, their posts compete by rank only.
    #[serde(default)]
    pub telegramslot: bool,
    /// Characters of a result's snippet in the summary's top results.
    #[serde(default = "default_snippet_chars")]
    pub snippetchars: usize,
    /// Results listed in the summary's top results.
    #[serde(default = "default_top_results")]
    pub topresults: usize,
    /// Whose snippet the summary shows first and synthesis lists first; the
    /// longest snippet when none of these found the result.
    #[serde(default = "default_snippet_prefer")]
    pub snippetprefer: Vec<String>,
}

impl Default for Research {
    fn default() -> Self {
        Self {
            fetch: default_fetch(),
            quickfetch: default_quick_fetch(),
            communityfetch: default_community_fetch(),
            concurrency: default_concurrency(),
            snowball: 0,
            engines: Vec::new(),
            width: crate::budget::Width::default(),
            standard: Vec::new(),
            costcap: default_cost_cap(),
            fetchtime: default_fetch_time(),
            quicktime: default_quick_time(),
            softtime: default_soft_time(),
            quicksoft: default_quick_soft(),
            redditlogin: default_reddit_login(),
            redditmin: default_reddit_min(),
            redditaccount: String::new(),
            redditlivemax: default_reddit_live_max(),
            redditmainmax: default_reddit_main_max(),
            dedupe: false,
            dedupemin: default_dedupe_min(),
            redditlive: false,
            redditarchive: false,
            xbird: false,
            githubminstars: 0,
            deepfetch: default_fetch(),
            rerank: false,
            fetchgrow: default_fetch_grow(),
            fetchmax: default_fetch_max(),
            fetchfloor: true,
            forumboth: true,
            telegramslot: false,
            snippetchars: default_snippet_chars(),
            topresults: default_top_results(),
            snippetprefer: default_snippet_prefer(),
        }
    }
}

fn default_fetch() -> usize {
    30
}
fn default_fetch_grow() -> usize {
    5
}
fn default_fetch_max() -> usize {
    50
}
fn default_true() -> bool {
    true
}
fn default_snippet_chars() -> usize {
    220
}
fn default_top_results() -> usize {
    15
}
fn default_snippet_prefer() -> Vec<String> {
    ["exa", "brave", "parallel", "linkup", "youcom", "tavily"].iter().map(|s| s.to_string()).collect()
}
fn default_reddit_live_max() -> usize {
    8
}
fn default_reddit_main_max() -> usize {
    8
}
fn default_dedupe_min() -> u64 {
    80
}
fn default_quick_fetch() -> usize {
    20
}
fn default_community_fetch() -> usize {
    20
}
fn default_concurrency() -> usize {
    10
}
fn default_cost_cap() -> f64 {
    1.0
}
fn default_fetch_time() -> u64 {
    90
}
fn default_soft_time() -> u64 {
    20
}
fn default_quick_soft() -> u64 {
    10
}
fn default_reddit_login() -> String {
    "always".into()
}
fn default_reddit_min() -> usize {
    10
}
fn default_quick_time() -> u64 {
    30
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            keys: ApiKeys::default(),
            settings: Settings {
                count: default_count(),
            },
            paths: Paths::default(),
            tools: ToolPaths {
                crwlprofile: default_crwl_profile(),
                ..ToolPaths::default()
            },
            research: Research::default(),
            synth: Synth::default(),
            jinadeepsearch: JinaDeepSearch::default(),
            ui: Ui::default(),
            balance: crate::budget::defaults(),
        }
    }
}

static CONFIG: OnceLock<AppConfig> = OnceLock::new();

/// Make the loaded config readable everywhere (tools, run archive).
pub fn init(config: &AppConfig) {
    let _ = CONFIG.set(config.clone());
}

/// The loaded config, or the defaults before `init`.
pub fn get() -> &'static AppConfig {
    CONFIG.get_or_init(AppConfig::default)
}

fn project() -> Option<ProjectDirs> {
    ProjectDirs::from("", "", "omnisearch")
}

/// One folder for config and data, when chosen: `OMNISEARCH_HOME`, else the
/// first line of a file named `omnisearch-home` next to the executable.
/// Otherwise the platform's own folders. Packaged (MSIX) apps such as the
/// Claude desktop app redirect AppData writes of the processes they start
/// into a private copy, so on Windows a home outside AppData keeps one set
/// of logins and logs for every terminal.
fn home_root() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("OMNISEARCH_HOME").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let exe = std::env::current_exe().ok()?;
    let text = std::fs::read_to_string(exe.parent()?.join("omnisearch-home")).ok()?;
    let line = text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#'))?;
    Some(PathBuf::from(line))
}

pub fn config_dir() -> PathBuf {
    if let Some(root) = home_root() {
        return root.join("config");
    }
    project()
        .map(|p| p.config_dir().to_path_buf())
        .unwrap_or_else(|| home().join(".config").join("omnisearch"))
}

/// Cache, logs, cookie store, tools.
pub fn data_dir() -> PathBuf {
    if let Some(root) = home_root() {
        return root.join("data");
    }
    project()
        .map(|p| p.data_local_dir().to_path_buf())
        .unwrap_or_else(|| home().join(".local").join("share").join("omnisearch"))
}

fn home() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn load_config() -> Result<AppConfig, Box<figment::Error>> {
    Ok(Figment::new()
        .merge(Serialized::defaults(AppConfig::default()))
        .merge(Toml::file(config_path()))
        .merge(Env::prefixed("OSEARCH_").split("_"))
        .extract()?)
}

/// Read `KEY=value` lines into the process environment. A variable already
/// set wins, so a session can override the file. Returns how many were set.
pub fn load_env_file(path: &str) -> usize {
    if path.is_empty() {
        return 0;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return 0;
    };
    let mut n = 0;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim();
        if k.is_empty() || k.contains(char::is_whitespace) {
            continue;
        }
        let v = v.trim().trim_matches('"').trim_matches('\'');
        if std::env::var_os(k).is_none() && !v.is_empty() {
            std::env::set_var(k, v);
            n += 1;
        }
    }
    n
}

/// Valid provider key names for `config set keys.<name>`.
pub fn provider_keys() -> Vec<&'static str> {
    crate::providers::all()
        .iter()
        .filter(|s| !s.env_keys.is_empty())
        .map(|s| s.name)
        .collect()
}

pub fn mask_key(key: &str) -> String {
    let n = key.chars().count();
    if key.is_empty() {
        "(not set)".to_string()
    } else if n <= 8 {
        let head: String = key.chars().take(2).collect();
        format!("{head}***")
    } else {
        let head: String = key.chars().take(4).collect();
        let tail: String = key.chars().skip(n - 4).collect();
        format!("{head}...{tail}")
    }
}

/// `osearch config show`: paths, keys (masked), research settings, limits.
pub fn config_show(config: &AppConfig) {
    println!("config file: {}", config_path().display());
    println!("data folder: {}", data_dir().display());
    println!("archive:     {}", config.paths.archive);
    println!("keys file:   {}", config.paths.envfile);
    println!("\n[keys]");
    for spec in crate::providers::all()
        .iter()
        .filter(|s| !s.env_keys.is_empty())
    {
        let key = crate::providers::key_for(config, spec);
        println!("  {:<12} {}", spec.name, mask_key(&key));
    }
    println!("\n[research]");
    let r = &config.research;
    println!("  fetch       {} (quick searches {}, community {}, deep {})", r.fetch, r.quickfetch, r.communityfetch, r.deepfetch);
    println!(
        "  fetchgrow   {} more pages per question past the third, up to {} (fetchmax) · fetchfloor {} (a request's fetch below the default is raised to it)",
        r.fetchgrow,
        r.fetchmax,
        if r.fetchfloor { "on" } else { "off" }
    );
    println!("  forumboth   {} (forum threads: donsetch and the built-in fetchers together, the longer copy kept)", if r.forumboth { "on" } else { "off" });
    println!("  telegramslot {} (Telegram side searches get a guaranteed page; off: by rank only)", if r.telegramslot { "on" } else { "off" });
    println!("  snippets    {} chars, {} top results in the summary · preferred: {}", r.snippetchars, r.topresults, r.snippetprefer.join(", "));
    println!("  concurrency {}", r.concurrency);
    println!("  snowball    {}", r.snowball);
    println!("  costcap     ${:.2}", r.costcap);
    println!("  fetchtime   {} s (quick searches {} s)", r.fetchtime, r.quicktime);
    println!("  softtime    {} s (quick searches {} s): then the rest finishes in the background", r.softtime, r.quicksoft);
    println!("  width       {}", r.width.as_str());
    println!(
        "  standard    {}",
        if r.standard.is_empty() {
            crate::budget::STANDARD.join(", ")
        } else {
            r.standard.join(", ")
        }
    );
    println!(
        "  engines     {}",
        if r.engines.is_empty() {
            "(picked by width and balance)".to_string()
        } else {
            r.engines.join(", ")
        }
    );
    let threshold = if r.redditlogin == "auto" { format!(" (when fewer than {} threads)", r.redditmin) } else { String::new() };
    println!("  redditlogin {}{threshold} · account {}", r.redditlogin, if r.redditaccount.is_empty() { "main" } else { &r.redditaccount });
    println!("  redditlivemax {} (live reads of picked threads per run) · redditmainmax {} (main-login calls per run when the run account fails)", r.redditlivemax, r.redditmainmax);
    println!("  dedupe      {} (leave near-copies, {}% the same, out of synthesis; the overlap is recorded either way)", if r.dedupe { "on" } else { "off" }, r.dedupemin);
    println!("  redditlive  {} (Reddit's own pages without a login)", if r.redditlive { "on" } else { "off" });
    println!("  redditarchive {} (the archive's subreddit title search)", if r.redditarchive { "on" } else { "off" });
    println!("  xbird       {} (X search in social mode with the saved x.com login)", if r.xbird { "on" } else { "off" });
    println!("  githubminstars {} (0 = no star floor for the GitHub list)", r.githubminstars);
    println!("  rerank      {} (Jina's reranker blended into the ranking)", if r.rerank { "on" } else { "off" });
    let s = &config.synth;
    println!("\n[synth]");
    println!("  enabled     {} (quick {}, requests {}, deep {}) · {}", s.enabled, s.quick, s.requests, s.deep, s.wait);
    if crate::synth::configured() || !s.url.is_empty() || !s.model.is_empty() {
        println!("  model       {} at {} ({} style, effort {})", s.model, s.url, s.api, if s.effort.is_empty() { "none" } else { &s.effort });
    } else {
        println!("  model       (not set up: synthesis is off until synth.url and synth.model are set)");
    }
    println!(
        "  reads       pages {} · engine answers {} · other results {} · up to {} tokens, {} per page",
        s.pages, s.answers, s.snippets, s.maxinput, s.pagecap
    );
    println!("  shows       {} (tags <{}> <{}>)", s.show, s.shorttag, s.fulltag);
    if s.inprice > 0.0 || s.outprice > 0.0 {
        println!("  price       ${} in, ${} out per million tokens (set by hand)", s.inprice, s.outprice);
    } else if !s.prices.is_empty() {
        println!("  price       from {}", s.prices);
    } else {
        println!("  price       unknown (set synth.inprice/outprice or synth.prices for the cost log)");
    }
    let j = &config.jinadeepsearch;
    println!("\n[jinadeepsearch]");
    println!(
        "  modes       {} · budget {} tokens · effort {} · wait {} s · stop {} s",
        if j.modes.is_empty() { "(off)".to_string() } else { j.modes.join(", ") },
        j.budget,
        j.effort,
        j.wait,
        j.stop
    );
    println!("\n[ui]");
    println!("  port        {} · notes {}", config.ui.port, if config.ui.notes.is_empty() { "off" } else { &config.ui.notes });
    println!("\n[balance] (osearch usage shows what is left)");
    for (k, a) in &config.balance {
        let when = match a.kind {
            crate::budget::Kind::Monthly => format!("monthly, refills on day {}", a.resetday),
            crate::budget::Kind::Onetime => format!("onetime, as of {}", a.checked),
            crate::budget::Kind::Expiring => format!("expires {}, as of {}", a.expires, a.checked),
        };
        println!("  {k:<14} {} {} · {when}{}", a.amount, a.unit, if a.plentiful { " · plentiful" } else { "" });
    }
    println!();
}

pub fn config_set(key: &str, value: &str) -> Result<(), SearchError> {
    let path = config_path();
    let mut doc: toml::Table = if path.exists() {
        std::fs::read_to_string(&path)?
            .parse()
            .map_err(|e: toml::de::Error| SearchError::Config(e.to_string()))?
    } else {
        toml::Table::new()
    };
    let parts: Vec<&str> = key.split('.').collect();
    if let ["balance", service, field] = parts.as_slice() {
        return set_balance(&path, doc, service, field, value);
    }
    let [section, name] = parts.as_slice() else {
        return Err(SearchError::Config(format!(
            "Invalid key '{key}'. Use a dotted form like 'keys.exa', 'research.fetch' or 'balance.exa.amount'."
        )));
    };
    let typed = match *section {
        "keys" => {
            let valid = provider_keys();
            if !valid.contains(name) {
                return Err(SearchError::Config(format!(
                    "Unknown provider 'keys.{name}'. Valid: {}",
                    valid.join(", ")
                )));
            }
            toml::Value::String(value.to_string())
        }
        "settings" | "research" => match *name {
            "redditlogin" => match value {
                "auto" | "always" | "never" => toml::Value::String(value.to_string()),
                _ => return Err(SearchError::Config(format!("{key} must be auto, always or never"))),
            },
            "redditaccount" => toml::Value::String(value.to_string()),
            "redditlive" | "redditarchive" | "xbird" | "rerank" | "dedupe" | "fetchfloor" | "forumboth" | "telegramslot" => match value {
                "true" | "on" | "yes" => toml::Value::Boolean(true),
                "false" | "off" | "no" => toml::Value::Boolean(false),
                _ => return Err(SearchError::Config(format!("{key} must be true or false"))),
            },
            "count" | "fetch" | "quickfetch" | "communityfetch" | "concurrency" | "snowball" | "fetchtime"
            | "quicktime" | "softtime" | "quicksoft" | "redditmin" | "githubminstars" | "deepfetch"
            | "redditlivemax" | "redditmainmax" | "dedupemin" | "fetchgrow" | "fetchmax" | "snippetchars"
            | "topresults" => {
                toml::Value::Integer(value.parse::<i64>().map_err(|_| {
                    SearchError::Config(format!("{key} must be a whole number, got '{value}'"))
                })?)
            }
            "costcap" => toml::Value::Float(value.parse::<f64>().map_err(|_| {
                SearchError::Config(format!("{key} must be a number, got '{value}'"))
            })?),
            "width" => match value {
                "standard" | "wide" => toml::Value::String(value.to_string()),
                _ => return Err(SearchError::Config(format!("{key} must be standard or wide"))),
            },
            "engines" | "standard" | "snippetprefer" => toml::Value::Array(
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| toml::Value::String(s.to_string()))
                    .collect(),
            ),
            other => {
                return Err(SearchError::Config(format!(
                    "Unknown setting '{section}.{other}'"
                )))
            }
        },
        "paths" | "tools" => toml::Value::String(value.to_string()),
        "ui" => match *name {
            "port" => toml::Value::Integer(value.parse::<u16>().map_err(|_| SearchError::Config(format!("{key} must be a port number")))?.into()),
            "notes" => toml::Value::String(value.to_string()),
            other => return Err(SearchError::Config(format!("Unknown setting 'ui.{other}'"))),
        },
        "jinadeepsearch" => match *name {
            "modes" => toml::Value::Array(
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| toml::Value::String(s.to_string()))
                    .collect(),
            ),
            "budget" | "wait" | "stop" => toml::Value::Integer(value.parse::<i64>().map_err(|_| {
                SearchError::Config(format!("{key} must be a whole number, got '{value}'"))
            })?),
            "effort" if ["low", "medium", "high"].contains(&value) => toml::Value::String(value.to_string()),
            "effort" => return Err(SearchError::Config(format!("{key} must be low, medium or high"))),
            other => return Err(SearchError::Config(format!("Unknown setting 'jinadeepsearch.{other}'"))),
        },
        "synth" => match *name {
            "quick" | "requests" | "deep" | "answers" | "snippets" => match value {
                "true" | "on" | "yes" => toml::Value::Boolean(true),
                "false" | "off" | "no" => toml::Value::Boolean(false),
                _ => return Err(SearchError::Config(format!("{key} must be true or false"))),
            },
            "maxtokens" | "timeout" | "maxinput" | "pagecap" => toml::Value::Integer(value.parse::<i64>().map_err(|_| {
                SearchError::Config(format!("{key} must be a whole number, got '{value}'"))
            })?),
            "inprice" | "outprice" => toml::Value::Float(value.parse::<f64>().map_err(|_| {
                SearchError::Config(format!("{key} must be a number, got '{value}'"))
            })?),
            "enabled" if !["auto", "on", "off"].contains(&value) => {
                return Err(SearchError::Config(format!("{key} must be auto, on or off")))
            }
            "wait" if !["wait", "background"].contains(&value) => {
                return Err(SearchError::Config(format!("{key} must be wait or background")))
            }
            "show" if !["full", "short", "both"].contains(&value) => {
                return Err(SearchError::Config(format!("{key} must be full, short or both")))
            }
            "pages" if !["full", "sections"].contains(&value) => {
                return Err(SearchError::Config(format!("{key} must be full or sections")))
            }
            "api" if !["anthropic", "openai"].contains(&value) => {
                return Err(SearchError::Config(format!("{key} must be anthropic or openai")))
            }
            "enabled" | "wait" | "url" | "model" | "key" | "effort" | "show" | "shorttag" | "fulltag" | "pages"
            | "prompt" | "prices" | "api" => toml::Value::String(value.to_string()),
            other => return Err(SearchError::Config(format!("Unknown setting 'synth.{other}'"))),
        },
        other => {
            return Err(SearchError::Config(format!(
                "Unknown section '{other}'. Use keys, settings, research, synth, jinadeepsearch, ui, balance, paths or tools."
            )))
        }
    };
    let entry = doc
        .entry(*section)
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    match entry {
        toml::Value::Table(t) => {
            t.insert(name.to_string(), typed);
        }
        _ => {
            return Err(SearchError::Config(format!(
                "Cannot set '{key}': '{section}' exists but is not a table."
            )))
        }
    }
    write_doc(&path, &doc)
}

fn write_doc(path: &std::path::Path, doc: &toml::Table) -> Result<(), SearchError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// `config set balance.<service>.<field> <value>`: amount, unitprice (numbers),
/// resetday (a day), kind, unit, expires, checked (text), plentiful (true/false).
/// Setting the amount of a onetime or expiring balance also dates it today.
fn set_balance(
    path: &std::path::Path,
    mut doc: toml::Table,
    service: &str,
    field: &str,
    value: &str,
) -> Result<(), SearchError> {
    let num = |v: &str| {
        v.parse::<f64>()
            .map_err(|_| SearchError::Config(format!("balance.{service}.{field} must be a number, got '{v}'")))
    };
    let typed = match field {
        "amount" | "unitprice" => toml::Value::Float(num(value)?),
        "resetday" => match value.parse::<i64>() {
            Ok(d @ 1..=31) => toml::Value::Integer(d),
            _ => return Err(SearchError::Config("resetday must be a day of the month (1-31)".into())),
        },
        "kind" => match value {
            "monthly" | "onetime" | "expiring" => toml::Value::String(value.into()),
            _ => return Err(SearchError::Config("kind must be monthly, onetime or expiring".into())),
        },
        "plentiful" => toml::Value::Boolean(value == "true"),
        "unit" | "expires" | "checked" => toml::Value::String(value.into()),
        other => {
            return Err(SearchError::Config(format!(
                "Unknown balance field '{other}'. Use amount, unit, unitprice, kind, resetday, expires, checked or plentiful."
            )))
        }
    };
    let balance = doc
        .entry("balance")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let toml::Value::Table(balance) = balance else {
        return Err(SearchError::Config("'balance' in the config file is not a table".into()));
    };
    let entry = balance
        .entry(service)
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let toml::Value::Table(entry) = entry else {
        return Err(SearchError::Config(format!("'balance.{service}' is not a table")));
    };
    entry.insert(field.to_string(), typed);
    if field == "amount" {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        entry.insert("checked".into(), toml::Value::String(today));
    }
    write_doc(path, &doc)
}
