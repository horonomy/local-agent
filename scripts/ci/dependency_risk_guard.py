#!/usr/bin/env python3
"""Validate the single approved maintenance exception before either scanner."""

from __future__ import annotations

import argparse
from datetime import date, datetime, timedelta, timezone
from pathlib import Path
import sys
import tomllib
from typing import Any

# TOML documents are heterogeneous; validate fields before using their values.
ADVISORY = "RUSTSEC-2026-0441"
SOURCE = "registry+https://github.com/rust-lang/crates.io-index"
IDENTITY = {
    "advisory": ADVISORY,
    "kind": "unmaintained",
    "priority": "P2",
    "crate": "pin-utils",
    "version": "0.1.0",
    "source": SOURCE,
    "checksum": "8b870d8c151b6f2fb93e84a13146138f05d02ed11c7e7c54f8826aaaf7c9f184",
    "parent": "async-imap",
    "parent_version": "0.11.3",
    "parent_source": SOURCE,
    "parent_checksum": "9a6728e0f7931b36d725ac234fcb02539e9f7888dbeaaa8a18d9ea5792181570",
    "owner": "@Chisanan232",
    "tracking": "https://lightning-dust-mite.atlassian.net/browse/HORO-2447",
    "upstream": "https://github.com/async-email/async-imap",
}
LEGACY_DENY = {"RUSTSEC-2025-0134", "RUSTSEC-2026-0319"}
LEGACY_AUDIT = {
    "RUSTSEC-2025-0134",
    "RUSTSEC-2026-0253",
    "RUSTSEC-2024-0429",
    "RUSTSEC-2024-0411",
    "RUSTSEC-2024-0412",
    "RUSTSEC-2024-0413",
    "RUSTSEC-2024-0414",
    "RUSTSEC-2024-0415",
    "RUSTSEC-2024-0416",
    "RUSTSEC-2024-0417",
    "RUSTSEC-2024-0418",
    "RUSTSEC-2024-0419",
    "RUSTSEC-2024-0420",
    "RUSTSEC-2025-0075",
    "RUSTSEC-2025-0080",
    "RUSTSEC-2025-0081",
    "RUSTSEC-2025-0098",
    "RUSTSEC-2025-0100",
    "RUSTSEC-2024-0388",
    "RUSTSEC-2025-0141",
    "RUSTSEC-2024-0384",
}


def validate_documents(
    registry: dict[str, Any],
    deny: dict[str, Any],
    audit: dict[str, Any],
    lock: dict[str, Any],
    *,
    today: date,
) -> list[str]:
    issues: list[str] = []
    entry = registry.get("exception", {})
    if set(registry) != {"exception"}:
        issues.append("unrecognized registry metadata")
    if not isinstance(entry, dict):
        return issues + ["invalid exception registry"]
    expected_keys = set(IDENTITY) | {
        "profiles",
        "justification",
        "controls",
        "retirement",
        "approved",
        "review",
        "expiry",
    }
    if set(entry) != expected_keys:
        issues.append("missing or unrecognized exception metadata")
    for key, expected in IDENTITY.items():
        if entry.get(key) != expected:
            issues.append(f"exception {key} is outside approved scope")
    for key in ("profiles", "justification", "controls", "retirement"):
        if not isinstance(entry.get(key), str) or not entry[key].strip():
            issues.append(f"missing exception {key}")
    approved, review, expiry = (entry.get(k) for k in ("approved", "review", "expiry"))
    if not all(type(value) is date for value in (approved, review, expiry)):
        issues.append("invalid exception dates")
    else:
        if approved != date(
            2026, 11, 6
        ) or not approved <= today < review < expiry <= approved + timedelta(days=30):
            issues.append("exception approval/review/expiry window is invalid")
        if review != date(2026, 11, 20) or today >= review:
            issues.append("exception review is due or outside approved date")
        if expiry != date(2026, 12, 6) or today >= expiry:
            issues.append("exception expired or outside approved expiry")
    for name, policy, inherited in (
        ("deny", deny, LEGACY_DENY),
        ("audit", audit, LEGACY_AUDIT),
    ):
        ignored = policy.get("advisories", {}).get("ignore", [])
        if not isinstance(ignored, list):
            issues.append(f"invalid {name} ignores")
            continue
        ids = [
            item
            if isinstance(item, str)
            else item.get("id")
            if isinstance(item, dict)
            else None
            for item in ignored
        ]
        if ids.count(ADVISORY) != 1:
            issues.append(f"{name} requires exactly one scoped ignore")
        if any(item not in inherited | {ADVISORY} for item in ids):
            issues.append(f"{name} contains an unregistered advisory ignore")
    advisories = deny.get("advisories", {})
    if (
        advisories.get("unmaintained") != "all"
        or advisories.get("yanked") != "deny"
        or advisories.get("unsound", "workspace") not in ("workspace", "all")
    ):
        issues.append("deny advisory detection weakened")
    if (
        advisories.get("db-urls", ["https://github.com/RustSec/advisory-db"])
        != ["https://github.com/RustSec/advisory-db"]
        or "db-path" in advisories
    ):
        issues.append("unexpected deny advisory database configuration")
    packages = lock.get("package", [])
    if not isinstance(packages, list):
        return issues + ["invalid lock packages"]
    for name, version, source, checksum in (
        ("pin-utils", IDENTITY["version"], SOURCE, IDENTITY["checksum"]),
        ("async-imap", IDENTITY["parent_version"], SOURCE, IDENTITY["parent_checksum"]),
    ):
        found = [p for p in packages if p.get("name") == name]
        if len(found) != 1 or any(
            found[0].get(k) != v
            for k, v in (
                ("version", version),
                ("source", source),
                ("checksum", checksum),
            )
        ):
            issues.append(f"unexpected locked {name} version/source/checksum")
    parents = set()
    for package in packages:
        for dependency in package.get("dependencies", []):
            if isinstance(dependency, str) and dependency.split()[0] == "pin-utils":
                if dependency not in (
                    "pin-utils",
                    "pin-utils 0.1.0",
                    f"pin-utils 0.1.0 ({SOURCE})",
                ):
                    issues.append("unexpected pin-utils dependency edge")
                parents.add((package.get("name"), package.get("version")))
    if parents != {("async-imap", "0.11.3")}:
        issues.append("unexpected pin-utils parent set")
    return issues


def validate_advisory(document: dict[str, Any]) -> list[str]:
    advisory = document.get("advisory", {})
    if not isinstance(advisory, dict):
        return ["invalid RustSec advisory"]
    if (
        advisory.get("id") != ADVISORY
        or advisory.get("package") != "pin-utils"
        or advisory.get("informational") != "unmaintained"
    ):
        return ["RustSec advisory is not the approved maintenance-only finding"]
    if any(advisory.get(key) for key in ("cvss", "aliases", "withdrawn", "categories")):
        return ["RustSec security/withdrawal metadata requires reassessment"]
    return []


def validate_database(root: Path) -> tuple[Path, list[str]]:
    databases = [p for p in root.iterdir() if p.is_dir() and (p / ".git").exists()]
    if len(databases) != 1:
        raise ValueError("expected one refreshed Cargo Deny advisory database")
    database = databases[0]
    text = (database / "crates/pin-utils" / f"{ADVISORY}.md").read_text()
    if not text.startswith("```toml\n") or "\n```" not in text[8:]:
        raise ValueError("malformed RustSec advisory record")
    document = tomllib.loads(text[8:].split("\n```", 1)[0])
    return database, validate_advisory(document)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db-root", type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    try:
        documents = [
            tomllib.loads((root / p).read_text())
            for p in (
                ".cargo/dependency-risk.toml",
                "deny.toml",
                ".cargo/audit.toml",
                "Cargo.lock",
            )
        ]
        issues = validate_documents(*documents, today=datetime.now(timezone.utc).date())
        database, advisory_issues = validate_database(args.db_root)
        issues.extend(advisory_issues)
        if issues:
            print("\n".join(issues), file=sys.stderr)
            return 1
        # The caller passes this same validated snapshot to cargo-audit --no-fetch.
        print(database.resolve())
        return 0
    except (OSError, ValueError, TypeError, KeyError) as error:
        print(f"dependency risk guard failed closed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
