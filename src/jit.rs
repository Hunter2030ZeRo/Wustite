use cranelift_codegen::ir::{AbiParam, InstBuilder, UserFuncName, types};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module, default_libcall_names};

type JITAddFunction = extern "C" fn(i64, i64) -> i64;

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

        let parameters = builder.block_params(entry);
        let lhs = parameters[0];
        let rhs = parameters[1];

        let result = builder.ins().iadd(lhs, rhs);

        builder.ins().return_(&[result]);

        builder.seal_all_blocks();
        builder.finalize(module.target_config());
    }

    if let Err(error) = module.define_function(function_id, &mut context) {
        eprintln!("Generated CLIF:\n{}", context.func.display());
        return Err(format!("{error:#?}"));
    }

    module.clear_context(&mut context);

    module
        .finalize_definitions()
        .map_err(|error| error.to_string())?;

    let code_ptr = module.get_finalized_function(function_id);

    let function: JITAddFunction = unsafe { std::mem::transmute(code_ptr) };

    let result = function(lhs, rhs);

    Ok(result)
}
