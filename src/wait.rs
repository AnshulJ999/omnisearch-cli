//! `osearch wait RUN`: wait for a run's background work (late pages, a
//! background synthesis), then print what landed. Run as a background
//! command, its exit is the notice. `--jina-deepsearch` waits for the run's
//! Jina DeepSearch instead.

use crate::errors::SearchError;
use std::time::{Duration, Instant};

pub async fn command(which: Option<String>, jina: bool) -> Result<i32, SearchError> {
    let dir = crate::run::resolve(which.as_deref().unwrap_or("last"))?;
    if jina {
        print!("{}", crate::jinadeep::wait(&dir).await);
        return Ok(0);
    }
    let waited: Vec<String> = crate::finish::pending_in(&dir).into_iter().map(|p| p.id).collect();
    let synth = dir.join(crate::synth::RUNNING).exists();
    let limit = crate::finish::MAX.max(Duration::from_secs(crate::config::get().synth.timeout)) + Duration::from_secs(30);
    let started = Instant::now();
    while (crate::finish::is_running(&dir) || dir.join(crate::synth::RUNNING).exists()) && started.elapsed() < limit {
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let mut out = crate::finish::report(&dir, &waited);
    if synth {
        out.push_str(&crate::synth::ready_text(&dir));
    }
    print!("{out}");
    Ok(0)
}
