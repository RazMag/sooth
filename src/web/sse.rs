use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::{Stream, StreamExt};
use tokio_stream::wrappers::BroadcastStream;

use crate::config::AppState;
use crate::events::DashboardEvent;

use super::templates;

/// Dashboard-wide live-update stream: forwards `DashboardEvent`s from the
/// app's broadcast channel as named SSE events. `status-{service}` carries a
/// freshly rendered status badge fragment (swapped in directly via the
/// `sse-swap` htmx extension); `units-changed` and `any-status` are pings
/// whose payload is a throwaway `"1"` -- every list page and the detail
/// page's action row react to them by re-fetching their own `/rows` or
/// `/actions` fragment via a normal htmx GET, rather than the SSE stream
/// trying to push pre-rendered markup for every possible page shape itself.
///
/// The payload has to be non-empty: an SSE event whose data buffer ends up
/// empty is, per the spec, never dispatched to `EventSource` listeners, so a
/// bare `data:` line would make these pings silently do nothing in the
/// browser.
pub async fn events_stream(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.events.subscribe();
    let mut shutdown = state.shutdown.clone();

    let stream = BroadcastStream::new(rx)
        .filter_map(|res| async move { res.ok() })
        .flat_map(|event| futures_util::stream::iter(render_event(event)));

    // Otherwise-infinite: a browser tab left open holds this connection
    // forever, and axum's graceful shutdown waits for in-flight requests
    // before exiting. Ending the stream here as soon as shutdown is
    // signaled is what lets that wait -- and thus the process -- finish.
    let stream = stream.take_until(async move {
        let _ = shutdown.wait_for(|done| *done).await;
    });

    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

fn render_event(event: DashboardEvent) -> Vec<Result<Event, Infallible>> {
    match event {
        DashboardEvent::Status { service, status } => {
            let html = templates::status_badge(&service, &status).into_string();
            vec![
                Ok(Event::default()
                    .event(format!("status-{service}"))
                    .data(html)),
                Ok(Event::default().event("any-status").data("1")),
            ]
        }
        DashboardEvent::UnitsChanged => vec![Ok(Event::default().event("units-changed").data("1"))],
    }
}
