use std::error::Error;
use std::fmt;

/// One-based source position attached to a frontend diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceLocation {
    pub line: usize,
    pub column: usize,
}

/// A syntax, subset, or lowering error produced by the Python frontend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonFrontendError {
    message: String,
    location: Option<SourceLocation>,
}

impl PythonFrontendError {
    pub(crate) fn new(message: impl Into<String>, location: Option<SourceLocation>) -> Self {
        Self {
            message: message.into(),
            location,
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn location(&self) -> Option<SourceLocation> {
        self.location
    }
}

impl fmt::Display for PythonFrontendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(location) = self.location {
            write!(
                formatter,
                "line {}, column {}: {}",
                location.line, location.column, self.message
            )
        } else {
            formatter.write_str(&self.message)
        }
    }
}

impl Error for PythonFrontendError {}
