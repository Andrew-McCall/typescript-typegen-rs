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

#[derive(Debug)]
pub enum WriteError {
    Overlap { output: PathBuf, scan: PathBuf },
    Io(std::io::Error),
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

#[derive(Debug, Default)]
pub struct WriteReport {
    pub written: Vec<String>,
    pub unchanged: Vec<String>,
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
        report.removed.push(name);
    }
    report.removed.sort();

    Ok(report)
}

/// `.ts` files in the output directory that carry the marker but are not in the
/// current generation set.
fn orphans(dir: &Path, files: &BTreeMap<String, String>) -> Result<Vec<String>, WriteError> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(out);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || path.extension().is_none_or(|e| e != "ts") {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if files.contains_key(name) {
            continue;
        }
        let is_generated =
            std::fs::read_to_string(&path).map(|c| c.starts_with(MARKER)).unwrap_or(false);
        if is_generated {
            out.push(name.to_string());
        }
    }
    out.sort();
    Ok(out)
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
