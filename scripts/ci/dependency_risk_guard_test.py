from __future__ import annotations

from copy import deepcopy
from datetime import date
from pathlib import Path
import sys
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts" / "ci"))


class DependencyRiskGuardTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.registry = tomllib.loads((ROOT / ".cargo/dependency-risk.toml").read_text())
        cls.deny = tomllib.loads((ROOT / "deny.toml").read_text())
        cls.audit = tomllib.loads((ROOT / ".cargo/audit.toml").read_text())
        cls.lock = tomllib.loads((ROOT / "Cargo.lock").read_text())

    def validate(self, **changes: object) -> list[str]:
        from dependency_risk_guard import validate_documents

        inputs = {
            "registry": deepcopy(self.registry),
            "deny": deepcopy(self.deny),
            "audit": deepcopy(self.audit),
            "lock": deepcopy(self.lock),
            "today": date(2026, 11, 6),
        }
        inputs.update(changes)
        return validate_documents(**inputs)

    def test_current_exception_is_valid(self) -> None:
        self.assertEqual(self.validate(), [])

    def test_audit_policy_without_exception_fails_closed(self) -> None:
        audit = deepcopy(self.audit)
        audit["advisories"]["ignore"].remove("RUSTSEC-2026-0441")
        self.assertTrue(any("audit" in issue for issue in self.validate(audit=audit)))

    def test_review_and_expiry_fail_on_their_utc_date(self) -> None:
        for today, expected in [
            (date(2026, 11, 20), "review"),
            (date(2026, 12, 6), "expired"),
        ]:
            with self.subTest(today=today):
                self.assertTrue(
                    any(expected in issue for issue in self.validate(today=today))
                )

    def test_invalid_dates_or_renewal_beyond_approved_window_fail(self) -> None:
        for field, value in [
            ("approved", date(2026, 11, 7)),
            ("expiry", date(2026, 12, 7)),
            ("review", "invalid"),
        ]:
            registry = deepcopy(self.registry)
            registry["exception"][field] = value
            with self.subTest(field=field):
                self.assertTrue(self.validate(registry=registry))

    def test_missing_owner_justification_or_tracking_fails(self) -> None:
        for field in (
            "owner",
            "justification",
            "tracking",
            "profiles",
            "controls",
            "retirement",
            "upstream",
        ):
            registry = deepcopy(self.registry)
            del registry["exception"][field]
            with self.subTest(field=field):
                self.assertTrue(self.validate(registry=registry))

    def test_unknown_registry_metadata_fails(self) -> None:
        registry = deepcopy(self.registry)
        registry["exception"]["unreviewed_override"] = True
        self.assertTrue(
            any("metadata" in issue for issue in self.validate(registry=registry))
        )
        registry = deepcopy(self.registry)
        registry["other_exception"] = {"advisory": "RUSTSEC-2099-9999"}
        self.assertTrue(
            any("metadata" in issue for issue in self.validate(registry=registry))
        )

    def test_registry_cannot_expand_advisory_or_package_identity(self) -> None:
        for field, value in [
            ("advisory", "RUSTSEC-2099-9999"),
            ("kind", "unsound"),
            ("crate", "other"),
            ("version", "0.2.0"),
            ("source", "git+https://example.invalid"),
            ("checksum", "0" * 64),
            ("parent_version", "0.12.0"),
        ]:
            registry = deepcopy(self.registry)
            registry["exception"][field] = value
            with self.subTest(field=field):
                self.assertTrue(self.validate(registry=registry))

    def test_changed_or_removed_lock_packages_fail(self) -> None:
        for name in ("pin-utils", "async-imap"):
            for field in ("version", "source", "checksum"):
                lock = deepcopy(self.lock)
                package = next(p for p in lock["package"] if p["name"] == name)
                package[field] = "changed"
                with self.subTest(name=name, field=field):
                    self.assertTrue(self.validate(lock=lock))
            lock = deepcopy(self.lock)
            lock["package"] = [p for p in lock["package"] if p["name"] != name]
            self.assertTrue(self.validate(lock=lock))

    def test_added_parent_or_second_package_version_fails(self) -> None:
        lock = deepcopy(self.lock)
        lock["package"].append(
            {
                "name": "unexpected-parent",
                "version": "1.0.0",
                "dependencies": ["pin-utils 0.1.0"],
            }
        )
        self.assertTrue(any("parent" in issue for issue in self.validate(lock=lock)))
        lock = deepcopy(self.lock)
        lock["package"].append({"name": "pin-utils", "version": "0.2.0"})
        self.assertTrue(self.validate(lock=lock))

    def test_exact_ignore_required_once_in_both_scanners(self) -> None:
        for name, policy in [("deny", self.deny), ("audit", self.audit)]:
            for duplicate in (False, True):
                changed = deepcopy(policy)
                ignores = changed["advisories"]["ignore"]
                target = next(
                    x
                    for x in ignores
                    if (x if isinstance(x, str) else x["id"]) == "RUSTSEC-2026-0441"
                )
                ignores.append(target) if duplicate else ignores.remove(target)
                with self.subTest(name=name, duplicate=duplicate):
                    self.assertTrue(self.validate(**{name: changed}))

    def test_new_ignore_or_weakened_detection_fails(self) -> None:
        deny = deepcopy(self.deny)
        deny["advisories"]["ignore"].append(
            {"id": "RUSTSEC-2099-9999", "reason": "temporary"}
        )
        self.assertTrue(self.validate(deny=deny))
        for field, value in [
            ("unmaintained", "none"),
            ("yanked", "allow"),
            ("unsound", "none"),
        ]:
            deny = deepcopy(self.deny)
            deny["advisories"][field] = value
            self.assertTrue(self.validate(deny=deny))

    def test_database_missing_ambiguous_or_reclassified_record_fails(self) -> None:
        from dependency_risk_guard import validate_database

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaises(ValueError):
                validate_database(root)
            database = root / "advisory-db"
            (database / ".git").mkdir(parents=True)
            package = database / "crates/pin-utils"
            package.mkdir(parents=True)
            record = package / "RUSTSEC-2026-0441.md"
            with self.assertRaises(OSError):
                validate_database(root)
            record.write_text(
                '```toml\n[advisory]\nid="RUSTSEC-2026-0441"\npackage="pin-utils"\ninformational="unmaintained"\n```\n'
            )
            self.assertEqual(validate_database(root), (database, []))
            record.write_text(record.read_text().replace('"unmaintained"', '"unsound"'))
            self.assertTrue(validate_database(root)[1])
            record.write_text("not an advisory")
            with self.assertRaises(ValueError):
                validate_database(root)
            (root / "second/.git").mkdir(parents=True)
            with self.assertRaises(ValueError):
                validate_database(root)

    def test_scanners_are_wired_after_guard_without_database_refetch(self) -> None:
        ci = (ROOT / ".github/workflows/ci.yml").read_text()
        security = ci.split("  security:\n", 1)[1].split("\n  path-changes:", 1)[0]
        self.assertLess(
            security.index("cargo deny fetch db"),
            security.index("advisory_db=$(python3 scripts/ci/dependency_risk_guard.py"),
        )
        self.assertLess(
            security.index("advisory_db=$(python3 scripts/ci/dependency_risk_guard.py"),
            security.index("cargo deny --offline --locked check"),
        )
        self.assertIn(
            'cargo audit --no-fetch --db "$LOCAL_AGENT_ADVISORY_DB"', security
        )
        daily = (ROOT / ".github/workflows/daily-audit.yml").read_text()
        self.assertIn(
            'advisory_db=$(python3 scripts/ci/dependency_risk_guard.py --db-root "${CARGO_HOME:-$HOME/.cargo}/advisory-dbs") || exit 1',
            daily,
        )
        self.assertIn("cargo deny --offline --locked check advisories", daily)
        self.assertIn('cargo audit --no-fetch --db "$advisory_db"', daily)
        self.assertLess(
            daily.index("advisory_db=$("), daily.index("cargo deny --offline")
        )

    def test_actual_advisory_must_remain_maintenance_only(self) -> None:
        from dependency_risk_guard import validate_advisory

        advisory = {
            "advisory": {
                "id": "RUSTSEC-2026-0441",
                "package": "pin-utils",
                "informational": "unmaintained",
            }
        }
        self.assertEqual(validate_advisory(advisory), [])
        for field, value in [
            ("id", "RUSTSEC-2099-9999"),
            ("package", "other"),
            ("informational", "unsound"),
            ("cvss", "CVSS:3.1/AV:N"),
            ("withdrawn", "2026-11-07"),
            ("aliases", ["CVE-2099-0001"]),
        ]:
            changed = deepcopy(advisory)
            changed["advisory"][field] = value
            with self.subTest(field=field):
                self.assertTrue(validate_advisory(changed))
        self.assertTrue(
            validate_advisory(
                {"advisory": {"id": "RUSTSEC-2026-0441", "package": "pin-utils"}}
            )
        )


if __name__ == "__main__":
    unittest.main()
