#!/usr/bin/env python3
"""Focused tests for the manual, report-only PR triage validator."""

from __future__ import annotations

from datetime import date
import io
import json
from pathlib import Path
import unittest
from unittest.mock import patch

try:
    from scripts.github import pr_triage as triage
except ModuleNotFoundError:
    import pr_triage as triage


HEAD = "a" * 40
TODAY = date(2026, 11, 6)
REPO_ROOT = Path(__file__).resolve().parents[2]


def decision(
    status: str = "triage:deferred",
    reasons: tuple[str, ...] = ("reason:low-priority",),
    **overrides: object,
) -> str:
    fields = {
        "Status": status,
        "Reason": ", ".join(reasons),
        "Evidence": "No urgent user impact reported.",
        "Resume condition": "A maintainer has capacity.",
        "Next review date": "2026-11-20",
        "Related Jira / PR dependencies": "None.",
        "Next action": "Reassess queue priority.",
        "Head": HEAD,
    }
    fields.update(overrides)
    return "<!-- local-agent-triage -->\n## Triage Decision\n" + "\n".join(
        f"- {name}: {value}" for name, value in fields.items()
    )


def pr(
    status: str = "triage:active",
    reasons: tuple[str, ...] = (),
    comments: list[dict[str, object]] | None = None,
    **overrides: object,
) -> dict[str, object]:
    value: dict[str, object] = {
        "number": 17,
        "headRefOid": HEAD,
        "labels": [
            {"name": status},
            *({"name": reason} for reason in reasons),
            {"name": "size:S"},
            {"name": "risk:low"},
            {"name": "provider:openai"},
        ],
        "comments": comments or [],
    }
    value.update(overrides)
    return value


def owner_comment(body: str) -> dict[str, object]:
    return {"body": body, "author": {"login": "Chisanan232"}}


def parked_pr(
    status: str = "triage:deferred",
    reasons: tuple[str, ...] = ("reason:low-priority",),
    **overrides: object,
) -> dict[str, object]:
    comments = overrides.pop("comments", [owner_comment(decision(status, reasons))])
    return pr(status, reasons, comments, **overrides)


class PrTriageTest(unittest.TestCase):
    def test_all_four_statuses_are_explicit_and_distinct(self) -> None:
        expected = {
            "triage:active",
            "triage:deferred",
            "triage:blocked",
            "triage:needs-assessment",
        }
        self.assertEqual(triage.STATUS_LABELS, expected)
        self.assertEqual(len(triage.STATUS_LABELS), 4)

    def test_reason_enum_matches_documented_guidance(self) -> None:
        expected = {
            "reason:low-priority",
            "reason:out-of-scope",
            "reason:upstream",
            "reason:prerequisite",
            "reason:ci",
            "reason:authorization",
            "reason:compatibility",
            "reason:security-review",
        }
        self.assertEqual(triage.REASON_LABELS, expected)
        guidance = (REPO_ROOT / "docs/local-agent/engineering-lifecycle.md").read_text()
        for value in expected | triage.STATUS_LABELS:
            self.assertIn(f"`{value}`", guidance)

    def test_accepts_one_status_and_preserves_unrelated_labels(self) -> None:
        errors, warnings = triage.validate_prs([pr()], TODAY)
        self.assertEqual(errors, [])
        self.assertEqual(warnings, [])

    def test_rejects_missing_multiple_and_unknown_status_labels(self) -> None:
        cases = [
            pr(labels=[{"name": "size:S"}], number=17),
            pr(
                labels=[{"name": "triage:active"}, {"name": "triage:blocked"}],
                number=18,
            ),
            pr(labels=[{"name": "triage:unknown"}], number=19),
        ]
        errors, _ = triage.validate_prs(cases, TODAY)
        self.assertEqual(len(errors), 3)
        self.assertTrue(
            all(
                "status" in error.lower() or "triage status" in error.lower()
                for error in errors
            )
        )
        self.assertFalse(any("duplicate" in error.lower() for error in errors))

    def test_requires_one_marked_comment_and_all_decision_fields(self) -> None:
        for status, reason in (
            ("triage:deferred", "reason:low-priority"),
            ("triage:blocked", "reason:ci"),
        ):
            reasons = (reason,)
            record = parked_pr(status, reasons)
            errors, warnings = triage.validate_prs([record], TODAY)
            self.assertEqual(errors, [])
            self.assertEqual(warnings, [])

            body = decision(status, reasons)
            for field in triage.REQUIRED_DECISION_FIELDS:
                incomplete = "\n".join(
                    line
                    for line in body.splitlines()
                    if not line.startswith(f"- {field}:")
                )
                record = parked_pr(
                    status, reasons, comments=[owner_comment(incomplete)]
                )
                errors, _ = triage.validate_prs([record], TODAY)
                self.assertTrue(
                    any(field.lower() in error.lower() for error in errors), field
                )

        errors, _ = triage.validate_prs([pr("triage:blocked", ("reason:ci",))], TODAY)
        self.assertTrue(any("comment" in error.lower() for error in errors))

    def test_rejects_invalid_reason_status_disagreement_and_reason_label_drift(
        self,
    ) -> None:
        invalid_reason = parked_pr(reasons=("reason:low-priority",))
        invalid_reason["comments"] = [
            owner_comment(decision(reasons=("reason:no-such-reason",)))
        ]
        wrong_status = parked_pr(reasons=("reason:low-priority",))
        wrong_status["number"] = 18
        wrong_status["comments"] = [
            owner_comment(decision("triage:blocked", ("reason:low-priority",)))
        ]
        mismatched_labels = parked_pr(reasons=("reason:ci",))
        mismatched_labels["number"] = 19
        errors, _ = triage.validate_prs(
            [invalid_reason, wrong_status, mismatched_labels], TODAY
        )
        self.assertTrue(
            any(
                "reason labels" in error.lower() and "invalid" in error.lower()
                for error in errors
            )
        )
        self.assertTrue(any("does not match" in error.lower() for error in errors))
        self.assertTrue(any("reason labels" in error.lower() for error in errors))

    def test_requires_exactly_one_current_decision_marker_but_keeps_history(
        self,
    ) -> None:
        errors, _ = triage.validate_prs(
            [parked_pr("triage:blocked", ("reason:ci",), comments=[])], TODAY
        )
        self.assertTrue(any("exactly one" in error.lower() for error in errors))
        duplicate = parked_pr("triage:blocked", ("reason:ci",))
        duplicate["comments"].append(
            owner_comment(decision("triage:blocked", ("reason:ci",)))
        )
        errors, _ = triage.validate_prs([duplicate], TODAY)
        self.assertTrue(any("exactly one" in error.lower() for error in errors))

        resumed = pr("triage:active", comments=[{"body": decision()}])
        errors, _ = triage.validate_prs([resumed], TODAY)
        self.assertEqual(errors, [])

    def test_nonparked_status_cannot_keep_stale_reason_labels(self) -> None:
        for status in ("triage:active", "triage:needs-assessment"):
            errors, _ = triage.validate_prs([pr(status, ("reason:ci",))], TODAY)
            self.assertTrue(any("stale reason" in error.lower() for error in errors))

    def test_requires_exact_current_head_sha(self) -> None:
        stale = parked_pr("triage:deferred")
        stale["comments"] = [owner_comment(decision(**{"Head": "b" * 40}))]
        errors, warnings = triage.validate_prs([stale], TODAY)
        self.assertEqual(errors, [])
        self.assertTrue(any("head is stale" in warning.lower() for warning in warnings))

    def test_rejects_invalid_review_date_and_warns_when_overdue(self) -> None:
        invalid = parked_pr()
        invalid["comments"] = [
            owner_comment(decision(**{"Next review date": "2026-02-30"}))
        ]
        errors, warnings = triage.validate_prs([invalid], TODAY)
        self.assertTrue(any("date" in error.lower() for error in errors))
        self.assertEqual(warnings, [])

        overdue = parked_pr()
        overdue["comments"] = [
            owner_comment(decision(**{"Next review date": "2026-11-05"}))
        ]
        errors, warnings = triage.validate_prs([overdue], TODAY)
        self.assertEqual(errors, [])
        self.assertTrue(any("overdue" in warning.lower() for warning in warnings))

        stale_head = parked_pr()
        stale_head["headRefOid"] = "b" * 40
        errors, warnings = triage.validate_prs([stale_head], TODAY)
        self.assertEqual(errors, [])
        self.assertTrue(any("head is stale" in warning.lower() for warning in warnings))

    def test_cli_reports_issues_with_nonzero_status_and_never_writes(self) -> None:
        with patch(
            "sys.stdin", io.StringIO(json.dumps([pr("triage:blocked", ("reason:ci",))]))
        ):
            self.assertEqual(triage.main(["--today", "2026-11-06"]), 1)

    def test_cli_accepts_clean_snapshot(self) -> None:
        with patch("sys.stdin", io.StringIO(json.dumps([pr()]))):
            self.assertEqual(triage.main(["--today", "2026-11-06"]), 0)

    def test_overdue_snapshot_exits_zero_with_manual_warning(self) -> None:
        overdue = parked_pr()
        overdue["comments"] = [
            owner_comment(decision(**{"Next review date": "2026-11-05"}))
        ]
        with patch("sys.stdin", io.StringIO(json.dumps([overdue]))):
            self.assertEqual(triage.main(["--today", "2026-11-06"]), 0)

    def test_rejects_non_list_json_input(self) -> None:
        with patch("sys.stdin", io.StringIO(json.dumps({"number": 17}))):
            self.assertEqual(triage.main(["--today", "2026-11-06"]), 1)


if __name__ == "__main__":
    unittest.main()
