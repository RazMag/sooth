//! Thin async wrappers around the `git` CLI -- the app's second shell-out,
//! after `journal.rs`'s `journalctl`. Every invocation disables the
//! interactive credential prompt (`GIT_TERMINAL_PROMPT=0`) so a bad remote or
//! missing credentials fails fast instead of hanging a poll loop forever, and
//! is bounded by [`TIMEOUT`] for the same reason. Auth is otherwise left
//! entirely to the host: whatever SSH agent, `~/.ssh/config`, or credential
//! helper already works for this user's own `git` is what sooth gets too,
//! since it runs the exact same binary as the exact same user -- [`GitAuth`]
//! is the one exception, a configured GitHub token injected only for
//! `https://github.com/...` remotes.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use super::GitSyncError;

const TIMEOUT: Duration = Duration::from_secs(30);

/// Bridges a configured GitHub access token (`Config.github_token`, set from
/// the Settings page) into `git`'s credential machinery for
/// `https://github.com/...` remotes -- the way private git-synced repos
/// authenticate. Deliberately *not* done by embedding the token in the
/// remote URL or via a `-c http.extraHeader=...` flag: both would put the
/// token in this process's argv, readable by anyone who can list
/// `/proc/<pid>/cmdline` for this user, and a URL-embedded token would also
/// get written into the checkout's `.git/config` on disk. Instead this sets
/// `GIT_ASKPASS` to a tiny wrapper script (written once by [`GitAuth::setup`])
/// that re-invokes this same `sooth` binary as `sooth --git-askpass
/// <prompt>` (see `main::git_askpass_cli`); the token itself travels only as
/// the `SOOTH_GIT_ASKPASS_TOKEN` environment variable of the `git` child
/// process (which `git` then passes on to the askpass child it spawns) --
/// never on a command line, never persisted to the checkout.
#[derive(Debug, Clone)]
pub struct GitAuth {
    askpass_path: PathBuf,
    token: String,
}

impl GitAuth {
    /// Writes the askpass wrapper script into `state_dir` (created if
    /// missing) and returns a `GitAuth` that [`clone`]/[`fetch`]/
    /// [`fetch_ref`] will use for a GitHub HTTPS remote, or `None` if
    /// `token` is blank -- the "no token configured" case, where those
    /// remotes behave exactly as before this existed.
    pub fn setup(exe_path: &Path, state_dir: &Path, token: &str) -> std::io::Result<Option<Self>> {
        let token = token.trim();
        if token.is_empty() {
            return Ok(None);
        }
        std::fs::create_dir_all(state_dir)?;
        let askpass_path = state_dir.join(".sooth-git-askpass.sh");
        let script = format!(
            "#!/bin/sh\nexec {} --git-askpass \"$1\"\n",
            shell_quote(&exe_path.display().to_string())
        );
        std::fs::write(&askpass_path, script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&askpass_path, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Some(Self {
            askpass_path,
            token: token.to_string(),
        }))
    }
}

/// Single-quotes `s` for embedding in the generated `sh` script, closing and
/// reopening the quote around any literal `'` in `s` (a path is the only
/// thing ever passed here, but this is correct for arbitrary content).
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Whether `remote` is an `https://github.com/...` URL -- the only case
/// [`GitAuth`] applies to, so a configured token is never sent to some other
/// host a sync happens to point at.
fn is_github_https(remote: &str) -> bool {
    let Some(rest) = remote.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = host.split(':').next().unwrap_or(host);
    host.eq_ignore_ascii_case("github.com")
}

fn apply_auth(cmd: &mut Command, remote: &str, auth: Option<&GitAuth>) {
    if let Some(auth) = auth.filter(|_| is_github_https(remote)) {
        cmd.env("GIT_ASKPASS", &auth.askpass_path)
            .env("SOOTH_GIT_ASKPASS_TOKEN", &auth.token);
    }
}

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
pub async fn clone(
    remote: &str,
    branch: Option<&str>,
    dest: &Path,
    auth: Option<&GitAuth>,
) -> Result<(), GitSyncError> {
    let mut cmd = base_command();
    apply_auth(&mut cmd, remote, auth);
    cmd.arg("clone").arg("--single-branch");
    if let Some(branch) = branch {
        cmd.arg("--branch").arg(branch);
    }
    cmd.arg(remote).arg(dest);
    run(cmd).await?;
    Ok(())
}

/// `git -C <dir> fetch --quiet origin <branch>`. `remote` is `origin`'s
/// configured URL -- not passed on the command line (the fetch targets the
/// already-configured `origin`), only used to decide whether `auth` applies.
pub async fn fetch(
    dir: &Path,
    branch: &str,
    remote: &str,
    auth: Option<&GitAuth>,
) -> Result<(), GitSyncError> {
    let mut cmd = base_command();
    apply_auth(&mut cmd, remote, auth);
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
/// `remote` is the same "which URL is this really talking to" hint as
/// `fetch`'s.
pub async fn fetch_ref(
    dir: &Path,
    branch: &str,
    remote: &str,
    auth: Option<&GitAuth>,
) -> Result<(), GitSyncError> {
    let mut cmd = base_command();
    apply_auth(&mut cmd, remote, auth);
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
            None,
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
            None,
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

        fetch(
            dest.path(),
            "main",
            &origin.path().display().to_string(),
            None,
        )
        .await
        .unwrap();
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
            None,
        )
        .await
        .unwrap();
        assert_eq!(current_branch(dest.path()).await.unwrap(), "main");

        // Switching the tracked branch (the `GitSyncManager::edit` path):
        // fetch the new branch's ref explicitly, since `--single-branch`
        // never brought it down, then check it out.
        fetch_ref(
            dest.path(),
            "staging",
            &origin.path().display().to_string(),
            None,
        )
        .await
        .unwrap();
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
            None,
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
            None,
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
        fetch(
            dest.path(),
            "main",
            &origin.path().display().to_string(),
            None,
        )
        .await
        .unwrap();
        let remote = rev_parse(dest.path(), "origin/main").await.unwrap();

        assert!(!is_ancestor(dest.path(), &local, &remote).await.unwrap());
    }

    #[tokio::test]
    async fn fetch_of_a_bad_remote_fails_fast_without_a_credential_prompt() {
        let dest = tempfile::tempdir().unwrap();
        std::fs::remove_dir(dest.path()).unwrap();
        let err = clone("https://example.invalid/nope.git", None, dest.path(), None)
            .await
            .unwrap_err();
        assert!(matches!(err, GitSyncError::Failed(_)));
    }

    #[test]
    fn is_github_https_matches_only_github_over_https() {
        assert!(is_github_https("https://github.com/owner/repo.git"));
        assert!(is_github_https("https://github.com/owner/repo"));
        assert!(is_github_https("https://GitHub.com/owner/repo.git"));
        assert!(is_github_https(
            "https://x-access-token@github.com/owner/repo.git"
        ));
        assert!(is_github_https("https://github.com:443/owner/repo.git"));
        assert!(!is_github_https("https://gitlab.com/owner/repo.git"));
        assert!(!is_github_https("git@github.com:owner/repo.git"));
        assert!(!is_github_https("ssh://git@github.com/owner/repo.git"));
        assert!(!is_github_https(
            "https://notgithub.com.evil.example/owner/repo.git"
        ));
    }

    #[test]
    fn git_auth_setup_is_none_for_a_blank_token() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            GitAuth::setup(Path::new("/usr/bin/sooth"), dir.path(), "   ")
                .unwrap()
                .is_none()
        );
        assert!(!dir.path().join(".sooth-git-askpass.sh").exists());
    }

    #[test]
    fn git_auth_setup_writes_an_executable_askpass_script() {
        let dir = tempfile::tempdir().unwrap();
        let auth = GitAuth::setup(Path::new("/usr/bin/sooth"), dir.path(), "ghp_example")
            .unwrap()
            .unwrap();

        let contents = std::fs::read_to_string(&auth.askpass_path).unwrap();
        assert!(contents.contains("--git-askpass"));
        assert!(contents.contains("/usr/bin/sooth"));
        assert!(
            !contents.contains("ghp_example"),
            "the token must never be written to disk"
        );

        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&auth.askpass_path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
