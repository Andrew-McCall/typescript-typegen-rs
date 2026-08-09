//! Diagnostics. The engine collects every problem before failing so one build
//! surfaces every issue rather than one issue per build.

use crate::ir::SourceLoc;

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub loc: SourceLoc,
    pub message: String,
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn new(loc: SourceLoc, message: impl Into<String>) -> Self {
        Diagnostic {
            loc,
            message: message.into(),
            help: None,
        }
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
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
