use std::cmp::Ordering;

use num_bigint::BigInt;
use num_traits::ToPrimitive;

use crate::bytecode::{BinaryOperator, CompareOperator, UnaryOperator};
use crate::object::{Object, ObjectHeap};
use crate::value::Value;

use super::equality;

#[path = "numeric_semantics_core.rs"]
pub(super) mod numeric_semantics;
mod sequence;
use numeric_semantics::{Number, compare_numbers, is_zero, number_to_big, number_to_f64};

pub(super) struct ValueOps<'a> {
    heap: &'a mut ObjectHeap,
}

impl<'a> ValueOps<'a> {
    pub(super) const fn new(heap: &'a mut ObjectHeap) -> Self {
        Self { heap }
    }

    pub(super) fn binary(
        &mut self,
        op: BinaryOperator,
        lhs: Value,
        rhs: Value,
    ) -> Result<Value, String> {
        if op == BinaryOperator::Add
            && let Some(value) = self.sequence_add(lhs, rhs)?
        {
            return Ok(value);
        }
        if op == BinaryOperator::Multiply
            && let Some(value) = self.sequence_repeat(lhs, rhs)?
        {
            return Ok(value);
        }
        let lhs_number = self.number(lhs)?;
        let rhs_number = self.number(rhs)?;
        match op {
            BinaryOperator::Add => self.add(lhs_number, rhs_number),
            BinaryOperator::Subtract => self.subtract(lhs_number, rhs_number),
            BinaryOperator::Multiply => self.multiply(lhs_number, rhs_number),
            BinaryOperator::Divide => self.divide(lhs_number, rhs_number),
            BinaryOperator::FloorDivide => self.floor_divide(lhs_number, rhs_number),
            BinaryOperator::Power => self.power(lhs_number, rhs_number),
        }
    }

    pub(super) fn immediate_binary(
        &mut self,
        op: BinaryOperator,
        lhs: Value,
        rhs: Value,
    ) -> Result<Option<Value>, String> {
        let lhs = match lhs {
            Value::SmallInt(value) => Number::Small(value),
            Value::Float(value) => Number::Float(value),
            Value::Bool(_) | Value::None | Value::Object(_) | Value::Uninitialized => {
                return Ok(None);
            }
        };
        let rhs = match rhs {
            Value::SmallInt(value) => Number::Small(value),
            Value::Float(value) => Number::Float(value),
            Value::Bool(_) | Value::None | Value::Object(_) | Value::Uninitialized => {
                return Ok(None);
            }
        };
        let value = match op {
            BinaryOperator::Add => self.add(lhs, rhs)?,
            BinaryOperator::Subtract => self.subtract(lhs, rhs)?,
            BinaryOperator::Multiply => self.multiply(lhs, rhs)?,
            BinaryOperator::Divide => self.divide(lhs, rhs)?,
            BinaryOperator::FloorDivide => self.floor_divide(lhs, rhs)?,
            BinaryOperator::Power => self.power(lhs, rhs)?,
        };
        Ok(Some(value))
    }

    pub(super) fn unary(&mut self, op: UnaryOperator, value: Value) -> Result<Value, String> {
        match (op, value) {
            (UnaryOperator::Not, Value::Bool(value)) => Ok(Value::Bool(!value)),
            (UnaryOperator::Negate, Value::SmallInt(value)) => match value.checked_neg() {
                Some(value) => Ok(Value::SmallInt(value)),
                None => self.allocate_big(-BigInt::from(value)),
            },
            (UnaryOperator::Negate, Value::Float(value)) => Ok(Value::Float(-value)),
            (UnaryOperator::Negate, Value::Object(reference)) => match self.heap.get(reference) {
                Ok(Object::BigInt(value)) => self.allocate_big(-value.clone()),
                Ok(object) => Err(format!("cannot negate {}", object_name(object))),
                Err(error) => Err(error.to_string()),
            },
            (UnaryOperator::Negate, Value::Bool(_) | Value::None | Value::Uninitialized) => {
                Err(format!("cannot negate {}", value_name(self.heap, value)))
            }
            (UnaryOperator::Not, other) => Err(format!(
                "cannot apply not to {}",
                value_name(self.heap, other)
            )),
        }
    }

    pub(super) fn compare(
        &self,
        op: CompareOperator,
        lhs: Value,
        rhs: Value,
    ) -> Result<Value, String> {
        if matches!(op, CompareOperator::Eq | CompareOperator::NotEq) {
            let equal = equality::values_equal(self.heap, lhs, rhs)?;
            return Ok(Value::Bool(if op == CompareOperator::Eq {
                equal
            } else {
                !equal
            }));
        }
        let ordering = match (self.number_optional(lhs)?, self.number_optional(rhs)?) {
            (Some(lhs), Some(rhs)) => compare_numbers(&lhs, &rhs)?,
            (None, None) => self.compare_strings(lhs, rhs)?,
            (Some(_), None) | (None, Some(_)) => {
                return Err("comparison requires two numeric or two string values".to_string());
            }
        };
        let result = match op {
            CompareOperator::Lt => ordering.is_lt(),
            CompareOperator::Le => ordering.is_le(),
            CompareOperator::Gt => ordering.is_gt(),
            CompareOperator::Ge => ordering.is_ge(),
            CompareOperator::Eq | CompareOperator::NotEq => false,
        };
        Ok(Value::Bool(result))
    }

    fn add(&mut self, lhs: Number, rhs: Number) -> Result<Value, String> {
        match (lhs, rhs) {
            (Number::Small(lhs), Number::Small(rhs)) => self.smallint_add(lhs, rhs),
            (Number::Float(lhs), rhs) => Ok(Value::Float(lhs + number_to_f64(&rhs)?)),
            (lhs, Number::Float(rhs)) => Ok(Value::Float(number_to_f64(&lhs)? + rhs)),
            (lhs, rhs) => self.allocate_big(number_to_big(lhs)? + number_to_big(rhs)?),
        }
    }

    pub(super) fn smallint_add(&mut self, lhs: i64, rhs: i64) -> Result<Value, String> {
        match lhs.checked_add(rhs) {
            Some(value) => Ok(Value::SmallInt(value)),
            None => self.allocate_big(BigInt::from(lhs) + rhs),
        }
    }

    fn subtract(&mut self, lhs: Number, rhs: Number) -> Result<Value, String> {
        match (lhs, rhs) {
            (Number::Small(lhs), Number::Small(rhs)) => match lhs.checked_sub(rhs) {
                Some(value) => Ok(Value::SmallInt(value)),
                None => self.allocate_big(BigInt::from(lhs) - rhs),
            },
            (Number::Float(lhs), rhs) => Ok(Value::Float(lhs - number_to_f64(&rhs)?)),
            (lhs, Number::Float(rhs)) => Ok(Value::Float(number_to_f64(&lhs)? - rhs)),
            (lhs, rhs) => self.allocate_big(number_to_big(lhs)? - number_to_big(rhs)?),
        }
    }

    fn multiply(&mut self, lhs: Number, rhs: Number) -> Result<Value, String> {
        match (lhs, rhs) {
            (Number::Small(lhs), Number::Small(rhs)) => match lhs.checked_mul(rhs) {
                Some(value) => Ok(Value::SmallInt(value)),
                None => self.allocate_big(BigInt::from(lhs) * rhs),
            },
            (Number::Float(lhs), rhs) => Ok(Value::Float(lhs * number_to_f64(&rhs)?)),
            (lhs, Number::Float(rhs)) => Ok(Value::Float(number_to_f64(&lhs)? * rhs)),
            (lhs, rhs) => self.allocate_big(number_to_big(lhs)? * number_to_big(rhs)?),
        }
    }

    fn divide(&self, lhs: Number, rhs: Number) -> Result<Value, String> {
        if is_zero(&rhs) {
            return Err("division by zero".to_string());
        }
        let value = match (&lhs, &rhs) {
            (Number::Float(_), _) | (_, Number::Float(_)) => {
                number_to_f64(&lhs)? / number_to_f64(&rhs)?
            }
            _ => {
                numeric_semantics::integer_ratio_to_f64(&number_to_big(lhs)?, &number_to_big(rhs)?)?
            }
        };
        Ok(Value::Float(value))
    }

    fn floor_divide(&mut self, lhs: Number, rhs: Number) -> Result<Value, String> {
        if is_zero(&rhs) {
            return Err("division by zero".to_string());
        }
        if matches!(lhs, Number::Float(_)) || matches!(rhs, Number::Float(_)) {
            return Ok(Value::Float(
                (number_to_f64(&lhs)? / number_to_f64(&rhs)?).floor(),
            ));
        }
        if let (Number::Small(lhs), Number::Small(rhs)) = (&lhs, &rhs)
            && let Some(value) = lhs.checked_div_euclid(*rhs)
        {
            return Ok(Value::SmallInt(value));
        }
        let lhs = number_to_big(lhs)?;
        let rhs = number_to_big(rhs)?;
        let quotient = &lhs / &rhs;
        let remainder = &lhs % &rhs;
        let value = if remainder != BigInt::from(0) && lhs.sign() != rhs.sign() {
            quotient - 1
        } else {
            quotient
        };
        if let Some(value) = value.to_i64() {
            Ok(Value::SmallInt(value))
        } else {
            self.allocate_big(value)
        }
    }

    fn power(&mut self, lhs: Number, rhs: Number) -> Result<Value, String> {
        let base = number_to_f64(&lhs)?;
        let exponent = number_to_f64(&rhs)?;
        Ok(Value::Float(base.powf(exponent)))
    }

    fn number(&self, value: Value) -> Result<Number, String> {
        self.number_optional(value)?.ok_or_else(|| {
            format!(
                "expected numeric value, found {}",
                value_name(self.heap, value)
            )
        })
    }

    fn number_optional(&self, value: Value) -> Result<Option<Number>, String> {
        match value {
            Value::SmallInt(value) => Ok(Some(Number::Small(value))),
            Value::Float(value) => Ok(Some(Number::Float(value))),
            Value::Object(reference) => match self.heap.get(reference) {
                Ok(Object::BigInt(value)) => Ok(Some(Number::Big(value.clone()))),
                Ok(_) => Ok(None),
                Err(error) => Err(error.to_string()),
            },
            Value::Bool(_) | Value::None | Value::Uninitialized => Ok(None),
        }
    }

    fn compare_strings(&self, lhs: Value, rhs: Value) -> Result<Ordering, String> {
        match (lhs, rhs) {
            (Value::Object(lhs), Value::Object(rhs)) => {
                match (self.heap.get(lhs), self.heap.get(rhs)) {
                    (Ok(Object::String(lhs)), Ok(Object::String(rhs))) => Ok(lhs.cmp(rhs)),
                    (Err(error), _) | (_, Err(error)) => Err(error.to_string()),
                    _ => Err("ordered comparison requires numeric or string values".to_string()),
                }
            }
            _ => Err("ordered comparison requires numeric or string values".to_string()),
        }
    }

    fn allocate_big(&mut self, value: BigInt) -> Result<Value, String> {
        self.allocate(Object::BigInt(value))
    }

    fn allocate(&mut self, object: Object) -> Result<Value, String> {
        self.heap
            .allocate(object)
            .map(Value::Object)
            .map_err(|error| error.to_string())
    }
}

pub(super) fn value_name(heap: &ObjectHeap, value: Value) -> &'static str {
    match value {
        Value::SmallInt(_) => "SmallInt",
        Value::Float(_) => "float",
        Value::Bool(_) => "bool",
        Value::None => "none",
        Value::Object(reference) => heap.get(reference).map_or("invalid object", object_name),
        Value::Uninitialized => "uninitialized",
    }
}

const fn object_name(object: &Object) -> &'static str {
    match object {
        Object::String(_) => "string",
        Object::Tuple(_) => "tuple",
        Object::BigInt(_) => "BigInt",
        Object::List(_) => "list",
        Object::Dict(_) => "dict",
        Object::Function(_) => "function",
        Object::Class(_) => "class",
        Object::Instance(_) => "instance",
        Object::BoundMethod(_) => "bound method",
    }
}

#[cfg(test)]
#[path = "arithmetic/tests.rs"]
mod tests;
