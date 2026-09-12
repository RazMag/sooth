use maud::{DOCTYPE, Markup, html};

use super::{BannerKind, Icon, NavItem, banner, csrf_input, icon, page_header, shell};
use crate::config::Config;
use crate::health::Health;

/// The values a settings form round-trips as plain strings (so a rejected
/// submission can be redisplayed exactly as typed, same pattern as the
/// quadlet create/edit forms).
pub struct FormValues {
    pub bind_addr: String,
    pub quadlet_dir: String,
    pub cookie_secure: bool,
    pub log_filter: String,
    pub session_idle_timeout_secs: String,
}

impl FormValues {
    pub fn from_config(config: &Config) -> Self {
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
        }
    }
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

pub fn page(
    values: &FormValues,
    locks: &EnvLocks,
    config_path: &str,
    csrf: &str,
    message: Option<&str>,
    error: Option<&str>,
    health: Health,
) -> Markup {
    let body = html! {
        (page_header("Settings", html! {}))
        p.page-meta { "Written to " code { (config_path) } }

        @if let Some(msg) = message { (banner(BannerKind::Success, msg)) }
        @if let Some(msg) = error { (banner(BannerKind::Error, msg)) }

        form.settings-form method="post" action="/settings" {
            (csrf_input(csrf))

            h2 { "Server" }
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

            button.btn.btn-primary type="submit" { "Save" }
            p.field-hint { "Changes take effect on the next restart of sooth." }
        }

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

        h2 { "Restart" }
        div.card {
            p.field-hint {
                "Restarts sooth in place to apply everything saved above. In-memory "
                "sessions are cleared (everyone signs in again) and the dashboard is "
                "briefly unavailable while it comes back."
            }
            form method="post" action="/settings/restart" {
                (csrf_input(csrf))
                button.btn.btn-restart type="submit" { "Restart sooth now" }
            }
        }

        h2 { "Password" }
        div.card {
            @if locks.auth_password_hash {
                p.field-note-env {
                    "The login password is set by the " code { "SOOTH_AUTH_PASSWORD_HASH" }
                    " environment variable. Change it there and restart."
                }
            } @else {
                form.settings-form method="post" action="/settings/password" {
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
