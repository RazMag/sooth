//! Machine scope: the `?m=` filter a fleet page passes through. No `m`
//! means every machine; `m=local` is this hub's own machine; any other value
//! is a paired machine's id (see `crate::fleet`).

use crate::error::AppError;
use crate::fleet::{Fleet, Linked};

/// The id this hub's own machine goes by in `?m=` (no paired machine can
/// take it, see `Fleet::add`).
pub const LOCAL: &str = "local";

pub enum Scope {
    All,
    Local,
    One(Box<Linked>),
}

impl Scope {
    /// Reads `?m=`. An id that isn't one of the hub's machines is a 404,
    /// rather than quietly showing something else.
    pub fn resolve(fleet: &Fleet, m: Option<&str>) -> Result<Self, AppError> {
        match m.map(str::trim).filter(|m| !m.is_empty()) {
            None => Ok(Self::All),
            Some(LOCAL) => Ok(Self::Local),
            Some(id) => fleet
                .get(id)
                .map(|m| Self::One(Box::new(m)))
                .ok_or_else(|| AppError::NotFound(format!("machine '{id}'"))),
        }
    }

    /// The `m` value that names this scope (`None` for every machine).
    pub fn param(&self) -> Option<&str> {
        match self {
            Self::All => None,
            Self::Local => Some(LOCAL),
            Self::One(m) => Some(&m.entry.id),
        }
    }

    /// `path` with this scope's `?m=` on it.
    pub fn link(&self, path: &str) -> String {
        match self.param() {
            Some(m) => format!("{path}?m={m}"),
            None => path.to_string(),
        }
    }

    pub fn includes_local(&self) -> bool {
        matches!(self, Self::All | Self::Local)
    }

    /// The paired machines in scope.
    pub fn remotes(&self, fleet: &Fleet) -> Vec<Linked> {
        match self {
            Self::All => fleet.machines(),
            Self::Local => Vec::new(),
            Self::One(m) => vec![(**m).clone()],
        }
    }
}
