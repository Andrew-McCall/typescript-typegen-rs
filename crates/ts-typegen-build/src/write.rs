//! Writing generated files into the source tree.
//!
//! A build script writing outside `OUT_DIR` is unusual enough to warrant explicit
//! guards: the output directory may not overlap a scan root, writes are skipped
//! when content is unchanged, and only files carrying the generated marker are
//! ever deleted.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::render::MARKER;

/// Everything that can go wrong writing generated files to disk.
#[derive(Debug)]
pub enum WriteError {
    /// The output directory overlaps a scan root, which would make generated
    /// files re-scanned on every build.
    Overlap {
        /// The configured output directory.
        output: PathBuf,
        /// The scan root it overlaps.
        scan: PathBuf,
    },
    /// A filesystem operation failed.
    Io(std::io::Error),
    /// [`Config::check`](crate::config::Config::check) mode found the
    /// on-disk bindings differ from what generation would produce; one
    /// formatted diff per changed or missing file.
    Drift(Vec<String>),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::Overlap { output, scan } => write!(
                f,
                "output directory {} overlaps scan root {}; generated files would be \
                 re-scanned and retrigger the build forever",
                output.display(),
                scan.display()
            ),
            WriteError::Io(e) => write!(f, "{e}"),
            WriteError::Drift(diffs) => {
                writeln!(f, "bindings are out of date")?;
                for d in diffs {
                    write!(f, "{d}")?;
                }
                write!(f, "  run `cargo build` to regenerate")
            }
        }
    }
}

impl std::error::Error for WriteError {}

impl From<std::io::Error> for WriteError {
    fn from(e: std::io::Error) -> Self {
        WriteError::Io(e)
    }
}

/// What [`write_all`] did, broken down by file name.
#[derive(Debug, Default)]
pub struct WriteReport {
    /// Files written because they were new or their content changed.
    pub written: Vec<String>,
    /// Files whose on-disk content already matched and were left untouched.
    pub unchanged: Vec<String>,
    /// Previously-generated files deleted because no current type produces them.
    pub removed: Vec<String>,
}

/// Normalise without requiring the path to exist, so validation runs before any
/// directory is created.
fn normalise(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Checks that the configured output directory does not overlap any scan
/// root. Called by [`write_all`] before touching the filesystem, and
/// available separately so callers can fail fast before generation runs.
pub fn validate_paths(cfg: &Config) -> Result<(), WriteError> {
    let out = normalise(cfg.output_path());
    for scan in cfg.scans() {
        let scan = normalise(scan);
        if out == scan || out.starts_with(&scan) || scan.starts_with(&out) {
            return Err(WriteError::Overlap { output: out, scan });
        }
    }
    Ok(())
}

/// Writes `files` (name to contents) into [`Config::output_path`], skipping
/// unchanged files and deleting stale generated files no longer produced.
/// In [`Config::check`] mode, nothing is written; instead this compares
/// against what's on disk and returns [`WriteError::Drift`] if it differs.
pub fn write_all(
    files: &BTreeMap<String, String>,
    cfg: &Config,
) -> Result<WriteReport, WriteError> {
    validate_paths(cfg)?;
    let dir = cfg.output_path();

    if cfg.is_check() {
        let mut diffs = Vec::new();
        for (name, want) in files {
            let have = std::fs::read_to_string(dir.join(name)).unwrap_or_default();
            if have != *want {
                diffs.push(diff_lines(name, &have, want));
            }
        }
        for name in orphans(dir, files)? {
            diffs.push(format!("  {name}\n    - (entire file; no matching Rust type)\n"));
        }
        if !diffs.is_empty() {
            return Err(WriteError::Drift(diffs));
        }
        return Ok(WriteReport::default());
    }

    std::fs::create_dir_all(dir)?;
    let mut report = WriteReport::default();

    for (name, contents) in files {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::read_to_string(&path) {
            Ok(existing) if existing == *contents => report.unchanged.push(name.clone()),
            _ => {
                std::fs::write(&path, contents)?;
                report.written.push(name.clone());
            }
        }
    }

    for name in orphans(dir, files)? {
        std::fs::remove_file(dir.join(&name))?;
        if let Some(parent) = Path::new(&name).parent().filter(|p| !p.as_os_str().is_empty()) {
            let _ = std::fs::remove_dir(dir.join(parent));
        }
        report.removed.push(name);
    }
    report.removed.sort();

    Ok(report)
}

/// `.ts` files in the output directory that carry the marker but are not in the
/// current generation set.
fn orphans(dir: &Path, files: &BTreeMap<String, String>) -> Result<Vec<String>, WriteError> {
    let mut out = Vec::new();
    collect_orphans(dir, dir, files, &mut out)?;
    out.sort();
    Ok(out)
}

fn collect_orphans(
    root: &Path,
    dir: &Path,
    files: &BTreeMap<String, String>,
    out: &mut Vec<String>,
) -> Result<(), WriteError> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_orphans(root, &path, files, out)?;
            continue;
        }
        if path.extension().is_none_or(|e| e != "ts") {
            continue;
        }
        let Ok(rel) = path.strip_prefix(root) else { continue };
        let key = rel.to_string_lossy().replace('\\', "/");
        if files.contains_key(&key) {
            continue;
        }
        if std::fs::read_to_string(&path).map(|c| c.starts_with(MARKER)).unwrap_or(false) {
            out.push(key);
        }
    }
    Ok(())
}

fn diff_lines(name: &str, have: &str, want: &str) -> String {
    let mut out = format!("  {name}\n");
    let have_lines: Vec<&str> = have.lines().collect();
    let want_lines: Vec<&str> = want.lines().collect();
    for i in 0..have_lines.len().max(want_lines.len()) {
        let h = have_lines.get(i);
        let w = want_lines.get(i);
        if h != w {
            if let Some(h) = h {
                out.push_str(&format!("    - {h}\n"));
            }
            if let Some(w) = w {
                out.push_str(&format!("    + {w}\n"));
            }
        }
    }
    out
}
