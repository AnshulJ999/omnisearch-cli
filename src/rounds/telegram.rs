//! Telegram beside community and social questions: `site:t.me` on Brave and
//! Jina finds public channels, then the two most found are searched inside
//! for the question's words.

use super::{local_response, plan_side, Responses, RoundOut, Slots};
use crate::context::AppContext;
use crate::research::{Planned, QuerySpec, Request};
use crate::run::raw;
use crate::types::Mode;
use std::sync::Arc;

/// The channel-finding search, planned with the question's own.
pub fn side(app: &Arc<AppContext>, req: &Request, statuses: &[crate::budget::Status], p: &Planned) -> Vec<Planned> {
    let spec = QuerySpec {
        q: format!("site:t.me {}", p.query),
        mode: Some("general".into()),
        engines: vec!["brave".into(), "jina".into()],
        count: Some(10),
        ..QuerySpec::default()
    };
    plan_side(app, req, statuses, p, "n", spec, slots(app)).into_iter().collect()
}

/// Telegram's pages get a guaranteed slot only when `research.telegramslot` is on.
fn slots(app: &AppContext) -> Slots {
    Slots { extra: false, cap: None, guaranteed: app.config.research.telegramslot }
}

/// After the first searches: a search inside the channels they found.
pub async fn round(
    app: &Arc<AppContext>,
    recorder: &raw::Recorder,
    planned: &[Planned],
    responses: &Responses,
) -> RoundOut {
    let mut out = RoundOut::default();
    let mut set = tokio::task::JoinSet::new();
    for p in planned.iter().filter(|p| !p.side && matches!(p.mode, Mode::Community | Mode::Social)) {
        let mut channels: Vec<(String, usize)> = Vec::new();
        if let Some(Ok(r)) = responses.get(&format!("{}n", p.label)) {
            for it in r.per_provider.iter().flat_map(|(_, v)| v.iter()).chain(r.results.iter()) {
                if let Some((ch, _)) = crate::providers::telegram::channel_of(&it.url) {
                    match channels.iter_mut().find(|(c, _)| c.eq_ignore_ascii_case(&ch)) {
                        Some(c) => c.1 += 1,
                        None => channels.push((ch, 1)),
                    }
                }
            }
        }
        channels.sort_by(|a, b| b.1.cmp(&a.1));
        let words = super::carrying_words(&p.query, 2);
        if words.is_empty() {
            continue;
        }
        for (i, (ch, _)) in channels.into_iter().take(2).enumerate() {
            let label = format!("{}n{}", p.label, i + 1);
            let q = format!("t.me/{ch}: {words}");
            out.planned.push(Planned::local(p, label.clone(), q.clone(), Mode::Community, "telegram", slots(app)));
            let (app, words) = (app.clone(), words.clone());
            let rec = Some(recorder.labeled(&label));
            set.spawn(raw::scope(rec, async move {
                let r = crate::providers::telegram::search(&app, &ch, &words)
                    .await
                    .map(|list| local_response("telegram", &q, Mode::Community, list))
                    .map_err(|e| crate::errors::SearchError::Config(format!("Telegram search in {ch} failed: {e}")));
                (label, r)
            }));
        }
    }
    while let Some(joined) = set.join_next().await {
        if let Ok(pair) = joined {
            out.responses.push(pair);
        }
    }
    out
}
