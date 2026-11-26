//! Every setting the web UI shows: its group, what it does, and how it is
//! edited. The one home for user-facing setting descriptions; a test checks
//! it names exactly the fields the config has.

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Number,
    Bool,
    Text,
    List,
    Choice,
}

#[derive(Debug, Clone, Serialize)]
pub struct Setting {
    pub key: &'static str,
    /// What the page shows as its title.
    pub name: &'static str,
    pub group: &'static str,
    pub kind: Kind,
    pub options: &'static [&'static str],
    pub about: &'static str,
}

const fn s(key: &'static str, name: &'static str, group: &'static str, kind: Kind, about: &'static str) -> Setting {
    Setting { key, name, group, kind, options: &[], about }
}

const fn c(key: &'static str, name: &'static str, group: &'static str, options: &'static [&'static str], about: &'static str) -> Setting {
    Setting { key, name, group, kind: Kind::Choice, options, about }
}

use Kind::{Bool, List, Number, Text};

pub const SETTINGS: &[Setting] = &[
    // Pages read per run
    s("research.fetch", "Pages read, request files", "Pages", Number, "Pages read per run from request files (up to three questions)."),
    s("research.quickfetch", "Pages read, quick search", "Pages", Number, "Pages read per quick search (`osearch \"query\"`)."),
    s("research.communityfetch", "Pages read, community questions", "Pages", Number, "Pages read when every question is in community mode."),
    s("research.deepfetch", "Pages read, deep questions", "Pages", Number, "Pages read when every question is in deep mode."),
    s("research.fetchgrow", "Pages added per extra question", "Pages", Number, "Pages added for each question past the third."),
    s("research.fetchmax", "Most pages from that growth", "Pages", Number, "The most pages that growth reaches; a run that asks for more is not cut."),
    s("research.fetchfloor", "Never read fewer than the default", "Pages", Bool, "A request asking for fewer pages than the default gets the default (0 stays search-only)."),
    s("research.snowball", "Shared links read", "Pages", Number, "Links several read pages share, read as well (0 lists them only)."),
    s("research.telegramslot", "A page for Telegram searches", "Pages", Bool, "Telegram side searches get a guaranteed page; off, their posts compete by rank."),
    s("research.concurrency", "Pages fetched at once", "Pages", Number, "Pages fetched at the same time."),
    s("research.forumboth", "Forum threads read two ways", "Pages", Bool, "On forum threads, donsetch and the built-in fetchers run together and the longer copy is kept."),
    s("research.dedupe", "Leave near-copies out of synthesis", "Pages", Bool, "Leave a page out of synthesis when most of it repeats another page of the run."),
    s("research.dedupemin", "Near-copy threshold (%)", "Pages", Number, "How much of a page (percent) must repeat another for `dedupe` to count it."),
    s("research.compressafter", "Compress runs older than (days)", "Pages", Number, "`osearch runs --compress` packs the raw answers and results index of runs older than this many days; 0 is off."),
    // Timing
    s("research.fetchtime", "Fetch time limit (s)", "Timing", Number, "Seconds the fetch step may take at most; slower pages are stopped."),
    s("research.quicktime", "Fetch time limit, quick search (s)", "Timing", Number, "The same for a quick search."),
    s("research.softtime", "Wait for pages (s)", "Timing", Number, "Seconds a run waits for its pages before the rest finish in the background."),
    s("research.quicksoft", "Wait for pages, quick search (s)", "Timing", Number, "The same for a quick search."),
    s("research.costcap", "Spend limit per run (USD)", "Engines", Number, "Estimated spend allowed per run in USD; searches past it are skipped with a warning."),
    // Engines
    c("research.width", "Search width", "Engines", &["standard", "wide"], "Standard asks about six engines; wide asks every web engine with balance to spare."),
    s("research.standard", "Standard engine set", "Engines", List, "The standard set, in order of preference; empty uses the built-in order."),
    s("research.engines", "Engines for general questions", "Engines", List, "Engines for general questions that name none; empty picks by width and balance."),
    s("research.rerank", "Rerank with Jina", "Engines", Bool, "Blend Jina's reranker into the ranking before pages are picked."),
    s("settings.count", "Results per engine", "Engines", Number, "Results per engine when a search sets none; 0 uses each engine's own count."),
    // Community
    c("research.redditlogin", "Logged-in Reddit search", "Community", &["always", "auto", "never"], "Logged-in Reddit search on community questions; auto runs it when few threads turned up."),
    s("research.redditmin", "Few threads means below", "Community", Number, "With redditlogin auto: run the logged-in search below this many threads."),
    s("research.redditaccount", "Reddit account for runs", "Community", Text, "The saved Reddit account a run's own Reddit use goes through; empty is the main login."),
    s("research.redditlivemax", "Live thread reads per round", "Community", Number, "Live reads of picked Reddit threads per round."),
    s("research.redditmainmax", "Main login calls per round", "Community", Number, "Calls the main Reddit login may make per round when the run account fails, or its share of side-by-side searches."),
    s("research.redditpace", "Reddit requests a minute", "Community", Number, "Logged-in Reddit requests a minute, per account, shared by every osearch process."),
    s("research.redditsidebyside", "Both Reddit logins side by side", "Community", Bool, "Logged-in Reddit searches go through the run account and the main login at the same time, each at its own pace (more requests from your address)."),
    s("research.redditcomments", "Reddit comment search", "Community", Bool, "Logged-in comment search beside the post search: threads whose answer is in a reply."),
    s("research.redditsubs", "Subreddits searched per question", "Community", Number, "Subreddits searched inside per community question."),
    s("research.redditrefused", "Skip refused Reddit routes (h)", "Community", Number, "Hours every run skips a Reddit route that refused osearch (HTTP 403); 0 tries it on every run."),
    s("research.redditfill", "Thread dates from the archive", "Community", Bool, "Dates of found Reddit threads that came without one, filled in from the archive."),
    c("research.githubbeside", "GitHub list beside questions", "Engines", &["all", "general", "off"], "The GitHub list beside questions: all of them, general and deep questions only, or none."),
    s("research.redditlive", "Reddit pages without a login", "Community", Bool, "Reddit's own pages without a login (Reddit usually refuses them)."),
    s("research.redditarchive", "Archive search inside subreddits", "Community", Bool, "The Reddit archive's title search inside subreddits (it times out on its server)."),
    s("research.xbird", "X search with your login", "Community", Bool, "X search in social mode through the saved x.com login (runs as the real account)."),
    s("research.githubminstars", "Fewest stars in the GitHub list", "Community", Number, "Repos with fewer stars stay out of the GitHub list (0 is no floor)."),
    // Summary
    s("research.snippetchars", "Snippet length (characters)", "Summary", Number, "Characters of each snippet in the summary's top results."),
    s("research.topresults", "Top results listed", "Summary", Number, "Results listed in the summary's top results."),
    s("research.snippetprefer", "Snippets from these engines first", "Summary", List, "Whose snippet is shown first in the summary and listed first for synthesis."),
    // Synthesis
    c("synth.enabled", "Synthesis", "Synthesis", &["auto", "on", "off"], "When a run gets an AI synthesis; auto lets the next three decide."),
    s("synth.quick", "Quick searches get one", "Synthesis", Bool, "With enabled auto: quick searches get one."),
    s("synth.requests", "Request files get one", "Synthesis", Bool, "With enabled auto: request files get one."),
    s("synth.deep", "Deep runs get one", "Synthesis", Bool, "With enabled auto: deep runs get one."),
    c("synth.wait", "Wait or background", "Synthesis", &["wait", "background"], "Wait prints summary and synthesis together; background prints the summary first."),
    c("synth.api", "API style", "Synthesis", &["anthropic", "openai"], "The endpoint's API style."),
    s("synth.url", "Endpoint", "Synthesis", Text, "The endpoint; synthesis is off until this and the model are set."),
    s("synth.model", "Model", "Synthesis", Text, "The model the endpoint should use."),
    s("synth.key", "API key", "Synthesis", Text, "The endpoint's API key, when it needs one."),
    s("synth.effort", "Reasoning effort", "Synthesis", Text, "Reasoning effort, when the model takes one; empty sends none."),
    s("synth.maxtokens", "Longest answer (tokens)", "Synthesis", Number, "Longest answer, in tokens."),
    s("synth.timeout", "Time limit (s)", "Synthesis", Number, "Seconds to wait for the answer."),
    s("synth.maxinput", "Most input (tokens)", "Synthesis", Number, "Tokens of material at most; past it, lower pages go as their best sections."),
    s("synth.pagecap", "Most per page (tokens)", "Synthesis", Number, "Tokens per page at most; a longer page goes as its opening and best sections."),
    c("synth.pages", "Whole pages or sections", "Synthesis", &["full", "sections"], "Whole pages, or each page's best sections."),
    s("synth.answers", "Include engine answers", "Synthesis", Bool, "The engines' own written answers go in too, marked unverified."),
    s("synth.snippets", "Include unread results' snippets", "Synthesis", Bool, "Snippets of results that were not read go in too."),
    c("synth.show", "Part printed for the agent", "Synthesis", &["full", "short", "both"], "Which part of the answer is printed for the agent."),
    s("synth.shorttag", "Short answer tag", "Synthesis", Text, "The tag around the short answer."),
    s("synth.fulltag", "Full answer tag", "Synthesis", Text, "The tag around the full answer."),
    s("synth.prompt", "Prompt file", "Synthesis", Text, "A prompt file read at every run; empty uses the built-in prompt."),
    s("synth.inprice", "Input price (USD per million)", "Synthesis", Number, "USD per million input tokens, for the cost log; 0 uses the price list file."),
    s("synth.outprice", "Output price (USD per million)", "Synthesis", Number, "USD per million output tokens, for the cost log."),
    s("synth.prices", "Price list file", "Synthesis", Text, "A price list file in OpenRouter's catalogue shape; empty is none."),
    // Jina DeepSearch
    s("jinadeepsearch.modes", "Starts on its own for", "Jina DeepSearch", List, "Run types that start it on their own: deep, research."),
    s("jinadeepsearch.budget", "Token budget per job", "Jina DeepSearch", Number, "Token budget per job."),
    c("jinadeepsearch.effort", "Reasoning effort", "Jina DeepSearch", &["low", "medium", "high"], "Reasoning effort."),
    s("jinadeepsearch.wait", "Wait for it (s)", "Jina DeepSearch", Number, "Seconds `osearch wait --jina-deepsearch` waits; the job keeps going."),
    s("jinadeepsearch.stop", "Give up after (s)", "Jina DeepSearch", Number, "Seconds after which a hung job is given up."),
    // Paths
    s("paths.archive", "Run archive folder", "Paths", Text, "Where runs are kept: <archive>/runs/<id>/."),
    s("paths.envfile", "Keys file", "Paths", Text, "A KEY=value file read at start for API keys; variables already set win."),
    // Outside tools
    s("tools.donsetch", "donsetch", "Outside tools", Text, "donsetch's path; empty uses osearch's tools folder, then PATH."),
    s("tools.moli", "moli", "Outside tools", Text, "moli's path; empty uses osearch's tools folder, then PATH."),
    s("tools.obscura", "Obscura", "Outside tools", Text, "Obscura's path; empty uses osearch's tools folder, then PATH."),
    c("tools.lightbrowser", "Light browser", "Outside tools", &["moli", "obscura", "off"], "The light browser tried before Chromium."),
    s("tools.chromium", "Chromium", "Outside tools", Text, "Chromium for the browser tier and logins; empty uses Playwright's newest."),
    s("tools.crwl", "crwl", "Outside tools", Text, "crwl's (crawl4ai) path; empty uses PATH."),
    s("tools.crwlprofile", "crwl browser profile", "Outside tools", Text, "The crwl browser profile used on hard walls."),
    s("tools.ytdlp", "yt-dlp", "Outside tools", Text, "yt-dlp's path; empty uses PATH."),
    s("tools.gh", "GitHub CLI", "Outside tools", Text, "GitHub CLI's path; empty uses PATH."),
    // Web UI
    s("ui.port", "Port", "Web UI", Number, "The port `osearch ui` listens on (this machine only)."),
    s("ui.notes", "Notes server", "Web UI", Text, "A notes server endpoint for the review-notes widget; empty turns notes off."),
];

#[cfg(test)]
mod tests {
    use super::SETTINGS;
    use std::collections::BTreeSet;

    fn config_keys() -> BTreeSet<String> {
        let v = serde_json::to_value(crate::config::AppConfig::default()).expect("config");
        let mut keys = BTreeSet::new();
        for section in ["research", "synth", "jinadeepsearch", "paths", "tools", "settings", "ui"] {
            for k in v[section].as_object().expect(section).keys() {
                keys.insert(format!("{section}.{k}"));
            }
        }
        keys
    }

    #[test]
    fn the_catalogue_names_exactly_the_config_fields() {
        let catalogue: BTreeSet<String> = SETTINGS.iter().map(|s| s.key.to_string()).collect();
        let config = config_keys();
        let missing: Vec<_> = config.difference(&catalogue).collect();
        let stale: Vec<_> = catalogue.difference(&config).collect();
        assert!(missing.is_empty() && stale.is_empty(), "not in the UI catalogue: {missing:?}; no longer in the config: {stale:?}");
    }

    #[test]
    fn every_setting_is_in_a_flow_and_every_flow_setting_exists() {
        let flows: serde_json::Value = serde_json::from_str(include_str!("../../ui/src/flows/flows.json")).expect("flows.json");
        let mut used = BTreeSet::new();
        for flow in flows.as_array().expect("flows") {
            for node in flow["nodes"].as_array().expect("nodes") {
                for k in node["settings"].as_array().into_iter().flatten() {
                    used.insert(k.as_str().expect("setting").to_string());
                }
            }
        }
        let catalogue: BTreeSet<String> = SETTINGS.iter().map(|s| s.key.to_string()).collect();
        let unused: Vec<_> = catalogue.difference(&used).collect();
        let unknown: Vec<_> = used.difference(&catalogue).collect();
        assert!(unused.is_empty() && unknown.is_empty(), "settings in no flow box: {unused:?}; flow boxes naming unknown settings: {unknown:?}");
    }
}
