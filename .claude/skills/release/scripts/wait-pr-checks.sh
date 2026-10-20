#!/usr/bin/env bash
# Wait for a PR's CI checks to register, then for all of them to finish.
#
# Usage: wait-pr-checks.sh <pr-number>
# Exit 0: every check passed. 1: a check failed. 2: no checks ever showed up.
set -u
pr=$1

# A just-opened PR (or just-pushed merge) has no checks for a few seconds;
# `gh pr checks --watch` errors out instead of waiting for them.
n=0
for _ in $(seq 1 36); do
  n=$(gh pr view "$pr" --json statusCheckRollup --jq '.statusCheckRollup | length' 2>/dev/null || echo 0)
  [ "$n" -gt 0 ] && break
  sleep 5
done
if [ "$n" -eq 0 ]; then
  echo "no checks registered on PR #$pr after 3 minutes"
  exit 2
fi

gh pr checks "$pr" --watch --fail-fast --interval 20
status=$?
echo "---"
gh pr checks "$pr"
exit "$status"
