use mysql::prelude::Queryable;
use mysql::{Error as MysqlError, MySqlError, Params, PooledConn, Row, Value};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::db::{DatabaseType, QueryExecutor, ScriptItem};
use crate::utils::arithmetic::{safe_div, safe_rem};

pub const RESULT_EDIT_SNAPSHOT_COLUMN: &str = "SQ_INTERNAL_EDIT_SNAPSHOT";
const RESULT_EDIT_SNAPSHOT_PREFIX: &str = "\x1eSQ_EDIT_V1:";
pub(crate) const MYSQL_EDIT_KEY_ALIAS_PREFIX: &str = "SQ_INTERNAL_EDIT_KEY_";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResultEditBackendPolicy {
    structured_requests: bool,
    hash_starts_line_comment: bool,
}

impl ResultEditBackendPolicy {
    pub fn supports_structured_requests(self) -> bool {
        self.structured_requests
    }

    fn hash_starts_line_comment(self) -> bool {
        self.hash_starts_line_comment
    }
}

pub fn result_edit_backend_policy(db_type: DatabaseType) -> ResultEditBackendPolicy {
    match db_type {
        DatabaseType::Oracle => ResultEditBackendPolicy {
            structured_requests: false,
            hash_starts_line_comment: false,
        },
        DatabaseType::MySQL => ResultEditBackendPolicy {
            structured_requests: true,
            hash_starts_line_comment: true,
        },
        DatabaseType::MariaDB => ResultEditBackendPolicy {
            structured_requests: true,
            hash_starts_line_comment: true,
        },
    }
}
const MYSQL_UNIQUE_INDEX_METADATA_SQL: &str = r#"
SELECT s.INDEX_NAME, s.COLUMN_NAME, s.SEQ_IN_INDEX, c.IS_NULLABLE
FROM INFORMATION_SCHEMA.STATISTICS s
LEFT JOIN INFORMATION_SCHEMA.COLUMNS c
  ON c.TABLE_SCHEMA = s.TABLE_SCHEMA
 AND c.TABLE_NAME = s.TABLE_NAME
 AND c.COLUMN_NAME = s.COLUMN_NAME
WHERE s.TABLE_SCHEMA = ?
  AND s.TABLE_NAME = ?
  AND s.NON_UNIQUE = 0
ORDER BY s.INDEX_NAME, s.SEQ_IN_INDEX"#;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultEditColumn {
    pub result_index: usize,
    pub source_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultEditDescriptor {
    pub db_type: DatabaseType,
    pub schema_name: String,
    pub table_name: String,
    pub locator_columns: Vec<String>,
    pub editable_columns: Vec<ResultEditColumn>,
    pub snapshot_column_index: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResultEditScalar {
    Null,
    Bytes(Vec<u8>),
    Int(i64),
    UInt(u64),
    FloatBits(u32),
    DoubleBits(u64),
    Date(u16, u8, u8, u8, u8, u8, u32),
    Time(bool, u32, u8, u8, u8, u32),
}

impl ResultEditScalar {
    pub fn from_mysql_value(value: &Value) -> Self {
        match value {
            Value::NULL => Self::Null,
            Value::Bytes(value) => Self::Bytes(value.clone()),
            Value::Int(value) => Self::Int(*value),
            Value::UInt(value) => Self::UInt(*value),
            Value::Float(value) => Self::FloatBits(value.to_bits()),
            Value::Double(value) => Self::DoubleBits(value.to_bits()),
            Value::Date(year, month, day, hour, minute, second, micros) => {
                Self::Date(*year, *month, *day, *hour, *minute, *second, *micros)
            }
            Value::Time(negative, days, hour, minute, second, micros) => {
                Self::Time(*negative, *days, *hour, *minute, *second, *micros)
            }
        }
    }

    pub fn text(value: impl Into<String>) -> Self {
        Self::Bytes(value.into().into_bytes())
    }

    fn to_mysql_value(&self) -> Value {
        match self {
            Self::Null => Value::NULL,
            Self::Bytes(value) => Value::Bytes(value.clone()),
            Self::Int(value) => Value::Int(*value),
            Self::UInt(value) => Value::UInt(*value),
            Self::FloatBits(value) => Value::Float(f32::from_bits(*value)),
            Self::DoubleBits(value) => Value::Double(f64::from_bits(*value)),
            Self::Date(year, month, day, hour, minute, second, micros) => {
                Value::Date(*year, *month, *day, *hour, *minute, *second, *micros)
            }
            Self::Time(negative, days, hour, minute, second, micros) => {
                Value::Time(*negative, *days, *hour, *minute, *second, *micros)
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultEditRowSnapshot {
    pub locator_values: Vec<ResultEditScalar>,
    /// Values are aligned with `ResultEditDescriptor::editable_columns`.
    pub original_values: Vec<ResultEditScalar>,
}

pub fn encode_result_edit_snapshot(snapshot: &ResultEditRowSnapshot) -> Result<String, String> {
    serde_json::to_string(snapshot)
        .map(|json| format!("{RESULT_EDIT_SNAPSHOT_PREFIX}{json}"))
        .map_err(|err| format!("Could not encode the result-row edit snapshot: {err}"))
}

pub fn decode_result_edit_snapshot(value: &str) -> Result<ResultEditRowSnapshot, String> {
    let json = value
        .strip_prefix(RESULT_EDIT_SNAPSHOT_PREFIX)
        .ok_or_else(|| "The result row does not contain a valid edit snapshot.".to_string())?;
    serde_json::from_str(json)
        .map_err(|err| format!("The result-row edit snapshot is invalid: {err}"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResultEditValue {
    Text(String),
    Null,
    Expression(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultEditAssignment {
    pub column_name: String,
    pub value: ResultEditValue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultEditOriginalValue {
    pub column_name: String,
    pub value: ResultEditScalar,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResultEditMutation {
    Update {
        locator_values: Vec<ResultEditScalar>,
        original_values: Vec<ResultEditOriginalValue>,
        assignments: Vec<ResultEditAssignment>,
    },
    Delete {
        locator_values: Vec<ResultEditScalar>,
        original_values: Vec<ResultEditOriginalValue>,
    },
    Insert {
        assignments: Vec<ResultEditAssignment>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultEditRequest {
    pub request_id: u64,
    pub descriptor: ResultEditDescriptor,
    pub mutations: Vec<ResultEditMutation>,
}

impl ResultEditRequest {
    pub fn request_tag(&self) -> String {
        format!("SQ_SAVE_REQUEST:{}", self.request_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MysqlEditableSelectPlan {
    pub sql: String,
    pub schema_name: String,
    pub table_name: String,
    pub locator_columns: Vec<String>,
    pub original_column_count: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MysqlSelectTarget {
    schema_name: Option<String>,
    table_name: String,
    qualifier: String,
    from_index: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MysqlUniqueIndexColumn {
    index_name: String,
    column_name: String,
    sequence: u64,
    nullable: bool,
}

/// Prepares an editable MySQL-family SELECT on the same physical connection
/// that will execute it. Metadata failure deliberately degrades to read-only.
pub fn prepare_mysql_editable_select(
    conn: &mut PooledConn,
    sql: &str,
) -> Option<MysqlEditableSelectPlan> {
    let target = parse_mysql_single_table_select(sql)?;
    let schema_name = match target.schema_name {
        Some(schema) => schema,
        None => match conn.query_first::<Option<String>, _>("SELECT DATABASE()") {
            Ok(Some(Some(schema))) if !schema.trim().is_empty() => schema,
            Ok(_) => return None,
            Err(err) => {
                log_read_only_fallback(&format!("could not resolve current database: {err}"));
                return None;
            }
        },
    };

    let columns = match load_mysql_unique_index_columns(conn, &schema_name, &target.table_name) {
        Ok(columns) => columns,
        Err(err) => {
            log_read_only_fallback(&format!("could not load unique-key metadata: {err}"));
            return None;
        }
    };
    let locator_columns = choose_mysql_locator(&columns)?;
    let mut injection = String::new();
    for (index, column) in locator_columns.iter().enumerate() {
        injection.push_str(", ");
        injection.push_str(&target.qualifier);
        injection.push('.');
        injection.push_str(&quote_mysql_identifier(column));
        injection.push_str(" AS ");
        injection.push_str(&quote_mysql_identifier(&mysql_edit_key_alias(index)));
    }
    let mut rewritten = String::with_capacity(sql.len().saturating_add(injection.len()));
    rewritten.push_str(&sql[..target.from_index]);
    rewritten.push_str(&injection);
    rewritten.push(' ');
    rewritten.push_str(&sql[target.from_index..]);
    Some(MysqlEditableSelectPlan {
        sql: rewritten,
        schema_name,
        table_name: target.table_name,
        locator_columns,
        original_column_count: None,
    })
}

fn log_read_only_fallback(message: &str) {
    crate::utils::logging::log_error(
        "result_edit::mysql_metadata",
        &format!("Result editing disabled for this query: {message}"),
    );
}

fn mysql_edit_key_alias(index: usize) -> String {
    format!("{MYSQL_EDIT_KEY_ALIAS_PREFIX}{index}")
}

fn load_mysql_unique_index_columns(
    conn: &mut PooledConn,
    schema_name: &str,
    table_name: &str,
) -> Result<Vec<MysqlUniqueIndexColumn>, MysqlError> {
    let rows: Vec<(String, Option<String>, u64, Option<String>)> =
        conn.exec(MYSQL_UNIQUE_INDEX_METADATA_SQL, (schema_name, table_name))?;
    Ok(rows
        .into_iter()
        .map(
            |(index_name, column_name, sequence, is_nullable)| MysqlUniqueIndexColumn {
                index_name,
                column_name: column_name.unwrap_or_default(),
                sequence,
                // Functional or otherwise non-column index parts have no
                // COLUMNS match and are never safe row locators.
                nullable: is_nullable
                    .as_deref()
                    .is_none_or(|value| !value.eq_ignore_ascii_case("NO")),
            },
        )
        .collect())
}

fn choose_mysql_locator(columns: &[MysqlUniqueIndexColumn]) -> Option<Vec<String>> {
    let mut by_index: BTreeMap<&str, Vec<&MysqlUniqueIndexColumn>> = BTreeMap::new();
    for column in columns {
        by_index
            .entry(column.index_name.as_str())
            .or_default()
            .push(column);
    }
    let mut candidates = by_index
        .into_iter()
        .filter_map(|(name, mut columns)| {
            columns.sort_by_key(|column| column.sequence);
            if columns.is_empty() || columns.iter().any(|column| column.nullable) {
                return None;
            }
            Some((
                !name.eq_ignore_ascii_case("PRIMARY"),
                columns.len(),
                name.to_ascii_lowercase(),
                columns
                    .into_iter()
                    .map(|column| column.column_name.clone())
                    .collect::<Vec<_>>(),
            ))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        (left.0, left.1, left.2.as_str()).cmp(&(right.0, right.1, right.2.as_str()))
    });
    candidates.into_iter().next().map(|candidate| candidate.3)
}

fn mysql_locator_is_unique(columns: &[MysqlUniqueIndexColumn], locator_columns: &[String]) -> bool {
    let mut by_index: BTreeMap<&str, Vec<&MysqlUniqueIndexColumn>> = BTreeMap::new();
    for column in columns {
        by_index
            .entry(column.index_name.as_str())
            .or_default()
            .push(column);
    }
    by_index.into_values().any(|mut columns| {
        columns.sort_by_key(|column| column.sequence);
        columns.len() == locator_columns.len()
            && !columns.iter().any(|column| column.nullable)
            && columns
                .iter()
                .zip(locator_columns)
                .all(|(column, locator)| column.column_name.eq_ignore_ascii_case(locator))
    })
}

/// Validates the actual wire metadata after key columns were appended and
/// returns the UI descriptor. Computed expressions and columns from another
/// table are intentionally read-only.
pub fn mysql_descriptor_from_columns(
    db_type: DatabaseType,
    plan: &MysqlEditableSelectPlan,
    columns: &[mysql::Column],
) -> Option<ResultEditDescriptor> {
    let visible_count = mysql_original_column_count(plan, columns)?;

    let mut source_name_counts = HashMap::<String, usize>::new();
    let mut candidates = Vec::<(usize, String)>::new();
    for (result_index, column) in columns[..visible_count].iter().enumerate() {
        let schema = column.schema_str();
        let table = column.org_table_str();
        let source_name = column.org_name_str();
        if schema.eq_ignore_ascii_case(&plan.schema_name)
            && table.eq_ignore_ascii_case(&plan.table_name)
            && !source_name.is_empty()
        {
            let normalized = source_name.to_ascii_lowercase();
            *source_name_counts.entry(normalized).or_insert(0) += 1;
            candidates.push((result_index, source_name.to_string()));
        }
    }
    let editable_columns = candidates
        .into_iter()
        .filter(|(_, source_name)| {
            source_name_counts
                .get(&source_name.to_ascii_lowercase())
                .copied()
                == Some(1)
        })
        .map(|(result_index, source_name)| ResultEditColumn {
            result_index,
            source_name,
        })
        .collect::<Vec<_>>();
    if editable_columns.is_empty() {
        return None;
    }
    Some(ResultEditDescriptor {
        db_type,
        schema_name: plan.schema_name.clone(),
        table_name: plan.table_name.clone(),
        locator_columns: plan.locator_columns.clone(),
        editable_columns,
        snapshot_column_index: visible_count,
    })
}

pub fn mysql_original_column_count(
    plan: &MysqlEditableSelectPlan,
    columns: &[mysql::Column],
) -> Option<usize> {
    let key_count = plan.locator_columns.len();
    let visible_count = columns.len().checked_sub(key_count)?;
    for index in 0..key_count {
        let column = columns.get(visible_count + index)?;
        if !column
            .name_str()
            .eq_ignore_ascii_case(&mysql_edit_key_alias(index))
        {
            return None;
        }
    }
    Some(visible_count)
}

pub fn mysql_snapshot_for_row(
    descriptor: &ResultEditDescriptor,
    row: &Row,
) -> Result<ResultEditRowSnapshot, String> {
    let key_count = descriptor.locator_columns.len();
    let expected_len = descriptor.snapshot_column_index.saturating_add(key_count);
    if row.len() != expected_len {
        return Err("The editable result row has an unexpected column count.".to_string());
    }
    let locator_values = (descriptor.snapshot_column_index..expected_len)
        .map(|index| {
            row.as_ref(index)
                .map(ResultEditScalar::from_mysql_value)
                .ok_or_else(|| "An injected key value is missing from the result row.".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if locator_values
        .iter()
        .any(|value| matches!(value, ResultEditScalar::Null))
    {
        return Err("An injected edit key unexpectedly contains NULL.".to_string());
    }
    let original_values = descriptor
        .editable_columns
        .iter()
        .map(|column| {
            row.as_ref(column.result_index)
                .map(ResultEditScalar::from_mysql_value)
                .ok_or_else(|| {
                    "An editable source value is missing from the result row.".to_string()
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ResultEditRowSnapshot {
        locator_values,
        original_values,
    })
}

pub fn validate_result_edit_expression(
    expression: &str,
    db_type: DatabaseType,
) -> Result<String, String> {
    let normalized = expression.trim();
    if normalized.is_empty() {
        return Err("SQL expression after '=' cannot be empty.".to_string());
    }
    if normalized.contains(';')
        || normalized.contains("--")
        || normalized.contains("/*")
        || normalized.contains("*/")
        || QueryExecutor::result_edit_expression_has_top_level_comma(normalized)
        || (result_edit_backend_policy(db_type).hash_starts_line_comment()
            && normalized.contains('#'))
    {
        return Err(
            "SQL expression cannot contain statement/comment delimiters or a top-level comma."
                .to_string(),
        );
    }
    let items = QueryExecutor::split_script_items_for_db_type(normalized, Some(db_type));
    if items.len() != 1 || !matches!(items.first(), Some(ScriptItem::Statement(_))) {
        return Err("SQL expression must contain exactly one expression.".to_string());
    }
    Ok(normalized.to_string())
}

pub fn compile_oracle_guarded_result_edit(statements: &[String]) -> Result<String, String> {
    if statements.is_empty() {
        return Err("The Oracle result edit contains no mutations.".to_string());
    }
    let guarded_statements = statements
        .iter()
        .map(|statement| statement.trim().trim_end_matches(';').trim())
        .map(|statement| {
            format!(
                "{statement};\nIF SQL%ROWCOUNT <> 1 THEN\n  RAISE_APPLICATION_ERROR(-20001, 'Result edit conflict: expected exactly one row');\nEND IF;"
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!(
        "BEGIN\nSAVEPOINT SQ_RESULT_EDIT;\n{guarded_statements}\nEXCEPTION\n  WHEN OTHERS THEN\n    ROLLBACK TO SQ_RESULT_EDIT;\n    RAISE;\nEND;"
    ))
}

/// Executes a result-edit request atomically. Existing rows are locked and
/// compared before any DML runs, and lock acquisition order is deterministic.
/// `scope` decides how this save brackets itself, and it is NOT the tab's
/// auto-commit flag: `START TRANSACTION` implicitly commits whatever the session
/// already holds, and an auto-commit tab can hold an explicit transaction of the
/// user's. See [`crate::db::AppOperationTransactionScope`].
pub fn execute_mysql_result_edit(
    conn: &mut PooledConn,
    request: &ResultEditRequest,
    scope: crate::db::AppOperationTransactionScope,
    mut is_cancelled: impl FnMut() -> bool,
) -> Result<usize, MysqlError> {
    if !result_edit_backend_policy(request.descriptor.db_type).supports_structured_requests() {
        return Err(mysql_edit_error(
            "The result edit request is not for a MySQL-family database.",
        ));
    }
    validate_request_shape(request).map_err(|message| mysql_edit_error(&message))?;
    let savepoint = format!("SQ_RESULT_EDIT_{}", request.request_id);
    let owns_the_transaction = scope.commits_its_own_work();
    if owns_the_transaction {
        conn.query_drop("START TRANSACTION")?;
    } else {
        conn.query_drop(format!("SAVEPOINT {savepoint}"))?;
    }

    let execution = (|| -> Result<usize, MysqlError> {
        let mut existing = request
            .mutations
            .iter()
            .filter(|mutation| !matches!(mutation, ResultEditMutation::Insert { .. }))
            .collect::<Vec<_>>();
        existing.sort_by_key(|mutation| mutation_locator_sort_key(mutation));
        let has_existing_mutations = !existing.is_empty();
        for mutation in existing {
            if is_cancelled() {
                return Err(mysql_edit_error("Result edit was cancelled."));
            }
            prelock_and_compare_mysql_row(conn, &request.descriptor, mutation)?;
        }
        if has_existing_mutations {
            let current_indexes = load_mysql_unique_index_columns(
                conn,
                &request.descriptor.schema_name,
                &request.descriptor.table_name,
            )?;
            if !mysql_locator_is_unique(&current_indexes, &request.descriptor.locator_columns) {
                return Err(mysql_conflict_error(
                    "The edit key is no longer protected by a non-null unique index.",
                ));
            }
        }

        let mut applied = 0usize;
        for mutation in &request.mutations {
            if is_cancelled() {
                return Err(mysql_edit_error("Result edit was cancelled."));
            }
            execute_mysql_mutation(conn, &request.descriptor, mutation)?;
            applied = applied.saturating_add(1);
        }
        if is_cancelled() {
            return Err(mysql_edit_error("Result edit was cancelled."));
        }
        if owns_the_transaction {
            conn.query_drop("COMMIT")?;
        } else {
            conn.query_drop(format!("RELEASE SAVEPOINT {savepoint}"))?;
        }
        Ok(applied)
    })();

    if let Err(error) = execution {
        let rollback_result = if owns_the_transaction {
            conn.query_drop("ROLLBACK")
        } else {
            conn.query_drop(format!("ROLLBACK TO SAVEPOINT {savepoint}"))
                .and_then(|_| conn.query_drop(format!("RELEASE SAVEPOINT {savepoint}")))
        };
        return match rollback_result {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(mysql_edit_error(&format!(
                "{error}; rollback of the result edit also failed: {rollback_error}"
            ))),
        };
    }
    execution
}

fn validate_request_shape(request: &ResultEditRequest) -> Result<(), String> {
    let descriptor = &request.descriptor;
    if descriptor.schema_name.is_empty()
        || descriptor.table_name.is_empty()
        || descriptor.locator_columns.is_empty()
        || descriptor.editable_columns.is_empty()
    {
        return Err("The result edit descriptor is incomplete.".to_string());
    }
    if request.mutations.is_empty() {
        return Err("The result edit contains no mutations.".to_string());
    }
    let allowed_columns = descriptor
        .editable_columns
        .iter()
        .map(|column| column.source_name.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let locator_columns = descriptor
        .locator_columns
        .iter()
        .map(|column| column.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    if allowed_columns.len() != descriptor.editable_columns.len()
        || allowed_columns.contains("")
        || locator_columns.len() != descriptor.locator_columns.len()
        || locator_columns.contains("")
    {
        return Err("The result edit descriptor contains duplicate or empty columns.".to_string());
    }
    let mut seen_locators = HashSet::new();
    for mutation in &request.mutations {
        match mutation {
            ResultEditMutation::Update {
                locator_values,
                original_values,
                assignments,
            } => {
                validate_locator_count(descriptor, locator_values)?;
                validate_named_values(original_values, &allowed_columns)?;
                validate_assignments(assignments, &allowed_columns, descriptor.db_type)?;
                if assignments.is_empty() {
                    return Err("An UPDATE edit has no assignments.".to_string());
                }
                let original_columns = original_values
                    .iter()
                    .map(|value| value.column_name.to_ascii_lowercase())
                    .collect::<HashSet<_>>();
                if assignments.len() != original_columns.len()
                    || assignments.iter().any(|assignment| {
                        !original_columns.contains(&assignment.column_name.to_ascii_lowercase())
                    })
                {
                    return Err(
                        "Every UPDATE assignment requires its exact original value.".to_string()
                    );
                }
                if !seen_locators.insert(locator_values.clone()) {
                    return Err("A result edit contains the same row locator twice.".to_string());
                }
            }
            ResultEditMutation::Delete {
                locator_values,
                original_values,
            } => {
                validate_locator_count(descriptor, locator_values)?;
                validate_named_values(original_values, &allowed_columns)?;
                if original_values.len() != allowed_columns.len() {
                    return Err(
                        "A DELETE edit must verify every displayed source column.".to_string()
                    );
                }
                if !seen_locators.insert(locator_values.clone()) {
                    return Err("A result edit contains the same row locator twice.".to_string());
                }
            }
            ResultEditMutation::Insert { assignments } => {
                validate_assignments(assignments, &allowed_columns, descriptor.db_type)?;
                if assignments.is_empty() {
                    return Err("An INSERT edit has no assignments.".to_string());
                }
            }
        }
    }
    Ok(())
}

fn validate_locator_count(
    descriptor: &ResultEditDescriptor,
    values: &[ResultEditScalar],
) -> Result<(), String> {
    if values.len() != descriptor.locator_columns.len()
        || values
            .iter()
            .any(|value| matches!(value, ResultEditScalar::Null))
    {
        return Err("The result edit row locator is invalid.".to_string());
    }
    Ok(())
}

fn validate_named_values(
    values: &[ResultEditOriginalValue],
    allowed: &HashSet<String>,
) -> Result<(), String> {
    let mut seen = HashSet::new();
    for value in values {
        let normalized = value.column_name.to_ascii_lowercase();
        if !allowed.contains(&normalized) || !seen.insert(normalized) {
            return Err("The result edit contains an invalid original-value column.".to_string());
        }
    }
    Ok(())
}

fn validate_assignments(
    assignments: &[ResultEditAssignment],
    allowed: &HashSet<String>,
    db_type: DatabaseType,
) -> Result<(), String> {
    let mut seen = HashSet::new();
    for assignment in assignments {
        let normalized = assignment.column_name.to_ascii_lowercase();
        if !allowed.contains(&normalized) || !seen.insert(normalized) {
            return Err("The result edit contains an invalid assignment column.".to_string());
        }
        if let ResultEditValue::Expression(expression) = &assignment.value {
            validate_result_edit_expression(expression, db_type)?;
        }
    }
    Ok(())
}

fn mutation_locator_sort_key(mutation: &ResultEditMutation) -> String {
    let values = match mutation {
        ResultEditMutation::Update { locator_values, .. }
        | ResultEditMutation::Delete { locator_values, .. } => locator_values,
        ResultEditMutation::Insert { .. } => return String::new(),
    };
    serde_json::to_string(values).unwrap_or_default()
}

fn prelock_and_compare_mysql_row(
    conn: &mut PooledConn,
    descriptor: &ResultEditDescriptor,
    mutation: &ResultEditMutation,
) -> Result<(), MysqlError> {
    let (locator_values, originals) = match mutation {
        ResultEditMutation::Update {
            locator_values,
            original_values,
            ..
        }
        | ResultEditMutation::Delete {
            locator_values,
            original_values,
        } => (locator_values, original_values),
        ResultEditMutation::Insert { .. } => return Ok(()),
    };
    let projected_columns = if originals.is_empty() {
        vec![descriptor.locator_columns[0].clone()]
    } else {
        originals
            .iter()
            .map(|value| value.column_name.clone())
            .collect::<Vec<_>>()
    };
    let sql = format!(
        "SELECT {} FROM {} WHERE {} LIMIT 2 FOR UPDATE",
        projected_columns
            .iter()
            .map(|column| quote_mysql_identifier(column))
            .collect::<Vec<_>>()
            .join(", "),
        quote_mysql_table(descriptor),
        mysql_locator_predicate(descriptor)
    );
    let params = Params::Positional(
        locator_values
            .iter()
            .map(ResultEditScalar::to_mysql_value)
            .collect(),
    );
    let rows: Vec<Row> = conn.exec(sql, params)?;
    if rows.len() != 1 {
        return Err(mysql_conflict_error(
            "The target row no longer exists or the edit key is not unique.",
        ));
    }
    if !originals.is_empty() {
        let row = &rows[0];
        for (index, original) in originals.iter().enumerate() {
            let actual = row
                .as_ref(index)
                .map(ResultEditScalar::from_mysql_value)
                .ok_or_else(|| mysql_edit_error("A locked source value is missing."))?;
            if !mysql_scalars_equal(&actual, &original.value) {
                return Err(mysql_conflict_error(
                    "The target row changed after it was queried. No edits were applied.",
                ));
            }
        }
    }
    Ok(())
}

fn mysql_scalars_equal(left: &ResultEditScalar, right: &ResultEditScalar) -> bool {
    if left == right {
        return true;
    }
    match (left, right) {
        (ResultEditScalar::Int(left), ResultEditScalar::UInt(right))
        | (ResultEditScalar::UInt(right), ResultEditScalar::Int(left)) => {
            u64::try_from(*left).ok() == Some(*right)
        }
        (ResultEditScalar::Bytes(bytes), scalar) | (scalar, ResultEditScalar::Bytes(bytes)) => {
            mysql_text_matches_scalar(bytes, scalar)
        }
        (ResultEditScalar::FloatBits(left), ResultEditScalar::DoubleBits(right))
        | (ResultEditScalar::DoubleBits(right), ResultEditScalar::FloatBits(left)) => {
            mysql_floats_equal(f32::from_bits(*left) as f64, f64::from_bits(*right))
        }
        _ => false,
    }
}

fn mysql_text_matches_scalar(bytes: &[u8], scalar: &ResultEditScalar) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    match scalar {
        ResultEditScalar::Bytes(other) => bytes == other,
        ResultEditScalar::Int(value) => text.parse::<i64>().ok() == Some(*value),
        ResultEditScalar::UInt(value) => text.parse::<u64>().ok() == Some(*value),
        ResultEditScalar::FloatBits(value) => text
            .parse::<f64>()
            .ok()
            .is_some_and(|parsed| mysql_floats_equal(parsed, f32::from_bits(*value) as f64)),
        ResultEditScalar::DoubleBits(value) => text
            .parse::<f64>()
            .ok()
            .is_some_and(|parsed| mysql_floats_equal(parsed, f64::from_bits(*value))),
        ResultEditScalar::Date(year, month, day, hour, minute, second, micros) => {
            parse_mysql_date_text(text)
                == Some((*year, *month, *day, *hour, *minute, *second, *micros))
        }
        ResultEditScalar::Time(negative, days, hour, minute, second, micros) => {
            parse_mysql_time_text(text)
                == Some((*negative, *days, *hour, *minute, *second, *micros))
        }
        ResultEditScalar::Null => false,
    }
}

fn mysql_floats_equal(left: f64, right: f64) -> bool {
    left == right || (left.is_nan() && right.is_nan())
}

fn parse_mysql_date_text(text: &str) -> Option<(u16, u8, u8, u8, u8, u8, u32)> {
    let (date, time) = text.split_once(' ').unwrap_or((text, ""));
    let mut date_parts = date.split('-');
    let year = date_parts.next()?.parse().ok()?;
    let month = date_parts.next()?.parse().ok()?;
    let day = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() {
        return None;
    }
    if time.is_empty() {
        return Some((year, month, day, 0, 0, 0, 0));
    }
    let (hour, minute, second, micros) = parse_mysql_clock_text(time)?;
    Some((year, month, day, hour, minute, second, micros))
}

fn parse_mysql_time_text(text: &str) -> Option<(bool, u32, u8, u8, u8, u32)> {
    let (negative, text) = match text.strip_prefix('-') {
        Some(text) => (true, text),
        None => (false, text),
    };
    let mut parts = text.split(':');
    let total_hours = parts.next()?.parse::<u32>().ok()?;
    let minute = parts.next()?.parse::<u8>().ok()?;
    let second_text = parts.next()?;
    if parts.next().is_some() || minute > 59 {
        return None;
    }
    let (second, micros) = parse_mysql_second_text(second_text)?;
    let days = safe_div(total_hours, 24);
    let hour = u8::try_from(safe_rem(total_hours, 24)).ok()?;
    Some((negative, days, hour, minute, second, micros))
}

fn parse_mysql_clock_text(text: &str) -> Option<(u8, u8, u8, u32)> {
    let mut parts = text.split(':');
    let hour = parts.next()?.parse::<u8>().ok()?;
    let minute = parts.next()?.parse::<u8>().ok()?;
    let second_text = parts.next()?;
    if parts.next().is_some() || hour > 23 || minute > 59 {
        return None;
    }
    let (second, micros) = parse_mysql_second_text(second_text)?;
    Some((hour, minute, second, micros))
}

fn parse_mysql_second_text(text: &str) -> Option<(u8, u32)> {
    let (seconds, fraction) = text.split_once('.').unwrap_or((text, ""));
    let seconds = seconds.parse::<u8>().ok()?;
    if seconds > 59 || fraction.len() > 6 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let micros = if fraction.is_empty() {
        0
    } else {
        let parsed = fraction.parse::<u32>().ok()?;
        parsed.checked_mul(10_u32.pow(6_u32.saturating_sub(fraction.len() as u32)))?
    };
    Some((seconds, micros))
}

fn execute_mysql_mutation(
    conn: &mut PooledConn,
    descriptor: &ResultEditDescriptor,
    mutation: &ResultEditMutation,
) -> Result<(), MysqlError> {
    match mutation {
        ResultEditMutation::Update {
            locator_values,
            assignments,
            ..
        } => {
            let (set_sql, mut params) = mysql_assignments(assignments, descriptor.db_type)?;
            params.extend(locator_values.iter().map(ResultEditScalar::to_mysql_value));
            let sql = format!(
                "UPDATE {} SET {} WHERE {} LIMIT 1",
                quote_mysql_table(descriptor),
                set_sql,
                mysql_locator_predicate(descriptor)
            );
            conn.exec_drop(sql, Params::Positional(params))?;
            if conn.affected_rows() > 1 {
                return Err(mysql_edit_error("An UPDATE affected more than one row."));
            }
        }
        ResultEditMutation::Delete { locator_values, .. } => {
            let sql = format!(
                "DELETE FROM {} WHERE {} LIMIT 1",
                quote_mysql_table(descriptor),
                mysql_locator_predicate(descriptor)
            );
            conn.exec_drop(
                sql,
                Params::Positional(
                    locator_values
                        .iter()
                        .map(ResultEditScalar::to_mysql_value)
                        .collect(),
                ),
            )?;
            if conn.affected_rows() != 1 {
                return Err(mysql_conflict_error(
                    "A DELETE did not affect exactly one row. No edits were applied.",
                ));
            }
        }
        ResultEditMutation::Insert { assignments } => {
            let mut columns = Vec::with_capacity(assignments.len());
            let mut values: Vec<String> = Vec::with_capacity(assignments.len());
            let mut params = Vec::new();
            for assignment in assignments {
                columns.push(quote_mysql_identifier(&assignment.column_name));
                match &assignment.value {
                    ResultEditValue::Text(value) => {
                        values.push("?".to_string());
                        params.push(Value::Bytes(value.as_bytes().to_vec()));
                    }
                    ResultEditValue::Null => values.push("NULL".to_string()),
                    ResultEditValue::Expression(expression) => {
                        values.push(
                            validate_result_edit_expression(expression, descriptor.db_type)
                                .map_err(|message| mysql_edit_error(&message))?,
                        );
                    }
                }
            }
            let sql = format!(
                "INSERT INTO {} ({}) VALUES ({})",
                quote_mysql_table(descriptor),
                columns.join(", "),
                values.join(", ")
            );
            conn.exec_drop(sql, Params::Positional(params))?;
            if conn.affected_rows() != 1 {
                return Err(mysql_edit_error(
                    "An INSERT did not affect exactly one row.",
                ));
            }
        }
    }
    Ok(())
}

fn mysql_assignments(
    assignments: &[ResultEditAssignment],
    db_type: DatabaseType,
) -> Result<(String, Vec<Value>), MysqlError> {
    let mut sql = Vec::with_capacity(assignments.len());
    let mut params = Vec::new();
    for assignment in assignments {
        let column = quote_mysql_identifier(&assignment.column_name);
        match &assignment.value {
            ResultEditValue::Text(value) => {
                sql.push(format!("{column} = ?"));
                params.push(Value::Bytes(value.as_bytes().to_vec()));
            }
            ResultEditValue::Null => sql.push(format!("{column} = NULL")),
            ResultEditValue::Expression(expression) => {
                let expression = validate_result_edit_expression(expression, db_type)
                    .map_err(|message| mysql_edit_error(&message))?;
                sql.push(format!("{column} = {expression}"));
            }
        }
    }
    Ok((sql.join(", "), params))
}

fn mysql_locator_predicate(descriptor: &ResultEditDescriptor) -> String {
    descriptor
        .locator_columns
        .iter()
        .map(|column| format!("{} <=> ?", quote_mysql_identifier(column)))
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn quote_mysql_table(descriptor: &ResultEditDescriptor) -> String {
    format!(
        "{}.{}",
        quote_mysql_identifier(&descriptor.schema_name),
        quote_mysql_identifier(&descriptor.table_name)
    )
}

pub(crate) fn quote_mysql_identifier(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}

fn mysql_edit_error(message: &str) -> MysqlError {
    MysqlError::MySqlError(MySqlError {
        state: "45000".to_string(),
        message: message.to_string(),
        code: 1644,
    })
}

fn mysql_conflict_error(message: &str) -> MysqlError {
    mysql_edit_error(&format!("Edit conflict: {message}"))
}

fn parse_mysql_single_table_select(sql: &str) -> Option<MysqlSelectTarget> {
    let trimmed_start = sql.len().saturating_sub(sql.trim_start().len());
    let trimmed = sql.trim_start();
    if !trimmed
        .get(..6)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("SELECT"))
        || trimmed
            .get(6..)
            .and_then(|tail| tail.chars().next())
            .is_some_and(is_identifier_char)
        || !QueryExecutor::is_rowid_edit_eligible_query(trimmed)
    {
        return None;
    }
    let from_relative = find_top_level_word(trimmed, "FROM")?;
    if find_top_level_word(trimmed, "INTO").is_some() {
        return None;
    }
    let from_index = trimmed_start.saturating_add(from_relative);
    let from_tail = trimmed.get(from_relative + 4..)?.trim_start();
    if from_tail.starts_with('(') {
        return None;
    }
    let (relation, consumed) = parse_mysql_qualified_identifier(from_tail)?;
    let mut remainder = from_tail.get(consumed..)?.trim_start();
    if word_prefix(remainder, "PARTITION")
        || word_prefix(remainder, "TABLESAMPLE")
        || (word_prefix(remainder, "FOR")
            && remainder
                .get(3..)
                .map(str::trim_start)
                .is_some_and(|tail| word_prefix(tail, "SYSTEM_TIME")))
    {
        return None;
    }
    let alias = if word_prefix(remainder, "AS") {
        remainder = remainder.get(2..)?.trim_start();
        let (alias, _used) = parse_mysql_identifier(remainder)?;
        Some(alias)
    } else if let Some((candidate, _used)) = parse_mysql_identifier(remainder) {
        if is_mysql_clause_keyword(&unquote_mysql_identifier(&candidate)) {
            None
        } else {
            Some(candidate)
        }
    } else {
        None
    };
    let parts = split_qualified_identifier(&relation)?;
    if parts.is_empty() || parts.len() > 2 {
        return None;
    }
    let table_name = unquote_mysql_identifier(parts.last()?);
    let schema_name = (parts.len() == 2).then(|| unquote_mysql_identifier(parts[0]));
    let qualifier = alias.unwrap_or_else(|| relation.clone());
    Some(MysqlSelectTarget {
        schema_name,
        table_name,
        qualifier,
        from_index,
    })
}

fn find_top_level_word(sql: &str, target: &str) -> Option<usize> {
    let bytes = sql.as_bytes();
    let mut index = 0usize;
    let mut depth = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => {
                index = skip_quoted(bytes, index, bytes[index])?;
            }
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                index += 2;
                while index < bytes.len() && !matches!(bytes[index], b'\n' | b'\r') {
                    index += 1;
                }
            }
            b'#' => {
                index += 1;
                while index < bytes.len() && !matches!(bytes[index], b'\n' | b'\r') {
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                let tail = sql.get(index + 2..)?;
                index = index + 2 + tail.find("*/")? + 2;
            }
            b'(' => {
                depth = depth.saturating_add(1);
                index += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            byte if depth == 0 && is_identifier_byte(byte) => {
                let start = index;
                index += 1;
                while index < bytes.len() && is_identifier_byte(bytes[index]) {
                    index += 1;
                }
                if sql
                    .get(start..index)
                    .is_some_and(|word| word.eq_ignore_ascii_case(target))
                {
                    return Some(start);
                }
            }
            _ => index += 1,
        }
    }
    None
}

fn skip_quoted(bytes: &[u8], mut index: usize, quote: u8) -> Option<usize> {
    index += 1;
    while index < bytes.len() {
        if bytes[index] == quote {
            if bytes.get(index + 1) == Some(&quote) {
                index += 2;
                continue;
            }
            return Some(index + 1);
        }
        if bytes[index] == b'\\' {
            index = index.saturating_add(2);
        } else {
            index += 1;
        }
    }
    None
}

fn parse_mysql_qualified_identifier(text: &str) -> Option<(String, usize)> {
    let (first, mut used) = parse_mysql_identifier(text)?;
    let mut relation = first;
    loop {
        let after = text.get(used..)?;
        let whitespace = after.len().saturating_sub(after.trim_start().len());
        let after = after.trim_start();
        if !after.starts_with('.') {
            break;
        }
        used = used.saturating_add(whitespace).saturating_add(1);
        let after_dot = text.get(used..)?;
        let whitespace = after_dot.len().saturating_sub(after_dot.trim_start().len());
        used = used.saturating_add(whitespace);
        let (part, part_used) = parse_mysql_identifier(text.get(used..)?)?;
        relation.push('.');
        relation.push_str(&part);
        used = used.saturating_add(part_used);
    }
    Some((relation, used))
}

fn parse_mysql_identifier(text: &str) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    let first = *bytes.first()?;
    if matches!(first, b'`' | b'"') {
        let end = skip_quoted(bytes, 0, first)?;
        return Some((text.get(..end)?.to_string(), end));
    }
    if !is_identifier_byte(first) {
        return None;
    }
    let end = bytes
        .iter()
        .position(|byte| !is_identifier_byte(*byte))
        .unwrap_or(bytes.len());
    Some((text.get(..end)?.to_string(), end))
}

fn split_qualified_identifier(identifier: &str) -> Option<Vec<&str>> {
    let bytes = identifier.as_bytes();
    let mut start = 0usize;
    let mut index = 0usize;
    let mut parts = Vec::new();
    while index < bytes.len() {
        if matches!(bytes[index], b'`' | b'"') {
            index = skip_quoted(bytes, index, bytes[index])?;
        } else if bytes[index] == b'.' {
            parts.push(identifier.get(start..index)?);
            start = index + 1;
            index += 1;
        } else {
            index += 1;
        }
    }
    parts.push(identifier.get(start..)?);
    Some(parts)
}

fn unquote_mysql_identifier(identifier: &str) -> String {
    let identifier = identifier.trim();
    if identifier.len() >= 2 {
        let first = identifier.as_bytes()[0];
        let last = identifier.as_bytes()[identifier.len() - 1];
        if (first == b'`' && last == b'`') || (first == b'"' && last == b'"') {
            let inner = &identifier[1..identifier.len() - 1];
            let doubled = if first == b'`' { "``" } else { "\"\"" };
            let single = if first == b'`' { "`" } else { "\"" };
            return inner.replace(doubled, single);
        }
    }
    identifier.to_string()
}

fn word_prefix(text: &str, word: &str) -> bool {
    text.get(..word.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(word))
        && text
            .get(word.len()..)
            .and_then(|tail| tail.chars().next())
            .is_none_or(|ch| !is_identifier_char(ch))
}

fn is_mysql_clause_keyword(word: &str) -> bool {
    [
        "WHERE",
        "ORDER",
        "LIMIT",
        "OFFSET",
        "FOR",
        "LOCK",
        "GROUP",
        "HAVING",
        "WINDOW",
        "USE",
        "FORCE",
        "IGNORE",
        "PARTITION",
        "JOIN",
        "INNER",
        "LEFT",
        "RIGHT",
        "CROSS",
        "NATURAL",
        "STRAIGHT_JOIN",
    ]
    .iter()
    .any(|keyword| word.eq_ignore_ascii_case(keyword))
}

fn is_identifier_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$')
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')
}

#[cfg(test)]
mod tests {
    use super::*;
    use mysql::{Opts, Pool};

    fn index_column(
        index_name: &str,
        column_name: &str,
        sequence: u64,
        nullable: bool,
    ) -> MysqlUniqueIndexColumn {
        MysqlUniqueIndexColumn {
            index_name: index_name.to_string(),
            column_name: column_name.to_string(),
            sequence,
            nullable,
        }
    }

    #[test]
    fn snapshot_round_trip_preserves_mysql_scalar_types() {
        let snapshot = ResultEditRowSnapshot {
            locator_values: vec![ResultEditScalar::UInt(u64::MAX)],
            original_values: vec![
                ResultEditScalar::Null,
                ResultEditScalar::Bytes(vec![0, 255, 42]),
                ResultEditScalar::DoubleBits(f64::NAN.to_bits()),
                ResultEditScalar::Date(2026, 8, 3, 1, 2, 3, 4),
            ],
        };
        let encoded = encode_result_edit_snapshot(&snapshot).expect("encode snapshot");
        assert_eq!(decode_result_edit_snapshot(&encoded), Ok(snapshot));
    }

    #[test]
    fn locator_prefers_primary_then_smallest_not_null_unique_key() {
        let columns = vec![
            index_column("uq_wide", "b", 2, false),
            index_column("uq_wide", "a", 1, false),
            index_column("PRIMARY", "id2", 2, false),
            index_column("PRIMARY", "id1", 1, false),
            index_column("uq_short", "code", 1, false),
        ];
        assert_eq!(
            choose_mysql_locator(&columns),
            Some(vec!["id1".to_string(), "id2".to_string()])
        );
        assert!(mysql_locator_is_unique(
            &columns,
            &["id1".to_string(), "id2".to_string()]
        ));
        assert!(!mysql_locator_is_unique(
            &columns,
            &["id2".to_string(), "id1".to_string()]
        ));

        let no_primary = columns
            .into_iter()
            .filter(|column| column.index_name != "PRIMARY")
            .collect::<Vec<_>>();
        assert_eq!(
            choose_mysql_locator(&no_primary),
            Some(vec!["code".to_string()])
        );
    }

    #[test]
    fn locator_rejects_nullable_unique_key() {
        let columns = vec![index_column("uq_email", "email", 1, true)];
        assert_eq!(choose_mysql_locator(&columns), None);
    }

    #[test]
    fn parses_safe_single_table_mysql_select() {
        let target = parse_mysql_single_table_select(
            "SELECT u.id, upper(u.name) AS n FROM `app`.`users` AS u WHERE u.active = 1",
        )
        .expect("editable target");
        assert_eq!(target.schema_name.as_deref(), Some("app"));
        assert_eq!(target.table_name, "users");
        assert_eq!(target.qualifier, "u");
    }

    #[test]
    fn mysql_select_parser_ignores_from_inside_literals_and_hash_comments() {
        let escaped_literal =
            parse_mysql_single_table_select(r#"SELECT "not a \" FROM fake", u.id FROM users u"#)
                .expect("escaped literal must not change the target table");
        assert_eq!(escaped_literal.table_name, "users");
        assert_eq!(escaped_literal.qualifier, "u");

        let hash_comment = parse_mysql_single_table_select("SELECT id # FROM fake\nFROM users")
            .expect("hash comment must not change the target table");
        assert_eq!(hash_comment.table_name, "users");
    }

    #[test]
    fn rejects_queries_that_cannot_preserve_single_table_result_semantics() {
        assert!(
            parse_mysql_single_table_select("SELECT a.id FROM a JOIN b ON b.id = a.id").is_none()
        );
        assert!(parse_mysql_single_table_select("SELECT DISTINCT id FROM a").is_none());
        assert!(parse_mysql_single_table_select("SELECT DISTINCTROW id FROM a").is_none());
        assert!(parse_mysql_single_table_select("SELECT id INTO @saved_id FROM a").is_none());
        assert!(parse_mysql_single_table_select("SELECT id FROM a INTO @saved_id").is_none());
        assert!(parse_mysql_single_table_select("SELECT id FROM a PARTITION (p0)").is_none());
        assert!(parse_mysql_single_table_select("SELECT id FROM a FOR SYSTEM_TIME ALL").is_none());
    }

    #[test]
    fn expression_validation_rejects_statement_delimiters() {
        assert!(validate_result_edit_expression("NOW()", DatabaseType::MySQL).is_ok());
        assert!(validate_result_edit_expression("1; DELETE FROM t", DatabaseType::MySQL).is_err());
        assert!(validate_result_edit_expression("1 /* comment */", DatabaseType::MySQL).is_err());
        assert!(validate_result_edit_expression("1, other_col = 2", DatabaseType::MySQL).is_err());
        assert!(
            validate_result_edit_expression("COALESCE(value_col, 0)", DatabaseType::MySQL).is_ok()
        );
        assert!(
            validate_result_edit_expression("1 # remove row guard", DatabaseType::MySQL).is_err()
        );
    }

    #[test]
    fn mysql_protocol_scalar_equivalence_is_strict_by_value_type() {
        assert!(mysql_scalars_equal(
            &ResultEditScalar::Bytes(b"42".to_vec()),
            &ResultEditScalar::Int(42)
        ));
        assert!(mysql_scalars_equal(
            &ResultEditScalar::Bytes(b"2026-08-03 01:02:03.004".to_vec()),
            &ResultEditScalar::Date(2026, 8, 3, 1, 2, 3, 4_000)
        ));
        assert!(mysql_scalars_equal(
            &ResultEditScalar::Bytes(b"-25:02:03.4".to_vec()),
            &ResultEditScalar::Time(true, 1, 1, 2, 3, 400_000)
        ));
        assert!(!mysql_scalars_equal(
            &ResultEditScalar::Bytes(b"42x".to_vec()),
            &ResultEditScalar::Int(42)
        ));
        assert!(!mysql_scalars_equal(
            &ResultEditScalar::Null,
            &ResultEditScalar::Bytes(Vec::new())
        ));
    }

    #[test]
    fn request_validation_requires_originals_and_unique_row_locators() {
        let descriptor = ResultEditDescriptor {
            db_type: DatabaseType::MySQL,
            schema_name: "sqtest".to_string(),
            table_name: "items".to_string(),
            locator_columns: vec!["id".to_string()],
            editable_columns: vec![ResultEditColumn {
                result_index: 0,
                source_name: "value_text".to_string(),
            }],
            snapshot_column_index: 1,
        };
        let missing_original = ResultEditRequest {
            request_id: 1,
            descriptor: descriptor.clone(),
            mutations: vec![ResultEditMutation::Update {
                locator_values: vec![ResultEditScalar::Int(1)],
                original_values: Vec::new(),
                assignments: vec![ResultEditAssignment {
                    column_name: "value_text".to_string(),
                    value: ResultEditValue::Text("new".to_string()),
                }],
            }],
        };
        assert!(validate_request_shape(&missing_original).is_err());

        let delete = || ResultEditMutation::Delete {
            locator_values: vec![ResultEditScalar::Int(1)],
            original_values: vec![ResultEditOriginalValue {
                column_name: "value_text".to_string(),
                value: ResultEditScalar::text("old"),
            }],
        };
        let duplicate_locator = ResultEditRequest {
            request_id: 2,
            descriptor,
            mutations: vec![delete(), delete()],
        };
        assert!(validate_request_shape(&duplicate_locator).is_err());
    }

    #[test]
    fn oracle_guarded_edit_checks_every_mutation_and_rolls_back_on_error() {
        let block = compile_oracle_guarded_result_edit(&[
            "UPDATE EMP SET ENAME = 'A' WHERE ROWID = 'AA'".to_string(),
            "DELETE FROM EMP WHERE ROWID = 'BB'".to_string(),
        ])
        .expect("Oracle edit block");
        assert!(block.starts_with("BEGIN\nSAVEPOINT SQ_RESULT_EDIT;"));
        assert_eq!(block.matches("IF SQL%ROWCOUNT <> 1").count(), 2);
        assert!(block.contains("ROLLBACK TO SQ_RESULT_EDIT;"));
        assert!(block.ends_with("RAISE;\nEND;"));
    }

    /// A save the app runs on the user's session never opens a transaction over
    /// work of theirs, on any backend.
    ///
    /// MySQL's `START TRANSACTION` implicitly COMMITS whatever the session
    /// already holds, and an auto-commit tab CAN hold an explicit transaction of
    /// the user's — the app supports that on purpose — so bracketing the save by
    /// the tab's auto-commit flag committed their uncommitted work for them and
    /// reported only the save's own success. The answer is "is there anything of
    /// the user's to lose", and Oracle's block meets the same rule by
    /// construction.
    #[test]
    fn a_save_never_opens_a_transaction_over_the_users_own_work() {
        use crate::db::{
            app_operation_transaction_scope, AppOperationTransactionScope, RetainedSessionState,
        };

        let clean = RetainedSessionState::default();
        let carries_work = RetainedSessionState::from_transaction_flags(true, false);
        assert!(carries_work.may_have_uncommitted_work());

        assert_eq!(
            app_operation_transaction_scope(true, clean),
            AppOperationTransactionScope::OwnTransaction,
            "auto-commit with nothing of the user's open: the save owns its transaction"
        );
        assert_eq!(
            app_operation_transaction_scope(true, carries_work),
            AppOperationTransactionScope::NestedInCallersTransaction,
            "auto-commit over the user's OPEN transaction: nest, or START TRANSACTION commits it"
        );
        for prior in [clean, carries_work] {
            assert_eq!(
                app_operation_transaction_scope(false, prior),
                AppOperationTransactionScope::NestedInCallersTransaction,
                "manual commit always nests and leaves the decision to the user"
            );
        }
        assert!(AppOperationTransactionScope::OwnTransaction.commits_its_own_work());
        assert!(!AppOperationTransactionScope::NestedInCallersTransaction.commits_its_own_work());

        // Oracle's save is the same promise, kept by construction: it nests in a
        // savepoint and has no transaction-opening statement to reach for.
        let block = compile_oracle_guarded_result_edit(&[
            "UPDATE EMP SET ENAME = 'A' WHERE ROWID = 'AA'".to_string(),
        ])
        .expect("Oracle edit block");
        let upper = block.to_uppercase();
        assert!(upper.contains("SAVEPOINT SQ_RESULT_EDIT"));
        for opener in ["START TRANSACTION", "SET TRANSACTION", "COMMIT"] {
            assert!(
                !upper.contains(opener),
                "the Oracle save must not open or end a transaction of its own: {opener}"
            );
        }
    }

    fn live_test_pool() -> Option<(Pool, DatabaseType)> {
        let host = std::env::var("SPACE_QUERY_TEST_MYSQL_HOST").ok()?;
        let port = std::env::var("SPACE_QUERY_TEST_MYSQL_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(3306);
        let database = std::env::var("SPACE_QUERY_TEST_MYSQL_DATABASE").ok()?;
        let user = std::env::var("SPACE_QUERY_TEST_MYSQL_USER").ok()?;
        let password = std::env::var("SPACE_QUERY_TEST_MYSQL_PASSWORD").ok()?;
        let db_type = if std::env::var("SPACE_QUERY_TEST_MYSQL_DB_TYPE")
            .unwrap_or_else(|_| "mysql".to_string())
            .eq_ignore_ascii_case("mariadb")
        {
            DatabaseType::MariaDB
        } else {
            DatabaseType::MySQL
        };
        let url = format!("mysql://{user}:{password}@{host}:{port}/{database}");
        Pool::new(Opts::from_url(&url).ok()?)
            .ok()
            .map(|pool| (pool, db_type))
    }

    fn live_descriptor_and_snapshots(
        conn: &mut PooledConn,
        db_type: DatabaseType,
        sql: &str,
    ) -> (ResultEditDescriptor, Vec<ResultEditRowSnapshot>) {
        let plan = prepare_mysql_editable_select(conn, sql).expect("editable select plan");
        let mut query_result = conn.query_iter(&plan.sql).expect("execute editable select");
        let columns = query_result.columns().as_ref().to_vec();
        let descriptor =
            mysql_descriptor_from_columns(db_type, &plan, &columns).expect("editable descriptor");
        let rows = query_result
            .by_ref()
            .collect::<Result<Vec<Row>, _>>()
            .expect("editable rows");
        drop(query_result);
        let snapshots = rows
            .iter()
            .map(|row| mysql_snapshot_for_row(&descriptor, row).expect("row snapshot"))
            .collect();
        (descriptor, snapshots)
    }

    fn original_for_column(
        descriptor: &ResultEditDescriptor,
        snapshot: &ResultEditRowSnapshot,
        column_name: &str,
    ) -> ResultEditOriginalValue {
        let index = descriptor
            .editable_columns
            .iter()
            .position(|column| column.source_name.eq_ignore_ascii_case(column_name))
            .expect("editable source column");
        ResultEditOriginalValue {
            column_name: column_name.to_string(),
            value: snapshot.original_values[index].clone(),
        }
    }

    fn originals_for_snapshot(
        descriptor: &ResultEditDescriptor,
        snapshot: &ResultEditRowSnapshot,
    ) -> Vec<ResultEditOriginalValue> {
        descriptor
            .editable_columns
            .iter()
            .zip(snapshot.original_values.iter())
            .map(|(column, value)| ResultEditOriginalValue {
                column_name: column.source_name.clone(),
                value: value.clone(),
            })
            .collect()
    }

    fn assignment(column_name: &str, value: ResultEditValue) -> ResultEditAssignment {
        ResultEditAssignment {
            column_name: column_name.to_string(),
            value,
        }
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn live_mysql_family_result_edit_is_exact_and_atomic() {
        let Some((pool, db_type)) = live_test_pool() else {
            eprintln!("skipping: SPACE_QUERY_TEST_MYSQL_* is not configured");
            return;
        };
        let mut conn = pool.get_conn().expect("test connection");
        conn.query_drop("DROP TABLE IF EXISTS sq_result_edit_fixture")
            .expect("drop old fixture");
        conn.query_drop(
            "CREATE TABLE sq_result_edit_fixture (\
             id BIGINT NOT NULL PRIMARY KEY, \
             code VARCHAR(40) NOT NULL UNIQUE, \
             value_text VARCHAR(100) NULL, \
             note VARCHAR(100) NULL) ENGINE=InnoDB",
        )
        .expect("create fixture");
        conn.query_drop(
            "INSERT INTO sq_result_edit_fixture (id, code, value_text, note) VALUES \
             (1, 'A', 'one', 'n1'), (2, 'B', 'two', NULL)",
        )
        .expect("seed fixture");

        let select_sql =
            "SELECT id, code, value_text, note FROM sq_result_edit_fixture ORDER BY id";
        let (descriptor, snapshots) = live_descriptor_and_snapshots(&mut conn, db_type, select_sql);
        assert_eq!(descriptor.locator_columns, vec!["id"]);
        assert_eq!(snapshots.len(), 2);

        let update = ResultEditRequest {
            request_id: 1,
            descriptor: descriptor.clone(),
            mutations: vec![ResultEditMutation::Update {
                locator_values: snapshots[0].locator_values.clone(),
                original_values: vec![original_for_column(
                    &descriptor,
                    &snapshots[0],
                    "value_text",
                )],
                assignments: vec![assignment(
                    "value_text",
                    ResultEditValue::Text("changed".to_string()),
                )],
            }],
        };
        assert_eq!(
            execute_mysql_result_edit(
                &mut conn,
                &update,
                crate::db::AppOperationTransactionScope::OwnTransaction,
                || false
            )
            .expect("single-row update"),
            1
        );
        let values: Vec<(u64, Option<String>)> = conn
            .query("SELECT id, value_text FROM sq_result_edit_fixture ORDER BY id")
            .expect("read updated rows");
        assert_eq!(
            values,
            vec![
                (1, Some("changed".to_string())),
                (2, Some("two".to_string()))
            ]
        );

        // The second INSERT violates the primary key. The first INSERT must
        // not survive the failed edit batch.
        let insert_batch = ResultEditRequest {
            request_id: 2,
            descriptor: descriptor.clone(),
            mutations: vec![
                ResultEditMutation::Insert {
                    assignments: vec![
                        assignment("id", ResultEditValue::Text("3".to_string())),
                        assignment("code", ResultEditValue::Text("C".to_string())),
                        assignment(
                            "value_text",
                            ResultEditValue::Expression("UPPER('three')".to_string()),
                        ),
                        assignment("note", ResultEditValue::Null),
                    ],
                },
                ResultEditMutation::Insert {
                    assignments: vec![
                        assignment("id", ResultEditValue::Text("1".to_string())),
                        assignment("code", ResultEditValue::Text("D".to_string())),
                    ],
                },
            ],
        };
        assert!(execute_mysql_result_edit(
            &mut conn,
            &insert_batch,
            crate::db::AppOperationTransactionScope::OwnTransaction,
            || false
        )
        .is_err());
        let row_three: Option<u64> = conn
            .query_first("SELECT id FROM sq_result_edit_fixture WHERE id = 3")
            .expect("check rolled-back insert");
        assert_eq!(row_three, None);

        let (_, current_snapshots) = live_descriptor_and_snapshots(&mut conn, db_type, select_sql);
        let mut stale_originals = originals_for_snapshot(&descriptor, &current_snapshots[1]);
        stale_originals
            .iter_mut()
            .find(|value| value.column_name.eq_ignore_ascii_case("value_text"))
            .expect("value_text original")
            .value = ResultEditScalar::text("not-the-original-value");
        let stale_delete = ResultEditRequest {
            request_id: 3,
            descriptor: descriptor.clone(),
            mutations: vec![ResultEditMutation::Delete {
                locator_values: current_snapshots[1].locator_values.clone(),
                original_values: stale_originals,
            }],
        };
        assert!(execute_mysql_result_edit(
            &mut conn,
            &stale_delete,
            crate::db::AppOperationTransactionScope::OwnTransaction,
            || false
        )
        .is_err());
        let row_two: Option<u64> = conn
            .query_first("SELECT id FROM sq_result_edit_fixture WHERE id = 2")
            .expect("check conflict preservation");
        assert_eq!(row_two, Some(2));

        // A manual transaction keeps the edit pending, and ROLLBACK restores
        // both work before the savepoint and the result edit.
        conn.query_drop("SET autocommit=0")
            .expect("disable autocommit");
        conn.query_drop("INSERT INTO sq_result_edit_fixture VALUES (9, 'Z', 'prior', NULL)")
            .expect("prior transaction work");
        let failed_manual_batch = ResultEditRequest {
            request_id: 40,
            descriptor: descriptor.clone(),
            mutations: vec![
                ResultEditMutation::Insert {
                    assignments: vec![
                        assignment("id", ResultEditValue::Text("10".to_string())),
                        assignment("code", ResultEditValue::Text("Y".to_string())),
                    ],
                },
                ResultEditMutation::Insert {
                    assignments: vec![
                        assignment("id", ResultEditValue::Text("1".to_string())),
                        assignment("code", ResultEditValue::Text("X".to_string())),
                    ],
                },
            ],
        };
        assert!(execute_mysql_result_edit(
            &mut conn,
            &failed_manual_batch,
            crate::db::AppOperationTransactionScope::NestedInCallersTransaction,
            || false
        )
        .is_err());
        let prior_work_count: u64 = conn
            .query_first("SELECT COUNT(*) FROM sq_result_edit_fixture WHERE id = 9")
            .expect("check prior transaction work")
            .expect("prior transaction count");
        let rolled_back_edit_count: u64 = conn
            .query_first("SELECT COUNT(*) FROM sq_result_edit_fixture WHERE id = 10")
            .expect("check failed edit rollback")
            .expect("failed edit count");
        assert_eq!(prior_work_count, 1);
        assert_eq!(rolled_back_edit_count, 0);
        let (_, manual_snapshots) = live_descriptor_and_snapshots(&mut conn, db_type, select_sql);
        let row_one = manual_snapshots
            .iter()
            .find(|snapshot| {
                snapshot
                    .locator_values
                    .first()
                    .is_some_and(|value| mysql_scalars_equal(value, &ResultEditScalar::Int(1)))
            })
            .expect("row one snapshot");
        let manual_update = ResultEditRequest {
            request_id: 4,
            descriptor: descriptor.clone(),
            mutations: vec![ResultEditMutation::Update {
                locator_values: row_one.locator_values.clone(),
                original_values: vec![original_for_column(&descriptor, row_one, "value_text")],
                assignments: vec![assignment(
                    "value_text",
                    ResultEditValue::Text("manual".to_string()),
                )],
            }],
        };
        assert_eq!(
            execute_mysql_result_edit(
                &mut conn,
                &manual_update,
                crate::db::AppOperationTransactionScope::NestedInCallersTransaction,
                || false
            )
            .expect("manual transaction update"),
            1
        );
        let within_transaction: String = conn
            .query_first("SELECT value_text FROM sq_result_edit_fixture WHERE id = 1")
            .expect("read manual update")
            .expect("row one");
        assert_eq!(within_transaction, "manual");
        conn.query_drop("ROLLBACK")
            .expect("rollback manual transaction");
        let after_rollback: String = conn
            .query_first("SELECT value_text FROM sq_result_edit_fixture WHERE id = 1")
            .expect("read after rollback")
            .expect("row one");
        assert_eq!(after_rollback, "changed");
        let prior_row: Option<u64> = conn
            .query_first("SELECT id FROM sq_result_edit_fixture WHERE id = 9")
            .expect("check prior work rollback");
        assert_eq!(prior_row, None);
        conn.query_drop("SET autocommit=1")
            .expect("restore autocommit");

        // Even a malformed descriptor that claims a non-unique locator must
        // fail before DML. LIMIT 1 is a last defense, not the uniqueness proof.
        conn.query_drop("DROP TABLE IF EXISTS sq_result_edit_nonunique_fixture")
            .expect("drop non-unique fixture");
        conn.query_drop(
            "CREATE TABLE sq_result_edit_nonunique_fixture (\
             id BIGINT NOT NULL, value_text VARCHAR(100) NOT NULL) ENGINE=InnoDB",
        )
        .expect("create non-unique fixture");
        conn.query_drop(
            "INSERT INTO sq_result_edit_nonunique_fixture VALUES (1, 'old'), (1, 'old')",
        )
        .expect("seed non-unique fixture");
        let unsafe_descriptor = ResultEditDescriptor {
            db_type,
            schema_name: descriptor.schema_name.clone(),
            table_name: "sq_result_edit_nonunique_fixture".to_string(),
            locator_columns: vec!["id".to_string()],
            editable_columns: vec![ResultEditColumn {
                result_index: 1,
                source_name: "value_text".to_string(),
            }],
            snapshot_column_index: 2,
        };
        let unsafe_update = ResultEditRequest {
            request_id: 5,
            descriptor: unsafe_descriptor,
            mutations: vec![ResultEditMutation::Update {
                locator_values: vec![ResultEditScalar::Int(1)],
                original_values: vec![ResultEditOriginalValue {
                    column_name: "value_text".to_string(),
                    value: ResultEditScalar::text("old"),
                }],
                assignments: vec![assignment(
                    "value_text",
                    ResultEditValue::Text("must-not-change".to_string()),
                )],
            }],
        };
        assert!(execute_mysql_result_edit(
            &mut conn,
            &unsafe_update,
            crate::db::AppOperationTransactionScope::OwnTransaction,
            || false
        )
        .is_err());
        let unchanged_rows: u64 = conn
            .query_first(
                "SELECT COUNT(*) FROM sq_result_edit_nonunique_fixture WHERE value_text = 'old'",
            )
            .expect("verify non-unique rows")
            .expect("non-unique row count");
        assert_eq!(unchanged_rows, 2);
        conn.query_drop("DROP TABLE sq_result_edit_nonunique_fixture")
            .expect("drop non-unique fixture");

        conn.query_drop("DROP TABLE IF EXISTS sq_result_edit_unique_fixture")
            .expect("drop unique fixture");
        conn.query_drop(
            "CREATE TABLE sq_result_edit_unique_fixture (\
             code VARCHAR(40) NOT NULL UNIQUE, value_text VARCHAR(100)) ENGINE=InnoDB",
        )
        .expect("create unique fixture");
        let unique_plan = prepare_mysql_editable_select(
            &mut conn,
            "SELECT code, value_text FROM sq_result_edit_unique_fixture",
        )
        .expect("NOT NULL UNIQUE edit plan");
        assert_eq!(unique_plan.locator_columns, vec!["code"]);
        conn.query_drop("DROP TABLE sq_result_edit_unique_fixture")
            .expect("drop unique fixture");

        conn.query_drop("DROP TABLE IF EXISTS sq_result_edit_nullable_fixture")
            .expect("drop nullable fixture");
        conn.query_drop(
            "CREATE TABLE sq_result_edit_nullable_fixture (\
             code VARCHAR(40) NULL UNIQUE, value_text VARCHAR(100)) ENGINE=InnoDB",
        )
        .expect("create nullable fixture");
        assert!(prepare_mysql_editable_select(
            &mut conn,
            "SELECT code, value_text FROM sq_result_edit_nullable_fixture"
        )
        .is_none());
        conn.query_drop("DROP TABLE sq_result_edit_nullable_fixture")
            .expect("drop nullable fixture");

        conn.query_drop("DROP TABLE sq_result_edit_fixture")
            .expect("drop fixture");
    }
}
