use inkwell::{AddressSpace, IntPredicate, context::Context, module::Module};

struct LoweringState<'ctx> {
    blocks: HashMap<WxBlockId, BasicBlock<'ctx>>,
    values: HashMap<WxValueId, BasicValueEnum<'ctx>>,

    block_phis: HashMap<WxBlockId, Vec<PhiValue<'ctx>>>,

    exit_blocks: HashMap<WxExitId, BasicBlock<'ctx>>,
    exit_phis: HashMap<WxExitId, Vec<PhiValue<'ctx>>>,
}

pub fn lower_function<'ctx>(
    context: &'ctx Context,
    function: &WxFunction,
    layout: &RegionLayout,
    symbol: &str,
) -> Result<Module<'ctx>, CompileError> {
    for block in &function.blocks {
        let llvm_block = context.append_basic_block(function, &format!("b{}", block.id.0));

        blocks.insert(block.id, llvm_block);
    }
    for block in &function.blocks {
        let llvm_block = blocks[&block.id];
        builder.position_at_end(llvm_block);

        let mut phis = Vec::new();

        for parameter in &block.parameters {
            let phi = builder
                .build_phi(llvm_type(context, parameter.ty)?, "param")
                .map_err(|error| CompileError::from(error))?;

            values.insert(parameter.id, phi.as_phasic_value());
            phis.push(phi);
        }

        block_phis.insert(block.id, phis);
    }
    let module = context.create_module(symbol);
    let builder = context.create_builder();

    let i1 = context.bool_type();
    let i32 = context.i32_type();
    let i64 = context.i64_type();

    let state_ptr = context.ptr_type(AddressSpace::default());

    let fn_ty = i32.fn_type(&[state_ptr.into()], false);

    let llvm_fn = module.add_function(symbol, fn_ty, None);

    Ok(module)
}
