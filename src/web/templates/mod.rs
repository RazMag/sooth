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

use crate::hostenv::EnvVar;
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
    pub fn href(self) -> &'static str {
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

    pub fn label(self) -> &'static str {
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

/// The list page a unit of this kind belongs under, as `(href, label)` -- the
/// target of the "Back" link on that unit's detail/logs/edit pages. Kube has
/// no sidebar section, so it points at the generic `/units` list.
pub fn section_back_target(kind: UnitKind) -> (&'static str, &'static str) {
    match NavItem::for_kind(kind) {
        Some(nav) => (nav.href(), nav.label()),
        None => ("/units", "All units"),
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

/// A small "← Back to X" button, shown at the very top of a page that sits
/// below a section list in the nav tree (a unit's detail/logs/edit pages and
/// the "New" pages). The target is structural -- the page's natural parent,
/// not wherever the user actually came from.
pub fn back_link(href: &str, label: &str) -> Markup {
    html! {
        a.back-link.btn.btn-sm href=(href) {
            (icon(Icon::ArrowLeft)) span { "Back to " (label) }
        }
    }
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
        }
    }
}

/// A small "Autostart" pill shown next to a unit's actions when its quadlet
/// file carries an `[Install]` / `WantedBy=` -- the rootless stand-in for
/// `systemctl is-enabled` (which always reports `generated` for these).
pub fn autostart_pill(enabled: bool) -> Markup {
    html! {
        @if enabled {
            span.chip title="Starts on login (has an [Install] section)" { "Autostart" }
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

/// The status-dependent action forms (start-or-stop+restart, then
/// enable-or-disable) -- shared by the kebab menu and the detail-page action
/// row. `start_class` / `other_class` style the buttons: empty inside the
/// kebab (styled by `.menu-panel button`), `.btn` combos on the detail page.
fn unit_action_forms(
    base: &str,
    service: &str,
    status: &UnitStatus,
    csrf: &str,
    start_class: &str,
    other_class: &str,
) -> Markup {
    html! {
        @if status.is_active() {
            (action_form(base, service, "stop", "Stop", Icon::Stop, csrf, other_class))
            (action_form(base, service, "restart", "Restart", Icon::Restart, csrf, other_class))
        } @else {
            (action_form(base, service, "start", "Start", Icon::Play, csrf, start_class))
        }
        @if status.is_autostart_enabled() {
            (action_form(base, service, "disable", "Disable", Icon::Power, csrf, other_class))
        } @else {
            (action_form(base, service, "enable", "Enable", Icon::Power, csrf, other_class))
        }
    }
}

/// Just the kebab's status-dependent forms, no wrapper -- the payload of
/// `GET {unit}/actions?style=menu`. Swapped into `.menu-actions` on a status
/// change so the Start/Stop choice tracks live state *without* re-rendering
/// (and thereby closing) the whole `<details>` menu.
pub fn kebab_action_forms(unit: &QuadletUnit, status: &UnitStatus, csrf: &str) -> Markup {
    unit_action_forms(
        &core::unit_url(unit),
        &unit.service_name(),
        status,
        csrf,
        "",
        "",
    )
}

/// The per-row quick-actions menu: a native `<details>` disclosure, no JS
/// needed. Every URL is built from `core::unit_url`, so this works
/// identically regardless of which section the unit belongs to. Only the
/// status-dependent forms live in a live-refreshing `.menu-actions` slot;
/// the `<details>` and the Edit/Logs/Delete links are never replaced, so an
/// open menu stays open through a status change.
pub fn kebab_menu(unit: &QuadletUnit, status: &UnitStatus, csrf: &str) -> Markup {
    let base = core::unit_url(unit);
    let service = unit.service_name();
    html! {
        details.menu {
            summary aria-label="Actions" { (icon(Icon::More)) }
            div.menu-panel {
                @if !unit.is_template() {
                    div.menu-actions hx-get={(base) "/actions?style=menu"}
                        hx-trigger={"sse:status-" (service) " delay:300ms"} hx-swap="innerHTML" {
                        (unit_action_forms(&base, &service, status, csrf, "", ""))
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
    html! {
        div.action-row {
            (unit_action_forms(
                &core::unit_url(unit),
                &unit.service_name(),
                status,
                csrf,
                "btn btn-primary btn-sm",
                "btn btn-ghost btn-sm",
            ))
            (autostart_pill(status.is_autostart_enabled()))
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

/// A unit's parsed `[Section]` blocks, one card each, rendered verbatim as
/// key/value tables. Used by the detail skeleton.
pub fn section_table(unit: &QuadletUnit) -> Markup {
    html! {
        @for section in &unit.sections {
            div.config-block {
                div.config-block-name { "[" (section.name) "]" }
                table.kv-table {
                    @for (k, v) in &section.entries {
                        tr { td { (k) } td { (v) } }
                    }
                }
            }
        }
    }
}

/// How the editor works out the full quadlet file name to live-validate
/// against `/validate`.
pub enum EditorFileName<'a> {
    /// Edit page: the name is fixed and not shown as an input.
    Fixed(&'a str),
    /// Section "New" page: `input` is the CSS selector of the stem field,
    /// `suffix` the fixed extension shown beside it (e.g. `.container`).
    StemSuffix { input: &'a str, suffix: &'a str },
    /// `/units/new`: `input` is the stem field, `select` the CSS selector of
    /// the `<select>` whose value is the extension.
    StemSelect { input: &'a str, select: &'a str },
}

/// The quadlet-content editor: a plain `<textarea>` progressively enhanced
/// into a syntax-highlighted, live-validated CodeMirror editor by the bundle.
pub fn code_editor(contents: &str, file_name: EditorFileName<'_>) -> Markup {
    let (fixed, stem_input, suffix, kind_select) = match file_name {
        EditorFileName::Fixed(n) => (Some(n), None, None, None),
        EditorFileName::StemSuffix { input, suffix } => (None, Some(input), Some(suffix), None),
        EditorFileName::StemSelect { input, select } => (None, Some(input), None, Some(select)),
    };
    html! {
        textarea.input
            name="contents"
            rows="18"
            data-code-editor
            data-file-name=[fixed]
            data-stem-input=[stem_input]
            data-suffix=[suffix]
            data-kind-select=[kind_select]
            required
            { (contents) }
        div id="validate-status" class="validate-status" {}
    }
}

/// The Name/Value environment-variable editor for `.container` / `.build`
/// units. `body` is the current `KEY=VALUE` lines (one per variable); it
/// renders as a plain `<textarea>` that `frontend/envvars.js` progressively
/// enhances into add/remove rows. Submitted as one `env_vars` field and
/// written to a sidecar `env/<name>.env` referenced by a managed
/// `EnvironmentFile=` line.
pub fn env_var_editor(body: &str) -> Markup {
    html! {
        div.field data-envvars {
            label { "Environment variables" }
            p.field-hint {
                "Saved to a sidecar " code { "env/<name>.env" }
                " and wired into the unit with " code { "EnvironmentFile=" } "."
            }
            textarea.input name="env_vars" rows="4" data-envvars-source { (body) }
        }
    }
}

/// A collapsible reference panel, shown near the editor, listing the host
/// `${NAME}` variables the systemd user manager passes to the quadlet
/// generator (see the Environment page). Each name is a button that inserts
/// its `${NAME}` reference at the editor's cursor; the current value is shown
/// beside it for context.
pub fn host_vars_panel(vars: &[EnvVar]) -> Markup {
    html! {
        details.host-vars {
            summary {
                "Host variables"
                @if !vars.is_empty() { span.muted { " · " (vars.len()) } }
            }
            div.host-vars-body {
                @if vars.is_empty() {
                    p.field-hint {
                        "None set. "
                        a href="/environment" { "Add host variables" }
                        " to reference them here as " code { "${NAME}" } "."
                    }
                } @else {
                    p.field-hint {
                        "Click a name to insert its " code { "${NAME}" } " reference at the cursor."
                    }
                    div.host-vars-grid {
                        @for v in vars {
                            button.chip type="button" data-insert-ref={"${" (v.name) "}"} {
                                "${" (v.name) "}"
                            }
                            span.host-vars-val {
                                @if v.value.is_empty() { span.muted { "—" } } @else { (v.value) }
                            }
                        }
                    }
                }
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_back_target_covers_every_kind() {
        assert_eq!(section_back_target(UnitKind::Container), ("/", "Services"));
        assert_eq!(section_back_target(UnitKind::Pod), ("/", "Services"));
        assert_eq!(
            section_back_target(UnitKind::Volume),
            ("/volumes", "Volumes")
        );
        assert_eq!(
            section_back_target(UnitKind::Network),
            ("/networks", "Networks")
        );
        assert_eq!(section_back_target(UnitKind::Image), ("/images", "Images"));
        assert_eq!(section_back_target(UnitKind::Build), ("/images", "Images"));
        // Kube has no sidebar section -- falls back to the generic list.
        assert_eq!(section_back_target(UnitKind::Kube), ("/units", "All units"));
    }

    #[test]
    fn detail_page_renders_overview_and_config_cards() {
        use crate::quadlet::model::Section;

        let unit = QuadletUnit {
            file_name: "web.container".into(),
            path: "/tmp/web.container".into(),
            kind: UnitKind::Container,
            sections: vec![Section {
                name: "Container".into(),
                entries: vec![("Image".into(), "docker.io/library/nginx".into())],
            }],
            raw: String::new(),
        };
        let markup = detail::detail_page(
            &unit,
            &UnitStatus::not_found(),
            "csrf",
            &[("Image", html! { code { "docker.io/library/nginx" } })],
            None,
        )
        .into_string();

        assert!(markup.contains("Overview"));
        assert!(markup.contains("Configuration"));
        assert!(markup.contains("config-block-name"));
        assert!(markup.contains("web.service"));
        assert!(markup.contains("detail-grid"));
    }
}
