//! Turn a result-grid selection into SQL text: `SQL Inserts`, `SQL Updates`,
//! and `Where Clause`.
//!
//! Pure functions over a snapshot of the grid — no FLTK, no database — so the
//! exact SQL a user gets on the clipboard is unit-testable.
//!
//! Literal rendering is driven by [`SqlValueKind`], the type each driver
//! reported for the column, never by the shape of the value. That is what keeps
//! a `VARCHAR2` holding `2024-01-01` from being wrapped in `TO_DATE`, and a
//! zero-padded code like `00123` from collapsing into a number.

use crate::db::{quote_mysql_identifier, DatabaseType, SqlValueKind};
use crate::ui::result_table::ResultTableWidget;

/// The table name used when the base table cannot be resolved from the SQL
/// (a join, a CTE, a synthetic grid). Same placeholder DataGrip emits.
const UNKNOWN_TABLE_NAME: &str = "MY_TABLE";

/// A snapshot of what the user selected in the grid.
///
/// Rows are carried at full width, with the selection expressed as indexes into
/// `all_columns`, so `SQL Updates` can read primary-key values from columns the
/// user did not select.
#[derive(Clone, Debug)]
pub struct GridSqlSelection {
    pub db_type: DatabaseType,
    /// Resolved base table, already qualified. `None` renders as `MY_TABLE`.
    pub table: Option<String>,
    /// Every non-internal grid column, in grid order.
    pub all_columns: Vec<String>,
    /// Literal kind per `all_columns` entry. Shorter than `all_columns` (in
    /// practice empty) means "unknown", i.e. quote everything.
    pub column_kinds: Vec<SqlValueKind>,
    /// Indexes into `all_columns` covered by the selection rectangle.
    pub selected_columns: Vec<usize>,
    /// Selected rows, each aligned to `all_columns`.
    pub rows: Vec<Vec<String>>,
    /// Display text the grid uses for SQL NULL.
    pub null_text: String,
}

impl GridSqlSelection {
    fn table_name(&self) -> String {
        match self.table.as_deref().map(str::trim) {
            Some(table) if !table.is_empty() => self.quote_identifier(table),
            _ => UNKNOWN_TABLE_NAME.to_string(),
        }
    }

    /// Quote a possibly dot-qualified name for this backend.
    fn quote_identifier(&self, name: &str) -> String {
        quote_qualified_name(self.db_type, name)
    }

    fn quote_column(&self, index: usize) -> String {
        let name = self.all_columns.get(index).map_or("", String::as_str);
        quote_column_name(self.db_type, name)
    }

    fn kind(&self, index: usize) -> SqlValueKind {
        self.column_kinds
            .get(index)
            .copied()
            .unwrap_or(SqlValueKind::Unknown)
    }

    fn cell(&self, row: &[String], index: usize) -> String {
        row.get(index).cloned().unwrap_or_default()
    }

    fn is_null(&self, row: &[String], index: usize) -> bool {
        ResultTableWidget::value_represents_null(&self.cell(row, index), &self.null_text)
    }

    fn literal(&self, row: &[String], index: usize) -> String {
        sql_literal(
            self.db_type,
            self.kind(index),
            &self.cell(row, index),
            &self.null_text,
        )
    }

    /// Column index for a name, matched case-insensitively the way SQL resolves
    /// unquoted identifiers.
    fn column_index(&self, name: &str) -> Option<usize> {
        let wanted = name.trim();
        self.all_columns
            .iter()
            .position(|column| column.trim().eq_ignore_ascii_case(wanted))
    }
}

/// The base table the generated SQL should name.
///
/// The grid-edit descriptor already names the exact table, so it wins.
/// Otherwise the table is resolved from the SQL that produced the grid, which
/// handles CTEs and `alias.ROWID` select lists. `None` renders as `MY_TABLE`.
/// Quote a possibly dot-qualified object name for `db_type`.
pub fn quote_qualified_name(db_type: DatabaseType, name: &str) -> String {
    if db_type.is_mysql_or_mariadb() {
        name.split('.')
            .map(|segment| quote_mysql_identifier(segment.trim()))
            .collect::<Vec<_>>()
            .join(".")
    } else {
        // Oracle: legal unquoted identifiers stay unquoted, so generated SQL
        // reads the way a person would write it.
        ResultTableWidget::quote_qualified_identifier(name)
    }
}

/// Quote a single column name for `db_type`.
pub fn quote_column_name(db_type: DatabaseType, name: &str) -> String {
    if db_type.is_mysql_or_mariadb() {
        quote_mysql_identifier(name.trim())
    } else {
        ResultTableWidget::quote_identifier_segment(name)
    }
}

pub fn resolve_export_table(descriptor_table: Option<String>, source_sql: &str) -> Option<String> {
    descriptor_table
        .or_else(|| crate::ui::sql_editor::query_text::resolve_edit_target_table(source_sql).ok())
        .filter(|table| !table.trim().is_empty())
}

/// `INSERT INTO <table> (<selected columns>) VALUES (…);` per selected row.
pub fn build_sql_inserts(selection: &GridSqlSelection) -> String {
    if selection.selected_columns.is_empty() || selection.rows.is_empty() {
        return String::new();
    }
    let table = selection.table_name();
    let columns = selection
        .selected_columns
        .iter()
        .map(|index| selection.quote_column(*index))
        .collect::<Vec<_>>()
        .join(", ");

    let mut out = String::new();
    for row in &selection.rows {
        let values = selection
            .selected_columns
            .iter()
            .map(|index| selection.literal(row, *index))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "INSERT INTO {table} ({columns}) VALUES ({values});\n"
        ));
    }
    out
}

/// `UPDATE <table> SET … WHERE <key columns>;` per selected row.
///
/// `key_columns` are primary-key column names. When none are known the WHERE
/// clause is omitted, matching DataGrip: it is the caller's job to tell the user
/// that happened.
pub fn build_sql_updates(selection: &GridSqlSelection, key_columns: &[String]) -> String {
    if selection.selected_columns.is_empty() || selection.rows.is_empty() {
        return String::new();
    }
    let table = selection.table_name();

    // A key column absent from the result set has no value to compare, so it
    // cannot take part in the WHERE clause.
    let keys: Vec<usize> = key_columns
        .iter()
        .filter_map(|name| selection.column_index(name))
        .collect();

    let mut assigned: Vec<usize> = selection
        .selected_columns
        .iter()
        .copied()
        .filter(|index| !keys.contains(index))
        .collect();
    // Selecting only key columns would leave nothing to SET; assign them rather
    // than emit a syntactically invalid statement.
    if assigned.is_empty() {
        assigned = selection.selected_columns.clone();
    }

    let mut out = String::new();
    for row in &selection.rows {
        let assignments = assigned
            .iter()
            .map(|index| {
                format!(
                    "{} = {}",
                    selection.quote_column(*index),
                    selection.literal(row, *index)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let mut statement = format!("UPDATE {table} SET {assignments}");
        if !keys.is_empty() {
            let predicates = keys
                .iter()
                .map(|index| equality_predicate(selection, row, *index))
                .collect::<Vec<_>>()
                .join(" AND ");
            statement.push_str(&format!(" WHERE {predicates}"));
        }
        statement.push_str(";\n");
        out.push_str(&statement);
    }
    out
}

/// A WHERE condition that matches exactly the selected cells.
///
/// Values in one row are AND-combined, rows are OR-combined, and a
/// single-column selection collapses into `IN` — DataGrip's rules. Two
/// departures, both because the alternative cannot match: a lone value uses `=`
/// instead of `IN (x)`, and NULLs are lifted out of the `IN` list into
/// `IS NULL`, since `IN` never matches NULL.
pub fn build_where_clause(selection: &GridSqlSelection) -> String {
    if selection.selected_columns.is_empty() || selection.rows.is_empty() {
        return String::new();
    }

    if let [column] = selection.selected_columns.as_slice() {
        return single_column_where(selection, *column);
    }

    let mut groups: Vec<String> = Vec::new();
    for row in &selection.rows {
        let group = selection
            .selected_columns
            .iter()
            .map(|index| equality_predicate(selection, row, *index))
            .collect::<Vec<_>>()
            .join(" AND ");
        if !group.is_empty() && !groups.contains(&group) {
            groups.push(group);
        }
    }

    match groups.len() {
        0 => String::new(),
        // One row needs no grouping parentheses.
        1 => groups.remove(0),
        _ => groups
            .into_iter()
            .map(|group| format!("({group})"))
            .collect::<Vec<_>>()
            .join(" OR "),
    }
}

fn single_column_where(selection: &GridSqlSelection, column: usize) -> String {
    let name = selection.quote_column(column);
    let mut values: Vec<String> = Vec::new();
    let mut has_null = false;
    for row in &selection.rows {
        if selection.is_null(row, column) {
            has_null = true;
            continue;
        }
        let literal = selection.literal(row, column);
        if !values.contains(&literal) {
            values.push(literal);
        }
    }

    let mut clause = match values.len() {
        0 => String::new(),
        1 => format!("{name} = {}", values[0]),
        _ => format!("{name} IN ({})", values.join(", ")),
    };
    if has_null {
        let null_test = format!("{name} IS NULL");
        if clause.is_empty() {
            clause = null_test;
        } else {
            clause = format!("{clause} OR {null_test}");
        }
    }
    clause
}

fn equality_predicate(selection: &GridSqlSelection, row: &[String], index: usize) -> String {
    let name = selection.quote_column(index);
    if selection.is_null(row, index) {
        format!("{name} IS NULL")
    } else {
        format!("{name} = {}", selection.literal(row, index))
    }
}

/// Render one displayed cell value as a SQL literal for `db_type`.
pub fn sql_literal(
    db_type: DatabaseType,
    kind: SqlValueKind,
    value: &str,
    null_text: &str,
) -> String {
    if ResultTableWidget::value_represents_null(value, null_text) {
        return "NULL".to_string();
    }
    sql_literal_for_value(db_type, kind, value)
}

/// Render `value` as a literal for `kind` with no NULL detection at all.
///
/// [`sql_literal`] reads the grid, where an empty cell and the text `NULL` both
/// mean SQL NULL. A caller that already knows a value is not NULL — a file
/// import, where the format said so explicitly — needs the empty string to stay
/// the empty string, and this is that entry point.
pub fn sql_literal_for_value(db_type: DatabaseType, kind: SqlValueKind, value: &str) -> String {
    let mysql_family = db_type.is_mysql_or_mariadb();
    match kind {
        SqlValueKind::Number | SqlValueKind::Boolean => value.trim().to_string(),
        SqlValueKind::Temporal => {
            if mysql_family {
                // MySQL and MariaDB accept the ISO text the grid already shows.
                quoted_string(value, mysql_family)
            } else {
                oracle_temporal_literal(value)
            }
        }
        SqlValueKind::Binary => {
            if mysql_family {
                // The bytes are gone: the grid holds a lossy UTF-8 rendering of
                // them, so the displayed text is all there is to emit.
                quoted_string(value, mysql_family)
            } else {
                // Oracle RAW is displayed as uppercase hex, which HEXTORAW
                // turns back into the same bytes.
                format!("HEXTORAW('{}')", value.trim())
            }
        }
        SqlValueKind::String | SqlValueKind::Unknown => quoted_string(value, mysql_family),
    }
}

fn quoted_string(value: &str, mysql_family: bool) -> String {
    let escaped = if mysql_family {
        // MySQL and MariaDB treat `\` as an escape inside string literals unless
        // NO_BACKSLASH_ESCAPES is set, and this app defaults to
        // sql_mode=TRADITIONAL, which does not set it.
        value.replace('\\', "\\\\").replace('\'', "''")
    } else {
        value.replace('\'', "''")
    };
    format!("'{escaped}'")
}

/// Wrap an Oracle date/timestamp in the conversion its displayed shape needs.
///
/// The shapes are exhaustive over what the Oracle executors render, so an
/// unrecognized one means the value is not a plain date — an INTERVAL, say —
/// and is safest emitted as a string.
fn oracle_temporal_literal(value: &str) -> String {
    let text = value.trim();
    let (datetime, zone) = split_timezone_suffix(text);
    let (date_part, time_part) = match datetime.split_once(' ') {
        Some((date, time)) => (date, Some(time)),
        None => (datetime, None),
    };
    if !is_iso_date(date_part) {
        return quoted_string(value, false);
    }
    let Some(time_part) = time_part else {
        return format!("TO_DATE('{text}','YYYY-MM-DD')");
    };
    let (clock, fraction) = match time_part.split_once('.') {
        Some((clock, fraction)) => (clock, Some(fraction)),
        None => (time_part, None),
    };
    if !is_clock_time(clock) {
        return quoted_string(value, false);
    }
    match (fraction, zone) {
        (None, None) => format!("TO_DATE('{text}','YYYY-MM-DD HH24:MI:SS')"),
        (Some(fraction), None) if is_all_digits(fraction) => {
            format!("TO_TIMESTAMP('{text}','YYYY-MM-DD HH24:MI:SS.FF')")
        }
        // The zone is re-joined with an explicit space so the text matches the
        // format model exactly. The drivers render the offset without one, and
        // Oracle then reads `TZH` off a value whose sign is where the space
        // should be — silently turning `-05:30` into `+05:30`.
        (Some(fraction), Some(zone)) if is_all_digits(fraction) => {
            format!("TO_TIMESTAMP_TZ('{datetime} {zone}','YYYY-MM-DD HH24:MI:SS.FF TZH:TZM')")
        }
        (None, Some(zone)) => {
            format!("TO_TIMESTAMP_TZ('{datetime} {zone}','YYYY-MM-DD HH24:MI:SS TZH:TZM')")
        }
        _ => quoted_string(value, false),
    }
}

/// Split a trailing `+HH:MM` / `-HH:MM` zone offset from a rendered timestamp.
fn split_timezone_suffix(text: &str) -> (&str, Option<&str>) {
    let Some(position) = text.rfind(['+', '-']) else {
        return (text, None);
    };
    // A leading sign is part of the value, not an offset, and the date's own
    // `-` separators sit before any time component.
    if position == 0 || !text[..position].contains(':') {
        return (text, None);
    }
    let (head, offset) = text.split_at(position);
    let digits = &offset[1..];
    match digits.split_once(':') {
        Some((hours, minutes))
            if hours.len() == 2
                && minutes.len() == 2
                && is_all_digits(hours)
                && is_all_digits(minutes) =>
        {
            (head.trim_end(), Some(offset))
        }
        _ => (text, None),
    }
}

fn is_iso_date(text: &str) -> bool {
    let mut parts = text.split('-');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(year), Some(month), Some(day), None)
            if year.len() == 4
                && month.len() == 2
                && day.len() == 2
                && is_all_digits(year)
                && is_all_digits(month)
                && is_all_digits(day)
    )
}

fn is_clock_time(text: &str) -> bool {
    let mut parts = text.split(':');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(hour), Some(minute), Some(second), None)
            if hour.len() == 2
                && minute.len() == 2
                && second.len() == 2
                && is_all_digits(hour)
                && is_all_digits(minute)
                && is_all_digits(second)
    )
}

fn is_all_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NULL_TEXT: &str = "NULL";

    fn selection(
        db_type: DatabaseType,
        columns: &[(&str, SqlValueKind)],
        rows: &[&[&str]],
    ) -> GridSqlSelection {
        GridSqlSelection {
            db_type,
            table: Some("HR.EMP".to_string()),
            all_columns: columns
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect(),
            column_kinds: columns.iter().map(|(_, kind)| *kind).collect(),
            selected_columns: (0..columns.len()).collect(),
            rows: rows
                .iter()
                .map(|row| row.iter().map(|value| (*value).to_string()).collect())
                .collect(),
            null_text: NULL_TEXT.to_string(),
        }
    }

    fn oracle_literal(kind: SqlValueKind, value: &str) -> String {
        sql_literal(DatabaseType::Oracle, kind, value, NULL_TEXT)
    }

    fn mysql_literal(kind: SqlValueKind, value: &str) -> String {
        sql_literal(DatabaseType::MySQL, kind, value, NULL_TEXT)
    }

    #[test]
    fn null_wins_over_every_kind() {
        for kind in [
            SqlValueKind::Unknown,
            SqlValueKind::String,
            SqlValueKind::Number,
            SqlValueKind::Boolean,
            SqlValueKind::Temporal,
            SqlValueKind::Binary,
        ] {
            assert_eq!(oracle_literal(kind, "NULL"), "NULL");
            assert_eq!(mysql_literal(kind, "NULL"), "NULL");
            assert_eq!(
                sql_literal(DatabaseType::MariaDB, kind, "", NULL_TEXT),
                "NULL"
            );
        }
    }

    #[test]
    fn number_kind_emits_bare_values() {
        assert_eq!(oracle_literal(SqlValueKind::Number, "0"), "0");
        assert_eq!(oracle_literal(SqlValueKind::Number, "123"), "123");
        assert_eq!(oracle_literal(SqlValueKind::Number, "-1.5"), "-1.5");
        assert_eq!(oracle_literal(SqlValueKind::Number, "1.2E+10"), "1.2E+10");
        assert_eq!(mysql_literal(SqlValueKind::Number, "42"), "42");
    }

    #[test]
    fn string_kind_keeps_leading_zeros_and_digits_quoted() {
        // The regression that made grid-edit drop numeric guessing: a char
        // column holding a zero-padded code must not become a number.
        assert_eq!(oracle_literal(SqlValueKind::String, "00123"), "'00123'");
        assert_eq!(oracle_literal(SqlValueKind::String, "123"), "'123'");
    }

    #[test]
    fn string_kind_escapes_quotes_and_backslashes_per_backend() {
        assert_eq!(oracle_literal(SqlValueKind::String, "it's"), "'it''s'");
        assert_eq!(mysql_literal(SqlValueKind::String, "it's"), "'it''s'");
        // Oracle takes a backslash literally; MySQL/MariaDB do not.
        assert_eq!(
            oracle_literal(SqlValueKind::String, r"a\b"),
            r"'a\b'".to_string()
        );
        assert_eq!(mysql_literal(SqlValueKind::String, r"a\b"), r"'a\\b'");
        assert_eq!(
            sql_literal(
                DatabaseType::MariaDB,
                SqlValueKind::String,
                r"a\b",
                NULL_TEXT
            ),
            r"'a\\b'"
        );
    }

    #[test]
    fn date_shaped_text_in_a_string_column_stays_a_string() {
        // The whole point of classifying by driver type instead of value shape.
        assert_eq!(
            oracle_literal(SqlValueKind::String, "2024-01-01 10:00:00"),
            "'2024-01-01 10:00:00'"
        );
    }

    #[test]
    fn oracle_temporal_kind_picks_the_matching_conversion() {
        assert_eq!(
            oracle_literal(SqlValueKind::Temporal, "1980-12-17"),
            "TO_DATE('1980-12-17','YYYY-MM-DD')"
        );
        assert_eq!(
            oracle_literal(SqlValueKind::Temporal, "1980-12-17 09:30:00"),
            "TO_DATE('1980-12-17 09:30:00','YYYY-MM-DD HH24:MI:SS')"
        );
        assert_eq!(
            oracle_literal(SqlValueKind::Temporal, "1980-12-17 09:30:00.123456"),
            "TO_TIMESTAMP('1980-12-17 09:30:00.123456','YYYY-MM-DD HH24:MI:SS.FF')"
        );
        // The offset is separated from the time by a space so it lines up
        // with `TZH:TZM` in the format model. Without it Oracle reads the sign
        // as the separator and a negative offset comes back positive.
        assert_eq!(
            oracle_literal(SqlValueKind::Temporal, "1980-12-17 09:30:00.123456+09:00"),
            "TO_TIMESTAMP_TZ('1980-12-17 09:30:00.123456 +09:00','YYYY-MM-DD HH24:MI:SS.FF TZH:TZM')"
        );
        assert_eq!(
            oracle_literal(SqlValueKind::Temporal, "1980-12-17 09:30:00-05:30"),
            "TO_TIMESTAMP_TZ('1980-12-17 09:30:00 -05:30','YYYY-MM-DD HH24:MI:SS TZH:TZM')"
        );
        // A driver that already renders the space produces the same literal.
        assert_eq!(
            oracle_literal(SqlValueKind::Temporal, "1980-12-17 09:30:00.123456 -05:30"),
            "TO_TIMESTAMP_TZ('1980-12-17 09:30:00.123456 -05:30','YYYY-MM-DD HH24:MI:SS.FF TZH:TZM')"
        );
    }

    #[test]
    fn oracle_temporal_kind_falls_back_to_a_string_for_intervals() {
        // INTERVAL and TIME render in shapes no TO_DATE model fits.
        assert_eq!(
            oracle_literal(SqlValueKind::Temporal, "+000000002 03:04:05.000000"),
            "'+000000002 03:04:05.000000'"
        );
        assert_eq!(oracle_literal(SqlValueKind::Temporal, "-01:30"), "'-01:30'");
    }

    #[test]
    fn mysql_temporal_kind_quotes_the_iso_text() {
        assert_eq!(
            mysql_literal(SqlValueKind::Temporal, "1980-12-17 09:30:00"),
            "'1980-12-17 09:30:00'"
        );
        assert_eq!(
            mysql_literal(SqlValueKind::Temporal, "23:59:59"),
            "'23:59:59'"
        );
    }

    #[test]
    fn binary_kind_round_trips_on_oracle_and_quotes_on_mysql() {
        assert_eq!(
            oracle_literal(SqlValueKind::Binary, "DEADBEEF"),
            "HEXTORAW('DEADBEEF')"
        );
        assert_eq!(mysql_literal(SqlValueKind::Binary, "abc"), "'abc'");
    }

    #[test]
    fn unknown_kind_quotes_lob_placeholders() {
        assert_eq!(oracle_literal(SqlValueKind::Unknown, "[LOB]"), "'[LOB]'");
    }

    #[test]
    fn inserts_cover_one_statement_per_row() {
        let selection = selection(
            DatabaseType::Oracle,
            &[
                ("ID", SqlValueKind::Number),
                ("NAME", SqlValueKind::String),
                ("HIREDATE", SqlValueKind::Temporal),
            ],
            &[
                &["7369", "SMITH", "1980-12-17 00:00:00"],
                &["7499", "ALLEN", "NULL"],
            ],
        );
        assert_eq!(
            build_sql_inserts(&selection),
            "INSERT INTO HR.EMP (ID, NAME, HIREDATE) VALUES (7369, 'SMITH', \
             TO_DATE('1980-12-17 00:00:00','YYYY-MM-DD HH24:MI:SS'));\n\
             INSERT INTO HR.EMP (ID, NAME, HIREDATE) VALUES (7499, 'ALLEN', NULL);\n"
        );
    }

    #[test]
    fn inserts_use_backticks_on_mysql() {
        let selection = selection(
            DatabaseType::MySQL,
            &[("id", SqlValueKind::Number), ("name", SqlValueKind::String)],
            &[&["1", "kim"]],
        );
        assert_eq!(
            build_sql_inserts(&selection),
            "INSERT INTO `HR`.`EMP` (`id`, `name`) VALUES (1, 'kim');\n"
        );
    }

    #[test]
    fn inserts_fall_back_to_my_table_when_unresolved() {
        let mut selection = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number)],
            &[&["1"]],
        );
        selection.table = None;
        assert_eq!(
            build_sql_inserts(&selection),
            "INSERT INTO MY_TABLE (ID) VALUES (1);\n"
        );
    }

    #[test]
    fn inserts_of_an_empty_selection_produce_nothing() {
        let mut selection = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number)],
            &[&["1"]],
        );
        selection.selected_columns.clear();
        assert!(build_sql_inserts(&selection).is_empty());
        assert!(build_sql_updates(&selection, &["ID".to_string()]).is_empty());
        assert!(build_where_clause(&selection).is_empty());
    }

    #[test]
    fn updates_set_non_key_columns_and_match_on_the_key() {
        let selection = selection(
            DatabaseType::Oracle,
            &[
                ("ID", SqlValueKind::Number),
                ("NAME", SqlValueKind::String),
                ("SAL", SqlValueKind::Number),
            ],
            &[&["7369", "SMITH", "800"]],
        );
        assert_eq!(
            build_sql_updates(&selection, &["ID".to_string()]),
            "UPDATE HR.EMP SET NAME = 'SMITH', SAL = 800 WHERE ID = 7369;\n"
        );
    }

    #[test]
    fn updates_match_on_a_composite_key() {
        let selection = selection(
            DatabaseType::Oracle,
            &[
                ("PART", SqlValueKind::String),
                ("SEQ", SqlValueKind::Number),
                ("QTY", SqlValueKind::Number),
            ],
            &[&["A-1", "2", "10"]],
        );
        assert_eq!(
            build_sql_updates(&selection, &["PART".to_string(), "SEQ".to_string()]),
            "UPDATE HR.EMP SET QTY = 10 WHERE PART = 'A-1' AND SEQ = 2;\n"
        );
    }

    #[test]
    fn updates_read_key_values_from_unselected_columns() {
        let mut selection = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number), ("NAME", SqlValueKind::String)],
            &[&["7369", "SMITH"]],
        );
        // The user selected only NAME; the key value still comes from the row.
        selection.selected_columns = vec![1];
        assert_eq!(
            build_sql_updates(&selection, &["ID".to_string()]),
            "UPDATE HR.EMP SET NAME = 'SMITH' WHERE ID = 7369;\n"
        );
    }

    #[test]
    fn updates_omit_where_when_no_key_is_known() {
        let selection = selection(
            DatabaseType::Oracle,
            &[("NAME", SqlValueKind::String)],
            &[&["SMITH"]],
        );
        assert_eq!(
            build_sql_updates(&selection, &[]),
            "UPDATE HR.EMP SET NAME = 'SMITH';\n"
        );
    }

    #[test]
    fn updates_omit_where_when_the_key_is_not_in_the_result_set() {
        let selection = selection(
            DatabaseType::Oracle,
            &[("NAME", SqlValueKind::String)],
            &[&["SMITH"]],
        );
        assert_eq!(
            build_sql_updates(&selection, &["ID".to_string()]),
            "UPDATE HR.EMP SET NAME = 'SMITH';\n"
        );
    }

    #[test]
    fn updates_assign_key_columns_when_nothing_else_is_selected() {
        let selection = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number)],
            &[&["7369"]],
        );
        assert_eq!(
            build_sql_updates(&selection, &["ID".to_string()]),
            "UPDATE HR.EMP SET ID = 7369 WHERE ID = 7369;\n"
        );
    }

    #[test]
    fn updates_compare_a_null_key_with_is_null() {
        let selection = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number), ("NAME", SqlValueKind::String)],
            &[&["NULL", "SMITH"]],
        );
        assert_eq!(
            build_sql_updates(&selection, &["ID".to_string()]),
            "UPDATE HR.EMP SET NAME = 'SMITH' WHERE ID IS NULL;\n"
        );
    }

    #[test]
    fn where_clause_of_one_cell_uses_equality() {
        let selection = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number)],
            &[&["7369"]],
        );
        assert_eq!(build_where_clause(&selection), "ID = 7369");
    }

    #[test]
    fn where_clause_of_one_column_collapses_into_in() {
        let selection = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number)],
            &[&["7369"], &["7499"], &["7521"]],
        );
        assert_eq!(build_where_clause(&selection), "ID IN (7369, 7499, 7521)");
    }

    #[test]
    fn where_clause_of_one_column_deduplicates_values() {
        let selection = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number)],
            &[&["7369"], &["7369"], &["7499"]],
        );
        assert_eq!(build_where_clause(&selection), "ID IN (7369, 7499)");
    }

    #[test]
    fn where_clause_lifts_nulls_out_of_the_in_list() {
        let selection = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number)],
            &[&["7369"], &["NULL"], &["7499"]],
        );
        assert_eq!(
            build_where_clause(&selection),
            "ID IN (7369, 7499) OR ID IS NULL"
        );
    }

    #[test]
    fn where_clause_of_only_nulls_is_an_is_null_test() {
        let selection = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number)],
            &[&["NULL"]],
        );
        assert_eq!(build_where_clause(&selection), "ID IS NULL");
    }

    #[test]
    fn where_clause_of_one_row_ands_the_columns() {
        let selection = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number), ("NAME", SqlValueKind::String)],
            &[&["7369", "SMITH"]],
        );
        assert_eq!(
            build_where_clause(&selection),
            "ID = 7369 AND NAME = 'SMITH'"
        );
    }

    #[test]
    fn where_clause_of_many_rows_ors_parenthesized_groups() {
        let selection = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number), ("NAME", SqlValueKind::String)],
            &[&["7369", "SMITH"], &["7499", "ALLEN"]],
        );
        assert_eq!(
            build_where_clause(&selection),
            "(ID = 7369 AND NAME = 'SMITH') OR (ID = 7499 AND NAME = 'ALLEN')"
        );
    }

    #[test]
    fn where_clause_deduplicates_identical_rows() {
        let selection = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number), ("NAME", SqlValueKind::String)],
            &[&["7369", "SMITH"], &["7369", "SMITH"]],
        );
        assert_eq!(
            build_where_clause(&selection),
            "ID = 7369 AND NAME = 'SMITH'"
        );
    }

    #[test]
    fn missing_kinds_quote_every_column() {
        // What the grid stores when the producer had no driver metadata, or when
        // the kinds went out of step with the headers.
        let mut selection = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number), ("NAME", SqlValueKind::String)],
            &[&["7369", "SMITH"]],
        );
        selection.column_kinds.clear();
        assert_eq!(
            build_sql_inserts(&selection),
            "INSERT INTO HR.EMP (ID, NAME) VALUES ('7369', 'SMITH');\n"
        );
    }

    #[test]
    fn key_columns_match_case_insensitively() {
        let selection = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number), ("NAME", SqlValueKind::String)],
            &[&["7369", "SMITH"]],
        );
        assert_eq!(
            build_sql_updates(&selection, &["id".to_string()]),
            "UPDATE HR.EMP SET NAME = 'SMITH' WHERE ID = 7369;\n"
        );
    }

    #[test]
    fn quoted_column_names_survive_generation() {
        let selection = selection(
            DatabaseType::Oracle,
            &[("odd name", SqlValueKind::String)],
            &[&["x"]],
        );
        assert_eq!(
            build_sql_inserts(&selection),
            "INSERT INTO HR.EMP (\"odd name\") VALUES ('x');\n"
        );
    }
}
