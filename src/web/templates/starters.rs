//! "Start from a template" on the New unit page: a few starter quadlets per
//! kind, each a plain INI body the editor opens with. They're starting
//! points to edit, so they use placeholder names (`myapp`, `example.com`)
//! rather than pretending to be complete.

use maud::{Markup, html};

use crate::quadlet::UnitKind;

pub struct Starter {
    /// The `?template=` value.
    pub id: &'static str,
    pub kind: UnitKind,
    pub label: &'static str,
    pub body: &'static str,
}

pub const STARTERS: &[Starter] = &[
    Starter {
        id: "web",
        kind: UnitKind::Container,
        label: "Web app",
        body: "[Unit]\nDescription=My web app\n\n[Container]\nImage=docker.io/library/nginx:1.27\nPublishPort=8080:80\nAutoUpdate=registry\n\n[Install]\nWantedBy=default.target\n",
    },
    Starter {
        id: "data",
        kind: UnitKind::Container,
        label: "App with a data volume",
        body: "[Unit]\nDescription=My app\n\n[Container]\nImage=docker.io/library/myapp:1.0\nPublishPort=3000:3000\nVolume=myapp-data.volume:/data\nEnvironment=TZ=${TZ}\n\n[Install]\nWantedBy=default.target\n",
    },
    Starter {
        id: "database",
        kind: UnitKind::Container,
        label: "Database with a secret",
        body: "[Unit]\nDescription=PostgreSQL\n\n[Container]\nImage=docker.io/library/postgres:17\nVolume=postgres-data.volume:/var/lib/postgresql/data\nSecret=postgres-password,type=env,target=POSTGRES_PASSWORD\nHealthCmd=pg_isready -U postgres\nHealthInterval=30s\n\n[Install]\nWantedBy=default.target\n",
    },
    Starter {
        id: "oneshot",
        kind: UnitKind::Container,
        label: "Run once",
        body: "[Unit]\nDescription=One-off job\n\n[Container]\nImage=docker.io/library/alpine:3.21\nExec=echo hello\n\n[Service]\nType=oneshot\nRemainAfterExit=no\n",
    },
    Starter {
        id: "named",
        kind: UnitKind::Volume,
        label: "Named volume",
        body: "[Volume]\n",
    },
    Starter {
        id: "bind",
        kind: UnitKind::Volume,
        label: "Folder on the host",
        body: "[Volume]\nDevice=/srv/myapp\nType=none\nOptions=bind\n",
    },
    Starter {
        id: "bridge",
        kind: UnitKind::Network,
        label: "Bridge network",
        body: "[Network]\n",
    },
    Starter {
        id: "internal",
        kind: UnitKind::Network,
        label: "Internal only (no internet)",
        body: "[Network]\nInternal=true\n",
    },
    Starter {
        id: "pull",
        kind: UnitKind::Image,
        label: "Pull an image",
        body: "[Image]\nImage=docker.io/library/nginx:1.27\n",
    },
];

/// The starters offered for a kind, in order.
pub fn for_kind(kind: UnitKind) -> impl Iterator<Item = &'static Starter> {
    STARTERS.iter().filter(move |s| s.kind == kind)
}

/// The starter `id` names for `kind`, if any.
pub fn find(kind: UnitKind, id: &str) -> Option<&'static Starter> {
    for_kind(kind).find(|s| s.id == id)
}

/// The row of template links above the editor. Each one reloads the page
/// with that starter in the editor; `current` is highlighted.
pub fn picker(kind: UnitKind, current: Option<&str>) -> Markup {
    html! {
        @if for_kind(kind).next().is_some() {
            div.starter-picker {
                span.field-hint { "Start from a template:" }
                @for s in for_kind(kind) {
                    a.chip.starter href={"?template=" (s.id)}
                        aria-current=[(current == Some(s.id)).then_some("true")] { (s.label) }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_starter_opens_with_its_kinds_section() {
        for s in STARTERS {
            assert!(
                s.body.contains(&format!("[{}]", s.kind.primary_section())),
                "{}",
                s.id
            );
        }
        assert!(find(UnitKind::Container, "web").is_some());
        assert!(find(UnitKind::Volume, "web").is_none());
    }

    #[test]
    fn picker_marks_the_current_starter() {
        let html = picker(UnitKind::Network, Some("internal")).into_string();
        assert!(html.contains(r#"href="?template=internal" aria-current="true""#));
        assert!(picker(UnitKind::Kube, None).into_string().is_empty());
    }
}
