#!/usr/bin/env bash

set -euo pipefail

BASE_SHA="${BASE_SHA:-}"
DOCS_FILES_RAW="${DOCS_FILES:-}"

if [ -z "$BASE_SHA" ] && git rev-parse --verify origin/master >/dev/null 2>&1; then
    BASE_SHA="$(git merge-base origin/master HEAD)"
fi

if [ -z "$DOCS_FILES_RAW" ] && [ -n "$BASE_SHA" ] && git cat-file -e "$BASE_SHA^{commit}" 2>/dev/null; then
    DOCS_FILES_RAW="$(git diff --name-only "$BASE_SHA" HEAD | awk '
        /^docs\/book\/src\/.*\.md$/ || /^docs\/book\/src\/.*\.mdx$/ {
            print
        }
    ')"
fi

if [ -z "$DOCS_FILES_RAW" ]; then
    echo "No docs files detected; skipping docs quality gate."
    exit 0
fi

if [ -z "$BASE_SHA" ] || ! git cat-file -e "$BASE_SHA^{commit}" 2>/dev/null; then
    echo "BASE_SHA is missing or invalid; checking selected authored files directly."
    BASE_SHA=""
fi

ALL_FILES=()
while IFS= read -r file; do
    if [ -n "$file" ]; then
        ALL_FILES+=("$file")
    fi
done < <(printf '%s\n' "$DOCS_FILES_RAW")

if [ "${#ALL_FILES[@]}" -eq 0 ]; then
    echo "No docs files detected after normalization; skipping docs quality gate."
    exit 0
fi

EXISTING_FILES=()
for file in "${ALL_FILES[@]}"; do
    if [ -f "$file" ]; then
        EXISTING_FILES+=("$file")
    fi
done

if [ "${#EXISTING_FILES[@]}" -eq 0 ]; then
    echo "No existing docs files to check; skipping docs quality gate."
    exit 0
fi

# Em-dash gate: no prose em-dashes (U+2014) in changed docs. Em-dashes inside
# fenced code blocks and inline `code` spans (including spans that wrap across
# lines) are allowed, since those quote literal source, CLI output, or table
# rules. Everything else uses a comma, colon, semicolon, or period instead.
python3 - "${EXISTING_FILES[@]}" <<'PY'
import re
import sys

EM = "\u2014"
files = sys.argv[1:]
violations = []

for path in files:
    try:
        with open(path, encoding="utf-8") as fh:
            lines = fh.readlines()
    except (FileNotFoundError, UnicodeDecodeError):
        continue

    in_fence = False
    span_width = 0
    for n, line in enumerate(lines, 1):
        stripped = line.lstrip()
        if stripped.startswith("```") or stripped.startswith("~~~"):
            in_fence = not in_fence
            span_width = 0
            continue
        if in_fence:
            continue
        if EM not in line and "`" not in line:
            continue
        for token in re.findall(r"`+|[^`]+", line):
            if token.startswith("`"):
                if span_width == len(token):
                    span_width = 0
                elif span_width == 0:
                    span_width = len(token)
            elif EM in token and span_width == 0:
                violations.append((path, n, line.rstrip()))
                break

if violations:
    print("Em-dash (\u2014) found in prose. Use a comma, colon, semicolon, or")
    print("period instead. Em-dashes are only allowed inside code spans/blocks.")
    print()
    for path, n, text in violations:
        print(f"  {path}:{n}: {text}")
    print()
    print(f"Blocking prose em-dashes: {len(violations)}")
    sys.exit(1)

print("No prose em-dashes in changed docs files.")
PY

# The third-party Markdown formatter was retired by the project owner because
# its dependency tree has unresolved security vulnerabilities. Do not execute
# it here. Link integrity and mdBook builds remain separate required checks.
