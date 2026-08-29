use crate::adaptive_v2::profile::RecordPermit;
use crate::adaptive_v2::trace::{EntryKind, ExecutableIdentity};
use crate::adaptive_v2::wxir_v2::dependency::{Dependency, DependencyKind};
use crate::adaptive_v2::wxir_v2::ir::{BlockId, SnapshotDraft, ValueType};
use crate::executable::ExecutableFunction;
use crate::value::Value;

mod cfg;
mod fused_trace;

pub(super) use fused_trace::{FusedTraceFacts, FusedTraceRequest, record as record_fused_entry};

pub(super) fn record_entry(
    executable: &ExecutableFunction,
    arguments: &[Value],
    permit: RecordPermit,
) -> Result<SnapshotDraft, String> {
    if arguments
        .iter()
        .any(|value| matches!(value, Value::Object(_)))
        || fused_trace::is_macro_candidate(executable, arguments)
    {
        let facts = FusedTraceFacts::from_proven_structure(executable, permit.schema_epoch());
        if let Some(draft) = record_fused_entry(FusedTraceRequest {
            executable,
            arguments,
            permit,
            facts: &facts,
        })? {
            return Ok(draft);
        }
    }
    let parameter_types = executable
        .parameters()
        .iter()
        .zip(arguments)
        .map(|(parameter, value)| scalar_type(value).map(|ty| (parameter.register, ty)))
        .collect::<Result<Vec<_>, _>>()?;
    if parameter_types.len() != executable.parameters().len()
        || parameter_types.len() != arguments.len()
    {
        return Err("adaptive-v2 entry argument arity changed".to_owned());
    }
    let epoch = executable.id().as_u64();
    let identity = ExecutableIdentity::new(epoch, epoch);
    let dependencies = dependencies(epoch);
    let lowered = cfg::lower(
        executable,
        &parameter_types,
        identity_true_parameter(executable, arguments),
        identity,
        &dependencies,
    )?;
    Ok(SnapshotDraft::new(
        identity,
        EntryKind::FunctionEntry,
        BlockId::new(0),
        lowered.blocks,
        lowered.root_maps,
        lowered.deopts,
        dependencies,
    )
    .with_schema_epoch(permit.schema_epoch()))
}

fn scalar_type(value: &Value) -> Result<ValueType, String> {
    match value {
        Value::SmallInt(_) => Ok(ValueType::I64),
        Value::Float(_) => Ok(ValueType::F64),
        Value::Bool(_) => Ok(ValueType::Bool),
        Value::Object(_) => Ok(ValueType::Handle),
        Value::None | Value::Uninitialized => {
            Err("adaptive-v2 entry requires scalar live arguments".to_owned())
        }
    }
}

fn identity_true_parameter(executable: &ExecutableFunction, arguments: &[Value]) -> Option<u16> {
    let [crate::bytecode::Instruction::Return { src }] = executable.bytecode().code.as_slice()
    else {
        return None;
    };
    executable
        .parameters()
        .iter()
        .zip(arguments)
        .find_map(|(parameter, value)| {
            (parameter.register == *src && matches!(value, Value::Bool(true)))
                .then_some(parameter.register)
        })
}

fn dependencies(epoch: u64) -> Vec<Dependency> {
    vec![
        Dependency::current(DependencyKind::Executable, epoch, epoch),
        Dependency::current(DependencyKind::Schema, epoch, epoch),
        Dependency::current(DependencyKind::GcAbi, 0, 1),
        Dependency::current(DependencyKind::HelperAbi, 0, 1),
    ]
}

#[cfg(test)]
mod tests;
