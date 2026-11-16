//! The Machine page (`/machine`).

use std::time::Duration;

use axum::extract::State;
use axum::response::IntoResponse;

use crate::config::AppState;
use crate::error::PageError;
use crate::quadlet::{UnitKind, discovery};
use crate::web::templates::machine::{Counts, MachinePage, Tool, parse_version};
use crate::web::templates::overview::MachineInfo;
use crate::web::templates::{self};

/// `<program> --version`, or `None` when it's missing, fails or hangs.
async fn version_of(program: &str) -> Option<String> {
    let run = tokio::process::Command::new(program)
        .arg("--version")
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(Duration::from_secs(3), run)
        .await
        .ok()?
        .ok()?;
    out.status
        .success()
        .then(|| parse_version(&String::from_utf8_lossy(&out.stdout)))
        .flatten()
}

pub async fn page(State(state): State<AppState>) -> Result<impl IntoResponse, PageError> {
    let all = discovery::load_all(&state.quadlet_dir)?;
    let count = |kinds: &[UnitKind]| all.iter().filter(|u| kinds.contains(&u.kind)).count();
    let counts = Counts {
        services: count(&[UnitKind::Container, UnitKind::Pod]),
        volumes: count(&[UnitKind::Volume]),
        networks: count(&[UnitKind::Network]),
        images: count(&[UnitKind::Image, UnitKind::Build]),
        all: all.len(),
    };
    let (podman, git) = tokio::join!(version_of("podman"), version_of("git"));
    let tools = vec![
        Tool {
            name: "podman",
            version: podman,
            needed_for: "running containers, pulling images and checking tags",
        },
        Tool {
            name: "git",
            version: git,
            needed_for: "Git Sync",
        },
    ];
    Ok(templates::machine::machine_page(&MachinePage {
        machine: MachineInfo::detect(),
        quadlet_dir: state.quadlet_dir.display().to_string(),
        tools,
        counts,
        health: state.health.get(),
    }))
}
