//! Searches osearch adds beside a request's own queries: Reddit routes,
//! forums, GitHub angles and "alternatives to X". Side queries planned up
//! front run with the request's own; rounds run after them, on what they found.

pub mod alternatives;
pub mod forums;
pub mod github;
pub mod reddit;
pub mod telegram;

use crate::context::AppContext;
use crate::research::{Planned, QuerySpec, Request};
use crate::types::{Mode, ResponseMetadata, SearchResponse, SearchResult};
use std::collections::BTreeMap;
use std::sync::Arc;

pub type Answer = Result<SearchResponse, crate::errors::SearchError>;
pub type Responses = BTreeMap<String, Answer>;

/// What one round adds: its queries (listed in the summary), their answers
/// and notes for the summary's warnings.
#[derive(Default)]
pub struct RoundOut {
    pub planned: Vec<Planned>,
    pub responses: Vec<(String, Answer)>,
    pub notes: Vec<String>,
    /// Routine facts for the summary's coverage section (not warnings).
    pub coverage: Vec<String>,
}

impl RoundOut {
    pub fn merge(&mut self, other: RoundOut) {
        self.planned.extend(other.planned);
        self.responses.extend(other.responses);
        self.notes.extend(other.notes);
        self.coverage.extend(other.coverage);
    }
}

/// How a side query counts for the fetch slots.
#[derive(Clone, Copy)]
pub struct Slots {
    /// Another kind of content than its parent (GitHub beside a web
    /// question): shares at most a third of the run's pages.
    pub extra: bool,
    /// At most this many pages from it.
    pub cap: Option<usize>,
    /// Gets one page before rank decides (same-kind side searches).
    pub guaranteed: bool,
}

pub const SAME: Slots = Slots { extra: false, cap: None, guaranteed: true };

/// A side query planned like a request's own: engines checked against
/// their modes and balance, cost estimated.
pub fn plan_side(
    app: &Arc<AppContext>,
    req: &Request,
    statuses: &[crate::budget::Status],
    parent: &Planned,
    suffix: &str,
    spec: QuerySpec,
    slots: Slots,
) -> Option<Planned> {
    let label = format!("{}{suffix}", parent.label);
    let mut p = crate::research::plan_query(app, label, &spec, req, statuses).ok()?;
    p.side = true;
    p.extra = slots.extra;
    p.cap = slots.cap;
    p.guaranteed = slots.guaranteed;
    p.width = parent.width;
    Some(p)
}

/// A list osearch found itself (no engine call) as a response.
pub fn local_response(engine: &str, query: &str, mode: Mode, results: Vec<SearchResult>) -> SearchResponse {
    let n = results.len();
    let mut metadata = ResponseMetadata {
        result_count: n,
        providers_queried: vec![engine.to_string()],
        ..ResponseMetadata::default()
    };
    metadata.provider_results.insert(engine.to_string(), n);
    SearchResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        status: if n == 0 { "no_results" } else { "success" }.to_string(),
        query: query.to_string(),
        mode: mode.to_string(),
        per_provider: vec![(engine.to_string(), results.clone())],
        results,
        answers: Vec::new(),
        metadata,
    }
}

/// Labels that belong to query `parent`: itself and its side queries
/// (`q1r`, `q1a2`, `q1f`), never `q12`.
pub fn belongs(label: &str, parent: &str) -> bool {
    label == parent
        || label
            .strip_prefix(parent)
            .and_then(|rest| rest.chars().next())
            .is_some_and(|c| c.is_ascii_alphabetic())
}

/// Every result that query `parent` and its side queries found.
pub fn results_of<'a>(parent: &str, responses: &'a Responses) -> Vec<&'a SearchResult> {
    responses
        .iter()
        .filter(|(label, _)| belongs(label, parent))
        .filter_map(|(_, r)| r.as_ref().ok())
        .flat_map(|r| r.per_provider.iter().flat_map(|(_, v)| v.iter()).chain(r.results.iter()))
        .collect()
}

/// Words that carry a query, at most `n`.
pub fn carrying_words(query: &str, n: usize) -> String {
    const FILLER: &[&str] = &[
        "a", "an", "the", "in", "on", "for", "with", "of", "to", "and", "or", "is", "are", "what",
        "how", "do", "does", "people", "say", "about", "best", "good", "reddit", "think", "opinions",
        "experience", "experiences", "worth", "it", "vs", "which", "my", "i", "should", "any",
    ];
    query
        .split_whitespace()
        .filter(|w| {
            let lower = w.to_lowercase();
            !FILLER.contains(&lower.trim_matches(|c: char| !c.is_alphanumeric()))
        })
        .take(n)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Lowercase letters and digits only.
pub fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}
