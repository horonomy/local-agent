from __future__ import annotations

import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import fcntl
import json
import os
from pathlib import Path
import tempfile
from typing import Callable, Iterator, TypeVar


T = TypeVar("T")


VERSION = 1
MAX_FAILED_CYCLES = 5
MAX_ACTIVE_SECONDS = 3600


class LedgerError(Exception):
    """A ledger is invalid, incomplete, or cannot safely be updated."""


def _now() -> str:
    return datetime.now(timezone.utc).isoformat()


def _canonical(path: Path) -> Path:
    if not isinstance(path, Path):
        raise TypeError("ledger path must be pathlib.Path")
    return path.resolve()


@contextmanager
def _locked(path: Path) -> Iterator[None]:
    path = _canonical(path)
    lock_path = path.with_name(path.name + ".lock")
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        with lock_path.open("a+b") as lock_file:
            fcntl.flock(lock_file.fileno(), fcntl.LOCK_EX)
            yield
    except OSError as exc:
        raise LedgerError(f"cannot coordinate ledger access: {exc}") from exc


def _read_unlocked(path: Path) -> dict[str, object]:
    if not path.exists():
        return {"version": VERSION, "problems": {}}
    try:
        raw = path.read_text(encoding="utf-8")
        document = json.loads(raw)
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise LedgerError(f"cannot read valid ledger: {exc}") from exc
    _validate(document)
    return document


def _validate(document: object) -> None:
    if not isinstance(document, dict) or set(document) != {"version", "problems"}:
        raise LedgerError("ledger shape is unknown")
    if type(document["version"]) is not int or document["version"] != VERSION:
        raise LedgerError("ledger version is unknown")
    problems = document["problems"]
    if not isinstance(problems, dict):
        raise LedgerError("ledger problems are malformed")
    required = {
        "problem_id",
        "ticket",
        "first_observation",
        "first_observed_at",
        "hypotheses",
        "results",
        "release_impact",
        "disposition",
        "failed_cycles",
        "active_seconds",
        "circuit_open",
        "recovered",
        "events",
    }
    for key, record in problems.items():
        if (
            not isinstance(key, str)
            or not isinstance(record, dict)
            or set(record) != required
        ):
            raise LedgerError(
                "problem record is malformed or has unknown historical fields"
            )
        for field in (
            "problem_id",
            "ticket",
            "first_observation",
            "first_observed_at",
            "release_impact",
        ):
            if not isinstance(record[field], str) or not record[field].strip():
                raise LedgerError(f"problem field {field} is unknown")
        if record["problem_id"] != key or record["disposition"] not in ("A", "B"):
            raise LedgerError("problem identity or disposition is invalid")
        if not isinstance(record["hypotheses"], list) or not all(
            isinstance(x, str) for x in record["hypotheses"]
        ):
            raise LedgerError("problem hypotheses are malformed")
        if not isinstance(record["results"], list) or not all(
            isinstance(x, str) for x in record["results"]
        ):
            raise LedgerError("problem results are malformed")
        if type(record["failed_cycles"]) is not int or record["failed_cycles"] < 0:
            raise LedgerError("failed-cycle budget is invalid or unknown")
        if type(record["active_seconds"]) is not int or record["active_seconds"] < 0:
            raise LedgerError("active-time budget is invalid or unknown")
        if record["failed_cycles"] > MAX_FAILED_CYCLES:
            raise LedgerError("failed-cycle budget exceeds its known maximum")
        if (
            type(record["circuit_open"]) is not bool
            or type(record["recovered"]) is not bool
        ):
            raise LedgerError("problem circuit state is malformed")
        if not isinstance(record["events"], list):
            raise LedgerError("problem event history is malformed")
        counted_cycles = 0
        counted_seconds = 0
        recovered_in_history = False
        recovered_for_resolution = False
        circuit_open_in_history = False
        resolution_active = False
        for event in record["events"]:
            if not isinstance(event, dict):
                raise LedgerError("problem event history is malformed")
            allowed_event_fields = {
                "type",
                "at",
                "observation",
                "result",
                "failed_cycle",
                "active_seconds",
                "recovered",
                "description",
                "availability",
                "hypotheses",
                "release_impact",
                "disposition",
                "owner_decision",
                "verification",
            }
            if set(event) - allowed_event_fields:
                raise LedgerError("event history contains unknown fields")
            if not isinstance(event.get("type"), str) or not isinstance(
                event.get("at"), str
            ):
                raise LedgerError("problem event history is malformed")
            if "failed_cycle" in event and type(event["failed_cycle"]) is not bool:
                raise LedgerError("event cycle accounting is malformed")
            if "recovered" in event and type(event["recovered"]) is not bool:
                raise LedgerError("event recovery state is malformed")
            if event["type"] == "result" and "result" not in event:
                raise LedgerError("result history is incomplete")
            if event["type"] == "probe" and "description" not in event:
                raise LedgerError("probe history is incomplete")
            seconds = event.get("active_seconds", 0)
            if type(seconds) is not int or seconds < 0:
                raise LedgerError("event active-time accounting is malformed")
            failed = event.get("failed_cycle") is True
            if circuit_open_in_history and (failed or seconds > 0):
                raise LedgerError("event history exceeds an open problem budget")
            if event["type"] == "resolution":
                if (
                    set(event) != {"type", "at", "owner_decision", "verification"}
                    or not event["at"].strip()
                    or not isinstance(event["owner_decision"], str)
                    or not event["owner_decision"].strip()
                    or not isinstance(event["verification"], str)
                    or not event["verification"].strip()
                    or not recovered_for_resolution
                    or not circuit_open_in_history
                ):
                    raise LedgerError("problem resolution history is incomplete")
                resolution_active = True
            if failed:
                counted_cycles += 1
                recovered_in_history = False
                recovered_for_resolution = False
                resolution_active = False
            counted_seconds += seconds
            opens_now = not circuit_open_in_history and (
                counted_cycles >= MAX_FAILED_CYCLES
                or counted_seconds >= MAX_ACTIVE_SECONDS
            )
            if event["type"] == "circuit_opened" or opens_now:
                circuit_open_in_history = True
                recovered_for_resolution = False
                resolution_active = False
            if event["type"] == "result" and event.get("recovered") is True:
                if failed:
                    raise LedgerError("a failed cycle cannot also verify recovery")
                recovered_in_history = True
                recovered_for_resolution = True
        if (
            counted_cycles != record["failed_cycles"]
            or counted_seconds != record["active_seconds"]
        ):
            raise LedgerError("problem budget does not match its event history")
        if record["recovered"] != recovered_in_history:
            raise LedgerError("problem recovery state does not match its event history")
        if circuit_open_in_history and not record["circuit_open"]:
            raise LedgerError("problem circuit state does not match its event history")
        if resolution_active and not record["recovered"]:
            raise LedgerError("problem resolution state is inconsistent")


def load(path: Path) -> dict[str, object]:
    path = _canonical(path)
    with _locked(path):
        return _read_unlocked(path)


def _write_unlocked(path: Path, document: dict[str, object]) -> None:
    path = _canonical(path)
    _validate(document)
    payload = (json.dumps(document, indent=2, sort_keys=True) + "\n").encode("utf-8")
    temp_name: str | None = None
    try:
        with tempfile.NamedTemporaryFile(
            dir=path.parent, prefix=path.name + ".", delete=False
        ) as temp:
            temp_name = temp.name
            temp.write(payload)
            temp.flush()
            os.fsync(temp.fileno())
        os.replace(temp_name, path)
        directory_fd = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    except OSError as exc:
        if temp_name is not None:
            try:
                os.unlink(temp_name)
            except OSError:
                pass
        raise LedgerError(f"cannot atomically save ledger: {exc}") from exc


def _mutate(path: Path, operation: Callable[[dict[str, object]], T]) -> T:
    path = _canonical(path)
    with _locked(path):
        document = _read_unlocked(path)
        result = operation(document)
        _write_unlocked(path, document)
        return result


def create_problem(
    path: Path,
    *,
    problem_id: str,
    ticket: str,
    observation: str,
    hypotheses: list[str],
    release_impact: str,
    disposition: str,
) -> dict[str, object]:
    values = (problem_id, ticket, observation, release_impact)
    if any(not isinstance(value, str) or not value.strip() for value in values):
        raise ValueError(
            "problem ID, ticket, observation, and release impact are required"
        )
    if (
        not isinstance(hypotheses, list)
        or disposition not in ("A", "B")
        or not all(isinstance(item, str) for item in hypotheses)
    ):
        raise ValueError("disposition must be A or B and hypotheses must be text")

    def create(document: dict[str, object]) -> dict[str, object]:
        problems = document["problems"]
        assert isinstance(problems, dict)
        if problem_id in problems:
            raise LedgerError(
                "problem ID already exists; identity and history are immutable"
            )
        record: dict[str, object] = {
            "problem_id": problem_id,
            "ticket": ticket,
            "first_observation": observation,
            "first_observed_at": _now(),
            "hypotheses": hypotheses,
            "results": [],
            "release_impact": release_impact,
            "disposition": disposition,
            "failed_cycles": 0,
            "active_seconds": 0,
            "circuit_open": False,
            "recovered": False,
            "events": [{"type": "observed", "at": _now(), "observation": observation}],
        }
        problems[problem_id] = record
        return record

    return _mutate(path, create)


def _record(
    path: Path, problem_id: str, event: dict[str, object], result: str | None = None
) -> None:
    def append(document: dict[str, object]) -> None:
        problems = document["problems"]
        assert isinstance(problems, dict)
        record = problems.get(problem_id)
        if not isinstance(record, dict):
            raise LedgerError("unknown problem; refusing to infer a fresh budget")
        events = record["events"]
        results = record["results"]
        assert isinstance(events, list) and isinstance(results, list)
        seconds = event.get("active_seconds", 0)
        if type(seconds) is not int or seconds < 0:
            raise ValueError("active seconds must be a non-negative integer")
        opened = (
            record["circuit_open"]
            or record["failed_cycles"] >= MAX_FAILED_CYCLES
            or record["active_seconds"] >= MAX_ACTIVE_SECONDS
        )
        if opened and (event.get("failed_cycle") is True or seconds > 0):
            raise LedgerError("problem budget exhausted; remediation is blocked")
        events.append(event)
        if result is not None:
            results.append(result)
        if event.get("failed_cycle") is True:
            record["failed_cycles"] += 1
            record["recovered"] = False
        record["active_seconds"] += seconds
        if (
            record["failed_cycles"] >= MAX_FAILED_CYCLES
            or record["active_seconds"] >= MAX_ACTIVE_SECONDS
        ):
            record["circuit_open"] = True
        if event.get("recovered") is True:
            record["recovered"] = True

    _mutate(path, append)


def update_problem(
    path: Path,
    problem_id: str,
    *,
    hypotheses: list[str] | None = None,
    release_impact: str | None = None,
    disposition: str | None = None,
) -> None:
    if hypotheses is not None and (
        not isinstance(hypotheses, list)
        or not all(isinstance(item, str) for item in hypotheses)
    ):
        raise ValueError("hypotheses must be a list of text")
    if release_impact is not None and (
        not isinstance(release_impact, str) or not release_impact.strip()
    ):
        raise ValueError("release impact must be non-empty text")
    if disposition is not None and (
        type(disposition) is not str or disposition not in ("A", "B")
    ):
        raise ValueError("disposition must be A or B")

    def update(document: dict[str, object]) -> None:
        problems = document["problems"]
        assert isinstance(problems, dict)
        record = problems.get(problem_id)
        if not isinstance(record, dict):
            raise LedgerError("unknown problem; refusing to infer a fresh budget")
        if hypotheses is not None:
            record["hypotheses"] = hypotheses
        if release_impact is not None:
            record["release_impact"] = release_impact
        if disposition is not None:
            record["disposition"] = disposition
        events = record["events"]
        assert isinstance(events, list)
        events.append(
            {
                "type": "problem_updated",
                "at": _now(),
                "hypotheses": hypotheses,
                "release_impact": release_impact,
                "disposition": disposition,
            }
        )

    _mutate(path, update)


def record_result(
    path: Path,
    problem_id: str,
    result: str,
    *,
    failed_cycle: bool = False,
    active_seconds: int = 0,
    recovered: bool = False,
) -> None:
    if (
        not isinstance(result, str)
        or not result.strip()
        or type(failed_cycle) is not bool
        or type(recovered) is not bool
    ):
        raise ValueError("result and boolean cycle/recovery flags are required")
    if failed_cycle and recovered:
        raise ValueError("a failed cycle cannot also verify recovery")
    if type(active_seconds) is not int or active_seconds < 0:
        raise ValueError("active seconds must be a non-negative integer")
    _record(
        path,
        problem_id,
        {
            "type": "result",
            "at": _now(),
            "result": result,
            "failed_cycle": failed_cycle,
            "active_seconds": active_seconds,
            "recovered": recovered,
        },
        result,
    )


def record_probe(
    path: Path,
    problem_id: str,
    description: str,
    *,
    availability: bool = False,
    active_seconds: int = 0,
) -> None:
    if not isinstance(description, str) or not description.strip():
        raise ValueError("probe description is required")
    if not isinstance(availability, bool):
        raise ValueError("availability must be boolean")
    _record(
        path,
        problem_id,
        {
            "type": "probe",
            "at": _now(),
            "description": description,
            "availability": availability,
            "active_seconds": active_seconds,
        },
    )


def resolve_problem(
    path: Path,
    problem_id: str,
    *,
    owner_decision: str,
    verification: str,
) -> None:
    if (
        not isinstance(owner_decision, str)
        or not owner_decision.strip()
        or not isinstance(verification, str)
        or not verification.strip()
    ):
        raise ValueError("owner decision and verification must be non-empty text")

    def resolve(document: dict[str, object]) -> None:
        problems = document["problems"]
        assert isinstance(problems, dict)
        record = problems.get(problem_id)
        if not isinstance(record, dict):
            raise LedgerError("unknown problem; refusing to resolve an unknown circuit")
        if not record["recovered"]:
            raise LedgerError("problem must have verified recovery before resolution")
        if not (
            record["circuit_open"]
            or record["failed_cycles"] >= MAX_FAILED_CYCLES
            or record["active_seconds"] >= MAX_ACTIVE_SECONDS
        ):
            raise LedgerError("problem resolution requires an exhausted circuit")
        events = record["events"]
        assert isinstance(events, list)
        events.append(
            {
                "type": "resolution",
                "at": _now(),
                "owner_decision": owner_decision,
                "verification": verification,
            }
        )

    _mutate(path, resolve)


def _is_resolved(record: dict[str, object]) -> bool:
    events = record["events"]
    assert isinstance(events, list)
    for event in reversed(events):
        if event.get("type") == "circuit_opened":
            return False
        if event.get("type") == "resolution":
            return True
        if event.get("type") == "result" and event.get("failed_cycle") is True:
            return False
    return False


def check(path: Path, problem_id: str, *, dependent: bool) -> bool:
    if type(dependent) is not bool:
        raise ValueError("dependent must be boolean")
    document = load(path)
    problems = document["problems"]
    assert isinstance(problems, dict)
    record = problems.get(problem_id)
    if not isinstance(record, dict):
        raise LedgerError("unknown problem; refusing to infer a fresh budget")
    opened = (
        record["circuit_open"]
        or record["failed_cycles"] >= MAX_FAILED_CYCLES
        or record["active_seconds"] >= MAX_ACTIVE_SECONDS
    )
    if not opened:
        return True
    if dependent:
        return _is_resolved(record)
    return record["disposition"] == "B"


def open_circuit(path: Path, problem_id: str) -> None:
    def update(document: dict[str, object]) -> None:
        problems = document["problems"]
        assert isinstance(problems, dict)
        record = problems.get(problem_id)
        if not isinstance(record, dict):
            raise LedgerError("unknown problem; refusing to create a circuit state")
        record["circuit_open"] = True
        events = record["events"]
        assert isinstance(events, list)
        events.append({"type": "circuit_opened", "at": _now()})

    _mutate(path, update)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Explicit local problem-ledger accounting and dependency check"
    )
    commands = parser.add_subparsers(dest="command", required=True)
    create_parser = commands.add_parser("create", help="create a stable problem record")
    create_parser.add_argument("ledger", type=Path)
    create_parser.add_argument("problem_id")
    create_parser.add_argument("ticket")
    create_parser.add_argument("observation")
    create_parser.add_argument("release_impact")
    create_parser.add_argument("--disposition", choices=("A", "B"), required=True)
    create_parser.add_argument("--hypothesis", action="append", default=[])
    result_parser = commands.add_parser(
        "result", help="record a result and explicit budget"
    )
    result_parser.add_argument("ledger", type=Path)
    result_parser.add_argument("problem_id")
    result_parser.add_argument("result")
    result_parser.add_argument("--failed-cycle", action="store_true")
    result_parser.add_argument("--active-seconds", type=int, default=0)
    result_parser.add_argument("--recovered", action="store_true")
    probe_parser = commands.add_parser(
        "probe", help="record diagnosis with explicit active-time accounting"
    )
    probe_parser.add_argument("ledger", type=Path)
    probe_parser.add_argument("problem_id")
    probe_parser.add_argument("description")
    probe_parser.add_argument("--availability", action="store_true")
    probe_parser.add_argument("--active-seconds", type=int, default=0)
    resolve_parser = commands.add_parser(
        "resolve", help="record an approved resolution after verified recovery"
    )
    resolve_parser.add_argument("ledger", type=Path)
    resolve_parser.add_argument("problem_id")
    resolve_parser.add_argument("--owner-decision", required=True)
    resolve_parser.add_argument("--verification", required=True)
    check_parser = commands.add_parser("check", help="check whether work may proceed")
    check_parser.add_argument("ledger", type=Path)
    check_parser.add_argument("problem_id")
    group = check_parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--dependent", action="store_true")
    group.add_argument("--independent", action="store_true")
    args = parser.parse_args()
    try:
        if args.command == "create":
            create_problem(
                args.ledger,
                problem_id=args.problem_id,
                ticket=args.ticket,
                observation=args.observation,
                hypotheses=args.hypothesis,
                release_impact=args.release_impact,
                disposition=args.disposition,
            )
            return 0
        if args.command == "result":
            record_result(
                args.ledger,
                args.problem_id,
                args.result,
                failed_cycle=args.failed_cycle,
                active_seconds=args.active_seconds,
                recovered=args.recovered,
            )
            return 0
        if args.command == "probe":
            record_probe(
                args.ledger,
                args.problem_id,
                description=args.description,
                availability=args.availability,
                active_seconds=args.active_seconds,
            )
            return 0
        if args.command == "resolve":
            resolve_problem(
                args.ledger,
                args.problem_id,
                owner_decision=args.owner_decision,
                verification=args.verification,
            )
            return 0
        allowed = check(args.ledger, args.problem_id, dependent=args.dependent)
        print("ALLOWED" if allowed else "BLOCKED")
        return 0 if allowed else 3
    except (LedgerError, ValueError) as exc:
        parser.exit(2, f"problem ledger error: {exc}\n")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
