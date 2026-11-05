#!/usr/bin/env python3

from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[2]


class DocsPublicationContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.workflow = (ROOT / ".github/workflows/docs-deploy.yml").read_text(encoding="utf-8")

    def test_workflow_has_read_only_repository_permission(self) -> None:
        self.assertIn("permissions:\n  contents: read\n", self.workflow)
        self.assertNotIn("contents: write", self.workflow)

    def test_real_documentation_build_is_preserved(self) -> None:
        self.assertIn("cargo mdbook build", self.workflow)
        self.assertIn("Build docs (refs + rustdoc + all locales)", self.workflow)
        self.assertIn("if: github.event_name != 'workflow_dispatch' || inputs.mode == 'build'", self.workflow)

    def test_inherited_pages_publication_is_explicitly_disabled(self) -> None:
        match = re.search(
            r"(?ms)^      - name: Update gh-pages branch[^\n]*\n(.*?)(?=^      - name:|^  [a-z]|\Z)",
            self.workflow,
        )
        self.assertIsNotNone(match)
        self.assertIn("if: github.repository == 'zeroclaw-labs/zeroclaw'", match.group(1))

    def test_stable_promotion_is_explicitly_disabled(self) -> None:
        promotion = self.workflow.split("\n  promote-stable:\n", 1)[1]
        self.assertIn("if: github.repository == 'zeroclaw-labs/zeroclaw'", promotion)


if __name__ == "__main__":
    unittest.main()
