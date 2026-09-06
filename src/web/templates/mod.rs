//! Shared rendering: the page shell/sidebar, and small widgets reused by
//! every section (status badges, action/delete forms, the kebab menu, error
//! pages). Section-specific templates live in sibling modules.

pub mod containers;
pub mod detail;
pub mod environment;
pub mod generic;
pub mod icons;
pub mod images;
pub mod list;
pub mod logs;
pub mod networks;
pub mod pods;
pub mod ports;
pub mod services;
pub mod settings;
pub mod volumes;

use axum::http::StatusCode;
use maud::{DOCTYPE, Markup, html};
use uuid::Uuid;

use crate::quadlet::{QuadletUnit, UnitKind};
use crate::systemd::UnitStatus;
use crate::web::core;

pub use icons::{Icon, icon};

/// Which sidebar link (if any) is "active" for the current page. `None` for
/// pages outside the sidebar entirely (login, the generic `/units` fallback,
/// error pages).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavItem {
    Services,
    Volumes,
    Networks,
    Images,
    Ports,
    Environment,
    /// Not part of `all()` -- rendered as its own control in the sidebar
    /// footer, not the main nav list.
    Settings,
}

impl NavItem {
    fn href(self) -> &'static str {
        match self {
            NavItem::Services => "/",
            NavItem::Volumes => "/volumes",
            NavItem::Networks => "/networks",
            NavItem::Images => "/images",
            NavItem::Ports => "/ports",
            NavItem::Environment => "/environment",
            NavItem::Settings => "/settings",
        }
    }

    fn label(self) -> &'static str {
        match self {
            NavItem::Services => "Services",
            NavItem::Volumes => "Volumes",
            NavItem::Networks => "Networks",
            NavItem::Images => "Images",
            NavItem::Ports => "Ports",
            NavItem::Environment => "Environment",
            NavItem::Settings => "Settings",
        }
    }

    fn nav_icon(self) -> Icon {
        match self {
            NavItem::Services => Icon::Services,
            NavItem::Volumes => Icon::Volumes,
            NavItem::Networks => Icon::Networks,
            NavItem::Images => Icon::Images,
            NavItem::Ports => Icon::Ports,
            NavItem::Environment => Icon::Environment,
            NavItem::Settings => Icon::Settings,
        }
    }

    fn all() -> [NavItem; 6] {
        [
            NavItem::Services,
            NavItem::Volumes,
            NavItem::Networks,
            NavItem::Images,
            NavItem::Ports,
            NavItem::Environment,
        ]
    }

    /// The sidebar item to highlight for a unit of a given kind -- the nav
    /// analogue of `core::section_path`, so a detail page's active link
    /// follows the loaded unit's real kind. `None` for Kube (no section).
    pub fn for_kind(kind: UnitKind) -> Option<NavItem> {
        match kind {
            UnitKind::Container | UnitKind::Pod => Some(NavItem::Services),
            UnitKind::Volume => Some(NavItem::Volumes),
            UnitKind::Network => Some(NavItem::Networks),
            UnitKind::Image | UnitKind::Build => Some(NavItem::Images),
            UnitKind::Kube => None,
        }
    }
}

/// The `<head>` shared by every full page. The bundled `app.js` carries htmx,
/// its SSE extension, and CodeMirror; the inline script resolves an effective
/// light/dark theme and stamps it on `<html>` before first paint (a deferred
/// script would run too late and flash the wrong theme).
fn head_tag(title: &str) -> Markup {
    html! {
        head {
            meta charset="utf-8";
            meta name="viewport" content="width=device-width, initial-scale=1";
            title { (title) " · sooth" }
            script {
                (maud::PreEscaped(
                    "try{var t=localStorage.getItem('sooth-theme');\
                     if(t!=='light'&&t!=='dark')t=matchMedia('(prefers-color-scheme: dark)').matches?'dark':'light';\
                     document.documentElement.dataset.theme=t;}catch(e){}"
                ))
            }
            link rel="stylesheet" href="/static/style.css";
            script src="/static/app.js" defer {}
        }
    }
}

/// The page shell: doctype, head, sidebar, and the page's own content.
/// `active` highlights the current sidebar link; pages outside the sidebar
/// pass `None`.
pub fn shell(title: &str, active: Option<NavItem>, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            (head_tag(title))
            body hx-ext="sse" sse-connect="/events" {
                div.app-shell {
                    div.nav-scrim data-nav-scrim {}
                    nav.sidebar {
                        a.brand href="/" { "sooth" }
                        ul.nav-list {
                            @for item in NavItem::all() {
                                li {
                                    a href=(item.href())
                                        aria-current=[active.filter(|a| *a == item).map(|_| "page")] {
                                        (icon(item.nav_icon())) span { (item.label()) }
                                    }
                                }
                            }
                        }
                        div.sidebar-footer {
                            a.btn-icon href="/settings"
                                aria-current=[(active == Some(NavItem::Settings)).then_some("page")]
                                title="Settings" { (icon(Icon::Settings)) }
                            button.btn-icon.theme-toggle type="button" data-theme-toggle
                                title="Toggle light/dark theme" aria-label="Toggle light/dark theme" {
                                span.i-sun { (icon(Icon::Sun)) }
                                span.i-moon { (icon(Icon::Moon)) }
                            }
                            span.spacer {}
                            form method="post" action="/logout" {
                                button.btn-link type="submit" { (icon(Icon::LogOut)) span { "Log out" } }
                            }
                        }
                    }
                    div.content {
                        div.content-topbar {
                            button.btn-icon type="button" data-nav-toggle aria-label="Open menu" {
                                (icon(Icon::Menu))
                            }
                            a.brand href="/" { "sooth" }
                        }
                        div.content-inner { (body) }
                    }
                }
            }
        }
    }
}

pub fn csrf_input(csrf: &str) -> Markup {
    html! { input type="hidden" name="csrf_token" value=(csrf); }
}

/// A page's title bar: `<h1>` plus a right-aligned slot for actions (a
/// "+ New" button, a status badge, ...). Used by every full page so headers
/// stay identical across sections.
pub fn page_header(title: &str, actions: Markup) -> Markup {
    html! {
        div.page-header {
            h1 { (title) }
            div.actions { (actions) }
        }
    }
}

#[derive(Clone, Copy)]
pub enum BannerKind {
    Error,
    Success,
    Info,
}

pub fn banner(kind: BannerKind, message: &str) -> Markup {
    let class = match kind {
        BannerKind::Error => "banner banner-error",
        BannerKind::Success => "banner banner-success",
        BannerKind::Info => "banner banner-info",
    };
    html! { div class=(class) { (message) } }
}

/// A systemd service name like `myapp.service` is not a valid bare CSS
/// identifier (the `.` reads as a class selector to `querySelector`, and
/// instance units can contain `@`), so it can't be used directly in an
/// `id`/`hx-target` pair -- only in places matched by plain string equality
/// (SSE event names). This gives every unit a stable, selector-safe DOM id.
pub fn dom_id(service: &str) -> String {
    let safe: String = service
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("status-{safe}")
}

/// A friendly label + `.badge-*` variant for a unit's live state. The raw
/// systemd `active/sub` states are kept in the badge's `title`.
fn status_display(status: &UnitStatus) -> (&'static str, &'static str) {
    if status.is_failed() {
        ("badge-failed", "Failed")
    } else if status.is_active() {
        ("badge-running", "Running")
    } else if status.load_state == "not-found" {
        ("badge-unknown", "Not loaded")
    } else {
        match status.active_state.as_str() {
            "activating" => ("badge-stopped", "Starting"),
            "deactivating" => ("badge-stopped", "Stopping"),
            _ => ("badge-stopped", "Stopped"),
        }
    }
}

/// The live-updating status badge for one unit. Rendered inline on
/// list/detail pages and as the payload of `status-{service}` SSE events, so
/// both paths converge on identical markup.
pub fn status_badge(service: &str, status: &UnitStatus) -> Markup {
    let (variant, label) = status_display(status);
    let raw = format!("{}/{}", status.active_state, status.sub_state);
    html! {
        span id=(dom_id(service)) class={"badge " (variant)} title=(raw)
            sse-swap={"status-" (service)} hx-swap="outerHTML" {
            (label)
            @if status.is_enabled() { span.chip { "enabled" } }
        }
    }
}

/// One systemd action, as a small htmx form that swaps the freshly rendered
/// status badge in place. `btn_class` is empty inside the kebab menu (styled
/// by `.menu-panel button`) and a `.btn` combo on detail pages.
fn action_form(
    base_url: &str,
    service: &str,
    action: &str,
    label: &str,
    ic: Icon,
    csrf: &str,
    btn_class: &str,
) -> Markup {
    html! {
        form.inline-form hx-post={(base_url) "/" (action)} hx-target={"#" (dom_id(service))} hx-swap="outerHTML" {
            (csrf_input(csrf))
            button class=[(!btn_class.is_empty()).then_some(btn_class)] type="submit" {
                (icon(ic)) span { (label) }
            }
        }
    }
}

fn delete_form(base_url: &str, csrf: &str, btn_class: &str) -> Markup {
    html! {
        form.inline-form
            method="post"
            action={(base_url) "/delete"}
            onsubmit="return confirm('Delete this quadlet file? This cannot be undone.')" {
            (csrf_input(csrf))
            button class=[(!btn_class.is_empty()).then_some(btn_class)] type="submit" {
                (icon(Icon::Trash)) span { "Delete" }
            }
        }
    }
}

/// The per-row quick-actions menu: a native `<details>` disclosure, no JS
/// needed. Every URL is built from `core::unit_url`, so this works
/// identically regardless of which section the unit belongs to.
pub fn kebab_menu(unit: &QuadletUnit, status: &UnitStatus, csrf: &str) -> Markup {
    let base = core::unit_url(unit);
    let service = unit.service_name();
    html! {
        details.menu {
            summary aria-label="Actions" { (icon(Icon::More)) }
            div.menu-panel {
                @if !unit.is_template() {
                    @if status.is_active() {
                        (action_form(&base, &service, "stop", "Stop", Icon::Stop, csrf, ""))
                        (action_form(&base, &service, "restart", "Restart", Icon::Restart, csrf, ""))
                    } @else {
                        (action_form(&base, &service, "start", "Start", Icon::Play, csrf, ""))
                    }
                    @if status.is_enabled() {
                        (action_form(&base, &service, "disable", "Disable", Icon::Power, csrf, ""))
                    } @else {
                        (action_form(&base, &service, "enable", "Enable", Icon::Power, csrf, ""))
                    }
                }
                a href={(base) "/edit"} { (icon(Icon::Edit)) span { "Edit" } }
                a href={(base) "/logs"} { (icon(Icon::Logs)) span { "Logs" } }
                (delete_form(&base, csrf, "danger"))
            }
        }
    }
}

/// The same actions as `kebab_menu`, laid out as plain buttons -- used on
/// detail pages where there's room and the extra visibility is welcome.
pub fn action_row(unit: &QuadletUnit, status: &UnitStatus, csrf: &str) -> Markup {
    let base = core::unit_url(unit);
    let service = unit.service_name();
    let ghost = "btn btn-ghost btn-sm";
    html! {
        div.action-row {
            @if status.is_active() {
                (action_form(&base, &service, "stop", "Stop", Icon::Stop, csrf, ghost))
                (action_form(&base, &service, "restart", "Restart", Icon::Restart, csrf, ghost))
            } @else {
                (action_form(&base, &service, "start", "Start", Icon::Play, csrf, "btn btn-primary btn-sm"))
            }
            @if status.is_enabled() {
                (action_form(&base, &service, "disable", "Disable", Icon::Power, csrf, ghost))
            } @else {
                (action_form(&base, &service, "enable", "Enable", Icon::Power, csrf, ghost))
            }
        }
    }
}

/// The Edit / Logs / Delete row shown on every detail page.
pub fn detail_links(base_url: &str, csrf: &str) -> Markup {
    html! {
        div.detail-links {
            a.btn.btn-ghost.btn-sm href={(base_url) "/edit"} { (icon(Icon::Edit)) span { "Edit" } }
            a.btn.btn-ghost.btn-sm href={(base_url) "/logs"} { (icon(Icon::Logs)) span { "Logs" } }
            (delete_form(base_url, csrf, "btn btn-danger btn-sm"))
        }
    }
}

pub fn login_page(error: Option<&str>) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            (head_tag("Sign in"))
            body.login-body {
                main.login-card {
                    h1 { "sooth" }
                    @if let Some(msg) = error { (banner(BannerKind::Error, msg)) }
                    form method="post" action="/login" {
                        div.field {
                            label for="password" { "Password" }
                            input.input type="password" id="password" name="password" required autofocus;
                        }
                        button.btn.btn-primary.btn-block type="submit" { "Sign in" }
                    }
                }
            }
        }
    }
}

/// A comma-joined summary of a Container/Pod unit's declared `PublishPort=`
/// entries, e.g. `8080:80, 53:53/udp` -- shared by the Services and Ports
/// list columns.
pub fn ports_summary(unit: &QuadletUnit) -> Markup {
    let mappings = crate::quadlet::ports::extract(std::slice::from_ref(unit));
    if mappings.is_empty() {
        return html! { span.muted { "—" } };
    }
    html! {
        @for (i, m) in mappings.iter().enumerate() {
            @if i > 0 { ", " }
            @match m.host_port {
                Some(range) if range.start == range.end => (format!("{}:{}", range.start, m.container_port)),
                Some(range) => (format!("{}-{}:{}", range.start, range.end, m.container_port)),
                None => (format!("{} (dynamic)", m.container_port)),
            }
        }
    }
}

pub fn error_page(status: StatusCode, message: &str, id: Uuid) -> Markup {
    let title = format!(
        "{} {}",
        status.as_u16(),
        status.canonical_reason().unwrap_or("Error")
    );
    let body = html! {
        (page_header(&title, html! {}))
        p.muted { (message) }
        p.muted { "Error ID: " code { (id.to_string()) } }
    };
    shell("Error", None, body)
}

/// A unit's parsed `[Section]` blocks, rendered verbatim as key/value
/// tables. Shared by the detail skeleton and the generic fallback.
pub fn section_table(unit: &QuadletUnit) -> Markup {
    html! {
        @for section in &unit.sections {
            table.kv-table {
                caption { "[" (section.name) "]" }
                @for (k, v) in &section.entries {
                    tr { td { (k) } td { (v) } }
                }
            }
        }
    }
}

/// The quadlet-content editor: a plain `<textarea>` progressively enhanced
/// into a syntax-highlighted, live-validated CodeMirror editor by the bundle.
pub fn code_editor(
    contents: &str,
    file_name_input_selector: Option<&str>,
    fixed_file_name: Option<&str>,
) -> Markup {
    html! {
        textarea.input
            name="contents"
            rows="18"
            data-code-editor
            data-file-input=[file_name_input_selector]
            data-file-name=[fixed_file_name]
            required
            { (contents) }
        div id="validate-status" class="validate-status" {}
    }
}

pub fn validate_ok() -> Markup {
    html! { div id="validate-status" class="validate-status valid" { (icon(Icon::Check)) span { "Valid" } } }
}

pub fn validate_error(message: &str) -> Markup {
    html! { div id="validate-status" class="validate-status invalid" { (icon(Icon::X)) span { (message) } } }
}

pub fn error_fragment(message: &str, id: Uuid) -> Markup {
    html! {
        div.banner.banner-error {
            (message)
            span.muted { " (id: " (id.to_string()) ")" }
        }
    }
}
