//! Architecture gate for standard GitHub-hosted runner and cache selection.
//! Runner labels stay explicit so job scheduling does not depend on formatting.
//! The inventories preserve every compile, housekeeping and kernel-specific job.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use regex::Regex;

/// Fixed standard images avoid ubuntu-latest image migration and paid runners.
const RUNNER_LABEL: &str = "ubuntu-24.04";
const HOUSEKEEPING_LABEL: &str = RUNNER_LABEL;
const WINDOWS_RECOVERY_LABEL: &str = "windows-2025";

/// Workflow-qualified IDs keep same-named jobs in different workflows distinct.
const HOUSEKEEPING_JOBS: [&str; 19] = [
    "ci.yml/fmt",
    "ci.yml/crates-preflight-changes",
    "ci.yml/master-debounce",
    "ci.yml/gate",
    "ci.yml/history-guard",
    "ci.yml/repo-structure",
    "ci.yml/docs-style",
    "ci.yml/zerocode-rpc-boundary",
    "ci.yml/parallel-runtime-test-changes",
    "ci.yml/path-changes",
    "ci.yml/relay-container-smoke-changes",
    "ci.yml/windows-clippy-tools-changes",
    "ci.yml/windows-required-changes",
    "ci.yml/nix-eval",
    "ci.yml/nix-hash-drift",
    "ci.yml/relay-container-smoke",
    "ci.yml/security",
    "ci.yml/web-permission-tests",
    "windows-tests.yml/windows-test-scope",
];

/// Landlock must retain the GitHub-hosted Linux kernel security check.
const HOSTED_LINUX_JOBS: [&str; 1] = ["test-landlock"];

/// Every job that compiles the workspace on the standard Linux image.
const COMPILE_JOBS: [&str; 14] = [
    "lint",
    "build",
    "check",
    "check-plugin-backends",
    "msrv",
    "check-32bit",
    "bench",
    "test",
    "test-channel-features",
    "test-tool-features",
    "test-cli-only",
    "memory-postgres-test",
    "parallel-runtime-test",
    "installer-drift",
];

/// Compile jobs that call a reusable workflow and hand it the fleet label as
/// its `runner` input. They have no `runs-on` of their own, so they are kept
/// apart from COMPILE_JOBS but still count toward the fleet inventory.
const REUSABLE_COMPILE_JOBS: [&str; 1] = ["crates-preflight"];

fn ci_workflow() -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(root.join(".github/workflows/ci.yml"))
        .expect("failed to read .github/workflows/ci.yml")
}

fn runner_workflow_jobs() -> BTreeMap<String, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/workflows");
    ["ci.yml", "windows-tests.yml"]
        .into_iter()
        .flat_map(|filename| {
            let workflow = fs::read_to_string(root.join(filename))
                .unwrap_or_else(|error| panic!("failed to read {filename}: {error}"));
            job_blocks(&workflow)
                .into_iter()
                .map(move |(name, block)| (format!("{filename}/{name}"), block))
        })
        .collect()
}

/// Split the `jobs:` mapping into `job id -> job body`. Only scans after the
/// top-level `jobs:` key so that `on:` children such as `pull_request:` are
/// never mistaken for jobs.
fn job_blocks(workflow: &str) -> BTreeMap<String, String> {
    let (_, jobs) = workflow
        .split_once("\njobs:\n")
        .expect("workflow must declare a top-level jobs mapping");
    let header = Regex::new(r"(?m)^  ([a-z0-9-]+):$").expect("valid job-header pattern");

    let starts: Vec<(usize, String)> = header
        .captures_iter(jobs)
        .map(|capture| {
            let whole = capture.get(0).expect("match 0 always exists");
            (whole.start(), capture[1].to_string())
        })
        .collect();
    assert!(!starts.is_empty(), "workflow must define at least one job");

    let mut blocks = BTreeMap::new();
    for (index, (offset, name)) in starts.iter().enumerate() {
        let end = starts.get(index + 1).map_or(jobs.len(), |(next, _)| *next);
        let previous = blocks.insert(name.clone(), jobs[*offset..end].to_string());
        assert!(previous.is_none(), "duplicate job id {name} in workflow");
    }
    blocks
}

/// The `needs:` list of a job, empty when it declares none.
fn needs(block: &str) -> Vec<String> {
    let Some(line) = block.lines().find(|line| line.starts_with("    needs: [")) else {
        return Vec::new();
    };
    line.trim_start()
        .trim_start_matches("needs: [")
        .trim_end_matches(']')
        .split(',')
        .map(|entry| entry.trim().to_string())
        .filter(|entry| !entry.is_empty())
        .collect()
}

#[test]
fn compile_jobs_pin_the_runner_label_instead_of_reading_it_from_fmt() {
    let workflow = ci_workflow();
    let blocks = job_blocks(&workflow);

    assert!(
        !workflow.contains("needs.fmt.outputs"),
        "no job may read a runner label or cache provider from fmt: that forces \
         every compile job to wait for a GitHub-hosted runner before it starts"
    );

    for name in COMPILE_JOBS {
        let block = blocks
            .get(name)
            .unwrap_or_else(|| panic!("ci.yml must define the {name} job"));
        assert!(
            !needs(block).iter().any(|dependency| dependency == "fmt"),
            "{name} must not declare needs: [fmt]; the gate job is what keeps a \
             formatting error blocking merge"
        );
        let pins_label = if name == "build" {
            // The Linux leg carries the label through the matrix; macOS names
            // its GitHub-hosted image. Windows is selected independently.
            block.contains(&format!("- os: {RUNNER_LABEL}\n"))
                && block.contains("runs-on: ${{ matrix.os }}\n")
        } else {
            block.contains(&format!("    runs-on: {RUNNER_LABEL}\n"))
        };
        assert!(pins_label, "{name} must run on {RUNNER_LABEL}");
    }
}

#[test]
fn only_the_declared_linux_jobs_claim_the_standard_image() {
    let blocks = runner_workflow_jobs();
    let claiming: BTreeSet<String> = blocks
        .iter()
        .filter(|(_, block)| block.contains(RUNNER_LABEL))
        .map(|(name, _)| name.clone())
        .collect();
    let declared: BTreeSet<String> = COMPILE_JOBS
        .into_iter()
        .chain(REUSABLE_COMPILE_JOBS)
        .chain(HOSTED_LINUX_JOBS)
        .map(|name| format!("ci.yml/{name}"))
        .chain(HOUSEKEEPING_JOBS.into_iter().map(str::to_string))
        .collect();
    assert_eq!(
        claiming, declared,
        "standard Linux jobs must retain their inventory"
    );
}

#[test]
fn rust_cache_uses_only_the_pinned_hosted_provider() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let action = fs::read_to_string(root.join(".github/actions/rust-cache/action.yml")).unwrap();
    assert!(action.contains("uses: Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6"));
    assert!(!action.contains("useblacksmith/"));
    assert!(!ci_workflow().contains("use-blacksmith:"));
}

#[test]
fn windows_recovery_pins_its_runner_and_trusted_cache_writer() {
    let blocks = runner_workflow_jobs();
    let recovery = &blocks["ci.yml/windows-task-owner-recovery"];
    assert!(
        recovery.contains(&format!("    runs-on: {WINDOWS_RECOVERY_LABEL}\n")),
        "recovery must pin the standard Windows image"
    );
    assert!(
        recovery.contains("uses: Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6")
            && !recovery.contains("uses: ./.github/actions/rust-cache"),
        "recovery must use the tested pinned cache action, not an external runner cache"
    );
    assert!(
        recovery.contains("          shared-key: windows-recovery-hosted\n"),
        "recovery needs its own stable cache rather than the GitHub compilation cache"
    );
    assert!(
        recovery.contains("          cache-on-failure: 'false'\n")
            && recovery.contains(
                "          save-if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/master' }}\n"
            ),
        "only successful master pushes may seed recovery; PRs and merge-queue runs must only read"
    );
}

#[test]
fn housekeeping_jobs_pin_the_standard_image() {
    let blocks = runner_workflow_jobs();

    for name in HOUSEKEEPING_JOBS {
        let block = blocks
            .get(name)
            .unwrap_or_else(|| panic!("missing workflow job {name}"));
        assert!(
            block.contains(&format!("    runs-on: {HOUSEKEEPING_LABEL}\n")),
            "{name} must run on {HOUSEKEEPING_LABEL}"
        );
    }
}

#[test]
fn runner_labels_remain_standard_across_workflows() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for entry in fs::read_dir(root.join(".github/workflows")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("yml") {
            continue;
        }
        let workflow = fs::read_to_string(&path).unwrap();
        for line in workflow.lines().filter(|line| {
            line.trim_start().starts_with("runs-on:")
                || line.trim_start().starts_with("- os:")
                || line.trim_start().starts_with("runner:")
        }) {
            assert!(!line.contains("blacksmith-"), "{}: {line}", path.display());
            assert!(
                !line.contains("-large") && !line.contains("-xlarge"),
                "{}: {line}",
                path.display()
            );
        }
    }
    let blocks = runner_workflow_jobs();
    assert!(blocks["ci.yml/test-landlock"].contains("    runs-on: ubuntu-24.04\n"));
}

#[test]
fn milestone_branches_retain_the_same_quality_gate_triggers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for name in [
        "ci.yml",
        "windows-tests.yml",
        "docker-image-pr.yml",
        "codeql.yml",
    ] {
        let workflow = fs::read_to_string(root.join(".github/workflows").join(name)).unwrap();
        assert!(
            workflow.contains("branches: [master, 'release/**']"),
            "{name} must retain master and add milestone release coverage"
        );
    }
}

#[test]
fn the_required_gate_still_waits_for_formatting() {
    let workflow = ci_workflow();
    let blocks = job_blocks(&workflow);
    let gate = blocks.get("gate").expect("ci.yml must define the gate job");

    assert!(
        needs(gate).iter().any(|dependency| dependency == "fmt"),
        "CI Required Gate must keep needing fmt: it is the only thing that still \
         makes a formatting error block merge"
    );
    for name in ["windows-required-changes", "windows-build"] {
        assert!(
            needs(gate).iter().any(|dependency| dependency == name),
            "CI Required Gate must directly consume {name} so selected Windows checks cannot silently skip"
        );
    }
    let build = blocks.get("build").expect("ci.yml must define build");
    assert!(
        !needs(build)
            .iter()
            .any(|dependency| dependency == "windows-required-changes"),
        "Linux/macOS compilation must not wait for Windows selection"
    );
}

#[test]
fn reusable_compile_jobs_pass_the_fleet_label() {
    let workflow = ci_workflow();
    let blocks = job_blocks(&workflow);

    // Reusable jobs must receive the same fixed standard image as their callers.
    for name in REUSABLE_COMPILE_JOBS {
        let block = blocks
            .get(name)
            .unwrap_or_else(|| panic!("ci.yml must define the {name} job"));
        assert!(
            block.contains("    uses: ./.github/workflows/")
                && block.contains(&format!("      runner: {RUNNER_LABEL}\n")),
            "{name} must pass runner: {RUNNER_LABEL} to its reusable workflow"
        );
    }
}

/// The job-level `if:` expression of a job, empty when it declares none.
fn job_if(block: &str) -> String {
    block
        .lines()
        .find(|line| line.starts_with("    if: "))
        .map(|line| line.trim_start().trim_start_matches("if: ").to_string())
        .unwrap_or_default()
}

/// Master pushes wait in `master-debounce` so a superseded push is cancelled
/// before its compile fleet starts. That job is skipped on pull_request and
/// merge_group, and GitHub's implicit `success()` propagates a skipped
/// ancestor transitively, so any job downstream of it that lacks an explicit
/// status function would silently skip on every PR while `CI Required Gate`
/// still reported green. This guard keeps every downstream job explicit.
#[test]
fn jobs_downstream_of_the_master_debounce_never_skip_silently() {
    let workflow = ci_workflow();
    let blocks = job_blocks(&workflow);

    let debounce = blocks
        .get("master-debounce")
        .expect("ci.yml must define the master-debounce job");
    assert_eq!(
        job_if(debounce),
        "github.event_name == 'push'",
        "master-debounce must run only on master pushes, never on PRs or the merge queue"
    );

    let gate = blocks.get("gate").expect("ci.yml must define the gate job");
    assert!(
        needs(gate)
            .iter()
            .any(|dependency| dependency == "master-debounce"),
        "CI Required Gate must need master-debounce, so a debounce failure cannot \
         leave skipped compile jobs behind a green gate"
    );

    let mut downstream: BTreeSet<String> = BTreeSet::from(["master-debounce".to_string()]);
    loop {
        let before = downstream.len();
        for (name, block) in &blocks {
            if needs(block)
                .iter()
                .any(|dependency| downstream.contains(dependency))
            {
                downstream.insert(name.clone());
            }
        }
        if downstream.len() == before {
            break;
        }
    }
    downstream.remove("master-debounce");
    downstream.remove("gate");
    assert!(
        downstream.len() > 10,
        "the compile fleet must wait on master-debounce; found only {downstream:?}"
    );

    for name in &downstream {
        let block = &blocks[name];
        let condition = job_if(block);
        assert!(
            condition.contains("!cancelled()"),
            "{name} is downstream of the push-only master-debounce and must use an \
             explicit `!cancelled()` condition; an implicit success() skips it on PRs"
        );
        for dependency in needs(block) {
            let expected = if dependency == "master-debounce" {
                "needs.master-debounce.result != 'failure'".to_string()
            } else {
                format!("needs.{dependency}.result == 'success'")
            };
            assert!(
                condition.contains(&expected),
                "{name} must check `{expected}` so replacing implicit success() does not \
                 let it run after a failed dependency"
            );
        }
    }
}
