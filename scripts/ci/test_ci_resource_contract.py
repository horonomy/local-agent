#!/usr/bin/env python3

from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[2]


class CiResourceContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        match = re.search(
            r"(?ms)^  test-landlock:\n(.*?)(?=^  [a-z][a-z0-9-]*:|\Z)", self.workflow
        )
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

    def test_landlock_debug_output_is_bounded_without_incremental_artifacts(
        self,
    ) -> None:
        self.assertIn('CARGO_PROFILE_TEST_DEBUG: "0"', self.landlock)
        self.assertIn("CARGO_INCREMENTAL: 0", self.workflow)

    def test_landlock_job_and_required_gate_are_preserved(self) -> None:
        self.assertIn("name: Test (Landlock)", self.landlock)
        self.assertNotIn("continue-on-error:", self.landlock)
        self.assertNotIn("--no-tests=pass", self.landlock)
        gate = self.workflow.split("\n  gate:\n", 1)[1]
        self.assertIn("test-landlock", gate.split("\n    steps:", 1)[0])

    def test_native_gnu_matrix_declares_shared_cross_binding(self) -> None:
        manual = (ROOT / ".github/workflows/cross-platform-build-manual.yml").read_text(
            encoding="utf-8"
        )
        matrix = manual.split("\n  stable-gnu-build:\n", 1)[1].split("\n    steps:", 1)[
            0
        ]
        self.assertEqual(matrix.count("            use_cross: false\n"), 4)
        self.assertNotIn("use_cross: true", matrix)
        caller = (
            manual.split("\n  stable-gnu-build:\n", 1)[1]
            .split("      - name: Build release\n", 1)[1]
            .split("      - name:", 1)[0]
        )
        self.assertIn("uses: ./.github/actions/build-release-payloads", caller)
        self.assertIn("use_cross: ${{ matrix.use_cross || 'false' }}", caller)

    def test_plugin_backend_builds_bound_debug_and_parallel_artifacts(self) -> None:
        backend = self.workflow.split("\n  check-plugin-backends:\n", 1)[1].split(
            "\n  check-32bit:\n", 1
        )[0]
        for setting in [
            'CARGO_PROFILE_DEV_DEBUG: "0"',
            'CARGO_PROFILE_TEST_DEBUG: "0"',
            'CARGO_BUILD_JOBS: "2"',
        ]:
            self.assertIn(setting, backend)
        self.assertIn("Run plugin component tests", backend)
        self.assertIn("Run channel egress boundary e2e", backend)
        self.assertNotIn("continue-on-error:", backend)

    def test_plugin_cold_build_budgets_are_scoped_to_serial_cranelift_suites(
        self,
    ) -> None:
        backend = self.workflow.split("\n  check-plugin-backends:\n", 1)[1].split(
            "\n    steps:", 1
        )[0]
        self.assertIn("timeout-minutes: ${{ matrix.timeout-minutes || 20 }}", backend)
        rows = dict(
            re.findall(
                r"(?ms)^          - name: (.*?)\n(.*?)(?=^          - name:|\Z)",
                backend,
            )
        )
        self.assertEqual(len(rows), 5)
        for name, budget in [
            ("plugins cranelift backend", 30),
            ("plugins cranelift runtime", 40),
        ]:
            self.assertIn(f"            timeout-minutes: {budget}\n", rows[name])
        for name in [
            "plugins pulley backend",
            "plugins runtime-only backend",
            "plugin artifact smoke",
        ]:
            self.assertNotIn("timeout-minutes:", rows[name])

    def test_cli_tests_bound_artifacts_without_changing_symbol_ratchet(self) -> None:
        cli = self.workflow.split("\n  test-cli-only:\n", 1)[1].split(
            "\n  memory-postgres-test:\n", 1
        )[0]
        self.assertIn('CARGO_PROFILE_TEST_DEBUG: "0"', cli)
        self.assertIn('CARGO_BUILD_JOBS: "2"', cli)
        self.assertNotIn("CARGO_PROFILE_DEV_DEBUG", cli)
        self.assertIn("timeout-minutes: 60", cli)
        self.assertIn(
            "cargo nextest run --locked -p zeroclaw --no-default-features --features "
            '"${{ steps.features.outputs.list }}" --lib --bins --test component '
            "--test integration --test system --test architecture",
            cli,
        )
        self.assertIn(
            "cargo build --locked -p zeroclaw --bin zeroclaw --no-default-features "
            '--features "${{ steps.features.outputs.list }}"',
            cli,
        )
        self.assertIn(
            "bash scripts/ci/cli_only_ratchet.sh run "
            "x86_64-unknown-linux-gnu target/debug/zeroclaw",
            cli,
        )
        self.assertNotIn("continue-on-error:", cli)

    def test_landlock_remains_on_standard_github_linux(self) -> None:
        self.assertRegex(self.landlock, r"runs-on: ubuntu-(?:24\.04|latest)\n")
        self.assertNotIn("blacksmith", self.landlock)


if __name__ == "__main__":
    unittest.main()
