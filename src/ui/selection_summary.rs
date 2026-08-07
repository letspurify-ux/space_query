//! The count/sum/average/min/max the status bar shows for a result-grid
//! selection.
//!
//! Grid values are strings (`QueryResult.rows: Vec<Vec<String>>`), so the
//! aggregate has to decide for itself what a number is. It parses the exact
//! decimal spelling the driver produced and accumulates in `i128` at a common
//! scale, so `0.1 + 0.2` is `0.3` and a 20-digit Oracle `NUMBER` keeps every
//! digit. When a value is not a plain decimal — or the exact arithmetic leaves
//! `i128` range — the numeric part is dropped entirely rather than reported
//! approximately.
//!
//! NULLs follow SQL aggregate semantics: they are skipped, and `Count` is the
//! number of non-NULL values, not the number of selected cells.

/// A decimal number held exactly: `units` scaled down by `10^scale`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Decimal {
    units: i128,
    scale: u32,
}

/// The largest scale a value may carry. Anything longer is not a number a grid
/// realistically shows, and rejecting it keeps the rescaling below from being
/// able to run away.
const MAX_SCALE: u32 = 30;

/// Extra fraction digits the average is computed with, on top of the scale the
/// sum already carries.
const AVERAGE_EXTRA_SCALE: u32 = 6;

/// Above this many selected cells the summary reports the selection size only.
/// Scanning is memoized per selection, but a select-all over a fully fetched
/// million-row result is still enough work to be felt as a hitch.
pub(crate) const MAX_SCANNED_CELLS: usize = 2_000_000;

impl Decimal {
    fn rescale_to(self, scale: u32) -> Option<Self> {
        if scale < self.scale {
            return None;
        }
        let factor = 10i128.checked_pow(scale.checked_sub(self.scale)?)?;
        Some(Self {
            units: self.units.checked_mul(factor)?,
            scale,
        })
    }

    /// Both values at the same scale, so they can be added or compared.
    fn aligned(self, other: Self) -> Option<(Self, Self)> {
        let scale = self.scale.max(other.scale);
        Some((self.rescale_to(scale)?, other.rescale_to(scale)?))
    }

    fn checked_add(self, other: Self) -> Option<Self> {
        let (left, right) = self.aligned(other)?;
        Some(Self {
            units: left.units.checked_add(right.units)?,
            scale: left.scale,
        })
    }

    fn is_less_than(self, other: Self) -> Option<bool> {
        let (left, right) = self.aligned(other)?;
        Some(left.units < right.units)
    }

    /// `self / divisor`, rounded half away from zero at `AVERAGE_EXTRA_SCALE`
    /// digits past the scale already held.
    fn checked_div_by(self, divisor: u64) -> Option<Self> {
        let divisor = i128::from(divisor);
        if divisor <= 0 {
            return None;
        }
        let scale = self.scale.checked_add(AVERAGE_EXTRA_SCALE)?;
        let scaled = self.rescale_to(scale)?;
        let quotient = scaled.units.checked_div(divisor)?;
        let remainder = scaled.units.checked_rem(divisor)?;
        // Round half away from zero: the doubled remainder decides, and the sign
        // of the dividend decides which way "away" is.
        let rounded = if remainder.checked_mul(2)?.abs() >= divisor {
            let step = if scaled.units < 0 { -1 } else { 1 };
            quotient.checked_add(step)?
        } else {
            quotient
        };
        Some(Self {
            units: rounded,
            scale,
        })
    }

    /// The plain decimal spelling, with trailing fraction zeros trimmed.
    fn to_display_string(self) -> String {
        if self.scale == 0 {
            return self.units.to_string();
        }
        let negative = self.units < 0;
        let digits = self.units.unsigned_abs().to_string();
        let scale = self.scale as usize;
        let (integer_part, fraction_part) = if digits.len() > scale {
            let split = digits.len() - scale;
            (digits[..split].to_string(), digits[split..].to_string())
        } else {
            (
                "0".to_string(),
                format!("{:0>width$}", digits, width = scale),
            )
        };
        let fraction_part = fraction_part.trim_end_matches('0');
        let sign = if negative && (integer_part != "0" || !fraction_part.is_empty()) {
            "-"
        } else {
            ""
        };
        if fraction_part.is_empty() {
            format!("{sign}{integer_part}")
        } else {
            format!("{sign}{integer_part}.{fraction_part}")
        }
    }
}

/// The exact decimal a cell holds, or `None` when the text is not one.
///
/// Accepted: an optional sign, digits with at most one decimal point, and an
/// optional `E` exponent — the spellings the drivers actually emit. Everything
/// else (thousands separators, currency, hex, `Infinity`, dates) is not a
/// number here.
fn parse_decimal(value: &str) -> Option<Decimal> {
    let text = value.trim();
    let (negative, text) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let (mantissa, exponent) = match text.find(['e', 'E']) {
        Some(index) => {
            let (mantissa, exponent) = text.split_at(index);
            (mantissa, Some(parse_exponent(&exponent[1..])?))
        }
        None => (text, None),
    };
    let (integer_digits, fraction_digits) = match mantissa.split_once('.') {
        Some((integer_digits, fraction_digits)) => (integer_digits, fraction_digits),
        None => (mantissa, ""),
    };
    if integer_digits.is_empty() && fraction_digits.is_empty() {
        return None;
    }
    if !integer_digits.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction_digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }

    let mut units: i128 = 0;
    for byte in integer_digits.bytes().chain(fraction_digits.bytes()) {
        units = units
            .checked_mul(10)?
            .checked_add(i128::from(byte - b'0'))?;
    }
    if negative {
        units = units.checked_neg()?;
    }

    let mut decimal = Decimal {
        units,
        scale: u32::try_from(fraction_digits.len()).ok()?,
    };
    if let Some(exponent) = exponent {
        decimal = apply_exponent(decimal, exponent)?;
    }
    if decimal.scale > MAX_SCALE {
        return None;
    }
    Some(decimal)
}

fn parse_exponent(text: &str) -> Option<i32> {
    if text.is_empty() {
        return None;
    }
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // A wild exponent cannot be represented exactly anyway; bail before the
    // scale arithmetic has to care.
    let value: i32 = digits.parse().ok()?;
    Some(if negative { -value } else { value })
}

fn apply_exponent(decimal: Decimal, exponent: i32) -> Option<Decimal> {
    if exponent == 0 {
        return Some(decimal);
    }
    if exponent > 0 {
        let shift = u32::try_from(exponent).ok()?;
        // Spend the shift on the existing fraction digits first; only what is
        // left has to grow the integer part.
        let consumed = shift.min(decimal.scale);
        let remaining = shift - consumed;
        let scaled = Decimal {
            units: decimal.units,
            scale: decimal.scale - consumed,
        };
        if remaining == 0 {
            return Some(scaled);
        }
        Some(Decimal {
            units: scaled.units.checked_mul(10i128.checked_pow(remaining)?)?,
            scale: 0,
        })
    } else {
        let shift = u32::try_from(-i64::from(exponent)).ok()?;
        Some(Decimal {
            units: decimal.units,
            scale: decimal.scale.checked_add(shift)?,
        })
    }
}

/// What the status bar knows about a selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SelectionSummary {
    /// Every non-NULL value parsed as a decimal.
    Numeric {
        count: u64,
        sum: String,
        average: String,
        minimum: String,
        maximum: String,
    },
    /// Some value was not a number, so only the count is meaningful.
    Count { count: u64 },
    /// The selection was too large to scan; only its size is reported.
    CellsOnly { cells: usize },
}

impl SelectionSummary {
    /// The single line the status bar paints.
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Numeric {
                count,
                sum,
                average,
                minimum,
                maximum,
            } => format!(
                "Count: {count}  Sum: {sum}  Avg: {average}  Min: {minimum}  Max: {maximum}"
            ),
            Self::Count { count } => format!("Count: {count}"),
            Self::CellsOnly { cells } => format!("Selected: {cells} cells"),
        }
    }
}

/// Running numeric state, abandoned the moment a value is not a number or the
/// exact arithmetic runs out of room.
struct NumericAccumulator {
    sum: Decimal,
    minimum: Decimal,
    maximum: Decimal,
}

impl NumericAccumulator {
    fn new(first: Decimal) -> Self {
        Self {
            sum: first,
            minimum: first,
            maximum: first,
        }
    }

    fn push(&mut self, value: Decimal) -> Option<()> {
        self.sum = self.sum.checked_add(value)?;
        if value.is_less_than(self.minimum)? {
            self.minimum = value;
        }
        if self.maximum.is_less_than(value)? {
            self.maximum = value;
        }
        Some(())
    }
}

/// The summary for the cells in `rows[row_start..=row_end][col_start..=col_end]`.
///
/// `hidden_col` is the zero-width ROWID column edit mode adds: it holds a value
/// the user never sees, so it must not reach the aggregate. Returns `None` when
/// the selection covers a single cell — one cell has nothing to aggregate, and
/// a permanent `Count: 1` in the status bar is noise.
pub(crate) fn summarize_selection(
    rows: &[Vec<String>],
    bounds: (usize, usize, usize, usize),
    hidden_col: Option<usize>,
    null_text: &str,
) -> Option<SelectionSummary> {
    let (row_start, col_start, row_end, col_end) = bounds;
    if row_start > row_end || col_start > col_end {
        return None;
    }
    let selected_columns = (col_start..=col_end)
        .filter(|col| hidden_col != Some(*col))
        .count();
    if selected_columns == 0 {
        return None;
    }
    let selected_rows = row_end - row_start + 1;
    let cells = selected_rows.saturating_mul(selected_columns);
    if cells <= 1 {
        return None;
    }
    if cells > MAX_SCANNED_CELLS {
        return Some(SelectionSummary::CellsOnly { cells });
    }

    let mut count: u64 = 0;
    let mut numeric: Option<NumericAccumulator> = None;
    let mut numeric_failed = false;
    for row in rows.iter().take(row_end + 1).skip(row_start) {
        for (col, value) in row.iter().enumerate().take(col_end + 1).skip(col_start) {
            if hidden_col == Some(col) {
                continue;
            }
            if crate::ui::result_table::ResultTableWidget::value_represents_null(value, null_text) {
                continue;
            }
            count += 1;
            if numeric_failed {
                continue;
            }
            let Some(decimal) = parse_decimal(value) else {
                numeric_failed = true;
                continue;
            };
            match numeric.as_mut() {
                Some(accumulator) => {
                    if accumulator.push(decimal).is_none() {
                        numeric_failed = true;
                    }
                }
                None => numeric = Some(NumericAccumulator::new(decimal)),
            }
        }
    }

    if count == 0 {
        return Some(SelectionSummary::Count { count: 0 });
    }
    let Some(accumulator) = numeric.filter(|_| !numeric_failed) else {
        return Some(SelectionSummary::Count { count });
    };
    let Some(average) = accumulator.sum.checked_div_by(count) else {
        return Some(SelectionSummary::Count { count });
    };
    Some(SelectionSummary::Numeric {
        count,
        sum: accumulator.sum.to_display_string(),
        average: average.to_display_string(),
        minimum: accumulator.minimum.to_display_string(),
        maximum: accumulator.maximum.to_display_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(values: &[&[&str]]) -> Vec<Vec<String>> {
        values
            .iter()
            .map(|row| row.iter().map(|value| (*value).to_string()).collect())
            .collect()
    }

    fn summary(values: &[&[&str]]) -> Option<SelectionSummary> {
        let rows = rows(values);
        let row_end = rows.len().saturating_sub(1);
        let col_end = rows.first().map_or(0, |row| row.len().saturating_sub(1));
        summarize_selection(&rows, (0, 0, row_end, col_end), None, "NULL")
    }

    #[test]
    fn numeric_selection_reports_every_aggregate() {
        assert_eq!(
            summary(&[&["1", "2"], &["3", "4"]]),
            Some(SelectionSummary::Numeric {
                count: 4,
                sum: "10".to_string(),
                average: "2.5".to_string(),
                minimum: "1".to_string(),
                maximum: "4".to_string(),
            })
        );
    }

    #[test]
    fn decimal_sum_is_exact_rather_than_binary_floating_point() {
        let Some(SelectionSummary::Numeric { sum, .. }) = summary(&[&["0.1", "0.2"]]) else {
            panic!("expected a numeric summary");
        };
        assert_eq!(sum, "0.3");
    }

    #[test]
    fn long_oracle_numbers_keep_every_digit() {
        let Some(SelectionSummary::Numeric { sum, maximum, .. }) =
            summary(&[&["12345678901234567890123456", "1"]])
        else {
            panic!("expected a numeric summary");
        };
        assert_eq!(sum, "12345678901234567890123457");
        assert_eq!(maximum, "12345678901234567890123456");
    }

    #[test]
    fn mixed_scales_align_before_adding() {
        let Some(SelectionSummary::Numeric {
            sum,
            minimum,
            maximum,
            ..
        }) = summary(&[&["1.5", "2", "0.125"]])
        else {
            panic!("expected a numeric summary");
        };
        assert_eq!(sum, "3.625");
        assert_eq!(minimum, "0.125");
        assert_eq!(maximum, "2");
    }

    #[test]
    fn scientific_notation_is_a_number() {
        let Some(SelectionSummary::Numeric { sum, .. }) = summary(&[&["1.5E2", "5e-1"]]) else {
            panic!("expected a numeric summary");
        };
        assert_eq!(sum, "150.5");
    }

    #[test]
    fn negative_values_are_summed_and_ordered() {
        let Some(SelectionSummary::Numeric {
            sum,
            average,
            minimum,
            maximum,
            ..
        }) = summary(&[&["-3", "1", "-1"]])
        else {
            panic!("expected a numeric summary");
        };
        assert_eq!(sum, "-3");
        assert_eq!(average, "-1");
        assert_eq!(minimum, "-3");
        assert_eq!(maximum, "1");
    }

    #[test]
    fn average_rounds_half_away_from_zero() {
        let Some(SelectionSummary::Numeric { average, .. }) = summary(&[&["1", "2", "2"]]) else {
            panic!("expected a numeric summary");
        };
        // 5/3 = 1.666666... at six fraction digits.
        assert_eq!(average, "1.666667");
    }

    #[test]
    fn nulls_are_skipped_and_do_not_count() {
        assert_eq!(
            summary(&[&["1", "NULL"], &["", "3"]]),
            Some(SelectionSummary::Numeric {
                count: 2,
                sum: "4".to_string(),
                average: "2".to_string(),
                minimum: "1".to_string(),
                maximum: "3".to_string(),
            })
        );
    }

    #[test]
    fn a_non_numeric_value_leaves_only_the_count() {
        assert_eq!(
            summary(&[&["1", "CLERK"]]),
            Some(SelectionSummary::Count { count: 2 })
        );
    }

    #[test]
    fn thousands_separators_are_not_numbers() {
        assert_eq!(
            summary(&[&["1,000", "2"]]),
            Some(SelectionSummary::Count { count: 2 })
        );
    }

    #[test]
    fn an_all_null_selection_counts_nothing() {
        assert_eq!(
            summary(&[&["NULL", ""]]),
            Some(SelectionSummary::Count { count: 0 })
        );
    }

    #[test]
    fn a_single_cell_has_no_summary() {
        assert_eq!(summary(&[&["1"]]), None);
    }

    #[test]
    fn the_hidden_rowid_column_is_not_aggregated() {
        let rows = rows(&[&["AAAR3s", "1"], &["AAAR3t", "2"]]);
        assert_eq!(
            summarize_selection(&rows, (0, 0, 1, 1), Some(0), "NULL"),
            Some(SelectionSummary::Numeric {
                count: 2,
                sum: "3".to_string(),
                average: "1.5".to_string(),
                minimum: "1".to_string(),
                maximum: "2".to_string(),
            })
        );
    }

    #[test]
    fn a_selection_of_only_the_hidden_column_has_no_summary() {
        let rows = rows(&[&["AAAR3s", "1"], &["AAAR3t", "2"]]);
        assert_eq!(
            summarize_selection(&rows, (0, 0, 1, 0), Some(0), "NULL"),
            None
        );
    }

    #[test]
    fn a_huge_selection_reports_its_size_without_scanning() {
        let rows = rows(&[&["1", "2"]]);
        let row_end = MAX_SCANNED_CELLS;
        let summary = summarize_selection(&rows, (0, 0, row_end, 1), None, "NULL");
        assert_eq!(
            summary,
            Some(SelectionSummary::CellsOnly {
                cells: (row_end + 1) * 2
            })
        );
    }

    #[test]
    fn selection_bounds_past_the_fetched_rows_only_see_what_exists() {
        let rows = rows(&[&["1", "2"]]);
        assert_eq!(
            summarize_selection(&rows, (0, 0, 9, 1), None, "NULL"),
            Some(SelectionSummary::Numeric {
                count: 2,
                sum: "3".to_string(),
                average: "1.5".to_string(),
                minimum: "1".to_string(),
                maximum: "2".to_string(),
            })
        );
    }

    #[test]
    fn labels_name_each_aggregate() {
        assert_eq!(
            SelectionSummary::Numeric {
                count: 4,
                sum: "10".to_string(),
                average: "2.5".to_string(),
                minimum: "1".to_string(),
                maximum: "4".to_string(),
            }
            .label(),
            "Count: 4  Sum: 10  Avg: 2.5  Min: 1  Max: 4"
        );
        assert_eq!(SelectionSummary::Count { count: 7 }.label(), "Count: 7");
        assert_eq!(
            SelectionSummary::CellsOnly { cells: 12 }.label(),
            "Selected: 12 cells"
        );
    }

    #[test]
    fn values_that_overflow_exact_arithmetic_fall_back_to_the_count() {
        let huge = "1".repeat(38);
        let rows = rows(&[&[huge.as_str(), huge.as_str(), huge.as_str()]]);
        assert_eq!(
            summarize_selection(&rows, (0, 0, 0, 2), None, "NULL"),
            Some(SelectionSummary::Count { count: 3 })
        );
    }

    #[test]
    fn a_value_longer_than_the_scale_limit_is_not_a_number() {
        assert!(parse_decimal(&format!("0.{}", "1".repeat(31))).is_none());
    }

    #[test]
    fn parse_decimal_rejects_non_numbers() {
        assert!(parse_decimal("").is_none());
        assert!(parse_decimal("-").is_none());
        assert!(parse_decimal(".").is_none());
        assert!(parse_decimal("1.2.3").is_none());
        assert!(parse_decimal("0x1F").is_none());
        assert!(parse_decimal("1e").is_none());
        assert!(parse_decimal("NaN").is_none());
        assert!(parse_decimal("2026-08-08").is_none());
    }

    #[test]
    fn parse_decimal_accepts_the_spellings_drivers_emit() {
        assert!(parse_decimal("  42  ").is_some());
        assert!(parse_decimal("+42").is_some());
        assert!(parse_decimal(".5").is_some());
        assert!(parse_decimal("5.").is_some());
        assert!(parse_decimal("00123").is_some());
    }

    #[test]
    fn zero_padding_does_not_change_the_value() {
        let Some(SelectionSummary::Numeric { sum, .. }) = summary(&[&["00123", "0"]]) else {
            panic!("expected a numeric summary");
        };
        assert_eq!(sum, "123");
    }
}
