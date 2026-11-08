//! HORO-2381: the local-agent-minimal binary must refuse every listener,
//! service and channel-receive command before touching config or opening a socket.
#![cfg(feature = "local-agent-minimal")]

use std::process::Command;

fn run_refused(args: &[&str]) -> String {
    let config_dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_zeroclaw"))
        .arg("--config-dir")
        .arg(config_dir.path())
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .expect("spawn zeroclaw");
    assert!(
        !output.status.success(),
        "`zeroclaw {}` must be refused in the minimal profile",
        args.join(" ")
    );
    assert!(
        std::fs::read_dir(config_dir.path())
            .unwrap()
            .next()
            .is_none(),
        "a refused command must not create config or state under the config dir"
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn ingress_commands_are_refused_before_any_setup() {
    for (args, surface) in [
        (vec!["daemon"], "daemon"),
        (vec!["gateway"], "gateway"),
        (vec!["acp"], "acp"),
        (vec!["channel", "start"], "channel start"),
        (vec!["service", "install"], "service"),
        (
            vec![
                "relay",
                "claim",
                "synthetic-token",
                "--control",
                "https://control.invalid",
            ],
            "relay",
        ),
    ] {
        let stderr = run_refused(&args);
        assert!(
            stderr.contains(&format!(
                "`zeroclaw {surface}` is not available in the local-agent-minimal profile"
            )),
            "refusal for `{surface}` must name the profile; got: {stderr}"
        );
    }
}

fn run_with_config(config_toml: &str, args: &[&str]) -> std::process::Output {
    let config_dir = tempfile::tempdir().unwrap();
    std::fs::write(config_dir.path().join("config.toml"), config_toml).unwrap();
    Command::new(env!("CARGO_BIN_EXE_zeroclaw"))
        .arg("--config-dir")
        .arg(config_dir.path())
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .expect("spawn zeroclaw")
}

#[test]
fn hostile_config_enabling_remote_surfaces_is_refused() {
    for (toml, needle) in [
        ("[wss]\nenabled = true\n", "[wss] enabled"),
        ("[relay]\nenabled = true\n", "[relay] enabled"),
        ("[enroll]\nenabled = true\n", "[enroll] enabled"),
        ("[a2a.server]\nenabled = true\n", "[a2a.server] enabled"),
        (
            "[tunnel]\ntunnel_provider = \"tailscale\"\n",
            "[tunnel] provider tailscale",
        ),
        (
            "[channels.telegram.default]\nbot_token = \"synthetic-test-token\"\n",
            "[channels.telegram] configured",
        ),
    ] {
        let output = run_with_config(toml, &["cron", "list"]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "`{needle}` must be refused");
        assert!(
            stderr.contains(needle) && stderr.contains("local-agent-minimal"),
            "refusal must name `{needle}` and the profile; got: {stderr}"
        );
    }
}

#[test]
fn hostile_config_does_not_block_repair_commands() {
    let output = run_with_config("[wss]\nenabled = true\n", &["status"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("local-agent-minimal profile does not provide"),
        "repair commands must stay reachable; got: {stderr}"
    );
}

#[test]
fn default_config_is_not_refused_as_hostile() {
    let output = run_with_config("", &["cron", "list"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && !stderr.contains("does not provide"),
        "a clean config must pass the hostile-config gate; got: {stderr}"
    );
}

#[test]
fn service_teardown_stays_reachable_for_units_from_a_standard_build() {
    let output = run_with_config("", &["service", "status"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("is not available in the local-agent-minimal profile"),
        "service teardown/inspection must not be profile-refused; got: {stderr}"
    );
}
