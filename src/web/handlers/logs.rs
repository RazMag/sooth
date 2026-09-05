use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
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
    let initial = journal::tail_recent(&unit.service_name(), 200).await.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "failed to read initial journal tail");
        String::new()
    });
    Ok(templates::logs_page(&unit, &initial))
}

pub async fn logs_stream(
    State(state): State<AppState>,
    Path(file_name): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, PageError> {
    let unit = discovery::load_by_name(&state.quadlet_dir, &file_name)?;
    let service = unit.service_name();
    let (child, reader) =
        journal::follow(&service).map_err(|e| anyhow::anyhow!("failed to start log tail for {service}: {e}"))?;

    let lines = LinesStream::new(reader.lines());
    // `child` is moved into the stream's state so it (and its kill-on-drop
    // subprocess) stays alive exactly as long as this SSE connection does.
    let stream = futures_util::stream::unfold((child, lines), |(child, mut lines)| async move {
        match lines.next().await {
            Some(Ok(line)) => Some((Ok(Event::default().data(line)), (child, lines))),
            _ => None,
        }
    });

    // Otherwise-infinite (journalctl -f never stops on its own): end it as
    // soon as shutdown is signaled, so axum's graceful shutdown -- which
    // waits for in-flight requests -- doesn't wait on this one forever.
    let mut shutdown = state.shutdown.clone();
    let stream = stream.take_until(async move {
        let _ = shutdown.wait_for(|done| *done).await;
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}
