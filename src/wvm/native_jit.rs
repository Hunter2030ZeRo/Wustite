use crate::bytecode::Instruction;
use crate::executable::ExecutableFunction;
use crate::jit::NativeDispatch;
use crate::jit::NativeSequenceView;
use crate::object::SequenceStrategy;
use crate::value::Value;

use super::{FunctionRuntime, Vm};

pub(super) mod leaf;
mod sequence;

use leaf::execute_numeric_leaf_call;
use sequence::{execute_small_int_sequence_access, fuse_reverse_prefix};

pub(crate) use sequence::{match_reverse_prefix, temporary_is_dead};

pub(super) struct NativeDispatcher<'a> {
    vm: &'a mut Vm,
    executable: &'a ExecutableFunction,
    runtime: &'a mut FunctionRuntime,
    resume_pc: &'a mut usize,
    skipped_pc: Option<usize>,
}

impl<'a> NativeDispatcher<'a> {
    pub(super) const fn new(
        vm: &'a mut Vm,
        executable: &'a ExecutableFunction,
        runtime: &'a mut FunctionRuntime,
        resume_pc: &'a mut usize,
    ) -> Self {
        Self {
            vm,
            executable,
            runtime,
            resume_pc,
            skipped_pc: None,
        }
    }
}

impl NativeDispatch for NativeDispatcher<'_> {
    fn execute(&mut self, registers: &mut [Value], pc: usize) -> Result<(), String> {
        let instruction = self
            .executable
            .bytecode()
            .code
            .get(pc)
            .ok_or_else(|| format!("invalid native JIT instruction pc {pc}"))?;
        self.vm.jit_report.record_native_helper(instruction);
        *self.resume_pc = pc;
        if self.skipped_pc.take() == Some(pc) {
            *self.resume_pc = pc.saturating_add(1);
            return Ok(());
        }
        let fused_reverse = matches!(instruction, Instruction::GetSlice { .. })
            && fuse_reverse_prefix(
                &self.executable.bytecode().code,
                pc,
                &mut self.vm.object_heap,
                registers,
            )?;
        if fused_reverse {
            self.skipped_pc = Some(pc.saturating_add(1));
        } else {
            let leaf_call = matches!(instruction, Instruction::Call { .. })
                && execute_numeric_leaf_call(self.vm, self.runtime, registers, pc, instruction)?;
            let sequence_access = matches!(
                instruction,
                Instruction::GetItem { .. }
                    | Instruction::SetItem { .. }
                    | Instruction::ListInsert { .. }
                    | Instruction::ListPop { .. }
                    | Instruction::GetSlice { .. }
                    | Instruction::SetSlice { .. }
            ) && execute_small_int_sequence_access(
                &mut self.vm.object_heap,
                registers,
                instruction,
            )?;
            if !leaf_call && !sequence_access {
                self.vm.execute_runtime_instruction(
                    self.executable,
                    self.runtime,
                    registers,
                    pc,
                    instruction,
                )?;
            }
        }
        self.runtime
            .profile
            .observe_instruction(pc, instruction, registers, &self.vm.object_heap);
        *self.resume_pc = pc.saturating_add(1);
        Ok(())
    }

    fn execute_sequence(&mut self, registers: &mut [Value], pc: usize) -> Result<(), String> {
        let instruction = self
            .executable
            .bytecode()
            .code
            .get(pc)
            .ok_or_else(|| format!("invalid native sequence instruction pc {pc}"))?;
        self.vm.jit_report.record_native_helper(instruction);
        *self.resume_pc = pc;
        if self.skipped_pc.take() == Some(pc) {
            *self.resume_pc = pc.saturating_add(1);
            return Ok(());
        }
        let fused_reverse = matches!(instruction, Instruction::GetSlice { .. })
            && fuse_reverse_prefix(
                &self.executable.bytecode().code,
                pc,
                &mut self.vm.object_heap,
                registers,
            )?;
        if fused_reverse {
            self.skipped_pc = Some(pc.saturating_add(1));
        } else if !execute_small_int_sequence_access(
            &mut self.vm.object_heap,
            registers,
            instruction,
        )? {
            self.vm.execute_runtime_instruction(
                self.executable,
                self.runtime,
                registers,
                pc,
                instruction,
            )?;
        }
        self.runtime
            .profile
            .observe_instruction(pc, instruction, registers, &self.vm.object_heap);
        *self.resume_pc = pc.saturating_add(1);
        Ok(())
    }

    fn sequence_view(
        &mut self,
        registers: &[Value],
        register: u32,
        expected: SequenceStrategy,
    ) -> Result<NativeSequenceView, String> {
        let value = registers
            .get(register as usize)
            .copied()
            .ok_or_else(|| format!("native JIT read invalid register r{register}"))?;
        let Value::Object(reference) = value else {
            return Err(format!("native JIT read type mismatch in r{register}"));
        };
        let view = self
            .vm
            .object_heap
            .sequence_view(reference)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("native JIT read type mismatch in r{register}"))?;
        if view.strategy != expected {
            return Err(format!("native JIT read type mismatch in r{register}"));
        }
        Ok(NativeSequenceView {
            data: view.data,
            len: u64::try_from(view.len).map_err(|error| error.to_string())?,
            layout_version: view.layout_version,
            writable: u64::from(view.writable),
        })
    }
}
