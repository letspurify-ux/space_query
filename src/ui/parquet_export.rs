//! Write an export grid as an Apache Parquet file.
//!
//! The grid holds what each cell SHOWS, beside the coarse kind the driver gave
//! its column. Parquet is typed, so every column needs a type, and the rule is
//! one sentence: **a column is typed only when every value in it converts to
//! that type exactly; otherwise it is written as the text the grid shows.** A
//! file never states a value the grid did not show — a type only says more
//! precisely what the shown value was, and only when that is provable.
//!
//! | Column kind | Written as | When |
//! | --- | --- | --- |
//! | Number | `INT64` | every value is an integer that fits 64 bits |
//! | Number | `DECIMAL(38, s)` | every value is a plain decimal of at most 38 digits |
//! | Boolean | `BOOLEAN` | every value is `true` or `false`, in any case |
//! | Temporal | `DATE` | every value is `YYYY-MM-DD` |
//! | Temporal | `TIMESTAMP` (local, µs or ns) | every value is `YYYY-MM-DD` or `YYYY-MM-DD HH:MM:SS[.f]` |
//! | anything else | `STRING` | always |
//!
//! SQL NULL is a Parquet null in every column, so no NULL display text is
//! involved. A column with no value at all proves nothing and stays `STRING`.
//!
//! Each column is converted once, before the schema is written, and the
//! conversion IS the values the writer stores: the type a column is declared
//! with and the values written under it come from one parse, so they cannot
//! disagree.

use std::ops::Range;
use std::sync::Arc;

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use parquet::basic::{
    Compression, DecimalType, LogicalType, Repetition, TimeUnit, TimestampType,
    Type as PhysicalType,
};
use parquet::data_type::{
    BoolType, ByteArray, ByteArrayType, DataType, FixedLenByteArray, FixedLenByteArrayType,
    Int32Type, Int64Type,
};
use parquet::errors::{ParquetError, Result as ParquetResult};
use parquet::file::properties::{WriterProperties, DEFAULT_MAX_ROW_GROUP_ROW_COUNT};
use parquet::file::writer::{SerializedColumnWriter, SerializedFileWriter};
use parquet::schema::types::Type;

use crate::db::SqlValueKind;
use crate::ui::result_export::{unique_field_names, ExportContent, ExportGrid};

/// The precision every `DECIMAL` column is declared with: the most digits
/// [`DECIMAL_BYTES`] can hold, and what Arrow's `Decimal128` reads.
///
/// Fixed rather than fitted to the values, so two exports of one table differ
/// at most in scale. A value that needs more digits keeps its column `STRING`.
const DECIMAL_PRECISION: usize = 38;

/// The width of a `DECIMAL(38, s)` value: an `i128`, big-endian.
const DECIMAL_BYTES: usize = 16;

/// Write `grid` as a Parquet file.
///
/// Refused only for a grid with no column, which no schema can describe.
pub fn render(grid: &ExportGrid) -> ExportContent<Vec<u8>> {
    render_in_row_groups(grid, DEFAULT_MAX_ROW_GROUP_ROW_COUNT)
}

/// [`render`], with the row-group size a test can make small.
fn render_in_row_groups(grid: &ExportGrid, rows_per_group: usize) -> ExportContent<Vec<u8>> {
    if grid.columns.is_empty() {
        return ExportContent::Refused(
            "There is no column to export, so no Parquet file was written.".to_string(),
        );
    }
    let columns: Vec<Column<'_>> = (0..grid.columns.len())
        .map(|index| {
            Column::convert(
                grid.kind(index),
                grid.rows
                    .iter()
                    .map(|row| grid.cell(row, index).as_deref())
                    .collect(),
            )
        })
        .collect();
    let names = unique_field_names(grid.columns.clone());
    match write_file(&names, &columns, grid.rows.len(), rows_per_group.max(1)) {
        Ok(bytes) => ExportContent::written(bytes, grid.rows.len()),
        Err(err) => {
            ExportContent::Refused(format!("The Parquet file could not be written: {err}"))
        }
    }
}

fn write_file(
    names: &[String],
    columns: &[Column<'_>],
    row_count: usize,
    rows_per_group: usize,
) -> ParquetResult<Vec<u8>> {
    let fields = names
        .iter()
        .zip(columns)
        .map(|(name, column)| column.field(name).map(Arc::new))
        .collect::<ParquetResult<Vec<_>>>()?;
    let schema = Arc::new(Type::group_type_builder("schema").with_fields(fields).build()?);
    // Snappy is what most Parquet writers default to and every reader decodes.
    let properties = Arc::new(
        WriterProperties::builder()
            .set_compression(Compression::SNAPPY)
            .build(),
    );
    let mut writer = SerializedFileWriter::new(Vec::new(), schema, properties)?;
    // The low-level writer never splits a row group on its own, so the rows go
    // out in groups of the size the library would choose for itself. No rows
    // means no row group, which is a complete file with a schema.
    let mut start = 0;
    while start < row_count {
        let end = row_count.min(start.saturating_add(rows_per_group));
        let mut row_group = writer.next_row_group()?;
        for column in columns {
            let mut column_writer = row_group.next_column()?.ok_or_else(|| {
                ParquetError::General("the schema ended before the columns did".to_string())
            })?;
            column.write(start..end, &mut column_writer)?;
            column_writer.close()?;
        }
        row_group.close()?;
        start = end;
    }
    writer.into_inner()
}

/// One column, converted to the Parquet type it is written as.
///
/// Every variant holds one entry per row; `None` is SQL NULL.
#[derive(Debug, PartialEq)]
enum Column<'a> {
    /// The text the grid shows.
    Text(Vec<Option<&'a str>>),
    Boolean(Vec<Option<bool>>),
    Int64(Vec<Option<i64>>),
    /// Unscaled values of `DECIMAL(38, scale)`.
    Decimal {
        scale: usize,
        values: Vec<Option<i128>>,
    },
    /// Days since 1970-01-01.
    Date(Vec<Option<i32>>),
    /// `unit`s since 1970-01-01 00:00:00 on the wall clock the grid showed —
    /// a local timestamp, not an instant, because the text carries no zone.
    Timestamp {
        unit: TimestampUnit,
        values: Vec<Option<i64>>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TimestampUnit {
    Micros,
    Nanos,
}

impl<'a> Column<'a> {
    /// The narrowest type every value of the column converts to exactly, with
    /// the values converted; the column's text when there is none.
    fn convert(kind: SqlValueKind, cells: Vec<Option<&'a str>>) -> Self {
        // No value, no evidence: a type chosen here would be a guess.
        if cells.iter().all(Option::is_none) {
            return Column::Text(cells);
        }
        let typed = match kind {
            SqlValueKind::Number => numeric_column(&cells),
            SqlValueKind::Boolean => boolean_column(&cells),
            SqlValueKind::Temporal => temporal_column(&cells),
            // `Binary` is not decoded: MySQL cannot tell `BLOB` from `TEXT`,
            // so the grid's text is the only value both of them have.
            SqlValueKind::String | SqlValueKind::Binary | SqlValueKind::Unknown => None,
        };
        typed.unwrap_or(Column::Text(cells))
    }

    /// The schema field this column is declared as. Every field is OPTIONAL,
    /// because every result column may hold NULL.
    fn field(&self, name: &str) -> ParquetResult<Type> {
        let builder = match self {
            Column::Text(_) => Type::primitive_type_builder(name, PhysicalType::BYTE_ARRAY)
                .with_logical_type(Some(LogicalType::String)),
            Column::Boolean(_) => Type::primitive_type_builder(name, PhysicalType::BOOLEAN),
            Column::Int64(_) => Type::primitive_type_builder(name, PhysicalType::INT64),
            Column::Decimal { scale, .. } => {
                let scale = i32::try_from(*scale).unwrap_or(i32::MAX);
                let precision = DECIMAL_PRECISION as i32;
                Type::primitive_type_builder(name, PhysicalType::FIXED_LEN_BYTE_ARRAY)
                    .with_length(DECIMAL_BYTES as i32)
                    .with_precision(precision)
                    .with_scale(scale)
                    .with_logical_type(Some(LogicalType::Decimal(DecimalType {
                        scale,
                        precision,
                    })))
            }
            Column::Date(_) => Type::primitive_type_builder(name, PhysicalType::INT32)
                .with_logical_type(Some(LogicalType::Date)),
            Column::Timestamp { unit, .. } => {
                Type::primitive_type_builder(name, PhysicalType::INT64).with_logical_type(Some(
                    LogicalType::Timestamp(TimestampType {
                        is_adjusted_to_u_t_c: false,
                        unit: match unit {
                            TimestampUnit::Micros => TimeUnit::MICROS,
                            TimestampUnit::Nanos => TimeUnit::NANOS,
                        },
                    }),
                ))
            }
        };
        builder.with_repetition(Repetition::OPTIONAL).build()
    }

    /// Write the rows in `rows` of this column into one column chunk.
    fn write(&self, rows: Range<usize>, writer: &mut SerializedColumnWriter<'_>) -> ParquetResult<()> {
        match self {
            Column::Text(values) => {
                write_optional::<ByteArrayType, _>(writer, &values[rows], |text| {
                    ByteArray::from(*text)
                })
            }
            Column::Boolean(values) => {
                write_optional::<BoolType, _>(writer, &values[rows], |value| *value)
            }
            Column::Int64(values) | Column::Timestamp { values, .. } => {
                write_optional::<Int64Type, _>(writer, &values[rows], |value| *value)
            }
            Column::Decimal { values, .. } => {
                write_optional::<FixedLenByteArrayType, _>(writer, &values[rows], |value| {
                    FixedLenByteArray::from(value.to_be_bytes().to_vec())
                })
            }
            Column::Date(values) => {
                write_optional::<Int32Type, _>(writer, &values[rows], |value| *value)
            }
        }
    }
}

/// Write one OPTIONAL column chunk: a definition level per row, and a value
/// for each row that has one.
fn write_optional<T: DataType, V>(
    writer: &mut SerializedColumnWriter<'_>,
    cells: &[Option<V>],
    encode: impl Fn(&V) -> T::T,
) -> ParquetResult<()> {
    let definition_levels: Vec<i16> = cells.iter().map(|cell| i16::from(cell.is_some())).collect();
    let values: Vec<T::T> = cells.iter().flatten().map(encode).collect();
    writer
        .typed::<T>()
        .write_batch(&values, Some(&definition_levels), None)?;
    Ok(())
}

/// Convert every value with `convert`, keeping NULLs; `None` as soon as one
/// value does not convert.
fn convert_all<T, U>(cells: &[Option<T>], convert: impl Fn(&T) -> Option<U>) -> Option<Vec<Option<U>>> {
    cells
        .iter()
        .map(|cell| match cell {
            Some(value) => convert(value).map(Some),
            None => Some(None),
        })
        .collect()
}

fn numeric_column(cells: &[Option<&str>]) -> Option<Column<'static>> {
    let numbers = convert_all(cells, |&text| PlainDecimal::parse(text))?;
    if let Some(values) = convert_all(&numbers, |number| number.to_i64()) {
        return Some(Column::Int64(values));
    }
    // One scale for the column — the widest any value shows — so every value
    // keeps every digit it was shown with.
    let scale = numbers
        .iter()
        .flatten()
        .map(|number| number.fraction.len())
        .max()
        .unwrap_or(0);
    let values = convert_all(&numbers, |number| number.unscaled_at(scale))?;
    Some(Column::Decimal { scale, values })
}

fn boolean_column(cells: &[Option<&str>]) -> Option<Column<'static>> {
    convert_all(cells, |text| {
        if text.eq_ignore_ascii_case("true") {
            Some(true)
        } else if text.eq_ignore_ascii_case("false") {
            Some(false)
        } else {
            None
        }
    })
    .map(Column::Boolean)
}

fn temporal_column(cells: &[Option<&str>]) -> Option<Column<'static>> {
    let moments = convert_all(cells, |text| Moment::parse(text))?;
    if moments
        .iter()
        .flatten()
        .all(|moment| moment.fraction_digits.is_none())
    {
        return convert_all(&moments, Moment::days_since_epoch).map(Column::Date);
    }
    // A date alone in a timestamp column is that day's midnight, which is
    // exactly what it showed. Microseconds unless a value shows a digit finer
    // than that; nanoseconds then, which only reach the years 1677 to 2262 —
    // a value outside them keeps the column text.
    let unit = if moments
        .iter()
        .flatten()
        .all(|moment| moment.fraction_digits.unwrap_or(0) <= 6)
    {
        TimestampUnit::Micros
    } else {
        TimestampUnit::Nanos
    };
    let values = convert_all(&moments, |moment| moment.since_epoch(unit))?;
    Some(Column::Timestamp { unit, values })
}

/// A number written in plain positional notation: `-12.340`, `.5`, `7`.
///
/// No exponent, no grouping, no surrounding space: those are renderings a
/// reader could take more than one way, and a column holding one stays text.
#[derive(Clone, Copy, Debug)]
struct PlainDecimal<'a> {
    negative: bool,
    /// The integer digits, leading zeros removed (empty for zero).
    integer: &'a str,
    /// The fraction digits as shown, trailing zeros kept: they are the scale
    /// the value was shown with.
    fraction: &'a str,
}

impl<'a> PlainDecimal<'a> {
    fn parse(text: &'a str) -> Option<Self> {
        let (negative, unsigned) = match text.as_bytes().first() {
            Some(b'-') => (true, &text[1..]),
            Some(b'+') => (false, &text[1..]),
            _ => (false, text),
        };
        let (integer, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
        let all_digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
        if (integer.is_empty() && fraction.is_empty()) || !all_digits(integer) || !all_digits(fraction)
        {
            return None;
        }
        Some(Self {
            negative,
            integer: integer.trim_start_matches('0'),
            fraction,
        })
    }

    /// The value as an `i64`, when it has no fraction and fits.
    ///
    /// A shown fraction — even `.0` — keeps the column a decimal, so the scale
    /// the value was shown with survives.
    fn to_i64(self) -> Option<i64> {
        if !self.fraction.is_empty() {
            return None;
        }
        i64::try_from(self.unscaled_at(0)?).ok()
    }

    /// The value times `10^scale`, when that has at most
    /// [`DECIMAL_PRECISION`] digits. `scale` is never below the value's own.
    fn unscaled_at(&self, scale: usize) -> Option<i128> {
        if self.integer.len() + scale > DECIMAL_PRECISION || self.fraction.len() > scale {
            return None;
        }
        let padding = scale - self.fraction.len();
        let mut value: i128 = 0;
        for byte in self
            .integer
            .bytes()
            .chain(self.fraction.bytes())
            .chain(std::iter::repeat_n(b'0', padding))
        {
            value = value * 10 + i128::from(byte - b'0');
        }
        Some(if self.negative { -value } else { value })
    }
}

/// A date, or a date and time of day, as the grid shows them:
/// `YYYY-MM-DD` or `YYYY-MM-DD HH:MM:SS[.f]` with one to nine fraction digits.
///
/// That is this app's own rendering of MySQL temporal values and of Oracle
/// dates and timestamps under its default NLS formats. Anything else — a
/// zone, an interval, MySQL's `0000-00-00`, a format the user chose — does
/// not parse, and its column stays text.
#[derive(Clone, Copy, Debug)]
struct Moment {
    at: NaiveDateTime,
    /// `None` for a date alone; otherwise how many fraction digits the value
    /// needs, trailing zeros not counted.
    fraction_digits: Option<usize>,
}

impl Moment {
    fn parse(text: &str) -> Option<Self> {
        let date = parse_date(text.get(..10)?)?;
        let Some(time_text) = text.get(10..).filter(|rest| !rest.is_empty()) else {
            return Some(Self {
                at: date.and_time(NaiveTime::MIN),
                fraction_digits: None,
            });
        };
        let clock = time_text.strip_prefix(' ')?;
        let (hms, fraction) = match clock.split_once('.') {
            Some((hms, fraction)) => (hms, fraction),
            None => (clock, ""),
        };
        if clock.contains('.') && !(1..=9).contains(&fraction.len()) {
            return None;
        }
        if !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let hms = hms.as_bytes();
        if hms.len() != 8 || hms[2] != b':' || hms[5] != b':' {
            return None;
        }
        let hour = two_digits(&hms[0..2])?;
        let minute = two_digits(&hms[3..5])?;
        let second = two_digits(&hms[6..8])?;
        // A second of 60 is a leap second, which a timestamp cannot hold.
        if second > 59 {
            return None;
        }
        let nanos = format!("{fraction:0<9}").parse::<u32>().ok()?;
        let time = NaiveTime::from_hms_nano_opt(hour, minute, second, nanos)?;
        Some(Self {
            at: date.and_time(time),
            fraction_digits: Some(fraction.trim_end_matches('0').len()),
        })
    }

    fn days_since_epoch(&self) -> Option<i32> {
        let epoch = NaiveDate::from_ymd_opt(1970, 1, 1)?;
        i32::try_from(self.at.date().signed_duration_since(epoch).num_days()).ok()
    }

    fn since_epoch(&self, unit: TimestampUnit) -> Option<i64> {
        let at = self.at.and_utc();
        match unit {
            TimestampUnit::Micros => Some(at.timestamp_micros()),
            TimestampUnit::Nanos => at.timestamp_nanos_opt(),
        }
    }
}

/// `YYYY-MM-DD`, a real calendar day.
fn parse_date(text: &str) -> Option<NaiveDate> {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year = bytes[0..4]
        .iter()
        .try_fold(0i32, |year, byte| byte.is_ascii_digit().then(|| year * 10 + i32::from(byte - b'0')))?;
    NaiveDate::from_ymd_opt(year, two_digits(&bytes[5..7])?, two_digits(&bytes[8..10])?)
}

fn two_digits(bytes: &[u8]) -> Option<u32> {
    match bytes {
        [tens, ones] if tens.is_ascii_digit() && ones.is_ascii_digit() => {
            Some(u32::from(tens - b'0') * 10 + u32::from(ones - b'0'))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::result_export::ExportCell;
    use parquet::file::reader::{FileReader, SerializedFileReader};
    use parquet::record::Field;

    fn grid(columns: &[(&str, SqlValueKind)], rows: Vec<Vec<ExportCell>>) -> ExportGrid {
        ExportGrid {
            columns: columns.iter().map(|(name, _)| name.to_string()).collect(),
            column_kinds: columns.iter().map(|(_, kind)| *kind).collect(),
            rows,
            null_text: "NULL".to_string(),
        }
    }

    /// One column of `kind` holding `values`, `None` being SQL NULL.
    fn converted(kind: SqlValueKind, values: &[Option<&'static str>]) -> Column<'static> {
        Column::convert(kind, values.to_vec())
    }

    fn written(grid: &ExportGrid) -> Vec<u8> {
        render(grid).into_parts().expect("written").0
    }

    fn reader(bytes: Vec<u8>) -> SerializedFileReader<bytes::Bytes> {
        SerializedFileReader::new(bytes::Bytes::from(bytes)).expect("a readable Parquet file")
    }

    /// Every row, as the record API reads it back.
    fn read_rows(bytes: Vec<u8>) -> Vec<Vec<(String, Field)>> {
        reader(bytes)
            .get_row_iter(None)
            .expect("rows")
            .map(|row| {
                row.expect("row")
                    .get_column_iter()
                    .map(|(name, field)| (name.clone(), field.clone()))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn integers_that_fit_64_bits_are_int64() {
        assert_eq!(
            converted(SqlValueKind::Number, &[Some("1"), None, Some("-42"), Some("007")]),
            Column::Int64(vec![Some(1), None, Some(-42), Some(7)])
        );
        assert_eq!(
            converted(SqlValueKind::Number, &[Some("9223372036854775807")]),
            Column::Int64(vec![Some(i64::MAX)])
        );
    }

    #[test]
    fn a_column_with_a_fraction_is_a_decimal_at_its_widest_scale() {
        assert_eq!(
            converted(SqlValueKind::Number, &[Some("1.5"), Some("-.25"), Some("3"), None]),
            Column::Decimal {
                scale: 2,
                values: vec![Some(150), Some(-25), Some(300), None],
            }
        );
        // A shown `.0` is a shown scale, not an integer.
        assert_eq!(
            converted(SqlValueKind::Number, &[Some("1.0")]),
            Column::Decimal {
                scale: 1,
                values: vec![Some(10)],
            }
        );
    }

    #[test]
    fn integers_past_64_bits_are_a_decimal_of_scale_zero() {
        assert_eq!(
            converted(SqlValueKind::Number, &[Some("12345678901234567890123")]),
            Column::Decimal {
                scale: 0,
                values: vec![Some(12_345_678_901_234_567_890_123)],
            }
        );
    }

    #[test]
    fn a_number_no_decimal_holds_exactly_keeps_the_column_text() {
        // 39 digits, an exponent, a special value, a grouped number, a space.
        for value in [
            "123456789012345678901234567890123456789",
            "1.5E+10",
            "NaN",
            "Infinity",
            "1,234",
            " 5",
            ".",
            "-",
        ] {
            assert_eq!(
                converted(SqlValueKind::Number, &[Some("1"), Some(value)]),
                Column::Text(vec![Some("1"), Some(value)]),
                "{value}"
            );
        }
        // Each value fits on its own, but together they need 39 digits.
        assert_eq!(
            converted(
                SqlValueKind::Number,
                &[Some("12345678901234567890123"), Some("0.1234567890123456")]
            ),
            Column::Text(vec![
                Some("12345678901234567890123"),
                Some("0.1234567890123456")
            ])
        );
    }

    #[test]
    fn booleans_are_true_or_false_in_any_case() {
        assert_eq!(
            converted(SqlValueKind::Boolean, &[Some("TRUE"), Some("false"), None]),
            Column::Boolean(vec![Some(true), Some(false), None])
        );
        assert_eq!(
            converted(SqlValueKind::Boolean, &[Some("TRUE"), Some("1")]),
            Column::Text(vec![Some("TRUE"), Some("1")])
        );
    }

    #[test]
    fn dates_alone_are_a_date_column() {
        assert_eq!(
            converted(SqlValueKind::Temporal, &[Some("1970-01-02"), Some("1969-12-31"), None]),
            Column::Date(vec![Some(1), Some(-1), None])
        );
    }

    #[test]
    fn date_times_are_a_local_timestamp_and_a_date_among_them_is_midnight() {
        assert_eq!(
            converted(
                SqlValueKind::Temporal,
                &[Some("1970-01-01 00:00:01.5"), Some("1970-01-02"), None]
            ),
            Column::Timestamp {
                unit: TimestampUnit::Micros,
                values: vec![Some(1_500_000), Some(86_400_000_000), None],
            }
        );
        // Trailing zeros are not precision: `.123456000` still fits micros.
        assert_eq!(
            converted(SqlValueKind::Temporal, &[Some("1970-01-01 00:00:00.123456000")]),
            Column::Timestamp {
                unit: TimestampUnit::Micros,
                values: vec![Some(123_456)],
            }
        );
    }

    #[test]
    fn a_digit_finer_than_a_microsecond_makes_the_column_nanoseconds() {
        assert_eq!(
            converted(
                SqlValueKind::Temporal,
                &[Some("1970-01-01 00:00:00.000000001"), Some("1970-01-01 00:00:01")]
            ),
            Column::Timestamp {
                unit: TimestampUnit::Nanos,
                values: vec![Some(1), Some(1_000_000_000)],
            }
        );
        // Nanoseconds since 1970 run out in 2262; the column stays text then.
        let late = [Some("2300-01-01 00:00:00.000000001")];
        assert_eq!(
            converted(SqlValueKind::Temporal, &late),
            Column::Text(late.to_vec())
        );
    }

    #[test]
    fn a_temporal_value_this_app_does_not_render_keeps_the_column_text() {
        for value in [
            "0000-00-00",
            "2024-02-30",
            "2024-01-01 24:00:00",
            "2024-01-01 12:00:60",
            "2024-01-01T12:00:00",
            "2024-01-01 12:00:00.",
            "2024-01-01 12:00:00.1234567890",
            "2024-01-01 12:00:00 +09:00",
            "12:00:00",
            "17-DEC-80",
            "+000000001 02:03:04.000000",
        ] {
            assert_eq!(
                converted(SqlValueKind::Temporal, &[Some("2024-01-01"), Some(value)]),
                Column::Text(vec![Some("2024-01-01"), Some(value)]),
                "{value}"
            );
        }
    }

    #[test]
    fn text_kinds_and_columns_without_a_value_stay_text() {
        for kind in [SqlValueKind::String, SqlValueKind::Binary, SqlValueKind::Unknown] {
            assert_eq!(
                converted(kind, &[Some("1")]),
                Column::Text(vec![Some("1")]),
                "{kind:?}"
            );
        }
        // NULLs alone prove no type.
        assert_eq!(
            converted(SqlValueKind::Number, &[None, None]),
            Column::Text(vec![None, None])
        );
    }

    /// The file reads back with the types and values the grid showed, NULL
    /// as null — through a reader, not through the converter that wrote it.
    #[test]
    fn a_written_file_reads_back_typed_with_nulls() {
        let grid = grid(
            &[
                ("ID", SqlValueKind::Number),
                ("PRICE", SqlValueKind::Number),
                ("NAME", SqlValueKind::String),
                ("ACTIVE", SqlValueKind::Boolean),
                ("BORN", SqlValueKind::Temporal),
                ("SEEN", SqlValueKind::Temporal),
                ("BLOB", SqlValueKind::Unknown),
            ],
            vec![
                vec![
                    Some("1".to_string()),
                    Some("12.50".to_string()),
                    Some("가나다 \"q\"".to_string()),
                    Some("TRUE".to_string()),
                    Some("1970-01-02".to_string()),
                    Some("1970-01-01 00:00:01.250".to_string()),
                    Some("(BLOB)".to_string()),
                ],
                vec![
                    Some("2".to_string()),
                    None,
                    Some(String::new()),
                    None,
                    None,
                    None,
                    None,
                ],
            ],
        );
        let rows = read_rows(written(&grid));
        assert_eq!(rows.len(), 2);
        let names: Vec<&str> = rows[0].iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["ID", "PRICE", "NAME", "ACTIVE", "BORN", "SEEN", "BLOB"]);
        let first: Vec<&Field> = rows[0].iter().map(|(_, field)| field).collect();
        assert_eq!(first[0], &Field::Long(1));
        match first[1] {
            Field::Decimal(decimal) => {
                assert_eq!((decimal.precision(), decimal.scale()), (38, 2));
                assert_eq!(decimal.data(), 1250i128.to_be_bytes().as_slice());
            }
            other => panic!("PRICE read back as {other:?}"),
        }
        assert_eq!(first[2], &Field::Str("가나다 \"q\"".to_string()));
        assert_eq!(first[3], &Field::Bool(true));
        assert_eq!(first[4], &Field::Date(1));
        assert_eq!(first[5], &Field::TimestampMicros(1_250_000));
        assert_eq!(first[6], &Field::Str("(BLOB)".to_string()));
        // The empty string is a value; only the NULLs read back as null.
        let second: Vec<&Field> = rows[1].iter().map(|(_, field)| field).collect();
        assert_eq!(second[0], &Field::Long(2));
        assert_eq!(second[2], &Field::Str(String::new()));
        for index in [1, 3, 4, 5, 6] {
            assert_eq!(second[index], &Field::Null, "{}", names[index]);
        }
    }

    #[test]
    fn the_schema_declares_local_timestamps_and_optional_fields() {
        let grid = grid(
            &[("AT", SqlValueKind::Temporal)],
            vec![vec![Some("2024-01-01 00:00:00.000000001".to_string())]],
        );
        let reader = reader(written(&grid));
        let schema = reader.metadata().file_metadata().schema_descr();
        let column = schema.column(0);
        assert_eq!(column.self_type().get_basic_info().repetition(), Repetition::OPTIONAL);
        assert_eq!(
            column.logical_type_ref(),
            Some(&LogicalType::Timestamp(TimestampType {
                is_adjusted_to_u_t_c: false,
                unit: TimeUnit::NANOS,
            }))
        );
    }

    #[test]
    fn repeated_column_names_are_made_unique() {
        let grid = grid(
            &[("ID", SqlValueKind::Number), ("ID", SqlValueKind::Number)],
            vec![vec![Some("1".to_string()), Some("2".to_string())]],
        );
        let rows = read_rows(written(&grid));
        let names: Vec<&str> = rows[0].iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["ID", "ID_2"]);
    }

    #[test]
    fn rows_are_split_into_row_groups_and_all_come_back_in_order() {
        let grid = grid(
            &[("N", SqlValueKind::Number)],
            (0..10).map(|index| vec![Some(index.to_string())]).collect(),
        );
        let bytes = render_in_row_groups(&grid, 4)
            .into_parts()
            .expect("written")
            .0;
        assert_eq!(reader(bytes.clone()).num_row_groups(), 3);
        let values: Vec<Field> = read_rows(bytes)
            .into_iter()
            .map(|mut row| row.remove(0).1)
            .collect();
        assert_eq!(values, (0..10).map(Field::Long).collect::<Vec<_>>());
    }

    #[test]
    fn a_result_with_no_rows_is_a_file_with_its_schema() {
        let grid = grid(&[("A", SqlValueKind::Number)], Vec::new());
        let content = render(&grid);
        assert_eq!(content.rows(), 0);
        let reader = reader(content.into_parts().expect("written").0);
        assert_eq!(reader.num_row_groups(), 0);
        assert_eq!(reader.metadata().file_metadata().schema_descr().num_columns(), 1);
    }

    #[test]
    fn a_grid_without_a_column_is_refused() {
        let content = render(&ExportGrid::default());
        assert!(content.refusal().is_some());
    }

    #[test]
    fn a_short_row_is_null_in_the_columns_it_does_not_reach() {
        let grid = grid(
            &[("A", SqlValueKind::String), ("B", SqlValueKind::String)],
            vec![vec![Some("only".to_string())]],
        );
        let rows = read_rows(written(&grid));
        assert_eq!(rows[0][1].1, Field::Null);
    }
}
