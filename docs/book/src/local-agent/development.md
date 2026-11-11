# Development

Verification for the minimal profile, from [Minimal runtime profile](../setup/local-agent-minimal.md#verification):

```sh
F="--no-default-features --features local-agent-minimal"
cargo test --locked -p zeroclaw $F --test component -- minimal_profile
scripts/ci/minimal_profile_artifact.sh tree
```

The profile page lists the remaining checks (integration tests, the binary symbol check and `scripts/local_agent/listener_probe.py`). The cumulative golden journeys are described in `docs/local-agent/golden-journeys-runner.md`.

Provenance of the derivative, the pinned upstream baseline and the divergence ledger are in `UPSTREAM.md` at the repository root.

Documentation pages (inherited):

- [Building the docs locally](../developing/building-docs.md)
- [Testing](../contributing/testing.md)
- [Generated documentation pipeline](../architecture/generated-documentation-pipeline.md)
- [Localization catalog lifecycle](../architecture/localization-catalog-lifecycle.md)
