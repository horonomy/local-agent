#!/usr/bin/env python3

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
GATE = ROOT / "scripts/ci/docs_quality_gate.sh"


class DocsQualityGateTests(unittest.TestCase):
    def run_gate(self, content: str) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory(prefix="local-agent-docs-gate-") as directory:
            root = Path(directory)
            page = root / "page.md"
            page.write_text(content, encoding="utf-8")
            env = os.environ.copy()
            env.update(BASE_SHA="", DOCS_FILES=str(page))
            return subprocess.run(
                ["bash", str(GATE)],
                cwd=root,
                env=env,
                capture_output=True,
                text=True,
                timeout=10,
                check=False,
            )

    def test_gate_has_no_third_party_formatter_execution(self) -> None:
        source = GATE.read_text(encoding="utf-8")
        self.assertNotIn("MD_CMD=", source)
        self.assertNotIn("npx --yes", source)
        self.assertNotIn("markdownlint-cli2@", source)

    def test_authored_document_passes_remaining_local_check(self) -> None:
        result = self.run_gate("# Local Agent\n\nUse approved local IPC.\n")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("No prose em-dashes", result.stdout)

    def test_existing_prose_check_still_rejects_its_violation(self) -> None:
        result = self.run_gate("# Local Agent\n\nUnapproved prose — punctuation.\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Blocking prose em-dashes: 1", result.stdout)

    def test_literal_code_remains_accepted(self) -> None:
        result = self.run_gate("# Reference\n\n`literal — text`\n\n```text\ncode — text\n```\n")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_multibacktick_code_spans_remain_accepted(self) -> None:
        result = self.run_gate("# Reference\n\n``literal — text``\n")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_ci_keeps_link_and_integrity_validation(self) -> None:
        source = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        for command in [
            "bash scripts/ci/docs_quality_gate.sh",
            "python3 scripts/ci/docs_quality_gate_test.py",
            "bash scripts/ci/docs_links_gate.sh",
            "python3 scripts/ci/check_internal_docs_links.py",
            "python3 scripts/ci/check_internal_docs_links_test.py",
        ]:
            self.assertIn(command, source)
        self.assertIn("docs-style", source)


if __name__ == "__main__":
    unittest.main()
