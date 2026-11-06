"""Synthetic tests for the local staged/history export review helper."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from scripts.local_agent import export_guard

SCRIPT = Path(export_guard.__file__).resolve()


def git(root: Path, *arguments: str, input_data: bytes | None = None) -> bytes:
    """Run a Git command in a synthetic repository without echoing its data."""
    result = subprocess.run(
        ["git", "-C", str(root), *arguments],
        input=input_data,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    if result.returncode:
        raise AssertionError("synthetic Git setup failed")
    return result.stdout


def write_reviewed_list(
    path: Path, contents_by_path: dict[str, bytes] | list[tuple[str, bytes]]
) -> None:
    """Write only hashes and repo-relative paths to an external temporary file."""
    entries = (
        contents_by_path.items()
        if isinstance(contents_by_path, dict)
        else contents_by_path
    )
    lines = [
        hashlib.sha256(contents).hexdigest() + "\t" + name
        for name, contents in sorted(entries)
    ]
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def initialize_repo(root: Path, initial_files: dict[str, bytes]) -> str:
    """Create a temporary repository with a deterministic synthetic base commit."""
    root.mkdir()
    git(root, "init", "-q")
    git(root, "config", "user.name", "Synthetic Reviewer")
    git(root, "config", "user.email", "reviewer@example.invalid")
    for name, data in initial_files.items():
        target = root / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    git(root, "add", "--all")
    git(root, "commit", "-qm", "synthetic base")
    return git(root, "rev-parse", "HEAD").decode("ascii").strip()


def add_file(root: Path, name: str, contents: bytes, stage: bool = True) -> None:
    """Write a synthetic blob and optionally add it to the temporary index."""
    target = root / name
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(contents)
    if stage:
        git(root, "add", "--", name)


class ExportGuardTest(unittest.TestCase):
    def run_cli(
        self, root: Path, command: str, reviewed: Path, *extra: str
    ) -> subprocess.CompletedProcess[str]:
        """Run the local-only CLI and capture only its sanitized summary."""
        return subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                command,
                *extra,
                "--reviewed-sources",
                str(reviewed),
            ],
            cwd=root,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            check=False,
        )

    def test_staged_scan_reads_index_not_unstaged_filesystem(self) -> None:
        safe = b'{"feature": true, "source": "public"}\n'
        secret_value = "synthetic" + "-secret-value"
        setting_name = "API" + "_TOKEN"
        safe = (setting_name + "=example\n").encode()
        safe_yaml = b"feature: true\nsource: public\n"
        base = b'{"feature": false}\n'
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            root = directory / "repo"
            initialize_repo(
                root,
                {"settings.json": base, "config.yaml": b"feature: false\n"},
            )
            add_file(root, "settings.json", safe)
            add_file(root, "config.yaml", safe_yaml)
            # The unstaged working-tree bytes are deliberately unsafe and differ
            # from the staged public configuration reviewed below.
            (root / "settings.json").write_bytes(
                (setting_name + "=" + secret_value + "\n").encode()
            )
            reviewed = directory / "reviewed.txt"
            write_reviewed_list(
                reviewed, {"settings.json": safe, "config.yaml": safe_yaml}
            )

            result = self.run_cli(root, "staged", reviewed)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("REVIEW PASS staged: 2 blob(s) reviewed", result.stdout)
        self.assertNotIn(secret_value, result.stdout + result.stderr)

    def test_unreviewed_safe_source_fails_closed(self) -> None:
        source = b"# synthetic public config\nfeature = true\n"
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            root = directory / "repo"
            initialize_repo(root, {"Cargo.toml": b"[package]\nname='fixture'\n"})
            add_file(root, "src/config.py", source)
            reviewed = directory / "reviewed.txt"
            write_reviewed_list(reviewed, {})
            # Empty lists are invalid by design; one unrelated reviewed entry
            # proves that the candidate itself, not a missing input, is blocked.
            write_reviewed_list(reviewed, {"other.toml": b"value = 1\n"})

            result = self.run_cli(root, "staged", reviewed)

        self.assertEqual(result.returncode, 1)
        self.assertIn("unreviewed-provenance", result.stdout)
        self.assertNotIn("public config", result.stdout + result.stderr)

    def test_renamed_visual_signature_is_rejected(self) -> None:
        image = b"\x89PNG\r\n\x1a\n" + b"synthetic-pixel-payload"
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            root = directory / "repo"
            initialize_repo(root, {"README.md": b"synthetic base\n"})
            add_file(root, "notes.txt", image)
            reviewed = directory / "reviewed.txt"
            write_reviewed_list(reviewed, {"notes.txt": image})

            result = self.run_cli(root, "staged", reviewed)

        self.assertEqual(result.returncode, 1)
        self.assertIn("visual-media", result.stdout)
        self.assertNotIn("synthetic-pixel-payload", result.stdout + result.stderr)

    def test_sensitive_paths_block_logs_media_and_archives(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            root = directory / "repo"
            initialize_repo(root, {"README.md": b"synthetic base\n"})
            for name, contents in (
                ("logs/session.py", b"public text\n"),
                ("screenshots/capture.json", b"{}\n"),
                ("bundle.zip", b"PK\x03\x04synthetic-archive"),
            ):
                add_file(root, name, contents)
            reviewed = directory / "reviewed.txt"
            write_reviewed_list(
                reviewed,
                {
                    "logs/session.py": b"public text\n",
                    "screenshots/capture.json": b"{}\n",
                    "bundle.zip": b"PK\x03\x04synthetic-archive",
                },
            )

            result = self.run_cli(root, "staged", reviewed)

        self.assertEqual(result.returncode, 1)
        self.assertIn("sensitive-path", result.stdout)
        self.assertIn("sensitive-file-type", result.stdout)
        self.assertIn("unreviewable-file-type", result.stdout)

    def test_secret_values_and_private_keys_are_rejected_without_echo(self) -> None:
        key_start = "-----BEGIN "
        key_kind = "PRIVATE KEY"
        key_end = "-----"
        synthetic_value = "synthetic" + "-value"
        setting = "api" + "_key"
        assembled_key = "OPENAI_" + setting.upper() + "=" + synthetic_value
        password = "db_" + "password=" + synthetic_value
        bearer = "Bearer " + "synthetic-token-value-012345"
        local_path = "path=" + "/Users/" + "example-private/workspace/file"
        self.assertIn(
            "credential-value",
            export_guard.content_categories((assembled_key + "\n").encode()),
        )
        self.assertIn(
            "credential-value",
            export_guard.content_categories(password.encode()),
        )
        self.assertIn(
            "private-key",
            export_guard.content_categories(
                (key_start + key_kind + key_end + "\nsynthetic-key-material\n").encode()
            ),
        )
        self.assertIn(
            "credential-value", export_guard.content_categories(bearer.encode())
        )
        self.assertIn(
            "local-path", export_guard.content_categories(local_path.encode())
        )
        formatted = export_guard.format_result(
            "staged",
            export_guard.ReviewResult(
                1,
                [
                    export_guard.Finding(
                        "client-corporate-name/secret-config.toml", "credential-value"
                    )
                ],
            ),
        )
        self.assertNotIn(synthetic_value, formatted)
        self.assertNotIn("client-corporate-name", formatted)
        self.assertNotIn("secret-config.toml", formatted)

    def test_log_trace_and_binary_payload_signatures_are_rejected(self) -> None:
        self.assertIn(
            "log-or-trace",
            export_guard.content_categories(
                b"2026-01-02 03:04:05 ERROR: synthetic failure"
            ),
        )
        self.assertIn(
            "log-or-trace",
            export_guard.content_categories(
                b"Traceback (most recent call last):\nsynthetic"
            ),
        )
        self.assertEqual(
            export_guard.signature_category(b"RIFFxxxxWEBP"), "visual-media"
        )
        self.assertEqual(
            export_guard.signature_category(b"%PDF-1.7 synthetic"), "visual-media"
        )
        self.assertEqual(
            export_guard.signature_category(b"SQLite format 3\x00fixture"), "data-dump"
        )

    def test_history_inspects_each_outbound_commit_blob(self) -> None:
        field = "API" + "_TOKEN"
        fixture_material = "synthetic" + "-transient-secret"
        transient_secret = (field + "=" + fixture_material + "\n").encode()
        final_safe = b"# reviewed replacement\nsetting = false\n"
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            root = directory / "repo"
            base = initialize_repo(root, {"src/config.py": b"setting = 0\n"})
            add_file(root, "src/config.py", transient_secret)
            git(root, "commit", "-qm", "synthetic intermediate")
            add_file(root, "src/config.py", final_safe)
            git(root, "commit", "-qm", "synthetic final")
            reviewed = directory / "reviewed.txt"
            write_reviewed_list(
                reviewed,
                [("src/config.py", transient_secret), ("src/config.py", final_safe)],
            )

            result = self.run_cli(root, "history", reviewed, "--range", base + "..HEAD")

        self.assertEqual(result.returncode, 1)
        self.assertIn("credential-value", result.stdout)
        self.assertNotIn(fixture_material, result.stdout + result.stderr)
        self.assertIn("REVIEW BLOCK history", result.stdout)

    def test_history_rejects_unreviewed_ancestor_content(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            root = directory / "repo"
            base = initialize_repo(root, {"source.yaml": b"public: true\n"})
            content = b"public: true\nchanged: true\n"
            add_file(root, "source.yaml", content)
            git(root, "commit", "-qm", "synthetic update")
            reviewed = directory / "reviewed.txt"
            write_reviewed_list(reviewed, {"source.yaml": b"public: true\n"})

            result = self.run_cli(root, "history", reviewed, "--range", base + "..HEAD")

        self.assertEqual(result.returncode, 1)
        self.assertIn("unreviewed-provenance", result.stdout)

    def test_staged_secret_is_rejected_even_when_worktree_is_safe(self) -> None:
        secret_value = "synthetic" + "-index-secret"
        assignment = "API" + "_TOKEN"
        secret = (assignment + "=" + secret_value + "\n").encode()
        safe = (assignment + "=example\n").encode()
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            root = directory / "repo"
            initialize_repo(root, {"settings.toml": b"mode = 'old'\n"})
            add_file(root, "settings.toml", secret)
            (root / "settings.toml").write_bytes(safe)
            reviewed = directory / "reviewed.txt"
            write_reviewed_list(reviewed, {"settings.toml": secret})

            result = self.run_cli(root, "staged", reviewed)

        self.assertEqual(result.returncode, 1)
        self.assertIn("credential-value", result.stdout)
        self.assertNotIn(secret_value, result.stdout + result.stderr)

    def test_stale_review_receipt_does_not_approve_changed_bytes(self) -> None:
        previous = b"setting = 'reviewed'\n"
        changed = b"setting = 'changed'\n"
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            root = directory / "repo"
            initialize_repo(root, {"config.toml": b"setting = 'base'\n"})
            add_file(root, "config.toml", changed)
            reviewed = directory / "reviewed.txt"
            write_reviewed_list(reviewed, {"config.toml": previous})

            result = self.run_cli(root, "staged", reviewed)

        self.assertEqual(result.returncode, 1)
        self.assertIn("unreviewed-provenance", result.stdout)

    def test_symlink_and_gitlink_staged_entries_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            root = directory / "repo"
            initialize_repo(root, {"README.md": b"synthetic base\n"})
            (root / "linked.txt").symlink_to("target.txt")
            git(root, "add", "--", "linked.txt")
            child = root / "nested"
            child.mkdir()
            git(child, "init", "-q")
            git(child, "config", "user.name", "Synthetic Reviewer")
            git(child, "config", "user.email", "reviewer@example.invalid")
            (child / "README.md").write_text(
                "synthetic nested repository\n", encoding="utf-8"
            )
            git(child, "add", "--all")
            git(child, "commit", "-qm", "synthetic nested commit")
            git(root, "add", "--", "nested")
            reviewed = directory / "reviewed.txt"
            write_reviewed_list(reviewed, {"linked.txt": b"target.txt\n"})

            result = self.run_cli(root, "staged", reviewed)

        self.assertEqual(result.returncode, 1)
        self.assertIn("unsupported-object-mode", result.stdout)
        self.assertNotIn("target.txt", result.stdout + result.stderr)

    def test_history_rejects_non_ancestor_range_and_shallow_repo(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            root = directory / "repo"
            base = initialize_repo(root, {"source.py": b"value = 0\n"})
            add_file(root, "source.py", b"value = 1\n")
            git(root, "commit", "-qm", "synthetic left branch")
            left = git(root, "rev-parse", "HEAD").decode("ascii").strip()
            git(root, "checkout", "-q", "--orphan", "right")
            git(root, "rm", "-q", "-rf", ".")
            add_file(root, "source.py", b"value = 2\n")
            git(root, "commit", "-qm", "synthetic unrelated branch")
            right = git(root, "rev-parse", "HEAD").decode("ascii").strip()
            reviewed = directory / "reviewed.txt"
            write_reviewed_list(reviewed, {"source.py": b"value = 2\n"})

            non_ancestor = self.run_cli(
                root, "history", reviewed, "--range", left + ".." + right
            )
            self.assertEqual(non_ancestor.returncode, 2)
            self.assertIn("unable to safely complete", non_ancestor.stderr)

            git(root, "update-ref", "refs/heads/base", base)
            (root / ".git" / "shallow").write_text(base + "\n", encoding="ascii")
            shallow = self.run_cli(
                root, "history", reviewed, "--range", base + ".." + right
            )

        self.assertEqual(shallow.returncode, 2)
        self.assertIn("unable to safely complete", shallow.stderr)

    def test_review_list_inside_any_git_worktree_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            root = directory / "repo"
            initialize_repo(root, {"README.md": b"synthetic base\n"})
            nested_worktree = directory / "other-repo"
            initialize_repo(nested_worktree, {"reviewed.txt": b"synthetic list\n"})
            reviewed = nested_worktree / "reviewed.txt"

            result = self.run_cli(root, "staged", reviewed)

        self.assertEqual(result.returncode, 2)
        self.assertIn("unable to safely complete", result.stderr)

    def test_sources_are_scannable_using_runtime_assembled_synthetic_literals(
        self,
    ) -> None:
        assignment = "API" + "_TOKEN"
        value = "synthetic" + "-source-secret"
        private_marker = "-----BEGIN " + "PRIVATE KEY-----"
        secret = (assignment + "=" + value + "\n").encode()
        key = (private_marker + "\nsynthetic-key-material\n").encode()
        self.assertIn("credential-value", export_guard.content_categories(secret))
        self.assertIn("private-key", export_guard.content_categories(key))

    def test_local_sources_pass_with_local_exact_hash_receipts(self) -> None:
        source_root = SCRIPT.parents[2]
        source_paths = (
            source_root / "scripts/local_agent/export_guard.py",
            source_root / "scripts/local_agent/test_export_guard.py",
            source_root / "docs/local-agent/export-policy.md",
        )
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            root = directory / "repo"
            initialize_repo(root, {"README.md": b"synthetic base\n"})
            entries: list[tuple[str, bytes]] = []
            for source_path in source_paths:
                relative_path = source_path.relative_to(source_root).as_posix()
                contents = source_path.read_bytes()
                add_file(root, relative_path, contents)
                entries.append((relative_path, contents))
            reviewed = directory / "reviewed.txt"
            write_reviewed_list(reviewed, entries)
            result = self.run_cli(root, "staged", reviewed)

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("REVIEW PASS staged: 3 blob(s) reviewed", result.stdout)

    def test_replacement_blobs_cannot_hide_staged_or_historical_secrets(self) -> None:
        secret = ("API" + "_TOKEN=" + "synthetic" + "-original-value\n").encode()
        replacement = b"setting = 'reviewed replacement'\n"
        for mode in ("staged", "history"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as temp:
                directory = Path(temp)
                root = directory / "repo"
                base = initialize_repo(root, {"settings.toml": b"setting = 0\n"})
                add_file(root, "settings.toml", secret)
                original = git(root, "rev-parse", ":settings.toml").decode().strip()
                benign = (
                    git(root, "hash-object", "-w", "--stdin", input_data=replacement)
                    .decode()
                    .strip()
                )
                git(root, "replace", original, benign)
                if mode == "history":
                    git(root, "commit", "-qm", "synthetic original object")
                reviewed = directory / "reviewed.txt"
                write_reviewed_list(reviewed, {"settings.toml": replacement})
                extra = ("--range", base + "..HEAD") if mode == "history" else ()
                result = self.run_cli(root, mode, reviewed, *extra)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn("credential-value", result.stdout)
                self.assertNotIn(secret.decode().strip(), result.stdout + result.stderr)

    def test_history_rejects_graft_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            root = directory / "repo"
            base = initialize_repo(root, {"source.py": b"value = 0\n"})
            add_file(root, "source.py", b"value = 1\n")
            git(root, "commit", "-qm", "synthetic intermediate")
            add_file(root, "source.py", b"value = 2\n")
            git(root, "commit", "-qm", "synthetic final")
            head = git(root, "rev-parse", "HEAD").decode().strip()
            graft_path = root / ".git/info/grafts"
            graft_path.write_text(head + " " + base + "\n", encoding="ascii")
            reviewed = directory / "reviewed.txt"
            write_reviewed_list(reviewed, {"source.py": b"value = 2\n"})
            result = self.run_cli(root, "history", reviewed, "--range", base + "..HEAD")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("unable to safely complete", result.stderr)

    def test_history_rejects_environment_redirected_grafts(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            root = directory / "repo"
            base = initialize_repo(root, {"source.py": b"value = 0\n"})
            add_file(root, "source.py", b"value = 1\n")
            git(root, "commit", "-qm", "synthetic intermediate")
            add_file(root, "source.py", b"value = 2\n")
            git(root, "commit", "-qm", "synthetic final")
            head = git(root, "rev-parse", "HEAD").decode().strip()
            graft_path = directory / "external-grafts"
            graft_path.write_text(head + " " + base + "\n", encoding="ascii")
            reviewed = directory / "reviewed.txt"
            write_reviewed_list(reviewed, {"source.py": b"value = 2\n"})
            with patch.dict(os.environ, {"GIT_GRAFT_FILE": str(graft_path)}):
                result = self.run_cli(
                    root, "history", reviewed, "--range", base + "..HEAD"
                )
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("unable to safely complete", result.stderr)

    def test_history_blocks_renamed_media_and_sensitive_paths(self) -> None:
        image = b"\x89PNG\r\n\x1a\n" + b"synthetic-only-payload"
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            root = directory / "repo"
            base = initialize_repo(root, {"README.md": b"synthetic base\n"})
            add_file(root, "notes.txt", image)
            add_file(root, "captures/source.py", b"value = 1\n")
            git(root, "commit", "-qm", "synthetic blocked content")
            reviewed = directory / "reviewed.txt"
            write_reviewed_list(
                reviewed, {"notes.txt": image, "captures/source.py": b"value = 1\n"}
            )
            result = self.run_cli(root, "history", reviewed, "--range", base + "..HEAD")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("visual-media", result.stdout)
        self.assertIn("sensitive-path", result.stdout)
        self.assertNotIn("synthetic-only-payload", result.stdout + result.stderr)

    def test_outside_repo_review_list_is_required(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / "repo"
            initialize_repo(root, {"README.md": b"synthetic base\n"})
            inside = root / "reviewed.txt"
            inside.write_text("0" * 64 + "\tREADME.md\n", encoding="utf-8")

            result = self.run_cli(root, "staged", inside)

        self.assertEqual(result.returncode, 2)
        self.assertIn("unable to safely complete", result.stderr)
        self.assertNotIn(str(root), result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
