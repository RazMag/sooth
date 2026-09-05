use maud::{Markup, html};

use super::{BannerKind, NavItem, banner, csrf_input, page_header, shell};
use crate::config::Config;

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

pub fn page(
    values: &FormValues,
    config_path: &str,
    csrf: &str,
    message: Option<&str>,
    error: Option<&str>,
) -> Markup {
    let body = html! {
        (page_header("Settings", html! {}))
        p.page-meta { "Written to " code { (config_path) } }

        @if let Some(msg) = message { (banner(BannerKind::Success, msg)) }
        @if let Some(msg) = error { (banner(BannerKind::Error, msg)) }
        (banner(
            BannerKind::Info,
            "A value also set via a SOOTH_* environment variable keeps overriding whatever is saved here, even after a restart.",
        ))

        form method="post" action="/settings" {
            (csrf_input(csrf))
            div.field {
                label for="bind_addr" { "Bind address" }
                input.input type="text" id="bind_addr" name="bind_addr" value=(values.bind_addr);
            }
            div.field {
                label for="quadlet_dir" { "Quadlet directory" }
                input.input type="text" id="quadlet_dir" name="quadlet_dir" value=(values.quadlet_dir);
            }
            div.field.field-inline {
                input type="checkbox" id="cookie_secure" name="cookie_secure" checked[values.cookie_secure];
                label for="cookie_secure" { "Require HTTPS for the session cookie" }
            }
            div.field {
                label for="log_filter" { "Log filter" }
                input.input type="text" id="log_filter" name="log_filter" value=(values.log_filter)
                    placeholder="sooth=info,tower_http=info,zbus=warn";
            }
            div.field {
                label for="session_idle_timeout_secs" { "Session idle timeout (seconds)" }
                input.input type="text" id="session_idle_timeout_secs" name="session_idle_timeout_secs"
                    value=(values.session_idle_timeout_secs);
            }
            button.btn.btn-primary type="submit" { "Save" }
        }
        p.field-hint { "Changes take effect on the next restart of sooth." }
    };
    shell("Settings", Some(NavItem::Settings), body)
}
