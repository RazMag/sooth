use axum::Router;
use axum::http::StatusCode;
use axum::middleware;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use tower_http::trace::TraceLayer;

use crate::auth;
use crate::config::AppState;

use super::handlers::{
    detail, edit_delete, environment, list, logs, ports, raw_create, services, settings, unit_ops,
    validate,
};

/// Registers the routes every section shares -- detail, status fragment,
/// actions, config fragment, edit, delete, logs -- at one URL prefix. Called
/// once per section (`/containers`, `/pods`, `/volumes`, `/networks`, `/images`,
/// and `/units` for the generic Kube/fallback section), always pointing at
/// the exact same handler functions: none of this logic differs by kind, so
/// there's exactly one implementation, just reachable at six prefixes.
fn mount_unit_routes(router: Router<AppState>, prefix: &str) -> Router<AppState> {
    router
        .route(&format!("{prefix}/{{file_name}}"), get(detail::show))
        .route(
            &format!("{prefix}/{{file_name}}/actions"),
            get(unit_ops::actions),
        )
        .route(
            &format!("{prefix}/{{file_name}}/config"),
            get(unit_ops::config),
        )
        .route(
            &format!("{prefix}/{{file_name}}/start"),
            post(unit_ops::start),
        )
        .route(
            &format!("{prefix}/{{file_name}}/stop"),
            post(unit_ops::stop),
        )
        .route(
            &format!("{prefix}/{{file_name}}/restart"),
            post(unit_ops::restart),
        )
        .route(
            &format!("{prefix}/{{file_name}}/enable"),
            post(unit_ops::enable),
        )
        .route(
            &format!("{prefix}/{{file_name}}/disable"),
            post(unit_ops::disable),
        )
        .route(
            &format!("{prefix}/{{file_name}}/edit"),
            get(edit_delete::edit_form).post(edit_delete::edit_submit),
        )
        .route(
            &format!("{prefix}/{{file_name}}/delete"),
            post(edit_delete::delete),
        )
        .route(
            &format!("{prefix}/{{file_name}}/logs"),
            get(logs::logs_page),
        )
        .route(
            &format!("{prefix}/{{file_name}}/logs/stream"),
            get(logs::logs_stream),
        )
}

pub fn build_router(state: AppState) -> Router {
    let mut protected = Router::new()
        // Services: the home page, combining Containers + Pods.
        .route("/", get(services::page))
        .route("/services/rows", get(services::rows))
        .route("/services/counts", get(services::counts))
        // Containers/Pods: no standalone list page (that's Services above),
        // but they still need a create entry point and POST target.
        .route("/containers", post(raw_create::create))
        .route("/containers/new", get(raw_create::containers_new_form))
        .route("/pods", post(raw_create::create))
        .route("/pods/new", get(raw_create::pods_new_form))
        // Volumes/Networks/Images: their own list page + shared raw-textarea create.
        .route("/volumes", get(list::volumes_page).post(raw_create::create))
        .route("/volumes/rows", get(list::volumes_rows))
        .route("/volumes/new", get(raw_create::volumes_new_form))
        .route(
            "/networks",
            get(list::networks_page).post(raw_create::create),
        )
        .route("/networks/rows", get(list::networks_rows))
        .route("/networks/new", get(raw_create::networks_new_form))
        .route("/images", get(list::images_page).post(raw_create::create))
        .route("/images/rows", get(list::images_rows))
        .route("/images/new", get(raw_create::images_new_form))
        // Generic fallback: all kinds, unlinked from the sidebar, Kube's only home.
        .route("/units", get(list::all_units_page).post(raw_create::create))
        .route("/units/rows", get(list::all_units_rows))
        .route("/units/new", get(raw_create::units_new_form))
        .route("/ports", get(ports::index))
        .route(
            "/environment",
            get(environment::index).post(environment::add),
        )
        .route("/environment/delete", post(environment::remove))
        .route("/settings", get(settings::page).post(settings::save))
        .route("/settings/password", post(settings::change_password))
        .route("/settings/restart", post(settings::restart))
        .route("/validate", post(validate::check))
        .route("/events", get(super::sse::events_stream))
        .route("/logout", post(auth::session::logout));

    for prefix in [
        "/containers",
        "/pods",
        "/volumes",
        "/networks",
        "/images",
        "/units",
    ] {
        protected = mount_unit_routes(protected, prefix);
    }
    protected = protected.route_layer(middleware::from_fn(auth::middleware::require_auth));

    let public = Router::new()
        .route(
            "/login",
            get(auth::session::login_page).post(auth::session::login_submit),
        )
        .route("/healthz", get(healthz))
        .route("/static/{*path}", get(super::assets::handler));

    Router::new()
        .merge(protected)
        .merge(public)
        .fallback(not_found)
        .layer(TraceLayer::new_for_http())
        .layer(auth::session::layer(
            state.config.cookie_secure,
            state.config.session_idle_timeout_secs,
        ))
        .with_state(state)
}

async fn healthz() -> impl IntoResponse {
    StatusCode::OK
}

async fn not_found(uri: axum::http::Uri) -> crate::error::PageError {
    crate::error::PageError(crate::error::AppError::NotFound(uri.path().to_string()))
}
