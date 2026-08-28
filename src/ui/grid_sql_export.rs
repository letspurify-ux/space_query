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

use crate::db::{
    quote_mysql_identifier, ConnectionInfo, DatabaseType, SessionBackslashRule, SqlValueKind,
};
use crate::ui::result_export::{ExportCell, ExportContent};
use crate::ui::result_table::ResultTableWidget;

/// The table name used when the base table cannot be resolved from the SQL
/// (a join, a CTE, a synthetic grid). Same placeholder DataGrip emits.
const UNKNOWN_TABLE_NAME: &str = "MY_TABLE";

/// How SQL TEXT has to be written for one connection.
///
/// The backend family answers most of it: which identifier quotes, which
/// conversion calls, whether `&` starts a substitution. One thing the family
/// alone cannot answer is whether a backslash escapes inside a string literal.
/// MySQL and MariaDB read `\` as an escape *unless* `NO_BACKSLASH_ESCAPES` is
/// in `sql_mode` — and this app lets every connection choose its own
/// (`ConnectionAdvancedSettings::mysql_sql_mode`, sent as `SET SESSION
/// sql_mode` at connect). A writer that assumed the default doubles every
/// backslash such a session then stores.
///
/// Carried as ONE value rather than as a [`DatabaseType`] beside a loose flag,
/// so "MySQL family, but with Oracle's backslash rule" cannot be assembled by
/// accident at a call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SqlWriteDialect {
    db_type: DatabaseType,
    /// How a `\` inside a string literal is read. Always
    /// [`SessionBackslashRule::Literal`] outside the MySQL family: Oracle has no
    /// such escape.
    backslash: SessionBackslashRule,
}

impl SqlWriteDialect {
    /// The rules a session on `info` runs under, as the CONNECTION was
    /// configured.
    ///
    /// This is the constructor to reach for wherever a connection is in hand;
    /// it is the only one that can be right about `sql_mode`.
    ///
    /// Its honest limit: this reads the connection SETTING, which the app sends
    /// as `SET SESSION sql_mode` at connect. A user who then runs
    /// `SET SESSION sql_mode = '…NO_BACKSLASH_ESCAPES'` in a query tab moves
    /// that tab's session out from under it, and nothing here notices — the app
    /// adopts an in-session change of the isolation level and of the read/write
    /// mode, but not of `sql_mode`. Two things follow, and only one of them is
    /// harmful:
    ///
    /// * a `SQL Inserts` FILE is unaffected. Its literals and the rule it
    ///   declares ([`sql_file_declares_no_backslash_escapes`]) both come from
    ///   this one value, so the file is self-consistent and reads back exactly
    ///   whatever it was written with;
    /// * text that will RUN on the drifted session — an import script, a
    ///   `Copy as SQL …` the user pastes back — is escaped for a rule that
    ///   session no longer follows, and a value holding a backslash is stored
    ///   with one too many or one too few.
    ///
    /// Closing that needs the session's own `sql_mode` to be tracked the way its
    /// transaction state already is; it is not something a writer can decide.
    pub fn for_connection(info: &ConnectionInfo) -> Self {
        Self::for_session(info, None)
    }

    /// The rules the session this text will run on ACTUALLY follows.
    ///
    /// `observed` is what that session was last seen running under — a tab's
    /// own `SET SESSION sql_mode` moves it, and nothing about the connection
    /// says so. `None` means nothing has been observed, so the connection's
    /// configured mode is the best answer there is; it is also the right answer
    /// for a session this app has only just set up, which is every session
    /// until a user changes it.
    ///
    /// Reach for this wherever the tab is known. [`Self::for_connection`] is the
    /// same thing with nothing observed, and it is the RIGHT answer for text
    /// that carries its own rule rather than running on the tab: the object
    /// tree's `SQL Inserts` export declares what it was written with
    /// ([`sql_inserts_dialect_preamble`]), so it is self-consistent whatever a
    /// tab's session is doing.
    ///
    /// An observed [`SessionBackslashRule::Unknown`] costs the values that hold
    /// a backslash — including in a file, which could have declared a rule if
    /// one were known. That is the price of not guessing, and it is paid only
    /// by a tab whose session was moved by a `sql_mode` of its own.
    pub fn for_session(info: &ConnectionInfo, observed: Option<SessionBackslashRule>) -> Self {
        let db_type = info.db_type;
        if !db_type.is_mysql_or_mariadb() {
            return Self {
                db_type,
                backslash: SessionBackslashRule::Literal,
            };
        }
        Self {
            db_type,
            backslash: observed.unwrap_or_else(|| {
                crate::db::session_backslash_rule_for_sql_mode(&info.advanced.mysql_sql_mode)
            }),
        }
    }

    /// The family's own default rules, for a caller with no connection to ask.
    ///
    /// Used where the text is not aimed at a specific session (a unit test) or
    /// where no connection is reachable. It answers `NO_BACKSLASH_ESCAPES` with
    /// the server default (escapes on), which is what this app's own default
    /// `sql_mode` (`TRADITIONAL`) leaves in place.
    pub fn family_default(db_type: DatabaseType) -> Self {
        Self {
            db_type,
            backslash: if db_type.is_mysql_or_mariadb() {
                SessionBackslashRule::Escapes
            } else {
                SessionBackslashRule::Literal
            },
        }
    }

    pub fn db_type(self) -> DatabaseType {
        self.db_type
    }

    pub fn is_mysql_or_mariadb(self) -> bool {
        self.db_type.is_mysql_or_mariadb()
    }

    /// Whether a literal written for this session doubles its backslashes, or
    /// `None` when the session's rule is not known.
    fn doubles_a_backslash(self) -> Option<bool> {
        self.backslash.doubles_a_backslash()
    }
}

/// A snapshot of what the user selected in the grid.
///
/// Rows are carried at full width, with the selection expressed as indexes into
/// `all_columns`, so `SQL Updates` can read primary-key values from columns the
/// user did not select.
#[derive(Clone, Debug)]
pub struct GridSqlSelection {
    /// How SQL text must be written for the connection this will run on.
    pub dialect: SqlWriteDialect,
    /// Resolved base table, already qualified. `None` renders as `MY_TABLE`.
    pub table: Option<String>,
    /// Every non-internal grid column, in grid order.
    pub all_columns: Vec<String>,
    /// Literal kind per `all_columns` entry. Shorter than `all_columns` (in
    /// practice empty) means "unknown", i.e. quote everything.
    pub column_kinds: Vec<SqlValueKind>,
    /// Indexes into `all_columns` covered by the selection rectangle.
    pub selected_columns: Vec<usize>,
    /// Selected rows, each aligned to `all_columns`. SQL NULL is [`None`], never
    /// a piece of text that reads like one.
    pub rows: Vec<Vec<ExportCell>>,
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
        quote_qualified_name(self.dialect.db_type(), name)
    }

    fn quote_column(&self, index: usize) -> String {
        let name = self.all_columns.get(index).map_or("", String::as_str);
        quote_column_name(self.dialect.db_type(), name)
    }

    fn kind(&self, index: usize) -> SqlValueKind {
        self.column_kinds
            .get(index)
            .copied()
            .unwrap_or(SqlValueKind::Unknown)
    }

    /// A cell of `row`. A column the row does not reach has no value at all,
    /// which is SQL NULL.
    fn cell<'a>(&self, row: &'a [ExportCell], index: usize) -> &'a ExportCell {
        row.get(index).unwrap_or(&None)
    }

    fn is_null(&self, row: &[ExportCell], index: usize) -> bool {
        self.cell(row, index).is_none()
    }

    /// One cell as a literal, or the sentence saying why it cannot be written.
    ///
    /// The row NUMBER travels with the cell because it is half of the only
    /// thing a refusal can tell the user — which value to look at — and the
    /// builders are the last place that still knows it.
    fn literal(
        &self,
        row_number: usize,
        row: &[ExportCell],
        index: usize,
    ) -> Result<String, String> {
        sql_literal_for_cell(self.dialect, self.kind(index), self.cell(row, index)).map_err(
            |refusal| {
                value_too_long_message(
                    self.all_columns.get(index).map_or("?", String::as_str),
                    row_number,
                    refusal,
                )
            },
        )
    }

    /// Why SQL cannot be written for this selection, or `None`.
    ///
    /// Generated SQL addresses a column by NAME, and a result set does not
    /// promise that a name belongs to one column: `SELECT a.id, b.id` gives two
    /// columns both called `ID`, and every driver reports them that way. The
    /// three shapes then fail three different ways, and two of them fail
    /// SILENTLY — measured on Oracle 23ai and MySQL 8:
    ///
    /// - `INSERT INTO t (ID, ID) VALUES (1, 2)` — the server refuses
    ///   (ORA-00957, MySQL 1110). Useless, but loud.
    /// - `UPDATE t SET ID = 2 WHERE ID = 1` — RUNS. One of the two columns
    ///   became the assignment and the other the key, and nothing says which.
    /// - `ID = 1 AND ID = 2` — matches nothing, ever.
    ///
    /// The writers already learned this on the other side: `unique_field_names`
    /// makes JSON keys and XML elements unique because a repeated name is one
    /// name to every reader. SQL cannot take that way out — a renamed column
    /// names no column — so this refuses instead, and says which name.
    ///
    /// `key_columns` is checked too, and against ALL columns rather than the
    /// selected ones: a key is looked up by name, and picking the first match
    /// would key the statement on a column the user cannot see it chose.
    ///
    /// Case-insensitively, and deliberately for BOTH families. The MySQL family
    /// resolves a column name case-insensitively whatever the backticks say, so
    /// `ID` and `id` really are one column there and generated SQL cannot
    /// address them apart. Oracle quotes a reported name exactly
    /// ([`quote_column_name`]), so there the two ARE separable — this refuses
    /// them all the same, because one rule that occasionally refuses a
    /// selection it could have written is worth more than two rules, and a
    /// refusal costs the user a rename while the alternative costs them a
    /// statement that silently means something else.
    pub fn ambiguous_column_refusal(&self, key_columns: &[String]) -> Option<String> {
        let selected = self
            .selected_columns
            .iter()
            .filter_map(|index| self.all_columns.get(*index))
            .map(String::as_str);
        if let Some(name) = first_repeated_column_name(selected) {
            return Some(ambiguous_column_message(&name));
        }
        for key in key_columns {
            let wanted = key.trim();
            let matches = self
                .all_columns
                .iter()
                .filter(|column| column.trim().eq_ignore_ascii_case(wanted))
                .count();
            if matches > 1 {
                return Some(ambiguous_column_message(wanted));
            }
        }
        None
    }

    /// Drop the columns the server COMPUTES from what a statement will name.
    ///
    /// The one place the rule is applied, and the reason
    /// [`writable_column_indices`] has a single caller. A statement that gives a
    /// value to a virtual, `GENERATED ALWAYS` or stored-generated column cannot
    /// run — Oracle answers `ORA-54013` on an `INSERT` and `ORA-54017` on an
    /// `UPDATE`, the MySQL family answers 3105 to both — so `SQL Inserts` and
    /// `SQL Updates`, the two shapes meant to be RE-RUN, must not name one.
    /// `Where Clause` READS, so it keeps every column and never calls this.
    ///
    /// `generated` is what the catalog reports, by name. An empty list narrows
    /// nothing, which is what "this table computes no column" means; a caller
    /// that could not read the catalog must say so rather than pass an empty
    /// list, because the two are not the same answer.
    ///
    /// Applied to `selected_columns` alone: `all_columns` still carries every
    /// value, so `SQL Updates` can still read a key from a column the user did
    /// not select.
    /// Answers the names it dropped, so a caller can say what it left out
    /// rather than quietly writing fewer columns than the user selected.
    pub fn restrict_to_writable_columns(&mut self, generated: &[String]) -> Vec<String> {
        if generated.is_empty() {
            return Vec::new();
        }
        let writable = writable_column_indices(&self.all_columns, generated);
        let mut dropped = Vec::new();
        self.selected_columns.retain(|index| {
            if writable.contains(index) {
                return true;
            }
            if let Some(name) = self.all_columns.get(*index) {
                dropped.push(name.clone());
            }
            false
        });
        dropped
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

/// Quote a possibly dot-qualified object name that is ALREADY spelled the way
/// its provenance requires.
///
/// Idempotent over a correctly quoted name, which is the only kind that should
/// reach it: [`resolve_export_table`] and
/// [`crate::ui::object_browser::ObjectBrowserWidget::qualified_object_name`]
/// are the two places that decide how a name is spelled, because they are the
/// two places that still know where it came from.
pub fn quote_qualified_name(db_type: DatabaseType, name: &str) -> String {
    if db_type.is_mysql_or_mariadb() {
        // Quote-aware and idempotent: the name reaching here has often been
        // quoted once already by the object browser, and a naive `split('.')`
        // would re-quote those segments into a different name.
        crate::db::quote_mysql_qualified_name(name)
    } else {
        // Oracle: a name that is already a legal bare identifier stays bare, so
        // generated SQL reads the way a person would write it, and one that is
        // already quoted is passed through.
        ResultTableWidget::quote_qualified_identifier(name)
    }
}

/// Quote a single name the SERVER reported — a catalog column, or a column of a
/// result set as the driver named it.
///
/// Identity-preserving, which is a different rule from the one that suits a
/// name the USER typed. Oracle's parser folds a bare identifier to upper case,
/// so a column really called `id` — declared `"id"`, which is what every tool
/// that quotes its DDL creates — is named by `"id"` and by nothing else:
/// writing it bare asks for `ID` and the server answers `ORA-00904`. A reserved
/// word (`"COMMENT"`) is the same story, and so is a name that needs quotes for
/// any other reason. This used to go through
/// [`ResultTableWidget::quote_identifier_segment`], whose rule is the right one
/// for a name PARSED out of the user's SQL (`FROM emp` means `EMP`) and the
/// wrong one here — so every statement `SQL Inserts`, `SQL Updates`,
/// `Where Clause` and a file import wrote for such a table failed outright.
///
/// [`crate::db::DatabaseConnection::quote_oracle_identifier`] is the app's ONE
/// answer to "how is an Oracle name the server reported written back", the same
/// one the object browser qualifies a table with; the MySQL family always
/// backticks, which is identity-preserving already.
///
/// Names are still COMPARED generously (case-insensitively, trimmed) wherever a
/// file column is paired with a table column: matching generously to FIND a
/// column and then naming it exactly as reported is the pair that works on
/// every backend here.
pub fn quote_column_name(db_type: DatabaseType, name: &str) -> String {
    quote_reported_name(db_type, name)
}

/// Quote ONE name the server reported, whatever kind of object it names.
///
/// A column and a table segment need the identical rule — "spell it so the
/// server names the same thing again" — so they share it rather than each
/// picking a quoter. [`quote_column_name`] is the name the writers ask it by;
/// [`resolve_export_table`] asks it for the two segments of a base table.
fn quote_reported_name(db_type: DatabaseType, name: &str) -> String {
    if db_type.is_mysql_or_mariadb() {
        quote_mysql_identifier(name.trim())
    } else {
        crate::db::DatabaseConnection::quote_oracle_identifier(name)
    }
}

/// The base table the generated SQL should name, spelled for `db_type`.
///
/// TWO provenances meet here and they need OPPOSITE treatment, which is why
/// this is the place that decides:
///
/// * `descriptor_table` is the schema and table of the grid-edit descriptor —
///   the names the SERVER reported. They are quoted exactly, by
///   [`quote_column_name`]'s rule, because a table created as `"emp"` is named
///   by `"emp"` and a bare `emp` names `EMP`.
/// * the fallback resolves the table out of the SQL that produced the grid,
///   which handles CTEs and `alias.ROWID` select lists. That name is what the
///   USER typed, so it is left to fold the way the parser folds it — quoting
///   `FROM emp` as `"emp"` would name a table that does not exist.
///
/// `None` renders as `MY_TABLE`.
pub fn resolve_export_table(
    db_type: DatabaseType,
    descriptor_table: Option<(String, String)>,
    source_sql: &str,
) -> Option<String> {
    descriptor_table
        .filter(|(_, table)| !table.trim().is_empty())
        .map(|(schema, table)| {
            let table = quote_reported_name(db_type, &table);
            if schema.trim().is_empty() {
                table
            } else {
                format!("{}.{table}", quote_reported_name(db_type, &schema))
            }
        })
        .or_else(|| crate::ui::sql_editor::query_text::resolve_edit_target_table(source_sql).ok())
        .filter(|table| !table.trim().is_empty())
}

/// The first name in `names` that is not the only one of its name, or `None`.
///
/// Compared the way SQL resolves an unquoted identifier, which is how every
/// other name is paired here.
pub fn first_repeated_column_name<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut seen: Vec<&str> = Vec::new();
    for name in names {
        let name = name.trim();
        if seen.iter().any(|taken| taken.eq_ignore_ascii_case(name)) {
            return Some(name.to_string());
        }
        seen.push(name);
    }
    None
}

/// What to tell a user whose result offers generated SQL no column to name.
///
/// One sentence, because the grid asks it before a snapshot is taken
/// ([`crate::ui::result_table::ResultTableWidget::sql_export_refusal`]) and the
/// builders answer it again for a caller with no grid at all — the object
/// tree's `Export Data...`, which used to write an EMPTY file and report the
/// table's full row count for it.
pub fn no_writable_column_message() -> String {
    "This result has no column that can be written into an INSERT.".to_string()
}

/// What to tell a user whose result has two columns of one name.
///
/// One sentence, because two askers reach it: the export path decides after the
/// format is chosen, and the grid's `Copy as SQL …` decides on the click.
pub fn ambiguous_column_message(name: &str) -> String {
    format!(
        "More than one column of this result is named {name}. Generated SQL addresses a \
         column by name, so it cannot tell them apart — give them different aliases in the \
         query, or select just one of them."
    )
}

/// The columns a statement may name, as indexes into `all_columns`.
///
/// Reached through [`GridSqlSelection::restrict_to_writable_columns`], which is
/// the one place that applies it — a rule with two appliers is a rule the two
/// can come to disagree about, and that is exactly what happened: the object
/// tree's export applied it and the result grid's did not, so `Copy as SQL
/// Inserts` and `Export ▸ SQL Inserts` on a table with a computed column wrote
/// a script the server refuses.
///
/// A column the server computes cannot be given a value — Oracle answers
/// `ORA-54013: INSERT operation disallowed on virtual columns`, MySQL and
/// MariaDB answer error 3105 — so a `SQL Inserts` export that names one writes
/// a script that cannot run. `SQL Inserts` is the one format whose PURPOSE is
/// to be re-run, which is why "write what the grid shows" is the wrong rule
/// for it and right for every other format.
///
/// The same rule the import side already applies in
/// [`crate::ui::object_browser::ObjectBrowserWidget::import_target_columns`],
/// stated once here so the two ends of the round trip cannot drift: what an
/// import will not offer as a target is what an export must not name.
///
/// Names are matched the way SQL resolves an unquoted identifier, because that
/// is how the catalog and the result set are paired everywhere else here.
fn writable_column_indices(all_columns: &[String], generated: &[String]) -> Vec<usize> {
    (0..all_columns.len())
        .filter(|index| {
            let name = all_columns[*index].trim();
            !generated
                .iter()
                .any(|skipped| skipped.trim().eq_ignore_ascii_case(name))
        })
        .collect()
}

/// The word a `SQL Inserts` build writes when its literals do NOT read a
/// backslash as an escape.
///
/// The escaping rule is the one thing about a file of `INSERT` statements that
/// cannot be read off the file itself. The literal `'a\b'` is the three
/// characters `a`, `\`, `b` when the session that wrote it ran under
/// `NO_BACKSLASH_ESCAPES`, and the two characters `a` and U+0008 under any
/// other MySQL-family session — the same bytes, two meanings — so a reader that
/// guesses is wrong for half the files. It guessed "escapes on" for anything
/// holding a backtick, which turned this app's own export of `a\b` back into
/// `a` + U+0008 and lost every statement after a value ending in a backslash.
///
/// So the FILE says it, in a comment, which is inert to run, ignored by every
/// other tool, and — being a comment BEFORE the first statement — somewhere a
/// VALUE can never appear.
const NO_BACKSLASH_ESCAPES_MARK: &str = "NO_BACKSLASH_ESCAPES";

/// What makes a leading comment a DECLARATION rather than prose.
///
/// The reader only looks past this, so a comment that merely mentions the mode
/// — "this file does not use NO_BACKSLASH_ESCAPES" — cannot flip the rule.
const SQL_MODE_DECLARATION: &str = "sql_mode:";

/// What a `SQL Inserts` build starts with so it can be read back.
///
/// Written only when the rule differs from the one a reader assumes for the
/// family, so an ordinary export is byte-for-byte what it always was.
///
/// Only `SQL Inserts` carries it, because only `SQL Inserts` is read back:
/// `SQL Updates` and `Where Clause` are clipboard text this app never parses,
/// and the import script [`crate::ui::table_import::build_insert_script`]
/// writes runs straight back on the connection whose rule it was written with.
///
/// It DECLARES the rule; it does not impose it. Running the file on a session
/// that follows the other rule still misreads the literals — that is true of
/// every SQL file and is not something a comment can change. Making it a real
/// `SET sql_mode` statement would fix that and move the user's session out from
/// under them, which is a bigger surprise than the one it removes; a dump that
/// wants both writes the statement and restores it, and this is not a dump.
fn sql_inserts_dialect_preamble(dialect: SqlWriteDialect) -> &'static str {
    if dialect.is_mysql_or_mariadb() && dialect.doubles_a_backslash() == Some(false) {
        concat!(
            "-- sql_mode: NO_BACKSLASH_ESCAPES",
            " (a backslash in a literal below is one character, not an escape)\n"
        )
    } else {
        ""
    }
}

/// Whether a file of `INSERT` statements DECLARES that its literals read a
/// backslash as an ordinary character.
///
/// The exact inverse of [`sql_inserts_dialect_preamble`], and it lives beside it
/// for the reason round 7 gave the other in-band marks this app writes: a mark
/// has to be read WHERE it is written, or a value can forge one. This one is
/// written in a comment ahead of every statement, so the scan stops at the first
/// character that is neither whitespace nor a comment — before any statement
/// exists, and therefore before any value does.
///
/// A file that declares nothing keeps its family's default, which is what every
/// dump written by another tool follows.
pub fn sql_file_declares_no_backslash_escapes(text: &str) -> bool {
    let mut rest = text;
    loop {
        rest = rest.trim_start();
        let (comment, tail) = if let Some(body) = rest.strip_prefix("--") {
            body.split_once('\n').unwrap_or((body, ""))
        } else if let Some(body) = rest.strip_prefix("/*") {
            // An unterminated block comment runs to the end of the file, so
            // everything left really is comment text.
            body.split_once("*/").unwrap_or((body, ""))
        } else {
            return false;
        };
        if declares_no_backslash_escapes(comment) {
            return true;
        }
        rest = tail;
    }
}

/// Whether ONE comment is the declaration, and says that mode.
///
/// The mode is matched as a whole word so a longer name that merely starts the
/// same way cannot answer for it.
fn declares_no_backslash_escapes(comment: &str) -> bool {
    let trimmed = comment.trim_start();
    let Some(declared) = trimmed
        .get(..SQL_MODE_DECLARATION.len())
        .filter(|head| head.eq_ignore_ascii_case(SQL_MODE_DECLARATION))
        .map(|head| &trimmed[head.len()..])
    else {
        return false;
    };
    declared
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .any(|word| word.eq_ignore_ascii_case(NO_BACKSLASH_ESCAPES_MARK))
}

/// `INSERT INTO <table> (<selected columns>) VALUES (…);` per selected row.
///
/// Every reason this can write nothing comes back as an
/// [`ExportContent::Refused`] carrying the sentence to show. It used to be an
/// empty string, which a caller cannot tell from an empty result — and the
/// callers then reported the INPUT row count for it, announcing an empty file
/// as a full export.
///
/// The build starts with [`sql_inserts_dialect_preamble`] when the connection's
/// escaping rule is not the one a reader would assume, so what this writes can
/// be read back — by this app's own `SQL Inserts` import above all.
pub fn build_sql_inserts(selection: &GridSqlSelection) -> ExportContent {
    if let Some(reason) = selection.ambiguous_column_refusal(&[]) {
        return ExportContent::Refused(reason);
    }
    if selection.selected_columns.is_empty() {
        return ExportContent::Refused(no_writable_column_message());
    }
    if selection.rows.is_empty() {
        return ExportContent::nothing();
    }
    let table = selection.table_name();
    let columns = selection
        .selected_columns
        .iter()
        .map(|index| selection.quote_column(*index))
        .collect::<Vec<_>>()
        .join(", ");

    let mut out = String::from(sql_inserts_dialect_preamble(selection.dialect));
    let mut written = 0usize;
    for (row_index, row) in selection.rows.iter().enumerate() {
        let values = match selection
            .selected_columns
            .iter()
            .map(|index| selection.literal(row_index + 1, row, *index))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(values) => values.join(", "),
            Err(reason) => return ExportContent::Refused(reason),
        };
        out.push_str(&format!(
            "INSERT INTO {table} ({columns}) VALUES ({values});\n"
        ));
        written += 1;
    }
    ExportContent::written(out, written)
}

/// `UPDATE <table> SET … WHERE <key columns>;` per selected row.
///
/// `key_columns` are primary-key column names. When none are known the WHERE
/// clause is omitted, matching DataGrip: it is the caller's job to tell the user
/// that happened.
pub fn build_sql_updates(selection: &GridSqlSelection, key_columns: &[String]) -> ExportContent {
    // This is the shape that used to RUN while meaning something nobody asked
    // for, so the gate is inside the builder now rather than beside it.
    if let Some(reason) = selection.ambiguous_column_refusal(key_columns) {
        return ExportContent::Refused(reason);
    }

    if selection.selected_columns.is_empty() {
        return ExportContent::Refused(no_writable_column_message());
    }
    if selection.rows.is_empty() {
        return ExportContent::nothing();
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
    let mut written = 0usize;
    for (row_index, row) in selection.rows.iter().enumerate() {
        let row_number = row_index + 1;
        let assignments = match assigned
            .iter()
            .map(|index| {
                selection
                    .literal(row_number, row, *index)
                    .map(|literal| format!("{} = {literal}", selection.quote_column(*index)))
            })
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(parts) => parts.join(", "),
            Err(reason) => return ExportContent::Refused(reason),
        };
        let mut statement = format!("UPDATE {table} SET {assignments}");
        if !keys.is_empty() {
            let predicates = match keys
                .iter()
                .map(|index| equality_predicate(selection, row_number, row, *index))
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(parts) => parts.join(" AND "),
                Err(reason) => return ExportContent::Refused(reason),
            };
            statement.push_str(&format!(" WHERE {predicates}"));
        }
        statement.push_str(";\n");
        out.push_str(&statement);
        written += 1;
    }
    ExportContent::written(out, written)
}

/// A WHERE condition that matches exactly the selected cells.
///
/// Values in one row are AND-combined, rows are OR-combined, and a
/// single-column selection collapses into `IN` — DataGrip's rules. Two
/// departures, both because the alternative cannot match: a lone value uses `=`
/// instead of `IN (x)`, and NULLs are lifted out of the `IN` list into
/// `IS NULL`, since `IN` never matches NULL.
pub fn build_where_clause(selection: &GridSqlSelection) -> ExportContent {
    if let Some(reason) = selection.ambiguous_column_refusal(&[]) {
        return ExportContent::Refused(reason);
    }
    if selection.selected_columns.is_empty() {
        return ExportContent::Refused(no_writable_column_message());
    }
    if selection.rows.is_empty() {
        return ExportContent::nothing();
    }

    if let [column] = selection.selected_columns.as_slice() {
        return single_column_where(selection, *column);
    }

    let mut groups: Vec<String> = Vec::new();
    // Kept beside the list rather than asked OF it: `Vec::contains` is a scan,
    // and a scan per row makes this quadratic in the SELECTION. Measured before
    // the set existed — 2 000 rows 35 ms, 4 000 128 ms, 8 000 473 ms, 16 000
    // 1.73 s, a clean x4 per doubling, extrapolating to a minute for a hundred
    // thousand — and it runs on the UI thread, under the app-state lock, for a
    // `Ctrl+A` and one menu pick. The set changes no output: the first spelling
    // of a group still wins and the order is still the rows'.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (row_index, row) in selection.rows.iter().enumerate() {
        let group = match selection
            .selected_columns
            .iter()
            .map(|index| equality_predicate(selection, row_index + 1, row, *index))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(parts) => parts.join(" AND "),
            Err(reason) => return ExportContent::Refused(reason),
        };
        if !group.is_empty() && seen.insert(group.clone()) {
            groups.push(group);
        }
    }

    // A clause covers every row it was built from, dedup or not: the count says
    // what the user asked for, and the text says it as briefly as it can.
    let rows = selection.rows.len();
    match groups.len() {
        0 => ExportContent::nothing(),
        // One row needs no grouping parentheses.
        1 => ExportContent::written(groups.remove(0), rows),
        _ => ExportContent::written(
            groups
                .into_iter()
                .map(|group| format!("({group})"))
                .collect::<Vec<_>>()
                .join(" OR "),
            rows,
        ),
    }
}

fn single_column_where(selection: &GridSqlSelection, column: usize) -> ExportContent {
    let name = selection.quote_column(column);
    let mut values: Vec<String> = Vec::new();
    // The same set, for the same reason as the row groups above: an `IN` list
    // built with a scan per row is quadratic in the selection.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut has_null = false;
    for (row_index, row) in selection.rows.iter().enumerate() {
        if selection.is_null(row, column) {
            has_null = true;
            continue;
        }
        let literal = match selection.literal(row_index + 1, row, column) {
            Ok(literal) => literal,
            Err(reason) => return ExportContent::Refused(reason),
        };
        if seen.insert(literal.clone()) {
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
    ExportContent::written(clause, selection.rows.len())
}

fn equality_predicate(
    selection: &GridSqlSelection,
    row_number: usize,
    row: &[ExportCell],
    index: usize,
) -> Result<String, String> {
    let name = selection.quote_column(index);
    if selection.is_null(row, index) {
        Ok(format!("{name} IS NULL"))
    } else {
        Ok(format!(
            "{name} = {}",
            selection.literal(row_number, row, index)?
        ))
    }
}

/// Render one exported cell as a SQL literal.
///
/// SQL NULL is [`None`] and nothing else: by the time a cell reaches here the
/// question "was this value NULL?" has already been answered once, where the
/// answer was still knowable, and no text is re-examined for it.
pub fn sql_literal_for_cell(
    dialect: SqlWriteDialect,
    kind: SqlValueKind,
    cell: &ExportCell,
) -> Result<String, ValueCannotBeWritten> {
    match cell {
        None => Ok("NULL".to_string()),
        Some(value) => sql_literal_for_value(dialect, kind, value),
    }
}

/// Render `value` as a literal for `kind`.
///
/// The one place SQL literal text is written for a value this app did not
/// author, and therefore the one place the dialect's escaping rules are
/// applied — including the Oracle substitution defusing, so text that is safe
/// to STORE is also safe to RUN.
///
/// `Err` means no statement can carry this value, for one of the two reasons
/// [`ValueCannotBeWritten`] names. It is a `Result` rather than a lenient
/// `String` because the alternative is writing SQL that is known to fail — or
/// known to be ambiguous — which is the one thing a writer must not do quietly.
pub fn sql_literal_for_value(
    dialect: SqlWriteDialect,
    kind: SqlValueKind,
    value: &str,
) -> Result<String, ValueCannotBeWritten> {
    let mysql_family = dialect.is_mysql_or_mariadb();
    let literal = match kind {
        SqlValueKind::Number | SqlValueKind::Boolean => {
            numeric_literal_or_quoted(value, kind, dialect)
        }
        SqlValueKind::Temporal => {
            if mysql_family {
                // MySQL and MariaDB accept the ISO text the grid already shows.
                quoted_string(value, dialect)
            } else {
                oracle_temporal_literal(value, dialect)
            }
        }
        SqlValueKind::Binary => {
            if mysql_family {
                // The bytes are gone: the grid holds a lossy UTF-8 rendering of
                // them, so the displayed text is all there is to emit.
                quoted_string(value, dialect)
            } else {
                oracle_binary_literal(value, dialect)
            }
        }
        SqlValueKind::String => oracle_text_literal(value, dialect),
        SqlValueKind::Unknown => quoted_string(value, dialect),
    }?;
    Ok(defuse_substitution(dialect, &literal))
}

/// The literal for a value that is going into MySQL-family text.
///
/// The one dialect with no limit on how much a single literal may hold, so this
/// cannot refuse and does not make its caller pretend otherwise. It picks the
/// dialect itself rather than taking one, which is what keeps the "cannot fail"
/// claim true: an Oracle caller cannot reach this at all.
///
/// Its caller is the bind prompt, which substitutes a prompted value into the
/// statement only on MySQL and MariaDB — Oracle BINDS its values, so no literal
/// is written there.
pub fn mysql_family_literal(kind: SqlValueKind, value: &str) -> String {
    let dialect = SqlWriteDialect::family_default(DatabaseType::MySQL);
    sql_literal_for_value(dialect, kind, value).unwrap_or_else(|_| {
        debug_assert!(
            false,
            "the MySQL family has no per-literal limit, and the family default names a KNOWN \
             escaping rule"
        );
        // Unreachable, and if it ever were not, this is what the writer did
        // before the limit existed: quote it and let the server answer.
        format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
    })
}

/// Keep an `&` that came out of the data from being read as a substitution
/// variable.
///
/// Oracle's client-side `DEFINE` is on by default — in this app too
/// ([`crate::db::SessionState::define_enabled`]) — and substitutes `&name`
/// *inside* string literals, the way SQL*Plus does. So a row holding `AT&T`
/// would stop and ask the user to "Enter value for T", whether it arrived from
/// a file being imported or from a `SQL Inserts` export the user re-runs. A
/// value is data, never a variable, so every `&` is lifted out of the literal
/// as `CHR(38)`. The stored text is identical, and the session's `DEFINE`
/// setting is left exactly as the user set it.
///
/// Only a plain string literal is rewritten. A number, a `TO_DATE(…)`, or a
/// `HEXTORAW(…)` cannot contain an `&`, and MySQL and MariaDB have no
/// substitution at all. Applying it twice is inert: the first pass leaves no
/// `&` behind.
pub fn defuse_substitution(dialect: SqlWriteDialect, literal: &str) -> String {
    if dialect.is_mysql_or_mariadb() || !literal.contains('&') {
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

/// How many bytes of TEXT one Oracle string literal may hold, quotes excluded.
///
/// Oracle's own limit, and it is on the LITERAL rather than on the statement:
/// a longer one is `ORA-01704: string literal too long` whatever the target
/// column is, a `CLOB` included. 4000 is what a database with the default
/// `MAX_STRING_SIZE = STANDARD` accepts — measured on Oracle 23ai, where 4000
/// went in and 4001 did not, and again through `HEXTORAW('<4000 hex digits>')`,
/// which is a full `RAW(2000)` and inserts. A database set to `EXTENDED` allows
/// 32767, but which one a connection is talking to is not something this writer
/// knows, and the smaller figure is correct on both.
///
/// Counted on the literal's CONTENT — the escaped text between the quotes —
/// because that is what the server counts. Counting the two quotes as well
/// would refuse a full `RAW(2000)` that the server accepts.
const ORACLE_MAX_LITERAL_BYTES: usize = 4000;

/// Why a value cannot be written as a literal at all.
///
/// Both arms are "this app never writes SQL it knows may be wrong", and both are
/// reported the same way: by name, with the row and the column, before anything
/// runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueCannotBeWritten {
    /// No single SQL literal can carry it.
    ///
    /// The Oracle-only arm. Text has a way out — [`oracle_text_literal`] writes
    /// it as a `TO_CLOB(…)||…` chain — and nothing else does: a `BLOB` reaches
    /// the grid as hex, and neither `HEXTORAW` nor any concatenation can build a
    /// `RAW` past its own limit. Saying so beats handing the server a statement
    /// that answers `ORA-01704` halfway through an import.
    TooLongForOneLiteral {
        /// Bytes the literal's content would need, escaping included.
        literal_bytes: usize,
    },
    /// The value holds a backslash and the session's rule for one is unknown.
    ///
    /// The MySQL-family arm. A backslash is the one character whose meaning
    /// inside a literal depends on `sql_mode` ([`SessionBackslashRule`]), so a
    /// value without one is spelled identically under either rule and is never
    /// refused for this.
    UnknownBackslashRule,
}

impl ValueCannotBeWritten {
    pub fn literal_limit() -> usize {
        ORACLE_MAX_LITERAL_BYTES
    }
}

/// What to tell a user whose value cannot be written as one SQL literal.
///
/// ONE sentence, because three askers reach it: a `SQL Inserts` export, the
/// grid's `Copy as SQL …`, and a file import — and the row and column are the
/// only things that tell the user WHICH value to look at. Row numbers are
/// 1-based and count the rows being written, which is what the user sees.
///
/// Deliberately silent about what to do instead: the export path can suggest a
/// data format and the import path cannot, so the advice belongs to the caller
/// and the fact belongs here.
pub fn value_too_long_message(
    column: &str,
    row_number: usize,
    refusal: ValueCannotBeWritten,
) -> String {
    match refusal {
        ValueCannotBeWritten::TooLongForOneLiteral { literal_bytes } => format!(
            "Row {row_number}, column {column}: this value needs {literal_bytes} bytes as a SQL \
             literal and Oracle accepts at most {} in one. Only text can be written as a \
             concatenation, and this value is not text, so no statement can carry it.",
            ValueCannotBeWritten::literal_limit()
        ),
        ValueCannotBeWritten::UnknownBackslashRule => format!(
            "Row {row_number}, column {column}: this value contains a backslash, and this tab's \
             session was moved by a sql_mode of its own — so this app can no longer tell whether \
             the server will store one backslash or two, and it will not guess. Reconnect the \
             tab, or close and reopen it, to get a session it can answer for."
        ),
    }
}

/// A text value as Oracle SQL, in as many literals as it takes.
///
/// One literal while it fits, which is every ordinary value and byte-for-byte
/// what this wrote before. Past that, `TO_CLOB('…')||TO_CLOB('…')` — Oracle's
/// own way to build a value longer than a literal can be, and the only way a
/// file with a long text column can be imported at all: the whole import used
/// to end at `ORA-01704`, and no batch size could help, because the limit is on
/// one VALUE.
///
/// A `VARCHAR2(n)` target still refuses a value that does not fit it, in its own
/// words — which is an honest complaint about the DATA rather than about the SQL
/// this app wrote.
///
/// Each piece is defused on its own: [`defuse_substitution`] only rewrites a
/// plain literal, so an `&` inside a concatenation would otherwise reach the
/// session's `DEFINE` and stop to ask for a value.
///
/// The MySQL family has no per-literal limit — what bites there is the packet,
/// which [`crate::ui::table_import::MAX_BATCH_BYTES`] bounds — so it keeps the
/// single literal.
fn oracle_text_literal(
    value: &str,
    dialect: SqlWriteDialect,
) -> Result<String, ValueCannotBeWritten> {
    // `TO_CLOB` is Oracle's, so the family that has no per-literal limit leaves
    // before the chunking exists — by returning, not by an assertion that a
    // release build would walk straight past into invalid SQL.
    if dialect.is_mysql_or_mariadb() {
        return quoted_string(value, dialect);
    }
    // Asked of the writer itself rather than measured a second time here: one
    // rule about what fits in a literal, stated where a literal is written.
    if let Ok(single) = quoted_string(value, dialect) {
        return Ok(single);
    }

    let mut parts: Vec<String> = Vec::new();
    let mut piece = String::new();
    let mut piece_bytes = 0usize;
    for ch in value.chars() {
        // A quote is DOUBLED on the way into a literal, so the escaped length
        // is what has to stay under the limit, not the source length.
        let cost = if ch == '\'' {
            2 * ch.len_utf8()
        } else {
            ch.len_utf8()
        };
        if !piece.is_empty() && piece_bytes + cost > ORACLE_MAX_LITERAL_BYTES {
            parts.push(defuse_substitution(
                dialect,
                &quoted_string(&piece, dialect)?,
            ));
            piece.clear();
            piece_bytes = 0;
        }
        piece.push(ch);
        piece_bytes += cost;
    }
    if !piece.is_empty() {
        parts.push(defuse_substitution(
            dialect,
            &quoted_string(&piece, dialect)?,
        ));
    }
    Ok(parts
        .into_iter()
        .map(|part| format!("TO_CLOB({part})"))
        .collect::<Vec<_>>()
        .join("||"))
}

/// Oracle RAW is displayed as an even run of hex digits, which `HEXTORAW`
/// turns back into the same bytes — but only text this can PROVE is that shape
/// may be spliced into the call.
///
/// `HEXTORAW('<value>')` used to be built by interpolation, and it was the one
/// literal here that neither proved its value nor escaped it. That is not a
/// theoretical hole: this writer is also what an IMPORT uses, so the value is
/// whatever a file said, and a cell holding
/// `41')) SELECT * FROM DUAL; DROP TABLE t; --` closed the call, closed the
/// VALUES list, ended the statement and added one of its own — which the app's
/// own script splitter then handed to the executor as a separate statement.
///
/// Nothing is lost by falling back to a quoted string: Oracle applies the SAME
/// implicit hex conversion to a string literal assigned to a RAW/BLOB column
/// (`INSERT INTO t (r) VALUES ('0102FF')` stores the same three bytes as
/// `HEXTORAW('0102FF')`), and raises the SAME `ORA-01465: invalid hex number`
/// when the text is not hex. So the fallback keeps the meaning, keeps the
/// error, and — because it goes through [`quoted_string`] — cannot carry a
/// quote out of the literal. It is also a plain literal again, so
/// [`defuse_substitution`] can reach an `&` inside it, which it never could
/// through `HEXTORAW(…)`.
fn oracle_binary_literal(
    value: &str,
    dialect: SqlWriteDialect,
) -> Result<String, ValueCannotBeWritten> {
    let text = value.trim();
    if is_plain_hex_literal(text) {
        // A full `RAW(2000)` is exactly 4000 digits and inserts; a `LONG RAW`
        // or a `BLOB` read as hex goes further and cannot be written at all.
        return Ok(format!("HEXTORAW('{}')", spliced_literal_content(text)?));
    }
    quoted_string(value, dialect)
}

/// Text this writer has already proven cannot carry a quote, on its way into a
/// conversion call.
///
/// The limit is the LITERAL's, so it applies here too: `HEXTORAW` and
/// `TO_TIMESTAMP` wrap a literal, they do not replace one. Stated once so the
/// two calls that splice proven text cannot come to disagree with
/// [`quoted_string`], which is where every other literal is measured.
fn spliced_literal_content(text: &str) -> Result<&str, ValueCannotBeWritten> {
    if text.len() > ORACLE_MAX_LITERAL_BYTES {
        return Err(ValueCannotBeWritten::TooLongForOneLiteral {
            literal_bytes: text.len(),
        });
    }
    Ok(text)
}

/// Whether this text is what `HEXTORAW` accepts: a non-empty, even-length run
/// of hex digits and nothing else.
///
/// The even length is Oracle's own rule — a hex string is a sequence of BYTES,
/// so an odd digit count is `ORA-01465` too — and requiring it here means the
/// unquoted form is only ever reached by text the server will also accept.
pub(crate) fn is_plain_hex_literal(text: &str) -> bool {
    !text.is_empty()
        && text.len().is_multiple_of(2)
        && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// A number is emitted as SQL TEXT, so it has to BE a number.
///
/// This is the one place a value the app did not write becomes part of a
/// statement without quotes — a CSV cell on its way into an `INSERT`, a bind
/// answer substituted into MySQL-family text — and an unchecked value carries
/// whatever it says INTO the statement: a cell holding
/// `1); DROP TABLE x; INSERT INTO t (a) VALUES (2` closes the `VALUES` list and
/// adds statements of its own, which the app then runs as the user's script.
///
/// So a value this cannot prove is a numeric literal is QUOTED instead, exactly
/// as the result grid's own cell editor already treats everything typed into it
/// (`sql_literal_from_input`: user text is a string literal unless the user
/// writes `=expr`). The server then rejects it for a numeric column, which is an
/// honest error about the value — not a statement nobody asked for. Callers that
/// know the value came from a person say so themselves: the bind prompt refuses
/// the run and names the placeholder rather than quietly comparing against a
/// string.
fn numeric_literal_or_quoted(
    value: &str,
    kind: SqlValueKind,
    dialect: SqlWriteDialect,
) -> Result<String, ValueCannotBeWritten> {
    let trimmed = value.trim();
    let provable = is_plain_numeric_literal(trimmed)
        // `TRUE`/`FALSE` are how a boolean is written where one exists, and
        // quoting them would make the server read `'TRUE'` as 0.
        || (kind == SqlValueKind::Boolean
            && (trimmed.eq_ignore_ascii_case("TRUE") || trimmed.eq_ignore_ascii_case("FALSE")));
    if provable {
        Ok(trimmed.to_string())
    } else {
        quoted_string(value, dialect)
    }
}

/// Whether this text is a plain SQL numeric literal: an optional sign, digits
/// with at most one decimal point, and an optional exponent.
///
/// Nothing else — no hex, no thousands separator, no `Inf`/`NaN`, no expression,
/// no trailing anything. The question is not "could some server parse this as a
/// number" but "is this text safe to put in a statement unquoted", and only a
/// shape this simple answers yes.
pub(crate) fn is_plain_numeric_literal(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut idx = 0usize;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        idx += 1;
    }
    let mut digits = 0usize;
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        idx += 1;
        digits += 1;
    }
    if idx < bytes.len() && bytes[idx] == b'.' {
        idx += 1;
        while idx < bytes.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return false;
    }
    if idx < bytes.len() && matches!(bytes[idx], b'e' | b'E') {
        idx += 1;
        if matches!(bytes.get(idx), Some(b'+' | b'-')) {
            idx += 1;
        }
        let mut exponent_digits = 0usize;
        while idx < bytes.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
            exponent_digits += 1;
        }
        if exponent_digits == 0 {
            return false;
        }
    }
    idx == bytes.len()
}

/// The ONE place a quoted literal is written — and therefore the one place
/// Oracle's limit on how much ONE literal may hold is enforced.
///
/// The limit used to live in [`oracle_text_literal`], which is reached only by
/// `SqlValueKind::String`. Every other kind that falls back to a quoted string
/// — `Unknown` (a `BLOB`, read as hex), an unprovable `Number`, an `INTERVAL`
/// that no `TO_*` shape matched — walked straight past it, so this app's own
/// `SQL Inserts` export of a `BLOB` wrote a file that answers `ORA-01704` when
/// re-imported (measured, thin and OCI alike). A rule that belongs to a value
/// cannot live in one kind's branch.
///
/// The MySQL family has no per-literal limit; what bites there is the packet,
/// which [`crate::ui::table_import::MAX_BATCH_BYTES`] bounds.
fn quoted_string(value: &str, dialect: SqlWriteDialect) -> Result<String, ValueCannotBeWritten> {
    // A doubled quote is how EVERY dialect here spells one inside a literal.
    // The backslash is the only disagreement, and the SESSION answers it — not
    // the family, and not the connection's configured mode: a MySQL-family
    // session running with `NO_BACKSLASH_ESCAPES` stores a doubled backslash as
    // two characters, and a tab can be moved into or out of that mode by a `SET`
    // of its own.
    let escaped = match dialect.doubles_a_backslash() {
        Some(true) => value.replace('\\', "\\\\").replace('\'', "''"),
        Some(false) => value.replace('\'', "''"),
        // The rule is unknown, so a value holding a backslash has two possible
        // meanings and this writer may not pick one. Every other value is
        // spelled identically under both rules and is written as it always was.
        None => {
            if value.contains('\\') {
                return Err(ValueCannotBeWritten::UnknownBackslashRule);
            }
            value.replace('\'', "''")
        }
    };
    if !dialect.is_mysql_or_mariadb() && escaped.len() > ORACLE_MAX_LITERAL_BYTES {
        return Err(ValueCannotBeWritten::TooLongForOneLiteral {
            literal_bytes: escaped.len(),
        });
    }
    Ok(format!("'{escaped}'"))
}

/// Wrap an Oracle date/timestamp in the conversion its displayed shape needs.
///
/// The shapes are exhaustive over what the Oracle executors render, so an
/// unrecognized one means the value is not a plain date — an INTERVAL, say —
/// and is safest emitted as a string.
fn oracle_temporal_literal(
    value: &str,
    dialect: SqlWriteDialect,
) -> Result<String, ValueCannotBeWritten> {
    let text = value.trim();
    let (datetime, zone) = split_timezone_suffix(text);
    let (date_part, time_part) = match datetime.split_once(' ') {
        Some((date, time)) => (date, Some(time)),
        None => (datetime, None),
    };
    if !is_iso_date(date_part) {
        return quoted_string(value, dialect);
    }
    let Some(time_part) = time_part else {
        return Ok(format!(
            "TO_DATE('{}','YYYY-MM-DD')",
            spliced_literal_content(text)?
        ));
    };
    let (clock, fraction) = match time_part.split_once('.') {
        Some((clock, fraction)) => (clock, Some(fraction)),
        None => (time_part, None),
    };
    if !is_clock_time(clock) {
        return quoted_string(value, dialect);
    }
    Ok(match (fraction, zone) {
        (None, None) => format!(
            "TO_DATE('{}','YYYY-MM-DD HH24:MI:SS')",
            spliced_literal_content(text)?
        ),
        // A fractional part is digits, and nothing here bounds how many of them
        // a FILE may carry — the drivers write at most nine.
        (Some(fraction), None) if is_all_digits(fraction) => format!(
            "TO_TIMESTAMP('{}','YYYY-MM-DD HH24:MI:SS.FF')",
            spliced_literal_content(text)?
        ),
        // The zone is re-joined with an explicit space so the text matches the
        // format model exactly. The drivers render the offset without one, and
        // Oracle then reads `TZH` off a value whose sign is where the space
        // should be — silently turning `-05:30` into `+05:30`.
        // The zone is part of the literal's content, so it is part of what is
        // measured — the datetime alone would let a value seven bytes over
        // the limit through.
        (Some(fraction), Some(zone)) if is_all_digits(fraction) => {
            let content = format!("{datetime} {zone}");
            format!(
                "TO_TIMESTAMP_TZ('{}','YYYY-MM-DD HH24:MI:SS.FF TZH:TZM')",
                spliced_literal_content(&content)?
            )
        }
        (None, Some(zone)) => {
            let content = format!("{datetime} {zone}");
            format!(
                "TO_TIMESTAMP_TZ('{}','YYYY-MM-DD HH24:MI:SS TZH:TZM')",
                spliced_literal_content(&content)?
            )
        }
        _ => return quoted_string(value, dialect),
    })
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

    /// An identifier the app did not write is quoted as ONE identifier, or not
    /// passed through at all.
    ///
    /// Both quoters used to hand text back untouched when it merely started and
    /// ended with the identifier delimiter — a different question from "is this
    /// one quoted identifier", which is what they meant. No catalog can produce
    /// such a name on either family (Oracle cannot hold a `"` in a quoted
    /// identifier, and the MySQL quoter always doubles), so this was a false
    /// premise in a quoter rather than a reachable bug.
    #[test]
    fn an_identifier_is_only_passed_through_when_it_is_one() {
        // Well formed: passed through unchanged, so generated SQL keeps reading
        // the way a person writes it.
        for name in ["\"HR\"", "\"Mixed Case\"", "\"A\"\"B\""] {
            assert_eq!(
                quote_column_name(DatabaseType::Oracle, name),
                name,
                "a well-formed quoted identifier is already quoted"
            );
        }
        // Not one identifier: quoted as a whole, so it cannot become statements.
        for name in [
            "\"A\"; DROP TABLE X --\"",
            "\"A\" || \"B\"",
            "\"A\".\"B\"; DELETE FROM t --\"",
        ] {
            let quoted = quote_column_name(DatabaseType::Oracle, name);
            assert!(
                quoted.starts_with('"') && quoted.ends_with('"') && quoted.contains("\"\""),
                "{name:?} is not one identifier and must be quoted whole, got {quoted}"
            );
            assert!(
                !quoted.contains("; DROP TABLE X --\"") || quoted.contains("\"\""),
                "and its inner quotes must be doubled so it cannot end early"
            );
        }
        // The MySQL family always wraps and doubles, so the same text is inert.
        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            let quoted = quote_column_name(db_type, "a`; DROP TABLE x; --");
            assert_eq!(quoted, "`a``; DROP TABLE x; --`");
        }
    }

    /// A value the app did not write becomes SQL text here, so a "number" that
    /// is not one must not be emitted unquoted.
    ///
    /// This is the only place a value crosses into a statement without quotes: a
    /// CSV cell on its way into an `INSERT`, a bind answer substituted into
    /// MySQL-family text. A cell holding
    /// `1); DROP TABLE x; INSERT INTO t (a) VALUES (2` closed the `VALUES` list
    /// and added statements of its own, which the app then ran as the user's
    /// script — and the connection's read-only guard had already judged the text
    /// the value was not yet in.
    #[test]
    fn a_number_literal_is_only_emitted_for_a_value_that_is_one() {
        for db_type in [
            DatabaseType::MySQL,
            DatabaseType::MariaDB,
            DatabaseType::Oracle,
        ] {
            // Real numbers, in every shape a driver or a file produces.
            for value in [
                "0",
                "1",
                "-1",
                "+1",
                "1.5",
                "-1.50",
                ".5",
                "1.",
                "1e3",
                "-1.5E+10",
                "12345678901234567890123",
            ] {
                assert_eq!(
                    value_literal(db_type, SqlValueKind::Number, value),
                    value.trim(),
                    "{db_type}: {value} is a number and must be emitted as one"
                );
            }
            // And anything else is quoted, so it can only ever be a VALUE.
            for value in [
                "1); DROP TABLE x; INSERT INTO t (a) VALUES (2",
                "1 OR 1=1",
                "1; SET GLOBAL max_connections = 5000",
                "abc",
                "1,234",
                "0x1F",
                "",
                "--1",
                "1 2",
            ] {
                let literal = value_literal(db_type, SqlValueKind::Number, value);
                assert!(
                    literal.starts_with('\'') && literal.ends_with('\''),
                    "{db_type}: {value:?} is not a number and must be quoted, got {literal}"
                );
                assert!(
                    !literal.contains("DROP TABLE") || literal.contains("'"),
                    "{db_type}: a quoted value cannot leave its literal"
                );
            }
            // `TRUE`/`FALSE` are how a boolean is written, and quoting them
            // would have the server read `'TRUE'` as 0.
            for value in ["TRUE", "false", "1", "0"] {
                assert_eq!(
                    value_literal(db_type, SqlValueKind::Boolean, value),
                    value.trim(),
                    "{db_type}: {value} is a boolean literal"
                );
            }
        }
    }

    /// A selection fixture.
    ///
    /// A cell written as the fixture's [`NULL_TEXT`] is SQL NULL — the fixtures
    /// have always meant it that way, and a grid snapshot now answers that
    /// question once, where it is knowable, instead of letting every writer
    /// re-derive it from the text. Everything else is a value.
    fn selection(
        db_type: DatabaseType,
        columns: &[(&str, SqlValueKind)],
        rows: &[&[&str]],
    ) -> GridSqlSelection {
        GridSqlSelection {
            dialect: SqlWriteDialect::family_default(db_type),
            table: Some("HR.EMP".to_string()),
            all_columns: columns
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect(),
            column_kinds: columns.iter().map(|(_, kind)| *kind).collect(),
            selected_columns: (0..columns.len()).collect(),
            rows: rows
                .iter()
                .map(|row| row.iter().map(|value| cell(value)).collect())
                .collect(),
        }
    }

    /// One fixture cell: SQL NULL, or the text itself.
    fn cell(value: &str) -> ExportCell {
        (value != NULL_TEXT).then(|| value.to_string())
    }

    /// The literal for a fixture cell.
    ///
    /// Unwraps on purpose: every fixture here is a value that CAN be written,
    /// so a refusal is a defect in the writer and a panic naming the cell is
    /// the most useful thing a test can do with it. The refusal path has tests
    /// of its own — see `a_value_too_long_for_any_literal_is_refused_by_name`.
    fn literal_for(db_type: DatabaseType, kind: SqlValueKind, cell: ExportCell) -> String {
        sql_literal_for_cell(SqlWriteDialect::family_default(db_type), kind, &cell)
            .unwrap_or_else(|refusal| panic!("{db_type} {kind:?} {cell:?} refused: {refusal:?}"))
    }

    /// [`sql_literal_for_value`] for a fixture value, with the same rule.
    fn value_literal(db_type: DatabaseType, kind: SqlValueKind, value: &str) -> String {
        literal_for(db_type, kind, Some(value.to_string()))
    }

    /// The text a builder wrote, for a fixture it must not refuse.
    ///
    /// A refusal is a defect for these fixtures, and its sentence is the most
    /// useful thing to fail with. The refusal paths are asserted directly.
    fn written(built: ExportContent) -> String {
        match built.into_parts() {
            Ok((text, _)) => text,
            Err(reason) => panic!("the builder refused a fixture it should write: {reason}"),
        }
    }

    fn oracle_literal(kind: SqlValueKind, value: &str) -> String {
        literal_for(DatabaseType::Oracle, kind, Some(value.to_string()))
    }

    fn mysql_literal(kind: SqlValueKind, value: &str) -> String {
        literal_for(DatabaseType::MySQL, kind, Some(value.to_string()))
    }

    /// A NULL cell renders as `NULL` whatever the column's kind — the original
    /// point of this test.
    ///
    /// What changed under it is where the answer comes from: an absent cell,
    /// decided once by the grid snapshot, instead of a serializer asking
    /// whether the TEXT reads like a null. That question was generously true
    /// for an empty box and for every spelling of `null`, and on the MySQL
    /// family both are real values — so the second half now states the other
    /// side: text that merely reads like a NULL is not one.
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
            for db_type in [
                DatabaseType::Oracle,
                DatabaseType::MySQL,
                DatabaseType::MariaDB,
            ] {
                assert_eq!(
                    literal_for(db_type, kind, None),
                    "NULL",
                    "{db_type} {kind:?}"
                );
                assert_ne!(
                    literal_for(db_type, kind, Some(String::new())),
                    "NULL",
                    "{db_type} {kind:?}: the empty string is a value"
                );
                assert_ne!(
                    literal_for(db_type, kind, Some("NULL".to_string())),
                    "NULL",
                    "{db_type} {kind:?}: the text NULL is a value"
                );
            }
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
            literal_for(
                DatabaseType::MariaDB,
                SqlValueKind::String,
                Some(r"a\b".to_string())
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
            written(build_sql_inserts(&selection)),
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
            written(build_sql_inserts(&selection)),
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
            written(build_sql_inserts(&selection)),
            "INSERT INTO MY_TABLE (ID) VALUES (1);\n"
        );
    }

    /// A selection covering no column is refused, and writes nothing.
    ///
    /// It used to write nothing and say nothing, which the callers then
    /// reported as a full export of the input's row count — an empty file
    /// announced as "N rows". Both halves are pinned: the text is still empty,
    /// and the refusal now says why.
    #[test]
    fn a_selection_that_covers_no_column_is_refused_and_writes_nothing() {
        let mut selection = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number)],
            &[&["1"]],
        );
        selection.selected_columns.clear();
        for built in [
            build_sql_inserts(&selection),
            build_sql_updates(&selection, &["ID".to_string()]),
            build_where_clause(&selection),
        ] {
            assert_eq!(built.refusal(), Some(no_writable_column_message().as_str()));
            assert!(built.text().is_empty());
            assert_eq!(built.rows(), 0);
        }
    }

    /// A selection with columns but no ROWS is not a refusal: there is simply
    /// nothing to write, and the count says so.
    #[test]
    fn a_selection_with_no_rows_writes_nothing_and_refuses_nothing() {
        let mut selection = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number)],
            &[&["1"]],
        );
        selection.rows.clear();
        for built in [
            build_sql_inserts(&selection),
            build_sql_updates(&selection, &["ID".to_string()]),
            build_where_clause(&selection),
        ] {
            assert_eq!(built.refusal(), None);
            assert!(built.text().is_empty());
            assert_eq!(built.rows(), 0);
        }
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
            written(build_sql_updates(&selection, &["ID".to_string()])),
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
            written(build_sql_updates(
                &selection,
                &["PART".to_string(), "SEQ".to_string()]
            )),
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
            written(build_sql_updates(&selection, &["ID".to_string()])),
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
            written(build_sql_updates(&selection, &[])),
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
            written(build_sql_updates(&selection, &["ID".to_string()])),
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
            written(build_sql_updates(&selection, &["ID".to_string()])),
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
            written(build_sql_updates(&selection, &["ID".to_string()])),
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
        assert_eq!(written(build_where_clause(&selection)), "ID = 7369");
    }

    #[test]
    fn where_clause_of_one_column_collapses_into_in() {
        let selection = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number)],
            &[&["7369"], &["7499"], &["7521"]],
        );
        assert_eq!(
            written(build_where_clause(&selection)),
            "ID IN (7369, 7499, 7521)"
        );
    }

    #[test]
    fn where_clause_of_one_column_deduplicates_values() {
        let selection = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number)],
            &[&["7369"], &["7369"], &["7499"]],
        );
        assert_eq!(
            written(build_where_clause(&selection)),
            "ID IN (7369, 7499)"
        );
    }

    #[test]
    fn where_clause_lifts_nulls_out_of_the_in_list() {
        let selection = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number)],
            &[&["7369"], &["NULL"], &["7499"]],
        );
        assert_eq!(
            written(build_where_clause(&selection)),
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
        assert_eq!(written(build_where_clause(&selection)), "ID IS NULL");
    }

    #[test]
    fn where_clause_of_one_row_ands_the_columns() {
        let selection = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number), ("NAME", SqlValueKind::String)],
            &[&["7369", "SMITH"]],
        );
        assert_eq!(
            written(build_where_clause(&selection)),
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
            written(build_where_clause(&selection)),
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
            written(build_where_clause(&selection)),
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
            written(build_sql_inserts(&selection)),
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
            written(build_sql_updates(&selection, &["id".to_string()])),
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
            written(build_sql_inserts(&selection)),
            "INSERT INTO HR.EMP (\"odd name\") VALUES ('x');\n"
        );
    }

    /// `NO_BACKSLASH_ESCAPES` is a per-CONNECTION setting of this app, so the
    /// writer has to ask the connection and not the family.
    ///
    /// A session running under that mode stores a doubled backslash as two
    /// characters, so a writer that assumed the server default rewrote the
    /// data on its way out.
    #[test]
    fn a_connection_decides_whether_a_backslash_escapes() {
        let mut info =
            ConnectionInfo::new_with_type("m", "u", "p", "h", 3306, "db", DatabaseType::MySQL);
        info.advanced.mysql_sql_mode = "TRADITIONAL".to_string();
        assert_eq!(
            sql_literal_for_value(
                SqlWriteDialect::for_connection(&info),
                SqlValueKind::String,
                r"a\b"
            )
            .expect("a short value fits in one literal"),
            r"'a\\b'",
            "the default doubles"
        );
        info.advanced.mysql_sql_mode = "TRADITIONAL,NO_BACKSLASH_ESCAPES".to_string();
        assert_eq!(
            sql_literal_for_value(
                SqlWriteDialect::for_connection(&info),
                SqlValueKind::String,
                r"a\b"
            )
            .expect("a short value fits in one literal"),
            r"'a\b'",
            "this connection does not"
        );
        // Oracle has no such escape whatever the mode says.
        let oracle =
            ConnectionInfo::new_with_type("o", "u", "p", "h", 1521, "s", DatabaseType::Oracle);
        assert_eq!(
            SqlWriteDialect::for_connection(&oracle).doubles_a_backslash(),
            Some(false)
        );
    }

    /// Only the explicit token turns it off; none of the compound modes this
    /// app offers includes it.
    ///
    /// The rule moved into `db::session_backslash_rule_for_sql_mode`, which is
    /// now the ONE reader of a `sql_mode` text: the connection's configured mode
    /// and a `SET` a user types in a tab are the same question asked of two
    /// sources, and two readers would answer them differently.
    #[test]
    fn only_the_named_mode_turns_backslash_escaping_off() {
        use crate::db::session_backslash_rule_for_sql_mode as rule_for;
        for mode in ["TRADITIONAL", "ANSI", "ANSI_QUOTES", "ORACLE", ""] {
            assert_eq!(
                rule_for(mode),
                SessionBackslashRule::Escapes,
                "{mode} does not include NO_BACKSLASH_ESCAPES"
            );
        }
        for mode in [
            "NO_BACKSLASH_ESCAPES",
            "traditional, no_backslash_escapes",
            "ANSI,NO_BACKSLASH_ESCAPES,ONLY_FULL_GROUP_BY",
        ] {
            assert_eq!(rule_for(mode), SessionBackslashRule::Literal, "{mode}");
        }
    }

    /// An `&` in the data is data, not a substitution variable.
    ///
    /// This app's own `DEFINE` is on by default and substitutes `&name` inside
    /// string literals the way SQL*Plus does, so a `SQL Inserts` export the
    /// user re-runs — or an import of a file holding `AT&T` — would stop and
    /// ask for a value. Lifting it out as `CHR(38)` stores the identical text
    /// and leaves the session's setting alone.
    #[test]
    fn an_ampersand_cannot_become_a_substitution_variable() {
        assert_eq!(
            oracle_literal(SqlValueKind::String, "AT&T"),
            "'AT'||CHR(38)||'T'"
        );
        // A value that is nothing but `&` has no literal parts at all.
        assert_eq!(oracle_literal(SqlValueKind::String, "&"), "CHR(38)");
        // Applying it twice is inert: the first pass leaves no `&` behind.
        assert_eq!(
            defuse_substitution(
                SqlWriteDialect::family_default(DatabaseType::Oracle),
                "'AT'||CHR(38)||'T'"
            ),
            "'AT'||CHR(38)||'T'"
        );
        // The MySQL family has no substitution to defuse.
        assert_eq!(mysql_literal(SqlValueKind::String, "AT&T"), "'AT&T'");
    }

    /// A MySQL-family name that another quoter already quoted must not be
    /// quoted a second time into a DIFFERENT name.
    #[test]
    fn a_mysql_qualified_name_is_quoted_once() {
        assert_eq!(
            quote_qualified_name(DatabaseType::MySQL, "app.orders"),
            "`app`.`orders`"
        );
        // Idempotent.
        assert_eq!(
            quote_qualified_name(DatabaseType::MySQL, "`app`.`orders`"),
            "`app`.`orders`"
        );
        // A dot INSIDE a quoted segment is part of the name, not a separator.
        assert_eq!(
            quote_qualified_name(DatabaseType::MariaDB, "`sales.ops`.`order.items`"),
            "`sales.ops`.`order.items`"
        );
        // A doubled backtick is one character of the name.
        assert_eq!(
            quote_qualified_name(DatabaseType::MySQL, "app.`zr``tick`"),
            "`app`.`zr``tick`"
        );
    }

    /// Text the app did not author never reaches an unquoted position.
    ///
    /// `HEXTORAW` is the only conversion whose argument is not a format model
    /// the writer chose, so it is the only one a VALUE can reach — and this
    /// writer serves the IMPORT path, where the value is whatever a file said.
    /// Only text that is provably hex may be spliced in; everything else is a
    /// quoted string, which Oracle reads with the same implicit hex conversion
    /// and rejects with the same ORA-01465.
    #[test]
    fn a_binary_value_that_is_not_provably_hex_is_quoted() {
        assert_eq!(
            oracle_literal(SqlValueKind::Binary, "DEADBEEF"),
            "HEXTORAW('DEADBEEF')"
        );
        assert_eq!(
            oracle_literal(SqlValueKind::Binary, "  00ff  "),
            "HEXTORAW('00ff')"
        );
        for value in [
            "hello",     // not hex at all
            "ABC",       // odd digit count: ORA-01465
            "",          // nothing to convert
            "[LOB]",     // a placeholder, not bytes
            "DEAD BEEF", // a space is not a hex digit
            "0x41",      // an `x` is not a hex digit
        ] {
            let literal = oracle_literal(SqlValueKind::Binary, value);
            assert!(
                literal.starts_with('\'') && literal.ends_with('\''),
                "{value:?} produced {literal}"
            );
        }
        // A quote in the value can no longer leave the literal...
        assert_eq!(oracle_literal(SqlValueKind::Binary, "a'b"), "'a''b'");
        // ...and an `&` is defused now that the fallback is a plain literal,
        // which it never was inside `HEXTORAW(…)`.
        assert_eq!(
            oracle_literal(SqlValueKind::Binary, "A&B"),
            "'A'||CHR(38)||'B'"
        );
        assert_eq!(mysql_literal(SqlValueKind::Binary, "abc"), "'abc'");
        assert_eq!(mysql_literal(SqlValueKind::Binary, "a'b"), "'a''b'");
    }

    /// No kind, and no value, may add a statement to the script it lands in.
    ///
    /// The property the `HEXTORAW` hole broke, stated for every kind at once so
    /// a kind added later is covered by the same rule: put the literal in the
    /// statement it is written for, split it with the app's OWN script
    /// splitter — the one an import script actually goes through — and require
    /// exactly one statement back. A value that closes its own call and starts
    /// a statement of its own is what this catches: it used to yield three.
    #[test]
    fn no_value_can_add_a_statement_to_the_script_it_lands_in() {
        use crate::db::query::QueryExecutor;

        let hostile = [
            "41')) SELECT * FROM DUAL; DROP TABLE victim; --",
            "x'); DROP TABLE victim; --",
            "1); DROP TABLE victim; INSERT INTO t (a) VALUES (2",
            "a'b",
            "'; DELETE FROM t; --",
            "a\\'; DELETE FROM t; --",
            "AT&T",
            "line1\nline2\n/\nDROP TABLE victim;",
            "]]>--",
            "0102FF",
            "DEADBEEF",
        ];
        for db_type in [
            DatabaseType::Oracle,
            DatabaseType::MySQL,
            DatabaseType::MariaDB,
        ] {
            let dialect = SqlWriteDialect::family_default(db_type);
            for kind in [
                SqlValueKind::Unknown,
                SqlValueKind::String,
                SqlValueKind::Number,
                SqlValueKind::Boolean,
                SqlValueKind::Temporal,
                SqlValueKind::Binary,
            ] {
                for value in hostile {
                    let literal = sql_literal_for_value(dialect, kind, value)
                        .expect("every hostile value here is short enough for one literal");
                    let script = format!("INSERT INTO t (c) VALUES ({literal});\n");
                    let items = QueryExecutor::split_script_items(&script);
                    assert_eq!(
                        items.len(),
                        1,
                        "{db_type} {kind:?} {value:?} produced {} statements: {script}",
                        items.len()
                    );
                }
            }
        }
    }

    /// A value longer than one Oracle literal is written in as many as it takes.
    ///
    /// `ORA-01704: string literal too long` is what a single literal past 4000
    /// bytes gets — measured on Oracle 23ai, where 4000 went in and 4001 did
    /// not — whatever the target column is, a `CLOB` included. No batch size
    /// helps, because the limit is on one VALUE.
    #[test]
    fn a_long_oracle_text_value_is_written_in_several_literals() {
        // Everything that fits is one literal, byte for byte what it always was.
        assert_eq!(oracle_literal(SqlValueKind::String, "short"), "'short'");
        // The limit is on the literal's CONTENT — measured on Oracle 23ai,
        // where 4000 characters went in and 4001 did not — so 4000 is still one
        // literal and one more character is not.
        let at_limit = "x".repeat(ORACLE_MAX_LITERAL_BYTES);
        assert_eq!(
            oracle_literal(SqlValueKind::String, &at_limit),
            format!("'{at_limit}'")
        );
        assert!(
            oracle_literal(SqlValueKind::String, &format!("{at_limit}x")).starts_with("TO_CLOB('")
        );

        let long = "y".repeat(10_000);
        let literal = oracle_literal(SqlValueKind::String, &long);
        assert!(literal.starts_with("TO_CLOB('"), "{}", &literal[..40]);
        assert_eq!(
            literal.matches("TO_CLOB(").count(),
            3,
            "10000 / 4000 rounds to 3"
        );
        for piece in literal.split("||") {
            assert!(
                piece.len() <= "TO_CLOB('')".len() + super::ORACLE_MAX_LITERAL_BYTES,
                "a piece is longer than one literal may be: {}",
                piece.len()
            );
        }
        // And it still says the same thing.
        assert_eq!(
            literal
                .split("||")
                .map(|piece| piece
                    .trim_start_matches("TO_CLOB('")
                    .trim_end_matches("')")
                    .to_string())
                .collect::<String>(),
            long
        );

        // A quote is DOUBLED on the way in, so the ESCAPED length is what the
        // limit applies to — a value of nothing but quotes must still fit.
        let quotes = "'".repeat(4000);
        for piece in oracle_literal(SqlValueKind::String, &quotes).split("||") {
            assert!(
                piece.len() <= "TO_CLOB('')".len() + super::ORACLE_MAX_LITERAL_BYTES,
                "an escaped piece overflowed: {}",
                piece.len()
            );
        }

        // An `&` inside a piece is defused per piece: the whole expression is
        // not a plain literal, so the outer defuser cannot reach into it.
        let ampersand = format!("{}&T", "z".repeat(5000));
        let literal = oracle_literal(SqlValueKind::String, &ampersand);
        assert!(
            literal.contains("CHR(38)"),
            "{}",
            &literal[literal.len() - 60..]
        );
        assert!(!literal.contains('&'), "an ampersand survived the defusing");

        // The MySQL family has no per-literal limit; what bites there is the
        // packet, which the batch size bounds.
        assert_eq!(
            mysql_literal(SqlValueKind::String, &long),
            format!("'{long}'")
        );
    }

    /// A value no single literal can carry is refused BY NAME, not sent.
    ///
    /// Text has a way out and everything else does not: a `BLOB` reaches the
    /// grid as hex, and `HEXTORAW` is a call around a literal rather than a way
    /// past one. Measured on Oracle 23ai through both drivers — a 3000-byte
    /// `BLOB` is 6000 hex characters, and the `INSERT` this used to write
    /// answered `ORA-01704: string literal too long` after the export had
    /// already claimed to be a file.
    #[test]
    fn a_value_too_long_for_any_literal_is_refused_by_name() {
        let oracle = SqlWriteDialect::family_default(DatabaseType::Oracle);
        let mysql = SqlWriteDialect::family_default(DatabaseType::MySQL);

        // A full RAW(2000) is exactly 4000 hex digits and the server takes it,
        // so the writer must too — the limit is on the literal's CONTENT.
        let at_limit = "AB".repeat(ORACLE_MAX_LITERAL_BYTES / 2);
        assert_eq!(at_limit.len(), ORACLE_MAX_LITERAL_BYTES);
        assert_eq!(
            sql_literal_for_value(oracle, SqlValueKind::Binary, &at_limit),
            Ok(format!("HEXTORAW('{at_limit}')"))
        );

        // One hex pair further is a value no statement can carry.
        let too_long = format!("{at_limit}CD");
        assert_eq!(
            sql_literal_for_value(oracle, SqlValueKind::Binary, &too_long),
            Err(ValueCannotBeWritten::TooLongForOneLiteral {
                literal_bytes: too_long.len()
            })
        );
        // Which is what a BLOB looks like: not hex-shaped, still too long.
        let blob_text = "z".repeat(6000);
        for kind in [
            SqlValueKind::Unknown,
            SqlValueKind::Binary,
            SqlValueKind::Number,
            SqlValueKind::Boolean,
            SqlValueKind::Temporal,
        ] {
            assert!(
                sql_literal_for_value(oracle, kind, &blob_text).is_err(),
                "{kind:?} wrote a literal the server cannot take"
            );
            // The MySQL family has no per-literal limit at all.
            assert!(
                sql_literal_for_value(mysql, kind, &blob_text).is_ok(),
                "{kind:?} was refused on a family that has no such limit"
            );
        }
        // Text is the one kind with a way out, and keeps it.
        assert!(sql_literal_for_value(oracle, SqlValueKind::String, &blob_text).is_ok());

        // A conversion call wraps a literal rather than replacing one, so the
        // limit reaches inside it. Nothing bounds how many digits of fractional
        // second a FILE carries; the drivers write at most nine.
        let long_fraction = format!("2024-01-01 00:00:00.{}", "1".repeat(5000));
        assert!(sql_literal_for_value(oracle, SqlValueKind::Temporal, &long_fraction).is_err());
        // The zone rides inside the same literal, so it counts toward the same
        // limit: measuring the datetime alone let a value through by seven.
        let at_limit_fraction = format!(
            "2024-01-01 00:00:00.{}",
            "1".repeat(ORACLE_MAX_LITERAL_BYTES - "2024-01-01 00:00:00.".len())
        );
        assert!(sql_literal_for_value(oracle, SqlValueKind::Temporal, &at_limit_fraction).is_ok());
        assert!(
            sql_literal_for_value(
                oracle,
                SqlValueKind::Temporal,
                &format!("{at_limit_fraction}+09:00")
            )
            .is_err(),
            "the zone pushes the same value past the limit"
        );
        assert_eq!(
            sql_literal_for_value(oracle, SqlValueKind::Temporal, "2024-01-01 00:00:00.123456"),
            Ok("TO_TIMESTAMP('2024-01-01 00:00:00.123456','YYYY-MM-DD HH24:MI:SS.FF')".to_string())
        );

        // The sentence names the row and the column, which is the only thing
        // that tells the user which value to look at.
        let message = value_too_long_message(
            "PHOTO",
            7,
            ValueCannotBeWritten::TooLongForOneLiteral {
                literal_bytes: 6000,
            },
        );
        assert!(message.contains("Row 7"), "{message}");
        assert!(message.contains("PHOTO"), "{message}");
        assert!(message.contains("6000"), "{message}");
        assert!(message.contains("4000"), "{message}");
    }

    /// And a builder handed one refuses the WHOLE build, naming the cell.
    #[test]
    fn a_builder_refuses_a_whole_build_for_one_value_it_cannot_write() {
        let mut selection = selection(
            DatabaseType::Oracle,
            &[
                ("ID", SqlValueKind::Number),
                ("PHOTO", SqlValueKind::Unknown),
            ],
            &[&["1", "short"], &["2", "short"]],
        );
        selection.rows[1][1] = Some("z".repeat(6000));

        for built in [
            build_sql_inserts(&selection),
            build_sql_updates(&selection, &["ID".to_string()]),
            build_where_clause(&selection),
        ] {
            let reason = built.refusal().expect("the second row cannot be written");
            assert!(reason.contains("Row 2"), "{reason}");
            assert!(reason.contains("PHOTO"), "{reason}");
            // Nothing partial reaches a file or the clipboard.
            assert!(built.text().is_empty(), "a refused build still wrote text");
        }

        // The same selection on the MySQL family writes every row: what bites
        // there is the packet, which the import batch size bounds.
        let mut mysql = selection.clone();
        mysql.dialect = SqlWriteDialect::family_default(DatabaseType::MySQL);
        assert_eq!(build_sql_inserts(&mysql).rows(), 2);
    }

    /// A name that belongs to two columns cannot be written as SQL at all.
    ///
    /// `SELECT a.id, b.id` is an ordinary query and every driver reports both
    /// columns as `ID`. Written out, one of the three shapes is refused by the
    /// server and the other two RUN while meaning something nobody asked for —
    /// measured on Oracle 23ai and MySQL 8:
    /// `UPDATE t SET ID = 2 WHERE ID = 1` reported one row updated, and
    /// `WHERE ID = 1 AND ID = 2` matched none.
    #[test]
    fn a_name_that_belongs_to_two_columns_is_refused() {
        let ambiguous = selection(
            DatabaseType::Oracle,
            &[("ID", SqlValueKind::Number), ("ID", SqlValueKind::Number)],
            &[&["1", "2"]],
        );
        let reason = ambiguous
            .ambiguous_column_refusal(&[])
            .expect("two selected columns named ID");
        assert!(
            reason.contains("ID"),
            "the sentence names the column: {reason}"
        );

        // And nothing is written, so a caller that forgot to ask gets nothing
        // rather than SQL that is refused — or worse, SQL that runs. The gate
        // is inside the builders now, so what comes back is the same sentence
        // rather than an empty string the caller cannot read.
        for built in [
            build_sql_inserts(&ambiguous),
            build_where_clause(&ambiguous),
            build_sql_updates(&ambiguous, &["ID".to_string()]),
        ] {
            assert_eq!(built.refusal(), Some(reason.as_str()));
            assert!(built.text().is_empty());
        }

        // Selecting only ONE of the two is not ambiguous, and still works.
        let mut one = ambiguous.clone();
        one.selected_columns = vec![0];
        assert_eq!(one.ambiguous_column_refusal(&[]), None);
        assert_eq!(
            written(build_sql_inserts(&one)),
            "INSERT INTO HR.EMP (ID) VALUES (1);\n"
        );

        // But a KEY looked up by that name still is: the lookup takes the first
        // match, and the two columns do not have to hold the same value.
        assert!(one.ambiguous_column_refusal(&["id".to_string()]).is_some());
        let keyed = build_sql_updates(&one, &["id".to_string()]);
        assert!(keyed.refusal().is_some());
        assert!(keyed.text().is_empty());
        // A key that names one column is fine.
        let plain = selection(
            DatabaseType::MySQL,
            &[("ID", SqlValueKind::Number), ("NAME", SqlValueKind::String)],
            &[&["1", "x"]],
        );
        assert_eq!(plain.ambiguous_column_refusal(&["ID".to_string()]), None);
        assert!(!written(build_sql_updates(&plain, &["ID".to_string()])).is_empty());
    }

    #[test]
    fn a_repeated_name_is_found_the_way_sql_resolves_one() {
        assert_eq!(
            first_repeated_column_name(["A", "b", " a "]),
            Some("a".to_string()),
            "unquoted identifiers are matched case-insensitively, and trimmed"
        );
        assert_eq!(first_repeated_column_name(["A", "B", "C"]), None);
        assert_eq!(first_repeated_column_name([]), None);
    }

    /// A `Where Clause` of a big selection is LINEAR in its rows.
    ///
    /// Both shapes de-duplicate — row groups on the multi-column road, values
    /// in the `IN` list on the single-column one — and both used to ask
    /// `Vec::contains`, which is a scan per row. Measured before the set:
    /// 2 000 rows 35 ms, 4 000 128 ms, 8 000 473 ms, 16 000 1.73 s, a clean x4
    /// per doubling, extrapolating to about a minute for a hundred thousand.
    /// It runs on the UI thread under the app-state lock, and `Ctrl+A` plus one
    /// menu pick is all it takes to ask for it.
    #[test]
    fn a_where_clause_of_a_big_selection_stays_linear() {
        let build = |rows: usize, columns: usize| {
            let selection = GridSqlSelection {
                dialect: SqlWriteDialect::family_default(DatabaseType::Oracle),
                table: Some("HR.EMP".to_string()),
                all_columns: (0..columns).map(|c| format!("C{c}")).collect(),
                column_kinds: vec![SqlValueKind::String; columns],
                selected_columns: (0..columns).collect(),
                // Every value distinct, which is the worst case for a dedup
                // that scans: nothing is ever found early.
                rows: (0..rows)
                    .map(|r| (0..columns).map(|c| Some(format!("v{r}-{c}"))).collect())
                    .collect(),
            };
            let start = std::time::Instant::now();
            let built = build_where_clause(&selection);
            assert_eq!(built.rows(), rows);
            (start.elapsed().as_secs_f64(), built.text().len())
        };

        for columns in [1usize, 3] {
            // Warm the allocator so the first measurement is not the odd one.
            let _ = build(4_000, columns);
            let (small, small_bytes) = build(4_000, columns);
            let (large, large_bytes) = build(8_000, columns);
            assert!(
                large_bytes > small_bytes,
                "{columns} column(s): the fixture must really grow"
            );
            assert!(
                large < small * 2.5 + 0.05,
                "{columns} column(s): twice the rows took {large:.4}s against {small:.4}s for \
                 half — that is the scan-per-row dedup, not linear work"
            );
        }
    }

    /// A session whose escaping rule is UNKNOWN refuses exactly the values whose
    /// meaning depends on it, and writes every other one as before.
    ///
    /// The rule is a property of the SESSION, and this app only knows the one it
    /// configured: a user's own `SET SESSION sql_mode` moves the tab's session
    /// out from under every writer, and a value this reader cannot resolve
    /// leaves it genuinely unnamed. A backslash is the one character that means
    /// two different things then — so it is refused, by name, and nothing else
    /// is.
    #[test]
    fn an_unknown_escaping_rule_refuses_only_a_value_that_depends_on_it() {
        let unknown = |db_type| {
            let mut info = ConnectionInfo::new_with_type("m", "u", "p", "h", 3306, "db", db_type);
            info.advanced.mysql_sql_mode = "TRADITIONAL".to_string();
            SqlWriteDialect::for_session(&info, Some(SessionBackslashRule::Unknown))
        };
        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            let dialect = unknown(db_type);
            // Nothing else changes: a value with no backslash is spelled the
            // same under either rule.
            assert_eq!(
                sql_literal_for_value(dialect, SqlValueKind::String, "it's plain"),
                Ok("'it''s plain'".to_string()),
                "{db_type}"
            );
            assert_eq!(
                sql_literal_for_value(dialect, SqlValueKind::Number, "42"),
                Ok("42".to_string()),
                "{db_type}"
            );
            // And the one that does is refused rather than guessed.
            assert_eq!(
                sql_literal_for_value(dialect, SqlValueKind::String, r"C:\path"),
                Err(ValueCannotBeWritten::UnknownBackslashRule),
                "{db_type}"
            );
            let message =
                value_too_long_message("PATH", 3, ValueCannotBeWritten::UnknownBackslashRule);
            assert!(message.contains("Row 3"), "{message}");
            assert!(message.contains("PATH"), "{message}");
            assert!(message.contains("sql_mode"), "{message}");
            // A builder handed one refuses the WHOLE build, the way it does for
            // a value no literal can hold.
            let mut selection =
                selection(db_type, &[("V", SqlValueKind::String)], &[&[r"C:\path"]]);
            selection.dialect = dialect;
            assert!(
                build_sql_inserts(&selection)
                    .refusal()
                    .is_some_and(|reason| reason.contains("PATH") || reason.contains("V")),
                "{db_type}: the build must refuse and name the cell"
            );
        }

        // A KNOWN rule is unaffected, whichever it is.
        for (mode, expected) in [
            ("TRADITIONAL", r"'C:\\path'"),
            ("TRADITIONAL,NO_BACKSLASH_ESCAPES", r"'C:\path'"),
        ] {
            let mut info =
                ConnectionInfo::new_with_type("m", "u", "p", "h", 3306, "db", DatabaseType::MySQL);
            info.advanced.mysql_sql_mode = mode.to_string();
            assert_eq!(
                sql_literal_for_value(
                    SqlWriteDialect::for_connection(&info),
                    SqlValueKind::String,
                    r"C:\path"
                ),
                Ok(expected.to_string()),
                "{mode}"
            );
        }
        // Oracle never has an unknown rule: it has no backslash escape at all.
        let oracle =
            ConnectionInfo::new_with_type("o", "u", "p", "h", 1521, "s", DatabaseType::Oracle);
        assert_eq!(
            SqlWriteDialect::for_session(&oracle, Some(SessionBackslashRule::Unknown))
                .doubles_a_backslash(),
            Some(false)
        );
    }

    /// A VALUE cannot tell the reader what escaping rule the file follows.
    ///
    /// The lesson round 7 taught the other marks this app writes: a statement's
    /// text INCLUDES its values, so a mark matched anywhere is a mark a value
    /// can forge. This one is read only from the comments that come BEFORE the
    /// first statement, where a value cannot be — and only from a comment that
    /// DECLARES the mode, so prose that merely names it says nothing.
    #[test]
    fn only_a_leading_declaration_says_how_a_file_escapes() {
        let declaration = sql_inserts_dialect_preamble(SqlWriteDialect::for_connection(&{
            let mut info =
                ConnectionInfo::new_with_type("m", "u", "p", "h", 3306, "db", DatabaseType::MySQL);
            info.advanced.mysql_sql_mode = "NO_BACKSLASH_ESCAPES".to_string();
            info
        }));
        assert!(!declaration.is_empty(), "the writer declares the mode");
        assert!(sql_file_declares_no_backslash_escapes(declaration));
        // Whitespace and other leading comments in front of it are still ahead
        // of every statement.
        assert!(sql_file_declares_no_backslash_escapes(&format!(
            "\n-- exported by this app\n/* two */\n{declaration}INSERT INTO `t` (`a`) VALUES (1);\n"
        )));

        for forged in [
            // The mark inside a VALUE, which is what a forgery would be.
            "INSERT INTO `t` (`a`) VALUES ('-- sql_mode: NO_BACKSLASH_ESCAPES');\n",
            // In a comment, but AFTER a statement has already been read.
            "INSERT INTO `t` (`a`) VALUES (1);\n-- sql_mode: NO_BACKSLASH_ESCAPES\n",
            // A leading comment that mentions the mode without declaring it.
            "-- this file does not use NO_BACKSLASH_ESCAPES\nINSERT INTO `t` (`a`) VALUES (1);\n",
            // A declaration of some OTHER mode.
            "-- sql_mode: NO_AUTO_VALUE_ON_ZERO\nINSERT INTO `t` (`a`) VALUES (1);\n",
            // A longer name that merely starts the same way.
            "-- sql_mode: NO_BACKSLASH_ESCAPES_OFF\nINSERT INTO `t` (`a`) VALUES (1);\n",
            "",
        ] {
            assert!(
                !sql_file_declares_no_backslash_escapes(forged),
                "a file must not be read as NO_BACKSLASH_ESCAPES for: {forged:?}"
            );
        }
    }

    /// A name the SERVER reported has to name the SAME object again.
    ///
    /// Stated as the property rather than as a spelling table: reading the
    /// written identifier back the way the backend's parser reads one must give
    /// the original name. Oracle folds a bare identifier to upper case, so
    /// anything that is not already a legal upper-case word — a column declared
    /// `"id"`, a reserved word, a name with a space or a non-ASCII letter — has
    /// to come back quoted.
    ///
    /// Before this, every such name was written BARE, because the quoter used
    /// here is the one that suits a name PARSED out of the user's SQL. A table
    /// with an `"id"` column therefore answered `ORA-00904` to every statement
    /// this module writes and to every file import, on both Oracle drivers,
    /// while the MySQL family was unaffected — which is exactly the kind of
    /// backend split this app exists to not have.
    #[test]
    fn a_reported_name_is_written_so_the_server_names_it_again() {
        /// What a backend's parser makes of ONE written identifier.
        fn denoted(db_type: DatabaseType, written: &str) -> String {
            if db_type.is_mysql_or_mariadb() {
                let inner = written
                    .strip_prefix('`')
                    .and_then(|rest| rest.strip_suffix('`'))
                    .unwrap_or_else(|| {
                        panic!("the MySQL family always backticks a reported name: {written}")
                    });
                return inner.replace("``", "`");
            }
            match written
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
            {
                Some(inner) => inner.replace("\"\"", "\""),
                // Oracle folds an unquoted identifier to upper case.
                None => written.to_uppercase(),
            }
        }

        for name in [
            "ID", // already upper case: stays bare on Oracle
            "id", // declared `"id"`: only quotes name it again
            "Id", "COMMENT", // reserved: bare is a syntax error
            "SELECT", "odd name",
            "MY$COL", // `$` and `#` are legal in a bare Oracle identifier
            "가격",   // a non-ASCII letter cannot start a bare one
        ] {
            for db_type in [
                DatabaseType::Oracle,
                DatabaseType::MySQL,
                DatabaseType::MariaDB,
            ] {
                let written = quote_column_name(db_type, name);
                assert_eq!(
                    denoted(db_type, &written),
                    name,
                    "{db_type} wrote {name:?} as {written}, which names something else"
                );
            }
        }
        // The readable spelling is kept where it costs nothing.
        assert_eq!(quote_column_name(DatabaseType::Oracle, "ID"), "ID");
        assert_eq!(quote_column_name(DatabaseType::Oracle, "id"), "\"id\"");
    }

    /// The grid-edit descriptor names a table the way the SERVER does; the SQL
    /// the grid came from names it the way the USER typed it. One quoter cannot
    /// serve both, so the resolver spells each before anything downstream sees
    /// it.
    #[test]
    fn a_resolved_table_is_spelled_for_where_the_name_came_from() {
        // Reported: quoted exactly, because `emp` would name `EMP`.
        assert_eq!(
            resolve_export_table(
                DatabaseType::Oracle,
                Some(("HR".to_string(), "emp".to_string())),
                "",
            ),
            Some("HR.\"emp\"".to_string())
        );
        assert_eq!(
            resolve_export_table(
                DatabaseType::MySQL,
                Some(("app".to_string(), "orders".to_string())),
                "",
            ),
            Some("`app`.`orders`".to_string())
        );
        // Parsed out of the user's SQL: left to fold the way the parser folds
        // it. Quoting `emp` here would name a table that does not exist.
        assert_eq!(
            resolve_export_table(DatabaseType::Oracle, None, "SELECT * FROM hr.emp"),
            Some("hr.emp".to_string())
        );
        // A descriptor with no table name is not a name; the SQL still answers.
        assert_eq!(
            resolve_export_table(
                DatabaseType::Oracle,
                Some(("HR".to_string(), "  ".to_string())),
                "SELECT * FROM hr.emp",
            ),
            Some("hr.emp".to_string())
        );
        // Neither: the placeholder.
        assert_eq!(resolve_export_table(DatabaseType::Oracle, None, ""), None);
    }

    /// A statement that WRITES never names a column the server computes; one
    /// that READS keeps every column.
    ///
    /// The rule used to be applied by the object tree's export alone, so the
    /// same table exported two different scripts and only the tree's could run:
    /// `INSERT`/`UPDATE` into a virtual or always-generated column is
    /// `ORA-54013`/`ORA-54017` on Oracle and 3105 on the MySQL family. Applying
    /// it through one method on the selection is what keeps the roads together.
    #[test]
    fn only_the_statements_that_write_drop_a_computed_column() {
        for db_type in [
            DatabaseType::Oracle,
            DatabaseType::MySQL,
            DatabaseType::MariaDB,
        ] {
            let full = selection(
                db_type,
                &[
                    ("ID", SqlValueKind::Number),
                    ("TOTAL", SqlValueKind::Number),
                    ("NAME", SqlValueKind::String),
                ],
                &[&["1", "9", "kim"]],
            );
            // `Where Clause` reads, so it is never narrowed and still names it.
            assert!(
                written(build_where_clause(&full)).contains("TOTAL"),
                "{db_type}: a WHERE clause reads every selected column"
            );

            let mut narrowed = full.clone();
            // Matched the way SQL resolves an unquoted name, and trimmed, which
            // is how the catalog's spelling is paired with the result's.
            narrowed.restrict_to_writable_columns(&[" total ".to_string()]);
            let inserts = written(build_sql_inserts(&narrowed));
            assert!(
                !inserts.contains("TOTAL") && inserts.contains("ID") && inserts.contains("NAME"),
                "{db_type}: an INSERT must not name a computed column: {inserts}"
            );
            let updates = written(build_sql_updates(&narrowed, &["ID".to_string()]));
            assert!(
                !updates.contains("TOTAL") && updates.contains("NAME"),
                "{db_type}: an UPDATE must not set a computed column: {updates}"
            );

            // A key column is read from `all_columns`, which narrowing leaves
            // alone, so the WHERE still finds its value.
            assert!(
                updates.contains("WHERE"),
                "{db_type}: narrowing must not cost the statement its key"
            );

            // Nothing computed narrows nothing.
            let mut untouched = full.clone();
            untouched.restrict_to_writable_columns(&[]);
            assert_eq!(untouched.selected_columns, full.selected_columns);

            // Every column computed leaves a statement no column to name, and
            // the builders say so rather than writing an empty file.
            let mut nothing_writable = full.clone();
            nothing_writable.restrict_to_writable_columns(&[
                "ID".to_string(),
                "TOTAL".to_string(),
                "NAME".to_string(),
            ]);
            assert_eq!(
                build_sql_inserts(&nothing_writable).refusal(),
                Some(no_writable_column_message().as_str())
            );
        }
    }

    /// A column the server computes is never named by `SQL Inserts`.
    #[test]
    fn generated_columns_are_not_writable_targets() {
        let columns = vec![
            "A".to_string(),
            "TOTAL".to_string(),
            "ID".to_string(),
            "VIRT".to_string(),
        ];
        assert_eq!(
            writable_column_indices(&columns, &["total".to_string(), " VIRT ".to_string()]),
            vec![0, 2],
            "matched the way SQL resolves an unquoted name"
        );
        // Nothing generated — every format but `SQL Inserts` — keeps every
        // column.
        assert_eq!(writable_column_indices(&columns, &[]), vec![0, 1, 2, 3]);
    }
}
