//! Podman's own health check (`HealthCmd=`) results, for the status cell.
//! systemd only knows whether the container runs; whether it's *healthy*
//! comes from `podman ps`, read once per list render and only when some
//! listed container declares a `HealthCmd=`.

use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::quadlet::{QuadletUnit, UnitKind, naming};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerHealth {
    Healthy,
    Unhealthy,
    Starting,
}

impl ContainerHealth {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Unhealthy => "unhealthy",
            Self::Starting => "starting",
        }
    }

    /// The `(healthy)` suffix `podman ps` puts on a checked container's
    /// status, e.g. `Up 2 hours (unhealthy)`.
    fn from_status(status: &str) -> Option<Self> {
        let inner = status.rsplit_once('(')?.1.strip_suffix(')')?;
        match inner.trim() {
            "healthy" => Some(Self::Healthy),
            "unhealthy" => Some(Self::Unhealthy),
            "starting" => Some(Self::Starting),
            _ => None,
        }
    }
}

/// Whether the unit is a container with a podman health check.
pub fn has_health_check(unit: &QuadletUnit) -> bool {
    unit.kind == UnitKind::Container
        && unit
            .section("Container")
            .and_then(|s| s.get("HealthCmd"))
            .is_some_and(|c| !c.trim().is_empty() && c.trim() != "none")
}

/// The container name podman runs the unit as: `ContainerName=`, else
/// quadlet's default `systemd-<stem>`.
pub fn container_name(unit: &QuadletUnit) -> String {
    unit.section("Container")
        .and_then(|s| s.get("ContainerName"))
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("systemd-{}", naming::stem(&unit.file_name)))
}

/// Each health-checked unit's current result, by file name. Empty when
/// none of `units` has a check, or podman doesn't answer within 3s.
pub async fn health_of<'a>(
    units: impl IntoIterator<Item = &'a QuadletUnit>,
) -> HashMap<String, ContainerHealth> {
    let checked: Vec<(String, String)> = units
        .into_iter()
        .filter(|u| has_health_check(u))
        .map(|u| (container_name(u), u.file_name.clone()))
        .collect();
    if checked.is_empty() {
        return HashMap::new();
    }
    let mut cmd = Command::new("podman");
    cmd.args(["ps", "--format", "{{.Names}}\t{{.Status}}"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let Ok(Ok(out)) = tokio::time::timeout(Duration::from_secs(3), cmd.output()).await else {
        return HashMap::new();
    };
    parse_ps(&String::from_utf8_lossy(&out.stdout), &checked)
}

fn parse_ps(output: &str, checked: &[(String, String)]) -> HashMap<String, ContainerHealth> {
    output
        .lines()
        .filter_map(|line| {
            let (name, status) = line.split_once('\t')?;
            let file = &checked.iter().find(|(n, _)| n == name.trim())?.1;
            Some((file.clone(), ContainerHealth::from_status(status)?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_health_suffix_from_podman_ps() {
        let checked = vec![
            ("systemd-db".to_string(), "db.container".to_string()),
            ("web".to_string(), "web.container".to_string()),
        ];
        let out = "systemd-db\tUp 2 hours (unhealthy)\nweb\tUp 5 minutes (healthy)\nother\tUp 1 hour (healthy)\n";
        let got = parse_ps(out, &checked);
        assert_eq!(got.get("db.container"), Some(&ContainerHealth::Unhealthy));
        assert_eq!(got.get("web.container"), Some(&ContainerHealth::Healthy));
        assert_eq!(got.len(), 2);
        assert_eq!(ContainerHealth::from_status("Up 3 seconds"), None);
        assert_eq!(
            ContainerHealth::from_status("Up 3 seconds (starting)"),
            Some(ContainerHealth::Starting)
        );
    }
}
