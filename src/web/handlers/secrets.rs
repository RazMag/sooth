//! The Secrets page: a thin UI over this user's podman secret store (see
//! `crate::secrets`) plus the cross-reference to which quadlets consume each
//! secret via `Secret=` (see `quadlet::refs::secret_refs`). A value is only
//! ever read back by an explicit "Show" (`reveal`, a CSRF-checked POST
//! answered `Cache-Control: no-store`); a submitted one is never echoed.
//!
//! Saving a secret can also restart the units using it (`restart_consumers`)
//! -- a container only reads its secrets at start, so a replaced value
//! otherwise does nothing until the next restart.

use axum::Form;
use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use maud::html;
use serde::{Deserialize, Serialize};
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, FragmentError, PageError};
use crate::quadlet::{discovery, refs};
use crate::secrets;
use crate::systemd::UnitStatus;
use crate::web::templates;
use crate::web::templates::secrets::SecretsPage;

#[derive(Deserialize)]
pub struct FlashQuery {
    /// `?set=<name>` / `?replaced=<name>` / `?removed=<name>` after a
    /// successful redirect. Only ever a name that passed
    /// `secrets::valid_name`, and escaped on render regardless.
    set: Option<String>,
    replaced: Option<String>,
    removed: Option<String>,
}

pub async fn index(
    State(state): State<AppState>,
    session: Session,
    Query(flash): Query<FlashQuery>,
) -> Result<Response, PageError> {
    Ok(render(&state, &session, "", Some(flash), None, StatusCode::OK).await)
}

#[derive(Deserialize)]
pub struct SetForm {
    csrf_token: String,
    name: String,
    value: String,
    #[serde(default)]
    replace: Option<String>,
    /// Checkbox: restart the units using this secret once it's saved.
    #[serde(default)]
    restart: Option<String>,
}

/// Browsers submit a `<textarea>`'s line breaks as CRLF; a multi-line secret
/// (a PEM key, say) almost always wants plain LF. Nothing else is touched --
/// no trimming, so leading/trailing whitespace the user typed is kept.
fn normalize_value(raw: &str) -> String {
    raw.replace("\r\n", "\n")
}

/// Whether a unit using a changed secret should be restarted: running ones
/// (to pick up the new value) and failed ones (typically failed *because* the
/// secret was missing) -- never a deliberately stopped one.
fn should_restart(status: &UnitStatus) -> bool {
    status.is_active() || status.is_failed()
}

/// Session key for the one-shot result of a restart, shown as a notice on
/// the page the POST redirects to (post/redirect/get, without having to fit
/// arbitrary unit file names into a query string).
const RESTART_FLASH_KEY: &str = "secrets.restart_report";

/// What `restart_consumers` did, per unit file name.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RestartReport {
    secret: String,
    restarted: Vec<String>,
    /// Deliberately stopped units -- left alone rather than started behind
    /// the operator's back.
    skipped: Vec<String>,
    failed: Vec<String>,
}

/// Restarts every unit whose quadlet references secret `name` and that is
/// running *or failed* -- failed covers the usual "it couldn't start because
/// the secret was missing" case. Stopped units are skipped. Best-effort per
/// unit: one failure doesn't stop the rest.
async fn restart_consumers(state: &AppState, name: &str) -> Result<RestartReport, AppError> {
    let all = discovery::load_all(&state.quadlet_dir)?;
    let mut report = RestartReport {
        secret: name.to_string(),
        ..Default::default()
    };
    for file_name in refs::secret_consumers(name, &all) {
        let Some(unit) = all.iter().find(|u| u.file_name == file_name) else {
            continue;
        };
        let service = unit.service_name();
        let status = match state.systemd.status(&service).await {
            Ok(st) => st,
            Err(e) => {
                tracing::warn!(unit = %service, error = %e, "status before secret restart failed");
                report.failed.push(file_name);
                continue;
            }
        };
        if !should_restart(&status) {
            report.skipped.push(file_name);
            continue;
        }
        match state.systemd.restart(&service).await {
            Ok(()) => report.restarted.push(file_name),
            Err(e) => {
                tracing::warn!(unit = %service, error = %e, "restart after secret change failed");
                report.failed.push(file_name);
            }
        }
    }
    tracing::info!(
        secret = name,
        restarted = report.restarted.len(),
        skipped = report.skipped.len(),
        failed = report.failed.len(),
        "restarted units using secret"
    );
    Ok(report)
}

async fn stash_report(session: &Session, report: &RestartReport) {
    if let Err(e) = session.insert(RESTART_FLASH_KEY, report).await {
        tracing::warn!(error = %e, "could not store restart report in session");
    }
}

pub async fn set(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<SetForm>,
) -> Result<Response, PageError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    let name = form.name.trim();
    let replace = form.replace.is_some();
    let value = normalize_value(&form.value);

    match secrets::set(name, value.as_bytes(), replace).await {
        Ok(()) => {
            // The name only -- never the value.
            tracing::info!(secret = name, replace, "podman secret set");
            if form.restart.is_some() {
                let report = restart_consumers(&state, name).await?;
                stash_report(&session, &report).await;
            }
            let key = if replace { "replaced" } else { "set" };
            Ok(Redirect::to(&format!("/secrets?{key}={name}")).into_response())
        }
        Err(e) if e.is_client_error() => Ok(render(
            &state,
            &session,
            name,
            None,
            Some(&e.to_string()),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await),
        Err(e) => Err(AppError::from(e).into()),
    }
}

#[derive(Deserialize)]
pub struct RemoveForm {
    csrf_token: String,
    name: String,
}

pub async fn remove(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<RemoveForm>,
) -> Result<Response, PageError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    let name = form.name.trim();

    // Same rule as `/groups/delete`: refuse while anything still uses it,
    // rather than leave a unit that can no longer start.
    let all = discovery::load_all(&state.quadlet_dir)?;
    let users = refs::secret_consumers(name, &all);
    if !users.is_empty() {
        let msg = format!(
            "'{name}' is still referenced by {} — remove those Secret= lines first.",
            users.join(", ")
        );
        return Ok(render(
            &state,
            &session,
            "",
            None,
            Some(&msg),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await);
    }

    secrets::remove(name).await?;
    tracing::info!(secret = name, "podman secret removed");
    Ok(Redirect::to(&format!("/secrets?removed={name}")).into_response())
}

#[derive(Deserialize)]
pub struct NameForm {
    csrf_token: String,
    name: String,
}

/// `POST /secrets/restart` -- restart the units using a secret without
/// changing it (e.g. after replacing it with the checkbox left off).
pub async fn restart(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<NameForm>,
) -> Result<Response, PageError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    let report = restart_consumers(&state, form.name.trim()).await?;
    stash_report(&session, &report).await;
    Ok(Redirect::to("/secrets").into_response())
}

/// `POST /secrets/reveal` -- the raw value, fetched by the Value column's eye
/// toggle (`frontend/secrets.js`). A POST so it's CSRF-checked and never prefetched; `no-store` so the
/// browser doesn't keep a copy in its cache.
pub async fn reveal(
    session: Session,
    Form(form): Form<NameForm>,
) -> Result<Response, FragmentError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    let name = form.name.trim();
    let value = secrets::reveal(name).await?;
    // The name only -- this is an audit trail, not a leak.
    tracing::info!(secret = name, "podman secret value revealed");
    // Plain text -- `frontend/secrets.js` sets it as `textContent`, so it's
    // never parsed as markup.
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        value,
    )
        .into_response())
}

/// `GET /secrets/chips` -- the insert-panel fragment the editor pages
/// lazy-load (see `templates::host_vars_panel`). A podman failure just
/// renders the empty state; it's a convenience, not worth an error banner
/// inside someone's editor.
pub async fn chips() -> impl IntoResponse {
    let mut names: Vec<String> = secrets::names()
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();
    names.sort();
    templates::secrets::chips(&names)
}

async fn render(
    state: &AppState,
    session: &Session,
    prefill_name: &str,
    flash: Option<FlashQuery>,
    error: Option<&str>,
    status: StatusCode,
) -> Response {
    let csrf = crate::auth::csrf::current(session)
        .await
        .unwrap_or_default();
    let all = discovery::load_all(&state.quadlet_dir).unwrap_or_default();

    let (stored, list_error) = match secrets::list().await {
        Ok(s) => (s, None),
        Err(e) => {
            tracing::warn!(error = %e, "podman secret ls failed");
            (
                Vec::new(),
                Some(format!("Could not list podman secrets: {e}")),
            )
        }
    };
    let used_by: Vec<Vec<String>> = stored
        .iter()
        .map(|s| refs::secret_consumers(&s.name, &all))
        .collect();
    // Only report "missing" when the listing actually worked -- otherwise
    // every reference would look missing.
    let missing = if list_error.is_none() {
        let existing = stored.iter().map(|s| s.name.clone()).collect();
        refs::missing_secrets(&all, &existing)
    } else {
        Default::default()
    };

    let restart_report = session
        .remove::<RestartReport>(RESTART_FLASH_KEY)
        .await
        .ok()
        .flatten();

    let notice = flash.and_then(|f| {
        if let Some(name) = f.replaced {
            let users = refs::secret_consumers(&name, &all);
            Some(html! {
                "Secret " code { (name) } " replaced."
                @if !users.is_empty() && restart_report.is_none() {
                    " Units using it keep the old value until they restart: "
                    (templates::unit_links(&all, &users)) " "
                    (templates::secrets::restart_button(&csrf, &name, users.len()))
                }
            })
        } else if let Some(name) = f.set {
            Some(html! { "Secret " code { (name) } " saved." })
        } else {
            f.removed
                .map(|name| html! { "Secret " code { (name) } " deleted." })
        }
    });

    let notice = match (notice, &restart_report) {
        (Some(n), Some(r)) => Some(html! { (n) " " (restart_summary(r, &all)) }),
        (None, Some(r)) => Some(restart_summary(r, &all)),
        (n, None) => n,
    };

    let error = error.or(list_error.as_deref());
    (
        status,
        templates::secrets::page(SecretsPage {
            csrf: &csrf,
            secrets: &stored,
            used_by: &used_by,
            missing: &missing,
            all_units: &all,
            prefill_name,
            notice,
            error,
            health: state.health.get(),
        }),
    )
        .into_response()
}

fn restart_summary(r: &RestartReport, all: &[crate::quadlet::QuadletUnit]) -> maud::Markup {
    html! {
        @if r.restarted.is_empty() && r.skipped.is_empty() && r.failed.is_empty() {
            "No units use " code { (r.secret) } "."
        } @else {
            @if !r.restarted.is_empty() {
                "Restarted " (templates::unit_links(all, &r.restarted)) ". "
            }
            @if !r.skipped.is_empty() {
                "Left stopped: " (templates::unit_links(all, &r.skipped)) ". "
            }
            @if !r.failed.is_empty() {
                strong { "Failed to restart: " } (templates::unit_links(all, &r.failed))
                " — check their logs."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_value_converts_crlf_only() {
        assert_eq!(
            normalize_value("-----BEGIN KEY-----\r\nabc\r\n-----END KEY-----\r\n"),
            "-----BEGIN KEY-----\nabc\n-----END KEY-----\n"
        );
        assert_eq!(normalize_value("  spaced pass  "), "  spaced pass  ");
        assert_eq!(normalize_value("lone\rcr"), "lone\rcr");
    }

    fn status(active_state: &str) -> UnitStatus {
        UnitStatus {
            active_state: active_state.into(),
            ..UnitStatus::not_found()
        }
    }

    #[test]
    fn restarts_running_and_failed_units_only() {
        assert!(should_restart(&status("active")));
        assert!(should_restart(&status("failed")));
        assert!(!should_restart(&status("inactive")));
        assert!(!should_restart(&status("activating")));
        assert!(!should_restart(&UnitStatus::not_found()));
    }
}
