# Local source export review aid

`export_guard.py` is an opt-in, local-only review helper. It does not upload, copy, stage, modify, or export files. It inspects changed staged blobs from the Git index (`staged`) or changed blobs reachable in an explicitly supplied `BASE..HEAD` commit range (`history`). History mode requires a non-shallow repository, rejects legacy graft metadata and verifies that `BASE` is an ancestor of `HEAD`. All Git operations disable replacement objects so receipts apply to the actual index/commit blobs, not locally substituted content. It scans changed blob versions in commits after the base, not the base tree itself. Therefore, it does **not** cover initial-push contents or any content already present at `BASE`; choose a base only when that ancestry is independently trusted and previously reviewed. Do not treat upstream history as implicitly reviewed.

## Reviewed-source list

Both commands require a reviewed-source list outside every Git worktree. Each non-comment line is the SHA-256 of an individually reviewed blob, a tab, then its exact repository-relative path:

```text
<64 lowercase hex SHA-256>\t<repo/relative/path>
```

The hash must match the staged or historical blob exactly. A change to either the path or bytes requires review-list renewal. Keep this file local and do not commit it. Empty lists are rejected; list entries are not blanket approvals.

Example invocation:

```text
python3 scripts/local_agent/export_guard.py staged --reviewed-sources /local/private/reviewed-sources.txt
python3 scripts/local_agent/export_guard.py history --range BASE..HEAD --reviewed-sources /local/private/reviewed-sources.txt
```

The helper never reads unstaged working-tree bytes when reviewing staged content. It rejects unmerged indexes, unsupported object modes, unknown file types, oversized text candidates, and content without an exact review-list match. Known log, trace, data-dump, media, screen/capture, archive, secret, private-key, and local-path patterns are blocked. A short content signature probe helps reject common binary payloads whose filenames were changed; paths with known local-only media/archive suffixes are blocked before opening blob contents.

## Limits and safe use

This is a review aid, not a DLP system or proof that sensitive data is absent. Pattern matching cannot recognize every secret, proprietary identifier, personal datum, image format, archive, binary encoding, indirect reference, or corporate path. Review provenance and source meaning yourself, and inspect any proposed outbound content independently. A `PASS` only means that the reviewed files in the selected scope had no indicators recognized here and matched the local path-and-hash list. The tool does not assess commit messages, tags, release assets, LFS payloads, or remote state and does not initiate a push.

Run its focused standard-library suite using:

```text
python3 -m unittest scripts.local_agent.test_export_guard
```
