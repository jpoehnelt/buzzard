//! Data types, sample types and property values.

use std::fmt;

use bytemuck::{Pod, Zeroable};

/// Seconds between the TDMS epoch (1904-01-01T00:00:00Z) and the Unix epoch.
pub const EPOCH_OFFSET_SECONDS: i64 = 2_082_844_800;

/// 2^64 as a float.
const TWO_POW_64: f64 = 4_294_967_296.0 * 4_294_967_296.0;

/// The type of a channel's values or of a property, as stored in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum DataType {
    /// No data (code 0).
    Void,
    /// Signed 8-bit integer.
    I8,
    /// Signed 16-bit integer.
    I16,
    /// Signed 32-bit integer.
    I32,
    /// Signed 64-bit integer.
    I64,
    /// Unsigned 8-bit integer.
    U8,
    /// Unsigned 16-bit integer.
    U16,
    /// Unsigned 32-bit integer.
    U32,
    /// Unsigned 64-bit integer.
    U64,
    /// 32-bit float.
    F32,
    /// 64-bit float.
    F64,
    /// Extended-precision float. Its layout is not published; channels of
    /// this type cannot be read.
    ExtendedFloat,
    /// 32-bit float whose unit is in the `unit_string` property.
    F32WithUnit,
    /// 64-bit float whose unit is in the `unit_string` property.
    F64WithUnit,
    /// Extended-precision float with unit; not readable.
    ExtendedFloatWithUnit,
    /// UTF-8 string.
    String,
    /// One byte, zero or one.
    Bool,
    /// 16-byte [`Timestamp`].
    Timestamp,
    /// Fixed-point number. Its layout is not published; not readable.
    FixedPoint,
    /// Two 32-bit floats: real, imaginary.
    ComplexF32,
    /// Two 64-bit floats: real, imaginary.
    ComplexF64,
    /// Raw buffers of a data-acquisition driver, read through scalers.
    DaqmxRaw,
}

impl DataType {
    /// The type for a `tdsDataType` code, or `None` for an unknown code.
    pub fn from_code(code: u32) -> Option<Self> {
        Some(match code {
            0 => Self::Void,
            1 => Self::I8,
            2 => Self::I16,
            3 => Self::I32,
            4 => Self::I64,
            5 => Self::U8,
            6 => Self::U16,
            7 => Self::U32,
            8 => Self::U64,
            9 => Self::F32,
            10 => Self::F64,
            11 => Self::ExtendedFloat,
            0x19 => Self::F32WithUnit,
            0x1A => Self::F64WithUnit,
            0x1B => Self::ExtendedFloatWithUnit,
            0x20 => Self::String,
            0x21 => Self::Bool,
            0x44 => Self::Timestamp,
            0x4F => Self::FixedPoint,
            0x0008_000C => Self::ComplexF32,
            0x0010_000D => Self::ComplexF64,
            0xFFFF_FFFF => Self::DaqmxRaw,
            _ => return None,
        })
    }

    /// The `tdsDataType` code stored in the file.
    pub fn code(self) -> u32 {
        match self {
            Self::Void => 0,
            Self::I8 => 1,
            Self::I16 => 2,
            Self::I32 => 3,
            Self::I64 => 4,
            Self::U8 => 5,
            Self::U16 => 6,
            Self::U32 => 7,
            Self::U64 => 8,
            Self::F32 => 9,
            Self::F64 => 10,
            Self::ExtendedFloat => 11,
            Self::F32WithUnit => 0x19,
            Self::F64WithUnit => 0x1A,
            Self::ExtendedFloatWithUnit => 0x1B,
            Self::String => 0x20,
            Self::Bool => 0x21,
            Self::Timestamp => 0x44,
            Self::FixedPoint => 0x4F,
            Self::ComplexF32 => 0x0008_000C,
            Self::ComplexF64 => 0x0010_000D,
            Self::DaqmxRaw => 0xFFFF_FFFF,
        }
    }

    /// Bytes per value for the fixed-size types that this crate can read.
    ///
    /// `None` for strings, raw driver data, `Void`, and the types whose
    /// layout is not published (extended floats, fixed point).
    pub fn size(self) -> Option<u64> {
        Some(match self {
            Self::I8 | Self::U8 | Self::Bool => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 | Self::F32WithUnit => 4,
            Self::I64 | Self::U64 | Self::F64 | Self::F64WithUnit | Self::ComplexF32 => 8,
            Self::Timestamp | Self::ComplexF64 => 16,
            Self::Void
            | Self::ExtendedFloat
            | Self::ExtendedFloatWithUnit
            | Self::String
            | Self::FixedPoint
            | Self::DaqmxRaw => return None,
        })
    }

    /// A short, stable, lower-case name: `i8`, `f64`, `string`, `timestamp`…
    ///
    /// The command line and the export metadata print these names.
    pub fn name(self) -> &'static str {
        match self {
            Self::Void => "void",
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::ExtendedFloat => "extended",
            Self::F32WithUnit => "f32_with_unit",
            Self::F64WithUnit => "f64_with_unit",
            Self::ExtendedFloatWithUnit => "extended_with_unit",
            Self::String => "string",
            Self::Bool => "bool",
            Self::Timestamp => "timestamp",
            Self::FixedPoint => "fixed_point",
            Self::ComplexF32 => "complex_f32",
            Self::ComplexF64 => "complex_f64",
            Self::DaqmxRaw => "daqmx_raw",
        }
    }

    /// The type without its "with unit" marker.
    #[must_use]
    pub fn without_unit(self) -> Self {
        match self {
            Self::F32WithUnit => Self::F32,
            Self::F64WithUnit => Self::F64,
            Self::ExtendedFloatWithUnit => Self::ExtendedFloat,
            other => other,
        }
    }
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A point in time as the file stores it: whole seconds since
/// 1904-01-01T00:00:00Z and a positive fraction in units of 2^-64 s.
///
/// Nothing is lost on reading; the conversions below state what they drop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Pod, Zeroable)]
#[repr(C)]
pub struct Timestamp {
    /// Positive fraction of a second, in units of 2^-64 s.
    pub fractions: u64,
    /// Whole seconds since 1904-01-01T00:00:00Z, ignoring leap seconds.
    pub seconds: i64,
}

impl PartialOrd for Timestamp {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Timestamp {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.seconds, self.fractions).cmp(&(other.seconds, other.fractions))
    }
}

impl Timestamp {
    /// A timestamp from its two stored fields.
    pub fn new(seconds: i64, fractions: u64) -> Self {
        Self { fractions, seconds }
    }

    /// The fraction as whole nanoseconds, rounded down (0 to 999,999,999).
    pub fn subsec_nanos(self) -> u32 {
        let nanos = (u128::from(self.fractions) * 1_000_000_000) >> 64;
        u32::try_from(nanos).unwrap_or(999_999_999)
    }

    /// Nanoseconds since the Unix epoch, rounded down to a whole nanosecond.
    pub fn to_unix_nanos(self) -> i128 {
        (i128::from(self.seconds) - i128::from(EPOCH_OFFSET_SECONDS)) * 1_000_000_000
            + i128::from(self.subsec_nanos())
    }

    /// [`Self::to_unix_nanos`] as an `i64`, or `None` outside the years
    /// 1677 to 2262 that 64 bits of nanoseconds can hold.
    pub fn to_unix_nanos_i64(self) -> Option<i64> {
        i64::try_from(self.to_unix_nanos()).ok()
    }

    /// Seconds since the Unix epoch as a float; exact to about 100 ns for
    /// present-day times.
    #[expect(
        clippy::cast_precision_loss,
        reason = "a float cannot hold 128 bits; the documentation says so"
    )]
    pub fn to_unix_seconds_f64(self) -> f64 {
        (self.seconds - EPOCH_OFFSET_SECONDS) as f64 + self.fractions as f64 / TWO_POW_64
    }

    /// The time in UTC as `YYYY-MM-DDTHH:MM:SS.nnnnnnnnnZ`, rounded down to
    /// a whole nanosecond. Years outside 0000 to 9999 are printed with a
    /// sign and as many digits as they need.
    pub fn to_rfc3339(self) -> String {
        let unix = i128::from(self.seconds) - i128::from(EPOCH_OFFSET_SECONDS);
        let days = unix.div_euclid(86_400);
        let secs = unix.rem_euclid(86_400);
        // Civil date from a day count (proleptic Gregorian calendar).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i128::from(month <= 2);
        let (h, m, s) = (secs / 3_600, secs % 3_600 / 60, secs % 60);
        let nanos = self.subsec_nanos();
        if (0..=9_999).contains(&year) {
            format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}.{nanos:09}Z")
        } else {
            format!("{year:+05}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}.{nanos:09}Z")
        }
    }
}

/// A complex number of two 32-bit floats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C)]
pub struct Complex32 {
    /// Real part.
    pub re: f32,
    /// Imaginary part.
    pub im: f32,
}

/// A complex number of two 64-bit floats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C)]
pub struct Complex64 {
    /// Real part.
    pub re: f64,
    /// Imaginary part.
    pub im: f64,
}

/// How the bytes of one value are rearranged between file and memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    /// One number of the value's whole size.
    Scalar,
    /// Two numbers of half the size each, in order (complex).
    Pair,
    /// A [`Timestamp`].
    Time,
}

mod sealed {
    pub trait Sealed {}
}

/// A value type that channel data can be read into.
///
/// Implemented for the integer and float primitives, [`Timestamp`],
/// [`Complex32`] and [`Complex64`]. Booleans are read as `u8`. The trait is
/// sealed.
pub trait Sample: Pod + Send + Sync + sealed::Sealed {
    /// The name used in error messages.
    const NAME: &'static str;
    /// Whether a channel that stores `dtype` can be read as this type.
    fn accepts(dtype: DataType) -> bool;
}

macro_rules! sample {
    ($t:ty, $($dtype:pat_param)|+) => {
        impl sealed::Sealed for $t {}
        impl Sample for $t {
            const NAME: &'static str = stringify!($t);
            fn accepts(dtype: DataType) -> bool {
                matches!(dtype, $($dtype)|+)
            }
        }
    };
}

sample!(i8, DataType::I8);
sample!(i16, DataType::I16);
sample!(i32, DataType::I32);
sample!(i64, DataType::I64);
sample!(u8, DataType::U8 | DataType::Bool);
sample!(u16, DataType::U16);
sample!(u32, DataType::U32);
sample!(u64, DataType::U64);
sample!(f32, DataType::F32 | DataType::F32WithUnit);
sample!(f64, DataType::F64 | DataType::F64WithUnit);
sample!(Timestamp, DataType::Timestamp);
sample!(Complex32, DataType::ComplexF32);
sample!(Complex64, DataType::ComplexF64);

/// The byte layout of the values of a readable fixed-size type.
pub(crate) fn shape_of(dtype: DataType) -> Shape {
    match dtype {
        DataType::Timestamp => Shape::Time,
        DataType::ComplexF32 | DataType::ComplexF64 => Shape::Pair,
        _ => Shape::Scalar,
    }
}

/// The value of a property.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum PropertyValue {
    /// Signed 8-bit integer.
    I8(i8),
    /// Signed 16-bit integer.
    I16(i16),
    /// Signed 32-bit integer.
    I32(i32),
    /// Signed 64-bit integer.
    I64(i64),
    /// Unsigned 8-bit integer.
    U8(u8),
    /// Unsigned 16-bit integer.
    U16(u16),
    /// Unsigned 32-bit integer.
    U32(u32),
    /// Unsigned 64-bit integer.
    U64(u64),
    /// 32-bit float (also for the "with unit" type).
    F32(f32),
    /// 64-bit float (also for the "with unit" type).
    F64(f64),
    /// Text. Invalid UTF-8 in the file is replaced with U+FFFD.
    String(String),
    /// Boolean.
    Bool(bool),
    /// Time.
    Timestamp(Timestamp),
    /// Complex number of 32-bit floats.
    ComplexF32(Complex32),
    /// Complex number of 64-bit floats.
    ComplexF64(Complex64),
}

impl PropertyValue {
    /// The value as a float, for every numeric and boolean value. 64-bit
    /// integers beyond 2^53 lose their low bits.
    #[expect(
        clippy::cast_precision_loss,
        reason = "documented: a convenience view, not the stored value"
    )]
    pub fn as_f64(&self) -> Option<f64> {
        Some(match *self {
            Self::I8(v) => f64::from(v),
            Self::I16(v) => f64::from(v),
            Self::I32(v) => f64::from(v),
            Self::I64(v) => v as f64,
            Self::U8(v) => f64::from(v),
            Self::U16(v) => f64::from(v),
            Self::U32(v) => f64::from(v),
            Self::U64(v) => v as f64,
            Self::F32(v) => f64::from(v),
            Self::F64(v) => v,
            Self::Bool(v) => f64::from(u8::from(v)),
            _ => return None,
        })
    }

    /// The value as an unsigned integer, when it is a non-negative integer.
    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            Self::I8(v) => u64::try_from(v).ok(),
            Self::I16(v) => u64::try_from(v).ok(),
            Self::I32(v) => u64::try_from(v).ok(),
            Self::I64(v) => u64::try_from(v).ok(),
            Self::U8(v) => Some(u64::from(v)),
            Self::U16(v) => Some(u64::from(v)),
            Self::U32(v) => Some(u64::from(v)),
            Self::U64(v) => Some(v),
            _ => None,
        }
    }

    /// The text of a string value.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    /// The time of a timestamp value.
    pub fn as_timestamp(&self) -> Option<Timestamp> {
        match *self {
            Self::Timestamp(t) => Some(t),
            _ => None,
        }
    }
}

/// Prints the value the way the command line and the exports show it:
/// numbers in their shortest exact form, times as RFC 3339, complex numbers
/// as `re+imi`.
impl fmt::Display for PropertyValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::I8(v) => write!(f, "{v}"),
            Self::I16(v) => write!(f, "{v}"),
            Self::I32(v) => write!(f, "{v}"),
            Self::I64(v) => write!(f, "{v}"),
            Self::U8(v) => write!(f, "{v}"),
            Self::U16(v) => write!(f, "{v}"),
            Self::U32(v) => write!(f, "{v}"),
            Self::U64(v) => write!(f, "{v}"),
            Self::F32(v) => write!(f, "{v}"),
            Self::F64(v) => write!(f, "{v}"),
            Self::String(v) => f.write_str(v),
            Self::Bool(v) => write!(f, "{v}"),
            Self::Timestamp(v) => f.write_str(&v.to_rfc3339()),
            Self::ComplexF32(v) => write!(f, "{}{:+}i", v.re, v.im),
            Self::ComplexF64(v) => write!(f, "{}{:+}i", v.re, v.im),
        }
    }
}

/// A named, typed value attached to the file, a group or a channel.
#[derive(Clone, Debug, PartialEq)]
pub struct Property {
    /// The property's name.
    pub name: String,
    /// The type code the file gave the value.
    pub dtype: DataType,
    /// The value.
    pub value: PropertyValue,
}

/// The values of one channel, in the type the file stores.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Column {
    /// Signed 8-bit integers.
    I8(Vec<i8>),
    /// Signed 16-bit integers.
    I16(Vec<i16>),
    /// Signed 32-bit integers.
    I32(Vec<i32>),
    /// Signed 64-bit integers.
    I64(Vec<i64>),
    /// Unsigned 8-bit integers.
    U8(Vec<u8>),
    /// Unsigned 16-bit integers.
    U16(Vec<u16>),
    /// Unsigned 32-bit integers.
    U32(Vec<u32>),
    /// Unsigned 64-bit integers.
    U64(Vec<u64>),
    /// 32-bit floats.
    F32(Vec<f32>),
    /// 64-bit floats.
    F64(Vec<f64>),
    /// Booleans, one byte each as stored (zero is false).
    Bool(Vec<u8>),
    /// Times.
    Timestamp(Vec<Timestamp>),
    /// Complex numbers of 32-bit floats.
    ComplexF32(Vec<Complex32>),
    /// Complex numbers of 64-bit floats.
    ComplexF64(Vec<Complex64>),
    /// Strings. Invalid UTF-8 in the file is replaced with U+FFFD.
    String(Vec<String>),
}

impl Column {
    /// Number of values.
    pub fn len(&self) -> usize {
        match self {
            Self::I8(v) => v.len(),
            Self::I16(v) => v.len(),
            Self::I32(v) => v.len(),
            Self::I64(v) => v.len(),
            Self::U8(v) | Self::Bool(v) => v.len(),
            Self::U16(v) => v.len(),
            Self::U32(v) => v.len(),
            Self::U64(v) => v.len(),
            Self::F32(v) => v.len(),
            Self::F64(v) => v.len(),
            Self::Timestamp(v) => v.len(),
            Self::ComplexF32(v) => v.len(),
            Self::ComplexF64(v) => v.len(),
            Self::String(v) => v.len(),
        }
    }

    /// Whether the column holds no values.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The type of the values (without a "with unit" marker).
    pub fn dtype(&self) -> DataType {
        match self {
            Self::I8(_) => DataType::I8,
            Self::I16(_) => DataType::I16,
            Self::I32(_) => DataType::I32,
            Self::I64(_) => DataType::I64,
            Self::U8(_) => DataType::U8,
            Self::U16(_) => DataType::U16,
            Self::U32(_) => DataType::U32,
            Self::U64(_) => DataType::U64,
            Self::F32(_) => DataType::F32,
            Self::F64(_) => DataType::F64,
            Self::Bool(_) => DataType::Bool,
            Self::Timestamp(_) => DataType::Timestamp,
            Self::ComplexF32(_) => DataType::ComplexF32,
            Self::ComplexF64(_) => DataType::ComplexF64,
            Self::String(_) => DataType::String,
        }
    }

    /// An empty column for a readable type; `None` for the others.
    pub fn empty(dtype: DataType) -> Option<Self> {
        Some(match dtype.without_unit() {
            DataType::I8 => Self::I8(Vec::new()),
            DataType::I16 => Self::I16(Vec::new()),
            DataType::I32 => Self::I32(Vec::new()),
            DataType::I64 => Self::I64(Vec::new()),
            DataType::U8 => Self::U8(Vec::new()),
            DataType::U16 => Self::U16(Vec::new()),
            DataType::U32 => Self::U32(Vec::new()),
            DataType::U64 => Self::U64(Vec::new()),
            DataType::F32 => Self::F32(Vec::new()),
            DataType::F64 => Self::F64(Vec::new()),
            DataType::Bool => Self::Bool(Vec::new()),
            DataType::Timestamp => Self::Timestamp(Vec::new()),
            DataType::ComplexF32 => Self::ComplexF32(Vec::new()),
            DataType::ComplexF64 => Self::ComplexF64(Vec::new()),
            DataType::String => Self::String(Vec::new()),
            _ => return None,
        })
    }

    /// The values in the file's little-endian byte layout, one after
    /// another. Strings are written as their UTF-8 bytes, each followed by a
    /// zero byte. The conformance tests hash this.
    pub fn to_le_bytes(&self) -> Vec<u8> {
        fn flat<T: Copy, const N: usize>(values: &[T], f: impl Fn(T) -> [u8; N]) -> Vec<u8> {
            values.iter().flat_map(|v| f(*v)).collect()
        }
        match self {
            Self::I8(v) => flat(v, i8::to_le_bytes),
            Self::I16(v) => flat(v, i16::to_le_bytes),
            Self::I32(v) => flat(v, i32::to_le_bytes),
            Self::I64(v) => flat(v, i64::to_le_bytes),
            Self::U8(v) | Self::Bool(v) => v.clone(),
            Self::U16(v) => flat(v, u16::to_le_bytes),
            Self::U32(v) => flat(v, u32::to_le_bytes),
            Self::U64(v) => flat(v, u64::to_le_bytes),
            Self::F32(v) => flat(v, f32::to_le_bytes),
            Self::F64(v) => flat(v, f64::to_le_bytes),
            Self::Timestamp(v) => v
                .iter()
                .flat_map(|t| {
                    let mut b = [0u8; 16];
                    b[..8].copy_from_slice(&t.fractions.to_le_bytes());
                    b[8..].copy_from_slice(&t.seconds.to_le_bytes());
                    b
                })
                .collect(),
            Self::ComplexF32(v) => v
                .iter()
                .flat_map(|c| [c.re.to_le_bytes(), c.im.to_le_bytes()].concat())
                .collect(),
            Self::ComplexF64(v) => v
                .iter()
                .flat_map(|c| [c.re.to_le_bytes(), c.im.to_le_bytes()].concat())
                .collect(),
            Self::String(v) => v
                .iter()
                .flat_map(|s| s.bytes().chain(std::iter::once(0)))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip() {
        for code in (0..0x60).chain([0x0008_000C, 0x0010_000D, 0xFFFF_FFFF]) {
            if let Some(dtype) = DataType::from_code(code) {
                assert_eq!(dtype.code(), code);
            }
        }
        assert_eq!(DataType::from_code(12), None);
    }

    #[test]
    fn timestamps_convert() {
        let epoch = Timestamp::new(0, 0);
        assert_eq!(epoch.to_rfc3339(), "1904-01-01T00:00:00.000000000Z");
        let unix = Timestamp::new(EPOCH_OFFSET_SECONDS, 1 << 63);
        assert_eq!(unix.to_rfc3339(), "1970-01-01T00:00:00.500000000Z");
        assert_eq!(unix.to_unix_nanos(), 500_000_000);
        let leap = Timestamp::new(EPOCH_OFFSET_SECONDS + 1_709_210_096, 0);
        assert_eq!(leap.to_rfc3339(), "2024-02-29T12:34:56.000000000Z");
        let before = Timestamp::new(-1, u64::MAX);
        assert_eq!(before.to_rfc3339(), "1903-12-31T23:59:59.999999999Z");
        assert_eq!(Timestamp::new(i64::MAX, 0).to_unix_nanos_i64(), None);
        assert!(Timestamp::new(i64::MIN, 0).to_rfc3339().starts_with('-'));
    }
}
