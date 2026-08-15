use std::cmp::Ordering;

use num_bigint::{BigInt, BigUint};
use num_traits::{FromPrimitive, ToPrimitive, Zero};
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Number {
    Small(i64),
    Big(BigInt),
    Float(f64),
}

pub(super) fn number_to_big(number: Number) -> Result<BigInt, String> {
    match number {
        Number::Small(value) => Ok(BigInt::from(value)),
        Number::Big(value) => Ok(value.clone()),
        Number::Float(_) => Err("float cannot enter exact integer arithmetic".to_string()),
    }
}

pub(super) fn number_to_f64(number: &Number) -> Result<f64, String> {
    match number {
        Number::Small(value) => value
            .to_f64()
            .ok_or_else(|| "SmallInt cannot be represented as float".to_string()),
        Number::Big(value) => value
            .to_f64()
            .filter(|value| value.is_finite())
            .ok_or_else(|| "BigInt cannot be represented as a finite float".to_string()),
        Number::Float(value) => Ok(*value),
    }
}

pub(super) fn compare_numbers(lhs: &Number, rhs: &Number) -> Result<Ordering, String> {
    match (lhs, rhs) {
        (Number::Float(lhs), Number::Float(rhs)) => lhs
            .partial_cmp(rhs)
            .ok_or_else(|| "NaN is not orderable".to_string()),
        (Number::Float(lhs), Number::Small(rhs)) => {
            compare_integer_float(&BigInt::from(*rhs), *lhs).map(Ordering::reverse)
        }
        (Number::Float(lhs), Number::Big(rhs)) => {
            compare_integer_float(rhs, *lhs).map(Ordering::reverse)
        }
        (Number::Small(lhs), Number::Float(rhs)) => {
            compare_integer_float(&BigInt::from(*lhs), *rhs)
        }
        (Number::Big(lhs), Number::Float(rhs)) => compare_integer_float(lhs, *rhs),
        (lhs, rhs) => Ok(number_to_big(lhs.clone())?.cmp(&number_to_big(rhs.clone())?)),
    }
}

pub(super) fn is_zero(number: &Number) -> bool {
    match number {
        Number::Small(value) => *value == 0,
        Number::Big(value) => value.is_zero(),
        Number::Float(value) => *value == 0.0,
    }
}

pub(in crate::wvm) fn integer_float_equal(integer: &BigInt, float: f64) -> bool {
    if !float.is_finite() || float.fract() != 0.0 {
        return false;
    }
    BigInt::from_f64(float).is_some_and(|value| value == *integer)
}

pub(super) fn compare_integer_float(integer: &BigInt, float: f64) -> Result<Ordering, String> {
    if float.is_nan() {
        return Err("NaN is not orderable".to_string());
    }
    if float == f64::INFINITY {
        return Ok(Ordering::Less);
    }
    if float == f64::NEG_INFINITY {
        return Ok(Ordering::Greater);
    }

    let truncated = BigInt::from_f64(float)
        .ok_or_else(|| "finite float could not be converted to an integer".to_string())?;
    match integer.cmp(&truncated) {
        Ordering::Equal if float.fract() > 0.0 => Ok(Ordering::Less),
        Ordering::Equal if float.fract() < 0.0 => Ok(Ordering::Greater),
        ordering => Ok(ordering),
    }
}

pub(super) fn integer_ratio_to_f64(
    numerator: &BigInt,
    denominator: &BigInt,
) -> Result<f64, String> {
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    let quotient_float = quotient
        .to_f64()
        .filter(|value| value.is_finite())
        .ok_or_else(|| "integer quotient cannot be represented as a finite float".to_string())?;
    if remainder.is_zero() {
        return Ok(quotient_float);
    }

    let fraction = magnitude_ratio(remainder.magnitude(), denominator.magnitude())?;
    if remainder.sign() == denominator.sign() {
        Ok(quotient_float + fraction)
    } else {
        Ok(quotient_float - fraction)
    }
}

fn magnitude_ratio(numerator: &BigUint, denominator: &BigUint) -> Result<f64, String> {
    let numerator_shift = numerator.bits().saturating_sub(53);
    let denominator_shift = denominator.bits().saturating_sub(53);
    let numerator_top = (numerator
        >> usize::try_from(numerator_shift)
            .map_err(|_| "integer magnitude is too large to scale".to_string())?)
    .to_f64()
    .ok_or_else(|| "integer remainder cannot be represented as a float".to_string())?;
    let denominator_top = (denominator
        >> usize::try_from(denominator_shift)
            .map_err(|_| "integer magnitude is too large to scale".to_string())?)
    .to_f64()
    .ok_or_else(|| "integer divisor cannot be represented as a float".to_string())?;
    let exponent = i64::try_from(numerator_shift)
        .and_then(|lhs| i64::try_from(denominator_shift).map(|rhs| lhs - rhs))
        .unwrap_or(i64::MIN);
    if exponent < -1_074 {
        return Ok(0.0);
    }
    let exponent = i32::try_from(exponent)
        .map_err(|_| "integer ratio exponent cannot be represented as a float".to_string())?;
    Ok((numerator_top / denominator_top) * 2.0_f64.powi(exponent))
}
