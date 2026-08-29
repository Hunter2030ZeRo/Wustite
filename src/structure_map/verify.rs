use crate::bytecode::{Instruction, Register};

use super::{
    ControlDependency, Fact, IdentityFact, StructureMap, ValueComposition, ValueFact, ValueId,
    ValueOrigin, ValueUse,
};

impl StructureMap {
    pub(crate) fn verify_analysis(&self, code: &[Instruction]) -> Result<(), String> {
        if self.instructions.is_empty() && self.values.is_empty() {
            return Ok(());
        }
        if self.instructions.len() != code.len() {
            return Err(format!(
                "StructureMap has {} instruction facts for {} bytecode instructions",
                self.instructions.len(),
                code.len()
            ));
        }

        for (index, value) in self.values.iter().enumerate() {
            if value.id.0 as usize != index {
                return Err(format!(
                    "value fact {index} has mismatched id {}",
                    value.id.0
                ));
            }
            self.verify_value(value, code)?;
        }
        for (pc, (instruction, fact)) in code.iter().zip(&self.instructions).enumerate() {
            if fact.pc != pc {
                return Err(format!(
                    "instruction fact {pc} claims to describe pc {}",
                    fact.pc
                ));
            }
            for input in &fact.inputs {
                self.verify_use(*input, &format!("instruction fact at pc {pc} input"))?;
            }
            for id in fact.mutated_values.candidate().into_iter().flatten() {
                self.require_value(*id, &format!("instruction fact at pc {pc} mutation"))?;
            }
            self.verify_output(pc, instruction, fact.output)?;
            for dependency in &fact.control_dependencies {
                self.verify_control_dependency(pc, dependency, code)?;
            }
        }
        Ok(())
    }

    fn verify_value(&self, value: &ValueFact, code: &[Instruction]) -> Result<(), String> {
        if let Some(pc) = value.defined_at.filter(|pc| *pc >= code.len()) {
            return Err(format!(
                "value {} is defined outside bytecode at pc {pc}",
                value.id.0
            ));
        }
        if let Fact::Proven(IdentityFact::AliasOf(source))
        | Fact::Guardable(IdentityFact::AliasOf(source)) = value.identity
        {
            self.require_value(source, &format!("value {} identity", value.id.0))?;
            if self.identity_root(value.id).is_none() {
                return Err(format!("value {} has a cyclic identity chain", value.id.0));
            }
        }
        if let Some(origin) = value.origin.candidate() {
            match origin {
                ValueOrigin::Projection { aggregate, .. } => {
                    self.verify_use(*aggregate, &format!("value {} projection", value.id.0))?;
                }
                ValueOrigin::Call { callable, .. } => {
                    self.verify_use(*callable, &format!("value {} call", value.id.0))?;
                }
                ValueOrigin::Alias { source, .. } => {
                    self.verify_use(*source, &format!("value {} alias", value.id.0))?;
                }
                ValueOrigin::Parameter { .. }
                | ValueOrigin::Immediate { .. }
                | ValueOrigin::ConstantPool { .. }
                | ValueOrigin::CurrentFunction { .. }
                | ValueOrigin::Allocation { .. }
                | ValueOrigin::Operation { .. }
                | ValueOrigin::Unknown { .. } => {}
            }
        }
        if let Some(composition) = value.composition.candidate() {
            match composition {
                ValueComposition::Sequence(items) => {
                    for item in items {
                        self.verify_use(*item, &format!("value {} member", value.id.0))?;
                    }
                }
                ValueComposition::Mapping(entries) => {
                    for (key, item) in entries {
                        self.verify_use(*key, &format!("value {} key", value.id.0))?;
                        self.verify_use(*item, &format!("value {} member", value.id.0))?;
                    }
                }
                ValueComposition::None => {}
            }
        }
        Ok(())
    }

    fn verify_output(
        &self,
        pc: usize,
        instruction: &Instruction,
        output: Option<ValueId>,
    ) -> Result<(), String> {
        let expected = output_register(instruction);
        match (expected, output) {
            (Some(register), Some(id)) => {
                let value =
                    self.require_value(id, &format!("instruction fact at pc {pc} output"))?;
                if value.register != register || value.defined_at != Some(pc) {
                    return Err(format!(
                        "instruction fact at pc {pc} output does not match r{register} defined there"
                    ));
                }
            }
            (None, None) => {}
            (Some(register), None) => {
                return Err(format!(
                    "instruction fact at pc {pc} is missing output r{register}"
                ));
            }
            (None, Some(_)) => {
                return Err(format!(
                    "instruction fact at pc {pc} has an unexpected output"
                ));
            }
        }
        Ok(())
    }

    fn verify_control_dependency(
        &self,
        pc: usize,
        dependency: &ControlDependency,
        code: &[Instruction],
    ) -> Result<(), String> {
        let Some(Instruction::Branch { cond, .. }) = code.get(dependency.branch_pc) else {
            return Err(format!(
                "instruction fact at pc {pc} depends on non-branch pc {}",
                dependency.branch_pc
            ));
        };
        if dependency.condition.register != *cond {
            return Err(format!(
                "instruction fact at pc {pc} has the wrong condition for branch pc {}",
                dependency.branch_pc
            ));
        }
        self.verify_use(
            dependency.condition,
            &format!("instruction fact at pc {pc} control dependency"),
        )
    }

    fn verify_use(&self, value_use: ValueUse, context: &str) -> Result<(), String> {
        let Some(id) = value_use.value else {
            return Ok(());
        };
        let value = self.require_value(id, context)?;
        if value.register == value_use.register {
            Ok(())
        } else {
            Err(format!(
                "{context} says value {} belongs to r{}, not r{}",
                id.0, value_use.register, value.register
            ))
        }
    }

    fn require_value(&self, id: ValueId, context: &str) -> Result<&ValueFact, String> {
        self.value(id)
            .ok_or_else(|| format!("{context} references unknown output value {}", id.0))
    }
}

fn output_register(instruction: &Instruction) -> Option<Register> {
    match instruction {
        Instruction::ConstSmallInt { dst, .. }
        | Instruction::ConstFloat { dst, .. }
        | Instruction::ConstBool { dst, .. }
        | Instruction::ConstNone { dst }
        | Instruction::LoadConstant { dst, .. }
        | Instruction::ConstI64 { dst, .. }
        | Instruction::LoadCurrentFunction { dst }
        | Instruction::BinaryOp { dst, .. }
        | Instruction::CompareOp { dst, .. }
        | Instruction::UnaryOp { dst, .. }
        | Instruction::BooleanOp { dst, .. }
        | Instruction::BuildTuple { dst, .. }
        | Instruction::BuildList { dst, .. }
        | Instruction::BuildDict { dst, .. }
        | Instruction::GetItem { dst, .. }
        | Instruction::GetAttr { dst, .. }
        | Instruction::GetSlice { dst, .. }
        | Instruction::ListPop { dst, .. }
        | Instruction::Length { dst, .. }
        | Instruction::Call { dst, .. }
        | Instruction::CallMethod { dst, .. }
        | Instruction::Move { dst, .. }
        | Instruction::AddI64 { dst, .. }
        | Instruction::LtI64 { dst, .. } => Some(*dst),
        Instruction::SetItem { .. }
        | Instruction::SetAttr { .. }
        | Instruction::SetSlice { .. }
        | Instruction::ListAppend { .. }
        | Instruction::ListInsert { .. }
        | Instruction::Jump { .. }
        | Instruction::Branch { .. }
        | Instruction::Return { .. } => None,
    }
}
