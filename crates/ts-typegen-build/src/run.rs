//! Orchestration: scan, render, write, and report.

use std::error::Error;
use std::fmt;

use crate::config::Config;
use crate::diag::Diagnostics;
use crate::lower::LowerCtx;
use crate::render::render_all;
use crate::scan::scan_roots;
use crate::write::{WriteReport, write_all};

/// The result of a successful [`generate`] call.
#[derive(Debug)]
pub struct Outcome {
    /// Which files were written, unchanged, or removed.
    pub report: WriteReport,
    /// True when generation was skipped because this is not a primary package.
    pub skipped: bool,
    /// Non-fatal diagnostics, already formatted for display.
    pub warnings: Vec<String>,
}

/// [`generate`]'s error type: every collected [`Diagnostic`](crate::diag::Diagnostic),
/// pre-formatted into one message.
#[derive(Debug)]
pub struct DiagError(String);

impl fmt::Display for DiagError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for DiagError {}

/// True when the manifest lives inside cargo's registry cache or a git
/// checkout, i.e. this crate is being compiled as somebody else's dependency.
///
/// `CARGO_PRIMARY_PACKAGE` is deliberately not used: cargo sets it for rustc
/// invocations, not for build scripts, so gating on it would disable generation
/// for every user rather than only for downstream consumers.
pub(crate) fn looks_vendored(manifest_dir: &str) -> bool {
    let dir = manifest_dir.replace('\\', "/");
    dir.contains("/registry/src/") || dir.contains("/git/checkouts/")
}

fn is_downstream_build() -> bool {
    std::env::var("CARGO_MANIFEST_DIR").is_ok_and(|d| looks_vendored(&d))
}

/// Runs the full pipeline: scan, lower, render, and write. This is what
/// [`Config::run`](crate::config::Config::run) calls; use it directly to get
/// the [`Outcome`] instead of having results printed as cargo directives.
pub fn generate(cfg: &Config) -> Result<Outcome, Box<dyn Error>> {
    if !cfg.always_run_enabled() && is_downstream_build() {
        return Ok(Outcome { report: WriteReport::default(), skipped: true, warnings: Vec::new() });
    }

    let ctx = LowerCtx {
        params: Default::default(),
        map: cfg.type_map().clone(),
        aliases: Default::default(),
    };
    let mut diags = Diagnostics::default();
    let reg = scan_roots(cfg.scans(), &ctx, &mut diags);
    if reg.decls().is_empty() {
        diags.push(
            crate::diag::Diagnostic::warning(
                crate::ir::SourceLoc { file: cfg.scans().first().cloned().unwrap_or_default(), line: 0 },
                "no #[derive(Ts)] types found in the scanned roots",
            )
            .with_help("check the .scan(..) paths, or remove the build script if unused"),
        );
    }

    let files = render_all(&reg, cfg, &mut diags);

    let render = |d: &crate::diag::Diagnostic| {
        let mut s = format!("{}:{} {}", d.loc.file.display(), d.loc.line, d.message);
        if let Some(help) = &d.help {
            s.push_str(&format!("\n  help: {help}"));
        }
        s
    };

    if diags.has_errors() {
        // Warnings ride along in the failure text so one build shows everything.
        let mut msg = String::new();
        for d in diags.iter() {
            msg.push_str(&render(d));
            msg.push('\n');
        }
        msg.push_str(&format!("{} error(s)", diags.error_count()));
        return Err(Box::new(DiagError(msg)));
    }

    let warnings: Vec<String> = diags.iter().map(render).collect();
    let report = write_all(&files, cfg)?;
    Ok(Outcome { report, skipped: false, warnings })
}

#[cfg(test)]
mod tests {
    use super::looks_vendored;

    #[test]
    fn registry_and_git_checkouts_are_downstream_builds() {
        assert!(looks_vendored(
            "/home/u/.cargo/registry/src/index.crates.io-6f17d22bba15001f/demo-0.1.0"
        ));
        assert!(looks_vendored("/home/u/.cargo/git/checkouts/demo-abc123/deadbee"));
        assert!(looks_vendored(
            r"C:\Users\u\.cargo\registry\src\index.crates.io-6f17\demo-0.1.0"
        ));
    }

    #[test]
    fn a_normal_project_directory_is_not_a_downstream_build() {
        assert!(!looks_vendored("/home/u/Develop/my-api"));
        assert!(!looks_vendored("/home/u/Develop/registry-service/src"));
        assert!(!looks_vendored("/work/git/my-project"));
    }
}
