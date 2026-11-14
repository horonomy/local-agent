#!/usr/bin/env python3
"""Process-tree benchmark for eager vs on-demand child processes (HORO-2402).

Counts live descendant processes of this script at four phases, once per spawn
policy, using real (idle, local) fake children:

  assembled     right after the agent would be assembled
  active        while one call is in flight
  after_idle    after the idle window has expired with no calls
  after_close   after shutdown

For `eager` every server child exists from `assembled` on. For `on_demand` the
child exists only while a call is in flight and until the idle window ends.
The supervisor here is a small stand-in that models the documented policy
(spawn on first call, ref-count in-flight calls, reap on idle, kill the whole
process group on close). It measures the process-tree shape that policy
produces; it does not drive the Rust implementation, which is covered by the
`mcp_lazy` tests. Nothing here touches a browser, Node, or the network.

`--observe PID` instead reports the live descendant count of an existing
process, as counts per executable basename only (no command lines or paths).

Usage: process_tree_bench.py [--servers 3] [--idle-ttl 1.0] [--json]
       process_tree_bench.py --observe PID
Exit 0: on-demand idle tree is empty and no child outlived close.
Exit 1: an on-demand idle tree is non-empty or an orphan remained.
"""

import argparse
import json
import os
import signal
import subprocess
import sys
import time
from collections import Counter

PHASES = ("assembled", "active", "after_idle", "after_close")
CHILD_CODE = "import time\nwhile True:\n    time.sleep(1)\n"


def parse_ps(text):
    """Parse `ps -axo pid=,ppid=,rss=,comm=` rows into (pid, ppid, rss_kb, name)."""
    rows = []
    for line in text.splitlines():
        parts = line.split(None, 3)
        if len(parts) < 4:
            continue
        try:
            pid, ppid, rss = int(parts[0]), int(parts[1]), int(parts[2])
        except ValueError:
            continue
        rows.append((pid, ppid, rss, os.path.basename(parts[3].strip())))
    return rows


def descendants(rows, root):
    """Rows for every transitive child of `root` (root itself excluded)."""
    by_parent = {}
    for row in rows:
        by_parent.setdefault(row[1], []).append(row)
    found, stack = [], [root]
    while stack:
        for row in by_parent.get(stack.pop(), []):
            found.append(row)
            stack.append(row[0])
    return found


def ps_rows():
    """Process rows, minus the `ps` probe itself (it is our own child)."""
    proc = subprocess.Popen(
        ["ps", "-axo", "pid=,ppid=,rss=,comm="],
        stdout=subprocess.PIPE, text=True,
    )
    out, _ = proc.communicate()
    return [row for row in parse_ps(out) if row[0] != proc.pid]


def snapshot(root):
    tree = descendants(ps_rows(), root)
    return {"children": len(tree), "rss_kb": sum(r[2] for r in tree)}


def alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    # A zombie still answers signal 0; treat it as gone once it is reaped.
    return True


class Supervisor:
    """Models eager / on_demand child ownership for a set of servers."""

    def __init__(self, mode, servers, idle_ttl, clock=time.monotonic):
        if mode not in ("eager", "on_demand"):
            raise ValueError(f"unknown mode {mode!r}")
        self.mode, self.idle_ttl, self.clock = mode, idle_ttl, clock
        self.names = [f"srv{i}" for i in range(servers)]
        self.children = {}
        self.inflight = {name: 0 for name in self.names}
        self.last_used = {}
        self.spawned_pids = []
        if mode == "eager":
            for name in self.names:
                self._spawn(name)

    def _spawn(self, name):
        proc = subprocess.Popen(
            [sys.executable, "-c", CHILD_CODE],
            start_new_session=True,  # own process group, like the Rust transport
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        self.children[name] = proc
        self.spawned_pids.append(proc.pid)

    def _reap(self, name):
        proc = self.children.pop(name, None)
        if proc is None:
            return
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        proc.wait()

    def begin_call(self, name):
        self.inflight[name] += 1
        if name not in self.children:
            self._spawn(name)

    def end_call(self, name):
        self.inflight[name] -= 1
        self.last_used[name] = self.clock()

    def tick(self):
        """Reap on-demand children that are idle past the window."""
        if self.mode != "on_demand":
            return
        now = self.clock()
        for name in list(self.children):
            if self.inflight[name] == 0 and now - self.last_used.get(name, now) >= self.idle_ttl:
                self._reap(name)

    def close(self):
        for name in list(self.children):
            self._reap(name)

    def orphans(self):
        return [pid for pid in self.spawned_pids if alive(pid) and not self._is_zombie(pid)]

    @staticmethod
    def _is_zombie(pid):
        out = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)],
                             capture_output=True, text=True).stdout.strip()
        return out == "" or out.startswith("Z")


def run_scenario(mode, servers, idle_ttl, sleep=time.sleep, root=None):
    root = os.getpid() if root is None else root
    sup = Supervisor(mode, servers, idle_ttl)
    result = {}
    try:
        result["assembled"] = snapshot(root)
        sup.begin_call("srv0")
        result["active"] = snapshot(root)
        sup.end_call("srv0")
        sleep(idle_ttl + 0.2)
        sup.tick()
        result["after_idle"] = snapshot(root)
    finally:
        sup.close()
    result["after_close"] = snapshot(root)
    result["orphans"] = len(sup.orphans())
    return result


def verdict(results):
    """Return a list of failure strings; empty means the policy held."""
    failures = []
    on_demand = results["on_demand"]
    for phase in ("assembled", "after_idle", "after_close"):
        if on_demand[phase]["children"] != 0:
            failures.append(f"on_demand {phase} tree not empty: {on_demand[phase]['children']}")
    for mode, data in results.items():
        if data["after_close"]["children"] != 0 or data["orphans"] != 0:
            failures.append(f"{mode} left {data['orphans']} orphan(s)")
    return failures


def observe(pid):
    tree = descendants(ps_rows(), pid)
    return {
        "children": len(tree),
        "rss_kb": sum(r[2] for r in tree),
        "by_executable": dict(Counter(r[3] for r in tree)),
    }


def render(results):
    lines = [f"{'mode':<10}" + "".join(f"{p:>13}" for p in PHASES) + f"{'orphans':>9}"]
    for mode, data in results.items():
        cells = "".join(f"{data[p]['children']:>13}" for p in PHASES)
        lines.append(f"{mode:<10}{cells}{data['orphans']:>9}")
    return "\n".join(lines)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--servers", type=int, default=3)
    ap.add_argument("--idle-ttl", type=float, default=1.0)
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--observe", type=int, metavar="PID")
    args = ap.parse_args(argv)
    if args.observe is not None:
        print(json.dumps(observe(args.observe), indent=2, sort_keys=True))
        return 0
    results = {m: run_scenario(m, args.servers, args.idle_ttl) for m in ("eager", "on_demand")}
    failures = verdict(results)
    print(json.dumps({"results": results, "failures": failures}, indent=2) if args.json
          else render(results))
    for failure in failures:
        print(f"FAIL: {failure}", file=sys.stderr)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
