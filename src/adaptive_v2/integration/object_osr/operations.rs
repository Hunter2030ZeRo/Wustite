use crate::adaptive_v2::native::{AdaptiveNativeContext, NativeValue};
use crate::bytecode::Instruction;
use crate::object::{Object, ObjectHeap};
use crate::value::Value;

use super::call::NumericMethod;
use super::{Operation, SiteOperation};

pub(super) fn decode(
    instruction: &Instruction,
    registers: &[Value],
    heap: &ObjectHeap,
) -> Option<Operation> {
    match instruction {
        Instruction::GetAttr { dst, object, name } => Some(Operation::ObjectGet {
            receiver: object_ref(registers, *object)?,
            field: name.clone(),
            dst: *dst,
        }),
        Instruction::SetAttr {
            object: receiver,
            name,
            value,
        } => Some(Operation::ObjectSet {
            receiver: object_ref(registers, *receiver)?,
            field: name.clone(),
            value: integer(registers, *value)?,
        }),
        Instruction::GetItem {
            dst,
            object: list,
            key,
        } => {
            let index = usize::try_from(integer(registers, *key)?).ok()?;
            Some(Operation::ListGet {
                list: object_ref(registers, *list)?,
                index,
                dst: *dst,
            })
        }
        Instruction::ListAppend { list, value } => Some(Operation::ListAppend {
            list: object_ref(registers, *list)?,
            value: integer(registers, *value)?,
        }),
        Instruction::SetItem {
            object, key, value, ..
        } => Some(Operation::ListSet {
            list: object_ref(registers, *object)?,
            index: integer(registers, *key)?,
            value: integer(registers, *value)?,
        }),
        Instruction::ListInsert { list, index, value } => Some(Operation::ListInsert {
            list: object_ref(registers, *list)?,
            index: integer(registers, *index)?,
            value: integer(registers, *value)?,
        }),
        Instruction::ListPop { dst, list, index } => Some(Operation::ListPop {
            list: object_ref(registers, *list)?,
            index: integer(registers, *index)?,
            dst: *dst,
        }),
        Instruction::Length { dst, object } => Some(Operation::ListLength {
            list: object_ref(registers, *object)?,
            dst: *dst,
        }),
        Instruction::CallMethod {
            dst,
            receiver,
            name,
            args,
        } if args.len() == 1 => {
            let receiver = object_ref(registers, *receiver)?;
            let (_, function) = heap.lookup_method(receiver, name).ok()?;
            let method = NumericMethod::analyze(&function)?;
            let argument = integer(registers, args[0])?;
            if !method.supports(argument) {
                return None;
            }
            Some(Operation::DirectCall {
                receiver,
                callee: function.id().as_u64(),
                method,
                argument,
                dst: *dst,
            })
        }
        Instruction::Call {
            dst,
            callable,
            args,
        } if args.len() == 1 => {
            let callable = object_ref(registers, *callable)?;
            let Object::BoundMethod(bound) = heap.get(callable).ok()? else {
                return None;
            };
            let function = bound.function();
            let method = NumericMethod::analyze(function)?;
            let argument = integer(registers, args[0])?;
            if !method.supports(argument) {
                return None;
            }
            Some(Operation::DirectCall {
                receiver: bound.receiver(),
                callee: function.id().as_u64(),
                method,
                argument,
                dst: *dst,
            })
        }
        Instruction::ConstSmallInt { .. }
        | Instruction::ConstBool { .. }
        | Instruction::ConstI64 { .. }
        | Instruction::ConstFloat { .. }
        | Instruction::ConstNone { .. }
        | Instruction::LoadConstant { .. }
        | Instruction::BinaryOp { .. }
        | Instruction::CompareOp { .. }
        | Instruction::UnaryOp { .. }
        | Instruction::BooleanOp { .. }
        | Instruction::BuildTuple { .. }
        | Instruction::BuildList { .. }
        | Instruction::BuildDict { .. }
        | Instruction::GetSlice { .. }
        | Instruction::SetSlice { .. }
        | Instruction::LoadCurrentFunction { .. }
        | Instruction::Call { .. }
        | Instruction::CallMethod { .. }
        | Instruction::AddI64 { .. }
        | Instruction::LtI64 { .. }
        | Instruction::Move { .. }
        | Instruction::Jump { .. }
        | Instruction::Branch { .. }
        | Instruction::Return { .. } => None,
    }
}

pub(super) fn invalidated_object(
    instruction: &Instruction,
    registers: &[Value],
) -> Option<crate::object::ObjectRef> {
    let register = match instruction {
        Instruction::SetItem { object, .. }
        | Instruction::SetSlice { object, .. }
        | Instruction::ListInsert { list: object, .. }
        | Instruction::ListPop { list: object, .. } => *object,
        Instruction::ConstSmallInt { .. }
        | Instruction::ConstBool { .. }
        | Instruction::ConstI64 { .. }
        | Instruction::ConstFloat { .. }
        | Instruction::ConstNone { .. }
        | Instruction::LoadConstant { .. }
        | Instruction::BinaryOp { .. }
        | Instruction::CompareOp { .. }
        | Instruction::UnaryOp { .. }
        | Instruction::BooleanOp { .. }
        | Instruction::BuildTuple { .. }
        | Instruction::BuildList { .. }
        | Instruction::BuildDict { .. }
        | Instruction::GetItem { .. }
        | Instruction::GetAttr { .. }
        | Instruction::GetSlice { .. }
        | Instruction::SetAttr { .. }
        | Instruction::ListAppend { .. }
        | Instruction::Length { .. }
        | Instruction::LoadCurrentFunction { .. }
        | Instruction::Call { .. }
        | Instruction::CallMethod { .. }
        | Instruction::AddI64 { .. }
        | Instruction::LtI64 { .. }
        | Instruction::Move { .. }
        | Instruction::Jump { .. }
        | Instruction::Branch { .. }
        | Instruction::Return { .. } => return None,
    };
    object_ref(registers, register)
}

pub(super) const fn invalidates_all_bindings(instruction: &Instruction) -> bool {
    matches!(
        instruction,
        Instruction::Call { .. } | Instruction::CallMethod { .. }
    )
}

impl Operation {
    pub(super) fn profile_case(&self, heap: &ObjectHeap) -> Option<u32> {
        use std::hash::{Hash, Hasher};

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        match self {
            Self::ObjectGet { receiver, .. } | Self::ObjectSet { receiver, .. } => {
                let Object::Instance(instance) = heap.get(*receiver).ok()? else {
                    return None;
                };
                instance.class().hash(&mut hasher);
                instance.shape().0.hash(&mut hasher);
            }
            Self::ListGet { list, .. }
            | Self::ListAppend { list, .. }
            | Self::ListSet { list, .. }
            | Self::ListInsert { list, .. }
            | Self::ListPop { list, .. }
            | Self::ListLength { list, .. } => {
                let Object::List(values) = heap.get(*list).ok()? else {
                    return None;
                };
                let strategy = match values.strategy() {
                    crate::object::SequenceStrategy::Empty => 0_u8,
                    crate::object::SequenceStrategy::Bool => 1,
                    crate::object::SequenceStrategy::I64 => 2,
                    crate::object::SequenceStrategy::F64 => 3,
                    crate::object::SequenceStrategy::Object => 4,
                };
                strategy.hash(&mut hasher);
            }
            Self::DirectCall { callee, .. } => callee.hash(&mut hasher),
        }
        let value = hasher.finish();
        Some((value as u32) ^ (value >> 32) as u32)
    }

    pub(super) const fn site_operation(&self) -> Option<SiteOperation> {
        match self {
            Self::ObjectGet { .. } => Some(SiteOperation::ObjectGet),
            Self::ListGet { .. } => Some(SiteOperation::ListGet),
            Self::ObjectSet { .. }
            | Self::ListAppend { .. }
            | Self::ListSet { .. }
            | Self::ListInsert { .. }
            | Self::ListPop { .. }
            | Self::ListLength { .. } => None,
            Self::DirectCall { .. } => Some(SiteOperation::DirectCall),
        }
    }

    pub(super) const fn receiver(&self) -> crate::object::ObjectRef {
        match self {
            Self::ObjectGet { receiver, .. } | Self::ObjectSet { receiver, .. } => *receiver,
            Self::ListGet { list, .. }
            | Self::ListAppend { list, .. }
            | Self::ListSet { list, .. }
            | Self::ListInsert { list, .. }
            | Self::ListPop { list, .. }
            | Self::ListLength { list, .. } => *list,
            Self::DirectCall { receiver, .. } => *receiver,
        }
    }

    pub(super) fn bind(
        &self,
        context: &mut AdaptiveNativeContext,
        key: u32,
    ) -> Result<(), crate::adaptive_v2::native::NativeError> {
        if let Self::ObjectGet { field, .. } | Self::ObjectSet { field, .. } = self {
            context.bind_field(i64::from(key), field);
        }
        if let Self::DirectCall { method, .. } = self {
            let method = method.clone();
            context.ensure_binary_callable(u64::from(key), move |argument, _| {
                method.evaluate(argument)
            })?;
        }
        Ok(())
    }

    pub(super) fn inputs(&self, receiver: NativeValue, key: u32) -> Vec<NativeValue> {
        match self {
            Self::ObjectGet { .. } => vec![receiver, NativeValue::Integer(i64::from(key))],
            Self::ObjectSet { value, .. } => vec![
                receiver,
                NativeValue::Integer(i64::from(key)),
                NativeValue::Integer(*value),
            ],
            Self::ListGet { index, .. } => vec![
                receiver,
                NativeValue::Integer(i64::try_from(*index).unwrap_or(i64::MAX)),
            ],
            Self::ListAppend { value, .. } => {
                vec![receiver, NativeValue::Integer(*value)]
            }
            Self::ListSet { .. }
            | Self::ListInsert { .. }
            | Self::ListPop { .. }
            | Self::ListLength { .. } => Vec::new(),
            Self::DirectCall { argument, .. } => {
                vec![NativeValue::Integer(*argument), NativeValue::Integer(0)]
            }
        }
    }

    pub(super) fn execute_authoritative(
        &self,
        context: &mut AdaptiveNativeContext,
        receiver: NativeValue,
        key: u32,
    ) -> Result<Option<(u16, Value)>, crate::adaptive_v2::native::NativeError> {
        match self {
            Self::ObjectSet { field, value, .. } => {
                context.set_integer_field(receiver, i64::from(key), field, *value)?;
                Ok(None)
            }
            Self::ListAppend { value, .. } => {
                context.append_integer(receiver, *value)?;
                Ok(None)
            }
            Self::ObjectGet { field, dst, .. } => context
                .get_integer_field(receiver, i64::from(key), field)
                .map(|value| Some((*dst, Value::SmallInt(value)))),
            Self::ListGet { index, dst, .. } => context
                .integer_at(receiver, *index)
                .map(|value| Some((*dst, Value::SmallInt(value)))),
            Self::ListSet { index, value, .. } => {
                let index = normalize_existing(*index, context.list_len(receiver)?)?;
                context.set_integer_at(receiver, index, *value)?;
                Ok(None)
            }
            Self::ListInsert { index, value, .. } => {
                let index = normalize_insert(*index, context.list_len(receiver)?)?;
                context.insert_integer(receiver, index, *value)?;
                Ok(None)
            }
            Self::ListPop { index, dst, .. } => {
                let index = normalize_existing(*index, context.list_len(receiver)?)?;
                context
                    .pop_integer(receiver, index)
                    .map(|value| Some((*dst, Value::SmallInt(value))))
            }
            Self::ListLength { dst, .. } => {
                let value = i64::try_from(context.list_len(receiver)?)
                    .map_err(|_| crate::adaptive_v2::native::NativeError::CountOverflow)?;
                Ok(Some((*dst, Value::SmallInt(value))))
            }
            Self::DirectCall { argument, dst, .. } => context
                .direct_call_value(u64::from(key), *argument, 0)
                .map(|value| Some((*dst, Value::SmallInt(value)))),
        }
    }

    pub(super) fn output_from(
        &self,
        native: &[NativeValue],
    ) -> Result<Option<(u16, Value)>, crate::adaptive_v2::native::NativeError> {
        match (self, native) {
            (
                Self::ObjectGet { dst, .. } | Self::ListGet { dst, .. },
                [NativeValue::Integer(value)],
            ) => Ok(Some((*dst, Value::SmallInt(*value)))),
            (Self::ObjectSet { .. } | Self::ListAppend { .. }, []) => Ok(None),
            (Self::DirectCall { dst, .. }, [NativeValue::Integer(value)]) => {
                Ok(Some((*dst, Value::SmallInt(*value))))
            }
            (
                Self::ListSet { .. }
                | Self::ListInsert { .. }
                | Self::ListPop { .. }
                | Self::ListLength { .. },
                _,
            ) => Err(crate::adaptive_v2::native::NativeError::MalformedValue),
            (Self::DirectCall { .. }, _) => {
                Err(crate::adaptive_v2::native::NativeError::MalformedValue)
            }
            (Self::ObjectGet { .. } | Self::ListGet { .. }, _)
            | (Self::ObjectSet { .. } | Self::ListAppend { .. }, _) => {
                Err(crate::adaptive_v2::native::NativeError::MalformedValue)
            }
        }
    }
}

fn normalize_existing(
    index: i64,
    length: usize,
) -> Result<usize, crate::adaptive_v2::native::NativeError> {
    let length = i64::try_from(length)
        .map_err(|_| crate::adaptive_v2::native::NativeError::CountOverflow)?;
    let index = if index < 0 {
        length.saturating_add(index)
    } else {
        index
    };
    if !(0..length).contains(&index) {
        return Err(crate::adaptive_v2::native::NativeError::Helper);
    }
    usize::try_from(index).map_err(|_| crate::adaptive_v2::native::NativeError::MalformedValue)
}

fn normalize_insert(
    index: i64,
    length: usize,
) -> Result<usize, crate::adaptive_v2::native::NativeError> {
    let length = i64::try_from(length)
        .map_err(|_| crate::adaptive_v2::native::NativeError::CountOverflow)?;
    let index = if index < 0 {
        length.saturating_add(index).max(0)
    } else {
        index.min(length)
    };
    usize::try_from(index).map_err(|_| crate::adaptive_v2::native::NativeError::MalformedValue)
}

fn object_ref(registers: &[Value], register: u16) -> Option<crate::object::ObjectRef> {
    match registers.get(usize::from(register))? {
        Value::Object(reference) => Some(*reference),
        Value::Float(_)
        | Value::SmallInt(_)
        | Value::Bool(_)
        | Value::None
        | Value::Uninitialized => None,
    }
}

fn integer(registers: &[Value], register: u16) -> Option<i64> {
    match registers.get(usize::from(register))? {
        Value::SmallInt(value) => Some(*value),
        Value::Float(_)
        | Value::Bool(_)
        | Value::None
        | Value::Object(_)
        | Value::Uninitialized => None,
    }
}
