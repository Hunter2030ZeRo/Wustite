use std::collections::HashMap;

use crate::bytecode::{Instruction, Register};
use crate::executable::ExecutableFunction;
use crate::jit::{CompiledRegion, CraneliftRegionCompiler, RegionCompiler};
use crate::planner::plan_hot_region;
use crate::profiler::Profile;
use crate::structure_map::RegionId;
use crate::value::Value;
use crate::verifier::verify;
use crate::wxir::build_region;

/// Default interpreted header executions before synchronous tier-up.
pub const DEFAULT_HOT_THRESHOLD: u64 = 1_000;

pub struct Frame {
    pc: usize,
    registers: Vec<Value>,
    skip_osr_once: bool,
}

pub struct Vm {
    profile: Option<Profile>,
    hot_threshold: u64,
    jit_report: JitReport,
}

struct JitRuntime {
    regions: HashMap<RegionId, RegionState>,
    compiler: CraneliftRegionCompiler,
}

enum RegionState {
    Compiled {
        region: Box<CompiledRegion>,
        static_exit_count: usize,
    },
    Disabled,
}

/// Observable tier-up activity for the latest execute invocation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JitReport {
    pub compilation_attempts: u64,
    pub compiled_regions: u64,
    pub disabled_regions: u64,
    pub native_executions: u64,
    pub last_resume_pc: Option<usize>,
}

pub struct ExecutionResult {
    pub value: Value,
}

impl Vm {
    pub fn new() -> Self {
        Self::with_hot_threshold(DEFAULT_HOT_THRESHOLD)
    }

    /// Creates a VM that tiers up after this many interpreted header executions.
    pub fn with_hot_threshold(hot_threshold: u64) -> Self {
        Self {
            profile: None,
            hot_threshold,
            jit_report: JitReport::default(),
        }
    }

    /// Changes the threshold used by the next execute invocation.
    pub fn set_hot_threshold(&mut self, hot_threshold: u64) {
        self.hot_threshold = hot_threshold;
    }

    pub fn profile(&self) -> Option<&Profile> {
        self.profile.as_ref()
    }

    /// Tier-up activity from the latest execute invocation, including failures.
    pub fn jit_report(&self) -> &JitReport {
        &self.jit_report
    }

    pub fn execute(&mut self, executable: &ExecutableFunction) -> Result<ExecutionResult, String> {
        self.profile = None;
        self.jit_report = JitReport::default();
        verify(executable)?;

        let function = &executable.bytecode;
        let mut frame = Frame {
            pc: 0,
            registers: vec![Value::Uninitialized; function.register_count],
            skip_osr_once: false,
        };
        let mut jit = JitRuntime {
            regions: HashMap::new(),
            compiler: CraneliftRegionCompiler::new(),
        };

        self.profile = Some(Profile::new(function.code.len()));

        while frame.pc < function.code.len() {
            if self.try_execute_region(executable, &mut frame, &mut jit) {
                continue;
            }

            self.profile
                .as_mut()
                .ok_or_else(|| "profile is not initialized".to_string())?
                .record(frame.pc);

            match &function.code[frame.pc] {
                Instruction::ConstI64 { dst, value } => {
                    write_register(&mut frame, *dst, Value::I64(*value))?;
                    frame.pc += 1;
                }

                Instruction::AddI64 { dst, lhs, rhs } => {
                    let lhs = read_i64(&frame, *lhs)?;
                    let rhs = read_i64(&frame, *rhs)?;

                    let result = lhs
                        .checked_add(rhs)
                        .ok_or_else(|| "i64 addition overflow".to_string())?;

                    write_register(&mut frame, *dst, Value::I64(result))?;
                    frame.pc += 1;
                }

                Instruction::LtI64 { dst, lhs, rhs } => {
                    let lhs = read_i64(&frame, *lhs)?;
                    let rhs = read_i64(&frame, *rhs)?;

                    write_register(&mut frame, *dst, Value::Bool(lhs < rhs))?;
                    frame.pc += 1;
                }

                Instruction::Jump { target } => {
                    frame.pc = *target;
                }

                Instruction::Branch { cond, yes, no } => {
                    let condition = read_bool(&frame, *cond)?;
                    let target = if condition { *yes } else { *no };

                    frame.pc = target;
                }

                Instruction::Return { src } => {
                    let value = read_register(&frame, *src)?;

                    return Ok(ExecutionResult { value });
                }
            }
        }

        Err("function ended without Return".to_string())
    }

    fn try_execute_region(
        &mut self,
        executable: &ExecutableFunction,
        frame: &mut Frame,
        jit: &mut JitRuntime,
    ) -> bool {
        if frame.skip_osr_once {
            frame.skip_osr_once = false;
            return false;
        }

        let Some(region_index) = executable
            .structure_map
            .loops
            .iter()
            .position(|region| region.header == frame.pc)
        else {
            return false;
        };
        let region_id = RegionId(region_index);

        if jit.regions.contains_key(&region_id) {
            return self.execute_cached_region(region_id, frame, jit);
        }

        let Some(plan) = self.profile.as_ref().and_then(|profile| {
            plan_hot_region(
                &executable.structure_map,
                profile,
                self.hot_threshold,
                region_id,
            )
        }) else {
            return false;
        };

        self.jit_report.compilation_attempts += 1;
        let static_exit_count = plan.exits.len();
        let compiled = build_region(executable, &plan)
            .map_err(|error| error.to_string())
            .and_then(|function| {
                jit.compiler
                    .compile(&function)
                    .map_err(|error| error.to_string())
            });

        match compiled {
            Ok(region) => {
                self.jit_report.compiled_regions += 1;
                jit.regions.insert(
                    region_id,
                    RegionState::Compiled {
                        region: Box::new(region),
                        static_exit_count,
                    },
                );
                self.execute_cached_region(region_id, frame, jit)
            }
            Err(_) => {
                self.jit_report.disabled_regions += 1;
                jit.regions.insert(region_id, RegionState::Disabled);
                false
            }
        }
    }

    fn execute_cached_region(
        &mut self,
        region_id: RegionId,
        frame: &mut Frame,
        jit: &mut JitRuntime,
    ) -> bool {
        let (execution, static_exit_count) = match jit.regions.get(&region_id) {
            Some(RegionState::Compiled {
                region,
                static_exit_count,
            }) => (region.execute(&mut frame.registers), *static_exit_count),
            Some(RegionState::Disabled) | None => return false,
        };

        match execution {
            Ok(execution) => {
                self.jit_report.native_executions += 1;
                self.jit_report.last_resume_pc = Some(execution.resume_pc);
                frame.pc = execution.resume_pc;
                frame.skip_osr_once =
                    usize::try_from(execution.exit.0).is_ok_and(|exit| exit >= static_exit_count);
                true
            }
            Err(_) => {
                self.jit_report.disabled_regions += 1;
                jit.regions.insert(region_id, RegionState::Disabled);
                false
            }
        }
    }
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

fn read_register(frame: &Frame, register: Register) -> Result<Value, String> {
    frame
        .registers
        .get(register as usize)
        .copied()
        .ok_or_else(|| format!("invalid register r{register}"))
}

fn write_register(frame: &mut Frame, register: Register, value: Value) -> Result<(), String> {
    let slot = frame
        .registers
        .get_mut(register as usize)
        .ok_or_else(|| format!("invalid register r{register}"))?;

    *slot = value;
    Ok(())
}

fn read_i64(frame: &Frame, register: Register) -> Result<i64, String> {
    match read_register(frame, register)? {
        Value::I64(value) => Ok(value),
        other => Err(format!("expected i64 in r{register}, found {other:?}")),
    }
}

fn read_bool(frame: &Frame, register: Register) -> Result<bool, String> {
    match read_register(frame, register)? {
        Value::Bool(value) => Ok(value),
        other => Err(format!("expected bool in r{register}, found {other:?}")),
    }
}
