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
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

pub async fn handler(Path(path): Path<String>) -> Response {
    match Assets::get(&path) {
        Some(file) => (
            [
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_static(content_type(&path)),
                ),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=3600"),
                ),
            ],
            file.data,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
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
