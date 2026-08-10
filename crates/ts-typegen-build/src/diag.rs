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

#[cfg(test)]
mod tests {
    use super::*;

    fn loc() -> SourceLoc {
        SourceLoc {
            file: "x.rs".into(),
            line: 1,
        }
    }

    #[test]
    fn new_is_an_error_and_warning_is_not() {
        assert!(Diagnostic::new(loc(), "boom").is_error());
        assert!(!Diagnostic::warning(loc(), "heads up").is_error());
    }

    #[test]
    fn with_help_attaches_optional_guidance() {
        let bare = Diagnostic::new(loc(), "boom");
        assert!(bare.help.is_none());
        let helped = Diagnostic::new(loc(), "boom").with_help("try this instead");
        assert_eq!(helped.help.as_deref(), Some("try this instead"));
    }

    #[test]
    fn an_empty_collection_has_no_errors() {
        let diags = Diagnostics::default();
        assert!(diags.is_empty());
        assert_eq!(diags.len(), 0);
        assert!(!diags.has_errors());
        assert_eq!(diags.error_count(), 0);
    }

    #[test]
    fn has_errors_ignores_warnings_but_counts_them_in_len() {
        let mut diags = Diagnostics::default();
        diags.push(Diagnostic::warning(loc(), "a"));
        diags.push(Diagnostic::warning(loc(), "b"));
        assert!(!diags.is_empty());
        assert_eq!(diags.len(), 2);
        assert!(!diags.has_errors(), "only warnings were pushed");
        assert_eq!(diags.error_count(), 0);
    }

    #[test]
    fn one_error_among_warnings_is_still_detected() {
        let mut diags = Diagnostics::default();
        diags.push(Diagnostic::warning(loc(), "a"));
        diags.push(Diagnostic::new(loc(), "b"));
        diags.push(Diagnostic::warning(loc(), "c"));
        assert!(diags.has_errors());
        assert_eq!(diags.error_count(), 1, "only the error counts, not the warnings");
        assert_eq!(diags.len(), 3);
    }

    #[test]
    fn iter_preserves_push_order() {
        let mut diags = Diagnostics::default();
        diags.push(Diagnostic::new(loc(), "first"));
        diags.push(Diagnostic::new(loc(), "second"));
        let messages: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(messages, vec!["first", "second"]);
    }
}
