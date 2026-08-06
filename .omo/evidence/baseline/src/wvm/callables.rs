use crate::executable::{ExecutableConstant, ExecutableFunction};
use crate::object::Object;
use crate::value::Value;

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

    pub(super) fn callable(&self, value: Value) -> Result<ExecutableFunction, String> {
        let Value::Object(reference) = value else {
            return Err("call target is not a function".to_string());
        };
        match self.object_heap.get(reference) {
            Ok(Object::Function(function)) => Ok(function.clone()),
            Ok(_) => Err("call target is not a function".to_string()),
            Err(error) => Err(error.to_string()),
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
            return self.execute_function(function, arguments);
        }

        let id = caller.id();
        if self.runtimes.contains_key(&id) {
            return Err("active function runtime is already cached".to_string());
        }
        let active_runtime = std::mem::replace(runtime, FunctionRuntime::new(caller));
        self.runtimes.insert(id, active_runtime);

        let result = self.execute_function(function, arguments);
        let Some(updated_runtime) = self.runtimes.remove(&id) else {
            return Err("active function runtime was not restored".to_string());
        };
        *runtime = updated_runtime;
        result
    }
}
