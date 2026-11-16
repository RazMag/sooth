//! The machine API an agent (`sooth --agent`) serves and
//! [`super::remote::RemoteMachine`] calls. JSON only, no UI, every route
//! behind a bearer token:
//!
//! | Route | Answer |
//! |---|---|
//! | `GET /api/v1/info` | [`Facts`] |
//! | `GET /api/v1/units?kinds=container,pod` | `[UnitRecord]` (status only for `kinds`; omitted means none) |
//! | `GET /api/v1/units/{file}/status` | `UnitStatus` |
//! | `POST /api/v1/units/{file}/{start,stop,restart}` | `UnitStatus` afterwards |
//! | `GET /api/v1/units/{file}/logs?lines=N` | `[LogLine]` (default 200, at most [`MAX_LOG_LINES`]) |
//!
//! A failure answers with its HTTP status and `{"error": "..."}`.

use std::sync::Arc;

use axum::extract::{Path, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use super::{Action, MAX_LOG_LINES, Machine};
use crate::error::AppError;
use crate::quadlet::UnitKind;

/// The version prefix every route lives under.
pub const PREFIX: &str = "/api/v1";

#[derive(Clone)]
pub struct ApiState {
    pub machine: Machine,
    pub token: Arc<str>,
}

pub fn router(state: ApiState) -> Router {
    let actions = [Action::Start, Action::Stop, Action::Restart];
    let mut api = Router::new()
        .route("/info", get(info))
        .route("/units", get(units))
        .route("/units/{file}/status", get(status))
        .route("/units/{file}/logs", get(logs));
    for action in actions {
        api = api.route(
            &format!("/units/{{file}}/{}", action.as_str()),
            post(move |s: State<ApiState>, f: Path<String>| act(s, f, action)),
        );
    }
    Router::new()
        .nest(
            PREFIX,
            api.layer(middleware::from_fn_with_state(state.clone(), require_token)),
        )
        .with_state(state)
}

/// The body of every failed answer.
#[derive(Debug, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub error: String,
}

struct ApiError(AppError);

impl<E: Into<AppError>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.0.status();
        if status.is_server_error() {
            tracing::error!(error = %self.0, "agent API request failed");
        }
        let body = ApiErrorBody {
            error: self.0.user_message(),
        };
        (status, Json(body)).into_response()
    }
}

async fn require_token(State(state): State<ApiState>, req: Request, next: Next) -> Response {
    let given = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match given {
        Some(t) if !state.token.is_empty() && same_secret(t, &state.token) => next.run(req).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            Json(ApiErrorBody {
                error: "missing or wrong agent token".into(),
            }),
        )
            .into_response(),
    }
}

/// Compares two secrets in time that depends only on their lengths.
fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
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
    Path(file): Path<String>,
    action: Action,
) -> Result<Json<crate::systemd::UnitStatus>, ApiError> {
    tracing::info!(unit = %file, action = action.as_str(), "agent API action");
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
