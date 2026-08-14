mod helpers;
mod instructions;
mod lowering;
pub(super) mod symbols;

#[cfg(test)]
mod tests;

use std::fmt;

use cranelift_codegen::ir::{AbiParam, UserFuncName, types};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module, default_libcall_names};

use crate::executable::ExecutableId;
use crate::wxir::{VerifiedWxFunction, WxFunction};

use super::compiled_region::{CompiledRegion, NativeRegionCode, NativeRegionEntry};
use super::layout::RegionLayout;
use super::{CompileError, RegionCompiler};

use lowering::lower_function;
use symbols::SymbolVersions;

/// Cranelift implementation of the WXIR region compiler.
pub struct CraneliftRegionCompiler {
    symbols: SymbolVersions,
    module: Option<JITModule>,
}

impl fmt::Debug for CraneliftRegionCompiler {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CraneliftRegionCompiler")
            .field("symbols", &self.symbols)
            .field("module_initialized", &self.module.is_some())
            .finish()
    }
}

impl CraneliftRegionCompiler {
    pub fn new(executable_id: ExecutableId) -> Self {
        Self {
            symbols: SymbolVersions::new(executable_id),
            module: None,
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

        let mut module = match self.module.take() {
            Some(module) => module,
            None => new_jit_module()?,
        };
        let compilation = compile_function(&mut module, function, &layout, symbol);
        self.module = Some(module);
        let entry = compilation?;

        Ok(CompiledRegion::new(
            NativeRegionCode::Cranelift(entry),
            layout,
            function,
        ))
    }
}

impl RegionCompiler for CraneliftRegionCompiler {
    fn compile(&mut self, function: &WxFunction) -> Result<CompiledRegion, CompileError> {
        let symbol = self.symbols.reserve(function.origin.region_id)?;
        crate::wxir::verify(function).map_err(CompileError::InvalidFunction)?;
        self.compile_validated(function, &symbol)
    }
}

fn new_jit_module() -> Result<JITModule, CompileError> {
    let jit_builder = JITBuilder::new(default_libcall_names())
        .map_err(|error| CompileError::Backend(error.to_string()))?;
    Ok(JITModule::new(jit_builder))
}

fn compile_function(
    module: &mut JITModule,
    function: &WxFunction,
    layout: &RegionLayout,
    symbol: &str,
) -> Result<NativeRegionEntry, CompileError> {
    let pointer_type = module.target_config().pointer_type();
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(pointer_type));
    signature.returns.push(AbiParam::new(types::I32));
    let function_id = module
        .declare_function(symbol, Linkage::Local, &signature)
        .map_err(|error| CompileError::Backend(error.to_string()))?;

    let mut context = module.make_context();
    context.func.signature = signature;
    context.func.name = UserFuncName::user(0, function_id.as_u32());
    let mut builder_context = FunctionBuilderContext::new();

    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
        lower_function(&mut builder, function, layout)?;
        builder.seal_all_blocks();
        builder.finalize(module.target_config());
    }

    module
        .define_function(function_id, &mut context)
        .map_err(|error| CompileError::Backend(format!("{error:#?}")))?;
    module.clear_context(&mut context);
    module
        .finalize_definitions()
        .map_err(|error| CompileError::Backend(error.to_string()))?;

    let code_ptr = module.get_finalized_function(function_id);
    // SAFETY: [Categories 3, 5, 6, and 14 — finalized JIT entry ABI]
    // `code_ptr` names a finalized function declared above with exactly one
    // native pointer argument and one 32-bit result. `NativeRegionEntry` uses
    // the same C ABI and bit widths. The compiler retains the JIT module on
    // success, and this crate never calls JITModule::free_memory.
    Ok(unsafe { std::mem::transmute::<*const u8, NativeRegionEntry>(code_ptr) })
}
