//! Orchestration: scan, render, write, and report.

use std::error::Error;
use std::fmt;

use crate::config::Config;
use crate::diag::Diagnostics;
use crate::lower::LowerCtx;
use crate::render::render_all;
use crate::scan::scan_roots;
use crate::write::{WriteReport, write_all};

#[derive(Debug)]
pub struct Outcome {
    pub report: WriteReport,
    /// True when generation was skipped because this is not a primary package.
    pub skipped: bool,
}

#[derive(Debug)]
pub struct DiagError(String);

impl fmt::Display for DiagError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for DiagError {}

pub fn generate(cfg: &Config) -> Result<Outcome, Box<dyn Error>> {
    if !cfg.always_run_enabled() && std::env::var_os("CARGO_PRIMARY_PACKAGE").is_none() {
        return Ok(Outcome { report: WriteReport::default(), skipped: true });
    }

    let ctx = LowerCtx { params: Default::default(), map: cfg.type_map().clone() };
    let mut diags = Diagnostics::default();
    let reg = scan_roots(cfg.scans(), &ctx, &mut diags);
    let files = render_all(&reg, cfg, &mut diags);

    if !diags.is_empty() {
        let mut msg = String::new();
        for d in diags.iter() {
            msg.push_str(&format!("{}:{} {}\n", d.loc.file.display(), d.loc.line, d.message));
            if let Some(help) = &d.help {
                msg.push_str(&format!("  help: {help}\n"));
            }
        }
        msg.push_str(&format!("{} error(s)", diags.len()));
        return Err(Box::new(DiagError(msg)));
    }

    let report = write_all(&files, cfg)?;
    Ok(Outcome { report, skipped: false })
}
