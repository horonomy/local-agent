#!/usr/bin/env python3
"""Measure binary size, cold start and idle memory of Local Agent builds.

Each variant is a built `zeroclaw` binary. Launches are interleaved (A B A B)
so machine drift hits every variant alike. Definitions, per baseline-method.md:

* cold start: monotonic time from just before the agent process is spawned to
  the moment a local stub provider receives the agent's first request. The
  process is "process-cold, file cache warm": purging the cache needs root.
  Every launch uses a fresh config, home and workspace, so it includes first-run
  state creation rather than a warm restart.
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
Exit 0: every budget met and nothing BLOCKED. Exit 1: a budget breached, a
verdict BLOCKED, or the batch is noisy. Exit 2: the premise failed (bad binary,
output dir inside Git, a heavy build holds the admission lock).

Size is the byte count of the file you pass, and it only counts as a shipped
measurement for a variant you name with --shipped (an assertion that the file is
the stripped `release`-profile artifact). Use binary_size_report.py for the
authoritative, reproducible size.
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
        "stdev": statistics.stdev(values) if len(values) > 1 else 0.0,
        "min": min(values),
        "max": max(values),
    }


def classify(comm: str, pid: int, root_pid: int) -> str:
    if pid == root_pid:
        return "agent"
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
    totals: dict[str, int] = {"agent": 0, "browser": 0, "other": 0}
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
        "heavy_lock": heavy_lock_held(),
    }


def is_noisy(state: dict) -> bool:
    return (
        state["cargo_processes"] > 0
        or state.get("heavy_lock", False)
        or state["loadavg_1m"] > state["cpus"] * 0.5
    )


def heavy_lock_held() -> bool:
    if not HEAVY_LOCK.exists():
        return False
    import fcntl

    fd = os.open(HEAVY_LOCK, os.O_RDONLY)  # read-only: never truncate the owner's file
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return True
        fcntl.flock(fd, fcntl.LOCK_UN)
    finally:
        os.close(fd)
    return False


def outside_git(path: Path) -> bool:
    probe = path
    while not probe.exists() and probe != probe.parent:
        probe = probe.parent  # nearest existing ancestor decides
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
        try:
            proc = lp.launch_agent(binary, root)
        except OSError:
            return {"ok": False}
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
        for cls in ("agent", "browser", "other")
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
        result["size"] = "BLOCKED (not named --shipped, or a debug path)"
    return result


def measure_variant(name: str, binary: Path, launches: list[dict], shipped_names=()) -> dict:
    good = [launch for launch in launches if launch.get("ok")]
    if not good:
        return {"variant": name, "verdict": "BLOCKED: no launch reached the provider"}
    medians = [per_launch_medians(launch) for launch in good]
    cold = summarize([launch["cold_start_s"] for launch in good])
    agent_rss = summarize([m["agent"] for m in medians])
    tree_rss_summary = summarize([sum(m.values()) for m in medians])
    listeners = any(launch["listeners"] for launch in good)
    size = binary.stat().st_size
    shipped = name in shipped_names and "/debug/" not in str(binary)
    result = {
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
    if len(good) < len(launches):  # a variant that only sometimes works has no verdict
        note = f"BLOCKED: only {len(good)} of {len(launches)} launches reached the provider"
        result["verdicts"] = dict.fromkeys(result["verdicts"], note)
    return result


def render(results: list[dict], first_launch: dict, state: dict, noisy: bool) -> str:
    lines = [f"batch: load {state['loadavg_1m']:.2f} on {state['cpus']} cpus, "
             f"cargo processes {state['cargo_processes']}, {'NOISY' if noisy else 'quiet'}"]
    for r in results:
        first = first_launch[r["variant"]]
        first_text = f"{first:.1f}s (reported, not sampled)" if first >= 0 else "FAILED"
        lines.append(f"[{r['variant']}] first launch {first_text}")
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


def warm_binary(binary: Path) -> float:
    """First launch (timed, reported) plus three discarded warm-ups; -1 if it fails."""
    try:
        first = lp.warm_up(binary)
        for _ in range(3):
            subprocess.run([str(binary), "--version"], capture_output=True, check=False,
                           timeout=lp.FIRST_LAUNCH_TIMEOUT)
        return first
    except (OSError, subprocess.TimeoutExpired):
        return -1.0


def run_batches(variants: dict[str, Path], launches: int, idle_seconds: int, port: int):
    first_launch = {name: warm_binary(binary) for name, binary in variants.items()}
    collected: dict[str, list[dict]] = {name: [] for name in variants}
    states = [machine_state()]
    for _ in range(launches):
        for name, binary in variants.items():
            collected[name].append(one_launch(binary, port, idle_seconds))
        states.append(machine_state())
    return first_launch, collected, states


def exit_code(results: list[dict], noisy: bool) -> int:
    """0 only when every budget passed and nothing is BLOCKED, FAILED or noisy."""
    verdict_text = [str(v) for r in results for v in r.get("verdicts", {}).values()]
    verdict_text += [r["verdict"] for r in results if "verdict" in r]
    unproven = any(v.startswith(("FAIL", "BLOCKED")) for v in verdict_text)
    return 1 if unproven or noisy or not results else 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--variant", action="append", required=True)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--shipped", action="append", default=[],
                        help="variant NAME whose binary is the stripped release-profile artifact")
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
        first_launch, collected, states = run_batches(
            variants, args.launches, args.idle_seconds, server.server_address[1]
        )
    finally:
        server.shutdown()
        server.server_close()
    noisy = any(is_noisy(state) for state in states)
    results = [measure_variant(n, variants[n], collected[n], args.shipped) for n in variants]
    args.out.mkdir(parents=True, exist_ok=True)
    raw = {"results": results, "first_launch_s": first_launch, "machine_states": states,
           "launches": collected}
    (args.out / "raw.json").write_text(json.dumps(raw, indent=2))
    print(render(results, first_launch, states[-1], noisy))
    return exit_code(results, noisy)


if __name__ == "__main__":
    sys.exit(main())
