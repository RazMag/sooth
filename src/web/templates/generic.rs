//! Templates for the generic `/units` fallback section: a kind-agnostic
//! detail page (used for Kube units, which have no dedicated section) and the
//! shared raw-textarea create/edit forms used by every section (Volumes,
//! Networks, Images) plus this fallback itself.

use maud::{Markup, html};

use super::detail;
use super::{
    BannerKind, EditorFileName, NavItem, back_link, banner, code_editor, csrf_input,
    env_var_editor, host_vars_panel, page_header, shell,
};
use crate::hostenv::EnvVar;
use crate::quadlet::{QuadletUnit, UnitKind};
use crate::systemd::UnitStatus;

/// A plain, kind-branch-free detail page -- used for any kind without a
/// dedicated section template (today, just Kube).
pub fn detail_page(unit: &QuadletUnit, status: &UnitStatus, csrf: &str) -> Markup {
    detail::detail_page(unit, status, csrf, &[], None)
}

/// How the "New" page's file-name field behaves: a stem plus a fixed
/// extension suffix (section pages), or a stem plus a `<select>` of every
/// kind (`/units/new`). Either way the server composes the full name.
pub enum KindChoice {
    Fixed(UnitKind),
    Choose { selected: UnitKind },
}

impl KindChoice {
    fn kind(&self) -> UnitKind {
        match self {
            KindChoice::Fixed(k) | KindChoice::Choose { selected: k } => *k,
        }
    }

    /// Whether this kind gets the sidecar env-file editor.
    fn wants_env_editor(&self) -> bool {
        matches!(self.kind(), UnitKind::Container | UnitKind::Build)
            || matches!(self, KindChoice::Choose { .. })
    }
}

/// Parameters for [`new_unit_page`]. Grouped into a struct because the "New"
/// form now carries the kind choice, a stem prefill, the env-var editor body,
/// and the host-variable panel on top of the original three.
pub struct NewUnitPage<'a> {
    pub csrf: &'a str,
    pub active: Option<NavItem>,
    /// POST target, e.g. `/containers`.
    pub action: &'a str,
    pub kind: KindChoice,
    /// The editor's starting text -- the skeleton on first render, the
    /// rejected submission on a 422 redisplay.
    pub editor_body: &'a str,
    /// The stem field's value (`""` first render, the posted stem on 422).
    pub stem_prefill: &'a str,
    /// Current `KEY=VALUE` lines for the env editor (`""` unless redisplaying
    /// a Container/Build submission).
    pub env_vars_body: &'a str,
    pub host_vars: &'a [EnvVar],
    pub error: Option<&'a str>,
}

/// Shared by every section's "New" page -- the differences are the POST
/// target, nav highlight, kind choice, and starter skeleton.
pub fn new_unit_page(p: NewUnitPage<'_>) -> Markup {
    let dot_ext = format!(".{}", p.kind.kind().extension());
    let file_name = match &p.kind {
        KindChoice::Fixed(_) => EditorFileName::StemSuffix {
            input: "#file_name",
            suffix: &dot_ext,
        },
        KindChoice::Choose { .. } => EditorFileName::StemSelect {
            input: "#file_name",
            select: "#file_kind",
        },
    };

    let (back_href, back_label) = match p.active {
        Some(nav) => (nav.href(), nav.label()),
        None => ("/units", "All units"),
    };

    let body = html! {
        (back_link(back_href, back_label))
        (page_header("New quadlet", html! {}))
        @if let Some(msg) = p.error { (banner(BannerKind::Error, msg)) }
        form method="post" action=(p.action) {
            (csrf_input(p.csrf))
            div.field {
                label for="file_name" { "File name" }
                div.stem-row {
                    input.input type="text" id="file_name" name="file_name"
                        value=(p.stem_prefill) placeholder="myapp"
                        autocomplete="off" autocapitalize="off" spellcheck="false" required;
                    @match &p.kind {
                        KindChoice::Fixed(k) => {
                            span.stem-suffix { "." (k.extension()) }
                            input type="hidden" name="kind" value=(k.extension());
                            input type="hidden" name="origin" value="section";
                        }
                        KindChoice::Choose { selected } => {
                            select.input #file_kind name="kind" {
                                @for k in UnitKind::all() {
                                    option value=(k.extension()) selected[k == *selected] { "." (k.extension()) }
                                }
                            }
                            input type="hidden" name="origin" value="units";
                        }
                    }
                }
            }
            div.editor-row {
                div.editor-col {
                    label { "Contents" }
                    (code_editor(p.editor_body, file_name))
                }
                @if p.kind.wants_env_editor() {
                    div.editor-col {
                        (env_var_editor(p.env_vars_body))
                    }
                }
            }
            (host_vars_panel(p.host_vars))
            button.btn.btn-primary type="submit" { "Create" }
        }
    };
    shell("New quadlet", p.active, body)
}

/// The raw-INI edit form. `.container` / `.build` units also get the sidecar
/// env-var editor (prefilled from `env_vars_body`); every kind gets the
/// host-variable reference panel.
pub fn edit_unit_page(
    unit: &QuadletUnit,
    csrf: &str,
    env_vars_body: &str,
    host_vars: &[EnvVar],
    error: Option<&str>,
) -> Markup {
    let base = crate::web::core::unit_url(unit);
    let wants_env = matches!(unit.kind, UnitKind::Container | UnitKind::Build);
    let body = html! {
        (back_link(&base, &unit.file_name))
        (page_header(&format!("Edit {}", unit.file_name), html! {}))
        @if let Some(msg) = error { (banner(BannerKind::Error, msg)) }
        form method="post" action=(base + "/edit") {
            (csrf_input(csrf))
            div.editor-row {
                div.editor-col {
                    label { "Contents" }
                    (code_editor(&unit.raw, EditorFileName::Fixed(&unit.file_name)))
                }
                @if wants_env {
                    div.editor-col {
                        (env_var_editor(env_vars_body))
                    }
                }
            }
            (host_vars_panel(host_vars))
            button.btn.btn-primary type="submit" { "Save" }
        }
    };
    shell(
        &format!("Edit {}", unit.file_name),
        NavItem::for_kind(unit.kind),
        body,
    )
}
