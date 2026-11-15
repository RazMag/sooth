//! The Images & tags page (`/tags`) and its live rows.

use axum::extract::State;
use axum::response::IntoResponse;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, FragmentError, PageError};
use crate::quadlet::{UnitKind, refs};
use crate::web::core;
use crate::web::templates::tagspage::TagRow;
use crate::web::templates::{self};

async fn rows(state: &AppState) -> Result<Vec<TagRow>, AppError> {
    let (units, all) = core::load_units_and_siblings(state, &[UnitKind::Container]).await?;
    let mut tags: Vec<(String, core::NewerTags)> =
        core::newer_tags(state, &units, &all).into_iter().collect();
    tags.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(tags
        .into_iter()
        .filter_map(|(file_name, tags)| {
            let unit = all.iter().find(|u| u.file_name == file_name)?;
            Some(TagRow {
                href: core::unit_url(unit),
                image: refs::container_image(unit, &all).unwrap_or_default(),
                file_name,
                tags,
            })
        })
        .collect())
}

async fn csrf(session: &Session) -> String {
    crate::auth::csrf::current(session)
        .await
        .unwrap_or_default()
}

pub async fn page(
    State(state): State<AppState>,
    session: Session,
) -> Result<impl IntoResponse, PageError> {
    let rows = rows(&state).await?;
    Ok(templates::tagspage::tags_page(
        &rows,
        &state.image_tags.snapshot(),
        &csrf(&session).await,
        state.health.get(),
    ))
}

pub async fn rows_fragment(
    State(state): State<AppState>,
    session: Session,
) -> Result<impl IntoResponse, FragmentError> {
    let rows = rows(&state).await?;
    Ok(templates::tagspage::tag_rows(&rows, &csrf(&session).await))
}
