#!/usr/bin/env python3
"""Measure binary size, cold start and idle memory of Local Agent builds.

Each variant is a built `zeroclaw` binary. Launches are interleaved (A B A B)
so machine drift hits every variant alike. Definitions, per baseline-method.md:

* cold start: monotonic time from just before the agent process is spawned to
  the moment a local stub provider receives the agent's first request. The
  process is "process-cold, file cache warm": purging the cache needs root.
* idle memory: the agent is alive and waiting on the stalling stub ("awaiting
  provider response"). After a settle period, resident memory of the agent
  process and of its whole tree is sampled once a second; each launch yields a
  median, and the report gives median, p95, stdev, min and max across launches.
* first launch: macOS assesses a freshly linked binary on its first exec, which
  can take minutes. It is measured once per binary and reported on its own line,
  never folded into the samples.

Raw results go to --out, which must be outside every Git worktree. Stdout is a
text summary with no paths or command lines.

Usage: runtime_bench.py --variant NAME=PATH [--variant NAME=PATH ...]
                        --out DIR [--launches 10] [--idle-seconds 10]
Exit 0: every budget met. Exit 1: a budget breached or the batch is noisy.
Exit 2: the premise failed (bad binary, output dir inside Git, machine busy).
"""

import argparse
import hashlib
import json
import math
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from http.server import ThreadingHTTPServer
from pathlib import Path

import listener_probe as lp

SIZE_BUDGET_BYTES = 25_000_000  # decimal MB, as written in the ticket
RSS_BUDGET_BYTES = 60_000_000
COLD_START_BUDGET_S = 0.5
SETTLE_SECONDS = 5
BROWSER_COMMS = ("agent-browser", "chromedriver", "chrome", "google chrome")
HEAVY_LOCK = Path.home() / ".cargo/targets/local-agent.lock"


class TimedStall(lp.Stall):
    """Stalling provider that also records when the first request arrived."""

    first_request_at: "float | None" = None

    def do_POST(self):  # noqa: N802 - http.server API
        if TimedStall.first_request_at is None:
            TimedStall.first_request_at = time.monotonic()
        super().do_POST()


def percentile(values: list[float], pct: float) -> float:
    """Nearest-rank percentile."""
    ordered = sorted(values)
    rank = max(1, math.ceil(pct / 100 * len(ordered)))
    return ordered[rank - 1]


def summarize(values: list[float]) -> dict:
    return {
        "n": len(values),
        "median": statistics.median(values),
        "p95": percentile(values, 95),
        "stdev": statistics.pstdev(values) if len(values) > 1 else 0.0,
        "min": min(values),
        "max": max(values),
    }


def classify(comm: str, pid: int, root_pid: int, mcp_pids: frozenset = frozenset()) -> str:
    if pid == root_pid:
        return "agent"
    if pid in mcp_pids:
        return "mcp"
    name = os.path.basename(comm).lower()
    if any(name.startswith(b) for b in BROWSER_COMMS):
        return "browser"
    return "other"


def parse_ps(text: str) -> list[tuple[int, int, int, int, str]]:
    rows = []
    for line in text.splitlines():
        parts = line.split(None, 4)
        if len(parts) == 5 and all(p.isdigit() for p in parts[:4]):
            rows.append((int(parts[0]), int(parts[1]), int(parts[2]), int(parts[3]), parts[4]))
    return rows


def tree_rss(root_pid: int) -> dict[str, int]:
    """Resident bytes per class for root_pid's tree. Reads comm only, never args."""
    out = subprocess.run(
        ["ps", "-axo", "pid=,ppid=,pgid=,rss=,comm="],
        capture_output=True, text=True, check=False,
    ).stdout
    rows = parse_ps(out)
    members = {root_pid}
    grew = True
    while grew:
        grew = False
        for pid, ppid, pgid, _rss, _comm in rows:
            if pid not in members and (ppid in members or pgid == root_pid):
                members.add(pid)
                grew = True
    totals: dict[str, int] = {"agent": 0, "mcp": 0, "browser": 0, "other": 0}
    for pid, _ppid, _pgid, rss_kb, comm in rows:
        if pid in members:
            totals[classify(comm, pid, root_pid)] += rss_kb * 1024
    return totals


def machine_state() -> dict:
    cargo = subprocess.run(["pgrep", "-x", "cargo"], capture_output=True, text=True).stdout.split()
    return {
        "loadavg_1m": os.getloadavg()[0],
        "cpus": os.cpu_count() or 1,
        "cargo_processes": len(cargo),
    }


def is_noisy(state: dict) -> bool:
    return state["cargo_processes"] > 0 or state["loadavg_1m"] > state["cpus"] * 0.5


def heavy_lock_held() -> bool:
    if not HEAVY_LOCK.exists():
        return False
    import fcntl

    with open(HEAVY_LOCK, "w") as handle:
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return True
        fcntl.flock(handle, fcntl.LOCK_UN)
    return False


def outside_git(path: Path) -> bool:
    probe = path if path.exists() else path.parent
    result = subprocess.run(
        ["git", "-C", str(probe), "rev-parse", "--is-inside-work-tree"],
        capture_output=True, text=True, check=False,
    )
    return result.returncode != 0


def one_launch(binary: Path, port: int, idle_seconds: int) -> dict:
    """One idle agent launch: cold start, settle, then idle memory samples."""
    root = Path(tempfile.mkdtemp(prefix="runtime-bench-"))
    proc = None
    try:
        lp.write_config(root, port)
        lp.Stall.connections = 0
        TimedStall.first_request_at = None
        started = time.monotonic()
        proc = lp.launch_agent(binary, root)
        if not lp.wait_until_idle(proc) or TimedStall.first_request_at is None:
            return {"ok": False}
        cold = TimedStall.first_request_at - started
        time.sleep(SETTLE_SECONDS)
        samples = []
        for _ in range(idle_seconds):
            samples.append(tree_rss(proc.pid))
            time.sleep(1)
        listeners = lp.inspect_tree(proc.pid)
        return {"ok": True, "cold_start_s": cold, "samples": samples, "listeners": listeners}
    finally:
        if proc is not None:
            lp.stop_agent(proc)
        shutil.rmtree(root, ignore_errors=True)


def per_launch_medians(launch: dict) -> dict[str, float]:
    return {
        cls: statistics.median(s[cls] for s in launch["samples"])
        for cls in ("agent", "mcp", "browser", "other")
    }


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verdicts(size: int, shipped: bool, cold: dict, rss: dict, listeners: bool) -> dict:
    def tail(summary: dict, budget: float) -> str:
        return " (tail breach: p95 over budget)" if summary["p95"] > budget else ""

    result = {
        "size": "PASS" if size <= SIZE_BUDGET_BYTES else "FAIL",
        "cold_start": "PASS" if cold["median"] <= COLD_START_BUDGET_S else "FAIL",
        "idle_rss": "PASS" if rss["median"] <= RSS_BUDGET_BYTES else "FAIL",
        "ingress": "FAIL" if listeners else "PASS",
    }
    result["cold_start"] += tail(cold, COLD_START_BUDGET_S)
    result["idle_rss"] += tail(rss, RSS_BUDGET_BYTES)
    if not shipped:
        result["size"] = "BLOCKED (not a stripped release build)"
    return result


def measure_variant(name: str, binary: Path, launches: list[dict]) -> dict:
    good = [launch for launch in launches if launch.get("ok")]
    if not good:
        return {"variant": name, "verdict": "BLOCKED: no launch reached the provider"}
    medians = [per_launch_medians(launch) for launch in good]
    cold = summarize([launch["cold_start_s"] for launch in good])
    agent_rss = summarize([m["agent"] for m in medians])
    tree_rss_summary = summarize([sum(m.values()) for m in medians])
    listeners = any(launch["listeners"] for launch in good)
    size = binary.stat().st_size
    shipped = "/debug/" not in str(binary)
    return {
        "variant": name,
        "bytes": size,
        "sha256": sha256(binary),
        "launches_ok": len(good),
        "launches_total": len(launches),
        "cold_start_s": cold,
        "agent_rss_bytes": agent_rss,
        "tree_rss_bytes": tree_rss_summary,
        "browser_rss_bytes": statistics.median(m["browser"] for m in medians),
        "listener_found": listeners,
        "verdicts": verdicts(size, shipped, cold, agent_rss, listeners),
    }


def render(results: list[dict], first_launch: dict, state: dict, noisy: bool) -> str:
    lines = [f"batch: load {state['loadavg_1m']:.2f} on {state['cpus']} cpus, "
             f"cargo processes {state['cargo_processes']}, {'NOISY' if noisy else 'quiet'}"]
    for r in results:
        lines.append(f"[{r['variant']}] first launch {first_launch[r['variant']]:.1f}s (reported, not sampled)")
        if "bytes" not in r:
            lines.append(f"  {r['verdict']}")
            continue
        c, a, t = r["cold_start_s"], r["agent_rss_bytes"], r["tree_rss_bytes"]
        lines += [
            f"  size {r['bytes']} B ({r['bytes'] / 2**20:.1f} MiB) sha256 {r['sha256'][:16]}  {r['verdicts']['size']}",
            f"  cold start median {c['median'] * 1000:.0f} ms p95 {c['p95'] * 1000:.0f} ms "
            f"sd {c['stdev'] * 1000:.0f} ms n={c['n']}  {r['verdicts']['cold_start']}",
            f"  idle RSS agent median {a['median'] / 1e6:.1f} MB p95 {a['p95'] / 1e6:.1f} MB "
            f"sd {a['stdev'] / 1e6:.1f} MB; tree {t['median'] / 1e6:.1f} MB; "
            f"browser {r['browser_rss_bytes'] / 1e6:.1f} MB  {r['verdicts']['idle_rss']}",
            f"  listening sockets in tree: {'FOUND' if r['listener_found'] else 'none'}  {r['verdicts']['ingress']}",
        ]
    return "\n".join(lines)


def parse_variants(values: list[str]) -> dict[str, Path] | None:
    parsed = {}
    for item in values:
        name, _, raw = item.partition("=")
        binary = lp.validated_binary(Path(raw)) if raw else None
        if not name or binary is None:
            return None
        parsed[name] = binary
    return parsed


def premise_error(variants, out: Path) -> str | None:
    if not variants:
        return "a variant is not NAME=PATH to an executable"
    if not outside_git(out):
        return "--out must be outside every Git worktree"
    if heavy_lock_held():
        return "a heavy Local Agent build holds the admission lock"
    return None


def run_batches(variants: dict[str, Path], launches: int, idle_seconds: int, port: int):
    first_launch = {name: lp.warm_up(binary) for name, binary in variants.items()}
    for binary in variants.values():  # discarded warm-ups
        for _ in range(3):
            subprocess.run([str(binary), "--version"], capture_output=True, check=False)
    collected: dict[str, list[dict]] = {name: [] for name in variants}
    for _ in range(launches):
        for name, binary in variants.items():
            collected[name].append(one_launch(binary, port, idle_seconds))
    return first_launch, collected


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--variant", action="append", required=True)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--launches", type=int, default=10)
    parser.add_argument("--idle-seconds", type=int, default=10)
    args = parser.parse_args()

    variants = parse_variants(args.variant)
    problem = premise_error(variants, args.out.resolve())
    if problem:
        print(json.dumps({"verdict": "premise-failed", "reason": problem}))
        return 2

    server = ThreadingHTTPServer(("127.0.0.1", 0), TimedStall)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        before = machine_state()
        first_launch, collected = run_batches(
            variants, args.launches, args.idle_seconds, server.server_address[1]
        )
        after = machine_state()
    finally:
        server.shutdown()
    noisy = is_noisy(before) or is_noisy(after)
    results = [measure_variant(n, variants[n], collected[n]) for n in variants]
    args.out.mkdir(parents=True, exist_ok=True)
    raw = {"results": results, "first_launch_s": first_launch, "before": before,
           "after": after, "launches": collected}
    (args.out / "raw.json").write_text(json.dumps(raw, indent=2))
    print(render(results, first_launch, after, noisy))
    breached = any("FAIL" in str(v) for r in results for v in r.get("verdicts", {}).values())
    return 1 if breached or noisy else 0


if __name__ == "__main__":
    sys.exit(main())
