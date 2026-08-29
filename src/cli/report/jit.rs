use std::collections::BTreeMap;

use serde::Serialize;
use wustite::bytecode::Register;
use wustite::wvm::JitReport;

use super::super::value_names::slot_type_name;

#[derive(Serialize)]
pub(crate) struct JitOutput {
    compilation_attempts: u64,
    compiled_regions: u64,
    tier2_compilation_attempts: u64,
    tier2_compiled_regions: u64,
    disabled_regions: u64,
    native_executions: u64,
    tier2_native_executions: u64,
    last_resume_pc: Option<usize>,
    last_exit_kind: Option<String>,
    helper_calls: HelperCallsOutput,
    guest_calls: GuestCallsOutput,
    call_sites: CallSitesOutput,
    runtime_ops: RuntimeOpsOutput,
    exits: ExitsOutput,
    calls: BTreeMap<String, u64>,
    native_calls: BTreeMap<String, u64>,
    failures: Vec<JitFailureOutput>,
}

impl JitOutput {
    pub(crate) fn snapshot(report: &JitReport) -> Self {
        Self {
            compilation_attempts: report.compilation_attempts,
            compiled_regions: report.compiled_regions,
            tier2_compilation_attempts: report.tier2_compilation_attempts,
            tier2_compiled_regions: report.tier2_compiled_regions,
            disabled_regions: report.disabled_regions,
            native_executions: report.native_executions,
            tier2_native_executions: report.tier2_native_executions,
            last_resume_pc: report.last_resume_pc,
            last_exit_kind: report.last_exit_kind_name().map(str::to_string),
            helper_calls: HelperCallsOutput {
                call: report.helper_calls.call,
                get_item: report.helper_calls.get_item,
                set_item: report.helper_calls.set_item,
                length: report.helper_calls.length,
                object_access: report.helper_calls.object_access,
            },
            guest_calls: GuestCallsOutput {
                direct_native: report.guest_calls.direct_native,
                interpreter_fallback: report.guest_calls.interpreter_fallback,
            },
            call_sites: CallSitesOutput {
                leaf_plans: report.call_sites.leaf_plans,
                prepared_leaf_hit: report.call_sites.prepared_leaf_hit,
                compiled_leaf_hit: report.call_sites.compiled_leaf_hit,
                inlined_leaf: report.call_sites.inlined_leaf,
                prepared_call_hit: report.call_sites.prepared_call_hit,
                call_guard_miss: report.call_sites.call_guard_miss,
                megamorphic_fallback: report.call_sites.megamorphic_fallback,
            },
            runtime_ops: RuntimeOpsOutput {
                load_constant: report.runtime_ops.load_constant,
                binary: report.runtime_ops.binary,
                compare: report.runtime_ops.compare,
                unary: report.runtime_ops.unary,
                boolean: report.runtime_ops.boolean,
                build_tuple: report.runtime_ops.build_tuple,
                build_list: report.runtime_ops.build_list,
                build_dict: report.runtime_ops.build_dict,
                other: report.runtime_ops.other,
            },
            exits: ExitsOutput {
                region_exit: report.exits.region_exit,
                replay_instruction: report.exits.replay_instruction,
                deopt: report.exits.deopt,
            },
            calls: report
                .calls
                .iter()
                .map(|(function, count)| (function.clone(), *count))
                .collect(),
            native_calls: report
                .native_calls
                .iter()
                .map(|(function, count)| (function.clone(), *count))
                .collect(),
            failures: report
                .failures
                .iter()
                .map(|failure| JitFailureOutput {
                    function: failure.function.clone(),
                    region_id: failure.region_id.0,
                    stage: failure.stage.as_str(),
                    register: failure.register,
                    actual_slot_type: failure.actual_slot_type.map(slot_type_name),
                    reason: failure.reason.clone(),
                })
                .collect(),
        }
    }
}

#[derive(Serialize)]
struct HelperCallsOutput {
    call: u64,
    get_item: u64,
    set_item: u64,
    length: u64,
    object_access: u64,
}

#[derive(Serialize)]
struct GuestCallsOutput {
    direct_native: u64,
    interpreter_fallback: u64,
}

#[derive(Serialize)]
struct CallSitesOutput {
    leaf_plans: u64,
    prepared_leaf_hit: u64,
    compiled_leaf_hit: u64,
    inlined_leaf: u64,
    prepared_call_hit: u64,
    call_guard_miss: u64,
    megamorphic_fallback: u64,
}

#[derive(Serialize)]
struct RuntimeOpsOutput {
    load_constant: u64,
    binary: u64,
    compare: u64,
    unary: u64,
    boolean: u64,
    build_tuple: u64,
    build_list: u64,
    build_dict: u64,
    other: u64,
}

#[derive(Serialize)]
struct ExitsOutput {
    region_exit: u64,
    replay_instruction: u64,
    deopt: u64,
}

#[derive(Serialize)]
struct JitFailureOutput {
    function: Option<String>,
    region_id: usize,
    stage: &'static str,
    register: Option<Register>,
    actual_slot_type: Option<&'static str>,
    reason: String,
}

pub(crate) fn print_jit_trace(runs: &[super::RunOutput]) {
    for run in runs {
        print_jit_debug(&format!("run {}", run.index), &run.jit);
    }
}

pub(crate) fn print_jit_debug(label: &str, jit: &JitOutput) {
    let mut line = format!(
        "{label}: compilation_attempts={} compiled_regions={} tier2_compilation_attempts={} tier2_compiled_regions={} disabled_regions={} native_executions={} tier2_native_executions={}",
        jit.compilation_attempts,
        jit.compiled_regions,
        jit.tier2_compilation_attempts,
        jit.tier2_compiled_regions,
        jit.disabled_regions,
        jit.native_executions,
        jit.tier2_native_executions
    );
    if let Some(resume_pc) = jit.last_resume_pc {
        line.push_str(&format!(" last_resume_pc={resume_pc}"));
    }
    if let Some(exit_kind) = &jit.last_exit_kind {
        line.push_str(&format!(" last_exit_kind={exit_kind}"));
    }
    eprintln!("{line}");
    eprintln!(
        "{label}: helper_calls call={} get_item={} set_item={} length={} object_access={}",
        jit.helper_calls.call,
        jit.helper_calls.get_item,
        jit.helper_calls.set_item,
        jit.helper_calls.length,
        jit.helper_calls.object_access
    );
    eprintln!(
        "{label}: guest_calls direct_native={} interpreter_fallback={}",
        jit.guest_calls.direct_native, jit.guest_calls.interpreter_fallback
    );
    eprintln!(
        "{label}: call_sites leaf_plans={} prepared_leaf_hit={} compiled_leaf_hit={} inlined_leaf={} prepared_call_hit={} call_guard_miss={} megamorphic_fallback={}",
        jit.call_sites.leaf_plans,
        jit.call_sites.prepared_leaf_hit,
        jit.call_sites.compiled_leaf_hit,
        jit.call_sites.inlined_leaf,
        jit.call_sites.prepared_call_hit,
        jit.call_sites.call_guard_miss,
        jit.call_sites.megamorphic_fallback
    );
    eprintln!(
        "{label}: runtime_ops load_constant={} binary={} compare={} unary={} boolean={} build_tuple={} build_list={} build_dict={} other={}",
        jit.runtime_ops.load_constant,
        jit.runtime_ops.binary,
        jit.runtime_ops.compare,
        jit.runtime_ops.unary,
        jit.runtime_ops.boolean,
        jit.runtime_ops.build_tuple,
        jit.runtime_ops.build_list,
        jit.runtime_ops.build_dict,
        jit.runtime_ops.other
    );
    eprintln!(
        "{label}: exits region_exit={} replay_instruction={} deopt={}",
        jit.exits.region_exit, jit.exits.replay_instruction, jit.exits.deopt
    );
    let mut calls = format!("{label}: calls");
    for (function, count) in &jit.calls {
        calls.push_str(&format!(" {function}={count}"));
    }
    eprintln!("{calls}");
    let mut native_calls = format!("{label}: native_calls");
    for (function, count) in &jit.native_calls {
        native_calls.push_str(&format!(" {function}={count}"));
    }
    eprintln!("{native_calls}");
    for failure in &jit.failures {
        let function = failure.function.as_deref().unwrap_or("-");
        let register = failure
            .register
            .map_or_else(|| "-".to_string(), |register| format!("r{register}"));
        let actual_slot_type = failure.actual_slot_type.unwrap_or("-");
        eprintln!(
            "{label}: failure function={function} region={} stage={} register={register} actual_slot_type={actual_slot_type} reason={}",
            failure.region_id, failure.stage, failure.reason
        );
    }
}
