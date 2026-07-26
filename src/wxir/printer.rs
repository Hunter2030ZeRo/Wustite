use super::ir::{
    WxBinaryOp, WxBlockTarget, WxCompareOp, WxConstant, WxFloatBinaryOp, WxFloatCompareOp,
    WxFunction, WxGuardMode, WxInst, WxInstKind, WxIntBinaryOp, WxIntCompareOp, WxIntOverflowOp,
    WxTerminator, WxValueId,
};

/// Renders a compact, human-readable WXIR listing for diagnostics and tests.
pub fn print_function(function: &WxFunction) -> String {
    let mut output = format!(
        "wxir region{} [wvm {}..={}] entry {}\n",
        function.origin.region_id.0,
        function.origin.bytecode_header,
        function.origin.bytecode_backedge,
        function.entry
    );

    for block in &function.blocks {
        let parameters = block
            .parameters
            .iter()
            .map(|parameter| format!("{}: {}", parameter.id, parameter.ty))
            .collect::<Vec<_>>()
            .join(", ");
        output.push_str(&format!("{}({parameters}):\n", block.id));

        for instruction in &block.instructions {
            output.push_str("  ");
            output.push_str(&print_instruction(instruction));
            output.push('\n');
        }

        output.push_str("  ");
        output.push_str(&print_terminator(&block.terminator));
        output.push('\n');
    }

    for side_exit in &function.side_exits {
        let state = side_exit
            .state
            .iter()
            .map(|value| format!("r{} = {}: {}", value.register, value.value, value.ty))
            .collect::<Vec<_>>()
            .join(", ");
        output.push_str(&format!(
            "side_exit {} resume_pc={} [{state}]\n",
            side_exit.id, side_exit.resume_pc
        ));
    }

    output
}

fn print_instruction(instruction: &WxInst) -> String {
    let results = instruction
        .results
        .iter()
        .map(|result| format!("{}: {}", result.id, result.ty))
        .collect::<Vec<_>>()
        .join(", ");
    let prefix = if results.is_empty() {
        String::new()
    } else {
        format!("{results} = ")
    };

    let operation = match &instruction.kind {
        WxInstKind::Constant(constant) => format!("const {}", print_constant(*constant)),
        WxInstKind::Binary { op, lhs, rhs } => {
            format!("{} {lhs}, {rhs}", print_binary_op(*op))
        }
        WxInstKind::IntegerBinaryWithOverflow { op, lhs, rhs } => {
            let operation = match op {
                WxIntOverflowOp::Add => "iadd.with_overflow",
            };
            format!("{operation} {lhs}, {rhs}")
        }
        WxInstKind::Compare { op, lhs, rhs } => {
            format!("{} {lhs}, {rhs}", print_compare_op(*op))
        }
        WxInstKind::Cast { op, value } => format!("cast.{op:?} {value}"),
        WxInstKind::Load { address } => format!("load {address}"),
        WxInstKind::Store { address, value } => format!("store {address}, {value}"),
        WxInstKind::PointerOffset { base, offset } => {
            format!("ptr_offset {base}, {offset}")
        }
        WxInstKind::Splat { value } => format!("splat {value}"),
        WxInstKind::ExtractLane { vector, lane } => {
            format!("extract_lane {vector}, {lane}")
        }
        WxInstKind::InsertLane {
            vector,
            lane,
            value,
        } => format!("insert_lane {vector}, {lane}, {value}"),
        WxInstKind::Shuffle { left, right, lanes } => {
            format!("shuffle {left}, {right}, {lanes:?}")
        }
        WxInstKind::Guard {
            condition,
            exit,
            mode,
        } => {
            let mode = match mode {
                WxGuardMode::ExitWhenTrue => "exit_when_true",
                WxGuardMode::ExitWhenFalse => "exit_when_false",
            };
            format!("guard.{mode} {condition}, {exit}")
        }
        WxInstKind::Call {
            callee, arguments, ..
        } => format!("call {callee}({})", print_values(arguments)),
    };

    format!("{prefix}{operation}")
}

fn print_terminator(terminator: &WxTerminator) -> String {
    match terminator {
        WxTerminator::Jump { target, arguments } => {
            format!("jump {}({})", target, print_values(arguments))
        }
        WxTerminator::Branch { condition, yes, no } => {
            format!(
                "branch {condition}, {}, {}",
                print_target(yes),
                print_target(no)
            )
        }
        WxTerminator::Return { values } => format!("return {}", print_values(values)),
        WxTerminator::SideExit { exit, values } => {
            format!("side_exit {exit}({})", print_values(values))
        }
    }
}

fn print_target(target: &WxBlockTarget) -> String {
    format!("{}({})", target.block, print_values(&target.arguments))
}

fn print_values(values: &[WxValueId]) -> String {
    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_constant(constant: WxConstant) -> String {
    match constant {
        WxConstant::Bool(value) => value.to_string(),
        WxConstant::Int(value) => value.to_string(),
        WxConstant::F32(value) => format!("{value}f32"),
        WxConstant::F64(value) => format!("{value}f64"),
        WxConstant::NullPtr => "null".to_string(),
    }
}

fn print_binary_op(op: WxBinaryOp) -> &'static str {
    match op {
        WxBinaryOp::Integer(WxIntBinaryOp::Add) => "iadd.wrapping",
        WxBinaryOp::Integer(WxIntBinaryOp::Sub) => "isub.wrapping",
        WxBinaryOp::Integer(WxIntBinaryOp::Mul) => "imul.wrapping",
        WxBinaryOp::Integer(WxIntBinaryOp::And) => "iand",
        WxBinaryOp::Integer(WxIntBinaryOp::Or) => "ior",
        WxBinaryOp::Integer(WxIntBinaryOp::Xor) => "ixor",
        WxBinaryOp::Float(WxFloatBinaryOp::Add) => "fadd",
        WxBinaryOp::Float(WxFloatBinaryOp::Sub) => "fsub",
        WxBinaryOp::Float(WxFloatBinaryOp::Mul) => "fmul",
        WxBinaryOp::Float(WxFloatBinaryOp::Div) => "fdiv",
    }
}

fn print_compare_op(op: WxCompareOp) -> &'static str {
    match op {
        WxCompareOp::Integer(WxIntCompareOp::Eq) => "icmp.eq",
        WxCompareOp::Integer(WxIntCompareOp::Ne) => "icmp.ne",
        WxCompareOp::Integer(WxIntCompareOp::SignedLt) => "icmp.slt",
        WxCompareOp::Integer(WxIntCompareOp::SignedLe) => "icmp.sle",
        WxCompareOp::Integer(WxIntCompareOp::UnsignedLt) => "icmp.ult",
        WxCompareOp::Integer(WxIntCompareOp::UnsignedLe) => "icmp.ule",
        WxCompareOp::Float(WxFloatCompareOp::Eq) => "fcmp.eq",
        WxCompareOp::Float(WxFloatCompareOp::Ne) => "fcmp.ne",
        WxCompareOp::Float(WxFloatCompareOp::Lt) => "fcmp.lt",
        WxCompareOp::Float(WxFloatCompareOp::Le) => "fcmp.le",
        WxCompareOp::Float(WxFloatCompareOp::Gt) => "fcmp.gt",
        WxCompareOp::Float(WxFloatCompareOp::Ge) => "fcmp.ge",
    }
}
