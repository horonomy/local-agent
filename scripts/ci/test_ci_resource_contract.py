#!/usr/bin/env python3

from pathlib import Path
import re
import subprocess
import tempfile
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

    def test_opt_in_tools_bound_test_debug_without_changing_feature_proofs(
        self,
    ) -> None:
        tools = self.workflow.split("\n  test-tool-features:\n", 1)[1].split(
            "\n  test-cli-only:\n", 1
        )[0]
        self.assertIn('CARGO_PROFILE_TEST_DEBUG: "0"', tools)
        self.assertIn("timeout-minutes: 40", tools)
        self.assertNotIn("CARGO_BUILD_JOBS", tools)
        self.assertNotIn("CARGO_PROFILE_DEV_DEBUG", tools)
        self.assertIn("cargo nextest run --locked --no-fail-fast", tools)
        self.assertIn("--features tools-compat", tools)
        self.assertIn("--lib --test integration", tools)
        self.assertIn(
            "--features zeroclaw-config/tool-notion --lib --run-ignored only", tools
        )
        self.assertIn("config_only_notion_feature_does_not_compile_owner", tools)
        self.assertNotIn("continue-on-error:", tools)

    def test_powershell_preflight_reports_runtime_failure_without_hiding_tests(
        self,
    ) -> None:
        section = self.workflow.split("\n  test:\n", 1)[1].split(
            "\n  test-channel-features:\n", 1
        )[0]
        self.assertIn("bash scripts/ci/powershell_preflight.sh", section)
        self.assertLess(
            section.index("bash scripts/ci/powershell_preflight.sh"),
            section.index(
                "cargo nextest run --locked --workspace --exclude zeroclaw-desktop"
            ),
        )
        with tempfile.TemporaryDirectory() as directory:
            interpreter = Path(directory) / "pwsh"
            interpreter.write_text(
                "#!/bin/sh\nprintf 'runtime-unhealthy\\n' >&2\nexit 23\n"
            )
            interpreter.chmod(0o700)
            result = subprocess.run(
                [
                    "bash",
                    str(ROOT / "scripts/ci/powershell_preflight.sh"),
                    str(interpreter),
                ],
                capture_output=True,
                text=True,
                timeout=5,
            )
            self.assertEqual(result.returncode, 23)
            self.assertIn("runtime-unhealthy", result.stderr)
            interpreter.write_text(
                '#!/bin/sh\ncase "$*" in *System.Collections.Specialized.NameValueCollection*) exit 0;; *) exit 9;; esac\n'
            )
            result = subprocess.run(
                [
                    "bash",
                    str(ROOT / "scripts/ci/powershell_preflight.sh"),
                    str(interpreter),
                ],
                capture_output=True,
                text=True,
                timeout=5,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            missing = subprocess.run(
                [
                    "bash",
                    str(ROOT / "scripts/ci/powershell_preflight.sh"),
                    str(interpreter.with_name("missing")),
                ],
                capture_output=True,
                text=True,
                timeout=5,
            )
            self.assertNotEqual(missing.returncode, 0)

    def test_platform_tests_install_nextest_in_existing_cargo_bin(self) -> None:
        workflow = (ROOT / ".github/workflows/platform-tests.yml").read_text(
            encoding="utf-8"
        )
        install = workflow.split("      - name: Install cargo-nextest\n", 1)[1].split(
            "      - name: Run tests\n", 1
        )[0]
        self.assertLess(
            install.index('mkdir -p "$HOME/.cargo/bin"'), install.index("curl -LsSf")
        )
        self.assertIn(
            'curl -LsSf "https://get.nexte.st/latest/${{ matrix.nextest-platform }}" '
            '| tar zxf - -C "$HOME/.cargo/bin"',
            install,
        )
        self.assertIn(
            "cargo nextest run --locked --workspace --exclude zeroclaw-desktop --no-fail-fast",
            workflow,
        )
        self.assertIn("          - name: macOS\n", workflow)
        self.assertIn("          - name: Windows\n", workflow)

    def test_platform_tests_bound_macos_debug_without_reducing_coverage(self) -> None:
        workflow = (ROOT / ".github/workflows/platform-tests.yml").read_text(
            encoding="utf-8"
        )
        platform = workflow.split("\n  platform-tests:\n", 1)[1]
        header, steps = platform.split("\n    steps:\n", 1)
        self.assertIn(
            "CARGO_PROFILE_TEST_DEBUG: ${{ matrix.name == 'macOS' && '0' || '2' }}",
            header,
        )
        self.assertIn("            timeout-minutes: 45\n", header)
        self.assertIn("            timeout-minutes: 60\n", header)
        self.assertIn("fail-fast: false", header)
        self.assertEqual(
            header.count("continue-on-error:"),
            1,
        )
        self.assertIn("continue-on-error: ${{ matrix.name == 'Windows' }}", header)
        self.assertIn("          shared-key: platform-test\n", steps)
        self.assertIn("          cache-on-failure: true\n", steps)
        self.assertIn(
            "          save-if: ${{ github.ref == 'refs/heads/master' }}", steps
        )
        run_tests = steps.split("      - name: Run tests\n", 1)[1]
        self.assertEqual(
            run_tests.strip(),
            "shell: bash\n"
            "        run: cargo nextest run --locked --workspace "
            "--exclude zeroclaw-desktop --no-fail-fast",
        )
        self.assertNotIn("CARGO_PROFILE_TEST_DEBUG_ASSERTIONS", workflow)
        self.assertNotIn("CARGO_PROFILE_TEST_OPT_LEVEL", workflow)

    def test_advisory_windows_bounds_debug_without_changing_native_phases(self) -> None:
        workflow = (ROOT / ".github/workflows/windows-tests.yml").read_text(
            encoding="utf-8"
        )
        native = workflow.split("\n  windows-test:\n", 1)[1]
        header, steps = native.split("\n    steps:\n", 1)
        self.assertIn('CARGO_PROFILE_TEST_DEBUG: "0"', header)
        self.assertIn("timeout-minutes: 90", header)
        self.assertIn("          toolchain: 1.96.1\n", steps)
        self.assertIn("          save-if: false\n", steps)
        self.assertEqual(steps.count("cargo nextest run --locked --no-fail-fast"), 11)
        for phase in (
            "baseline",
            "hardware",
            "plugin_components",
            "plugin_lib",
            "plugin_runtime_config",
            "plugin_runtime_admission",
            "plugin_gateway",
            "plugin_cli",
            "plugin_update_cli",
            "plugin_root",
        ):
            self.assertIn(f"{phase}_status=${{PIPESTATUS[0]}}", steps)
        self.assertIn('exit "$overall_status"', steps)
        self.assertNotIn("CARGO_PROFILE_TEST_DEBUG_ASSERTIONS", workflow)
        self.assertNotIn("CARGO_PROFILE_TEST_OPT_LEVEL", workflow)

    def test_landlock_remains_on_standard_github_linux(self) -> None:

        self.assertRegex(self.landlock, r"runs-on: ubuntu-(?:24\.04|latest)\n")
        self.assertNotIn("blacksmith", self.landlock)


if __name__ == "__main__":
    unittest.main()
