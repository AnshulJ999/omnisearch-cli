//! AI synthesis of a run: a large-context model reads a round's pages and
//! writes a cited answer (`synthesis.rN.md`, the latest also as
//! `synthesis.md`), and every quote it gives is checked word for word
//! against the saved pages. A follow-up round is answered from its own
//! pages, with the earlier rounds' answers as background; `--all` reads
//! every round. What was sent, the raw answer and the figures are kept
//! beside it (`synthesis.rN.request.json`, `.response.json`, `.json`).
//! The model is reached
//! over an Anthropic-style (`/v1/messages`) or OpenAI-style
//! (`/v1/chat/completions`) endpoint (`synth.api`); the prompt is built in,
//! or a text file read at run time (`synth.prompt`).

use crate::context::AppContext;
use crate::research::{Planned, Request};
use crate::run::{PageEntry, Run};
use crate::types::Mode;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const OUT: &str = "synthesis.md";
pub const RUNNING: &str = "synthesis.running";
/// Some endpoints refuse a text block over 1 MiB.
const BLOCK_BYTES: usize = 900_000;

/// The part of a run a synthesis reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Round(usize),
    All,
}

impl Scope {
    fn tag(self) -> String {
        match self {
            Scope::Round(n) => format!("r{n}"),
            Scope::All => "all".into(),
        }
    }
    fn file(self) -> String {
        format!("synthesis.{}.md", self.tag())
    }
    fn label(self) -> String {
        match self {
            Scope::Round(n) => format!("round {n}"),
            Scope::All => "all rounds".into(),
        }
    }
}

/// The newest round with a request file.
pub fn latest_round(dir: &Path) -> usize {
    (1..1000).take_while(|n| dir.join(format!("request.r{n}.json")).exists()).last().unwrap_or(1)
}

/// Synthesis needs an endpoint and a model; without them it is off.
pub fn configured() -> bool {
    let s = &crate::config::get().synth;
    !s.url.trim().is_empty() && !s.model.trim().is_empty()
}

/// Whether a run gets a synthesis: the run's own choice, else `synth`
/// settings (auto: deep and request-file runs on, quick searches off).
pub fn wanted(req: &Request, planned: &[Planned]) -> bool {
    if !configured() {
        return false;
    }
    if let Some(b) = req.synth {
        return b;
    }
    // Known pages only (`osearch fetch`): nothing to answer.
    if req.queries.is_empty() {
        return false;
    }
    let s = &crate::config::get().synth;
    match s.enabled.as_str() {
        "on" => true,
        "off" => false,
        _ if planned.iter().any(|p| !p.side && p.mode == Mode::Deep) => s.deep,
        _ if req.quick => s.quick,
        _ => s.requests,
    }
}

/// What a run prints about its synthesis: a status line (and the short
/// answer when it is ready) for the top of the summary, the rest for its end.
#[derive(Default)]
pub struct Printed {
    pub top: String,
    pub tail: String,
}

/// The status line for a run that asked for a synthesis that is not set up.
pub fn not_set_up(req: &Request) -> Option<String> {
    (req.synth == Some(true) && !configured())
        .then(|| "synthesis: not set up, so none was written (set synth.url and synth.model; see config.example.toml)\n".to_string())
}

/// After a round: its synthesis, waited for, or started in its own process.
pub async fn after_run(app: &Arc<AppContext>, run: &Run, _req: &Request) -> Printed {
    // No pages this round (`--fetch 0`): nothing to synthesize, nothing to report.
    if !run.pages().iter().any(|p| p.ok && p.round == run.round) && !crate::finish::is_running(&run.dir) {
        return Printed::default();
    }
    let s = &crate::config::get().synth;
    if s.wait == "background" {
        return match spawn(&run.id) {
            Ok(()) => Printed {
                top: format!(
                    "synthesis: running in the background ({}); start `osearch wait {}` as a background command to be told when it is ready\n",
                    s.model, run.id
                ),
                tail: format!(
                    "\n## AI synthesis\n\nRunning in the background ({}). Start `osearch wait {}` as a background command to be told when it is ready.\n",
                    s.model, run.id
                ),
            },
            Err(e) => Printed {
                top: format!("synthesis: not started ({e}); `osearch synth {}` runs it\n", run.id),
                tail: format!("\n## AI synthesis\n\nNot started: {e}. Run it with `osearch synth {}`.\n", run.id),
            },
        };
    }
    render(&synthesize(app, &run.dir, Scope::Round(run.round)).await, &run.id)
}

/// A synthesis of this run is under way (a marker younger than the longest
/// one could take; an older marker is left from a crash).
fn running(dir: &Path) -> bool {
    let limit = Duration::from_secs(crate::config::get().synth.timeout + 180);
    std::fs::metadata(dir.join(RUNNING))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < limit)
}

/// `osearch synth [RUN] [--wait] [--all]`.
pub async fn command(app: Arc<AppContext>, which: Option<String>, wait: bool, all: bool) -> Result<i32, crate::errors::SearchError> {
    if !configured() {
        return Err(crate::errors::SearchError::Config(
            "synthesis is not set up: set synth.url and synth.model (and synth.api, synth.key) in the config; see config.example.toml".into(),
        ));
    }
    let dir = crate::run::resolve(which.as_deref().unwrap_or("last"))?;
    let id = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let scope = if all { Scope::All } else { Scope::Round(latest_round(&dir)) };
    if wait {
        let limit = Duration::from_secs(crate::config::get().synth.timeout + 30);
        let started = Instant::now();
        while dir.join(RUNNING).exists() && started.elapsed() < limit {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        if dir.join(OUT).exists() && !dir.join(RUNNING).exists() {
            let text = std::fs::read_to_string(dir.join(OUT)).unwrap_or_default();
            println!("run {id}\n{}", shown(&text));
            return Ok(0);
        }
    } else {
        if running(&dir) {
            return Err(crate::errors::SearchError::InvalidInput {
                message: format!("a synthesis of {id} is already running; `osearch synth {id} --wait` prints it when it is done"),
            });
        }
        // Marked now, so a second start while pages land is refused.
        let _ = std::fs::write(dir.join(RUNNING), chrono::Local::now().to_rfc3339());
        // Let the run's background pages land first.
        let started = Instant::now();
        while crate::finish::is_running(&dir) && started.elapsed() < Duration::from_secs(120) {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
    let r = synthesize(&app, &dir, scope).await;
    let p = render(&r, &id);
    print!("run {id}\n{}{}", p.top, p.tail);
    Ok(if r.is_ok() { 0 } else { 1 })
}

/// Start `osearch synth <id>` as its own process, outliving this one.
fn spawn(id: &str) -> Result<(), String> {
    spawn_self(&["synth", id])
}

/// Start osearch with `args` as its own process, outliving this one (the
/// Claude app's job object included).
pub(crate) fn spawn_self(args: &[&str]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        // Windows hands every inheritable handle to a child, our own output
        // pipe included: whoever reads this command's output would then wait
        // for the background process too. Our standard handles stop being
        // inheritable (std duplicates them for later children that need them).
        use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT};
        use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
        for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: plain Win32 calls on this process's own standard handles.
            unsafe {
                let h = GetStdHandle(which);
                if !h.is_null() {
                    SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
                }
            }
        }
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
        if cmd.creation_flags(base | CREATE_BREAKAWAY_FROM_JOB).spawn().is_ok() {
            return Ok(());
        }
        cmd.creation_flags(base);
    }
    // Its own process group, so a terminal's hangup or Ctrl-C does not reach it.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

struct Done {
    scope: Scope,
    /// Earlier rounds given as their own syntheses.
    earlier: usize,
    /// Micro-USD; None when no price is known for the model.
    cost: Option<u64>,
    /// The cost is osearch's own price-list estimate, not the endpoint's figure.
    estimated: bool,
    short: Option<String>,
    full: String,
    quotes: usize,
    found: usize,
    bad_cites: usize,
    secs: u64,
    tokens_in: u64,
    tokens_out: u64,
    pages_sent: usize,
    /// Pages sent as their best sections only (size limit).
    pages_cut: usize,
    /// Pages still fetching in the background when it read the run.
    pages_pending: usize,
    pages_left: usize,
}

/// The short answer goes at the top of the output (agents often read only
/// the start); the end keeps the full text, or a pointer when `synth.show`
/// asks for the short answer only.
fn render(r: &Result<Done, String>, id: &str) -> Printed {
    match r {
        Ok(d) => {
            let s = &crate::config::get().synth;
            let file = d.scope.file();
            let mut top = format!("synthesis: ready ({}) · the full text is at the end and in {file}\n", d.scope.label());
            if let Some(short) = &d.short {
                top.push_str(&format!("\n## Answer (short, from the synthesis)\n\n{short}\n"));
            }
            let body = match (s.show.as_str(), &d.short) {
                ("short", Some(_)) => format!("The short answer is at the top of this output; the full text is in {file}."),
                _ => d.full.clone(),
            };
            Printed { top, tail: format!("\n{}\n\n{body}\n", header(d, &s.model)) }
        }
        Err(e) => Printed {
            top: format!("synthesis: failed ({e}); `osearch synth {id}` tries again\n"),
            tail: format!("\n## AI synthesis\n\nSynthesis failed: {e}. Run `osearch synth {id}` to try again.\n"),
        },
    }
}

fn header(d: &Done, model: &str) -> String {
    let mut left = String::new();
    if d.earlier > 0 {
        left.push_str(&format!(", {} earlier round(s) as their syntheses", d.earlier));
    }
    if d.pages_cut > 0 {
        left.push_str(&format!(", {} as excerpts", d.pages_cut));
    }
    if d.pages_left > 0 {
        left.push_str(&format!(", {} left out (size limit)", d.pages_left));
    }
    if d.pages_pending > 0 {
        left.push_str(&format!(", {} still fetching (not read)", d.pages_pending));
    }
    let cost = match (d.cost, d.estimated) {
        (None, _) => "price unknown".to_string(),
        (Some(c), false) => format!("${:.4}", c as f64 / 1e6),
        (Some(c), true) => format!("~${:.4} estimated", c as f64 / 1e6),
    };
    format!(
        "## AI synthesis ({}; {model}, {} s, {} pages read{left}, {}k tokens in, {}k out, {cost}; quotes: {} of {} found in the pages{}; full text: {})",
        d.scope.label(),
        d.secs,
        d.pages_sent,
        d.tokens_in / 1000,
        d.tokens_out / 1000,
        d.found,
        d.quotes,
        if d.bad_cites > 0 { format!(", {} citations to missing sections", d.bad_cites) } else { String::new() },
        d.scope.file()
    )
}

/// What `osearch wait` prints for a synthesis that ran in the background.
pub fn ready_text(dir: &Path) -> String {
    if dir.join(RUNNING).exists() {
        return "\nAI synthesis: still running; run `osearch wait` again later.\n".into();
    }
    match std::fs::read_to_string(dir.join(OUT)) {
        Ok(text) => format!("\n## AI synthesis\n{}", shown(&text)),
        Err(_) => "\nAI synthesis: none was written (`osearch synth` runs it).\n".into(),
    }
}

/// What `--wait` prints from a saved synthesis: the header line plus the
/// part `synth.show` asks for.
fn shown(text: &str) -> String {
    let s = &crate::config::get().synth;
    let short = between(text, &s.shorttag);
    let full = between(text, &s.fulltag);
    let head = text.lines().next().unwrap_or("").to_string();
    let body = match (s.show.as_str(), short, full) {
        ("short", Some(sh), _) => sh,
        ("both", Some(sh), Some(f)) => format!("{sh}\n\n---\n\n{f}"),
        (_, _, Some(f)) => f,
        _ => text.lines().skip(1).collect::<Vec<_>>().join("\n"),
    };
    format!("\n{head}\n\n{body}\n")
}

/// The text inside `<tag>...</tag>`, trimmed.
fn between(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let a = text.find(&open)? + open.len();
    let b = text[a..].find(&close)? + a;
    Some(text[a..b].trim().to_string())
}

/// The prompt: the file `synth.prompt` names, else the copy built in.
fn prompt_text() -> String {
    let configured = crate::config::get().synth.prompt.clone();
    let builtin = include_str!("../prompts/synth.md");
    let text = if configured.is_empty() {
        builtin.to_string()
    } else {
        std::fs::read_to_string(PathBuf::from(&configured)).unwrap_or_else(|_| builtin.to_string())
    };
    // The file's leading HTML comment is for people, not the model.
    match (text.find("<!--"), text.find("-->")) {
        (Some(0), Some(end)) => text[end + 3..].trim_start().to_string(),
        _ => text,
    }
}

/// A round's topic and searches, from its request file.
fn round_request(dir: &Path, round: usize) -> Option<(String, Vec<String>)> {
    let t = std::fs::read_to_string(dir.join(format!("request.r{round}.json"))).ok()?;
    let v = serde_json::from_str::<serde_json::Value>(&t).ok()?;
    let mut queries: Vec<String> = Vec::new();
    for q in v["queries"].as_array().into_iter().flatten() {
        let text = q.as_str().or_else(|| q["q"].as_str()).unwrap_or("").to_string();
        if !text.is_empty() && !queries.contains(&text) {
            queries.push(text);
        }
    }
    Some((v["topic"].as_str().unwrap_or("").trim().to_string(), queries))
}

/// The question and the searches: one round's (its own topic, else its
/// first search), or the whole run's (round 1's topic, every search).
fn question(dir: &Path, scope: Scope) -> (String, Vec<String>) {
    let rounds: Vec<usize> = match scope {
        Scope::Round(n) => vec![n],
        Scope::All => (1..=latest_round(dir)).collect(),
    };
    let mut topic = String::new();
    let mut queries: Vec<String> = Vec::new();
    for r in rounds {
        let Some((t, qs)) = round_request(dir, r) else { continue };
        if topic.is_empty() {
            topic = t;
        }
        for q in qs {
            if !queries.contains(&q) {
                queries.push(q);
            }
        }
    }
    if topic.is_empty() {
        topic = queries.first().cloned().unwrap_or_default();
    }
    (topic, queries)
}

/// Earlier rounds of a run as background for round `n`: what each asked and
/// the short answer of its own synthesis. Returns the block and the count.
fn earlier_rounds(dir: &Path, n: usize) -> (String, usize) {
    let tag = &crate::config::get().synth.shorttag;
    let mut out = String::new();
    let mut count = 0;
    for r in 1..n {
        let Some((topic, queries)) = round_request(dir, r) else { continue };
        if queries.is_empty() {
            continue;
        }
        let answer = std::fs::read_to_string(dir.join(Scope::Round(r).file()))
            .ok()
            .and_then(|t| between(&t, tag))
            .map(|a| strip_marks(&a))
            .unwrap_or_else(|| "(no synthesis was saved for this round)".into());
        let asked = if topic.is_empty() { queries[0].clone() } else { topic };
        out.push_str(&format!("Round {r} asked: {asked}\nIts searches: {}\nIts synthesis said:\n{answer}\n\n", queries.join(" | ")));
        count += 1;
    }
    (out, count)
}

/// An earlier answer without its quote-check marks and citations, which
/// point at pages this synthesis is not given.
fn strip_marks(text: &str) -> String {
    static MARKS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r" ?\[(verified|not found in [^\]]*|no such section)\]| ?\[p\d+#\d+\]").expect("regex")
    });
    MARKS.replace_all(text, "").into_owned()
}

/// A page with `[p3#4]` marking where each section begins.
fn marked(id: &str, md: &str, secs: &[crate::outline::Section]) -> String {
    let mut starts: std::collections::HashMap<usize, usize> = Default::default();
    for s in secs {
        starts.entry(s.start).or_insert(s.id);
    }
    let mut out = String::with_capacity(md.len() + secs.len() * 12);
    for (i, line) in md.lines().enumerate() {
        if let Some(n) = starts.get(&i) {
            out.push_str(&format!("[{id}#{n}]\n"));
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The `## Engine answers` parts of the rounds' summaries (`summary.rN.md`;
/// `summary.md` only holds the latest round).
fn engine_answers(dir: &Path, scope: Scope) -> String {
    let mut files: Vec<String> = match scope {
        Scope::Round(n) => vec![format!("summary.r{n}.md")].into_iter().filter(|f| dir.join(f).exists()).collect(),
        Scope::All => (1..100).map(|n| format!("summary.r{n}.md")).take_while(|f| dir.join(f).exists()).collect(),
    };
    if files.is_empty() {
        files.push("summary.md".into());
    }
    let mut lines: Vec<String> = Vec::new();
    for f in files {
        let s = std::fs::read_to_string(dir.join(f)).unwrap_or_default();
        let Some(a) = s.find("## Engine answers") else { continue };
        let rest = &s[a..];
        let end = rest[3..].find("\n## ").map_or(rest.len(), |i| i + 3);
        for l in rest[..end].lines().skip(1).map(str::trim).filter(|l| !l.is_empty()) {
            if !lines.iter().any(|x| x == l) {
                lines.push(l.to_string());
            }
        }
    }
    lines.join("\n")
}

/// Results with no fetched page (never picked, or picked and not fetched):
/// title, address and every engine's snippet, `research.snippetprefer`
/// engines first.
fn other_results(dir: &Path, n: usize, scope: Scope) -> String {
    let read: std::collections::HashSet<String> =
        crate::run::load_pages(dir).into_iter().filter(|p| p.ok).map(|p| p.id).collect();
    let prefer = &crate::config::get().research.snippetprefer;
    let text = crate::run::read_text(&dir.join("results.jsonl")).unwrap_or_default();
    let rows: Vec<serde_json::Value> = text.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).collect();
    // Runs from before rows carried their round are read whole.
    let tagged = rows.iter().any(|r| r["round"].as_u64().is_some());
    rows.into_iter()
        .filter(|r| match scope {
            Scope::Round(k) if tagged => r["round"].as_u64() == Some(k as u64),
            _ => true,
        })
        .filter(|r| r["page"].as_str().is_none_or(|id| !read.contains(id)))
        .take(n)
        .map(|r| {
            let pairs: Vec<(String, String)> = r["found_by"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| {
                    (
                        f["engine"].as_str().unwrap_or("").to_string(),
                        f["snippet"].as_str().unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" "),
                    )
                })
                .collect();
            let date = r["found_by"]
                .as_array()
                .and_then(|a| a.iter().find_map(|f| f["published"].as_str()))
                .map(|d| format!(" | {}", crate::research::short_date(d)))
                .unwrap_or_default();
            let mut out = format!("- {} | {}{date}", r["title"].as_str().unwrap_or(""), r["url"].as_str().unwrap_or(""));
            for (engine, snippet) in crate::research::order_snippets(pairs, prefer) {
                out.push_str(&format!("\n  {engine}: {}", crate::providers::thread::clip(&snippet, 3000)));
            }
            out
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn page_number(id: &str) -> usize {
    id.trim_start_matches('p').parse().unwrap_or(usize::MAX)
}

/// A page cut to `cap` tokens: its opening, then its best sections for the
/// question, then the sections after the opening; kept in page order with
/// `[p3#4]` marks.
fn excerpt(id: &str, md: &str, secs: &[crate::outline::Section], terms: &[String], cap: usize) -> String {
    use crate::outline::tokens;
    let lines: Vec<&str> = md.lines().collect();
    let text = |a: usize, b: usize| lines[a.min(lines.len())..b.min(lines.len())].join("\n");
    let mut chosen: Vec<(usize, String)> = Vec::new();
    let mut used = 0usize;
    // The opening: whole sections up to a quarter of the cap, or the first
    // one's first lines when it alone is longer.
    for sec in secs {
        if used + sec.tokens > cap / 4 {
            if chosen.is_empty() {
                let mut part = String::new();
                for l in &lines[sec.start.min(lines.len())..sec.end.min(lines.len())] {
                    let t = tokens(l);
                    if used + t > cap / 4 {
                        break;
                    }
                    used += t;
                    part.push_str(l);
                    part.push('\n');
                }
                chosen.push((sec.id, part));
            }
            break;
        }
        used += sec.tokens;
        chosen.push((sec.id, text(sec.start, sec.end)));
    }
    let ranked = crate::outline::best_sections(md, secs, terms, secs.len());
    let rest = secs.iter().map(|s| (s.id, 0usize));
    for (sid, _) in ranked.into_iter().chain(rest) {
        if chosen.iter().any(|(c, _)| *c == sid) {
            continue;
        }
        let Some(sec) = secs.iter().find(|s| s.id == sid) else { continue };
        if used + sec.tokens > cap {
            continue;
        }
        used += sec.tokens;
        chosen.push((sid, text(sec.start, sec.end)));
    }
    chosen.sort_by_key(|(sid, _)| *sid);
    let mut out = String::new();
    let mut prev = 0usize;
    for (sid, body) in chosen {
        if prev != 0 && sid != prev + 1 {
            out.push_str("[...]\n");
        }
        out.push_str(&format!("[{id}#{sid}]\n{}\n", body.trim_end()));
        prev = sid;
    }
    out
}

/// The material, one text block per source, best pages first. Under
/// `maxinput` tokens every page goes whole (each up to `pagecap`); over it,
/// top pages whole and the rest as their best sections, so every page is
/// represented; pages left out only when even that does not fit.
struct Material {
    blocks: Vec<String>,
    /// Per page: how it was sent (whole, excerpt, left out) and its tokens.
    sent_as: Vec<serde_json::Value>,
    sent: usize,
    cut: usize,
    left: usize,
}

fn material(dir: &Path, pages: &[PageEntry], terms: &[String], scope: Scope) -> Material {
    use crate::outline::tokens;
    let s = &crate::config::get().synth;
    const SHORT: usize = 4_000;
    struct Piece {
        id: String,
        round: usize,
        head: String,
        whole: String,
        whole_tok: usize,
        short: String,
        short_tok: usize,
    }
    let mut pieces = Vec::new();
    for p in pages {
        let Ok(md) = std::fs::read_to_string(dir.join(format!("pages/{}.md", p.id))) else { continue };
        let full = marked(&p.id, &md, &p.sections);
        let full_tok = tokens(&full);
        let short = excerpt(&p.id, &md, &p.sections, terms, SHORT.min(s.pagecap));
        let whole = if s.pages == "sections" {
            short.clone()
        } else if full_tok > s.pagecap {
            excerpt(&p.id, &md, &p.sections, terms, s.pagecap)
        } else {
            full
        };
        let head = format!(
            "<page id=\"{}\" title=\"{}\" url=\"{}\" fetched=\"tier {} {}\">\n",
            p.id,
            p.title.replace('"', "'"),
            p.final_url,
            p.tier,
            p.fetcher
        );
        pieces.push(Piece {
            id: p.id.clone(),
            round: p.round,
            head,
            whole_tok: tokens(&whole),
            whole,
            short_tok: tokens(&short),
            short,
        });
    }
    let mut blocks = Vec::new();
    let mut sent_as = Vec::new();
    let (mut sent, mut cut, mut left) = (0, 0, 0);
    let mut used = 0usize;
    for i in 0..pieces.len() {
        let rest: usize = pieces[i + 1..].iter().map(|p| p.short_tok).sum();
        let p = &pieces[i];
        let (body, how, tok) = if used + p.whole_tok + rest <= s.maxinput {
            used += p.whole_tok;
            (&p.whole, "whole", p.whole_tok)
        } else if used + p.short_tok <= s.maxinput {
            used += p.short_tok;
            cut += 1;
            (&p.short, "excerpt", p.short_tok)
        } else {
            left += 1;
            sent_as.push(serde_json::json!({ "id": p.id, "round": p.round, "sent": "left out", "tokens": 0 }));
            continue;
        };
        sent_as.push(serde_json::json!({ "id": p.id, "round": p.round, "sent": how, "tokens": tok }));
        sent += 1;
        let block = format!("{}{body}\n</page>", p.head);
        // One block per page, split where a page passes the block limit.
        let mut rest = block.as_str();
        while rest.len() > BLOCK_BYTES {
            let mut cut = BLOCK_BYTES;
            while !rest.is_char_boundary(cut) {
                cut -= 1;
            }
            blocks.push(rest[..cut].to_string());
            rest = &rest[cut..];
        }
        blocks.push(rest.to_string());
    }
    if s.answers {
        let a = engine_answers(dir, scope);
        if !a.is_empty() {
            blocks.push(format!("<engine_answers>\n{a}\n</engine_answers>"));
        }
    }
    if s.snippets {
        let o = other_results(dir, 80, scope);
        if !o.is_empty() {
            blocks.push(format!("<other_results>\n{o}\n</other_results>"));
        }
    }
    Material { blocks, sent_as, sent, cut, left }
}

async fn synthesize(_app: &Arc<AppContext>, dir: &Path, scope: Scope) -> Result<Done, String> {
    let _ = std::fs::write(dir.join(RUNNING), chrono::Local::now().to_rfc3339());
    // Its own task, so a bug in it fails the synthesis, not the run.
    let d = dir.to_path_buf();
    let r = match tokio::spawn(async move { synthesize_inner(&d, scope).await }).await {
        Ok(r) => r,
        Err(e) if e.is_panic() => Err("an internal error in osearch (a bug; the run's pages are fine)".into()),
        Err(e) => Err(e.to_string()),
    };
    let _ = std::fs::remove_file(dir.join(RUNNING));
    r
}

/// The synthesis client: no idle-read limit (a non-streaming answer is
/// silent until it is complete); each request sets its own timeout.
fn client() -> &'static reqwest::Client {
    static C: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .tcp_nodelay(true)
            .user_agent(format!("omnisearch/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("synthesis HTTP client")
    })
}

async fn synthesize_inner(dir: &Path, scope: Scope) -> Result<Done, String> {
    let s = crate::config::get().synth.clone();
    let rc = &crate::config::get().research;
    let near_copy = |p: &PageEntry| rc.dedupe && p.same_as.is_some() && u64::from(p.overlap.unwrap_or(0)) >= rc.dedupemin;
    let in_scope = |p: &PageEntry| match scope {
        Scope::Round(n) => p.round == n,
        Scope::All => true,
    };
    let mut pages: Vec<PageEntry> = crate::run::load_pages(dir).into_iter().filter(|p| p.ok && !near_copy(p) && in_scope(p)).collect();
    pages.sort_by_key(|p| page_number(&p.id));
    if pages.is_empty() {
        return Err(match scope {
            Scope::Round(n) => format!("round {n} has no fetched pages to read (`--all` reads every round)"),
            Scope::All => "the run has no fetched pages to read".into(),
        });
    }
    let (q, queries) = question(dir, scope);
    let terms = crate::outline::terms(&queries.join(" "));
    let Material { mut blocks, sent_as, sent, cut, left } = material(dir, &pages, &terms, scope);
    let (earlier, earlier_n) = match scope {
        Scope::Round(n) if n > 1 => earlier_rounds(dir, n),
        _ => (String::new(), 0),
    };
    if let Scope::Round(n) = scope {
        if earlier_n > 0 {
            blocks.insert(
                0,
                format!(
                    "This is round {n} of a research run. Answer this round's question from this round's pages below. \
                     The earlier rounds' questions and answers are in <earlier_rounds> as background: use them to connect \
                     findings and avoid repeating what is already known, and cite only this round's pages."
                ),
            );
            blocks.push(format!("<earlier_rounds>\n{}</earlier_rounds>", earlier));
        }
    }
    blocks.push(format!("The question again: {q}"));
    let system = prompt_text()
        .replace("{{date}}", &chrono::Local::now().format("%Y-%m-%d").to_string())
        .replace("{{question}}", &q)
        .replace("{{queries}}", &queries.join(" | "))
        .replace("{{short_tag}}", &s.shorttag)
        .replace("{{full_tag}}", &s.fulltag);
    let content: Vec<serde_json::Value> = blocks
        .iter()
        .map(|b| serde_json::json!({ "type": "text", "text": b }))
        .collect();
    let openai = s.api == "openai";
    let mut body = if openai {
        serde_json::json!({
            "model": s.model,
            "max_tokens": s.maxtokens,
            "stream": false,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": content },
            ],
        })
    } else {
        serde_json::json!({
            "model": s.model,
            "max_tokens": s.maxtokens,
            "stream": false,
            "system": system,
            "messages": [{ "role": "user", "content": content }],
        })
    };
    if !s.effort.is_empty() {
        if openai {
            body["reasoning_effort"] = serde_json::json!(s.effort);
        } else {
            body["output_config"] = serde_json::json!({ "effort": s.effort });
        }
    }
    let run_id = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tag = scope.tag();
    // What is sent is kept beside the answer (the key travels in a header, never here).
    let _ = std::fs::write(dir.join(format!("synthesis.{tag}.request.json")), serde_json::to_string(&body).unwrap_or_default());
    let started = Instant::now();
    // One session id and one body for every attempt: a proxy that keeps
    // sessions (the x-session-id header) can treat an
    // identical request sent within its cancel grace (3 s) as a retry of the
    // dropped one and hands back the same answer, billed once.
    let session = format!("osearch-synth-{run_id}-{}", chrono::Utc::now().timestamp());
    let deadline = started + Duration::from_secs(s.timeout);
    let (mut status, mut text) = post(&s, &body, &session, deadline, openai).await?;
    // OpenAI's reasoning models refuse `max_tokens`; nothing was billed.
    if openai && status.as_u16() == 400 && text.contains("max_tokens") {
        if let Some(obj) = body.as_object_mut() {
            let n = obj.remove("max_tokens").unwrap_or(serde_json::json!(s.maxtokens));
            obj.insert("max_completion_tokens".into(), n);
        }
        (status, text) = post(&s, &body, &session, deadline, openai).await?;
    }
    let _ = std::fs::write(dir.join(format!("synthesis.{tag}.response.json")), &text);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    if !status.is_success() {
        let detail = v["error"]["message"].as_str().map(str::to_string).unwrap_or_else(|| crate::providers::thread::clip(&text, 200));
        return Err(format!("the model answered HTTP {}: {detail}", status.as_u16()));
    }
    let reply = Reply::read(&v, openai);
    let answer = reply.text;
    if answer.trim().len() < 100 {
        return Err(format!("the answer came back {} characters long (stop reason {})", answer.trim().len(), reply.stop));
    }
    let secs = started.elapsed().as_secs();
    let (tokens_in, tokens_out, tokens_cached) = (reply.tokens_in, reply.tokens_out, reply.tokens_cached);
    // The endpoint's own cost figure when it gives one; else the price list.
    let known = v["usage"]["cost_known"].as_bool() == Some(true);
    let (cost, estimated) = match v["usage"]["cost_usd"].as_f64().filter(|_| known) {
        Some(usd) => (Some((usd * 1e6).round() as u64), false),
        None => (
            prices(&s).map(|(pin, pout, pcache)| {
                ((tokens_in as f64 * pin + tokens_out as f64 * pout + tokens_cached as f64 * pcache) * 1e6).round() as u64
            }),
            true,
        ),
    };
    crate::logging::log_spend("synth", cost.unwrap_or(0), &q, "synth");

    let texts: Vec<(String, String)> = pages
        .iter()
        .filter_map(|p| std::fs::read_to_string(dir.join(format!("pages/{}.md", p.id))).ok().map(|t| (p.id.clone(), t)))
        .collect();
    let (checked, quotes, found, bad_cites) = check(&answer, &pages, &texts);
    let short = between(&checked, &s.shorttag);
    let full = between(&checked, &s.fulltag).unwrap_or_else(|| checked.clone());
    let done = Done {
        scope,
        earlier: earlier_n,
        cost,
        estimated,
        short,
        full,
        quotes,
        found,
        bad_cites,
        secs,
        tokens_in,
        tokens_out,
        pages_sent: sent,
        pages_cut: cut,
        pages_pending: crate::finish::pending_in(dir).len(),
        pages_left: left,
    };
    let manifest = serde_json::json!({
        "scope": scope.label(),
        "question": q,
        "queries": queries,
        "earlier_rounds": earlier_n,
        "model": s.model,
        "api": s.api,
        "effort": s.effort,
        "started": chrono::Local::now().to_rfc3339(),
        "secs": secs,
        "stop": reply.stop,
        "tokens_in": tokens_in,
        "tokens_out": tokens_out,
        "tokens_cached": tokens_cached,
        "cost_usd": cost.map(|c| c as f64 / 1e6),
        "cost_estimated": estimated,
        "quotes": quotes,
        "quotes_found": found,
        "bad_citations": bad_cites,
        "pages": { "sent": sent, "as_excerpts": cut, "left_out": left, "still_fetching": done.pages_pending },
        "sent_as": sent_as,
    });
    let _ = std::fs::write(dir.join(format!("synthesis.{tag}.json")), serde_json::to_string_pretty(&manifest).unwrap_or_default());
    let file = format!("{}\n\n{checked}\n", header(&done, &s.model));
    std::fs::write(dir.join(scope.file()), &file).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(OUT), file).map_err(|e| e.to_string())?;
    Ok(done)
}

/// One request to the synthesis endpoint, retried when the connection drops.
async fn post(
    s: &crate::config::Synth,
    body: &serde_json::Value,
    session: &str,
    deadline: Instant,
    openai: bool,
) -> Result<(reqwest::StatusCode, String), String> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(format!("no answer within {} s ({attempts} attempts)", s.timeout));
        }
        let mut request = client()
            .post(&s.url)
            .header("content-type", "application/json")
            .header("x-session-id", session)
            .timeout(left)
            .json(&body);
        if !openai {
            request = request.header("anthropic-version", "2023-06-01");
        }
        if !s.key.is_empty() {
            request = if openai {
                request.bearer_auth(&s.key)
            } else {
                request.header("x-api-key", &s.key)
            };
        }
        let dropped = |e: &reqwest::Error| !e.is_connect() && !e.is_builder() && !e.is_status();
        match request.send().await {
            Ok(resp) => {
                let status = resp.status();
                match resp.text().await {
                    Ok(t) => return Ok((status, t)),
                    Err(e) if dropped(&e) && attempts < 6 => continue,
                    Err(e) => return Err(format!("the answer was cut off ({})", e.without_url())),
                }
            }
            Err(e) if e.is_connect() => {
                return Err(format!("could not reach the synthesis endpoint at {} ({}); is it running?", s.url, e.without_url()))
            }
            Err(e) if e.is_timeout() => return Err(format!("no answer within {} s ({attempts} attempts)", s.timeout)),
            Err(e) if dropped(&e) && attempts < 6 => continue,
            Err(e) => return Err(format!("the connection to the synthesis endpoint failed ({})", e.without_url())),
        }
    }
}

/// The answer text, stop reason and token counts of either API style.
struct Reply {
    text: String,
    stop: String,
    tokens_in: u64,
    tokens_out: u64,
    tokens_cached: u64,
}

impl Reply {
    fn read(v: &serde_json::Value, openai: bool) -> Reply {
        let parts = |c: &serde_json::Value| -> String {
            match c {
                serde_json::Value::String(t) => t.clone(),
                serde_json::Value::Array(a) => a
                    .iter()
                    .filter(|b| b["type"] == "text")
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            }
        };
        let n = |p: &str| v.pointer(p).and_then(serde_json::Value::as_u64).unwrap_or(0);
        if openai {
            Reply {
                text: parts(&v["choices"][0]["message"]["content"]),
                stop: v["choices"][0]["finish_reason"].as_str().unwrap_or("?").to_string(),
                tokens_in: n("/usage/prompt_tokens"),
                tokens_out: n("/usage/completion_tokens"),
                tokens_cached: n("/usage/prompt_tokens_details/cached_tokens"),
            }
        } else {
            Reply {
                text: parts(&v["content"]),
                stop: v["stop_reason"].as_str().unwrap_or("?").to_string(),
                tokens_in: n("/usage/input_tokens"),
                tokens_out: n("/usage/output_tokens"),
                tokens_cached: n("/usage/cache_read_input_tokens"),
            }
        }
    }
}

/// USD per token (input, output, cached input): the `inprice`/`outprice`
/// settings when set, else a price list file (`synth.prices`, OpenRouter's
/// catalogue shape), looked up by the slug, `~slug`, the slug without `-latest`.
fn prices(s: &crate::config::Synth) -> Option<(f64, f64, f64)> {
    if s.inprice > 0.0 || s.outprice > 0.0 {
        return Some((s.inprice / 1e6, s.outprice / 1e6, s.inprice / 1e6));
    }
    if s.prices.is_empty() {
        return None;
    }
    let text = std::fs::read_to_string(&s.prices).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let m = s.model.as_str();
    let keys = [m.to_string(), format!("~{m}"), m.strip_suffix("-latest").unwrap_or(m).to_string()];
    let p = keys.iter().map(|k| &v["prices"][k.as_str()]).find(|p| p.is_object())?;
    let input = p["prompt"].as_f64()?;
    let output = p["completion"].as_f64()?;
    Some((input, output, p["cacheRead"].as_f64().unwrap_or(input)))
}

/// Lowercase, one space between words, plain quote marks and dashes, no
/// markdown emphasis: how quotes and pages are compared.
fn norm(s: &str) -> String {
    // Links keep their text: a page's "[html5ever](url)" is quoted as "html5ever".
    static LINK: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"!?\[([^\]]*)\]\([^)]*\)").expect("regex"));
    let s = LINK.replace_all(s, "$1");
    let mapped: String = s
        .chars()
        .filter_map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{201B}' => Some('\''),
            '\u{201C}' | '\u{201D}' => Some('"'),
            '\u{2013}' | '\u{2014}' | '\u{2212}' => Some('-'),
            '*' | '_' | '`' | '[' | ']' => None,
            c => Some(c),
        })
        .collect();
    mapped.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Mark each quote found or not found in the page it cites (any page when
/// it cites none), and each citation to a section that does not exist.
fn check(answer: &str, pages: &[PageEntry], texts: &[(String, String)]) -> (String, usize, usize, usize) {
    // Every pair of quote marks in order, so a short quoted word does not
    // pair the next quote's opening mark with its own closing one.
    static QUOTE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new("\"([^\"\\n]*)\"|\u{201C}([^\u{201D}\\n]*)\u{201D}").expect("regex")
    });
    static CITE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\[(p\d+)#(\d+)\]").expect("regex"));
    let normed: Vec<(String, String)> = texts.iter().map(|(id, t)| (id.clone(), norm(t))).collect();
    let mut out = String::with_capacity(answer.len() + 256);
    let (mut quotes, mut found) = (0, 0);
    let mut last = 0;
    // The closing "Gaps" part quotes searches to run, not sources.
    let gaps = answer.find("\n## Gaps").unwrap_or(answer.len());
    for m in QUOTE.captures_iter(answer) {
        let whole = m.get(0).expect("match");
        let q = m.get(1).or_else(|| m.get(2)).map_or("", |g| g.as_str());
        if q.chars().count() < 20 || whole.start() > gaps {
            continue;
        }
        let nq = norm(q.trim_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace()));
        // The citation right after the quote, if any.
        let mut end = answer.len().min(whole.end() + 40);
        while !answer.is_char_boundary(end) {
            end -= 1;
        }
        let tail = &answer[whole.end()..end];
        let cited = CITE.captures(tail).filter(|c| c.get(0).is_some_and(|g| g.start() < 6)).map(|c| c[1].to_string());
        let hit = normed
            .iter()
            .filter(|(id, _)| cited.as_ref().is_none_or(|c| c == id))
            .any(|(_, t)| t.contains(&nq));
        quotes += 1;
        out.push_str(&answer[last..whole.end()]);
        last = whole.end();
        let cite_end = cited
            .as_ref()
            .and_then(|_| CITE.find(tail))
            .map(|c| whole.end() + c.end());
        if let Some(e) = cite_end {
            out.push_str(&answer[last..e]);
            last = e;
        }
        if hit {
            found += 1;
            out.push_str(" [verified]");
        } else {
            out.push_str(&format!(" [not found in {}]", cited.as_deref().unwrap_or("the pages")));
        }
    }
    out.push_str(&answer[last..]);
    // Citations to pages or sections that do not exist.
    let mut bad = 0;
    let marked = CITE
        .replace_all(&out, |c: &regex::Captures| {
            let ok = pages
                .iter()
                .find(|p| p.id == c[1])
                .is_some_and(|p| c[2].parse::<usize>().is_ok_and(|n| p.sections.iter().any(|s| s.id == n)));
            if ok {
                c[0].to_string()
            } else {
                bad += 1;
                format!("{} [no such section]", &c[0])
            }
        })
        .into_owned();
    (marked, quotes, found, bad)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("osearch-test-synth-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("pages")).unwrap();
        d
    }

    /// A round's synthesis gets that round's engine answers; `--all` gets every round's.
    #[test]
    fn engine_answers_follow_the_scope() {
        let d = dir("answers");
        std::fs::write(d.join("summary.r1.md"), "# r1\n\n## Engine answers (x)\n\n- q1 tavily: first\n\n## Pages\n").unwrap();
        std::fs::write(d.join("summary.r2.md"), "# r2\n\n## Engine answers (x)\n\n- q2 tavily: second\n").unwrap();
        std::fs::write(d.join("summary.md"), "# r2\n\n## Engine answers (x)\n\n- q2 tavily: second\n").unwrap();
        let all = engine_answers(&d, Scope::All);
        assert!(all.contains("first") && all.contains("second"), "{all}");
        let r2 = engine_answers(&d, Scope::Round(2));
        assert!(r2.contains("second") && !r2.contains("first"), "{r2}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A picked page that failed to fetch still gets its snippets sent, every engine's, preferred first.
    #[test]
    fn failed_pages_keep_their_snippets() {
        let d = dir("others");
        let page = |id: &str, ok: bool| {
            format!("{{\"id\":\"{id}\",\"url\":\"https://e.com/{id}\",\"final_url\":\"https://e.com/{id}\",\"title\":\"t\",\"ok\":{ok}}}\n")
        };
        std::fs::write(d.join("pages/index.jsonl"), format!("{}{}", page("p1", true), page("p2", false))).unwrap();
        let row = |id: &str, page: &str| {
            format!(
                "{{\"rank\":1,\"url\":\"https://e.com/{id}\",\"title\":\"T {id}\",\"found_by\":[{{\"q\":\"q1\",\"engine\":\"brave\",\"rank\":1,\"title\":\"\",\"snippet\":\"short brave text\"}},{{\"q\":\"q1\",\"engine\":\"exa\",\"rank\":2,\"title\":\"\",\"snippet\":\"long exa passage\"}}]{page}}}\n"
            )
        };
        let rows = format!("{}{}{}", row("p1", ",\"page\":\"p1\""), row("p2", ",\"page\":\"p2\""), row("p3", ""));
        std::fs::write(d.join("results.jsonl"), rows).unwrap();
        let out = other_results(&d, 80, Scope::All);
        assert!(!out.contains("e.com/p1"), "a fetched page is not repeated: {out}");
        assert!(out.contains("e.com/p2") && out.contains("e.com/p3"), "{out}");
        assert!(out.find("exa:").unwrap() < out.find("brave:").unwrap(), "preferred engine first: {out}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
