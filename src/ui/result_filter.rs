//! Gates and relation assembly for filtering a finished query result by
//! re-querying it: the result's own statement becomes a derived table that the
//! table-browse filter bar can hang a `WHERE` / `ORDER BY` on, so the existing
//! `build_logical_sql` / `build_page_sql` pipeline needs no new SQL shapes.
//!
//! Every dialect fact encoded here was measured against Oracle 26ai (thin),
//! MySQL 8.0.46, and MariaDB 12.2.2 by `src/bin/verify_derived_table_wrap.rs`.
//! The two that drive the design:
//!
//! * The alias must be written `(...) sq_src` — Oracle rejects `AS` on a table
//!   alias and the MySQL family requires an alias to be present at all.
//! * Duplicate column names split the families. Oracle wraps them happily and
//!   only fails when the filter *names* a duplicated column; MySQL and MariaDB
//!   reject the derived table outright, whatever the filter says.
//!
//! Driver error codes are deliberately absent from the messages here: a guard
//! test forbids them in `src/ui`, and the UI does not know which backend an
//! error came from anyway.

use std::collections::{HashMap, HashSet};

use crate::db::query::QueryExecutor;
use crate::db::DatabaseType;
use crate::sql_parser_engine::lexical_spans;
use crate::sql_text::is_identifier_start_byte;

/// Alias given to the derived table wrapping a user statement. Written without
/// `AS` because Oracle rejects that spelling on a table alias.
pub(crate) const DERIVED_RELATION_ALIAS: &str = "sq_src";

/// Whether a finished result can be filtered, and with what caveat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ResultFilterSupport {
    /// The result wraps cleanly and every column may be named in the filter.
    Full,
    /// Oracle: the result wraps, but these column names occur more than once,
    /// so naming one of them in `WHERE` / `ORDER BY` fails on the server. Every
    /// other column filters normally.
    AmbiguousColumns(Vec<String>),
    /// The result cannot be filtered; the string says why, in the user's terms.
    Blocked(String),
}

/// The relation expression that puts `user_sql` where a table name would go.
///
/// The closing parenthesis goes on its own line on purpose: a statement ending
/// in a line comment (`SELECT * FROM t -- note`) would otherwise swallow the
/// `)` into the comment. Trailing semicolons go first, matching what
/// `compose_edit_script` already does before reusing a result's statement.
pub(crate) fn derived_relation_sql(user_sql: &str) -> String {
    let body = user_sql.trim().trim_end_matches(';').trim();
    format!("(\n{body}\n) {DERIVED_RELATION_ALIAS}")
}

/// Column names that appear more than once, in first-appearance order and in
/// their original spelling.
///
/// Comparison is case-insensitive because neither family distinguishes column
/// names by case when resolving a reference. Blank names are skipped: they
/// cannot be named in a filter regardless, and a blank reaching the grid means
/// the name was blanked on the client (`SET HEADING OFF`), so it says nothing
/// about what the server would call the column.
pub(crate) fn duplicate_column_names(column_names: &[String]) -> Vec<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for name in column_names {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            continue;
        }
        *counts.entry(trimmed.to_uppercase()).or_default() += 1;
    }

    let mut reported = HashSet::new();
    let mut duplicates = Vec::new();
    for name in column_names {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = trimmed.to_uppercase();
        if counts.get(&key).copied().unwrap_or(0) > 1 && reported.insert(key) {
            duplicates.push(trimmed.to_string());
        }
    }
    duplicates
}

/// Whether `sql` carries a placeholder whose value the app cannot reproduce on
/// a re-run: an Oracle bind (`:name`, `:1`) or a SQL*Plus substitution
/// variable (`&name`).
///
/// Only code positions count — a `'HH24:MI:SS'` format model or a `q'[a&b]'`
/// literal is not a placeholder. `&` is checked on Oracle only: it is the
/// bitwise AND operator in the MySQL family.
/// The locking clause this statement ends with, if any.
///
/// Filtering re-runs the statement, and a locking read would take its locks
/// again — silently, from a filter box. The clause is also the one shape the
/// derived-table wrap cannot carry on Oracle, so this gate keeps that error
/// from reaching the user as a driver message.
///
/// Only whole words outside strings and comments count, so a column named
/// `SHARE` or a comment mentioning `FOR UPDATE` does not block anything.
fn locking_clause(sql: &str, db_type: DatabaseType) -> Option<&'static str> {
    const CLAUSES: [&[&str]; 4] = [
        &["FOR", "UPDATE"],
        &["FOR", "SHARE"],
        &["FOR", "NO", "KEY", "UPDATE"],
        &["LOCK", "IN", "SHARE", "MODE"],
    ];
    const LABELS: [&str; 4] = [
        "FOR UPDATE",
        "FOR SHARE",
        "FOR NO KEY UPDATE",
        "LOCK IN SHARE MODE",
    ];

    let words = code_words(sql, db_type.is_mysql_or_mariadb());
    CLAUSES
        .iter()
        .zip(LABELS)
        .find(|(clause, _)| {
            words.windows(clause.len()).any(|window| {
                window
                    .iter()
                    .zip(clause.iter())
                    .all(|(word, keyword)| word == keyword)
            })
        })
        .map(|(_, label)| label)
}

/// The statement terminator a filter clause carries, if it carries one.
///
/// The clause is spliced into a statement this window builds, so a terminator
/// inside it ends that statement early: whatever follows is either dropped on
/// the way to the server or would run as a second statement of its own. A
/// filter runs one query or none, so the clause is refused before it is built.
///
/// Only code positions count — a `;` inside a string, a quoted identifier or a
/// comment is text — and what terminates is per family: every backend here
/// ends a statement on `;`, and the Oracle family ends one on a line that is
/// nothing but `/` as well.
pub(crate) fn clause_statement_terminator(
    clause: &str,
    db_type: DatabaseType,
) -> Option<&'static str> {
    let mysql = db_type.is_mysql_or_mariadb();
    let spans = lexical_spans(clause, mysql);
    let in_code = |offset: usize| !spans.iter().any(|span| span.contains(offset));

    if clause
        .bytes()
        .enumerate()
        .any(|(offset, byte)| byte == b';' && in_code(offset))
    {
        return Some(";");
    }
    if mysql {
        return None;
    }
    let mut offset = 0usize;
    for line in clause.split_inclusive('\n') {
        let indent = line.len() - line.trim_start().len();
        if line.trim() == "/" && in_code(offset + indent) {
            return Some("/");
        }
        offset += line.len();
    }
    None
}

/// The statement's words, uppercased, with strings and comments left out.
fn code_words(sql: &str, mysql: bool) -> Vec<String> {
    let spans = lexical_spans(sql, mysql);
    let bytes = sql.as_bytes();
    let mut words = Vec::new();
    let mut word = String::new();
    let mut idx = 0usize;
    let mut span_idx = 0usize;

    while idx < bytes.len() {
        while span_idx < spans.len() && spans[span_idx].end <= idx {
            span_idx += 1;
        }
        if let Some(span) = spans.get(span_idx) {
            if span.contains(idx) {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                idx = span.end;
                continue;
            }
        }

        let byte = bytes[idx];
        if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' || byte == b'#' {
            word.push(byte.to_ascii_uppercase() as char);
        } else if !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        idx += 1;
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

fn has_unreproducible_placeholder(sql: &str, db_type: DatabaseType) -> bool {
    let mysql = db_type.is_mysql_or_mariadb();
    let spans = lexical_spans(sql, mysql);
    let bytes = sql.as_bytes();
    let mut idx = 0usize;
    let mut span_idx = 0usize;

    while idx < bytes.len() {
        while span_idx < spans.len() && spans[span_idx].end <= idx {
            span_idx += 1;
        }
        if let Some(span) = spans.get(span_idx) {
            if span.contains(idx) {
                idx = span.end;
                continue;
            }
        }

        let byte = bytes[idx];
        if byte == b':' || (byte == b'&' && !mysql) {
            let followed_by_name = bytes
                .get(idx + 1)
                .is_some_and(|next| is_identifier_start_byte(*next) || next.is_ascii_digit());
            if followed_by_name {
                return true;
            }
        }
        idx += 1;
    }
    false
}

/// Decide whether a finished result can be filtered by re-querying it.
///
/// `sql` is the statement that produced the result and `column_names` are the
/// headers the driver reported for it.
pub(crate) fn result_filter_support(
    sql: &str,
    column_names: &[String],
    db_type: DatabaseType,
) -> ResultFilterSupport {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    if trimmed.is_empty() {
        return ResultFilterSupport::Blocked(
            "This result has no statement to re-run, so it cannot be filtered.".to_string(),
        );
    }

    if !QueryExecutor::is_select_statement(trimmed) {
        return ResultFilterSupport::Blocked(
            "Only a query result can be filtered. This result did not come from a SELECT."
                .to_string(),
        );
    }

    if has_unreproducible_placeholder(trimmed, db_type) {
        return ResultFilterSupport::Blocked(
            "This statement uses bind or substitution variables, so it cannot be re-run for \
             filtering."
                .to_string(),
        );
    }

    if let Some(clause) = locking_clause(trimmed, db_type) {
        return ResultFilterSupport::Blocked(format!(
            "This statement locks the rows it reads ({clause}), and filtering re-runs it. Remove \
             the locking clause to filter the result."
        ));
    }

    let duplicates = duplicate_column_names(column_names);
    if duplicates.is_empty() {
        return ResultFilterSupport::Full;
    }

    if db_type.is_mysql_or_mariadb() {
        ResultFilterSupport::Blocked(format!(
            "This result repeats the column name {}, which this database cannot re-query. Give \
             the repeated columns distinct aliases to filter the result.",
            quote_names(&duplicates)
        ))
    } else {
        ResultFilterSupport::AmbiguousColumns(duplicates)
    }
}

fn quote_names(names: &[String]) -> String {
    names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn a_locking_read_is_not_re_run_for_filtering() {
        for db_type in [DatabaseType::Oracle, DatabaseType::MySQL] {
            assert!(matches!(
                result_filter_support("SELECT * FROM EMP FOR UPDATE", &names(&["EMPNO"]), db_type),
                ResultFilterSupport::Blocked(_)
            ));
        }
        assert!(matches!(
            result_filter_support(
                "SELECT * FROM EMP LOCK IN SHARE MODE",
                &names(&["EMPNO"]),
                DatabaseType::MySQL
            ),
            ResultFilterSupport::Blocked(_)
        ));
        assert!(matches!(
            result_filter_support(
                "SELECT * FROM EMP FOR UPDATE OF SAL NOWAIT",
                &names(&["EMPNO"]),
                DatabaseType::Oracle
            ),
            ResultFilterSupport::Blocked(_)
        ));
    }

    #[test]
    fn locking_words_only_count_as_code() {
        // A column or alias that reads like the clause, and the clause inside a
        // comment or a literal, leave the result filterable.
        assert!(matches!(
            result_filter_support(
                "SELECT UPDATE_FOR, 'FOR UPDATE' FROM EMP -- FOR UPDATE",
                &names(&["UPDATE_FOR", "LABEL"]),
                DatabaseType::Oracle
            ),
            ResultFilterSupport::Full
        ));
        assert!(matches!(
            result_filter_support(
                "SELECT SHARE FROM EMP",
                &names(&["SHARE"]),
                DatabaseType::MySQL
            ),
            ResultFilterSupport::Full
        ));
    }

    #[test]
    fn derived_relation_uses_an_alias_without_as() {
        let relation = derived_relation_sql("SELECT 1 FROM DUAL");
        assert!(relation.ends_with(") sq_src"));
        assert!(!relation.to_uppercase().contains(" AS SQ_SRC"));
    }

    #[test]
    fn derived_relation_closes_on_its_own_line_so_a_trailing_comment_cannot_eat_it() {
        let relation = derived_relation_sql("SELECT * FROM EMP -- only the ones I want");
        let last_line = relation.lines().last().unwrap();
        assert_eq!(last_line, ") sq_src");
    }

    #[test]
    fn derived_relation_drops_a_trailing_semicolon() {
        let relation = derived_relation_sql("  SELECT 1 FROM DUAL;  ");
        assert_eq!(relation, "(\nSELECT 1 FROM DUAL\n) sq_src");
    }

    #[test]
    fn duplicate_columns_are_reported_once_in_first_appearance_order() {
        let duplicates = duplicate_column_names(&names(&[
            "EMPNO", "DEPTNO", "DNAME", "DEPTNO", "EMPNO", "LOC",
        ]));
        assert_eq!(duplicates, names(&["EMPNO", "DEPTNO"]));
    }

    #[test]
    fn duplicate_columns_ignore_case() {
        let duplicates = duplicate_column_names(&names(&["deptno", "DEPTNO"]));
        assert_eq!(duplicates, names(&["deptno"]));
    }

    #[test]
    fn duplicate_columns_ignore_blank_names() {
        // SET HEADING OFF blanks every name on the way to the grid; blanks are
        // not evidence that the server repeats a column.
        let duplicates = duplicate_column_names(&names(&["", "  ", ""]));
        assert!(duplicates.is_empty());
    }

    #[test]
    fn unique_columns_report_no_duplicates() {
        assert!(duplicate_column_names(&names(&["A", "B", "C"])).is_empty());
    }

    #[test]
    fn a_plain_select_supports_filtering() {
        assert_eq!(
            result_filter_support(
                "SELECT * FROM EMP",
                &names(&["EMPNO"]),
                DatabaseType::Oracle
            ),
            ResultFilterSupport::Full
        );
    }

    #[test]
    fn a_with_clause_query_supports_filtering() {
        // Measured: all three servers accept a WITH clause inside the derived
        // table, so CTE statements must not be gated out.
        assert_eq!(
            result_filter_support(
                "WITH q AS (SELECT 1 AS ID FROM DUAL) SELECT * FROM q",
                &names(&["ID"]),
                DatabaseType::Oracle,
            ),
            ResultFilterSupport::Full
        );
    }

    #[test]
    fn a_set_operator_query_supports_filtering() {
        assert_eq!(
            result_filter_support(
                "SELECT ID FROM A UNION ALL SELECT ID FROM B",
                &names(&["ID"]),
                DatabaseType::MySQL,
            ),
            ResultFilterSupport::Full
        );
    }

    #[test]
    fn a_parenthesized_query_supports_filtering() {
        assert_eq!(
            result_filter_support(
                "(SELECT ID FROM A) UNION (SELECT ID FROM B)",
                &names(&["ID"]),
                DatabaseType::MySQL,
            ),
            ResultFilterSupport::Full
        );
    }

    #[test]
    fn an_empty_statement_is_blocked() {
        assert!(matches!(
            result_filter_support("   ", &names(&["A"]), DatabaseType::Oracle),
            ResultFilterSupport::Blocked(_)
        ));
    }

    #[test]
    fn a_non_select_statement_is_blocked() {
        for sql in ["UPDATE EMP SET SAL = 1", "BEGIN NULL; END;"] {
            assert!(
                matches!(
                    result_filter_support(sql, &names(&["A"]), DatabaseType::Oracle),
                    ResultFilterSupport::Blocked(_)
                ),
                "{sql} should be blocked"
            );
        }
    }

    #[test]
    fn show_and_describe_are_blocked_even_though_they_return_rows() {
        // These classify as `SelectLike`, which is why the gate uses
        // `is_select_statement` instead: neither can be a derived table.
        for sql in ["SHOW TABLES", "DESCRIBE EMP", "DESC EMP"] {
            assert!(
                matches!(
                    result_filter_support(sql, &names(&["A"]), DatabaseType::MySQL),
                    ResultFilterSupport::Blocked(_)
                ),
                "{sql} should be blocked"
            );
        }
    }

    #[test]
    fn a_bind_variable_blocks_filtering() {
        assert!(matches!(
            result_filter_support(
                "SELECT * FROM EMP WHERE EMPNO = :empno",
                &names(&["EMPNO"]),
                DatabaseType::Oracle,
            ),
            ResultFilterSupport::Blocked(_)
        ));
    }

    #[test]
    fn a_numbered_bind_variable_blocks_filtering() {
        assert!(matches!(
            result_filter_support(
                "SELECT * FROM EMP WHERE EMPNO = :1",
                &names(&["EMPNO"]),
                DatabaseType::Oracle,
            ),
            ResultFilterSupport::Blocked(_)
        ));
    }

    #[test]
    fn a_substitution_variable_blocks_filtering_on_oracle() {
        assert!(matches!(
            result_filter_support(
                "SELECT * FROM EMP WHERE DEPTNO = &dept",
                &names(&["EMPNO"]),
                DatabaseType::Oracle,
            ),
            ResultFilterSupport::Blocked(_)
        ));
    }

    #[test]
    fn a_colon_inside_a_literal_is_not_a_bind_variable() {
        // The single most common false positive: a date format model.
        assert_eq!(
            result_filter_support(
                "SELECT TO_CHAR(HIREDATE, 'HH24:MI:SS') AS T FROM EMP",
                &names(&["T"]),
                DatabaseType::Oracle,
            ),
            ResultFilterSupport::Full
        );
    }

    #[test]
    fn an_ampersand_inside_a_literal_is_not_a_substitution_variable() {
        assert_eq!(
            result_filter_support(
                "SELECT 'Salt & Pepper' AS N FROM DUAL",
                &names(&["N"]),
                DatabaseType::Oracle,
            ),
            ResultFilterSupport::Full
        );
    }

    #[test]
    fn a_colon_inside_a_q_quote_literal_is_not_a_bind_variable() {
        assert_eq!(
            result_filter_support(
                "SELECT q'[a:b and c&d]' AS N FROM DUAL",
                &names(&["N"]),
                DatabaseType::Oracle,
            ),
            ResultFilterSupport::Full
        );
    }

    #[test]
    fn a_colon_inside_a_comment_is_not_a_bind_variable() {
        assert_eq!(
            result_filter_support(
                "SELECT ID FROM T -- see ticket :1234",
                &names(&["ID"]),
                DatabaseType::Oracle,
            ),
            ResultFilterSupport::Full
        );
    }

    #[test]
    fn mysql_bitwise_and_is_not_a_substitution_variable() {
        // `&` is an operator in the MySQL family, so it must not gate a result.
        assert_eq!(
            result_filter_support(
                "SELECT FLAGS & MASK AS F FROM T",
                &names(&["F"]),
                DatabaseType::MySQL,
            ),
            ResultFilterSupport::Full
        );
    }

    #[test]
    fn mysql_assignment_operator_is_not_a_bind_variable() {
        assert_eq!(
            result_filter_support(
                "SELECT @row := @row + 1 AS RN FROM T",
                &names(&["RN"]),
                DatabaseType::MySQL,
            ),
            ResultFilterSupport::Full
        );
    }

    #[test]
    fn duplicate_columns_block_the_mysql_family_outright() {
        // Measured: the derived table itself is refused, whatever the filter
        // names, so the whole feature is unavailable for this result.
        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            let support = result_filter_support(
                "SELECT * FROM A JOIN B ON A.DEPTNO = B.DEPTNO",
                &names(&["EMPNO", "DEPTNO", "DEPTNO", "DNAME"]),
                db_type,
            );
            assert!(
                matches!(support, ResultFilterSupport::Blocked(_)),
                "{db_type:?} should block a duplicate-column result"
            );
        }
    }

    #[test]
    fn duplicate_columns_only_narrow_the_filter_on_oracle() {
        // Measured: Oracle wraps and pages the same relation fine, and fails
        // only when the filter names the repeated column.
        let support = result_filter_support(
            "SELECT * FROM A JOIN B ON A.DEPTNO = B.DEPTNO",
            &names(&["EMPNO", "DEPTNO", "DEPTNO", "DNAME"]),
            DatabaseType::Oracle,
        );
        assert_eq!(
            support,
            ResultFilterSupport::AmbiguousColumns(names(&["DEPTNO"]))
        );
    }

    #[test]
    fn blocked_messages_carry_no_driver_error_codes() {
        // A guard test forbids driver markers in src/ui; keep the messages
        // written for the person reading them.
        let cases = [
            (
                "UPDATE T SET A = 1",
                vec!["A".to_string()],
                DatabaseType::Oracle,
            ),
            (
                "SELECT * FROM A JOIN B ON A.D = B.D",
                names(&["D", "D"]),
                DatabaseType::MySQL,
            ),
        ];
        for (sql, columns, db_type) in cases {
            if let ResultFilterSupport::Blocked(message) =
                result_filter_support(sql, &columns, db_type)
            {
                let lowered = message.to_lowercase();
                assert!(!lowered.contains("ora-"), "{message}");
                assert!(!lowered.contains("dpi-"), "{message}");
            }
        }
    }
}
