use std::path::Path;

use maud::{DOCTYPE, Markup, html};

use super::{BannerKind, Icon, NavItem, banner, csrf_input, icon, page_header, selfupdate, shell};
use crate::config::Config;
use crate::health::Health;
use crate::selfupdate::{SelfUpdateConfig, UpdateStatus};

/// The values a settings form round-trips as plain strings (so a rejected
/// submission can be redisplayed exactly as typed, same pattern as the
/// quadlet create/edit forms). Includes the self-update fields -- they're
/// part of this one form now, not a form of their own.
pub struct FormValues {
    pub bind_addr: String,
    pub quadlet_dir: String,
    pub cookie_secure: bool,
    pub log_filter: String,
    pub session_idle_timeout_secs: String,
    pub self_update_mode: String,
    pub self_update_poll_interval_secs: String,
    pub self_update_repo: String,
    /// Active vs. saved GitHub token -- never the token itself, which
    /// (unlike every other field here) is never round-tripped into the
    /// rendered page.
    pub github_token: GithubTokenStatus,
}

impl FormValues {
    /// `self_update` comes from `AppState.self_update`'s own live snapshot,
    /// not `config.self_update` -- unlike the rest of `Config`, self-update
    /// settings apply immediately rather than only on the next load, so
    /// `state.config` (frozen at startup) would show a saved change as
    /// reverted until a restart.
    pub fn from_config(
        config: &Config,
        config_path: &Path,
        self_update: &SelfUpdateConfig,
    ) -> Self {
        let quadlet_dir = config
            .quadlet_dir
            .clone()
            .or_else(|| crate::quadlet::discovery::default_quadlet_dir().ok())
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        Self {
            bind_addr: config.bind_addr.to_string(),
            quadlet_dir,
            cookie_secure: config.cookie_secure,
            log_filter: config.log_filter.clone().unwrap_or_default(),
            session_idle_timeout_secs: config.session_idle_timeout_secs.to_string(),
            self_update_mode: self_update.mode.as_str().to_string(),
            self_update_poll_interval_secs: self_update.poll_interval_secs.to_string(),
            self_update_repo: self_update.repo.clone(),
            github_token: GithubTokenStatus::detect(config, config_path),
        }
    }
}

/// Whether a GitHub access token is in use, and whether the config file
/// holds a different one than the running process loaded. `Config` is
/// frozen at startup, so a token saved from Settings only applies after a
/// restart -- reading the file too is what lets the page say "saved,
/// restart to apply" instead of still claiming no token is saved.
///
/// Only last-4 suffixes are kept, for on-screen identification (the same
/// tradeoff most services make showing a card's last 4 digits).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GithubTokenStatus {
    /// Suffix of the token the running process uses, if any.
    pub active: Option<String>,
    /// Suffix of the token in the config file, if any. Equal to `active`
    /// unless a save is waiting for a restart.
    pub saved: Option<String>,
    /// The saved and active tokens differ (a restart would change which
    /// one git-sync uses). Always `false` when the env var pins it.
    pub pending_restart: bool,
    /// Pinned by `SOOTH_GITHUB_TOKEN` -- the config file is then ignored.
    pub env_locked: bool,
}

impl GithubTokenStatus {
    pub fn detect(config: &Config, config_path: &Path) -> Self {
        let env_locked = EnvLocks::detect().github_token;
        let active = config.github_token.trim();
        let saved = if env_locked {
            active.to_string()
        } else {
            // An unreadable file can't have been saved to either -- fall
            // back to "same as active" rather than inventing a pending change.
            crate::config::read_config_toml(config_path)
                .ok()
                .map(|t| {
                    t.get("github_token")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .trim()
                        .to_string()
                })
                .unwrap_or_else(|| active.to_string())
        };
        Self::from_tokens(active, &saved, env_locked)
    }

    fn from_tokens(active: &str, saved: &str, env_locked: bool) -> Self {
        let suffix = |t: &str| (!t.is_empty()).then(|| last4(t));
        Self {
            active: suffix(active),
            saved: suffix(saved),
            pending_restart: !env_locked && active != saved,
            env_locked,
        }
    }
}

/// One-line description of a [`GithubTokenStatus`], shared by the Settings
/// page's "Security & sessions" GitHub token card and the Git Sync add form.
pub fn github_token_summary(status: &GithubTokenStatus) -> Markup {
    let ending = |s: &str| html! { " ending in " code { "…" (s) } };
    html! {
        @match (&status.active, status.pending_restart, &status.saved) {
            (Some(a), false, _) => {
                span.badge.badge-running { "Token active" }
                " A GitHub token" (ending(a)) " is in use"
                @if status.env_locked { " (from " code { "SOOTH_GITHUB_TOKEN" } ")" } "."
            }
            (None, false, _) => {
                span.badge.badge-stopped { "No token" }
                " No GitHub token is saved."
            }
            (_, true, Some(s)) => {
                span.badge.badge-warn { "Restart needed" }
                " A new GitHub token" (ending(s)) " is saved but not in use until sooth restarts"
                @if let Some(a) = &status.active { " (still using the one" (ending(a)) ")" } "."
            }
            (_, true, None) => {
                span.badge.badge-warn { "Restart needed" }
                " The GitHub token was removed, but sooth keeps using it until it restarts."
            }
        }
    }
}

/// The last 4 characters of `token`, or the whole thing if it's shorter than
/// that -- for the "which token is this" hint next to the Settings page's
/// GitHub token field. Empty in, empty out.
fn last4(token: &str) -> String {
    let len = token.chars().count();
    token.chars().skip(len.saturating_sub(4)).collect()
}

/// Which settings are currently pinned by a `SOOTH_*` environment variable.
/// A pinned value wins over the TOML file on every startup (see
/// `Config::load`), so the form marks those fields read-only and says so
/// rather than letting the operator "save" a change that silently never
/// applies.
pub struct EnvLocks {
    pub bind_addr: bool,
    pub quadlet_dir: bool,
    pub cookie_secure: bool,
    pub log_filter: bool,
    pub session_idle_timeout_secs: bool,
    pub auth_password_hash: bool,
    pub github_token: bool,
}

impl EnvLocks {
    pub fn detect() -> Self {
        let set = |key: &str| std::env::var_os(key).is_some();
        Self {
            bind_addr: set("SOOTH_BIND_ADDR"),
            quadlet_dir: set("SOOTH_QUADLET_DIR"),
            cookie_secure: set("SOOTH_COOKIE_SECURE"),
            log_filter: set("SOOTH_LOG_FILTER"),
            session_idle_timeout_secs: set("SOOTH_SESSION_IDLE_TIMEOUT_SECS"),
            auth_password_hash: set("SOOTH_AUTH_PASSWORD_HASH"),
            github_token: set("SOOTH_GITHUB_TOKEN"),
        }
    }
}

fn env_note(env_var: &str) -> Markup {
    html! {
        p.field-note-env {
            "Set by the " code { (env_var) } " environment variable — changes here are ignored."
        }
    }
}

/// One labelled text field with a description line above the input and an
/// optional "pinned by an env var" note below it.
fn text_field(
    id: &str,
    label: &str,
    value: &str,
    hint: Markup,
    locked: bool,
    env_var: &str,
    placeholder: Option<&str>,
) -> Markup {
    html! {
        div.field {
            label for=(id) { (label) }
            p.field-hint { (hint) }
            input.input type="text" id=(id) name=(id) value=(value)
                placeholder=[placeholder] readonly[locked];
            @if locked { (env_note(env_var)) }
        }
    }
}

/// The page's live (as opposed to persisted-config) status, bundled into one
/// argument so `page`'s parameter count stays sane.
pub struct LiveStatus<'a> {
    pub health: Health,
    pub self_update_status: &'a UpdateStatus,
}

pub fn page(
    values: &FormValues,
    locks: &EnvLocks,
    config_path: &str,
    csrf: &str,
    message: Option<&str>,
    error: Option<&str>,
    live: LiveStatus,
) -> Markup {
    let LiveStatus {
        health,
        self_update_status,
    } = live;
    let body = html! {
        (page_header("Settings", html! {}))
        p.page-meta { "Written to " code { (config_path) } }

        @if let Some(msg) = message { (banner(BannerKind::Success, msg)) }
        @if let Some(msg) = error { (banner(BannerKind::Error, msg)) }

        // Shown/hidden by `initSettingsForm` in settings.js, same dirty
        // check that gates the Save button -- this just makes that state
        // visible up here too, since the button itself is all the way down
        // past every section by the time there's something to save.
        div.banner.banner-warn #settings-unsaved-banner hidden aria-live="polite" {
            "You have unsaved changes below."
        }

        section.settings-section {
            div.section-header {
                h2 { "System" }
                div.actions {
                    form.inline-form hx-post="/settings/health/refresh" hx-target="#health-card" hx-swap="outerHTML" {
                        (csrf_input(csrf))
                        button.btn.btn-sm type="submit" { (icon(Icon::Refresh)) span { "Refresh" } }
                    }
                }
            }
            (health_card(health))
        }

        // `autocomplete="off"` -- not about password managers here, but
        // about a *different* browser feature with the same attribute:
        // Chrome (and others) silently restore whatever a form's fields were
        // set to before you navigated away, reapplying it on top of the
        // freshly served page when you come back via back/forward. Without
        // this, that's indistinguishable from the real saved config -- see
        // the bug note on `initSettingsForm` in `settings.js` for the other
        // half of this fix (bfcache doing the same thing a different way).
        form #settings-form autocomplete="off" method="post" action="/settings" {
            (csrf_input(csrf))

            section.settings-section {
                h2 { "Server & network" }
                div.card {
                    (text_field(
                        "bind_addr", "Bind address", &values.bind_addr,
                        html! {
                            "Address and port the dashboard listens on. "
                            code { "127.0.0.1" } " keeps it local to this machine; "
                            code { "0.0.0.0" } " exposes it on the network."
                        },
                        locks.bind_addr, "SOOTH_BIND_ADDR", None,
                    ))
                }
            }

            section.settings-section {
                h2 { "Filesystem" }
                div.card {
                    (text_field(
                        "quadlet_dir", "Quadlet directory", &values.quadlet_dir,
                        html! {
                            "Folder sooth reads " code { ".container" } " / " code { ".pod" } " / "
                            code { ".volume" } " / … unit files from. Leave as the default unless "
                            "your quadlets live elsewhere."
                        },
                        locks.quadlet_dir, "SOOTH_QUADLET_DIR", None,
                    ))
                }
            }

            section.settings-section {
                h2 { "Security & sessions" }
                div.card {
                    div.field {
                        label.checkbox-line for="cookie_secure" {
                            input type="checkbox" id="cookie_secure" name="cookie_secure"
                                checked[values.cookie_secure];
                            "Require HTTPS for the session cookie"
                        }
                        p.field-hint {
                            "Adds the " code { "Secure" } " flag to the login cookie so browsers only "
                            "send it over HTTPS. Turn on when sooth runs behind a TLS-terminating "
                            "reverse proxy; leave off for plain-HTTP localhost."
                        }
                        @if locks.cookie_secure { (env_note("SOOTH_COOKIE_SECURE")) }
                    }
                    (text_field(
                        "session_idle_timeout_secs", "Session idle timeout (seconds)",
                        &values.session_idle_timeout_secs,
                        html! {
                            "How long a login stays valid with no activity. Default "
                            code { "43200" } " (12 hours)."
                        },
                        locks.session_idle_timeout_secs, "SOOTH_SESSION_IDLE_TIMEOUT_SECS", None,
                    ))
                }
                // The GitHub token saves through its own route (like the
                // password), not "Save changes" -- so its controls are
                // associated via `form=` with `#github-token-form`, which
                // lives outside `#settings-form` (forms can't nest). That
                // also keeps them out of `settings.js`'s dirty check, which
                // only looks at `#settings-form`'s own elements.
                div.card #github-token-card {
                    h3 { "GitHub access token" }
                    p.field-hint {
                        "Used to clone/fetch a " a href="/git-sync" { "git-synced" }
                        " group's remote when it's a private " code { "https://github.com/..." }
                        " repository. Public repos and non-GitHub remotes don't need this -- and "
                        "this token is only ever sent to " code { "github.com" } ", never to some "
                        "other host a sync happens to point at."
                    }
                    p.field-hint #github-token-status { (github_token_summary(&values.github_token)) }
                    @if locks.github_token {
                        (env_note("SOOTH_GITHUB_TOKEN"))
                    } @else {
                        div.field {
                            label for="github_token" { "New token" }
                            input.input type="password" id="github_token" name="github_token"
                                form="github-token-form" autocomplete="off" placeholder="ghp_…";
                            p.field-hint {
                                "Leave blank and save to remove the saved token. Applied the "
                                "next time sooth restarts."
                            }
                        }
                        button.btn.btn-primary type="submit" form="github-token-form" { "Save token" }
                    }
                }
            }

            section.settings-section {
                h2 { "Updates" }
                div.card {
                    (selfupdate::status_fragment(self_update_status, csrf))
                    div.field {
                        label { "Update checks" }
                        (selfupdate::mode_toggle(&values.self_update_mode))
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
                            (selfupdate::interval_options(&values.self_update_poll_interval_secs))
                        }
                    }
                    div.field {
                        label for="self_update_repo" { "GitHub repository" }
                        input.input type="text" id="self_update_repo" name="repo"
                            value=(values.self_update_repo) placeholder="owner/name";
                        p.field-hint { "Point this at your own fork if it publishes its own releases." }
                    }
                    p.field-hint { "Applies immediately -- no restart needed." }
                }
            }

            section.settings-section {
                h2 { "Diagnostics" }
                div.card {
                    (text_field(
                        "log_filter", "Log filter", &values.log_filter,
                        html! {
                            "A " code { "tracing" } " " code { "EnvFilter" } " directive string "
                            "controlling log verbosity. Empty falls back to the default."
                        },
                        locks.log_filter, "SOOTH_LOG_FILTER",
                        Some("sooth=info,tower_http=info,zbus=warn"),
                    ))
                }
            }

            section.settings-section {
                div.card.settings-save-card {
                    p.field-hint {
                        "Everything above is saved together. Server, filesystem, security, "
                        "and diagnostics changes take effect on the next restart of sooth; "
                        "Updates changes apply immediately."
                    }
                    button #settings-save.btn.btn-primary type="submit" { "Save changes" }
                }
            }
        }

        @if !locks.github_token {
            form #github-token-form autocomplete="off" method="post" action="/settings/github-token" {
                (csrf_input(csrf))
            }
        }

        section.settings-section {
            h2 { "Restart" }
            div.card {
                p.field-hint {
                    "Restarts sooth in place to apply everything saved above. In-memory "
                    "sessions are cleared (everyone signs in again) and the dashboard is "
                    "briefly unavailable while it comes back."
                }
                form #restart-form method="post" action="/settings/restart" {
                    (csrf_input(csrf))
                    button.btn.btn-restart type="submit" { "Restart sooth now" }
                }
            }
        }

        section.settings-section {
            h2 { "Password" }
            div.card {
                @if locks.auth_password_hash {
                    p.field-note-env {
                        "The login password is set by the " code { "SOOTH_AUTH_PASSWORD_HASH" }
                        " environment variable. Change it there and restart."
                    }
                } @else {
                    form method="post" action="/settings/password" {
                        (csrf_input(csrf))
                        p.field-hint {
                            "Sets a new dashboard login password. Saved to the config file and "
                            "applied the next time sooth restarts."
                        }
                        div.field {
                            label for="current_password" { "Current password" }
                            input.input type="password" id="current_password" name="current_password"
                                autocomplete="current-password" required;
                        }
                        div.field {
                            label for="new_password" { "New password" }
                            input.input type="password" id="new_password" name="new_password"
                                autocomplete="new-password" minlength="8" required;
                        }
                        div.field {
                            label for="confirm_password" { "Confirm new password" }
                            input.input type="password" id="confirm_password" name="confirm_password"
                                autocomplete="new-password" minlength="8" required;
                        }
                        button.btn.btn-primary type="submit" { "Change password" }
                    }
                }
            }
        }
    };
    shell("Settings", Some(NavItem::Settings), Some(health), body)
}

/// The connection to the systemd user session bus is a startup precondition
/// (sooth can't be running this page without it), so it's not re-checked
/// here -- just the two dependencies that degrade gracefully instead. See
/// `crate::health` and `health_banners` for the checks themselves.
pub fn health_card(health: Health) -> Markup {
    html! {
        div.card #health-card {
            table.kv-table {
                tr {
                    td { "systemd user session" }
                    td { (status_pill(true, "Connected", "")) }
                }
                tr {
                    td { "Podman quadlet generator" }
                    @if health.podman_generator_found {
                        td { (status_pill(true, "Found", "")) }
                    } @else {
                        td {
                            (status_pill(false, "Not found", ""))
                            p.field-hint {
                                "Create/edit will skip dry-run validation against it; structural "
                                "checks still run."
                            }
                        }
                    }
                }
                tr {
                    td { "Linger (survives logout)" }
                    @match health.linger_enabled {
                        Some(true) => td { (status_pill(true, "Enabled", "")) },
                        Some(false) => td {
                            (status_pill(false, "Disabled", ""))
                            p.field-hint {
                                "sooth and everything it manages will stop when you log out. Run "
                                code { "loginctl enable-linger $USER" } " to keep them running."
                            }
                        },
                        None => td { (status_pill(false, "Unknown", "could not determine the current user")) },
                    }
                }
            }
        }
    }
}

fn status_pill(ok: bool, label: &str, title: &str) -> Markup {
    let variant = if ok { "badge-running" } else { "badge-warn" };
    html! { span class={"badge " (variant)} title=(title) { (label) } }
}

/// Shown after the "Restart" button: a standalone card (the app is winding
/// down, so no sidebar/SSE shell) that polls `/healthz` and returns to the
/// dashboard once the fresh process answers.
pub fn restarting_page() -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Restarting · sooth" }
                link rel="stylesheet" href="/static/style.css";
                noscript { meta http-equiv="refresh" content="6;url=/"; }
            }
            body.login-body {
                main.login-card {
                    h1 { "Restarting…" }
                    p.field-hint {
                        "sooth is restarting to apply your saved settings. This page "
                        "returns to the dashboard automatically once it's back."
                    }
                }
                script {
                    (maud::PreEscaped(
                        "(function(){function ping(){\
                         fetch('/healthz',{cache:'no-store'}).then(function(r){\
                         if(r.ok){location.href='/';}else{setTimeout(ping,1000);}\
                         }).catch(function(){setTimeout(ping,1000);});}\
                         setTimeout(ping,2500);})()"
                    ))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GithubTokenStatus;

    #[test]
    fn token_status_matches_when_saved_equals_active() {
        let s = GithubTokenStatus::from_tokens("ghp_abcd1234", "ghp_abcd1234", false);
        assert_eq!(s.active.as_deref(), Some("1234"));
        assert!(!s.pending_restart);
    }

    #[test]
    fn token_status_flags_a_save_awaiting_restart() {
        let s = GithubTokenStatus::from_tokens("", "ghp_abcd1234", false);
        assert_eq!(s.active, None);
        assert_eq!(s.saved.as_deref(), Some("1234"));
        assert!(s.pending_restart);

        let removed = GithubTokenStatus::from_tokens("ghp_abcd1234", "", false);
        assert!(removed.pending_restart);
        assert_eq!(removed.saved, None);
    }

    #[test]
    fn token_status_never_pending_when_env_locked() {
        let s = GithubTokenStatus::from_tokens("ghp_abcd1234", "", true);
        assert!(!s.pending_restart);
    }
}
