"""Synthetic checks for runtime_bench: statistics, verdicts, classes, guards."""

import tempfile
import unittest
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
        self.assertGreater(summary["stdev"], 0)


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

    def test_known_mcp_pid_is_its_own_class(self):
        self.assertEqual(rb.classify("python3", 101, 100, frozenset({101})), "mcp")


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


class GuardTests(unittest.TestCase):
    def test_output_directory_must_be_outside_git(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertTrue(rb.outside_git(Path(tmp) / "evidence"))
        self.assertFalse(rb.outside_git(Path(__file__).parent))

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
                         "samples": [{"agent": 5_000_000, "mcp": 0, "browser": 0, "other": 0}]}]
            result = rb.measure_variant("minimal", binary, launches)
            text = rb.render([result], {"minimal": 0.5},
                             {"loadavg_1m": 0.1, "cpus": 8, "cargo_processes": 0}, False)
        self.assertNotIn(tmp, text)
        self.assertNotIn("/", text.replace("/s", ""))


if __name__ == "__main__":
    unittest.main()
