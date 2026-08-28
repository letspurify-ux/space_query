//! Turn a parsed file into the `INSERT` script that loads it into a table.
//!
//! Pure functions over [`crate::ui::result_import::ImportedTable`] and the
//! target table's own column metadata — no FLTK, no database — so the exact SQL
//! that will run is unit-testable before anything touches a server.
//!
//! Two decisions shape everything here:
//!
//! - **The target column's type decides the literal, never the value's shape.**
//!   The file carries text; [`crate::db::SqlValueKind::for_declared_type`],
//!   read off the column's declared type in the catalog, says whether that text
//!   is quoted, converted, or emitted bare. It is the same rule `SQL Inserts`
//!   export follows — literally the same, because the kind an export reaches
//!   through the DRIVER and the kind an import reaches through the CATALOG are
//!   held to writing the same literal — and it is what keeps a zero-padded code
//!   out of a number and a `VARCHAR2` holding `2024-01-01` out of `TO_DATE`.
//! - **Rows are batched into few statements.** MySQL takes a multi-row
//!   `VALUES` list, Oracle takes `INSERT ALL`. A thousand-row file becomes a
//!   handful of statements instead of a thousand round trips.
//!
//! The script is ordinary SQL text: it runs through the same executor, the same
//! transaction, and the same auto-commit setting as anything typed in the
//! editor, so an import commits exactly when the user's session says it does.

use crate::db::SqlValueKind;
use crate::ui::grid_sql_export::{
    quote_column_name, quote_qualified_name, sql_literal_for_value, SqlWriteDialect,
};
use crate::ui::result_import::{ImportCell, ImportedTable};

/// Re-exported so the one place that turns a value into Oracle literal text
/// stays reachable by name from the import side that first needed it.
pub use crate::ui::grid_sql_export::defuse_substitution;

/// How many rows go into one statement. Large enough that a big file is a few
/// statements, small enough to stay well inside every backend's limit on
/// expressions per statement and to keep one failing statement readable.
pub const DEFAULT_BATCH_ROWS: usize = 100;

/// How much SQL TEXT one statement may carry.
///
/// A row count cannot bound this, and the row count is all there used to be:
/// what a server refuses is a PACKET, and 100 rows of a document column is
/// twenty megabytes of it. Measured — MariaDB's default `max_allowed_packet`
/// is 16 MiB and it answered `Packet is larger than max_allowed_packet`, losing
/// the whole import rather than a row of it.
///
/// Four mebibytes: a quarter of that default, an sixteenth of MySQL 8's, and
/// far above anything an ordinary row reaches, so a normal import still batches
/// [`DEFAULT_BATCH_ROWS`] rows into one statement exactly as before. It is a
/// fixed figure rather than one read from the server because nothing else this
/// writer knows comes from a live session, and a conservative bound is worth
/// more than a round trip per import.
pub const MAX_BATCH_BYTES: usize = 4 * 1024 * 1024;

/// A column of the table being loaded.
///
/// Only columns a value can actually be written into are ever built into one:
/// [`crate::ui::object_browser::ObjectBrowserWidget::import_target_columns`]
/// leaves out the generated and always-identity columns the catalog reports, so
/// a mapping that the server would reject cannot be expressed here at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetColumn {
    pub name: String,
    pub kind: SqlValueKind,
    pub nullable: bool,
}

/// Which target column each source column feeds. `None` skips the source
/// column. Indexes point into the target column list.
pub type ColumnMapping = Vec<Option<usize>>;

/// One column of the table being loaded, and the two facts that decide what it
/// means to a file.
///
/// They are genuinely different questions and a column can answer them in any
/// combination: a generated column IS in every export and may not be written
/// into; an invisible column may be written into and is in no export.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ImportColumn {
    name: String,
    kind: SqlValueKind,
    nullable: bool,
    /// Whether a statement may supply a value for it. An Oracle virtual column
    /// or `GENERATED ALWAYS AS IDENTITY`, or a MySQL-family generated column,
    /// says no — and the server refuses the whole script for naming one.
    writable: bool,
    /// Whether a `SELECT *` file carries a value for it. An INVISIBLE column
    /// does not appear in one, so it claims no POSITION — measured on MySQL 8.0
    /// and MariaDB 12.2, where it keeps its `ORDINAL_POSITION` in the catalog
    /// and therefore sat in the middle of this list, shifting every later value
    /// one column left.
    in_select_star: bool,
}

/// The table's columns as an import sees them: EVERY column, in the table's own
/// order, each saying whether a value may be written into it.
///
/// One value, rather than a list of writable columns beside a list of the names
/// left out. A file with a header is mapped by NAME and needs only the first;
/// a file WITHOUT one is mapped by POSITION, and a position is a place in the
/// TABLE. Given only the writable list, the dialog answered that question with
/// it — file column *i* fed the *i*-th WRITABLE column — so a table with a
/// generated column in the middle shifted every later value one column left.
/// Silently, and for exactly the file this app's own CSV export writes, which
/// carries every column the table has.
///
/// Built from the catalog in one place so "what may be written into" and "what
/// position means" cannot drift apart.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportTargets {
    columns: Vec<ImportColumn>,
}

impl ImportTargets {
    /// Read the table's columns as the import builder needs them: the declared
    /// type resolved to the literal kind it accepts.
    pub fn from_catalog(
        db_type: crate::db::DatabaseType,
        columns: &[crate::db::TableColumnDetail],
    ) -> Self {
        Self {
            columns: columns
                .iter()
                .map(|column| ImportColumn {
                    name: column.name.clone(),
                    kind: SqlValueKind::for_declared_type(db_type, &column.data_type),
                    nullable: column.nullable,
                    writable: !column.is_generated,
                    in_select_star: !column.is_invisible,
                })
                .collect(),
        }
    }

    /// A table whose every column may be written into.
    ///
    /// For callers with no catalog in reach: the capture tour's sample table
    /// and the unit tests. A real table goes through [`Self::from_catalog`],
    /// which is the only thing that can know a column is computed — so this
    /// cannot be used to claim that one is writable when it is not.
    pub fn all_writable(targets: Vec<TargetColumn>) -> Self {
        Self {
            columns: targets
                .into_iter()
                .map(|target| ImportColumn {
                    name: target.name,
                    kind: target.kind,
                    nullable: target.nullable,
                    writable: true,
                    in_select_star: true,
                })
                .collect(),
        }
    }

    /// The columns a mapping may point at, in table order.
    ///
    /// A column the server computes is left out rather than offered and then
    /// refused: `default_mapping` matches by name, so it would map itself and
    /// the server would reject the whole script (Oracle ORA-54013, MySQL 3105).
    /// The export side of the same rule lives in
    /// [`crate::ui::grid_sql_export::GridSqlSelection::restrict_to_writable_columns`]
    /// — what an import will not offer is what an export must not name.
    pub fn writable(&self) -> Vec<TargetColumn> {
        self.columns
            .iter()
            .filter(|column| column.writable)
            .map(|column| TargetColumn {
                name: column.name.clone(),
                kind: column.kind,
                nullable: column.nullable,
            })
            .collect()
    }

    /// The columns [`Self::writable`] left out, so a dialog can say so instead
    /// of leaving the user to wonder where a column went.
    pub fn generated_names(&self) -> Vec<String> {
        self.columns
            .iter()
            .filter(|column| !column.writable)
            .map(|column| column.name.clone())
            .collect()
    }

    /// The mapping a file with no header means: its columns are the columns a
    /// `SELECT *` of this table returns, in that order.
    ///
    /// Two different skips, because the two facts are different:
    ///
    /// * a GENERATED column is in the file (every data-format export of this
    ///   table writes it) and may take no value, so it consumes a file position
    ///   and maps to nothing. Letting the next value slide over it would put
    ///   that value in the wrong column;
    /// * an INVISIBLE column is NOT in the file, so it consumes no position at
    ///   all — while still keeping its place among the targets, which a
    ///   statement may name.
    ///
    /// Indexes are into [`Self::writable`], which is what a
    /// [`ColumnMapping`] points at.
    pub fn positional_mapping(&self, source_count: usize) -> ColumnMapping {
        let mut writable_index = 0usize;
        let mut mapping: ColumnMapping = Vec::with_capacity(source_count);
        for column in &self.columns {
            // Counted for EVERY writable column, in the order `writable()`
            // builds them — the index has to point at the same entry whether
            // or not a file position ever reaches it.
            let target = column.writable.then(|| {
                let index = writable_index;
                writable_index += 1;
                index
            });
            if !column.in_select_star {
                continue;
            }
            if mapping.len() == source_count {
                break;
            }
            mapping.push(target);
        }
        // A file wider than the table has nothing left to feed.
        mapping.resize(source_count, None);
        mapping
    }
}

/// Everything the script needs, already resolved.
pub struct ImportRequest<'a> {
    /// How SQL text must be written for the connection this will run on.
    pub dialect: SqlWriteDialect,
    /// The table to load, qualified but not yet quoted.
    pub table: &'a str,
    pub targets: &'a [TargetColumn],
    pub mapping: &'a ColumnMapping,
    pub data: &'a ImportedTable,
    pub batch_rows: usize,
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
///
/// The ONE validator. The dialog asks it while it is still open, so a mapping
/// it refuses can be corrected in place, and [`build_insert_script`] asks it
/// again before writing anything — neither can accept what the other rejects.
pub fn check_mapping(
    targets: &[TargetColumn],
    mapping: &ColumnMapping,
    data: &ImportedTable,
) -> Result<Vec<(usize, usize)>, String> {
    if data.rows.is_empty() {
        return Err("The file has no rows to import.".to_string());
    }
    if mapping.len() != data.columns.len() {
        return Err("The column mapping does not match the file.".to_string());
    }

    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for (source, target) in mapping.iter().enumerate() {
        let Some(target) = *target else { continue };
        if target >= targets.len() {
            return Err(
                "The column mapping points at a column the table does not have.".to_string(),
            );
        }
        if pairs.iter().any(|(_, taken)| *taken == target) {
            return Err(format!(
                "Two file columns are mapped to {}.",
                targets[target].name
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

fn resolve_mapping(request: &ImportRequest) -> Result<Vec<(usize, usize)>, String> {
    check_mapping(request.targets, request.mapping, request.data)
}

/// Build the script that loads `data` into `table`.
pub fn build_insert_script(request: &ImportRequest) -> Result<String, String> {
    let pairs = resolve_mapping(request)?;
    let db_type = request.dialect.db_type();
    let table = quote_qualified_name(db_type, request.table);
    let column_list = pairs
        .iter()
        .map(|(_, target)| quote_column_name(db_type, &request.targets[*target].name))
        .collect::<Vec<_>>()
        .join(", ");

    let mut rows: Vec<String> = Vec::with_capacity(request.data.rows.len());
    for (row_index, row) in request.data.rows.iter().enumerate() {
        let values = pairs
            .iter()
            .map(|(source, target)| {
                let column = &request.targets[*target];
                cell_literal(
                    request.dialect,
                    &column.name,
                    row_index + 1,
                    column.kind,
                    row.get(*source).unwrap_or(&None),
                )
            })
            .collect::<Result<Vec<_>, _>>()?
            .join(", ");
        rows.push(values);
    }

    let mut script = String::new();
    for chunk in batches(&rows, request.batch_rows) {
        if !script.is_empty() {
            script.push('\n');
        }
        script.push_str(&if request.dialect.is_mysql_or_mariadb() {
            mysql_batch(&table, &column_list, chunk)
        } else {
            oracle_batch(&table, &column_list, chunk)
        });
        script.push('\n');
    }
    Ok(script)
}

/// Split `rows` into the groups that each become ONE statement.
///
/// Two limits, because a row count alone does not bound what actually gets
/// sent. `MAX_BATCH_BYTES` is the one that bites on a file of long values:
/// 100 rows of a 200 KB document column is a twenty-megabyte statement, and
/// MariaDB refuses it outright — `Packet is larger than max_allowed_packet`,
/// measured, with the whole import lost rather than a row of it.
///
/// A group always holds at least one row. A single row that is larger than the
/// cap cannot be split any further here — that is one value the server itself
/// has to accept or refuse, and it says so in its own words.
///
/// What is counted is the VALUES text; the statement adds a table name, a
/// column list and a few characters of punctuation per row on top. That is
/// kilobytes against a cap of megabytes, which is why the cap is set well below
/// the smallest limit it has to respect rather than computed exactly.
fn batches(rows: &[String], max_rows: usize) -> Vec<&[String]> {
    let max_rows = max_rows.max(1);
    let mut groups: Vec<&[String]> = Vec::new();
    let mut start = 0usize;
    while start < rows.len() {
        let mut end = start;
        let mut bytes = 0usize;
        while end < rows.len() && end - start < max_rows {
            let next = rows[end].len();
            if end > start && bytes + next > MAX_BATCH_BYTES {
                break;
            }

            bytes += next;
            end += 1;
        }
        groups.push(&rows[start..end]);
        start = end;
    }
    groups
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

/// One cell as a SQL literal, or the sentence saying why it cannot be written.
///
/// NULL comes from the file saying so, never from the text looking empty: the
/// format already answered that question, and [`sql_literal_for_value`] does not
/// re-open it. Oracle substitution defusing happens inside that same writer, so
/// an `&` is as safe here as in a `SQL Inserts` export.
///
/// A value too long for any literal is refused HERE, by name, rather than sent:
/// the server answers `ORA-01704` in the middle of a multi-statement script,
/// which leaves the earlier batches inserted and the rest not.
fn cell_literal(
    dialect: SqlWriteDialect,
    column: &str,
    row_number: usize,
    kind: SqlValueKind,
    cell: &ImportCell,
) -> Result<String, String> {
    match cell {
        None => Ok("NULL".to_string()),
        Some(value) => sql_literal_for_value(dialect, kind, value).map_err(|refusal| {
            crate::ui::grid_sql_export::value_too_long_message(column, row_number, refusal)
        }),
    }
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
    use crate::db::DatabaseType;
    use crate::ui::result_export::ExportFormat;
    use crate::ui::result_import::{parse, ImportOptions};

    fn writable_column(name: &str, kind: SqlValueKind) -> ImportColumn {
        ImportColumn {
            name: name.to_string(),
            kind,
            nullable: true,
            writable: true,
            in_select_star: true,
        }
    }

    /// A column the server computes: offered to nothing, and counted by
    /// position all the same — every data-format export writes its value.
    fn computed_column(name: &str) -> ImportColumn {
        ImportColumn {
            name: name.to_string(),
            kind: SqlValueKind::Unknown,
            nullable: true,
            writable: false,
            in_select_star: true,
        }
    }

    /// A column `SELECT *` leaves out: still a target a statement may name, but
    /// no file position feeds it.
    fn invisible_column(name: &str, kind: SqlValueKind) -> ImportColumn {
        ImportColumn {
            name: name.to_string(),
            kind,
            nullable: true,
            writable: true,
            in_select_star: false,
        }
    }

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
            file_named_the_columns: true,
        }
    }

    fn script(db_type: DatabaseType, batch: usize) -> String {
        let targets = targets();
        let data = data();
        let mapping = default_mapping(&data.columns, &targets);
        build_insert_script(&ImportRequest {
            dialect: SqlWriteDialect::family_default(db_type),
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

    /// A batch is bounded by the SIZE of the statement it builds, not only by a
    /// row count.
    ///
    /// A row count cannot see what actually gets sent: 100 rows of a document
    /// column is a twenty-megabyte statement, and MariaDB answers `Packet is
    /// larger than max_allowed_packet` (measured) — losing the whole import
    /// rather than a row of it.
    #[test]
    fn a_batch_is_bounded_by_bytes_as_well_as_rows() {
        let big = "x".repeat(MAX_BATCH_BYTES / 4);
        let rows: Vec<String> = (0..10).map(|_| big.clone()).collect();
        let groups = batches(&rows, DEFAULT_BATCH_ROWS);
        assert_eq!(groups.len(), 3, "10 rows of a quarter of the cap is 4+4+2");
        for group in &groups {
            let bytes: usize = group.iter().map(String::len).sum();
            assert!(bytes <= MAX_BATCH_BYTES, "a group carries {bytes} bytes");
        }
        assert_eq!(groups.iter().map(|g| g.len()).sum::<usize>(), rows.len());

        // A row larger than the cap cannot be split further, so it goes alone
        // rather than being dropped or merged.
        let huge = vec!["y".repeat(MAX_BATCH_BYTES * 2), "z".to_string()];
        let groups = batches(&huge, DEFAULT_BATCH_ROWS);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].len(), 1);

        // And the row count still applies where it is the smaller limit, which
        // is every ordinary file.
        let small: Vec<String> = (0..250).map(|i| i.to_string()).collect();
        let groups = batches(&small, DEFAULT_BATCH_ROWS);
        assert_eq!(
            groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
            vec![100, 100, 50]
        );
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
            file_named_the_columns: true,
        };
        let mapping = default_mapping(&data.columns, &targets);
        let script = build_insert_script(&ImportRequest {
            dialect: SqlWriteDialect::family_default(DatabaseType::Oracle),
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
            file_named_the_columns: true,
        };
        let mapping = default_mapping(&data.columns, &targets);
        let script = build_insert_script(&ImportRequest {
            dialect: SqlWriteDialect::family_default(DatabaseType::Oracle),
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

    /// A file with no header means the TABLE's columns, in the table's order.
    ///
    /// The dialog used to ask this of the writable list alone, so a table with
    /// a generated column in the middle shifted every later value one column
    /// left — and every data-format export this app writes carries every
    /// column, generated ones included, which is exactly such a file.
    #[test]
    fn positional_mapping_skips_a_column_no_value_may_be_written_into() {
        let table = ImportTargets {
            columns: vec![
                writable_column("A", SqlValueKind::Number),
                computed_column("B"),
                writable_column("C", SqlValueKind::String),
            ],
        };
        assert_eq!(
            table
                .writable()
                .iter()
                .map(|target| target.name.clone())
                .collect::<Vec<_>>(),
            vec!["A".to_string(), "C".to_string()]
        );
        assert_eq!(table.generated_names(), vec!["B".to_string()]);

        // Three file columns, one per table column: A feeds A, B is skipped,
        // and C feeds C — not the writable column that follows B.
        assert_eq!(
            table.positional_mapping(3),
            vec![Some(0), None, Some(1)],
            "the file's third column must reach the table's third column"
        );

        // A short file stops where it stops.
        assert_eq!(table.positional_mapping(2), vec![Some(0), None]);
        // A file wider than the table has nothing left to feed.
        assert_eq!(
            table.positional_mapping(5),
            vec![Some(0), None, Some(1), None, None]
        );

        // And a table with nothing computed maps one to one, as it always did.
        let plain = ImportTargets::all_writable(targets());
        assert_eq!(plain.positional_mapping(2), vec![Some(0), Some(1)]);
    }

    /// A column `SELECT *` leaves out claims no position in the file.
    ///
    /// The other half of the same root. An INVISIBLE column (MySQL 8.0.23+,
    /// MariaDB 10.3+, Oracle 12c+) is a target a statement may name and a
    /// column no export writes, so counting it as a position shifted every
    /// later value one column left — measured live on MySQL 8.0 and MariaDB
    /// 12.2, where the catalog keeps it in the MIDDLE of the list.
    #[test]
    fn positional_mapping_gives_no_position_to_a_column_select_star_omits() {
        let table = ImportTargets {
            columns: vec![
                writable_column("A", SqlValueKind::Number),
                invisible_column("B", SqlValueKind::Number),
                writable_column("C", SqlValueKind::String),
            ],
        };
        // Every column is still a target: an explicit INSERT into an invisible
        // column is legal on both families.
        assert_eq!(
            table
                .writable()
                .iter()
                .map(|target| target.name.clone())
                .collect::<Vec<_>>(),
            vec!["A".to_string(), "B".to_string(), "C".to_string()]
        );
        // A `SELECT *` file of this table has TWO columns, and the second is C.
        assert_eq!(
            table.positional_mapping(2),
            vec![Some(0), Some(2)],
            "the file's second column must reach C, not the invisible B"
        );

        // Oracle sorts an invisible column last (its COLUMN_ID is NULL), which
        // lines up by itself — and must keep lining up.
        let oracle_order = ImportTargets {
            columns: vec![
                writable_column("A", SqlValueKind::Number),
                writable_column("C", SqlValueKind::String),
                invisible_column("B", SqlValueKind::Number),
            ],
        };
        assert_eq!(oracle_order.positional_mapping(2), vec![Some(0), Some(1)]);

        // And the two facts compose: computed AND invisible in one table.
        let both = ImportTargets {
            columns: vec![
                writable_column("A", SqlValueKind::Number),
                invisible_column("HIDDEN", SqlValueKind::Number),
                computed_column("TOTAL"),
                writable_column("C", SqlValueKind::String),
            ],
        };
        // `SELECT *` returns A, TOTAL, C — three columns, and only A and C
        // take a value.
        assert_eq!(both.positional_mapping(3), vec![Some(0), None, Some(2)]);
        assert_eq!(
            both.writable()
                .iter()
                .map(|target| target.name.clone())
                .collect::<Vec<_>>(),
            vec!["A".to_string(), "HIDDEN".to_string(), "C".to_string()]
        );
    }

    /// The whole point of the shift: the values land in the right columns.
    #[test]
    fn a_headerless_file_of_every_column_writes_the_right_values() {
        let table = ImportTargets {
            columns: vec![
                writable_column("A", SqlValueKind::Number),
                computed_column("B"),
                writable_column("C", SqlValueKind::String),
            ],
        };
        let targets = table.writable();
        let data = ImportedTable {
            columns: vec![
                "COLUMN_1".to_string(),
                "COLUMN_2".to_string(),
                "COLUMN_3".to_string(),
            ],
            rows: vec![vec![
                Some("1".to_string()),
                Some("computed".to_string()),
                Some("three".to_string()),
            ]],
            file_named_the_columns: false,
        };
        let script = build_insert_script(&ImportRequest {
            dialect: SqlWriteDialect::family_default(DatabaseType::Oracle),
            table: "T",
            targets: &targets,
            mapping: &table.positional_mapping(data.columns.len()),
            data: &data,
            batch_rows: 10,
        })
        .expect("builds");
        assert!(
            script.contains("INTO T (A, C) VALUES (1, 'three')"),
            "the third file column must be C's value: {script}"
        );
    }

    /// A value no literal can carry stops the import BEFORE anything runs, and
    /// says which cell.
    ///
    /// The alternative is what used to happen: the script goes out, the server
    /// answers `ORA-01704` at some statement in the middle, and the batches
    /// before it are already in.
    #[test]
    fn a_value_too_long_for_a_literal_refuses_the_script_and_names_the_cell() {
        let targets = vec![
            target("SEQ", SqlValueKind::Number),
            target("PHOTO", SqlValueKind::Unknown),
        ];
        let data = ImportedTable {
            columns: vec!["SEQ".to_string(), "PHOTO".to_string()],
            rows: vec![
                vec![Some("1".to_string()), Some("ab".to_string())],
                vec![Some("2".to_string()), Some("z".repeat(6000))],
            ],
            file_named_the_columns: true,
        };
        let mapping: ColumnMapping = vec![Some(0), Some(1)];
        let request = |db_type| ImportRequest {
            dialect: SqlWriteDialect::family_default(db_type),
            table: "T",
            targets: &targets,
            mapping: &mapping,
            data: &data,
            batch_rows: 10,
        };
        let error = build_insert_script(&request(DatabaseType::Oracle))
            .expect_err("Oracle cannot carry a 6000-byte literal");
        assert!(error.contains("Row 2"), "{error}");
        assert!(error.contains("PHOTO"), "{error}");

        // The MySQL family has no such limit, so the same file imports.
        assert!(build_insert_script(&request(DatabaseType::MySQL)).is_ok());
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
            file_named_the_columns: true,
        };
        let mapping = default_mapping(&data.columns, &targets);
        let script = build_insert_script(&ImportRequest {
            dialect: SqlWriteDialect::family_default(DatabaseType::Oracle),
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
            dialect: SqlWriteDialect::family_default(DatabaseType::Oracle),
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
            dialect: SqlWriteDialect::family_default(DatabaseType::Oracle),
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
            file_named_the_columns: true,
        };
        assert!(build_insert_script(&ImportRequest {
            dialect: SqlWriteDialect::family_default(DatabaseType::Oracle),
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
            file_named_the_columns: true,
        };
        let mapping = default_mapping(&data.columns, &targets);
        let script = build_insert_script(&ImportRequest {
            dialect: SqlWriteDialect::family_default(DatabaseType::Oracle),
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
            file_named_the_columns: true,
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
                dialect: SqlWriteDialect::family_default(db_type),
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
                defuse_substitution(
                    SqlWriteDialect::family_default(DatabaseType::Oracle),
                    literal
                ),
                expected,
                "{literal}"
            );
        }
    }

    #[test]
    fn the_mysql_family_has_no_substitution_to_defuse() {
        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            assert_eq!(
                defuse_substitution(SqlWriteDialect::family_default(db_type), "'AT&T'"),
                "'AT&T'"
            );
        }
    }

    #[test]
    fn an_ampersand_in_a_value_reaches_the_script_as_chr_38() {
        let targets = vec![target("NAME", SqlValueKind::String)];
        let data = ImportedTable {
            columns: vec!["NAME".to_string()],
            rows: vec![vec![Some("R&D".to_string())]],
            file_named_the_columns: true,
        };
        let mapping = default_mapping(&data.columns, &targets);
        let oracle = build_insert_script(&ImportRequest {
            dialect: SqlWriteDialect::family_default(DatabaseType::Oracle),
            table: "T",
            targets: &targets,
            mapping: &mapping,
            data: &data,
            batch_rows: 100,
        })
        .expect("builds");
        assert!(oracle.contains("VALUES ('R'||CHR(38)||'D')"), "{oracle}");
        let mysql = build_insert_script(&ImportRequest {
            dialect: SqlWriteDialect::family_default(DatabaseType::MySQL),
            table: "T",
            targets: &targets,
            mapping: &mapping,
            data: &data,
            batch_rows: 100,
        })
        .expect("builds");
        assert!(mysql.contains("('R&D')"), "{mysql}");
    }

    /// The import script names the table the catalog named, even when the
    /// object browser already quoted part of it.
    #[test]
    fn an_already_quoted_mysql_table_name_is_not_quoted_twice() {
        let targets = vec![target("A", SqlValueKind::Number)];
        let data = ImportedTable {
            columns: vec!["A".to_string()],
            rows: vec![vec![Some("1".to_string())]],
            file_named_the_columns: true,
        };
        let mapping = default_mapping(&data.columns, &targets);
        for (db_type, table, expected) in [
            (
                DatabaseType::MySQL,
                "app.`zr``tick`",
                "INSERT INTO `app`.`zr``tick` (`A`)",
            ),
            (
                DatabaseType::MariaDB,
                "`sales.ops`.`order.items`",
                "INSERT INTO `sales.ops`.`order.items` (`A`)",
            ),
            (
                DatabaseType::Oracle,
                "SCOTT.\"my table\"",
                "INTO SCOTT.\"my table\" (A)",
            ),
        ] {
            let script = build_insert_script(&ImportRequest {
                dialect: SqlWriteDialect::family_default(db_type),
                table,
                targets: &targets,
                mapping: &mapping,
                data: &data,
                batch_rows: 100,
            })
            .expect("builds");
            assert!(script.contains(expected), "{db_type}: {script}");
        }
    }

    /// The one validator refuses a mapping the server would, and the DIALOG
    /// asks it — so this is what the user sees before the modal closes.
    #[test]
    fn the_shared_validator_refuses_two_file_columns_on_one_target() {
        let targets = targets();
        let data = data();
        let error =
            check_mapping(&targets, &vec![Some(0), Some(0), None], &data).expect_err("refused");
        assert!(error.contains("Two file columns"), "{error}");
        assert!(check_mapping(&targets, &vec![None, None, None], &data)
            .expect_err("refused")
            .contains("at least one"),);
        assert!(check_mapping(&targets, &default_mapping(&data.columns, &targets), &data).is_ok());
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
            dialect: SqlWriteDialect::family_default(DatabaseType::MySQL),
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
            file_named_the_columns: true,
        };
        let mapping = default_mapping(&data.columns, &targets);
        assert_eq!(
            describe(&ImportRequest {
                dialect: SqlWriteDialect::family_default(DatabaseType::Oracle),
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
