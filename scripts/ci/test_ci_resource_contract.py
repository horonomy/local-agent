#!/usr/bin/env python3

from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[2]


class CiResourceContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        match = re.search(r"(?ms)^  test-landlock:\n(.*?)(?=^  [a-z][a-z0-9-]*:|\Z)", self.workflow)
        self.assertIsNotNone(match, "Landlock security job must remain present")
        self.landlock = match.group(1)

    def test_landlock_keeps_all_owning_package_test_targets(self) -> None:
        self.assertIn(
            "cargo nextest run --locked -p zeroclaw-runtime --no-default-features "
            "--features sandbox-landlock -- landlock",
            self.landlock,
        )
        self.assertNotIn("--workspace", self.landlock)
        self.assertNotIn("--lib", self.landlock)

    def test_landlock_debug_output_is_bounded_without_incremental_artifacts(self) -> None:
        self.assertIn('CARGO_PROFILE_TEST_DEBUG: "0"', self.landlock)
        self.assertIn("CARGO_INCREMENTAL: 0", self.workflow)

    def test_landlock_job_and_required_gate_are_preserved(self) -> None:
        self.assertIn("name: Test (Landlock)", self.landlock)
        self.assertNotIn("continue-on-error:", self.landlock)
        self.assertNotIn("--no-tests=pass", self.landlock)
        gate = self.workflow.split("\n  gate:\n", 1)[1]
        self.assertIn("test-landlock", gate.split("\n    steps:", 1)[0])

    def test_landlock_remains_on_standard_github_linux(self) -> None:
        self.assertRegex(self.landlock, r"runs-on: ubuntu-(?:24\.04|latest)\n")
        self.assertNotIn("blacksmith", self.landlock)


if __name__ == "__main__":
    unittest.main()
