use std::fmt;

/// Scalar values representable in WXIR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WxScalarType {
    I1,
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
    Ptr,
}

/// A scalar or fixed-width SIMD value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WxType {
    Scalar(WxScalarType),
    Vector { lane: WxScalarType, lanes: u16 },
}

impl WxType {
    pub(crate) fn is_pointer(self) -> bool {
        self == Self::Scalar(WxScalarType::Ptr)
    }

    pub(crate) fn is_integer(self) -> bool {
        match self {
            Self::Scalar(lane) | Self::Vector { lane, .. } => matches!(
                lane,
                WxScalarType::I1
                    | WxScalarType::I8
                    | WxScalarType::I16
                    | WxScalarType::I32
                    | WxScalarType::I64
            ),
        }
    }

    pub(crate) fn is_float(self) -> bool {
        match self {
            Self::Scalar(lane) | Self::Vector { lane, .. } => {
                matches!(lane, WxScalarType::F32 | WxScalarType::F64)
            }
        }
    }
}

impl fmt::Display for WxScalarType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::I1 => "i1",
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::Ptr => "ptr",
        };
        formatter.write_str(name)
    }
}

impl fmt::Display for WxType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scalar(scalar) => scalar.fmt(formatter),
            Self::Vector { lane, lanes } => write!(formatter, "{lane}x{lanes}"),
        }
    }
}
