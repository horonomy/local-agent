#!/usr/bin/env bash
# Prove the local-agent-minimal profile excludes inbound/messaging code.
#
#   minimal_profile_artifact.sh tree
#       Feature closure of the shipped crate graph (dev-dependencies excluded so
#       they cannot hide anything). Positive control: the default graph must show
#       every marker, otherwise the check is vacuous and fails.
#   minimal_profile_artifact.sh binary PATH
#       Symbols of an UNSTRIPPED minimal binary. Positive control: core agent
#       symbols must be present, and so must client helpers from the relay,
#       enrollment and local-RPC modules whose listener entry points are asserted
#       absent. Those entry points are compiled out by the runtime
#       `daemon-listeners` feature (HORO-2383); this check is the second line of
#       defence against a gate being lost, while the client helpers stay because
#       other commands call them.
set -euo pipefail

MIN=(--no-default-features --features local-agent-minimal)
fail=0

# The full graph does not print the root crate's own feature nodes, so the graph
# inverted at zeroclaw-runtime is added: that is where daemon-listeners and the
# metrics exporter show up when the root enables them.
tree_features() { # $1.. = extra cargo args
  {
    cargo tree --locked -p zeroclaw -e no-dev,features --prefix none "$@"
    cargo tree --locked -p zeroclaw -e no-dev,features -i zeroclaw-runtime --prefix none "$@"
  } 2>/dev/null | sort -u
}

absent_markers='zeroclaw-gateway|zerorelay|zeroclaw-runtime feature "daemon-listeners"|zeroclaw-channels feature "(channel-|whatsapp-web|voice-wake)|zeroclaw-runtime feature "observability-prometheus"|^prometheus v|^tokio-tungstenite v|^tungstenite v'

check_tree() {
  local minimal default
  minimal=$(tree_features "${MIN[@]}")
  default=$(tree_features)
  # Positive control: the default graph must contain the markers.
  for marker in 'zeroclaw-gateway' 'zeroclaw-runtime feature "daemon-listeners"' 'zeroclaw-channels feature "channel-' 'zeroclaw-runtime feature "observability-prometheus"'; do
    if ! grep -qF -- "$marker" <<<"$default"; then
      echo "FAIL control: default graph lacks '$marker'; the check cannot prove anything" >&2
      exit 2
    fi
  done
  if ! grep -qE -- '^prometheus v' <<<"$default"; then
    echo "FAIL control: default graph lacks the prometheus crate; the exporter check cannot prove anything" >&2
    exit 2
  fi
  if ! grep -qE -- '^(tokio-)?tungstenite v' <<<"$default"; then
    echo "FAIL control: default graph lacks tungstenite; the WebSocket check cannot prove anything" >&2
    exit 2
  fi
  if [[ -z "$minimal" ]] || ! grep -q '^zeroclaw-runtime' <<<"$minimal"; then
    echo "FAIL control: minimal graph is empty or lacks zeroclaw-runtime" >&2
    exit 2
  fi
  if hits=$(grep -E -- "$absent_markers" <<<"$minimal"); then
    echo "FAIL tree: forbidden features in the minimal closure:" >&2
    echo "$hits" >&2
    fail=1
  else
    echo "PASS tree: no gateway, relay app, daemon-listeners, channel feature, metrics exporter or WebSocket library in the minimal closure"
  fi
}

check_binary() {
  local bin=$1 syms
  [[ -x "$bin" ]] || { echo "FAIL: $bin is not an executable" >&2; exit 2; }
  syms=$(nm -C --defined-only "$bin" 2>/dev/null || true)
  if ! grep -q 'zeroclaw_runtime::agent' <<<"$syms"; then
    echo "FAIL control: no core agent symbols; the binary is stripped or not a zeroclaw build" >&2
    exit 2
  fi
  if hits=$(grep -E 'zeroclaw_gateway::|zeroclaw_runtime::rpc::wss|zeroclaw_channels::(telegram|discord|webhook|email_channel|acp_channel|filesystem|matrix|slack|line|lark)::|orchestrator::acp_server::' <<<"$syms" | head -5) && [[ -n "$hits" ]]; then
    echo "FAIL binary: forbidden symbols are linked:" >&2
    echo "$hits" >&2
    fail=1
  else
    echo "PASS binary: no gateway, WSS or channel-adapter symbols"
  fi
  # The relay bridge, enrollment server and local RPC listener are compiled out by
  # `daemon-listeners`; this symbol check backs up that gate. Controls first: sibling
  # client helpers from the same modules must be linked, otherwise a pattern typo
  # would pass this check.
  for control in 'zeroclaw_runtime::relay::ensure_node_id' 'zeroclaw_runtime::enroll::paircode_admin::' 'zeroclaw_runtime::rpc::local::'; do
    if ! grep -qF -- "$control" <<<"$syms"; then
      echo "FAIL control: expected client symbol '$control' is not linked" >&2
      exit 2
    fi
  done
  for entry in 'zeroclaw_runtime::relay::run_relay_bridge' 'zeroclaw_runtime::enroll::serve' 'zeroclaw_runtime::enroll::EnrollServer' 'zeroclaw_runtime::rpc::local::run_local_listener' 'zeroclaw_runtime::rpc::local::LocalTransport'; do
    if grep -qF -- "$entry" <<<"$syms"; then
      echo "FAIL binary: listener entry point '$entry' is linked" >&2
      fail=1
    fi
  done
  if [[ "$fail" -eq 0 ]]; then
    echo "PASS binary: relay bridge, enrollment server and local RPC listener entry points are not linked"
  fi
}

case "${1:-}" in
  tree) check_tree ;;
  binary) check_binary "${2:?usage: $0 binary PATH}" ;;
  *) echo "usage: $0 tree | binary PATH" >&2; exit 64 ;;
esac
exit "$fail"
