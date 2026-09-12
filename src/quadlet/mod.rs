pub mod autoupdate;
pub mod discovery;
pub mod envfile;
pub mod gitsync;
pub mod install;
pub mod model;
pub mod naming;
pub mod parser;
pub mod ports;
pub mod refs;
pub mod writer;

pub use model::{QuadletUnit, UnitKind};

#[derive(Debug, thiserror::Error)]
pub enum QuadletError {
    #[error("{0} not found")]
    NotFound(String),
    #[error("{0}")]
    Validation(String),
    #[error("podman quadlet generator rejected this file:\n{0}")]
    GeneratorRejected(String),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
}

impl QuadletError {
    /// True for errors that mean "the content the user submitted was bad" --
    /// callers redisplay the originating form with the message rather than
    /// rendering a generic error page.
    pub fn is_client_error(&self) -> bool {
        matches!(
            self,
            QuadletError::Validation(_) | QuadletError::GeneratorRejected(_)
        )
    }
}
