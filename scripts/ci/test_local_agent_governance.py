#!/usr/bin/env python3
"""Focused Local Agent ownership and review-policy regression checks."""

from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[2]


class GovernanceContractTests(unittest.TestCase):
    def test_codeowners_routes_every_path_to_current_maintainer(self) -> None:
        lines = (ROOT / ".github/CODEOWNERS").read_text().splitlines()
        rules = [
            line.split()
            for line in lines
            if line.strip() and not line.lstrip().startswith("#")
        ]
        self.assertEqual(rules, [["*", "@Chisanan232"]])

    def test_current_governance_distinguishes_owner_review_from_human_approval(
        self,
    ) -> None:
        lifecycle = (ROOT / "docs/local-agent/engineering-lifecycle.md").read_text()
        for phrase in [
            "single-maintainer",
            "@Chisanan232",
            "independent technical review",
            "not a GitHub human approval",
            "Do not bypass an active requirement",
            "exact candidate",
        ]:
            self.assertIn(phrase, lifecycle)
        self.assertNotIn("unavailable upstream people", lifecycle)
        self.assertRegex(lifecycle, r"Claude Code\s+or Codex")
        self.assertNotRegex(lifecycle, r"Claude(?: Code)?\s+and Codex")
        self.assertNotIn("CI and human/code-owner gates", lifecycle)

    def test_bootstrap_and_contribution_route_to_single_maintainer_contract(
        self,
    ) -> None:
        for name in ("AGENTS.md", "CLAUDE.md", "CONTRIBUTING.md"):
            with self.subTest(path=name):
                source = (ROOT / name).read_text()
                self.assertIn("single-maintainer", source)
                self.assertIn("engineering-lifecycle.md", source)

    def test_review_and_merge_skill_entrypoints_do_not_reinstate_upstream_governance(
        self,
    ) -> None:
        for name in (
            "github-pr",
            "github-pr-review-session",
            "squash-merge",
            "pr-architecture-check",
        ):
            with self.subTest(skill=name):
                source = (ROOT / f".claude/skills/{name}/SKILL.md").read_text()
                entry = source.split("## Instructions", 1)[0]
                self.assertIn("Local Agent execution scope", entry)
                self.assertIn("single-maintainer", entry)
                self.assertIn("horonomy/local-agent", entry)

    def test_template_uses_current_base_and_truthful_review_evidence(self) -> None:
        source = (ROOT / ".github/pull_request_template.md").read_text()
        self.assertIn("release/mvp-X.0", source)
        self.assertIn("independent technical review", source)
        self.assertIn("not a GitHub human approval", source)
        self.assertNotIn("`master` (all contributions)", source)

    def test_governance_contract_runs_in_existing_read_only_pr_check(self) -> None:
        workflow = (ROOT / ".github/workflows/pr-title.yml").read_text()
        self.assertIn("contents: read", workflow)
        self.assertIn("python3 scripts/ci/test_local_agent_governance.py", workflow)
        self.assertIn("./scripts/check-pr-title.test.sh", workflow)
        self.assertIn('./scripts/check-pr-title.sh "$PR_TITLE"', workflow)
        self.assertNotIn("continue-on-error", workflow)

    def test_current_authority_headers_do_not_require_upstream_reviewers(self) -> None:
        for name in (
            "docs/book/src/contributing/how-to.md",
            "docs/book/src/maintainers/pr-workflow.md",
            "docs/book/src/maintainers/reviewer-playbook.md",
        ):
            with self.subTest(path=name):
                source = (ROOT / name).read_text()
                authority = source.split("\n\n", 2)[1]
                self.assertIn("single-maintainer", authority)
                self.assertIn("not a GitHub human", authority)
                self.assertNotIn("required human/code-owner", authority)
                self.assertIn("historical context", authority)

    def test_historical_attribution_is_not_rewritten_as_current_ownership(self) -> None:
        for name in ("LICENSE-MIT", "LICENSE-APACHE", "NOTICE"):
            self.assertTrue((ROOT / name).is_file())
        historical = ROOT / "docs/book/src/architecture/decisions"
        self.assertTrue(any(historical.glob("ADR-*.md")))
        source = (ROOT / "AGENTS.md").read_text()
        self.assertRegex(source, re.compile(r"Preserve upstream\s+licenses"))
        self.assertIn("historical decisions", source)


if __name__ == "__main__":
    unittest.main()
