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
        "voice-wake",
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

/// Transitive closure of a feature, following root features and the `crate/feature`
/// references into the workspace crates that carry the capability.
fn closure(root: &toml::Table, start: &str) -> std::collections::BTreeSet<String> {
    fn crate_features(name: &str) -> toml::Table {
        let manifest = match name {
            "zeroclaw-runtime" => include_str!("../../crates/zeroclaw-runtime/Cargo.toml"),
            "zeroclaw-tools" => include_str!("../../crates/zeroclaw-tools/Cargo.toml"),
            "zeroclaw-config" => include_str!("../../crates/zeroclaw-config/Cargo.toml"),
            _ => return toml::Table::new(),
        };
        manifest
            .parse::<toml::Table>()
            .expect("crate manifest parses")["features"]
            .as_table()
            .expect("crate has [features]")
            .clone()
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut queue = vec![(None::<String>, start.to_string())];
    while let Some((krate, feature)) = queue.pop() {
        let key = format!("{}/{feature}", krate.as_deref().unwrap_or("root"));
        // `agent-core` is the shared base the minimal profile already ships.
        if !seen.insert(key) || feature == "agent-core" {
            continue;
        }
        let table = krate
            .as_deref()
            .map_or_else(|| root.clone(), crate_features);
        if !table.contains_key(&feature) {
            continue;
        }
        for member in members(&table, &feature) {
            if member.starts_with("dep:") {
                continue;
            } else if let Some((c, f)) = member.split_once('/') {
                queue.push((Some(c.to_string()), f.to_string()));
            } else {
                queue.push((krate.clone(), member));
            }
        }
    }
    seen
}

#[test]
fn local_capability_features_pull_no_listener_channel_or_external_tools() {
    let features = root_features();
    let forbidden = [
        "tools-external",
        "tools-compat",
        "daemon-listeners",
        "gateway",
        "acp-bridge",
        "embedded-web",
        "default-channels",
        "channels-full",
        "browser-native",
    ];
    for capability in ["local-browser", "local-desktop"] {
        let reached = closure(&features, capability);
        assert!(
            reached.contains(&format!("root/{capability}")),
            "`{capability}` must exist"
        );
        for entry in &reached {
            let feature = entry.rsplit('/').next().unwrap();
            assert!(
                !forbidden.contains(&feature) && !feature.starts_with("channel-"),
                "`{capability}` must not reach `{entry}`"
            );
        }
    }
}

#[test]
fn local_capabilities_stay_out_of_minimal_default_and_installer_rows() {
    let features = root_features();
    assert_eq!(members(&features, "local-agent-minimal"), ["agent-core"]);
    for capability in ["local-browser", "local-desktop"] {
        for bundle in [
            "default",
            "local-agent-minimal",
            "agent-runtime",
            "tools-compat",
        ] {
            assert!(
                !closure(&features, bundle).contains(&format!("root/{capability}")),
                "`{bundle}` must not enable `{capability}`"
            );
        }
        let manifest = include_str!("../../Cargo.toml")
            .parse::<toml::Table>()
            .unwrap();
        let non_row = manifest["package"]["metadata"]["zeroclaw"]["non_row_features"]
            .as_array()
            .expect("non_row_features is a list");
        assert!(
            non_row.iter().any(|v| v.as_str() == Some(capability)),
            "`{capability}` must be a non-row feature so generated installer files stay identical"
        );
    }
}

#[test]
fn capture_module_is_gated_by_tools_external_or_local_desktop_only() {
    let lib = include_str!("../../crates/zeroclaw-tools/src/lib.rs");
    assert!(lib.contains(
        "#[cfg(any(feature = \"tools-external\", feature = \"local-desktop\"))]\npub mod capture;"
    ));
    let tools = include_str!("../../crates/zeroclaw-tools/Cargo.toml");
    assert!(tools.contains("local-desktop = [\"zeroclaw-config/local-desktop\"]"));
}

#[test]
fn browser_module_is_gated_by_tools_external_or_local_browser_only() {
    let lib = include_str!("../../crates/zeroclaw-tools/src/lib.rs");
    assert!(lib.contains(
        "#[cfg(any(feature = \"tools-external\", feature = \"local-browser\"))]\npub mod browser;"
    ));
    for still_external in [
        "browser_open",
        "browser_delegate",
        "text_browser",
        "screenshot",
    ] {
        assert!(
            lib.contains(&format!(
                "#[cfg(feature = \"tools-external\")]\npub mod {still_external};"
            )),
            "`{still_external}` must stay behind tools-external"
        );
    }
}

#[test]
fn computer_use_module_is_gated_by_local_desktop_only() {
    let lib = include_str!("../../crates/zeroclaw-tools/src/lib.rs");
    assert!(
        lib.contains("#[cfg(feature = \"local-desktop\")]\npub mod computer_use;"),
        "`computer_use` must compile only under `local-desktop`"
    );
    let features = root_features();
    let reached = closure(&features, "local-desktop");
    for needed in [
        "zeroclaw-runtime/local-desktop",
        "zeroclaw-tools/local-desktop",
    ] {
        assert!(
            reached.contains(needed),
            "`local-desktop` must forward to `{needed}`"
        );
    }
}
