//! Pins the `local-agent-minimal` feature graph (HORO-2381): the profile is only
//! `agent-runtime`, and the compile-time guard must name every root feature that
//! brings in a gateway, ACP, web or messaging ingress surface.

fn root_feature_values(feature: &str) -> Vec<String> {
    let manifest = include_str!("../../Cargo.toml");
    let prefix = format!("{feature} ");
    let line = manifest
        .lines()
        .find(|line| line.trim_start().starts_with(&prefix))
        .unwrap_or_else(|| panic!("root Cargo.toml must define the `{feature}` feature"));
    let list_start = line.find('[').expect("feature line must carry a list");
    let list_end = line.rfind(']').expect("feature line must close its list");
    line[list_start + 1..list_end]
        .split(',')
        .map(|v| v.trim().trim_matches('"').to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

#[test]
fn minimal_profile_is_exactly_agent_runtime() {
    assert_eq!(
        root_feature_values("local-agent-minimal"),
        vec!["agent-runtime".to_string()],
        "the minimal profile must add nothing beyond agent-runtime"
    );
}

#[test]
fn compile_guard_names_the_known_ingress_features() {
    let lib = include_str!("../../src/lib.rs");
    let guard_start = lib
        .find("feature = \"local-agent-minimal\"")
        .expect("src/lib.rs must carry the local-agent-minimal compile guard");
    let guard_len = lib[guard_start..]
        .find("compile_error!")
        .expect("the guard must end at its compile_error!");
    let guard = &lib[guard_start..guard_start + guard_len];
    for ingress in [
        "gateway",
        "acp-bridge",
        "embedded-web",
        "default-channels",
        "channels-full",
        "channel-acp-server",
        "channel-webhook",
        "channel-voice-call",
        "channel-line",
        "channel-lark",
        "whatsapp-web",
    ] {
        assert!(
            guard.contains(&format!("feature = \"{ingress}\"")),
            "compile guard must reject `{ingress}` alongside local-agent-minimal"
        );
    }
}

#[test]
fn default_features_do_not_include_the_minimal_profile() {
    let manifest = include_str!("../../Cargo.toml");
    let start = manifest
        .find("\ndefault = [")
        .expect("root Cargo.toml must define the `default` feature");
    let block = &manifest[start..];
    let defaults = &block[..block.find(']').expect("default list must close")];
    assert!(
        !defaults.contains("local-agent-minimal"),
        "the marker feature must never be a default; got: {defaults}"
    );
}
