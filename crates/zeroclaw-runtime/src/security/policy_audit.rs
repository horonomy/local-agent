//! Bounded, local-only policy decision log (ADR-019 D6).
//!
//! [`FileDecisionSink`] appends one JSON line per [`DecisionRecord`] to
//! `policy-audit.jsonl`. The file never exceeds `max_size_mb`; at the limit it
//! rotates to `policy-audit.jsonl.1` and so on, keeping `generations` rotated
//! files, so the total never exceeds `max_size_mb * (generations + 1)`
//! (validated against the 64 MiB ceiling in `[governance.audit]`). Records
//! carry digests only; the writer never sees raw arguments or tool output.
//! Any failure is returned to the caller, which turns it into a deny.
//!
//! Concurrency: [`FileDecisionSink::shared`] hands out one sink per directory
//! and limits, so threads of a process share one mutex; an exclusive `flock`
//! on `policy-audit.lock` additionally serialises writers across sinks and
//! processes, so rotation can never interleave with another writer.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result, bail};
use parking_lot::Mutex;
use zeroclaw_config::governance::{GovernanceAuditConfig, GovernanceConfig};

use super::policy_contract::{DecisionRecord, DecisionSink};

/// File name of the live log; rotated generations append `.1`, `.2`, ...
pub const POLICY_AUDIT_FILE: &str = "policy-audit.jsonl";

const LOCK_FILE: &str = "policy-audit.lock";
const MIB: u64 = 1024 * 1024;

type SinkKey = (PathBuf, u64, u32);
static SHARED: OnceLock<Mutex<HashMap<SinkKey, Arc<FileDecisionSink>>>> = OnceLock::new();

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

    /// The process-wide sink for this directory and limits.
    pub fn shared(dir: &Path, config: &GovernanceAuditConfig) -> Arc<Self> {
        let max_bytes = config.max_size_mb.saturating_mul(MIB);
        let key = (dir.to_path_buf(), max_bytes, config.generations);
        let map = SHARED.get_or_init(Default::default);
        Arc::clone(
            map.lock().entry(key).or_insert_with(|| {
                Arc::new(Self::with_max_bytes(dir, max_bytes, config.generations))
            }),
        )
    }

    /// Check now that the log can be opened, creating the directory. Lets
    /// startup report an unwritable location instead of every call failing.
    pub fn probe(&self) -> Result<()> {
        let _guard = self.write_lock.lock();
        ensure_dir(&self.dir).context("create policy audit directory")?;
        let _flock = DirLock::acquire(&self.dir)?;
        let current = self.live_len()?;
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
    builder.create(dir)?;
    tighten(&fs::metadata(dir)?, dir, 0o700);
    Ok(())
}

/// Restrict a pre-existing directory or file to the owner, but only when this
/// user owns it (a directory we do not own is not ours to chmod).
#[cfg(unix)]
fn tighten(meta: &fs::Metadata, path: &Path, mode: u32) {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    // SAFETY: geteuid has no preconditions and cannot fail.
    if meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o777 != mode {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    }
}

#[cfg(not(unix))]
fn tighten(_meta: &fs::Metadata, _path: &Path, _mode: u32) {}

/// Exclusive advisory lock on the directory's lock file, held for one write.
struct DirLock {
    #[cfg(unix)]
    _file: File,
}

impl DirLock {
    #[cfg(unix)]
    fn acquire(dir: &Path) -> Result<Self> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        let path = dir.join(LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .context("open policy audit lock")?;
        // SAFETY: the descriptor is valid for the life of `file`.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            bail!("lock policy audit directory");
        }
        Ok(Self { _file: file })
    }

    #[cfg(not(unix))]
    fn acquire(_dir: &Path) -> Result<Self> {
        Ok(Self {})
    }
}

/// How a session obtains its decision sink.
#[derive(Clone, Default)]
pub enum AuditWiring {
    /// Build the file sink from `[governance.audit]` and the data directory.
    #[default]
    FromConfig,
    /// Record nothing.
    Disabled,
    /// Use this sink regardless of configuration.
    Sink(Arc<dyn DecisionSink>),
}

/// Resolve the sink for a session. `audit.enabled = false` turns recording
/// off for every wiring except an explicit sink.
pub fn resolve_sink(
    wiring: AuditWiring,
    governance: &GovernanceConfig,
    data_dir: &Path,
) -> Option<Arc<dyn DecisionSink>> {
    match wiring {
        AuditWiring::Sink(sink) => Some(sink),
        AuditWiring::Disabled => None,
        AuditWiring::FromConfig if !governance.audit.enabled => None,
        AuditWiring::FromConfig => {
            let sink = FileDecisionSink::shared(data_dir, &governance.audit);
            if sink.probe().is_err() {
                // Every tool call will now be denied `audit_write_failed`.
                // Set `governance.audit.enabled = false` to opt out.
                zeroclaw_log::record!(
                    WARN,
                    zeroclaw_log::Event::new(module_path!(), zeroclaw_log::Action::Fail),
                    "policy audit location is not writable; tool calls will be denied"
                );
            }
            Some(sink)
        }
    }
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
