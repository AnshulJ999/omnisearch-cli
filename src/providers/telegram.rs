//! Telegram public channels through their web preview (`t.me/s/<channel>`):
//! recent posts without a login, and a search inside one channel (`?q=`).
//! Groups and private channels have no preview.

use crate::context::AppContext;
use crate::providers::thread::clip;
use crate::types::SearchResult;
use dom_query::Document;
use std::time::{Duration, Instant};

pub struct Post {
    pub link: String,
    pub date: String,
    pub views: String,
    pub text: String,
}

/// Paths under `t.me` that are not channels.
const NOT_CHANNELS: &[&str] = &[
    "joinchat", "addstickers", "addemoji", "share", "proxy", "socks", "iv", "c", "login", "setlanguage", "addlist",
    "contact", "boost", "s",
];

/// The public channel a `t.me` link names, and the post number if it names one.
pub fn channel_of(url: &str) -> Option<(String, Option<String>)> {
    let u = url::Url::parse(url).ok()?;
    let host = u.host_str()?.to_ascii_lowercase();
    if !matches!(host.as_str(), "t.me" | "www.t.me" | "telegram.me" | "www.telegram.me") {
        return None;
    }
    let mut segs: Vec<&str> = u.path_segments()?.filter(|s| !s.is_empty()).collect();
    if segs.first() == Some(&"s") {
        segs.remove(0);
    }
    let ch = *segs.first()?;
    let name_ok = ch.len() >= 4 && ch.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !name_ok || NOT_CHANNELS.contains(&ch.to_ascii_lowercase().as_str()) {
        return None;
    }
    let post = segs.get(1).filter(|p| p.chars().all(|c| c.is_ascii_digit())).map(|p| p.to_string());
    Some((ch.to_string(), post))
}

/// A preview page as received.
pub async fn page(ctx: &AppContext, url: &str) -> Result<String, String> {
    let started = Instant::now();
    let resp = ctx
        .client
        .get(url)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    crate::run::raw::record_page("telegram", url, status, started.elapsed(), &text);
    if status >= 400 {
        return Err(format!("HTTP {status}"));
    }
    Ok(text)
}

/// The channel's title and the posts on a preview page, oldest first.
pub fn parse(html: &str) -> (String, Vec<Post>) {
    let doc = Document::from(html);
    let title = doc.select(".tgme_channel_info_header_title").text().trim().to_string();
    let mut posts = Vec::new();
    for node in doc.select(".tgme_widget_message[data-post]").iter() {
        let Some(id) = node.attr("data-post") else { continue };
        let body = node.select(".tgme_widget_message_text").first().html().to_string();
        let text = message_text(&body);
        if text.is_empty() {
            continue;
        }
        posts.push(Post {
            link: format!("https://t.me/{id}"),
            date: node.select("time[datetime]").attr("datetime").map(|d| d.to_string()).unwrap_or_default(),
            views: node.select(".tgme_widget_message_views").first().text().trim().to_string(),
            text,
        });
    }
    (title, posts)
}

/// A message's HTML as text, its line breaks kept.
fn message_text(html: &str) -> String {
    if html.is_empty() {
        return String::new();
    }
    let html = html.replace("<br/>", "\n").replace("<br>", "\n");
    Document::from(html.as_str()).select("body").text().trim().to_string()
}

/// Posts of `channel` that match `words`, newest first.
pub async fn search(ctx: &AppContext, channel: &str, words: &str) -> Result<Vec<SearchResult>, String> {
    let q: String = url::form_urlencoded::byte_serialize(words.as_bytes()).collect();
    let html = page(ctx, &format!("https://t.me/s/{channel}?q={q}")).await?;
    let (title, posts) = parse(&html);
    let name = if title.is_empty() { channel.to_string() } else { title };
    Ok(posts
        .into_iter()
        .rev()
        .map(|p| SearchResult {
            title: format!("{name}: {}", clip(&crate::providers::thread::one_line(&p.text), 90)),
            url: p.link,
            snippet: format!("{} ({} views)", clip(&p.text, 400), p.views),
            source: "telegram".into(),
            published: Some(p.date).filter(|d| !d.is_empty()),
            image_url: None,
            extra: Some(serde_json::json!({ "channel": channel, "views": p.views })),
        })
        .collect())
}

/// A preview page's posts as markdown, one section per post.
pub fn markdown(page_url: &str, title: &str, posts: &[Post]) -> String {
    let mut md = format!("# {title} (Telegram)\n\n{page_url}\n\n");
    for p in posts {
        let date = p.date.get(..10).unwrap_or(&p.date);
        md.push_str(&format!("## {date} · {} views\n\n{}\n\n{}\n\n", p.views, p.link, p.text));
    }
    md
}
