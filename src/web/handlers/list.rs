//! Per-section list pages (Volumes, Networks, Images, and the generic
//! all-kinds fallback). Each is a thin pair (full page + `/rows` refresh
//! fragment) over `core::load_units_for_kinds` and the generic
//! `templates::list` renderer -- only the `ListSpec` (title, columns, kinds)
//! differs per section. Containers/Pods live on the combined Services home
//! page instead (`handlers::services`), not as their own list here.

use axum::extract::State;
use axum::response::IntoResponse;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{FragmentError, PageError};
use crate::quadlet::UnitKind;
use crate::web::core;
use crate::web::templates::list::{Column, ListSpec, kind_cell};
use crate::web::templates::{self, NavItem};

const ALL_UNITS_COLUMNS: &[Column] = &[Column {
    header: "Kind",
    cell: kind_cell,
}];

const VOLUMES_SPEC: ListSpec = ListSpec {
    title: "Volumes",
    active_nav: Some(NavItem::Volumes),
    columns: templates::volumes::COLUMNS,
    new_href: "/volumes/new",
    empty_hint: "No volumes yet.",
};
const VOLUMES_KINDS: &[UnitKind] = &[UnitKind::Volume];

const NETWORKS_SPEC: ListSpec = ListSpec {
    title: "Networks",
    active_nav: Some(NavItem::Networks),
    columns: templates::networks::COLUMNS,
    new_href: "/networks/new",
    empty_hint: "No networks yet.",
};
const NETWORKS_KINDS: &[UnitKind] = &[UnitKind::Network];

const IMAGES_SPEC: ListSpec = ListSpec {
    title: "Images",
    active_nav: Some(NavItem::Images),
    columns: templates::images::COLUMNS,
    new_href: "/images/new",
    empty_hint: "No images or builds yet.",
};
const IMAGES_KINDS: &[UnitKind] = &[UnitKind::Image, UnitKind::Build];

const ALL_UNITS_SPEC: ListSpec = ListSpec {
    title: "All units",
    active_nav: None,
    columns: ALL_UNITS_COLUMNS,
    new_href: "/units/new",
    empty_hint: "No quadlet files found yet.",
};
const ALL_UNITS_KINDS: &[UnitKind] = &[
    UnitKind::Container,
    UnitKind::Pod,
    UnitKind::Volume,
    UnitKind::Network,
    UnitKind::Image,
    UnitKind::Build,
    UnitKind::Kube,
];

macro_rules! list_handlers {
    ($page_fn:ident, $rows_fn:ident, $kinds:expr, $spec:expr) => {
        pub async fn $page_fn(
            State(state): State<AppState>,
            session: Session,
        ) -> Result<impl IntoResponse, PageError> {
            let csrf = crate::auth::csrf::current(&session)
                .await
                .unwrap_or_default();
            let units = core::load_units_for_kinds(&state, $kinds).await?;
            Ok(templates::list::list_page($spec, &units, &csrf))
        }

        pub async fn $rows_fn(
            State(state): State<AppState>,
            session: Session,
        ) -> Result<impl IntoResponse, FragmentError> {
            let csrf = crate::auth::csrf::current(&session)
                .await
                .unwrap_or_default();
            let units = core::load_units_for_kinds(&state, $kinds).await?;
            Ok(templates::list::list_rows($spec, &units, &csrf))
        }
    };
}

list_handlers!(volumes_page, volumes_rows, VOLUMES_KINDS, &VOLUMES_SPEC);
list_handlers!(networks_page, networks_rows, NETWORKS_KINDS, &NETWORKS_SPEC);
list_handlers!(images_page, images_rows, IMAGES_KINDS, &IMAGES_SPEC);
list_handlers!(
    all_units_page,
    all_units_rows,
    ALL_UNITS_KINDS,
    &ALL_UNITS_SPEC
);
