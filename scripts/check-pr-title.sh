#!/usr/bin/env bash
# Validate scoped Conventional Commits or Local Agent ticketed Gitmoji PR titles.
# Usage: check-pr-title.sh "<title>"
# Exits 0 on accept, 1 on reject.

set -u

title="${1-}"

case "$title" in
  *$'\n'*|*$'\r'*)
    echo "::error::PR title must be a single line"
    exit 1
    ;;
esac

pattern='^(build|chore|ci|docs|feat|fix|perf|refactor|revert|style|test)\([a-z0-9._/:-]+\)!?: .+'
local_pattern='^\[HORO-[0-9]+\] (✨|🐛|♻️|✅|📝|🔧|🔌|🪝|👨‍💻|🧭|⬆️|🗑️|🚨|🛡️) [a-z0-9._/:-]+: [^[:space:]].*'

if printf '%s' "$title" | grep -qE "$pattern|$local_pattern"; then
  exit 0
fi

echo "::error::PR title requires scoped Conventional Commits or '[HORO-number] <gitmoji> scope: summary'"
echo "Allowed types: build, chore, ci, docs, feat, fix, perf, refactor, revert, style, test"
echo "Got: $title"
exit 1
