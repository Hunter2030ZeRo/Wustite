use std::collections::{HashMap, HashSet, VecDeque};
use std::error::Error;
use std::fmt;

use crate::bytecode::{Instruction, Register};
use crate::executable::ExecutableFunction;
use crate::planner::JitPlan;
use crate::structure_map::{LiveSlot, SlotType};
use crate::verifier as wvm_verifier;

use super::ir::{
    WxBlock, WxBlockId, WxBlockParam, WxBlockTarget, WxCompareOp, WxExitId, WxFunction,
    WxGuardMode, WxInst, WxInstKind, WxInstResult, WxIntCompareOp, WxIntOverflowOp, WxRegionOrigin,
    WxSideExit, WxStateValue, WxTerminator, WxValueId,
};
use super::types::{WxScalarType, WxType};
use super::verifier as wxir_verifier;

/// A recoverable failure while translating WVM bytecode into WXIR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WxBuildError {
    InvalidExecutable(String),
    InvalidPlan(String),
    UnsupportedInstruction {
        pc: usize,
        instruction: &'static str,
    },
    MissingRegister {
        pc: usize,
        register: Register,
    },
    TypeMismatch {
        pc: usize,
        register: Register,
        expected: WxType,
        actual: WxType,
    },
    IdSpaceExhausted(&'static str),
    InvalidWxir(String),
}

impl fmt::Display for WxBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidExecutable(error) => write!(formatter, "invalid executable: {error}"),
            Self::InvalidPlan(error) => write!(formatter, "invalid JIT plan: {error}"),
            Self::UnsupportedInstruction { pc, instruction } => {
                write!(
                    formatter,
                    "unsupported WVM instruction {instruction} at pc {pc}"
                )
            }
            Self::MissingRegister { pc, register } => {
                write!(formatter, "r{register} has no WXIR value at pc {pc}")
            }
            Self::TypeMismatch {
                pc,
                register,
                expected,
                actual,
            } => write!(
                formatter,
                "r{register} has type {actual} at pc {pc}, expected {expected}"
            ),
            Self::IdSpaceExhausted(kind) => write!(formatter, "{kind} ID space exhausted"),
            Self::InvalidWxir(error) => write!(formatter, "generated invalid WXIR: {error}"),
        }
    }
}

impl Error for WxBuildError {}

/// Builds and verifies typed SSA for one planned WVM hot region.
pub fn build_region(
    executable: &ExecutableFunction,
    plan: &JitPlan,
) -> Result<WxFunction, WxBuildError> {
    wvm_verifier::verify(executable).map_err(WxBuildError::InvalidExecutable)?;
    verify_plan(executable, plan)?;

    let mut builder = RegionBuilder::new(executable, plan)?;
    let function = builder.build()?;
    wxir_verifier::verify(&function).map_err(WxBuildError::InvalidWxir)?;
    Ok(function)
}

fn verify_plan(executable: &ExecutableFunction, plan: &JitPlan) -> Result<(), WxBuildError> {
    let region = executable
        .structure_map
        .loops
        .get(plan.region_id.0)
        .ok_or_else(|| {
            WxBuildError::InvalidPlan(format!("unknown region ID {}", plan.region_id.0))
        })?;

    if region.header != plan.header
        || region.backedge != plan.backedge
        || region.exits != plan.exits
        || region.live_slots != plan.live_slots
    {
        return Err(WxBuildError::InvalidPlan(
            "plan metadata does not match its StructureMap region".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct TypedValue {
    id: WxValueId,
    ty: WxType,
}

#[derive(Debug, Clone)]
struct BlockSpec {
    id: WxBlockId,
    pc: usize,
    parameters: Vec<(Register, WxBlockParam)>,
}

#[derive(Debug, Clone)]
struct ExitBlockSpec {
    id: WxBlockId,
    exit: WxExitId,
    resume_pc: usize,
    parameters: Vec<(Register, WxBlockParam)>,
}

struct RegionBuilder<'a> {
    executable: &'a ExecutableFunction,
    plan: &'a JitPlan,
    leaders: HashSet<usize>,
    exit_by_pc: HashMap<usize, usize>,
    block_specs: HashMap<usize, BlockSpec>,
    exit_specs: HashMap<usize, ExitBlockSpec>,
    synthetic_exits: Vec<WxSideExit>,
    queue: VecDeque<usize>,
    built: HashSet<usize>,
    blocks: Vec<WxBlock>,
    next_value: u32,
    next_block: u32,
    next_exit: u32,
}

impl<'a> RegionBuilder<'a> {
    fn new(executable: &'a ExecutableFunction, plan: &'a JitPlan) -> Result<Self, WxBuildError> {
        let mut leaders = HashSet::from([plan.header]);
        let mut exit_by_pc = HashMap::new();

        for (index, exit) in plan.exits.iter().enumerate() {
            if exit_by_pc.insert(exit.target, index).is_some() {
                return Err(WxBuildError::InvalidPlan(format!(
                    "multiple exits resume at bytecode pc {}",
                    exit.target
                )));
            }
        }

        for pc in plan.header..=plan.backedge {
            match &executable.bytecode.code[pc] {
                Instruction::Jump { target } => {
                    if (plan.header..=plan.backedge).contains(target) {
                        leaders.insert(*target);
                    }
                }
                Instruction::Branch { yes, no, .. } => {
                    if (plan.header..=plan.backedge).contains(yes) {
                        leaders.insert(*yes);
                    }
                    if (plan.header..=plan.backedge).contains(no) {
                        leaders.insert(*no);
                    }
                }
                _ => {}
            }
        }

        let mut builder = Self {
            executable,
            plan,
            leaders,
            exit_by_pc,
            block_specs: HashMap::new(),
            exit_specs: HashMap::new(),
            synthetic_exits: Vec::new(),
            queue: VecDeque::new(),
            built: HashSet::new(),
            blocks: Vec::new(),
            next_value: 0,
            next_block: 0,
            next_exit: u32::try_from(plan.exits.len())
                .map_err(|_| WxBuildError::IdSpaceExhausted("side-exit"))?,
        };

        let entry_id = builder.allocate_block()?;
        let live_slots = plan.live_slots.clone();
        let parameters = builder.parameters_for_slots(&live_slots)?;
        builder.block_specs.insert(
            plan.header,
            BlockSpec {
                id: entry_id,
                pc: plan.header,
                parameters,
            },
        );
        builder.queue.push_back(plan.header);

        Ok(builder)
    }

    fn build(&mut self) -> Result<WxFunction, WxBuildError> {
        while let Some(pc) = self.queue.pop_front() {
            if self.built.insert(pc) {
                self.build_block(pc)?;
            }
        }

        if self.exit_specs.len() != self.plan.exits.len() {
            let missing = self
                .plan
                .exits
                .iter()
                .find(|exit| !self.exit_specs.contains_key(&exit.target))
                .map(|exit| exit.target)
                .unwrap_or_default();
            return Err(WxBuildError::InvalidPlan(format!(
                "exit at bytecode pc {missing} is not reachable from the region"
            )));
        }

        let mut exit_specs: Vec<_> = self.exit_specs.values().cloned().collect();
        exit_specs.sort_by_key(|spec| spec.exit.0);
        let mut side_exits = Vec::with_capacity(exit_specs.len());

        for spec in exit_specs {
            let values = spec
                .parameters
                .iter()
                .map(|(_, parameter)| parameter.id)
                .collect();
            let state = spec
                .parameters
                .iter()
                .map(|(register, parameter)| WxStateValue {
                    register: *register,
                    value: parameter.id,
                    ty: parameter.ty,
                })
                .collect();

            self.blocks.push(WxBlock {
                id: spec.id,
                parameters: spec
                    .parameters
                    .iter()
                    .map(|(_, parameter)| *parameter)
                    .collect(),
                instructions: Vec::new(),
                terminator: WxTerminator::SideExit {
                    exit: spec.exit,
                    values,
                },
            });
            side_exits.push(WxSideExit {
                id: spec.exit,
                resume_pc: spec.resume_pc,
                state,
            });
        }
        side_exits.append(&mut self.synthetic_exits);
        side_exits.sort_by_key(|side_exit| side_exit.id.0);

        let entry = self
            .block_specs
            .get(&self.plan.header)
            .map(|spec| spec.id)
            .ok_or_else(|| WxBuildError::InvalidPlan("missing entry block".to_string()))?;
        let entry_state = self
            .block_specs
            .get(&self.plan.header)
            .map(|spec| {
                spec.parameters
                    .iter()
                    .map(|(register, parameter)| WxStateValue {
                        register: *register,
                        value: parameter.id,
                        ty: parameter.ty,
                    })
                    .collect()
            })
            .ok_or_else(|| WxBuildError::InvalidPlan("missing entry state".to_string()))?;

        Ok(WxFunction {
            origin: WxRegionOrigin {
                region_id: self.plan.region_id,
                bytecode_header: self.plan.header,
                bytecode_backedge: self.plan.backedge,
            },
            entry,
            entry_state,
            blocks: std::mem::take(&mut self.blocks),
            returns: Vec::new(),
            side_exits,
        })
    }

    fn build_block(&mut self, start_pc: usize) -> Result<(), WxBuildError> {
        let spec =
            self.block_specs.get(&start_pc).cloned().ok_or_else(|| {
                WxBuildError::InvalidPlan(format!("missing block for pc {start_pc}"))
            })?;
        let mut environment: HashMap<Register, TypedValue> = spec
            .parameters
            .iter()
            .map(|(register, parameter)| {
                (
                    *register,
                    TypedValue {
                        id: parameter.id,
                        ty: parameter.ty,
                    },
                )
            })
            .collect();
        let mut instructions = Vec::new();
        let mut pc = spec.pc;

        let terminator = loop {
            if pc > self.plan.backedge {
                return Err(WxBuildError::InvalidPlan(format!(
                    "block starting at {start_pc} falls through the region"
                )));
            }
            if pc != start_pc && self.leaders.contains(&pc) {
                let target = self.internal_target(pc, &environment)?;
                break WxTerminator::Jump {
                    target: target.block,
                    arguments: target.arguments,
                };
            }

            match &self.executable.bytecode.code[pc] {
                Instruction::ConstI64 { dst, value } => {
                    let result = self.allocate_value()?;
                    let ty = WxType::Scalar(WxScalarType::I64);
                    instructions.push(WxInst {
                        results: vec![WxInstResult { id: result, ty }],
                        kind: WxInstKind::Constant(super::ir::WxConstant::Int(*value)),
                    });
                    environment.insert(*dst, TypedValue { id: result, ty });
                    pc += 1;
                }
                Instruction::AddI64 { dst, lhs, rhs } => {
                    let lhs = self.read_register(&environment, pc, *lhs, WxScalarType::I64)?;
                    let rhs = self.read_register(&environment, pc, *rhs, WxScalarType::I64)?;
                    let result = self.allocate_value()?;
                    let overflow = self.allocate_value()?;
                    let ty = WxType::Scalar(WxScalarType::I64);
                    instructions.push(WxInst {
                        results: vec![
                            WxInstResult { id: result, ty },
                            WxInstResult {
                                id: overflow,
                                ty: WxType::Scalar(WxScalarType::I1),
                            },
                        ],
                        kind: WxInstKind::IntegerBinaryWithOverflow {
                            op: WxIntOverflowOp::Add,
                            lhs: lhs.id,
                            rhs: rhs.id,
                        },
                    });
                    let exit = self.create_overflow_exit(pc, &environment)?;
                    instructions.push(WxInst {
                        results: Vec::new(),
                        kind: WxInstKind::Guard {
                            condition: overflow,
                            exit,
                            mode: WxGuardMode::ExitWhenTrue,
                        },
                    });
                    environment.insert(*dst, TypedValue { id: result, ty });
                    pc += 1;
                }
                Instruction::LtI64 { dst, lhs, rhs } => {
                    let lhs = self.read_register(&environment, pc, *lhs, WxScalarType::I64)?;
                    let rhs = self.read_register(&environment, pc, *rhs, WxScalarType::I64)?;
                    let result = self.allocate_value()?;
                    let ty = WxType::Scalar(WxScalarType::I1);
                    instructions.push(WxInst {
                        results: vec![WxInstResult { id: result, ty }],
                        kind: WxInstKind::Compare {
                            op: WxCompareOp::Integer(WxIntCompareOp::SignedLt),
                            lhs: lhs.id,
                            rhs: rhs.id,
                        },
                    });
                    environment.insert(*dst, TypedValue { id: result, ty });
                    pc += 1;
                }
                Instruction::Jump { target } => {
                    let target = self.control_target(*target, &environment)?;
                    break WxTerminator::Jump {
                        target: target.block,
                        arguments: target.arguments,
                    };
                }
                Instruction::Branch { cond, yes, no } => {
                    let condition =
                        self.read_register(&environment, pc, *cond, WxScalarType::I1)?;
                    let yes = self.control_target(*yes, &environment)?;
                    let no = self.control_target(*no, &environment)?;
                    break WxTerminator::Branch {
                        condition: condition.id,
                        yes,
                        no,
                    };
                }
                Instruction::Return { .. } => {
                    return Err(WxBuildError::UnsupportedInstruction {
                        pc,
                        instruction: "Return",
                    });
                }
            }
        };

        self.blocks.push(WxBlock {
            id: spec.id,
            parameters: spec
                .parameters
                .iter()
                .map(|(_, parameter)| *parameter)
                .collect(),
            instructions,
            terminator,
        });
        Ok(())
    }

    fn control_target(
        &mut self,
        target: usize,
        environment: &HashMap<Register, TypedValue>,
    ) -> Result<WxBlockTarget, WxBuildError> {
        if (self.plan.header..=self.plan.backedge).contains(&target) {
            self.internal_target(target, environment)
        } else {
            self.exit_target(target, environment)
        }
    }

    fn internal_target(
        &mut self,
        target: usize,
        environment: &HashMap<Register, TypedValue>,
    ) -> Result<WxBlockTarget, WxBuildError> {
        if !self.leaders.contains(&target) {
            return Err(WxBuildError::InvalidPlan(format!(
                "pc {target} is not a region block leader"
            )));
        }

        if !self.block_specs.contains_key(&target) {
            let id = self.allocate_block()?;
            let mut registers: Vec<_> = environment.keys().copied().collect();
            registers.sort_unstable();
            let mut parameters = Vec::with_capacity(registers.len());
            for register in registers {
                let value =
                    environment
                        .get(&register)
                        .copied()
                        .ok_or(WxBuildError::MissingRegister {
                            pc: target,
                            register,
                        })?;
                parameters.push((
                    register,
                    WxBlockParam {
                        id: self.allocate_value()?,
                        ty: value.ty,
                    },
                ));
            }
            self.block_specs.insert(
                target,
                BlockSpec {
                    id,
                    pc: target,
                    parameters,
                },
            );
            self.queue.push_back(target);
        }

        let spec =
            self.block_specs.get(&target).cloned().ok_or_else(|| {
                WxBuildError::InvalidPlan(format!("missing block for pc {target}"))
            })?;
        let arguments = self.arguments_for(&spec.parameters, environment, target)?;
        Ok(WxBlockTarget {
            block: spec.id,
            arguments,
        })
    }

    fn exit_target(
        &mut self,
        target: usize,
        environment: &HashMap<Register, TypedValue>,
    ) -> Result<WxBlockTarget, WxBuildError> {
        let exit_index = self.exit_by_pc.get(&target).copied().ok_or_else(|| {
            WxBuildError::InvalidPlan(format!("region edge to pc {target} has no JitPlan exit"))
        })?;

        if !self.exit_specs.contains_key(&target) {
            let block_id = self.allocate_block()?;
            let exit_id = WxExitId(
                u32::try_from(exit_index)
                    .map_err(|_| WxBuildError::IdSpaceExhausted("side-exit"))?,
            );
            let live_slots = self.plan.live_slots.clone();
            let parameters = self.parameters_for_slots(&live_slots)?;
            self.exit_specs.insert(
                target,
                ExitBlockSpec {
                    id: block_id,
                    exit: exit_id,
                    resume_pc: target,
                    parameters,
                },
            );
        }

        let spec =
            self.exit_specs.get(&target).cloned().ok_or_else(|| {
                WxBuildError::InvalidPlan(format!("missing exit for pc {target}"))
            })?;
        let arguments = self.arguments_for(&spec.parameters, environment, target)?;
        Ok(WxBlockTarget {
            block: spec.id,
            arguments,
        })
    }

    fn parameters_for_slots(
        &mut self,
        slots: &[LiveSlot],
    ) -> Result<Vec<(Register, WxBlockParam)>, WxBuildError> {
        slots
            .iter()
            .map(|slot| {
                Ok((
                    slot.register,
                    WxBlockParam {
                        id: self.allocate_value()?,
                        ty: slot_type(slot.ty),
                    },
                ))
            })
            .collect()
    }

    fn arguments_for(
        &self,
        parameters: &[(Register, WxBlockParam)],
        environment: &HashMap<Register, TypedValue>,
        pc: usize,
    ) -> Result<Vec<WxValueId>, WxBuildError> {
        parameters
            .iter()
            .map(|(register, parameter)| {
                let value =
                    environment
                        .get(register)
                        .copied()
                        .ok_or(WxBuildError::MissingRegister {
                            pc,
                            register: *register,
                        })?;
                if value.ty != parameter.ty {
                    return Err(WxBuildError::TypeMismatch {
                        pc,
                        register: *register,
                        expected: parameter.ty,
                        actual: value.ty,
                    });
                }
                Ok(value.id)
            })
            .collect()
    }

    fn read_register(
        &self,
        environment: &HashMap<Register, TypedValue>,
        pc: usize,
        register: Register,
        expected: WxScalarType,
    ) -> Result<TypedValue, WxBuildError> {
        let value = environment
            .get(&register)
            .copied()
            .ok_or(WxBuildError::MissingRegister { pc, register })?;
        let expected = WxType::Scalar(expected);
        if value.ty == expected {
            Ok(value)
        } else {
            Err(WxBuildError::TypeMismatch {
                pc,
                register,
                expected,
                actual: value.ty,
            })
        }
    }

    fn create_overflow_exit(
        &mut self,
        pc: usize,
        environment: &HashMap<Register, TypedValue>,
    ) -> Result<WxExitId, WxBuildError> {
        let exit = WxExitId(self.next_exit);
        self.next_exit = self
            .next_exit
            .checked_add(1)
            .ok_or(WxBuildError::IdSpaceExhausted("side-exit"))?;

        let mut registers: Vec<_> = environment.keys().copied().collect();
        registers.sort_unstable();
        let state = registers
            .into_iter()
            .map(|register| {
                let value = environment
                    .get(&register)
                    .copied()
                    .ok_or(WxBuildError::MissingRegister { pc, register })?;
                Ok(WxStateValue {
                    register,
                    value: value.id,
                    ty: value.ty,
                })
            })
            .collect::<Result<_, WxBuildError>>()?;
        self.synthetic_exits.push(WxSideExit {
            id: exit,
            resume_pc: pc,
            state,
        });
        Ok(exit)
    }

    fn allocate_value(&mut self) -> Result<WxValueId, WxBuildError> {
        let id = WxValueId(self.next_value);
        self.next_value = self
            .next_value
            .checked_add(1)
            .ok_or(WxBuildError::IdSpaceExhausted("value"))?;
        Ok(id)
    }

    fn allocate_block(&mut self) -> Result<WxBlockId, WxBuildError> {
        let id = WxBlockId(self.next_block);
        self.next_block = self
            .next_block
            .checked_add(1)
            .ok_or(WxBuildError::IdSpaceExhausted("block"))?;
        Ok(id)
    }
}

fn slot_type(slot_type: SlotType) -> WxType {
    match slot_type {
        SlotType::I64 => WxType::Scalar(WxScalarType::I64),
        SlotType::Bool => WxType::Scalar(WxScalarType::I1),
    }
}
