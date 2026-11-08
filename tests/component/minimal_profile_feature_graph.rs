//! Pins the `local-agent-minimal` feature graph (HORO-2381): the profile is only
//! `agent-runtime`, and the compile-time guard must name the root features that
//! bring in a gateway, ACP, web or listener-bearing channel surface.

fn root_features() -> toml::Table {
    include_str!("../../Cargo.toml")
        .parse::<toml::Table>()
        .expect("root Cargo.toml must parse")["features"]
        .as_table()
        .expect("root Cargo.toml must have a [features] table")
        .clone()
}

fn members(features: &toml::Table, name: &str) -> Vec<String> {
    features[name]
        .as_array()
        .unwrap_or_else(|| panic!("`{name}` must be a feature list"))
        .iter()
        .map(|v| v.as_str().expect("feature members are strings").to_string())
        .collect()
}

#[test]
fn minimal_profile_is_exactly_agent_core_and_never_a_default() {
    let features = root_features();
    assert_eq!(members(&features, "local-agent-minimal"), ["agent-core"]);
    assert_eq!(
        members(&features, "agent-runtime"),
        ["agent-core", "daemon-listeners"],
        "agent-runtime must stay the core plus the inbound listeners"
    );
    assert!(
        !members(&features, "default")
            .iter()
            .any(|m| m == "local-agent-minimal"),
        "the marker feature must never be a default"
    );
}

#[test]
fn compile_guard_rejects_listeners_bundles_and_every_channel_feature() {
    let lib = include_str!("../../src/lib.rs");
    let start = lib
        .find("feature = \"local-agent-minimal\"")
        .expect("src/lib.rs must carry the local-agent-minimal compile guard");
    let guard = &lib[start
        ..start
            + lib[start..]
                .find("compile_error!")
                .expect("guard ends at compile_error!")];
    let features = root_features();
    let fixed = [
        "daemon-listeners",
        "gateway",
        "acp-bridge",
        "embedded-web",
        "default-channels",
        "channels-full",
        "whatsapp-web",
    ];
    // Every messaging adapter feature in the manifest, so a newly added channel
    // cannot slip into the profile unnoticed.
    let channels = features.keys().filter(|name| name.starts_with("channel-"));
    for ingress in fixed.iter().copied().chain(channels.map(String::as_str)) {
        assert!(
            features.contains_key(ingress),
            "`{ingress}` must exist in the manifest"
        );
        assert!(
            guard.contains(&format!("feature = \"{ingress}\"")),
            "compile guard must reject `{ingress}` alongside local-agent-minimal"
        );
    }
}
