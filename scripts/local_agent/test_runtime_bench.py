"""Synthetic checks for runtime_bench: statistics, verdicts, classes, guards."""

import subprocess
import tempfile
import unittest
from unittest import mock
from pathlib import Path

import runtime_bench as rb

PS_SAMPLE = """\
  100     1   100   20480 /opt/zeroclaw
  101   100   100    5120 /usr/bin/python3
  102   100   100    8192 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome
  103   100   100    2048 chromedriver
  104     1     1    4096 /usr/sbin/unrelated
not a process line
"""


class StatisticsTests(unittest.TestCase):
    def test_percentile_is_nearest_rank(self):
        values = [float(v) for v in range(1, 21)]
        self.assertEqual(rb.percentile(values, 95), 19.0)
        self.assertEqual(rb.percentile([4.0], 95), 4.0)

    def test_summary_reports_spread(self):
        summary = rb.summarize([1.0, 2.0, 3.0, 4.0, 10.0])
        self.assertEqual(
            (summary["median"], summary["min"], summary["max"], summary["n"]),
            (3.0, 1.0, 10.0, 5),
        )
        self.assertAlmostEqual(summary["stdev"], 3.5355339, places=5)  # sample stdev


class ProcessTreeTests(unittest.TestCase):
    def test_ps_parser_skips_noise(self):
        rows = rb.parse_ps(PS_SAMPLE)
        self.assertEqual([r[0] for r in rows], [100, 101, 102, 103, 104])

    def test_classes_separate_agent_children_and_browsers(self):
        rows = {pid: comm for pid, _ppid, _pgid, _rss, comm in rb.parse_ps(PS_SAMPLE)}
        got = {pid: rb.classify(comm, pid, 100) for pid, comm in rows.items()}
        self.assertEqual(got[100], "agent")
        self.assertEqual(got[102], "browser")  # positive control: a Chrome process
        self.assertEqual(got[103], "browser")
        self.assertEqual(got[101], "other")



class VerdictTests(unittest.TestCase):
    def summary(self, median, p95):
        return {"median": median, "p95": p95}

    def test_budgets_pass_and_fail(self):
        ok = rb.verdicts(24_000_000, True, self.summary(0.4, 0.45),
                         self.summary(59e6, 59e6), False)
        self.assertEqual(set(ok.values()), {"PASS"})
        bad = rb.verdicts(26_000_000, True, self.summary(0.6, 0.6),
                          self.summary(61e6, 61e6), True)
        self.assertTrue(all(v.startswith("FAIL") for v in bad.values()))

    def test_tail_breach_is_reported_not_hidden(self):
        result = rb.verdicts(1, True, self.summary(0.4, 0.9), self.summary(1, 1), False)
        self.assertIn("tail breach", result["cold_start"])
        self.assertTrue(result["cold_start"].startswith("PASS"))

    def test_debug_build_cannot_pass_the_size_budget(self):
        result = rb.verdicts(1, False, self.summary(0.1, 0.1), self.summary(1, 1), False)
        self.assertTrue(result["size"].startswith("BLOCKED"))


class ExitCodeTests(unittest.TestCase):
    PASS = {"variant": "a", "verdicts": {"size": "PASS", "cold_start": "PASS"}}

    def test_all_pass_and_quiet_is_zero(self):
        self.assertEqual(rb.exit_code([self.PASS], False), 0)

    def test_noise_blocked_fail_and_empty_are_not_zero(self):
        failed = {"variant": "a", "verdicts": {"size": "FAIL"}}
        blocked = {"variant": "a", "verdicts": {"size": "BLOCKED (debug)"}}
        all_launches_failed = {"variant": "a", "verdict": "BLOCKED: no launch reached the provider"}
        self.assertEqual(rb.exit_code([self.PASS], True), 1)
        for result in (failed, blocked, all_launches_failed):
            self.assertEqual(rb.exit_code([result], False), 1)
        self.assertEqual(rb.exit_code([], False), 1)

    def test_partial_launch_failure_cannot_pass(self):
        with tempfile.TemporaryDirectory() as tmp:
            binary = Path(tmp) / "zeroclaw"
            binary.write_bytes(b"x")
            good = {"ok": True, "cold_start_s": 0.1, "listeners": {},
                    "samples": [{"agent": 1_000_000, "browser": 0, "other": 0}]}
            result = rb.measure_variant("m", binary, [good, {"ok": False}], ("m",))
        self.assertTrue(all(v.startswith("BLOCKED") for v in result["verdicts"].values()))
        self.assertEqual(rb.exit_code([result], False), 1)

    def test_size_needs_an_explicit_shipped_claim(self):
        with tempfile.TemporaryDirectory() as tmp:
            binary = Path(tmp) / "zeroclaw"
            binary.write_bytes(b"x")
            good = {"ok": True, "cold_start_s": 0.1, "listeners": {},
                    "samples": [{"agent": 1_000_000, "browser": 0, "other": 0}]}
            unclaimed = rb.measure_variant("m", binary, [good])
            claimed = rb.measure_variant("m", binary, [good], ("m",))
        self.assertTrue(unclaimed["verdicts"]["size"].startswith("BLOCKED"))
        self.assertEqual(claimed["verdicts"]["size"], "PASS")


class DeadAgentTests(unittest.TestCase):
    def test_agent_that_exits_during_sampling_is_not_ok(self):
        proc = mock.Mock(pid=1)
        proc.poll.return_value = 1
        with mock.patch.object(rb.lp, "write_config"), \
                mock.patch.object(rb.lp, "launch_agent", return_value=proc), \
                mock.patch.object(rb.lp, "wait_until_idle", return_value=True), \
                mock.patch.object(rb.lp, "stop_agent"), \
                mock.patch.object(rb, "SETTLE_SECONDS", 0), \
                mock.patch.object(rb.time, "sleep"), \
                mock.patch.object(rb, "tree_rss", return_value={"agent": 0, "browser": 0, "other": 0}):
            rb.TimedStall.first_request_at = rb.time.monotonic()
            self.assertEqual(rb.one_launch(Path("/x"), 1, 2), {"ok": False})


class GuardTests(unittest.TestCase):
    def test_output_directory_must_be_outside_git(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertTrue(rb.outside_git(Path(tmp) / "evidence"))

    def test_nested_missing_directory_inside_a_repo_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            subprocess.run(["git", "init", "-q", tmp], check=True)
            self.assertFalse(rb.outside_git(Path(tmp)))
            self.assertFalse(rb.outside_git(Path(tmp) / "evidence" / "a" / "b"))

    def test_unknown_git_failure_is_not_treated_as_outside(self):
        failed = subprocess.CompletedProcess([], 128, "", "fatal: detected dubious ownership")
        with mock.patch.object(rb.subprocess, "run", return_value=failed):
            self.assertFalse(rb.outside_git(Path(tempfile.gettempdir())))
        with mock.patch.object(rb.subprocess, "run", side_effect=FileNotFoundError):
            self.assertFalse(rb.outside_git(Path(tempfile.gettempdir())))

    def test_premise_rejects_unusable_sampling_windows(self):
        with tempfile.TemporaryDirectory() as tmp:
            binary = Path(tmp) / "zeroclaw"
            binary.write_bytes(b"x")
            out = Path(tmp) / "evidence"
            with mock.patch.object(rb, "heavy_lock_held", return_value=False):
                self.assertIsNone(rb.premise_error({"m": binary}, out, 1, 10))
                for launches, idle in ((0, 10), (1, 0), (1, 25)):
                    self.assertIsNotNone(rb.premise_error({"m": binary}, out, launches, idle))

    def test_noise_detection(self):
        quiet = {"loadavg_1m": 0.2, "cpus": 16, "cargo_processes": 0}
        self.assertFalse(rb.is_noisy(quiet))
        self.assertTrue(rb.is_noisy({**quiet, "cargo_processes": 1}))
        self.assertTrue(rb.is_noisy({**quiet, "loadavg_1m": 12.0}))

    def test_summary_text_has_no_paths(self):
        with tempfile.TemporaryDirectory() as tmp:
            binary = Path(tmp) / "zeroclaw"
            binary.write_bytes(b"x" * 10)
            launches = [{"ok": True, "cold_start_s": 0.3, "listeners": {},
                         "samples": [{"agent": 5_000_000, "browser": 0, "other": 0}]}]
            result = rb.measure_variant("minimal", binary, launches)
            text = rb.render([result], {"minimal": 0.5},
                             {"loadavg_1m": 0.1, "cpus": 8, "cargo_processes": 0}, False)
        self.assertNotIn(tmp, text)
        self.assertNotIn(str(Path.home()), text)


if __name__ == "__main__":
    unittest.main()
