//! Source-language frontends that lower directly to WVM executables.

pub mod python;

pub use python::{PythonFrontendError, SourceLocation, compile_python_function};
