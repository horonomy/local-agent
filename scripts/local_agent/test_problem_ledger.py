from __future__ import annotations

import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from scripts.local_agent import problem_ledger as ledger


class ProblemLedgerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.addCleanup(self.tempdir.cleanup)
        self.path = Path(self.tempdir.name) / "ledger.json"

    def test_new_problem_records_identity_hypotheses_and_disposition(self) -> None:
        record = ledger.create_problem(
            self.path,
            problem_id="issue-1",
            ticket="HORO-2420",
            observation="reproducible symptom",
            hypotheses=["hypothesis one"],
            release_impact="blocks release",
            disposition="A",
        )
        self.assertEqual(record["problem_id"], "issue-1")
        self.assertEqual(record["first_observation"], "reproducible symptom")
        self.assertEqual(record["disposition"], "A")
        self.assertEqual(record["events"][0]["type"], "observed")
        self.assertEqual(ledger.load(self.path)["problems"]["issue-1"], record)

    def test_same_problem_appends_result_without_resetting_history_or_budget(
        self,
    ) -> None:
        ledger.create_problem(
            self.path,
            problem_id="p",
            ticket="T-1",
            observation="seen",
            hypotheses=[],
            release_impact="none",
            disposition="B",
        )
        ledger.record_result(
            self.path, "p", "failed", failed_cycle=True, active_seconds=13
        )
        ledger.record_result(self.path, "p", "recovered", recovered=True)
        record = ledger.load(self.path)["problems"]["p"]
        self.assertEqual(record["failed_cycles"], 1)
        self.assertEqual(record["active_seconds"], 13)
        self.assertEqual(
            [event["type"] for event in record["events"]],
            ["observed", "result", "result"],
        )
        self.assertTrue(record["recovered"])

    def test_passive_availability_and_active_diagnosis_have_distinct_accounting(
        self,
    ) -> None:
        ledger.create_problem(
            self.path,
            problem_id="p",
            ticket="T-1",
            observation="seen",
            hypotheses=[],
            release_impact="none",
            disposition="A",
        )
        ledger.record_probe(self.path, "p", "diagnostic check", availability=True)
        record = ledger.load(self.path)["problems"]["p"]
        self.assertEqual(record["failed_cycles"], 0)
        self.assertEqual(record["active_seconds"], 0)
        self.assertNotIn("failed_cycle", record["events"][-1])
        ledger.record_probe(self.path, "p", "active investigation", active_seconds=3601)
        record = ledger.load(self.path)["problems"]["p"]
        self.assertEqual(record["failed_cycles"], 0)
        self.assertEqual(record["active_seconds"], 3601)
        self.assertTrue(record["circuit_open"])
        self.assertFalse(ledger.check(self.path, "p", dependent=True))

    def test_dependent_work_blocks_when_circuit_exhausted_but_independent_is_allowed(
        self,
    ) -> None:
        ledger.create_problem(
            self.path,
            problem_id="p",
            ticket="T-1",
            observation="seen",
            hypotheses=[],
            release_impact="none",
            disposition="B",
        )
        for index in range(5):
            ledger.record_result(self.path, "p", f"failed {index}", failed_cycle=True)
        before = ledger.load(self.path)
        with self.assertRaises(ledger.LedgerError):
            ledger.record_result(self.path, "p", "sixth", failed_cycle=True)
        self.assertEqual(ledger.load(self.path), before)
        self.assertFalse(ledger.check(self.path, "p", dependent=True))
        self.assertTrue(ledger.check(self.path, "p", dependent=False))

    def test_active_time_budget_and_explicit_duration_are_enforced(self) -> None:
        ledger.create_problem(
            self.path,
            problem_id="p",
            ticket="T-1",
            observation="seen",
            hypotheses=[],
            release_impact="none",
            disposition="B",
        )
        ledger.record_result(self.path, "p", "attempt", active_seconds=3590)
        ledger.record_result(self.path, "p", "attempt", active_seconds=10)
        self.assertFalse(ledger.check(self.path, "p", dependent=True))
        self.assertEqual(
            ledger.load(self.path)["problems"]["p"]["active_seconds"], 3600
        )
        with self.assertRaises(ledger.LedgerError):
            ledger.record_result(self.path, "p", "over budget", active_seconds=1)
        with self.assertRaises(ValueError):
            ledger.record_result(
                self.path, "p", "bad", failed_cycle=False, active_seconds=-1
            )

    def test_elapsed_overrun_is_preserved_and_opens_circuit(self) -> None:
        ledger.create_problem(
            self.path,
            problem_id="p",
            ticket="T-1",
            observation="seen",
            hypotheses=[],
            release_impact="none",
            disposition="A",
        )
        ledger.record_result(self.path, "p", "3599", active_seconds=3599)
        ledger.record_result(self.path, "p", "over", active_seconds=2)
        record = ledger.load(self.path)["problems"]["p"]
        self.assertEqual(record["active_seconds"], 3601)
        self.assertEqual(record["events"][-1]["active_seconds"], 2)
        self.assertTrue(record["circuit_open"])
        self.assertFalse(ledger.check(self.path, "p", dependent=True))
        before = ledger.load(self.path)
        with self.assertRaises(ledger.LedgerError):
            ledger.record_result(self.path, "p", "another", active_seconds=1)
        self.assertEqual(ledger.load(self.path), before)

    def test_failed_result_rejected_without_partial_history_at_cycle_limit(
        self,
    ) -> None:
        ledger.create_problem(
            self.path,
            problem_id="p",
            ticket="T-1",
            observation="seen",
            hypotheses=[],
            release_impact="none",
            disposition="A",
        )
        for index in range(5):
            ledger.record_result(self.path, "p", str(index), failed_cycle=True)
        before = ledger.load(self.path)
        with self.assertRaises(ledger.LedgerError):
            ledger.record_result(self.path, "p", "sixth", failed_cycle=True)
        self.assertEqual(ledger.load(self.path), before)

    def test_corrupt_wrong_version_negative_or_unknown_budget_fails_closed(
        self,
    ) -> None:
        for raw in (b"{", b'{"version": 9, "problems": {}}'):
            self.path.write_bytes(raw)
            with self.assertRaises(ledger.LedgerError):
                ledger.check(self.path, "p", dependent=True)
        self.path.write_text(
            json.dumps(
                {
                    "version": 1,
                    "problems": {
                        "p": {
                            "failed_cycles": -1,
                            "active_seconds": 0,
                            "events": [],
                            "disposition": "A",
                        }
                    },
                }
            )
        )
        with self.assertRaises(ledger.LedgerError):
            ledger.check(self.path, "p", dependent=True)
        self.path.write_text(
            json.dumps(
                {
                    "version": 1,
                    "problems": {
                        "p": {"failed_cycles": 0, "events": [], "disposition": "A"}
                    },
                }
            )
        )
        with self.assertRaises(ledger.LedgerError):
            ledger.check(self.path, "p", dependent=True)
        self.path = Path(self.tempdir.name) / "counter-mismatch.json"
        ledger.create_problem(
            self.path,
            problem_id="p",
            ticket="T-1",
            observation="seen",
            hypotheses=[],
            release_impact="none",
            disposition="A",
        )
        document = ledger.load(self.path)
        document["problems"]["p"]["failed_cycles"] = 3
        self.path.write_text(json.dumps(document))
        with self.assertRaises(ledger.LedgerError):
            ledger.check(self.path, "p", dependent=True)

        self.path = Path(self.tempdir.name) / "count-mismatch.json"
        ledger.create_problem(
            self.path,
            problem_id="q",
            ticket="T-2",
            observation="seen",
            hypotheses=[],
            release_impact="none",
            disposition="A",
        )
        ledger.record_result(self.path, "q", "counted", failed_cycle=True)
        document = ledger.load(self.path)
        document["problems"]["q"]["failed_cycles"] = 3
        self.path.write_text(json.dumps(document))
        with self.assertRaises(ledger.LedgerError):
            ledger.check(self.path, "q", dependent=True)

    def test_malformed_history_event_fails_closed(self) -> None:

        ledger.create_problem(
            self.path,
            problem_id="p",
            ticket="T-1",
            observation="seen",
            hypotheses=[],
            release_impact="none",
            disposition="A",
        )
        document = ledger.load(self.path)
        document["problems"]["p"]["events"].append(
            {"type": "result", "at": "now", "active_seconds": -1}
        )
        self.path.write_text(json.dumps(document))
        with self.assertRaises(ledger.LedgerError):
            ledger.check(self.path, "p", dependent=True)

    def test_close_and_unknown_problem_fail_closed(self) -> None:
        ledger.create_problem(
            self.path,
            problem_id="p",
            ticket="T-1",
            observation="seen",
            hypotheses=[],
            release_impact="none",
            disposition="A",
        )
        ledger.record_result(self.path, "p", "failed", failed_cycle=True)
        code = "from pathlib import Path; from scripts.local_agent import problem_ledger as l; import sys; print(l.load(Path(sys.argv[1]))['problems']['p']['failed_cycles'])"
        result = subprocess.run(
            ["python3.13", "-c", code, str(self.path)],
            check=True,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.stdout.strip(), "1")
        with self.assertRaises(ledger.LedgerError):
            ledger.check(self.path, "unknown", dependent=True)
        ledger.open_circuit(self.path, "p")
        self.assertFalse(ledger.check(self.path, "p", dependent=True))
        before = ledger.load(self.path)
        with self.assertRaises(ledger.LedgerError):
            ledger.record_result(self.path, "p", "retry", failed_cycle=True)
        self.assertEqual(ledger.load(self.path), before)
        ledger.record_probe(self.path, "p", "passive availability", availability=True)
        ledger.record_result(
            self.path, "p", "verified external recovery", recovered=True
        )
        self.assertFalse(ledger.check(self.path, "p", dependent=True))
        self.assertEqual(ledger.load(self.path)["problems"]["p"]["failed_cycles"], 1)

    def test_concurrent_appends_preserve_all_events_and_counts(self) -> None:
        ledger.create_problem(
            self.path,
            problem_id="p",
            ticket="T-1",
            observation="seen",
            hypotheses=[],
            release_impact="none",
            disposition="A",
        )
        code = "from pathlib import Path; from scripts.local_agent import problem_ledger as l; import sys; l.record_result(Path(sys.argv[1]), 'p', 'failed', failed_cycle=True)"
        processes = [
            subprocess.Popen(
                ["python3.13", "-c", code, str(self.path)],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            for _ in range(8)
        ]
        statuses = [process.wait() for process in processes]
        self.assertEqual(statuses.count(0), 5)
        self.assertEqual(statuses.count(1), 3)
        record = ledger.load(self.path)["problems"]["p"]
        self.assertEqual(record["failed_cycles"], 5)
        self.assertEqual(len(record["events"]), 6)

    def test_create_existing_problem_preserves_canonical_identity(self) -> None:
        ledger.create_problem(
            self.path,
            problem_id="p",
            ticket="T-1",
            observation="original",
            hypotheses=[],
            release_impact="none",
            disposition="A",
        )
        with self.assertRaises(ledger.LedgerError):
            ledger.create_problem(
                self.path,
                problem_id="p",
                ticket="T-2",
                observation="different",
                hypotheses=[],
                release_impact="none",
                disposition="B",
            )
        self.assertEqual(ledger.load(self.path)["problems"]["p"]["ticket"], "T-1")

    def test_recovery_and_metadata_updates_preserve_budget_history(self) -> None:
        ledger.create_problem(
            self.path,
            problem_id="p",
            ticket="T-1",
            observation="seen",
            hypotheses=["h1"],
            release_impact="blocks",
            disposition="A",
        )
        ledger.record_result(
            self.path, "p", "attempt", failed_cycle=True, active_seconds=9
        )
        ledger.update_problem(
            self.path, "p", hypotheses=["h2"], release_impact="clear", disposition="B"
        )
        ledger.record_result(self.path, "p", "recovered", recovered=True)
        record = ledger.load(self.path)["problems"]["p"]
        self.assertEqual(record["failed_cycles"], 1)
        self.assertEqual(record["active_seconds"], 9)
        self.assertEqual(record["hypotheses"], ["h2"])
        self.assertEqual(record["disposition"], "B")
        self.assertTrue(record["recovered"])
        self.assertEqual(len(record["events"]), 4)

    def test_recovery_annotation_preserves_open_circuit_and_budget(self) -> None:
        ledger.create_problem(
            self.path,
            problem_id="p",
            ticket="T-1",
            observation="seen",
            hypotheses=[],
            release_impact="blocks release",
            disposition="A",
        )
        for index in range(5):
            ledger.record_result(self.path, "p", f"failed {index}", failed_cycle=True)
        ledger.record_result(
            self.path, "p", "externally verified recovery", recovered=True
        )
        self.assertFalse(ledger.check(self.path, "p", dependent=True))
        record = ledger.load(self.path)["problems"]["p"]
        self.assertEqual(record["failed_cycles"], 5)
        self.assertEqual(len(record["events"]), 7)
        with self.assertRaises(ledger.LedgerError):
            ledger.record_result(self.path, "p", "sixth", failed_cycle=True)
        ledger.open_circuit(self.path, "p")
        self.assertFalse(ledger.check(self.path, "p", dependent=True))
        with self.assertRaises(ValueError):
            ledger.record_result(
                self.path, "p", "contradictory", failed_cycle=True, recovered=True
            )

    def test_resolution_allows_dependent_work_after_exhaustion_without_resetting_budget(
        self,
    ) -> None:
        ledger.create_problem(
            self.path,
            problem_id="p",
            ticket="T-1",
            observation="seen",
            hypotheses=[],
            release_impact="blocks",
            disposition="A",
        )
        for index in range(5):
            ledger.record_result(
                self.path, "p", f"failed {index}", failed_cycle=True, active_seconds=720
            )
        ledger.record_result(self.path, "p", "verified recovery", recovered=True)
        self.assertFalse(ledger.check(self.path, "p", dependent=True))
        blocked = subprocess.run(
            [
                "python3.13",
                "scripts/local_agent/problem_ledger.py",
                "check",
                str(self.path),
                "p",
                "--dependent",
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(blocked.returncode, 3)

        result = subprocess.run(
            [
                "python3.13",
                "scripts/local_agent/problem_ledger.py",
                "resolve",
                str(self.path),
                "p",
                "--owner-decision",
                "Owner approved dependent continuation",
                "--verification",
                "Independent synthetic recovery verification",
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(ledger.check(self.path, "p", dependent=True))
        record = ledger.load(self.path)["problems"]["p"]
        self.assertEqual(record["failed_cycles"], 5)
        self.assertEqual(record["active_seconds"], 3600)
        self.assertEqual(len(record["events"]), 8)
        self.assertEqual(record["results"][-1], "verified recovery")
        resolution = record["events"][-1]
        self.assertEqual(resolution["type"], "resolution")
        self.assertEqual(
            resolution["owner_decision"], "Owner approved dependent continuation"
        )
        self.assertEqual(
            resolution["verification"], "Independent synthetic recovery verification"
        )

        before = ledger.load(self.path)
        with self.assertRaises(ledger.LedgerError):
            ledger.record_result(
                self.path, "p", "another failed cycle", failed_cycle=True
            )
        with self.assertRaises(ledger.LedgerError):
            ledger.record_result(self.path, "p", "more time", active_seconds=1)
        self.assertEqual(ledger.load(self.path), before)

    def test_manual_open_revokes_a_prior_resolution(self) -> None:
        ledger.create_problem(
            self.path,
            problem_id="p",
            ticket="T-1",
            observation="seen",
            hypotheses=[],
            release_impact="blocks",
            disposition="A",
        )
        for index in range(5):
            ledger.record_result(self.path, "p", f"failed {index}", failed_cycle=True)
        ledger.record_result(self.path, "p", "verified recovery", recovered=True)
        ledger.resolve_problem(
            self.path,
            "p",
            owner_decision="Owner approved continuation",
            verification="Synthetic verification passed",
        )
        self.assertTrue(ledger.check(self.path, "p", dependent=True))

        ledger.open_circuit(self.path, "p")
        self.assertFalse(ledger.check(self.path, "p", dependent=True))
        record = ledger.load(self.path)["problems"]["p"]
        self.assertEqual(record["failed_cycles"], 5)
        self.assertEqual(record["active_seconds"], 0)
        self.assertEqual(record["events"][-1]["type"], "circuit_opened")
        dependent = subprocess.run(
            [
                "python3.13",
                "scripts/local_agent/problem_ledger.py",
                "check",
                str(self.path),
                "p",
                "--dependent",
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(dependent.returncode, 3)

    def test_resolution_requires_recovery_and_nonblank_attestations(self) -> None:
        ledger.create_problem(
            self.path,
            problem_id="p",
            ticket="T-1",
            observation="seen",
            hypotheses=[],
            release_impact="blocks",
            disposition="A",
        )
        for index in range(5):
            ledger.record_result(self.path, "p", f"failed {index}", failed_cycle=True)
        with self.assertRaises(ledger.LedgerError):
            ledger.resolve_problem(
                self.path,
                "p",
                owner_decision="Owner approved continuation",
                verification="Synthetic verification passed",
            )
        ledger.record_result(self.path, "p", "verified recovery", recovered=True)
        for decision, verification in (
            (" ", "Synthetic verification passed"),
            ("Owner approved continuation", "  "),
        ):
            with self.assertRaises(ValueError):
                ledger.resolve_problem(
                    self.path,
                    "p",
                    owner_decision=decision,
                    verification=verification,
                )
        self.assertFalse(ledger.check(self.path, "p", dependent=True))

    def test_time_exhaustion_invalidates_older_recovery_for_resolution(self) -> None:
        ledger.create_problem(
            self.path,
            problem_id="p",
            ticket="T-1",
            observation="seen",
            hypotheses=[],
            release_impact="blocks",
            disposition="A",
        )
        ledger.record_result(self.path, "p", "recovered", recovered=True)
        ledger.record_probe(
            self.path, "p", "synthetic active investigation", active_seconds=3600
        )
        with self.assertRaises(ledger.LedgerError):
            ledger.resolve_problem(
                self.path,
                "p",
                owner_decision="SYNTHETIC_TEST_OWNER approves continuation",
                verification="Synthetic verification passed",
            )
        self.assertFalse(ledger.check(self.path, "p", dependent=True))

    def test_malformed_historical_resolution_fails_closed(self) -> None:
        ledger.create_problem(
            self.path,
            problem_id="p",
            ticket="T-1",
            observation="seen",
            hypotheses=[],
            release_impact="blocks",
            disposition="A",
        )
        ledger.record_result(self.path, "p", "verified recovery", recovered=True)
        document = ledger.load(self.path)
        document["problems"]["p"]["events"].append(
            {
                "type": "resolution",
                "at": "now",
                "owner_decision": " ",
                "verification": "Synthetic verification passed",
            }
        )
        self.path.write_text(json.dumps(document))
        with self.assertRaises(ledger.LedgerError):
            ledger.check(self.path, "p", dependent=True)

    def test_resolution_history_must_qualify_before_later_exhaustion(self) -> None:
        ledger.create_problem(
            self.path,
            problem_id="p",
            ticket="T-1",
            observation="seen",
            hypotheses=[],
            release_impact="blocks",
            disposition="A",
        )
        ledger.record_result(self.path, "p", "synthetic recovery", recovered=True)
        document = ledger.load(self.path)
        record = document["problems"]["p"]
        record["events"].extend(
            [
                {
                    "type": "resolution",
                    "at": "now",
                    "owner_decision": "SYNTHETIC_TEST_OWNER decision",
                    "verification": "synthetic verification",
                },
                {
                    "type": "probe",
                    "at": "later",
                    "description": "synthetic work",
                    "active_seconds": 3600,
                },
            ]
        )
        record["active_seconds"] = 3600
        record["circuit_open"] = True
        self.path.write_text(json.dumps(document))
        with self.assertRaises(ledger.LedgerError):
            ledger.check(self.path, "p", dependent=True)

    def test_legacy_open_preserves_record_but_requires_fresh_recovery(self) -> None:
        ledger.create_problem(
            self.path,
            problem_id="p",
            ticket="T-1",
            observation="seen",
            hypotheses=[],
            release_impact="blocks",
            disposition="A",
        )
        ledger.record_result(self.path, "p", "synthetic old recovery", recovered=True)
        document = ledger.load(self.path)
        record = document["problems"]["p"]
        record["circuit_open"] = True
        record["events"].append({"type": "circuit_opened", "at": "later"})
        self.path.write_text(json.dumps(document))
        before = ledger.load(self.path)
        self.assertTrue(before["problems"]["p"]["recovered"])
        self.assertFalse(ledger.check(self.path, "p", dependent=True))
        with self.assertRaises(ledger.LedgerError):
            ledger.resolve_problem(
                self.path,
                "p",
                owner_decision="SYNTHETIC_TEST_OWNER decision",
                verification="synthetic verification",
            )
        self.assertEqual(ledger.load(self.path), before)
        ledger.record_result(self.path, "p", "synthetic fresh recovery", recovered=True)
        ledger.resolve_problem(
            self.path,
            "p",
            owner_decision="SYNTHETIC_TEST_OWNER decision",
            verification="synthetic verification",
        )
        after = ledger.load(self.path)["problems"]["p"]
        self.assertEqual(after["events"][: len(record["events"])], record["events"])
        self.assertEqual((after["failed_cycles"], after["active_seconds"]), (0, 0))
        self.assertTrue(after["circuit_open"])
        self.assertTrue(ledger.check(self.path, "p", dependent=True))
        with self.assertRaises(ledger.LedgerError):
            ledger.record_result(
                self.path, "p", "synthetic new attempt", failed_cycle=True
            )

    def test_cli_create_result_and_check_are_explicit(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "ledger.json"
            with patch(
                "sys.argv",
                [
                    "problem_ledger",
                    "create",
                    str(path),
                    "p",
                    "T-1",
                    "seen",
                    "blocks release",
                    "--disposition",
                    "A",
                ],
            ):
                self.assertEqual(ledger.main(), 0)
            with patch(
                "sys.argv",
                [
                    "problem_ledger",
                    "result",
                    str(path),
                    "p",
                    "fixed",
                    "--failed-cycle",
                    "--active-seconds",
                    "4",
                ],
            ):
                self.assertEqual(ledger.main(), 0)
            with patch(
                "sys.argv", ["problem_ledger", "check", str(path), "p", "--dependent"]
            ):
                self.assertEqual(ledger.main(), 0)
            record = ledger.load(path)["problems"]["p"]
            self.assertEqual(record["failed_cycles"], 1)
            self.assertEqual(record["active_seconds"], 4)


if __name__ == "__main__":
    unittest.main()
