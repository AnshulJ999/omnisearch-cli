//! Community mode beyond Reddit and Hacker News: Google's forum filter
//! (SerpApi google_forums, then Serper udm=18), a meaning-based angle on
//! Parallel, GitHub Discussions, Lemmy, and forums searched directly (their
//! own Discourse search, else `site:` on Brave). Forums seen in runs are
//! remembered (their software, how often seen); only the built-in forums
//! carry topic words, so a question matches those and never a learned forum.

use super::{carrying_words, local_response, plan_side, results_of, Responses, RoundOut, SAME};
use crate::context::AppContext;
use crate::research::{search_cached, Planned, QuerySpec, Request};
use crate::run::raw;
use crate::types::{Mode, SearchOpts, SearchResult};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// Well-known forums by topic, searched directly when a question's words
/// match. Forums met in runs join them in `forums.json` without words.
const SEED: &[(&str, &str)] = &[
    ("forums.servethehome.com", "homelab server servers nas ups networking hardware rack"),
    ("forum.level1techs.com", "homelab linux pc hardware virtualization gpu"),
    ("forums.truenas.com", "nas truenas zfs storage homelab drives"),
    ("forum.proxmox.com", "proxmox virtualization vm lxc homelab hypervisor"),
    ("forums.unraid.net", "unraid nas docker homelab storage"),
    ("forums.lawrencesystems.com", "networking pfsense homelab firewall"),
    ("community.spiceworks.com", "sysadmin windows networking it server"),
    ("community.home-assistant.io", "home assistant smart automation zigbee esphome"),
    ("forum.openwrt.org", "router openwrt networking wifi"),
    ("forum.opnsense.org", "opnsense firewall router networking"),
    ("discourse.nixos.org", "nixos nix linux"),
    ("forum.syncthing.net", "syncthing sync backup"),
    ("forum.rclone.org", "rclone backup cloud storage"),
    ("forum.restic.net", "restic backup"),
    ("kopia.discourse.group", "kopia backup snapshot"),
    ("forum.duplicati.com", "duplicati backup"),
    ("bbs.archlinux.org", "arch linux"),
    ("forum.manjaro.org", "manjaro linux"),
    ("forum.endeavouros.com", "endeavouros arch linux"),
    ("discourse.ubuntu.com", "ubuntu linux"),
    ("forums.linuxmint.com", "mint linux"),
    ("discuss.python.org", "python"),
    ("users.rust-lang.org", "rust cargo crate"),
    ("discuss.huggingface.co", "huggingface model models ml llm transformers"),
    ("community.openai.com", "openai gpt api llm chatgpt"),
    ("forum.cursor.com", "cursor ide coding agent"),
    ("community.cloudflare.com", "cloudflare dns workers tunnel"),
    ("discuss.pytorch.org", "pytorch ml training"),
    ("community.fly.io", "fly deploy hosting"),
    ("forum.obsidian.md", "obsidian notes markdown"),
    ("forum.godotengine.org", "godot game engine"),
    ("xdaforums.com", "android rom root phone xda"),
    ("gearspace.com", "audio recording mixing studio daw microphone"),
    ("kvraudio.com", "audio plugin plugins vst synth daw"),
    ("forum.cockos.com", "reaper daw audio"),
    ("audiosciencereview.com", "audio headphones dac speakers amplifier"),
    ("head-fi.org", "headphones iem audio dac"),
    ("linustechtips.com", "pc build gpu cpu hardware monitor"),
    ("overclock.net", "pc overclocking cpu gpu hardware cooling"),
    ("techpowerup.com", "gpu cpu pc hardware"),
    ("forums.tomshardware.com", "pc hardware build"),
    ("avsforum.com", "tv home theater projector av receiver"),
    ("dpreview.com", "camera photography lens"),
    ("techenclave.com", "india tech pc hardware homelab"),
    ("team-bhp.com", "car cars india driving"),
    ("bogleheads.org", "investing finance retirement index funds"),
    ("flyertalk.com", "travel airline miles hotel points"),
    ("geekhack.org", "keyboard keyboards mechanical"),
    ("forum.prusa3d.com", "3d printing printer filament"),
    ("eevblog.com", "electronics circuit oscilloscope"),
];

/// Hosts that have their own route or are not forums.
const NOT_FORUMS: &[&str] = &[
    "reddit.com", "news.ycombinator.com", "quora.com", "youtube.com", "facebook.com", "x.com",
    "twitter.com", "github.com", "stackoverflow.com", "stackexchange.com", "medium.com", "linkedin.com",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Forum {
    host: String,
    /// "discourse", "other", or "" (not checked yet).
    #[serde(default)]
    software: String,
    /// The built-in topic words (seed forums only; learned forums have none).
    #[serde(default)]
    words: Vec<String>,
    #[serde(default)]
    seen: u64,
    #[serde(default)]
    last: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Known {
    #[serde(default)]
    forums: Vec<Forum>,
}

fn known_path() -> std::path::PathBuf {
    crate::config::data_dir().join("forums.json")
}

/// The remembered forums, with the seed list folded in. A seed forum always
/// carries the built-in topic words; any other forum carries none (older
/// files held words learned from questions, which made every forum match
/// ordinary words).
fn load() -> Known {
    let mut k: Known = std::fs::read_to_string(known_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    for f in k.forums.iter_mut() {
        f.words = SEED
            .iter()
            .find(|(host, _)| *host == f.host)
            .map(|(_, topics)| topics.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default();
    }
    for (host, topics) in SEED {
        if !k.forums.iter().any(|f| f.host == *host) {
            k.forums.push(Forum {
                host: host.to_string(),
                words: topics.split_whitespace().map(str::to_string).collect(),
                ..Forum::default()
            });
        }
    }
    k
}

fn save(k: &Known) {
    if let Ok(text) = serde_json::to_string_pretty(k) {
        let path = known_path();
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }
}

fn host_of(url: &str) -> Option<String> {
    let h = url::Url::parse(url).ok()?.host_str()?.to_ascii_lowercase();
    Some(h.strip_prefix("www.").unwrap_or(&h).to_string())
}

fn not_forum(host: &str) -> bool {
    NOT_FORUMS.iter().any(|n| crate::cookies::under(host, n))
}

/// A thread address the common forum software uses.
pub(crate) fn thread_like(url: &str) -> bool {
    let l = url.to_ascii_lowercase();
    let discourse = regex_is(&l, r"/t/[^/]+/\d+");
    discourse
        || ["viewtopic", "showthread", "/threads/", "/topic/", "/topics/", "index.php?threads", "/forum/", "/forums/", "/discussion/"]
            .iter()
            .any(|p| l.contains(p))
}

fn regex_is(text: &str, pattern: &str) -> bool {
    regex::Regex::new(pattern).is_ok_and(|r| r.is_match(text))
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// The side queries of a community question, run with the request's own:
/// Google's forum filter and the meaning-based angle.
pub fn side(
    app: &Arc<AppContext>,
    req: &Request,
    statuses: &[crate::budget::Status],
    p: &Planned,
) -> Vec<Planned> {
    let mut out = Vec::new();
    let can = |mode: Mode, engine: &str| {
        let configured = crate::providers::build_providers(app)
            .into_iter()
            .any(|a| a.name() == engine && a.is_configured() && a.spec().modes.contains(&mode));
        configured && !crate::budget::pick(mode, crate::budget::Width::Wide, &[engine], statuses).engines.is_empty()
    };
    // SerpApi refills monthly; Serper's credits are one-time, so it only
    // runs when SerpApi cannot.
    let forums = ["serpapi", "serper"].into_iter().find(|e| can(Mode::Forums, e));
    let spec = match forums {
        Some(engine) => QuerySpec {
            q: p.query.clone(),
            mode: Some("forums".into()),
            engines: vec![engine.into()],
            count: Some(10),
            since: p.opts.freshness.clone(),
            ..QuerySpec::default()
        },
        None if can(Mode::General, "brave") => QuerySpec {
            q: format!(
                "{} (forum OR discussion OR thread) -site:reddit.com -site:quora.com -site:youtube.com",
                p.query
            ),
            mode: Some("general".into()),
            engines: vec!["brave".into()],
            count: Some(20),
            since: p.opts.freshness.clone(),
            ..QuerySpec::default()
        },
        None => QuerySpec::default(),
    };
    if !spec.q.is_empty() {
        out.extend(plan_side(app, req, statuses, p, "f", spec, SAME));
    }
    if can(Mode::General, "parallel") {
        let spec = QuerySpec {
            q: format!(
                "Forum and community discussion threads where people share their real experience with: {}",
                p.query
            ),
            mode: Some("general".into()),
            engines: vec!["parallel".into()],
            count: Some(15),
            since: p.opts.freshness.clone(),
            ..QuerySpec::default()
        };
        out.extend(plan_side(app, req, statuses, p, "p", spec, SAME));
    }
    out
}

/// The date a freshness word reaches back to (YYYY-MM-DD).
fn since_date(freshness: Option<&str>) -> Option<String> {
    freshness.and_then(crate::providers::freshness_days).map(crate::providers::date_days_ago)
}

/// GitHub Discussions matching `words` (gh's GraphQL search).
async fn discussions(words: &str, since: Option<&str>) -> Result<Vec<SearchResult>, String> {
    let words = match since_date(since) {
        Some(d) => format!("{words} created:>={d}"),
        None => words.to_string(),
    };
    let query = "query($q:String!){search(query:$q,type:DISCUSSION,first:20){nodes{... on Discussion{title url bodyText upvoteCount updatedAt comments{totalCount} repository{nameWithOwner}}}}}";
    let args: Vec<String> = vec![
        "api".into(),
        "graphql".into(),
        "-f".into(),
        format!("query={query}"),
        "-f".into(),
        format!("q={words}"),
    ];
    let ran = crate::tools::run(crate::tools::Tool::Gh, &args, Duration::from_secs(25)).await?;
    if ran.code != Some(0) {
        return Err(ran.stderr.trim().to_string());
    }
    let v: serde_json::Value = serde_json::from_slice(&ran.stdout).map_err(|e| e.to_string())?;
    Ok(v.pointer("/data/search/nodes")
        .and_then(|n| n.as_array())
        .into_iter()
        .flatten()
        .filter_map(|n| {
            let url = n["url"].as_str()?.to_string();
            let repo = n["repository"]["nameWithOwner"].as_str().unwrap_or("");
            Some(SearchResult {
                title: format!("{repo} discussion: {}", n["title"].as_str().unwrap_or("")),
                url,
                snippet: format!(
                    "{} comments · {} upvotes · {}",
                    n["comments"]["totalCount"].as_u64().unwrap_or(0),
                    n["upvoteCount"].as_u64().unwrap_or(0),
                    crate::providers::thread::clip(n["bodyText"].as_str().unwrap_or(""), 240)
                ),
                source: "ghdiscussions".into(),
                published: n["updatedAt"].as_str().map(str::to_string),
                image_url: None,
                extra: None,
            })
        })
        .collect())
}

/// Lemmy posts across the instances lemmy.world federates with.
async fn lemmy(app: &AppContext, words: &str, since: Option<&str>) -> Result<Vec<SearchResult>, String> {
    let sort = match since {
        Some("day") => "TopDay",
        Some("week") => "TopWeek",
        Some("month") => "TopMonth",
        Some("year") => "TopYear",
        _ => "TopAll",
    };
    let resp = app
        .client
        .get("https://lemmy.world/api/v3/search")
        .query(&[("q", words), ("type_", "Posts"), ("sort", sort), ("listing_type", "All"), ("limit", "20")])
        .timeout(Duration::from_secs(12))
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| e.without_url().to_string())?;
    Ok(v["posts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let url = p["post"]["ap_id"].as_str()?.to_string();
            Some(SearchResult {
                title: p["post"]["name"].as_str().unwrap_or("").to_string(),
                url,
                snippet: format!(
                    "c/{} · {} points · {} comments",
                    p["community"]["name"].as_str().unwrap_or("?"),
                    p["counts"]["score"].as_i64().unwrap_or(0),
                    p["counts"]["comments"].as_u64().unwrap_or(0)
                ),
                source: "lemmy".into(),
                published: p["post"]["published"].as_str().map(str::to_string),
                image_url: None,
                extra: None,
            })
        })
        .collect())
}

/// A Discourse forum's own search: topics with their reply counts.
/// `Ok(None)` when the host does not answer like Discourse.
async fn discourse_search(app: &AppContext, host: &str, words: &str, since: Option<&str>) -> Result<Option<Vec<SearchResult>>, String> {
    let q = match since_date(since) {
        Some(d) => format!("{words} after:{d}"),
        None => words.to_string(),
    };
    let resp = app
        .client
        .get(format!("https://{host}/search.json"))
        .query(&[("q", q.as_str())])
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let Ok(v) = resp.json::<serde_json::Value>().await else {
        return Ok(None);
    };
    let Some(topics) = v["topics"].as_array().or_else(|| v.get("grouped_search_result").map(|_| &EMPTY)) else {
        return Ok(None);
    };
    let blurb = |id: u64| {
        v["posts"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p["topic_id"].as_u64() == Some(id))
            .and_then(|p| p["blurb"].as_str())
            .unwrap_or("")
            .to_string()
    };
    Ok(Some(
        topics
            .iter()
            .take(15)
            .filter_map(|t| {
                let id = t["id"].as_u64()?;
                let slug = t["slug"].as_str().unwrap_or("topic");
                Some(SearchResult {
                    title: t["title"].as_str().unwrap_or("").to_string(),
                    url: format!("https://{host}/t/{slug}/{id}"),
                    snippet: format!("{} posts · {}", t["posts_count"].as_u64().unwrap_or(0), crate::providers::thread::clip(&blurb(id), 220)),
                    source: "discourse".into(),
                    published: t["last_posted_at"].as_str().map(str::to_string),
                    image_url: None,
                    extra: None,
                })
            })
            .collect(),
    ))
}

static EMPTY: Vec<serde_json::Value> = Vec::new();

/// After the first searches of a community question: GitHub Discussions,
/// Lemmy, and the forums worth searching directly (remembered ones whose
/// words match, plus forums this run found).
pub async fn round(
    app: &Arc<AppContext>,
    recorder: &raw::Recorder,
    planned: &[Planned],
    responses: &Responses,
    no_cache: bool,
) -> RoundOut {
    let mut out = RoundOut::default();
    let parents: Vec<&Planned> = planned
        .iter()
        .filter(|p| p.mode == Mode::Community && !p.side)
        .collect();
    if parents.is_empty() {
        return out;
    }
    let mut known = load();
    // Every search of every community question runs at once; the answers
    // are put back in planning order.
    enum Job {
        Got(String, Planned, super::Answer),
        Note(String),
        Mark(String, &'static str),
    }
    type JobFut<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = Vec<Job>> + Send + 'a>>;
    let mut jobs: Vec<JobFut<'_>> = Vec::new();
    let gh = crate::tools::locate(crate::tools::Tool::Gh).is_some();
    for p in parents {
        let words = carrying_words(&p.query, 5);
        if words.is_empty() {
            continue;
        }
        let topic = p.query.clone();
        if gh {
            let (label, words, topic, p) = (format!("{}d", p.label), words.clone(), topic.clone(), p.clone());
            let rec = recorder.labeled(&label);
            jobs.push(Box::pin(async move {
                match raw::scope(Some(rec), discussions(&words, p.opts.freshness.as_deref())).await {
                    Ok(list) => {
                        let list = crate::relevance::keep(list, &topic);
                        let planned = Planned::local(&p, label.clone(), words.clone(), Mode::Community, "ghdiscussions", SAME);
                        vec![Job::Got(label, planned, Ok(local_response("ghdiscussions", &words, Mode::Community, list)))]
                    }
                    Err(e) => vec![Job::Note(format!("{label}: GitHub Discussions search failed: {e}"))],
                }
            }));
        }
        {
            let (label, words, topic, p) = (format!("{}m", p.label), words.clone(), topic.clone(), p.clone());
            let rec = recorder.labeled(&label);
            jobs.push(Box::pin(async move {
                match raw::scope(Some(rec), lemmy(app, &words, p.opts.freshness.as_deref())).await {
                    Ok(list) => {
                        let list = crate::relevance::keep(list, &topic);
                        let planned = Planned::local(&p, label.clone(), words.clone(), Mode::Community, "lemmy", SAME);
                        vec![Job::Got(label, planned, Ok(local_response("lemmy", &words, Mode::Community, list)))]
                    }
                    Err(e) => vec![Job::Note(format!("{label}: Lemmy search failed: {e}"))],
                }
            }));
        }

        // Forums to search directly: this run's forum hits, then remembered
        // forums sharing at least two of the question's words (one when it
        // has only one or two).
        let terms: Vec<String> = crate::outline::terms(&p.query);
        let need = if terms.len() <= 2 { 1 } else { 2 };
        let mut hosts: Vec<String> = Vec::new();
        for r in results_of(&p.label, responses) {
            if let Some(h) = host_of(&r.url).filter(|h| !not_forum(h) && thread_like(&r.url)) {
                if !hosts.contains(&h) {
                    hosts.push(h);
                }
            }
        }
        hosts.truncate(2);
        let mut scored: Vec<(usize, String)> = known
            .forums
            .iter()
            .map(|f| (f.words.iter().filter(|w| terms.contains(w)).count(), f.host.clone()))
            .filter(|(n, h)| *n >= need && !hosts.contains(h))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        hosts.extend(scored.into_iter().take(3).map(|(_, h)| h));
        // At most two `site:` searches on Brave per question.
        let brave_left = Arc::new(std::sync::atomic::AtomicUsize::new(2));
        for (i, host) in hosts.into_iter().enumerate() {
            let label = format!("{}k{}", p.label, i + 1);
            let software = known.forums.iter().find(|f| f.host == host).map(|f| f.software.clone()).unwrap_or_default();
            let (words, topic, p, brave_left) = (words.clone(), topic.clone(), p.clone(), brave_left.clone());
            let rec = recorder.labeled(&label);
            jobs.push(Box::pin(async move {
                let mut got = Vec::new();
                if software != "other" {
                    match raw::scope(Some(rec.clone()), discourse_search(app, &host, &words, p.opts.freshness.as_deref())).await {
                        Ok(Some(list)) => {
                            let list = crate::relevance::keep(list, &topic);
                            let q = format!("{host}: {words}");
                            got.push(Job::Mark(host.clone(), "discourse"));
                            let planned = Planned::local(&p, label.clone(), q.clone(), Mode::Community, "discourse", SAME);
                            got.push(Job::Got(label, planned, Ok(local_response("discourse", &q, Mode::Community, list))));
                            return got;
                        }
                        // Only a forum not checked yet is judged by one answer:
                        // a known Discourse forum may just be rate-limiting.
                        Ok(None) if software.is_empty() => got.push(Job::Mark(host.clone(), "other")),
                        Ok(None) | Err(_) if software == "discourse" => {
                            got.push(Job::Note(format!("{label}: {host}'s own search did not answer this time; searched with site: instead")));
                        }
                        Ok(None) | Err(_) => {}
                    }
                }
                let take = brave_left.try_update(std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst, |n| n.checked_sub(1));
                if take.is_err() {
                    return got;
                }
                let q = format!("site:{host} {words}");
                // Brave and Jina at once: a wider net on the forum.
                let engines = Some(vec!["brave".to_string(), "jina".to_string()]);
                let opts = SearchOpts { freshness: p.opts.freshness.clone(), ..SearchOpts::default() };
                let r = raw::scope(
                    Some(rec),
                    search_cached(app.clone(), &q, Mode::General, 10, &engines, &opts, no_cache),
                )
                .await;
                let planned = Planned::local(&p, label.clone(), q, Mode::General, "brave, jina", SAME);
                got.push(Job::Got(label, planned, r));
                got
            }));
        }
    }
    for job in futures_util::future::join_all(jobs).await.into_iter().flatten() {
        match job {
            Job::Got(label, planned, r) => {
                out.responses.push((label, r));
                out.planned.push(planned);
            }
            Job::Note(n) => out.notes.push(n),
            Job::Mark(host, software) => mark(&mut known, &host, software),
        }
    }
    save(&known);
    out
}

fn mark(k: &mut Known, host: &str, software: &str) {
    match k.forums.iter_mut().find(|f| f.host == host) {
        Some(f) => f.software = software.to_string(),
        None => k.forums.push(Forum {
            host: host.to_string(),
            software: software.to_string(),
            ..Forum::default()
        }),
    }
}

/// Remember the forums a community question's searches found (how often
/// seen, whether Discourse). No question words are kept: a forum is searched
/// directly when a run's own results include a thread from it, or when the
/// question matches a built-in forum's topic words.
pub fn learn(planned: &[Planned], responses: &Responses) {
    let mut known = load();
    let mut changed = false;
    for p in planned.iter().filter(|p| p.mode == Mode::Community && !p.side) {
        let mut hosts: Vec<String> = Vec::new();
        for r in results_of(&p.label, responses) {
            if let Some(h) = host_of(&r.url).filter(|h| !not_forum(h) && thread_like(&r.url)) {
                if !hosts.contains(&h) {
                    hosts.push(h);
                }
            }
        }
        for h in hosts {
            let f = match known.forums.iter_mut().position(|f| f.host == h) {
                Some(i) => &mut known.forums[i],
                None => {
                    known.forums.push(Forum { host: h.clone(), ..Forum::default() });
                    known.forums.last_mut().expect("just pushed")
                }
            };
            f.seen += 1;
            f.last = today();
            // Discourse thread addresses: worth checking its own search again.
            if f.software == "other" && results_of(&p.label, responses).iter().any(|r| host_of(&r.url).as_deref() == Some(h.as_str()) && regex_is(&r.url, r"/t/[^/]+/\d+")) {
                f.software.clear();
            }
            changed = true;
        }
    }
    if changed {
        save(&known);
    }
}
