---
name: release
description: Cut a sooth release end to end - bump the version and changelog, merge the bump PR, merge main into `release`, wait for CI and the release workflow, then write the GitHub release notes. Use when the user says "create a release", "cut a release", "ship a release", "release vX.Y.Z" or runs /release. Pauses for the user on any failure.
argument-hint: "[patch|minor|major|X.Y.Z]"
---

# Release sooth

The user asked for a whole release, start to finish. That request is your
authorization to commit, push, open PRs, merge both PRs, and edit the
release notes, **for this release only**. Don't stop to confirm the steps
below. Do stop when a step fails (see **Pausing**).

Arguments: `$ARGUMENTS`. This is a bump level (`patch`/`minor`/`major`) or an
exact version, and it overrides your own choice. Empty means you pick.

Tell the user what's happening in one line per step ("bump PR #65 open,
waiting for CI"). Don't send a play-by-play.

## Resuming

The process can stop partway, because of a pause or a new chat. Before
step 1, find where things stand and pick up from there. `PREV` is the
latest published release (`gh release view --json tagName --jq .tagName`).
Don't use `git describe`: the tags aren't all reachable from `main`.

| State | Resume at |
|---|---|
| `Cargo.toml` version == `PREV` and no open `chore/release-v*` PR | 1 |
| An open `chore/release-vX` PR | 3, for that PR |
| `main`'s `Cargo.toml` is ahead of `PREV`, and no `vX` release or open `main`→`release` PR | 5 |
| An open `main`→`release` PR | 6, for that PR |
| `release` already has the merge but no `vX` release | 7 |
| A `vX` release exists with an empty body | 8 |

## Steps

### 1. Preflight

```bash
git checkout main && git pull --ff-only && git fetch --tags -q
git status --porcelain --untracked-files=no   # must be empty
git log --oneline vPREV..main --no-merges
gh run list --workflow ci.yml --branch main --limit 1 --json headSha,status,conclusion
```

- If tracked files have changes, **pause**. Never stash or discard them.
- If no commits since `PREV` (or only `chore(release)` ones), stop and say
  there's nothing to release. That isn't a failure.
- `main`'s latest CI run must be for `HEAD` and `success`. If it's still
  running, wait with `gh run watch <id> --exit-status` (as a background
  Bash call). If it failed, **pause**.
- Note any open PRs into `main` (`gh pr list --base main`). They aren't
  part of this release. Mention them in the final summary, but don't wait
  for them.

### 2. Bump PR

**Version.** Use `$ARGUMENTS` if given. Otherwise pick a version and say it
in one line with the reason, then carry on:
- Only fixes, or one modest feature: **patch**.
- A sizeable feature, or several features: **minor**.
- Never **major** unless the user asks.

**Changelog.** Look at `CHANGELOG.md`'s `## [Unreleased]` section.
- If it has entries, keep them as they are.
- If it's empty, write entries from the commits and merged PRs since
  `PREV`. Use `gh pr view <n>` for detail.
- Match the existing style:
  - `### Added` / `### Changed` / `### Fixed` subsections.
  - Each entry starts with a bold sentence, followed by plain user-facing
    prose about what changed and why it matters.
  - No commit hashes or internal module names.
  - `docs:`, `ci:`, `chore:`, `style:`, `test:` and `refactor:` commits
    don't get entries unless users would notice them.

Then:

```bash
git checkout -b chore/release-vX
sed -i '0,/^version = "PREV"/s//version = "X"/' Cargo.toml
cargo check -q                      # refreshes Cargo.lock
```

- Rename `## [Unreleased]` to `## [X] - <today, YYYY-MM-DD>`, and put a
  fresh empty `## [Unreleased]` above it.
- In the link list at the bottom, point `[Unreleased]` at
  `compare/vX...HEAD`, and add
  `[X]: https://github.com/RazMag/sooth/compare/vPREV...vX` under it.
- Run `just ci`. If it fails, **pause**.
- Commit `CHANGELOG.md`, `Cargo.toml` and `Cargo.lock` only, as
  `chore(release): bump version to X`. End the message with the
  attribution lines this session gives you.
- `git push -u origin chore/release-vX`.
- Open the PR into `main`, titled
  `chore(release): bump version to X`. The body should:
  - say what moved in the changelog,
  - list the release's changes, one bullet each with its PR number,
  - say that `just ci` passes,
  - end with the session's PR attribution line.

### 3. Wait for the bump PR's CI

Run this as a background Bash call (`run_in_background: true`). It re-invokes
you when it exits, so don't poll on top of it:

```bash
.claude/skills/release/scripts/wait-pr-checks.sh <N>
```

Exit 0 means continue. Any other exit means **pause**.

### 4. Merge the bump PR

```bash
gh pr merge <N> --merge --delete-branch
git checkout main && git pull --ff-only
grep -m1 '^version' Cargo.toml       # must say X
git branch -D chore/release-vX 2>/dev/null || true
```

Always merge with `--merge`, because the repo's history uses merge commits.
If the merge is refused (conflict, not mergeable), **pause**.

### 5. Release PR (`main` → `release`)

```bash
gh pr create --base release --head main --title "release: vX" --body-file <file>
```

The body follows the earlier release PRs (for example #64):
- A `## Summary` with these bullets:
  - "Publishes vX, a <patch|minor> release." followed by one sub-bullet
    per change, tagged **Added:**/**Changed:**/**Fixed:**, each with its
    PR number.
  - A bullet saying a push to `release` triggers `release.yml`, which
    reads the version from `Cargo.toml` (bumped in #<bump PR>).
- A `## Test plan` with two unchecked boxes:
  - CI green on `main` (`<short sha>`).
  - `release.yml` completes test -> version -> build -> publish.
- The PR attribution line at the end.

### 6. Wait for its CI, then merge

Run the same background `wait-pr-checks.sh <M>`. If it fails, **pause**.
Then merge:

```bash
gh pr merge <M> --merge
gh pr view <M> --json mergeCommit --jq .mergeCommit.oid
```

**Never pass `--delete-branch` here.** The head branch is `main`.

### 7. Wait for the release workflow

Run this in the background:

```bash
.claude/skills/release/scripts/wait-release-run.sh <merge sha>
```

If it exits non-zero, **pause**. Use `gh run view <id> --log-failed | tail -60`
for the excerpt.

Then check the release itself:

```bash
gh release view vX --json tagName,isDraft,assets --jq '{tag:.tagName,draft:.isDraft,assets:[.assets[].name]}'
```

It must not be a draft, and it needs exactly these assets:
`SHA256SUMS`, `sooth-x86_64-unknown-linux-gnu` and
`sooth-aarch64-unknown-linux-gnu`.

A green run with no `vX` release means `version` found `already_released`
and skipped the build. That's unexpected here: **pause**.

### 8. Release notes

```bash
.claude/skills/release/scripts/release-notes.sh X PREV > <scratchpad>/notes-vX.md
gh release edit vX --notes-file <scratchpad>/notes-vX.md
gh release view vX --json body --jq .body | head -5
```

### 9. Wrap up

Give a short summary:
- the release URL, the version and the bump reason,
- both PR links,
- the assets,
- any open PRs into `main` that didn't make it in.

If a `PushNotification` tool is available, send one saying vX is out.

## Pausing

On any failure, stop right there:
- `just ci` fails, or a PR check fails or never shows up
- a merge is refused
- the release run fails, or the release comes out wrong
- something else is unexpected, like a dirty tree or a version mismatch

Don't fix it, re-run it, or work around it yourself. Then:

1. Tell the user:
   - which step failed (by number),
   - the failing check or job and its URL,
   - a short excerpt of the log (`gh run view <id> --log-failed | tail -40`),
   - what has already happened (PRs open or merged, what's been pushed),
   - the choices: fix it and continue, re-run the failed jobs
     (`gh run rerun <id> --failed`), or abort.
2. If a `PushNotification` tool is available, send one:
   "sooth release vX paused at step N: <what failed>".
3. End the turn and wait. When the user says to continue, re-check state
   with **Resuming** before acting.

## Never

- Push to `release` directly. Only open a PR from `main` into it.
- Force-push.
- Merge with `--admin` or any other bypass.
- Delete or re-create a tag or a release.
- Edit `.github/workflows/*` as part of a release.
- Fix a failing test or lint inside the release PR. That needs its own
  fix PR, which the user decides on.
