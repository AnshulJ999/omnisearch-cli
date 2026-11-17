//! A fetched page to markdown, with two loss guards: when article extraction
//! drops the numbers and prices the page shows (fatsearch's loss check,
//! simplified), or the replies of a discussion page, the whole page is kept
//! instead.

use super::{Got, Miss, TierResult};
use crate::page::{self, Page};
use regex::Regex;
use serde::Serialize;
use std::collections::BTreeSet;
use std::sync::LazyLock;

/// What the loss guard measured on one page.
#[derive(Debug, Clone, Serialize)]
pub struct Loss {
    /// Distinct number facts (prices, decimals, grouped thousands, percents)
    /// in the page's visible text.
    pub facts: usize,
    /// Of those, missing from the article extraction.
    pub lost_by_article: usize,
    /// True when the whole page was kept instead of the article.
    pub kept_whole_page: bool,
    /// The whole page was kept because the article dropped a discussion's replies.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replies: bool,
}

/// Repeated post or comment containers: a discussion page (forum thread,
/// comment section, GitHub discussion).
static REPLY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"class="[^"]*\b(?:comment|timeline-comment|message--post|topic-post|reply|postbody|post-body)\b"#)
        .expect("reply pattern")
});

fn words(text: &str) -> usize {
    text.split_whitespace().filter(|w| w.chars().any(char::is_alphabetic)).count()
}

static FACT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
        [$€£₹¥]\s?\d[\d,]*(?:\.\d+)?        # a price
        | \d[\d,]*(?:\.\d+)?\s?%              # a percent
        | \b\d{1,3}(?:,\d{3})+(?:\.\d+)?\b    # grouped thousands
        | \b\d+\.\d+\b                        # a decimal
        ",
    )
    .expect("fact pattern")
});

fn facts(text: &str) -> BTreeSet<String> {
    FACT.find_iter(text)
        .map(|m| m.as_str().split_whitespace().collect::<String>())
        .collect()
}

/// Facts of `visible` missing from `markdown` (whitespace ignored).
fn missing(visible: &BTreeSet<String>, markdown: &str) -> usize {
    let squashed: String = markdown.split_whitespace().collect();
    visible.iter().filter(|f| !squashed.contains(f.as_str())).count()
}

/// HTML (or markdown, or text) of one page to markdown: the raw verdict,
/// the conversion, the loss guard.
pub async fn from_page(p: Page) -> TierResult {
    tokio::task::spawn_blocking(move || convert(p))
        .await
        .map_err(|e| (Miss::Other, format!("conversion panicked: {e}")))?
}

/// Expressive Code (Astro and Starlight docs) puts each code line in its
/// own `<div class="ec-line">` with no newline between them, so a code block
/// converts to one line. Give every such line its newline back.
fn restore_code_lines(mut p: Page) -> Page {
    if p.body.contains("ec-line") {
        p.body = p
            .body
            .replace("<div class=\"ec-line", "\n<div class=\"ec-line");
    }
    p
}

fn convert(p: Page) -> TierResult {
    let p = restore_code_lines(p);
    let extracted = match page::extract(&p) {
        Ok(ex) => ex,
        Err(v) => return Err((Miss::from_verdict(&v), format!("{} at {}", v.code(), p.final_url))),
    };
    let is_html = !p.is_markdown()
        && p.content_type
            .as_deref()
            .map_or(true, |c| c.to_ascii_lowercase().contains("html"));
    let mut markdown = extracted.markdown;
    let mut loss = None;
    if is_html {
        let visible = facts(&page::visible_text(&p.body));
        let lost = missing(&visible, &markdown);
        if visible.len() >= 5 {
            let mut whole = false;
            if lost >= 5 && lost * 5 >= visible.len() {
                let (_, md) = page::whole_page_markdown(&p.body, &p.final_url);
                if missing(&visible, &md) < lost {
                    markdown = md;
                    whole = true;
                }
            }
            loss = Some(Loss {
                facts: visible.len(),
                lost_by_article: lost,
                kept_whole_page: whole,
                replies: false,
            });
        }
        // A discussion whose article kept the first post only.
        let seen = words(&page::visible_text(&p.body));
        let kept = words(&markdown);
        if seen >= 300 && kept * 100 < seen * 35 && REPLY.find_iter(&p.body).take(6).count() >= 6 {
            let (_, md) = page::whole_page_markdown(&p.body, &p.final_url);
            if words(&md) >= kept * 2 {
                markdown = md;
                let l = loss.get_or_insert(Loss { facts: 0, lost_by_article: 0, kept_whole_page: true, replies: false });
                l.kept_whole_page = true;
                l.replies = true;
            }
        }
    }
    Ok(Got {
        stamp: None,
        note: None,
        published: extracted.published,
        final_url: p.final_url,
        title: extracted.title,
        markdown,
        loss,
        cost_micros: 0,
    })
}

/// Rendered HTML from a browser tier.
pub async fn from_html(final_url: &str, html: String) -> TierResult {
    let p = Page {
        final_url: final_url.to_string(),
        status: 200,
        content_type: Some("text/html".to_string()),
        body: html,
        headers: reqwest::header::HeaderMap::new(),
    };
    from_page(p).await
}
