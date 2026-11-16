//! The machine sooth itself runs on: quadlets read from disk, systemd over the
//! user D-Bus session, logs from `journalctl`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{Action, Facts, UnitListing, UnitRecord};
use crate::error::AppError;
use crate::health::HealthCell;
use crate::journal::{self, LogLine};
use crate::quadlet::{UnitKind, discovery};
use crate::systemd::{Client, UnitStatus};
use crate::web::templates::overview::MachineInfo;

#[derive(Clone)]
pub struct LocalMachine {
    quadlet_dir: Arc<PathBuf>,
    systemd: Arc<Client>,
    health: HealthCell,
}

impl LocalMachine {
    pub fn new(quadlet_dir: Arc<PathBuf>, systemd: Arc<Client>, health: HealthCell) -> Self {
        Self {
            quadlet_dir,
            systemd,
            health,
        }
    }

    pub fn quadlet_dir(&self) -> &Path {
        &self.quadlet_dir
    }

    pub fn facts(&self) -> Facts {
        let info = MachineInfo::detect();
        Facts {
            hostname: info.hostname,
            arch: info.arch.to_string(),
            os: info.os,
            sooth_version: env!("CARGO_PKG_VERSION").to_string(),
            quadlet_dir: self.quadlet_dir.display().to_string(),
            health: self.health.get(),
        }
    }

    pub async fn units(&self, kinds: &[UnitKind]) -> Result<UnitListing, AppError> {
        let all = discovery::load_all(&self.quadlet_dir)?;
        let mut out = Vec::new();
        for unit in all.iter().filter(|u| kinds.contains(&u.kind)) {
            let status = if unit.is_template() {
                UnitStatus::not_found()
            } else {
                self.systemd.status(&unit.service_name()).await?
            };
            out.push((unit.clone(), status));
        }
        Ok((out, all))
    }

    pub async fn records(&self, kinds: &[UnitKind]) -> Result<Vec<UnitRecord>, AppError> {
        let (with_status, all) = self.units(kinds).await?;
        Ok(all
            .iter()
            .map(|u| {
                let status = with_status
                    .iter()
                    .find(|(s, _)| s.file_name == u.file_name)
                    .map(|(_, st)| st.clone());
                UnitRecord::new(u, status)
            })
            .collect())
    }

    /// The service of the quadlet named `file_name`, which must exist here --
    /// so a name from a request never reaches systemd or `journalctl` unless
    /// it's one of this machine's own units.
    fn service_of(&self, file_name: &str) -> Result<String, AppError> {
        Ok(discovery::load_by_name(&self.quadlet_dir, file_name)?.service_name())
    }

    pub async fn status(&self, file_name: &str) -> Result<UnitStatus, AppError> {
        let service = self.service_of(file_name)?;
        Ok(self.systemd.status(&service).await?)
    }

    pub async fn act(&self, file_name: &str, action: Action) -> Result<UnitStatus, AppError> {
        let service = self.service_of(file_name)?;
        match action {
            Action::Start => self.systemd.start(&service).await?,
            Action::Stop => self.systemd.stop(&service).await?,
            Action::Restart => self.systemd.restart(&service).await?,
        }
        Ok(self.systemd.status(&service).await?)
    }

    pub async fn logs(&self, file_name: &str, lines: u32) -> Result<Vec<LogLine>, AppError> {
        let service = self.service_of(file_name)?;
        Ok(journal::tail_recent(&service, lines).await?)
    }
}
