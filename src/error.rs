//! Central error handling. Every fallible handler returns
//! `Result<impl IntoResponse, PageError>` (full-page routes) or
//! `Result<impl IntoResponse, FragmentError>` (htmx action routes) and uses
//! `?` throughout -- the `IntoResponse` impls below are the single place
//! where an error becomes an HTTP response, so logging and rendering never
//! have to be duplicated per handler.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use tracing::{error, warn};
use uuid::Uuid;

use crate::auth::AuthError;
use crate::quadlet::QuadletError;
use crate::systemd::SystemdError;
use crate::web::templates;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("csrf token missing or invalid")]
    Csrf,
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Quadlet(#[from] QuadletError),
    #[error(transparent)]
    Systemd(#[from] SystemdError),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Csrf | AppError::Auth(_) => StatusCode::FORBIDDEN,
            AppError::Quadlet(QuadletError::Validation(_) | QuadletError::GeneratorRejected(_)) => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            AppError::Quadlet(QuadletError::NotFound(_)) => StatusCode::NOT_FOUND,
            AppError::Quadlet(QuadletError::Io(_)) => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::Systemd(_) => StatusCode::BAD_GATEWAY,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Logs the full error chain with a correlation id, at a severity that
    /// matches how "expected" the failure is (a bad quadlet file the user
    /// just typed is not the same severity as a D-Bus connection dying), and
    /// returns that id so the response can carry it without leaking the raw
    /// error text to the browser.
    fn log(&self) -> Uuid {
        let id = Uuid::new_v4();
        match self {
            AppError::NotFound(_)
            | AppError::Quadlet(QuadletError::Validation(_) | QuadletError::GeneratorRejected(_)) =>
            {
                warn!(error_id = %id, error = %self, "request failed");
            }
            _ => {
                error!(error_id = %id, error = %self, "request failed");
            }
        }
        id
    }

    fn user_message(&self) -> String {
        match self {
            AppError::NotFound(what) => format!("Not found: {what}"),
            AppError::Csrf => {
                "Your session expired or the form was resubmitted. Please try again.".into()
            }
            AppError::Auth(_) => "You need to sign in to do that.".into(),
            AppError::Quadlet(
                QuadletError::Validation(msg) | QuadletError::GeneratorRejected(msg),
            ) => {
                format!("That quadlet file is not valid:\n{msg}")
            }
            AppError::Quadlet(QuadletError::NotFound(what)) => format!("Not found: {what}"),
            AppError::Quadlet(QuadletError::Io(_)) => {
                "Could not read or write the quadlet directory.".into()
            }
            AppError::Systemd(_) => "systemd did not accept that action.".into(),
            AppError::Internal(_) => "Something went wrong.".into(),
        }
    }
}

/// Error wrapper for full-page (GET/navigation) handlers: renders a complete
/// error page with the site shell.
pub struct PageError(pub AppError);

/// Error wrapper for htmx action handlers: renders a small inline error
/// banner meant to be swapped into the triggering row/section, not a full page.
pub struct FragmentError(pub AppError);

impl<E: Into<AppError>> From<E> for PageError {
    fn from(e: E) -> Self {
        PageError(e.into())
    }
}

impl<E: Into<AppError>> From<E> for FragmentError {
    fn from(e: E) -> Self {
        FragmentError(e.into())
    }
}

impl IntoResponse for PageError {
    fn into_response(self) -> Response {
        let id = self.0.log();
        let status = self.0.status();
        (
            status,
            templates::error_page(status, &self.0.user_message(), id),
        )
            .into_response()
    }
}

impl IntoResponse for FragmentError {
    fn into_response(self) -> Response {
        let id = self.0.log();
        let status = self.0.status();
        (
            status,
            templates::error_fragment(&self.0.user_message(), id),
        )
            .into_response()
    }
}
