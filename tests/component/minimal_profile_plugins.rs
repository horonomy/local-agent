//! HORO-2391: with the plugin host compiled in, the local-agent-minimal profile
//! still has no channel runtime, so a channel plugin and a `[channels.plugin.*]`
//! config must be refused with an actionable error while a tool plugin installs.
#![cfg(all(feature = "local-agent-minimal", feature = "plugins-wasm"))]

use std::process::Command;

fn run(config_dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_zeroclaw"))
        .arg("--config-dir")
        .arg(config_dir)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .expect("spawn zeroclaw")
}

fn package(root: &std::path::Path, name: &str, capability: &str) -> std::path::PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("manifest.toml"),
        format!(
            "name = \"{name}\"\nversion = \"0.1.0\"\nwasm_path = \"plugin.wasm\"\ncapabilities = [\"{capability}\"]\n"
        ),
    )
    .unwrap();
    std::fs::write(dir.join("plugin.wasm"), b"\0asm").unwrap();
    dir
}

#[test]
fn channel_plugin_install_is_refused_naming_plugin_and_profile() {
    let config = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let dir = package(src.path(), "mail-bridge", "channel");
    let out = run(
        config.path(),
        &["plugin", "install", dir.to_str().unwrap(), "--no-verify"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "must be refused; stderr: {stderr}");
    assert!(
        stderr.contains("mail-bridge")
            && stderr.contains("`channel` capability")
            && stderr.contains("local-agent-minimal"),
        "refusal must name the plugin, capability and profile; got: {stderr}"
    );
}

#[test]
fn tool_plugin_install_is_not_refused_by_the_profile() {
    let config = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let dir = package(src.path(), "echo-tool", "tool");
    let out = run(
        config.path(),
        &["plugin", "install", dir.to_str().unwrap(), "--no-verify"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("local-agent-minimal"),
        "a tool plugin must not be refused by the profile; got: {stderr}"
    );
    assert!(out.status.success(), "tool install failed: {stderr}");
}

#[test]
fn channels_plugin_config_is_refused_naming_the_section() {
    let config = tempfile::tempdir().unwrap();
    std::fs::write(
        config.path().join("config.toml"),
        "schema_version = 3\n\n[channels.plugin.mail]\npackage = \"mail-bridge\"\nenabled = true\n",
    )
    .unwrap();
    let out = run(config.path(), &["cron", "list"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "must be refused; stderr: {stderr}");
    assert!(
        stderr.contains("[channels.plugin.mail] configured")
            && stderr.contains("local-agent-minimal"),
        "got: {stderr}"
    );
}
