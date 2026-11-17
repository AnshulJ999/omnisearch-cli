//! Query-centric relevance of a result's text (ported from last30days'
//! `relevance.py`, MIT, mvanhorn/last30days-skill): exact phrases score
//! high, partial matches pay a penalty, and matches on generic words alone
//! ("review", "best") stay below the keep line.

use crate::types::SearchResult;
use std::collections::HashSet;

/// Results scoring below this are off-topic.
pub const FLOOR: f64 = 0.25;

const STOP: &[&str] = &[
    "the", "a", "an", "to", "for", "how", "is", "in", "of", "on", "and", "with", "from", "by", "at", "this", "that",
    "it", "my", "your", "i", "me", "we", "you", "what", "are", "do", "can", "its", "be", "or", "not", "no", "so",
    "if", "but", "about", "all", "just", "get", "has", "have", "was", "will",
];

const LOW_SIGNAL: &[&str] = &[
    "advice", "best", "chance", "chances", "code", "compare", "comparison", "differences", "explain", "guide",
    "guides", "how", "latest", "news", "odds", "opinion", "opinions", "prediction", "predictions", "prompt",
    "prompts", "rate", "review", "reviews", "thoughts", "tip", "tips", "tutorial", "tutorials", "update",
    "updates", "use", "using", "versus", "vs", "worth", "current", "days", "evidence", "experience", "experiences",
    "last", "most", "new", "people", "real", "recent", "relevant", "good", "think", "say", "reddit", "forum",
    "forums", "discussion", "threads", "legit", "paid",
];

const SYNONYMS: &[(&str, &[&str])] = &[
    ("js", &["javascript"]),
    ("javascript", &["js"]),
    ("ts", &["typescript"]),
    ("typescript", &["ts"]),
    ("ai", &["artificial", "intelligence"]),
    ("ml", &["machine", "learning"]),
    ("react", &["reactjs"]),
    ("reactjs", &["react"]),
    ("vue", &["vuejs"]),
    ("vuejs", &["vue"]),
];

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

fn tokenize(text: &str) -> HashSet<String> {
    let base: HashSet<String> = words(text).into_iter().filter(|w| w.chars().count() > 1 && !STOP.contains(&w.as_str())).collect();
    let mut out = base.clone();
    for t in &base {
        if let Some((_, more)) = SYNONYMS.iter().find(|(k, _)| k == t) {
            out.extend(more.iter().map(|m| m.to_string()));
        }
    }
    out
}

/// A query prepared once and scored against many texts.
pub struct Query {
    tokens: HashSet<String>,
    informative: HashSet<String>,
    phrase: String,
}

impl Query {
    pub fn new(query: &str) -> Self {
        let tokens = tokenize(query);
        let informative: HashSet<String> = tokens.iter().filter(|t| !LOW_SIGNAL.contains(&t.as_str())).cloned().collect();
        Self {
            informative: if informative.is_empty() { tokens.clone() } else { informative },
            tokens,
            phrase: words(query).join(" "),
        }
    }

    /// 0.0 to 1.0; 0.5 when the query has no usable words.
    pub fn score(&self, text: &str) -> f64 {
        if self.tokens.is_empty() {
            return 0.5;
        }
        let t = tokenize(text);
        let overlap = self.tokens.intersection(&t).count();
        if overlap == 0 {
            return 0.0;
        }
        let coverage = overlap as f64 / self.tokens.len() as f64;
        let informative = self.informative.intersection(&t).count() as f64 / self.informative.len() as f64;
        let precision = overlap as f64 / (t.len().min(self.tokens.len() + 4).max(1)) as f64;
        let base = 0.55 * coverage.powf(1.35) + 0.25 * informative + 0.20 * precision;
        if self.informative.intersection(&t).next().is_none() {
            return base.min(0.24);
        }
        let bonus = if !self.phrase.is_empty() && words(text).join(" ").contains(&self.phrase) {
            if self.phrase.contains(' ') { 0.12 } else { 0.16 }
        } else {
            0.0
        };
        (base + bonus).min(1.0)
    }
}

/// The results whose title and snippet are on topic for `query`.
pub fn keep(results: Vec<SearchResult>, query: &str) -> Vec<SearchResult> {
    let q = Query::new(query);
    results.into_iter().filter(|r| q.score(&format!("{} {}", r.title, r.snippet)) >= FLOOR).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Cases from the field log: generic words alone must not pass; the
    // topic's own words must.
    #[test]
    fn generic_words_alone_stay_below_the_floor() {
        let q = Query::new("get paid for my AI chat logs");
        assert!(q.score("Best Rasa forum tips: get paid faster") < FLOOR);
        assert!(q.score("Selling AI chat logs: who pays for chat transcripts") >= FLOOR);
    }
}
