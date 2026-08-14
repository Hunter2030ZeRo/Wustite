mod helpers;
mod instructions;
mod lowering;
mod state_buffer;

use std::fmt;

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{CodeModel, InitializationConfig, RelocMode, Target, TargetMachine};

use crate::executable::ExecutableId;
use crate::wxir::{VerifiedWxFunction, WxFunction};

use super::compiled_region::{CompiledRegion, NativeRegionCode, NativeRegionEntry};
use super::cranelift::symbols::SymbolVersions;
use super::layout::RegionLayout;
use super::{CompileError, RegionCompiler};

use lowering::lower_function;

const OPTIMIZATION_PIPELINE: &str = "default<O3>";

/// LLVM O3 implementation of the WXIR region compiler.
pub struct LlvmRegionCompiler {
    symbols: SymbolVersions,
}

impl fmt::Debug for LlvmRegionCompiler {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LlvmRegionCompiler")
            .field("symbols", &self.symbols)
            .field("optimization_pipeline", &OPTIMIZATION_PIPELINE)
            .finish()
    }
}

impl LlvmRegionCompiler {
    pub fn new(executable_id: ExecutableId) -> Self {
        Self {
            symbols: SymbolVersions::new(executable_id),
        }
    }

    pub(crate) fn compile_verified(
        &mut self,
        function: &VerifiedWxFunction,
    ) -> Result<CompiledRegion, CompileError> {
        let function = function.as_function();
        let symbol = self.symbols.reserve(function.origin.region_id)?;
        self.compile_validated(function, &symbol)
    }

    fn compile_validated(
        &mut self,
        function: &WxFunction,
        symbol: &str,
    ) -> Result<CompiledRegion, CompileError> {
        let layout = RegionLayout::new(function)?;
        let context = Box::new(Context::create());
        let context_pointer = &*context as *const Context;
        // SAFETY: [Categories 1 and 14 — pinned LLVM context lifetime]
        // The Box allocation does not move, NativeRegionCode owns that Box, and
        // its entry field is dropped before the context field.
        let context_reference = unsafe { &*context_pointer };
        let entry = compile_function(context_reference, function, &layout, symbol)?;
        let code = NativeRegionCode::Llvm {
            entry,
            _context: context,
        };
        Ok(CompiledRegion::new(code, layout, function))
    }
}

impl RegionCompiler for LlvmRegionCompiler {
    fn compile(&mut self, function: &WxFunction) -> Result<CompiledRegion, CompileError> {
        let symbol = self.symbols.reserve(function.origin.region_id)?;
        crate::wxir::verify(function).map_err(CompileError::InvalidFunction)?;
        self.compile_validated(function, &symbol)
    }
}

fn compile_function(
    context: &'static Context,
    function: &WxFunction,
    layout: &RegionLayout,
    symbol: &str,
) -> Result<inkwell::execution_engine::JitFunction<'static, NativeRegionEntry>, CompileError> {
    let module = lower_function(context, function, layout, symbol)?;
    module.verify().map_err(llvm_error)?;

    let target_machine = native_target_machine()?;
    let triple = target_machine.get_triple();
    let target_data = target_machine.get_target_data();
    module.set_triple(&triple);
    module.set_data_layout(&target_data.get_data_layout());

    let pass_options = PassBuilderOptions::create();
    pass_options.set_verify_each(true);
    pass_options.set_loop_interleaving(true);
    pass_options.set_loop_vectorization(true);
    pass_options.set_loop_slp_vectorization(true);
    pass_options.set_loop_unrolling(true);
    module
        .run_passes(OPTIMIZATION_PIPELINE, &target_machine, pass_options)
        .map_err(llvm_error)?;
    module.verify().map_err(llvm_error)?;

    let execution_engine = module
        .create_jit_execution_engine(OptimizationLevel::Aggressive)
        .map_err(llvm_error)?;
    // SAFETY: [Categories 3, 5, 8, and 14 — finalized LLVM JIT ABI]
    // Lowering declares `symbol` as NativeRegionEntry and JitFunction retains
    // the execution engine, so the entry cannot outlive its executable code.
    let entry = unsafe {
        execution_engine
            .get_function::<NativeRegionEntry>(symbol)
            .map_err(llvm_error)?
    };
    Ok(entry)
}

fn native_target_machine() -> Result<TargetMachine, CompileError> {
    Target::initialize_native(&InitializationConfig::default()).map_err(llvm_error)?;
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).map_err(llvm_error)?;
    let cpu = TargetMachine::get_host_cpu_name();
    let features = TargetMachine::get_host_cpu_features();
    target
        .create_target_machine(
            &triple,
            &cpu.to_string_lossy(),
            &features.to_string_lossy(),
            OptimizationLevel::Aggressive,
            RelocMode::Default,
            CodeModel::JITDefault,
        )
        .ok_or_else(|| CompileError::Backend("LLVM could not create a target machine".to_string()))
}

pub(super) fn llvm_error(error: impl fmt::Display) -> CompileError {
    CompileError::Backend(format!("LLVM: {error}"))
}
