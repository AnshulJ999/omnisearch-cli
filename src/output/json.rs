//! `--json`: every command writes exactly one JSON document to stdout,
//! success or error. Diagnostics go to stderr only.

use crate::errors::SearchError;
use serde::Serialize;
use std::io::Write;

fn emit<T: Serialize + ?Sized>(value: &T) {
    let mut out = std::io::stdout().lock();
    // A closed pipe (`search … | head -c1`) must not panic the process the way
    // `println!` would; the reader has already gone, so there is no one to tell.
    let written = if super::pretty() {
        serde_json::to_writer_pretty(&mut out, value)
    } else {
        serde_json::to_writer(&mut out, value)
    };
    if written.is_ok() {
        let _ = out.write_all(b"\n");
        let _ = out.flush();
    }
}

/// The error envelope goes to stdout too: an agent capturing stdout must be
/// able to read why a command failed, not just see an empty stream.
pub fn render_error(error: &SearchError) {
    emit(&error.to_error_response());
}

pub fn render_value(value: &serde_json::Value) {
    emit(value);
}
