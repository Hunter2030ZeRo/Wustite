//! Backend-independent typed SSA IR for compiled Wustite regions.

pub mod builder;
pub mod ir;
pub mod printer;
pub mod types;
pub mod verifier;

pub use builder::{WxBuildError, build_region};
pub use ir::*;
pub use printer::print_function;
pub use types::{WxScalarType, WxType};
pub use verifier::verify;
