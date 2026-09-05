use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::{Stream, StreamExt};
use tokio_stream::wrappers::BroadcastStream;
use tower_sessions::Session;

use crate::config::AppState;
use crate::events::DashboardEvent;

use super::templates;

/// Dashboard-wide live-update stream: forwards `DashboardEvent`s from the
/// app's broadcast channel as named SSE events. `status-{service}` events
/// carry a freshly rendered status badge fragment; `units-changed` carries
/// the whole re-rendered unit list, for when files are created/edited/
/// deleted (by this session or externally, e.g. someone editing a quadlet
/// file by hand).
pub async fn events_stream(
    State(state): State<AppState>,
    session: Session,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let csrf = crate::auth::csrf::current(&session).await.unwrap_or_default();
    let rx = state.events.subscribe();
    let mut shutdown = state.shutdown.clone();

    let stream = BroadcastStream::new(rx).filter_map(|res| async move { res.ok() }).then(move |event| {
        let state = state.clone();
        let csrf = csrf.clone();
        async move { render_event(&state, &csrf, event).await }
    });

    // Otherwise-infinite: a browser tab left open holds this connection
    // forever, and axum's graceful shutdown waits for in-flight requests
    // before exiting. Ending the stream here as soon as shutdown is
    // signaled is what lets that wait -- and thus the process -- finish.
    let stream = stream.take_until(async move {
        let _ = shutdown.wait_for(|done| *done).await;
    });

    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

async fn render_event(state: &AppState, csrf: &str, event: DashboardEvent) -> Result<Event, Infallible> {
    match event {
        DashboardEvent::Status { service, status } => {
            let html = templates::status_badge(&service, &status).into_string();
            Ok(Event::default().event(format!("status-{service}")).data(html))
        }
        DashboardEvent::UnitsChanged => {
            let html = super::handlers::dashboard::render_unit_rows(state, csrf).await.unwrap_or_default();
            Ok(Event::default().event("units-changed").data(html))
        }
    }
}
