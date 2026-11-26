//! The run archive: one folder per research run under `<archive>/runs/`,
//! outside git. A follow-up run names its parent and lands in the same
//! folder as a new round; URLs already fetched there are skipped.
//!
//! ```text
//! <id>/request.r1.json   the request of each round
//! <id>/calls.jsonl       every provider and tool call (raw/ holds the bodies)
//! <id>/raw/              every answer untouched
//! <id>/results.jsonl     every URL: each engine's snippet, rank, raw file
//! <id>/pages/pN.md       each fetched page, whole
//! <id>/pages/index.jsonl one line per page: url, tier, fetcher, sections
//! <id>/summary.md        the latest round's summary (summary.rN.md each)
//! ```

pub mod raw;

use crate::errors::SearchError;
use crate::outline::Section;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn archive_root() -> PathBuf {
    PathBuf::from(&crate::config::get().paths.archive)
}

pub fn runs_root() -> PathBuf {
    archive_root().join("runs")
}

fn zst_path(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".zst");
    PathBuf::from(p)
}

/// A run file, or its compressed copy (`<name>.zst`) in a compressed run.
pub fn read_text(path: &Path) -> std::io::Result<String> {
    match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let bytes = zstd::decode_all(std::fs::File::open(zst_path(path))?)?;
            String::from_utf8(bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        }
        r => r,
    }
}

/// What `osearch runs --compress` did.
#[derive(Default)]
pub struct Compressed {
    pub runs: usize,
    pub files: usize,
    pub before: u64,
    pub after: u64,
}

/// Compress the raw engine answers and the results index of runs whose
/// last round is older than `days`; pages, summaries and syntheses stay as
/// they are. Each copy is read back before its original goes. Runs still
/// fetching or writing a synthesis are left alone.
pub fn compress_old(days: u64) -> Compressed {
    let mut done = Compressed::default();
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86_400);
    let Ok(entries) = std::fs::read_dir(runs_root()) else { return done };
    for e in entries.flatten() {
        let dir = e.path();
        let last = (1..1000)
            .map(|n| dir.join(format!("request.r{n}.json")))
            .take_while(|p| p.exists())
            .last()
            .and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
        let Some(last) = last else { continue };
        if last > cutoff || crate::finish::is_running(&dir) || dir.join(crate::synth::RUNNING).exists() {
            continue;
        }
        let mut files: Vec<PathBuf> = vec![dir.join("results.jsonl")];
        if let Ok(raw) = std::fs::read_dir(dir.join("raw")) {
            files.extend(raw.flatten().map(|f| f.path()));
        }
        let mut any = false;
        for f in files {
            if !f.is_file() || f.extension().is_some_and(|x| x == "zst") {
                continue;
            }
            let Ok(data) = std::fs::read(&f) else { continue };
            let Ok(packed) = zstd::encode_all(&data[..], 9) else { continue };
            let ok = zstd::decode_all(&packed[..]).is_ok_and(|back| back == data);
            if !ok || std::fs::write(zst_path(&f), &packed).is_err() {
                continue;
            }
            if std::fs::remove_file(&f).is_ok() {
                done.files += 1;
                done.before += data.len() as u64;
                done.after += packed.len() as u64;
                any = true;
            }
        }
        done.runs += usize::from(any);
    }
    done
}

/// One line per run round in `<archive>/runs.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunLine {
    pub id: String,
    pub round: usize,
    pub ts: u64,
    pub topic: String,
    pub queries: Vec<String>,
    #[serde(default)]
    pub urls: usize,
    #[serde(default)]
    pub pages: usize,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub kind: String,
}

/// One saved page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageEntry {
    pub id: String,
    pub url: String,
    pub final_url: String,
    pub title: String,
    pub ok: bool,
    #[serde(default)]
    pub tier: u8,
    #[serde(default)]
    pub fetcher: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub tokens: usize,
    #[serde(default)]
    pub sections: Vec<Section>,
    #[serde(default)]
    pub round: usize,
    /// What led here: `q2`, `url`, `snowball`, `crawl`.
    #[serde(default)]
    pub from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loss: Option<serde_json::Value>,
    #[serde(default)]
    pub attempts: Vec<serde_json::Value>,
    #[serde(default)]
    pub cost_micros: u64,
    /// Fetched by the background process after the run returned.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub late: bool,
    /// Where the copy came from: `read live <time>`, or an archive, reader
    /// cache or Wayback copy with how old it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stamp: Option<String>,
    /// What the fetcher noted about the page (a Reddit thread's missing comments).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The page's own date when known (its metadata, else the engine's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    /// The earlier page of the run this one mostly repeats, and how much
    /// of it (percent); `research.dedupe` leaves such pages out of synthesis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_as: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlap: Option<u8>,
}

pub struct Run {
    pub id: String,
    pub dir: PathBuf,
    pub round: usize,
    pub recorder: raw::Recorder,
    pages: Vec<PageEntry>,
    /// The highest page number given out (saved or reserved).
    last_id: usize,
}

/// The highest `pN` among saved pages.
fn max_id(pages: &[PageEntry]) -> usize {
    pages
        .iter()
        .filter_map(|p| p.id.strip_prefix('p').and_then(|n| n.parse().ok()))
        .max()
        .unwrap_or(0)
}

/// `2026-11-08_013512_rust-html-parsers`.
fn new_id(topic: &str) -> String {
    let slug: String = topic
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .take(8)
        .collect::<Vec<_>>()
        .join("-");
    let slug: String = slug.chars().take(60).collect();
    format!("{}_{}", local_stamp(), if slug.is_empty() { "run".into() } else { slug })
}

/// Local date and time as `YYYY-MM-DD_HHMMSS`.
pub fn local_stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d_%H%M%S").to_string()
}

impl Run {
    /// A new run folder.
    pub fn create(topic: &str) -> Result<Self, SearchError> {
        let id = new_id(topic);
        let dir = runs_root().join(&id);
        std::fs::create_dir_all(dir.join("pages"))?;
        Ok(Self {
            recorder: raw::Recorder::new(dir.clone(), 1),
            id,
            dir,
            round: 1,
            pages: Vec::new(),
            last_id: 0,
        })
    }

    /// The next round of an existing run (a follow-up).
    pub fn follow(parent: &str) -> Result<Self, SearchError> {
        let dir = resolve(parent)?;
        let id = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let pages = load_pages(&dir);
        let round = (1..)
            .find(|n| !dir.join(format!("request.r{n}.json")).exists())
            .unwrap_or(1);
        let calls = std::fs::read_to_string(dir.join("calls.jsonl"))
            .map(|t| t.lines().count())
            .unwrap_or(0);
        std::fs::create_dir_all(dir.join("pages"))?;
        let recorder = raw::Recorder::new(dir.clone(), calls + 1);
        recorder.set_round(round);
        Ok(Self {
            recorder,
            id,
            dir,
            round,
            last_id: max_id(&pages),
            pages,
        })
    }

    /// An existing run in its current round (the background fetcher).
    pub fn attach(which: &str) -> Result<Self, SearchError> {
        let mut run = Self::follow(which)?;
        run.round = run.round.saturating_sub(1).max(1);
        run.recorder.set_round(run.round);
        Ok(run)
    }

    pub fn pages(&self) -> &[PageEntry] {
        &self.pages
    }

    /// Already fetched in this run (by normalized URL).
    pub fn has_url(&self, url: &str) -> bool {
        let key = crate::engine::normalize_url(url);
        self.pages.iter().any(|p| {
            p.ok && (crate::engine::normalize_url(&p.url) == key
                || crate::engine::normalize_url(&p.final_url) == key)
        })
    }

    /// Page numbers for pages about to be fetched, so they keep their
    /// order (p1 the top result) whatever order the fetches finish in.
    pub fn reserve(&mut self, n: usize) -> Vec<String> {
        (0..n)
            .map(|_| {
                self.last_id += 1;
                format!("p{}", self.last_id)
            })
            .collect()
    }

    pub fn write_json(&self, name: &str, value: &impl Serialize) -> Result<(), SearchError> {
        let text = serde_json::to_string_pretty(value)?;
        std::fs::write(self.dir.join(name), text)?;
        Ok(())
    }

    pub fn write_text(&self, name: &str, text: &str) -> Result<(), SearchError> {
        std::fs::write(self.dir.join(name), text)?;
        Ok(())
    }

    pub fn append_jsonl(&self, name: &str, value: &impl Serialize) {
        append_line(&self.dir.join(name), value);
    }

    /// Save a fetched (or failed) page: the markdown whole in `pages/`, its
    /// line in `pages/index.jsonl`. An entry without a reserved id gets the next.
    pub fn save_page(&mut self, mut entry: PageEntry, markdown: &str) -> PageEntry {
        if entry.id.is_empty() {
            entry.id = self.reserve(1).remove(0);
        }
        entry.round = self.round;
        if entry.ok {
            let _ = std::fs::write(self.dir.join(format!("pages/{}.md", entry.id)), markdown);
        }
        append_line(&self.dir.join("pages/index.jsonl"), &entry);
        self.pages.push(entry.clone());
        entry
    }

    /// The round's line in the runs index.
    pub fn index(&self, line: &RunLine) {
        append_line(&archive_root().join("runs.jsonl"), line);
    }
}

pub fn append_line(path: &Path, value: &impl Serialize) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut line = serde_json::to_string(value).unwrap_or_default();
    line.push('\n');
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Pages saved in a run folder, in order.
pub fn load_pages(dir: &Path) -> Vec<PageEntry> {
    std::fs::read_to_string(dir.join("pages/index.jsonl"))
        .map(|t| {
            t.lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Every run round recorded, oldest first.
pub fn all_runs() -> Vec<RunLine> {
    std::fs::read_to_string(archive_root().join("runs.jsonl"))
        .map(|t| {
            t.lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Run folders that never reached the index (stopped or killed): listed
/// with kind "unfinished" so `osearch runs` still shows them.
pub fn unfinished_runs(indexed: &[RunLine]) -> Vec<RunLine> {
    let known: std::collections::HashSet<&str> = indexed.iter().map(|l| l.id.as_str()).collect();
    let Ok(dir) = std::fs::read_dir(runs_root()) else {
        return Vec::new();
    };
    dir.flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().into_owned();
            if known.contains(id.as_str()) {
                return None;
            }
            let req: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(e.path().join("request.r1.json")).ok()?).ok()?;
            let topic = req.get("topic").and_then(|t| t.as_str()).unwrap_or("").to_string();
            let raw = std::fs::read_dir(e.path().join("raw")).map(|d| d.count()).unwrap_or(0);
            Some(RunLine {
                id,
                round: 1,
                ts: 0,
                topic: format!("{topic} ({raw} engine answers saved)"),
                queries: Vec::new(),
                urls: 0,
                pages: 0,
                cost_usd: 0.0,
                kind: "unfinished".into(),
            })
        })
        .collect()
}

/// A run folder from `last`, an exact id, a unique prefix or a unique
/// substring of an id.
pub fn resolve(which: &str) -> Result<PathBuf, SearchError> {
    let root = runs_root();
    let mut ids: Vec<String> = std::fs::read_dir(&root)
        .map(|d| {
            d.flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    if ids.is_empty() {
        return Err(SearchError::InvalidInput {
            message: format!("no runs yet in {}", root.display()),
        });
    }
    if which.is_empty() || which == "last" {
        return Ok(root.join(ids.last().expect("non-empty")));
    }
    if ids.iter().any(|i| i == which) {
        return Ok(root.join(which));
    }
    let hits: Vec<&String> = ids
        .iter()
        .filter(|i| i.starts_with(which) || i.contains(which))
        .collect();
    match hits.as_slice() {
        [one] => Ok(root.join(one)),
        [] => Err(SearchError::InvalidInput {
            message: format!("no run matches '{which}' (osearch runs lists them)"),
        }),
        many => Err(SearchError::InvalidInput {
            message: format!(
                "'{which}' matches {} runs: {}",
                many.len(),
                many.iter().take(5).map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            ),
        }),
    }
}
