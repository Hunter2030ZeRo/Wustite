use std::collections::BTreeMap;

use inkwell::AddressSpace;
use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{CodeModel, InitializationConfig, RelocMode, Target, TargetMachine};
use inkwell::values::{BasicValueEnum, IntValue, PointerValue};

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
use super::{NativeCode, NativeError, NativeOwner};
use crate::adaptive_v2::wxir_v2::VerifiedSnapshot;
use crate::adaptive_v2::wxir_v2::ir::{
    Constant, InstructionKind, NumericComparison, Terminator, ValueId, ValueType,
};

mod scalar;

pub(super) fn compile(
    snapshot: &VerifiedSnapshot,
    symbol: &str,
) -> Result<NativeCode, NativeError> {
    let context = Box::new(Context::create());
    let context_pointer = &*context as *const Context;
    // SAFETY: [Categories 1 and 3] the boxed context never moves and is stored
    // beside the JitFunction, whose field is dropped before the context field.
    let context_reference = unsafe { &*context_pointer };
    let entry = compile_function(context_reference, snapshot, symbol)?;
    let entry_block = snapshot
        .body()
        .blocks
        .iter()
        .find(|block| block.id == snapshot.body().entry)
        .ok_or(NativeError::Unsupported("missing entry"))?;
    let output_types = output_types(snapshot)?;
    Ok(NativeCode {
        snapshot_id: snapshot.id(),
        input_types: entry_block
            .parameters
            .iter()
            .map(|parameter| parameter.ty)
            .collect(),
        output_types,
        direct_storage: super::direct_storage::verify(snapshot),
        _owner: NativeOwner::Llvm {
            _entry: entry,
            _context: context,
        },
    })
}

fn compile_function(
    context: &'static Context,
    snapshot: &VerifiedSnapshot,
    symbol: &str,
) -> Result<inkwell::execution_engine::JitFunction<'static, super::entry::NativeEntry>, NativeError>
{
    let direct_storage = super::direct_storage::verify(snapshot);
    if snapshot.body().blocks.iter().any(|block| {
        block
            .parameters
            .iter()
            .chain(
                block
                    .instructions
                    .iter()
                    .filter_map(|instruction| instruction.output.as_ref()),
            )
            .any(|value| value.ty == ValueType::F64)
    }) {
        return scalar::compile(context, snapshot, symbol);
    }
    if snapshot.body().blocks.len() != 1
        || !matches!(
            snapshot.body().blocks[0].terminator,
            Terminator::Return { .. } | Terminator::SideExit { .. }
        )
    {
        return compile_cfg_function(context, snapshot, symbol);
    }
    let module = context.create_module(symbol);
    let builder = context.create_builder();
    let pointer_type = context.ptr_type(AddressSpace::default());
    let function_type = context.i32_type().fn_type(&[pointer_type.into()], false);
    let function = module.add_function(symbol, function_type, None);
    let helpers = HelperFunctions::declare(context, &module);
    let block = context.append_basic_block(function, "entry");
    builder.position_at_end(block);
    let frame = function
        .get_first_param()
        .and_then(|value| match value {
            BasicValueEnum::PointerValue(pointer) => Some(pointer),
            _ => None,
        })
        .ok_or(NativeError::Backend("missing native frame".to_string()))?;
    let entries_pointer = byte_pointer(
        context,
        &builder,
        frame,
        MACHINE_ENTRIES_OFFSET,
        "entries_ptr",
    )?;
    let entries = builder
        .build_load(context.i64_type(), entries_pointer, "entries")
        .map_err(llvm_error)?
        .into_int_value();
    let entries = builder
        .build_int_add(
            entries,
            context.i64_type().const_int(1, false),
            "next_entries",
        )
        .map_err(llvm_error)?;
    builder
        .build_store(entries_pointer, entries)
        .map_err(llvm_error)?;
    let inputs_pointer = load_pointer(context, &builder, frame, INPUTS_OFFSET, "inputs")?;
    let helper_context = load_pointer(
        context,
        &builder,
        frame,
        HELPER_CONTEXT_OFFSET,
        "helper_context",
    )?;
    let entry = snapshot
        .body()
        .blocks
        .iter()
        .find(|block| block.id == snapshot.body().entry)
        .ok_or(NativeError::Unsupported("missing entry"))?;
    if snapshot.body().blocks.len() != 1 {
        return Err(NativeError::Unsupported("LLVM CFG"));
    }
    let mut values = BTreeMap::new();
    let mut value_types = BTreeMap::new();
    for (index, parameter) in entry.parameters.iter().enumerate() {
        native_type(parameter.ty)?;
        let pointer = byte_pointer(
            context,
            &builder,
            inputs_pointer,
            slot_offset(index)?,
            "input",
        )?;
        let value = builder
            .build_load(context.i64_type(), pointer, "input_value")
            .map_err(llvm_error)?
            .into_int_value();
        values.insert(parameter.id, value);
        value_types.insert(parameter.id, parameter.ty);
    }
    for instruction in &entry.instructions {
        let result = match instruction.kind.semantic() {
            InstructionKind::Constant(constant) => Some(lower_constant(context, constant)?),
            InstructionKind::Copy => Some(value(&values, instruction.inputs[0])?),
            InstructionKind::IntegerAdd => {
                let [left, right] = two_inputs(&values, &instruction.inputs)?;
                Some(
                    builder
                        .build_int_add(left, right, "add")
                        .map_err(llvm_error)?,
                )
            }
            InstructionKind::IntegerSubtract => {
                let [left, right] = two_inputs(&values, &instruction.inputs)?;
                Some(
                    builder
                        .build_int_sub(left, right, "subtract")
                        .map_err(llvm_error)?,
                )
            }
            InstructionKind::IntegerMultiply => {
                let [left, right] = two_inputs(&values, &instruction.inputs)?;
                Some(
                    builder
                        .build_int_mul(left, right, "multiply")
                        .map_err(llvm_error)?,
                )
            }
            InstructionKind::IntegerLessThan => {
                let [left, right] = two_inputs(&values, &instruction.inputs)?;
                Some(
                    builder
                        .build_int_compare(inkwell::IntPredicate::SLT, left, right, "less_than")
                        .map_err(llvm_error)?,
                )
            }
            InstructionKind::IntegerCompare { comparison } => {
                let [left, right] = two_inputs(&values, &instruction.inputs)?;
                Some(
                    builder
                        .build_int_compare(int_predicate(*comparison), left, right, "compare")
                        .map_err(llvm_error)?,
                )
            }
            InstructionKind::IntegerNegate => Some(
                builder
                    .build_int_neg(value(&values, instruction.inputs[0])?, "negate")
                    .map_err(llvm_error)?,
            ),
            InstructionKind::BooleanNot => Some(
                builder
                    .build_xor(
                        value(&values, instruction.inputs[0])?,
                        context.i64_type().const_int(1, false),
                        "not",
                    )
                    .map_err(llvm_error)?,
            ),
            InstructionKind::BooleanAnd | InstructionKind::BooleanOr => {
                let [left, right] = two_inputs(&values, &instruction.inputs)?;
                Some(match instruction.kind.semantic() {
                    InstructionKind::BooleanAnd => {
                        builder.build_and(left, right, "and").map_err(llvm_error)?
                    }
                    InstructionKind::BooleanOr => {
                        builder.build_or(left, right, "or").map_err(llvm_error)?
                    }
                    _ => return Err(NativeError::Unsupported("LLVM boolean operation")),
                })
            }
            InstructionKind::Select => {
                let condition = value(&values, instruction.inputs[0])?;
                let yes = value(&values, instruction.inputs[1])?;
                let no = value(&values, instruction.inputs[2])?;
                Some(
                    builder
                        .build_select(
                            builder
                                .build_int_compare(
                                    inkwell::IntPredicate::NE,
                                    condition,
                                    context.i64_type().const_zero(),
                                    "select_condition",
                                )
                                .map_err(llvm_error)?,
                            yes,
                            no,
                            "select",
                        )
                        .map_err(llvm_error)?
                        .into_int_value(),
                )
            }
            InstructionKind::OwnedList {
                identity,
                reset_on_definition,
                copy_from_source,
                ..
            } => {
                let output = instruction.output.ok_or(NativeError::Unsupported(
                    "LLVM owned list definition has no output",
                ))?;
                let (storage_index, storage) = direct_storage
                    .as_ref()
                    .and_then(|plan| plan.storage_for(output.id))
                    .ok_or(NativeError::Unsupported("LLVM owned list direct storage"))?;
                let alias = context
                    .i64_type()
                    .const_int(super::direct_storage::owned_alias(*identity), false);
                if *copy_from_source {
                    lower_direct_list_copy(
                        context,
                        &builder,
                        function,
                        frame,
                        storage_index,
                        alias,
                        storage.copy_from.ok_or(NativeError::Unsupported(
                            "LLVM owned list copy source storage",
                        ))?,
                        value(&values, instruction.inputs[1])?,
                    )?;
                } else if *reset_on_definition {
                    lower_direct_list_clear(
                        context,
                        &builder,
                        function,
                        frame,
                        storage_index,
                        alias,
                    )?;
                }
                Some(alias)
            }
            InstructionKind::Guard { guard } => {
                let condition = value(&values, instruction.inputs[0])?;
                let recipe = snapshot
                    .body()
                    .deopts
                    .iter()
                    .find(|recipe| recipe.id == *guard)
                    .ok_or(NativeError::Unsupported("missing LLVM guard deopt"))?;
                lower_guard(
                    context,
                    &builder,
                    function,
                    frame,
                    condition,
                    *guard,
                    recipe.root_point.get(),
                )?;
                None
            }
            InstructionKind::ObjectGet
                if direct_storage.as_ref().is_some_and(|plan| {
                    plan.storage_for(instruction.inputs[0])
                        .is_some_and(|(_, storage)| {
                            instruction
                                .output
                                .is_some_and(|output| Some(output.id) == storage.output)
                        })
                }) =>
            {
                let (storage_index, _) = direct_storage
                    .as_ref()
                    .and_then(|plan| plan.storage_for(instruction.inputs[0]))
                    .ok_or(NativeError::Unsupported("LLVM object direct storage"))?;
                Some(lower_direct_list_get(
                    context,
                    &builder,
                    function,
                    frame,
                    storage_index,
                    value(&values, instruction.inputs[0])?,
                    value(&values, instruction.inputs[1])?,
                )?)
            }
            InstructionKind::ObjectGet => Some(lower_helper(
                context,
                &builder,
                frame,
                helper_context,
                helpers.object_get,
                &helper_values(&values, &instruction.inputs)?,
            )?),
            InstructionKind::ObjectSet => {
                let _ = lower_helper(
                    context,
                    &builder,
                    frame,
                    helper_context,
                    helpers.object_set,
                    &helper_values(&values, &instruction.inputs)?,
                )?;
                None
            }
            InstructionKind::ListGet
                if direct_storage.as_ref().is_some_and(|plan| {
                    plan.is_dynamic(instruction.inputs[0])
                        || plan
                            .storage_for(instruction.inputs[0])
                            .is_some_and(|(_, storage)| {
                                storage.kind == super::direct_storage::DirectStorageKind::List
                            })
                }) =>
            {
                let handle = instruction.inputs[0];
                let alias = value(&values, handle)?;
                let index = value(&values, instruction.inputs[1])?;
                Some(
                    if let Some((storage_index, _)) = direct_storage
                        .as_ref()
                        .and_then(|plan| plan.storage_for(handle))
                    {
                        lower_direct_list_get(
                            context,
                            &builder,
                            function,
                            frame,
                            storage_index,
                            alias,
                            index,
                        )?
                    } else {
                        lower_dynamic_list_get(context, &builder, function, frame, alias, index)?
                    },
                )
            }
            InstructionKind::ListGet => Some(lower_helper(
                context,
                &builder,
                frame,
                helper_context,
                helpers.list_get,
                &helper_values(&values, &instruction.inputs)?,
            )?),
            InstructionKind::ListLength => {
                let (storage_index, _) = direct_storage
                    .as_ref()
                    .and_then(|plan| plan.storage_for(instruction.inputs[0]))
                    .ok_or(NativeError::Unsupported("LLVM direct list length"))?;
                Some(lower_direct_list_length(
                    context,
                    &builder,
                    function,
                    frame,
                    storage_index,
                    value(&values, instruction.inputs[0])?,
                )?)
            }
            InstructionKind::ListSet => {
                if direct_storage
                    .as_ref()
                    .is_some_and(|plan| plan.is_dynamic(instruction.inputs[0]))
                {
                    lower_dynamic_list_set(
                        context,
                        &builder,
                        function,
                        frame,
                        value(&values, instruction.inputs[0])?,
                        value(&values, instruction.inputs[1])?,
                        value(&values, instruction.inputs[2])?,
                    )?;
                } else {
                    let _ = lower_helper(
                        context,
                        &builder,
                        frame,
                        helper_context,
                        helpers.list_set,
                        &helper_values(&values, &instruction.inputs)?,
                    )?;
                }
                None
            }
            InstructionKind::ListReversePrefix { element_type } => {
                if *element_type != ValueType::I64 {
                    return Err(NativeError::Unsupported(
                        "LLVM direct list reverse element type",
                    ));
                }
                let handle = instruction.inputs[0];
                let plan = direct_storage
                    .as_ref()
                    .ok_or(NativeError::Unsupported("LLVM direct list reverse"))?;
                let alias = value(&values, handle)?;
                let descriptor = if let Some((storage_index, storage)) = plan.storage_for(handle) {
                    if storage.kind != super::direct_storage::DirectStorageKind::List {
                        return Err(NativeError::Unsupported("LLVM direct list reverse storage"));
                    }
                    resolve_fixed_direct_storage(
                        context,
                        &builder,
                        function,
                        frame,
                        storage_index,
                        storage.source,
                        alias,
                    )?
                } else if plan.is_dynamic(handle) {
                    resolve_dynamic_direct_storage(context, &builder, function, frame, alias)?
                } else {
                    return Err(NativeError::Unsupported("LLVM direct list reverse"));
                };
                lower_list_reverse_prefix(
                    context,
                    &builder,
                    function,
                    descriptor,
                    value(&values, instruction.inputs[1])?,
                )?;
                None
            }
            InstructionKind::ListClear => {
                let (storage_index, _) = direct_storage
                    .as_ref()
                    .and_then(|plan| plan.storage_for(instruction.inputs[0]))
                    .ok_or(NativeError::Unsupported("LLVM direct list clear"))?;
                let alias = value(&values, instruction.inputs[0])?;
                lower_direct_list_clear(context, &builder, function, frame, storage_index, alias)?;
                Some(alias)
            }
            InstructionKind::ListAppend => {
                if let Some((storage_index, storage)) = direct_storage
                    .as_ref()
                    .and_then(|plan| plan.storage_for(instruction.inputs[0]))
                    .filter(|(_, storage)| {
                        storage.kind == super::direct_storage::DirectStorageKind::List
                    })
                {
                    let _ = storage;
                    let alias = value(&values, instruction.inputs[0])?;
                    lower_direct_list_append(
                        context,
                        &builder,
                        function,
                        frame,
                        storage_index,
                        alias,
                        value(&values, instruction.inputs[1])?,
                    )?;
                    Some(alias)
                } else {
                    let _ = lower_helper(
                        context,
                        &builder,
                        frame,
                        helper_context,
                        helpers.list_append,
                        &helper_values(&values, &instruction.inputs)?,
                    )?;
                    None
                }
            }
            InstructionKind::Call { callee } => {
                let mut arguments = vec![context.i64_type().const_int(*callee, false)];
                arguments.extend(helper_values(&values, &instruction.inputs)?);
                Some(lower_helper(
                    context,
                    &builder,
                    frame,
                    helper_context,
                    helpers.direct_call,
                    &arguments,
                )?)
            }
            _ => return Err(NativeError::Unsupported("LLVM instruction")),
        };
        if let (Some(output), Some(result)) = (instruction.output, result) {
            values.insert(output.id, result);
            value_types.insert(output.id, output.ty);
        }
    }
    let returned = match &entry.terminator {
        Terminator::Return { values } | Terminator::SideExit { values, .. } => values,
        _ => return Err(NativeError::Unsupported("LLVM terminator")),
    };
    let outputs_pointer = load_pointer(context, &builder, frame, OUTPUTS_OFFSET, "outputs")?;
    for (index, id) in returned.iter().enumerate() {
        let payload = value(&values, *id)?;
        let tag_pointer = byte_pointer(
            context,
            &builder,
            outputs_pointer,
            slot_offset(index)? - SLOT_PAYLOAD_OFFSET,
            "tag",
        )?;
        let tag = value_tag(
            *value_types
                .get(id)
                .ok_or(NativeError::Unsupported("missing LLVM output type"))?,
        );
        builder
            .build_store(
                tag_pointer,
                context.i32_type().const_int(u64::from(tag), false),
            )
            .map_err(llvm_error)?;
        let payload_pointer = byte_pointer(
            context,
            &builder,
            outputs_pointer,
            slot_offset(index)?,
            "payload",
        )?;
        builder
            .build_store(payload_pointer, payload)
            .map_err(llvm_error)?;
    }
    let raw_exit = match &entry.terminator {
        Terminator::Return { .. } => context.i32_type().const_zero(),
        Terminator::SideExit { id, .. } => {
            store_i32(context, &builder, frame, EXIT_KIND_OFFSET, 1, "exit_kind")?;
            store_i32(context, &builder, frame, EXIT_ID_OFFSET, *id, "exit_id")?;
            store_i32(context, &builder, frame, DEOPT_ID_OFFSET, *id, "deopt_id")?;
            let recipe = snapshot
                .body()
                .deopts
                .iter()
                .find(|recipe| recipe.id == *id)
                .ok_or(NativeError::Unsupported("missing LLVM side-exit deopt"))?;
            store_i32(
                context,
                &builder,
                frame,
                SAFEPOINT_ID_OFFSET,
                recipe.root_point.get(),
                "safepoint",
            )?;
            context.i32_type().const_int(1, false)
        }
        _ => return Err(NativeError::Unsupported("LLVM terminator")),
    };
    builder.build_return(Some(&raw_exit)).map_err(llvm_error)?;
    module.verify().map_err(llvm_error)?;
    let target_machine = native_target_machine()?;
    module.set_triple(&target_machine.get_triple());
    module.set_data_layout(&target_machine.get_target_data().get_data_layout());
    let options = PassBuilderOptions::create();
    options.set_verify_each(true);
    module
        .run_passes("default<O3>", &target_machine, options)
        .map_err(llvm_error)?;
    module.verify().map_err(llvm_error)?;
    let engine = module
        .create_jit_execution_engine(OptimizationLevel::Aggressive)
        .map_err(llvm_error)?;
    map_helpers(&module, &engine);
    // SAFETY: [Categories 3, 5, 8, and 14] the module declares `symbol` with
    // the exact NativeEntry ABI and JitFunction retains the execution engine.
    unsafe { engine.get_function(symbol).map_err(llvm_error) }
}

fn compile_cfg_function(
    context: &'static Context,
    snapshot: &VerifiedSnapshot,
    symbol: &str,
) -> Result<inkwell::execution_engine::JitFunction<'static, super::entry::NativeEntry>, NativeError>
{
    let module = context.create_module(symbol);
    let builder = context.create_builder();
    let pointer_type = context.ptr_type(AddressSpace::default());
    let function_type = context.i32_type().fn_type(&[pointer_type.into()], false);
    let function = module.add_function(symbol, function_type, None);
    let helpers = HelperFunctions::declare(context, &module);
    let prologue = context.append_basic_block(function, "prologue");
    let blocks = snapshot
        .body()
        .blocks
        .iter()
        .map(|block| {
            (
                block.id,
                context.append_basic_block(function, &format!("block_{}", block.id.get())),
            )
        })
        .collect::<BTreeMap<_, _>>();
    builder.position_at_end(prologue);
    let frame = function
        .get_first_param()
        .and_then(|value| match value {
            BasicValueEnum::PointerValue(pointer) => Some(pointer),
            _ => None,
        })
        .ok_or_else(|| NativeError::Backend("missing native frame".into()))?;
    let entries_pointer =
        byte_pointer(context, &builder, frame, MACHINE_ENTRIES_OFFSET, "entries")?;
    let entries = builder
        .build_load(context.i64_type(), entries_pointer, "entry_count")
        .map_err(llvm_error)?
        .into_int_value();
    let entries = builder
        .build_int_add(
            entries,
            context.i64_type().const_int(1, false),
            "next_entry",
        )
        .map_err(llvm_error)?;
    builder
        .build_store(entries_pointer, entries)
        .map_err(llvm_error)?;
    let inputs = load_pointer(context, &builder, frame, INPUTS_OFFSET, "inputs")?;
    let helper_context = load_pointer(
        context,
        &builder,
        frame,
        HELPER_CONTEXT_OFFSET,
        "helper_context",
    )?;
    let entry = snapshot
        .body()
        .blocks
        .iter()
        .find(|block| block.id == snapshot.body().entry)
        .ok_or(NativeError::Unsupported("missing entry"))?;
    let mut values = BTreeMap::new();
    let mut value_types = BTreeMap::new();
    for (index, parameter) in entry.parameters.iter().enumerate() {
        native_type(parameter.ty)?;
        let pointer = byte_pointer(context, &builder, inputs, slot_offset(index)?, "input")?;
        let loaded = builder
            .build_load(context.i64_type(), pointer, "input_value")
            .map_err(llvm_error)?
            .into_int_value();
        values.insert(parameter.id, loaded);
        value_types.insert(parameter.id, parameter.ty);
    }
    let mut phis = BTreeMap::new();
    for block in &snapshot.body().blocks {
        if block.id == snapshot.body().entry {
            continue;
        }
        builder.position_at_end(blocks[&block.id]);
        for parameter in &block.parameters {
            native_type(parameter.ty)?;
            let phi = builder
                .build_phi(context.i64_type(), "block_parameter")
                .map_err(llvm_error)?;
            values.insert(parameter.id, phi.as_basic_value().into_int_value());
            value_types.insert(parameter.id, parameter.ty);
            phis.insert(parameter.id, phi);
        }
    }
    builder.position_at_end(prologue);
    builder
        .build_unconditional_branch(blocks[&snapshot.body().entry])
        .map_err(llvm_error)?;

    for source in &snapshot.body().blocks {
        builder.position_at_end(blocks[&source.id]);
        for instruction in &source.instructions {
            let result = lower_cfg_instruction(
                (
                    context,
                    &builder,
                    function,
                    frame,
                    helper_context,
                    &helpers,
                    snapshot,
                ),
                instruction,
                &values,
            )?;
            if let (Some(output), Some(result)) = (instruction.output, result) {
                values.insert(output.id, result);
                value_types.insert(output.id, output.ty);
            }
        }
        let current = builder
            .get_insert_block()
            .ok_or_else(|| NativeError::Backend("missing LLVM block".into()))?;
        match &source.terminator {
            Terminator::Jump { target, arguments } => {
                let target_block = snapshot
                    .body()
                    .blocks
                    .iter()
                    .find(|block| block.id == *target)
                    .ok_or(NativeError::Unsupported("missing LLVM jump target"))?;
                for (parameter, argument) in target_block.parameters.iter().zip(arguments) {
                    phis.get(&parameter.id)
                        .ok_or(NativeError::Unsupported("missing LLVM phi"))?
                        .add_incoming(&[(&value(&values, *argument)?, current)]);
                }
                builder
                    .build_unconditional_branch(blocks[target])
                    .map_err(llvm_error)?;
            }
            Terminator::Branch { condition, yes, no } => {
                let condition = builder
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        value(&values, *condition)?,
                        context.i64_type().const_zero(),
                        "branch_condition",
                    )
                    .map_err(llvm_error)?;
                builder
                    .build_conditional_branch(condition, blocks[yes], blocks[no])
                    .map_err(llvm_error)?;
            }
            Terminator::Return { values: returned } => {
                store_cfg_outputs(context, &builder, frame, returned, &values, &value_types)?;
                builder
                    .build_return(Some(&context.i32_type().const_zero()))
                    .map_err(llvm_error)?;
            }
            Terminator::SideExit { id, values: exited } => {
                store_cfg_outputs(context, &builder, frame, exited, &values, &value_types)?;
                store_i32(context, &builder, frame, EXIT_KIND_OFFSET, 1, "exit_kind")?;
                store_i32(context, &builder, frame, EXIT_ID_OFFSET, *id, "exit_id")?;
                store_i32(context, &builder, frame, DEOPT_ID_OFFSET, *id, "deopt_id")?;
                let recipe = snapshot
                    .body()
                    .deopts
                    .iter()
                    .find(|recipe| recipe.id == *id)
                    .ok_or(NativeError::Unsupported("missing LLVM side-exit deopt"))?;
                store_i32(
                    context,
                    &builder,
                    frame,
                    SAFEPOINT_ID_OFFSET,
                    recipe.root_point.get(),
                    "safepoint",
                )?;
                builder
                    .build_return(Some(&context.i32_type().const_int(1, false)))
                    .map_err(llvm_error)?;
            }
            Terminator::Backedge {
                target_pc,
                safepoint,
            } => {
                let recipe = snapshot
                    .body()
                    .deopts
                    .iter()
                    .find(|recipe| recipe.root_point == *safepoint)
                    .ok_or(NativeError::Unsupported("missing LLVM backedge deopt"))?;
                store_i32(
                    context,
                    &builder,
                    frame,
                    EXIT_KIND_OFFSET,
                    1,
                    "backedge_kind",
                )?;
                store_i32(
                    context,
                    &builder,
                    frame,
                    EXIT_ID_OFFSET,
                    *target_pc,
                    "backedge_target",
                )?;
                store_i32(
                    context,
                    &builder,
                    frame,
                    SAFEPOINT_ID_OFFSET,
                    safepoint.get(),
                    "backedge_safepoint",
                )?;
                store_i32(
                    context,
                    &builder,
                    frame,
                    DEOPT_ID_OFFSET,
                    recipe.id,
                    "backedge_deopt",
                )?;
                builder
                    .build_return(Some(&context.i32_type().const_int(1, false)))
                    .map_err(llvm_error)?;
            }
            Terminator::IrreducibleBackedge => {
                return Err(NativeError::Unsupported("LLVM irreducible backedge"));
            }
        }
    }
    finalize_cfg_module(context, &module, symbol)
}

fn lower_cfg_instruction<'ctx>(
    state: (
        &'ctx Context,
        &inkwell::builder::Builder<'ctx>,
        inkwell::values::FunctionValue<'ctx>,
        PointerValue<'ctx>,
        PointerValue<'ctx>,
        &HelperFunctions<'ctx>,
        &VerifiedSnapshot,
    ),
    instruction: &crate::adaptive_v2::wxir_v2::ir::Instruction,
    values: &BTreeMap<ValueId, IntValue<'ctx>>,
) -> Result<Option<IntValue<'ctx>>, NativeError> {
    let (context, builder, function, frame, helper_context, helpers, snapshot) = state;
    Ok(match instruction.kind.semantic() {
        InstructionKind::Constant(constant) => Some(lower_constant(context, constant)?),
        InstructionKind::Copy => Some(value(values, instruction.inputs[0])?),
        InstructionKind::IntegerAdd => {
            let [left, right] = two_inputs(values, &instruction.inputs)?;
            Some(
                builder
                    .build_int_add(left, right, "add")
                    .map_err(llvm_error)?,
            )
        }
        InstructionKind::IntegerSubtract => {
            let [left, right] = two_inputs(values, &instruction.inputs)?;
            Some(
                builder
                    .build_int_sub(left, right, "subtract")
                    .map_err(llvm_error)?,
            )
        }
        InstructionKind::IntegerMultiply => {
            let [left, right] = two_inputs(values, &instruction.inputs)?;
            Some(
                builder
                    .build_int_mul(left, right, "multiply")
                    .map_err(llvm_error)?,
            )
        }
        InstructionKind::IntegerLessThan => {
            let [left, right] = two_inputs(values, &instruction.inputs)?;
            Some(
                builder
                    .build_int_compare(inkwell::IntPredicate::SLT, left, right, "less_than")
                    .map_err(llvm_error)?,
            )
        }
        InstructionKind::IntegerCompare { comparison } => {
            let [left, right] = two_inputs(values, &instruction.inputs)?;
            Some(
                builder
                    .build_int_compare(int_predicate(*comparison), left, right, "compare")
                    .map_err(llvm_error)?,
            )
        }
        InstructionKind::IntegerNegate => Some(
            builder
                .build_int_neg(value(values, instruction.inputs[0])?, "negate")
                .map_err(llvm_error)?,
        ),
        InstructionKind::BooleanNot => Some(
            builder
                .build_xor(
                    value(values, instruction.inputs[0])?,
                    context.i64_type().const_int(1, false),
                    "not",
                )
                .map_err(llvm_error)?,
        ),
        InstructionKind::BooleanAnd | InstructionKind::BooleanOr => {
            let [left, right] = two_inputs(values, &instruction.inputs)?;
            Some(match instruction.kind.semantic() {
                InstructionKind::BooleanAnd => {
                    builder.build_and(left, right, "and").map_err(llvm_error)?
                }
                InstructionKind::BooleanOr => {
                    builder.build_or(left, right, "or").map_err(llvm_error)?
                }
                _ => return Err(NativeError::Unsupported("LLVM CFG boolean operation")),
            })
        }
        InstructionKind::Guard { guard } => {
            let recipe = snapshot
                .body()
                .deopts
                .iter()
                .find(|recipe| recipe.id == *guard)
                .ok_or(NativeError::Unsupported("missing LLVM guard deopt"))?;
            lower_guard(
                context,
                builder,
                function,
                frame,
                value(values, instruction.inputs[0])?,
                *guard,
                recipe.root_point.get(),
            )?;
            None
        }
        InstructionKind::ObjectGet => Some(lower_helper(
            context,
            builder,
            frame,
            helper_context,
            helpers.object_get,
            &helper_values(values, &instruction.inputs)?,
        )?),
        InstructionKind::ObjectSet => {
            let _ = lower_helper(
                context,
                builder,
                frame,
                helper_context,
                helpers.object_set,
                &helper_values(values, &instruction.inputs)?,
            )?;
            None
        }
        InstructionKind::ListGet => Some(lower_helper(
            context,
            builder,
            frame,
            helper_context,
            helpers.list_get,
            &helper_values(values, &instruction.inputs)?,
        )?),
        InstructionKind::ListSet => {
            let _ = lower_helper(
                context,
                builder,
                frame,
                helper_context,
                helpers.list_set,
                &helper_values(values, &instruction.inputs)?,
            )?;
            None
        }
        InstructionKind::ListReversePrefix { element_type } => {
            if *element_type != ValueType::I64 {
                return Err(NativeError::Unsupported(
                    "LLVM CFG direct list reverse element type",
                ));
            }
            let handle = instruction.inputs[0];
            let plan = super::direct_storage::verify(snapshot)
                .ok_or(NativeError::Unsupported("LLVM CFG direct list reverse"))?;
            let alias = value(values, handle)?;
            let descriptor = if let Some((storage_index, storage)) = plan.storage_for(handle) {
                if storage.kind != super::direct_storage::DirectStorageKind::List {
                    return Err(NativeError::Unsupported(
                        "LLVM CFG direct list reverse storage",
                    ));
                }
                resolve_fixed_direct_storage(
                    context,
                    builder,
                    function,
                    frame,
                    storage_index,
                    storage.source,
                    alias,
                )?
            } else if plan.is_dynamic(handle) {
                resolve_dynamic_direct_storage(context, builder, function, frame, alias)?
            } else {
                return Err(NativeError::Unsupported("LLVM CFG direct list reverse"));
            };
            lower_list_reverse_prefix(
                context,
                builder,
                function,
                descriptor,
                value(values, instruction.inputs[1])?,
            )?;
            None
        }
        InstructionKind::ListClear => {
            return Err(NativeError::Unsupported("scalar LLVM list clear"));
        }
        InstructionKind::ListAppend => {
            let _ = lower_helper(
                context,
                builder,
                frame,
                helper_context,
                helpers.list_append,
                &helper_values(values, &instruction.inputs)?,
            )?;
            None
        }
        InstructionKind::Call { callee } => {
            let mut arguments = vec![context.i64_type().const_int(*callee, false)];
            arguments.extend(helper_values(values, &instruction.inputs)?);
            Some(lower_helper(
                context,
                builder,
                frame,
                helper_context,
                helpers.direct_call,
                &arguments,
            )?)
        }
        _ => return Err(NativeError::Unsupported("LLVM CFG instruction")),
    })
}

fn store_cfg_outputs<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    frame: PointerValue<'ctx>,
    returned: &[ValueId],
    values: &BTreeMap<ValueId, IntValue<'ctx>>,
    value_types: &BTreeMap<ValueId, ValueType>,
) -> Result<(), NativeError> {
    let outputs = load_pointer(context, builder, frame, OUTPUTS_OFFSET, "outputs")?;
    for (index, id) in returned.iter().enumerate() {
        let offset = slot_offset(index)?;
        let tag = value_tag(
            *value_types
                .get(id)
                .ok_or(NativeError::Unsupported("missing output type"))?,
        );
        let tag_pointer = byte_pointer(
            context,
            builder,
            outputs,
            offset - SLOT_PAYLOAD_OFFSET,
            "tag",
        )?;
        builder
            .build_store(
                tag_pointer,
                context.i32_type().const_int(u64::from(tag), false),
            )
            .map_err(llvm_error)?;
        let payload = byte_pointer(context, builder, outputs, offset, "payload")?;
        builder
            .build_store(payload, value(values, *id)?)
            .map_err(llvm_error)?;
    }
    Ok(())
}

fn finalize_cfg_module(
    _context: &'static Context,
    module: &inkwell::module::Module<'static>,
    symbol: &str,
) -> Result<inkwell::execution_engine::JitFunction<'static, super::entry::NativeEntry>, NativeError>
{
    module.verify().map_err(llvm_error)?;
    let target_machine = native_target_machine()?;
    module.set_triple(&target_machine.get_triple());
    module.set_data_layout(&target_machine.get_target_data().get_data_layout());
    let options = PassBuilderOptions::create();
    options.set_verify_each(true);
    module
        .run_passes("default<O3>", &target_machine, options)
        .map_err(llvm_error)?;
    module.verify().map_err(llvm_error)?;
    let engine = module
        .create_jit_execution_engine(OptimizationLevel::Aggressive)
        .map_err(llvm_error)?;
    map_helpers(module, &engine);
    // SAFETY: [Categories 3, 5, 8, and 14] the verified CFG module declares
    // the exact NativeEntry ABI and the JitFunction owns the execution engine.
    unsafe { engine.get_function(symbol).map_err(llvm_error) }
}

fn lower_direct_list_append<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    function: inkwell::values::FunctionValue<'ctx>,
    frame: PointerValue<'ctx>,
    storage_index: usize,
    alias: IntValue<'ctx>,
    value: IntValue<'ctx>,
) -> Result<(), NativeError> {
    let base = load_pointer(
        context,
        builder,
        frame,
        DIRECT_STORAGE_OFFSET,
        "direct_storage",
    )?;
    let descriptor = unsafe {
        builder
            .build_gep(
                context.i8_type(),
                base,
                &[context.i64_type().const_int(
                    (storage_index * std::mem::size_of::<super::abi::NativeDirectStorage>()) as u64,
                    false,
                )],
                "direct_storage_slot",
            )
            .map_err(llvm_error)?
    };
    let magic = load_i64(
        context,
        builder,
        descriptor,
        DIRECT_MAGIC_OFFSET,
        "direct_magic",
    )?;
    let abi = load_i32(
        context,
        builder,
        descriptor,
        DIRECT_ABI_OFFSET,
        "direct_abi",
    )?;
    let strategy = load_i32(
        context,
        builder,
        descriptor,
        DIRECT_STRATEGY_OFFSET,
        "direct_strategy",
    )?;
    let expected_alias = load_i64(
        context,
        builder,
        descriptor,
        DIRECT_ALIAS_OFFSET,
        "direct_alias",
    )?;
    let version = load_i64(
        context,
        builder,
        descriptor,
        DIRECT_VERSION_OFFSET,
        "direct_version",
    )?;
    let length = load_i64(
        context,
        builder,
        descriptor,
        DIRECT_LENGTH_OFFSET,
        "direct_length",
    )?;
    let capacity = load_i64(
        context,
        builder,
        descriptor,
        DIRECT_CAPACITY_OFFSET,
        "direct_capacity",
    )?;
    let values = load_pointer(
        context,
        builder,
        descriptor,
        DIRECT_VALUES_OFFSET,
        "direct_values",
    )?;
    let checks = [
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            magic,
            context.i64_type().const_int(DIRECT_STORAGE_MAGIC, false),
            "valid_magic",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            abi,
            context
                .i32_type()
                .const_int(u64::from(DIRECT_STORAGE_ABI), false),
            "valid_abi",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            strategy,
            context.i32_type().const_int(1, false),
            "valid_strategy",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            expected_alias,
            alias,
            "valid_alias",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            builder
                .build_and(
                    version,
                    context.i64_type().const_int(1, false),
                    "version_bit",
                )
                .map_err(llvm_error)?,
            context.i64_type().const_zero(),
            "valid_version",
        ),
        builder.build_int_compare(inkwell::IntPredicate::ULT, length, capacity, "has_capacity"),
    ];
    let mut checks = checks.into_iter();
    let mut valid = checks
        .next()
        .ok_or(NativeError::Unsupported("LLVM direct storage checks"))?
        .map_err(llvm_error)?;
    for check in checks {
        valid = builder
            .build_and(valid, check.map_err(llvm_error)?, "direct_valid")
            .map_err(llvm_error)?;
    }
    let store = context.append_basic_block(function, "direct_append");
    let invalid = context.append_basic_block(function, "direct_append_invalid");
    builder
        .build_conditional_branch(valid, store, invalid)
        .map_err(llvm_error)?;
    builder.position_at_end(invalid);
    builder
        .build_return(Some(&context.i32_type().const_int(2, false)))
        .map_err(llvm_error)?;
    builder.position_at_end(store);
    let pointer = unsafe {
        builder
            .build_gep(context.i64_type(), values, &[length], "append_slot")
            .map_err(llvm_error)?
    };
    builder.build_store(pointer, value).map_err(llvm_error)?;
    let length_pointer = byte_pointer(
        context,
        builder,
        descriptor,
        DIRECT_LENGTH_OFFSET,
        "direct_length_ptr",
    )?;
    let next = builder
        .build_int_add(
            length,
            context.i64_type().const_int(1, false),
            "next_length",
        )
        .map_err(llvm_error)?;
    builder
        .build_store(length_pointer, next)
        .map_err(llvm_error)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn lower_direct_list_copy<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    function: inkwell::values::FunctionValue<'ctx>,
    frame: PointerValue<'ctx>,
    destination_index: usize,
    destination_alias: IntValue<'ctx>,
    source_index: usize,
    source_alias: IntValue<'ctx>,
) -> Result<(), NativeError> {
    let source_length = lower_direct_list_length(
        context,
        builder,
        function,
        frame,
        source_index,
        source_alias,
    )?;
    let _ = lower_direct_list_length(
        context,
        builder,
        function,
        frame,
        destination_index,
        destination_alias,
    )?;
    let base = load_pointer(
        context,
        builder,
        frame,
        DIRECT_STORAGE_OFFSET,
        "copy_storage",
    )?;
    let descriptor = |index: usize, name: &str| unsafe {
        builder
            .build_gep(
                context.i8_type(),
                base,
                &[context.i64_type().const_int(
                    (index * std::mem::size_of::<super::abi::NativeDirectStorage>()) as u64,
                    false,
                )],
                name,
            )
            .map_err(llvm_error)
    };
    let destination = descriptor(destination_index, "copy_destination")?;
    let source = descriptor(source_index, "copy_source")?;
    let destination_capacity = load_i64(
        context,
        builder,
        destination,
        DIRECT_CAPACITY_OFFSET,
        "copy_destination_capacity",
    )?;
    let valid = builder
        .build_int_compare(
            inkwell::IntPredicate::UGE,
            destination_capacity,
            source_length,
            "copy_capacity_valid",
        )
        .map_err(llvm_error)?;
    let ready = context.append_basic_block(function, "copy_capacity_valid");
    let invalid = context.append_basic_block(function, "copy_capacity_invalid");
    builder
        .build_conditional_branch(valid, ready, invalid)
        .map_err(llvm_error)?;
    builder.position_at_end(invalid);
    builder
        .build_return(Some(&context.i32_type().const_int(2, false)))
        .map_err(llvm_error)?;
    builder.position_at_end(ready);
    let destination_values = load_pointer(
        context,
        builder,
        destination,
        DIRECT_VALUES_OFFSET,
        "copy_destination_values",
    )?;
    let source_values = load_pointer(
        context,
        builder,
        source,
        DIRECT_VALUES_OFFSET,
        "copy_source_values",
    )?;
    let preheader = builder
        .get_insert_block()
        .ok_or(NativeError::Backend("missing copy preheader".to_owned()))?;
    let loop_block = context.append_basic_block(function, "copy_loop");
    let copy = context.append_basic_block(function, "copy_value");
    let done = context.append_basic_block(function, "copy_done");
    builder
        .build_unconditional_branch(loop_block)
        .map_err(llvm_error)?;
    builder.position_at_end(loop_block);
    let index_phi = builder
        .build_phi(context.i64_type(), "copy_index")
        .map_err(llvm_error)?;
    let zero = context.i64_type().const_zero();
    index_phi.add_incoming(&[(&zero, preheader)]);
    let index = index_phi.as_basic_value().into_int_value();
    let more = builder
        .build_int_compare(
            inkwell::IntPredicate::ULT,
            index,
            source_length,
            "copy_more",
        )
        .map_err(llvm_error)?;
    builder
        .build_conditional_branch(more, copy, done)
        .map_err(llvm_error)?;
    builder.position_at_end(copy);
    let source_pointer = unsafe {
        builder
            .build_gep(
                context.i64_type(),
                source_values,
                &[index],
                "copy_source_value",
            )
            .map_err(llvm_error)?
    };
    let destination_pointer = unsafe {
        builder
            .build_gep(
                context.i64_type(),
                destination_values,
                &[index],
                "copy_destination_value",
            )
            .map_err(llvm_error)?
    };
    let copied = builder
        .build_load(context.i64_type(), source_pointer, "copied_value")
        .map_err(llvm_error)?;
    builder
        .build_store(destination_pointer, copied)
        .map_err(llvm_error)?;
    let next = builder
        .build_int_add(index, context.i64_type().const_int(1, false), "copy_next")
        .map_err(llvm_error)?;
    builder
        .build_unconditional_branch(loop_block)
        .map_err(llvm_error)?;
    index_phi.add_incoming(&[(&next, copy)]);
    builder.position_at_end(done);
    let length = byte_pointer(
        context,
        builder,
        destination,
        DIRECT_LENGTH_OFFSET,
        "copy_destination_length",
    )?;
    builder
        .build_store(length, source_length)
        .map_err(llvm_error)?;
    Ok(())
}

fn lower_direct_list_clear<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    function: inkwell::values::FunctionValue<'ctx>,
    frame: PointerValue<'ctx>,
    storage_index: usize,
    alias: IntValue<'ctx>,
) -> Result<(), NativeError> {
    let _ = lower_direct_list_length(context, builder, function, frame, storage_index, alias)?;
    let base = load_pointer(
        context,
        builder,
        frame,
        DIRECT_STORAGE_OFFSET,
        "direct_storage",
    )?;
    let descriptor = unsafe {
        builder
            .build_gep(
                context.i8_type(),
                base,
                &[context.i64_type().const_int(
                    (storage_index * std::mem::size_of::<super::abi::NativeDirectStorage>()) as u64,
                    false,
                )],
                "direct_storage_slot",
            )
            .map_err(llvm_error)?
    };
    let length = byte_pointer(
        context,
        builder,
        descriptor,
        DIRECT_LENGTH_OFFSET,
        "direct_length",
    )?;
    builder
        .build_store(length, context.i64_type().const_zero())
        .map_err(llvm_error)?;
    Ok(())
}

fn lower_direct_list_length<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    function: inkwell::values::FunctionValue<'ctx>,
    frame: PointerValue<'ctx>,
    storage_index: usize,
    alias: IntValue<'ctx>,
) -> Result<IntValue<'ctx>, NativeError> {
    let base = load_pointer(
        context,
        builder,
        frame,
        DIRECT_STORAGE_OFFSET,
        "direct_storage",
    )?;
    let descriptor = unsafe {
        builder
            .build_gep(
                context.i8_type(),
                base,
                &[context.i64_type().const_int(
                    (storage_index * std::mem::size_of::<super::abi::NativeDirectStorage>()) as u64,
                    false,
                )],
                "direct_storage_slot",
            )
            .map_err(llvm_error)?
    };
    let magic = load_i64(
        context,
        builder,
        descriptor,
        DIRECT_MAGIC_OFFSET,
        "direct_magic",
    )?;
    let abi = load_i32(
        context,
        builder,
        descriptor,
        DIRECT_ABI_OFFSET,
        "direct_abi",
    )?;
    let strategy = load_i32(
        context,
        builder,
        descriptor,
        DIRECT_STRATEGY_OFFSET,
        "direct_strategy",
    )?;
    let expected_alias = load_i64(
        context,
        builder,
        descriptor,
        DIRECT_ALIAS_OFFSET,
        "direct_alias",
    )?;
    let version = load_i64(
        context,
        builder,
        descriptor,
        DIRECT_VERSION_OFFSET,
        "direct_version",
    )?;
    let checks = [
        builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                magic,
                context.i64_type().const_int(DIRECT_STORAGE_MAGIC, false),
                "valid_magic",
            )
            .map_err(llvm_error)?,
        builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                abi,
                context
                    .i32_type()
                    .const_int(u64::from(DIRECT_STORAGE_ABI), false),
                "valid_abi",
            )
            .map_err(llvm_error)?,
        builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                strategy,
                context.i32_type().const_int(1, false),
                "valid_strategy",
            )
            .map_err(llvm_error)?,
        builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                expected_alias,
                alias,
                "valid_alias",
            )
            .map_err(llvm_error)?,
        builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                builder
                    .build_and(
                        version,
                        context.i64_type().const_int(1, false),
                        "version_bit",
                    )
                    .map_err(llvm_error)?,
                context.i64_type().const_zero(),
                "valid_version",
            )
            .map_err(llvm_error)?,
    ];
    let mut valid = checks[0];
    for check in &checks[1..] {
        valid = builder
            .build_and(valid, *check, "direct_valid")
            .map_err(llvm_error)?;
    }
    let ready = context.append_basic_block(function, "direct_length");
    let invalid = context.append_basic_block(function, "direct_length_invalid");
    builder
        .build_conditional_branch(valid, ready, invalid)
        .map_err(llvm_error)?;
    builder.position_at_end(invalid);
    builder
        .build_return(Some(&context.i32_type().const_int(2, false)))
        .map_err(llvm_error)?;
    builder.position_at_end(ready);
    load_i64(
        context,
        builder,
        descriptor,
        DIRECT_LENGTH_OFFSET,
        "direct_length",
    )
}

fn resolve_dynamic_direct_storage<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    function: inkwell::values::FunctionValue<'ctx>,
    frame: PointerValue<'ctx>,
    alias: IntValue<'ctx>,
) -> Result<PointerValue<'ctx>, NativeError> {
    let descriptor_base = load_pointer(
        context,
        builder,
        frame,
        DIRECT_STORAGE_OFFSET,
        "dynamic_descriptors",
    )?;
    let receipt_base = load_pointer(
        context,
        builder,
        frame,
        DIRECT_STORAGE_RECEIPTS_OFFSET,
        "dynamic_receipts",
    )?;
    let count = load_i32(
        context,
        builder,
        frame,
        DIRECT_STORAGE_COUNT_OFFSET,
        "dynamic_count",
    )?;
    let index_base = load_pointer(
        context,
        builder,
        frame,
        DIRECT_STORAGE_INDEX_OFFSET,
        "dynamic_index",
    )?;
    let slot = builder
        .build_int_truncate(alias, context.i32_type(), "dynamic_slot")
        .map_err(llvm_error)?;
    let generation = builder
        .build_right_shift(
            alias,
            context.i64_type().const_int(32, false),
            false,
            "dynamic_generation_shift",
        )
        .map_err(llvm_error)?;
    let generation = builder
        .build_and(
            generation,
            context.i64_type().const_int(u64::from(u16::MAX), false),
            "dynamic_generation",
        )
        .map_err(llvm_error)?;
    let reserved = builder
        .build_right_shift(
            alias,
            context.i64_type().const_int(48, false),
            false,
            "dynamic_reserved",
        )
        .map_err(llvm_error)?;
    let token_checks = [
        builder.build_is_not_null(index_base, "dynamic_index_present"),
        builder.build_int_compare(
            inkwell::IntPredicate::ULT,
            slot,
            context.i32_type().const_int(
                crate::adaptive_v2::handles::NATIVE_HANDLE_CAPACITY as u64,
                false,
            ),
            "dynamic_slot_valid",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::NE,
            generation,
            context.i64_type().const_zero(),
            "dynamic_generation_valid",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            reserved,
            context.i64_type().const_zero(),
            "dynamic_reserved_valid",
        ),
    ];
    let mut token_checks = token_checks.into_iter();
    let mut token_valid = token_checks
        .next()
        .ok_or(NativeError::Unsupported("LLVM dynamic token checks"))?
        .map_err(llvm_error)?;
    for check in token_checks {
        token_valid = builder
            .build_and(
                token_valid,
                check.map_err(llvm_error)?,
                "dynamic_token_valid",
            )
            .map_err(llvm_error)?;
    }
    let indexed = context.append_basic_block(function, "dynamic_indexed");
    let invalid = context.append_basic_block(function, "dynamic_missing");
    builder
        .build_conditional_branch(token_valid, indexed, invalid)
        .map_err(llvm_error)?;
    builder.position_at_end(invalid);
    builder
        .build_return(Some(&context.i32_type().const_int(2, false)))
        .map_err(llvm_error)?;
    builder.position_at_end(indexed);
    let slot = builder
        .build_int_z_extend(slot, context.i64_type(), "dynamic_slot_index")
        .map_err(llvm_error)?;
    let index_address = unsafe {
        builder
            .build_gep(context.i8_type(), index_base, &[slot], "dynamic_index_slot")
            .map_err(llvm_error)?
    };
    let selected_index = builder
        .build_load(context.i8_type(), index_address, "dynamic_descriptor_index")
        .map_err(llvm_error)?
        .into_int_value();
    let selected_index_i32 = builder
        .build_int_z_extend(selected_index, context.i32_type(), "dynamic_index_i32")
        .map_err(llvm_error)?;
    let found = builder
        .build_and(
            builder
                .build_int_compare(
                    inkwell::IntPredicate::NE,
                    selected_index,
                    context.i8_type().const_int(
                        u64::from(super::direct_storage::DYNAMIC_STORAGE_INDEX_EMPTY),
                        false,
                    ),
                    "dynamic_index_present",
                )
                .map_err(llvm_error)?,
            builder
                .build_int_compare(
                    inkwell::IntPredicate::ULT,
                    selected_index_i32,
                    count,
                    "dynamic_index_in_count",
                )
                .map_err(llvm_error)?,
            "dynamic_found",
        )
        .map_err(llvm_error)?;
    let ready = context.append_basic_block(function, "dynamic_found");
    let invalid = context.append_basic_block(function, "dynamic_index_missing");
    builder
        .build_conditional_branch(found, ready, invalid)
        .map_err(llvm_error)?;
    builder.position_at_end(invalid);
    builder
        .build_return(Some(&context.i32_type().const_int(2, false)))
        .map_err(llvm_error)?;
    builder.position_at_end(ready);
    let selected_index = builder
        .build_int_z_extend(selected_index, context.i64_type(), "dynamic_index_i64")
        .map_err(llvm_error)?;
    let descriptor_offset = builder
        .build_int_mul(
            selected_index,
            context.i64_type().const_int(
                std::mem::size_of::<super::abi::NativeDirectStorage>() as u64,
                false,
            ),
            "dynamic_descriptor_offset",
        )
        .map_err(llvm_error)?;
    let receipt_offset = builder
        .build_int_mul(
            selected_index,
            context.i64_type().const_int(
                std::mem::size_of::<super::abi::NativeDirectStorageReceipt>() as u64,
                false,
            ),
            "dynamic_receipt_offset",
        )
        .map_err(llvm_error)?;
    let selected = unsafe {
        builder
            .build_gep(
                context.i8_type(),
                descriptor_base,
                &[descriptor_offset],
                "selected_descriptor",
            )
            .map_err(llvm_error)?
    };
    let selected_receipt = unsafe {
        builder
            .build_gep(
                context.i8_type(),
                receipt_base,
                &[receipt_offset],
                "selected_receipt",
            )
            .map_err(llvm_error)?
    };
    let magic = load_i64(
        context,
        builder,
        selected,
        DIRECT_MAGIC_OFFSET,
        "dynamic_magic",
    )?;
    let abi = load_i32(context, builder, selected, DIRECT_ABI_OFFSET, "dynamic_abi")?;
    let strategy = load_i32(
        context,
        builder,
        selected,
        DIRECT_STRATEGY_OFFSET,
        "dynamic_strategy",
    )?;
    let descriptor_alias = load_i64(
        context,
        builder,
        selected,
        DIRECT_ALIAS_OFFSET,
        "dynamic_alias",
    )?;
    let owner = load_i64(
        context,
        builder,
        selected,
        DIRECT_OWNER_OFFSET,
        "dynamic_owner",
    )?;
    let layout = load_i64(
        context,
        builder,
        selected,
        DIRECT_LAYOUT_EPOCH_OFFSET,
        "dynamic_layout",
    )?;
    let version = load_i64(
        context,
        builder,
        selected,
        DIRECT_VERSION_OFFSET,
        "dynamic_version",
    )?;
    let length = load_i64(
        context,
        builder,
        selected,
        DIRECT_LENGTH_OFFSET,
        "dynamic_length",
    )?;
    let capacity = load_i64(
        context,
        builder,
        selected,
        DIRECT_CAPACITY_OFFSET,
        "dynamic_capacity",
    )?;
    let values = load_pointer(
        context,
        builder,
        selected,
        DIRECT_VALUES_OFFSET,
        "dynamic_values",
    )?;
    let receipt_identity = load_i64(
        context,
        builder,
        selected_receipt,
        RECEIPT_STORAGE_IDENTITY_OFFSET,
        "receipt_identity",
    )?;
    let receipt_strategy = load_i32(
        context,
        builder,
        selected_receipt,
        RECEIPT_STRATEGY_OFFSET,
        "receipt_strategy",
    )?;
    let receipt_alias = load_i64(
        context,
        builder,
        selected_receipt,
        RECEIPT_ALIAS_OFFSET,
        "receipt_alias",
    )?;
    let receipt_owner = load_i64(
        context,
        builder,
        selected_receipt,
        RECEIPT_OWNER_OFFSET,
        "receipt_owner",
    )?;
    let receipt_layout = load_i64(
        context,
        builder,
        selected_receipt,
        RECEIPT_LAYOUT_EPOCH_OFFSET,
        "receipt_layout",
    )?;
    let receipt_version = load_i64(
        context,
        builder,
        selected_receipt,
        RECEIPT_VERSION_OFFSET,
        "receipt_version",
    )?;
    let version_bit = builder
        .build_and(
            version,
            context.i64_type().const_int(1, false),
            "dynamic_version_bit",
        )
        .map_err(llvm_error)?;
    let checks = [
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            magic,
            context.i64_type().const_int(DIRECT_STORAGE_MAGIC, false),
            "dynamic_valid_magic",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            abi,
            context
                .i32_type()
                .const_int(u64::from(DIRECT_STORAGE_ABI), false),
            "dynamic_valid_abi",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            strategy,
            context.i32_type().const_int(1, false),
            "dynamic_valid_strategy",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            strategy,
            receipt_strategy,
            "dynamic_receipt_strategy",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            descriptor_alias,
            alias,
            "dynamic_descriptor_alias",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            receipt_identity,
            alias,
            "dynamic_receipt_identity",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            receipt_alias,
            alias,
            "dynamic_receipt_alias",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            owner,
            receipt_owner,
            "dynamic_receipt_owner",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            layout,
            receipt_layout,
            "dynamic_receipt_layout",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            version,
            receipt_version,
            "dynamic_receipt_version",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            version_bit,
            context.i64_type().const_zero(),
            "dynamic_stable_version",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::ULE,
            length,
            capacity,
            "dynamic_valid_length",
        ),
        builder.build_is_not_null(values, "dynamic_values_present"),
    ];
    let mut checks = checks.into_iter();
    let mut valid = checks
        .next()
        .ok_or(NativeError::Unsupported("LLVM dynamic storage checks"))?
        .map_err(llvm_error)?;
    for check in checks {
        valid = builder
            .build_and(valid, check.map_err(llvm_error)?, "dynamic_valid")
            .map_err(llvm_error)?;
    }
    let valid_block = context.append_basic_block(function, "dynamic_valid");
    let invalid = context.append_basic_block(function, "dynamic_invalid");
    builder
        .build_conditional_branch(valid, valid_block, invalid)
        .map_err(llvm_error)?;
    builder.position_at_end(invalid);
    builder
        .build_return(Some(&context.i32_type().const_int(2, false)))
        .map_err(llvm_error)?;
    builder.position_at_end(valid_block);
    Ok(selected)
}

fn resolve_fixed_direct_storage<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    function: inkwell::values::FunctionValue<'ctx>,
    frame: PointerValue<'ctx>,
    storage_index: usize,
    source: super::direct_storage::DirectStorageSource,
    runtime_alias: IntValue<'ctx>,
) -> Result<PointerValue<'ctx>, NativeError> {
    let descriptor_base = load_pointer(
        context,
        builder,
        frame,
        DIRECT_STORAGE_OFFSET,
        "reverse_descriptors",
    )?;
    let receipt_base = load_pointer(
        context,
        builder,
        frame,
        DIRECT_STORAGE_RECEIPTS_OFFSET,
        "reverse_receipts",
    )?;
    let bases_present = builder
        .build_and(
            builder
                .build_is_not_null(descriptor_base, "reverse_descriptors_present")
                .map_err(llvm_error)?,
            builder
                .build_is_not_null(receipt_base, "reverse_receipts_present")
                .map_err(llvm_error)?,
            "reverse_bases_present",
        )
        .map_err(llvm_error)?;
    let present = context.append_basic_block(function, "reverse_storage_present");
    let missing = context.append_basic_block(function, "reverse_storage_missing");
    builder
        .build_conditional_branch(bases_present, present, missing)
        .map_err(llvm_error)?;
    builder.position_at_end(missing);
    builder
        .build_return(Some(&context.i32_type().const_int(2, false)))
        .map_err(llvm_error)?;
    builder.position_at_end(present);
    let descriptor_offset = context.i64_type().const_int(
        (storage_index * std::mem::size_of::<super::abi::NativeDirectStorage>()) as u64,
        false,
    );
    let receipt_offset = context.i64_type().const_int(
        (storage_index * std::mem::size_of::<super::abi::NativeDirectStorageReceipt>()) as u64,
        false,
    );
    let descriptor = unsafe {
        builder
            .build_gep(
                context.i8_type(),
                descriptor_base,
                &[descriptor_offset],
                "reverse_descriptor",
            )
            .map_err(llvm_error)?
    };
    let receipt = unsafe {
        builder
            .build_gep(
                context.i8_type(),
                receipt_base,
                &[receipt_offset],
                "reverse_receipt",
            )
            .map_err(llvm_error)?
    };
    let expected_alias = match source {
        super::direct_storage::DirectStorageSource::EntryHandle(_) => runtime_alias,
        super::direct_storage::DirectStorageSource::OwnedList { identity } => context
            .i64_type()
            .const_int(super::direct_storage::owned_alias(identity), false),
    };
    let magic = load_i64(
        context,
        builder,
        descriptor,
        DIRECT_MAGIC_OFFSET,
        "reverse_magic",
    )?;
    let abi = load_i32(
        context,
        builder,
        descriptor,
        DIRECT_ABI_OFFSET,
        "reverse_abi",
    )?;
    let strategy = load_i32(
        context,
        builder,
        descriptor,
        DIRECT_STRATEGY_OFFSET,
        "reverse_strategy",
    )?;
    let alias = load_i64(
        context,
        builder,
        descriptor,
        DIRECT_ALIAS_OFFSET,
        "reverse_alias",
    )?;
    let owner = load_i64(
        context,
        builder,
        descriptor,
        DIRECT_OWNER_OFFSET,
        "reverse_owner",
    )?;
    let layout = load_i64(
        context,
        builder,
        descriptor,
        DIRECT_LAYOUT_EPOCH_OFFSET,
        "reverse_layout",
    )?;
    let version = load_i64(
        context,
        builder,
        descriptor,
        DIRECT_VERSION_OFFSET,
        "reverse_version",
    )?;
    let length = load_i64(
        context,
        builder,
        descriptor,
        DIRECT_LENGTH_OFFSET,
        "reverse_length",
    )?;
    let capacity = load_i64(
        context,
        builder,
        descriptor,
        DIRECT_CAPACITY_OFFSET,
        "reverse_capacity",
    )?;
    let values = load_pointer(
        context,
        builder,
        descriptor,
        DIRECT_VALUES_OFFSET,
        "reverse_values",
    )?;
    let receipt_identity = load_i64(
        context,
        builder,
        receipt,
        RECEIPT_STORAGE_IDENTITY_OFFSET,
        "reverse_receipt_identity",
    )?;
    let receipt_strategy = load_i32(
        context,
        builder,
        receipt,
        RECEIPT_STRATEGY_OFFSET,
        "reverse_receipt_strategy",
    )?;
    let receipt_alias = load_i64(
        context,
        builder,
        receipt,
        RECEIPT_ALIAS_OFFSET,
        "reverse_receipt_alias",
    )?;
    let receipt_owner = load_i64(
        context,
        builder,
        receipt,
        RECEIPT_OWNER_OFFSET,
        "reverse_receipt_owner",
    )?;
    let receipt_layout = load_i64(
        context,
        builder,
        receipt,
        RECEIPT_LAYOUT_EPOCH_OFFSET,
        "reverse_receipt_layout",
    )?;
    let receipt_version = load_i64(
        context,
        builder,
        receipt,
        RECEIPT_VERSION_OFFSET,
        "reverse_receipt_version",
    )?;
    let version_bit = builder
        .build_and(
            version,
            context.i64_type().const_int(1, false),
            "reverse_version_bit",
        )
        .map_err(llvm_error)?;
    let expected_identity = context
        .i64_type()
        .const_int(source.receipt_identity(), false);
    let checks = [
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            magic,
            context.i64_type().const_int(DIRECT_STORAGE_MAGIC, false),
            "reverse_valid_magic",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            abi,
            context
                .i32_type()
                .const_int(u64::from(DIRECT_STORAGE_ABI), false),
            "reverse_valid_abi",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            strategy,
            context.i32_type().const_int(1, false),
            "reverse_valid_strategy",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            strategy,
            receipt_strategy,
            "reverse_receipt_strategy_match",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            alias,
            expected_alias,
            "reverse_expected_alias",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            alias,
            receipt_alias,
            "reverse_receipt_alias_match",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            receipt_identity,
            expected_identity,
            "reverse_receipt_identity_match",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            owner,
            receipt_owner,
            "reverse_receipt_owner_match",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            layout,
            receipt_layout,
            "reverse_receipt_layout_match",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            version,
            receipt_version,
            "reverse_receipt_version_match",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::EQ,
            version_bit,
            context.i64_type().const_zero(),
            "reverse_stable_version",
        ),
        builder.build_int_compare(
            inkwell::IntPredicate::ULE,
            length,
            capacity,
            "reverse_valid_length",
        ),
        builder.build_is_not_null(values, "reverse_values_present"),
    ];
    let mut checks = checks.into_iter();
    let mut valid = checks
        .next()
        .ok_or(NativeError::Unsupported("LLVM reverse storage checks"))?
        .map_err(llvm_error)?;
    for check in checks {
        valid = builder
            .build_and(valid, check.map_err(llvm_error)?, "reverse_storage_valid")
            .map_err(llvm_error)?;
    }
    let ready = context.append_basic_block(function, "reverse_storage_valid");
    let invalid = context.append_basic_block(function, "reverse_storage_invalid");
    builder
        .build_conditional_branch(valid, ready, invalid)
        .map_err(llvm_error)?;
    builder.position_at_end(invalid);
    builder
        .build_return(Some(&context.i32_type().const_int(2, false)))
        .map_err(llvm_error)?;
    builder.position_at_end(ready);
    Ok(descriptor)
}

fn lower_list_reverse_prefix<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    function: inkwell::values::FunctionValue<'ctx>,
    descriptor: PointerValue<'ctx>,
    end: IntValue<'ctx>,
) -> Result<(), NativeError> {
    let length = load_i64(
        context,
        builder,
        descriptor,
        DIRECT_LENGTH_OFFSET,
        "reverse_current_length",
    )?;
    let positive = builder
        .build_int_compare(
            inkwell::IntPredicate::SGE,
            end,
            context.i64_type().const_int(1, false),
            "reverse_end_positive",
        )
        .map_err(llvm_error)?;
    let in_bounds = builder
        .build_int_compare(
            inkwell::IntPredicate::ULE,
            end,
            length,
            "reverse_end_in_bounds",
        )
        .map_err(llvm_error)?;
    let valid = builder
        .build_and(positive, in_bounds, "reverse_bounds_valid")
        .map_err(llvm_error)?;
    let ready = context.append_basic_block(function, "reverse_bounds_valid");
    let invalid = context.append_basic_block(function, "reverse_bounds_invalid");
    builder
        .build_conditional_branch(valid, ready, invalid)
        .map_err(llvm_error)?;
    builder.position_at_end(invalid);
    builder
        .build_return(Some(&context.i32_type().const_int(2, false)))
        .map_err(llvm_error)?;
    builder.position_at_end(ready);
    let values = load_pointer(
        context,
        builder,
        descriptor,
        DIRECT_VALUES_OFFSET,
        "reverse_values_ptr",
    )?;
    let preheader = builder
        .get_insert_block()
        .ok_or(NativeError::Backend("missing reverse preheader".to_owned()))?;
    let loop_block = context.append_basic_block(function, "reverse_loop");
    let swap = context.append_basic_block(function, "reverse_swap");
    let done = context.append_basic_block(function, "reverse_done");
    builder
        .build_unconditional_branch(loop_block)
        .map_err(llvm_error)?;
    builder.position_at_end(loop_block);
    let left_phi = builder
        .build_phi(context.i64_type(), "reverse_left")
        .map_err(llvm_error)?;
    let right_phi = builder
        .build_phi(context.i64_type(), "reverse_right")
        .map_err(llvm_error)?;
    let zero = context.i64_type().const_zero();
    let initial_right = builder
        .build_int_sub(
            end,
            context.i64_type().const_int(1, false),
            "reverse_initial_right",
        )
        .map_err(llvm_error)?;
    left_phi.add_incoming(&[(&zero, preheader)]);
    right_phi.add_incoming(&[(&initial_right, preheader)]);
    let left = left_phi.as_basic_value().into_int_value();
    let right = right_phi.as_basic_value().into_int_value();
    let more = builder
        .build_int_compare(inkwell::IntPredicate::ULT, left, right, "reverse_more")
        .map_err(llvm_error)?;
    builder
        .build_conditional_branch(more, swap, done)
        .map_err(llvm_error)?;
    builder.position_at_end(swap);
    let left_pointer = unsafe {
        builder
            .build_gep(context.i64_type(), values, &[left], "reverse_left_ptr")
            .map_err(llvm_error)?
    };
    let right_pointer = unsafe {
        builder
            .build_gep(context.i64_type(), values, &[right], "reverse_right_ptr")
            .map_err(llvm_error)?
    };
    let left_value = builder
        .build_load(context.i64_type(), left_pointer, "reverse_left_value")
        .map_err(llvm_error)?;
    let right_value = builder
        .build_load(context.i64_type(), right_pointer, "reverse_right_value")
        .map_err(llvm_error)?;
    builder
        .build_store(left_pointer, right_value)
        .map_err(llvm_error)?;
    builder
        .build_store(right_pointer, left_value)
        .map_err(llvm_error)?;
    let next_left = builder
        .build_int_add(
            left,
            context.i64_type().const_int(1, false),
            "reverse_next_left",
        )
        .map_err(llvm_error)?;
    let next_right = builder
        .build_int_sub(
            right,
            context.i64_type().const_int(1, false),
            "reverse_next_right",
        )
        .map_err(llvm_error)?;
    builder
        .build_unconditional_branch(loop_block)
        .map_err(llvm_error)?;
    left_phi.add_incoming(&[(&next_left, swap)]);
    right_phi.add_incoming(&[(&next_right, swap)]);
    builder.position_at_end(done);
    Ok(())
}

fn dynamic_element_pointer<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    function: inkwell::values::FunctionValue<'ctx>,
    frame: PointerValue<'ctx>,
    alias: IntValue<'ctx>,
    index: IntValue<'ctx>,
) -> Result<PointerValue<'ctx>, NativeError> {
    let descriptor = resolve_dynamic_direct_storage(context, builder, function, frame, alias)?;
    let length = load_i64(
        context,
        builder,
        descriptor,
        DIRECT_LENGTH_OFFSET,
        "dynamic_length",
    )?;
    let values = load_pointer(
        context,
        builder,
        descriptor,
        DIRECT_VALUES_OFFSET,
        "dynamic_values",
    )?;
    let nonnegative = builder
        .build_int_compare(
            inkwell::IntPredicate::SGE,
            index,
            context.i64_type().const_zero(),
            "dynamic_nonnegative",
        )
        .map_err(llvm_error)?;
    let in_bounds = builder
        .build_int_compare(
            inkwell::IntPredicate::ULT,
            index,
            length,
            "dynamic_in_bounds",
        )
        .map_err(llvm_error)?;
    let valid = builder
        .build_and(nonnegative, in_bounds, "dynamic_bounds")
        .map_err(llvm_error)?;
    let access = context.append_basic_block(function, "dynamic_access");
    let invalid = context.append_basic_block(function, "dynamic_bounds_invalid");
    builder
        .build_conditional_branch(valid, access, invalid)
        .map_err(llvm_error)?;
    builder.position_at_end(invalid);
    builder
        .build_return(Some(&context.i32_type().const_int(2, false)))
        .map_err(llvm_error)?;
    builder.position_at_end(access);
    unsafe {
        builder
            .build_gep(context.i64_type(), values, &[index], "dynamic_element")
            .map_err(llvm_error)
    }
}

fn lower_dynamic_list_get<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    function: inkwell::values::FunctionValue<'ctx>,
    frame: PointerValue<'ctx>,
    alias: IntValue<'ctx>,
    index: IntValue<'ctx>,
) -> Result<IntValue<'ctx>, NativeError> {
    let pointer = dynamic_element_pointer(context, builder, function, frame, alias, index)?;
    Ok(builder
        .build_load(context.i64_type(), pointer, "dynamic_value")
        .map_err(llvm_error)?
        .into_int_value())
}

fn lower_dynamic_list_set<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    function: inkwell::values::FunctionValue<'ctx>,
    frame: PointerValue<'ctx>,
    alias: IntValue<'ctx>,
    index: IntValue<'ctx>,
    value: IntValue<'ctx>,
) -> Result<(), NativeError> {
    let pointer = dynamic_element_pointer(context, builder, function, frame, alias, index)?;
    builder.build_store(pointer, value).map_err(llvm_error)?;
    Ok(())
}

fn lower_direct_list_get<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    function: inkwell::values::FunctionValue<'ctx>,
    frame: PointerValue<'ctx>,
    storage_index: usize,
    alias: IntValue<'ctx>,
    index: IntValue<'ctx>,
) -> Result<IntValue<'ctx>, NativeError> {
    let descriptor_base = load_pointer(
        context,
        builder,
        frame,
        DIRECT_STORAGE_OFFSET,
        "direct_storage",
    )?;
    let descriptor = unsafe {
        builder
            .build_gep(
                context.i8_type(),
                descriptor_base,
                &[context.i64_type().const_int(
                    (storage_index * std::mem::size_of::<super::abi::NativeDirectStorage>()) as u64,
                    false,
                )],
                "direct_storage_slot",
            )
            .map_err(llvm_error)?
    };
    let magic = load_i64(
        context,
        builder,
        descriptor,
        DIRECT_MAGIC_OFFSET,
        "direct_magic",
    )?;
    let abi = load_i32(
        context,
        builder,
        descriptor,
        DIRECT_ABI_OFFSET,
        "direct_abi",
    )?;
    let strategy = load_i32(
        context,
        builder,
        descriptor,
        DIRECT_STRATEGY_OFFSET,
        "direct_strategy",
    )?;
    let expected_alias = load_i64(
        context,
        builder,
        descriptor,
        DIRECT_ALIAS_OFFSET,
        "direct_alias",
    )?;
    let version = load_i64(
        context,
        builder,
        descriptor,
        DIRECT_VERSION_OFFSET,
        "direct_version",
    )?;
    let length = load_i64(
        context,
        builder,
        descriptor,
        DIRECT_LENGTH_OFFSET,
        "direct_length",
    )?;
    let values = load_pointer(
        context,
        builder,
        descriptor,
        DIRECT_VALUES_OFFSET,
        "direct_values",
    )?;
    let predicates = [
        builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                magic,
                context.i64_type().const_int(DIRECT_STORAGE_MAGIC, false),
                "valid_magic",
            )
            .map_err(llvm_error)?,
        builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                abi,
                context
                    .i32_type()
                    .const_int(u64::from(DIRECT_STORAGE_ABI), false),
                "valid_abi",
            )
            .map_err(llvm_error)?,
        builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                strategy,
                context.i32_type().const_int(1, false),
                "valid_strategy",
            )
            .map_err(llvm_error)?,
        builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                expected_alias,
                alias,
                "valid_alias",
            )
            .map_err(llvm_error)?,
        builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                builder
                    .build_and(
                        version,
                        context.i64_type().const_int(1, false),
                        "version_bit",
                    )
                    .map_err(llvm_error)?,
                context.i64_type().const_zero(),
                "valid_version",
            )
            .map_err(llvm_error)?,
        builder
            .build_int_compare(
                inkwell::IntPredicate::SGE,
                index,
                context.i64_type().const_zero(),
                "nonnegative_index",
            )
            .map_err(llvm_error)?,
        builder
            .build_int_compare(inkwell::IntPredicate::ULT, index, length, "in_bounds")
            .map_err(llvm_error)?,
    ];
    let mut valid = predicates[0];
    for predicate in &predicates[1..] {
        valid = builder
            .build_and(valid, *predicate, "direct_valid")
            .map_err(llvm_error)?;
    }
    let load = context.append_basic_block(function, "direct_load");
    let invalid = context.append_basic_block(function, "direct_invalid");
    builder
        .build_conditional_branch(valid, load, invalid)
        .map_err(llvm_error)?;
    builder.position_at_end(invalid);
    builder
        .build_return(Some(&context.i32_type().const_int(2, false)))
        .map_err(llvm_error)?;
    builder.position_at_end(load);
    let address = builder
        .build_int_add(
            builder
                .build_ptr_to_int(values, context.i64_type(), "values_address")
                .map_err(llvm_error)?,
            builder
                .build_int_mul(
                    index,
                    context.i64_type().const_int(8, false),
                    "element_offset",
                )
                .map_err(llvm_error)?,
            "element_address",
        )
        .map_err(llvm_error)?;
    let pointer = builder
        .build_int_to_ptr(
            address,
            context.ptr_type(AddressSpace::default()),
            "element_pointer",
        )
        .map_err(llvm_error)?;
    // SAFETY: [Categories 3, 8, 10, and 13] the checked index is within the
    // owned Arc segment retained by DirectStorageLease until native return.
    Ok(builder
        .build_load(context.i64_type(), pointer, "direct_integer")
        .map_err(llvm_error)?
        .into_int_value())
}

fn load_i64<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    base: PointerValue<'ctx>,
    offset: i32,
    name: &str,
) -> Result<IntValue<'ctx>, NativeError> {
    let pointer = byte_pointer(context, builder, base, offset, name)?;
    Ok(builder
        .build_load(context.i64_type(), pointer, name)
        .map_err(llvm_error)?
        .into_int_value())
}

fn load_i32<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    base: PointerValue<'ctx>,
    offset: i32,
    name: &str,
) -> Result<IntValue<'ctx>, NativeError> {
    let pointer = byte_pointer(context, builder, base, offset, name)?;
    Ok(builder
        .build_load(context.i32_type(), pointer, name)
        .map_err(llvm_error)?
        .into_int_value())
}

fn lower_guard<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    function: inkwell::values::FunctionValue<'ctx>,
    frame: PointerValue<'ctx>,
    condition: IntValue<'ctx>,
    guard: u32,
    safepoint: u32,
) -> Result<(), NativeError> {
    let pass = context.append_basic_block(function, "guard_pass");
    let fail = context.append_basic_block(function, "guard_fail");
    let condition = builder
        .build_int_compare(
            inkwell::IntPredicate::NE,
            condition,
            context.i64_type().const_zero(),
            "guard_condition",
        )
        .map_err(llvm_error)?;
    builder
        .build_conditional_branch(condition, pass, fail)
        .map_err(llvm_error)?;
    builder.position_at_end(fail);
    store_i32(context, builder, frame, EXIT_KIND_OFFSET, 1, "guard_exit")?;
    store_i32(context, builder, frame, GUARD_ID_OFFSET, guard, "guard_id")?;
    store_i32(
        context,
        builder,
        frame,
        DEOPT_ID_OFFSET,
        guard,
        "guard_deopt",
    )?;
    store_i32(
        context,
        builder,
        frame,
        SAFEPOINT_ID_OFFSET,
        safepoint,
        "guard_safepoint",
    )?;
    let count_pointer = byte_pointer(context, builder, frame, DEOPTS_OFFSET, "deopts")?;
    let count = builder
        .build_load(context.i64_type(), count_pointer, "deopt_count")
        .map_err(llvm_error)?
        .into_int_value();
    let count = builder
        .build_int_add(count, context.i64_type().const_int(1, false), "next_deopt")
        .map_err(llvm_error)?;
    builder
        .build_store(count_pointer, count)
        .map_err(llvm_error)?;
    builder
        .build_return(Some(&context.i32_type().const_int(1, false)))
        .map_err(llvm_error)?;
    builder.position_at_end(pass);
    Ok(())
}

fn load_pointer<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    base: PointerValue<'ctx>,
    offset: i32,
    name: &str,
) -> Result<PointerValue<'ctx>, NativeError> {
    let pointer = byte_pointer(context, builder, base, offset, name)?;
    Ok(builder
        .build_load(context.ptr_type(AddressSpace::default()), pointer, name)
        .map_err(llvm_error)?
        .into_pointer_value())
}

fn store_i32<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    frame: PointerValue<'ctx>,
    offset: i32,
    value: u32,
    name: &str,
) -> Result<(), NativeError> {
    let pointer = byte_pointer(context, builder, frame, offset, name)?;
    builder
        .build_store(
            pointer,
            context.i32_type().const_int(u64::from(value), false),
        )
        .map(|_| ())
        .map_err(llvm_error)
}

fn byte_pointer<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    base: PointerValue<'ctx>,
    offset: i32,
    name: &str,
) -> Result<PointerValue<'ctx>, NativeError> {
    let index = context.i64_type().const_int(offset as u64, true);
    // SAFETY: [Categories 8, 10, and 13] every offset is a compile-time field
    // or verified slot offset inside the caller-validated NativeFrame buffers.
    unsafe {
        builder
            .build_gep(context.i8_type(), base, &[index], name)
            .map_err(llvm_error)
    }
}

fn native_type(ty: ValueType) -> Result<(), NativeError> {
    match ty {
        ValueType::I64 | ValueType::F64 | ValueType::Bool | ValueType::Handle => Ok(()),
        ValueType::BorrowedView => Err(NativeError::Unsupported("LLVM value type")),
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
    if types.values().any(|ty| *ty == ValueType::BorrowedView) {
        return Err(NativeError::Unsupported("LLVM value type"));
    }
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
                .ok_or(NativeError::Unsupported("missing LLVM return type"))
        })
        .collect()
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

const fn int_predicate(comparison: NumericComparison) -> inkwell::IntPredicate {
    match comparison {
        NumericComparison::Equal => inkwell::IntPredicate::EQ,
        NumericComparison::NotEqual => inkwell::IntPredicate::NE,
        NumericComparison::LessThan => inkwell::IntPredicate::SLT,
        NumericComparison::LessEqual => inkwell::IntPredicate::SLE,
        NumericComparison::GreaterThan => inkwell::IntPredicate::SGT,
        NumericComparison::GreaterEqual => inkwell::IntPredicate::SGE,
    }
}

fn lower_constant<'ctx>(
    context: &'ctx Context,
    constant: &Constant,
) -> Result<IntValue<'ctx>, NativeError> {
    match constant {
        Constant::Integer(value) => Ok(context.i64_type().const_int(*value as u64, true)),
        Constant::Boolean(value) => Ok(context.i64_type().const_int(u64::from(*value), false)),
        Constant::HandleBits(value) => Ok(context.i64_type().const_int(*value, false)),
        Constant::FloatBits(_) | Constant::UndefinedDead => {
            Err(NativeError::Unsupported("LLVM constant"))
        }
    }
}

fn value<'ctx>(
    values: &BTreeMap<ValueId, IntValue<'ctx>>,
    id: ValueId,
) -> Result<IntValue<'ctx>, NativeError> {
    values
        .get(&id)
        .copied()
        .ok_or(NativeError::Unsupported("missing LLVM value"))
}

fn two_inputs<'ctx>(
    values: &BTreeMap<ValueId, IntValue<'ctx>>,
    inputs: &[ValueId],
) -> Result<[IntValue<'ctx>; 2], NativeError> {
    match inputs {
        [left, right] => Ok([value(values, *left)?, value(values, *right)?]),
        _ => Err(NativeError::Unsupported("LLVM binary arity")),
    }
}

fn slot_offset(index: usize) -> Result<i32, NativeError> {
    i32::try_from(index)
        .ok()
        .and_then(|index| index.checked_mul(SLOT_SIZE))
        .and_then(|offset| offset.checked_add(SLOT_PAYLOAD_OFFSET))
        .ok_or(NativeError::CountOverflow)
}

fn native_target_machine() -> Result<TargetMachine, NativeError> {
    Target::initialize_native(&InitializationConfig::default()).map_err(llvm_error)?;
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).map_err(llvm_error)?;
    target
        .create_target_machine(
            &triple,
            &TargetMachine::get_host_cpu_name().to_string_lossy(),
            &TargetMachine::get_host_cpu_features().to_string_lossy(),
            OptimizationLevel::Aggressive,
            RelocMode::Default,
            CodeModel::JITDefault,
        )
        .ok_or_else(|| NativeError::Backend("LLVM target machine".to_string()))
}

fn llvm_error(error: impl std::fmt::Display) -> NativeError {
    NativeError::Backend(format!("LLVM: {error}"))
}

struct HelperFunctions<'ctx> {
    object_get: inkwell::values::FunctionValue<'ctx>,
    object_set: inkwell::values::FunctionValue<'ctx>,
    list_get: inkwell::values::FunctionValue<'ctx>,
    list_set: inkwell::values::FunctionValue<'ctx>,
    list_append: inkwell::values::FunctionValue<'ctx>,
    direct_call: inkwell::values::FunctionValue<'ctx>,
}

impl<'ctx> HelperFunctions<'ctx> {
    fn declare(context: &'ctx Context, module: &inkwell::module::Module<'ctx>) -> Self {
        Self {
            object_get: declare_helper(context, module, "wustite_v2_object_get", 2),
            object_set: declare_helper(context, module, "wustite_v2_object_set", 3),
            list_get: declare_helper(context, module, "wustite_v2_list_get", 2),
            list_set: declare_helper(context, module, "wustite_v2_list_set", 3),
            list_append: declare_helper(context, module, "wustite_v2_list_append", 2),
            direct_call: declare_helper(context, module, "wustite_v2_direct_call", 3),
        }
    }
}

fn declare_helper<'ctx>(
    context: &'ctx Context,
    module: &inkwell::module::Module<'ctx>,
    name: &str,
    argument_count: usize,
) -> inkwell::values::FunctionValue<'ctx> {
    let pointer = context.ptr_type(AddressSpace::default());
    let mut arguments: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> = vec![pointer.into()];
    arguments.extend(
        (0..argument_count)
            .map(|_| inkwell::types::BasicMetadataTypeEnum::from(context.i64_type())),
    );
    module.add_function(name, context.i64_type().fn_type(&arguments, false), None)
}

fn lower_helper<'ctx>(
    context: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    frame: PointerValue<'ctx>,
    helper_context: PointerValue<'ctx>,
    helper: inkwell::values::FunctionValue<'ctx>,
    arguments: &[IntValue<'ctx>],
) -> Result<IntValue<'ctx>, NativeError> {
    let calls_pointer = byte_pointer(context, builder, frame, HELPER_CALLS_OFFSET, "helper_calls")?;
    let calls = builder
        .build_load(context.i64_type(), calls_pointer, "calls")
        .map_err(llvm_error)?
        .into_int_value();
    let calls = builder
        .build_int_add(calls, context.i64_type().const_int(1, false), "next_calls")
        .map_err(llvm_error)?;
    builder
        .build_store(calls_pointer, calls)
        .map_err(llvm_error)?;
    let mut call_arguments: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
        vec![helper_context.into()];
    call_arguments.extend(
        arguments
            .iter()
            .map(|value| inkwell::values::BasicMetadataValueEnum::from(*value)),
    );
    builder
        .build_call(helper, &call_arguments, "helper_result")
        .map_err(llvm_error)?
        .try_as_basic_value()
        .basic()
        .map(BasicValueEnum::into_int_value)
        .ok_or_else(|| NativeError::Backend("LLVM helper result".to_string()))
}

fn helper_values<'ctx>(
    values: &BTreeMap<ValueId, IntValue<'ctx>>,
    ids: &[ValueId],
) -> Result<Vec<IntValue<'ctx>>, NativeError> {
    ids.iter().map(|id| value(values, *id)).collect()
}

fn map_helpers(
    module: &inkwell::module::Module<'_>,
    engine: &inkwell::execution_engine::ExecutionEngine<'_>,
) {
    let helpers = [
        (
            "wustite_v2_object_get",
            super::helpers::object_get as *const (),
        ),
        (
            "wustite_v2_object_set",
            super::helpers::object_set as *const (),
        ),
        ("wustite_v2_list_get", super::helpers::list_get as *const ()),
        ("wustite_v2_list_set", super::helpers::list_set as *const ()),
        (
            "wustite_v2_list_append",
            super::helpers::list_append as *const (),
        ),
        (
            "wustite_v2_direct_call",
            super::helpers::direct_call as *const (),
        ),
    ];
    for (name, address) in helpers {
        if let Some(function) = module.get_function(name) {
            engine.add_global_mapping(&function, address.expose_provenance());
        }
    }
}
