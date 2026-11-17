//! The reading model (after fatsearch, Apache-2.0): every page is kept whole
//! and cut into sections at its headings; each section has an id and a
//! token count, so an agent reads the outline first and then one section
//! (`read p3#4`) instead of the whole page. A page with fewer than two
//! headings is cut into blocks of about [`CHUNK`] tokens ("part N").

use serde::{Deserialize, Serialize};

/// Block size for heading-less text, in tokens.
pub const CHUNK: usize = 400;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    pub id: usize,
    pub level: usize,
    pub title: String,
    /// Line range in the page file (0-based, end exclusive), headings
    /// included.
    pub start: usize,
    pub end: usize,
    /// This section's own text, without subsections.
    pub tokens: usize,
}

/// Token estimate for Claude-family tokenizers: ASCII letters count about
/// 1/2.7 of a token, every other visible character about 1/0.8 (fatsearch's
/// fitted `tok`).
pub fn tokens(s: &str) -> usize {
    let mut prev = ' ';
    let mut w = 0usize;
    for c in s.chars() {
        let mark = |x: char| x.is_ascii_punctuation();
        let letter = c.is_ascii_alphabetic() || (mark(prev) && mark(c));
        prev = c;
        if !c.is_whitespace() {
            w += if letter { 80 } else { 270 };
        }
    }
    w.div_ceil(216)
}

fn heading(line: &str) -> Option<(usize, String)> {
    let t = line.trim_start();
    let level = t.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &t[level..];
    if !rest.starts_with(' ') {
        return None;
    }
    let title = rest.trim().trim_end_matches('#').trim().to_string();
    (!title.is_empty() && title.chars().count() <= 160).then_some((level, title))
}

/// Sections of a markdown page. Headings inside code fences do not count.
pub fn sections(markdown: &str) -> Vec<Section> {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut heads: Vec<(usize, usize, String)> = Vec::new();
    let mut fence: Option<&str> = None;
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim_start();
        let marker = ["```", "~~~"].into_iter().find(|m| t.starts_with(m));
        if let Some(open) = fence {
            if marker == Some(open) {
                fence = None;
            }
            continue;
        }
        if marker.is_some() {
            fence = marker;
            continue;
        }
        if let Some((lv, title)) = heading(l) {
            heads.push((i, lv, title));
        }
    }
    let mut out: Vec<Section> = Vec::new();
    if heads.len() >= 2 {
        if heads[0].0 > 0 {
            let text = lines[..heads[0].0].join("\n");
            if !text.trim().is_empty() {
                out.push(Section {
                    id: 0,
                    level: 1,
                    title: "(top)".into(),
                    start: 0,
                    end: heads[0].0,
                    tokens: tokens(&text),
                });
            }
        }
        for (k, (start, lv, title)) in heads.iter().enumerate() {
            let end = heads.get(k + 1).map_or(lines.len(), |h| h.0);
            out.push(Section {
                id: 0,
                level: *lv,
                title: title.clone(),
                start: *start,
                end,
                tokens: tokens(&lines[*start..end].join("\n")),
            });
        }
    } else {
        let mut start = 0;
        let mut acc = 0;
        for (i, l) in lines.iter().enumerate() {
            acc += tokens(l);
            if acc >= CHUNK || i + 1 == lines.len() {
                out.push(Section {
                    id: 0,
                    level: 1,
                    title: String::new(),
                    start,
                    end: i + 1,
                    tokens: acc,
                });
                start = i + 1;
                acc = 0;
            }
        }
        let n = out.len();
        for (k, s) in out.iter_mut().enumerate() {
            s.title = if n == 1 {
                "(whole page)".into()
            } else {
                format!("part {}", k + 1)
            };
        }
    }
    for (k, s) in out.iter_mut().enumerate() {
        s.id = k + 1;
    }
    out
}

/// Section `id` and its subsections: the line range to print.
pub fn span(secs: &[Section], id: usize) -> Option<(usize, usize)> {
    let i = secs.iter().position(|s| s.id == id)?;
    let lv = secs[i].level;
    let end = secs[i + 1..]
        .iter()
        .take_while(|s| s.level > lv)
        .last()
        .map_or(secs[i].end, |s| s.end);
    Some((secs[i].start, end))
}

/// Tokens of section `id` with its subsections.
pub fn span_tokens(secs: &[Section], id: usize) -> usize {
    let Some(i) = secs.iter().position(|s| s.id == id) else {
        return 0;
    };
    let lv = secs[i].level;
    secs[i].tokens
        + secs[i + 1..]
            .iter()
            .take_while(|s| s.level > lv)
            .map(|s| s.tokens)
            .sum::<usize>()
}

/// "12.3k" / "840".
pub fn ktok(n: usize) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

/// The outline as text: one line per section, indented by level, with its
/// id and size (subsections included).
pub fn render(page_id: &str, title: &str, url: &str, secs: &[Section]) -> String {
    let total: usize = secs.iter().map(|s| s.tokens).sum();
    let mut out = format!("{page_id}  {title}\n{url}\n{} tok, {} sections\n", ktok(total), secs.len());
    let min = secs.iter().map(|s| s.level).min().unwrap_or(1);
    for s in secs {
        let indent = "  ".repeat(s.level.saturating_sub(min));
        out.push_str(&format!(
            "{indent}#{} {} ({})\n",
            s.id,
            s.title,
            ktok(span_tokens(secs, s.id))
        ));
    }
    out
}

/// Sections ranked for `terms`: how often the words occur, heading hits
/// counting triple. Returns (section id, score), best first.
pub fn best_sections(markdown: &str, secs: &[Section], terms: &[String], n: usize) -> Vec<(usize, usize)> {
    if terms.is_empty() {
        return Vec::new();
    }
    let lines: Vec<&str> = markdown.lines().collect();
    let total: usize = secs.iter().map(|s| s.tokens).sum();
    let mut scored: Vec<(usize, usize)> = secs
        .iter()
        .filter(|s| total < 1000 || span_tokens(secs, s.id) * 5 < total * 3)
        .map(|s| {
            let body = lines[s.start..s.end.min(lines.len())].join("\n").to_lowercase();
            let head = s.title.to_lowercase();
            let score = terms
                .iter()
                .map(|t| body.matches(t.as_str()).count() + 3 * head.matches(t.as_str()).count())
                .sum::<usize>();
            // Matches of several distinct words, per square root of size: a
            // specific section beats the long parent that contains it.
            let distinct = terms.iter().filter(|t| body.contains(t.as_str())).count();
            let raw = score * (1 + distinct);
            let density = (raw as f64 * 100.0 / ((s.tokens as f64) + 150.0).sqrt()) as usize;
            (s.id, if raw == 0 { 0 } else { density.max(1) })
        })
        .filter(|(_, sc)| *sc > 0)
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.truncate(n);
    scored
}

/// Query words worth matching: lowercase, 3+ characters, minus stop words.
pub fn terms(query: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "the", "and", "for", "with", "what", "how", "why", "are", "is", "best", "vs", "from",
        "does", "can", "you", "your", "about", "into", "that", "this", "which", "when", "who",
        "site", "2026", "2025",
    ];
    let mut out: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '.')
        .map(|w| w.trim_matches('.').to_string())
        .filter(|w| w.chars().count() >= 3 && !STOP.contains(&w.as_str()))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Markdown links `[text](https://...)` in a page: (url, anchor text).
pub fn links(markdown: &str) -> Vec<(String, String)> {
    static LINK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\[([^\]]{0,200})\]\((https?://[^)\s]+)\)").expect("link pattern")
    });
    LINK.captures_iter(markdown)
        .map(|c| (c[2].to_string(), c[1].trim().to_string()))
        .collect()
}
