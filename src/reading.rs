//! Reading a run: `read` (a page's outline, one section, or the whole
//! page), `grep` (a pattern across a run's pages, labelled by page and
//! section) and `runs` (past runs, `--find` before new research).

use crate::errors::SearchError;
use crate::outline::{self, Section};
use crate::run::{self, PageEntry};
use std::path::{Path, PathBuf};

/// `p3`, `p3#4`, `<run>/p3#4`: the run folder, page id, section.
fn parse_ref(r: &str, run_opt: Option<&str>) -> Result<(PathBuf, String, Option<usize>), SearchError> {
    let (run_part, page_part) = match r.rsplit_once('/') {
        Some((a, b)) => (Some(a), b),
        None => (None, r),
    };
    let which = run_opt.or(run_part).unwrap_or("last");
    let dir = run::resolve(which)?;
    let (page, sec) = match page_part.split_once('#') {
        Some((p, s)) => (
            p.to_string(),
            Some(s.parse::<usize>().map_err(|_| SearchError::InvalidInput {
                message: format!("section must be a number: '{s}'"),
            })?),
        ),
        None => (page_part.to_string(), None),
    };
    let page = if page.starts_with('p') {
        page
    } else {
        format!("p{page}")
    };
    Ok((dir, page, sec))
}

fn find_page(dir: &Path, id: &str) -> Result<PageEntry, SearchError> {
    run::load_pages(dir)
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| SearchError::InvalidInput {
            message: format!("no page {id} in {}", dir.display()),
        })
}

fn page_text(dir: &Path, id: &str) -> Result<String, SearchError> {
    Ok(std::fs::read_to_string(dir.join(format!("pages/{id}.md")))?)
}

/// The first output line of `read` and `grep`: which run it came from, so
/// a newest-run default that picked another session's run is visible.
fn run_line(dir: &Path) -> String {
    format!(
        "run {}\n",
        dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    )
}

/// `osearch read`: the outline (no section), one section with its
/// subsections, the whole page (`full`), or the lines matching `find`.
pub fn read(r: &str, run_opt: Option<&str>, full: bool, find: Option<&str>) -> Result<String, SearchError> {
    let (dir, _, _) = parse_ref(r, run_opt)?;
    Ok(run_line(&dir) + &read_page(r, run_opt, full, find)?)
}

fn read_page(r: &str, run_opt: Option<&str>, full: bool, find: Option<&str>) -> Result<String, SearchError> {
    let (dir, id, sec) = parse_ref(r, run_opt)?;
    let entry = find_page(&dir, &id)?;
    if !entry.ok {
        return Ok(format!(
            "{id} was not fetched: [{}] {}\n{}",
            entry.failure.as_deref().unwrap_or("?"),
            entry.message,
            entry.url
        ));
    }
    let text = page_text(&dir, &id)?;
    let secs: Vec<Section> = if entry.sections.is_empty() {
        outline::sections(&text)
    } else {
        entry.sections.clone()
    };
    let lines: Vec<&str> = text.lines().collect();
    if let Some(words) = find {
        let terms = outline::terms(words);
        let terms = if terms.is_empty() { vec![words.to_lowercase()] } else { terms };
        let mut out = format!("{id} lines matching {}:\n", terms.join(" "));
        let mut n = 0;
        for (i, l) in lines.iter().enumerate() {
            let low = l.to_lowercase();
            if terms.iter().any(|t| low.contains(t.as_str())) {
                let sid = secs.iter().find(|s| s.start <= i && i < s.end).map_or(0, |s| s.id);
                out.push_str(&format!("{id}#{sid} L{}: {}\n", i + 1, l.trim()));
                n += 1;
                if n >= 80 {
                    out.push_str("... (more; narrow the words)\n");
                    break;
                }
            }
        }
        return Ok(out);
    }
    if full {
        return Ok(format!("[untrusted web content: {}]\n{text}", entry.final_url));
    }
    match sec {
        None => Ok(outline::render(&id, &entry.title, &entry.final_url, &secs)
            + &format!("tier {} via {}\nread one part: osearch read {id}#N\n", entry.tier, entry.fetcher)),
        Some(s) => {
            let (a, b) = outline::span(&secs, s).ok_or_else(|| SearchError::InvalidInput {
                message: format!("{id} has no section {s} (it has {})", secs.len()),
            })?;
            let body = lines[a.min(lines.len())..b.min(lines.len())].join("\n");
            let next = secs
                .iter()
                .find(|x| x.start >= b)
                .map(|x| format!("\nnext: osearch read {id}#{} ({})", x.id, x.title))
                .unwrap_or_default();
            Ok(format!(
                "[{id}#{s} of {}, untrusted web content]\n{body}{next}\n",
                entry.final_url
            ))
        }
    }
}

/// `osearch grep`: a case-insensitive pattern over every page of a run.
pub fn grep(pattern: &str, run_opt: Option<&str>, max: usize) -> Result<String, SearchError> {
    let dir = run::resolve(run_opt.unwrap_or("last"))?;
    let re = regex::RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .or_else(|_| {
            regex::RegexBuilder::new(&regex::escape(pattern))
                .case_insensitive(true)
                .build()
        })
        .map_err(|e| SearchError::InvalidInput { message: e.to_string() })?;
    let mut out = run_line(&dir);
    let mut hits = 0;
    let mut per_page: Vec<(String, usize)> = Vec::new();
    for p in run::load_pages(&dir).into_iter().filter(|p| p.ok) {
        let Ok(text) = page_text(&dir, &p.id) else { continue };
        let secs = if p.sections.is_empty() { outline::sections(&text) } else { p.sections.clone() };
        let mut n = 0;
        for (i, l) in text.lines().enumerate() {
            if re.is_match(l) {
                n += 1;
                if hits < max {
                    let s = secs.iter().find(|s| s.start <= i && i < s.end);
                    let label = s.map(|s| format!("#{} {}", s.id, crate::providers::thread::clip(&s.title, 40))).unwrap_or_default();
                    out.push_str(&format!(
                        "{}{label} L{}: {}\n",
                        p.id,
                        i + 1,
                        crate::providers::thread::clip(l.trim(), 240)
                    ));
                }
                hits += 1;
            }
        }
        if n > 0 {
            per_page.push((p.id.clone(), n));
        }
    }
    if hits == 0 {
        return Ok(format!("no match for /{pattern}/ in {}\n", dir.display()));
    }
    let counts: Vec<String> = per_page.iter().map(|(p, n)| format!("{p} {n}")).collect();
    out.push_str(&format!(
        "{hits} matches ({}){}\n",
        counts.join(", "),
        if hits > max { format!("; first {max} shown") } else { String::new() }
    ));
    Ok(out)
}

/// `osearch runs`: recent runs, or those matching `find` (topic, queries
/// and summary text).
/// Whole words of a run's topic, queries and page titles.
fn run_words(l: &run::RunLine, titles: &mut std::collections::HashMap<String, String>) -> std::collections::HashSet<String> {
    let t = titles.entry(l.id.clone()).or_insert_with(|| {
        run::load_pages(&run::runs_root().join(&l.id)).iter().map(|p| p.title.clone()).collect::<Vec<_>>().join(" ")
    });
    format!("{} {} {t}", l.topic, l.queries.join(" "))
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '.')
        .map(|w| w.trim_matches('.').to_string())
        .filter(|w| !w.is_empty())
        .collect()
}

/// `osearch runs [--find TOPIC]`. A topic is matched as whole words in each
/// run's topic, queries and page titles, best matches first; when nothing
/// matches, a clearly labelled loose pass looks for word fragments anywhere
/// in the summaries.
pub fn runs(find: Option<&str>, limit: usize) -> String {
    let mut lines = run::all_runs();
    lines.extend(run::unfinished_runs(&lines));
    lines.sort_by(|a, b| a.id.cmp(&b.id).then(a.round.cmp(&b.round)));
    if let Some(found) = find.and_then(|f| by_meaning(f, &lines, limit)) {
        return found;
    }
    let terms = find.map(outline::terms).unwrap_or_default();
    let mut picked: Vec<&run::RunLine> = lines.iter().rev().collect();
    let mut loose = false;
    if !terms.is_empty() {
        let mut titles = std::collections::HashMap::new();
        let need = terms.len().div_ceil(2);
        let mut strict: Vec<(usize, &run::RunLine)> = picked
            .iter()
            .map(|l| {
                let words = run_words(l, &mut titles);
                (terms.iter().filter(|t| words.contains(t.as_str())).count(), *l)
            })
            .filter(|(n, _)| *n >= need)
            .collect();
        strict.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.id.cmp(&a.1.id)));
        if strict.is_empty() {
            loose = true;
            picked.retain(|l| {
                let mut hay = format!("{} {}", l.topic, l.queries.join(" ")).to_lowercase();
                if let Ok(s) = std::fs::read_to_string(run::runs_root().join(&l.id).join("summary.md")) {
                    hay.push_str(&s.to_lowercase());
                }
                terms.iter().filter(|t| hay.contains(t.as_str())).count() >= need
            });
        } else {
            picked = strict.into_iter().map(|(_, l)| l).collect();
        }
    }
    let mut out = String::new();
    if loose && !picked.is_empty() {
        out.push_str(&format!(
            "No run has these words in its topic, queries or page titles. Loose matches (word fragments anywhere in the summaries; often unrelated):\n"
        ));
    }
    let mut shown = 0;
    for l in picked {
        out.push_str(&format!(
            "{} r{} [{}] {} · {} URLs · {} pages · ${:.4}\n",
            l.id,
            l.round,
            l.kind,
            crate::providers::thread::clip(&l.topic, 80),
            l.urls,
            l.pages,
            l.cost_usd
        ));
        shown += 1;
        if shown >= limit {
            break;
        }
    }
    if shown == 0 {
        return match find {
            Some(f) => format!("no past run matches '{f}'\n"),
            None => format!("no runs yet ({})\n", run::archive_root().display()),
        };
    }
    out.push_str(&format!("archive: {}\n", run::runs_root().display()));
    out
}

/// Past runs found by meaning as well as words, through `ctx` (a local
/// search tool over the runs folder) when it is installed and answers;
/// None falls back to word matching.
fn by_meaning(find: &str, lines: &[run::RunLine], limit: usize) -> Option<String> {
    let v = ctx_search(find)?;
    let root = run::runs_root().to_string_lossy().replace('\\', "/").to_lowercase();
    let mut out = String::new();
    let mut seen: Vec<String> = Vec::new();
    for r in v["results"].as_array()? {
        let path = r["path"].as_str().unwrap_or("").replace('\\', "/");
        let Some(rest) = path.to_lowercase().strip_prefix(&format!("{root}/")).map(|_| &path[root.len() + 1..]) else { continue };
        let id = rest.split('/').next().unwrap_or("").to_string();
        if id.is_empty() || seen.contains(&id) {
            continue;
        }
        let Some(l) = lines.iter().filter(|l| l.id == id).max_by_key(|l| l.round) else { continue };
        let hint = r["lines"].as_array().and_then(|a| a.first()).and_then(|x| x.as_str()).unwrap_or("").trim();
        out.push_str(&format!(
            "{} r{} [{}] {} · {} pages\n    {}: {}\n",
            l.id,
            l.round,
            l.kind,
            crate::providers::thread::clip(&l.topic, 80),
            l.pages,
            rest.split_once('/').map_or("", |(_, f)| f),
            crate::providers::thread::clip(hint, 140)
        ));
        seen.push(id);
        if seen.len() >= limit {
            break;
        }
    }
    if seen.is_empty() {
        return None;
    }
    Some(format!("Past runs matching '{find}' by meaning and words (ctx), best first:\n{out}archive: {}\n", run::runs_root().display()))
}

/// `ctx "<question>" -b runs --json`, or None when it is missing, slow or fails.
fn ctx_search(find: &str) -> Option<serde_json::Value> {
    let exe = if cfg!(windows) { "ctx.cmd" } else { "ctx" };
    let mut child = std::process::Command::new(exe)
        .args([find, "-b", "runs", "-n", "40", "--json"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut buf);
        buf
    });
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if started.elapsed() > std::time::Duration::from_secs(45) => {
                let _ = child.kill();
                return None;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
    serde_json::from_str(&reader.join().ok()?).ok()
}
