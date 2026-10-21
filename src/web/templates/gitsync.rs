use std::collections::BTreeMap;
use std::time::SystemTime;

use maud::{Markup, html};

use super::settings::{GithubTokenStatus, github_token_summary};
use super::{BannerKind, Icon, NavItem, banner, csrf_input, group_field, icon, page_header, shell};
use crate::health::Health;
use crate::quadlet::gitsync::git::RemoteBranches;
use crate::quadlet::gitsync::{GitSyncConfig, SyncState, SyncStatus};

/// One sync card's data: its config, live status, the `Secret=` names its
/// units reference that podman doesn't have yet, and the `${NAME}` host
/// variables they reference that the user manager doesn't have (each name ->
/// file names), from `refs::missing_secrets` / `refs::missing_env`.
pub type SyncEntry = (
    GitSyncConfig,
    SyncStatus,
    BTreeMap<String, Vec<String>>,
    BTreeMap<String, Vec<String>>,
);

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

pub fn page(
    entries: &[SyncEntry],
    csrf: &str,
    known_groups: &[String],
    token: &GithubTokenStatus,
    health: Health,
) -> Markup {
    render(
        entries,
        csrf,
        &AddFormValues::default(),
        known_groups,
        token,
        None,
        health,
    )
}

pub fn page_with_add_error(
    entries: &[SyncEntry],
    csrf: &str,
    values: &AddFormValues,
    known_groups: &[String],
    token: &GithubTokenStatus,
    error: &str,
    health: Health,
) -> Markup {
    render(
        entries,
        csrf,
        values,
        known_groups,
        token,
        Some(error),
        health,
    )
}

fn render(
    entries: &[SyncEntry],
    csrf: &str,
    values: &AddFormValues,
    known_groups: &[String],
    token: &GithubTokenStatus,
    add_error: Option<&str>,
    health: Health,
) -> Markup {
    let body = html! {
        (page_header("Git Sync", html! {
            button.btn.btn-sm.btn-primary type="button" data-gitsync-add {
                (icon(Icon::Plus)) span { "Git sync" }
            }
        }))
        p.page-meta {
            "Keep a group directory in sync with a git repository. Sooth checks each "
            "remote on its own interval and fast-forwards the local checkout when it "
            "moves; files inside a synced group are managed by the remote and are "
            "overwritten on the next sync, so don't hand-edit them here."
        }

        div #git-sync-rows hx-get="/git-sync/rows" hx-trigger="sse:git-sync-changed delay:300ms"
            hx-swap="innerHTML" {
            (rows(entries, csrf))
        }

        (add_dialog(csrf, values, known_groups, token, add_error))
        (edit_dialog(csrf))
    };
    shell("Git Sync", Some(NavItem::GitSync), Some(health), body)
}

/// The "Add a git-sync" modal, opened from the page header. A rejected
/// submission re-renders the page with it already open
/// (`data-gitsync-open-on-load`, see `frontend/gitsync.js`) and the values as
/// typed. A plain form post, not htmx: it redirects back to the page once
/// the clone is done.
fn add_dialog(
    csrf: &str,
    values: &AddFormValues,
    known_groups: &[String],
    token: &GithubTokenStatus,
    error: Option<&str>,
) -> Markup {
    html! {
        dialog.modal-dialog.gitsync-dialog #git-sync-add-dialog data-gitsync-dialog
            data-gitsync-open-on-load[error.is_some()] {
            div.modal-header {
                h2 { "Add a git sync" }
                button.btn-icon type="button" data-gitsync-dialog-close aria-label="Close" {
                    (icon(Icon::X))
                }
            }
            form.modal-body method="post" action="/git-sync" id="git-sync-add-form" {
                @if let Some(msg) = error { (banner(BannerKind::Error, msg)) }
                (csrf_input(csrf))
                div.gitsync-grid {
                    div {
                        (group_field(&values.group, known_groups, true))
                        p.field-hint.gitsync-hint {
                            "A new or empty folder, e.g. " code { "media" } " or "
                            code { "infra/monitoring" } " -- the repository is cloned straight into it."
                        }
                    }
                    div.field {
                        label for="remote" { "Remote URL" }
                        input.input type="text" id="remote" name="remote" value=(values.remote)
                            placeholder="git@github.com:you/quadlets.git" required
                            autocomplete="off" autocapitalize="off" spellcheck="false";
                        p.field-hint {
                            "Anything " code { "git clone" } " accepts. A private "
                            code { "https://github.com/…" } " repo uses the GitHub token from "
                            a href="/settings#github-token-card" { "Settings" } "; other private "
                            "remotes use this user's own git auth (SSH agent, "
                            code { "~/.ssh/config" } ", a credential helper)."
                        }
                        p.field-hint #git-sync-token-status { (github_token_summary(token)) }
                    }
                    (branch_field(BranchMode::Add, &values.remote, &values.branch))
                    (interval_field("poll_interval_secs", &values.poll_interval_secs))
                }
                div.modal-actions.gitsync-dialog-actions {
                    p.field-hint.gitsync-wait #git-sync-add-wait hidden {
                        "Cloning the repository -- this can take a moment for a large one."
                    }
                    button.btn.btn-ghost type="button" data-gitsync-dialog-close { "Cancel" }
                    button.btn.btn-primary type="submit" id="git-sync-add-submit" { "Add" }
                }
            }
        }
    }
}

/// The page's one shared Edit modal, filled in from the clicked card's Edit
/// button by `frontend/gitsync.js`. It lives outside `#git-sync-rows`, so a
/// `git-sync-changed` refresh of the cards never closes it mid-edit. Posts
/// over htmx; the script closes it on success and shows a rejection's
/// message inline.
fn edit_dialog(csrf: &str) -> Markup {
    html! {
        dialog.modal-dialog.gitsync-dialog #git-sync-edit-dialog data-gitsync-dialog
            data-gitsync-edit-dialog {
            div.modal-header {
                h2 { "Edit " code data-gitsync-edit-group {} }
                button.btn-icon type="button" data-gitsync-dialog-close aria-label="Close" {
                    (icon(Icon::X))
                }
            }
            form.modal-body hx-post="/git-sync/edit" hx-swap="none"
                hx-disabled-elt="find button[type=submit]" {
                div data-gitsync-edit-error {}
                (csrf_input(csrf))
                input type="hidden" name="group" value="";
                div.gitsync-grid {
                    div.field {
                        label for="git-sync-edit-remote" { "Remote URL" }
                        input.input type="text" id="git-sync-edit-remote" name="remote" required
                            autocomplete="off" autocapitalize="off" spellcheck="false";
                        p.field-hint {
                            "Changing the remote or branch checks the new one out, replacing "
                            "the files in the group."
                        }
                    }
                    (branch_field(BranchMode::Edit, "", ""))
                    (interval_field("git-sync-edit-interval", ""))
                }
                div.modal-actions.gitsync-dialog-actions {
                    button.btn.btn-ghost type="button" data-gitsync-dialog-close { "Cancel" }
                    button.btn.btn-primary type="submit" { "Save" }
                }
            }
        }
    }
}

/// "Check every [60] seconds" -- the poll interval, sized for a number.
fn interval_field(id: &str, value: &str) -> Markup {
    html! {
        div.field {
            label for=(id) { "Check every" }
            div.input-unit {
                input.input type="text" id=(id) name="poll_interval_secs" value=(value)
                    inputmode="numeric" required;
                span { "seconds" }
            }
        }
    }
}

/// The status cards only -- the payload of `GET /git-sync/rows`, re-fetched
/// by the page on `sse:git-sync-changed` (see `crate::events::DashboardEvent`).
pub fn rows(entries: &[SyncEntry], csrf: &str) -> Markup {
    html! {
        @if entries.is_empty() {
            p.empty { "No git-synced groups configured yet." }
        } @else {
            @for (config, status, secrets, env) in entries {
                (sync_card(config, status, secrets, env, csrf))
            }
        }
    }
}

fn sync_card(
    config: &GitSyncConfig,
    status: &SyncStatus,
    missing_secrets: &BTreeMap<String, Vec<String>>,
    missing_env: &BTreeMap<String, Vec<String>>,
    csrf: &str,
) -> Markup {
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
                (super::secrets::missing_row(missing_secrets))
                (super::environment::missing_row(missing_env))
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
                button.btn.btn-sm type="button" data-gitsync-edit=(config.group)
                    data-remote=(config.remote) data-branch=(config.branch.as_deref().unwrap_or(""))
                    data-interval=(config.poll_interval_secs) { "Edit" }
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
                            "Left unchecked, Sooth just stops checking this repo and leaves \""
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

/// Which form a branch picker sits in. In the add form a blank branch
/// means "track the remote's default"; in a sync's Edit form it means "keep
/// the branch already tracked" (see `GitSyncManager::edit`), and a listed
/// branch is always preselected instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchMode {
    Add,
    Edit,
}

impl BranchMode {
    fn as_str(self) -> &'static str {
        match self {
            BranchMode::Add => "add",
            BranchMode::Edit => "edit",
        }
    }
}

/// The Branch field: a text box until the remote's branches are listed, then
/// a `<select>` of them. The picker re-fetches `POST /git-sync/branches`
/// (with the whole form, so the typed remote and current choice go along)
/// whenever the remote changes -- and, in the Edit dialog, each time it
/// opens for a sync (`frontend/gitsync.js` fires `gitsync:list-branches`),
/// so the page doesn't `ls-remote` every sync on load.
fn branch_field(mode: BranchMode, remote: &str, selected: &str) -> Markup {
    let trigger = match mode {
        BranchMode::Add if remote.trim().is_empty() => "change from:#remote",
        BranchMode::Add => "load, change from:#remote",
        BranchMode::Edit => {
            "gitsync:list-branches, change[target.name=='remote'] from:closest form"
        }
    };
    let vals = format!(r#"{{"mode":"{}"}}"#, mode.as_str());
    html! {
        div.field {
            label for=(branch_id(mode)) {
                "Branch " span.htmx-indicator.muted { "listing branches…" }
            }
            // Target and swap spelled out: the Edit form around it is
            // `hx-swap="none"`, which it would otherwise inherit.
            div.branch-picker hx-post="/git-sync/branches" hx-trigger=(trigger)
                hx-target="this" hx-swap="innerHTML"
                hx-vals=(vals) hx-sync="this:replace" hx-indicator="closest .field" {
                @match mode {
                    BranchMode::Edit if !selected.is_empty() => {
                        select.input name="branch" { option value=(selected) selected { (selected) } }
                    }
                    _ => (branch_text(mode, selected, None)),
                }
            }
        }
    }
}

/// The picker's contents for a listing attempt -- the payload of
/// `POST /git-sync/branches`. A failed listing (or no remote yet) falls back
/// to the plain text box, with the reason, so a remote `ls-remote` can't
/// reach (but a clone somehow could) still takes a typed branch.
pub fn branch_picker(
    mode: BranchMode,
    selected: &str,
    listing: Option<&Result<RemoteBranches, String>>,
) -> Markup {
    let listing = match listing {
        None => return branch_text(mode, selected, None),
        Some(Err(e)) => return branch_text(mode, selected, Some(e)),
        Some(Ok(l)) if l.branches.is_empty() => {
            return branch_text(mode, selected, Some("the remote has no branches"));
        }
        Some(Ok(l)) => l,
    };
    let selected = match mode {
        BranchMode::Edit if selected.is_empty() => listing.default.as_deref().unwrap_or(""),
        _ => selected,
    };
    let unlisted = !selected.is_empty() && !listing.branches.iter().any(|b| b == selected);
    html! {
        select.input id=(branch_id(mode)) name="branch" {
            @if mode == BranchMode::Add {
                option value="" selected[selected.is_empty()] {
                    "Remote default"
                    @if let Some(d) = &listing.default { " (" (d) ")" }
                }
            }
            @for b in &listing.branches {
                option value=(b) selected[b == selected] {
                    (b)
                    @if listing.default.as_deref() == Some(b.as_str()) { " (default)" }
                }
            }
            @if unlisted {
                option value=(selected) selected { (selected) " (not on remote)" }
            }
        }
    }
}

fn branch_text(mode: BranchMode, value: &str, error: Option<&str>) -> Markup {
    let blank = match mode {
        BranchMode::Add => "Leave blank to track the remote's default branch.",
        BranchMode::Edit => "Leave blank to keep tracking the current branch.",
    };
    html! {
        input.input type="text" id=(branch_id(mode)) name="branch"
            value=(value) placeholder="main"
            autocomplete="off" autocapitalize="off" spellcheck="false";
        @if let Some(e) = error {
            p.field-hint.field-error { "Couldn't list the remote's branches: " (e) }
        }
        p.field-hint { (blank) }
    }
}

/// Both dialogs are in the page at once, so each picker gets its own id.
fn branch_id(mode: BranchMode) -> &'static str {
    match mode {
        BranchMode::Add => "branch",
        BranchMode::Edit => "git-sync-edit-branch",
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

#[cfg(test)]
mod tests {
    use super::*;

    fn listing() -> Result<RemoteBranches, String> {
        Ok(RemoteBranches {
            default: Some("main".into()),
            branches: vec!["main".into(), "staging".into()],
        })
    }

    fn config() -> GitSyncConfig {
        GitSyncConfig {
            group: "infra/web".into(),
            remote: "git@example.com:me/q.git".into(),
            branch: Some("staging".into()),
            poll_interval_secs: 90,
        }
    }

    #[test]
    fn a_card_edit_button_carries_what_the_shared_dialog_needs() {
        let entry = (
            config(),
            SyncStatus::default(),
            Default::default(),
            Default::default(),
        );
        let html = rows(&[entry], "tok").into_string();
        assert!(html.contains(r#"data-gitsync-edit="infra/web""#));
        assert!(html.contains(r#"data-remote="git@example.com:me/q.git""#));
        assert!(html.contains(r#"data-branch="staging""#));
        assert!(html.contains(r#"data-interval="90""#));
    }

    #[test]
    fn only_a_rejected_add_reopens_its_dialog() {
        let values = AddFormValues::default();
        let open = add_dialog(
            "tok",
            &values,
            &[],
            &GithubTokenStatus::default(),
            Some("bad"),
        );
        let shut = add_dialog("tok", &values, &[], &GithubTokenStatus::default(), None);
        assert!(open.into_string().contains("data-gitsync-open-on-load"));
        assert!(!shut.into_string().contains("data-gitsync-open-on-load"));
    }

    #[test]
    fn add_picker_offers_the_remote_default_as_a_blank_choice() {
        let html = branch_picker(BranchMode::Add, "", Some(&listing())).into_string();
        assert!(html.contains(r#"<option value="" selected>Remote default (main)</option>"#));
        assert!(html.contains(r#"<option value="staging">staging</option>"#));
        assert!(html.contains(r#"id="branch""#));
    }

    #[test]
    fn edit_picker_preselects_the_tracked_branch_with_no_blank_choice() {
        let html = branch_picker(BranchMode::Edit, "staging", Some(&listing())).into_string();
        assert!(html.contains(r#"<option value="staging" selected>staging</option>"#));
        assert!(!html.contains(r#"value="""#));
    }

    #[test]
    fn a_branch_missing_from_the_remote_stays_selectable() {
        let html = branch_picker(BranchMode::Edit, "gone", Some(&listing())).into_string();
        assert!(html.contains(r#"<option value="gone" selected>gone (not on remote)</option>"#));
    }

    #[test]
    fn a_failed_listing_falls_back_to_a_text_box_with_the_reason() {
        let failed = Err("Repository not found".to_string());
        let html = branch_picker(BranchMode::Add, "dev", Some(&failed)).into_string();
        assert!(html.contains("Repository not found"));
        assert!(html.contains(r#"type="text""#) && html.contains(r#"value="dev""#));
        assert!(!html.contains("<select"));
    }
}
