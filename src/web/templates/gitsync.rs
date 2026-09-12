use std::time::SystemTime;

use maud::{Markup, html};

use super::{BannerKind, NavItem, banner, csrf_input, page_header, shell};
use crate::health::Health;
use crate::quadlet::gitsync::{GitSyncConfig, SyncState, SyncStatus};

/// The add-sync form's values, round-tripped as plain strings so a rejected
/// submission redisplays exactly as typed -- same pattern as
/// `settings::FormValues`.
pub struct AddFormValues {
    pub group: String,
    pub remote: String,
    pub branch: String,
    pub poll_interval_secs: String,
}

impl Default for AddFormValues {
    fn default() -> Self {
        Self {
            group: String::new(),
            remote: String::new(),
            branch: String::new(),
            poll_interval_secs: "60".to_string(),
        }
    }
}

pub fn page(entries: &[(GitSyncConfig, SyncStatus)], csrf: &str, health: Health) -> Markup {
    render(entries, csrf, &AddFormValues::default(), None, health)
}

pub fn page_with_add_error(
    entries: &[(GitSyncConfig, SyncStatus)],
    csrf: &str,
    values: &AddFormValues,
    error: &str,
    health: Health,
) -> Markup {
    render(entries, csrf, values, Some(error), health)
}

fn render(
    entries: &[(GitSyncConfig, SyncStatus)],
    csrf: &str,
    values: &AddFormValues,
    add_error: Option<&str>,
    health: Health,
) -> Markup {
    let body = html! {
        (page_header("Git Sync", html! {}))
        p.page-meta {
            "Keep a group directory in sync with a git repository. sooth checks each "
            "remote on its own interval and fast-forwards the local checkout when it "
            "moves; files inside a synced group are managed by the remote and are "
            "overwritten on the next sync, so don't hand-edit them here."
        }

        div #git-sync-rows hx-get="/git-sync/rows" hx-trigger="sse:git-sync-changed delay:300ms"
            hx-swap="innerHTML" {
            (rows(entries, csrf))
        }

        h2 { "Add a git-sync" }
        div.card {
            @if let Some(msg) = add_error { (banner(BannerKind::Error, msg)) }
            form.settings-form method="post" action="/git-sync" id="git-sync-add-form" {
                (csrf_input(csrf))
                div.field {
                    label for="group" { "Group" }
                    p.field-hint {
                        "A new or empty subdirectory of the quadlet directory, e.g. "
                        code { "media" } " or " code { "infra/monitoring" } "."
                    }
                    input.input type="text" id="group" name="group" value=(values.group)
                        placeholder="media" required;
                }
                div.field {
                    label for="remote" { "Remote URL" }
                    p.field-hint {
                        "Anything " code { "git clone" } " accepts. Private repos need this "
                        "user's own git to already be able to reach it (SSH agent, "
                        code { "~/.ssh/config" } ", or a credential helper) -- sooth has no "
                        "credentials of its own."
                    }
                    input.input type="text" id="remote" name="remote" value=(values.remote)
                        placeholder="git@github.com:you/quadlets.git" required;
                }
                div.field {
                    label for="branch" { "Branch" }
                    p.field-hint { "Leave blank to track the remote's default branch." }
                    input.input type="text" id="branch" name="branch" value=(values.branch)
                        placeholder="main";
                }
                div.field {
                    label for="poll_interval_secs" { "Poll interval (seconds)" }
                    p.field-hint { "How often to check the remote for new commits." }
                    input.input type="text" id="poll_interval_secs" name="poll_interval_secs"
                        value=(values.poll_interval_secs);
                }
                button.btn.btn-primary type="submit" id="git-sync-add-submit" { "Add" }
                p.field-hint #git-sync-add-wait hidden {
                    "Cloning the repository -- this can take a moment for a large one."
                }
            }
        }
        // The POST blocks on a real `git clone` (see
        // `GitSyncManager::add`), so this is a genuine multi-second wait,
        // not an instant round trip -- swap the button for a spinner and
        // reveal the note above so it reads as "working", not stalled.
        // Plain inline script (same pattern as `restarting_page`'s) since
        // this page-specific one-shot behavior doesn't earn a spot in the
        // bundled `frontend/**`.
        script {
            (maud::PreEscaped(
                "document.getElementById('git-sync-add-form').addEventListener('submit', function () {\
                 document.getElementById('git-sync-add-submit').classList.add('btn-loading');\
                 document.getElementById('git-sync-add-submit').disabled = true;\
                 document.getElementById('git-sync-add-wait').hidden = false;\
                 });"
            ))
        }
    };
    shell("Git Sync", Some(NavItem::GitSync), Some(health), body)
}

/// The status cards only -- the payload of `GET /git-sync/rows`, re-fetched
/// by the page on `sse:git-sync-changed` (see `crate::events::DashboardEvent`).
pub fn rows(entries: &[(GitSyncConfig, SyncStatus)], csrf: &str) -> Markup {
    html! {
        @if entries.is_empty() {
            p.empty { "No git-synced groups configured yet." }
        } @else {
            @for (config, status) in entries {
                (sync_card(config, status, csrf))
            }
        }
    }
}

fn sync_card(config: &GitSyncConfig, status: &SyncStatus, csrf: &str) -> Markup {
    // A single-quoted JS string embedded straight into an `onsubmit`
    // attribute -- safe because `naming::valid_group` (checked server-side
    // on every write) restricts a group to `[A-Za-z0-9_.-]` and `/`, so it
    // can never contain a `'`, a `\`, or a newline that would escape it.
    let confirm_js = format!(
        "if (this.confirm_name.value !== '{group}') {{ \
         alert('Type \"{group}\" exactly to confirm removing this sync.'); return false; }} \
         return true;",
        group = config.group,
    );
    html! {
        div.card {
            div.section-header {
                h2 { (config.group) }
                div.actions { (state_badge(&status.state)) }
            }
            table.kv-table {
                tr { td { "Remote" } td { (config.remote) } }
                tr { td { "Branch" } td { (config.branch.as_deref().unwrap_or("(default)")) } }
                tr { td { "Poll interval" } td { (config.poll_interval_secs) "s" } }
                tr { td { "Last checked" } td { (checked_at(status)) } }
                @if let SyncState::Error(msg) = &status.state {
                    tr { td { "Error" } td { (msg) } }
                }
            }
            div.gitsync-actions {
                form.inline-form method="post" action="/git-sync/sync" {
                    (csrf_input(csrf))
                    input type="hidden" name="group" value=(config.group);
                    button.btn.btn-sm type="submit" { "Sync now" }
                }
                @if matches!(status.state, SyncState::Error(_)) {
                    form.inline-form method="post" action="/git-sync/force" {
                        (csrf_input(csrf))
                        input type="hidden" name="group" value=(config.group);
                        button.btn.btn-sm type="submit" { "Force resync" }
                    }
                }
                details.gitsync-disclosure data-group=(config.group) data-kind="edit" {
                    summary.btn.btn-sm { "Edit" }
                    form.gitsync-disclosure-body
                        hx-post="/git-sync/edit" hx-swap="none"
                        hx-on::response-error="alert('That change was rejected — check the remote and branch and try again.')" {
                        (csrf_input(csrf))
                        input type="hidden" name="group" value=(config.group);
                        div.field {
                            label { "Remote URL" }
                            input.input type="text" name="remote" value=(config.remote) required;
                        }
                        div.field {
                            label { "Branch" }
                            p.field-hint { "Leave blank to keep tracking the current branch." }
                            input.input type="text" name="branch"
                                value=(config.branch.as_deref().unwrap_or(""))
                                placeholder=(config.branch.as_deref().unwrap_or("main"));
                        }
                        div.field {
                            label { "Poll interval (seconds)" }
                            input.input type="text" name="poll_interval_secs"
                                value=(config.poll_interval_secs);
                        }
                        button.btn.btn-sm.btn-primary type="submit" { "Save" }
                    }
                }
            }
            // Set apart from the routine actions above: the trigger itself
            // is always a plain button, and only reveals the confirmation
            // form (typed-name check + the file-deleting checkbox) when
            // clicked, so "Remove" is never one accidental click away from
            // actually removing anything.
            details.gitsync-disclosure.gitsync-danger data-group=(config.group) data-kind="remove" {
                summary.btn.btn-sm.btn-danger { "Remove" }
                form.gitsync-disclosure-body method="post" action="/git-sync/delete"
                    onsubmit=(confirm_js) {
                    (csrf_input(csrf))
                    input type="hidden" name="group" value=(config.group);
                    div.field {
                        label.checkbox-line {
                            input type="checkbox" name="delete_files";
                            "Also permanently delete " code { (config.group) } " and its files from disk"
                        }
                        p.field-hint {
                            "Left unchecked, sooth just stops checking this repo and leaves \""
                            (config.group) "\" on disk as an ordinary, no-longer-synced group."
                        }
                    }
                    div.field {
                        label { "Type " code { (config.group) } " to confirm" }
                        input.input type="text" name="confirm_name" autocomplete="off"
                            autocapitalize="off" spellcheck="false" placeholder=(config.group);
                    }
                    button.btn.btn-sm.btn-danger type="submit" { "Remove sync" }
                }
            }
        }
    }
}

fn state_badge(state: &SyncState) -> Markup {
    let (class, label) = match state {
        SyncState::Cloning => ("badge-warn", "Cloning…".to_string()),
        SyncState::Checking => ("badge-warn", "Checking…".to_string()),
        SyncState::Syncing => ("badge-warn", "Syncing…".to_string()),
        SyncState::UpToDate { commit } => (
            "badge-running",
            format!("Up to date @ {}", short_commit(commit)),
        ),
        SyncState::Error(_) => ("badge-failed", "Error".to_string()),
    };
    html! { span class={"badge " (class)} { (label) } }
}

fn short_commit(commit: &str) -> &str {
    &commit[..commit.len().min(8)]
}

/// A coarse "how long ago" for the last check, e.g. `"42s ago"` /
/// `"3m ago"` / `"2h ago"`. No new dependency for something this rough --
/// nothing here needs calendar-accurate formatting, just a sense of
/// freshness.
fn checked_at(status: &SyncStatus) -> String {
    let Some(at) = status.checked_at else {
        return "never".to_string();
    };
    let secs = SystemTime::now()
        .duration_since(at)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}
