//! Machine scope: the `?m=` filter a fleet page passes through. No `m`
//! means every machine; `m=local` is this hub's own machine; any other value
//! is a paired machine's id (see `crate::fleet`).

use crate::error::AppError;
use crate::fleet::{Fleet, Linked};
use crate::web::templates::overview::MachineInfo;
use crate::web::templates::scope::ScopeChoice;

/// The id this hub's own machine goes by in `?m=` (no paired machine can
/// take it, see `Fleet::add`).
pub const LOCAL: &str = "local";

/// A page's query string when `?m=` is all it takes.
#[derive(Debug, Default, serde::Deserialize)]
pub struct ScopeQuery {
    pub m: Option<String>,
}

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

    /// The switcher's chips for the page at `path`: every machine, this
    /// hub, each paired one. `None` when the hub has no other machines.
    pub fn choices(&self, fleet: &Fleet, path: &str) -> Option<Vec<ScopeChoice>> {
        let machines = fleet.machines();
        if machines.is_empty() {
            return None;
        }
        let current = self.param();
        let mut out = vec![
            ScopeChoice {
                href: path.to_string(),
                label: "All machines".into(),
                dot: None,
                current: current.is_none(),
            },
            ScopeChoice {
                href: format!("{path}?m={LOCAL}"),
                label: MachineInfo::detect().hostname,
                dot: Some(true),
                current: current == Some(LOCAL),
            },
        ];
        for m in machines {
            let online = fleet.seen(&m.entry.id).map(|s| s.reach.is_online());
            out.push(ScopeChoice {
                current: current == Some(m.entry.id.as_str()),
                href: format!("{path}?m={}", m.entry.id),
                label: m.entry.name,
                dot: online,
            });
        }
        Some(out)
    }
}
