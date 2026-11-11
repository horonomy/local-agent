//! HORO-2385: in the minimal profile only a provider named in the config is
//! contacted. Loopback recorders stand in for the approved endpoint and for one
//! nobody approved; a control in this file shows a recorder does see a
//! connection when one is made, so "zero connections" cannot pass vacuously.
//!
//! These tests run on the host without a sandbox: they prove no connection to the
//! recorders, not that nothing reached a public address. The sandboxed
//! `scripts/local_agent/egress_probe.py` is the evidence for the latter.
#![cfg(feature = "local-agent-minimal")]

use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const REFUSAL: &str = "must name a provider configured";

struct Recorder {
    port: u16,
    seen: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl Recorder {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (counter, halt) = (Arc::clone(&seen), Arc::clone(&stop));
        std::thread::spawn(move || {
            while !halt.load(Ordering::Relaxed) {
                if listener.accept().is_ok() {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        Self { port, seen, stop }
    }

    fn connections(&self) -> usize {
        self.seen.load(Ordering::Relaxed)
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn config(agent_provider: Option<&str>, port: u16, root: &Path) -> String {
    let provider_line = agent_provider
        .map(|p| format!("model_provider = \"{p}\"\n"))
        .unwrap_or_default();
    format!(
        "schema_version = 3\n\
         [providers.models.custom.approved]\n\
         uri = \"http://127.0.0.1:{port}/v1\"\n\
         model = \"approved-model\"\n\
         wire_api = \"chat_completions\"\n\
         timeout_secs = 3\n\
         [agents.probe]\n{provider_line}\
         risk_profile = \"probe\"\nruntime_profile = \"probe\"\n\
         [agents.probe.workspace]\npath = \"{ws}\"\n\
         [agents.probe.memory]\nbackend = \"none\"\n\
         [runtime_profiles.probe]\nagentic = true\nmax_tool_iterations = 1\n\
         [risk_profiles.probe]\nlevel = \"supervised\"\nworkspace_only = true\n\
         allowed_tools = []\nauto_approve = []\n",
        ws = root.join("workspace").display()
    )
}

fn prepare(provider: Option<&str>, recorder: &Recorder) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for dir in ["config", "workspace", "home"] {
        std::fs::create_dir(root.path().join(dir)).unwrap();
    }
    let toml = config(provider, recorder.port, root.path());
    std::fs::write(root.path().join("config/config.toml"), toml).unwrap();
    root
}

/// `zeroclaw --config-dir <root>/config <args…>` with a clean environment.
fn command(root: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_zeroclaw"));
    cmd.env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", root.join("home"))
        .env("LC_ALL", "C")
        .arg("--config-dir")
        .arg(root.join("config"))
        .args(args)
        .stdin(Stdio::null());
    cmd
}

const AGENT: [&str; 5] = ["agent", "--agent", "probe", "--message", "hello"];

fn run_to_exit(root: &Path, args: &[&str]) -> Output {
    command(root, args).output().expect("spawn zeroclaw")
}

/// Start the command and report whether `recorder` is contacted before it exits.
fn reaches(root: &Path, args: &[&str], recorder: &Recorder) -> bool {
    let mut child = command(root, args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn zeroclaw");
    let deadline = Instant::now() + Duration::from_secs(120);
    while recorder.connections() == 0
        && matches!(child.try_wait(), Ok(None))
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    recorder.connections() > 0
}

fn assert_refused_without_echo(output: &Output, needle: &str, context: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "{context}: must be refused; stderr: {stderr}"
    );
    assert!(
        stderr.contains(needle),
        "{context}: refusal must say why; got: {stderr}"
    );
    assert!(
        !stderr.contains("localhost"),
        "{context}: the rejected value must not be echoed (it can carry a credential)"
    );
}

#[test]
fn approved_provider_is_reached() {
    let recorder = Recorder::start();
    let root = prepare(Some("custom.approved"), &recorder);
    assert!(
        reaches(root.path(), &AGENT, &recorder),
        "control: the configured, approved provider must be contacted"
    );
}

#[test]
fn unresolved_or_missing_provider_makes_no_connection() {
    for provider in [Some("custom.ghost"), None] {
        let recorder = Recorder::start();
        let root = prepare(provider, &recorder);
        let output = run_to_exit(root.path(), &AGENT);
        assert_refused_without_echo(
            &output,
            "model_provider does not resolve",
            &format!("{provider:?}"),
        );
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            recorder.connections(),
            0,
            "provider {provider:?}: no endpoint may be contacted"
        );
    }
}

#[test]
fn provider_override_naming_the_configured_alias_is_honored() {
    let recorder = Recorder::start();
    let root = prepare(Some("custom.approved"), &recorder);
    let args: Vec<&str> = AGENT
        .iter()
        .copied()
        .chain(["--model-provider", "custom.approved"])
        .collect();
    assert!(
        reaches(root.path(), &args, &recorder),
        "control: an override naming a configured alias must still work"
    );
}

#[test]
fn every_provider_taking_command_refuses_an_unconfigured_override() {
    let cases: [&[&str]; 5] = [
        &AGENT,
        &["models", "refresh"],
        &["models", "list", "--check"],
        &["doctor", "models"],
        &["doctor", "update-context-windows"],
    ];
    for base in cases {
        let (approved, unapproved) = (Recorder::start(), Recorder::start());
        let root = prepare(Some("custom.approved"), &approved);
        let url = format!("custom:http://localhost:{}", unapproved.port);
        let args: Vec<&str> = base
            .iter()
            .copied()
            .chain(["--model-provider", &url])
            .collect();
        let output = run_to_exit(root.path(), &args);
        assert_refused_without_echo(&output, REFUSAL, &base.join(" "));
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            (approved.connections(), unapproved.connections()),
            (0, 0),
            "`{}`: a refused override must contact no endpoint",
            base.join(" ")
        );
    }
}
