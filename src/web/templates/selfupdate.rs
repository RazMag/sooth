//! The Settings page's "Updates" card: current version, self-update
//! settings, and live status -- see `crate::selfupdate`.

use std::time::SystemTime;

use maud::{Markup, html};

use super::{BannerKind, banner, csrf_input};
use crate::selfupdate::{SelfUpdateConfig, UpdateMode, UpdateState, UpdateStatus};

/// Re-rendered whole on `sse:self-update-changed` (the self-update analogue
/// of `gitsync::rows`) -- both the settings form and the status line always
/// agree, e.g. after an edit through another tab, or once the background
/// poll task finds an update.
pub fn card(config: &SelfUpdateConfig, status: &UpdateStatus, csrf: &str) -> Markup {
    render(config, status, csrf, None)
}

/// The save form's own htmx target re-renders with a 422 and an inline error
/// banner when the submitted settings don't validate, so the form stays put
/// (and keeps whatever else the user typed) instead of vanishing behind a
/// generic error fragment.
pub fn card_with_error(
    config: &SelfUpdateConfig,
    status: &UpdateStatus,
    csrf: &str,
    error: &str,
) -> Markup {
    render(config, status, csrf, Some(error))
}

fn render(
    config: &SelfUpdateConfig,
    status: &UpdateStatus,
    csrf: &str,
    error: Option<&str>,
) -> Markup {
    html! {
        div.card #self-update-card
            hx-get="/settings/self-update" hx-trigger="sse:self-update-changed delay:300ms" hx-swap="outerHTML" {

            p.selfupdate-version { "Current version: " code { (env!("CARGO_PKG_VERSION")) } }

            (status_line(status, csrf))

            @if let Some(msg) = error { (banner(BannerKind::Error, msg)) }

            form.settings-form
                hx-post="/settings/self-update" hx-target="#self-update-card" hx-swap="outerHTML" {
                (csrf_input(csrf))
                div.field {
                    label { "Update checks" }
                    (mode_toggle(config.mode))
                    dl.selfupdate-mode-hints {
                        dt { code { "Off" } } dd { "Never checks." }
                        dt { code { "Notify" } }
                        dd { "Shows a banner here when a newer version exists, then lets you "
                             "download and install it on your own schedule." }
                        dt { code { "Auto" } }
                        dd { "Downloads and installs automatically, restarting sooth when it does." }
                    }
                }
                div.field {
                    label for="self_update_poll_interval_secs" { "Check every" }
                    select.input id="self_update_poll_interval_secs" name="poll_interval_secs" {
                        (interval_options(config.poll_interval_secs))
                    }
                }
                div.field {
                    label for="self_update_repo" { "GitHub repository" }
                    input.input type="text" id="self_update_repo" name="repo" value=(config.repo)
                        placeholder="owner/name";
                    p.field-hint { "Point this at your own fork if it publishes its own releases." }
                }
                button.btn.btn-primary type="submit" { "Save" }
            }
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
/// dropdown.
fn mode_toggle(current: UpdateMode) -> Markup {
    let segment = |value: &'static str, id: &'static str, mode: UpdateMode, label: &'static str| {
        html! {
            input type="radio" id=(id) name="mode" value=(value) checked[current == mode];
            label for=(id) { (label) }
        }
    };
    html! {
        div.segmented role="radiogroup" aria-label="Update checks" {
            (segment("off", "self_update_mode_off", UpdateMode::Off, "Off"))
            (segment("notify", "self_update_mode_notify", UpdateMode::Notify, "Notify"))
            (segment("auto", "self_update_mode_auto", UpdateMode::Auto, "Auto"))
        }
    }
}

/// The interval `<select>`'s options. When `current` doesn't match any
/// preset (e.g. a value hand-edited into the TOML file, or a future preset
/// list that no longer includes it), an extra option holding that exact
/// value is appended and selected, so saving the form without touching this
/// field can never silently change it.
fn interval_options(current: u64) -> Markup {
    html! {
        @for &(secs, label) in INTERVAL_PRESETS {
            option value=(secs) selected[secs == current] { "Every " (label) }
        }
        @if !INTERVAL_PRESETS.iter().any(|&(secs, _)| secs == current) {
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
                    form.inline-form hx-post="/settings/self-update/download" hx-swap="none" {
                        (csrf_input(csrf))
                        button.btn.btn-primary type="submit" { "Download update" }
                    }
                }
                UpdateState::ReadyToRestart { .. } => {
                    // Installing a downloaded update is just an ordinary
                    // restart -- same action as the Restart card further
                    // down the page, not a distinct self-update step.
                    form.inline-form method="post" action="/settings/restart" {
                        (csrf_input(csrf))
                        button.btn.btn-primary type="submit" { "Install and restart" }
                    }
                }
                UpdateState::Checking | UpdateState::Downloading => {}
                _ => {
                    form.inline-form hx-post="/settings/self-update/check" hx-swap="none" {
                        (csrf_input(csrf))
                        button.btn.btn-sm type="submit" { "Check now" }
                    }
                }
            }
        }
    }
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
