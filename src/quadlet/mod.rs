pub mod discovery;
pub mod model;
pub mod naming;
pub mod parser;
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
