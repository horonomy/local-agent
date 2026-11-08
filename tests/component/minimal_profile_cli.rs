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
        (vec!["service", "status"], "service"),
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
