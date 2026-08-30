use std::collections::BTreeMap;

use cranelift_codegen::ir::{
    AbiParam, BlockArg, FuncRef, InstBuilder, MemFlagsData, UserFuncName, types,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module, default_libcall_names};

use super::abi::{
    DEOPT_ID_OFFSET, DEOPTS_OFFSET, DIRECT_ABI_OFFSET, DIRECT_ALIAS_OFFSET, DIRECT_CAPACITY_OFFSET,
    DIRECT_LAYOUT_EPOCH_OFFSET, DIRECT_LENGTH_OFFSET, DIRECT_MAGIC_OFFSET, DIRECT_OWNER_OFFSET,
    DIRECT_STORAGE_ABI, DIRECT_STORAGE_COUNT_OFFSET, DIRECT_STORAGE_INDEX_OFFSET,
    DIRECT_STORAGE_MAGIC, DIRECT_STORAGE_OFFSET, DIRECT_STORAGE_RECEIPTS_OFFSET,
    DIRECT_STRATEGY_OFFSET, DIRECT_VALUES_OFFSET, DIRECT_VERSION_OFFSET, EXIT_ID_OFFSET,
    EXIT_KIND_OFFSET, GUARD_ID_OFFSET, HELPER_CALLS_OFFSET, HELPER_CONTEXT_OFFSET, INPUTS_OFFSET,
    MACHINE_ENTRIES_OFFSET, OUTPUTS_OFFSET, RECEIPT_ALIAS_OFFSET, RECEIPT_LAYOUT_EPOCH_OFFSET,
    RECEIPT_OWNER_OFFSET, RECEIPT_STORAGE_IDENTITY_OFFSET, RECEIPT_STRATEGY_OFFSET,
    RECEIPT_VERSION_OFFSET, SAFEPOINT_ID_OFFSET, SLOT_PAYLOAD_OFFSET, SLOT_SIZE,
};
use super::{NativeCode, NativeError};
use crate::adaptive_v2::wxir_v2::VerifiedSnapshot;
use crate::adaptive_v2::wxir_v2::ir::{
    BlockId, Constant, InstructionKind, NumericComparison, Terminator, ValueId, ValueType,
};

pub(super) fn compile(
    snapshot: &VerifiedSnapshot,
    symbol: &str,
) -> Result<NativeCode, NativeError> {
    let mut builder = JITBuilder::with_flags(&[("opt_level", "speed")], default_libcall_names())
        .map_err(|error| NativeError::Backend(error.to_string()))?;
    register_helpers(&mut builder);
    let mut module = JITModule::new(builder);
    let pointer_type = module.target_config().pointer_type();
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(pointer_type));
    signature.returns.push(AbiParam::new(types::I32));
    let function_id = module
        .declare_function(symbol, Linkage::Local, &signature)
        .map_err(|error| NativeError::Backend(error.to_string()))?;
    let mut context = module.make_context();
    context.func.signature = signature;
    context.func.name = UserFuncName::user(0, function_id.as_u32());
    let helper_functions = HelperFunctions::declare(&mut module, &mut context.func)?;
    validate_supported_types(snapshot)?;
    let output_types = output_types(snapshot)?;
    let input_types = snapshot
        .body()
        .blocks
        .iter()
        .find(|block| block.id == snapshot.body().entry)
        .ok_or(NativeError::Unsupported("missing entry"))?
        .parameters
        .iter()
        .map(|parameter| parameter.ty)
        .collect();
    let direct_storage = super::direct_storage::verify(snapshot);
    let mut builder_context = FunctionBuilderContext::new();
    {
        let mut function = FunctionBuilder::new(&mut context.func, &mut builder_context);
        lower(
            &mut function,
            snapshot,
            &helper_functions,
            direct_storage.as_ref(),
        )?;
        function.seal_all_blocks();
        function.finalize(module.target_config());
    }
    if let Some(path) = super::clif_artifact_path(symbol) {
        let directory = path
            .parent()
            .ok_or_else(|| NativeError::Backend("CLIF artifact path has no parent".to_owned()))?;
        std::fs::create_dir_all(directory)
            .map_err(|error| NativeError::Backend(error.to_string()))?;
        std::fs::write(path, context.func.display().to_string())
            .map_err(|error| NativeError::Backend(error.to_string()))?;
    }
    module
        .define_function(function_id, &mut context)
        .map_err(|error| NativeError::Backend(format!("{error:#?}")))?;
    module.clear_context(&mut context);
    module
        .finalize_definitions()
        .map_err(|error| NativeError::Backend(error.to_string()))?;
    let entry = super::entry::from_code_ptr(module.get_finalized_function(function_id));
    Ok(NativeCode {
        snapshot_id: snapshot.id(),
        input_types,
        output_types,
        direct_storage,
        _owner: super::NativeOwner::Cranelift {
            entry,
            _module: Box::new(module),
        },
    })
}

fn lower(
    builder: &mut FunctionBuilder<'_>,
    snapshot: &VerifiedSnapshot,
    helpers: &HelperFunctions,
    direct_storage: Option<&super::direct_storage::DirectStoragePlan>,
) -> Result<(), NativeError> {
    let body = snapshot.body();
    let proven_direct_gets = direct_storage
        .map(|plan| {
            canonical_direct_gets(body, |handle| {
                plan.storage_for(handle).map(|(storage, _)| storage)
            })
        })
        .unwrap_or_default();
    if std::env::var_os("WUSTITE_ADAPTIVE_V2_CLIF_DIR").is_some() {
        eprintln!("prevalidated_range_gets={}", proven_direct_gets.len());
        eprintln!(
            "prevalidated_appends={}",
            direct_storage.map_or(0, |plan| {
                body.blocks
                    .iter()
                    .flat_map(|block| &block.instructions)
                    .filter_map(|instruction| instruction.output)
                    .filter(|output| plan.append_capacity_is_proven(output.id))
                    .count()
            })
        );
    }
    let flags = MemFlagsData::new();
    let mut blocks = BTreeMap::new();
    for block in &body.blocks {
        let native = builder.create_block();
        for parameter in &block.parameters {
            builder.append_block_param(native, native_type(parameter.ty)?);
        }
        if let Some(plan) = direct_storage {
            for _ in plan.storages() {
                builder.append_block_param(native, types::I64);
            }
        }
        blocks.insert(block.id, native);
    }
    let prologue = builder.create_block();
    builder.switch_to_block(prologue);
    builder.append_block_params_for_function_params(prologue);
    let frame = builder.block_params(prologue)[0];
    let entries = builder
        .ins()
        .load(types::I64, flags, frame, MACHINE_ENTRIES_OFFSET);
    let one = builder.ins().iconst(types::I64, 1);
    let entries = builder.ins().iadd(entries, one);
    builder
        .ins()
        .store(flags, entries, frame, MACHINE_ENTRIES_OFFSET);
    let inputs = builder.ins().load(types::I64, flags, frame, INPUTS_OFFSET);
    let helper_context = builder
        .ins()
        .load(types::I64, flags, frame, HELPER_CONTEXT_OFFSET);
    let direct_views = direct_storage.map_or_else(BTreeMap::new, |plan| {
        prevalidate_direct_storage(builder, frame, inputs, plan)
    });
    if direct_storage.is_some_and(super::direct_storage::DirectStoragePlan::has_dynamic) {
        prevalidate_dynamic_direct_storage(builder, frame);
    }
    let entry = body
        .blocks
        .iter()
        .find(|block| block.id == body.entry)
        .ok_or(NativeError::Unsupported("missing entry"))?;
    let mut arguments = entry
        .parameters
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            let offset = slot_offset(index)?;
            Ok(BlockArg::from(builder.ins().load(
                native_type(parameter.ty)?,
                flags,
                inputs,
                offset,
            )))
        })
        .collect::<Result<Vec<_>, NativeError>>()?;
    arguments.extend(
        direct_views
            .values()
            .map(|view| BlockArg::from(view.length)),
    );
    builder.ins().jump(block(&blocks, body.entry)?, &arguments);

    let mut values = BTreeMap::new();
    let mut value_types = BTreeMap::new();
    for source in &body.blocks {
        let native = block(&blocks, source.id)?;
        for (parameter, value) in source.parameters.iter().zip(builder.block_params(native)) {
            values.insert(parameter.id, *value);
            value_types.insert(parameter.id, parameter.ty);
        }
    }
    for source in &body.blocks {
        builder.switch_to_block(block(&blocks, source.id)?);
        let native = block(&blocks, source.id)?;
        let mut block_views = direct_views.clone();
        for ((_, view), length) in block_views.iter_mut().zip(
            builder
                .block_params(native)
                .iter()
                .skip(source.parameters.len()),
        ) {
            view.length = *length;
        }
        let mut dynamic_views = BTreeMap::new();
        for instruction in &source.instructions {
            if instruction.effect.is_barrier() {
                dynamic_views.clear();
            }
            let semantic = instruction.kind.semantic();
            let result = match semantic {
                InstructionKind::Constant(constant) => Some(lower_constant(builder, constant)?),
                InstructionKind::Copy => Some(value(&values, instruction.inputs[0])?),
                InstructionKind::IntegerAdd => {
                    let [left, right] = two_inputs(&values, &instruction.inputs)?;
                    Some(builder.ins().iadd(left, right))
                }
                InstructionKind::IntegerSubtract => {
                    let [left, right] = two_inputs(&values, &instruction.inputs)?;
                    Some(builder.ins().isub(left, right))
                }
                InstructionKind::IntegerMultiply => {
                    let [left, right] = two_inputs(&values, &instruction.inputs)?;
                    Some(builder.ins().imul(left, right))
                }
                InstructionKind::IntegerFloorDivide { divisor } => {
                    let left = value(&values, instruction.inputs[0])?;
                    let right = builder.ins().iconst(types::I64, *divisor);
                    let quotient = builder.ins().sdiv(left, right);
                    let remainder = builder.ins().srem(left, right);
                    let has_remainder = builder.ins().icmp_imm_s(
                        cranelift_codegen::ir::condcodes::IntCC::NotEqual,
                        remainder,
                        0,
                    );
                    let left_negative = builder.ins().icmp_imm_s(
                        cranelift_codegen::ir::condcodes::IntCC::SignedLessThan,
                        left,
                        0,
                    );
                    let right_negative = builder.ins().icmp_imm_s(
                        cranelift_codegen::ir::condcodes::IntCC::SignedLessThan,
                        right,
                        0,
                    );
                    let signs_differ = builder.ins().bxor(left_negative, right_negative);
                    let adjust = builder.ins().band(has_remainder, signs_differ);
                    let one = builder.ins().iconst(types::I64, 1);
                    let zero = builder.ins().iconst(types::I64, 0);
                    let correction = builder.ins().select(adjust, one, zero);
                    Some(builder.ins().isub(quotient, correction))
                }
                InstructionKind::IntegerToFloat => {
                    let input = value(&values, instruction.inputs[0])?;
                    Some(builder.ins().fcvt_from_sint(types::F64, input))
                }
                InstructionKind::IntegerLessThan => {
                    let [left, right] = two_inputs(&values, &instruction.inputs)?;
                    let compared = builder.ins().icmp(
                        cranelift_codegen::ir::condcodes::IntCC::SignedLessThan,
                        left,
                        right,
                    );
                    Some(builder.ins().uextend(types::I64, compared))
                }
                InstructionKind::IntegerCompare { comparison } => {
                    let [left, right] = two_inputs(&values, &instruction.inputs)?;
                    let compared = builder
                        .ins()
                        .icmp(integer_condition(*comparison), left, right);
                    Some(builder.ins().uextend(types::I64, compared))
                }
                InstructionKind::FloatAdd
                | InstructionKind::FloatSubtract
                | InstructionKind::FloatMultiply
                | InstructionKind::FloatDivide
                | InstructionKind::FloatPower => {
                    let [left, right] = two_inputs(&values, &instruction.inputs)?;
                    Some(match semantic {
                        InstructionKind::FloatAdd => builder.ins().fadd(left, right),
                        InstructionKind::FloatSubtract => builder.ins().fsub(left, right),
                        InstructionKind::FloatMultiply => builder.ins().fmul(left, right),
                        InstructionKind::FloatDivide => builder.ins().fdiv(left, right),
                        InstructionKind::FloatPower => {
                            let call = builder.ins().call(helpers.float_power, &[left, right]);
                            builder.inst_results(call)[0]
                        }
                        _ => return Err(NativeError::Unsupported("float arithmetic")),
                    })
                }
                InstructionKind::FloatCompare { comparison } => {
                    let [left, right] = two_inputs(&values, &instruction.inputs)?;
                    let compared = builder
                        .ins()
                        .fcmp(float_condition(*comparison), left, right);
                    Some(builder.ins().uextend(types::I64, compared))
                }
                InstructionKind::IntegerNegate => {
                    let input = value(&values, instruction.inputs[0])?;
                    Some(builder.ins().ineg(input))
                }
                InstructionKind::FloatNegate => {
                    let input = value(&values, instruction.inputs[0])?;
                    Some(builder.ins().fneg(input))
                }
                InstructionKind::BooleanNot => {
                    let input = value(&values, instruction.inputs[0])?;
                    let one = builder.ins().iconst(types::I64, 1);
                    Some(builder.ins().bxor(input, one))
                }
                InstructionKind::BooleanAnd | InstructionKind::BooleanOr => {
                    let [left, right] = two_inputs(&values, &instruction.inputs)?;
                    Some(match semantic {
                        InstructionKind::BooleanAnd => builder.ins().band(left, right),
                        InstructionKind::BooleanOr => builder.ins().bor(left, right),
                        _ => return Err(NativeError::Unsupported("boolean arithmetic")),
                    })
                }
                InstructionKind::Select => {
                    let condition = value(&values, instruction.inputs[0])?;
                    let yes = value(&values, instruction.inputs[1])?;
                    let no = value(&values, instruction.inputs[2])?;
                    Some(builder.ins().select(condition, yes, no))
                }
                InstructionKind::OwnedList {
                    identity,
                    reset_on_definition,
                    copy_from_source,
                    ..
                } => {
                    let output = instruction.output.ok_or(NativeError::Unsupported(
                        "owned list definition has no output",
                    ))?;
                    let (storage_index, storage) = direct_storage
                        .and_then(|plan| plan.storage_for(output.id))
                        .ok_or(NativeError::Unsupported("owned list direct storage"))?;
                    let alias = builder.ins().iconst(
                        types::I64,
                        i64::from_ne_bytes(
                            super::direct_storage::owned_alias(*identity).to_ne_bytes(),
                        ),
                    );
                    if *copy_from_source {
                        let source_index = storage
                            .copy_from
                            .ok_or(NativeError::Unsupported("owned list copy source storage"))?;
                        let source = direct_view(&block_views, source_index)?;
                        let destination = direct_view(&block_views, storage_index)?;
                        lower_direct_list_copy(builder, destination, source);
                        block_views
                            .get_mut(&storage_index)
                            .expect("verified storage")
                            .length = source.length;
                    } else if *reset_on_definition {
                        let view = direct_view(&block_views, storage_index)?;
                        if let Some(extent_storage) =
                            direct_storage.and_then(|plan| plan.capacity_extent_for(output.id))
                        {
                            guard_direct_list_capacity(
                                builder,
                                view,
                                direct_view(&block_views, extent_storage)?.length,
                            );
                        }
                        let (descriptor, _, _, _) = direct_list(view);
                        let zero = builder.ins().iconst(types::I64, 0);
                        builder.ins().store(
                            MemFlagsData::new(),
                            zero,
                            descriptor,
                            DIRECT_LENGTH_OFFSET,
                        );
                        block_views
                            .get_mut(&storage_index)
                            .expect("verified storage")
                            .length = zero;
                    }
                    Some(alias)
                }
                InstructionKind::Guard { guard } => {
                    let condition = value(&values, instruction.inputs[0])?;
                    let recipe = body
                        .deopts
                        .iter()
                        .find(|recipe| recipe.id == *guard)
                        .ok_or(NativeError::Unsupported("missing guard deopt"))?;
                    lower_guard(builder, frame, condition, *guard, recipe.root_point.get())?;
                    None
                }
                InstructionKind::ObjectGet
                    if direct_storage.is_some_and(|plan| {
                        plan.storage_for(instruction.inputs[0])
                            .is_some_and(|(_, storage)| {
                                instruction
                                    .output
                                    .is_some_and(|output| Some(output.id) == storage.output)
                            })
                    }) =>
                {
                    let (storage_index, _) = direct_storage
                        .and_then(|plan| plan.storage_for(instruction.inputs[0]))
                        .ok_or(NativeError::Unsupported("object direct storage"))?;
                    let raw = lower_direct_list_get(
                        builder,
                        direct_view(&block_views, storage_index)?,
                        value(&values, instruction.inputs[1])?,
                        instruction
                            .output
                            .is_some_and(|output| proven_direct_gets.contains(&output.id)),
                    );
                    Some(match instruction.output.map(|output| output.ty) {
                        Some(ValueType::F64) => {
                            builder.ins().bitcast(types::F64, MemFlagsData::new(), raw)
                        }
                        Some(ValueType::I64 | ValueType::Handle | ValueType::Bool)
                        | Some(ValueType::BorrowedView)
                        | None => raw,
                    })
                }
                InstructionKind::ObjectGet => Some(lower_helper(
                    builder,
                    frame,
                    helper_context,
                    helpers.object_get,
                    &helper_values(&values, &instruction.inputs)?,
                )?),
                InstructionKind::ObjectSet => {
                    let _ = lower_helper(
                        builder,
                        frame,
                        helper_context,
                        helpers.object_set,
                        &helper_values(&values, &instruction.inputs)?,
                    )?;
                    None
                }
                InstructionKind::ListGet
                    if direct_storage.is_some_and(|plan| {
                        plan.is_dynamic(instruction.inputs[0])
                            || plan.storage_for(instruction.inputs[0]).is_some_and(
                                |(_, storage)| {
                                    storage.kind == super::direct_storage::DirectStorageKind::List
                                },
                            )
                    }) =>
                {
                    let handle = instruction.inputs[0];
                    let view = if let Some((storage_index, _)) =
                        direct_storage.and_then(|plan| plan.storage_for(handle))
                    {
                        direct_view(&block_views, storage_index)?
                    } else if let Some(view) = dynamic_views.get(&handle).copied() {
                        view
                    } else {
                        let view =
                            resolve_dynamic_direct_storage(builder, frame, value(&values, handle)?);
                        dynamic_views.insert(handle, view);
                        view
                    };
                    let raw = lower_direct_list_get(
                        builder,
                        view,
                        value(&values, instruction.inputs[1])?,
                        instruction
                            .output
                            .is_some_and(|output| proven_direct_gets.contains(&output.id)),
                    );
                    Some(match instruction.output.map(|output| output.ty) {
                        Some(ValueType::F64) => {
                            builder.ins().bitcast(types::F64, MemFlagsData::new(), raw)
                        }
                        Some(ValueType::I64 | ValueType::Handle | ValueType::Bool)
                        | Some(ValueType::BorrowedView)
                        | None => raw,
                    })
                }
                InstructionKind::ListGet => Some(lower_helper(
                    builder,
                    frame,
                    helper_context,
                    helpers.list_get,
                    &helper_values(&values, &instruction.inputs)?,
                )?),
                InstructionKind::ListLength => {
                    let (storage_index, _) = direct_storage
                        .and_then(|plan| plan.storage_for(instruction.inputs[0]))
                        .ok_or(NativeError::Unsupported("direct list length"))?;
                    let (_, length, _, _) = direct_list(direct_view(&block_views, storage_index)?);
                    Some(length)
                }
                InstructionKind::ListSet => {
                    if let Some((storage_index, storage)) = direct_storage
                        .and_then(|plan| plan.storage_for(instruction.inputs[0]))
                        .filter(|(_, storage)| {
                            storage.kind == super::direct_storage::DirectStorageKind::List
                        })
                    {
                        let _ = storage;
                        lower_direct_list_set(
                            builder,
                            direct_view(&block_views, storage_index)?,
                            value(&values, instruction.inputs[1])?,
                            value(&values, instruction.inputs[2])?,
                        );
                        None
                    } else if direct_storage
                        .is_some_and(|plan| plan.is_dynamic(instruction.inputs[0]))
                    {
                        let handle = instruction.inputs[0];
                        let view = if let Some(view) = dynamic_views.get(&handle).copied() {
                            view
                        } else {
                            let view = resolve_dynamic_direct_storage(
                                builder,
                                frame,
                                value(&values, handle)?,
                            );
                            dynamic_views.insert(handle, view);
                            view
                        };
                        lower_direct_list_set(
                            builder,
                            view,
                            value(&values, instruction.inputs[1])?,
                            value(&values, instruction.inputs[2])?,
                        );
                        None
                    } else {
                        let _ = lower_helper(
                            builder,
                            frame,
                            helper_context,
                            helpers.list_set,
                            &helper_values(&values, &instruction.inputs)?,
                        )?;
                        None
                    }
                }
                InstructionKind::ListReversePrefix { element_type } => {
                    if *element_type != ValueType::I64 {
                        return Err(NativeError::Unsupported("direct list reverse element type"));
                    }
                    let handle = instruction.inputs[0];
                    let view = if let Some((storage_index, storage)) =
                        direct_storage.and_then(|plan| plan.storage_for(handle))
                    {
                        if storage.kind != super::direct_storage::DirectStorageKind::List {
                            return Err(NativeError::Unsupported("direct list reverse storage"));
                        }
                        direct_view(&block_views, storage_index)?
                    } else if direct_storage.is_some_and(|plan| plan.is_dynamic(handle)) {
                        // Resolve immediately before mutation; never retain this raw view across an
                        // intervening instruction.
                        resolve_dynamic_direct_storage(builder, frame, value(&values, handle)?)
                    } else {
                        return Err(NativeError::Unsupported("direct list reverse"));
                    };
                    lower_direct_list_reverse_prefix(
                        builder,
                        view,
                        value(&values, instruction.inputs[1])?,
                    );
                    None
                }
                InstructionKind::ListClear => {
                    let (storage_index, _) = direct_storage
                        .and_then(|plan| plan.storage_for(instruction.inputs[0]))
                        .ok_or(NativeError::Unsupported("direct list clear"))?;
                    let alias = value(&values, instruction.inputs[0])?;
                    let view = direct_view(&block_views, storage_index)?;
                    if let Some(extent_storage) = instruction.output.and_then(|output| {
                        direct_storage.and_then(|plan| plan.capacity_extent_for(output.id))
                    }) {
                        guard_direct_list_capacity(
                            builder,
                            view,
                            direct_view(&block_views, extent_storage)?.length,
                        );
                    }
                    let (descriptor, _, _, _) = direct_list(view);
                    let zero = builder.ins().iconst(types::I64, 0);
                    builder.ins().store(
                        MemFlagsData::new(),
                        zero,
                        descriptor,
                        DIRECT_LENGTH_OFFSET,
                    );
                    block_views
                        .get_mut(&storage_index)
                        .expect("verified storage")
                        .length = zero;
                    Some(alias)
                }
                InstructionKind::ListAppend => {
                    if let Some((storage_index, storage)) = direct_storage
                        .and_then(|plan| plan.storage_for(instruction.inputs[0]))
                        .filter(|(_, storage)| {
                            storage.kind == super::direct_storage::DirectStorageKind::List
                        })
                    {
                        let _ = storage;
                        let alias = value(&values, instruction.inputs[0])?;
                        let next_length = lower_direct_list_append(
                            builder,
                            direct_view(&block_views, storage_index)?,
                            value(&values, instruction.inputs[1])?,
                            instruction.output.is_some_and(|output| {
                                direct_storage
                                    .is_some_and(|plan| plan.append_capacity_is_proven(output.id))
                            }),
                        );
                        block_views
                            .get_mut(&storage_index)
                            .expect("verified storage")
                            .length = next_length;
                        Some(alias)
                    } else {
                        let _ = lower_helper(
                            builder,
                            frame,
                            helper_context,
                            helpers.list_append,
                            &helper_values(&values, &instruction.inputs)?,
                        )?;
                        None
                    }
                }
                InstructionKind::ListInsert => {
                    let alias = value(&values, instruction.inputs[0])?;
                    if let Some((storage_index, _)) =
                        direct_storage.and_then(|plan| plan.storage_for(instruction.inputs[0]))
                    {
                        let next_length = lower_direct_list_insert(
                            builder,
                            direct_view(&block_views, storage_index)?,
                            value(&values, instruction.inputs[1])?,
                            value(&values, instruction.inputs[2])?,
                        );
                        block_views
                            .get_mut(&storage_index)
                            .expect("verified storage")
                            .length = next_length;
                        Some(alias)
                    } else {
                        return Err(NativeError::Unsupported("direct list insert"));
                    }
                }
                InstructionKind::ListPop => {
                    if let Some((storage_index, _)) =
                        direct_storage.and_then(|plan| plan.storage_for(instruction.inputs[0]))
                    {
                        let (removed, next_length) = lower_direct_list_pop(
                            builder,
                            direct_view(&block_views, storage_index)?,
                            value(&values, instruction.inputs[1])?,
                        );
                        block_views
                            .get_mut(&storage_index)
                            .expect("verified storage")
                            .length = next_length;
                        Some(removed)
                    } else {
                        return Err(NativeError::Unsupported("direct list pop"));
                    }
                }
                InstructionKind::Call { callee } => {
                    let mut arguments = vec![builder.ins().iconst(types::I64, *callee as i64)];
                    arguments.extend(helper_values(&values, &instruction.inputs)?);
                    Some(lower_helper(
                        builder,
                        frame,
                        helper_context,
                        helpers.direct_call,
                        &arguments,
                    )?)
                }
                _ => return Err(NativeError::Unsupported("instruction")),
            };
            if let (Some(output), Some(result)) = (instruction.output, result) {
                values.insert(output.id, result);
                value_types.insert(output.id, output.ty);
            }
        }
        lower_terminator(
            builder,
            frame,
            &blocks,
            &values,
            &value_types,
            &source.terminator,
            body,
            &block_views,
        )?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct DirectListView {
    descriptor: cranelift_codegen::ir::Value,
    length: cranelift_codegen::ir::Value,
    capacity: cranelift_codegen::ir::Value,
    values: cranelift_codegen::ir::Value,
}

fn direct_view(
    views: &BTreeMap<usize, DirectListView>,
    storage_index: usize,
) -> Result<DirectListView, NativeError> {
    views
        .get(&storage_index)
        .copied()
        .ok_or(NativeError::Unsupported(
            "missing prevalidated direct storage",
        ))
}

fn canonical_direct_gets(
    body: &crate::adaptive_v2::wxir_v2::ir::SnapshotBody,
    storage_for: impl Fn(ValueId) -> Option<usize>,
) -> std::collections::BTreeSet<ValueId> {
    let definitions = body
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter_map(|instruction| instruction.output.map(|output| (output.id, instruction)))
        .collect::<BTreeMap<_, _>>();
    let source = |mut value: ValueId| {
        while let Some(instruction) = definitions.get(&value)
            && matches!(instruction.kind.semantic(), InstructionKind::Copy)
            && instruction.inputs.len() == 1
        {
            value = instruction.inputs[0];
        }
        value
    };
    let incoming = |target: BlockId, position: usize| {
        body.blocks
            .iter()
            .filter_map(|block| match &block.terminator {
                Terminator::Jump {
                    target: candidate,
                    arguments,
                } if *candidate == target => arguments.get(position).copied(),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let preheader_source = |value: ValueId| {
        let value = source(value);
        body.blocks
            .iter()
            .find_map(|block| {
                let position = block
                    .parameters
                    .iter()
                    .position(|parameter| parameter.id == value)?;
                let arguments = incoming(block.id, position);
                (arguments.len() == 1).then(|| source(arguments[0]))
            })
            .unwrap_or(value)
    };
    let integer = |value: ValueId| match definitions
        .get(&preheader_source(value))
        .map(|instruction| instruction.kind.semantic())
    {
        Some(InstructionKind::Constant(Constant::Integer(value))) => Some(*value),
        _ => None,
    };
    let mut proven = std::collections::BTreeSet::new();
    for header in &body.blocks {
        let Terminator::Branch { condition, yes, .. } = header.terminator else {
            continue;
        };
        let Some(condition) = definitions.get(&source(condition)) else {
            continue;
        };
        let conjunction = matches!(condition.kind.semantic(), InstructionKind::BooleanAnd)
            && condition.inputs.len() == 2;
        let comparisons = if conjunction {
            condition
                .inputs
                .iter()
                .filter_map(|value| definitions.get(&source(*value)).copied())
                .collect::<Vec<_>>()
        } else {
            vec![*condition]
        };
        if conjunction && comparisons.len() != 2 {
            continue;
        }
        for comparison in comparisons {
            if !matches!(
                comparison.kind.semantic(),
                InstructionKind::IntegerLessThan
                    | InstructionKind::IntegerCompare {
                        comparison: NumericComparison::LessThan
                    }
            ) || comparison.inputs.len() != 2
            {
                continue;
            }
            let index = source(comparison.inputs[0]);
            let Some((position, parameter)) = header
                .parameters
                .iter()
                .enumerate()
                .find(|(_, parameter)| parameter.id == index)
            else {
                continue;
            };
            let loop_source = |value: ValueId| {
                let value = source(value);
                let Some(position) = header
                    .parameters
                    .iter()
                    .position(|parameter| parameter.id == value)
                else {
                    return preheader_source(value);
                };
                let arguments = incoming(header.id, position);
                if arguments.len() == 2
                    && arguments
                        .iter()
                        .any(|candidate| source(*candidate) == value)
                {
                    arguments
                        .iter()
                        .map(|candidate| preheader_source(*candidate))
                        .find(|candidate| *candidate != value)
                        .unwrap_or(value)
                } else {
                    value
                }
            };
            let is_step = |value: ValueId| {
                definitions.get(&source(value)).is_some_and(|instruction| {
                    matches!(instruction.kind.semantic(), InstructionKind::IntegerAdd)
                        && instruction.inputs.len() == 2
                        && ((source(instruction.inputs[0]) == parameter.id
                            && integer(loop_source(instruction.inputs[1])) == Some(1))
                            || (source(instruction.inputs[1]) == parameter.id
                                && integer(loop_source(instruction.inputs[0])) == Some(1)))
                })
            };
            let index_arguments = incoming(header.id, position);
            if index_arguments.len() != 2
                || !((integer(index_arguments[0]).is_some_and(|value| value >= 0)
                    && is_step(index_arguments[1]))
                    || (integer(index_arguments[1]).is_some_and(|value| value >= 0)
                        && is_step(index_arguments[0])))
            {
                continue;
            }
            let Some(length) = definitions.get(&loop_source(comparison.inputs[1])) else {
                continue;
            };
            if !matches!(length.kind.semantic(), InstructionKind::ListLength)
                || length.inputs.len() != 1
            {
                continue;
            }
            let Some(storage) =
                storage_for(length.inputs[0]).or_else(|| storage_for(source(length.inputs[0])))
            else {
                continue;
            };
            let Some(target) = body.blocks.iter().find(|candidate| candidate.id == yes) else {
                continue;
            };
            for instruction in &target.instructions {
                if instruction.effect.is_barrier()
                    || matches!(
                        instruction.kind.semantic(),
                        InstructionKind::ListSet
                            | InstructionKind::ListReversePrefix { .. }
                            | InstructionKind::ListClear
                            | InstructionKind::ListAppend
                            | InstructionKind::ListInsert
                            | InstructionKind::ListPop
                    )
                {
                    break;
                }
                if matches!(instruction.kind.semantic(), InstructionKind::ListGet)
                    && instruction.inputs.len() == 2
                    && source(instruction.inputs[1]) == parameter.id
                    && storage_for(instruction.inputs[0])
                        .or_else(|| storage_for(source(instruction.inputs[0])))
                        == Some(storage)
                    && let Some(output) = instruction.output
                {
                    proven.insert(output.id);
                }
            }
        }
    }
    proven
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod range_tests {
    use super::*;
    use crate::adaptive_v2::trace::{EntryKind, ExecutableIdentity};
    use crate::adaptive_v2::wxir_v2::ir::{
        Block, Effect, Instruction, SnapshotBody, ValueDef, WxIrAbi,
    };

    fn body(
        init: i64,
        step: i64,
        comparison: NumericComparison,
        get_storage: u32,
        prefix: u8,
        taken: bool,
    ) -> SnapshotBody {
        let value = |id, ty| ValueDef::new(ValueId::new(id), ty);
        let instruction = |kind: InstructionKind,
                           inputs: Vec<ValueId>,
                           id: Option<u32>,
                           ty: ValueType,
                           effect: Effect| {
            Instruction::new(kind, inputs, id.map(|id| value(id, ty)), effect)
        };
        let mut get = Vec::new();
        if prefix != 0 {
            let (kind, effect) = if prefix == 1 {
                (InstructionKind::ListClear, Effect::Write)
            } else {
                (InstructionKind::Helper { helper: 1 }, Effect::Helper)
            };
            get.push(Instruction::new(kind, Vec::new(), None, effect));
        }
        get.extend([
            instruction(
                InstructionKind::ListGet,
                vec![ValueId::new(get_storage), ValueId::new(4)],
                Some(9),
                ValueType::I64,
                Effect::Read,
            ),
            instruction(
                InstructionKind::IntegerAdd,
                vec![ValueId::new(4), ValueId::new(5)],
                Some(10),
                ValueType::I64,
                Effect::Pure,
            ),
        ]);
        SnapshotBody {
            abi: WxIrAbi::V2,
            executable: ExecutableIdentity::new(1, 1),
            schema_epoch: 0,
            entry_kind: EntryKind::FunctionEntry,
            entry: BlockId::new(0),
            parent: None,
            blocks: vec![
                Block::new(
                    BlockId::new(0),
                    vec![value(0, ValueType::Handle), value(1, ValueType::Handle)],
                    vec![
                        instruction(
                            InstructionKind::Constant(Constant::Integer(init)),
                            vec![],
                            Some(2),
                            ValueType::I64,
                            Effect::Pure,
                        ),
                        instruction(
                            InstructionKind::Constant(Constant::Integer(step)),
                            vec![],
                            Some(3),
                            ValueType::I64,
                            Effect::Pure,
                        ),
                        instruction(
                            InstructionKind::ListLength,
                            vec![ValueId::new(0)],
                            Some(7),
                            ValueType::I64,
                            Effect::Read,
                        ),
                    ],
                    Terminator::Jump {
                        target: BlockId::new(1),
                        arguments: vec![ValueId::new(2), ValueId::new(3), ValueId::new(7)],
                    },
                ),
                Block::new(
                    BlockId::new(1),
                    vec![
                        value(4, ValueType::I64),
                        value(5, ValueType::I64),
                        value(6, ValueType::I64),
                    ],
                    vec![instruction(
                        InstructionKind::IntegerCompare { comparison },
                        vec![ValueId::new(4), ValueId::new(6)],
                        Some(8),
                        ValueType::Bool,
                        Effect::Pure,
                    )],
                    Terminator::Branch {
                        condition: ValueId::new(8),
                        yes: BlockId::new(if taken { 2 } else { 3 }),
                        no: BlockId::new(if taken { 3 } else { 2 }),
                    },
                ),
                Block::new(
                    BlockId::new(2),
                    vec![],
                    get,
                    Terminator::Jump {
                        target: BlockId::new(1),
                        arguments: vec![ValueId::new(10), ValueId::new(5), ValueId::new(6)],
                    },
                ),
                Block::new(
                    BlockId::new(3),
                    vec![],
                    vec![],
                    Terminator::Return { values: vec![] },
                ),
            ],
            root_maps: vec![],
            deopts: vec![],
            dependencies: vec![],
        }
    }

    #[test]
    fn canonical_zero_plus_one_same_length_get_proven() {
        assert_eq!(
            canonical_direct_gets(
                &body(0, 1, NumericComparison::LessThan, 0, 0, true),
                |value| Some(value.get() as usize)
            ),
            [ValueId::new(9)].into_iter().collect()
        );
    }

    #[test]
    fn noncanonical_or_mutating_shapes_keep_bounds_check() {
        for rejected in [
            (-1, 1, NumericComparison::LessThan, 0, 0, true),
            (0, 2, NumericComparison::LessThan, 0, 0, true),
            (0, 1, NumericComparison::LessEqual, 0, 0, true),
            (0, 1, NumericComparison::LessThan, 1, 0, true),
            (0, 1, NumericComparison::LessThan, 0, 1, true),
            (0, 1, NumericComparison::LessThan, 0, 2, true),
            (0, 1, NumericComparison::LessThan, 0, 0, false),
        ] {
            assert!(
                canonical_direct_gets(
                    &body(
                        rejected.0, rejected.1, rejected.2, rejected.3, rejected.4, rejected.5
                    ),
                    |value| Some(value.get() as usize)
                )
                .is_empty()
            );
        }
    }
}

fn prevalidate_direct_storage(
    builder: &mut FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    inputs: cranelift_codegen::ir::Value,
    plan: &super::direct_storage::DirectStoragePlan,
) -> BTreeMap<usize, DirectListView> {
    use cranelift_codegen::ir::condcodes::IntCC;

    let flags = MemFlagsData::new();
    let descriptor_base = builder
        .ins()
        .load(types::I64, flags, frame, DIRECT_STORAGE_OFFSET);
    let receipt_base = builder
        .ins()
        .load(types::I64, flags, frame, DIRECT_STORAGE_RECEIPTS_OFFSET);
    let descriptor_present = builder
        .ins()
        .icmp_imm_u(IntCC::NotEqual, descriptor_base, 0);
    let receipt_present = builder.ins().icmp_imm_u(IntCC::NotEqual, receipt_base, 0);
    let storage_present = builder.ins().band(descriptor_present, receipt_present);
    let present = builder.create_block();
    let invalid = builder.create_block();
    builder
        .ins()
        .brif(storage_present, present, &[], invalid, &[]);
    builder.switch_to_block(invalid);
    let error = builder.ins().iconst(types::I32, 2);
    builder.ins().return_(&[error]);
    builder.switch_to_block(present);
    let mut views = BTreeMap::new();
    for (storage_index, storage) in plan.storages().iter().enumerate() {
        let descriptor = builder.ins().iadd_imm_u(
            descriptor_base,
            (storage_index * std::mem::size_of::<super::abi::NativeDirectStorage>()) as i64,
        );
        let receipt = builder.ins().iadd_imm_u(
            receipt_base,
            (storage_index * std::mem::size_of::<super::abi::NativeDirectStorageReceipt>()) as i64,
        );
        let expected_alias = match storage.source {
            super::direct_storage::DirectStorageSource::EntryHandle(input) => builder.ins().load(
                types::I64,
                flags,
                inputs,
                slot_offset(input).expect("verified input slot"),
            ),
            super::direct_storage::DirectStorageSource::OwnedList { identity } => {
                builder.ins().iconst(
                    types::I64,
                    super::direct_storage::owned_alias(identity) as i64,
                )
            }
        };
        let magic = builder
            .ins()
            .load(types::I64, flags, descriptor, DIRECT_MAGIC_OFFSET);
        let abi = builder
            .ins()
            .load(types::I32, flags, descriptor, DIRECT_ABI_OFFSET);
        let strategy = builder
            .ins()
            .load(types::I32, flags, descriptor, DIRECT_STRATEGY_OFFSET);
        let alias = builder
            .ins()
            .load(types::I64, flags, descriptor, DIRECT_ALIAS_OFFSET);
        let version = builder
            .ins()
            .load(types::I64, flags, descriptor, DIRECT_VERSION_OFFSET);
        let owner = builder
            .ins()
            .load(types::I64, flags, descriptor, DIRECT_OWNER_OFFSET);
        let layout_epoch =
            builder
                .ins()
                .load(types::I64, flags, descriptor, DIRECT_LAYOUT_EPOCH_OFFSET);
        let initial_length =
            builder
                .ins()
                .load(types::I64, flags, descriptor, DIRECT_LENGTH_OFFSET);
        let capacity = builder
            .ins()
            .load(types::I64, flags, descriptor, DIRECT_CAPACITY_OFFSET);
        let values = builder
            .ins()
            .load(types::I64, flags, descriptor, DIRECT_VALUES_OFFSET);
        let receipt_identity =
            builder
                .ins()
                .load(types::I64, flags, receipt, RECEIPT_STORAGE_IDENTITY_OFFSET);
        let receipt_strategy =
            builder
                .ins()
                .load(types::I32, flags, receipt, RECEIPT_STRATEGY_OFFSET);
        let receipt_alias = builder
            .ins()
            .load(types::I64, flags, receipt, RECEIPT_ALIAS_OFFSET);
        let receipt_owner = builder
            .ins()
            .load(types::I64, flags, receipt, RECEIPT_OWNER_OFFSET);
        let receipt_layout_epoch =
            builder
                .ins()
                .load(types::I64, flags, receipt, RECEIPT_LAYOUT_EPOCH_OFFSET);
        let receipt_version =
            builder
                .ins()
                .load(types::I64, flags, receipt, RECEIPT_VERSION_OFFSET);
        let masked_version = builder.ins().band_imm_u(version, 1);
        let expected_identity = builder
            .ins()
            .iconst(types::I64, storage.source.receipt_identity() as i64);
        let mut valid = builder
            .ins()
            .icmp_imm_u(IntCC::Equal, magic, DIRECT_STORAGE_MAGIC as i64);
        for condition in [
            builder
                .ins()
                .icmp_imm_u(IntCC::Equal, abi, i64::from(DIRECT_STORAGE_ABI)),
            builder.ins().icmp_imm_u(IntCC::Equal, strategy, 1),
            builder.ins().icmp(IntCC::Equal, strategy, receipt_strategy),
            builder
                .ins()
                .icmp(IntCC::Equal, receipt_identity, expected_identity),
            builder
                .ins()
                .icmp(IntCC::Equal, receipt_alias, expected_alias),
            builder.ins().icmp(IntCC::Equal, alias, receipt_alias),
            builder.ins().icmp(IntCC::Equal, owner, receipt_owner),
            builder
                .ins()
                .icmp(IntCC::Equal, layout_epoch, receipt_layout_epoch),
            builder.ins().icmp(IntCC::Equal, version, receipt_version),
            builder.ins().icmp_imm_u(IntCC::Equal, masked_version, 0),
            builder
                .ins()
                .icmp(IntCC::UnsignedLessThanOrEqual, initial_length, capacity),
            builder
                .ins()
                .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, capacity, 0),
            builder.ins().icmp_imm_u(IntCC::NotEqual, values, 0),
        ] {
            valid = builder.ins().band(valid, condition);
        }
        let ready = builder.create_block();
        let invalid = builder.create_block();
        builder.ins().brif(valid, ready, &[], invalid, &[]);
        builder.switch_to_block(invalid);
        let error = builder.ins().iconst(types::I32, 2);
        builder.ins().return_(&[error]);
        builder.switch_to_block(ready);
        views.insert(
            storage_index,
            DirectListView {
                descriptor,
                length: initial_length,
                capacity,
                values,
            },
        );
    }
    views
}

fn prevalidate_dynamic_direct_storage(
    builder: &mut FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
) {
    use cranelift_codegen::ir::condcodes::IntCC;

    let flags = MemFlagsData::new();
    let descriptors = builder
        .ins()
        .load(types::I64, flags, frame, DIRECT_STORAGE_OFFSET);
    let receipts = builder
        .ins()
        .load(types::I64, flags, frame, DIRECT_STORAGE_RECEIPTS_OFFSET);
    let count = builder
        .ins()
        .load(types::I32, flags, frame, DIRECT_STORAGE_COUNT_OFFSET);
    let index_base = builder
        .ins()
        .load(types::I64, flags, frame, DIRECT_STORAGE_INDEX_OFFSET);
    let mut valid = builder.ins().icmp_imm_u(IntCC::NotEqual, descriptors, 0);
    for condition in [
        builder.ins().icmp_imm_u(IntCC::NotEqual, receipts, 0),
        builder.ins().icmp_imm_u(IntCC::NotEqual, index_base, 0),
        builder.ins().icmp_imm_u(IntCC::NotEqual, count, 0),
        builder.ins().icmp_imm_u(
            IntCC::UnsignedLessThanOrEqual,
            count,
            super::direct_storage::MAX_DYNAMIC_DIRECT_STORAGES as i64,
        ),
    ] {
        valid = builder.ins().band(valid, condition);
    }
    let header = builder.create_block();
    let body = builder.create_block();
    let next = builder.create_block();
    let done = builder.create_block();
    let invalid = builder.create_block();
    builder.append_block_param(header, types::I32);
    let zero = builder.ins().iconst(types::I32, 0);
    builder
        .ins()
        .brif(valid, header, &[BlockArg::from(zero)], invalid, &[]);

    builder.switch_to_block(invalid);
    let error = builder.ins().iconst(types::I32, 2);
    builder.ins().return_(&[error]);

    builder.switch_to_block(header);
    let index = builder.block_params(header)[0];
    let in_range = builder.ins().icmp(IntCC::UnsignedLessThan, index, count);
    builder.ins().brif(in_range, body, &[], done, &[]);

    builder.switch_to_block(body);
    let wide_index = builder.ins().uextend(types::I64, index);
    let descriptor_offset = builder.ins().imul_imm_u(
        wide_index,
        std::mem::size_of::<super::abi::NativeDirectStorage>() as i64,
    );
    let receipt_offset = builder.ins().imul_imm_u(
        wide_index,
        std::mem::size_of::<super::abi::NativeDirectStorageReceipt>() as i64,
    );
    let descriptor = builder.ins().iadd(descriptors, descriptor_offset);
    let receipt = builder.ins().iadd(receipts, receipt_offset);
    let magic = builder
        .ins()
        .load(types::I64, flags, descriptor, DIRECT_MAGIC_OFFSET);
    let abi = builder
        .ins()
        .load(types::I32, flags, descriptor, DIRECT_ABI_OFFSET);
    let strategy = builder
        .ins()
        .load(types::I32, flags, descriptor, DIRECT_STRATEGY_OFFSET);
    let alias = builder
        .ins()
        .load(types::I64, flags, descriptor, DIRECT_ALIAS_OFFSET);
    let owner = builder
        .ins()
        .load(types::I64, flags, descriptor, DIRECT_OWNER_OFFSET);
    let layout = builder
        .ins()
        .load(types::I64, flags, descriptor, DIRECT_LAYOUT_EPOCH_OFFSET);
    let version = builder
        .ins()
        .load(types::I64, flags, descriptor, DIRECT_VERSION_OFFSET);
    let length = builder
        .ins()
        .load(types::I64, flags, descriptor, DIRECT_LENGTH_OFFSET);
    let capacity = builder
        .ins()
        .load(types::I64, flags, descriptor, DIRECT_CAPACITY_OFFSET);
    let values = builder
        .ins()
        .load(types::I64, flags, descriptor, DIRECT_VALUES_OFFSET);
    let receipt_strategy = builder
        .ins()
        .load(types::I32, flags, receipt, RECEIPT_STRATEGY_OFFSET);
    let receipt_alias = builder
        .ins()
        .load(types::I64, flags, receipt, RECEIPT_ALIAS_OFFSET);
    let receipt_owner = builder
        .ins()
        .load(types::I64, flags, receipt, RECEIPT_OWNER_OFFSET);
    let receipt_layout =
        builder
            .ins()
            .load(types::I64, flags, receipt, RECEIPT_LAYOUT_EPOCH_OFFSET);
    let receipt_version = builder
        .ins()
        .load(types::I64, flags, receipt, RECEIPT_VERSION_OFFSET);
    let masked_version = builder.ins().band_imm_u(version, 1);
    let mut descriptor_valid =
        builder
            .ins()
            .icmp_imm_u(IntCC::Equal, magic, DIRECT_STORAGE_MAGIC as i64);
    for condition in [
        builder
            .ins()
            .icmp_imm_u(IntCC::Equal, abi, i64::from(DIRECT_STORAGE_ABI)),
        builder.ins().icmp_imm_u(IntCC::Equal, strategy, 1),
        builder.ins().icmp(IntCC::Equal, strategy, receipt_strategy),
        builder.ins().icmp(IntCC::Equal, alias, receipt_alias),
        builder.ins().icmp(IntCC::Equal, owner, receipt_owner),
        builder.ins().icmp(IntCC::Equal, layout, receipt_layout),
        builder.ins().icmp(IntCC::Equal, version, receipt_version),
        builder.ins().icmp_imm_u(IntCC::Equal, masked_version, 0),
        builder
            .ins()
            .icmp(IntCC::UnsignedLessThanOrEqual, length, capacity),
        builder
            .ins()
            .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, capacity, 0),
        builder.ins().icmp_imm_u(IntCC::NotEqual, values, 0),
    ] {
        descriptor_valid = builder.ins().band(descriptor_valid, condition);
    }
    builder
        .ins()
        .brif(descriptor_valid, next, &[], invalid, &[]);

    builder.switch_to_block(next);
    let one = builder.ins().iconst(types::I32, 1);
    let index = builder.ins().iadd(index, one);
    builder.ins().jump(header, &[BlockArg::from(index)]);

    builder.switch_to_block(done);
}

fn resolve_dynamic_direct_storage(
    builder: &mut FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    expected_alias: cranelift_codegen::ir::Value,
) -> DirectListView {
    use cranelift_codegen::ir::condcodes::IntCC;

    let flags = MemFlagsData::new();
    let descriptors = builder
        .ins()
        .load(types::I64, flags, frame, DIRECT_STORAGE_OFFSET);
    let receipts = builder
        .ins()
        .load(types::I64, flags, frame, DIRECT_STORAGE_RECEIPTS_OFFSET);
    let count = builder
        .ins()
        .load(types::I32, flags, frame, DIRECT_STORAGE_COUNT_OFFSET);
    let index_base = builder
        .ins()
        .load(types::I64, flags, frame, DIRECT_STORAGE_INDEX_OFFSET);
    let slot = builder.ins().ireduce(types::I32, expected_alias);
    let slot_valid = builder.ins().icmp_imm_u(
        IntCC::UnsignedLessThan,
        slot,
        crate::adaptive_v2::handles::NATIVE_HANDLE_CAPACITY as i64,
    );
    let indexed = builder.create_block();
    let invalid = builder.create_block();
    builder.ins().brif(slot_valid, indexed, &[], invalid, &[]);
    builder.switch_to_block(invalid);
    let error = builder.ins().iconst(types::I32, 2);
    builder.ins().return_(&[error]);
    builder.switch_to_block(indexed);
    let slot = builder.ins().uextend(types::I64, slot);
    let index_address = builder.ins().iadd(index_base, slot);
    let selected_index = builder.ins().load(types::I8, flags, index_address, 0);
    let selected_index_i32 = builder.ins().uextend(types::I32, selected_index);
    let in_count = builder
        .ins()
        .icmp(IntCC::UnsignedLessThan, selected_index_i32, count);
    let ready = builder.create_block();
    let invalid = builder.create_block();
    builder.ins().brif(in_count, ready, &[], invalid, &[]);
    builder.switch_to_block(invalid);
    let error = builder.ins().iconst(types::I32, 2);
    builder.ins().return_(&[error]);
    builder.switch_to_block(ready);
    let selected_index = builder.ins().uextend(types::I64, selected_index);
    let descriptor_offset = builder.ins().imul_imm_u(
        selected_index,
        std::mem::size_of::<super::abi::NativeDirectStorage>() as i64,
    );
    let receipt_offset = builder.ins().imul_imm_u(
        selected_index,
        std::mem::size_of::<super::abi::NativeDirectStorageReceipt>() as i64,
    );
    let selected_descriptor = builder.ins().iadd(descriptors, descriptor_offset);
    let selected_receipt = builder.ins().iadd(receipts, receipt_offset);
    let descriptor_alias =
        builder
            .ins()
            .load(types::I64, flags, selected_descriptor, DIRECT_ALIAS_OFFSET);
    let length = builder
        .ins()
        .load(types::I64, flags, selected_descriptor, DIRECT_LENGTH_OFFSET);
    let capacity = builder.ins().load(
        types::I64,
        flags,
        selected_descriptor,
        DIRECT_CAPACITY_OFFSET,
    );
    let values = builder
        .ins()
        .load(types::I64, flags, selected_descriptor, DIRECT_VALUES_OFFSET);
    let receipt_identity = builder.ins().load(
        types::I64,
        flags,
        selected_receipt,
        RECEIPT_STORAGE_IDENTITY_OFFSET,
    );
    let mut valid = builder
        .ins()
        .icmp(IntCC::Equal, descriptor_alias, expected_alias);
    let receipt_matches = builder
        .ins()
        .icmp(IntCC::Equal, receipt_identity, expected_alias);
    valid = builder.ins().band(valid, receipt_matches);
    let valid_block = builder.create_block();
    let invalid = builder.create_block();
    builder.ins().brif(valid, valid_block, &[], invalid, &[]);
    builder.switch_to_block(invalid);
    let error = builder.ins().iconst(types::I32, 2);
    builder.ins().return_(&[error]);
    builder.switch_to_block(valid_block);
    DirectListView {
        descriptor: selected_descriptor,
        length,
        capacity,
        values,
    }
}

fn lower_direct_list_get(
    builder: &mut FunctionBuilder<'_>,
    view: DirectListView,
    index: cranelift_codegen::ir::Value,
    range_proven: bool,
) -> cranelift_codegen::ir::Value {
    use cranelift_codegen::ir::condcodes::IntCC;

    let flags = MemFlagsData::new();
    if !range_proven {
        let invalid = builder.create_block();
        let valid = builder
            .ins()
            .icmp(IntCC::UnsignedLessThan, index, view.length);
        let load = builder.create_block();
        builder.ins().brif(valid, load, &[], invalid, &[]);
        builder.switch_to_block(invalid);
        let error = builder.ins().iconst(types::I32, 2);
        builder.ins().return_(&[error]);
        builder.switch_to_block(load);
    }
    let offset = builder.ins().imul_imm_u(index, 8);
    let address = builder.ins().iadd(view.values, offset);
    // SAFETY: [Categories 3, 8, 10, and 13] the checked index is within the
    // owned Arc segment retained by DirectStorageLease until native return.
    builder.ins().load(types::I64, flags, address, 0)
}

fn direct_list(
    view: DirectListView,
) -> (
    cranelift_codegen::ir::Value,
    cranelift_codegen::ir::Value,
    cranelift_codegen::ir::Value,
    cranelift_codegen::ir::Value,
) {
    (view.descriptor, view.length, view.capacity, view.values)
}

fn lower_direct_list_set(
    builder: &mut FunctionBuilder<'_>,
    view: DirectListView,
    index: cranelift_codegen::ir::Value,
    value: cranelift_codegen::ir::Value,
) {
    use cranelift_codegen::ir::condcodes::IntCC;
    let (_, length, _, values) = direct_list(view);
    let valid = builder.ins().icmp(IntCC::UnsignedLessThan, index, length);
    let store = builder.create_block();
    let invalid = builder.create_block();
    builder.ins().brif(valid, store, &[], invalid, &[]);
    builder.switch_to_block(invalid);
    let error = builder.ins().iconst(types::I32, 2);
    builder.ins().return_(&[error]);
    builder.switch_to_block(store);
    let offset = builder.ins().imul_imm_u(index, 8);
    let address = builder.ins().iadd(values, offset);
    builder.ins().store(MemFlagsData::new(), value, address, 0);
}

fn lower_direct_list_reverse_prefix(
    builder: &mut FunctionBuilder<'_>,
    view: DirectListView,
    end: cranelift_codegen::ir::Value,
) {
    use cranelift_codegen::ir::condcodes::IntCC;
    let (_, length, _, values) = direct_list(view);
    let positive = builder
        .ins()
        .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, end, 1);
    let in_bounds = builder
        .ins()
        .icmp(IntCC::UnsignedLessThanOrEqual, end, length);
    let valid = builder.ins().band(positive, in_bounds);
    let ready = builder.create_block();
    let invalid = builder.create_block();
    builder.ins().brif(valid, ready, &[], invalid, &[]);
    builder.switch_to_block(invalid);
    let error = builder.ins().iconst(types::I32, 2);
    builder.ins().return_(&[error]);
    builder.switch_to_block(ready);

    let loop_block = builder.create_block();
    builder.append_block_param(loop_block, types::I64);
    builder.append_block_param(loop_block, types::I64);
    let swap = builder.create_block();
    let done = builder.create_block();
    let left = builder.ins().iconst(types::I64, 0);
    let right = builder.ins().iadd_imm_s(end, -1);
    builder
        .ins()
        .jump(loop_block, &[BlockArg::from(left), BlockArg::from(right)]);
    builder.switch_to_block(loop_block);
    let left = builder.block_params(loop_block)[0];
    let right = builder.block_params(loop_block)[1];
    let more = builder.ins().icmp(IntCC::UnsignedLessThan, left, right);
    builder.ins().brif(more, swap, &[], done, &[]);
    builder.switch_to_block(swap);
    let left_offset = builder.ins().imul_imm_u(left, 8);
    let left_address = builder.ins().iadd(values, left_offset);
    let right_offset = builder.ins().imul_imm_u(right, 8);
    let right_address = builder.ins().iadd(values, right_offset);
    let flags = MemFlagsData::new();
    let left_value = builder.ins().load(types::I64, flags, left_address, 0);
    let right_value = builder.ins().load(types::I64, flags, right_address, 0);
    builder.ins().store(flags, right_value, left_address, 0);
    builder.ins().store(flags, left_value, right_address, 0);
    let next_left = builder.ins().iadd_imm_u(left, 1);
    let next_right = builder.ins().iadd_imm_s(right, -1);
    builder.ins().jump(
        loop_block,
        &[BlockArg::from(next_left), BlockArg::from(next_right)],
    );
    builder.switch_to_block(done);
}

fn lower_direct_list_copy(
    builder: &mut FunctionBuilder<'_>,
    destination: DirectListView,
    source: DirectListView,
) {
    use cranelift_codegen::ir::condcodes::IntCC;
    guard_direct_list_capacity(builder, destination, source.length);
    let loop_block = builder.create_block();
    builder.append_block_param(loop_block, types::I64);
    let copy = builder.create_block();
    let done = builder.create_block();
    let index = builder.ins().iconst(types::I64, 0);
    builder.ins().jump(loop_block, &[BlockArg::from(index)]);
    builder.switch_to_block(loop_block);
    let index = builder.block_params(loop_block)[0];
    let more = builder
        .ins()
        .icmp(IntCC::UnsignedLessThan, index, source.length);
    builder.ins().brif(more, copy, &[], done, &[]);
    builder.switch_to_block(copy);
    let offset = builder.ins().imul_imm_u(index, 8);
    let source_address = builder.ins().iadd(source.values, offset);
    let destination_address = builder.ins().iadd(destination.values, offset);
    let flags = MemFlagsData::new();
    let value = builder.ins().load(types::I64, flags, source_address, 0);
    builder.ins().store(flags, value, destination_address, 0);
    let next = builder.ins().iadd_imm_u(index, 1);
    builder.ins().jump(loop_block, &[BlockArg::from(next)]);
    builder.switch_to_block(done);
    builder.ins().store(
        MemFlagsData::new(),
        source.length,
        destination.descriptor,
        DIRECT_LENGTH_OFFSET,
    );
}

fn lower_direct_list_append(
    builder: &mut FunctionBuilder<'_>,
    view: DirectListView,
    value: cranelift_codegen::ir::Value,
    capacity_proven: bool,
) -> cranelift_codegen::ir::Value {
    use cranelift_codegen::ir::condcodes::IntCC;
    let (descriptor, length, capacity, values) = direct_list(view);
    if !capacity_proven {
        let valid = builder
            .ins()
            .icmp(IntCC::UnsignedLessThan, length, capacity);
        let store = builder.create_block();
        let invalid = builder.create_block();
        builder.ins().brif(valid, store, &[], invalid, &[]);
        builder.switch_to_block(invalid);
        let error = builder.ins().iconst(types::I32, 2);
        builder.ins().return_(&[error]);
        builder.switch_to_block(store);
    }
    let offset = builder.ins().imul_imm_u(length, 8);
    let address = builder.ins().iadd(values, offset);
    builder.ins().store(MemFlagsData::new(), value, address, 0);
    let next = builder.ins().iadd_imm_u(length, 1);
    builder
        .ins()
        .store(MemFlagsData::new(), next, descriptor, DIRECT_LENGTH_OFFSET);
    next
}

fn guard_direct_list_capacity(
    builder: &mut FunctionBuilder<'_>,
    view: DirectListView,
    extent: cranelift_codegen::ir::Value,
) {
    use cranelift_codegen::ir::condcodes::IntCC;
    let valid = builder
        .ins()
        .icmp(IntCC::UnsignedGreaterThanOrEqual, view.capacity, extent);
    let proceed = builder.create_block();
    let invalid = builder.create_block();
    builder.ins().brif(valid, proceed, &[], invalid, &[]);
    builder.switch_to_block(invalid);
    let error = builder.ins().iconst(types::I32, 2);
    builder.ins().return_(&[error]);
    builder.switch_to_block(proceed);
}

fn lower_direct_list_insert(
    builder: &mut FunctionBuilder<'_>,
    view: DirectListView,
    index: cranelift_codegen::ir::Value,
    value: cranelift_codegen::ir::Value,
) -> cranelift_codegen::ir::Value {
    use cranelift_codegen::ir::condcodes::IntCC;
    let (descriptor, length, capacity, values) = direct_list(view);
    let has_capacity = builder
        .ins()
        .icmp(IntCC::UnsignedLessThan, length, capacity);
    let proceed = builder.create_block();
    let invalid = builder.create_block();
    builder.ins().brif(has_capacity, proceed, &[], invalid, &[]);
    builder.switch_to_block(invalid);
    let error = builder.ins().iconst(types::I32, 2);
    builder.ins().return_(&[error]);
    builder.switch_to_block(proceed);
    let negative = builder.ins().icmp_imm_s(IntCC::SignedLessThan, index, 0);
    let from_end = builder.ins().iadd(length, index);
    let zero = builder.ins().iconst(types::I64, 0);
    let below_zero = builder.ins().icmp_imm_s(IntCC::SignedLessThan, from_end, 0);
    let negative_index = builder.ins().select(below_zero, zero, from_end);
    let above_length = builder.ins().icmp(IntCC::SignedGreaterThan, index, length);
    let positive_index = builder.ins().select(above_length, length, index);
    let normalized = builder
        .ins()
        .select(negative, negative_index, positive_index);
    let shift = builder.create_block();
    builder.append_block_param(shift, types::I64);
    let move_one = builder.create_block();
    let done = builder.create_block();
    builder.ins().jump(shift, &[BlockArg::from(length)]);
    builder.switch_to_block(shift);
    let cursor = builder.block_params(shift)[0];
    let more = builder
        .ins()
        .icmp(IntCC::UnsignedGreaterThan, cursor, normalized);
    builder.ins().brif(more, move_one, &[], done, &[]);
    builder.switch_to_block(move_one);
    let previous = builder.ins().iadd_imm_s(cursor, -1);
    let source_offset = builder.ins().imul_imm_u(previous, 8);
    let source = builder.ins().iadd(values, source_offset);
    let item = builder
        .ins()
        .load(types::I64, MemFlagsData::new(), source, 0);
    let target_offset = builder.ins().imul_imm_u(cursor, 8);
    let target = builder.ins().iadd(values, target_offset);
    builder.ins().store(MemFlagsData::new(), item, target, 0);
    builder.ins().jump(shift, &[BlockArg::from(previous)]);
    builder.switch_to_block(done);
    let target_offset = builder.ins().imul_imm_u(normalized, 8);
    let target = builder.ins().iadd(values, target_offset);
    builder.ins().store(MemFlagsData::new(), value, target, 0);
    let next_length = builder.ins().iadd_imm_u(length, 1);
    builder.ins().store(
        MemFlagsData::new(),
        next_length,
        descriptor,
        DIRECT_LENGTH_OFFSET,
    );
    next_length
}

fn lower_direct_list_pop(
    builder: &mut FunctionBuilder<'_>,
    view: DirectListView,
    index: cranelift_codegen::ir::Value,
) -> (cranelift_codegen::ir::Value, cranelift_codegen::ir::Value) {
    use cranelift_codegen::ir::condcodes::IntCC;
    let (descriptor, length, _, values) = direct_list(view);
    let negative = builder.ins().icmp_imm_s(IntCC::SignedLessThan, index, 0);
    let from_end = builder.ins().iadd(length, index);
    let normalized = builder.ins().select(negative, from_end, index);
    let nonnegative = builder
        .ins()
        .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, normalized, 0);
    let in_bounds = builder
        .ins()
        .icmp(IntCC::UnsignedLessThan, normalized, length);
    let valid = builder.ins().band(nonnegative, in_bounds);
    let proceed = builder.create_block();
    let invalid = builder.create_block();
    builder.ins().brif(valid, proceed, &[], invalid, &[]);
    builder.switch_to_block(invalid);
    let error = builder.ins().iconst(types::I32, 2);
    builder.ins().return_(&[error]);
    builder.switch_to_block(proceed);
    let offset = builder.ins().imul_imm_u(normalized, 8);
    let address = builder.ins().iadd(values, offset);
    let removed = builder
        .ins()
        .load(types::I64, MemFlagsData::new(), address, 0);
    let shift = builder.create_block();
    builder.append_block_param(shift, types::I64);
    let move_one = builder.create_block();
    let done = builder.create_block();
    builder.ins().jump(shift, &[BlockArg::from(normalized)]);
    builder.switch_to_block(shift);
    let cursor = builder.block_params(shift)[0];
    let next = builder.ins().iadd_imm_u(cursor, 1);
    let more = builder.ins().icmp(IntCC::UnsignedLessThan, next, length);
    builder.ins().brif(more, move_one, &[], done, &[]);
    builder.switch_to_block(move_one);
    let source_offset = builder.ins().imul_imm_u(next, 8);
    let source = builder.ins().iadd(values, source_offset);
    let item = builder
        .ins()
        .load(types::I64, MemFlagsData::new(), source, 0);
    let target_offset = builder.ins().imul_imm_u(cursor, 8);
    let target = builder.ins().iadd(values, target_offset);
    builder.ins().store(MemFlagsData::new(), item, target, 0);
    builder.ins().jump(shift, &[BlockArg::from(next)]);
    builder.switch_to_block(done);
    let next_length = builder.ins().iadd_imm_s(length, -1);
    builder.ins().store(
        MemFlagsData::new(),
        next_length,
        descriptor,
        DIRECT_LENGTH_OFFSET,
    );
    (removed, next_length)
}

fn lower_guard(
    builder: &mut FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    condition: cranelift_codegen::ir::Value,
    guard: u32,
    safepoint: u32,
) -> Result<(), NativeError> {
    let pass = builder.create_block();
    let fail = builder.create_block();
    builder.ins().brif(condition, pass, &[], fail, &[]);
    builder.switch_to_block(fail);
    let flags = MemFlagsData::new();
    let exit_kind = builder.ins().iconst(types::I32, 1);
    let guard_id = builder.ins().iconst(types::I32, i64::from(guard));
    let safepoint_id = builder.ins().iconst(types::I32, i64::from(safepoint));
    builder
        .ins()
        .store(flags, exit_kind, frame, EXIT_KIND_OFFSET);
    builder.ins().store(flags, guard_id, frame, GUARD_ID_OFFSET);
    builder
        .ins()
        .store(flags, safepoint_id, frame, SAFEPOINT_ID_OFFSET);
    builder.ins().store(flags, guard_id, frame, DEOPT_ID_OFFSET);
    let count = builder.ins().load(types::I64, flags, frame, DEOPTS_OFFSET);
    let count = builder.ins().iadd_imm_u(count, 1);
    builder.ins().store(flags, count, frame, DEOPTS_OFFSET);
    let exit = builder.ins().iconst(types::I32, 1);
    builder.ins().return_(&[exit]);
    builder.switch_to_block(pass);
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "terminators need both guest SSA values and the implicit direct-storage SSA state"
)]
fn lower_terminator(
    builder: &mut FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    blocks: &BTreeMap<BlockId, cranelift_codegen::ir::Block>,
    values: &BTreeMap<ValueId, cranelift_codegen::ir::Value>,
    value_types: &BTreeMap<ValueId, ValueType>,
    terminator: &Terminator,
    body: &crate::adaptive_v2::wxir_v2::ir::SnapshotBody,
    direct_views: &BTreeMap<usize, DirectListView>,
) -> Result<(), NativeError> {
    match terminator {
        Terminator::Jump { target, arguments } => {
            let mut arguments = block_args(values, arguments)?;
            arguments.extend(
                direct_views
                    .values()
                    .map(|view| BlockArg::from(view.length)),
            );
            builder.ins().jump(block(blocks, *target)?, &arguments);
        }
        Terminator::Branch { condition, yes, no } => {
            let arguments = direct_views
                .values()
                .map(|view| BlockArg::from(view.length))
                .collect::<Vec<_>>();
            builder.ins().brif(
                value(values, *condition)?,
                block(blocks, *yes)?,
                &arguments,
                block(blocks, *no)?,
                &arguments,
            );
        }
        Terminator::Return { values: returned } => {
            let flags = MemFlagsData::new();
            let outputs = builder.ins().load(types::I64, flags, frame, OUTPUTS_OFFSET);
            for (index, id) in returned.iter().enumerate() {
                let payload = value(values, *id)?;
                let offset = slot_offset(index)?;
                let tag = builder.ins().iconst(
                    types::I32,
                    i64::from(value_tag(
                        *value_types
                            .get(id)
                            .ok_or(NativeError::Unsupported("missing output type"))?,
                    )),
                );
                builder
                    .ins()
                    .store(flags, tag, outputs, offset - SLOT_PAYLOAD_OFFSET);
                builder.ins().store(flags, payload, outputs, offset);
            }
            let exit = builder.ins().iconst(types::I32, 0);
            builder.ins().return_(&[exit]);
        }
        Terminator::SideExit { id, values: exited } => {
            let flags = MemFlagsData::new();
            let outputs = builder.ins().load(types::I64, flags, frame, OUTPUTS_OFFSET);
            for (index, value_id) in exited.iter().enumerate() {
                let payload = value(values, *value_id)?;
                let offset = slot_offset(index)?;
                let tag = builder.ins().iconst(
                    types::I32,
                    i64::from(value_tag(
                        *value_types
                            .get(value_id)
                            .ok_or(NativeError::Unsupported("missing side-exit type"))?,
                    )),
                );
                builder
                    .ins()
                    .store(flags, tag, outputs, offset - SLOT_PAYLOAD_OFFSET);
                builder.ins().store(flags, payload, outputs, offset);
            }
            let exit_kind = builder.ins().iconst(types::I32, 1);
            let exit_id = builder.ins().iconst(types::I32, i64::from(*id));
            builder
                .ins()
                .store(flags, exit_kind, frame, EXIT_KIND_OFFSET);
            builder.ins().store(flags, exit_id, frame, EXIT_ID_OFFSET);
            builder.ins().store(flags, exit_id, frame, DEOPT_ID_OFFSET);
            let recipe = body
                .deopts
                .iter()
                .find(|recipe| recipe.id == *id)
                .ok_or(NativeError::Unsupported("missing side-exit deopt"))?;
            let safepoint = builder
                .ins()
                .iconst(types::I32, i64::from(recipe.root_point.get()));
            builder
                .ins()
                .store(flags, safepoint, frame, SAFEPOINT_ID_OFFSET);
            let exit = builder.ins().iconst(types::I32, 1);
            builder.ins().return_(&[exit]);
        }
        Terminator::Backedge {
            target_pc,
            safepoint,
        } => {
            let flags = MemFlagsData::new();
            let recipe = body
                .deopts
                .iter()
                .find(|recipe| recipe.root_point == *safepoint)
                .ok_or(NativeError::Unsupported("missing backedge deopt"))?;
            let exit_kind = builder.ins().iconst(types::I32, 1);
            let exit_id = builder.ins().iconst(types::I32, i64::from(*target_pc));
            let safepoint_id = builder.ins().iconst(types::I32, i64::from(safepoint.get()));
            let deopt_id = builder.ins().iconst(types::I32, i64::from(recipe.id));
            builder
                .ins()
                .store(flags, exit_kind, frame, EXIT_KIND_OFFSET);
            builder.ins().store(flags, exit_id, frame, EXIT_ID_OFFSET);
            builder
                .ins()
                .store(flags, safepoint_id, frame, SAFEPOINT_ID_OFFSET);
            builder.ins().store(flags, deopt_id, frame, DEOPT_ID_OFFSET);
            let exit = builder.ins().iconst(types::I32, 1);
            builder.ins().return_(&[exit]);
        }
        Terminator::IrreducibleBackedge => {
            return Err(NativeError::Unsupported("irreducible backedge"));
        }
    }
    Ok(())
}

fn lower_constant(
    builder: &mut FunctionBuilder<'_>,
    constant: &Constant,
) -> Result<cranelift_codegen::ir::Value, NativeError> {
    match constant {
        Constant::Integer(value) => Ok(builder.ins().iconst(types::I64, *value)),
        Constant::Boolean(value) => Ok(builder.ins().iconst(types::I64, i64::from(*value))),
        Constant::HandleBits(value) => Ok(builder.ins().iconst(types::I64, *value as i64)),
        Constant::FloatBits(value) => Ok(builder
            .ins()
            .f64const(cranelift_codegen::ir::immediates::Ieee64::with_bits(*value))),
        Constant::UndefinedDead => Err(NativeError::Unsupported("constant")),
    }
}

fn native_type(ty: ValueType) -> Result<cranelift_codegen::ir::Type, NativeError> {
    match ty {
        ValueType::I64 | ValueType::Bool | ValueType::Handle => Ok(types::I64),
        ValueType::F64 => Ok(types::F64),
        ValueType::BorrowedView => Err(NativeError::Unsupported("value type")),
    }
}

fn output_types(snapshot: &VerifiedSnapshot) -> Result<Vec<ValueType>, NativeError> {
    let types = snapshot
        .body()
        .blocks
        .iter()
        .flat_map(|block| {
            block.parameters.iter().chain(
                block
                    .instructions
                    .iter()
                    .filter_map(|instruction| instruction.output.as_ref()),
            )
        })
        .map(|value| (value.id, value.ty))
        .collect::<BTreeMap<_, _>>();
    let returned = snapshot
        .body()
        .blocks
        .iter()
        .find_map(|block| match &block.terminator {
            Terminator::Return { values } | Terminator::SideExit { values, .. } => Some(values),
            _ => None,
        })
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    returned
        .iter()
        .map(|id| {
            types
                .get(id)
                .copied()
                .ok_or(NativeError::Unsupported("missing return type"))
        })
        .collect()
}

fn validate_supported_types(snapshot: &VerifiedSnapshot) -> Result<(), NativeError> {
    if snapshot
        .body()
        .blocks
        .iter()
        .flat_map(|block| {
            block.parameters.iter().chain(
                block
                    .instructions
                    .iter()
                    .filter_map(|instruction| instruction.output.as_ref()),
            )
        })
        .any(|value| matches!(value.ty, ValueType::BorrowedView))
    {
        return Err(NativeError::Unsupported("value type"));
    }
    Ok(())
}

const fn value_tag(ty: ValueType) -> u32 {
    match ty {
        ValueType::I64 => 1,
        ValueType::Bool => 2,
        ValueType::Handle => 3,
        ValueType::F64 => 4,
        ValueType::BorrowedView => 0,
    }
}

const fn integer_condition(
    comparison: NumericComparison,
) -> cranelift_codegen::ir::condcodes::IntCC {
    use cranelift_codegen::ir::condcodes::IntCC;
    match comparison {
        NumericComparison::Equal => IntCC::Equal,
        NumericComparison::NotEqual => IntCC::NotEqual,
        NumericComparison::LessThan => IntCC::SignedLessThan,
        NumericComparison::LessEqual => IntCC::SignedLessThanOrEqual,
        NumericComparison::GreaterThan => IntCC::SignedGreaterThan,
        NumericComparison::GreaterEqual => IntCC::SignedGreaterThanOrEqual,
    }
}

const fn float_condition(
    comparison: NumericComparison,
) -> cranelift_codegen::ir::condcodes::FloatCC {
    use cranelift_codegen::ir::condcodes::FloatCC;
    match comparison {
        NumericComparison::Equal => FloatCC::Equal,
        NumericComparison::NotEqual => FloatCC::NotEqual,
        NumericComparison::LessThan => FloatCC::LessThan,
        NumericComparison::LessEqual => FloatCC::LessThanOrEqual,
        NumericComparison::GreaterThan => FloatCC::GreaterThan,
        NumericComparison::GreaterEqual => FloatCC::GreaterThanOrEqual,
    }
}

fn slot_offset(index: usize) -> Result<i32, NativeError> {
    i32::try_from(index)
        .ok()
        .and_then(|index| index.checked_mul(SLOT_SIZE))
        .and_then(|offset| offset.checked_add(SLOT_PAYLOAD_OFFSET))
        .ok_or(NativeError::CountOverflow)
}

fn block(
    blocks: &BTreeMap<BlockId, cranelift_codegen::ir::Block>,
    id: BlockId,
) -> Result<cranelift_codegen::ir::Block, NativeError> {
    blocks
        .get(&id)
        .copied()
        .ok_or(NativeError::Unsupported("missing block"))
}

fn value(
    values: &BTreeMap<ValueId, cranelift_codegen::ir::Value>,
    id: ValueId,
) -> Result<cranelift_codegen::ir::Value, NativeError> {
    values
        .get(&id)
        .copied()
        .ok_or(NativeError::Unsupported("missing value"))
}

fn two_inputs(
    values: &BTreeMap<ValueId, cranelift_codegen::ir::Value>,
    inputs: &[ValueId],
) -> Result<[cranelift_codegen::ir::Value; 2], NativeError> {
    match inputs {
        [left, right] => Ok([value(values, *left)?, value(values, *right)?]),
        _ => Err(NativeError::Unsupported("binary arity")),
    }
}

fn block_args(
    values: &BTreeMap<ValueId, cranelift_codegen::ir::Value>,
    ids: &[ValueId],
) -> Result<Vec<BlockArg>, NativeError> {
    ids.iter()
        .map(|id| value(values, *id).map(BlockArg::from))
        .collect()
}

struct HelperFunctions {
    object_get: FuncRef,
    object_set: FuncRef,
    list_get: FuncRef,
    list_set: FuncRef,
    list_append: FuncRef,
    direct_call: FuncRef,
    float_power: FuncRef,
}

impl HelperFunctions {
    fn declare(
        module: &mut JITModule,
        function: &mut cranelift_codegen::ir::Function,
    ) -> Result<Self, NativeError> {
        Ok(Self {
            object_get: declare_helper(module, function, "wustite_v2_object_get", 2)?,
            object_set: declare_helper(module, function, "wustite_v2_object_set", 3)?,
            list_get: declare_helper(module, function, "wustite_v2_list_get", 2)?,
            list_set: declare_helper(module, function, "wustite_v2_list_set", 3)?,
            list_append: declare_helper(module, function, "wustite_v2_list_append", 2)?,
            direct_call: declare_helper(module, function, "wustite_v2_direct_call", 3)?,
            float_power: declare_float_power(module, function)?,
        })
    }
}

fn declare_float_power(
    module: &mut JITModule,
    function: &mut cranelift_codegen::ir::Function,
) -> Result<FuncRef, NativeError> {
    let mut signature = module.make_signature();
    signature.params.extend([AbiParam::new(types::F64); 2]);
    signature.returns.push(AbiParam::new(types::F64));
    let id = module
        .declare_function("wustite_v2_float_power", Linkage::Import, &signature)
        .map_err(|error| NativeError::Backend(error.to_string()))?;
    Ok(module.declare_func_in_func(id, function))
}

fn declare_helper(
    module: &mut JITModule,
    function: &mut cranelift_codegen::ir::Function,
    name: &str,
    argument_count: usize,
) -> Result<FuncRef, NativeError> {
    let mut signature = module.make_signature();
    signature
        .params
        .push(AbiParam::new(module.target_config().pointer_type()));
    signature
        .params
        .extend((0..argument_count).map(|_| AbiParam::new(types::I64)));
    signature.returns.push(AbiParam::new(types::I64));
    let id = module
        .declare_function(name, Linkage::Import, &signature)
        .map_err(|error| NativeError::Backend(error.to_string()))?;
    Ok(module.declare_func_in_func(id, function))
}

fn register_helpers(builder: &mut JITBuilder) {
    builder.symbol(
        "wustite_v2_object_get",
        super::helpers::object_get as *const u8,
    );
    builder.symbol(
        "wustite_v2_object_set",
        super::helpers::object_set as *const u8,
    );
    builder.symbol("wustite_v2_list_get", super::helpers::list_get as *const u8);
    builder.symbol("wustite_v2_list_set", super::helpers::list_set as *const u8);
    builder.symbol(
        "wustite_v2_list_append",
        super::helpers::list_append as *const u8,
    );
    builder.symbol(
        "wustite_v2_direct_call",
        super::helpers::direct_call as *const u8,
    );
    builder.symbol("wustite_v2_float_power", float_power as *const u8);
}

extern "C" fn float_power(left: f64, right: f64) -> f64 {
    left.powf(right)
}

fn lower_helper(
    builder: &mut FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    context: cranelift_codegen::ir::Value,
    helper: FuncRef,
    arguments: &[cranelift_codegen::ir::Value],
) -> Result<cranelift_codegen::ir::Value, NativeError> {
    let flags = MemFlagsData::new();
    let calls = builder
        .ins()
        .load(types::I64, flags, frame, HELPER_CALLS_OFFSET);
    let calls = builder.ins().iadd_imm_u(calls, 1);
    builder
        .ins()
        .store(flags, calls, frame, HELPER_CALLS_OFFSET);
    let mut call_arguments = Vec::with_capacity(arguments.len() + 1);
    call_arguments.push(context);
    call_arguments.extend_from_slice(arguments);
    let call = builder.ins().call(helper, &call_arguments);
    builder
        .inst_results(call)
        .first()
        .copied()
        .ok_or(NativeError::Backend("helper result".to_string()))
}

fn helper_values(
    values: &BTreeMap<ValueId, cranelift_codegen::ir::Value>,
    ids: &[ValueId],
) -> Result<Vec<cranelift_codegen::ir::Value>, NativeError> {
    ids.iter().map(|id| value(values, *id)).collect()
}
