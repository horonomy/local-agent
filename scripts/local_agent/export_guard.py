#!/usr/bin/env python3
"""Review Git-staged or explicitly selected historical blobs before export.

This local-only helper reads Git object data and a user-maintained review list.
It never uploads content, modifies the index, or exports files. A clean result is
not proof that sensitive information is absent.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
from typing import Iterable, Sequence

MAX_TEXT_BYTES = 2 * 1024 * 1024
PROBE_BYTES = 512
TEXT_SUFFIXES = frozenset(
    ".bash .c .cc .cfg .conf .cpp .css .dockerfile .go .h .hpp .html .ini "
    ".java .js .json .json5 .jsonc .lua .md .mk .nix .pl .py .rb .rs .scss "
    ".sh .sql .swift .toml .ts .tsx .txt .xml .yaml .yml .zsh".split()
)
TEXT_BASENAMES = frozenset(
    {".editorconfig", ".gitignore", ".gitmodules", "dockerfile", "justfile", "makefile"}
)
BLOCKED_SUFFIXES = frozenset(
    ".7z .a .avif .avi .bmp .bz2 .cer .core .crt .db .der .dmp .dump .gif "
    ".gz .heic .ico .jpeg .jpg .key .log .m4a .mkv .mov .mp3 .mp4 .pem "
    ".p12 .pfx .png .rar .sqlite .tar .tif .tiff .trace .wav .webm .webp .xz .zip".split()
)
BLOCKED_COMPONENTS = frozenset(
    "artifact artifacts capture captures credential credentials dump dumps log logs "
    "media private recording recordings screen screens screenshot screenshots "
    "secret secrets trace traces".split()
)

# These signatures are checked before the rest of an object is read. This keeps
# media and archive payloads out of the scanner's input buffer even when renamed.
MAGIC_SIGNATURES: tuple[tuple[bytes, bytes, str], ...] = (
    (b"\x89PNG\r\n\x1a\n", b"", "visual-media"),
    (b"\xff\xd8\xff", b"", "visual-media"),
    (b"GIF87a", b"", "visual-media"),
    (b"GIF89a", b"", "visual-media"),
    (b"BM", b"", "visual-media"),
    (b"II*\x00", b"", "visual-media"),
    (b"MM\x00*", b"", "visual-media"),
    (b"\x00\x00\x01\x00", b"", "visual-media"),
    (b"PK\x03\x04", b"", "archive"),
    (b"PK\x05\x06", b"", "archive"),
    (b"PK\x07\x08", b"", "archive"),
    (b"\x1f\x8b", b"", "archive"),
    (b"BZh", b"", "archive"),
    (b"\xfd7zXZ\x00", b"", "archive"),
    (b"7z\xbc\xaf'\x1c", b"", "archive"),
    (b"Rar!\x1a\x07", b"", "archive"),
    (b"%PDF-", b"", "visual-media"),
    (b"SQLite format 3\x00", b"", "data-dump"),
    (b"PAR1", b"", "data-dump"),
    (b"\x7fELF", b"", "binary-content"),
    (b"MZ", b"", "binary-content"),
)

CREDENTIAL_ASSIGNMENT = re.compile(
    r"(?i)(?:\b|_)(?:[A-Za-z0-9]+[_-])*(?:api[_-]?(?:key|token)|"
    r"access[_-]?(?:key|token)|auth[_-]?token|client[_-]?secret|password|"
    r"passwd|private[_-]?key|secret(?:[_-]?key)?|signing[_-]?key|token)"
    r"\b\s*[:=]\s*[\"']?([A-Za-z0-9_./+=:-]{4,})"
)
KNOWN_TOKEN_PATTERN = re.compile(
    r"(?i)\b(?:AKIA|ASIA)[A-Z0-9]{16}\b|\bgh[pousr]_[A-Za-z0-9]{20,}\b|"
    r"\bgithub_pat_[A-Za-z0-9_]{20,}\b|\bxox[baprs]-[A-Za-z0-9-]{10,}\b|"
    r"\bBearer\s+[A-Za-z0-9._~+/-]{16,}"
)
PRIVATE_KEY_PATTERN = re.compile(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----")
LOCAL_PATH = re.compile(
    r"(?i)(?:^|[\s\"'=])/(?:Users|home|Volumes|private/var/folders)/"
    r"[^\s\"'<>]+|(?:^|[\s\"'=])[A-Z]:\\Users\\[^\s\"'<>]+"
)
LOG_LINE = re.compile(
    r"(?im)^\s*(?:\d{4}-\d\d-\d\d[T ][^\r\n]{0,40}\s)?"
    r"(?:TRACE|DEBUG|INFO|WARN|WARNING|ERROR|FATAL)\s*(?:[:|\]])"
)
TRACEBACK = re.compile(r"(?m)^Traceback \(most recent call last\):")
PLACEHOLDER_VALUES = {
    "changeme",
    "example",
    "false",
    "none",
    "null",
    "placeholder",
    "redacted",
    "replace_me",
    "todo",
    "true",
    "your_api_key",
    "your_token",
}


class GuardError(Exception):
    """A safe, content-free failure message for a local review operation."""


@dataclass(frozen=True)
class Finding:
    """One sanitized finding associated with a repo-relative path."""

    path: str
    category: str


@dataclass(frozen=True)
class ReviewResult:
    """Counts and findings from one staged or historical review."""

    inspected: int
    findings: list[Finding]


def run_git(root: Path, arguments: Sequence[str | bytes]) -> bytes:
    """Run one local Git command without exposing its stderr or content."""
    command: list[str | bytes] = [
        "git",
        "--no-replace-objects",
        "-C",
        str(root),
        *arguments,
    ]
    result = subprocess.run(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    if result.returncode != 0:
        raise GuardError("a required local Git operation failed")
    return result.stdout


def repository_root(start: Path) -> Path:
    """Resolve the current repository root or fail without leaking paths."""
    result = subprocess.run(
        [
            "git",
            "--no-replace-objects",
            "-C",
            str(start),
            "rev-parse",
            "--show-toplevel",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    if result.returncode != 0:
        raise GuardError("run this command inside a Git repository")
    return Path(os.fsdecode(result.stdout.rstrip(b"\r\n"))).resolve()


def normalized_repo_path(raw_path: bytes) -> str:
    """Validate and decode a Git path without printing its content."""
    try:
        path = raw_path.decode("utf-8", errors="strict")
    except UnicodeDecodeError as error:
        raise GuardError("a Git path is not valid UTF-8") from error
    pure = PurePosixPath(path)
    if (
        not path
        or pure.is_absolute()
        or "\\" in path
        or any(part in ("", ".", "..") for part in path.split("/"))
    ):
        raise GuardError("a Git path is not a safe repository-relative path")
    return path


def load_reviewed_sources(list_path: Path) -> set[tuple[str, str]]:
    """Load exact repo-relative path and SHA-256 approvals from a local list."""
    resolved = list_path.resolve(strict=True)
    locations = {list_path.absolute().parent, resolved.parent}
    for location in locations:
        probe = subprocess.run(
            [
                "git",
                "--no-replace-objects",
                "-C",
                str(location),
                "rev-parse",
                "--show-toplevel",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        if probe.returncode == 0:
            raise GuardError(
                "the reviewed-source list must be outside every Git worktree"
            )
    try:
        lines = resolved.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeError) as error:
        raise GuardError("the reviewed-source list is unreadable") from error
    reviewed: set[tuple[str, str]] = set()
    for line in lines:
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        digest, separator, path_value = line.partition("\t")
        if not separator or not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise GuardError("the reviewed-source list has an invalid entry")
        try:
            path = normalized_repo_path(path_value.encode("utf-8"))
        except GuardError as error:
            raise GuardError("the reviewed-source list has an invalid path") from error
        entry = (path, digest)
        if entry in reviewed:
            raise GuardError("the reviewed-source list contains a duplicate entry")
        reviewed.add(entry)
    if not reviewed:
        raise GuardError("the reviewed-source list has no reviewed entries")
    return reviewed


def path_categories(path: str) -> set[str]:
    """Block known local-only path names and file classes."""
    pure = PurePosixPath(path)
    lower_parts = [part.casefold() for part in pure.parts]
    categories: set[str] = set()
    for part in lower_parts:
        tokens = re.findall(r"[a-z0-9]+", part)
        if set(tokens) & BLOCKED_COMPONENTS:
            categories.add("sensitive-path")
        if part in {"id_rsa", "id_dsa", "id_ecdsa", "id_ed25519"}:
            categories.add("private-key")
        if part == ".env" or (part.startswith(".env.") and part != ".env.example"):
            categories.add("sensitive-path")
    suffix = pure.suffix.casefold()
    if suffix in BLOCKED_SUFFIXES:
        categories.add("sensitive-file-type")
    if suffix in {".pem", ".key", ".p12", ".pfx", ".der"}:
        categories.add("private-key")
    return categories


def text_candidate(path: str) -> bool:
    """Return whether the path names an explicitly supported text format."""
    pure = PurePosixPath(path)
    if pure.name.casefold() in TEXT_BASENAMES or pure.name.casefold() == ".env.example":
        return True
    return pure.suffix.casefold() in TEXT_SUFFIXES


def signature_category(prefix: bytes) -> str | None:
    """Recognize common media, archive, data, and executable signatures."""
    if len(prefix) >= 262 and prefix[257:262] == b"ustar":
        return "archive"
    for start, marker, category in MAGIC_SIGNATURES:
        if prefix.startswith(start) and (not marker or marker in prefix[:32]):
            return category
    if prefix.startswith(b"RIFF") and len(prefix) >= 12:
        if prefix[8:12] in {b"WAVE", b"AVI ", b"WEBP"}:
            return "visual-media"
    if (
        prefix.startswith(b"\x1aE\xdf\xa3")
        or prefix.startswith(b"OggS")
        or prefix.startswith(b"fLaC")
        or prefix.startswith(b"ID3")
    ):
        return "visual-media"
    return None


def terminate_process(process: subprocess.Popen[bytes]) -> None:
    """Stop a cat-file producer after a bounded prefix probe."""
    if process.poll() is None:
        process.terminate()
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=2)
    if process.stdout is not None:
        process.stdout.close()


def read_text_blob(root: Path, object_id: str) -> tuple[bytes | None, str | None]:
    """Probe an object, then read only bounded text candidates in full."""
    process = subprocess.Popen(
        ["git", "--no-replace-objects", "-C", str(root), "cat-file", "--batch"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    if process.stdin is None or process.stdout is None:
        terminate_process(process)
        raise GuardError("could not inspect a staged Git object")
    try:
        process.stdin.write(object_id.encode("ascii") + b"\n")
        process.stdin.close()
        header = process.stdout.readline().split()
        if len(header) != 3 or header[1] != b"blob" or not header[2].isdigit():
            terminate_process(process)
            raise GuardError("a Git object could not be safely inspected")
        size = int(header[2])
        if size > MAX_TEXT_BYTES:
            terminate_process(process)
            return None, "oversized-content"
        prefix = process.stdout.read(min(size, PROBE_BYTES))
        detected = signature_category(prefix)
        if detected is not None:
            terminate_process(process)
            return None, detected
        rest = process.stdout.read(size - len(prefix))
        separator = process.stdout.read(1)
        return_code = process.wait(timeout=10)
        process.stdout.close()
        if return_code != 0 or len(prefix) + len(rest) != size or separator != b"\n":
            raise GuardError("a Git object could not be safely inspected")
        return prefix + rest, None
    except (OSError, subprocess.TimeoutExpired) as error:
        terminate_process(process)
        raise GuardError("a Git object could not be safely inspected") from error


def content_categories(content: bytes) -> set[str]:
    """Detect high-confidence sensitive-content indicators without echoing data."""
    categories: set[str] = set()
    if b"\x00" in content or any(byte < 9 or 13 < byte < 32 for byte in content):
        return {"binary-content"}
    try:
        text = content.decode("utf-8-sig", errors="strict")
    except UnicodeDecodeError:
        return {"binary-content"}
    if LOCAL_PATH.search(text):
        categories.add("local-path")
    if PRIVATE_KEY_PATTERN.search(text):
        categories.add("private-key")
    if KNOWN_TOKEN_PATTERN.search(text):
        categories.add("credential-value")
    for match in CREDENTIAL_ASSIGNMENT.finditer(text):
        value = match.group(1).casefold().strip(".=/")
        if value not in PLACEHOLDER_VALUES and not (
            value.startswith("${") and value.endswith("}")
        ):
            categories.add("credential-value")
            break
    if TRACEBACK.search(text) or LOG_LINE.search(text):
        categories.add("log-or-trace")
    return categories


def inspect_blob(
    root: Path,
    path: str,
    object_id: str,
    mode: str,
    reviewed: set[tuple[str, str]],
) -> list[Finding]:
    """Apply path, type, content, and exact-provenance checks to one blob."""
    categories = path_categories(path)
    if mode not in {"100644", "100755"}:
        categories.add("unsupported-object-mode")
    if not text_candidate(path):
        categories.add("unreviewable-file-type")
    if categories:
        return [Finding(path, category) for category in sorted(categories)]

    content, signature = read_text_blob(root, object_id)
    if signature is not None:
        return [Finding(path, signature)]
    if content is None:
        return [Finding(path, "unreadable-content")]
    categories.update(content_categories(content))
    digest = hashlib.sha256(content).hexdigest()
    if (path, digest) not in reviewed:
        categories.add("unreviewed-provenance")
    return [Finding(path, category) for category in sorted(categories)]


def parse_raw_diff(raw: bytes) -> list[tuple[str, str, str]]:
    """Extract changed staged object mode, blob id, and path from Git raw output."""
    records = raw.split(b"\x00")
    if records and records[-1] == b"":
        records.pop()
    if len(records) % 2:
        raise GuardError("the staged index diff could not be safely parsed")
    parsed: list[tuple[str, str, str]] = []
    for index in range(0, len(records), 2):
        fields = records[index].split()
        if len(fields) != 5 or not fields[0].startswith(b":"):
            raise GuardError("the staged index diff could not be safely parsed")
        new_mode = fields[1].decode("ascii", errors="strict")
        object_id = fields[3].decode("ascii", errors="strict")
        status = fields[4]
        if not object_id or set(object_id) == {"0"}:
            continue
        if status[:1] not in {b"A", b"C", b"M", b"T"}:
            continue
        parsed.append((new_mode, object_id, normalized_repo_path(records[index + 1])))
    return parsed


def inspect_staged(root: Path, reviewed: set[tuple[str, str]]) -> ReviewResult:
    """Inspect staged changed blobs exclusively from Git's index objects."""
    unmerged = run_git(root, ["ls-files", "-u", "-z"])
    if unmerged:
        raise GuardError("resolve staged merge conflicts before review")
    raw = run_git(
        root,
        ["diff", "--cached", "--raw", "-z", "--no-renames", "--diff-filter=ACMT"],
    )
    items = parse_raw_diff(raw)
    findings: list[Finding] = []
    for mode, object_id, path in items:
        findings.extend(inspect_blob(root, path, object_id, mode, reviewed))
    return ReviewResult(len(items), findings)


def resolve_commit(root: Path, reference: str) -> str:
    """Resolve a caller-supplied revision to a commit object id."""
    if not reference or reference.startswith("-"):
        raise GuardError("history review requires an explicit base..head range")
    result = run_git(root, ["rev-parse", "--verify", reference + "^{commit}"])
    value = result.strip().decode("ascii", errors="strict")
    if not re.fullmatch(r"[0-9a-fA-F]{40,64}", value):
        raise GuardError("history review could not resolve the requested range")
    return value.lower()


def require_ancestor_range(root: Path, base: str, head: str) -> None:
    """Reject grafted/shallow history and ranges whose base is not an ancestor."""
    grafts = Path(
        os.fsdecode(
            run_git(root, ["rev-parse", "--git-path", "info/grafts"]).rstrip(b"\r\n")
        )
    )
    if not grafts.is_absolute():
        grafts = root / grafts
    if grafts.exists() or grafts.is_symlink():
        raise GuardError("history review cannot certify grafted history")
    shallow = run_git(root, ["rev-parse", "--is-shallow-repository"]).strip()
    if shallow != b"false":
        raise GuardError("history review cannot certify a shallow repository")
    result = subprocess.run(
        [
            "git",
            "--no-replace-objects",
            "-C",
            str(root),
            "merge-base",
            "--is-ancestor",
            base,
            head,
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    if result.returncode != 0:
        raise GuardError("history review requires base to be an ancestor of head")


def changed_paths(root: Path, commit: str) -> list[str]:
    """Return paths changed in a commit, including per-parent merge changes."""
    raw = run_git(
        root,
        [
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-only",
            "-r",
            "-z",
            "--no-renames",
            "-m",
            "--diff-filter=ACMT",
            commit,
        ],
    )
    return [normalized_repo_path(path) for path in raw.split(b"\x00") if path]


def commit_blobs(
    root: Path, commit: str, paths: Iterable[str]
) -> list[tuple[str, str, str]]:
    """Resolve changed paths to blob IDs in that commit's tree, without content reads."""
    path_list = list(dict.fromkeys(paths))
    if not path_list:
        return []
    arguments: list[str | bytes] = ["ls-tree", "-r", "-z", commit, "--"]
    arguments.extend(":(literal)".encode() + path.encode("utf-8") for path in path_list)
    raw = run_git(root, arguments)
    blobs: list[tuple[str, str, str]] = []
    for record in raw.split(b"\x00"):
        if not record:
            continue
        metadata, separator, raw_path = record.partition(b"\t")
        fields = metadata.split()
        if not separator or len(fields) != 3:
            raise GuardError("a historical Git tree could not be safely parsed")
        mode = fields[0].decode("ascii", errors="strict")
        object_type = fields[1]
        object_id = fields[2].decode("ascii", errors="strict")
        path = normalized_repo_path(raw_path)
        if object_type in {b"blob", b"commit"} and path in path_list:
            blobs.append((mode, object_id, path))
    return blobs


def inspect_history(
    root: Path, range_value: str, reviewed: set[tuple[str, str]]
) -> ReviewResult:
    """Review changed blob versions in only the explicitly supplied base..head range."""
    if range_value.count("..") != 1 or "..." in range_value:
        raise GuardError("history review requires an explicit base..head range")
    base_ref, head_ref = range_value.split("..", 1)
    base = resolve_commit(root, base_ref)
    head = resolve_commit(root, head_ref)
    require_ancestor_range(root, base, head)
    commits_output = run_git(
        root, ["rev-list", "--reverse", "--topo-order", head, "^" + base]
    )
    commits = [
        line.decode("ascii", errors="strict")
        for line in commits_output.splitlines()
        if line
    ]
    findings: list[Finding] = []
    inspected = 0
    for commit in commits:
        paths = changed_paths(root, commit)
        # Path-only blocks happen before ls-tree or cat-file can read payload data.
        safe_paths: list[str] = []
        for path in paths:
            categories = path_categories(path)
            if categories:
                findings.extend(
                    Finding(path, category) for category in sorted(categories)
                )
            elif not text_candidate(path):
                findings.append(Finding(path, "unreviewable-file-type"))
            else:
                safe_paths.append(path)
        for mode, object_id, path in commit_blobs(root, commit, safe_paths):
            inspected += 1
            findings.extend(inspect_blob(root, path, object_id, mode, reviewed))
    return ReviewResult(inspected, findings)


def format_result(label: str, result: ReviewResult) -> str:
    """Format a deterministic result without disclosing repository paths."""
    if not result.findings:
        return f"REVIEW PASS {label}: {result.inspected} blob(s) reviewed; no export performed."
    grouped: dict[str, int] = {}
    for finding in result.findings:
        grouped[finding.category] = grouped.get(finding.category, 0) + 1
    lines = [
        f"REVIEW BLOCK {label}: {result.inspected} blob(s) inspected; "
        f"{len(result.findings)} finding(s)."
    ]
    for category in sorted(grouped):
        lines.append(f"  {category}: {grouped[category]} finding(s)")
    lines.append("Paths and file contents were not printed or exported.")
    return "\n".join(lines)


def build_parser() -> argparse.ArgumentParser:
    """Build the standard-library CLI parser."""
    parser = argparse.ArgumentParser(
        description=__doc__,
        epilog=(
            "A clean result is not proof that sensitive data is absent. Every blob "
            "must match an exact path-and-SHA-256 entry in a local reviewed-source list."
        ),
    )
    commands = parser.add_subparsers(dest="command", required=True)
    staged = commands.add_parser(
        "staged", help="inspect changed blobs in the Git index"
    )
    staged.add_argument("--reviewed-sources", type=Path, required=True, metavar="PATH")
    history = commands.add_parser(
        "history", help="inspect changed blob versions in a range"
    )
    history.add_argument("--range", required=True, metavar="BASE..HEAD")
    history.add_argument("--reviewed-sources", type=Path, required=True, metavar="PATH")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Run a local staged or explicitly bounded historical review."""
    args = build_parser().parse_args(argv)
    try:
        root = repository_root(Path.cwd())
        list_path: Path = args.reviewed_sources.expanduser()
        reviewed = load_reviewed_sources(list_path)
        if args.command == "staged":
            result = inspect_staged(root, reviewed)
            label = "staged"
        else:
            result = inspect_history(root, args.range, reviewed)
            label = "history"
        print(format_result(label, result))
        return 1 if result.findings else 0
    except (GuardError, OSError, UnicodeError, ValueError, subprocess.SubprocessError):
        print("REVIEW BLOCK: unable to safely complete local review.", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
