//! `osearch crawl <url>`: a whole site (usually docs) into a run folder.
//! `llms-full.txt` (the whole docs as one file) needs no crawl. Otherwise the
//! page list is `llms.txt`'s links merged with the sitemap (else donsetch's
//! map, else the seed page's links); pages fetched through the tiers in
//! parallel, each saved with its outline, one index. `--resume <run>`
//! continues a stopped crawl.

use crate::context::AppContext;
use crate::errors::SearchError;
use crate::fetch::FetchOpts;
use crate::outline;
use crate::research;
use crate::run::{raw, Run, RunLine};
use crate::tools::{self, Tool};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct CrawlArgs {
    pub url: String,
    pub topic: Option<String>,
    pub max_pages: usize,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub resume: Option<String>,
    pub any_path: bool,
}

fn glob_match(glob: &str, path: &str) -> bool {
    let pattern = format!(
        "^{}$",
        regex::escape(glob).replace(r"\*\*", ".*").replace(r"\*", ".*")
    );
    regex::Regex::new(&pattern).is_ok_and(|r| r.is_match(path))
}

async fn get_text(app: &Arc<AppContext>, url: &str) -> Option<(String, String)> {
    let started = Instant::now();
    let resp = app
        .client
        .get(url)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .ok()?;
    let status = resp.status().as_u16();
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let text = resp.text().await.ok()?;
    raw::record_page("crawl", url, status, started.elapsed(), &text);
    (status == 200).then_some((ct, text))
}

/// Plain text that is not an HTML page (a soft 404 serves HTML).
fn is_text_file(ct: &str, text: &str) -> bool {
    !ct.contains("html") && !text.trim_start().starts_with('<')
}

fn sitemap_locs(xml: &str) -> Vec<String> {
    let re = regex::Regex::new(r"<loc>\s*([^<\s]+)\s*</loc>").expect("loc pattern");
    re.captures_iter(xml)
        .map(|c| c[1].replace("&amp;", "&"))
        .collect()
}

/// The sitemap's URLs: robots.txt's sitemaps, else the usual names at each
/// base (the seed's folder first: docs often keep their own sitemap there).
async fn sitemap_urls(app: &Arc<AppContext>, origin: &str, bases: &[String]) -> Vec<String> {
    let mut maps: Vec<String> = Vec::new();
    for base in bases.iter().rev() {
        maps.push(format!("{base}/sitemap-index.xml"));
        maps.push(format!("{base}/sitemap_index.xml"));
        maps.push(format!("{base}/sitemap.xml"));
    }
    if let Some((_, robots)) = get_text(app, &format!("{origin}/robots.txt")).await {
        for l in robots.lines() {
            if let Some(u) = l.trim().strip_prefix("Sitemap:").or_else(|| l.trim().strip_prefix("sitemap:")) {
                maps.insert(0, u.trim().to_string());
            }
        }
    }
    maps.dedup();
    let mut out = Vec::new();
    let mut tried = 0;
    let mut found_index = false;
    while let Some(m) = maps.pop() {
        tried += 1;
        if tried > 25 {
            break;
        }
        let Some((_, xml)) = get_text(app, &m).await else { continue };
        if !xml.contains("<loc>") {
            continue;
        }
        let locs = sitemap_locs(&xml);
        if xml.contains("<sitemapindex") {
            found_index = true;
            maps.extend(locs);
        } else {
            out.extend(locs);
            // One sitemap (or one index's children) is the map; the other
            // candidate names would only repeat it.
            if !found_index {
                break;
            }
        }
        if out.len() > 20_000 {
            break;
        }
    }
    out
}

/// One key for a page and its markdown twin: `/a/index.md` and `/a.md`
/// are `/a/` and `/a`.
fn page_key(u: &url::Url) -> String {
    let mut clean = u.clone();
    clean.set_fragment(None);
    let path = clean.path().to_string();
    let path = path
        .strip_suffix("index.md")
        .or_else(|| path.strip_suffix(".md"))
        .unwrap_or(&path)
        .to_string();
    clean.set_path(&path);
    crate::engine::normalize_url(clean.as_str())
}

/// donsetch's map mode: the site's URL inventory.
async fn donsetch_map(url: &str) -> Vec<String> {
    let args: Vec<String> = vec![
        "crawl".into(),
        url.into(),
        "--mode".into(),
        "map".into(),
        "--map-limit".into(),
        "5000".into(),
        "--json".into(),
        "-q".into(),
    ];
    let Ok(ran) = tools::run(Tool::Donsetch, &args, Duration::from_secs(120)).await else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&ran.stdout) else {
        tools::contract_warning("donsetch crawl --mode map", "output is not JSON");
        return Vec::new();
    };
    let mut out = Vec::new();
    for key in ["/meta/map", "/meta/pages"] {
        if let Some(list) = v.pointer(key).and_then(|x| x.as_array()) {
            for it in list {
                if let Some(u) = it.as_str().or_else(|| it["url"].as_str()) {
                    out.push(u.to_string());
                }
            }
        }
    }
    if out.is_empty() && v["ok"].as_bool() == Some(true) {
        tools::contract_warning("donsetch crawl --mode map", "no meta.map URL list");
    }
    out
}

pub async fn crawl(app: Arc<AppContext>, args: CrawlArgs) -> Result<research::Outcome, SearchError> {
    let started = Instant::now();
    let seed = url::Url::parse(&args.url).map_err(|e| SearchError::InvalidInput {
        message: format!("not a URL: {e}"),
    })?;
    let origin = seed.origin().ascii_serialization();
    let host = seed.host_str().unwrap_or("").to_string();
    let prefix = {
        let p = seed.path();
        if args.any_path { "/".to_string() } else { p[..=p.rfind('/').unwrap_or(0)].to_string() }
    };
    let topic_terms = args.topic.as_deref().map(outline::terms).unwrap_or_default();
    let mut run = match args.resume.as_deref() {
        Some(r) => Run::follow(r)?,
        None => Run::create(&format!("crawl {host} {}", args.topic.clone().unwrap_or_default()))?,
    };
    let recorder = run.recorder.clone();
    run.write_json(
        &format!("request.r{}.json", run.round),
        &serde_json::json!({
            "crawl": args.url, "topic": args.topic, "max_pages": args.max_pages,
            "include": args.include, "exclude": args.exclude,
        }),
    )?;
    let mut notes: Vec<String> = Vec::new();
    let rec = Some(recorder.labeled("crawl map"));

    // 1. llms-full.txt / llms.txt, at the seed's folder and the site root.
    let mut bases = vec![format!("{origin}{}", prefix.trim_end_matches('/'))];
    if prefix != "/" {
        bases.push(origin.clone());
    }
    // (url, link text): llms.txt and page links carry text the topic can match.
    let mut list: Vec<(String, String)> = Vec::new();
    let mut llms_keys: std::collections::HashSet<String> = Default::default();
    let mut sources: Vec<String> = Vec::new();
    let mut source = String::new();
    for base in &bases {
        let full = raw::scope(rec.clone(), get_text(&app, &format!("{base}/llms-full.txt"))).await;
        if let Some((ct, text)) = full.filter(|(ct, t)| is_text_file(ct, t) && t.len() > 2000) {
            let _ = ct;
            if !run.has_url(&format!("{base}/llms-full.txt")) {
                let f = crate::fetch::Fetched {
                    stamp: None,
                    note: None,
                    published: None,
                    url: format!("{base}/llms-full.txt"),
                    final_url: format!("{base}/llms-full.txt"),
                    title: format!("{host} docs (llms-full.txt)"),
                    markdown: text,
                    ok: true,
                    tier: 0,
                    fetcher: "llms-full.txt".into(),
                    failure: None,
                    message: String::new(),
                    attempts: Vec::new(),
                    loss: None,
                    cost_micros: 0,
                };
                research::save(&mut run, f, "crawl");
            }
            notes.push(format!("{base}/llms-full.txt holds the whole docs as one file: saved as one page, no crawl needed (read its outline)"));
            source = "llms-full.txt".into();
            break;
        }
        if list.is_empty() {
            let idx = raw::scope(rec.clone(), get_text(&app, &format!("{base}/llms.txt"))).await;
            if let Some((_, text)) = idx.filter(|(ct, t)| is_text_file(ct, t)) {
                list = outline::links(&text);
                if !list.is_empty() {
                    sources.push(format!("{base}/llms.txt ({} links)", list.len()));
                    llms_keys = list
                        .iter()
                        .filter_map(|(u, _)| url::Url::parse(u).ok().map(|p| page_key(&p)))
                        .collect();
                }
            }
        }
    }

    if source != "llms-full.txt" {
        // 2. llms.txt is often a short hand-picked list: merge the sitemap
        // in, else donsetch's map; the seed's links when there is nothing.
        let map = raw::scope(rec.clone(), sitemap_urls(&app, &origin, &bases)).await;
        if !map.is_empty() {
            sources.push(format!("sitemap ({} URLs)", map.len()));
            list.extend(map.into_iter().map(|u| (u, String::new())));
        } else if tools::locate(Tool::Donsetch).is_some() {
            let map = raw::scope(rec.clone(), donsetch_map(&args.url)).await;
            if !map.is_empty() {
                sources.push(format!("donsetch map ({} URLs)", map.len()));
                list.extend(map.into_iter().map(|u| (u, String::new())));
            }
        }
        if list.is_empty() {
            let f = raw::scope(rec.clone(), crate::fetch::fetch(&app, &args.url, &FetchOpts { links: true, skip: vec!["wayback".into()], ..FetchOpts::default() })).await;
            if f.ok {
                list = outline::links(&f.markdown);
            }
            list.insert(0, (args.url.clone(), String::new()));
            sources.push(format!("the seed page's links ({} URLs)", list.len()));
        }
        source = sources.join(" + ");
        // Docs are often split over sibling folders (/guides/, /chatml/,
        // /grid/): name the ones this crawl leaves out.
        if !args.any_path && prefix != "/" && args.include.is_empty() {
            const NOT_DOCS: &[&str] = &[
                "blog", "news", "case", "cases", "customers", "pricing", "terms", "privacy", "legal", "about",
                "careers", "jobs", "press", "tag", "tags", "category", "categories", "author", "authors",
                "company", "contact", "events", "webinars", "podcast", "store", "shop", "compare", "vs",
                "alternatives", "templates", "integrations", "changelog", "partners", "affiliates",
            ];
            let mine = prefix.trim_matches('/').split('/').next().unwrap_or("").to_string();
            let mut other: std::collections::BTreeMap<String, usize> = Default::default();
            for (u, _) in &list {
                let Ok(p) = url::Url::parse(u) else { continue };
                if p.host_str().map(|h| h.trim_start_matches("www.")) != Some(host.trim_start_matches("www.")) {
                    continue;
                }
                let path = p.path().trim_start_matches('/');
                let first = path.split('/').next().unwrap_or("");
                // Folders only: something must follow the folder's name.
                if first.is_empty()
                    || first == mine
                    || !path.contains('/')
                    || NOT_DOCS.contains(&first.to_ascii_lowercase().as_str())
                {
                    continue;
                }
                *other.entry(first.to_string()).or_default() += 1;
            }
            let mut other: Vec<(String, usize)> = other.into_iter().filter(|(_, n)| *n >= 3).collect();
            other.sort_by(|a, b| b.1.cmp(&a.1));
            other.truncate(4);
            if !other.is_empty() {
                notes.push(format!(
                    "the map also has {} outside {prefix}; to add them to this run: `osearch crawl {origin}/ --any-path --include \"{}\" --resume {}`",
                    other.iter().map(|(f, n)| format!("/{f}/ ({n})")).collect::<Vec<_>>().join(", "),
                    other.iter().map(|(f, _)| format!("/{f}/*")).collect::<Vec<_>>().join(","),
                    run.id
                ));
            }
        }
        // 3. Filter to the site and the seed's folder, the globs, the topic.
        let mut seen = std::collections::HashSet::new();
        let mut picked: Vec<(usize, String)> = list
            .into_iter()
            .filter_map(|(u, text)| {
                let p = url::Url::parse(&u).ok()?;
                let h = p.host_str()?.to_string();
                let same = h == host || h.trim_start_matches("www.") == host.trim_start_matches("www.");
                let path = p.path().to_string();
                let key = page_key(&p);
                let ok = same
                    && (path.starts_with(&prefix) || llms_keys.contains(&key) || args.any_path)
                    && (args.include.is_empty() || args.include.iter().any(|g| glob_match(g, &path)))
                    && !args.exclude.iter().any(|g| glob_match(g, &path));
                let mut clean = p.clone();
                clean.set_fragment(None);
                (ok && seen.insert(key)).then(|| {
                    let low = format!("{} {}", clean.as_str(), text).to_lowercase();
                    let score = topic_terms.iter().filter(|t| low.contains(t.as_str())).count();
                    (score, clean.to_string())
                })
            })
            .filter(|(score, _)| topic_terms.is_empty() || *score > 0)
            .collect();
        if !topic_terms.is_empty() {
            picked.sort_by(|a, b| b.0.cmp(&a.0));
        }
        let total = picked.len();
        let skipped_done = picked.iter().filter(|(_, u)| run.has_url(u)).count();
        let targets: Vec<(String, String)> = picked
            .into_iter()
            .map(|(_, u)| u)
            .filter(|u| !run.has_url(u))
            .take(args.max_pages)
            .map(|u| (u, "crawl".to_string()))
            .collect();
        notes.push(format!(
            "map from {source}: {total} pages match; fetching {} (max {}){}",
            targets.len(),
            args.max_pages,
            if skipped_done > 0 { format!(", {skipped_done} already in this run") } else { String::new() }
        ));
        if total > targets.len() + skipped_done {
            // The same filters, so the next round picks from the same list.
            let mut flags = String::new();
            if let Some(t) = &args.topic {
                flags.push_str(&format!(" --topic \"{t}\""));
            }
            if args.any_path {
                flags.push_str(" --any-path");
            }
            if !args.include.is_empty() {
                flags.push_str(&format!(" --include \"{}\"", args.include.join(",")));
            }
            if !args.exclude.is_empty() {
                flags.push_str(&format!(" --exclude \"{}\"", args.exclude.join(",")));
            }
            notes.push(format!(
                "{} more pages are left: `osearch crawl {}{flags} --resume {}` continues",
                total - targets.len() - skipped_done,
                args.url,
                run.id
            ));
        }
        let fopts = FetchOpts { links: true, skip: vec!["wayback".into()], ..FetchOpts::default() };
        let ids = run.reserve(targets.len());
        let targets: Vec<research::Target> = targets
            .into_iter()
            .zip(ids)
            .map(|((url, from), id)| research::Target::new(url, from, id))
            .collect();
        // A crawl waits for every page, except those of a site that keeps
        // walling us: they finish in the background.
        let (targets, later) = research::split_walled(targets);
        research::fetch_all(&app, &recorder, &mut run, targets, &fopts, &research::Limits::default()).await;
        if let Some(line) = research::leave_to_background(&mut run, later, &fopts) {
            notes.push(line);
        }
    }

    // The index: every page with its size and top sections.
    let mut index = format!("# Crawl of {}\n\n", args.url);
    if run.round > 1 {
        index.push_str(&format!(
            "- {} pages in this run over {} rounds (every page is listed below; the notes are round {}'s)\n",
            run.pages().len(),
            run.round,
            run.round
        ));
    }
    for n in &notes {
        index.push_str(&format!("- {n}\n"));
    }
    index.push('\n');
    let mut cost = 0u64;
    for p in run.pages() {
        cost += p.cost_micros;
        if p.ok {
            let heads: Vec<String> = p
                .sections
                .iter()
                .filter(|s| s.level <= 2)
                .take(8)
                .map(|s| format!("#{} {}", s.id, crate::providers::thread::clip(&s.title, 40)))
                .collect();
            index.push_str(&format!(
                "- {} {} ({} tok) {}\n    {}\n",
                p.id,
                crate::providers::thread::clip(&p.title, 90),
                outline::ktok(p.tokens),
                p.final_url,
                heads.join(" · ")
            ));
        } else {
            index.push_str(&format!(
                "- {} FAILED [{}] {} ({})\n",
                p.id,
                p.failure.as_deref().unwrap_or("?"),
                p.url,
                p.message
            ));
        }
    }
    let ok = run.pages().iter().filter(|p| p.ok).count();
    let failed = run.pages().len() - ok;
    let tiers: std::collections::BTreeMap<String, usize> = run
        .pages()
        .iter()
        .filter(|p| p.ok)
        .fold(Default::default(), |mut m, p| {
            *m.entry(p.fetcher.clone()).or_default() += 1;
            m
        });
    let contract = tools::contract_warnings();
    let mut summary = format!(
        "# OmniSearch crawl {} (round {})\n\nfolder: {}\n{ok} pages saved, {failed} failed · fetchers: {} · ~${:.4} · {:.1} s\n",
        run.id,
        run.round,
        run.dir.display(),
        tiers.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", "),
        cost as f64 / 1e6,
        started.elapsed().as_secs_f64()
    );
    if !contract.is_empty() {
        summary.push_str("\n## !!! CONTRACT WARNINGS\n\n");
        for c in &contract {
            summary.push_str(&format!("- {c}\n"));
        }
    }
    for n in &notes {
        summary.push_str(&format!("- {n}\n"));
    }
    summary.push_str("\nIndex of every page: index.md in the folder. Read one: `osearch read pN` (outline) or `osearch read pN#K`; search all: `osearch grep <words>`.\n");
    run.write_text("index.md", &index)?;
    run.write_text(&format!("summary.r{}.md", run.round), &summary)?;
    run.write_text("summary.md", &summary)?;
    run.index(&RunLine {
        id: run.id.clone(),
        round: run.round,
        ts: raw::now(),
        topic: format!("crawl {}{}", args.url, args.topic.map(|t| format!(" ({t})")).unwrap_or_default()),
        queries: Vec::new(),
        urls: run.pages().len(),
        pages: ok,
        cost_usd: cost as f64 / 1e6,
        kind: "crawl".into(),
    });
    Ok(research::Outcome {
        summary,
        run_dir: run.dir.clone(),
    })
}
