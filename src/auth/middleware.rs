use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use tower_sessions::Session;
use tracing::debug;

/// Guards every route it's layered onto: authenticated requests pass through
/// unchanged, everything else is sent to `/login` -- as a real redirect for
/// normal page navigation, or via the `HX-Redirect` response header for
/// htmx requests (htmx follows that client-side instead of swapping the
/// unauthenticated response into the page).
pub async fn require_auth(session: Session, request: Request, next: Next) -> Response {
    if super::session::is_authenticated(&session).await {
        return next.run(request).await;
    }
    debug!("rejecting unauthenticated request");
    if request.headers().contains_key("HX-Request") {
        let mut resp = StatusCode::UNAUTHORIZED.into_response();
        resp.headers_mut()
            .insert("HX-Redirect", HeaderValue::from_static("/login"));
        resp
    } else {
        Redirect::to("/login").into_response()
    }
}
