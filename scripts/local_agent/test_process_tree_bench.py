"""Tests for process_tree_bench: parsing, supervisor policy, verdicts."""

import unittest

import process_tree_bench as ptb

PS_SAMPLE = """\
  100     1  2048 /opt/zeroclaw
  101   100  1024 /usr/bin/python3
  102   101   512 helper
  103     1   256 /usr/sbin/unrelated
junk line
"""


class FakeClock:
    def __init__(self):
        self.now = 0.0

    def __call__(self):
        return self.now


class ParsingTests(unittest.TestCase):
    def test_descendants_are_transitive_and_exclude_root(self):
        rows = ptb.parse_ps(PS_SAMPLE)
        self.assertEqual([r[0] for r in ptb.descendants(rows, 100)], [101, 102])

    def test_names_are_basenames_only(self):
        rows = ptb.parse_ps(PS_SAMPLE)
        self.assertIn("python3", [r[3] for r in rows])


class SupervisorTests(unittest.TestCase):
    def tearDown(self):
        for sup in getattr(self, "sups", []):
            sup.close()

    def make(self, mode, servers=2, ttl=5.0):
        clock = FakeClock()
        sup = ptb.Supervisor(mode, servers, ttl, clock=clock)
        self.sups = getattr(self, "sups", []) + [sup]
        return sup, clock

    def test_eager_spawns_every_child_up_front(self):
        sup, _ = self.make("eager", servers=3)
        self.assertEqual(len(sup.children), 3)

    def test_on_demand_spawns_nothing_until_a_call(self):
        sup, _ = self.make("on_demand", servers=3)
        self.assertEqual(len(sup.children), 0)
        sup.begin_call("srv0")
        self.assertEqual(list(sup.children), ["srv0"])

    def test_concurrent_calls_share_one_child(self):
        sup, _ = self.make("on_demand")
        sup.begin_call("srv0")
        sup.begin_call("srv0")
        self.assertEqual(len(sup.spawned_pids), 1)

    def test_inflight_call_blocks_idle_reap(self):
        sup, clock = self.make("on_demand", ttl=5.0)
        sup.begin_call("srv0")
        clock.now = 100.0
        sup.tick()
        self.assertIn("srv0", sup.children)

    def test_idle_reap_happens_only_after_the_window(self):
        sup, clock = self.make("on_demand", ttl=5.0)
        sup.begin_call("srv0")
        sup.end_call("srv0")
        clock.now = 4.9
        sup.tick()
        self.assertIn("srv0", sup.children)
        clock.now = 5.0
        sup.tick()
        self.assertNotIn("srv0", sup.children)

    def test_reaped_child_is_gone_and_next_call_respawns(self):
        sup, clock = self.make("on_demand", ttl=1.0)
        sup.begin_call("srv0")
        sup.end_call("srv0")
        clock.now = 2.0
        sup.tick()
        self.assertEqual(sup.orphans(), [])
        sup.begin_call("srv0")
        self.assertEqual(len(sup.spawned_pids), 2)

    def test_close_leaves_no_orphans(self):
        sup, _ = self.make("eager", servers=2)
        sup.close()
        self.assertEqual(sup.orphans(), [])

    def test_unknown_mode_rejected(self):
        with self.assertRaises(ValueError):
            ptb.Supervisor("lazyish", 1, 1.0)


class VerdictTests(unittest.TestCase):
    def phase(self, n):
        return {"children": n, "rss_kb": 0}

    def results(self, on_demand_idle=0, orphans=0):
        eager = {"assembled": self.phase(3), "active": self.phase(3),
                 "after_idle": self.phase(3), "after_close": self.phase(0), "orphans": 0}
        lazy = {"assembled": self.phase(0), "active": self.phase(1),
                "after_idle": self.phase(on_demand_idle), "after_close": self.phase(0),
                "orphans": orphans}
        return {"eager": eager, "on_demand": lazy}

    def test_healthy_run_passes(self):
        self.assertEqual(ptb.verdict(self.results()), [])

    def test_non_empty_idle_tree_fails(self):
        self.assertTrue(ptb.verdict(self.results(on_demand_idle=1)))

    def test_orphan_fails(self):
        self.assertTrue(ptb.verdict(self.results(orphans=1)))


class ScenarioTests(unittest.TestCase):
    def test_end_to_end_shape(self):
        results = {m: ptb.run_scenario(m, 2, 0.1) for m in ("eager", "on_demand")}
        self.assertEqual(results["eager"]["assembled"]["children"], 2)
        self.assertEqual(results["on_demand"]["assembled"]["children"], 0)
        self.assertEqual(results["on_demand"]["active"]["children"], 1)
        self.assertEqual(ptb.verdict(results), [])


if __name__ == "__main__":
    unittest.main()
