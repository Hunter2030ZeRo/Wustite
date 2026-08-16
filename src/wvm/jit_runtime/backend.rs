use crate::executable::ExecutableId;
#[cfg(feature = "inkwell")]
use crate::jit::LlvmRegionCompiler;
use crate::jit::{CompileError, CompiledRegion, CompilerBackend, CraneliftRegionCompiler};
use crate::wxir::VerifiedWxFunction;

pub(super) enum BackendCompiler {
    Cranelift(Box<CraneliftRegionCompiler>),
    #[cfg(feature = "inkwell")]
    Llvm(Box<LlvmRegionCompiler>),
    Tiered {
        tier1: Box<CraneliftRegionCompiler>,
        #[cfg(feature = "inkwell")]
        tier2: Box<LlvmRegionCompiler>,
    },
}

pub(super) enum InitialRegion {
    Cranelift(Box<CompiledRegion>),
    #[cfg(feature = "inkwell")]
    Tier1 {
        region: Box<CompiledRegion>,
        function: Box<VerifiedWxFunction>,
    },
    #[cfg(feature = "inkwell")]
    Llvm(Box<CompiledRegion>),
}

impl BackendCompiler {
    pub(super) fn new(executable_id: ExecutableId, backend: CompilerBackend) -> Self {
        match backend {
            CompilerBackend::Cranelift => {
                Self::Cranelift(Box::new(CraneliftRegionCompiler::new(executable_id)))
            }
            #[cfg(feature = "inkwell")]
            CompilerBackend::Llvm => Self::Llvm(Box::new(LlvmRegionCompiler::new(executable_id))),
            CompilerBackend::Tiered => Self::Tiered {
                tier1: Box::new(CraneliftRegionCompiler::new(executable_id)),
                #[cfg(feature = "inkwell")]
                tier2: Box::new(LlvmRegionCompiler::new(executable_id)),
            },
        }
    }

    pub(super) const fn initial_tier_is_llvm(&self) -> bool {
        match self {
            Self::Cranelift(_) | Self::Tiered { .. } => false,
            #[cfg(feature = "inkwell")]
            Self::Llvm(_) => true,
        }
    }

    pub(super) fn compile_initial(
        &mut self,
        function: VerifiedWxFunction,
    ) -> Result<InitialRegion, CompileError> {
        match self {
            Self::Cranelift(compiler) => compiler
                .compile_verified(&function)
                .map(Box::new)
                .map(InitialRegion::Cranelift),
            #[cfg(feature = "inkwell")]
            Self::Llvm(compiler) => compiler
                .compile_verified(&function)
                .map(Box::new)
                .map(InitialRegion::Llvm),
            Self::Tiered { tier1, .. } => {
                let region = Box::new(tier1.compile_verified(&function)?);
                #[cfg(feature = "inkwell")]
                return Ok(InitialRegion::Tier1 {
                    region,
                    function: Box::new(function),
                });
                #[cfg(not(feature = "inkwell"))]
                Ok(InitialRegion::Cranelift(region))
            }
        }
    }

    #[cfg(feature = "inkwell")]
    pub(super) fn compile_tier2(
        &mut self,
        function: &VerifiedWxFunction,
    ) -> Option<Result<CompiledRegion, CompileError>> {
        match self {
            Self::Tiered { tier2, .. } => Some(tier2.compile_verified(function)),
            Self::Cranelift(_) | Self::Llvm(_) => None,
        }
    }
}
