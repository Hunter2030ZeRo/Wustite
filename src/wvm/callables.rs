use crate::executable::{ExecutableConstant, ExecutableFunction};
use crate::object::{Object, ObjectKind, ObjectRef, ShapeId};
use crate::value::Value;
use std::sync::Arc;

use super::{ExecutionResult, FunctionRuntime, Vm};

impl Vm {
    pub(super) fn load_constant(
        &mut self,
        executable: &ExecutableFunction,
        runtime: &mut FunctionRuntime,
        index: usize,
    ) -> Result<Value, String> {
        if let Some(value) = runtime.constants.get(index).copied().flatten() {
            return Ok(value);
        }
        let constant = executable
            .constants()
            .get(index)
            .ok_or_else(|| format!("invalid constant c{index}"))?;
        let object = match constant {
            ExecutableConstant::String(value) => Object::String(value.clone()),
            ExecutableConstant::BigInt(value) => Object::BigInt(value.clone()),
            ExecutableConstant::Function(function) => Object::Function((**function).clone()),
            ExecutableConstant::Class(class) => Object::Class(class.clone()),
        };
        let value = self
            .object_heap
            .allocate(object)
            .map(Value::Object)
            .map_err(|error| error.to_string())?;
        let slot = runtime
            .constants
            .get_mut(index)
            .ok_or_else(|| format!("invalid constant cache slot c{index}"))?;
        *slot = Some(value);
        Ok(value)
    }

    pub(super) fn load_current_function(
        &mut self,
        executable: &ExecutableFunction,
        runtime: &mut FunctionRuntime,
    ) -> Result<Value, String> {
        if let Some(value) = runtime.current_function {
            return Ok(value);
        }
        let value = self
            .object_heap
            .allocate(Object::Function(executable.clone()))
            .map(Value::Object)
            .map_err(|error| error.to_string())?;
        runtime.current_function = Some(value);
        Ok(value)
    }

    pub(super) fn prepared_callable(
        &mut self,
        runtime: &mut FunctionRuntime,
        pc: usize,
        value: Value,
    ) -> Result<Arc<ExecutableFunction>, String> {
        let Value::Object(reference) = value else {
            return Err("call target is not a function".to_string());
        };
        let Some(site) = runtime.call_sites.get_mut(pc) else {
            return Err(format!("missing call site at pc {pc}"));
        };
        if let Some(target) = site
            .targets
            .iter()
            .flatten()
            .find(|target| target.key == PreparedCallKey::Function(reference))
        {
            return Ok(Arc::clone(&target.function));
        }
        let function = match self.object_heap.get(reference) {
            Ok(Object::Function(function)) => Arc::new(function.clone()),
            Ok(_) => return Err("call target is not a function".to_string()),
            Err(error) => return Err(error.to_string()),
        };
        if site.megamorphic {
            self.jit_report.call_sites.megamorphic_fallback = self
                .jit_report
                .call_sites
                .megamorphic_fallback
                .saturating_add(1);
            return Ok(function);
        }
        if let Some(slot) = site.targets.iter_mut().find(|target| target.is_none()) {
            *slot = Some(PreparedCallTarget {
                key: PreparedCallKey::Function(reference),
                function: Arc::clone(&function),
            });
            self.jit_report.call_sites.call_guard_miss =
                self.jit_report.call_sites.call_guard_miss.saturating_add(1);
        } else {
            site.megamorphic = true;
            self.jit_report.call_sites.megamorphic_fallback = self
                .jit_report
                .call_sites
                .megamorphic_fallback
                .saturating_add(1);
        }
        Ok(function)
    }

    pub(super) fn prepared_method(
        &mut self,
        runtime: &mut FunctionRuntime,
        pc: usize,
        receiver: Value,
        name: &str,
    ) -> Result<(ObjectRef, Arc<ExecutableFunction>), String> {
        let Value::Object(receiver) = receiver else {
            return Err("method receiver is not an object".to_string());
        };
        let (shape, function) = self
            .object_heap
            .lookup_method(receiver, name)
            .map_err(|error| error.to_string())?;
        let Some(site) = runtime.call_sites.get_mut(pc) else {
            return Err(format!("missing call site at pc {pc}"));
        };
        if let Some(target) = site
            .targets
            .iter()
            .flatten()
            .find(|target| target.key == PreparedCallKey::Method(shape))
        {
            return Ok((receiver, Arc::clone(&target.function)));
        }
        let function = Arc::new(function);
        if site.megamorphic {
            self.jit_report.call_sites.megamorphic_fallback = self
                .jit_report
                .call_sites
                .megamorphic_fallback
                .saturating_add(1);
            return Ok((receiver, function));
        }
        if let Some(slot) = site.targets.iter_mut().find(|target| target.is_none()) {
            *slot = Some(PreparedCallTarget {
                key: PreparedCallKey::Method(shape),
                function: Arc::clone(&function),
            });
            self.jit_report.call_sites.call_guard_miss =
                self.jit_report.call_sites.call_guard_miss.saturating_add(1);
        } else {
            site.megamorphic = true;
            self.jit_report.call_sites.megamorphic_fallback = self
                .jit_report
                .call_sites
                .megamorphic_fallback
                .saturating_add(1);
        }
        Ok((receiver, function))
    }

    pub(super) fn invoke_callable(
        &mut self,
        caller: &ExecutableFunction,
        runtime: &mut FunctionRuntime,
        pc: usize,
        callable: Value,
        arguments: &[Value],
    ) -> Result<Value, String> {
        let Value::Object(reference) = callable else {
            return Err("call target is not callable".to_string());
        };
        match self
            .object_heap
            .kind(reference)
            .map_err(|error| error.to_string())?
        {
            ObjectKind::Function => {
                let function = self.prepared_callable(runtime, pc, callable)?;
                self.invoke(caller, runtime, function.as_ref(), arguments)
                    .map(|result| result.value)
            }
            ObjectKind::String
            | ObjectKind::Tuple
            | ObjectKind::BigInt
            | ObjectKind::List
            | ObjectKind::Dict
            | ObjectKind::Class
            | ObjectKind::Instance
            | ObjectKind::BoundMethod => {
                self.invoke_non_function_callable(caller, runtime, reference, arguments)
            }
        }
    }

    #[inline(never)]
    fn invoke_non_function_callable(
        &mut self,
        caller: &ExecutableFunction,
        runtime: &mut FunctionRuntime,
        reference: ObjectRef,
        arguments: &[Value],
    ) -> Result<Value, String> {
        let object = self
            .object_heap
            .get(reference)
            .map_err(|error| error.to_string())?
            .clone();
        match object {
            Object::BoundMethod(method) => {
                let mut bound_arguments = Vec::with_capacity(arguments.len() + 1);
                bound_arguments.push(Value::Object(method.receiver()));
                bound_arguments.extend_from_slice(arguments);
                self.invoke(caller, runtime, method.function(), &bound_arguments)
                    .map(|result| result.value)
            }
            Object::Class(class) => {
                let instance = self
                    .object_heap
                    .instantiate(reference)
                    .map_err(|error| error.to_string())?;
                if let Some(initializer) = class.method("__init__") {
                    let mut initializer_arguments = Vec::with_capacity(arguments.len() + 1);
                    initializer_arguments.push(Value::Object(instance));
                    initializer_arguments.extend_from_slice(arguments);
                    let result = self
                        .invoke(caller, runtime, initializer, &initializer_arguments)?
                        .value;
                    if result != Value::None {
                        return Err("__init__ must return None".to_string());
                    }
                } else if !arguments.is_empty() {
                    return Err(format!("{}() takes no arguments", class.name()));
                }
                Ok(Value::Object(instance))
            }
            Object::Function(_) => {
                Err("function callable reached non-function dispatch".to_string())
            }
            _ => Err("call target is not callable".to_string()),
        }
    }

    pub(super) fn invoke(
        &mut self,
        caller: &ExecutableFunction,
        runtime: &mut FunctionRuntime,
        function: &ExecutableFunction,
        arguments: &[Value],
    ) -> Result<ExecutionResult, String> {
        if caller.id() != function.id() {
            return self.execute_prepared_function(function, arguments);
        }

        let id = caller.id();
        if self.runtimes.contains_key(&id) {
            return Err("active function runtime is already cached".to_string());
        }
        let quick_code = Arc::clone(&runtime.quick_code);
        let placeholder = FunctionRuntime::recursive_placeholder(caller, quick_code);
        let active_runtime = std::mem::replace(runtime, placeholder);
        self.runtimes.insert(id, active_runtime);

        let result = self.execute_prepared_function(function, arguments);
        let Some(updated_runtime) = self.runtimes.remove(&id) else {
            return Err("active function runtime was not restored".to_string());
        };
        *runtime = updated_runtime;
        result
    }
}

#[derive(Default)]
pub(super) struct PreparedCallSite {
    targets: [Option<PreparedCallTarget>; 2],
    megamorphic: bool,
}

struct PreparedCallTarget {
    key: PreparedCallKey,
    function: Arc<ExecutableFunction>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PreparedCallKey {
    Function(ObjectRef),
    Method(ShapeId),
}
