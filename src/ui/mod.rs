//! `osearch ui`: a local web UI over osearch's own data. Runs, pages,
//! settings and searches are read from (and started through) the same
//! files and commands the CLI uses. Listens on 127.0.0.1 only.

mod api;
mod insights;
mod jobs;
pub mod settings;

use crate::context::AppContext;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;
use rust_embed::RustEmbed;
use std::sync::Arc;

/// The built page (`ui/dist`, made by `npm run build`), packed into the exe.
#[derive(RustEmbed)]
#[folder = "ui/dist"]
#[allow_missing = true]
struct Assets;

pub struct UiState {
    pub app: Arc<AppContext>,
    pub port: u16,
    pub jobs: jobs::Jobs,
    pub insights: insights::Cache,
}

pub async fn serve(app: Arc<AppContext>, port: Option<u16>, open: bool) -> Result<i32, crate::errors::SearchError> {
    let port = port.unwrap_or(app.config.ui.port);
    let state = Arc::new(UiState { app, port, jobs: jobs::Jobs::default(), insights: insights::Cache::load() });
    let router = Router::new()
        .nest("/api", api::routes())
        .fallback(asset)
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| crate::errors::SearchError::Config(format!("cannot listen on 127.0.0.1:{port}: {e} (another `osearch ui`? use --port)")))?;
    let url = format!("http://127.0.0.1:{port}/");
    if Assets::get("index.html").is_none() {
        eprintln!("osearch ui: the page was not built into this osearch (run `npm run build` in ui/ before cargo build); the API still answers");
    }
    println!("osearch ui on {url} (Ctrl-C stops it)");
    if open {
        open_browser(&url);
    }
    axum::serve(listener, router)
        .await
        .map_err(|e| crate::errors::SearchError::Config(e.to_string()))?;
    Ok(0)
}

/// Only this machine's own page may use the API: the Host must be ours (no
/// DNS rebinding), and a request that changes anything must carry the UI's
/// header, which another site's page cannot send without a preflight we refuse.
async fn guard(State(state): State<Arc<UiState>>, req: Request, next: Next) -> Response {
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    let ours = [format!("127.0.0.1:{}", state.port), format!("localhost:{}", state.port)];
    if !ours.iter().any(|o| o == host) {
        return (StatusCode::FORBIDDEN, "unknown host").into_response();
    }
    if req.method() != Method::GET && req.method() != Method::HEAD {
        let marked = req.headers().get("x-osearch-ui").is_some();
        let origin = req.headers().get(header::ORIGIN).and_then(|h| h.to_str().ok());
        let same = origin.is_none_or(|o| ours.iter().any(|h| o == format!("http://{h}")));
        if !marked || !same {
            return (StatusCode::FORBIDDEN, "not from the osearch page").into_response();
        }
    }
    next.run(req).await
}

/// The page's files; any other path gets index.html (the page routes itself).
async fn asset(req: Request) -> Response {
    let path = req.uri().path().trim_start_matches('/');
    let (file, name) = match Assets::get(path) {
        Some(f) if !path.is_empty() => (f, path.to_string()),
        _ => match Assets::get("index.html") {
            Some(f) => (f, "index.html".to_string()),
            None => return (StatusCode::NOT_FOUND, "the web UI was not built into this osearch").into_response(),
        },
    };
    let mime = file.metadata.mimetype().to_string();
    let cache = if name.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
    let mut res = Response::new(Body::from(file.data.into_owned()));
    if let Ok(v) = HeaderValue::from_str(&mime) {
        res.headers_mut().insert(header::CONTENT_TYPE, v);
    }
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    res
}

fn open_browser(url: &str) {
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    let _ = cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
}
