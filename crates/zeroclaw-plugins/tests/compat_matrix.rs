//! Manifest-level compatibility-matrix evidence for the WASM plugin host
//! (HORO-2391). Each test backs a row of
//! `docs/book/src/local-agent/compatibility-matrix.md`.
//!
//! These tests need no compiled component: admission reads and judges the
//! manifest before any payload is instantiated. ABI-pin behaviour (HORO-2390)
//! and component-level behaviour are covered by the unit tests in `component.rs`
//! / `validate.rs` and by the `*_e2e.rs` fixtures, and are not repeated here.

use std::path::PathBuf;

use tempfile::tempdir;
use zeroclaw_plugins::host::PluginHost;
use zeroclaw_plugins::{PluginCapability, PluginManifest, PluginPermission};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn source_with(manifest: &str) -> tempfile::TempDir {
    let source = tempdir().unwrap();
    std::fs::write(source.path().join("manifest.toml"), manifest).unwrap();
    std::fs::write(source.path().join("plugin.wasm"), b"\0asm").unwrap();
    source
}

/// Installs `manifest` into a fresh host. The returned guard keeps the plugins
/// root alive for as long as the caller inspects the host.
fn install(manifest: &str) -> (PluginHost, tempfile::TempDir, Result<String, String>) {
    let source = source_with(manifest);
    let home = tempdir().unwrap();
    let mut host = PluginHost::from_plugins_dir(home.path()).unwrap();
    let result = host
        .install(source.path().to_str().unwrap())
        .map_err(|e| e.to_string());
    (host, home, result)
}

/// Every canonical in-tree fixture manifest is a valid manifest for this host
/// and asks only for capabilities and permissions the host implements.
#[test]
fn in_tree_fixture_manifests_parse_and_request_only_known_surface() {
    let mut seen = Vec::new();
    for entry in std::fs::read_dir(fixtures_dir()).unwrap() {
        let path = entry.unwrap().path().join("plugin-manifest.toml");
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let manifest: PluginManifest =
            toml::from_str(&text).unwrap_or_else(|e| panic!("{} must parse: {e}", path.display()));
        assert!(!manifest.capabilities.is_empty(), "{}", path.display());
        assert!(
            manifest.egress.hosts.iter().all(|h| !h.contains("://")),
            "{}",
            path.display()
        );
        seen.push(manifest.name);
    }
    assert!(
        seen.len() >= 3,
        "expected the tool, channel and memory fixtures at minimum; saw {seen:?}"
    );
}

/// A capability that would mean "run a server" or "listen" does not exist in
/// the manifest vocabulary: the manifest is refused and the error names the
/// rejected value and the accepted ones.
#[test]
fn install_refuses_unknown_capability_with_the_accepted_list() {
    for cap in ["listener", "server", "webhook", "daemon"] {
        let (_host, _home, result) = install(&format!(
            "name = \"p\"\nversion = \"0.1.0\"\nwasm_path = \"plugin.wasm\"\ncapabilities = [\"{cap}\"]\n"
        ));
        let err = result.expect_err(cap);
        assert!(err.contains(cap), "error must name `{cap}`: {err}");
        assert!(
            err.contains("tool"),
            "error must list accepted values: {err}"
        );
    }
}

/// Likewise for permissions: only client-side, host-mediated permissions exist.
#[test]
fn install_refuses_unknown_permission_such_as_a_listener() {
    for perm in [
        "socket_server",
        "listen",
        "bind",
        "shell",
        "exec",
        "env_all",
    ] {
        let (_host, _home, result) = install(&format!(
            "name = \"p\"\nversion = \"0.1.0\"\nwasm_path = \"plugin.wasm\"\ncapabilities = [\"tool\"]\npermissions = [\"{perm}\"]\n"
        ));
        let err = result.expect_err(perm);
        assert!(err.contains(perm), "error must name `{perm}`: {err}");
    }
}

#[test]
fn install_refuses_a_manifest_that_declares_no_capability() {
    let (_host, _home, result) = install(
        "name = \"p\"\nversion = \"0.1.0\"\nwasm_path = \"plugin.wasm\"\ncapabilities = []\n",
    );
    let err = result.expect_err("no capability");
    assert!(err.contains("declares no capabilities"), "{err}");
}

/// Characterization: keys the manifest schema does not define are ignored, not
/// rejected. The matrix records this so nobody reads such a key as a grant:
/// the admitted manifest carries no permission and no egress destination from
/// it, and the host gives it no meaning.
#[test]
fn unknown_manifest_keys_are_ignored_and_grant_nothing() {
    let (host, _home, result) = install(
        "name = \"p\"\nversion = \"0.1.0\"\nwasm_path = \"plugin.wasm\"\ncapabilities = [\"tool\"]\n\
         listen = \"0.0.0.0:8080\"\nallowed_hosts = [\"*\"]\n\n[network]\ninbound = true\n",
    );
    result.expect("an unknown key does not by itself refuse the package");
    let info = host.get_plugin("p").expect("installed");
    assert!(info.permissions.is_empty(), "no permission is conferred");
    let manifest = host.manifest("p").unwrap();
    assert!(manifest.egress.hosts.is_empty(), "no egress is conferred");
    assert_eq!(manifest.capabilities, vec![PluginCapability::Tool]);
}

/// An observer capability has no loader in this host: the package can be
/// listed, but it is neither a tool plugin nor a channel plugin, so nothing
/// activates it.
#[test]
fn observer_only_plugin_is_never_activated_as_tool_or_channel() {
    let (host, _home, result) = install(
        "name = \"obs\"\nversion = \"0.1.0\"\nwasm_path = \"plugin.wasm\"\ncapabilities = [\"observer\"]\n",
    );
    result.expect("observer manifest is admitted");
    assert!(host.tool_plugins().is_empty());
    assert!(host.channel_plugins().is_empty());
}

/// A declared egress destination is an attestation, never a grant; the install
/// does not add a permission the manifest did not request.
#[test]
fn declared_egress_without_http_permission_requests_no_network_permission() {
    let (host, _home, result) = install(
        "name = \"eg\"\nversion = \"0.1.0\"\nwasm_path = \"plugin.wasm\"\ncapabilities = [\"tool\"]\n\n[egress]\nhosts = [\"api.example.com\"]\n",
    );
    result.expect("valid egress declaration is admitted");
    let info = host.get_plugin("eg").unwrap();
    assert!(!info.permissions.contains(&PluginPermission::HttpClient));
    assert!(!info.permissions.contains(&PluginPermission::SocketClient));
    assert!(
        !info
            .permissions
            .contains(&PluginPermission::WebSocketClient)
    );
}
