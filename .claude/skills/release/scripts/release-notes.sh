#!/usr/bin/env bash
# Print the GitHub release notes for <version>: its CHANGELOG.md section
# (minus the `## [x.y.z]` heading) followed by a compare link.
#
# Usage: release-notes.sh <version> <previous-version> [changelog]
set -euo pipefail
ver=$1
prev=$2
changelog=${3:-CHANGELOG.md}

body=$(awk -v h="## [$ver]" 'index($0, h) == 1 { f = 1; next } /^## \[/ { f = 0 } f' "$changelog" \
  | sed -e '/./,$!d' | sed -e ':a' -e '/^\n*$/{$d;N;ba' -e '}')
if [ -z "$body" ]; then
  echo "no ## [$ver] section in $changelog" >&2
  exit 1
fi

printf '%s\n\n**Full diff:** https://github.com/RazMag/sooth/compare/v%s...v%s\n' "$body" "$prev" "$ver"
