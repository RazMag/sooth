use std::collections::HashSet;

use axum::extract::State;
use axum::response::IntoResponse;

use crate::config::AppState;
use crate::error::PageError;
use crate::quadlet::UnitKind;
use crate::quadlet::ports;
use crate::web::core;
use crate::web::templates::ports::PortRow;
use crate::web::templates::{self};

pub async fn index(State(state): State<AppState>) -> Result<impl IntoResponse, PageError> {
    let units = core::load_units_for_kinds(&state, &[UnitKind::Container, UnitKind::Pod]).await?;
    let unit_refs: Vec<_> = units.iter().map(|(u, _)| u.clone()).collect();

    let mut mappings = ports::extract(&unit_refs);
    mappings.sort_by_key(|m| m.host_port.map(|r| r.start).unwrap_or(u16::MAX));

    let collisions = ports::find_collisions(&mappings);
    let collision_files: HashSet<&str> = collisions
        .iter()
        .flat_map(|c| c.mappings.iter().map(|m| m.file_name.as_str()))
        .collect();

    let rows: Vec<PortRow> = mappings
        .iter()
        .filter_map(|m| {
            let (u, s) = units.iter().find(|(u, _)| u.file_name == m.file_name)?;
            Some(PortRow {
                mapping: m,
                owner_href: core::unit_url(u),
                status: s,
                collides: collision_files.contains(m.file_name.as_str()),
            })
        })
        .collect();

    Ok(templates::ports::ports_page(&rows))
}
