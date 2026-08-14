mod compiled_region;
mod cranelift;
mod layout;
mod llvm;

use std::error::Error;
use std::fmt;

use cranelift_codegen::ir::{AbiParam, InstBuilder, UserFuncName, types};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module, default_libcall_names};

use inkwell::OptimizationLevel;
use inkwell::execution_engine::JitFunction;

use crate::wxir::{WxFunction, WxType};

pub use compiled_region::{CompiledRegion, ExecuteError, RegionExecution};
pub use cranelift::CraneliftRegionCompiler;
pub use layout::{RegionLayout, RegionSlot};

/// A recoverable WXIR compilation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileError {
    InvalidFunction(String),
    UnsupportedType(WxType),
    UnsupportedInstruction(&'static str),
    Backend(String),
}

impl fmt::Display for CompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFunction(error) => write!(formatter, "invalid WXIR: {error}"),
            Self::UnsupportedType(ty) => write!(formatter, "unsupported WXIR type {ty}"),
            Self::UnsupportedInstruction(instruction) => {
                write!(formatter, "unsupported WXIR instruction {instruction}")
            }
            Self::Backend(error) => write!(formatter, "Cranelift backend error: {error}"),
        }
    }
}

impl Error for CompileError {}

/// Compiles a verified WXIR region into an executable region handle.
pub trait RegionCompiler {
    fn compile(&mut self, function: &WxFunction) -> Result<CompiledRegion, CompileError>;
}

type JitAddFunction = extern "C" fn(i64, i64) -> i64;

/// Preserved standalone Cranelift smoke test entry.
pub fn run_jit_add(lhs: i64, rhs: i64) -> Result<i64, String> {
    let jit_builder =
        JITBuilder::new(default_libcall_names()).map_err(|error| error.to_string())?;
    let mut module = JITModule::new(jit_builder);
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I64));
    signature.params.push(AbiParam::new(types::I64));
    signature.returns.push(AbiParam::new(types::I64));

    let function_id = module
        .declare_function("wustite_add_i64", Linkage::Local, &signature)
        .map_err(|error| error.to_string())?;
    let mut context = module.make_context();
    context.func.signature = signature;
    context.func.name = UserFuncName::user(0, function_id.as_u32());
    let mut builder_context = FunctionBuilderContext::new();

    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.append_block_params_for_function_params(entry);
        let lhs = builder.block_params(entry)[0];
        let rhs = builder.block_params(entry)[1];
        let result = builder.ins().iadd(lhs, rhs);
        builder.ins().return_(&[result]);
        builder.seal_all_blocks();
        builder.finalize(module.target_config());
    }

    module
        .define_function(function_id, &mut context)
        .map_err(|error| format!("{error:#?}"))?;
    module.clear_context(&mut context);
    module
        .finalize_definitions()
        .map_err(|error| error.to_string())?;

    let code_ptr = module.get_finalized_function(function_id);
    // SAFETY: the finalized symbol has the exact two-i64 signature above, and
    // `module` remains alive through the call.
    let function: JitAddFunction = unsafe { std::mem::transmute(code_ptr) };
    Ok(function(lhs, rhs))
}
