use std::collections::{BTreeMap, BTreeSet};

use crate::adaptive_v2::trace::ExecutableIdentity;
use crate::adaptive_v2::wxir_v2::deopt::DeoptRecipe;
use crate::adaptive_v2::wxir_v2::dependency::Dependency;
use crate::adaptive_v2::wxir_v2::ir::{
    Block, BlockId, Effect, Instruction, RootMap, Terminator, ValueDef, ValueId, ValueType,
};
use crate::bytecode::{Instruction as WvmInstruction, Register};
use crate::executable::ExecutableFunction;

mod ops;
mod state;

const MAX_BLOCKS: u32 = 256;

#[derive(Clone, Copy)]
struct SsaValue {
    id: ValueId,
    ty: ValueType,
}

pub(super) struct LoweredEntry {
    pub(super) blocks: Vec<Block>,
    pub(super) root_maps: Vec<RootMap>,
    pub(super) deopts: Vec<DeoptRecipe>,
}

pub(super) fn lower(
    executable: &ExecutableFunction,
    parameter_types: &[(Register, ValueType)],
    guard_true: Option<Register>,
    identity: ExecutableIdentity,
    dependencies: &[Dependency],
) -> Result<LoweredEntry, String> {
    let mut builder = Builder::new(executable, identity, dependencies);
    let mut values = BTreeMap::new();
    let parameters = parameter_types
        .iter()
        .map(|(register, ty)| state::define(&mut builder, &mut values, *register, *ty))
        .collect::<Vec<_>>();
    let mut prefix = Vec::new();
    if let Some(register) = guard_true {
        state::add_true_guard(&mut builder, register, &values, &mut prefix)?;
    }
    builder.emit(
        BlockId::new(0),
        0,
        values,
        parameters,
        prefix,
        &mut BTreeSet::new(),
    )?;
    builder.blocks.sort_by_key(|block| block.id.get());
    Ok(LoweredEntry {
        blocks: builder.blocks,
        root_maps: builder.root_maps,
        deopts: builder.deopts,
    })
}

struct Builder<'a> {
    executable: &'a ExecutableFunction,
    identity: ExecutableIdentity,
    dependencies: &'a [Dependency],
    leaders: BTreeSet<usize>,
    next_value: u32,
    next_block: u32,
    blocks: Vec<Block>,
    root_maps: Vec<RootMap>,
    deopts: Vec<DeoptRecipe>,
}

impl<'a> Builder<'a> {
    fn new(
        executable: &'a ExecutableFunction,
        identity: ExecutableIdentity,
        dependencies: &'a [Dependency],
    ) -> Self {
        Self {
            executable,
            identity,
            dependencies,
            leaders: leaders(&executable.bytecode().code),
            next_value: 0,
            next_block: 1,
            blocks: Vec::new(),
            root_maps: Vec::new(),
            deopts: Vec::new(),
        }
    }

    fn emit(
        &mut self,
        id: BlockId,
        start: usize,
        mut values: BTreeMap<Register, SsaValue>,
        parameters: Vec<ValueDef>,
        mut instructions: Vec<Instruction>,
        active: &mut BTreeSet<usize>,
    ) -> Result<(), String> {
        if !active.insert(start) {
            return Err("adaptive-v2 entry contains a cycle; loop OSR owns cyclic CFG".to_owned());
        }
        let code = &self.executable.bytecode().code;
        let mut pc = start;
        let terminator = loop {
            if pc >= code.len() {
                return Err("adaptive-v2 entry path has no return".to_owned());
            }
            if pc != start && self.leaders.contains(&pc) {
                break Terminator::Jump {
                    target: self.child(pc, values.clone(), active)?,
                    arguments: Vec::new(),
                };
            }
            match &code[pc] {
                WvmInstruction::Return { src } => {
                    break Terminator::Return {
                        values: vec![state::read(&values, *src)?.id],
                    };
                }
                WvmInstruction::Jump { target } => {
                    break Terminator::Jump {
                        target: self.child(*target, values.clone(), active)?,
                        arguments: Vec::new(),
                    };
                }
                WvmInstruction::Branch { cond, yes, no } => {
                    let condition = state::read(&values, *cond)?;
                    if condition.ty != ValueType::Bool {
                        return Err("adaptive-v2 entry branch condition is not boolean".to_owned());
                    }
                    break Terminator::Branch {
                        condition: condition.id,
                        yes: self.child(*yes, values.clone(), active)?,
                        no: self.child(*no, values.clone(), active)?,
                    };
                }
                instruction => {
                    self.lower_instruction(pc, instruction, &mut values, &mut instructions)?;
                }
            }
            pc = pc.saturating_add(1);
        };
        active.remove(&start);
        self.blocks
            .push(Block::new(id, parameters, instructions, terminator));
        Ok(())
    }

    fn child(
        &mut self,
        target: usize,
        values: BTreeMap<Register, SsaValue>,
        active: &mut BTreeSet<usize>,
    ) -> Result<BlockId, String> {
        if self.next_block >= MAX_BLOCKS {
            return Err("adaptive-v2 entry CFG exceeds the conservative block limit".to_owned());
        }
        let id = BlockId::new(self.next_block);
        self.next_block = self.next_block.saturating_add(1);
        self.emit(id, target, values, Vec::new(), Vec::new(), active)?;
        Ok(id)
    }

    fn lower_instruction(
        &mut self,
        pc: usize,
        instruction: &WvmInstruction,
        values: &mut BTreeMap<Register, SsaValue>,
        lowered: &mut Vec<Instruction>,
    ) -> Result<(), String> {
        let lookup = |register| {
            values
                .get(&register)
                .copied()
                .map(|value| (value.id, value.ty))
                .ok_or_else(|| format!("adaptive-v2 entry reads undefined r{register}"))
        };
        let operation = ops::lower(instruction, lookup)
            .map_err(|error| format!("{error} at pc {pc} in adaptive-v2 entry"))?;
        let output = state::define(self, values, operation.dst, operation.ty);
        lowered.push(Instruction::new(
            operation.kind.at_pc(pc_u32(pc)?),
            operation.inputs,
            Some(output),
            Effect::Pure,
        ));
        Ok(())
    }
}

fn leaders(code: &[WvmInstruction]) -> BTreeSet<usize> {
    let mut leaders = BTreeSet::from([0]);
    for (pc, instruction) in code.iter().enumerate() {
        match instruction {
            WvmInstruction::Jump { target } => {
                leaders.insert(*target);
                leaders.insert(pc.saturating_add(1));
            }
            WvmInstruction::Branch { yes, no, .. } => {
                leaders.insert(*yes);
                leaders.insert(*no);
                leaders.insert(pc.saturating_add(1));
            }
            _ => {}
        }
    }
    leaders
}

fn pc_u32(pc: usize) -> Result<u32, String> {
    u32::try_from(pc).map_err(|_| "adaptive-v2 pc overflow".to_owned())
}
