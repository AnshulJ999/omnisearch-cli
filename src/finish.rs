//! Pages a run did not wait for: handed to a separate `osearch fetch
//! --finish RUN@ROUND` process that fetches them into the same run folder
//! (their page numbers were given when they were picked), each continuing
//! from the next fetcher it had not tried. Each round of a run has its own
//! marker files (`pending.rN.json`, `fetch.rN.running`), so a follow-up round
//! never touches the pages another round is still fetching. `osearch wait`
//! waits for every round's process and lists what came in.

use crate::context::AppContext;
use crate::fetch::{Attempt, FetchOpts};
use crate::research::{fetch_all, Limits, Target};
use crate::run::Run;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// How long the background process keeps at it.
pub const MAX: Duration = Duration::from_secs(300);

/// The pages a round left to the background.
fn file(round: usize) -> String {
    format!("pending.r{round}.json")
}

/// Present while that round's background process works.
fn running(round: usize) -> String {
    format!("fetch.r{round}.running")
}

/// The rounds of a run folder that have a background marker, in order.
fn rounds_in(dir: &std::path::Path) -> Vec<usize> {
    let mut rounds: Vec<usize> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_prefix("fetch.r")?.strip_suffix(".running")?.parse().ok()
        })
        .collect();
    rounds.sort_unstable();
    rounds
}

/// Whether any round of the run still has a background fetch going.
pub fn is_running(dir: &std::path::Path) -> bool {
    !rounds_in(dir).is_empty()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pending {
    pub id: String,
    pub url: String,
    pub from: String,
    /// Fetchers already tried for it in the run.
    #[serde(default)]
    pub tried: Vec<Attempt>,
    /// The date the engine gave the result, when it gave one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Handoff {
    #[serde(default)]
    platforms: Vec<String>,
    #[serde(default)]
    no_paid: bool,
    #[serde(default)]
    links: bool,
    #[serde(default)]
    fresh: bool,
    pages: Vec<Pending>,
}

/// Write the pages left and start the background process. `Err` leaves
/// nothing behind: the caller lists the pages as not fetched.
pub fn hand_off(run: &Run, pending: &[Pending], opts: &FetchOpts) -> Result<(), String> {
    let h = Handoff {
        platforms: opts.platforms.clone(),
        no_paid: opts.no_paid,
        links: opts.links,
        fresh: opts.fresh,
        pages: pending.to_vec(),
    };
    let text = serde_json::to_string_pretty(&h).map_err(|e| e.to_string())?;
    std::fs::write(run.dir.join(file(run.round)), text).map_err(|e| e.to_string())?;
    let _ = std::fs::write(run.dir.join(running(run.round)), chrono::Local::now().to_rfc3339());
    crate::synth::spawn_self(&["fetch", "--finish", &format!("{}@{}", run.id, run.round)]).inspect_err(|_| {
        let _ = std::fs::remove_file(run.dir.join(file(run.round)));
        let _ = std::fs::remove_file(run.dir.join(running(run.round)));
    })
}

/// The pages still being fetched in the background for a run folder, every round.
pub fn pending_in(dir: &std::path::Path) -> Vec<Pending> {
    rounds_in(dir)
        .into_iter()
        .filter_map(|r| std::fs::read_to_string(dir.join(file(r))).ok())
        .filter_map(|t| serde_json::from_str::<Handoff>(&t).ok())
        .flat_map(|h| h.pages)
        .collect()
}

/// `osearch fetch --finish RUN@ROUND`: the background process.
pub async fn finish(app: Arc<AppContext>, which: &str) -> Result<i32, crate::errors::SearchError> {
    let (id, round) = match which.split_once('@') {
        Some((id, r)) => (id, r.parse::<usize>().ok()),
        None => (which, None),
    };
    let mut run = Run::attach(id)?;
    if let Some(r) = round {
        run.round = r;
        run.recorder.set_round(r);
    }
    let text = std::fs::read_to_string(run.dir.join(file(run.round)))?;
    let h: Handoff = serde_json::from_str(&text)?;
    let opts = FetchOpts {
        platforms: h.platforms,
        no_paid: h.no_paid,
        links: h.links,
        fresh: h.fresh,
        ..FetchOpts::default()
    };
    let targets: Vec<Target> = h
        .pages
        .into_iter()
        .map(|p| Target {
            url: p.url,
            from: p.from,
            id: p.id,
            // Only fetchers that failed: one that worked is never skipped.
            skip: p.tried.iter().filter(|a| !a.outcome.starts_with("ok")).map(|a| a.fetcher.clone()).collect(),
            tried: p.tried,
            late: true,
            date: p.date,
        })
        .collect();
    let recorder = run.recorder.clone();
    let limits = Limits {
        hard: Some(MAX),
        ..Limits::default()
    };
    let (_, left) = fetch_all(&app, &recorder, &mut run, targets, &opts, &limits).await;
    // A page that did not finish in time still gets its entry, so its number
    // shows up as not fetched with the command to retry.
    let run_id = run.id.clone();
    crate::research::save_unfinished(&mut run, left, true, |p| {
        format!("not finished in the background (it stopped after {} s); retry: osearch fetch {} --parent {run_id}", MAX.as_secs(), p.url)
    });
    let _ = std::fs::remove_file(run.dir.join(file(run.round)));
    let _ = std::fs::remove_file(run.dir.join(running(run.round)));
    Ok(0)
}

/// `osearch fetch --wait --run RUN`: wait for the background pages, then
/// list them.
pub async fn wait(which: &str) -> Result<String, crate::errors::SearchError> {
    let dir = crate::run::resolve(which)?;
    let waited: Vec<String> = pending_in(&dir).into_iter().map(|p| p.id).collect();
    let started = std::time::Instant::now();
    while is_running(&dir) && started.elapsed() < MAX + Duration::from_secs(30) {
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    Ok(report(&dir, &waited))
}

/// What came of the pages a run left to the background.
pub fn report(dir: &std::path::Path, waited: &[String]) -> String {
    let id = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let pages = crate::run::load_pages(dir);
    let late: Vec<&crate::run::PageEntry> = pages.iter().filter(|p| p.late && p.fetcher != "jina-deepsearch").collect();
    let mut out = format!("run {id}\n");
    if is_running(dir) {
        out.push_str("still fetching after the wait; run this again later\n");
    }
    if late.is_empty() && waited.is_empty() {
        out.push_str("no pages were left to the background in this run\n");
    }
    for p in late {
        if p.ok {
            out.push_str(&format!(
                "{} ok · {} · {} tok · {} · {}\n",
                p.id,
                crate::providers::thread::clip(&p.title, 80),
                p.tokens,
                p.fetcher,
                p.url
            ));
        } else {
            out.push_str(&format!(
                "{} not fetched ({}) · {}\n",
                p.id,
                p.failure.as_deref().unwrap_or("error"),
                p.url
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handoff(id: &str) -> String {
        let page = Pending { id: id.into(), url: "https://example.com".into(), from: "q1".into(), tried: Vec::new(), date: None };
        serde_json::to_string(&Handoff { platforms: Vec::new(), no_paid: false, links: false, fresh: false, pages: vec![page] }).unwrap()
    }

    /// A follow-up round's hand-off must not hide, or be hidden by, the first round's.
    #[test]
    fn each_round_keeps_its_own_background_pages() {
        let dir = std::env::temp_dir().join(format!("osearch-test-finish-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (round, id) in [(1, "p1"), (2, "p2")] {
            std::fs::write(dir.join(file(round)), handoff(id)).unwrap();
            std::fs::write(dir.join(running(round)), "x").unwrap();
        }
        assert_eq!(pending_in(&dir).len(), 2);
        std::fs::remove_file(dir.join(running(1))).unwrap();
        assert!(is_running(&dir));
        let ids: Vec<String> = pending_in(&dir).into_iter().map(|p| p.id).collect();
        assert_eq!(ids, ["p2"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
