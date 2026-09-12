//! Thin async wrappers around the `git` CLI -- the app's second shell-out,
//! after `journal.rs`'s `journalctl`. Every invocation disables the
//! interactive credential prompt (`GIT_TERMINAL_PROMPT=0`) so a bad remote or
//! missing credentials fails fast instead of hanging a poll loop forever, and
//! is bounded by [`TIMEOUT`] for the same reason. Auth itself is left
//! entirely to the host: whatever SSH agent, `~/.ssh/config`, or credential
//! helper already works for this user's own `git` is what sooth gets too,
//! since it runs the exact same binary as the exact same user.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use super::GitSyncError;

const TIMEOUT: Duration = Duration::from_secs(30);

fn base_command() -> Command {
    let mut cmd = Command::new("git");
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

/// Runs `cmd`, mapping a missing binary / timeout / non-zero exit to
/// `GitSyncError`, and returns trimmed stdout on success.
async fn run(mut cmd: Command) -> Result<String, GitSyncError> {
    let output = tokio::time::timeout(TIMEOUT, cmd.output())
        .await
        .map_err(|_| GitSyncError::Timeout)?
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                GitSyncError::GitNotFound
            } else {
                GitSyncError::Io(e)
            }
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(GitSyncError::Failed(if stderr.is_empty() {
            format!("git exited with {}", output.status)
        } else {
            stderr
        }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// `git clone [--branch <branch>] --single-branch <remote> <dest>`. `dest`
/// must already exist (created by the caller) and be empty -- `git clone`
/// accepts an existing empty directory as its target.
pub async fn clone(remote: &str, branch: Option<&str>, dest: &Path) -> Result<(), GitSyncError> {
    let mut cmd = base_command();
    cmd.arg("clone").arg("--single-branch");
    if let Some(branch) = branch {
        cmd.arg("--branch").arg(branch);
    }
    cmd.arg(remote).arg(dest);
    run(cmd).await?;
    Ok(())
}

/// `git -C <dir> fetch --quiet origin <branch>`.
pub async fn fetch(dir: &Path, branch: &str) -> Result<(), GitSyncError> {
    let mut cmd = base_command();
    cmd.arg("-C")
        .arg(dir)
        .arg("fetch")
        .arg("--quiet")
        .arg("origin")
        .arg(branch);
    run(cmd).await?;
    Ok(())
}

/// `git -C <dir> fetch --quiet origin <branch>:refs/remotes/origin/<branch>`
/// -- an explicit refspec, unlike [`fetch`], so the remote-tracking ref
/// exists locally even for a branch the original `--single-branch` clone
/// never fetched. Needed before [`checkout_branch`] can switch to a branch
/// that isn't the one already checked out (see `GitSyncManager::edit`).
pub async fn fetch_ref(dir: &Path, branch: &str) -> Result<(), GitSyncError> {
    let mut cmd = base_command();
    cmd.arg("-C")
        .arg(dir)
        .arg("fetch")
        .arg("--quiet")
        .arg("origin")
        .arg(format!("{branch}:refs/remotes/origin/{branch}"));
    run(cmd).await?;
    Ok(())
}

/// `git -C <dir> remote set-url origin <remote>`.
pub async fn set_remote_url(dir: &Path, remote: &str) -> Result<(), GitSyncError> {
    let mut cmd = base_command();
    cmd.arg("-C")
        .arg(dir)
        .arg("remote")
        .arg("set-url")
        .arg("origin")
        .arg(remote);
    run(cmd).await?;
    Ok(())
}

/// `git -C <dir> checkout -B <branch> origin/<branch> --` -- (re)creates the
/// local branch pointing at the remote-tracking ref and checks it out,
/// discarding whatever the working tree previously held (consistent with
/// [`super::manager`]'s "files in a synced group are remote-managed"
/// posture). Used when a sync's tracked branch or remote changes, after
/// [`fetch_ref`] has made sure `origin/<branch>` exists locally.
pub async fn checkout_branch(dir: &Path, branch: &str) -> Result<(), GitSyncError> {
    let mut cmd = base_command();
    cmd.arg("-C")
        .arg(dir)
        .arg("checkout")
        .arg("-B")
        .arg(branch)
        .arg(format!("origin/{branch}"))
        .arg("--");
    run(cmd).await?;
    Ok(())
}

/// `git -C <dir> rev-parse <rev>`.
pub async fn rev_parse(dir: &Path, rev: &str) -> Result<String, GitSyncError> {
    let mut cmd = base_command();
    cmd.arg("-C").arg(dir).arg("rev-parse").arg(rev);
    run(cmd).await
}

/// `git -C <dir> rev-parse --abbrev-ref HEAD` -- the branch name actually
/// checked out, used to resolve `GitSyncConfig.branch` when it wasn't pinned
/// up front.
pub async fn current_branch(dir: &Path) -> Result<String, GitSyncError> {
    let mut cmd = base_command();
    cmd.arg("-C")
        .arg(dir)
        .arg("rev-parse")
        .arg("--abbrev-ref")
        .arg("HEAD");
    run(cmd).await
}

/// Whether `ancestor` is an ancestor of (or equal to) `descendant`, i.e.
/// whether moving from `ancestor` to `descendant` is a pure fast-forward --
/// `git merge-base --is-ancestor`, which signals via exit status rather than
/// output: `0` yes, `1` no, anything else a real error (e.g. an unknown rev).
pub async fn is_ancestor(
    dir: &Path,
    ancestor: &str,
    descendant: &str,
) -> Result<bool, GitSyncError> {
    let mut cmd = base_command();
    cmd.arg("-C")
        .arg(dir)
        .arg("merge-base")
        .arg("--is-ancestor")
        .arg(ancestor)
        .arg(descendant);
    let output = tokio::time::timeout(TIMEOUT, cmd.output())
        .await
        .map_err(|_| GitSyncError::Timeout)?
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                GitSyncError::GitNotFound
            } else {
                GitSyncError::Io(e)
            }
        })?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(GitSyncError::Failed(stderr))
        }
    }
}

/// `git -C <dir> reset --hard <rev>`.
pub async fn reset_hard(dir: &Path, rev: &str) -> Result<(), GitSyncError> {
    let mut cmd = base_command();
    cmd.arg("-C").arg(dir).arg("reset").arg("--hard").arg(rev);
    run(cmd).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a plain (non-wrapped) `git` command for test setup/assertions --
    /// separate from the `base_command`/`run` pair under test.
    fn git(dir: &Path, args: &[&str]) -> std::process::Output {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            // Override any host-global `commit.gpgsign=true`: these are
            // disposable fixtures, and a host gpg-agent that needs an
            // interactive pinentry (or has none configured) would otherwise
            // hang or fail commits, unrelated to what these tests check.
            .arg("-c")
            .arg("commit.gpgsign=false")
            .args(args)
            .env("GIT_AUTHOR_NAME", "sooth-test")
            .env("GIT_AUTHOR_EMAIL", "sooth-test@example.invalid")
            .env("GIT_COMMITTER_NAME", "sooth-test")
            .env("GIT_COMMITTER_EMAIL", "sooth-test@example.invalid")
            .output()
            .expect("git must be installed to run this test")
    }

    /// A local repo with one commit on `main`, usable as a clone source with
    /// no network involved.
    fn make_origin() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            git(dir.path(), &["init", "--quiet", "--initial-branch=main"])
                .status
                .success()
        );
        std::fs::write(dir.path().join("web.container"), "[Container]\nImage=x\n").unwrap();
        assert!(git(dir.path(), &["add", "."]).status.success());
        assert!(
            git(dir.path(), &["commit", "--quiet", "-m", "initial"])
                .status
                .success()
        );
        dir
    }

    #[tokio::test]
    async fn clone_checks_out_the_requested_branch() {
        let origin = make_origin();
        let dest = tempfile::tempdir().unwrap();
        std::fs::remove_dir(dest.path()).unwrap(); // clone requires a non-existent or empty dir

        clone(
            &origin.path().display().to_string(),
            Some("main"),
            dest.path(),
        )
        .await
        .unwrap();

        assert!(dest.path().join("web.container").is_file());
        assert_eq!(current_branch(dest.path()).await.unwrap(), "main");
    }

    #[tokio::test]
    async fn fetch_and_ancestor_detect_a_fast_forward() {
        let origin = make_origin();
        let dest = tempfile::tempdir().unwrap();
        std::fs::remove_dir(dest.path()).unwrap();
        clone(
            &origin.path().display().to_string(),
            Some("main"),
            dest.path(),
        )
        .await
        .unwrap();

        let before = rev_parse(dest.path(), "HEAD").await.unwrap();

        // Move the origin forward.
        std::fs::write(
            origin.path().join("web.container"),
            "[Container]\nImage=y\n",
        )
        .unwrap();
        assert!(
            git(origin.path(), &["commit", "--quiet", "-am", "update"])
                .status
                .success()
        );

        fetch(dest.path(), "main").await.unwrap();
        let remote = rev_parse(dest.path(), "origin/main").await.unwrap();
        assert_ne!(before, remote);
        assert!(is_ancestor(dest.path(), &before, &remote).await.unwrap());

        reset_hard(dest.path(), &remote).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dest.path().join("web.container")).unwrap(),
            "[Container]\nImage=y\n"
        );
    }

    #[tokio::test]
    async fn fetch_ref_and_checkout_branch_switch_tracked_branches() {
        let origin = make_origin();
        assert!(
            git(origin.path(), &["checkout", "--quiet", "-b", "staging"])
                .status
                .success()
        );
        std::fs::write(
            origin.path().join("web.container"),
            "[Container]\nImage=staging\n",
        )
        .unwrap();
        assert!(
            git(
                origin.path(),
                &["commit", "--quiet", "-am", "staging build"]
            )
            .status
            .success()
        );

        let dest = tempfile::tempdir().unwrap();
        std::fs::remove_dir(dest.path()).unwrap();
        clone(
            &origin.path().display().to_string(),
            Some("main"),
            dest.path(),
        )
        .await
        .unwrap();
        assert_eq!(current_branch(dest.path()).await.unwrap(), "main");

        // Switching the tracked branch (the `GitSyncManager::edit` path):
        // fetch the new branch's ref explicitly, since `--single-branch`
        // never brought it down, then check it out.
        fetch_ref(dest.path(), "staging").await.unwrap();
        checkout_branch(dest.path(), "staging").await.unwrap();

        assert_eq!(current_branch(dest.path()).await.unwrap(), "staging");
        assert_eq!(
            std::fs::read_to_string(dest.path().join("web.container")).unwrap(),
            "[Container]\nImage=staging\n"
        );
    }

    #[tokio::test]
    async fn set_remote_url_repoints_origin() {
        let origin = make_origin();
        let other_origin = make_origin();
        let dest = tempfile::tempdir().unwrap();
        std::fs::remove_dir(dest.path()).unwrap();
        clone(
            &origin.path().display().to_string(),
            Some("main"),
            dest.path(),
        )
        .await
        .unwrap();

        set_remote_url(dest.path(), &other_origin.path().display().to_string())
            .await
            .unwrap();

        let url = git(dest.path(), &["remote", "get-url", "origin"]);
        assert_eq!(
            String::from_utf8_lossy(&url.stdout).trim(),
            other_origin.path().display().to_string()
        );
    }

    #[tokio::test]
    async fn is_ancestor_is_false_after_divergence() {
        let origin = make_origin();
        let dest = tempfile::tempdir().unwrap();
        std::fs::remove_dir(dest.path()).unwrap();
        clone(
            &origin.path().display().to_string(),
            Some("main"),
            dest.path(),
        )
        .await
        .unwrap();

        // The local checkout gains a commit the remote doesn't have...
        std::fs::write(
            dest.path().join("local.container"),
            "[Container]\nImage=z\n",
        )
        .unwrap();
        assert!(git(dest.path(), &["add", "."]).status.success());
        assert!(
            git(dest.path(), &["commit", "--quiet", "-m", "local-only"])
                .status
                .success()
        );
        let local = rev_parse(dest.path(), "HEAD").await.unwrap();

        // ...while the remote also moves forward independently.
        std::fs::write(
            origin.path().join("web.container"),
            "[Container]\nImage=y\n",
        )
        .unwrap();
        assert!(
            git(origin.path(), &["commit", "--quiet", "-am", "update"])
                .status
                .success()
        );
        fetch(dest.path(), "main").await.unwrap();
        let remote = rev_parse(dest.path(), "origin/main").await.unwrap();

        assert!(!is_ancestor(dest.path(), &local, &remote).await.unwrap());
    }

    #[tokio::test]
    async fn fetch_of_a_bad_remote_fails_fast_without_a_credential_prompt() {
        let dest = tempfile::tempdir().unwrap();
        std::fs::remove_dir(dest.path()).unwrap();
        let err = clone("https://example.invalid/nope.git", None, dest.path())
            .await
            .unwrap_err();
        assert!(matches!(err, GitSyncError::Failed(_)));
    }
}
