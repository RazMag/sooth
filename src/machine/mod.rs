//! One machine sooth manages, wherever it is. Everything the UI needs from a
//! host -- its facts, its quadlets with their live status, start/stop/restart
//! and the journal -- goes through [`Machine`], so a page renders the same
//! whether the units live on this host ([`local::LocalMachine`]) or on an
//! agent reached over its API ([`remote::RemoteMachine`], speaking to
//! [`api::router`] on the far side).
//!
//! An enum rather than a trait object: there are exactly two real kinds of
//! machine (plus a test double), and an enum keeps the calls plain `async fn`s
//! with no boxing.
//!
//! What crosses the wire is a [`UnitRecord`]: a quadlet's file name, group and
//! raw text, plus its status. The receiving side parses the text again with
//! the same parser the local path uses, so a remote unit is a normal
//! [`QuadletUnit`] by the time a template sees it.

pub mod api;
#[cfg(test)]
pub mod fake;
pub mod local;
pub mod remote;

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::health::Health;
use crate::journal::LogLine;
use crate::quadlet::{QuadletError, QuadletUnit, UnitKind, naming, parser};
use crate::systemd::UnitStatus;

/// What a machine says about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facts {
    pub hostname: String,
    pub arch: String,
    /// `/etc/os-release`'s `PRETTY_NAME`, when readable.
    pub os: Option<String>,
    pub sooth_version: String,
    pub quadlet_dir: String,
    pub health: Health,
}

/// A quadlet as it travels between machines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitRecord {
    pub file_name: String,
    pub group: String,
    pub raw: String,
    /// Live status, when it was asked for (see [`Machine::units`]).
    pub status: Option<UnitStatus>,
}

impl UnitRecord {
    pub fn new(unit: &QuadletUnit, status: Option<UnitStatus>) -> Self {
        Self {
            file_name: unit.file_name.clone(),
            group: unit.group.clone(),
            raw: unit.raw.clone(),
            status,
        }
    }

    /// Parses the record back into a unit, as if it sat under `root`. The
    /// file name is checked the same way discovery checks one on disk, so a
    /// record can't smuggle in a path or an unknown kind.
    pub fn into_unit(self, root: &Path) -> Result<(QuadletUnit, Option<UnitStatus>), AppError> {
        let invalid = |msg: String| AppError::from(QuadletError::Validation(msg));
        if self.file_name.contains('/') || self.file_name.starts_with('.') {
            return Err(invalid(format!("bad unit file name '{}'", self.file_name)));
        }
        if !self.group.is_empty() && !naming::valid_group(&self.group) {
            return Err(invalid(format!("bad group '{}'", self.group)));
        }
        let kind = naming::kind_of(&self.file_name).ok_or_else(|| {
            invalid(format!(
                "unrecognized quadlet extension: {}",
                self.file_name
            ))
        })?;
        let sections = parser::parse(&self.raw)?;
        let path = root.join(naming::compose_rel_path(&self.group, &self.file_name));
        let unit = QuadletUnit {
            file_name: self.file_name,
            group: self.group,
            path,
            kind,
            sections,
            raw: self.raw,
            port_notes: Default::default(),
        };
        Ok((unit, self.status))
    }
}

/// A systemd action a machine runs on one unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Restart,
}

impl Action {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "start" => Some(Self::Start),
            "stop" => Some(Self::Stop),
            "restart" => Some(Self::Restart),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }
}

/// Units with their status, plus every unit on the machine -- the shape
/// [`Machine::units`] answers in.
pub type UnitListing = (Vec<(QuadletUnit, UnitStatus)>, Vec<QuadletUnit>);

#[derive(Clone)]
pub enum Machine {
    Local(local::LocalMachine),
    // Reached once the hub links machines; until then only tests build one.
    #[cfg_attr(not(test), allow(dead_code))]
    Remote(remote::RemoteMachine),
    #[cfg(test)]
    Fake(fake::FakeMachine),
}

impl Machine {
    pub async fn facts(&self) -> Result<Facts, AppError> {
        match self {
            Self::Local(m) => Ok(m.facts()),
            Self::Remote(m) => m.facts().await,
            #[cfg(test)]
            Self::Fake(m) => Ok(m.facts()),
        }
    }

    /// Every quadlet on the machine as records; those of `kinds` carry their
    /// live status (templates never do, they have no service of their own).
    pub async fn records(&self, kinds: &[UnitKind]) -> Result<Vec<UnitRecord>, AppError> {
        match self {
            Self::Local(m) => m.records(kinds).await,
            Self::Remote(m) => m.records(kinds).await,
            #[cfg(test)]
            Self::Fake(m) => Ok(m.records(kinds)),
        }
    }

    /// The units of `kinds` with their status, and every unit on the machine
    /// (for columns that resolve references across kinds).
    pub async fn units(&self, kinds: &[UnitKind]) -> Result<UnitListing, AppError> {
        if let Self::Local(m) = self {
            return m.units(kinds).await;
        }
        let root = self.root();
        let mut with_status = Vec::new();
        let mut all = Vec::new();
        for record in self.records(kinds).await? {
            let (unit, status) = record.into_unit(&root)?;
            if kinds.contains(&unit.kind) {
                with_status.push((unit.clone(), status.unwrap_or_else(UnitStatus::not_found)));
            }
            all.push(unit);
        }
        Ok((with_status, all))
    }

    pub async fn status(&self, file_name: &str) -> Result<UnitStatus, AppError> {
        match self {
            Self::Local(m) => m.status(file_name).await,
            Self::Remote(m) => m.status(file_name).await,
            #[cfg(test)]
            Self::Fake(m) => m.status(file_name),
        }
    }

    /// Runs `action` on the unit and returns its status afterwards.
    pub async fn act(&self, file_name: &str, action: Action) -> Result<UnitStatus, AppError> {
        match self {
            Self::Local(m) => m.act(file_name, action).await,
            Self::Remote(m) => m.act(file_name, action).await,
            #[cfg(test)]
            Self::Fake(m) => m.act(file_name, action),
        }
    }

    /// The last `lines` journal entries of the unit.
    pub async fn logs(&self, file_name: &str, lines: u32) -> Result<Vec<LogLine>, AppError> {
        match self {
            Self::Local(m) => m.logs(file_name, lines).await,
            Self::Remote(m) => m.logs(file_name, lines).await,
            #[cfg(test)]
            Self::Fake(m) => m.logs(file_name),
        }
    }

    /// Where the machine's quadlets live, for the paths of parsed records.
    fn root(&self) -> std::path::PathBuf {
        match self {
            Self::Local(m) => m.quadlet_dir().to_path_buf(),
            Self::Remote(_) => std::path::PathBuf::from("/"),
            #[cfg(test)]
            Self::Fake(_) => std::path::PathBuf::from("/fake"),
        }
    }
}

/// The largest journal tail one request may ask for.
pub const MAX_LOG_LINES: u32 = 5000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_parses_back_into_the_same_unit() {
        let record = UnitRecord {
            file_name: "web.container".into(),
            group: "media".into(),
            raw: "[Container]\nImage=nginx\n".into(),
            status: Some(UnitStatus::not_found()),
        };
        let (unit, status) = record.clone().into_unit(Path::new("/q")).unwrap();
        assert_eq!(unit.kind, UnitKind::Container);
        assert_eq!(unit.rel_path(), "media/web.container");
        assert_eq!(unit.path, Path::new("/q/media/web.container"));
        assert_eq!(
            unit.section("Container").unwrap().get("Image"),
            Some("nginx")
        );
        assert_eq!(UnitRecord::new(&unit, status), record);
    }

    #[test]
    fn a_record_with_a_bad_name_is_refused() {
        let bad = |file_name: &str, group: &str| UnitRecord {
            file_name: file_name.into(),
            group: group.into(),
            raw: "[Container]\n".into(),
            status: None,
        };
        let root = Path::new("/q");
        assert!(bad("web.service", "").into_unit(root).is_err());
        assert!(bad("../web.container", "").into_unit(root).is_err());
        assert!(bad("web.container", "../etc").into_unit(root).is_err());
        assert!(bad("web.container", "ok/group").into_unit(root).is_ok());
    }

    #[test]
    fn actions_round_trip_their_names() {
        for a in [Action::Start, Action::Stop, Action::Restart] {
            assert_eq!(Action::parse(a.as_str()), Some(a));
        }
        assert_eq!(Action::parse("enable"), None);
    }
}
