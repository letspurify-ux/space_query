use std::borrow::Cow;

use crate::db::connection::DatabaseType;
use crate::sql_text;

/// SQL classification for cancel / session-reuse decisions (session.md §6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqlKind {
    SelectLike,
    Dml,
    Ddl,
    SessionControl,
    PlsqlOrProcedure,
    Script,
    TransactionControl,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SqlClassificationProfile {
    Oracle,
    MySqlCompatible,
}

fn classification_profile_for_db_type(db_type: DatabaseType) -> SqlClassificationProfile {
    match db_type {
        DatabaseType::Oracle => SqlClassificationProfile::Oracle,
        DatabaseType::MySQL | DatabaseType::MariaDB => SqlClassificationProfile::MySqlCompatible,
    }
}

impl SqlClassificationProfile {
    fn mysql_compatible_comments(self) -> bool {
        matches!(self, SqlClassificationProfile::MySqlCompatible)
    }
}

impl SqlKind {
    pub fn is_select_like(self) -> bool {
        matches!(self, SqlKind::SelectLike)
    }

    pub fn is_dml_or_ddl_or_plsql_or_script(self) -> bool {
        matches!(
            self,
            SqlKind::Dml | SqlKind::Ddl | SqlKind::PlsqlOrProcedure | SqlKind::Script
        )
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SqlStatementAnalysis<'a> {
    stripped_sql: Cow<'a, str>,
    words: Vec<String>,
    has_multiple_statements: bool,
}

impl<'a> SqlStatementAnalysis<'a> {
    pub(crate) fn new(sql: &'a str) -> Self {
        Self::new_with_options(sql, false, false)
    }

    pub(crate) fn new_for_db_type(db_type: DatabaseType, sql: &'a str) -> Self {
        let profile = classification_profile_for_db_type(db_type);
        let mysql_compatible = profile.mysql_compatible_comments();
        let stripped_sql = strip_leading_comments_and_whitespace_with_mode(sql, mysql_compatible);
        let stripped_sql = if mysql_compatible {
            mysql_sql_with_executable_comments_expanded(stripped_sql)
        } else {
            Cow::Borrowed(stripped_sql)
        };
        let stripped_sql = match db_type {
            DatabaseType::MariaDB => {
                mariadb_set_statement_inner_sql_from_prepared(stripped_sql.as_ref())
                    .map(Cow::Owned)
                    .unwrap_or(stripped_sql)
            }
            DatabaseType::MySQL | DatabaseType::Oracle => stripped_sql,
        };
        Self::from_prepared_sql(stripped_sql, mysql_compatible)
    }

    fn new_with_options(
        sql: &'a str,
        mysql_compatible_comments: bool,
        expand_mysql_executable_comments: bool,
    ) -> Self {
        let stripped_sql =
            strip_leading_comments_and_whitespace_with_mode(sql, mysql_compatible_comments);
        let stripped_sql = if expand_mysql_executable_comments {
            mysql_sql_with_executable_comments_expanded(stripped_sql)
        } else {
            Cow::Borrowed(stripped_sql)
        };
        Self::from_prepared_sql(stripped_sql, mysql_compatible_comments)
    }

    fn from_prepared_sql(stripped_sql: Cow<'a, str>, mysql_compatible_comments: bool) -> Self {
        let words = statement_words(stripped_sql.as_ref(), mysql_compatible_comments);
        let has_multiple_statements =
            contains_multiple_statements(stripped_sql.as_ref(), mysql_compatible_comments);
        Self {
            stripped_sql,
            words,
            has_multiple_statements,
        }
    }

    pub(crate) fn words(&self) -> &[String] {
        &self.words
    }

    pub(crate) fn leading_keyword(&self) -> Option<&str> {
        self.words.first().map(String::as_str)
    }

    pub(crate) fn leading_keyword_owned(&self) -> Option<String> {
        self.leading_keyword().map(str::to_string)
    }

    pub(crate) fn starts_with_words(&self, expected: &[&str]) -> bool {
        self.words.len() >= expected.len()
            && self
                .words
                .iter()
                .zip(expected.iter())
                .all(|(actual, expected)| actual == expected)
    }

    pub(crate) fn classify_for_db_type(&self, db_type: DatabaseType) -> SqlKind {
        if self.stripped_sql.is_empty() {
            return SqlKind::Unknown;
        }

        let first_word = self.leading_keyword().unwrap_or_default();

        match classification_profile_for_db_type(db_type) {
            SqlClassificationProfile::Oracle => match first_word {
                "BEGIN" | "DECLARE" => {
                    // PL/SQL blocks contain internal semicolons, but trailing
                    // SQL after the block must still be treated as a script for
                    // cancel/timeout session policy.
                    return if oracle_plsql_block_has_trailing_statement(self.stripped_sql.as_ref())
                    {
                        SqlKind::Script
                    } else {
                        SqlKind::PlsqlOrProcedure
                    };
                }
                _ => {}
            },
            SqlClassificationProfile::MySqlCompatible => {}
        }

        if self.has_multiple_statements {
            return SqlKind::Script;
        }

        classify_first_word_for_db_type(db_type, first_word, self.stripped_sql.as_ref())
    }
}

pub(crate) fn mariadb_set_statement_inner_sql(sql: &str) -> Option<String> {
    let stripped_sql = strip_leading_comments_and_whitespace_with_mode(sql, true);
    let stripped_sql = mysql_sql_with_executable_comments_expanded(stripped_sql);
    mariadb_set_statement_inner_sql_from_prepared(stripped_sql.as_ref())
}

fn mariadb_set_statement_inner_sql_from_prepared(sql: &str) -> Option<String> {
    let (set_token, _, after_set) = next_top_level_word(sql, 0, true)?;
    if set_token != "SET" {
        return None;
    }

    let (statement_token, _, after_statement) = next_top_level_word(sql, after_set, true)?;
    if statement_token != "STATEMENT" {
        return None;
    }

    let for_start = find_top_level_word(sql, after_statement, "FOR", true)?;
    let inner_sql = sql[for_start + "FOR".len()..].trim();
    (!inner_sql.is_empty()).then(|| inner_sql.to_string())
}

fn find_top_level_word(
    sql: &str,
    mut idx: usize,
    expected: &str,
    mysql_compatible_comments: bool,
) -> Option<usize> {
    while let Some((word, start, after_word)) =
        next_top_level_word(sql, idx, mysql_compatible_comments)
    {
        if word == expected {
            return Some(start);
        }
        idx = after_word;
    }
    None
}

pub(crate) fn strip_leading_comments_and_whitespace(sql: &str) -> &str {
    strip_leading_comments_and_whitespace_with_mode(sql, false)
}

fn strip_leading_comments_and_whitespace_with_mode(
    sql: &str,
    mysql_compatible_comments: bool,
) -> &str {
    let mut remaining = sql;

    loop {
        let trimmed = remaining.trim_start();

        if line_comment_starts(trimmed, mysql_compatible_comments) {
            if let Some(line_end) = trimmed.find('\n') {
                remaining = &trimmed[line_end + 1..];
                continue;
            }
            return "";
        }

        if trimmed.starts_with("/*")
            && !sql_text::is_mysql_executable_comment_start(trimmed.as_bytes(), 0)
        {
            if let Some(block_end) = trimmed.find("*/") {
                remaining = &trimmed[block_end + 2..];
                continue;
            }
            return "";
        }

        return trimmed;
    }
}

fn line_comment_starts(line: &str, mysql_compatible_comments: bool) -> bool {
    let bytes = line.as_bytes();
    if mysql_compatible_comments {
        sql_text::is_mysql_dash_comment_start(bytes, 0)
            || sql_text::is_mysql_hash_comment_start(bytes, 0)
    } else {
        sql_text::is_sqlplus_comment_line(line) || sql_text::is_mysql_hash_comment_line(line)
    }
}

pub(crate) fn mysql_sql_with_executable_comments_expanded(sql: &str) -> Cow<'_, str> {
    let bytes = sql.as_bytes();
    let mut output = None::<String>;
    let mut idx = 0usize;
    let mut last_copied = 0usize;

    while idx < bytes.len() {
        if let Some(next) = skip_q_quote(sql, idx).or_else(|| skip_quoted(sql, idx)) {
            idx = next;
            continue;
        }

        if let Some((body_start, body_end, after_comment)) = mysql_executable_comment_span(sql, idx)
        {
            let expanded = output.get_or_insert_with(|| String::with_capacity(sql.len()));
            expanded.push_str(&sql[last_copied..idx]);
            expanded.push(' ');
            expanded.push_str(&sql[body_start..body_end]);
            expanded.push(' ');
            idx = after_comment;
            last_copied = after_comment;
            continue;
        }

        idx += 1;
    }

    if let Some(mut expanded) = output {
        expanded.push_str(&sql[last_copied..]);
        Cow::Owned(expanded)
    } else {
        Cow::Borrowed(sql)
    }
}

fn mysql_executable_comment_span(sql: &str, idx: usize) -> Option<(usize, usize, usize)> {
    let bytes = sql.as_bytes();
    if !sql_text::is_mysql_executable_comment_start(bytes, idx) {
        return None;
    }

    let mut body_start = if bytes.get(idx + 2) == Some(&b'!') {
        idx + 3
    } else {
        idx + 4
    };
    while bytes
        .get(body_start)
        .is_some_and(|byte| byte.is_ascii_digit())
    {
        body_start += 1;
    }

    let body_end = sql[body_start..]
        .find("*/")
        .map(|offset| body_start + offset)?;
    Some((body_start, body_end, body_end + 2))
}

fn statement_words(sql: &str, mysql_compatible_comments: bool) -> Vec<String> {
    let mut words = Vec::new();
    let mut idx = 0usize;
    while let Some((word, _, after_word)) = next_top_level_word(sql, idx, mysql_compatible_comments)
    {
        words.push(word);
        idx = after_word;
    }
    words
}

fn statement_words_any_depth(sql: &str, mysql_compatible_comments: bool) -> Vec<String> {
    let mut words = Vec::new();
    let mut idx = 0usize;
    while let Some((word, _, after_word)) = next_word_any_depth(sql, idx, mysql_compatible_comments)
    {
        words.push(word);
        idx = after_word;
    }
    words
}

fn classify_first_word_for_db_type(db_type: DatabaseType, first_word: &str, sql: &str) -> SqlKind {
    let (oracle_family, mysql_family) = match classification_profile_for_db_type(db_type) {
        SqlClassificationProfile::Oracle => (true, false),
        SqlClassificationProfile::MySqlCompatible => (false, true),
    };
    let mysql_compatible_comments = mysql_family;
    match first_word {
        "WITH" => classify_with_sql_for_db_type(db_type, sql),
        "SELECT" => classify_select_sql_for_db_type(db_type, sql, mysql_compatible_comments),
        "DESCRIBE" | "DESC" | "SHOW" => SqlKind::SelectLike,
        "EXPLAIN" => classify_explain_sql_for_db_type(db_type, sql),
        "VALUES" | "TABLE" if mysql_family => SqlKind::SelectLike,
        "ANALYZE" | "CHECK" | "CHECKSUM" | "OPTIMIZE" | "REPAIR" if mysql_family => {
            // These MySQL/MariaDB table-maintenance statements return result
            // sets, but they are not read-only SELECTs for cancel/timeout
            // safety; keep result-display routing separate from session reuse.
            SqlKind::Ddl
        }
        "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "REPLACE" => SqlKind::Dml,
        "LOAD" if mysql_family => classify_mysql_load_sql(sql, mysql_compatible_comments),
        "LOAD" => SqlKind::Dml,
        "CREATE" | "DROP" | "TRUNCATE" | "RENAME" | "COMMENT" | "GRANT" | "REVOKE" => SqlKind::Ddl,
        "ALTER" => classify_alter_sql_for_db_type(db_type, sql, mysql_compatible_comments),
        "ANALYZE" | "AUDIT" | "NOAUDIT" | "PURGE" | "FLASHBACK" if oracle_family => SqlKind::Ddl,
        "FLUSH" if mysql_family => SqlKind::Ddl,
        "CACHE" | "INSTALL" | "UNINSTALL" if mysql_family => SqlKind::Ddl,
        "LOCK" if oracle_family => classify_oracle_lock_sql(sql),
        "LOCK" if mysql_family => classify_mysql_lock_sql(sql, mysql_compatible_comments),
        "UNLOCK" if mysql_family => classify_mysql_unlock_sql(sql, mysql_compatible_comments),
        "USE" if mysql_family => SqlKind::SessionControl,
        "CALL" | "EXEC" | "EXECUTE" => SqlKind::PlsqlOrProcedure,
        "DO" if mysql_family => classify_mysql_do_sql(sql, mysql_compatible_comments),
        "BEGIN" => classify_begin_sql_for_db_type(db_type, sql, mysql_compatible_comments),
        "COMMIT" | "ROLLBACK" | "SAVEPOINT" => SqlKind::TransactionControl,
        "RELEASE" if mysql_family => classify_release_sql(sql, mysql_compatible_comments),
        "RELEASE" => SqlKind::Unknown,
        "XA" if mysql_family => classify_xa_sql(sql, mysql_compatible_comments),
        "RESET" if mysql_family => classify_mysql_reset_sql(sql, mysql_compatible_comments),
        "SET" => classify_set_sql_for_db_type(db_type, sql, mysql_compatible_comments),
        "START" => classify_start_sql_for_db_type(db_type, sql, mysql_compatible_comments),
        "STOP" | "CHANGE" if mysql_family => {
            classify_mysql_replication_sql(sql, mysql_compatible_comments)
        }
        _ => SqlKind::Unknown,
    }
}

fn classify_mysql_load_sql(sql: &str, mysql_compatible_comments: bool) -> SqlKind {
    let words = statement_words(sql, mysql_compatible_comments);
    if words.first().is_some_and(|word| word == "LOAD")
        && words.get(1).is_some_and(|word| word == "INDEX")
    {
        SqlKind::Ddl
    } else {
        SqlKind::Dml
    }
}

fn classify_mysql_lock_sql(sql: &str, mysql_compatible_comments: bool) -> SqlKind {
    let words = statement_words(sql, mysql_compatible_comments);
    if words.first().is_some_and(|word| word == "LOCK")
        && matches!(
            words.get(1).map(String::as_str),
            Some("TABLE" | "TABLES" | "INSTANCE")
        )
    {
        SqlKind::SessionControl
    } else {
        SqlKind::Unknown
    }
}

fn classify_mysql_unlock_sql(sql: &str, mysql_compatible_comments: bool) -> SqlKind {
    let words = statement_words(sql, mysql_compatible_comments);
    if words.first().is_some_and(|word| word == "UNLOCK")
        && matches!(
            words.get(1).map(String::as_str),
            Some("TABLE" | "TABLES" | "INSTANCE")
        )
    {
        SqlKind::SessionControl
    } else {
        SqlKind::Unknown
    }
}

fn classify_mysql_reset_sql(sql: &str, mysql_compatible_comments: bool) -> SqlKind {
    let words = statement_words(sql, mysql_compatible_comments);
    if words.first().is_none_or(|word| word != "RESET") {
        return SqlKind::Unknown;
    }

    match words.get(1).map(String::as_str) {
        Some("CONNECTION") => SqlKind::SessionControl,
        Some("PERSIST" | "REPLICA" | "SLAVE" | "MASTER") => SqlKind::Ddl,
        _ => SqlKind::Unknown,
    }
}

fn classify_mysql_do_sql(sql: &str, mysql_compatible_comments: bool) -> SqlKind {
    if mysql_do_statement_is_known_lock_function_only(sql, mysql_compatible_comments) {
        SqlKind::SessionControl
    } else {
        SqlKind::PlsqlOrProcedure
    }
}

fn mysql_do_statement_is_known_lock_function_only(
    sql: &str,
    mysql_compatible_comments: bool,
) -> bool {
    let Some((do_token, _, after_do)) = next_top_level_word(sql, 0, mysql_compatible_comments)
    else {
        return false;
    };
    if do_token != "DO" {
        return false;
    }

    let function_start = skip_ws_and_comments(sql, after_do, mysql_compatible_comments);
    ["GET_LOCK", "RELEASE_LOCK", "RELEASE_ALL_LOCKS"]
        .iter()
        .any(|function_name| {
            mysql_statement_has_single_safe_lock_function_call(
                sql,
                function_start,
                function_name,
                mysql_compatible_comments,
            )
        })
}

fn mysql_statement_has_single_safe_lock_function_call(
    sql: &str,
    function_start: usize,
    function_name: &str,
    mysql_compatible_comments: bool,
) -> bool {
    let Some(after_name) = consume_keyword_at(sql, function_start, function_name) else {
        return false;
    };
    let open_paren = skip_ws_and_comments(sql, after_name, mysql_compatible_comments);
    if sql.as_bytes().get(open_paren) != Some(&b'(') {
        return false;
    }
    let Some(after_call) = skip_balanced_parens(sql, open_paren, mysql_compatible_comments) else {
        return false;
    };
    let args = &sql[open_paren + 1..after_call - 1];
    if function_name == "RELEASE_ALL_LOCKS" && !args.trim().is_empty() {
        return false;
    }
    if mysql_lock_function_args_contain_nested_execution(args, mysql_compatible_comments) {
        return false;
    }
    only_statement_terminators_remain(sql, after_call, mysql_compatible_comments)
}

fn consume_keyword_at(sql: &str, start: usize, keyword: &str) -> Option<usize> {
    let end = start.checked_add(keyword.len())?;
    let candidate = sql.get(start..end)?;
    if !candidate.eq_ignore_ascii_case(keyword) {
        return None;
    }
    if sql
        .as_bytes()
        .get(end)
        .is_some_and(|byte| is_word_part(*byte) || matches!(byte, b'$' | b'#'))
    {
        return None;
    }
    Some(end)
}

fn only_statement_terminators_remain(
    sql: &str,
    mut idx: usize,
    mysql_compatible_comments: bool,
) -> bool {
    loop {
        idx = skip_ws_and_comments(sql, idx, mysql_compatible_comments);
        if idx >= sql.len() {
            return true;
        }
        if sql.as_bytes().get(idx) == Some(&b';') {
            idx += 1;
            continue;
        }
        return false;
    }
}

fn mysql_lock_function_args_contain_nested_execution(
    args: &str,
    mysql_compatible_comments: bool,
) -> bool {
    let bytes = args.as_bytes();
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] == b'@' {
            idx += 1;
            if bytes.get(idx) == Some(&b'@') {
                idx += 1;
            }
            while bytes
                .get(idx)
                .is_some_and(|byte| is_word_part(*byte) || matches!(byte, b'.' | b'$'))
            {
                idx += 1;
            }
            continue;
        }
        if let Some(next) = skip_ignored_span(args, idx, mysql_compatible_comments) {
            idx = next;
            continue;
        }
        if is_word_start(bytes[idx]) {
            let start = idx;
            idx += 1;
            while idx < bytes.len() && is_word_part(bytes[idx]) {
                idx += 1;
            }
            let word = args[start..idx].to_ascii_uppercase();
            if matches!(
                word.as_str(),
                "SELECT"
                    | "WITH"
                    | "INSERT"
                    | "UPDATE"
                    | "DELETE"
                    | "REPLACE"
                    | "CALL"
                    | "DO"
                    | "BEGIN"
            ) {
                return true;
            }
            let after_word = skip_ws_and_comments(args, idx, mysql_compatible_comments);
            if args.as_bytes().get(after_word) == Some(&b'(') {
                return true;
            }
            continue;
        }
        idx += 1;
    }
    false
}

fn classify_alter_sql_for_db_type(
    db_type: DatabaseType,
    sql: &str,
    mysql_compatible_comments: bool,
) -> SqlKind {
    match classification_profile_for_db_type(db_type) {
        SqlClassificationProfile::Oracle => {
            let words = statement_words(sql, mysql_compatible_comments);
            if words
                .get(1)
                .is_some_and(|word| matches!(word.as_str(), "SESSION" | "SYSTEM"))
            {
                // Oracle ALTER SESSION/SYSTEM is session/system control, not DDL
                // with an implicit transaction commit. Classify it separately so
                // interrupt policy does not borrow DDL's implicit-commit meaning.
                return SqlKind::SessionControl;
            }
        }
        SqlClassificationProfile::MySqlCompatible => {}
    }
    SqlKind::Ddl
}

fn classify_oracle_lock_sql(sql: &str) -> SqlKind {
    let words = statement_words(sql, false);
    if words.first().is_some_and(|word| word == "LOCK")
        && words.get(1).is_some_and(|word| word == "TABLE")
    {
        SqlKind::Dml
    } else {
        SqlKind::Unknown
    }
}

fn classify_explain_sql_for_db_type(db_type: DatabaseType, sql: &str) -> SqlKind {
    match classification_profile_for_db_type(db_type) {
        SqlClassificationProfile::MySqlCompatible => return SqlKind::SelectLike,
        SqlClassificationProfile::Oracle => {}
    }

    let words = statement_words(sql, false);
    if words.first().is_some_and(|word| word == "EXPLAIN")
        && words.get(1).is_some_and(|word| word == "PLAN")
    {
        SqlKind::Dml
    } else {
        SqlKind::Unknown
    }
}

fn classify_select_sql_for_db_type(
    db_type: DatabaseType,
    sql: &str,
    mysql_compatible_comments: bool,
) -> SqlKind {
    let locking_select = match classification_profile_for_db_type(db_type) {
        SqlClassificationProfile::Oracle => statement_contains_word_sequence_any_depth(
            sql,
            mysql_compatible_comments,
            &["FOR", "UPDATE"],
        ),
        SqlClassificationProfile::MySqlCompatible => {
            statement_contains_word_sequence_any_depth(
                sql,
                mysql_compatible_comments,
                &["FOR", "UPDATE"],
            ) || statement_contains_word_sequence_any_depth(
                sql,
                mysql_compatible_comments,
                &["FOR", "SHARE"],
            ) || statement_contains_word_sequence_any_depth(
                sql,
                mysql_compatible_comments,
                &["LOCK", "IN", "SHARE", "MODE"],
            )
        }
    };
    if locking_select {
        SqlKind::Dml
    } else {
        SqlKind::SelectLike
    }
}

pub(crate) fn sql_contains_word_sequence_any_depth_for_db_type(
    db_type: DatabaseType,
    sql: &str,
    expected: &[&str],
) -> bool {
    let mysql_compatible_comments =
        classification_profile_for_db_type(db_type).mysql_compatible_comments();
    let stripped_sql =
        strip_leading_comments_and_whitespace_with_mode(sql, mysql_compatible_comments);
    let expanded_sql = if mysql_compatible_comments {
        mysql_sql_with_executable_comments_expanded(stripped_sql)
    } else {
        Cow::Borrowed(stripped_sql)
    };
    statement_contains_word_sequence_any_depth(
        expanded_sql.as_ref(),
        mysql_compatible_comments,
        expected,
    )
}

fn statement_contains_word_sequence_any_depth(
    sql: &str,
    mysql_compatible_comments: bool,
    expected: &[&str],
) -> bool {
    words_contain_sequence(
        &statement_words_any_depth(sql, mysql_compatible_comments),
        expected,
    )
}

fn words_contain_sequence(words: &[String], expected: &[&str]) -> bool {
    !expected.is_empty()
        && words.windows(expected.len()).any(|window| {
            window
                .iter()
                .zip(expected)
                .all(|(word, expected)| word == expected)
        })
}

fn classify_begin_sql_for_db_type(
    db_type: DatabaseType,
    sql: &str,
    mysql_compatible_comments: bool,
) -> SqlKind {
    let words = statement_words(sql, mysql_compatible_comments);
    if words.first().is_some_and(|word| word == "BEGIN")
        && words.get(1).is_some_and(|word| word == "NOT")
        && words.get(2).is_some_and(|word| word == "ATOMIC")
    {
        match db_type {
            DatabaseType::MariaDB => {
                // MariaDB `BEGIN NOT ATOMIC` is an executable compound statement,
                // not a transaction BEGIN. It can run DML, routines, and lock
                // functions before an interrupt, so route it through the unsafe
                // procedure/script policy instead of the Unknown discard fallback.
                SqlKind::PlsqlOrProcedure
            }
            DatabaseType::MySQL | DatabaseType::Oracle => SqlKind::Unknown,
        }
    } else {
        SqlKind::TransactionControl
    }
}

fn classify_set_sql_for_db_type(
    db_type: DatabaseType,
    sql: &str,
    mysql_compatible_comments: bool,
) -> SqlKind {
    let mysql_family = classification_profile_for_db_type(db_type).mysql_compatible_comments();
    let words = statement_words(sql, mysql_compatible_comments);
    if words.first().is_none_or(|word| word != "SET") {
        return SqlKind::Unknown;
    }

    if words.get(1).is_some_and(|word| word == "ROLE") {
        return SqlKind::SessionControl;
    }

    if mysql_family
        && (words.get(1).is_some_and(|word| word == "PASSWORD")
            || (words.get(1).is_some_and(|word| word == "DEFAULT")
                && words.get(2).is_some_and(|word| word == "ROLE")))
    {
        return SqlKind::Ddl;
    }

    if set_sql_contains_transaction_control_assignment(sql, mysql_compatible_comments) {
        return SqlKind::TransactionControl;
    }

    if mysql_family && set_sql_affects_only_global_or_persist_scope(sql, mysql_compatible_comments)
    {
        return SqlKind::Ddl;
    }

    if set_body_starts_with_user_variable(sql, mysql_compatible_comments) {
        return SqlKind::Unknown;
    }

    match words.get(1).map(String::as_str) {
        Some(
            "AUTOCOMMIT"
            | "TRANSACTION"
            | "CONSTRAINT"
            | "CONSTRAINTS"
            | "TRANSACTION_ISOLATION"
            | "TX_ISOLATION"
            | "TRANSACTION_READ_ONLY"
            | "TX_READ_ONLY",
        ) => SqlKind::TransactionControl,
        Some("SESSION" | "LOCAL")
            if matches!(
                words.get(2).map(String::as_str),
                Some(
                    "AUTOCOMMIT"
                        | "TRANSACTION"
                        | "TRANSACTION_ISOLATION"
                        | "TX_ISOLATION"
                        | "TRANSACTION_READ_ONLY"
                        | "TX_READ_ONLY"
                )
            ) =>
        {
            SqlKind::TransactionControl
        }
        _ => SqlKind::Unknown,
    }
}

fn set_sql_contains_transaction_control_assignment(
    sql: &str,
    mysql_compatible_comments: bool,
) -> bool {
    let Some((set_token, _, after_set)) = next_top_level_word(sql, 0, mysql_compatible_comments)
    else {
        return false;
    };
    if set_token != "SET" {
        return false;
    }

    let mut assignment_start = after_set;
    let mut idx = after_set;
    let mut depth = 0usize;
    let bytes = sql.as_bytes();
    while idx < bytes.len() {
        if let Some(next) = skip_ignored_span(sql, idx, mysql_compatible_comments) {
            idx = next;
            continue;
        }
        match bytes[idx] {
            b'(' => {
                depth = depth.saturating_add(1);
                idx += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                idx += 1;
            }
            b',' if depth == 0 => {
                if set_assignment_targets_transaction_control(
                    &sql[assignment_start..idx],
                    mysql_compatible_comments,
                ) {
                    return true;
                }
                assignment_start = idx + 1;
                idx += 1;
            }
            _ => idx += 1,
        }
    }

    set_assignment_targets_transaction_control(&sql[assignment_start..], mysql_compatible_comments)
}

fn set_sql_affects_only_global_or_persist_scope(
    sql: &str,
    mysql_compatible_comments: bool,
) -> bool {
    let words = statement_words(sql, mysql_compatible_comments);
    if words.first().is_some_and(|word| word == "SET")
        && words.get(1).is_some_and(|word| word == "GLOBAL")
        && words.get(2).is_some_and(|word| word == "TRANSACTION")
    {
        return true;
    }

    let Some((set_token, _, after_set)) = next_top_level_word(sql, 0, mysql_compatible_comments)
    else {
        return false;
    };
    if set_token != "SET" {
        return false;
    }

    let mut saw_assignment = false;
    let mut assignment_start = after_set;
    let mut idx = after_set;
    let mut depth = 0usize;
    let bytes = sql.as_bytes();
    while idx < bytes.len() {
        if let Some(next) = skip_ignored_span(sql, idx, mysql_compatible_comments) {
            idx = next;
            continue;
        }
        match bytes[idx] {
            b'(' => {
                depth = depth.saturating_add(1);
                idx += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                idx += 1;
            }
            b',' if depth == 0 => {
                if !set_assignment_targets_global_or_persist_scope(
                    &sql[assignment_start..idx],
                    mysql_compatible_comments,
                ) {
                    return false;
                }
                saw_assignment = true;
                assignment_start = idx + 1;
                idx += 1;
            }
            _ => idx += 1,
        }
    }

    set_assignment_targets_global_or_persist_scope(
        &sql[assignment_start..],
        mysql_compatible_comments,
    ) && (saw_assignment || !sql[assignment_start..].trim().is_empty())
}

pub(crate) fn mysql_set_statement_assigns_session_variable(
    sql: &str,
    variable_names: &[&str],
) -> bool {
    let mysql_compatible_comments = true;
    let Some((set_token, _, after_set)) = next_top_level_word(sql, 0, mysql_compatible_comments)
    else {
        return false;
    };
    if set_token != "SET" {
        return false;
    }

    let mut assignment_start = after_set;
    let mut idx = after_set;
    let mut depth = 0usize;
    let bytes = sql.as_bytes();
    while idx < bytes.len() {
        if let Some(next) = skip_ignored_span(sql, idx, mysql_compatible_comments) {
            idx = next;
            continue;
        }
        match bytes[idx] {
            b'(' => {
                depth = depth.saturating_add(1);
                idx += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                idx += 1;
            }
            b',' if depth == 0 => {
                if set_assignment_targets_session_variable(
                    &sql[assignment_start..idx],
                    mysql_compatible_comments,
                    variable_names,
                ) {
                    return true;
                }
                assignment_start = idx + 1;
                idx += 1;
            }
            _ => idx += 1,
        }
    }

    set_assignment_targets_session_variable(
        &sql[assignment_start..],
        mysql_compatible_comments,
        variable_names,
    )
}

fn set_assignment_targets_transaction_control(
    assignment: &str,
    mysql_compatible_comments: bool,
) -> bool {
    let Some(target) = set_assignment_target(assignment, mysql_compatible_comments) else {
        return false;
    };
    normalized_set_target_is_transaction_control(&target)
}

fn set_assignment_targets_global_or_persist_scope(
    assignment: &str,
    mysql_compatible_comments: bool,
) -> bool {
    let Some(target) = set_assignment_target(assignment, mysql_compatible_comments) else {
        return false;
    };
    normalized_set_target_is_global_or_persist_scope(&target)
}

fn set_assignment_targets_session_variable(
    assignment: &str,
    mysql_compatible_comments: bool,
    variable_names: &[&str],
) -> bool {
    let Some(target) = set_assignment_target(assignment, mysql_compatible_comments) else {
        return false;
    };
    normalized_set_target_is_named_session_variable(&target, variable_names)
}

fn set_assignment_target(assignment: &str, mysql_compatible_comments: bool) -> Option<String> {
    let bytes = assignment.as_bytes();
    let mut idx = 0usize;
    let mut depth = 0usize;
    while idx < bytes.len() {
        if let Some(next) = skip_ignored_span(assignment, idx, mysql_compatible_comments) {
            idx = next;
            continue;
        }
        match bytes[idx] {
            b'(' => {
                depth = depth.saturating_add(1);
                idx += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                idx += 1;
            }
            b'=' if depth == 0 => {
                return Some(normalize_set_assignment_target(
                    &assignment[..idx],
                    mysql_compatible_comments,
                ));
            }
            b':' if depth == 0 && bytes.get(idx + 1) == Some(&b'=') => {
                return Some(normalize_set_assignment_target(
                    &assignment[..idx],
                    mysql_compatible_comments,
                ));
            }
            _ => idx += 1,
        }
    }
    None
}

fn normalize_set_assignment_target(target: &str, mysql_compatible_comments: bool) -> String {
    let bytes = target.as_bytes();
    let mut normalized = String::with_capacity(target.len());
    let mut idx = 0usize;
    let mut pending_space = false;
    while idx < bytes.len() {
        if let Some(next) = skip_ignored_span(target, idx, mysql_compatible_comments) {
            pending_space = !normalized.is_empty();
            idx = next;
            continue;
        }
        let Some(ch) = target[idx..].chars().next() else {
            break;
        };
        if ch.is_whitespace() {
            pending_space = !normalized.is_empty();
        } else {
            if pending_space {
                normalized.push(' ');
                pending_space = false;
            }
            normalized.extend(ch.to_uppercase());
        }
        idx += ch.len_utf8();
    }
    normalized
}

fn normalized_set_target_is_global_or_persist_scope(target: &str) -> bool {
    target.starts_with("GLOBAL ")
        || target.starts_with("PERSIST ")
        || target.starts_with("PERSIST_ONLY ")
        || target.starts_with("@@GLOBAL.")
        || target.starts_with("@@PERSIST.")
        || target.starts_with("@@PERSIST_ONLY.")
}

fn normalized_set_target_is_transaction_control(target: &str) -> bool {
    if target.starts_with('@') && !target.starts_with("@@") {
        return false;
    }
    if target.starts_with("GLOBAL ")
        || target.starts_with("PERSIST ")
        || target.starts_with("PERSIST_ONLY ")
        || target.starts_with("@@GLOBAL.")
        || target.starts_with("@@PERSIST.")
        || target.starts_with("@@PERSIST_ONLY.")
    {
        return false;
    }

    let target = target
        .strip_prefix("SESSION ")
        .or_else(|| target.strip_prefix("LOCAL "))
        .or_else(|| target.strip_prefix("@@SESSION."))
        .or_else(|| target.strip_prefix("@@LOCAL."))
        .or_else(|| target.strip_prefix("@@"))
        .unwrap_or(target);

    matches!(
        target,
        "AUTOCOMMIT"
            | "TRANSACTION_ISOLATION"
            | "TX_ISOLATION"
            | "TRANSACTION_READ_ONLY"
            | "TX_READ_ONLY"
    )
}

fn normalized_set_target_is_named_session_variable(target: &str, variable_names: &[&str]) -> bool {
    let target = target.strip_prefix("STATEMENT ").unwrap_or(target);
    if target.starts_with('@') && !target.starts_with("@@") {
        return false;
    }
    if target.starts_with("GLOBAL ")
        || target.starts_with("PERSIST ")
        || target.starts_with("PERSIST_ONLY ")
        || target.starts_with("@@GLOBAL.")
        || target.starts_with("@@PERSIST.")
        || target.starts_with("@@PERSIST_ONLY.")
    {
        return false;
    }

    let target = target
        .strip_prefix("SESSION ")
        .or_else(|| target.strip_prefix("LOCAL "))
        .or_else(|| target.strip_prefix("@@SESSION."))
        .or_else(|| target.strip_prefix("@@LOCAL."))
        .or_else(|| target.strip_prefix("@@"))
        .unwrap_or(target);

    variable_names.contains(&target)
}

fn set_body_starts_with_user_variable(sql: &str, mysql_compatible_comments: bool) -> bool {
    let Some((set_token, _, after_set)) = next_top_level_word(sql, 0, mysql_compatible_comments)
    else {
        return false;
    };
    if set_token != "SET" {
        return false;
    }

    let pos = skip_ws_and_comments(sql, after_set, mysql_compatible_comments);
    sql.as_bytes().get(pos) == Some(&b'@') && sql.as_bytes().get(pos + 1) != Some(&b'@')
}

fn classify_start_sql_for_db_type(
    db_type: DatabaseType,
    sql: &str,
    mysql_compatible_comments: bool,
) -> SqlKind {
    let mysql_family = classification_profile_for_db_type(db_type).mysql_compatible_comments();
    let words = statement_words(sql, mysql_compatible_comments);
    if words.first().is_some_and(|word| word == "START")
        && words.get(1).is_some_and(|word| word == "TRANSACTION")
    {
        SqlKind::TransactionControl
    } else if mysql_family && mysql_replication_words(&words) {
        SqlKind::Ddl
    } else {
        SqlKind::Unknown
    }
}

fn classify_mysql_replication_sql(sql: &str, mysql_compatible_comments: bool) -> SqlKind {
    let words = statement_words(sql, mysql_compatible_comments);
    if mysql_replication_words(&words) {
        SqlKind::Ddl
    } else {
        SqlKind::Unknown
    }
}

fn mysql_replication_words(words: &[String]) -> bool {
    matches!(
        words.first().map(String::as_str),
        Some("START" | "STOP" | "RESET")
    ) && matches!(words.get(1).map(String::as_str), Some("REPLICA" | "SLAVE"))
        || words.first().is_some_and(|word| word == "CHANGE")
            && matches!(
                words.get(1).map(String::as_str),
                Some("REPLICATION" | "MASTER")
            )
}

fn classify_release_sql(sql: &str, mysql_compatible_comments: bool) -> SqlKind {
    let words = statement_words(sql, mysql_compatible_comments);
    if words.first().is_some_and(|word| word == "RELEASE")
        && words.get(1).is_some_and(|word| word == "SAVEPOINT")
    {
        SqlKind::TransactionControl
    } else {
        SqlKind::Unknown
    }
}

fn classify_xa_sql(sql: &str, mysql_compatible_comments: bool) -> SqlKind {
    let words = statement_words(sql, mysql_compatible_comments);
    if words.first().is_none_or(|word| word != "XA") {
        return SqlKind::Unknown;
    }

    match words.get(1).map(String::as_str) {
        Some("RECOVER") => SqlKind::SelectLike,
        Some("START" | "BEGIN" | "END" | "PREPARE" | "COMMIT" | "ROLLBACK") => {
            SqlKind::TransactionControl
        }
        _ => SqlKind::Unknown,
    }
}

fn classify_with_sql_for_db_type(db_type: DatabaseType, sql: &str) -> SqlKind {
    let mysql_compatible_comments =
        classification_profile_for_db_type(db_type).mysql_compatible_comments();
    let Some((with_token, _, mut pos)) = next_top_level_word(sql, 0, mysql_compatible_comments)
    else {
        return SqlKind::Unknown;
    };
    if with_token != "WITH" {
        return SqlKind::Unknown;
    }

    if let Some((token, _, after_token)) = next_top_level_word(sql, pos, mysql_compatible_comments)
    {
        if token == "RECURSIVE" {
            pos = after_token;
        }
    }

    loop {
        let Some((_cte_name, _, after_name)) =
            next_top_level_word(sql, pos, mysql_compatible_comments)
        else {
            return SqlKind::Unknown;
        };
        pos = after_name;
        pos = skip_ws_and_comments(sql, pos, mysql_compatible_comments);

        if sql.as_bytes().get(pos) == Some(&b'(') {
            let Some(after_columns) = skip_balanced_parens(sql, pos, mysql_compatible_comments)
            else {
                return SqlKind::Unknown;
            };
            pos = skip_ws_and_comments(sql, after_columns, mysql_compatible_comments);
        }

        let Some((as_token, _, after_as)) =
            next_top_level_word(sql, pos, mysql_compatible_comments)
        else {
            return SqlKind::Unknown;
        };
        if as_token != "AS" {
            return SqlKind::Unknown;
        }

        pos = skip_ws_and_comments(sql, after_as, mysql_compatible_comments);
        if sql.as_bytes().get(pos) != Some(&b'(') {
            return SqlKind::Unknown;
        }
        let Some(after_cte_body) = skip_balanced_parens(sql, pos, mysql_compatible_comments) else {
            return SqlKind::Unknown;
        };

        pos = skip_ws_and_comments(sql, after_cte_body, mysql_compatible_comments);
        if sql.as_bytes().get(pos) == Some(&b',') {
            pos += 1;
            continue;
        }

        let Some((main_token, main_start, _)) =
            next_top_level_word(sql, pos, mysql_compatible_comments)
        else {
            return SqlKind::Unknown;
        };
        if main_token == "SELECT" {
            return classify_select_sql_for_db_type(db_type, sql, mysql_compatible_comments);
        }
        return classify_first_word_for_db_type(db_type, &main_token, &sql[main_start..]);
    }
}

fn contains_multiple_statements(sql: &str, mysql_compatible_comments: bool) -> bool {
    let bytes = sql.as_bytes();
    let mut idx = 0usize;
    while idx < bytes.len() {
        if let Some(next) = skip_ignored_span(sql, idx, mysql_compatible_comments) {
            idx = next;
            continue;
        }
        if bytes[idx] == b';' && has_significant_sql_after(sql, idx + 1, mysql_compatible_comments)
        {
            return true;
        }
        idx += 1;
    }
    false
}

fn has_significant_sql_after(sql: &str, mut idx: usize, mysql_compatible_comments: bool) -> bool {
    let bytes = sql.as_bytes();
    while idx < bytes.len() {
        if bytes[idx].is_ascii_whitespace() || bytes[idx] == b';' {
            idx += 1;
            continue;
        }
        if let Some(next) = skip_comment(sql, idx, mysql_compatible_comments) {
            idx = next;
            continue;
        }
        return true;
    }
    false
}

fn oracle_plsql_block_has_trailing_statement(sql: &str) -> bool {
    oracle_plsql_block_end_boundary(sql)
        .is_some_and(|idx| has_significant_sql_after(sql, idx, false))
}

fn oracle_plsql_block_end_boundary(sql: &str) -> Option<usize> {
    let first_word = next_top_level_word(sql, 0, false)?.0;
    let mut idx = 0usize;
    let mut top_depth = 0usize;
    let mut local_declare_depth = 0usize;
    let mut declare_waiting_for_body = first_word == "DECLARE";
    let mut declare_local_subprogram_pending = false;

    while let Some((word, _, after_word)) = next_top_level_word(sql, idx, false) {
        if declare_waiting_for_body && top_depth == 0 {
            if local_declare_depth > 0 {
                match word.as_str() {
                    "BEGIN" | "CASE" | "IF" | "LOOP" => {
                        local_declare_depth = local_declare_depth.saturating_add(1);
                        idx = after_word;
                    }
                    "END" => {
                        local_declare_depth = local_declare_depth.saturating_sub(1);
                        idx = oracle_skip_end_qualifier(sql, after_word);
                    }
                    _ => {
                        idx = after_word;
                    }
                }
                continue;
            }

            match word.as_str() {
                "PROCEDURE" | "FUNCTION" => {
                    declare_local_subprogram_pending = true;
                    idx = after_word;
                }
                "BEGIN" if declare_local_subprogram_pending => {
                    declare_local_subprogram_pending = false;
                    local_declare_depth = 1;
                    idx = after_word;
                }
                "BEGIN" => {
                    declare_waiting_for_body = false;
                    top_depth = 1;
                    idx = after_word;
                }
                _ => {
                    idx = after_word;
                }
            }
            continue;
        }

        match word.as_str() {
            "BEGIN" => {
                top_depth = top_depth.saturating_add(1);
                idx = after_word;
            }
            "CASE" | "IF" | "LOOP" if top_depth > 0 => {
                top_depth = top_depth.saturating_add(1);
                idx = after_word;
            }
            "END" if top_depth > 0 => {
                top_depth = top_depth.saturating_sub(1);
                let after_end = oracle_skip_end_qualifier(sql, after_word);
                if top_depth == 0 {
                    return Some(oracle_skip_plsql_block_terminator(sql, after_end));
                }
                idx = after_end;
            }
            _ => {
                idx = after_word;
            }
        }
    }

    None
}

fn oracle_skip_end_qualifier(sql: &str, after_end: usize) -> usize {
    let Some((word, start, after_word)) = next_top_level_word(sql, after_end, false) else {
        return after_end;
    };
    if !matches!(word.as_str(), "IF" | "LOOP" | "CASE") {
        return after_end;
    }
    let qualifier_is_before_terminator = sql[after_end..start]
        .bytes()
        .all(|byte| byte.is_ascii_whitespace());
    if qualifier_is_before_terminator {
        after_word
    } else {
        after_end
    }
}

fn oracle_skip_plsql_block_terminator(sql: &str, mut idx: usize) -> usize {
    let bytes = sql.as_bytes();
    while idx < bytes.len() {
        if bytes[idx].is_ascii_whitespace() {
            idx += 1;
            continue;
        }
        if bytes[idx] == b';' {
            idx += 1;
            continue;
        }
        if bytes[idx] == b'/' && oracle_slash_is_line_terminator(sql, idx) {
            idx += 1;
            continue;
        }
        break;
    }
    idx
}

fn oracle_slash_is_line_terminator(sql: &str, idx: usize) -> bool {
    let before = sql[..idx]
        .rsplit_once('\n')
        .map(|(_, line)| line)
        .unwrap_or(&sql[..idx]);
    let after = sql[idx + 1..]
        .split_once('\n')
        .map(|(line, _)| line)
        .unwrap_or(&sql[idx + 1..]);
    before.trim().is_empty() && after.trim().is_empty()
}

fn next_top_level_word(
    sql: &str,
    mut idx: usize,
    mysql_compatible_comments: bool,
) -> Option<(String, usize, usize)> {
    let bytes = sql.as_bytes();
    let mut depth = 0usize;
    while idx < bytes.len() {
        if let Some(next) = skip_ignored_span(sql, idx, mysql_compatible_comments) {
            idx = next;
            continue;
        }
        match bytes[idx] {
            b'(' => {
                depth = depth.saturating_add(1);
                idx += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                idx += 1;
            }
            byte if depth == 0 && is_word_start(byte) => {
                let start = idx;
                idx += 1;
                while idx < bytes.len() && is_word_part(bytes[idx]) {
                    idx += 1;
                }
                let upper = sql[start..idx].to_ascii_uppercase();
                return Some((upper, start, idx));
            }
            _ => idx += 1,
        }
    }
    None
}

fn next_word_any_depth(
    sql: &str,
    mut idx: usize,
    mysql_compatible_comments: bool,
) -> Option<(String, usize, usize)> {
    let bytes = sql.as_bytes();
    while idx < bytes.len() {
        if let Some(next) = skip_ignored_span(sql, idx, mysql_compatible_comments) {
            idx = next;
            continue;
        }
        match bytes[idx] {
            byte if is_word_start(byte) => {
                let start = idx;
                idx += 1;
                while idx < bytes.len() && is_word_part(bytes[idx]) {
                    idx += 1;
                }
                let upper = sql[start..idx].to_ascii_uppercase();
                return Some((upper, start, idx));
            }
            _ => idx += 1,
        }
    }
    None
}

fn skip_ws_and_comments(sql: &str, mut idx: usize, mysql_compatible_comments: bool) -> usize {
    let bytes = sql.as_bytes();
    while idx < bytes.len() {
        if bytes[idx].is_ascii_whitespace() {
            idx += 1;
            continue;
        }
        if let Some(next) = skip_comment(sql, idx, mysql_compatible_comments) {
            idx = next;
            continue;
        }
        break;
    }
    idx
}

fn skip_balanced_parens(
    sql: &str,
    mut idx: usize,
    mysql_compatible_comments: bool,
) -> Option<usize> {
    let bytes = sql.as_bytes();
    if bytes.get(idx) != Some(&b'(') {
        return None;
    }
    let mut depth = 0usize;
    while idx < bytes.len() {
        if let Some(next) = skip_ignored_span(sql, idx, mysql_compatible_comments) {
            idx = next;
            continue;
        }
        match bytes[idx] {
            b'(' => {
                depth += 1;
                idx += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                idx += 1;
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => idx += 1,
        }
    }
    None
}

fn skip_ignored_span(sql: &str, idx: usize, mysql_compatible_comments: bool) -> Option<usize> {
    skip_comment(sql, idx, mysql_compatible_comments)
        .or_else(|| skip_q_quote(sql, idx))
        .or_else(|| skip_quoted(sql, idx))
}

fn skip_comment(sql: &str, idx: usize, mysql_compatible_comments: bool) -> Option<usize> {
    let bytes = sql.as_bytes();
    let dash_comment_start = if mysql_compatible_comments {
        sql_text::is_mysql_dash_comment_start(bytes, idx)
    } else {
        bytes.get(idx..idx + 2) == Some(b"--")
    };
    if dash_comment_start {
        return Some(
            bytes[idx..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|offset| idx + offset + 1)
                .unwrap_or(bytes.len()),
        );
    }
    if bytes.get(idx) == Some(&b'#') {
        return Some(
            bytes[idx..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|offset| idx + offset + 1)
                .unwrap_or(bytes.len()),
        );
    }
    if bytes.get(idx..idx + 2) == Some(b"/*") {
        return Some(
            sql[idx + 2..]
                .find("*/")
                .map(|offset| idx + 2 + offset + 2)
                .unwrap_or(bytes.len()),
        );
    }
    None
}

fn skip_quoted(sql: &str, idx: usize) -> Option<usize> {
    let bytes = sql.as_bytes();
    let quote = *bytes.get(idx)?;
    if !matches!(quote, b'\'' | b'"' | b'`') {
        return None;
    }
    let mut pos = idx + 1;
    while pos < bytes.len() {
        if bytes[pos] == b'\\' {
            pos = pos.saturating_add(2);
            continue;
        }
        if bytes[pos] == quote {
            if bytes.get(pos + 1) == Some(&quote) {
                pos += 2;
            } else {
                return Some(pos + 1);
            }
        } else {
            pos += 1;
        }
    }
    Some(bytes.len())
}

fn skip_q_quote(sql: &str, idx: usize) -> Option<usize> {
    let bytes = sql.as_bytes();
    let (prefix_len, delimiter) = q_quote_prefix(bytes, idx)?;
    let closing = sql_text::q_quote_closing_byte(delimiter);
    let mut pos = idx + prefix_len;
    while pos + 1 < bytes.len() {
        if bytes[pos] == closing && bytes[pos + 1] == b'\'' {
            return Some(pos + 2);
        }
        pos += 1;
    }
    Some(bytes.len())
}

fn q_quote_prefix(bytes: &[u8], idx: usize) -> Option<(usize, u8)> {
    if idx > 0 && is_word_part(bytes[idx - 1]) {
        return None;
    }
    let first = bytes.get(idx)?.to_ascii_uppercase();
    if first == b'Q' && bytes.get(idx + 1) == Some(&b'\'') {
        let delimiter = *bytes.get(idx + 2)?;
        return sql_text::is_valid_q_quote_delimiter_byte(delimiter).then_some((3, delimiter));
    }
    if first == b'N'
        && bytes.get(idx + 1).map(u8::to_ascii_uppercase) == Some(b'Q')
        && bytes.get(idx + 2) == Some(&b'\'')
    {
        let delimiter = *bytes.get(idx + 3)?;
        return sql_text::is_valid_q_quote_delimiter_byte(delimiter).then_some((4, delimiter));
    }
    None
}

fn is_word_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_word_part(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' || byte == b'#'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mysql_executable_comment_body_is_classified_as_sql() {
        assert_eq!(
            SqlStatementAnalysis::new_for_db_type(
                DatabaseType::MySQL,
                "/*!80000 SET autocommit = 0 */"
            )
            .classify_for_db_type(DatabaseType::MySQL),
            SqlKind::TransactionControl
        );
        assert_eq!(
            SqlStatementAnalysis::new_for_db_type(DatabaseType::MariaDB, "/*M!100100 SELECT 1 */")
                .classify_for_db_type(DatabaseType::MariaDB),
            SqlKind::SelectLike
        );
    }

    #[test]
    fn mysql_executable_comment_body_is_not_expanded_for_oracle() {
        assert_eq!(
            SqlStatementAnalysis::new_for_db_type(
                DatabaseType::Oracle,
                "/*!80000 SET @feature_flag = 1 */"
            )
            .classify_for_db_type(DatabaseType::Oracle),
            SqlKind::Unknown
        );
    }

    #[test]
    fn mariadb_set_statement_classification_uses_inner_statement() {
        for (sql, expected) in [
            (
                "SET STATEMENT max_statement_time=1 FOR SELECT 1",
                SqlKind::SelectLike,
            ),
            (
                "SET STATEMENT max_statement_time=1 FOR SELECT * FROM accounts FOR UPDATE",
                SqlKind::Dml,
            ),
            (
                "SET STATEMENT max_statement_time=1 FOR UPDATE accounts SET balance = balance + 1",
                SqlKind::Dml,
            ),
            (
                "SET STATEMENT max_statement_time=1 FOR COMMIT",
                SqlKind::TransactionControl,
            ),
            (
                "SET STATEMENT max_statement_time=1 FOR CALL sync_accounts()",
                SqlKind::PlsqlOrProcedure,
            ),
        ] {
            assert_eq!(
                SqlStatementAnalysis::new_for_db_type(DatabaseType::MariaDB, sql)
                    .classify_for_db_type(DatabaseType::MariaDB),
                expected,
                "{sql}"
            );
        }

        assert_eq!(
            SqlStatementAnalysis::new_for_db_type(
                DatabaseType::MySQL,
                "SET STATEMENT max_statement_time=1 FOR SELECT 1",
            )
            .classify_for_db_type(DatabaseType::MySQL),
            SqlKind::Unknown,
            "SET STATEMENT is MariaDB-specific and must not alter MySQL classification"
        );
    }

    #[test]
    fn mariadb_set_statement_inner_sql_skips_option_literals_comments_and_parens() {
        let sql = "\
            SET STATEMENT \
              max_statement_time=1, \
              optimizer_switch='for=literal', \
              sql_mode = CONCAT('ANSI', ' FOR ignored'), \
              sort_buffer_size=(1024 + 1) \
            FOR SELECT 1";

        assert_eq!(
            mariadb_set_statement_inner_sql(sql).as_deref(),
            Some("SELECT 1")
        );
    }

    #[test]
    fn mysql_executable_comment_expansion_ignores_quoted_markers() {
        let expanded = mysql_sql_with_executable_comments_expanded(
            "SELECT '/*!80000 SET @x=1 */' AS literal, /*!80000 GET_LOCK('k', 0) */",
        );

        assert!(expanded.contains("'/*!80000 SET @x=1 */'"));
        assert!(expanded.contains("GET_LOCK('k', 0)"));
    }

    #[test]
    fn mysql_dash_dash_without_following_space_is_not_a_comment() {
        assert_eq!(
            SqlStatementAnalysis::new_for_db_type(
                DatabaseType::MySQL,
                "SELECT 1--2; UPDATE accounts SET balance = balance + 1",
            )
            .classify_for_db_type(DatabaseType::MySQL),
            SqlKind::Script
        );
        assert_eq!(
            SqlStatementAnalysis::new_for_db_type(
                DatabaseType::MySQL,
                "SELECT 1-- comment; UPDATE accounts SET balance = balance + 1",
            )
            .classify_for_db_type(DatabaseType::MySQL),
            SqlKind::SelectLike
        );
        assert_eq!(
            SqlStatementAnalysis::new_for_db_type(
                DatabaseType::Oracle,
                "SELECT 1--2; UPDATE accounts SET balance = balance + 1",
            )
            .classify_for_db_type(DatabaseType::Oracle),
            SqlKind::SelectLike
        );
    }

    #[test]
    fn locking_selects_are_classified_as_dml_for_session_safety() {
        for (db_type, sql) in [
            (DatabaseType::Oracle, "SELECT * FROM accounts FOR UPDATE"),
            (
                DatabaseType::Oracle,
                "WITH q AS (SELECT 1 id FROM dual) SELECT * FROM q FOR UPDATE",
            ),
            (
                DatabaseType::Oracle,
                "SELECT * FROM accounts FOR UPDATE SKIP LOCKED",
            ),
            (DatabaseType::MySQL, "SELECT * FROM accounts FOR UPDATE"),
            (
                DatabaseType::MySQL,
                "SELECT * FROM accounts FOR UPDATE NOWAIT",
            ),
            (
                DatabaseType::MySQL,
                "SELECT * FROM accounts FOR UPDATE SKIP LOCKED",
            ),
            (DatabaseType::MySQL, "SELECT * FROM accounts FOR SHARE"),
            (
                DatabaseType::MySQL,
                "SELECT * FROM accounts LOCK IN SHARE MODE",
            ),
            (
                DatabaseType::MySQL,
                "SELECT * FROM accounts /*!80000 FOR UPDATE */",
            ),
        ] {
            assert_eq!(
                SqlStatementAnalysis::new_for_db_type(db_type, sql).classify_for_db_type(db_type),
                SqlKind::Dml,
                "{sql}"
            );
        }
    }

    #[test]
    fn oracle_lock_table_is_classified_as_dml_for_session_safety() {
        for sql in [
            "LOCK TABLE accounts IN EXCLUSIVE MODE",
            "/* wait for writer */ LOCK TABLE accounts IN SHARE MODE",
        ] {
            assert_eq!(
                SqlStatementAnalysis::new_for_db_type(DatabaseType::Oracle, sql)
                    .classify_for_db_type(DatabaseType::Oracle),
                SqlKind::Dml,
                "{sql}"
            );
        }

        assert_eq!(
            SqlStatementAnalysis::new_for_db_type(
                DatabaseType::MySQL,
                "LOCK TABLES accounts WRITE"
            )
            .classify_for_db_type(DatabaseType::MySQL),
            SqlKind::SessionControl
        );
    }

    #[test]
    fn oracle_implicit_commit_admin_statements_are_classified_as_ddl() {
        for sql in [
            "ANALYZE TABLE emp COMPUTE STATISTICS",
            "AUDIT SELECT TABLE",
            "NOAUDIT SELECT TABLE",
            "PURGE TABLE emp",
            "FLASHBACK TABLE emp TO BEFORE DROP",
        ] {
            assert_eq!(
                SqlStatementAnalysis::new_for_db_type(DatabaseType::Oracle, sql)
                    .classify_for_db_type(DatabaseType::Oracle),
                SqlKind::Ddl,
                "{sql}"
            );
        }
    }

    #[test]
    fn oracle_alter_session_and_system_are_session_control() {
        for sql in [
            "ALTER SESSION SET CURRENT_SCHEMA = APP_USER",
            "/* keep tx */ ALTER SESSION SET NLS_DATE_FORMAT = 'YYYY-MM-DD'",
            "ALTER SYSTEM SET optimizer_mode = ALL_ROWS",
        ] {
            assert_eq!(
                SqlStatementAnalysis::new_for_db_type(DatabaseType::Oracle, sql)
                    .classify_for_db_type(DatabaseType::Oracle),
                SqlKind::SessionControl,
                "{sql}"
            );
        }

        assert_eq!(
            SqlStatementAnalysis::new_for_db_type(DatabaseType::Oracle, "ALTER TABLE t ADD c INT")
                .classify_for_db_type(DatabaseType::Oracle),
            SqlKind::Ddl
        );
    }

    #[test]
    fn oracle_plsql_block_with_trailing_sql_is_classified_as_script() {
        for sql in [
            "BEGIN\n  NULL;\nEND;\nSELECT 1 FROM dual",
            "BEGIN\n  NULL;\nEND;\n/\nUPDATE t SET c = 1",
            "DECLARE\n  v NUMBER;\nBEGIN\n  v := 1;\nEND;\nSELECT * FROM dual",
            "BEGIN\n  IF 1 = 1 THEN\n    NULL;\n  END IF;\nEND;\nSELECT 1 FROM dual",
        ] {
            assert_eq!(
                SqlStatementAnalysis::new_for_db_type(DatabaseType::Oracle, sql)
                    .classify_for_db_type(DatabaseType::Oracle),
                SqlKind::Script,
                "{sql}"
            );
        }
    }

    #[test]
    fn oracle_single_plsql_block_internal_semicolons_remain_procedure_like() {
        for sql in [
            "BEGIN\n  NULL;\nEND;",
            "DECLARE\n  v NUMBER;\nBEGIN\n  v := 1;\nEND;\n/",
            "BEGIN\n  v := CASE WHEN flag THEN 1 ELSE 0 END;\nEND;",
            "DECLARE\n  PROCEDURE p IS\n  BEGIN\n    NULL;\n  END;\nBEGIN\n  p;\nEND;",
        ] {
            assert_eq!(
                SqlStatementAnalysis::new_for_db_type(DatabaseType::Oracle, sql)
                    .classify_for_db_type(DatabaseType::Oracle),
                SqlKind::PlsqlOrProcedure,
                "{sql}"
            );
        }
    }

    #[test]
    fn mysql_table_maintenance_is_not_select_like_for_session_safety() {
        for sql in [
            "ANALYZE TABLE t",
            "CHECK TABLE t",
            "CHECKSUM TABLE t",
            "OPTIMIZE TABLE t",
            "REPAIR TABLE t",
            "LOAD INDEX INTO CACHE t",
            "FLUSH STATUS",
            "CACHE INDEX t IN key_cache",
            "INSTALL PLUGIN audit_log SONAME 'audit_log.so'",
            "UNINSTALL PLUGIN audit_log",
            "START REPLICA",
            "STOP REPLICA",
            "RESET REPLICA",
            "CHANGE REPLICATION SOURCE TO SOURCE_HOST = 'replica.example.com'",
            "SET GLOBAL max_connections = 200",
            "SET @@global.transaction_isolation = 'SERIALIZABLE'",
            "SET PERSIST_ONLY transaction_read_only = OFF",
            "SET GLOBAL TRANSACTION ISOLATION LEVEL READ COMMITTED",
            "SET DEFAULT ROLE app_read TO 'worker'@'%'",
            "SET PASSWORD FOR 'worker'@'%' = 'x'",
        ] {
            assert_eq!(
                SqlStatementAnalysis::new_for_db_type(DatabaseType::MySQL, sql)
                    .classify_for_db_type(DatabaseType::MySQL),
                SqlKind::Ddl,
                "{sql}"
            );
        }
    }

    #[test]
    fn session_scope_controls_are_classified_for_interrupt_safety() {
        for (db_type, sql) in [
            (DatabaseType::MySQL, "USE app_db"),
            (DatabaseType::MariaDB, "USE app_db"),
            (DatabaseType::MySQL, "LOCK TABLES accounts WRITE"),
            (DatabaseType::MySQL, "LOCK TABLE accounts WRITE"),
            (DatabaseType::MySQL, "UNLOCK TABLES"),
            (DatabaseType::MySQL, "UNLOCK TABLE"),
            (DatabaseType::MySQL, "LOCK INSTANCE FOR BACKUP"),
            (DatabaseType::MySQL, "UNLOCK INSTANCE"),
            (DatabaseType::MySQL, "RESET CONNECTION"),
            (DatabaseType::MySQL, "DO GET_LOCK('qt', 0)"),
            (DatabaseType::MySQL, "DO GET_LOCK(@qt_lock_name, 0)"),
            (DatabaseType::MariaDB, "DO RELEASE_LOCK('qt')"),
            (DatabaseType::MariaDB, "DO RELEASE_LOCK(@qt_lock_name)"),
            (DatabaseType::MySQL, "DO RELEASE_ALL_LOCKS()"),
            (DatabaseType::Oracle, "SET ROLE app_read"),
            (DatabaseType::MySQL, "SET ROLE DEFAULT"),
        ] {
            assert_eq!(
                SqlStatementAnalysis::new_for_db_type(db_type, sql).classify_for_db_type(db_type),
                SqlKind::SessionControl,
                "{sql}"
            );
        }
    }

    #[test]
    fn mysql_do_lock_classifier_does_not_hide_other_procedure_effects() {
        for sql in [
            "DO sync_side_effect()",
            "DO GET_LOCK('qt', 0), sync_side_effect()",
            "DO GET_LOCK('qt', 0) OR sync_side_effect()",
            "DO GET_LOCK(CONCAT('qt_', sync_side_effect()), 0)",
            "DO GET_LOCK((SELECT 'qt'), 0)",
            "DO RELEASE_LOCK(CONCAT('qt_', sync_side_effect()))",
            "DO RELEASE_LOCK((SELECT 'qt'))",
            "DO RELEASE_ALL_LOCKS(@unexpected_arg)",
        ] {
            assert_eq!(
                SqlStatementAnalysis::new_for_db_type(DatabaseType::MySQL, sql)
                    .classify_for_db_type(DatabaseType::MySQL),
                SqlKind::PlsqlOrProcedure,
                "{sql}"
            );
        }
    }

    #[test]
    fn nested_locking_selects_are_classified_as_dml_for_session_safety() {
        for (db_type, sql) in [
            (
                DatabaseType::Oracle,
                "WITH locked AS (SELECT * FROM accounts FOR UPDATE) SELECT * FROM locked",
            ),
            (
                DatabaseType::MySQL,
                "WITH locked AS (SELECT * FROM accounts FOR UPDATE) SELECT * FROM locked",
            ),
            (
                DatabaseType::MySQL,
                "SELECT * FROM accounts WHERE id IN (SELECT id FROM locks FOR SHARE)",
            ),
            (
                DatabaseType::MySQL,
                "SELECT * FROM accounts WHERE id IN (SELECT id FROM locks LOCK IN SHARE MODE)",
            ),
        ] {
            assert_eq!(
                SqlStatementAnalysis::new_for_db_type(db_type, sql).classify_for_db_type(db_type),
                SqlKind::Dml,
                "{sql}"
            );
        }
    }

    #[test]
    fn non_locking_select_phrases_remain_select_like() {
        for (db_type, sql) in [
            (
                DatabaseType::Oracle,
                "SELECT 'FOR UPDATE' AS note FROM dual",
            ),
            (
                DatabaseType::Oracle,
                "SELECT * FROM accounts /* FOR UPDATE */",
            ),
            (DatabaseType::MySQL, "SELECT 'LOCK IN SHARE MODE' AS note"),
            (
                DatabaseType::MySQL,
                "SELECT * FROM accounts -- FOR UPDATE\n",
            ),
        ] {
            assert_eq!(
                SqlStatementAnalysis::new_for_db_type(db_type, sql).classify_for_db_type(db_type),
                SqlKind::SelectLike,
                "{sql}"
            );
        }
    }

    #[test]
    fn mixed_set_assignments_classify_session_transaction_controls() {
        for sql in [
            "SET sql_notes = 0, autocommit = 0",
            "SET @qt_note = 'kept', @@session.autocommit = OFF",
            "SET @qt_note = 'kept', SESSION transaction_isolation = 'SERIALIZABLE'",
            "SET autocommit = ON, LOCAL tx_read_only = 1",
            "SET GLOBAL autocommit = ON, SESSION autocommit = OFF",
            "SET PERSIST transaction_read_only = OFF, @@session.tx_isolation = 'SERIALIZABLE'",
        ] {
            assert_eq!(
                SqlStatementAnalysis::new_for_db_type(DatabaseType::MySQL, sql)
                    .classify_for_db_type(DatabaseType::MySQL),
                SqlKind::TransactionControl,
                "{sql}"
            );
        }
    }

    #[test]
    fn mixed_set_assignment_classifier_ignores_user_variables() {
        for sql in [
            "SET @autocommit = 0, @transaction_isolation = 'SERIALIZABLE'",
            "SET @qt_note = '@@session.autocommit = 0'",
            "SET @qt_note = 'kept', GLOBAL transaction_isolation = 'SERIALIZABLE'",
        ] {
            assert_eq!(
                SqlStatementAnalysis::new_for_db_type(DatabaseType::MySQL, sql)
                    .classify_for_db_type(DatabaseType::MySQL),
                SqlKind::Unknown,
                "{sql}"
            );
        }
    }

    #[test]
    fn global_or_persist_only_set_assignments_are_admin_statements() {
        for sql in [
            "SET @@global.autocommit = 0",
            "SET GLOBAL transaction_isolation = 'SERIALIZABLE'",
            "SET @@persist.transaction_read_only = OFF",
            "SET PERSIST_ONLY max_connections = 200",
        ] {
            assert_eq!(
                SqlStatementAnalysis::new_for_db_type(DatabaseType::MySQL, sql)
                    .classify_for_db_type(DatabaseType::MySQL),
                SqlKind::Ddl,
                "{sql}"
            );
        }
    }

    #[test]
    fn set_transaction_classifier_requires_scope_boundaries() {
        for sql in [
            "SET sessionautocommit = 0",
            "SET localtransaction_isolation = 'SERIALIZABLE'",
            "SET globalautocommit = 0",
            "SET @@globalautocommit = 0",
            "SET persist_onlyautocommit = 0",
        ] {
            assert_eq!(
                SqlStatementAnalysis::new_for_db_type(DatabaseType::MySQL, sql)
                    .classify_for_db_type(DatabaseType::MySQL),
                SqlKind::Unknown,
                "{sql}"
            );
        }

        for sql in [
            "SET SESSION autocommit = 0",
            "SET LOCAL transaction_isolation = 'SERIALIZABLE'",
            "SET @@SESSION.autocommit = 1",
            "SET sql_notes = 0, SESSION transaction_read_only = ON",
        ] {
            assert_eq!(
                SqlStatementAnalysis::new_for_db_type(DatabaseType::MySQL, sql)
                    .classify_for_db_type(DatabaseType::MySQL),
                SqlKind::TransactionControl,
                "{sql}"
            );
        }
    }

    #[test]
    fn mysql_xa_statements_are_classified_by_subcommand() {
        for sql in [
            "XA START 'qt-xa'",
            "XA BEGIN 'qt-xa'",
            "XA END 'qt-xa'",
            "XA PREPARE 'qt-xa'",
            "XA COMMIT 'qt-xa'",
            "XA ROLLBACK 'qt-xa'",
        ] {
            assert_eq!(
                SqlStatementAnalysis::new_for_db_type(DatabaseType::MySQL, sql)
                    .classify_for_db_type(DatabaseType::MySQL),
                SqlKind::TransactionControl,
                "{sql}"
            );
        }

        assert_eq!(
            SqlStatementAnalysis::new_for_db_type(DatabaseType::MySQL, "XA RECOVER")
                .classify_for_db_type(DatabaseType::MySQL),
            SqlKind::SelectLike
        );
    }
}
