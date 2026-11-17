//! Every provider answer and outside-tool output of a run, saved untouched in
//! `raw/`, with one line per call in `calls.jsonl` (provider, endpoint,
//! request, status, time, bytes, raw file). Active only inside a run: the
//! recorder rides in a task-local, carried into spawned tasks by [`scope`].

use serde::Serialize;
use std::fs::OpenOptions;
use std::future::Future;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
pub struct CallRec {
    pub n: usize,
    /// Which part of the run made the call: `q2` (query 2), `fetch p3`, ...
    pub label: String,
    pub provider: String,
    pub kind: &'static str,
    pub endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    pub status: Option<i32>,
    pub elapsed_ms: u128,
    pub bytes: usize,
    pub raw: String,
    pub ts: u64,
}

struct Inner {
    dir: PathBuf,
    seq: AtomicUsize,
    calls: Mutex<Vec<CallRec>>,
    /// The run round this recorder belongs to (per-round use counts).
    round: AtomicUsize,
    /// Serializes the use-count file within this process.
    uses: Mutex<()>,
}

/// The run's recorder, with the label of the work it is attached to.
#[derive(Clone)]
pub struct Recorder {
    inner: Arc<Inner>,
    label: Arc<str>,
}

impl Recorder {
    /// `dir` is the run folder; `first` the next call number (a follow-up
    /// round continues the numbering).
    pub fn new(dir: PathBuf, first: usize) -> Self {
        let _ = std::fs::create_dir_all(dir.join("raw"));
        Self {
            inner: Arc::new(Inner {
                dir,
                seq: AtomicUsize::new(first),
                calls: Mutex::new(Vec::new()),
                round: AtomicUsize::new(1),
                uses: Mutex::new(()),
            }),
            label: Arc::from(""),
        }
    }

    pub fn set_round(&self, round: usize) {
        self.inner.round.store(round.max(1), Ordering::SeqCst);
    }

    /// Count `n` uses of `kind` in this round's `use.rN.json`; false (nothing
    /// counted) when that would pass `max`. Per round, not per process, so a
    /// run's background finisher shares the count.
    pub fn take_use(&self, kind: &str, n: usize, max: usize) -> bool {
        let _guard = self.inner.uses.lock().unwrap_or_else(|e| e.into_inner());
        let path = self.inner.dir.join(format!("use.r{}.json", self.inner.round.load(Ordering::SeqCst)));
        let mut counts: serde_json::Map<String, serde_json::Value> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let used = counts.get(kind).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        if used + n > max {
            return false;
        }
        counts.insert(kind.to_string(), serde_json::json!(used + n));
        let _ = std::fs::write(&path, serde_json::Value::Object(counts).to_string());
        true
    }

    pub fn labeled(&self, label: &str) -> Self {
        Self {
            inner: self.inner.clone(),
            label: Arc::from(label),
        }
    }

    pub fn calls(&self) -> Vec<CallRec> {
        self.inner
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn save(
        &self,
        provider: &str,
        kind: &'static str,
        endpoint: String,
        request: Option<String>,
        status: Option<i32>,
        elapsed: Duration,
        body: &[u8],
        ext: &str,
    ) -> String {
        let n = self.inner.seq.fetch_add(1, Ordering::SeqCst);
        let safe: String = provider
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
            .collect();
        let raw = format!("raw/{n:04}_{safe}.{ext}");
        let _ = std::fs::write(self.inner.dir.join(&raw), body);
        let rec = CallRec {
            n,
            label: self.label.to_string(),
            provider: provider.to_string(),
            kind,
            endpoint: crate::errors::redact_secrets(&endpoint),
            request: request.map(|r| crate::errors::redact_secrets(&clip(&r, 4000))),
            status,
            elapsed_ms: elapsed.as_millis(),
            bytes: body.len(),
            raw: raw.clone(),
            ts: now(),
        };
        let mut line = serde_json::to_string(&rec).unwrap_or_default();
        line.push('\n');
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.inner.dir.join("calls.jsonl"))
        {
            let _ = f.write_all(line.as_bytes());
        }
        self.inner
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(rec);
        raw
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

tokio::task_local! {
    static REC: Option<Recorder>;
}

/// The recorder of the current task, if it runs inside a run.
pub fn current() -> Option<Recorder> {
    REC.try_with(|r| r.clone()).ok().flatten()
}

/// Run `fut` with `rec` as its recorder.
pub fn scope<F: Future>(rec: Option<Recorder>, fut: F) -> impl Future<Output = F::Output> {
    REC.scope(rec, fut)
}

fn ext_for(content_type: Option<&str>, body: &[u8]) -> &'static str {
    let ct = content_type.unwrap_or("").to_ascii_lowercase();
    let head = body.iter().find(|b| !b.is_ascii_whitespace()).copied();
    if ct.contains("json") || matches!(head, Some(b'{') | Some(b'[')) {
        "json"
    } else if ct.contains("html") || head == Some(b'<') {
        "html"
    } else if ct.contains("markdown") {
        "md"
    } else {
        "txt"
    }
}

/// One HTTP call to a provider, its answer body untouched.
pub fn record_http(
    provider: &str,
    method: &str,
    url: &str,
    request: Option<&[u8]>,
    status: u16,
    elapsed: Duration,
    content_type: Option<&str>,
    body: &[u8],
) -> Option<String> {
    let rec = current()?;
    let req = request.map(|b| String::from_utf8_lossy(b).into_owned());
    Some(rec.save(
        provider,
        "http",
        format!("{method} {url}"),
        req,
        Some(status as i32),
        elapsed,
        body,
        ext_for(content_type, body),
    ))
}

/// One run of an outside tool: its stdout untouched, stderr in the request
/// field's place when it failed.
pub fn record_tool(label: &str, args: &[String], ran: &crate::tools::Ran) {
    let Some(rec) = current() else { return };
    let note = (ran.code != Some(0) && !ran.stderr.trim().is_empty())
        .then(|| format!("stderr: {}", ran.stderr.trim()));
    let trimmed = (TRIM_YTDLP_CAPTIONS && label == "yt-dlp").then(|| trim_captions(&ran.stdout)).flatten();
    let body = trimmed.as_deref().unwrap_or(&ran.stdout);
    rec.save(
        label,
        "tool",
        format!("{label} {}", args.join(" ")),
        note,
        ran.code,
        ran.elapsed,
        body,
        ext_for(None, body),
    );
}

/// yt-dlp lists a caption link for every machine-translated language, most
/// of its answer; only the tracks osearch reads are kept in the archive.
const TRIM_YTDLP_CAPTIONS: bool = true;

fn trim_captions(stdout: &[u8]) -> Option<Vec<u8>> {
    let mut v: serde_json::Value = serde_json::from_slice(stdout).ok()?;
    let caps = v.get_mut("automatic_captions")?.as_object_mut()?;
    caps.retain(|k, _| k == "en" || k.starts_with("en-") || k.starts_with("en_") || k.ends_with("-orig"));
    serde_json::to_vec(&v).ok()
}

/// A page fetch by OmniSearch's own fetchers (direct, stealth, browser):
/// the body as received.
pub fn record_page(fetcher: &str, url: &str, status: u16, elapsed: Duration, body: &str) {
    record_page_as(fetcher, url, status, elapsed, body, None);
}

/// [`record_page`] with a note kept in the call's request field (the Reddit
/// login account a call went through).
pub fn record_page_as(fetcher: &str, url: &str, status: u16, elapsed: Duration, body: &str, note: Option<String>) {
    let Some(rec) = current() else { return };
    rec.save(
        fetcher,
        "page",
        format!("GET {url}"),
        note,
        Some(status as i32),
        elapsed,
        body.as_bytes(),
        ext_for(None, body.as_bytes()),
    );
}
