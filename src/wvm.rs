use std::collections::HashMap;

use crate::bytecode::{Instruction, Register};
use crate::executable::{ExecutableFunction, ExecutableId};
use crate::jit::{CompiledRegion, CraneliftRegionCompiler, RegionCompiler};
use crate::planner::plan_hot_region;
use crate::profiler::Profile;
use crate::structure_map::RegionId;
use crate::value::Value;
use crate::verifier::verify;
use crate::wxir::{WxExitKind, build_region};

/// Default observed region entries before synchronous tier-up.
pub const DEFAULT_HOT_THRESHOLD: u64 = 1_000;

pub struct Frame {
    pc: usize,
    registers: Vec<Value>,
    suppress_osr_pc: Option<usize>,
}

pub struct Vm {
    hot_threshold: u64,
    jit_report: JitReport,
    runtimes: HashMap<ExecutableId, FunctionRuntime>,
    last_executed: Option<ExecutableId>,
}

struct FunctionRuntime {
    profile: Profile,
    jit: JitRuntime,
}

struct JitRuntime {
    regions: HashMap<RegionId, RegionState>,
    region_by_header: Vec<Option<RegionId>>,
    compiler: CraneliftRegionCompiler,
}

enum RegionState {
    Compiled { region: Box<CompiledRegion> },
    Disabled { reason: String },
}

impl JitRuntime {
    fn new(executable: &ExecutableFunction) -> Self {
        let mut region_by_header = vec![None; executable.bytecode().code.len()];

        for (index, region) in executable.structure_map().loops.iter().enumerate() {
            region_by_header[region.header] = Some(RegionId(index));
        }

        Self {
            regions: HashMap::new(),
            region_by_header,
            compiler: CraneliftRegionCompiler::new(),
        }
    }

    fn region_at(&self, pc: usize) -> Option<RegionId> {
        self.region_by_header.get(pc).copied().flatten()
    }
}

/// Stage at which one region was disabled for the current execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitFailureStage {
    BuildWxir,
    Compile,
    Execute,
}

impl JitFailureStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BuildWxir => "build_wxir",
            Self::Compile => "compile",
            Self::Execute => "execute",
        }
    }
}

/// Preserved diagnostic for a failed synchronous tier-up operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JitFailure {
    pub region_id: RegionId,
    pub stage: JitFailureStage,
    pub reason: String,
}

/// Observable tier-up activity for the latest execute invocation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JitReport {
    pub compilation_attempts: u64,
    pub compiled_regions: u64,
    pub disabled_regions: u64,
    pub native_executions: u64,
    pub last_resume_pc: Option<usize>,
    pub last_exit_kind: Option<WxExitKind>,
    pub failures: Vec<JitFailure>,
}

impl JitReport {
    pub const fn last_exit_kind_name(&self) -> Option<&'static str> {
        match self.last_exit_kind {
            Some(WxExitKind::RegionExit) => Some("region_exit"),
            Some(WxExitKind::ReplayInstruction) => Some("replay_instruction"),
            Some(WxExitKind::Deopt) => Some("deopt"),
            None => None,
        }
    }
}

pub struct ExecutionResult {
    pub value: Value,
}

impl Vm {
    pub fn new() -> Self {
        Self::with_hot_threshold(DEFAULT_HOT_THRESHOLD)
    }

    /// Creates a VM that tiers up after this many observed region entries.
    pub fn with_hot_threshold(hot_threshold: u64) -> Self {
        Self {
            hot_threshold,
            jit_report: JitReport::default(),
            runtimes: HashMap::new(),
            last_executed: None,
        }
    }

    /// Changes the threshold used by the next execute invocation.
    pub fn set_hot_threshold(&mut self, hot_threshold: u64) {
        self.hot_threshold = hot_threshold;
    }

    /// Returns the profile for the most recently executed valid executable.
    pub fn profile(&self) -> Option<&Profile> {
        self.last_executed
            .and_then(|id| self.runtimes.get(&id))
            .map(|runtime| &runtime.profile)
    }

    /// Returns the cached profile for this executable identity, if present.
    pub fn profile_for(&self, executable: &ExecutableFunction) -> Option<&Profile> {
        self.runtimes
            .get(&executable.id())
            .map(|runtime| &runtime.profile)
    }

    /// Tier-up activity from the latest execute invocation, including failures.
    pub fn jit_report(&self) -> &JitReport {
        &self.jit_report
    }

    pub fn execute(&mut self, executable: &ExecutableFunction) -> Result<ExecutionResult, String> {
        self.jit_report = JitReport::default();
        verify(executable)?;
        let id = executable.id();
        let mut runtime = self
            .runtimes
            .remove(&id)
            .unwrap_or_else(|| FunctionRuntime {
                profile: Profile::new(executable.structure_map().loops.len()),
                jit: JitRuntime::new(executable),
            });
        let result = self.execute_with_runtime(executable, &mut runtime);
        self.runtimes.insert(id, runtime);
        self.last_executed = Some(id);
        result
    }

    fn execute_with_runtime(
        &mut self,
        executable: &ExecutableFunction,
        runtime: &mut FunctionRuntime,
    ) -> Result<ExecutionResult, String> {
        let function = executable.bytecode();
        let mut frame = Frame {
            pc: 0,
            registers: vec![Value::Uninitialized; function.register_count],
            suppress_osr_pc: None,
        };

        while frame.pc < function.code.len() {
            if let Some(region_id) = runtime.jit.region_at(frame.pc) {
                runtime.profile.record_entry(region_id);
            }

            if self.try_execute_region(executable, &mut frame, runtime) {
                continue;
            }

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

                Instruction::Move { dst, src } => {
                    let value = read_register(&frame, *src)?;
                    write_register(&mut frame, *dst, value)?;
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
        runtime: &mut FunctionRuntime,
    ) -> bool {
        if frame.suppress_osr_pc == Some(frame.pc) {
            frame.suppress_osr_pc = None;
            return false;
        }

        let Some(region_id) = runtime.jit.region_at(frame.pc) else {
            return false;
        };

        if runtime.jit.regions.contains_key(&region_id) {
            return self.execute_cached_region(region_id, frame, &mut runtime.jit);
        }

        // TODO: Future tiering/eviction profiles may distinguish interpreted
        // warm-up entries, native entries, and side exits or guard failures.
        let Some(plan) = plan_hot_region(
            executable.structure_map(),
            &runtime.profile,
            self.hot_threshold,
            region_id,
        ) else {
            return false;
        };

        self.jit_report.compilation_attempts += 1;
        let function = match build_region(executable, &plan) {
            Ok(function) => function,
            Err(error) => {
                self.disable_region(
                    &mut runtime.jit,
                    region_id,
                    JitFailureStage::BuildWxir,
                    error.to_string(),
                );
                return false;
            }
        };
        let region = match runtime.jit.compiler.compile(&function) {
            Ok(region) => region,
            Err(error) => {
                self.disable_region(
                    &mut runtime.jit,
                    region_id,
                    JitFailureStage::Compile,
                    error.to_string(),
                );
                return false;
            }
        };

        self.jit_report.compiled_regions += 1;
        runtime.jit.regions.insert(
            region_id,
            RegionState::Compiled {
                region: Box::new(region),
            },
        );
        self.execute_cached_region(region_id, frame, &mut runtime.jit)
    }

    fn execute_cached_region(
        &mut self,
        region_id: RegionId,
        frame: &mut Frame,
        jit: &mut JitRuntime,
    ) -> bool {
        let execution = match jit.regions.get_mut(&region_id) {
            Some(RegionState::Compiled { region }) => region.execute(&mut frame.registers),
            Some(RegionState::Disabled { reason }) => {
                let _ = reason;
                return false;
            }
            None => return false,
        };

        match execution {
            Ok(execution) => {
                self.jit_report.native_executions += 1;
                self.jit_report.last_resume_pc = Some(execution.resume_pc);
                self.jit_report.last_exit_kind = Some(execution.kind);
                frame.pc = execution.resume_pc;
                frame.suppress_osr_pc = match execution.kind {
                    WxExitKind::ReplayInstruction => Some(execution.resume_pc),
                    WxExitKind::RegionExit => None,
                    // Deopt currently resumes interpretation directly. Future
                    // speculation metadata may require richer reconstruction.
                    WxExitKind::Deopt => None,
                };
                true
            }
            Err(error) => {
                self.disable_region(jit, region_id, JitFailureStage::Execute, error.to_string());
                false
            }
        }
    }

    fn disable_region(
        &mut self,
        jit: &mut JitRuntime,
        region_id: RegionId,
        stage: JitFailureStage,
        reason: String,
    ) {
        self.jit_report.disabled_regions += 1;
        self.jit_report.failures.push(JitFailure {
            region_id,
            stage,
            reason: reason.clone(),
        });
        jit.regions
            .insert(region_id, RegionState::Disabled { reason });
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
