//! Jina's reranker: how well each text answers a question, from 0 to 1.

use crate::context::AppContext;
use std::time::Duration;

const URL: &str = "https://api.jina.ai/v1/rerank";
const MODEL: &str = "jina-reranker-v3.5";

/// One score per document, in the documents' order.
pub async fn scores(app: &AppContext, question: &str, docs: &[String]) -> Result<Vec<f64>, String> {
    let key = crate::providers::key_for(&app.config, &crate::providers::jina::SPEC);
    if key.is_empty() {
        return Err("no Jina key".into());
    }
    let body = serde_json::json!({
        "model": MODEL,
        "query": question,
        "documents": docs,
        "top_n": docs.len(),
        "return_documents": false,
    });
    let resp = app
        .client
        .post(URL)
        .bearer_auth(&key)
        .json(&body)
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    let status = resp.status();
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {}: {}", status.as_u16(), crate::providers::thread::clip(&v.to_string(), 200)));
    }
    let mut out = vec![0.0; docs.len()];
    for r in v["results"].as_array().into_iter().flatten() {
        if let (Some(i), Some(s)) = (r["index"].as_u64(), r["relevance_score"].as_f64()) {
            if let Some(slot) = out.get_mut(i as usize) {
                *slot = s;
            }
        }
    }
    // Jina tokens: $0.05 per million.
    if let Some(t) = v["usage"]["total_tokens"].as_u64() {
        crate::logging::log_spend("jina", t.div_ceil(20), question, "rerank");
    }
    Ok(out)
}

/// The blended score: half the fused score scaled to the best, half the
/// reranker's.
pub fn blend(score: f64, best: f64, relevance: f64) -> f64 {
    0.5 * score / best.max(1e-9) + 0.5 * relevance
}
