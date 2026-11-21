//! The machine scope switcher: a chip per machine above a fleet page, the
//! current one marked (see `crate::web::scope`).

use maud::{Markup, html};

/// One chip of the switcher.
pub struct ScopeChoice {
    pub href: String,
    pub label: String,
    /// Online, offline, or not known yet; `None` for "All machines".
    pub dot: Option<bool>,
    pub current: bool,
}

pub fn scope_switch(choices: &[ScopeChoice]) -> Markup {
    html! {
        nav.scope-switch aria-label="Machines" {
            @for c in choices {
                a.chip href=(c.href) aria-current=[c.current.then_some("page")] {
                    @if let Some(online) = c.dot {
                        span.machine-dot.is-online[online].is-offline[!online] aria-hidden="true" {}
                    }
                    (c.label)
                }
            }
        }
    }
}
