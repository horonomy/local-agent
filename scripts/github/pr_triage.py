#!/usr/bin/env python3
"""Validate a manual PR-triage snapshot from JSON on stdin; never writes GitHub."""

from __future__ import annotations

import argparse
from datetime import date
import json
import sys
from typing import Any

STATUS_LABELS = frozenset(
    {
        "triage:active",
        "triage:deferred",
        "triage:blocked",
        "triage:needs-assessment",
    }
)
PARKED_STATUSES = frozenset({"triage:deferred", "triage:blocked"})
REASON_LABELS = frozenset(
    {
        "reason:low-priority",
        "reason:out-of-scope",
        "reason:upstream",
        "reason:prerequisite",
        "reason:ci",
        "reason:authorization",
        "reason:compatibility",
        "reason:security-review",
    }
)
DECISION_MARKER = "<!-- local-agent-triage -->"
DECISION_HEADING = "## Triage Decision"
DECISION_OWNER = "chisanan232"
REQUIRED_DECISION_FIELDS = (
    "Status",
    "Reason",
    "Evidence",
    "Resume condition",
    "Next review date",
    "Related Jira / PR dependencies",
    "Next action",
    "Head",
)


def label_names(raw_labels: Any) -> list[str] | None:
    if not isinstance(raw_labels, list):
        return None
    names: list[str] = []
    for item in raw_labels:
        name = item.get("name") if isinstance(item, dict) else item
        if not isinstance(name, str):
            return None
        names.append(name)
    return names


def comment_author(comment: dict[str, Any]) -> str | None:
    author = comment.get("author", comment.get("user"))
    if isinstance(author, dict):
        login = author.get("login")
        return login.casefold() if isinstance(login, str) else None
    return author.casefold() if isinstance(author, str) else None


def parse_decision(body: str) -> tuple[dict[str, str] | None, str | None]:
    lines = body.splitlines()
    marker_lines = [
        index for index, line in enumerate(lines) if line.strip() == DECISION_MARKER
    ]
    if len(marker_lines) != 1:
        return None, "expected exactly one marked triage decision"
    try:
        heading = next(
            index
            for index, line in enumerate(lines)
            if index > marker_lines[0] and line.strip() == DECISION_HEADING
        )
    except StopIteration:
        return (
            None,
            f"marked triage comment must include {DECISION_HEADING} after its marker",
        )

    values: dict[str, str] = {}
    for line in lines[heading + 1 :]:
        content = line.strip()
        if content.startswith("## "):
            break
        if not content.startswith("- "):
            continue
        field, separator, value = content[2:].partition(":")
        field = field.strip()
        value = value.strip()
        if field not in REQUIRED_DECISION_FIELDS or not separator:
            continue
        if field in values:
            return None, f"duplicate triage decision field: {field}"
        values[field] = value

    missing = [field for field in REQUIRED_DECISION_FIELDS if field not in values]
    if missing:
        return None, f"missing triage decision field(s): {', '.join(missing)}"
    empty = [field for field in REQUIRED_DECISION_FIELDS if not values[field]]
    if empty:
        return None, f"empty triage decision field(s): {', '.join(empty)}"
    return values, None


def valid_head(value: str) -> bool:
    return len(value) == 40 and all(
        character in "0123456789abcdefABCDEF" for character in value
    )


def iso_date(value: str) -> date | None:
    try:
        parsed = date.fromisoformat(value)
    except ValueError:
        return None
    return parsed if parsed.isoformat() == value else None


def validate_prs(rows: Any, today: date) -> tuple[list[str], list[str]]:
    errors: list[str] = []
    warnings: list[str] = []
    if not isinstance(rows, list):
        return ["input must be a JSON array of PR objects"], warnings

    seen_numbers: set[int] = set()
    for index, pr in enumerate(rows):
        if not isinstance(pr, dict):
            errors.append(f"row {index + 1}: expected a PR object")
            continue
        number = pr.get("number")
        if not isinstance(number, int) or isinstance(number, bool) or number < 1:
            errors.append(f"row {index + 1}: number must be a positive integer")
            continue
        if number in seen_numbers:
            errors.append(f"PR #{number}: duplicate PR in input")
            continue
        seen_numbers.add(number)
        prefix = f"PR #{number}"

        raw_labels = label_names(pr.get("labels"))
        if raw_labels is None:
            errors.append(
                f"{prefix}: labels must be an array of names or objects with a name"
            )
            continue
        statuses = [name for name in raw_labels if name.startswith("triage:")]
        known_statuses = [name for name in statuses if name in STATUS_LABELS]
        if len(statuses) != 1 or len(known_statuses) != 1:
            errors.append(
                f"{prefix}: expected exactly one recognized triage status label"
            )
            continue
        status = known_statuses[0]
        reasons = {name for name in raw_labels if name.startswith("reason:")}
        if reasons - REASON_LABELS:
            errors.append(
                f"{prefix}: unrecognized reason label(s); use only the documented reason labels"
            )
        if status not in PARKED_STATUSES:
            if reasons:
                errors.append(
                    f"{prefix}: stale reason labels must be removed for {status}"
                )
            continue
        if not reasons:
            errors.append(
                f"{prefix}: {status} requires at least one valid reason label"
            )
            continue

        comments = pr.get("comments")
        if not isinstance(comments, list):
            errors.append(
                f"{prefix}: comments must be an array for parked PR validation"
            )
            continue
        marked_comments = [
            comment
            for comment in comments
            if isinstance(comment, dict)
            and isinstance(comment.get("body"), str)
            and DECISION_MARKER in comment["body"]
            and comment_author(comment) == DECISION_OWNER
        ]
        if len(marked_comments) != 1:
            errors.append(
                f"{prefix}: expected exactly one current triage decision comment by @{DECISION_OWNER}"
            )
            continue

        decision, problem = parse_decision(marked_comments[0]["body"])
        if problem:
            errors.append(f"{prefix}: {problem}")
            continue
        assert decision is not None
        if decision["Status"] != status:
            errors.append(
                f"{prefix}: decision status does not match the {status} label"
            )
        decision_reasons = {
            item.strip() for item in decision["Reason"].split(",") if item.strip()
        }
        if not decision_reasons or decision_reasons - REASON_LABELS:
            errors.append(
                f"{prefix}: decision contains invalid reason labels or is missing a reason"
            )
        elif decision_reasons != reasons:
            errors.append(
                f"{prefix}: decision reason labels do not match live reason labels"
            )

        head = pr.get("headRefOid")
        if not isinstance(head, str) or not valid_head(head):
            errors.append(
                f"{prefix}: current headRefOid must be a full 40-character SHA"
            )
        elif not valid_head(decision["Head"]):
            errors.append(f"{prefix}: decision head must be a full 40-character SHA")
        elif decision["Head"].casefold() != head.casefold():
            warnings.append(
                f"{prefix}: triage decision head is stale; refresh decision for current head {head}"
            )

        review_date = iso_date(decision["Next review date"])
        if review_date is None:
            errors.append(f"{prefix}: next review date must be an ISO YYYY-MM-DD date")
        elif review_date < today:
            warnings.append(
                f"{prefix}: triage decision is overdue for review (due {review_date.isoformat()})"
            )

    return errors, warnings


def parse_today(value: str) -> date:
    parsed = iso_date(value)
    if parsed is None:
        raise argparse.ArgumentTypeError("must be an ISO YYYY-MM-DD date")
    return parsed


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--today",
        type=parse_today,
        default=date.today(),
        help="override today's date for deterministic checks",
    )
    args = parser.parse_args(argv)
    try:
        rows = json.load(sys.stdin)
    except json.JSONDecodeError as exc:
        print(f"ERROR: invalid JSON on stdin: {exc}", file=sys.stderr)
        return 1

    errors, warnings = validate_prs(rows, args.today)
    for error in errors:
        print(f"ERROR: {error}", file=sys.stderr)
    for warning in warnings:
        print(f"WARNING: {warning}", file=sys.stderr)
    if errors:
        return 1
    if warnings:
        print(
            f"OK: validated {len(rows)} PR triage record(s) with review warnings; no GitHub state changed"
        )
    else:
        print(f"OK: validated {len(rows)} PR triage record(s); no GitHub state changed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
