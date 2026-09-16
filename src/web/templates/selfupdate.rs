//! The Settings page's "Updates" section: the live status fragment (current
//! version, check/download/install actions) -- see `crate::selfupdate`. The
//! editable settings (mode, poll interval, repo) are built with
//! `mode_toggle()` / `interval_options()` below but live in the main
//! settings form now, not a form of their own -- see
//! `crate::web::handlers::settings::save`, which persists and live-applies
//! them together with the rest of the page in one request.
use std::time::SystemTime;

use maud::{Markup, html};

use super::{BannerKind, banner};
use crate::selfupdate::{UpdateState, UpdateStatus};

/// Re-rendered on `sse:self-update-changed` (the self-update analogue of
/// `gitsync::rows`) -- the status line always reflects the latest check/
/// download progress, e.g. once the background poll task finds an update.
pub fn status_fragment(status: &UpdateStatus, csrf: &str) -> Markup {
    html! {
        div #self-update-status
            hx-get="/settings/self-update" hx-trigger="sse:self-update-changed delay:300ms" hx-swap="outerHTML" {

            p.selfupdate-version { "Current version: " code { (env!("CARGO_PKG_VERSION")) } }

            (status_line(status, csrf))
        }
    }
}

/// `(seconds, label)` presets for the "Check every" field, ordered shortest
/// first.
const INTERVAL_PRESETS: &[(u64, &str)] = &[
    (3_600, "hour"),
    (21_600, "6 hours"),
    (43_200, "12 hours"),
    (86_400, "day"),
    (604_800, "week"),
];

/// The three-way Off/Notify/Auto control: a segmented toggle built from
/// plain radio inputs (each visually hidden, its `<label>` styled as the
/// segment -- see `.segmented` in `styles.css`), not a `<select>`, so all
/// three choices are visible and one click away instead of hidden behind a
/// dropdown. `current` is the raw form value ("off"/"notify"/"auto"), same
/// round-trip-as-a-string convention as every other field on the page.
pub(crate) fn mode_toggle(current: &str) -> Markup {
    let segment = |value: &'static str, id: &'static str, label: &'static str| {
        html! {
            input type="radio" id=(id) name="mode" value=(value) checked[current == value];
            label for=(id) { (label) }
        }
    };
    html! {
        div.segmented role="radiogroup" aria-label="Update checks" {
            (segment("off", "self_update_mode_off", "Off"))
            (segment("notify", "self_update_mode_notify", "Notify"))
            (segment("auto", "self_update_mode_auto", "Auto"))
        }
    }
}

/// The interval `<select>`'s options. `current` is the raw form value; when
/// it doesn't match any preset (e.g. a value hand-edited into the TOML file,
/// a future preset list that no longer includes it, or a rejected
/// submission redisplaying exactly what was typed), an extra option holding
/// that exact value is appended and selected, so saving the form without
/// touching this field can never silently change it.
pub(crate) fn interval_options(current: &str) -> Markup {
    html! {
        @for &(secs, label) in INTERVAL_PRESETS {
            option value=(secs) selected[secs.to_string() == current] { "Every " (label) }
        }
        @if !INTERVAL_PRESETS.iter().any(|&(secs, _)| secs.to_string() == current) {
            option value=(current) selected { "Custom (every " (current) "s)" }
        }
    }
}

fn status_line(status: &UpdateStatus, csrf: &str) -> Markup {
    html! {
        div.selfupdate-status {
            @match &status.state {
                UpdateState::Idle => p.field-hint { "Not checked yet." },
                UpdateState::Checking => p.field-hint { "Checking for updates…" },
                UpdateState::UpToDate => p.field-hint { "Up to date (checked " (checked_at(status)) ")." },
                UpdateState::UpdateAvailable { version } => (banner(
                    BannerKind::Info,
                    &format!("Update available: v{version}"),
                )),
                UpdateState::Downloading => p.field-hint { "Downloading update…" },
                UpdateState::ReadyToRestart { version } => (banner(
                    BannerKind::Success,
                    &format!("v{version} downloaded and ready to install."),
                )),
                UpdateState::Applying => p.field-hint {
                    "Downloaded -- sooth will restart automatically to install it."
                },
                UpdateState::Error(msg) => (banner(BannerKind::Error, msg)),
            }
        }
        div.selfupdate-actions {
            @match &status.state {
                UpdateState::UpdateAvailable { .. } => {
                    // A plain button, not a `form.inline-form` -- this status
                    // fragment now renders inside the main settings `<form>`
                    // (see `templates::settings::page`), and a nested `<form>`
                    // would be dropped by the HTML parser, silently breaking
                    // this action. `hx-params` whitelists just the `hx-vals`
                    // csrf token: htmx would otherwise also pick up every
                    // field in the enclosing form (harmless server-side, since
                    // the handler ignores unknown fields, but pointless to
                    // send) -- and "none" would strip `hx-vals` too, not just
                    // the form's fields, so it has to be a named whitelist.
                    button.btn.btn-primary type="button"
                        hx-post="/settings/self-update/download" hx-swap="none"
                        hx-params="csrf_token" hx-vals=(csrf_vals(csrf)) { "Download update" }
                }
                UpdateState::ReadyToRestart { .. } => {
                    // Installing a downloaded update is just an ordinary
                    // restart -- same action, and the same actual `<form>`
                    // (by id, since this button isn't a descendant of it), as
                    // the Restart card further down the page, not a distinct
                    // self-update step.
                    button.btn.btn-primary type="submit" form="restart-form" {
                        "Install and restart"
                    }
                }
                UpdateState::Checking | UpdateState::Downloading => {}
                _ => {
                    button.btn.btn-sm type="button"
                        hx-post="/settings/self-update/check" hx-swap="none"
                        hx-params="csrf_token" hx-vals=(csrf_vals(csrf)) { "Check now" }
                }
            }
        }
    }
}

/// The `hx-vals` JSON for a CSRF-only htmx request -- the plain-button
/// replacement for a `csrf_input()` hidden field inside a `form.inline-form`.
fn csrf_vals(csrf: &str) -> String {
    format!(r#"{{"csrf_token":"{csrf}"}}"#)
}

/// A coarse "how long ago" for the last check -- same rough formatting as
/// `gitsync::checked_at`.
fn checked_at(status: &UpdateStatus) -> String {
    let Some(at) = status.checked_at else {
        return "never".to_string();
    };
    let secs = SystemTime::now()
        .duration_since(at)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}
