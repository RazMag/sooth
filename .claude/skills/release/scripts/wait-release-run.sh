#!/usr/bin/env bash
# Wait for the release.yml run triggered by a push of <sha> to `release`,
# then for it to finish.
#
# Usage: wait-release-run.sh <merge-commit-sha>
# Exit 0: run succeeded. 1: run failed. 2: no run showed up.
set -u
sha=$1

id=
for _ in $(seq 1 36); do
  id=$(gh run list --workflow release.yml --branch release --limit 5 \
    --json databaseId,headSha --jq ".[] | select(.headSha == \"$sha\") | .databaseId" 2>/dev/null | head -1)
  [ -n "$id" ] && break
  sleep 5
done
if [ -z "$id" ]; then
  echo "no release.yml run for $sha after 3 minutes"
  exit 2
fi

echo "release run: $id"
gh run watch "$id" --exit-status --interval 20 >/dev/null
status=$?
gh run view "$id"
exit "$status"
