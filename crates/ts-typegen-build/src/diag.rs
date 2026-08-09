//! Diagnostics. The engine collects every problem before failing so one build
//! surfaces every issue rather than one issue per build.

use crate::ir::SourceLoc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Correct output is impossible; fail the build.
    Error,
    /// The output is valid TypeScript, but something ambiguous was resolved on
    /// the author's behalf and they should know about it.
    Warning,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub loc: SourceLoc,
    pub message: String,
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn new(loc: SourceLoc, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Error,
            loc,
            message: message.into(),
            help: None,
        }
    }

    pub fn warning(loc: SourceLoc, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Warning,
            loc,
            message: message.into(),
            help: None,
        }
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

#[derive(Debug, Default)]
pub struct Diagnostics(Vec<Diagnostic>);

impl Diagnostics {
    pub fn push(&mut self, d: Diagnostic) {
        self.0.push(d);
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Diagnostic> {
        self.0.iter()
    }
}

impl Diagnostics {
    /// Only errors stop a build; warnings are reported and generation continues.
    pub fn has_errors(&self) -> bool {
        self.iter().any(Diagnostic::is_error)
    }

    pub fn error_count(&self) -> usize {
        self.iter().filter(|d| d.is_error()).count()
    }
}
