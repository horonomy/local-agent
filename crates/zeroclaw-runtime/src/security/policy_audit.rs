//! Bounded, local-only policy decision log (ADR-019 D6).
//!
//! [`FileDecisionSink`] appends one JSON line per [`DecisionRecord`] to
//! `policy-audit.jsonl`. The file never exceeds `max_size_mb`; at the limit it
//! rotates to `policy-audit.jsonl.1` and so on, keeping `generations` rotated
//! files, so the total never exceeds `max_size_mb * (generations + 1)`
//! (validated against the 64 MiB ceiling in `[governance.audit]`). Records
//! carry digests only; the writer never sees raw arguments or tool output.
//! Any failure is returned to the caller, which turns it into a deny.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use parking_lot::Mutex;
use zeroclaw_config::governance::GovernanceAuditConfig;

use super::policy_contract::{DecisionRecord, DecisionSink};

/// File name of the live log; rotated generations append `.1`, `.2`, ...
pub const POLICY_AUDIT_FILE: &str = "policy-audit.jsonl";

const MIB: u64 = 1024 * 1024;

pub struct FileDecisionSink {
    dir: PathBuf,
    max_bytes: u64,
    generations: u32,
    write_lock: Mutex<()>,
}

impl FileDecisionSink {
    /// Sink writing under `dir`. Nothing touches the disk until the first
    /// record, so an unwritable directory surfaces as a denied call.
    pub fn new(dir: impl Into<PathBuf>, config: &GovernanceAuditConfig) -> Self {
        Self::with_max_bytes(
            dir,
            config.max_size_mb.saturating_mul(MIB),
            config.generations,
        )
    }

    fn with_max_bytes(dir: impl Into<PathBuf>, max_bytes: u64, generations: u32) -> Self {
        Self {
            dir: dir.into(),
            max_bytes,
            generations,
            write_lock: Mutex::new(()),
        }
    }

    fn live_path(&self) -> PathBuf {
        self.dir.join(POLICY_AUDIT_FILE)
    }

    fn generation_path(&self, n: u32) -> PathBuf {
        self.dir.join(format!("{POLICY_AUDIT_FILE}.{n}"))
    }

    fn rotate(&self) -> io::Result<()> {
        if self.generations == 0 {
            return fs::remove_file(self.live_path());
        }
        match fs::remove_file(self.generation_path(self.generations)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        for n in (1..self.generations).rev() {
            match fs::rename(self.generation_path(n), self.generation_path(n + 1)) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        fs::rename(self.live_path(), self.generation_path(1))
    }

    fn open_live(&self) -> Result<File> {
        let path = self.live_path();
        // A planted symlink would redirect the log; refuse it.
        if let Ok(meta) = fs::symlink_metadata(&path)
            && !meta.file_type().is_file()
        {
            bail!("policy audit path is not a regular file");
        }
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(&path).context("open policy audit file")
    }

    /// Delete the live log and every rotated generation. Returns how many
    /// files were removed. Other files in the directory are untouched.
    pub fn delete(&self) -> Result<usize> {
        let _guard = self.write_lock.lock();
        delete_policy_audit(&self.dir)
    }
}

impl DecisionSink for FileDecisionSink {
    fn record(&self, record: &DecisionRecord) -> Result<()> {
        let mut line = serde_json::to_vec(record).context("serialize policy audit record")?;
        line.push(b'\n');
        if line.len() as u64 > self.max_bytes {
            bail!("policy audit record exceeds the configured file size");
        }
        let _guard = self.write_lock.lock();
        ensure_dir(&self.dir).context("create policy audit directory")?;
        let current = match fs::symlink_metadata(self.live_path()) {
            Ok(meta) => meta.len(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => 0,
            Err(e) => return Err(e).context("stat policy audit file"),
        };
        if current > 0 && current + line.len() as u64 > self.max_bytes {
            self.rotate().context("rotate policy audit file")?;
        }
        let mut file = self.open_live()?;
        file.write_all(&line).context("write policy audit record")?;
        file.flush().context("flush policy audit record")?;
        Ok(())
    }
}

fn ensure_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Delete `policy-audit.jsonl` and its rotated generations under `dir`.
/// Returns the number of files removed; a missing log is not an error.
pub fn delete_policy_audit(dir: &Path) -> Result<usize> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e).context("read policy audit directory"),
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry.context("read policy audit directory entry")?;
        let name = entry.file_name();
        let is_audit_file = name.to_str().is_some_and(|name| {
            name == POLICY_AUDIT_FILE
                || name
                    .strip_prefix(POLICY_AUDIT_FILE)
                    .and_then(|rest| rest.strip_prefix('.'))
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        });
        if is_audit_file {
            fs::remove_file(entry.path()).context("delete policy audit file")?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests;
