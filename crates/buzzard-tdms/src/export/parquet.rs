//! Parquet export.
//!
//! Column types: integers and floats keep their width; booleans are
//! `BOOLEAN`; strings are UTF-8; a complex channel becomes two float
//! columns, `.re` and `.im`; a time becomes a UTC timestamp in nanoseconds
//! since the Unix epoch, rounded down, and is missing where the file's
//! value lies outside the years 1677 to 2262 that this can hold.
//!
//! Every property of the file, of the exported channels and of their
//! groups is in the file's key-value metadata, as text:
//!
//! | Key | Value |
//! | --- | --- |
//! | `tdms.file.<property>` | a property of the file |
//! | `tdms.group.<group>.<property>` | a property of a group |
//! | `tdms.channel.<column>.<property>` | a property of a channel |
//! | `buzzard.channel.<column>.path`, `.dtype`, `.length` | the channel's path, stored type and length |
//! | `buzzard.channel.<column>.scaling` | `linear <slope> <intercept>` when the column was scaled |
//! | `buzzard.channel.<column>.wf_start_time`, `.wf_start_offset`, `.wf_increment` | the channel's waveform timing |
//! | `buzzard.time.column`, `.increment_s`, `.start_offset_s`, `.start` | the generated time column, when there is one |
//! | `buzzard.tdms.version` | the version of this crate |
//!
//! The output depends only on the input and the options: the keys are in a
//! fixed order, no creation time is written, and the Arrow schema is not
//! embedded (its field metadata would be serialized in hash order).

use std::io::Write;
use std::sync::Arc;

use arrow_array::types::{
    Float32Type, Float64Type, Int16Type, Int32Type, Int64Type, Int8Type, TimestampNanosecondType,
    UInt16Type, UInt32Type, UInt64Type, UInt8Type,
};
use arrow_array::{ArrayRef, ArrowPrimitiveType, BooleanArray, PrimitiveArray, StringArray};
use arrow_schema::{DataType as ArrowType, Field, Schema, TimeUnit};
use parquet::arrow::arrow_writer::{compute_leaves, ArrowRowGroupWriterFactory};
use parquet::arrow::ArrowSchemaConverter;
use parquet::basic::Compression as Codec;
use parquet::file::metadata::KeyValue;
use parquet::file::properties::{EnabledStatistics, WriterProperties};
use parquet::file::writer::SerializedFileWriter;
use rayon::prelude::*;

use super::{ExportOptions, Exported, Plan, TIME_COLUMN};
use crate::error::{Error, Result};
use crate::file::{Channel, TdmsFile};
use crate::types::{Column, DataType};

/// How the column data is compressed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Compression {
    /// Snappy: fast, read by every Parquet reader. The default.
    #[default]
    Snappy,
    /// No compression.
    None,
}

fn failed(e: impl std::fmt::Display) -> Error {
    Error::Export(e.to_string())
}

fn arrow_type(dtype: DataType) -> ArrowType {
    match dtype {
        DataType::I8 => ArrowType::Int8,
        DataType::I16 => ArrowType::Int16,
        DataType::I32 => ArrowType::Int32,
        DataType::I64 => ArrowType::Int64,
        DataType::U8 => ArrowType::UInt8,
        DataType::U16 => ArrowType::UInt16,
        DataType::U32 => ArrowType::UInt32,
        DataType::U64 => ArrowType::UInt64,
        DataType::F32 | DataType::ComplexF32 => ArrowType::Float32,
        DataType::Bool => ArrowType::Boolean,
        DataType::Timestamp => ArrowType::Timestamp(TimeUnit::Nanosecond, Some("UTC".into())),
        DataType::String => ArrowType::Utf8,
        _ => ArrowType::Float64,
    }
}

/// An array of `rows` values: the given ones, then missing values.
fn primitive<T: ArrowPrimitiveType>(values: Vec<T::Native>, rows: usize) -> ArrayRef
where
    PrimitiveArray<T>: From<Vec<T::Native>> + From<Vec<Option<T::Native>>>,
{
    if values.len() == rows {
        Arc::new(PrimitiveArray::<T>::from(values))
    } else {
        let mut padded: Vec<Option<T::Native>> = values.into_iter().map(Some).collect();
        padded.resize(rows, None);
        Arc::new(PrimitiveArray::<T>::from(padded))
    }
}

/// The arrays for one column of the table: one, or two for complex values.
fn arrays(column: Column, rows: usize) -> Vec<ArrayRef> {
    match column {
        Column::I8(v) => vec![primitive::<Int8Type>(v, rows)],
        Column::I16(v) => vec![primitive::<Int16Type>(v, rows)],
        Column::I32(v) => vec![primitive::<Int32Type>(v, rows)],
        Column::I64(v) => vec![primitive::<Int64Type>(v, rows)],
        Column::U8(v) => vec![primitive::<UInt8Type>(v, rows)],
        Column::U16(v) => vec![primitive::<UInt16Type>(v, rows)],
        Column::U32(v) => vec![primitive::<UInt32Type>(v, rows)],
        Column::U64(v) => vec![primitive::<UInt64Type>(v, rows)],
        Column::F32(v) => vec![primitive::<Float32Type>(v, rows)],
        Column::F64(v) => vec![primitive::<Float64Type>(v, rows)],
        Column::Bool(v) => {
            let mut values: Vec<Option<bool>> = v.into_iter().map(|b| Some(b != 0)).collect();
            values.resize(rows, None);
            vec![Arc::new(BooleanArray::from(values))]
        }
        Column::Timestamp(v) => {
            let mut values: Vec<Option<i64>> = v.iter().map(|t| t.to_unix_nanos_i64()).collect();
            values.resize(rows, None);
            vec![Arc::new(
                PrimitiveArray::<TimestampNanosecondType>::from(values).with_timezone("UTC"),
            )]
        }
        Column::ComplexF32(v) => vec![
            primitive::<Float32Type>(v.iter().map(|c| c.re).collect(), rows),
            primitive::<Float32Type>(v.iter().map(|c| c.im).collect(), rows),
        ],
        Column::ComplexF64(v) => vec![
            primitive::<Float64Type>(v.iter().map(|c| c.re).collect(), rows),
            primitive::<Float64Type>(v.iter().map(|c| c.im).collect(), rows),
        ],
        Column::String(v) => {
            let mut values: Vec<Option<String>> = v.into_iter().map(Some).collect();
            values.resize(rows, None);
            vec![Arc::new(StringArray::from(values))]
        }
    }
}

/// Writes the channels as one Parquet table; see the [module](self) for
/// its shape.
///
/// Each batch of rows becomes one row group. Its columns are decoded from
/// the TDMS file and encoded in parallel on the current rayon thread pool.
///
/// # Errors
///
/// When a channel cannot be read or exported, or writing fails.
pub fn write_parquet<W: Write + Send>(
    file: &TdmsFile,
    channels: &[Channel<'_>],
    out: W,
    options: &ExportOptions,
    compression: Compression,
) -> Result<Exported> {
    let plan = Plan::new(file, channels, options)?;
    let mut fields = Vec::new();
    if plan.time.is_some() {
        fields.push(Field::new(TIME_COLUMN, ArrowType::Float64, false));
    }
    for column in &plan.columns {
        let nullable = column.channel.len() < plan.rows || column.dtype == DataType::Timestamp;
        let kind = arrow_type(column.dtype);
        if matches!(column.dtype, DataType::ComplexF32 | DataType::ComplexF64) {
            fields.push(Field::new(
                format!("{}.re", column.name),
                kind.clone(),
                nullable,
            ));
            fields.push(Field::new(format!("{}.im", column.name), kind, nullable));
        } else {
            fields.push(Field::new(&column.name, kind, nullable));
        }
    }
    let schema = Arc::new(Schema::new(fields));
    let metadata = plan
        .metadata()
        .into_iter()
        .map(|(key, value)| KeyValue::new(key, value))
        .collect();
    let properties = Arc::new(
        WriterProperties::builder()
            .set_compression(match compression {
                Compression::Snappy => Codec::SNAPPY,
                Compression::None => Codec::UNCOMPRESSED,
            })
            // Measurement data rarely repeats values; building dictionaries
            // costs time and is abandoned after the first pages.
            .set_dictionary_enabled(false)
            .set_statistics_enabled(EnabledStatistics::Chunk)
            .set_created_by(concat!("buzzard-tdms ", env!("CARGO_PKG_VERSION")).to_owned())
            .set_key_value_metadata(Some(metadata))
            .build(),
    );
    let parquet_schema = ArrowSchemaConverter::new()
        .with_coerce_types(properties.coerce_types())
        .convert(&schema)
        .map_err(failed)?;
    let mut writer = SerializedFileWriter::new(out, parquet_schema.root_schema_ptr(), properties)
        .map_err(failed)?;
    let factory = ArrowRowGroupWriterFactory::new(&writer, Arc::clone(&schema));

    let mut start = 0;
    let mut group = 0;
    // An empty table still gets its schema and metadata.
    while start < plan.rows {
        let end = plan.rows.min(start + plan.batch);
        let rows = usize::try_from(end - start).map_err(|_| Error::Alloc { bytes: end - start })?;
        let mut columns: Vec<ArrayRef> = Vec::with_capacity(schema.fields().len());
        if let Some(w) = plan.time {
            let times: Vec<f64> = (start..end).map(|row| w.x(row)).collect();
            columns.push(primitive::<Float64Type>(times, rows));
        }
        for column in plan.read(start, end)? {
            columns.extend(arrays(column, rows));
        }
        let writers = factory.create_column_writers(group).map_err(failed)?;
        let chunks = writers
            .into_par_iter()
            .zip(columns.into_par_iter())
            .zip(schema.fields().par_iter())
            .map(|((mut writer, array), field)| {
                for leaf in compute_leaves(field, &array)? {
                    writer.write(&leaf)?;
                }
                writer.close()
            })
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(failed)?;
        let mut row_group = writer.next_row_group().map_err(failed)?;
        for chunk in chunks {
            chunk.append_to_row_group(&mut row_group).map_err(failed)?;
        }
        row_group.close().map_err(failed)?;
        start = end;
        group += 1;
    }
    writer.close().map_err(failed)?;
    Ok(plan.done())
}
