//! Static assets. `rust-embed` bakes `static/` into the binary for release
//! builds and reads it from disk in debug builds (so frontend edits show up on
//! reload without a `cargo` rebuild). This replaces `tower_http`'s `ServeDir`,
//! which resolved `static/` relative to the process CWD and so forced the
//! binary to be run from the repo root.
//!
//! `static/style.css` and `static/app.js` are build artifacts produced by
//! `npm run build` (see `scripts/build-frontend.mjs`) and committed to the
//! repo; the rest (nothing, currently) would be plain vendored files.

use axum::extract::Path;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

pub async fn handler(Path(path): Path<String>, headers: HeaderMap) -> Response {
    let Some(file) = Assets::get(&path) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    // A content hash as the ETag, with `no-cache` so the browser always
    // revalidates: an unchanged asset costs one tiny 304, a rebuilt one
    // (e.g. `style.css` after a frontend edit) is picked up immediately
    // rather than being served stale for up to an hour. `sha256_hash()` is
    // 32 bytes, so `etag` is always a valid header value.
    let etag = etag_value(&file.metadata.sha256_hash());
    let matches = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag));
    if matches {
        return StatusCode::NOT_MODIFIED.into_response();
    }

    let mut response = (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static(content_type(&path)),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
        ],
        file.data,
    )
        .into_response();
    if let Ok(value) = HeaderValue::from_str(&etag) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response
}

/// A quoted lowercase-hex ETag for a byte digest, e.g. `"a1b2…"`.
fn etag_value(digest: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(digest.len() * 2 + 2);
    s.push('"');
    for b in digest {
        let _ = write!(s, "{b:02x}");
    }
    s.push('"');
    s
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, ext)| ext) {
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("map" | "json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("woff2") => "font/woff2",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}
