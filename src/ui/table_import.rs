//! Turn a parsed file into the `INSERT` script that loads it into a table.
//!
//! Pure functions over [`crate::ui::result_import::ImportedTable`] and the
//! target table's own column metadata — no FLTK, no database — so the exact SQL
//! that will run is unit-testable before anything touches a server.
//!
//! Two decisions shape everything here:
//!
//! - **The target column's type decides the literal, never the value's shape.**
//!   The file carries text; [`crate::db::SqlValueKind`], derived from the
//!   column's declared type, says whether that text is quoted, converted, or
//!   emitted bare. It is the same rule `SQL Inserts` export follows, and it is
//!   what keeps a zero-padded code out of a number and a `VARCHAR2` holding
//!   `2024-01-01` out of `TO_DATE`.
//! - **Rows are batched into few statements.** MySQL takes a multi-row
//!   `VALUES` list, Oracle takes `INSERT ALL`. A thousand-row file becomes a
//!   handful of statements instead of a thousand round trips.
//!
//! The script is ordinary SQL text: it runs through the same executor, the same
//! transaction, and the same auto-commit setting as anything typed in the
//! editor, so an import commits exactly when the user's session says it does.

use crate::db::{DatabaseType, SqlValueKind};
use crate::ui::grid_sql_export::{quote_column_name, quote_qualified_name, sql_literal_for_value};
use crate::ui::result_import::{ImportCell, ImportedTable};

/// How many rows go into one statement. Large enough that a big file is a few
/// statements, small enough to stay well inside every backend's limit on
/// expressions per statement and to keep one failing statement readable.
pub const DEFAULT_BATCH_ROWS: usize = 100;

/// A column of the table being loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetColumn {
    pub name: String,
    pub kind: SqlValueKind,
    pub nullable: bool,
}

/// Which target column each source column feeds. `None` skips the source
/// column. Indexes point into the target column list.
pub type ColumnMapping = Vec<Option<usize>>;

/// Everything the script needs, already resolved.
pub struct ImportRequest<'a> {
    pub db_type: DatabaseType,
    /// The table to load, qualified but not yet quoted.
    pub table: &'a str,
    pub targets: &'a [TargetColumn],
    pub mapping: &'a ColumnMapping,
    pub data: &'a ImportedTable,
    pub batch_rows: usize,
}

/// Classify a declared column type into the literal kind it accepts.
///
/// The input is what the backend's own catalog reports: Oracle's
/// `ALL_TAB_COLUMNS.DATA_TYPE` (`VARCHAR2`, `TIMESTAMP(6) WITH TIME ZONE`) or
/// MySQL's `INFORMATION_SCHEMA.COLUMNS.COLUMN_TYPE` (`varchar(50)`,
/// `int unsigned`, `enum('a','b')`). Only the leading type word matters.
pub fn column_kind_for_data_type(db_type: DatabaseType, data_type: &str) -> SqlValueKind {
    let upper = data_type.trim().to_ascii_uppercase();
    let head: &str = upper.split(['(', ' ']).next().unwrap_or_default();

    if db_type.is_mysql_or_mariadb() {
        return match head {
            "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "INTEGER" | "BIGINT" | "DECIMAL"
            | "DEC" | "NUMERIC" | "FIXED" | "FLOAT" | "DOUBLE" | "REAL" | "BIT" | "YEAR" => {
                SqlValueKind::Number
            }
            "BOOL" | "BOOLEAN" => SqlValueKind::Boolean,
            "DATE" | "DATETIME" | "TIMESTAMP" | "TIME" => SqlValueKind::Temporal,
            "BINARY" | "VARBINARY" | "TINYBLOB" | "BLOB" | "MEDIUMBLOB" | "LONGBLOB" => {
                SqlValueKind::Binary
            }
            "CHAR" | "VARCHAR" | "TINYTEXT" | "TEXT" | "MEDIUMTEXT" | "LONGTEXT" | "ENUM"
            | "SET" | "JSON" => SqlValueKind::String,
            _ => SqlValueKind::Unknown,
        };
    }

    match head {
        "NUMBER" | "FLOAT" | "BINARY_FLOAT" | "BINARY_DOUBLE" | "INTEGER" | "INT" | "SMALLINT"
        | "DECIMAL" | "NUMERIC" | "DEC" | "DOUBLE" | "REAL" => SqlValueKind::Number,
        "BOOLEAN" => SqlValueKind::Boolean,
        // `INTERVAL DAY(2) TO SECOND(6)` and the TIMESTAMP family all start
        // with a word this catches.
        "DATE" | "TIMESTAMP" | "INTERVAL" => SqlValueKind::Temporal,
        "RAW" | "BLOB" | "BFILE" => SqlValueKind::Binary,
        // `LONG` is text, `LONG RAW` is not.
        "LONG" if upper.starts_with("LONG RAW") => SqlValueKind::Binary,
        "VARCHAR2" | "NVARCHAR2" | "VARCHAR" | "CHAR" | "NCHAR" | "CLOB" | "NCLOB" | "LONG"
        | "ROWID" | "UROWID" | "XMLTYPE" | "JSON" => SqlValueKind::String,
        _ => SqlValueKind::Unknown,
    }
}

/// Match every source column to a target column of the same name, ignoring
/// case and surrounding whitespace. A source column with no match is skipped,
/// and a target already claimed by an earlier source column is not reused.
pub fn default_mapping(source_columns: &[String], targets: &[TargetColumn]) -> ColumnMapping {
    let mut taken = vec![false; targets.len()];
    source_columns
        .iter()
        .map(|source| {
            let found = targets
                .iter()
                .position(|target| target.name.trim().eq_ignore_ascii_case(source.trim()));
            match found {
                Some(index) if !taken[index] => {
                    taken[index] = true;
                    Some(index)
                }
                _ => None,
            }
        })
        .collect()
}

/// Everything wrong with a mapping, as a message, or `Ok` with the source
/// column indexes that will be written, in target order.
fn resolve_mapping(request: &ImportRequest) -> Result<Vec<(usize, usize)>, String> {
    if request.data.rows.is_empty() {
        return Err("The file has no rows to import.".to_string());
    }
    if request.mapping.len() != request.data.columns.len() {
        return Err("The column mapping does not match the file.".to_string());
    }

    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for (source, target) in request.mapping.iter().enumerate() {
        let Some(target) = *target else { continue };
        if target >= request.targets.len() {
            return Err(
                "The column mapping points at a column the table does not have.".to_string(),
            );
        }
        if pairs.iter().any(|(_, taken)| *taken == target) {
            return Err(format!(
                "Two file columns are mapped to {}.",
                request.targets[target].name
            ));
        }
        pairs.push((source, target));
    }
    if pairs.is_empty() {
        return Err("Map at least one file column to a table column.".to_string());
    }
    pairs.sort_by_key(|(_, target)| *target);
    Ok(pairs)
}

/// Build the script that loads `data` into `table`.
pub fn build_insert_script(request: &ImportRequest) -> Result<String, String> {
    let pairs = resolve_mapping(request)?;
    let table = quote_qualified_name(request.db_type, request.table);
    let column_list = pairs
        .iter()
        .map(|(_, target)| quote_column_name(request.db_type, &request.targets[*target].name))
        .collect::<Vec<_>>()
        .join(", ");

    let mut rows: Vec<String> = Vec::with_capacity(request.data.rows.len());
    for row in &request.data.rows {
        let values = pairs
            .iter()
            .map(|(source, target)| {
                cell_literal(
                    request.db_type,
                    request.targets[*target].kind,
                    row.get(*source).unwrap_or(&None),
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        rows.push(values);
    }

    let batch = request.batch_rows.max(1);
    let mut script = String::new();
    for chunk in rows.chunks(batch) {
        if !script.is_empty() {
            script.push('\n');
        }
        script.push_str(&if request.db_type.is_mysql_or_mariadb() {
            mysql_batch(&table, &column_list, chunk)
        } else {
            oracle_batch(&table, &column_list, chunk)
        });
        script.push('\n');
    }
    Ok(script)
}

/// `INSERT INTO t (a, b) VALUES (…), (…);` — one statement, many rows.
fn mysql_batch(table: &str, column_list: &str, rows: &[String]) -> String {
    let values = rows
        .iter()
        .map(|row| format!("  ({row})"))
        .collect::<Vec<_>>()
        .join(",\n");
    format!("INSERT INTO {table} ({column_list}) VALUES\n{values};")
}

/// `INSERT ALL INTO t (a, b) VALUES (…) … SELECT * FROM DUAL;` — Oracle's
/// multi-row insert. The trailing query is required by the syntax and supplies
/// exactly one driving row.
fn oracle_batch(table: &str, column_list: &str, rows: &[String]) -> String {
    let inserts = rows
        .iter()
        .map(|row| format!("  INTO {table} ({column_list}) VALUES ({row})"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("INSERT ALL\n{inserts}\nSELECT * FROM DUAL;")
}

/// One cell as a SQL literal.
///
/// NULL comes from the file saying so, never from the text looking empty:
/// [`sql_literal_for_value`] is the entry point that does not second-guess an
/// empty string.
fn cell_literal(db_type: DatabaseType, kind: SqlValueKind, cell: &ImportCell) -> String {
    match cell {
        None => "NULL".to_string(),
        Some(value) => defuse_substitution(db_type, &sql_literal_for_value(db_type, kind, value)),
    }
}

/// Keep an `&` that came out of a file from being read as a substitution
/// variable.
///
/// Oracle's client-side `DEFINE` is on by default and substitutes `&name`
/// *inside* string literals too, the way SQL*Plus does — so importing a row
/// holding `AT&T` would stop and ask the user to "Enter value for T". A value
/// that came from a file is data, never a variable, so every `&` is lifted out
/// of the literal as `CHR(38)`. The stored text is identical, and the session's
/// `DEFINE` setting is left exactly as the user set it.
///
/// Only a plain string literal is rewritten. A number, a `TO_DATE(…)`, or a
/// `HEXTORAW(…)` cannot contain an `&`, and MySQL and MariaDB have no
/// substitution at all.
pub fn defuse_substitution(db_type: DatabaseType, literal: &str) -> String {
    if db_type.is_mysql_or_mariadb() || !literal.contains('&') {
        return literal.to_string();
    }
    let Some(inner) = literal
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    else {
        return literal.to_string();
    };
    let mut parts: Vec<String> = Vec::new();
    for (index, piece) in inner.split('&').enumerate() {
        if index > 0 {
            parts.push("CHR(38)".to_string());
        }
        if !piece.is_empty() {
            parts.push(format!("'{piece}'"));
        }
    }
    parts.join("||")
}

/// A one-line summary of what an import will do, for the dialog and the status
/// line.
pub fn describe(request: &ImportRequest) -> Result<String, String> {
    let pairs = resolve_mapping(request)?;
    let rows = request.data.rows.len();
    let skipped = request
        .mapping
        .iter()
        .filter(|target| target.is_none())
        .count();
    let mut summary = format!(
        "{rows} row(s) into {} column(s) of {}",
        pairs.len(),
        request.table
    );
    if skipped > 0 {
        summary.push_str(&format!(", {skipped} file column(s) skipped"));
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::result_export::ExportFormat;
    use crate::ui::result_import::{parse, ImportOptions};

    fn target(name: &str, kind: SqlValueKind) -> TargetColumn {
        TargetColumn {
            name: name.to_string(),
            kind,
            nullable: true,
        }
    }

    fn targets() -> Vec<TargetColumn> {
        vec![
            target("ID", SqlValueKind::Number),
            target("NAME", SqlValueKind::String),
            target("CODE", SqlValueKind::String),
        ]
    }

    fn data() -> ImportedTable {
        ImportedTable {
            columns: vec!["ID".to_string(), "NAME".to_string(), "CODE".to_string()],
            rows: vec![
                vec![
                    Some("1".to_string()),
                    Some("it's".to_string()),
                    Some("00123".to_string()),
                ],
                vec![Some("2".to_string()), None, Some(String::new())],
            ],
        }
    }

    fn script(db_type: DatabaseType, batch: usize) -> String {
        let targets = targets();
        let data = data();
        let mapping = default_mapping(&data.columns, &targets);
        build_insert_script(&ImportRequest {
            db_type,
            table: "APP.T",
            targets: &targets,
            mapping: &mapping,
            data: &data,
            batch_rows: batch,
        })
        .expect("builds")
    }

    #[test]
    fn oracle_batches_rows_into_one_insert_all() {
        assert_eq!(
            script(DatabaseType::Oracle, 100),
            "INSERT ALL\n\
             \x20 INTO APP.T (ID, NAME, CODE) VALUES (1, 'it''s', '00123')\n\
             \x20 INTO APP.T (ID, NAME, CODE) VALUES (2, NULL, '')\n\
             SELECT * FROM DUAL;\n"
        );
    }

    #[test]
    fn mysql_batches_rows_into_one_multi_row_values_list() {
        assert_eq!(
            script(DatabaseType::MySQL, 100),
            "INSERT INTO `APP`.`T` (`ID`, `NAME`, `CODE`) VALUES\n\
             \x20 (1, 'it''s', '00123'),\n\
             \x20 (2, NULL, '');\n"
        );
    }

    #[test]
    fn mariadb_uses_the_same_dialect_as_mysql() {
        assert_eq!(
            script(DatabaseType::MariaDB, 100),
            script(DatabaseType::MySQL, 100)
        );
    }

    #[test]
    fn a_batch_size_splits_the_script_into_whole_statements() {
        let oracle = script(DatabaseType::Oracle, 1);
        assert_eq!(oracle.matches("INSERT ALL").count(), 2);
        assert_eq!(oracle.matches("SELECT * FROM DUAL;").count(), 2);
        let mysql = script(DatabaseType::MySQL, 1);
        assert_eq!(mysql.matches("INSERT INTO").count(), 2);
    }

    #[test]
    fn a_batch_size_of_zero_still_produces_valid_statements() {
        assert_eq!(
            script(DatabaseType::Oracle, 0),
            script(DatabaseType::Oracle, 1)
        );
    }

    #[test]
    fn an_empty_string_stays_an_empty_string_and_only_null_is_null() {
        // The grid reads an empty cell as NULL; a file that said "empty string"
        // must not be rewritten into one.
        let oracle = script(DatabaseType::Oracle, 100);
        assert!(oracle.contains("(2, NULL, '')"), "{oracle}");
    }

    #[test]
    fn the_text_null_from_a_file_is_a_string_not_a_null() {
        let targets = vec![target("NAME", SqlValueKind::String)];
        let data = ImportedTable {
            columns: vec!["NAME".to_string()],
            rows: vec![vec![Some("NULL".to_string())]],
        };
        let mapping = default_mapping(&data.columns, &targets);
        let script = build_insert_script(&ImportRequest {
            db_type: DatabaseType::Oracle,
            table: "T",
            targets: &targets,
            mapping: &mapping,
            data: &data,
            batch_rows: 100,
        })
        .expect("builds");
        assert!(script.contains("VALUES ('NULL')"), "{script}");
    }

    #[test]
    fn the_target_type_decides_the_literal_not_the_value() {
        // The same text goes into a number column bare and into a string
        // column quoted, and a date-shaped string stays a string.
        let targets = vec![
            target("N", SqlValueKind::Number),
            target("S", SqlValueKind::String),
            target("D", SqlValueKind::Temporal),
        ];
        let data = ImportedTable {
            columns: vec!["N".to_string(), "S".to_string(), "D".to_string()],
            rows: vec![vec![
                Some("00123".to_string()),
                Some("2024-01-01".to_string()),
                Some("2024-01-01".to_string()),
            ]],
        };
        let mapping = default_mapping(&data.columns, &targets);
        let script = build_insert_script(&ImportRequest {
            db_type: DatabaseType::Oracle,
            table: "T",
            targets: &targets,
            mapping: &mapping,
            data: &data,
            batch_rows: 100,
        })
        .expect("builds");
        assert!(
            script.contains("VALUES (00123, '2024-01-01', TO_DATE('2024-01-01','YYYY-MM-DD'))"),
            "{script}"
        );
    }

    #[test]
    fn default_mapping_matches_names_case_insensitively() {
        let targets = targets();
        let source = vec![
            "  code ".to_string(),
            "Id".to_string(),
            "UNRELATED".to_string(),
        ];
        assert_eq!(
            default_mapping(&source, &targets),
            vec![Some(2), Some(0), None]
        );
    }

    #[test]
    fn default_mapping_never_claims_one_target_twice() {
        let targets = targets();
        let source = vec!["ID".to_string(), "id".to_string()];
        assert_eq!(default_mapping(&source, &targets), vec![Some(0), None]);
    }

    #[test]
    fn columns_come_out_in_table_order_whatever_order_the_file_used() {
        let targets = targets();
        let data = ImportedTable {
            columns: vec!["CODE".to_string(), "ID".to_string()],
            rows: vec![vec![Some("x".to_string()), Some("1".to_string())]],
        };
        let mapping = default_mapping(&data.columns, &targets);
        let script = build_insert_script(&ImportRequest {
            db_type: DatabaseType::Oracle,
            table: "T",
            targets: &targets,
            mapping: &mapping,
            data: &data,
            batch_rows: 100,
        })
        .expect("builds");
        assert!(
            script.contains("INTO T (ID, CODE) VALUES (1, 'x')"),
            "{script}"
        );
    }

    #[test]
    fn a_mapping_with_nothing_in_it_is_refused() {
        let targets = targets();
        let data = data();
        let error = build_insert_script(&ImportRequest {
            db_type: DatabaseType::Oracle,
            table: "T",
            targets: &targets,
            mapping: &vec![None, None, None],
            data: &data,
            batch_rows: 100,
        })
        .expect_err("refused");
        assert!(error.contains("at least one"), "{error}");
    }

    #[test]
    fn two_file_columns_on_one_target_are_refused() {
        let targets = targets();
        let data = data();
        let error = build_insert_script(&ImportRequest {
            db_type: DatabaseType::Oracle,
            table: "T",
            targets: &targets,
            mapping: &vec![Some(0), Some(0), None],
            data: &data,
            batch_rows: 100,
        })
        .expect_err("refused");
        assert!(error.contains("Two file columns"), "{error}");
    }

    #[test]
    fn a_file_with_no_rows_is_refused() {
        let targets = targets();
        let data = ImportedTable {
            columns: vec!["ID".to_string()],
            rows: Vec::new(),
        };
        assert!(build_insert_script(&ImportRequest {
            db_type: DatabaseType::Oracle,
            table: "T",
            targets: &targets,
            mapping: &vec![Some(0)],
            data: &data,
            batch_rows: 100,
        })
        .is_err());
    }

    #[test]
    fn a_short_row_writes_null_for_the_missing_values() {
        let targets = targets();
        let data = ImportedTable {
            columns: vec!["ID".to_string(), "NAME".to_string(), "CODE".to_string()],
            rows: vec![vec![Some("1".to_string())]],
        };
        let mapping = default_mapping(&data.columns, &targets);
        let script = build_insert_script(&ImportRequest {
            db_type: DatabaseType::Oracle,
            table: "T",
            targets: &targets,
            mapping: &mapping,
            data: &data,
            batch_rows: 100,
        })
        .expect("builds");
        assert!(script.contains("VALUES (1, NULL, NULL)"), "{script}");
    }

    #[test]
    fn a_name_that_cannot_stand_unquoted_is_quoted() {
        let targets = vec![target("MY COL", SqlValueKind::Number)];
        let data = ImportedTable {
            columns: vec!["MY COL".to_string()],
            rows: vec![vec![Some("1".to_string())]],
        };
        let mapping = default_mapping(&data.columns, &targets);
        for (db_type, expected) in [
            (DatabaseType::Oracle, "INTO APP.\"MY TABLE\" (\"MY COL\")"),
            (
                DatabaseType::MySQL,
                "INSERT INTO `app`.`my table` (`MY COL`)",
            ),
        ] {
            let script = build_insert_script(&ImportRequest {
                db_type,
                table: if db_type.is_mysql_or_mariadb() {
                    "app.my table"
                } else {
                    "APP.MY TABLE"
                },
                targets: &targets,
                mapping: &mapping,
                data: &data,
                batch_rows: 100,
            })
            .expect("builds");
            assert!(script.contains(expected), "{script}");
        }
    }

    #[test]
    fn an_ampersand_never_becomes_an_oracle_substitution_prompt() {
        for (literal, expected) in [
            ("'AT&T'", "'AT'||CHR(38)||'T'"),
            ("'&start'", "CHR(38)||'start'"),
            ("'end&'", "'end'||CHR(38)"),
            ("'&'", "CHR(38)"),
            // `&&x` is SQL*Plus's other substitution form, so both have to go.
            ("'a&&x'", "'a'||CHR(38)||CHR(38)||'x'"),
            ("'&&'", "CHR(38)||CHR(38)"),
            // Nothing else is touched.
            ("'plain'", "'plain'"),
            ("'a|b'", "'a|b'"),
            ("1234", "1234"),
            ("NULL", "NULL"),
            (
                "TO_DATE('2024-01-01','YYYY-MM-DD')",
                "TO_DATE('2024-01-01','YYYY-MM-DD')",
            ),
        ] {
            assert_eq!(
                defuse_substitution(DatabaseType::Oracle, literal),
                expected,
                "{literal}"
            );
        }
    }

    #[test]
    fn the_mysql_family_has_no_substitution_to_defuse() {
        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            assert_eq!(defuse_substitution(db_type, "'AT&T'"), "'AT&T'");
        }
    }

    #[test]
    fn an_ampersand_in_a_value_reaches_the_script_as_chr_38() {
        let targets = vec![target("NAME", SqlValueKind::String)];
        let data = ImportedTable {
            columns: vec!["NAME".to_string()],
            rows: vec![vec![Some("R&D".to_string())]],
        };
        let mapping = default_mapping(&data.columns, &targets);
        let oracle = build_insert_script(&ImportRequest {
            db_type: DatabaseType::Oracle,
            table: "T",
            targets: &targets,
            mapping: &mapping,
            data: &data,
            batch_rows: 100,
        })
        .expect("builds");
        assert!(oracle.contains("VALUES ('R'||CHR(38)||'D')"), "{oracle}");
        let mysql = build_insert_script(&ImportRequest {
            db_type: DatabaseType::MySQL,
            table: "T",
            targets: &targets,
            mapping: &mapping,
            data: &data,
            batch_rows: 100,
        })
        .expect("builds");
        assert!(mysql.contains("('R&D')"), "{mysql}");
    }

    #[test]
    fn oracle_types_classify_the_way_the_driver_would() {
        for (data_type, kind) in [
            ("VARCHAR2", SqlValueKind::String),
            ("NVARCHAR2", SqlValueKind::String),
            ("CHAR", SqlValueKind::String),
            ("CLOB", SqlValueKind::String),
            ("LONG", SqlValueKind::String),
            ("NUMBER", SqlValueKind::Number),
            ("BINARY_DOUBLE", SqlValueKind::Number),
            ("DATE", SqlValueKind::Temporal),
            ("TIMESTAMP(6)", SqlValueKind::Temporal),
            ("TIMESTAMP(6) WITH TIME ZONE", SqlValueKind::Temporal),
            ("INTERVAL DAY(2) TO SECOND(6)", SqlValueKind::Temporal),
            ("RAW", SqlValueKind::Binary),
            ("LONG RAW", SqlValueKind::Binary),
            ("BLOB", SqlValueKind::Binary),
            ("BOOLEAN", SqlValueKind::Boolean),
            ("SDO_GEOMETRY", SqlValueKind::Unknown),
        ] {
            assert_eq!(
                column_kind_for_data_type(DatabaseType::Oracle, data_type),
                kind,
                "{data_type}"
            );
        }
    }

    #[test]
    fn mysql_types_classify_the_way_the_driver_would() {
        for (data_type, kind) in [
            ("varchar(50)", SqlValueKind::String),
            ("longtext", SqlValueKind::String),
            ("enum('a','b')", SqlValueKind::String),
            ("json", SqlValueKind::String),
            ("int unsigned", SqlValueKind::Number),
            ("decimal(12,2)", SqlValueKind::Number),
            ("bigint(20)", SqlValueKind::Number),
            ("bit(1)", SqlValueKind::Number),
            ("year(4)", SqlValueKind::Number),
            ("date", SqlValueKind::Temporal),
            ("datetime(6)", SqlValueKind::Temporal),
            ("time(3)", SqlValueKind::Temporal),
            ("varbinary(16)", SqlValueKind::Binary),
            ("longblob", SqlValueKind::Binary),
            ("geometry", SqlValueKind::Unknown),
        ] {
            for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
                assert_eq!(
                    column_kind_for_data_type(db_type, data_type),
                    kind,
                    "{data_type}"
                );
            }
        }
    }

    #[test]
    fn a_csv_file_becomes_a_script_end_to_end() {
        let data = parse(
            "ID,NAME\n1,alpha\n2,NULL\n",
            &ImportOptions {
                format: ExportFormat::Csv,
                ..ImportOptions::default()
            },
        )
        .expect("parses");
        let targets = vec![
            target("ID", SqlValueKind::Number),
            target("NAME", SqlValueKind::String),
        ];
        let mapping = default_mapping(&data.columns, &targets);
        let request = ImportRequest {
            db_type: DatabaseType::MySQL,
            table: "app.people",
            targets: &targets,
            mapping: &mapping,
            data: &data,
            batch_rows: DEFAULT_BATCH_ROWS,
        };
        assert_eq!(
            build_insert_script(&request).expect("builds"),
            "INSERT INTO `app`.`people` (`ID`, `NAME`) VALUES\n  (1, 'alpha'),\n  (2, NULL);\n"
        );
        assert_eq!(
            describe(&request).expect("describes"),
            "2 row(s) into 2 column(s) of app.people"
        );
    }

    #[test]
    fn describe_counts_the_columns_that_will_not_be_written() {
        let targets = targets();
        let data = ImportedTable {
            columns: vec!["ID".to_string(), "UNRELATED".to_string()],
            rows: vec![vec![Some("1".to_string()), Some("x".to_string())]],
        };
        let mapping = default_mapping(&data.columns, &targets);
        assert_eq!(
            describe(&ImportRequest {
                db_type: DatabaseType::Oracle,
                table: "T",
                targets: &targets,
                mapping: &mapping,
                data: &data,
                batch_rows: 100,
            })
            .expect("describes"),
            "1 row(s) into 1 column(s) of T, 1 file column(s) skipped"
        );
    }
}
