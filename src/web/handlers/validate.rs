//! Live validation for the code editor: calls the exact same
//! `quadlet::writer::validate` the real create/edit write path uses, so
//! what the editor reports matches what submitting would actually do.
//! Deliberately not CSRF-checked -- it performs no write or state change
//! (CSRF exists to stop unwanted state changes, which this can't cause),
//! and it's reachable only when already logged in, like every other route
//! in the protected router.

use axum::Form;
use axum::response::IntoResponse;
use serde::Deserialize;

use crate::quadlet::writer;
use crate::web::templates;

#[derive(Deserialize)]
pub struct ValidateForm {
    file_name: String,
    contents: String,
}

pub async fn check(Form(form): Form<ValidateForm>) -> impl IntoResponse {
    match writer::validate(&form.file_name, &form.contents) {
        Ok(()) => templates::validate_ok(),
        Err(e) => templates::validate_error(&e.to_string()),
    }
}
