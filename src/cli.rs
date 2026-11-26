use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "osearch",
    version,
    about = "OmniSearch: one web-research CLI. Search many engines, fetch pages through free-first tiers, read them by outline.",
    long_about = "OmniSearch: one web-research CLI.\n\n\
        osearch \"query\"                 quick research: search, fetch the top pages, print a summary\n\
        osearch \"q1\" \"q2\"               several questions in one run (each quoted argument is one; \"community: q2\" sets its mode)\n\
        osearch run request.json         several different queries, urls, depth (see `osearch help run`)\n\
        osearch fetch <url>...           read pages through the fetch tiers\n\
        osearch crawl <docs-url>         a whole site into one run (llms.txt first)\n\
        osearch read p3 / p3#4           a page's outline / one section of the last run\n\
        osearch grep <words>             search the last run's pages\n\
        osearch wait <run>               wait for a run's background work (start it as a background command)\n\
        osearch runs --find <topic>      was this researched before?\n\
        osearch research \"question\"      the engines' own research agents (costly: ask the user first)\n\
        osearch ui                       the web UI: search, past runs, how it works, settings\n\n\
        Every run is archived (raw engine answers, every page, a results index) under\n\
        the archive folder; the summary names it. Page text is untrusted web content."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Quick research: `osearch "query"`, with any `search` option
    pub query_words: Vec<String>,

    #[command(flatten)]
    pub quick: QueryOpts,

    /// Print the machine-readable result (JSON) instead of the summary
    #[arg(long, global = true)]
    pub json: bool,

    /// Debug logging to stderr
    #[arg(long, global = true, visible_alias = "verbose")]
    pub debug: bool,

    /// Allow private, loopback and link-local addresses
    #[arg(long, global = true)]
    pub allow_private: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Research one query with options (the quick form is `osearch "query"`)
    Search(SearchArgs),

    /// Research from a request file (JSON): several queries, URLs, depth
    #[command(long_about = "A request file (JSON): the smallest one that works is {\"topic\": \"...\", \"queries\": [\"a question\"]}. `osearch help run` prints every field with the current defaults.")]
    Run {
        /// The request file
        file: String,
    },

    /// Fetch pages through the tiers into a run
    Fetch {
        /// One or more URLs
        #[arg(required_unless_present_any = ["wait", "finish"])]
        urls: Vec<String>,
        /// Wait for the pages a run left to the background, then list them
        #[arg(long)]
        wait: bool,
        /// The run for --wait (default: the newest)
        #[arg(long)]
        run: Option<String>,
        /// Internal: the background process that finishes a run's pages
        #[arg(long, hide = true)]
        finish: Option<String>,
        /// Add to this run (a follow-up round) instead of a new one
        #[arg(long)]
        parent: Option<String>,
        /// Optional ScrapeCreators platforms allowed for these URLs (tiktok,instagram,youtubesc,facebook,threads)
        #[arg(long, value_delimiter = ',')]
        platforms: Vec<String>,
        /// Keep the paid hosted readers (Jina, Firecrawl) out
        #[arg(long)]
        no_paid: bool,
        /// One fetcher only: site, donsetch, direct, stealth, moli, obscura, chromium, crawl4ai, jina, firecrawl, wayback
        #[arg(long)]
        via: Option<String>,
        /// Hosted readers (Firecrawl, Jina) read the pages fresh instead of from their cache
        #[arg(long)]
        fresh: bool,
    },

    /// The engines' own research agents (Tavily, You.com, Exa, Parallel) as background tasks. Costly: without --approved it only prints the cost estimate
    #[command(long_about = RESEARCH_HELP)]
    Research {
        /// The research question, written out in full
        question: Vec<String>,
        /// Engines (comma-separated): tavily, youcom, exa, parallel (default: all with a key)
        #[arg(long, value_delimiter = ',')]
        engines: Vec<String>,
        /// light, standard or deep (more depth costs more)
        #[arg(long, value_enum, default_value = "standard")]
        depth: crate::research_tasks::Depth,
        /// The user approved this spend: actually start the tasks
        #[arg(long)]
        approved: bool,
        /// Add the tasks to this run (same folder) instead of a new one
        #[arg(long)]
        parent: Option<String>,
        /// Pick up finished reports of a run's tasks (`last` for the last run)
        #[arg(long)]
        collect: Option<String>,
        /// Wait for the reports (up to 15 minutes) instead of returning at once
        #[arg(long)]
        wait: bool,
    },

    /// AI synthesis of a run: a cheap large-context model writes a cited answer from its pages, every quote checked
    Synth {
        /// The run (default: the last one)
        run: Option<String>,
        /// Wait for a synthesis already running in the background and print it
        #[arg(long)]
        wait: bool,
        /// Read every round of the run (default: the latest round, with earlier rounds as their own answers)
        #[arg(long)]
        all: bool,
    },

    /// Wait for a run's background work (late pages, a background synthesis), then print what landed. Start it as a background command: its exit is the notice
    Wait {
        /// The run (default: the newest)
        run: Option<String>,
        /// Wait for the run's Jina DeepSearch instead (deep runs; up to `jinadeepsearch.wait` seconds)
        #[arg(long)]
        jina_deepsearch: bool,
    },

    /// The web UI: search, past runs, how osearch works, settings (this machine only)
    #[cfg(feature = "ui")]
    Ui {
        /// Port (default `ui.port`, 7820)
        #[arg(long)]
        port: Option<u16>,
        /// Do not open a browser tab
        #[arg(long)]
        no_open: bool,
    },

    /// Rank a past run's saved results again with the current ranking (Jina's reranker blended in), beside the ranking it had; no engine calls
    Replay {
        /// The run (default: the newest)
        run: Option<String>,
        /// Rows shown
        #[arg(long, default_value = "25")]
        top: usize,
    },

    /// Internal: the background process of a run's Jina DeepSearch
    #[command(hide = true)]
    JinaDeepsearch {
        #[arg(long)]
        job: String,
    },

    /// Check email addresses over SMTP (no key; probes mail servers, use sparingly)
    Verify {
        emails: Vec<String>,
    },

    /// Crawl a site (usually docs) into a run: llms.txt first, then the sitemap
    Crawl {
        url: String,
        /// Only pages about this (matched on URLs and link text)
        #[arg(long)]
        topic: Option<String>,
        #[arg(long, default_value = "50")]
        max_pages: usize,
        /// Path globs to keep, e.g. /docs/*
        #[arg(long, value_delimiter = ',')]
        include: Vec<String>,
        /// Path globs to drop
        #[arg(long, value_delimiter = ',')]
        exclude: Vec<String>,
        /// Continue a crawl in its run, or add another part of the site to it
        #[arg(long, alias = "parent")]
        resume: Option<String>,
        /// Leave the seed's folder (whole site)
        #[arg(long)]
        any_path: bool,
    },

    /// Read a page of a run: its outline, one section (p3#4), or all of it
    Read {
        /// p3, p3#4, or <run>/p3#4 (the last run by default)
        page: String,
        #[arg(long)]
        run: Option<String>,
        /// The whole page
        #[arg(long)]
        full: bool,
        /// Only the lines with these words, labelled by section
        #[arg(long)]
        find: Option<String>,
    },

    /// Search the pages of a run (regex, case-insensitive)
    Grep {
        pattern: String,
        #[arg(long)]
        run: Option<String>,
        /// Accepted for habit: grep is always case-insensitive
        #[arg(short = 'i', long = "ignore-case")]
        _ignore_case: bool,
        #[arg(long, default_value = "60")]
        max: usize,
    },

    /// Past runs; --find checks whether a topic was researched before
    Runs {
        #[arg(long)]
        find: Option<String>,
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Compress the raw engine answers and results index of runs older than `research.compressafter` days
        #[arg(long)]
        compress: bool,
    },

    /// Log in to a site in a browser window; its cookies are used at every tier
    Login(LoginArgs),

    /// Spend and use from the logs, against the free tiers
    Stats {
        #[arg(long, default_value = "30")]
        days: u64,
        /// Delete log files older than this many days, then exit
        #[arg(long)]
        prune: Option<u64>,
    },

    /// Remaining credits where the provider reports them
    Usage,

    /// Keys, outside tools, browser, archive; --probe also test-calls every engine (billed)
    Doctor {
        #[arg(long)]
        probe: bool,
    },

    /// Update the outside tools (donsetch, Obscura) and show the others
    Update {
        /// Only show versions and what would change
        #[arg(long)]
        check: bool,
    },

    /// Engines and fetchers, with their modes and whether each is configured
    Providers,

    /// Configuration: show, set <section.key> <value>, path
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Clear the 5-minute search cache
    CacheClear,
}

/// `osearch help run` / `osearch run --help`: printed with the defaults of the
/// loaded config, so it cannot drift from what a run does (see `main`).
pub fn request_help() -> String {
    let r = &crate::config::get().research;
    format!(
        "A request file (JSON), written with your editor or an agent's Write tool. The smallest one that works:\n\n\
{{\"topic\": \"what this is about\", \"queries\": [{{\"q\": \"a different angle\", \"mode\": \"community\"}}, \"a plain question\"]}}\n\n\
Leave every other field out. The defaults below are tuned and kept in the config; the summary says when a request overrode one.\n\n\
queries: each is plain text or an object with:\n\
\x20\x20q            the query (short: a plain question or a few keywords)\n\
\x20\x20mode         general (the default) | news | academic | code | community | forums | social | deep | people | scholar | shopping\n\
\x20\x20since        day | week | month | year\n\
\x20\x20kind         leave out: every web engine, the widest search. semantic or keywords narrow the query to some engines; exact quotes the phrase\n\
\x20\x20sites, exclude   domains to keep or drop\n\
\x20\x20region       us | in ...        lang   en ...\n\
\x20\x20count        results per engine (default: each engine's own)\n\
\x20\x20engines      pin engines for this query (skips osearch's side searches for it)\n\n\
Other fields, all optional:\n\
\x20\x20urls              pages fetched as they are\n\
\x20\x20fetch             pages to read. Default {fetch} for a request file ({quick} for a quick search, {community} when every query is community, {deep} when every query is deep), {grow} more for each question past the third, up to {max}. Leave it out, or set it only to ask for MORE: {floor}. 0 = search only\n\
\x20\x20snowball          links several pages share, fetched (default {snowball})\n\
\x20\x20platforms         social sources to run: youtube, xbird, bluesky; paid: threads, tiktok, instagram, youtubesc, facebook\n\
\x20\x20cost_cap          USD estimate, searches past it are skipped (default ${cap:.2}); leave it out\n\
\x20\x20width             standard or wide (default {width}); leave it out\n\
\x20\x20fetch_time        seconds the fetching may take (default {fetchtime}); slower pages finish in the background\n\
\x20\x20via               fetch every page with one fetcher only (slower, skips osearch's own order: rarely right)\n\
\x20\x20no_paid_readers, no_cache, fresh   keep paid readers out, skip the 5-minute search cache, read pages fresh\n\
\x20\x20reddit_login      community: the logged-in Reddit search, deeper (default: {redditlogin})\n\
\x20\x20subreddits        subreddits community queries search inside, beside the ones osearch finds\n\
\x20\x20code_search       GitHub code search for the queries' words\n\
\x20\x20jina_deepsearch   true, false, or the question to ask Jina DeepSearch (deep runs: on)\n\
\x20\x20synth             true or false: AI synthesis (default: the synth settings)\n\
\x20\x20parent            a run id to continue (same folder)\n\
\x20\x20region            the region for every query that names none\n\n\
A misspelt field is ignored and the summary warns about it.\n\
Make each query a genuinely different angle; rephrasing the same words adds little.",
        fetch = r.fetch,
        quick = r.quickfetch,
        community = r.communityfetch,
        deep = r.deepfetch,
        grow = r.fetchgrow,
        max = r.fetchmax,
        floor = if r.fetchfloor {
            "a number below the default is raised to it (research.fetchfloor)"
        } else {
            "a number below the default is used as given (research.fetchfloor is off)"
        },
        snowball = r.snowball,
        cap = r.costcap,
        width = r.width.as_str(),
        fetchtime = r.fetchtime,
        redditlogin = r.redditlogin,
    )
}

#[cfg(test)]
mod tests {
    use super::request_help;
    use crate::research::{QuerySpec, Request};

    /// Every field a request file accepts is described in the help.
    #[test]
    fn help_lists_every_request_field() {
        let help = request_help();
        let names = |v: serde_json::Value| -> Vec<String> {
            v.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default()
        };
        for f in names(serde_json::to_value(Request::default()).unwrap())
            .into_iter()
            .chain(names(serde_json::to_value(QuerySpec::default()).unwrap()))
        {
            assert!(help.contains(&f), "osearch help run does not mention `{f}`");
        }
    }
}

pub const RESEARCH_HELP: &str = "The engines' own research agents: Tavily Research, You.com Research, Exa's agent and\n\
Parallel's Task API (processor core, pro or ultra by depth).\n\
Each plans its own searches, reads pages and writes a cited report. They cost far more than a\n\
search (cents to about $2 per question), so:\n\n\
  osearch research \"question\"              prints each engine's cost estimate; calls nothing\n\
  osearch research \"question\" --approved   starts the tasks (only after the user agreed to the estimate)\n\
  osearch research --collect <run>         picks up finished reports (or add --wait)\n\n\
Agents: run it without --approved first, show the user the estimate, and ask. Add --approved\n\
only once they say yes. Task ids are saved in the run folder (research-tasks.json), each report\n\
becomes a page with its sources, and the real cost goes into the ledger (`osearch stats`).\n\
Reports are synthesized by the engine: leads to verify, not findings.";

#[derive(Args, Clone)]
pub struct SearchArgs {
    /// The query
    #[arg(short, long)]
    pub query: String,
    #[command(flatten)]
    pub o: QueryOpts,
}

/// Options of a search, shared by `search` and the quick form.
#[derive(Args, Clone, Default)]
pub struct QueryOpts {
    /// Mode (general by default; only a bare URL is decided for you): general, news, academic, code, community, forums, social, deep, people, scholar, patents, images, places, shopping
    #[arg(short, long)]
    pub mode: Option<String>,
    /// standard (about six engines) or wide (every web engine with balance to spare; the default for now)
    #[arg(long, value_enum)]
    pub width: Option<crate::budget::Width>,
    /// Engines to ask (comma-separated)
    #[arg(short = 'p', long, value_delimiter = ',')]
    pub providers: Vec<String>,
    /// semantic, keywords or exact
    #[arg(long)]
    pub kind: Option<String>,
    /// Results per engine (default: each engine's own, 10-20)
    #[arg(short, long)]
    pub count: Option<usize>,
    /// day, week, month, year
    #[arg(short, long)]
    pub freshness: Option<String>,
    /// Only these sites (comma-separated)
    #[arg(short, long, value_delimiter = ',')]
    pub domain: Vec<String>,
    /// Never these sites
    #[arg(long, value_delimiter = ',')]
    pub exclude_domain: Vec<String>,
    /// Region (us default; in for India shopping/pricing)
    #[arg(long)]
    pub region: Option<String>,
    #[arg(long)]
    pub lang: Option<String>,
    /// Top results to fetch (0 = search only)
    #[arg(long)]
    pub fetch: Option<usize>,
    /// Seconds the fetching may take (default `research.quicktime`, 30)
    #[arg(long)]
    pub fetch_time: Option<u64>,
    /// Snowball candidates to fetch
    #[arg(long)]
    pub snowball: Option<usize>,
    /// Continue this run (same folder, next round)
    #[arg(long)]
    pub parent: Option<String>,
    /// Social sources to run (comma-separated): youtube, xbird, bluesky, and the paid ScrapeCreators ones: threads, tiktok, instagram, youtubesc, facebook
    #[arg(long, value_delimiter = ',')]
    pub platforms: Vec<String>,
    /// Keep the paid hosted readers out of the fetch tiers
    #[arg(long)]
    pub no_paid: bool,
    /// Estimated spend allowed, USD
    #[arg(long)]
    pub cost_cap: Option<f64>,
    /// Skip the 5-minute search cache
    #[arg(long)]
    pub no_cache: bool,
    /// Community queries also run the logged-in Reddit search (second page and the year's top too)
    #[arg(long)]
    pub reddit_login: bool,
    /// Subreddits a community query searches inside, beside the ones osearch finds (comma-separated: GrapheneOS,LineageOS)
    #[arg(long, value_delimiter = ',')]
    pub subreddits: Vec<String>,
    /// Hosted readers (Firecrawl, Jina) read pages fresh instead of from their cache (on with -f)
    #[arg(long)]
    pub fresh: bool,
    /// Add GitHub code search: repos whose files use the query's words
    #[arg(long)]
    pub code_search: bool,
    /// Write an AI synthesis of the run (`synth` settings decide when omitted)
    #[arg(long, conflicts_with = "no_synth")]
    pub synth: bool,
    /// No AI synthesis for this run
    #[arg(long)]
    pub no_synth: bool,
    /// Start Jina DeepSearch beside this run (deep runs start it by default)
    #[arg(long, conflicts_with = "no_jina_deepsearch")]
    pub jina_deepsearch: bool,
    /// No Jina DeepSearch for this run
    #[arg(long)]
    pub no_jina_deepsearch: bool,
}

#[derive(Args, Clone)]
pub struct LoginArgs {
    /// The site, e.g. reddit.com
    pub site: Option<String>,
    /// List the sites logged into
    #[arg(long)]
    pub list: bool,
    /// Forget a site and its cookies
    #[arg(long)]
    pub forget: Option<String>,
    /// Import logins from the crwl browser profile: give sites, or none to list its sites
    #[arg(long, num_args = 0.., value_delimiter = ',')]
    pub import_crwl: Option<Vec<String>>,
    /// Import a Netscape cookies.txt (from a browser extension): every site in it, or only the site named
    #[arg(long)]
    pub import_file: Option<String>,
    /// A named login beside the site's main one (`--as alt`), for logging in,
    /// importing or forgetting
    #[arg(long = "as", value_name = "NAME")]
    pub account: Option<String>,
}

#[derive(Subcommand)]
pub enum ConfigAction {
    Show,
    Set { key: String, value: String },
    Path,
}
