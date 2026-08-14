//! Backend-independent typed SSA IR for compiled Wustite regions.

pub mod builder;
pub mod ir;
pub mod printer;
pub mod types;
pub mod verifier;

#[cfg(test)]
mod tests;

pub(crate) use builder::build_verified_region;
pub use builder::{WxBuildError, build_region};
pub use ir::*;
pub use printer::print_function;
pub use types::{WxScalarType, WxType};

pub(crate) struct VerifiedWxFunction(WxFunction);

impl VerifiedWxFunction {
    fn validate(function: WxFunction) -> Result<Self, String> {
        verify(&function)?;
        Ok(Self(function))
    }

    pub(crate) fn as_function(&self) -> &WxFunction {
        &self.0
    }

    fn into_function(self) -> WxFunction {
        self.0
    }
}

#[cfg(test)]
std::thread_local! {
    static VERIFICATION_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub fn verify(function: &WxFunction) -> Result<(), String> {
    #[cfg(test)]
    VERIFICATION_COUNT.set(VERIFICATION_COUNT.get() + 1);

    verifier::verify(function)
}

#[cfg(test)]
fn reset_verification_count() {
    VERIFICATION_COUNT.set(0);
}

#[cfg(test)]
fn verification_count() -> usize {
    VERIFICATION_COUNT.get()
}
