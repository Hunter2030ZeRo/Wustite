use std::collections::BTreeMap;

use crate::adaptive_v2::wxir_v2::dependency::{Dependency, DependencyKind};
use crate::adaptive_v2::wxir_v2::ir::{
    Constant, Effect, Instruction, InstructionKind, ValueDef, ValueId, ValueType,
};
use crate::bytecode::{BinaryOperator, Instruction as WvmInstruction, Register};
use crate::executable::{ExecutableConstant, ExecutableFunction};

mod inline;

#[derive(Clone)]
pub(super) enum Node {
    Scalar(ValueDef),
    Class(usize),
    Object(VirtualObject),
    None,
}

#[derive(Clone)]
pub(super) struct VirtualObject {
    class: usize,
    fields: BTreeMap<String, ValueDef>,
}

pub(super) struct Emitter<'a> {
    executable: &'a ExecutableFunction,
    dependencies: &'a mut Vec<Dependency>,
    next_value: u32,
    instructions: Vec<Instruction>,
}

impl<'a> Emitter<'a> {
    pub(super) const fn new(
        executable: &'a ExecutableFunction,
        dependencies: &'a mut Vec<Dependency>,
    ) -> Self {
        Self {
            executable,
            dependencies,
            next_value: 0,
            instructions: Vec::new(),
        }
    }

    pub(super) fn next(&mut self, ty: ValueType) -> Result<ValueDef, String> {
        let id = self.next_value;
        self.next_value = self
            .next_value
            .checked_add(1)
            .ok_or_else(|| "macro trace value identifier overflow".to_owned())?;
        Ok(ValueDef::new(ValueId::new(id), ty))
    }

    pub(super) fn take_instructions(&mut self) -> Vec<Instruction> {
        std::mem::take(&mut self.instructions)
    }

    pub(super) fn lower_range(
        &mut self,
        code: &[WvmInstruction],
        start_pc: usize,
        values: &mut BTreeMap<Register, Node>,
    ) -> Result<bool, String> {
        for (offset, instruction) in code.iter().enumerate() {
            if !self.lower_instruction(instruction, start_pc.saturating_add(offset), values)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn lower_instruction(
        &mut self,
        instruction: &WvmInstruction,
        pc: usize,
        values: &mut BTreeMap<Register, Node>,
    ) -> Result<bool, String> {
        match instruction {
            WvmInstruction::ConstSmallInt { dst, value }
            | WvmInstruction::ConstI64 { dst, value } => {
                let output = self.emit(
                    InstructionKind::Constant(Constant::Integer(*value)),
                    Vec::new(),
                    ValueType::I64,
                    pc,
                )?;
                values.insert(*dst, Node::Scalar(output));
            }
            WvmInstruction::ConstNone { dst } => {
                values.insert(*dst, Node::None);
            }
            WvmInstruction::Move { dst, src } => {
                let Some(value) = values.get(src).cloned() else {
                    return Ok(false);
                };
                values.insert(*dst, value);
            }
            WvmInstruction::LoadConstant { dst, constant }
                if matches!(
                    self.executable.constants().get(constant.0),
                    Some(ExecutableConstant::Class(_))
                ) =>
            {
                values.insert(*dst, Node::Class(constant.0));
            }
            WvmInstruction::BinaryOp {
                dst, op, lhs, rhs, ..
            } => {
                let Some(left) = scalar(values, *lhs) else {
                    return Ok(false);
                };
                let Some(right) = scalar(values, *rhs) else {
                    return Ok(false);
                };
                if left.ty != ValueType::I64 || right.ty != ValueType::I64 {
                    return Ok(false);
                }
                let kind = match op {
                    BinaryOperator::Add => InstructionKind::IntegerAdd,
                    BinaryOperator::Subtract => InstructionKind::IntegerSubtract,
                    BinaryOperator::Multiply => InstructionKind::IntegerMultiply,
                    BinaryOperator::Divide
                    | BinaryOperator::FloorDivide
                    | BinaryOperator::Power => {
                        return Ok(false);
                    }
                };
                let output = self.emit(kind, vec![left.id, right.id], ValueType::I64, pc)?;
                values.insert(*dst, Node::Scalar(output));
            }
            WvmInstruction::CompareOp {
                dst,
                op: crate::bytecode::CompareOperator::Lt,
                lhs,
                rhs,
                ..
            } => {
                let Some(left) = scalar(values, *lhs) else {
                    return Ok(false);
                };
                let Some(right) = scalar(values, *rhs) else {
                    return Ok(false);
                };
                let output = self.emit(
                    InstructionKind::IntegerLessThan,
                    vec![left.id, right.id],
                    ValueType::Bool,
                    pc,
                )?;
                values.insert(*dst, Node::Scalar(output));
            }
            WvmInstruction::Call {
                dst,
                callable,
                args,
            } => {
                let Some(Node::Class(class)) = values.get(callable).cloned() else {
                    return Ok(false);
                };
                let Some(ExecutableConstant::Class(class_object)) =
                    self.executable.constants().get(class)
                else {
                    return Ok(false);
                };
                push_dependency(
                    self.dependencies,
                    Dependency::current(
                        DependencyKind::Class,
                        class_object.id().0,
                        class_object.id().0,
                    ),
                );
                let mut object = VirtualObject {
                    class,
                    fields: BTreeMap::new(),
                };
                if let Some(initializer) = class_object.method("__init__") {
                    let arguments = args
                        .iter()
                        .map(|register| values.get(register).cloned())
                        .collect::<Option<Vec<_>>>();
                    let Some(arguments) = arguments else {
                        return Ok(false);
                    };
                    self.inline(initializer, &mut object, &arguments, pc)?;
                }
                values.insert(*dst, Node::Object(object));
            }
            WvmInstruction::CallMethod {
                dst,
                receiver,
                name,
                args,
            } => {
                let Some(Node::Object(mut object)) = values.get(receiver).cloned() else {
                    return Ok(false);
                };
                let Some(ExecutableConstant::Class(class)) =
                    self.executable.constants().get(object.class)
                else {
                    return Ok(false);
                };
                let Some(method) = class.method(name) else {
                    return Ok(false);
                };
                let arguments = args
                    .iter()
                    .map(|register| values.get(register).cloned())
                    .collect::<Option<Vec<_>>>();
                let Some(arguments) = arguments else {
                    return Ok(false);
                };
                let Some(result) = self.inline(method, &mut object, &arguments, pc)? else {
                    return Ok(false);
                };
                values.insert(*receiver, Node::Object(object));
                values.insert(*dst, result);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn emit(
        &mut self,
        kind: InstructionKind,
        inputs: Vec<ValueId>,
        ty: ValueType,
        pc: usize,
    ) -> Result<ValueDef, String> {
        let output = self.next(ty)?;
        let pc = u32::try_from(pc).map_err(|_| "macro trace pc overflow".to_owned())?;
        self.instructions.push(Instruction::new(
            kind.at_pc(pc),
            inputs,
            Some(output),
            Effect::Pure,
        ));
        Ok(output)
    }
}

fn scalar(values: &BTreeMap<Register, Node>, register: Register) -> Option<ValueDef> {
    match values.get(&register) {
        Some(Node::Scalar(value)) => Some(*value),
        Some(Node::Class(_) | Node::Object(_) | Node::None) | None => None,
    }
}

fn push_dependency(dependencies: &mut Vec<Dependency>, dependency: Dependency) {
    if !dependencies.contains(&dependency) {
        dependencies.push(dependency);
    }
}
