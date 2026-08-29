use std::collections::BTreeMap;

use super::{Emitter, Node, VirtualObject, push_dependency, scalar};
use crate::adaptive_v2::wxir_v2::dependency::{Dependency, DependencyKind};
use crate::bytecode::Instruction as WvmInstruction;
use crate::executable::ExecutableFunction;

impl Emitter<'_> {
    pub(super) fn inline(
        &mut self,
        function: &ExecutableFunction,
        object: &mut VirtualObject,
        arguments: &[Node],
        call_pc: usize,
    ) -> Result<Option<Node>, String> {
        crate::verifier::verify(function)?;
        if function.parameters().len() != arguments.len().saturating_add(1) {
            return Ok(None);
        }
        push_dependency(
            self.dependencies,
            Dependency::current(
                DependencyKind::Callee,
                function.id().as_u64(),
                function.id().as_u64(),
            ),
        );
        let mut values = BTreeMap::new();
        values.insert(
            function.parameters()[0].register,
            Node::Object(object.clone()),
        );
        for (parameter, argument) in function.parameters()[1..].iter().zip(arguments) {
            values.insert(parameter.register, argument.clone());
        }
        for instruction in &function.bytecode().code {
            match instruction {
                WvmInstruction::GetAttr {
                    dst,
                    object: receiver,
                    name,
                } => {
                    let Some(Node::Object(receiver)) = values.get(receiver) else {
                        return Ok(None);
                    };
                    let Some(value) = receiver.fields.get(name).copied() else {
                        return Ok(None);
                    };
                    values.insert(*dst, Node::Scalar(value));
                }
                WvmInstruction::SetAttr {
                    object: receiver,
                    name,
                    value,
                } => {
                    let Some(stored) = scalar(&values, *value) else {
                        return Ok(None);
                    };
                    let Some(Node::Object(receiver)) = values.get_mut(receiver) else {
                        return Ok(None);
                    };
                    receiver.fields.insert(name.clone(), stored);
                }
                WvmInstruction::Return { src } => {
                    if let Some(Node::Object(receiver)) =
                        values.get(&function.parameters()[0].register)
                    {
                        *object = receiver.clone();
                    }
                    return Ok(values.get(src).cloned());
                }
                instruction => {
                    if !self.lower_instruction(instruction, call_pc, &mut values)? {
                        return Ok(None);
                    }
                }
            }
        }
        Ok(None)
    }
}
