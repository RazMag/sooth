use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::{Stream, StreamExt};
use tokio::io::AsyncBufReadExt;
use tokio_stream::wrappers::LinesStream;

use crate::config::AppState;
use crate::error::PageError;
use crate::journal;
use crate::quadlet::discovery;
use crate::web::templates;

pub async fn logs_page(
    State(state): State<AppState>,
    Path(file_name): Path<String>,
) -> Result<impl IntoResponse, PageError> {
    let unit = discovery::load_by_name(&state.quadlet_dir, &file_name)?;
    let initial = journal::tail_recent(&unit.service_name(), 200)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to read initial journal tail");
            Vec::new()
        });
    Ok(templates::logs::logs_page(
        &unit,
        &initial,
        state.health.get(),
    ))
}

pub async fn logs_stream(
    State(state): State<AppState>,
    Path(file_name): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, PageError> {
    let unit = discovery::load_by_name(&state.quadlet_dir, &file_name)?;
    let service = unit.service_name();
    let (child, reader) = journal::follow(&service)
        .map_err(|e| anyhow::anyhow!("failed to start log tail for {service}: {e}"))?;

    let lines = LinesStream::new(reader.lines());
    let stream = futures_util::stream::unfold((child, lines), |(child, mut lines)| async move {
        match lines.next().await {
            Some(Ok(line)) => Some((line, (child, lines))),
            _ => None,
        }
    })
    // Old-run marking is logs.js's job for live lines (it knows which run is
    // latest as they arrive), so every fragment goes out unmarked.
    .filter_map(|line| async move {
        let line = journal::parse_json_line(&line)?;
        let html = templates::logs::log_line(&line, false).into_string();
        Some(Ok(Event::default().data(normalize_newlines(&html))))
    });

    let mut shutdown = state.shutdown.clone();
    let stream = stream.take_until(async move {
        let _ = shutdown.wait_for(|done| *done).await;
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

/// Rewrites `\r\n` and lone `\r` to `\n` -- exactly what the HTML parser
/// does to the initial page's lines -- so a live line renders the same as it
/// would after a reload. Journal messages do carry carriage returns
/// (progress-bar style output). Sent raw, `Event::data` would treat each one
/// as an SSE line break: a lone `\r` survives as a newline, but `\r\n` turns
/// into two, adding a blank line.
fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::normalize_newlines;

    #[test]
    fn leaves_text_without_carriage_returns_alone() {
        assert_eq!(normalize_newlines(""), "");
        assert_eq!(normalize_newlines("plain line"), "plain line");
        assert_eq!(normalize_newlines("two\nlines\n"), "two\nlines\n");
        assert_eq!(normalize_newlines("ünïcödé ✓"), "ünïcödé ✓");
    }

    #[test]
    fn lone_carriage_return_becomes_a_newline() {
        assert_eq!(
            normalize_newlines("progress 10%\rprogress 100%"),
            "progress 10%\nprogress 100%"
        );
        assert_eq!(normalize_newlines("\rleading"), "\nleading");
        assert_eq!(normalize_newlines("trailing\r"), "trailing\n");
    }

    #[test]
    fn crlf_becomes_a_single_newline() {
        assert_eq!(normalize_newlines("a\r\nb"), "a\nb");
        assert_eq!(normalize_newlines("a\r\nb\r\n"), "a\nb\n");
    }

    #[test]
    fn runs_of_carriage_returns_match_html_parsing() {
        // The HTML parser reads `\r\r\n` as CR + CRLF: two line breaks.
        assert_eq!(normalize_newlines("a\r\r\nb"), "a\n\nb");
        assert_eq!(normalize_newlines("a\r\rb"), "a\n\nb");
        assert_eq!(normalize_newlines("a\n\rb"), "a\n\nb");
    }

    #[test]
    fn mixed_endings_in_one_message() {
        assert_eq!(
            normalize_newlines("1%\r50%\r100%\r\ndone\nok"),
            "1%\n50%\n100%\ndone\nok"
        );
    }

    #[test]
    fn output_never_contains_a_carriage_return() {
        for input in ["\r", "\r\r", "\r\n\r", "x\r\r\r\ny\rz", "\n\r\n\r"] {
            assert!(!normalize_newlines(input).contains('\r'), "{input:?}");
        }
    }
}
