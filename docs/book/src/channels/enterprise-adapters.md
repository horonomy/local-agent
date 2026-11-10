# Enterprise-owned channel adapters

The generic `Channel` abstraction and its message types are kept as a **source-level
extension point**, so an enterprise can add an adapter for its own messaging system in
a custom build. This page lists the touch points. Every path is relative to the
repository root; read the named code rather than copying it from here.

## Scope and support

- **Source-level only.** There is no dynamic plugin loader and no new network
  endpoint. An adapter exists only if it is compiled into your binary.
- **Custom builds use the `agent-runtime` profile.** The shipped
  `local-agent-minimal` profile starts no messaging listeners and makes no messaging
  egress, and it refuses to compile with any `channel-*` feature (see
  [Keep the minimal profile guard accurate](#keep-the-minimal-profile-guard-accurate)
  and [Minimal profile](../setup/local-agent-minimal.md)).
- **Unsupported by the project.** The project ships and tests no enterprise adapter and
  gives no compatibility guarantee for the internal `Channel` surface between releases.
- **The adapter owner is responsible** for its network egress, authentication, data
  handling and security review.
- **Bundled Matrix was removed.** The Matrix adapter, its orchestrator wiring and its
  `[channels.matrix]` configuration are gone. An existing `[channels.matrix.<alias>]`
  section is skipped at load and reported as a retired section, not an error. Revoke
  the Matrix access token and device and delete `<config_dir>/state/matrix/<alias>/` by
  hand (the guidance string lives in `RETIRED_CHANNEL_TYPES`). To talk to Matrix again,
  write an adapter as described below.

## Touch points

An adapter named `acme` touches these places. The existing IRC adapter is the smallest
complete reference to follow end to end.

1. **Implement `Channel` and `Attributable`.**
   `Channel` is declared in `crates/zeroclaw-api/src/channel.rs` as
   `pub trait Channel: Send + Sync + Attributable`. The required methods are `name`,
   `send` and `listen` (inbound messages are pushed into the `mpsc::Sender` passed to
   `listen`); the other methods have defaults. `Attributable`
   (`crates/zeroclaw-api/src/attribution.rs`) requires `role()` and `alias()`; `role()`
   returns `Role::Channel(ChannelKind::<Variant>)`. See `crates/zeroclaw-channels/src/irc.rs`
   for both impls. Put the module in `crates/zeroclaw-channels/src/acme.rs` and declare it
   in `crates/zeroclaw-channels/src/lib.rs` behind `#[cfg(feature = "channel-acme")]`.
2. **Add a `ChannelKind` variant** in `crates/zeroclaw-api/src/attribution.rs`
   (strum `snake_case` by default; use `#[strum(serialize = "...")]` for a different
   token). Review `ChannelKind::inbound_capable` (a deny-list: a new kind counts as an inbound
   channel unless you add it there). Do not reuse the retained `Matrix` or `Wati` variants: they exist only so
   historical attribution records still deserialize.
3. **Add a config struct** in `crates/zeroclaw-config/src/schema.rs` that derives
   `Configurable` and carries `#[prefix = "channels.acme"]` (compare `IrcConfig`), with an
   `enabled: bool` that defaults to `false`. Mark every credential field `#[secret]`.
   Add `pub acme: HashMap<String, AcmeConfig>` (with `#[nested]`) to `ChannelsConfig`,
   then update the places that enumerate its fields: `ChannelsConfig::channels()`,
   `has_any_enabled`, `channel_presence` (a fixed-size array, so its length constant changes too) and `impl Default for ChannelsConfig`. A
   compiler error or the config tests will point at any you miss.
4. **Do not reuse a retired key.** `RETIRED_CHANNEL_TYPES` in the same file (currently
   `wati` and `matrix`) makes a leftover section of that name skipped and flagged, and
   makes references to it fail validation with a removal hint. A new adapter must use a
   different key.
5. **Add a listing row.** Append a `ChannelCompileSpec` to `CHANNEL_COMPILE_SPECS` in `crates/zeroclaw-channels/src/listing.rs` with
   `schema_name` matching the `ChannelsConfig::channels()` display name, the accepted
   `type_keys`, and `compiled: cfg!(feature = "channel-acme")`. Tests assert the names and
   keys do not drift from the config crate.
6. **Register in the orchestrator.** In `crates/zeroclaw-channels/src/orchestrator/mod.rs`:
   re-export the type behind its feature next to the `IrcChannel` line, build the
   channel inside `collect_configured_channels_with_authority` (called by
   `collect_configured_channels`), and add a matching arm in `build_channel_by_id` (the `"irc"` arm shows the shape), plus a `#[cfg(not(feature = ...))]` arm that returns a "requires the
   `channel-acme` feature" error. A test in that file checks every compiled channel family
   has a listener registration or explicit runtime owner.
7. **Add the Cargo feature.** Declare `channel-acme` in
   `crates/zeroclaw-channels/Cargo.toml` (list any transport dependency feature, as
   `channel-irc = ["tls-client"]` does) and forward it from the root `Cargo.toml` as
   `channel-acme = ["zeroclaw-channels/channel-acme"]`. Do not add it to
   `default-channels` or `channels-full` unless you want it in those bundles.
8. **Keep the minimal profile guard accurate.** See the next section.

## Keep the minimal profile guard accurate

The guard makes sure no shipped build can pull an adapter in by accident:

- `src/lib.rs` has a `compile_error!` that fires when `local-agent-minimal` is combined
  with a listed feature. Add `feature = "channel-acme"` to the `any(...)` list.
  `tests/component/minimal_profile_feature_graph.rs` fails when the manifest has a
  `channel-*` feature missing from that list.
- `scripts/ci/minimal_profile_artifact.sh tree` rejects any
  `zeroclaw-channels feature "channel-...` node in the minimal feature closure, so a new
  `channel-*` name is covered automatically.
- `scripts/ci/minimal_profile_artifact.sh binary` rejects linked symbols by an explicit
  module list: `zeroclaw_channels::(telegram|discord|webhook|email_channel|acp_channel|filesystem|matrix|slack|line|lark)::`.
  A new adapter is not covered until you add its module name to that regex. (`matrix` is
  now a stale entry and harmless.)

A fork that never builds `local-agent-minimal` with its adapter can skip the last item,
but the first is required: the feature-graph test enforces it.

## Secrets

Mark credential fields `#[secret]` in the config struct so the config machinery treats them as
secrets (stored through the secret store, encrypted on disk when `[secrets] encrypt` is
on, and flagged `x-secret` in the schema). Do not log, echo or format secret values, including in
error messages and `Debug` output. Authentication material belongs in the config secret
store, not in source or the build environment.

## Testing

Run the adapter's unit tests with the feature enabled and the defaults off, the same
shape CI uses in the `test-channel-features` job in `.github/workflows/ci.yml`:

```sh
la-cargo nextest run --locked -p zeroclaw-channels --no-default-features \
  --features channel-acme --lib acme::
```

Also run the orchestrator registration and listing tests in the same crate with the feature
on, and the root minimal-profile guard test:

```sh
la-cargo nextest run --locked -p zeroclaw-channels --no-default-features \
  --features channel-acme --lib orchestrator:: listing::
la-cargo nextest run --locked --test component minimal_profile_feature_graph
```

On a machine without `la-cargo`, use plain `cargo` with the same arguments. Build the
custom binary with `--features agent-runtime,channel-acme`.
