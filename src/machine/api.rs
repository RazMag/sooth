//! The machine API an agent (`sooth --agent`) serves and
//! [`super::remote::RemoteMachine`] calls. JSON only, no UI, served over
//! mutual TLS (`link::listener`): every caller shows a client certificate,
//! and every route but the two pairing ones needs that certificate to be a
//! hub this agent pinned (`link::trust::HubStore`).
//!
//! | Route | Who | Answer |
//! |---|---|---|
//! | `GET /api/v1/hello` | anyone | [`Hello`] |
//! | `POST /api/v1/pair` | anyone with the code | [`Paired`], and the caller is pinned |
//! | `DELETE /api/v1/pair` | a paired hub | the hub is forgotten |
//! | `GET /api/v1/info` | a paired hub | [`Facts`](super::Facts) |
//! | `GET /api/v1/units?kinds=container,pod` | a paired hub | `[UnitRecord]` (status only for `kinds`) |
//! | `GET /api/v1/units/{file}/status` | a paired hub | `UnitStatus` |
//! | `POST /api/v1/units/{file}/{start,stop,restart}` | a hub with Full access | `UnitStatus` afterwards |
//! | `GET /api/v1/units/{file}/logs?lines=N` | a paired hub | `[LogLine]` (default 200, at most [`MAX_LOG_LINES`]) |
//!
//! A failure answers with its HTTP status and `{"error": "..."}`.

use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use tower_http::trace::{DefaultOnResponse, TraceLayer};

use super::{Action, Facts, MAX_LOG_LINES, Machine};
use crate::error::AppError;
use crate::link::listener::Peer;
use crate::link::trust::{HubStore, Permission, TrustedHub};
use crate::link::{API_VERSION, Fingerprint};
use crate::quadlet::UnitKind;

/// The version prefix every route lives under.
pub const PREFIX: &str = "/api/v1";

#[derive(Clone)]
pub struct ApiState {
    pub machine: Machine,
    pub hubs: Arc<HubStore>,
}

pub fn router(state: ApiState) -> Router {
    let actions = [Action::Start, Action::Stop, Action::Restart];
    let mut paired = Router::new()
        .route("/pair", axum::routing::delete(unpair))
        .route("/info", get(info))
        .route("/units", get(units))
        .route("/units/{file}/status", get(status))
        .route("/units/{file}/logs", get(logs));
    for action in actions {
        paired = paired.route(
            &format!("/units/{{file}}/{}", action.as_str()),
            post(
                move |s: State<ApiState>, h: Extension<TrustedHub>, f: Path<String>| {
                    act(s, h, f, action)
                },
            ),
        );
    }
    let api = Router::new()
        .route("/hello", get(hello))
        .route("/pair", post(pair))
        .merge(paired.layer(middleware::from_fn_with_state(state.clone(), require_hub)));
    Router::new()
        .nest(PREFIX, api)
        .layer(
            TraceLayer::new_for_http()
                .on_response(DefaultOnResponse::new().level(tracing::Level::INFO)),
        )
        .with_state(state)
}

/// The body of every failed answer.
#[derive(Debug, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub error: String,
}

struct ApiError(StatusCode, String);

impl<E: Into<AppError>> From<E> for ApiError {
    fn from(e: E) -> Self {
        let e = e.into();
        let status = e.status();
        if status.is_server_error() {
            tracing::error!(error = %e, "agent API request failed");
        }
        Self(status, e.user_message())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(ApiErrorBody { error: self.1 })).into_response()
    }
}

fn refuse(status: StatusCode, message: impl Into<String>) -> Response {
    ApiError(status, message.into()).into_response()
}

/// The caller's certificate fingerprint. Always present behind the TLS
/// listener; a router served some other way has no callers it can trust.
fn caller(peer: &Peer) -> Result<Fingerprint, ApiError> {
    peer.fingerprint.ok_or_else(|| {
        ApiError(
            StatusCode::UNAUTHORIZED,
            "a client certificate is required".into(),
        )
    })
}

async fn require_hub(
    State(state): State<ApiState>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    mut req: Request,
    next: Next,
) -> Response {
    match peer.fingerprint.and_then(|fp| state.hubs.find(&fp)) {
        Some(hub) => {
            req.extensions_mut().insert(hub);
            next.run(req).await
        }
        None => refuse(
            StatusCode::UNAUTHORIZED,
            "this hub isn't paired with this machine",
        ),
    }
}

/// Compares two secrets in time that depends only on their lengths.
pub fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// What anyone may learn before pairing: enough to show the person which
/// machine they're about to add.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub hostname: String,
    pub sooth_version: String,
    pub api_version: u32,
}

async fn hello(State(s): State<ApiState>) -> Result<Json<Hello>, ApiError> {
    let facts = s.machine.facts().await?;
    Ok(Json(Hello {
        hostname: facts.hostname,
        sooth_version: facts.sooth_version,
        api_version: API_VERSION,
    }))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PairRequest {
    pub code: String,
    /// What the hub calls itself, shown in this agent's log.
    pub hub_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Paired {
    pub permission: Permission,
    pub facts: Facts,
}

async fn pair(
    State(s): State<ApiState>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    Json(req): Json<PairRequest>,
) -> Result<Json<Paired>, ApiError> {
    let fingerprint = caller(&peer)?;
    let hub = s
        .hubs
        .pair(&req.code, fingerprint, &req.hub_name)
        .map_err(|why| {
            tracing::warn!(addr = %peer.addr, "pairing refused: {}", why.message());
            ApiError(StatusCode::FORBIDDEN, why.message().into())
        })?;
    tracing::info!(
        hub = %hub.name,
        %fingerprint,
        permission = hub.permission.label(),
        "paired with a hub"
    );
    Ok(Json(Paired {
        permission: hub.permission,
        facts: s.machine.facts().await?,
    }))
}

async fn unpair(
    State(s): State<ApiState>,
    Extension(hub): Extension<TrustedHub>,
) -> Result<StatusCode, ApiError> {
    s.hubs.remove(&hub.fingerprint)?;
    tracing::info!(hub = %hub.name, "a hub unpaired itself");
    Ok(StatusCode::NO_CONTENT)
}

async fn info(State(s): State<ApiState>) -> Result<impl IntoResponse, ApiError> {
    Ok(Json(s.machine.facts().await?))
}

#[derive(Deserialize)]
struct UnitsQuery {
    kinds: Option<String>,
}

/// `container,pod` -> those kinds; an unknown name is an error rather than
/// silently dropped, so a newer hub asking for a kind this agent doesn't
/// know hears about it.
fn parse_kinds(s: Option<&str>) -> Result<Vec<UnitKind>, AppError> {
    s.unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(|k| {
            UnitKind::from_extension(k).ok_or_else(|| {
                crate::quadlet::QuadletError::Validation(format!("unknown kind '{k}'")).into()
            })
        })
        .collect()
}

async fn units(
    State(s): State<ApiState>,
    Query(q): Query<UnitsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let kinds = parse_kinds(q.kinds.as_deref())?;
    Ok(Json(s.machine.records(&kinds).await?))
}

async fn status(
    State(s): State<ApiState>,
    Path(file): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(Json(s.machine.status(&file).await?))
}

async fn act(
    State(s): State<ApiState>,
    Extension(hub): Extension<TrustedHub>,
    Path(file): Path<String>,
    action: Action,
) -> Result<Json<crate::systemd::UnitStatus>, ApiError> {
    if hub.permission == Permission::ReadOnly {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "this hub has read-only access to this machine".into(),
        ));
    }
    tracing::info!(unit = %file, action = action.as_str(), hub = %hub.name, "agent API action");
    Ok(Json(s.machine.act(&file, action).await?))
}

#[derive(Deserialize)]
struct LogsQuery {
    lines: Option<u32>,
}

async fn logs(
    State(s): State<ApiState>,
    Path(file): Path<String>,
    Query(q): Query<LogsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let lines = q.lines.unwrap_or(200).clamp(1, MAX_LOG_LINES);
    Ok(Json(s.machine.logs(&file, lines).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_secret_needs_an_exact_match() {
        assert!(same_secret("abc123", "abc123"));
        assert!(!same_secret("abc123", "abc124"));
        assert!(!same_secret("abc", "abc123"));
        assert!(!same_secret("", "x"));
    }

    #[test]
    fn kinds_parse_by_extension_and_refuse_unknown_ones() {
        assert_eq!(
            parse_kinds(Some("container, pod")).unwrap(),
            vec![UnitKind::Container, UnitKind::Pod]
        );
        assert!(parse_kinds(None).unwrap().is_empty());
        assert!(parse_kinds(Some("service")).is_err());
    }
}
