//! Ordering for the result grid's local header sort.
//!
//! The grid stores every value as the text the driver rendered, so ordering has
//! to reconstruct enough of the value to compare it. The column's
//! [`SqlValueKind`] says which reconstruction to attempt; when it does not
//! apply, comparison falls back to the plain text order, which is what the grid
//! did for every column before.
//!
//! Two defects this replaces (docs_items/item_list.md, appendix B.2):
//!
//! * Numbers went through `f64`, so two distinct Oracle `NUMBER`s beyond 17
//!   significant digits collapsed to the same value and ordered arbitrarily.
//!   Comparison here is exact, on the digits themselves. This one bites under
//!   the shipped defaults.
//! * Dates were compared as text. Year-first renderings happen to sort right
//!   that way, and the app pins Oracle to one by default, so this only showed
//!   up once a session used something else — `DD-MON-RR` orders by month
//!   *name*, putting `APR` before `AUG` before `DEC`.
//!
//! This is a local sort over rows already fetched, and it is deliberately not a
//! reimplementation of the server's collation: text still compares by bytes.
//! Where exact server ordering matters, the caller should re-query with an
//! `ORDER BY` instead.

use std::cmp::Ordering;

use crate::db::SqlValueKind;

/// Where NULLs land on an ascending sort.
///
/// Oracle orders them last, the MySQL family first. The grid does not know
/// which connection produced it, so the caller resolves this and the grid
/// stores the answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NullOrdering {
    LastOnAscending,
    FirstOnAscending,
}

/// Everything needed to order one column's values.
#[derive(Clone, Copy, Debug)]
pub struct SortColumn {
    pub kind: SqlValueKind,
    pub nulls: NullOrdering,
}

/// Order two rendered cell values from the same column.
///
/// `left_null` / `right_null` come from the grid's own NULL rule rather than
/// being re-derived here, so a configured NULL display text stays a single
/// source of truth.
///
/// The returned order is always the ascending one; the caller reverses it for a
/// descending sort. NULL placement is expressed in ascending terms too, so
/// reversing moves NULLs to the other end, matching what the servers do.
pub fn compare_cell_values(
    left: &str,
    left_null: bool,
    right: &str,
    right_null: bool,
    column: SortColumn,
) -> Ordering {
    match (left_null, right_null) {
        (true, true) => return Ordering::Equal,
        (true, false) => {
            return match column.nulls {
                NullOrdering::LastOnAscending => Ordering::Greater,
                NullOrdering::FirstOnAscending => Ordering::Less,
            }
        }
        (false, true) => {
            return match column.nulls {
                NullOrdering::LastOnAscending => Ordering::Less,
                NullOrdering::FirstOnAscending => Ordering::Greater,
            }
        }
        (false, false) => {}
    }

    match column.kind {
        SqlValueKind::Number | SqlValueKind::Boolean => compare_as_number(left, right),
        SqlValueKind::Temporal => compare_as_temporal(left, right),
        // Text, raw bytes, and anything the driver could not classify keep the
        // previous behaviour: numeric when both sides look numeric, else text.
        SqlValueKind::String | SqlValueKind::Binary | SqlValueKind::Unknown => {
            compare_as_number(left, right)
        }
    }
}

/// Exact ordering for two rendered numbers, falling back to text when either
/// side is not a plain decimal.
fn compare_as_number(left: &str, right: &str) -> Ordering {
    match (parse_decimal(left), parse_decimal(right)) {
        (Some(left_value), Some(right_value)) => compare_decimal(&left_value, &right_value),
        // A number sorts ahead of text, which is what the grid always did.
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => left.cmp(right),
    }
}

/// Chronological ordering for two rendered dates, falling back to text when
/// either side is in a rendering this does not recognise.
fn compare_as_temporal(left: &str, right: &str) -> Ordering {
    match (parse_temporal(left), parse_temporal(right)) {
        (Some(left_key), Some(right_key)) => left_key.cmp(&right_key),
        _ => left.cmp(right),
    }
}

/// A decimal split into the pieces needed to compare it without rounding.
struct Decimal<'a> {
    negative: bool,
    integer: &'a str,
    fraction: &'a str,
}

impl Decimal<'_> {
    fn is_zero(&self) -> bool {
        self.integer.bytes().all(|byte| byte == b'0')
            && self.fraction.bytes().all(|byte| byte == b'0')
    }
}

/// Parse a plain decimal: an optional sign, digits, and an optional fraction.
///
/// Anything else — an exponent, a grouping separator, a currency symbol — is
/// rejected so the caller can fall back rather than compare a value it only
/// partly understood.
fn parse_decimal(value: &str) -> Option<Decimal<'_>> {
    let trimmed = value.trim();
    let (negative, rest) = match trimmed.as_bytes().first()? {
        b'-' => (true, &trimmed[1..]),
        b'+' => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };
    let (integer, fraction) = rest.split_once('.').unwrap_or((rest, ""));
    if integer.is_empty() && fraction.is_empty() {
        return None;
    }
    let digits_only = |text: &str| text.bytes().all(|byte| byte.is_ascii_digit());
    if !digits_only(integer) || !digits_only(fraction) {
        return None;
    }
    Some(Decimal {
        negative,
        integer,
        fraction,
    })
}

fn compare_decimal(left: &Decimal<'_>, right: &Decimal<'_>) -> Ordering {
    // `-0` and `0` are the same number however they were rendered.
    if left.is_zero() && right.is_zero() {
        return Ordering::Equal;
    }
    match (left.negative, right.negative) {
        (false, true) => return Ordering::Greater,
        (true, false) => return Ordering::Less,
        _ => {}
    }
    let magnitude = compare_magnitude(left, right);
    if left.negative {
        magnitude.reverse()
    } else {
        magnitude
    }
}

fn compare_magnitude(left: &Decimal<'_>, right: &Decimal<'_>) -> Ordering {
    let left_integer = left.integer.trim_start_matches('0');
    let right_integer = right.integer.trim_start_matches('0');
    // More significant digits means a larger magnitude, and equal lengths
    // compare digit by digit — no conversion, so no precision to lose.
    match left_integer.len().cmp(&right_integer.len()) {
        Ordering::Equal => {}
        other => return other,
    }
    match left_integer.cmp(right_integer) {
        Ordering::Equal => {}
        other => return other,
    }
    let left_fraction = left.fraction.as_bytes();
    let right_fraction = right.fraction.as_bytes();
    let width = left_fraction.len().max(right_fraction.len());
    for index in 0..width {
        let left_digit = left_fraction.get(index).copied().unwrap_or(b'0');
        let right_digit = right_fraction.get(index).copied().unwrap_or(b'0');
        match left_digit.cmp(&right_digit) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    Ordering::Equal
}

/// Year, month, day, hour, minute, second, fractional second — a tuple whose
/// natural order is chronological order.
type TemporalKey = (i32, u32, u32, u32, u32, u32, u32);

/// Recognise the renderings that reach the grid.
///
/// The app pins Oracle to `yyyy-mm-dd hh24:mi:ss` by default
/// (`ConnectionAdvancedSettings::default_oracle_nls_date_format`) and the MySQL
/// family renders ISO too, so year-first is the common case — and it happens to
/// sort correctly as text already. What this exists for is the sessions that do
/// not look like that: a user-chosen `NLS_DATE_FORMAT` such as
/// `YYYY/MM/DD HH24:MI:SS`, or the database's own `DD-MON-RR` default when the
/// setting is cleared.
///
/// Year-first dates accept `-` or `/` as the separator. A format this does not
/// recognise — `MM/DD/YYYY`, say, which cannot be told apart from `DD/MM/YYYY`
/// by looking — falls back to text ordering rather than guessing.
fn parse_temporal(value: &str) -> Option<TemporalKey> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (date_part, time_part) = split_date_and_time(trimmed);
    // One spelling to parse: `/` and `-` never mean different things here.
    let date_part = date_part.replace('/', "-");
    let (year, month, day) =
        parse_iso_date(&date_part).or_else(|| parse_dd_mon_date(&date_part))?;
    let (hour, minute, second, fraction) = parse_time(time_part)?;
    Some((year, month, day, hour, minute, second, fraction))
}

fn split_date_and_time(value: &str) -> (&str, &str) {
    match value.split_once([' ', 'T', 't']) {
        Some((date, time)) => (date, time.trim()),
        None => (value, ""),
    }
}

fn parse_iso_date(value: &str) -> Option<(i32, u32, u32)> {
    let mut parts = value.split('-');
    let year = parts.next()?;
    let month = parts.next()?;
    let day = parts.next()?;
    if parts.next().is_some() || year.len() != 4 {
        return None;
    }
    Some((
        parse_digits(year)? as i32,
        parse_digits(month)?,
        parse_digits(day)?,
    ))
}

fn parse_dd_mon_date(value: &str) -> Option<(i32, u32, u32)> {
    let mut parts = value.split('-');
    let day = parts.next()?;
    let month = parts.next()?;
    let year = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let month = month_from_abbreviation(month)?;
    let day = parse_digits(day)?;
    let year_digits = parse_digits(year)?;
    let year = match year.len() {
        4 => year_digits as i32,
        // Oracle's RR rule: 00-49 is this century, 50-99 the previous one.
        2 if year_digits < 50 => 2000 + year_digits as i32,
        2 => 1900 + year_digits as i32,
        _ => return None,
    };
    Some((year, month, day))
}

fn month_from_abbreviation(value: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
    ];
    let upper = value.to_ascii_uppercase();
    MONTHS
        .iter()
        .position(|month| upper.starts_with(month))
        .map(|index| index as u32 + 1)
}

/// Parse `HH:MI:SS[.fraction]`, treating an absent time as midnight. Trailing
/// text a time zone or meridian would add is not understood, so it fails and
/// the whole value falls back to text ordering.
fn parse_time(value: &str) -> Option<(u32, u32, u32, u32)> {
    if value.is_empty() {
        return Some((0, 0, 0, 0));
    }
    let (clock, fraction) = value.split_once('.').unwrap_or((value, "0"));
    let mut parts = clock.split(':');
    let hour = parse_digits(parts.next()?)?;
    let minute = parse_digits(parts.next()?)?;
    let second = match parts.next() {
        Some(text) => parse_digits(text)?,
        None => 0,
    };
    if parts.next().is_some() {
        return None;
    }
    // Compare fractions on a common scale so `.5` outranks `.25`.
    let mut scaled = String::from(fraction);
    scaled.truncate(9);
    while scaled.len() < 9 {
        scaled.push('0');
    }
    Some((hour, minute, second, parse_digits(&scaled)?))
}

fn parse_digits(value: &str) -> Option<u32> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(kind: SqlValueKind) -> SortColumn {
        SortColumn {
            kind,
            nulls: NullOrdering::LastOnAscending,
        }
    }

    fn compare(left: &str, right: &str, kind: SqlValueKind) -> Ordering {
        compare_cell_values(left, false, right, false, column(kind))
    }

    #[test]
    fn numbers_order_by_value_not_by_text() {
        assert_eq!(compare("9", "10", SqlValueKind::Number), Ordering::Less);
    }

    #[test]
    fn numbers_beyond_f64_precision_stay_distinct() {
        // Oracle NUMBER carries 38 significant digits; f64 keeps about 17, so
        // these two collapsed to the same value and ordered arbitrarily.
        let left = "12345678901234567890123456789012345678";
        let right = "12345678901234567890123456789012345679";
        assert_eq!(compare(left, right, SqlValueKind::Number), Ordering::Less);
        assert_eq!(
            compare(right, left, SqlValueKind::Number),
            Ordering::Greater
        );
    }

    #[test]
    fn long_numbers_differing_in_the_last_digit_are_not_equal() {
        let left = "9007199254740993";
        let right = "9007199254740992";
        assert_eq!(
            compare(left, right, SqlValueKind::Number),
            Ordering::Greater
        );
    }

    #[test]
    fn negative_numbers_order_below_positive_ones() {
        assert_eq!(compare("-1", "1", SqlValueKind::Number), Ordering::Less);
        assert_eq!(compare("-10", "-9", SqlValueKind::Number), Ordering::Less);
    }

    #[test]
    fn negative_zero_equals_zero() {
        assert_eq!(compare("-0", "0", SqlValueKind::Number), Ordering::Equal);
        assert_eq!(compare("-0.00", "0", SqlValueKind::Number), Ordering::Equal);
    }

    #[test]
    fn fractions_compare_digit_by_digit() {
        assert_eq!(
            compare("0.5", "0.25", SqlValueKind::Number),
            Ordering::Greater
        );
        assert_eq!(
            compare("1.10", "1.1", SqlValueKind::Number),
            Ordering::Equal
        );
    }

    #[test]
    fn leading_zeros_do_not_change_a_number() {
        assert_eq!(compare("007", "7", SqlValueKind::Number), Ordering::Equal);
        assert_eq!(compare("007", "10", SqlValueKind::Number), Ordering::Less);
    }

    #[test]
    fn a_plus_sign_is_accepted() {
        assert_eq!(compare("+5", "5", SqlValueKind::Number), Ordering::Equal);
    }

    #[test]
    fn unparseable_numbers_fall_back_to_text_order() {
        // Scientific notation is deliberately not decoded; falling back is
        // better than comparing a value only half understood.
        assert_eq!(
            compare("1.5E+30", "1.5E+30", SqlValueKind::Number),
            Ordering::Equal
        );
        assert_eq!(compare("abc", "abd", SqlValueKind::Number), Ordering::Less);
    }

    #[test]
    fn numbers_sort_ahead_of_text_in_a_mixed_column() {
        assert_eq!(compare("5", "apple", SqlValueKind::Unknown), Ordering::Less);
        assert_eq!(
            compare("apple", "5", SqlValueKind::Unknown),
            Ordering::Greater
        );
    }

    #[test]
    fn oracle_default_dates_order_chronologically_not_alphabetically() {
        // The headline defect: as text, APR < AUG < DEC < FEB.
        let mut values = vec!["17-DEC-80", "20-FEB-81", "02-APR-81", "01-MAY-81"];
        values.sort_by(|left, right| compare(left, right, SqlValueKind::Temporal));
        assert_eq!(
            values,
            vec!["17-DEC-80", "20-FEB-81", "02-APR-81", "01-MAY-81"]
        );
    }

    #[test]
    fn two_digit_years_follow_the_rr_rule() {
        // 80 is 1980, 05 is 2005, so the 1980 date sorts first.
        assert_eq!(
            compare("17-DEC-80", "01-JAN-05", SqlValueKind::Temporal),
            Ordering::Less
        );
    }

    #[test]
    fn four_digit_years_are_taken_literally() {
        assert_eq!(
            compare("17-DEC-1980", "01-JAN-2005", SqlValueKind::Temporal),
            Ordering::Less
        );
    }

    #[test]
    fn iso_dates_order_chronologically() {
        assert_eq!(
            compare("1980-12-17", "1981-02-20", SqlValueKind::Temporal),
            Ordering::Less
        );
    }

    #[test]
    fn the_apps_default_oracle_rendering_orders_chronologically() {
        // ConnectionAdvancedSettings pins NLS_DATE_FORMAT to
        // `yyyy-mm-dd hh24:mi:ss`, so this is what Oracle dates normally look
        // like in the grid.
        assert_eq!(
            compare(
                "1980-12-17 00:00:00",
                "1981-02-20 00:00:00",
                SqlValueKind::Temporal
            ),
            Ordering::Less
        );
    }

    #[test]
    fn a_slash_separated_year_first_format_orders_chronologically() {
        // `YYYY/MM/DD HH24:MI:SS` is a format the connection dialog accepts.
        assert_eq!(
            compare(
                "1980/12/17 00:00:00",
                "1981/02/20 00:00:00",
                SqlValueKind::Temporal
            ),
            Ordering::Less
        );
    }

    #[test]
    fn a_day_first_ambiguous_format_is_not_guessed() {
        // `10-11-12` could be DD-MM-YY or MM-DD-YY; text order is the honest
        // answer rather than picking one.
        let (left, right) = ("10-11-12", "09-12-11");
        assert_eq!(
            compare(left, right, SqlValueKind::Temporal),
            left.cmp(right)
        );
    }

    #[test]
    fn iso_timestamps_order_by_time_within_a_day() {
        assert_eq!(
            compare(
                "2026-08-06 09:30:00",
                "2026-08-06 11:00:00",
                SqlValueKind::Temporal
            ),
            Ordering::Less
        );
    }

    #[test]
    fn fractional_seconds_compare_on_a_common_scale() {
        assert_eq!(
            compare(
                "2026-08-06 09:30:00.5",
                "2026-08-06 09:30:00.25",
                SqlValueKind::Temporal
            ),
            Ordering::Greater
        );
    }

    #[test]
    fn a_dated_value_with_a_time_outranks_the_same_date_at_midnight() {
        assert_eq!(
            compare("2026-08-06", "2026-08-06 00:00:01", SqlValueKind::Temporal),
            Ordering::Less
        );
    }

    #[test]
    fn an_unrecognised_date_format_falls_back_to_text_order() {
        // A session using a different NLS_DATE_FORMAT keeps the old behaviour
        // rather than getting a wrong chronological answer. Assert against the
        // text order itself so the test states the rule, not a direction.
        for (left, right) in [("12/17/1980", "02/20/1981"), ("not a date", "also not")] {
            assert_eq!(
                compare(left, right, SqlValueKind::Temporal),
                left.cmp(right),
                "{left} vs {right} should fall back to text order"
            );
        }
    }

    #[test]
    fn text_columns_are_not_reinterpreted_as_dates() {
        // A VARCHAR2 holding date-like text still compares as text, even
        // though the same rendering would parse as a date.
        let (left, right) = ("17-DEC-80", "02-APR-81");
        assert_eq!(compare(left, right, SqlValueKind::String), left.cmp(right));
        assert_ne!(
            compare(left, right, SqlValueKind::String),
            compare(left, right, SqlValueKind::Temporal),
            "the kind must decide how the value is read"
        );
    }

    #[test]
    fn nulls_sort_last_on_ascending_for_oracle() {
        let column = SortColumn {
            kind: SqlValueKind::Number,
            nulls: NullOrdering::LastOnAscending,
        };
        assert_eq!(
            compare_cell_values("", true, "1", false, column),
            Ordering::Greater
        );
        assert_eq!(
            compare_cell_values("1", false, "", true, column),
            Ordering::Less
        );
    }

    #[test]
    fn nulls_sort_first_on_ascending_for_the_mysql_family() {
        let column = SortColumn {
            kind: SqlValueKind::Number,
            nulls: NullOrdering::FirstOnAscending,
        };
        assert_eq!(
            compare_cell_values("", true, "1", false, column),
            Ordering::Less
        );
        assert_eq!(
            compare_cell_values("1", false, "", true, column),
            Ordering::Greater
        );
    }

    #[test]
    fn two_nulls_are_equal_so_the_sort_stays_stable() {
        let column = SortColumn {
            kind: SqlValueKind::Number,
            nulls: NullOrdering::LastOnAscending,
        };
        assert_eq!(
            compare_cell_values("", true, "(null)", true, column),
            Ordering::Equal
        );
    }

    #[test]
    fn a_null_is_never_compared_by_its_display_text() {
        // `(null)` must not sort among the letters just because that is what
        // the cell shows.
        let column = SortColumn {
            kind: SqlValueKind::String,
            nulls: NullOrdering::LastOnAscending,
        };
        assert_eq!(
            compare_cell_values("(null)", true, "zebra", false, column),
            Ordering::Greater
        );
    }

    #[test]
    fn booleans_order_numerically() {
        assert_eq!(compare("0", "1", SqlValueKind::Boolean), Ordering::Less);
    }
}
