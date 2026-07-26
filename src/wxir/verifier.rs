use std::collections::{HashMap, HashSet};

use super::ir::{
    WxBinaryOp, WxBlock, WxBlockId, WxBlockTarget, WxCastOp, WxCompareOp, WxConstant, WxExitId,
    WxFunction, WxGuardMode, WxInst, WxInstKind, WxSideExit, WxTerminator, WxValueId,
};
use super::types::{WxScalarType, WxType};

/// Verifies SSA identity, typing, control-flow edges, and side-exit metadata.
pub fn verify(function: &WxFunction) -> Result<(), String> {
    let mut blocks = HashMap::new();
    for block in &function.blocks {
        if blocks.insert(block.id, block).is_some() {
            return Err(format!("duplicate block ID {}", block.id));
        }
    }
    let entry = blocks
        .get(&function.entry)
        .ok_or_else(|| format!("entry block {} does not exist", function.entry))?;
    verify_entry_state(function, entry)?;

    for ty in &function.returns {
        verify_type(*ty)?;
    }

    let mut value_types = HashMap::new();
    for block in &function.blocks {
        for parameter in &block.parameters {
            verify_type(parameter.ty)?;
            define_value(&mut value_types, parameter.id, parameter.ty)?;
        }
        for instruction in &block.instructions {
            for result in &instruction.results {
                verify_type(result.ty)?;
                define_value(&mut value_types, result.id, result.ty)?;
            }
        }
    }

    let mut side_exits = HashMap::new();
    for side_exit in &function.side_exits {
        if side_exits.insert(side_exit.id, side_exit).is_some() {
            return Err(format!("duplicate side-exit metadata {}", side_exit.id));
        }

        let mut registers = HashSet::new();
        for state in &side_exit.state {
            verify_type(state.ty)?;
            if !registers.insert(state.register) {
                return Err(format!(
                    "side exit {} contains duplicate WVM register r{}",
                    side_exit.id, state.register
                ));
            }
        }
    }

    let mut used_exits = HashSet::new();
    for block in &function.blocks {
        verify_block(
            block,
            &blocks,
            &side_exits,
            &function.returns,
            &mut used_exits,
        )?;
    }

    for exit_id in side_exits.keys() {
        if !used_exits.contains(exit_id) {
            return Err(format!("side-exit metadata {exit_id} is never referenced"));
        }
    }

    Ok(())
}

fn verify_entry_state(function: &WxFunction, entry: &WxBlock) -> Result<(), String> {
    if function.entry_state.len() != entry.parameters.len() {
        return Err(format!(
            "entry state count {} does not match entry parameter count {}",
            function.entry_state.len(),
            entry.parameters.len()
        ));
    }

    let mut registers = HashSet::new();
    let mut values = HashSet::new();
    for state in &function.entry_state {
        if !registers.insert(state.register) {
            return Err(format!(
                "entry state contains duplicate WVM register r{}",
                state.register
            ));
        }
        if !values.insert(state.value) {
            return Err(format!(
                "entry state contains duplicate value {}",
                state.value
            ));
        }
        let parameter = entry
            .parameters
            .iter()
            .find(|parameter| parameter.id == state.value)
            .ok_or_else(|| {
                format!(
                    "entry state value {} is not an entry block parameter",
                    state.value
                )
            })?;
        if parameter.ty != state.ty {
            return Err(format!(
                "entry state value {} has type {}, expected {}",
                state.value, state.ty, parameter.ty
            ));
        }
    }
    Ok(())
}

fn verify_block(
    block: &WxBlock,
    blocks: &HashMap<WxBlockId, &WxBlock>,
    side_exits: &HashMap<WxExitId, &WxSideExit>,
    returns: &[WxType],
    used_exits: &mut HashSet<WxExitId>,
) -> Result<(), String> {
    let mut available = HashMap::new();
    for parameter in &block.parameters {
        available.insert(parameter.id, parameter.ty);
    }

    for instruction in &block.instructions {
        verify_instruction(instruction, &available, side_exits, used_exits)?;
        for result in &instruction.results {
            available.insert(result.id, result.ty);
        }
    }

    match &block.terminator {
        WxTerminator::Jump { target, arguments } => {
            verify_target(*target, arguments, &available, blocks)?;
        }
        WxTerminator::Branch { condition, yes, no } => {
            expect_type(&available, *condition, WxType::Scalar(WxScalarType::I1))?;
            verify_block_target(yes, &available, blocks)?;
            verify_block_target(no, &available, blocks)?;
        }
        WxTerminator::Return { values } => {
            verify_values(values, returns, &available, "return")?;
        }
        WxTerminator::SideExit { exit, values } => {
            let expected = verify_exit_use(*exit, &available, side_exits, used_exits)?;
            if values != &expected {
                return Err(format!(
                    "side exit {exit} values do not match its state metadata"
                ));
            }
        }
    }

    Ok(())
}

fn verify_instruction(
    instruction: &WxInst,
    available: &HashMap<WxValueId, WxType>,
    side_exits: &HashMap<WxExitId, &WxSideExit>,
    used_exits: &mut HashSet<WxExitId>,
) -> Result<(), String> {
    match &instruction.kind {
        WxInstKind::Constant(constant) => {
            let result = one_result(instruction)?;
            match constant {
                WxConstant::Bool(_) if result.ty == WxType::Scalar(WxScalarType::I1) => {}
                WxConstant::Int(_) if is_scalar_integer(result.ty) => {}
                WxConstant::F32(_) if result.ty == WxType::Scalar(WxScalarType::F32) => {}
                WxConstant::F64(_) if result.ty == WxType::Scalar(WxScalarType::F64) => {}
                WxConstant::NullPtr if result.ty == WxType::Scalar(WxScalarType::Ptr) => {}
                _ => return Err(format!("constant type does not match {}", result.ty)),
            }
        }
        WxInstKind::Binary { op, lhs, rhs } => {
            let result = one_result(instruction)?;
            let lhs_ty = value_type(available, *lhs)?;
            let rhs_ty = value_type(available, *rhs)?;
            if lhs_ty != rhs_ty || result.ty != lhs_ty {
                return Err("binary operands and result must have the same type".to_string());
            }
            match op {
                WxBinaryOp::Integer(_) if lhs_ty.is_integer() => {}
                WxBinaryOp::Float(_) if lhs_ty.is_float() => {}
                _ => return Err(format!("binary operation is incompatible with {lhs_ty}")),
            }
        }
        WxInstKind::IntegerBinaryWithOverflow { lhs, rhs, .. } => {
            let [value_result, overflow_result] = two_results(instruction)?;
            let lhs_ty = value_type(available, *lhs)?;
            let rhs_ty = value_type(available, *rhs)?;
            if lhs_ty != rhs_ty || !is_scalar_integer(lhs_ty) {
                return Err(
                    "checked integer operands must have the same scalar integer type".to_string(),
                );
            }
            if value_result.ty != lhs_ty || overflow_result.ty != WxType::Scalar(WxScalarType::I1) {
                return Err(
                    "checked integer results must be the operand type followed by i1".to_string(),
                );
            }
        }
        WxInstKind::Compare { op, lhs, rhs } => {
            let result = one_result(instruction)?;
            let lhs_ty = value_type(available, *lhs)?;
            let rhs_ty = value_type(available, *rhs)?;
            if lhs_ty != rhs_ty {
                return Err("comparison operands must have the same type".to_string());
            }
            match op {
                WxCompareOp::Integer(_) if lhs_ty.is_integer() => {}
                WxCompareOp::Float(_) if lhs_ty.is_float() => {}
                _ => return Err(format!("comparison is incompatible with {lhs_ty}")),
            }
            if result.ty != comparison_result_type(lhs_ty) {
                return Err(format!("comparison result has invalid type {}", result.ty));
            }
        }
        WxInstKind::Cast { op, value } => {
            let result = one_result(instruction)?;
            verify_cast(*op, value_type(available, *value)?, result.ty)?;
        }
        WxInstKind::Load { address } => {
            one_result(instruction)?;
            expect_type(available, *address, WxType::Scalar(WxScalarType::Ptr))?;
        }
        WxInstKind::Store { address, value } => {
            no_results(instruction)?;
            expect_type(available, *address, WxType::Scalar(WxScalarType::Ptr))?;
            value_type(available, *value)?;
        }
        WxInstKind::PointerOffset { base, offset } => {
            let result = one_result(instruction)?;
            expect_type(available, *base, WxType::Scalar(WxScalarType::Ptr))?;
            let offset_ty = value_type(available, *offset)?;
            if !matches!(
                offset_ty,
                WxType::Scalar(
                    WxScalarType::I8 | WxScalarType::I16 | WxScalarType::I32 | WxScalarType::I64
                )
            ) {
                return Err("pointer offset must be a scalar integer".to_string());
            }
            if !result.ty.is_pointer() {
                return Err("pointer offset result must be Ptr".to_string());
            }
        }
        WxInstKind::Splat { value } => {
            let result = one_result(instruction)?;
            let value_ty = value_type(available, *value)?;
            match result.ty {
                WxType::Vector { lane, .. } if value_ty == WxType::Scalar(lane) => {}
                _ => return Err("splat result must be a vector of the input scalar".to_string()),
            }
        }
        WxInstKind::ExtractLane { vector, lane } => {
            let result = one_result(instruction)?;
            match value_type(available, *vector)? {
                WxType::Vector {
                    lane: vector_lane,
                    lanes,
                } if *lane < lanes && result.ty == WxType::Scalar(vector_lane) => {}
                _ => return Err("invalid extract-lane types or lane index".to_string()),
            }
        }
        WxInstKind::InsertLane {
            vector,
            lane,
            value,
        } => {
            let result = one_result(instruction)?;
            let vector_ty = value_type(available, *vector)?;
            match vector_ty {
                WxType::Vector {
                    lane: vector_lane,
                    lanes,
                } if *lane < lanes
                    && value_type(available, *value)? == WxType::Scalar(vector_lane)
                    && result.ty == vector_ty => {}
                _ => return Err("invalid insert-lane types or lane index".to_string()),
            }
        }
        WxInstKind::Shuffle { left, right, lanes } => {
            let result = one_result(instruction)?;
            let input_ty = value_type(available, *left)?;
            if value_type(available, *right)? != input_ty {
                return Err("shuffle inputs must have the same vector type".to_string());
            }
            match (input_ty, result.ty) {
                (
                    WxType::Vector {
                        lane: input_lane,
                        lanes: input_lanes,
                    },
                    WxType::Vector {
                        lane: result_lane,
                        lanes: result_lanes,
                    },
                ) if input_lane == result_lane
                    && usize::from(result_lanes) == lanes.len()
                    && lanes
                        .iter()
                        .all(|lane| u32::from(*lane) < u32::from(input_lanes) * 2) => {}
                _ => return Err("invalid shuffle result or lane selection".to_string()),
            }
        }
        WxInstKind::Guard {
            condition,
            exit,
            mode,
        } => {
            no_results(instruction)?;
            expect_type(available, *condition, WxType::Scalar(WxScalarType::I1))?;
            match mode {
                WxGuardMode::ExitWhenTrue | WxGuardMode::ExitWhenFalse => {}
            }
            verify_exit_use(*exit, available, side_exits, used_exits)?;
        }
        WxInstKind::Call {
            callee,
            arguments,
            parameter_types,
        } => {
            if callee.is_empty() {
                return Err("call target cannot be empty".to_string());
            }
            for ty in parameter_types {
                verify_type(*ty)?;
            }
            verify_values(arguments, parameter_types, available, "call")?;
        }
    }

    Ok(())
}

fn verify_block_target(
    target: &WxBlockTarget,
    available: &HashMap<WxValueId, WxType>,
    blocks: &HashMap<WxBlockId, &WxBlock>,
) -> Result<(), String> {
    verify_target(target.block, &target.arguments, available, blocks)
}

fn verify_target(
    target: WxBlockId,
    arguments: &[WxValueId],
    available: &HashMap<WxValueId, WxType>,
    blocks: &HashMap<WxBlockId, &WxBlock>,
) -> Result<(), String> {
    let block = blocks
        .get(&target)
        .ok_or_else(|| format!("target block {target} does not exist"))?;
    let parameter_types: Vec<_> = block.parameters.iter().map(|param| param.ty).collect();
    verify_values(
        arguments,
        &parameter_types,
        available,
        &format!("edge to {target}"),
    )
}

fn verify_values(
    values: &[WxValueId],
    expected_types: &[WxType],
    available: &HashMap<WxValueId, WxType>,
    context: &str,
) -> Result<(), String> {
    if values.len() != expected_types.len() {
        return Err(format!(
            "{context} argument count {} does not match parameter count {}",
            values.len(),
            expected_types.len()
        ));
    }
    for (index, (value, expected)) in values.iter().zip(expected_types).enumerate() {
        let actual = value_type(available, *value)?;
        if actual != *expected {
            return Err(format!(
                "{context} argument {index} has type {actual}, expected {expected}"
            ));
        }
    }
    Ok(())
}

fn verify_exit_use(
    exit: WxExitId,
    available: &HashMap<WxValueId, WxType>,
    side_exits: &HashMap<WxExitId, &WxSideExit>,
    used_exits: &mut HashSet<WxExitId>,
) -> Result<Vec<WxValueId>, String> {
    let metadata = side_exits
        .get(&exit)
        .ok_or_else(|| format!("side exit {exit} has no metadata"))?;
    if !used_exits.insert(exit) {
        return Err(format!("side exit {exit} is referenced more than once"));
    }

    let mut values = Vec::with_capacity(metadata.state.len());
    for state in &metadata.state {
        expect_type(available, state.value, state.ty)?;
        values.push(state.value);
    }
    Ok(values)
}

fn verify_cast(op: WxCastOp, from: WxType, to: WxType) -> Result<(), String> {
    let valid = match op {
        WxCastOp::ZeroExtend | WxCastOp::SignExtend => {
            matches!(
                (scalar_integer_bits(from), scalar_integer_bits(to)),
                (Some(from_bits), Some(to_bits)) if from_bits < to_bits
            )
        }
        WxCastOp::Truncate => matches!(
            (scalar_integer_bits(from), scalar_integer_bits(to)),
            (Some(from_bits), Some(to_bits)) if from_bits > to_bits
        ),
        WxCastOp::IntToFloat { .. } => {
            scalar_integer_bits(from).is_some() && scalar_float_bits(to).is_some()
        }
        WxCastOp::FloatToInt { .. } => {
            scalar_float_bits(from).is_some() && scalar_integer_bits(to).is_some()
        }
        WxCastOp::FloatPromote => {
            from == WxType::Scalar(WxScalarType::F32) && to == WxType::Scalar(WxScalarType::F64)
        }
        WxCastOp::FloatDemote => {
            from == WxType::Scalar(WxScalarType::F64) && to == WxType::Scalar(WxScalarType::F32)
        }
        WxCastOp::PtrToInt => from.is_pointer() && scalar_integer_bits(to).is_some(),
        WxCastOp::IntToPtr => scalar_integer_bits(from).is_some() && to.is_pointer(),
        WxCastOp::Bitcast => {
            scalar_bit_width(from).is_some() && scalar_bit_width(from) == scalar_bit_width(to)
        }
    };
    if valid {
        Ok(())
    } else {
        Err(format!("invalid {op:?} cast from {from} to {to}"))
    }
}

fn verify_type(ty: WxType) -> Result<(), String> {
    match ty {
        WxType::Vector { lanes: 0, .. } => Err("vector lane count must be non-zero".to_string()),
        // I1 vectors are valid mask values. Pointer vectors remain invalid until
        // WXIR has a target-independent pointer-lane ABI and provenance model.
        WxType::Vector {
            lane: WxScalarType::Ptr,
            ..
        } => Err("Ptr vector lanes are not supported".to_string()),
        _ => Ok(()),
    }
}

fn define_value(
    values: &mut HashMap<WxValueId, WxType>,
    id: WxValueId,
    ty: WxType,
) -> Result<(), String> {
    if values.insert(id, ty).is_some() {
        Err(format!("value {id} is defined more than once"))
    } else {
        Ok(())
    }
}

fn value_type(available: &HashMap<WxValueId, WxType>, value: WxValueId) -> Result<WxType, String> {
    available
        .get(&value)
        .copied()
        .ok_or_else(|| format!("value {value} is used before it is defined"))
}

fn expect_type(
    available: &HashMap<WxValueId, WxType>,
    value: WxValueId,
    expected: WxType,
) -> Result<(), String> {
    let actual = value_type(available, value)?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "value {value} has type {actual}, expected {expected}"
        ))
    }
}

fn one_result(instruction: &WxInst) -> Result<super::ir::WxInstResult, String> {
    match instruction.results.as_slice() {
        [result] => Ok(*result),
        results => Err(format!(
            "instruction requires one result, found {}",
            results.len()
        )),
    }
}

fn two_results(instruction: &WxInst) -> Result<[super::ir::WxInstResult; 2], String> {
    match instruction.results.as_slice() {
        [value, overflow] => Ok([*value, *overflow]),
        results => Err(format!(
            "checked integer instruction requires two results, found {}",
            results.len()
        )),
    }
}

fn no_results(instruction: &WxInst) -> Result<(), String> {
    if instruction.results.is_empty() {
        Ok(())
    } else {
        Err("instruction must not define results".to_string())
    }
}

fn comparison_result_type(operand: WxType) -> WxType {
    match operand {
        WxType::Scalar(_) => WxType::Scalar(WxScalarType::I1),
        WxType::Vector { lanes, .. } => WxType::Vector {
            lane: WxScalarType::I1,
            lanes,
        },
    }
}

fn is_scalar_integer(ty: WxType) -> bool {
    scalar_integer_bits(ty).is_some()
}

fn scalar_integer_bits(ty: WxType) -> Option<u16> {
    match ty {
        WxType::Scalar(WxScalarType::I1) => Some(1),
        WxType::Scalar(WxScalarType::I8) => Some(8),
        WxType::Scalar(WxScalarType::I16) => Some(16),
        WxType::Scalar(WxScalarType::I32) => Some(32),
        WxType::Scalar(WxScalarType::I64) => Some(64),
        _ => None,
    }
}

fn scalar_float_bits(ty: WxType) -> Option<u16> {
    match ty {
        WxType::Scalar(WxScalarType::F32) => Some(32),
        WxType::Scalar(WxScalarType::F64) => Some(64),
        _ => None,
    }
}

fn scalar_bit_width(ty: WxType) -> Option<u16> {
    scalar_integer_bits(ty).or_else(|| scalar_float_bits(ty))
}
