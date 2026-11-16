//! An in-memory machine for tests: a fixed set of quadlets whose status flips
//! when they're started or stopped, and a log of the actions it was asked for.

use std::sync::{Arc, Mutex};

use super::{Action, Facts, UnitRecord};
use crate::error::AppError;
use crate::health::Health;
use crate::journal::LogLine;
use crate::quadlet::{UnitKind, naming};
use crate::systemd::UnitStatus;

#[derive(Clone, Default)]
pub struct FakeMachine {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    /// `(group, file_name, raw, active)`.
    units: Vec<(String, String, String, bool)>,
    actions: Vec<(String, Action)>,
}

impl FakeMachine {
    pub fn with_units(units: &[(&str, &str, &str)]) -> Self {
        let fake = Self::default();
        fake.inner.lock().unwrap().units = units
            .iter()
            .map(|(g, f, raw)| (g.to_string(), f.to_string(), raw.to_string(), false))
            .collect();
        fake
    }

    /// Every action run so far, in order.
    pub fn actions(&self) -> Vec<(String, Action)> {
        self.inner.lock().unwrap().actions.clone()
    }

    pub fn facts(&self) -> Facts {
        Facts {
            hostname: "fake01".into(),
            arch: "x86_64".into(),
            os: Some("Fake Linux".into()),
            sooth_version: env!("CARGO_PKG_VERSION").into(),
            quadlet_dir: "/fake".into(),
            health: Health::default(),
        }
    }

    fn status_of(active: bool) -> UnitStatus {
        if active {
            UnitStatus {
                load_state: "loaded".into(),
                active_state: "active".into(),
                sub_state: "running".into(),
                ..UnitStatus::not_found()
            }
        } else {
            UnitStatus {
                load_state: "loaded".into(),
                ..UnitStatus::not_found()
            }
        }
    }

    pub fn records(&self, kinds: &[UnitKind]) -> Vec<UnitRecord> {
        self.inner
            .lock()
            .unwrap()
            .units
            .iter()
            .map(|(group, file_name, raw, active)| {
                let wanted = naming::kind_of(file_name).is_some_and(|k| kinds.contains(&k));
                UnitRecord {
                    file_name: file_name.clone(),
                    group: group.clone(),
                    raw: raw.clone(),
                    status: wanted.then(|| Self::status_of(*active)),
                }
            })
            .collect()
    }

    pub fn status(&self, file_name: &str) -> Result<UnitStatus, AppError> {
        let inner = self.inner.lock().unwrap();
        let (.., active) = inner
            .units
            .iter()
            .find(|u| u.1 == file_name)
            .ok_or_else(|| AppError::NotFound(file_name.to_string()))?;
        Ok(Self::status_of(*active))
    }

    pub fn act(&self, file_name: &str, action: Action) -> Result<UnitStatus, AppError> {
        let mut inner = self.inner.lock().unwrap();
        let unit = inner
            .units
            .iter_mut()
            .find(|u| u.1 == file_name)
            .ok_or_else(|| AppError::NotFound(file_name.to_string()))?;
        unit.3 = action != Action::Stop;
        let status = Self::status_of(unit.3);
        inner.actions.push((file_name.to_string(), action));
        Ok(status)
    }

    pub fn logs(&self, file_name: &str) -> Result<Vec<LogLine>, AppError> {
        self.status(file_name)?;
        Ok(vec![LogLine {
            realtime_us: 1,
            invocation: Some("run1".into()),
            ident: naming::stem(file_name).to_string(),
            pid: Some("42".into()),
            message: format!("hello from {file_name}"),
        }])
    }
}
