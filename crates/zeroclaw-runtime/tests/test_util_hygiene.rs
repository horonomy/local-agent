//! The `test-util` feature mints a permit-all policy gate and raw tool
//! registries. It must never reach a shipped build: no workspace manifest may
//! enable it on `zeroclaw-runtime` or `zeroclaw-channels` outside
//! `[dev-dependencies]`, and no feature may forward it.

use std::path::{Path, PathBuf};

const GUARDED: &[&str] = &["zeroclaw-runtime", "zeroclaw-channels"];

fn manifests() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut out = vec![root.join("Cargo.toml")];
    for dir in ["crates", "apps"] {
        if let Ok(entries) = std::fs::read_dir(root.join(dir)) {
            for entry in entries.flatten() {
                let manifest = entry.path().join("Cargo.toml");
                if manifest.is_file() {
                    out.push(manifest);
                }
            }
        }
    }
    out
}

fn enables_test_util(spec: &toml::Value) -> bool {
    spec.get("features")
        .and_then(|f| f.as_array())
        .is_some_and(|f| f.iter().any(|v| v.as_str() == Some("test-util")))
}

fn offending_deps(table: Option<&toml::Value>, label: &str, file: &Path, bad: &mut Vec<String>) {
    let Some(table) = table.and_then(|t| t.as_table()) else {
        return;
    };
    for (name, spec) in table {
        if GUARDED.contains(&name.as_str()) && enables_test_util(spec) {
            bad.push(format!("{}: {label}.{name}", file.display()));
        }
    }
}

#[test]
fn test_util_is_only_enabled_by_dev_dependencies() {
    let mut bad = Vec::new();
    let mut seen = 0;
    for file in manifests() {
        let text = std::fs::read_to_string(&file).unwrap();
        let doc: toml::Value = toml::from_str(&text).unwrap();
        seen += 1;
        for label in ["dependencies", "build-dependencies"] {
            offending_deps(doc.get(label), label, &file, &mut bad);
        }
        offending_deps(
            doc.get("workspace").and_then(|w| w.get("dependencies")),
            "workspace.dependencies",
            &file,
            &mut bad,
        );
        if let Some(targets) = doc.get("target").and_then(|t| t.as_table()) {
            for (cfg, body) in targets {
                for label in ["dependencies", "build-dependencies"] {
                    offending_deps(
                        body.get(label),
                        &format!("target.{cfg}.{label}"),
                        &file,
                        &mut bad,
                    );
                }
            }
        }
        if let Some(features) = doc.get("features").and_then(|f| f.as_table()) {
            for (feature, members) in features {
                let forwards = members.as_array().is_some_and(|m| {
                    m.iter().filter_map(|v| v.as_str()).any(|v| {
                        GUARDED.iter().any(|c| v == format!("{c}/test-util"))
                            || (feature != "test-util" && v == "test-util")
                    })
                });
                if forwards {
                    bad.push(format!("{}: features.{feature}", file.display()));
                }
            }
        }
    }
    assert!(seen > 5, "positive control: workspace manifests were found");
    assert!(bad.is_empty(), "test-util must stay dev-only: {bad:?}");
}
