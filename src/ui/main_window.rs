use fltk::{
    app,
    button::{Button, CheckButton},
    dialog::{FileDialog, FileDialogType},
    draw::set_cursor,
    enums::{Cursor, FrameType},
    frame::Frame,
    group::{Flex, FlexType, Group, Tile},
    input::IntInput,
    menu::{Choice, MenuBar},
    prelude::*,
    text::TextBuffer,
    widget::Widget,
    window::Window,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use crate::app_icon;
use crate::db::{
    create_shared_connection, format_connection_busy_message, try_lock_connection_with_activity,
    ColumnInfo, DatabaseType, ObjectBrowser, QueryResult, RetainedSessionMutationOutcome,
    RetainedSessionPreflightAction, RetainedSessionPreflightDecision,
    RetainedSessionResolutionAction, SharedConnection, TransactionAccessMode, TransactionIsolation,
    TransactionMode,
};
use crate::ui::constants::*;
use crate::ui::result_table::{ResultGridSqlExecuteCallback, ResultTableContextAction};
use crate::ui::theme;
use crate::ui::{
    font_settings, show_settings_dialog, ConnectionDialog, FindReplaceDialog, HighlightData,
    IntellisenseData, MenuBarBuilder, ObjectBrowserMetadataSnapshot, ObjectBrowserWidget,
    QualifiedMemberKind, QueryHistoryDialog, QueryOperationToken, QueryProgress, QueryTabId,
    QueryTabsWidget, ResultMessageKind, ResultTabCloseTarget, ResultTabId, ResultTabRequest,
    ResultTabStatus, ResultTabsWidget, SqlAction, SqlEditorContextAction, SqlEditorWidget,
};
use crate::utils::arithmetic::{safe_div, safe_div_f64_to_usize, safe_rem};
use crate::utils::{malloc_trim_process, AppConfig, QueryHistory};

type MutexFlag = Arc<Mutex<Option<u64>>>;

const RESULT_ONE_TAB_PER_QUERY_LABEL: &str = " One tab per query";
const RESULT_CHECKBOX_GROUP_GAP: i32 = TOOLBAR_SPACING;

static NEXT_MUTEX_FLAG_TOKEN: AtomicU64 = AtomicU64::new(1);

fn next_mutex_flag_token() -> u64 {
    NEXT_MUTEX_FLAG_TOKEN.fetch_add(1, Ordering::Relaxed).max(1)
}

fn try_set_mutex_flag(flag: &MutexFlag) -> Option<u64> {
    let token = next_mutex_flag_token();
    match flag.lock() {
        Ok(mut guard) => {
            if guard.is_some() {
                None
            } else {
                *guard = Some(token);
                Some(token)
            }
        }
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            if guard.is_some() {
                None
            } else {
                *guard = Some(token);
                Some(token)
            }
        }
    }
}

fn clear_mutex_flag(flag: &MutexFlag) {
    match flag.lock() {
        Ok(mut guard) => *guard = None,
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            *guard = None;
        }
    }
}

fn clear_mutex_flag_if_token(flag: &MutexFlag, token: u64) {
    match flag.lock() {
        Ok(mut guard) => {
            if *guard == Some(token) {
                *guard = None;
            }
        }
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            if *guard == Some(token) {
                *guard = None;
            }
        }
    }
}

struct MutexFlagClearGuard {
    flag: MutexFlag,
    token: u64,
}

impl MutexFlagClearGuard {
    fn new(flag: MutexFlag, token: u64) -> Self {
        Self { flag, token }
    }
}

impl Drop for MutexFlagClearGuard {
    fn drop(&mut self) {
        clear_mutex_flag_if_token(&self.flag, self.token);
    }
}

fn mutex_flag_is_set(flag: &MutexFlag) -> bool {
    match flag.lock() {
        Ok(guard) => guard.is_some(),
        Err(poisoned) => poisoned.into_inner().is_some(),
    }
}

fn panic_payload_to_string(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

fn auto_commit_changed_progress_status(enabled: bool) -> &'static str {
    if enabled {
        "Tab auto-commit enabled"
    } else {
        "Tab auto-commit disabled"
    }
}

fn next_active_editor_tab_id_after_close(
    tab_ids: &[QueryTabId],
    closing_index: usize,
    active_editor_tab_id: QueryTabId,
) -> Option<QueryTabId> {
    let closing_tab_id = *tab_ids.get(closing_index)?;
    if active_editor_tab_id != closing_tab_id {
        return Some(active_editor_tab_id);
    }

    tab_ids
        .get(closing_index + 1)
        .or_else(|| {
            closing_index
                .checked_sub(1)
                .and_then(|prev| tab_ids.get(prev))
        })
        .copied()
}

#[derive(Clone)]
struct SchemaUpdate {
    data: IntellisenseData,
    highlight_data: HighlightData,
    connection_generation: u64,
    db_type: DatabaseType,
    selected_scope: Option<String>,
}

struct RetainedSessionOptionChangePlan {
    connection_generation: u64,
    db_type: DatabaseType,
    retained_editors: Vec<SqlEditorWidget>,
}

impl RetainedSessionOptionChangePlan {
    fn new(
        connection: &crate::db::DatabaseConnection,
        retained_editors: Vec<SqlEditorWidget>,
    ) -> Self {
        Self {
            connection_generation: connection.connection_generation(),
            db_type: connection.db_type(),
            retained_editors,
        }
    }

    fn apply_auto_commit(
        &self,
        pool_context_epoch: u64,
        enabled: bool,
        db_activity: &str,
    ) -> Vec<RetainedSessionMutationOutcome> {
        self.apply(|editor| {
            editor.apply_auto_commit_to_retained_session(
                self.connection_generation,
                pool_context_epoch,
                self.db_type,
                enabled,
                db_activity,
            )
        })
    }

    fn apply_transaction_mode(
        &self,
        pool_context_epoch: u64,
        mode: TransactionMode,
        db_activity: &str,
    ) -> Vec<RetainedSessionMutationOutcome> {
        self.apply(|editor| {
            editor.apply_transaction_mode_to_retained_session(
                self.connection_generation,
                pool_context_epoch,
                self.db_type,
                mode,
                db_activity,
            )
        })
    }

    fn validate_transaction_option_change(&self, action: &str) -> Result<(), String> {
        // This is the atomicity gate for global option changes: every retained
        // editor session is checked before the primary connection setter runs.
        // Per-editor apply failures after that point discard failed MySQL
        // sessions instead of leaving old option state retained beside updated
        // sessions.
        for editor in &self.retained_editors {
            let Some(snapshot) = editor.pooled_session_activity_snapshot() else {
                continue;
            };
            if self
                .db_type
                .retained_session_blocks_transaction_mode_change(snapshot.retained_state())
                && action == "transaction mode"
            {
                return Err(format!(
                    "Cannot change {action} while a retained {} DB session is {}. Resolve or discard it first.",
                    self.db_type.display_name(),
                    snapshot.retained_state().label()
                ));
            }
            if self
                .db_type
                .can_replace_retained_transaction_mode(snapshot.retained_state())
                && action == "transaction mode"
            {
                continue;
            }
            crate::db::DatabaseConnection::ensure_retained_session_option_change_allowed(
                snapshot.retained_state(),
                action,
            )?;
        }
        Ok(())
    }

    fn apply<F>(&self, apply_one: F) -> Vec<RetainedSessionMutationOutcome>
    where
        F: FnMut(&SqlEditorWidget) -> RetainedSessionMutationOutcome,
    {
        // Safe only after validate_transaction_option_change() has succeeded
        // for the same retained editor set. The individual MySQL mutation
        // helpers discard on apply errors so a failed retained session does
        // not continue with stale auto-commit or transaction-mode settings.
        self.retained_editors
            .iter()
            .map(apply_one)
            .filter(|outcome| outcome.should_alert_user())
            .collect()
    }
}

trait SchemaMetadataLoader: Sync {
    fn load(
        &self,
        context: crate::db::DbPoolSessionContext,
        requested_scope: Option<String>,
    ) -> Option<IntellisenseData>;
}

struct OracleSchemaMetadataLoader;
struct MysqlSchemaMetadataLoader;

static ORACLE_SCHEMA_METADATA_LOADER: OracleSchemaMetadataLoader = OracleSchemaMetadataLoader;
static MYSQL_SCHEMA_METADATA_LOADER: MysqlSchemaMetadataLoader = MysqlSchemaMetadataLoader;

fn schema_metadata_loader_for(db_type: DatabaseType) -> &'static dyn SchemaMetadataLoader {
    match db_type {
        DatabaseType::Oracle => &ORACLE_SCHEMA_METADATA_LOADER,
        DatabaseType::MySQL => &MYSQL_SCHEMA_METADATA_LOADER,
        DatabaseType::MariaDB => &MYSQL_SCHEMA_METADATA_LOADER,
    }
}

fn apply_schema_objects_to_intellisense(
    data: &mut IntellisenseData,
    schema_objects: &HashMap<String, Vec<(String, String)>>,
) {
    for (qualifier, objects) in schema_objects {
        let qualifier_members = objects
            .iter()
            .map(|(name, object_type)| {
                (
                    name.clone(),
                    QualifiedMemberKind::from_object_type_name(object_type),
                )
            })
            .collect();
        data.set_members_for_qualifier_with_kinds(qualifier, qualifier_members);
    }
}

fn apply_relation_members_to_intellisense(
    data: &mut IntellisenseData,
    relation_members: &HashMap<String, Vec<String>>,
) {
    for (qualifier, members) in relation_members {
        data.set_relation_members_for_qualifier(qualifier, members.clone());
    }
}

fn canonical_intellisense_scope(
    data: &IntellisenseData,
    scope: Option<String>,
    db_type: DatabaseType,
) -> Option<String> {
    let scope = scope
        .map(|scope| scope.trim().to_string())
        .filter(|scope| !scope.is_empty())?;
    if crate::sql_text::mysql_compatibility_for_sql("", Some(db_type)) {
        Some(data.canonical_qualifier_name(&scope).unwrap_or(scope))
    } else {
        Some(scope)
    }
}

fn add_object_name_to_intellisense_list(
    data: &mut IntellisenseData,
    name: &str,
    object_type: &str,
) {
    match object_type {
        "TABLE" => data.tables.push(name.to_string()),
        "VIEW" | "EDITIONING VIEW" => data.views.push(name.to_string()),
        "MATERIALIZED VIEW" => data.materialized_views.push(name.to_string()),
        "TYPE" | "TYPE BODY" => data.types.push(name.to_string()),
        "TRIGGER" => data.triggers.push(name.to_string()),
        "INDEX" => data.indexes.push(name.to_string()),
        "PROCEDURE" => data.procedures.push(name.to_string()),
        "FUNCTION" => data.functions.push(name.to_string()),
        "PACKAGE" | "PACKAGE BODY" => data.packages.push(name.to_string()),
        "SEQUENCE" => data.sequences.push(name.to_string()),
        "SYNONYM" => data.synonyms.push(name.to_string()),
        "EVENT" => data.events.push(name.to_string()),
        "DATABASE LINK" => data.database_links.push(name.to_string()),
        "DIRECTORY" => data.directories.push(name.to_string()),
        "LIBRARY" => data.libraries.push(name.to_string()),
        "CLUSTER" => data.clusters.push(name.to_string()),
        "CONTEXT" => data.contexts.push(name.to_string()),
        "DIMENSION" => data.dimensions.push(name.to_string()),
        "OPERATOR" => data.operators.push(name.to_string()),
        "INDEXTYPE" => data.indextypes.push(name.to_string()),
        "EDITION" => data.editions.push(name.to_string()),
        "JAVA SOURCE" => data.java_sources.push(name.to_string()),
        "JAVA CLASS" => data.java_classes.push(name.to_string()),
        "JAVA RESOURCE" => data.java_resources.push(name.to_string()),
        _ => {}
    }
}

fn apply_selected_scope_objects_to_intellisense(
    data: &mut IntellisenseData,
    schema_objects: &HashMap<String, Vec<(String, String)>>,
    selected_scope: Option<&str>,
    db_type: DatabaseType,
) {
    let Some(selected_scope) = selected_scope else {
        return;
    };

    let objects = if let Some((_, objects)) = schema_objects
        .iter()
        .find(|(qualifier, _)| qualifier.as_str() == selected_scope)
    {
        objects
    } else if crate::sql_text::mysql_compatibility_for_sql("", Some(db_type)) {
        let mut matches = schema_objects
            .iter()
            .filter(|(qualifier, _)| qualifier.eq_ignore_ascii_case(selected_scope));
        match (matches.next(), matches.next()) {
            (Some((_, objects)), None) => objects,
            _ => return,
        }
    } else {
        return;
    };

    for (name, object_type) in objects {
        add_object_name_to_intellisense_list(data, name, object_type);
    }
}

fn apply_public_synonyms_to_intellisense(
    data: &mut IntellisenseData,
    schema_objects: &HashMap<String, Vec<(String, String)>>,
) {
    let Some(objects) = schema_objects
        .iter()
        .find(|(qualifier, _)| qualifier.as_str() == "PUBLIC")
        .map(|(_, objects)| objects)
    else {
        return;
    };

    for (name, object_type) in objects {
        if object_type == "PUBLIC SYNONYM" {
            data.public_synonyms.push(name.clone());
        }
    }
}

impl SchemaMetadataLoader for OracleSchemaMetadataLoader {
    fn load(
        &self,
        context: crate::db::DbPoolSessionContext,
        requested_scope: Option<String>,
    ) -> Option<IntellisenseData> {
        context.ensure_current().ok()?;
        let (current_schema, mut owners, schema_objects, relation_members) = match context
            .acquire_session_for_current_scope()
        {
            Ok(crate::db::DbPoolSession::Oracle(conn)) => {
                let current_schema = context
                    .oracle_current_schema
                    .clone()
                    .or_else(|| {
                        ObjectBrowser::get_current_schema(&conn)
                            .ok()
                            .map(|schema| schema.trim().to_string())
                            .filter(|schema| !schema.is_empty())
                    })
                    .or_else(|| {
                        let username = context.connection_info.username.trim();
                        (!username.is_empty()).then(|| username.to_ascii_uppercase())
                    });
                let owners = match ObjectBrowser::get_users(&conn) {
                    Ok(owners) => owners,
                    Err(err) => {
                        eprintln!("Warning: failed to load Oracle owner list: {err}");
                        Vec::new()
                    }
                };
                context.ensure_current().ok()?;
                let schema_objects = match ObjectBrowser::get_schema_objects_by_owner(&conn) {
                    Ok(objects) => objects,
                    Err(err) => {
                        eprintln!(
                                "Warning: failed to load Oracle schema objects, keeping previous metadata: {err}"
                            );
                        return None;
                    }
                };
                context.ensure_current().ok()?;
                let relation_members = match ObjectBrowser::get_schema_relation_members_by_owner(
                    &conn,
                ) {
                    Ok(members) => members,
                    Err(err) => {
                        eprintln!(
                                    "Warning: failed to load Oracle relation members, keeping previous metadata: {err}"
                                );
                        return None;
                    }
                };
                (current_schema, owners, schema_objects, relation_members)
            }
            Ok(crate::db::DbPoolSession::OracleThin(mut conn)) => {
                let current_schema = context
                    .oracle_current_schema
                    .clone()
                    .or_else(|| {
                        ObjectBrowser::get_thin_current_schema(&mut conn)
                            .ok()
                            .map(|schema| schema.trim().to_string())
                            .filter(|schema| !schema.is_empty())
                    })
                    .or_else(|| {
                        let username = context.connection_info.username.trim();
                        (!username.is_empty()).then(|| username.to_ascii_uppercase())
                    });
                let owners = match ObjectBrowser::get_thin_users(&mut conn) {
                    Ok(owners) => owners,
                    Err(err) => {
                        eprintln!("Warning: failed to load Oracle Thin owner list: {err}");
                        Vec::new()
                    }
                };
                context.ensure_current().ok()?;
                let selected_owner = requested_scope
                    .clone()
                    .filter(|scope| !scope.trim().is_empty())
                    .or_else(|| current_schema.clone())
                    .or_else(|| owners.first().cloned());
                let (schema_objects, relation_members) = if let Some(ref selected_owner) =
                    selected_owner
                {
                    let schema_objects = match ObjectBrowser::get_thin_schema_objects_for_owner(
                        &mut conn,
                        selected_owner,
                    ) {
                        Ok(objects) => objects,
                        Err(err) => {
                            eprintln!(
                                    "Warning: failed to load Oracle Thin schema objects, keeping previous metadata: {err}"
                                );
                            return None;
                        }
                    };
                    context.ensure_current().ok()?;
                    let relation_members =
                        match ObjectBrowser::get_thin_schema_relation_members_for_owner(
                            &mut conn,
                            selected_owner,
                        ) {
                            Ok(members) => members,
                            Err(err) => {
                                eprintln!(
                                        "Warning: failed to load Oracle Thin relation members, keeping previous metadata: {err}"
                                    );
                                return None;
                            }
                        };
                    (schema_objects, relation_members)
                } else {
                    (HashMap::new(), HashMap::new())
                };
                (current_schema, owners, schema_objects, relation_members)
            }
            Ok(other) => {
                eprintln!(
                    "Warning: expected Oracle metadata session but acquired {}",
                    other.db_type()
                );
                return None;
            }
            Err(err) => {
                eprintln!("Warning: failed to acquire Oracle metadata session: {err}");
                return None;
            }
        };
        if let Some(ref current_schema) = current_schema {
            if !owners.iter().any(|owner| owner == current_schema) {
                owners.push(current_schema.clone());
            }
        }
        owners.sort();
        owners.dedup();

        let selected_owner = requested_scope
            .filter(|scope| !scope.trim().is_empty())
            .or(current_schema)
            .or_else(|| owners.first().cloned());

        let mut data = IntellisenseData::new();
        data.users = owners;
        data.set_default_qualifier(selected_owner.clone());
        apply_schema_objects_to_intellisense(&mut data, &schema_objects);
        apply_relation_members_to_intellisense(&mut data, &relation_members);
        apply_selected_scope_objects_to_intellisense(
            &mut data,
            &schema_objects,
            selected_owner.as_deref(),
            DatabaseType::Oracle,
        );
        apply_public_synonyms_to_intellisense(&mut data, &schema_objects);
        context.ensure_current().ok()?;
        Some(data)
    }
}

impl SchemaMetadataLoader for MysqlSchemaMetadataLoader {
    fn load(
        &self,
        context: crate::db::DbPoolSessionContext,
        requested_scope: Option<String>,
    ) -> Option<IntellisenseData> {
        let expected_db_type = context.connection_info.db_type;
        let display_name = expected_db_type.display_name();
        context.ensure_current().ok()?;
        let mut mysql_conn = match context.acquire_session_for_current_scope() {
            Ok(crate::db::DbPoolSession::MySQL { conn, db_type })
                if db_type.is_same_type_as(expected_db_type) =>
            {
                conn
            }
            Ok(other) => {
                eprintln!(
                    "Warning: expected {display_name} metadata session but acquired {}",
                    other.db_type()
                );
                return None;
            }
            Err(err) => {
                eprintln!("Warning: failed to acquire {display_name} metadata session: {err}");
                return None;
            }
        };
        let current_database = context.current_service_name.trim().to_string();
        let requested_schema = requested_scope
            .map(|scope| scope.trim().to_string())
            .filter(|scope| !scope.is_empty());
        let mut schemas = match crate::db::query::mysql_executor::MysqlObjectBrowser::get_schemas(
            mysql_conn.as_mut(),
        ) {
            Ok(schemas) => schemas,
            Err(err) => {
                eprintln!("Warning: failed to load {display_name} schema list: {err}");
                Vec::new()
            }
        };
        context.ensure_current().ok()?;
        if !current_database.is_empty()
            && !schemas
                .iter()
                .any(|schema| schema.eq_ignore_ascii_case(&current_database))
        {
            schemas.push(current_database.clone());
        }
        schemas.sort();
        schemas.dedup();

        let selected_schema = requested_schema
            .or_else(|| (!current_database.is_empty()).then_some(current_database.clone()))
            .or_else(|| schemas.first().cloned());

        let schema_objects =
            match crate::db::query::mysql_executor::MysqlObjectBrowser::get_schema_objects_by_schema(
                mysql_conn.as_mut(),
            ) {
                Ok(objects) => objects,
                Err(err) => {
                    eprintln!(
                        "Warning: failed to load {display_name} schema objects, keeping previous metadata: {err}"
                    );
                    return None;
                }
            };
        context.ensure_current().ok()?;
        let relation_members = match
            crate::db::query::mysql_executor::MysqlObjectBrowser::get_schema_relation_members_by_schema(
                mysql_conn.as_mut(),
            )
        {
            Ok(members) => members,
            Err(err) => {
                eprintln!(
                    "Warning: failed to load {display_name} relation members, keeping previous metadata: {err}"
                );
                return None;
            }
        };

        let mut data = IntellisenseData::new();
        data.schemas = schemas.clone();
        data.users = schemas;
        let selected_schema =
            canonical_intellisense_scope(&data, selected_schema, expected_db_type);
        data.set_default_qualifier(selected_schema.clone());
        apply_schema_objects_to_intellisense(&mut data, &schema_objects);
        apply_relation_members_to_intellisense(&mut data, &relation_members);
        apply_selected_scope_objects_to_intellisense(
            &mut data,
            &schema_objects,
            selected_schema.as_deref(),
            expected_db_type,
        );
        context.ensure_current().ok()?;
        Some(data)
    }
}

fn pending_metadata_refresh_after_start_attempt(has_live_connection: bool, started: bool) -> bool {
    !started && has_live_connection
}

#[derive(Clone)]
struct QueryEditorTab {
    tab_id: QueryTabId,
    base_label: String,
    sql_editor: SqlEditorWidget,
    sql_buffer: TextBuffer,
    current_file: Option<PathBuf>,
    pristine_text: String,
    current_text_len: usize,
    is_dirty: bool,
}

#[derive(Clone)]
struct QueryProgressContext {
    operation_token: Option<QueryOperationToken>,
    execution_target: Option<ResultTabId>,
    result_tab_ids: HashMap<usize, ResultTabId>,
    fetch_row_counts: HashMap<usize, usize>,
    lazy_fetch_sessions: HashMap<u64, usize>,
    lazy_fetch_tokens: HashMap<u64, LazyFetchProgressToken>,
    waiting_lazy_fetch_sessions: HashSet<u64>,
    closed_statement_indices: HashSet<usize>,
    batch_finished: bool,
    last_fetch_status_update: Instant,
    started_at: Instant,
    activity_label: String,
    active_statement_index: Option<usize>,
    running_statement_index: Option<usize>,
    state_label: String,
    auto_selected_result_tab: bool,
}

type RetainedScopeUpdate = (
    DatabaseType,
    u64,
    u64,
    crate::db::ConnectionAdvancedSettings,
    String,
    Vec<SqlEditorWidget>,
);

fn first_retained_outcome_message(outcomes: &[RetainedSessionMutationOutcome]) -> Option<String> {
    outcomes
        .iter()
        .find(|outcome| outcome.should_alert_user())
        .map(|outcome| {
            outcome
                .message()
                .map(|message| format!("{}: {}", outcome.status_label(), message))
                .unwrap_or_else(|| outcome.status_label().to_string())
        })
}

fn apply_retained_scope_update(update: RetainedScopeUpdate) -> Vec<RetainedSessionMutationOutcome> {
    // Scope switch methods on DatabaseConnection are intentionally low-level:
    // UI scope changes must pair the primary connection switch with this
    // retained-session update so pooled editors cannot keep running against
    // the previous database/schema.
    // Preserved sessions are rejected by retained_scope_change_blocker()
    // before this runs; if an apply failure still happens, the editor helper
    // restores only reusable preserved state and otherwise discards the stale
    // physical session so a retained tab is not silently left on the old scope.
    let (db_type, connection_generation, pool_context_epoch, advanced, selected_scope, editors) =
        update;
    let mut retained_outcomes = Vec::new();
    for editor in editors {
        let outcome = editor.apply_current_scope_to_retained_session(
            connection_generation,
            pool_context_epoch,
            db_type,
            &selected_scope,
            &advanced,
        );
        if outcome.should_alert_user() {
            retained_outcomes.push(outcome);
        }
    }
    retained_outcomes
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LazyFetchProgressToken {
    statement_index: usize,
    operation_id: u64,
    connection_generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueryEditorCloseOutcome {
    Closed,
    Deferred,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionActivityEntry {
    tab_name: String,
    result_tab: Option<usize>,
    state: String,
    database: String,
    sql_preview: String,
    fetched_rows: usize,
    elapsed: String,
}

impl QueryProgressContext {
    fn new(
        execution_target: Option<ResultTabId>,
        activity_label: String,
        operation_token: Option<QueryOperationToken>,
    ) -> Self {
        let now = Instant::now();
        Self {
            operation_token,
            execution_target,
            result_tab_ids: HashMap::new(),
            fetch_row_counts: HashMap::new(),
            lazy_fetch_sessions: HashMap::new(),
            lazy_fetch_tokens: HashMap::new(),
            waiting_lazy_fetch_sessions: HashSet::new(),
            closed_statement_indices: HashSet::new(),
            batch_finished: false,
            last_fetch_status_update: now,
            started_at: now,
            activity_label,
            active_statement_index: None,
            running_statement_index: None,
            state_label: ResultTabStatus::Running.label().to_string(),
            auto_selected_result_tab: false,
        }
    }

    fn mark_statement_running(&mut self, statement_index: usize) {
        self.active_statement_index = Some(statement_index);
        self.running_statement_index = Some(statement_index);
    }

    fn mark_statement_finished(&mut self, statement_index: usize) {
        if self.running_statement_index == Some(statement_index) {
            self.running_statement_index = None;
        }
        self.active_statement_index = Some(statement_index);
    }

    fn canceling_statement_index(&self) -> Option<usize> {
        self.running_statement_index
    }

    fn mark_statement_closed(&mut self, statement_index: usize) {
        self.closed_statement_indices.insert(statement_index);
        self.result_tab_ids.remove(&statement_index);
        self.fetch_row_counts.remove(&statement_index);
        if let Some(session_id) = self.lazy_fetch_session_for_statement(statement_index) {
            self.waiting_lazy_fetch_sessions.remove(&session_id);
        }
        if self.active_statement_index == Some(statement_index) {
            self.active_statement_index = None;
        }
        if self.running_statement_index == Some(statement_index) {
            self.running_statement_index = None;
        }
        self.state_label = ResultTabStatus::Cancelled.label().to_string();
    }

    fn mark_all_result_statements_closed(&mut self) {
        let mut statement_indices = self
            .lazy_fetch_sessions
            .values()
            .copied()
            .collect::<Vec<_>>();
        statement_indices.extend(self.fetch_row_counts.keys().copied());
        statement_indices.extend(self.result_tab_ids.keys().copied());
        if let Some(index) = self.active_statement_index {
            statement_indices.push(index);
        }
        if let Some(index) = self.running_statement_index {
            statement_indices.push(index);
        }
        statement_indices.sort_unstable();
        statement_indices.dedup();

        for statement_index in statement_indices {
            self.mark_statement_closed(statement_index);
        }
        self.lazy_fetch_sessions.clear();
        self.lazy_fetch_tokens.clear();
        self.waiting_lazy_fetch_sessions.clear();
    }

    fn lazy_fetch_session_for_statement(&self, statement_index: usize) -> Option<u64> {
        self.lazy_fetch_sessions
            .iter()
            .find_map(|(session_id, index)| {
                if *index == statement_index {
                    Some(*session_id)
                } else {
                    None
                }
            })
    }

    fn register_lazy_fetch_session(
        &mut self,
        session_id: u64,
        statement_index: usize,
        operation_id: u64,
        connection_generation: u64,
    ) {
        self.lazy_fetch_sessions.insert(session_id, statement_index);
        self.lazy_fetch_tokens.insert(
            session_id,
            LazyFetchProgressToken {
                statement_index,
                operation_id,
                connection_generation,
            },
        );
        self.waiting_lazy_fetch_sessions.remove(&session_id);
    }

    fn lazy_fetch_event_matches(
        &self,
        session_id: u64,
        statement_index: usize,
        operation_id: u64,
        connection_generation: u64,
    ) -> bool {
        self.lazy_fetch_tokens
            .get(&session_id)
            .is_some_and(|token| {
                token.statement_index == statement_index
                    && token.operation_id == operation_id
                    && token.connection_generation == connection_generation
            })
    }

    fn mark_lazy_fetch_active_for_statement(&mut self, statement_index: usize) {
        if let Some(session_id) = self.lazy_fetch_session_for_statement(statement_index) {
            self.waiting_lazy_fetch_sessions.remove(&session_id);
        }
    }

    fn mark_lazy_fetch_waiting(&mut self, session_id: u64, statement_index: usize) -> bool {
        if self.lazy_fetch_sessions.get(&session_id) != Some(&statement_index) {
            return false;
        }
        self.waiting_lazy_fetch_sessions.insert(session_id);
        true
    }

    fn remove_lazy_fetch_session(&mut self, session_id: u64) -> Option<usize> {
        self.waiting_lazy_fetch_sessions.remove(&session_id);
        self.lazy_fetch_tokens.remove(&session_id);
        self.lazy_fetch_sessions.remove(&session_id)
    }

    fn has_waiting_lazy_fetch(&self) -> bool {
        self.lazy_fetch_sessions
            .keys()
            .any(|session_id| self.waiting_lazy_fetch_sessions.contains(session_id))
    }

    fn lazy_fetch_sessions_without_result_tab_mapping<F>(&self, mut session_at: F) -> Vec<u64>
    where
        F: FnMut(ResultTabId) -> Option<u64>,
    {
        let mut sessions = self
            .lazy_fetch_sessions
            .iter()
            .filter_map(|(session_id, statement_index)| {
                let Some(tab_id) = self.result_tab_id_for_statement(*statement_index) else {
                    return Some(*session_id);
                };
                if session_at(tab_id) != Some(*session_id) {
                    Some(*session_id)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        sessions.sort_unstable();
        sessions
    }

    fn ensure_result_tab_id<F>(&mut self, statement_index: usize, reserve_id: F) -> ResultTabId
    where
        F: FnOnce() -> ResultTabId,
    {
        if let Some(tab_id) = self.result_tab_id_for_statement(statement_index) {
            return tab_id;
        }
        let tab_id = self.execution_target.unwrap_or_else(reserve_id);
        self.result_tab_ids.insert(statement_index, tab_id);
        tab_id
    }

    fn claim_result_tab_auto_select(&mut self) -> bool {
        if self.auto_selected_result_tab {
            false
        } else {
            self.auto_selected_result_tab = true;
            true
        }
    }

    fn result_tab_id_for_statement(&self, statement_index: usize) -> Option<ResultTabId> {
        self.result_tab_ids.get(&statement_index).copied()
    }

    fn statement_index_for_result_tab(&self, result_tab_id: ResultTabId) -> Option<usize> {
        self.result_tab_ids
            .iter()
            .find_map(|(statement_index, tab_id)| {
                if *tab_id == result_tab_id {
                    Some(*statement_index)
                } else {
                    None
                }
            })
    }
}

pub struct AppState {
    pub connection: SharedConnection,
    query_tabs: QueryTabsWidget,
    query_top_group: Group,
    pub query_split_bar: Frame,
    editor_tabs: Vec<QueryEditorTab>,
    active_editor_tab_id: QueryTabId,
    next_editor_tab_number: usize,
    pub sql_editor: SqlEditorWidget,
    pub sql_buffer: TextBuffer,
    schema_intellisense_data: Arc<Mutex<IntellisenseData>>,
    schema_highlight_data: HighlightData,
    query_timeout_input: IntInput,
    pub result_tabs: ResultTabsWidget,
    result_toolbar: Flex,
    result_one_tab_per_query_check: CheckButton,
    result_one_tab_edit_gap: Frame,
    result_edit_check: CheckButton,
    result_insert_btn: Button,
    result_delete_btn: Button,
    result_save_btn: Button,
    result_cancel_btn: Button,
    execute_btn: Button,
    query_cancel_btn: Button,
    explain_btn: Button,
    commit_btn: Button,
    rollback_btn: Button,
    transaction_isolation_choice: Choice,
    transaction_access_choice: Choice,
    result_grid_execution_target: Option<ResultTabId>,
    progress_contexts: HashMap<QueryTabId, QueryProgressContext>,
    abandoned_query_operations: HashSet<QueryOperationToken>,
    pending_query_canceling_tabs: HashSet<QueryTabId>,
    pending_lazy_fetch_canceling_sessions: HashSet<u64>,
    pub object_browser: ObjectBrowserWidget,
    pub status_bar: Frame,
    pub current_file: Arc<Mutex<Option<PathBuf>>>,
    pub popups: Arc<Mutex<Vec<Window>>>,
    pub window: Window,
    pub right_tile: Tile,
    /// Saved query/result split ratio (0.0–1.0).  `None` means the user has
    /// not adjusted the split bar yet (use default 40%).
    pub query_split_ratio: Arc<Mutex<Option<f64>>>,
    pub connection_info: Arc<Mutex<Option<crate::db::ConnectionInfo>>>,
    has_live_connection: bool,
    pending_connection_metadata_refresh: bool,
    pub config: Arc<Mutex<AppConfig>>,
    status_animation_running: bool,
    status_animation_message: String,
    status_animation_frame: usize,
    schema_sender: Option<std::sync::mpsc::Sender<SchemaUpdate>>,
    file_sender: Option<std::sync::mpsc::Sender<FileActionResult>>,
    schema_refresh_in_progress: MutexFlag,
}

fn set_result_action_button_visibility(toolbar: &mut Flex, button: &mut Button, visible: bool) {
    if visible {
        toolbar.fixed(button, BUTTON_WIDTH_SMALL);
        if !button.visible() {
            button.show();
        }
        button.activate();
    } else {
        button.deactivate();
        if button.visible() {
            button.hide();
        }
        toolbar.fixed(button, 0);
    }
}

fn result_toolbar_checkbox_width(check: &CheckButton, min_width: i32) -> i32 {
    let (label_w, _) = check.measure_label();
    result_toolbar_checkbox_width_for_label(label_w, min_width)
}

fn result_toolbar_checkbox_width_for_label(label_w: i32, min_width: i32) -> i32 {
    const CHECK_INDICATOR_AND_PADDING: i32 = 34;
    label_w
        .max(0)
        .saturating_add(CHECK_INDICATOR_AND_PADDING)
        .max(min_width)
}

fn transaction_isolation_choice_labels(
    db_type: DatabaseType,
    default_isolation: TransactionIsolation,
) -> String {
    db_type.transaction_isolation_choice_labels(Some(default_isolation))
}

fn transaction_isolation_from_choice_index(
    db_type: DatabaseType,
    index: i32,
) -> TransactionIsolation {
    db_type.transaction_isolation_from_choice_index(index, TransactionIsolation::Default)
}

fn transaction_isolation_choice_index(
    db_type: DatabaseType,
    isolation: TransactionIsolation,
) -> i32 {
    db_type.choice_index_from_transaction_isolation(isolation, TransactionIsolation::Default)
}

fn transaction_access_from_choice_index(index: i32) -> TransactionAccessMode {
    match index {
        1 => TransactionAccessMode::ReadOnly,
        _ => TransactionAccessMode::ReadWrite,
    }
}

fn transaction_access_choice_index(access_mode: TransactionAccessMode) -> i32 {
    match access_mode {
        TransactionAccessMode::ReadWrite => 0,
        TransactionAccessMode::ReadOnly => 1,
    }
}

fn transaction_mode_new_transaction_notice() -> &'static str {
    "Isolation/access mode changes apply only to new transactions.\nExisting transactions keep their current transaction mode."
}

impl AppState {
    fn app_window_title() -> String {
        format!("SPACE Query {}", crate::version::display_version())
    }

    fn next_spinner_frame(current_frame: usize, frame_count: usize) -> Option<usize> {
        if frame_count == 0 {
            return None;
        }

        Some(safe_rem(current_frame.saturating_add(1), frame_count))
    }

    const STATUS_SPINNER_FRAMES: [&'static str; 10] =
        ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

    fn tab_display_label(tab: &QueryEditorTab) -> String {
        let mut label = match &tab.current_file {
            Some(path) => path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            None => tab.base_label.clone(),
        };
        if tab.is_dirty {
            label.push('*');
        }
        label
    }

    fn refresh_window_title(&mut self) {
        let base_title = Self::app_window_title();
        if let Some(index) = self.find_tab_index(self.active_editor_tab_id) {
            let label = Self::tab_display_label(&self.editor_tabs[index]);
            self.window.set_label(&format!("{base_title} - {label}"));
            return;
        }
        self.window.set_label(&base_title);
    }

    fn hide_all_intellisense_popups(&self) {
        self.sql_editor.try_hide_intellisense_popup();
        self.sql_editor.hide_signature_popup();
        for tab in &self.editor_tabs {
            tab.sql_editor.try_hide_intellisense_popup();
            tab.sql_editor.hide_signature_popup();
        }
    }

    fn find_tab_index(&self, tab_id: QueryTabId) -> Option<usize> {
        self.editor_tabs.iter().position(|tab| tab.tab_id == tab_id)
    }

    fn normalize_scope_name(scope: Option<String>) -> Option<String> {
        scope
            .map(|scope| scope.trim().to_string())
            .filter(|scope| !scope.is_empty())
    }

    fn current_connection_scope(
        conn_guard: &crate::db::DatabaseConnection,
        db_type: DatabaseType,
    ) -> Option<String> {
        conn_guard
            .db_type()
            .is_same_type_as(db_type)
            .then(|| conn_guard.current_scope_name())
            .flatten()
            .and_then(|scope| Self::normalize_scope_name(Some(scope)))
    }

    fn scope_matches_current_connection(&self, scope: &str) -> bool {
        let scope = scope.trim();
        if scope.is_empty() {
            return false;
        }
        let conn_guard = self
            .connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !conn_guard.is_connected() {
            return false;
        }
        let db_type = conn_guard.db_type();
        Self::current_connection_scope(&conn_guard, db_type).is_some_and(|current_scope| {
            db_type.scope_values_match(Some(&current_scope), Some(scope))
        })
    }

    fn set_active_editor_tab(&mut self, tab_id: QueryTabId) -> bool {
        self.set_active_editor_tab_with_display_stabilization(tab_id, true)
    }

    fn set_active_editor_tab_with_display_stabilization(
        &mut self,
        tab_id: QueryTabId,
        stabilize_display: bool,
    ) -> bool {
        let Some(index) = self.find_tab_index(tab_id) else {
            return false;
        };
        let tab = self.editor_tabs[index].clone();
        self.active_editor_tab_id = tab_id;
        self.sql_editor = tab.sql_editor;
        self.sql_editor.sync_db_type_from_connection();
        self.sql_editor.mark_display_metrics_pending();
        if stabilize_display {
            self.sql_editor.stabilize_display_metrics();
        }
        self.sql_buffer = tab.sql_buffer;
        *self
            .current_file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = tab.current_file;
        self.refresh_window_title();
        true
    }

    fn is_any_query_running(&self) -> bool {
        self.sql_editor.is_query_running()
            || self
                .editor_tabs
                .iter()
                .any(|tab| tab.sql_editor.is_query_running())
    }

    fn is_query_running_for_tab(&self, tab_id: QueryTabId) -> bool {
        self.editor_tabs
            .iter()
            .find(|tab| tab.tab_id == tab_id)
            .map(|tab| tab.sql_editor.is_query_running())
            .unwrap_or(false)
    }

    fn has_running_query_or_lazy_fetch_for_tab(&self, tab_id: QueryTabId) -> bool {
        let editor_has_work = self
            .editor_tabs
            .iter()
            .find(|tab| tab.tab_id == tab_id)
            .map(|tab| {
                tab.sql_editor.is_query_running()
                    || tab.sql_editor.active_lazy_fetch_session().is_some()
            })
            .unwrap_or(false);
        let progress_has_lazy_fetch = self
            .progress_contexts
            .get(&tab_id)
            .map(|context| !context.lazy_fetch_sessions.is_empty())
            .unwrap_or(false);
        editor_has_work || progress_has_lazy_fetch
    }

    fn should_show_progress_status_for_tab(&self, tab_id: QueryTabId) -> bool {
        self.is_query_running_for_tab(tab_id) || !self.is_any_query_running()
    }

    fn has_active_lazy_fetches(&self) -> bool {
        !self.lazy_fetch_sessions_for_abort().is_empty()
    }

    fn has_running_query_or_lazy_fetch(&self) -> bool {
        self.is_any_query_running() || self.has_active_lazy_fetches()
    }

    fn lazy_fetch_session_is_active_in_editor(&self, session_id: u64) -> bool {
        self.sql_editor.active_lazy_fetch_session() == Some(session_id)
            || self
                .editor_tabs
                .iter()
                .any(|tab| tab.sql_editor.active_lazy_fetch_session() == Some(session_id))
    }

    fn mark_lazy_fetch_result_tab_cancelled(&mut self, session_id: u64) {
        let mut result_tab_ids = Vec::new();
        for context in self.progress_contexts.values_mut() {
            let Some(statement_index) = context.lazy_fetch_sessions.get(&session_id).copied()
            else {
                continue;
            };
            context.active_statement_index = Some(statement_index);
            context.state_label = ResultTabStatus::Cancelled.label().to_string();
            if let Some(tab_id) = context.result_tab_id_for_statement(statement_index) {
                result_tab_ids.push(tab_id);
            }
        }
        result_tab_ids.sort_by_key(|id| *id);
        result_tab_ids.dedup();
        for tab_id in result_tab_ids {
            self.result_tabs.mark_statement_cancelled_by_id(tab_id);
        }
    }

    fn mark_lazy_fetch_result_tab_closed(&mut self, session_id: u64) {
        self.pending_lazy_fetch_canceling_sessions
            .remove(&session_id);
        let mut finished_contexts = Vec::new();
        for (tab_id, context) in self.progress_contexts.iter_mut() {
            let Some(statement_index) = context.remove_lazy_fetch_session(session_id) else {
                continue;
            };
            context.mark_statement_closed(statement_index);
            if context.lazy_fetch_sessions.is_empty() && context.batch_finished {
                finished_contexts.push(*tab_id);
            }
        }
        for tab_id in finished_contexts {
            self.finish_progress_context(tab_id);
        }
    }

    fn mark_result_tab_closed_by_id(&mut self, result_tab_id: ResultTabId) -> Vec<u64> {
        let mut finished_contexts = Vec::new();
        let mut tabs_needing_active_session_lookup = Vec::new();
        let mut sessions_to_cancel = Vec::new();
        for (tab_id, context) in self.progress_contexts.iter_mut() {
            let Some(statement_index) = context.statement_index_for_result_tab(result_tab_id)
            else {
                continue;
            };
            let has_matching_session = context
                .lazy_fetch_sessions
                .values()
                .any(|index| *index == statement_index);
            let is_relevant_statement = has_matching_session
                || context.active_statement_index == Some(statement_index)
                || context.fetch_row_counts.contains_key(&statement_index);
            if !is_relevant_statement {
                continue;
            }
            context.mark_statement_closed(statement_index);

            let matching_sessions = context
                .lazy_fetch_sessions
                .iter()
                .filter_map(|(session_id, index)| {
                    if *index == statement_index {
                        Some(*session_id)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            if matching_sessions.is_empty() {
                tabs_needing_active_session_lookup.push(*tab_id);
            }
            for session_id in matching_sessions {
                context.remove_lazy_fetch_session(session_id);
                sessions_to_cancel.push(session_id);
            }
            if context.lazy_fetch_sessions.is_empty() && context.batch_finished {
                finished_contexts.push(*tab_id);
            }
        }

        for tab_id in tabs_needing_active_session_lookup {
            let active_session = self
                .find_tab_index(tab_id)
                .and_then(|index| self.editor_tabs.get(index))
                .and_then(|tab| tab.sql_editor.active_lazy_fetch_session());
            if let Some(session_id) = active_session {
                if !sessions_to_cancel.contains(&session_id) {
                    sessions_to_cancel.push(session_id);
                }
            }
        }

        for tab_id in finished_contexts {
            self.finish_progress_context(tab_id);
        }
        sessions_to_cancel
    }

    fn abort_lazy_fetches_without_result_tab_mapping(&mut self) -> Vec<u64> {
        let mut sessions_to_cancel = Vec::new();
        for context in self.progress_contexts.values() {
            let unmapped = context.lazy_fetch_sessions_without_result_tab_mapping(|tab_id| {
                self.result_tabs.lazy_fetch_session_for_id(tab_id)
            });
            for session_id in unmapped {
                Self::push_unique_session_id(&mut sessions_to_cancel, session_id);
            }
        }
        if sessions_to_cancel.is_empty() {
            return sessions_to_cancel;
        }

        let mut finished_contexts = Vec::new();
        for (tab_id, context) in self.progress_contexts.iter_mut() {
            for session_id in &sessions_to_cancel {
                let Some(statement_index) = context.remove_lazy_fetch_session(*session_id) else {
                    continue;
                };
                context.mark_statement_closed(statement_index);
            }
            if context.lazy_fetch_sessions.is_empty() && context.batch_finished {
                finished_contexts.push(*tab_id);
            }
        }
        for session_id in &sessions_to_cancel {
            self.pending_lazy_fetch_canceling_sessions
                .remove(session_id);
        }
        for session_id in &sessions_to_cancel {
            self.result_tabs.abort_lazy_fetch_session(*session_id);
        }
        for tab_id in finished_contexts {
            self.finish_progress_context(tab_id);
        }
        sessions_to_cancel
    }

    fn finish_progress_context(&mut self, tab_id: QueryTabId) {
        self.pending_query_canceling_tabs.remove(&tab_id);
        if let Some(context) = self.progress_contexts.remove(&tab_id) {
            for session_id in context.lazy_fetch_sessions.keys() {
                self.pending_lazy_fetch_canceling_sessions
                    .remove(session_id);
            }
        }
        self.result_grid_execution_target = None;
        self.start_pending_metadata_refresh_if_ready();
    }

    fn operation_progress_matches(
        &self,
        tab_id: QueryTabId,
        token: QueryOperationToken,
        _progress: &QueryProgress,
    ) -> bool {
        let Some(editor) = self
            .find_tab_index(tab_id)
            .and_then(|index| self.editor_tabs.get(index))
            .map(|tab| &tab.sql_editor)
        else {
            return false;
        };
        operation_progress_token_matches_current_editor(
            tab_id,
            token,
            Some(editor.editor_instance_id()),
            editor.current_operation_id_value(),
            editor.last_completed_operation_id_value(),
            &self.abandoned_query_operations,
        )
    }

    fn operation_abandoned_matches(&self, tab_id: QueryTabId, token: QueryOperationToken) -> bool {
        token.tab_id == tab_id
            && token.operation_id != 0
            && self
                .find_tab_index(tab_id)
                .and_then(|index| self.editor_tabs.get(index))
                .is_some_and(|tab| tab.sql_editor.editor_instance_id() == token.editor_id)
    }

    fn mark_operation_abandoned_cancelled(
        &mut self,
        tab_id: QueryTabId,
        token: QueryOperationToken,
    ) {
        self.abandoned_query_operations.insert(token);
        self.pending_query_canceling_tabs.remove(&tab_id);
        let result_tab_id = self.progress_contexts.get(&tab_id).and_then(|context| {
            if context.operation_token != Some(token) {
                return None;
            }
            context
                .active_statement_index
                .and_then(|statement_index| context.result_tab_id_for_statement(statement_index))
        });
        if let Some(result_tab_id) = result_tab_id {
            self.result_tabs
                .mark_statement_cancelled_by_id(result_tab_id);
        }
        if self
            .progress_contexts
            .get(&tab_id)
            .is_some_and(|context| context.operation_token == Some(token))
        {
            self.finish_progress_context(tab_id);
        }
    }

    fn start_pending_metadata_refresh_if_ready(&mut self) {
        if !self.progress_contexts.is_empty()
            || !self.pending_connection_metadata_refresh
            || !self.has_live_connection
        {
            return;
        }
        if let Some(schema_sender) = self.schema_sender.clone() {
            let started = MainWindow::start_connection_metadata_refresh(self, &schema_sender);
            self.update_pending_metadata_refresh_after_start_attempt(started);
        }
    }

    fn update_pending_metadata_refresh_after_start_attempt(&mut self, started: bool) {
        self.pending_connection_metadata_refresh =
            pending_metadata_refresh_after_start_attempt(self.has_live_connection, started);
    }

    fn mark_lazy_fetch_result_tabs_closed<I>(&mut self, session_ids: I)
    where
        I: IntoIterator<Item = u64>,
    {
        for session_id in session_ids {
            self.mark_lazy_fetch_result_tab_closed(session_id);
        }
    }

    fn abort_lazy_fetch_result_tabs_for_connection_transition(&mut self) -> Vec<u64> {
        let lazy_fetch_sessions = self.lazy_fetch_sessions_for_abort();
        if lazy_fetch_sessions.is_empty() {
            return lazy_fetch_sessions;
        }

        for session_id in &lazy_fetch_sessions {
            self.mark_lazy_fetch_result_tab_cancelled(*session_id);
        }
        self.mark_lazy_fetch_result_tabs_closed(lazy_fetch_sessions.iter().copied());
        for session_id in &lazy_fetch_sessions {
            self.result_tabs.abort_lazy_fetch_session(*session_id);
        }
        self.refresh_result_edit_controls();
        lazy_fetch_sessions
    }

    fn release_all_pooled_db_sessions(&self) -> bool {
        let mut released_any = self.sql_editor.release_pooled_db_session();
        for tab in &self.editor_tabs {
            released_any |= tab.sql_editor.release_pooled_db_session();
        }
        released_any
    }

    fn release_all_resolved_pooled_db_sessions(&self) -> Result<bool, String> {
        let mut released_any = self.sql_editor.release_pooled_db_session_if_resolved()?;
        for tab in &self.editor_tabs {
            released_any |= tab.sql_editor.release_pooled_db_session_if_resolved()?;
        }
        Ok(released_any)
    }

    fn sync_mysql_auto_commit_overrides_with_global_setting(&self, enabled: bool) {
        self.sql_editor
            .sync_mysql_auto_commit_with_global_setting(enabled);
        for tab in &self.editor_tabs {
            tab.sql_editor
                .sync_mysql_auto_commit_with_global_setting(enabled);
        }
    }

    fn oldest_lazy_fetch_session(&self) -> Option<u64> {
        self.lazy_fetch_sessions_for_abort().into_iter().min()
    }

    fn mark_lazy_fetch_cancelled_without_status(&mut self, session_id: u64) {
        self.mark_lazy_fetch_result_tab_cancelled(session_id);
        self.mark_lazy_fetch_result_tab_closed(session_id);
        self.result_tabs.abort_lazy_fetch_session(session_id);
    }

    fn mark_lazy_fetch_cancelled(&mut self, session_id: u64, status_message: &str) {
        self.mark_lazy_fetch_cancelled_without_status(session_id);
        self.set_status_message(status_message);
        self.refresh_result_edit_controls();
    }

    fn mark_canceling_progress_contexts_cancelled(&mut self) {
        let mut result_tab_ids = Vec::new();
        self.pending_query_canceling_tabs.clear();
        for context in self.progress_contexts.values_mut() {
            if context.state_label != ResultTabStatus::Canceling.label() {
                continue;
            }
            context.state_label = ResultTabStatus::Cancelled.label().to_string();
            if let Some(statement_index) = context.active_statement_index {
                if let Some(tab_id) = context.result_tab_id_for_statement(statement_index) {
                    result_tab_ids.push(tab_id);
                }
            }
        }
        result_tab_ids.sort_unstable();
        result_tab_ids.dedup();
        for tab_id in result_tab_ids {
            self.result_tabs.mark_statement_cancelled_by_id(tab_id);
        }
    }

    fn mark_all_result_tabs_closed_for_clear(&mut self) {
        let mut finished_contexts = Vec::new();
        for (tab_id, context) in self.progress_contexts.iter_mut() {
            context.mark_all_result_statements_closed();
            if context.batch_finished {
                finished_contexts.push(*tab_id);
            }
        }
        self.pending_query_canceling_tabs.clear();
        self.pending_lazy_fetch_canceling_sessions.clear();
        for tab_id in finished_contexts {
            self.finish_progress_context(tab_id);
        }
    }

    fn clear_result_grids_for_new_query_batch(&mut self) -> Vec<u64> {
        let had_tabs = self.result_tabs.tab_count() > 0;
        let mut lazy_fetch_sessions = Vec::new();
        for session_id in self.result_tabs.lazy_fetch_sessions() {
            Self::push_unique_session_id(&mut lazy_fetch_sessions, session_id);
        }
        for context in self.progress_contexts.values() {
            for session_id in context.lazy_fetch_sessions.keys().copied() {
                Self::push_unique_session_id(&mut lazy_fetch_sessions, session_id);
            }
        }
        self.result_tabs.clear_grids();
        self.mark_lazy_fetch_result_tabs_closed(lazy_fetch_sessions.clone());
        self.mark_all_result_tabs_closed_for_clear();
        if had_tabs {
            malloc_trim_process();
        }
        self.refresh_result_edit_controls();
        lazy_fetch_sessions
    }

    fn push_unique_session_id(session_ids: &mut Vec<u64>, session_id: u64) {
        if !session_ids.contains(&session_id) {
            session_ids.push(session_id);
        }
    }

    fn lazy_fetch_sessions_for_abort(&self) -> Vec<u64> {
        let mut session_ids = Vec::new();
        for session_id in self.result_tabs.lazy_fetch_sessions() {
            Self::push_unique_session_id(&mut session_ids, session_id);
        }
        for context in self.progress_contexts.values() {
            for session_id in context.lazy_fetch_sessions.keys().copied() {
                Self::push_unique_session_id(&mut session_ids, session_id);
            }
        }
        for session_id in self
            .editor_tabs
            .iter()
            .filter_map(|tab| tab.sql_editor.active_lazy_fetch_session())
        {
            Self::push_unique_session_id(&mut session_ids, session_id);
        }
        Self::push_unique_session_id_if_some(
            &mut session_ids,
            self.sql_editor.active_lazy_fetch_session(),
        );
        session_ids
    }

    fn mark_progress_context_canceling(&mut self, tab_id: QueryTabId) -> bool {
        self.pending_query_canceling_tabs.insert(tab_id);
        let Some(context) = self.progress_contexts.get_mut(&tab_id) else {
            return false;
        };
        context.state_label = ResultTabStatus::Canceling.label().to_string();
        let Some(statement_index) = context.canceling_statement_index() else {
            return false;
        };
        let Some(tab_id) = context.result_tab_id_for_statement(statement_index) else {
            return false;
        };
        self.result_tabs.mark_statement_canceling_by_id(tab_id);
        true
    }

    fn mark_lazy_fetch_canceling(&mut self, session_id: u64) -> bool {
        self.pending_lazy_fetch_canceling_sessions
            .insert(session_id);
        let mut result_tab_ids = Vec::new();
        let active_lazy_fetch_tab_id = self.active_lazy_fetch_tab_id(session_id);
        for (tab_id, context) in self.progress_contexts.iter_mut() {
            let Some(statement_index) = lazy_fetch_canceling_statement_index(
                context,
                session_id,
                active_lazy_fetch_tab_id == Some(*tab_id),
            ) else {
                continue;
            };
            context.active_statement_index = Some(statement_index);
            context.state_label = ResultTabStatus::Canceling.label().to_string();
            if let Some(tab_id) = context.result_tab_id_for_statement(statement_index) {
                result_tab_ids.push(tab_id);
            }
        }
        result_tab_ids.sort_unstable();
        result_tab_ids.dedup();
        let mut marked = false;
        for tab_id in result_tab_ids {
            self.result_tabs.mark_statement_canceling_by_id(tab_id);
            marked = true;
        }
        self.result_tabs.mark_lazy_fetch_canceling(session_id) || marked
    }

    fn active_lazy_fetch_tab_id(&self, session_id: u64) -> Option<QueryTabId> {
        if self.sql_editor.active_lazy_fetch_session() == Some(session_id) {
            return Some(self.active_editor_tab_id);
        }
        self.editor_tabs
            .iter()
            .find(|tab| tab.sql_editor.active_lazy_fetch_session() == Some(session_id))
            .map(|tab| tab.tab_id)
    }

    fn lazy_fetch_canceling_is_pending(&self, session_id: u64) -> bool {
        self.pending_lazy_fetch_canceling_sessions
            .contains(&session_id)
    }

    fn push_unique_session_id_if_some(session_ids: &mut Vec<u64>, session_id: Option<u64>) {
        if let Some(session_id) = session_id {
            Self::push_unique_session_id(session_ids, session_id);
        }
    }

    fn request_lazy_fetch_on_editors(
        state: &Arc<Mutex<AppState>>,
        session_id: u64,
        request: crate::ui::sql_editor::LazyFetchRequest,
    ) -> bool {
        let editors = {
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut editors = Vec::with_capacity(s.editor_tabs.len().saturating_add(1));
            editors.push(s.sql_editor.clone());
            editors.extend(s.editor_tabs.iter().map(|tab| tab.sql_editor.clone()));
            editors
        };
        for editor in editors {
            if editor.request_lazy_fetch(session_id, request) {
                return true;
            }
        }
        false
    }

    fn tab_sql_text(&self, tab_id: QueryTabId) -> Option<String> {
        self.find_tab_index(tab_id)
            .map(|index| self.editor_tabs[index].sql_buffer.text())
    }

    fn tab_file_path(&self, tab_id: QueryTabId) -> Option<PathBuf> {
        self.find_tab_index(tab_id)
            .and_then(|index| self.editor_tabs[index].current_file.clone())
    }

    fn tab_display_name(&self, tab_id: QueryTabId) -> Option<String> {
        self.find_tab_index(tab_id)
            .map(|index| Self::tab_display_label(&self.editor_tabs[index]))
    }

    fn is_tab_dirty(&self, tab_id: QueryTabId) -> bool {
        self.find_tab_index(tab_id)
            .map(|index| self.editor_tabs[index].is_dirty)
            .unwrap_or(false)
    }

    fn set_tab_dirty(&mut self, tab_id: QueryTabId, is_dirty: bool) {
        let Some(index) = self.find_tab_index(tab_id) else {
            return;
        };
        if self.editor_tabs[index].is_dirty == is_dirty {
            return;
        }
        self.editor_tabs[index].is_dirty = is_dirty;
        let label = Self::tab_display_label(&self.editor_tabs[index]);
        self.query_tabs.set_tab_label(tab_id, &label);
        if self.active_editor_tab_id == tab_id {
            self.refresh_window_title();
        }
    }

    fn set_tab_pristine_text(&mut self, tab_id: QueryTabId, text: String) {
        let Some(index) = self.find_tab_index(tab_id) else {
            return;
        };
        self.editor_tabs[index].current_text_len = text.len();
        self.editor_tabs[index].pristine_text = text;
        self.set_tab_dirty(tab_id, false);
    }

    fn dirty_state_from_equal_length_local_edit(
        pristine_text: &str,
        was_dirty: bool,
        start: usize,
        inserted_text: &str,
    ) -> Option<bool> {
        let end = start.saturating_add(inserted_text.len());
        if pristine_text.get(start..end) != Some(inserted_text) {
            return Some(true);
        }
        (!was_dirty).then_some(false)
    }

    fn on_tab_buffer_modified(
        &mut self,
        tab_id: QueryTabId,
        pos: i32,
        ins: i32,
        del: i32,
        buf: &TextBuffer,
    ) {
        let Some(index) = self.find_tab_index(tab_id) else {
            return;
        };

        let inserted = ins.max(0) as usize;
        let deleted = del.max(0) as usize;
        let tab = &mut self.editor_tabs[index];
        tab.current_text_len = tab
            .current_text_len
            .saturating_add(inserted)
            .saturating_sub(deleted);

        if tab.current_text_len != tab.pristine_text.len() {
            self.set_tab_dirty(tab_id, true);
            return;
        }

        let start = pos.max(0) as usize;
        let inserted_end = pos.saturating_add(ins.max(0)).min(buf.length());
        let inserted_text = buf.text_range(pos.max(0), inserted_end).unwrap_or_default();
        if let Some(is_dirty) = Self::dirty_state_from_equal_length_local_edit(
            &tab.pristine_text,
            tab.is_dirty,
            start,
            &inserted_text,
        ) {
            self.set_tab_dirty(tab_id, is_dirty);
            return;
        }

        let is_dirty = {
            let tab = &self.editor_tabs[index];
            !tab.sql_editor
                .highlight_shadow_text_matches(&tab.pristine_text)
        };
        self.set_tab_dirty(tab_id, is_dirty);
    }

    fn set_tab_file_path(&mut self, tab_id: QueryTabId, path: Option<PathBuf>) {
        let Some(index) = self.find_tab_index(tab_id) else {
            return;
        };
        self.editor_tabs[index].current_file = path.clone();
        let label = Self::tab_display_label(&self.editor_tabs[index]);
        self.query_tabs.set_tab_label(tab_id, &label);
        if self.active_editor_tab_id == tab_id {
            *self
                .current_file
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = path;
            self.refresh_window_title();
        }
    }

    fn normalized_file_identity(path: &Path) -> PathBuf {
        fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    fn sql_file_paths_match(left: &Path, right: &Path) -> bool {
        let left = Self::normalized_file_identity(left);
        let right = Self::normalized_file_identity(right);

        #[cfg(windows)]
        {
            left.to_string_lossy()
                .eq_ignore_ascii_case(&right.to_string_lossy())
        }
        #[cfg(not(windows))]
        {
            left == right
        }
    }

    fn find_tab_id_by_file_path(&self, path: &Path) -> Option<QueryTabId> {
        if path.as_os_str().is_empty() {
            return None;
        }
        self.editor_tabs.iter().find_map(|tab| {
            let current_path = tab.current_file.as_ref()?;
            if Self::sql_file_paths_match(current_path, path) {
                Some(tab.tab_id)
            } else {
                None
            }
        })
    }

    fn activate_editor_tab(&mut self, tab_id: QueryTabId) -> bool {
        self.query_tabs.select(tab_id);
        if self.set_active_editor_tab(tab_id) {
            self.sql_editor.focus();
            true
        } else {
            false
        }
    }

    fn set_status_message(&mut self, message: &str) {
        self.status_animation_running = false;
        self.status_animation_message.clear();
        self.status_animation_frame = 0;
        let conn_info = self
            .connection_info
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        self.status_bar
            .set_label(&format_status(message, &conn_info));
    }

    fn transaction_choice_labels(choice: &Choice) -> String {
        (0..choice.size())
            .filter_map(|index| choice.text(index))
            .collect::<Vec<_>>()
            .join("|")
    }

    fn transaction_control_state(
        &self,
    ) -> Option<(DatabaseType, bool, TransactionMode, TransactionIsolation)> {
        self.connection
            .try_lock()
            .map(|guard| {
                (
                    guard.db_type(),
                    guard.is_connected(),
                    guard.transaction_mode(),
                    guard.default_transaction_isolation(),
                )
            })
            .ok()
    }

    fn selected_transaction_mode_from_controls(&self, db_type: DatabaseType) -> TransactionMode {
        TransactionMode::new(
            transaction_isolation_from_choice_index(
                db_type,
                self.transaction_isolation_choice.value(),
            ),
            transaction_access_from_choice_index(self.transaction_access_choice.value()),
        )
    }

    fn sync_transaction_mode_controls(&mut self) {
        // FLTK Choice menus cannot be rebuilt while a pulldown owns the grab.
        if app::grab().is_some() {
            return;
        }

        let Some((db_type, is_connected, mode, default_isolation)) =
            self.transaction_control_state()
        else {
            if self.has_live_connection {
                self.transaction_isolation_choice.activate();
                self.transaction_access_choice.activate();
            } else {
                self.transaction_isolation_choice.deactivate();
                self.transaction_access_choice.deactivate();
            }
            return;
        };
        let labels = transaction_isolation_choice_labels(db_type, default_isolation);
        if Self::transaction_choice_labels(&self.transaction_isolation_choice) != labels {
            self.transaction_isolation_choice.clear();
            self.transaction_isolation_choice.add_choice(&labels);
        }

        self.transaction_isolation_choice
            .set_value(transaction_isolation_choice_index(db_type, mode.isolation));
        self.transaction_access_choice
            .set_value(transaction_access_choice_index(mode.access_mode));

        if is_connected {
            self.transaction_isolation_choice.activate();
            self.transaction_access_choice.activate();
        } else {
            self.transaction_isolation_choice.deactivate();
            self.transaction_access_choice.deactivate();
        }
    }

    fn sync_transaction_mode_controls_for_connected_db(&mut self, db_type: DatabaseType) {
        // FLTK Choice menus cannot be rebuilt while a pulldown owns the grab.
        if app::grab().is_some() {
            return;
        }

        let labels =
            transaction_isolation_choice_labels(db_type, TransactionIsolation::ReadCommitted);
        if Self::transaction_choice_labels(&self.transaction_isolation_choice) != labels {
            self.transaction_isolation_choice.clear();
            self.transaction_isolation_choice.add_choice(&labels);
        }

        self.transaction_isolation_choice
            .set_value(transaction_isolation_choice_index(
                db_type,
                TransactionIsolation::Default,
            ));
        self.transaction_access_choice
            .set_value(transaction_access_choice_index(
                TransactionAccessMode::ReadWrite,
            ));
        self.transaction_isolation_choice.activate();
        self.transaction_access_choice.activate();
    }

    fn retained_session_preflight_blocker(
        &self,
        action: RetainedSessionPreflightAction,
        action_label: &str,
    ) -> Option<String> {
        if let Some(snapshot) = self.sql_editor.pooled_session_activity_snapshot() {
            let state = snapshot.retained_state;
            if crate::db::retained_session_state_preflight_decision(action, state)
                == RetainedSessionPreflightDecision::RequireResolution
            {
                return Some(format!(
                    "Cannot {action_label} while tab 'Query' has a {} DB session. Commit, rollback, or discard it first.",
                    state.label()
                ));
            }
        }

        self.editor_tabs.iter().find_map(|tab| {
            let snapshot = tab.sql_editor.pooled_session_activity_snapshot()?;
            let state = snapshot.retained_state;
            if crate::db::retained_session_state_preflight_decision(action, state)
                == RetainedSessionPreflightDecision::RequireResolution
            {
                Some(format!(
                    "Cannot {action_label} while tab '{}' has a {} DB session. Commit, rollback, or discard it first.",
                    Self::tab_display_label(tab),
                    state.label()
                ))
            } else {
                None
            }
        })
    }

    fn retained_transaction_option_blocker(&self, action: &str) -> Option<String> {
        let action_label = format!("change {action}");
        self.retained_session_transaction_option_blocker(action, &action_label)
    }

    fn retained_scope_change_blocker(&self) -> Option<String> {
        self.retained_session_preflight_blocker(
            RetainedSessionPreflightAction::ScopeChange,
            "change scope",
        )
    }

    fn retained_session_editors(&self) -> Vec<SqlEditorWidget> {
        let mut editors = Vec::new();
        if self.sql_editor.pooled_session_activity_snapshot().is_some() {
            editors.push(self.sql_editor.clone());
        }
        editors.extend(
            self.editor_tabs
                .iter()
                .filter(|tab| tab.sql_editor.pooled_session_activity_snapshot().is_some())
                .map(|tab| tab.sql_editor.clone()),
        );
        editors
    }

    fn retained_session_transaction_option_decision(
        action: &str,
        snapshot: crate::db::PooledSessionLeaseSnapshot,
    ) -> RetainedSessionPreflightDecision {
        if action == "transaction mode" {
            crate::db::retained_session_state_transaction_mode_change_preflight_decision(
                snapshot.db_type,
                snapshot.retained_state(),
            )
        } else {
            crate::db::retained_session_state_preflight_decision(
                RetainedSessionPreflightAction::TransactionOptionChange,
                snapshot.retained_state(),
            )
        }
    }

    fn retained_session_transaction_option_blocker(
        &self,
        action: &str,
        action_label: &str,
    ) -> Option<String> {
        if let Some(snapshot) = self.sql_editor.pooled_session_activity_snapshot() {
            let state = snapshot.retained_state;
            if Self::retained_session_transaction_option_decision(action, snapshot)
                == RetainedSessionPreflightDecision::RequireResolution
            {
                return Some(format!(
                    "Cannot {action_label} while tab 'Query' has a {} DB session. Commit, rollback, or discard it first.",
                    state.label()
                ));
            }
        }

        self.editor_tabs.iter().find_map(|tab| {
            let snapshot = tab.sql_editor.pooled_session_activity_snapshot()?;
            let state = snapshot.retained_state;
            if Self::retained_session_transaction_option_decision(action, snapshot)
                == RetainedSessionPreflightDecision::RequireResolution
            {
                Some(format!(
                    "Cannot {action_label} while tab '{}' has a {} DB session. Commit, rollback, or discard it first.",
                    Self::tab_display_label(tab),
                    state.label()
                ))
            } else {
                None
            }
        })
    }

    fn retained_session_editors_for_transaction_option_change(
        &self,
        action: &str,
    ) -> Vec<SqlEditorWidget> {
        let mut editors = Vec::new();
        if self
            .sql_editor
            .pooled_session_activity_snapshot()
            .is_some_and(|snapshot| {
                Self::retained_session_transaction_option_decision(action, snapshot)
                    == RetainedSessionPreflightDecision::Allow
            })
        {
            editors.push(self.sql_editor.clone());
        }
        editors.extend(
            self.editor_tabs
                .iter()
                .filter(|tab| {
                    tab.sql_editor
                        .pooled_session_activity_snapshot()
                        .is_some_and(|snapshot| {
                            Self::retained_session_transaction_option_decision(action, snapshot)
                                == RetainedSessionPreflightDecision::Allow
                        })
                })
                .map(|tab| tab.sql_editor.clone()),
        );
        editors
    }

    fn retained_scope_update(&self, scope: Option<String>) -> Option<RetainedScopeUpdate> {
        let scope = Self::normalize_scope_name(scope)?;
        let conn_guard = self
            .connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !conn_guard.is_connected() {
            return None;
        }
        let db_type = conn_guard.db_type();
        if !db_type.has_connection_scope() {
            return None;
        }
        let current_scope = Self::current_connection_scope(&conn_guard, db_type)?;
        if !db_type.scope_values_match(Some(&current_scope), Some(&scope)) {
            return None;
        }
        Some((
            db_type,
            conn_guard.connection_generation(),
            conn_guard.pool_context_epoch(),
            conn_guard.get_info().advanced.clone(),
            scope,
            self.retained_session_editors(),
        ))
    }

    fn append_result_tab_request(&mut self, request: ResultTabRequest) {
        let mut result_tabs = self.result_tabs.clone();
        let tab_id = result_tabs.reserve_result_tab_id();
        let status_message = request.result.message.clone();
        result_tabs.ensure_statement_tab_by_id(tab_id, &request.label, true);
        result_tabs.display_result_by_id(tab_id, &request.result);
        self.refresh_result_edit_controls();
        self.set_status_message(&status_message);
    }

    fn build_session_activity_result_request(&self) -> ResultTabRequest {
        let info = self
            .connection_info
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let connection_name = info
            .as_ref()
            .map(|info| info.name.as_str())
            .unwrap_or("Not connected");
        let db_type = info
            .as_ref()
            .map(|info| info.db_type.to_string())
            .unwrap_or_else(|| "-".to_string());
        let pool_size = self
            .config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .normalized_connection_pool_size();
        let current_activity =
            crate::db::current_db_activity().unwrap_or_else(|| "Idle".to_string());
        let mut entries = self
            .progress_contexts
            .iter()
            .map(|(tab_id, context)| {
                let result_tab = context
                    .active_statement_index
                    .and_then(|statement_index| {
                        context
                            .result_tab_id_for_statement(statement_index)
                            .and_then(|id| self.result_tabs.result_tab_index_for_id(id))
                    })
                    .map(|tab_index| tab_index + 1);
                let fetched_rows = context
                    .active_statement_index
                    .and_then(|statement_index| {
                        context.fetch_row_counts.get(&statement_index).copied()
                    })
                    .unwrap_or(0);
                (
                    *tab_id,
                    SessionActivityEntry {
                        tab_name: self
                            .tab_display_name(*tab_id)
                            .unwrap_or_else(|| format!("Tab {}", tab_id)),
                        result_tab,
                        state: context.state_label.clone(),
                        database: db_type.clone(),
                        sql_preview: context.activity_label.clone(),
                        fetched_rows,
                        elapsed: format_session_activity_elapsed(context.started_at.elapsed()),
                    },
                )
            })
            .collect::<Vec<_>>();
        let progress_tab_ids = entries
            .iter()
            .map(|(tab_id, _)| *tab_id)
            .collect::<HashSet<_>>();
        entries.extend(self.editor_tabs.iter().filter_map(|tab| {
            if progress_tab_ids.contains(&tab.tab_id) {
                return None;
            }
            let snapshot = tab.sql_editor.pooled_session_activity_snapshot()?;
            let state = if snapshot.retained_state.requires_transaction_decision() {
                "Pooled session (transaction decision required)"
            } else if snapshot
                .retained_state
                .requires_physical_session_preservation()
            {
                "Pooled session (transaction, lock, session state, or transaction mode retained)"
            } else {
                "Pooled session (tab session retained)"
            };
            Some((
                tab.tab_id,
                SessionActivityEntry {
                    tab_name: Self::tab_display_label(tab),
                    result_tab: None,
                    state: state.to_string(),
                    database: snapshot.db_type.to_string(),
                    sql_preview: "Idle pooled database session".to_string(),
                    fetched_rows: 0,
                    elapsed: "-".to_string(),
                },
            ))
        }));
        entries.sort_by_key(|(tab_id, _)| *tab_id);
        let mut entries = entries
            .into_iter()
            .map(|(_, entry)| entry)
            .collect::<Vec<_>>();
        entries.extend(
            crate::db::active_pool_db_activity_snapshots()
                .into_iter()
                .map(|activity| SessionActivityEntry {
                    tab_name: "Background".to_string(),
                    result_tab: None,
                    state: "Pool session active".to_string(),
                    database: activity
                        .db_type
                        .map(|db_type| db_type.to_string())
                        .unwrap_or_else(|| db_type.clone()),
                    sql_preview: activity.activity,
                    fetched_rows: 0,
                    elapsed: format_session_activity_elapsed(activity.started_at.elapsed()),
                }),
        );

        build_session_activity_result_request(
            connection_name,
            &db_type,
            pool_size,
            &current_activity,
            entries,
        )
    }

    fn start_status_animation(&mut self, message: &str) {
        self.status_animation_running = true;
        self.status_animation_message.clear();
        self.status_animation_message.push_str(message);
        self.status_animation_frame = 0;
        self.render_status_animation_frame();
    }

    fn update_status_animation(&mut self, message: &str) {
        if !self.status_animation_running {
            self.start_status_animation(message);
            return;
        }
        self.status_animation_message.clear();
        self.status_animation_message.push_str(message);
        self.render_status_animation_frame();
    }

    fn tick_status_animation(&mut self) {
        if !self.status_animation_running {
            return;
        }
        let frame_count = Self::STATUS_SPINNER_FRAMES.len();
        let Some(next_frame) = Self::next_spinner_frame(self.status_animation_frame, frame_count)
        else {
            self.status_animation_running = false;
            self.status_animation_message.clear();
            return;
        };
        self.status_animation_frame = next_frame;
        self.render_status_animation_frame();
    }

    fn render_status_animation_frame(&mut self) {
        if !self.status_animation_running {
            return;
        }
        if Self::STATUS_SPINNER_FRAMES.is_empty() {
            self.status_animation_running = false;
            self.status_animation_message.clear();
            return;
        }
        let frame_idx = self
            .status_animation_frame
            .min(Self::STATUS_SPINNER_FRAMES.len().saturating_sub(1));
        let frame = Self::STATUS_SPINNER_FRAMES[frame_idx];
        let conn_info = self
            .connection_info
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        self.status_bar.set_label(&format_status(
            &format!("{} {}", frame, self.status_animation_message),
            &conn_info,
        ));
    }

    fn refresh_result_edit_controls(&mut self) {
        let can_edit = self.result_tabs.can_current_begin_edit_mode();
        let edit_active = self.result_tabs.is_current_edit_mode_enabled();
        let save_pending = self.result_tabs.is_current_save_pending();
        let query_running = self.is_any_query_running();
        let show_edit_check = can_edit;
        if show_edit_check {
            self.result_toolbar
                .fixed(&self.result_one_tab_edit_gap, RESULT_CHECKBOX_GROUP_GAP);
            if !self.result_one_tab_edit_gap.visible() {
                self.result_one_tab_edit_gap.show();
            }
            self.result_toolbar.fixed(
                &self.result_edit_check,
                result_toolbar_checkbox_width(&self.result_edit_check, BUTTON_WIDTH_SMALL),
            );
            if !self.result_edit_check.visible() {
                self.result_edit_check.show();
            }
            if query_running || save_pending {
                self.result_edit_check.deactivate();
            } else {
                self.result_edit_check.activate();
            }
        } else {
            self.result_edit_check.deactivate();
            if self.result_edit_check.visible() {
                self.result_edit_check.hide();
            }
            if self.result_one_tab_edit_gap.visible() {
                self.result_one_tab_edit_gap.hide();
            }
            self.result_toolbar.fixed(&self.result_one_tab_edit_gap, 0);
            self.result_toolbar.fixed(&self.result_edit_check, 0);
        }
        let desired_checked = edit_active && can_edit;
        if self.result_edit_check.value() != desired_checked {
            self.result_edit_check.set(desired_checked);
        }

        let show_action_buttons = edit_active && can_edit;
        let actions_enabled = show_action_buttons && !save_pending && !query_running;
        set_result_action_button_visibility(
            &mut self.result_toolbar,
            &mut self.result_insert_btn,
            show_action_buttons,
        );
        set_result_action_button_visibility(
            &mut self.result_toolbar,
            &mut self.result_delete_btn,
            show_action_buttons,
        );
        set_result_action_button_visibility(
            &mut self.result_toolbar,
            &mut self.result_save_btn,
            show_action_buttons,
        );
        set_result_action_button_visibility(
            &mut self.result_toolbar,
            &mut self.result_cancel_btn,
            show_action_buttons,
        );
        if show_action_buttons {
            if actions_enabled {
                self.result_insert_btn.activate();
                self.result_delete_btn.activate();
                self.result_save_btn.activate();
                self.result_cancel_btn.activate();
                self.result_edit_check.activate();
            } else {
                self.result_insert_btn.deactivate();
                self.result_delete_btn.deactivate();
                self.result_save_btn.deactivate();
                self.result_cancel_btn.deactivate();
                self.result_edit_check.deactivate();
            }
        }
        self.result_toolbar.layout();
        self.result_toolbar.redraw();
    }

    /// Enable or disable connection-dependent toolbar buttons and menu items.
    /// Execute remains enabled even when disconnected so scripts can CONNECT.
    /// Call this whenever the connection state changes
    /// (connect, disconnect, or connection lost).
    fn refresh_connection_dependent_controls(&mut self) {
        // If the connection lock is held (query is running) treat the state as
        // connected so we never disable buttons mid-execution.
        let is_connected = self
            .connection
            .try_lock()
            .map(|g| g.is_connected())
            .unwrap_or(true);

        // Regression guard: keep Execute enabled even when disconnected.
        // Script execution may begin with CONNECT (or @script that contains CONNECT),
        // so re-coupling this button to `is_connected` would break reconnect workflows.
        self.execute_btn.activate();

        if is_connected {
            self.query_cancel_btn.activate();
            self.explain_btn.activate();
            self.commit_btn.activate();
            self.rollback_btn.activate();
        } else {
            self.query_cancel_btn.deactivate();
            self.explain_btn.deactivate();
            self.commit_btn.deactivate();
            self.rollback_btn.deactivate();
        }

        // Sync the Disconnect menu item so it is only active when connected.
        if let Some(menu) = app::widget_from_id::<MenuBar>("main_menu") {
            if let Some(mut item) = menu.find_item("&File/&Disconnect") {
                if is_connected {
                    item.activate();
                } else {
                    item.deactivate();
                }
            }
        }
        self.sync_transaction_mode_controls();
    }
}

const FETCH_STATUS_UPDATE_INTERVAL: Duration = Duration::from_millis(250);
const STATUS_ANIMATION_INTERVAL: f64 = 0.08;

/// 접속 정보를 상태 표시줄 메시지 끝에 붙는 헬퍼
fn format_status(msg: &str, conn_info: &Option<crate::db::ConnectionInfo>) -> String {
    match conn_info {
        Some(info) => format!("{} | {}", msg, info.name),
        None => msg.to_string(),
    }
}

fn execution_finished_status_override(
    event: &crate::db::session_policy::ExecutionFinishedEvent,
    snapshot: Option<crate::db::PooledSessionLeaseSnapshot>,
) -> Option<&'static str> {
    if event.cancelled
        && snapshot.is_some_and(|snapshot| snapshot.retained_state.requires_transaction_decision())
    {
        Some("Cancelled | Transaction decision required")
    } else {
        None
    }
}

fn execution_finished_event_matches_current_editor(
    event: &crate::db::session_policy::ExecutionFinishedEvent,
    callback_tab_id: QueryTabId,
    current_editor_id: Option<u64>,
    current_operation_id: u64,
    last_completed_operation_id: u64,
    current_connection_generation: Option<u64>,
) -> bool {
    if event.tab_id != callback_tab_id || current_editor_id != Some(event.editor_id) {
        return false;
    }
    if event.operation_id == 0 || event.connection_generation == 0 {
        return false;
    }
    // The worker emits ExecutionFinished before clearing current_operation_id,
    // but the UI may poll it after cleanup. Once current_operation_id is back
    // to zero, require the editor's last completed operation id; otherwise an
    // older event can pass after a newer operation has already finished.
    if current_operation_id != 0 {
        if current_operation_id != event.operation_id {
            return false;
        }
    } else if last_completed_operation_id != event.operation_id {
        return false;
    }
    current_connection_generation == Some(event.connection_generation)
}

fn operation_progress_token_matches_current_editor(
    callback_tab_id: QueryTabId,
    token: QueryOperationToken,
    current_editor_id: Option<u64>,
    current_operation_id: u64,
    last_completed_operation_id: u64,
    abandoned_operations: &HashSet<QueryOperationToken>,
) -> bool {
    if token.tab_id != callback_tab_id
        || token.operation_id == 0
        || current_editor_id != Some(token.editor_id)
        || abandoned_operations.contains(&token)
    {
        return false;
    }
    if current_operation_id != 0 {
        current_operation_id == token.operation_id
    } else {
        last_completed_operation_id == token.operation_id
    }
}

fn should_update_fetch_status(previous_count: usize, elapsed: Duration) -> bool {
    previous_count == 0 || elapsed >= FETCH_STATUS_UPDATE_INTERVAL
}

fn should_refresh_fetch_status_animation(
    status_animation_running: bool,
    previous_count: usize,
    elapsed: Duration,
) -> bool {
    !status_animation_running || should_update_fetch_status(previous_count, elapsed)
}

pub struct MainWindow {
    state: Arc<Mutex<AppState>>,
}

#[derive(Clone)]
enum ConnectionResult {
    Success(Box<crate::db::ConnectionInfo>),
    Failure(String),
}

enum FileActionResult {
    OpenInNewTab {
        path: PathBuf,
        result: Result<String, String>,
    },
    Export {
        path: PathBuf,
        row_count: usize,
        result: Result<(), String>,
    },
}

enum SaveTabOutcome {
    Saved,
    Cancelled,
    Failed(String),
}

fn should_ignore_query_progress_when_disconnected(
    has_live_connection: bool,
    has_running_queries: bool,
) -> bool {
    !has_live_connection && !has_running_queries
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ResultPaneRoute {
    DataGrid,
    #[cfg(test)]
    ScriptOutput,
    #[cfg(test)]
    DbmsOutput,
    MessagesInfo,
    MessagesErrors,
}

fn statement_finished_result_routes(
    result: &QueryResult,
    script_transcript: bool,
    result_status: ResultTabStatus,
) -> Vec<ResultPaneRoute> {
    let mut routes = Vec::new();
    if should_display_result_in_data_grid(result) {
        routes.push(ResultPaneRoute::DataGrid);
    }
    if result_status == ResultTabStatus::Error && !result.message.trim().is_empty() {
        routes.push(ResultPaneRoute::MessagesErrors);
    } else if result_status != ResultTabStatus::Cancelled
        && should_send_success_message_to_info(result, script_transcript)
    {
        routes.push(ResultPaneRoute::MessagesInfo);
    }
    routes
}

#[cfg(test)]
pub(crate) fn result_pane_routes_for_progress(progress: &QueryProgress) -> Vec<ResultPaneRoute> {
    result_pane_routes_for_progress_with_script_context(progress, false)
}

#[cfg(test)]
pub(crate) fn result_pane_routes_for_progress_with_script_context(
    progress: &QueryProgress,
    script_transcript: bool,
) -> Vec<ResultPaneRoute> {
    match progress {
        QueryProgress::Operation { progress, .. } => {
            result_pane_routes_for_progress_with_script_context(progress, script_transcript)
        }
        QueryProgress::OperationAbandoned { .. } => Vec::new(),
        QueryProgress::StatementStart { .. } => Vec::new(),
        QueryProgress::SelectStart { columns, .. } => {
            if columns.is_empty() {
                Vec::new()
            } else {
                vec![ResultPaneRoute::DataGrid]
            }
        }
        QueryProgress::Rows { .. }
        | QueryProgress::LazyFetchSession { .. }
        | QueryProgress::LazyFetchWaiting { .. }
        | QueryProgress::LazyFetchCanceling { .. }
        | QueryProgress::LazyFetchClosed { .. } => vec![ResultPaneRoute::DataGrid],
        QueryProgress::ScriptOutput { .. } => vec![ResultPaneRoute::ScriptOutput],
        QueryProgress::DbmsOutput { .. } => vec![ResultPaneRoute::DbmsOutput],
        QueryProgress::Message { kind, .. } => match kind {
            ResultMessageKind::Info => vec![ResultPaneRoute::MessagesInfo],
            ResultMessageKind::Error => vec![ResultPaneRoute::MessagesErrors],
        },
        QueryProgress::ExplainPlanOutput { .. } => vec![ResultPaneRoute::DataGrid],
        QueryProgress::StatementFinished { result, .. } => statement_finished_result_routes(
            result,
            script_transcript,
            statement_finished_status(result, false),
        ),
        QueryProgress::BatchStart { .. }
        | QueryProgress::PromptInput { .. }
        | QueryProgress::RequestCancelOldestLazyFetchForSessionPool { .. }
        | QueryProgress::NotifyCancelOldestLazyFetchForSessionPool
        | QueryProgress::AutoCommitChanged { .. }
        | QueryProgress::ConnectionChanged { .. }
        | QueryProgress::DatabaseChanged { .. }
        | QueryProgress::ScopeChangedNotice { .. }
        | QueryProgress::WorkerPanicked { .. }
        | QueryProgress::MetadataRefreshNeeded
        | QueryProgress::ExecutionFinished(_)
        | QueryProgress::BatchFinished => Vec::new(),
    }
}

fn should_display_result_in_data_grid(result: &QueryResult) -> bool {
    result.is_select && result.success && !result.columns.is_empty()
}

fn should_send_success_message_to_info(result: &QueryResult, script_transcript: bool) -> bool {
    result.success && !result.message.trim().is_empty() && (result.is_select || !script_transcript)
}

fn script_transcript_owns_success_message(context: Option<&QueryProgressContext>) -> bool {
    context.is_some_and(|context| context.activity_label.starts_with("Executing script:"))
}

fn should_select_support_result_pane(context: Option<&QueryProgressContext>) -> bool {
    !context.is_some_and(|context| {
        context.execution_target.is_some() || !context.result_tab_ids.is_empty()
    })
}

fn should_run_global_batch_cleanup(has_running_queries: bool) -> bool {
    !has_running_queries
}

fn should_accept_lazy_fetch_session_event(
    event_is_current: bool,
    active_lazy_fetch_session: Option<u64>,
    context: Option<&QueryProgressContext>,
    statement_index: usize,
) -> bool {
    if event_is_current {
        return true;
    }

    if active_lazy_fetch_session.is_some() {
        return false;
    }

    context.is_some_and(|context| {
        !context.batch_finished
            && context.active_statement_index == Some(statement_index)
            && !context.closed_statement_indices.contains(&statement_index)
    })
}

fn validate_result_edit_action_allowed(has_running_queries: bool) -> Result<(), String> {
    if has_running_queries {
        Err("A query is running. Wait for completion before editing result rows.".to_string())
    } else {
        Ok(())
    }
}

fn connection_transition_block_message(
    has_running_query: bool,
    has_active_lazy_fetches: bool,
    action: &str,
) -> Option<String> {
    if has_running_query {
        Some(format!("A query is running. Stop it before {action}."))
    } else if has_active_lazy_fetches {
        Some(format!(
            "A lazy fetch is still open. Fetch all rows or cancel it before {action}."
        ))
    } else {
        None
    }
}

fn transaction_option_block_message(
    has_running_query: bool,
    has_active_lazy_fetches: bool,
    action: &str,
) -> Option<String> {
    connection_transition_block_message(has_running_query, has_active_lazy_fetches, action)
}

fn should_finish_progress_after_lazy_fetch_close(
    _cancelled: bool,
    finished_all_lazy_fetches: bool,
) -> bool {
    finished_all_lazy_fetches
}

fn orphaned_canceling_lazy_fetch_sessions<F>(
    context: Option<&QueryProgressContext>,
    pending_canceling_sessions: &HashSet<u64>,
    mut session_is_active: F,
) -> Vec<u64>
where
    F: FnMut(u64) -> bool,
{
    let Some(context) = context else {
        return Vec::new();
    };
    let context_is_canceling = context.state_label == ResultTabStatus::Canceling.label();
    let mut sessions = context
        .lazy_fetch_sessions
        .keys()
        .copied()
        .filter(|session_id| {
            (context_is_canceling || pending_canceling_sessions.contains(session_id))
                && !session_is_active(*session_id)
        })
        .collect::<Vec<_>>();
    sessions.sort_unstable();
    sessions
}

fn statement_finished_status(result: &QueryResult, context_was_canceling: bool) -> ResultTabStatus {
    if context_was_canceling && !result.success {
        ResultTabStatus::Cancelled
    } else {
        ResultTabStatus::from_query_result(result)
    }
}

fn statement_start_status(current_label: &str, query_canceling_pending: bool) -> ResultTabStatus {
    if query_canceling_pending || current_label == ResultTabStatus::Canceling.label() {
        ResultTabStatus::Canceling
    } else {
        ResultTabStatus::Running
    }
}

fn lazy_fetch_canceling_statement_index(
    context: &QueryProgressContext,
    session_id: u64,
    fallback_to_active_statement: bool,
) -> Option<usize> {
    context.lazy_fetch_sessions.get(&session_id).copied().or({
        if fallback_to_active_statement {
            context.active_statement_index
        } else {
            None
        }
    })
}

fn lazy_fetch_close_should_abort_result_tab(
    cancelled: bool,
    cursor_closed: bool,
    fetch_worker_done: bool,
    error_kind: crate::ui::sql_editor::InterruptKind,
) -> bool {
    cancelled
        || !cursor_closed
        || !fetch_worker_done
        || matches!(
            error_kind,
            crate::ui::sql_editor::InterruptKind::UnsafeOrUnknown
                | crate::ui::sql_editor::InterruptKind::ConnectionError
                | crate::ui::sql_editor::InterruptKind::NonRecoverableTimeout
        )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionPoolSlotAction {
    None,
    CancelLazyFetch,
}

fn session_pool_slot_action(
    active_lazy_fetches: usize,
    connection_pool_size: u32,
) -> SessionPoolSlotAction {
    let connection_pool_size = (connection_pool_size as usize).max(1);
    if active_lazy_fetches >= connection_pool_size {
        return SessionPoolSlotAction::CancelLazyFetch;
    }
    SessionPoolSlotAction::None
}

fn request_lazy_fetch_cancel_for_session_pool(
    state: &Arc<Mutex<AppState>>,
    session_id: u64,
) -> bool {
    let requested = AppState::request_lazy_fetch_on_editors(
        state,
        session_id,
        crate::ui::sql_editor::LazyFetchRequest::CancelAndDiscard,
    );
    let mut guard = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if requested {
        guard.mark_lazy_fetch_canceling(session_id);
        guard.set_status_message("Session pool full; canceling oldest lazy fetch...");
        guard.refresh_result_edit_controls();
        true
    } else {
        guard.mark_lazy_fetch_cancelled(session_id, "Session pool full; lazy fetch already closed");
        false
    }
}

#[derive(Clone, Copy)]
enum SqlExecutionRequest {
    Current,
    StatementAtCursor,
    Selected,
}

fn acquire_sql_editor_if_idle(state: &Arc<Mutex<AppState>>) -> Option<SqlEditorWidget> {
    let (editor, blocked_message) = {
        let guard = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.is_any_query_running() {
            (None, Some(crate::db::format_connection_busy_message()))
        } else {
            (Some(guard.sql_editor.clone()), None)
        }
    };

    if let Some(message) = blocked_message {
        SqlEditorWidget::show_alert_dialog(&message);
    }

    editor
}

fn cancel_oldest_lazy_fetch_if_session_pool_full(state: &Arc<Mutex<AppState>>) -> bool {
    let connection = {
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .connection
            .clone()
    };
    let connection_pool_size = connection
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .connection_pool_size();

    let session_id = {
        let guard = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let active_sessions = guard.lazy_fetch_sessions_for_abort();
        match session_pool_slot_action(active_sessions.len(), connection_pool_size) {
            SessionPoolSlotAction::None => return false,
            SessionPoolSlotAction::CancelLazyFetch => {}
        }
        let Some(session_id) = guard.oldest_lazy_fetch_session() else {
            return false;
        };
        session_id
    };

    request_lazy_fetch_cancel_for_session_pool(state, session_id)
}

fn run_sql_execution_request(state: &Arc<Mutex<AppState>>, request: SqlExecutionRequest) {
    let Some(editor) = acquire_sql_editor_if_idle(state) else {
        return;
    };
    match request {
        SqlExecutionRequest::Current => editor.execute_current(),
        SqlExecutionRequest::StatementAtCursor => editor.execute_statement_at_cursor(),
        SqlExecutionRequest::Selected => editor.execute_selected(),
    }
}

fn execute_sql_request_with_session_pool_slot(
    state: &Arc<Mutex<AppState>>,
    request: SqlExecutionRequest,
) {
    if cancel_oldest_lazy_fetch_if_session_pool_full(state) {
        let state_for_execute = Arc::clone(state);
        crate::ui::ui_timeout::schedule(0.2, move || {
            run_sql_execution_request(&state_for_execute, request);
        });
    } else {
        run_sql_execution_request(state, request);
    }
}

fn update_transaction_mode_from_controls(state: &Arc<Mutex<AppState>>) {
    let (connection, previous_mode, mode, retained_editors) = {
        let mut s = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(message) = transaction_option_block_message(
            s.is_any_query_running(),
            s.has_active_lazy_fetches(),
            "changing transaction mode",
        ) {
            crate::ui::alert_on_main(&message);
            s.sync_transaction_mode_controls();
            s.set_status_message(&message);
            return;
        }
        if let Some(message) = s.retained_transaction_option_blocker("transaction mode") {
            crate::ui::alert_on_main(&message);
            s.sync_transaction_mode_controls();
            s.set_status_message(&message);
            return;
        }
        let Some((db_type, _, current_mode, _)) = s.transaction_control_state() else {
            crate::ui::alert_on_main(&format_connection_busy_message());
            return;
        };
        (
            s.connection.clone(),
            current_mode,
            s.selected_transaction_mode_from_controls(db_type),
            s.retained_session_editors_for_transaction_option_change("transaction mode"),
        )
    };

    let shared_connection = Arc::clone(&connection);
    let (status, should_sync_controls, mode_applied) = if let Some(mut connection) =
        try_lock_connection_with_activity(&connection, "Updating transaction mode")
    {
        let retained_plan = RetainedSessionOptionChangePlan::new(&connection, retained_editors);
        if let Err(err) = retained_plan.validate_transaction_option_change("transaction mode") {
            crate::ui::alert_on_main(&err);
            (format!("Transaction mode unchanged: {}", err), true, false)
        } else {
            crate::db::clear_pool_session_context_for_shared_connection(&shared_connection);
            match connection.set_transaction_mode(mode) {
                Ok(()) => {
                    crate::db::refresh_pool_session_context_cache_for_shared_connection(
                        &shared_connection,
                        &connection,
                    );
                    let pool_context_epoch = connection.pool_context_epoch();
                    drop(connection);
                    let retained_outcomes = retained_plan.apply_transaction_mode(
                        pool_context_epoch,
                        mode,
                        "Updating transaction mode",
                    );
                    if let Some(message) = first_retained_outcome_message(&retained_outcomes) {
                        crate::ui::alert_on_main(&format!(
                        "Transaction mode was changed, but a retained session could not be updated. It was restored or discarded according to session safety: {}",
                        message
                    ));
                    }
                    (format!("Transaction mode: {}", mode.label()), true, true)
                }
                Err(err) => {
                    crate::ui::alert_on_main(&err);
                    (format!("Transaction mode unchanged: {}", err), true, false)
                }
            }
        }
    } else {
        let busy_message = format_connection_busy_message();
        crate::ui::alert_on_main(&busy_message);
        (busy_message, false, false)
    };

    let mut s = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if should_sync_controls {
        s.sync_transaction_mode_controls();
    }
    s.set_status_message(&status);
    drop(s);

    if mode_applied && mode != previous_mode {
        crate::ui::message_on_main(transaction_mode_new_transaction_notice());
    }
}

fn resolve_active_progress_tab_id(
    state: &AppState,
    tab_id: QueryTabId,
    statement_index: usize,
) -> Option<ResultTabId> {
    let has_running_queries = state.sql_editor.is_query_running()
        || state
            .editor_tabs
            .iter()
            .any(|tab| tab.sql_editor.is_query_running());
    if should_ignore_query_progress_when_disconnected(
        state.has_live_connection,
        has_running_queries,
    ) {
        return None;
    }

    let context = state.progress_contexts.get(&tab_id)?;
    if context.closed_statement_indices.contains(&statement_index) {
        return None;
    }

    context.result_tab_id_for_statement(statement_index)
}

fn format_session_activity_elapsed(elapsed: Duration) -> String {
    let total_seconds = elapsed.as_secs();
    let minutes = safe_div(total_seconds, 60);
    let seconds = safe_rem(total_seconds, 60);
    if minutes > 0 {
        format!("{}m {:02}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}

fn session_activity_column(name: &str, data_type: &str) -> ColumnInfo {
    ColumnInfo {
        name: name.to_string(),
        data_type: data_type.to_string(),
    }
}

fn build_session_activity_result_request(
    connection_name: &str,
    db_type: &str,
    pool_size: u32,
    current_activity: &str,
    entries: Vec<SessionActivityEntry>,
) -> ResultTabRequest {
    let columns = vec![
        session_activity_column("Connection", "VARCHAR2"),
        session_activity_column("Database", "VARCHAR2"),
        session_activity_column("Pool Size", "NUMBER"),
        session_activity_column("Tab", "VARCHAR2"),
        session_activity_column("Result Tab", "VARCHAR2"),
        session_activity_column("State", "VARCHAR2"),
        session_activity_column("Current Activity", "VARCHAR2"),
        session_activity_column("SQL Preview", "VARCHAR2"),
        session_activity_column("Fetched Rows", "NUMBER"),
        session_activity_column("Elapsed", "VARCHAR2"),
    ];

    let pool_size = pool_size.to_string();
    let has_active_entries = !entries.is_empty();
    let rows = if !has_active_entries {
        vec![vec![
            connection_name.to_string(),
            db_type.to_string(),
            pool_size,
            "-".to_string(),
            "-".to_string(),
            "Idle".to_string(),
            current_activity.to_string(),
            "-".to_string(),
            "-".to_string(),
            "-".to_string(),
        ]]
    } else {
        entries
            .into_iter()
            .map(|entry| {
                vec![
                    connection_name.to_string(),
                    entry.database,
                    pool_size.clone(),
                    entry.tab_name,
                    entry
                        .result_tab
                        .map(|index| index.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    entry.state,
                    current_activity.to_string(),
                    entry.sql_preview,
                    entry.fetched_rows.to_string(),
                    entry.elapsed,
                ]
            })
            .collect::<Vec<_>>()
    };
    let message = if has_active_entries {
        format!("{} session(s)", rows.len())
    } else {
        "No active sessions".to_string()
    };

    ResultTabRequest {
        label: "Session Activity".to_string(),
        result: QueryResult {
            sql: String::new(),
            columns,
            row_count: rows.len(),
            rows,
            execution_time: Duration::from_secs(0),
            message,
            is_select: true,
            success: true,
        },
    }
}

impl MainWindow {
    fn clone_result_tabs_for_edit_action(
        state: &Arc<Mutex<AppState>>,
    ) -> Result<ResultTabsWidget, String> {
        let mut guard = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Err(err) = validate_result_edit_action_allowed(guard.is_any_query_running()) {
            guard.set_status_message(&err);
            guard.refresh_result_edit_controls();
            return Err(err);
        }
        Ok(guard.result_tabs.clone())
    }

    fn prepare_result_export(
        state: &Arc<Mutex<AppState>>,
        callback: Box<dyn FnMut(String, usize)>,
    ) -> Result<Option<(String, usize)>, String> {
        let result_tabs = {
            let guard = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !guard.result_tabs.has_data() {
                return Err("No results to export".to_string());
            }
            guard.result_tabs.clone()
        };

        Ok(result_tabs.export_to_csv_after_fetch_all(callback))
    }

    fn sync_recent_sql_file_menu(recent_sql_files: &[PathBuf]) {
        let recent_sql_files = recent_sql_files.to_vec();
        crate::ui::ui_timeout::schedule(0.0, move || {
            if let Some(mut menu) = app::widget_from_id::<MenuBar>("main_menu") {
                MenuBarBuilder::sync_recent_sql_file_items(&mut menu, &recent_sql_files);
            }
        });
    }

    fn record_recent_sql_file(state: &mut AppState, path: &Path) {
        let (recent_sql_files, save_result) = {
            let mut config = state
                .config
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            config.add_recent_sql_file(path);
            let recent_sql_files = config.recent_sql_files.clone();
            let save_result = config.save().map_err(|err| err.to_string());
            (recent_sql_files, save_result)
        };
        Self::sync_recent_sql_file_menu(&recent_sql_files);
        if let Err(err) = save_result {
            crate::utils::logging::log_warning(
                "config",
                &format!("Failed to save recent SQL file history: {err}"),
            );
        }
    }

    fn open_sql_file_path(
        state: &Arc<Mutex<AppState>>,
        file_sender: &std::sync::mpsc::Sender<FileActionResult>,
        path: PathBuf,
    ) {
        if path.as_os_str().is_empty() {
            return;
        }

        {
            let mut s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if MainWindow::focus_existing_tab_with_same_file_path(&mut s, &path) {
                MainWindow::record_recent_sql_file(&mut s, &path);
                return;
            }
            let conn_info = s
                .connection_info
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            let file_label = path.file_name().unwrap_or_default().to_string_lossy();
            s.status_bar.set_label(&format_status(
                &format!("Opening {} in new tab", file_label),
                &conn_info,
            ));
        }

        let sender = file_sender.clone();
        thread::spawn(move || {
            let result = fs::read_to_string(&path).map_err(|err| err.to_string());
            let _ = sender.send(FileActionResult::OpenInNewTab { path, result });
            app::awake();
        });
    }

    fn open_recent_sql_file_path(
        state: &Arc<Mutex<AppState>>,
        schema_sender: &std::sync::mpsc::Sender<SchemaUpdate>,
        file_sender: &std::sync::mpsc::Sender<FileActionResult>,
        path: PathBuf,
    ) {
        if path.as_os_str().is_empty() {
            return;
        }

        {
            let mut s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if MainWindow::focus_existing_tab_with_same_file_path(&mut s, &path) {
                MainWindow::record_recent_sql_file(&mut s, &path);
                return;
            }
            let conn_info = s
                .connection_info
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            let file_label = path.file_name().unwrap_or_default().to_string_lossy();
            s.status_bar.set_label(&format_status(
                &format!("Opening {} in new tab", file_label),
                &conn_info,
            ));
        }

        let result = fs::read_to_string(&path).map_err(|err| err.to_string());
        let mut created_tab: Option<QueryTabId> = None;
        let mut created_editor: Option<SqlEditorWidget> = None;
        let mut created_right_tile: Option<Tile> = None;
        let mut deferred_alert: Option<String> = None;

        {
            let mut s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match result {
                Ok(content) => {
                    if MainWindow::focus_existing_tab_with_same_file_path(&mut s, &path) {
                        MainWindow::record_recent_sql_file(&mut s, &path);
                        return;
                    }
                    let normalized_content = MainWindow::normalize_line_endings_for_editor(content);
                    if let Some(tab_id) = MainWindow::create_query_editor_tab(&mut s) {
                        s.sql_buffer.set_text(&normalized_content);
                        s.sql_editor.reset_undo_redo_history();
                        s.set_tab_file_path(tab_id, Some(path.clone()));
                        s.set_tab_pristine_text(tab_id, normalized_content);
                        created_editor = Some(s.sql_editor.clone());
                        created_right_tile = Some(s.right_tile.clone());
                        created_tab = Some(tab_id);
                        MainWindow::record_recent_sql_file(&mut s, &path);
                    }
                }
                Err(err) => {
                    deferred_alert = Some(format!("Failed to open SQL file: {}", err));
                }
            }
        }

        if let Some(alert_msg) = deferred_alert {
            crate::ui::alert_on_main(&alert_msg);
        }

        if let Some(tab_id) = created_tab {
            MainWindow::attach_editor_callbacks(state, tab_id, schema_sender.clone());
            MainWindow::attach_file_drop_callback(state, tab_id, file_sender.clone());
            if let Some(mut editor) = created_editor {
                editor.focus();
            }
            if let Some(mut right_tile) = created_right_tile {
                right_tile.redraw();
            }
            app::redraw();
        }
    }

    /// Appends `default_ext` (without a dot) to a save target whose typed name
    /// lacks an extension, mirroring the extension of the selected file-type
    /// filter. When the "All Files" filter is selected (`all_files_index`) the
    /// name is returned untouched.
    fn apply_default_extension(
        path: PathBuf,
        default_ext: &str,
        filter_value: i32,
        all_files_index: Option<i32>,
    ) -> PathBuf {
        if Some(filter_value) == all_files_index || path.extension().is_some() {
            return path;
        }
        path.with_extension(default_ext)
    }

    fn export_current_results_to_csv(
        state: &Arc<Mutex<AppState>>,
        file_sender: &std::sync::mpsc::Sender<FileActionResult>,
    ) {
        let mut dialog = FileDialog::new(FileDialogType::BrowseSaveFile);
        dialog.set_filter("CSV Files\t*.csv");
        dialog.show();
        let filename = dialog.filename();
        if filename.as_os_str().is_empty() {
            return;
        }
        // The native chooser auto-appends an "All Files" entry after our single
        // "CSV Files" filter, so it sits at index 1 (skip); index 0 → force .csv.
        let filename =
            Self::apply_default_extension(filename, "csv", dialog.filter_value(), Some(1));

        let sender = file_sender.clone();
        let deferred_sender = sender.clone();
        let deferred_filename = filename.clone();
        let export = match MainWindow::prepare_result_export(
            state,
            Box::new(move |csv, row_count| {
                let sender = deferred_sender.clone();
                let filename = deferred_filename.clone();
                thread::spawn(move || {
                    let result = fs::write(&filename, csv).map_err(|err| err.to_string());
                    let _ = sender.send(FileActionResult::Export {
                        path: filename,
                        row_count,
                        result,
                    });
                    app::awake();
                });
            }),
        ) {
            Ok(export) => export,
            Err(message) => {
                crate::ui::alert_on_main(&message);
                return;
            }
        };
        let Some((csv, row_count)) = export else {
            return;
        };
        thread::spawn(move || {
            let result = fs::write(&filename, csv).map_err(|err| err.to_string());
            let _ = sender.send(FileActionResult::Export {
                path: filename,
                row_count,
                result,
            });
            app::awake();
        });
    }

    fn close_current_result_tab(state: &Arc<Mutex<AppState>>) {
        let target = {
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            s.result_tabs
                .active_result_id()
                .map(ResultTabCloseTarget::Result)
                .unwrap_or(ResultTabCloseTarget::ScriptOutput)
        };
        Self::close_result_tab_by_target(state, target);
    }

    fn close_result_tab_by_target(state: &Arc<Mutex<AppState>>, target: ResultTabCloseTarget) {
        let query_running = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_any_query_running();
        if query_running {
            crate::ui::alert_on_main("A query is running. Stop it before closing tabs.");
            return;
        }
        let lazy_fetch_sessions = {
            let mut s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match target {
                ResultTabCloseTarget::Result(result_tab_id) => {
                    let closed = s
                        .result_tabs
                        .close_tab_by_id_and_take_lazy_fetch(result_tab_id);
                    if let Some((closed_tab_id, lazy_fetch_session)) = closed {
                        let mut sessions_to_cancel = s.mark_result_tab_closed_by_id(closed_tab_id);
                        if let Some(session_id) = lazy_fetch_session {
                            s.mark_lazy_fetch_result_tab_closed(session_id);
                            AppState::push_unique_session_id(&mut sessions_to_cancel, session_id);
                        }
                        for session_id in s.abort_lazy_fetches_without_result_tab_mapping() {
                            AppState::push_unique_session_id(&mut sessions_to_cancel, session_id);
                        }
                        malloc_trim_process();
                        s.refresh_result_edit_controls();
                        app::redraw();
                        sessions_to_cancel
                    } else {
                        s.refresh_result_edit_controls();
                        app::redraw();
                        Vec::new()
                    }
                }
                ResultTabCloseTarget::ScriptOutput => {
                    s.result_tabs.close_script_output_tab();
                    s.refresh_result_edit_controls();
                    app::redraw();
                    Vec::new()
                }
            }
        };
        for session_id in lazy_fetch_sessions {
            AppState::request_lazy_fetch_on_editors(
                state,
                session_id,
                crate::ui::sql_editor::LazyFetchRequest::CancelAndDiscard,
            );
        }
    }

    fn close_all_result_tabs(state: &Arc<Mutex<AppState>>) {
        let query_running = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_any_query_running();
        if query_running {
            crate::ui::alert_on_main("A query is running. Stop it before closing grid tabs.");
            return;
        }
        let lazy_fetch_sessions = {
            let mut s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let had_tabs = s.result_tabs.tab_count() > 0;
            let lazy_fetch_sessions = s.lazy_fetch_sessions_for_abort();
            s.result_tabs.clear_grids();
            s.mark_lazy_fetch_result_tabs_closed(lazy_fetch_sessions.clone());
            s.mark_all_result_tabs_closed_for_clear();
            if had_tabs {
                malloc_trim_process();
            }
            s.refresh_result_edit_controls();
            app::redraw();
            lazy_fetch_sessions
        };
        for session_id in lazy_fetch_sessions {
            AppState::request_lazy_fetch_on_editors(
                state,
                session_id,
                crate::ui::sql_editor::LazyFetchRequest::CancelAndDiscard,
            );
        }
    }

    fn clear_current_result_view(state: &Arc<Mutex<AppState>>) {
        let result_target = {
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            s.result_tabs
                .active_result_id()
                .map(ResultTabCloseTarget::Result)
        };
        if let Some(target) = result_target {
            Self::close_result_tab_by_target(state, target);
            return;
        }

        Self::clear_current_result_support_section(state);
    }

    fn clear_all_result_views(state: &Arc<Mutex<AppState>>) {
        let query_running = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_any_query_running();
        if query_running {
            crate::ui::alert_on_main("A query is running. Stop it before clearing results.");
            return;
        }
        let lazy_fetch_sessions = {
            let mut s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let had_tabs = s.result_tabs.tab_count() > 0;
            let lazy_fetch_sessions = s.lazy_fetch_sessions_for_abort();
            s.result_tabs.clear();
            s.mark_lazy_fetch_result_tabs_closed(lazy_fetch_sessions.clone());
            s.mark_all_result_tabs_closed_for_clear();
            if had_tabs {
                malloc_trim_process();
            }
            s.refresh_result_edit_controls();
            app::redraw();
            lazy_fetch_sessions
        };
        for session_id in lazy_fetch_sessions {
            AppState::request_lazy_fetch_on_editors(
                state,
                session_id,
                crate::ui::sql_editor::LazyFetchRequest::CancelAndDiscard,
            );
        }
    }

    fn clear_current_result_support_section(state: &Arc<Mutex<AppState>>) {
        let mut s = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if s.result_tabs.clear_current_support_section() {
            s.set_status_message("Cleared current result view");
        } else {
            s.set_status_message("Nothing to clear");
        }
        s.refresh_result_edit_controls();
        app::redraw();
    }

    fn start_status_animation_timer(state: &Arc<Mutex<AppState>>) {
        let weak_state = Arc::downgrade(state);
        crate::ui::ui_timeout::schedule(STATUS_ANIMATION_INTERVAL, move || {
            let Some(state_for_tick) = weak_state.upgrade() else {
                return;
            };
            let should_reschedule = match state_for_tick.try_lock() {
                Ok(mut s) => {
                    s.tick_status_animation();
                    s.status_animation_running
                }
                Err(_) => true,
            };
            if should_reschedule {
                MainWindow::start_status_animation_timer(&state_for_tick);
            }
        });
    }

    fn transition_to_disconnected_state(
        state: &mut AppState,
        error_message: Option<&str>,
    ) -> Vec<u64> {
        let lazy_fetch_sessions = state.lazy_fetch_sessions_for_abort();
        *state
            .connection_info
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        state.has_live_connection = false;
        state.pending_connection_metadata_refresh = false;
        clear_mutex_flag(&state.schema_refresh_in_progress);
        // Active lazy fetch cancellation is dispatched by the caller after the
        // AppState lock is released; only drop idle/reusable leases here.
        state.release_all_pooled_db_sessions();

        for session_id in &lazy_fetch_sessions {
            state.mark_lazy_fetch_result_tab_cancelled(*session_id);
        }
        state.mark_canceling_progress_contexts_cancelled();

        // Disconnection can happen mid-stream (network drop,
        // explicit disconnect while a worker is still unwinding). Ensure every
        // result grid exits streaming mode immediately so edit controls are not
        // left disabled waiting for a BatchFinished event that may never arrive.
        state.result_tabs.clear_all_lazy_fetch_state_for_abort();
        state.result_tabs.finish_all_streaming();
        state.progress_contexts.clear();
        state.pending_lazy_fetch_canceling_sessions.clear();

        let recovered_save_states = state.result_tabs.clear_orphaned_save_requests();
        let recovered_edit_states = state.result_tabs.clear_orphaned_query_edit_backups();
        if recovered_save_states > 0 {
            state.set_status_message("Disconnected (save interrupted; staged edits preserved)");
        } else if recovered_edit_states > 0 {
            state.set_status_message("Disconnected (staged result-grid edits restored)");
        } else {
            state.set_status_message("Disconnected");
        }
        Self::update_schema_snapshot(state, IntellisenseData::new(), HighlightData::new());

        // Clear object browser cache and tree so stale metadata from the previous
        // connection is not visible when connecting to a different database.
        state.object_browser.clear_on_disconnect();

        // DO NOT clear result_tabs on disconnect.
        //
        // Users frequently disconnect and reconnect (e.g. session timeout, switching
        // environments) and still need to read the query results that were already
        // fetched. Clearing tabs here would destroy that data silently.
        //
        // Staged edit data (pending INSERT/UPDATE/DELETE rows) must also survive
        // across a disconnect so the user can reconnect and retry the save without
        // losing their edits.
        //
        // If you are tempted to add result_tabs.clear() here — don't.
        // Let the user close individual tabs manually when they are done with them.

        // Reset session state (bind variables, settings, etc.) so they do not
        // leak into a subsequent connection, e.g. when disconnected by the health
        // disconnect path rather than via an explicit "Disconnect" menu action.
        if let Ok(conn_guard) = state.connection.try_lock() {
            let session = conn_guard.session_state();
            // Drop the connection guard before locking the session to preserve
            // the single-lock-at-a-time invariant.
            drop(conn_guard);
            let lock_result = session.lock();
            match lock_result {
                Ok(mut guard) => guard.reset(),
                Err(poisoned) => {
                    poisoned.into_inner().reset();
                }
            }
        }

        if let Some(message) = error_message {
            crate::utils::logging::log_error("connection", message);
            state
                .result_tabs
                .append_message_lines(ResultMessageKind::Error, &[message.to_string()]);
            state.result_tabs.select_messages_errors();
        }

        state.refresh_connection_dependent_controls();
        // Refresh the result-grid edit toolbar after orphan recovery may have
        // changed pending_save_request, ensuring buttons reflect the final state
        // rather than any intermediate snapshot from before orphan cleanup.
        state.refresh_result_edit_controls();
        lazy_fetch_sessions
    }

    fn cancel_all_running_queries(state: &Arc<Mutex<AppState>>) {
        let (running_editors, lazy_fetch_sessions) = {
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut running_editors = s
                .editor_tabs
                .iter()
                .filter(|tab| tab.sql_editor.is_query_running())
                .map(|tab| (tab.tab_id, tab.sql_editor.clone()))
                .collect::<Vec<_>>();
            if s.find_tab_index(s.active_editor_tab_id).is_none() && s.sql_editor.is_query_running()
            {
                running_editors.push((s.active_editor_tab_id, s.sql_editor.clone()));
            }
            (running_editors, s.lazy_fetch_sessions_for_abort())
        };

        let lazy_fetch_requests = lazy_fetch_sessions
            .iter()
            .map(|session_id| {
                let requested = AppState::request_lazy_fetch_on_editors(
                    state,
                    *session_id,
                    crate::ui::sql_editor::LazyFetchRequest::Cancel,
                );
                (*session_id, requested)
            })
            .collect::<Vec<_>>();

        if running_editors.is_empty() && lazy_fetch_requests.is_empty() {
            return;
        }

        for (_, editor) in &running_editors {
            editor.cancel_current();
        }

        let mut s = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !running_editors.is_empty() {
            for (tab_id, _) in &running_editors {
                s.mark_progress_context_canceling(*tab_id);
            }
        }
        if !lazy_fetch_requests.is_empty() {
            for (session_id, requested) in &lazy_fetch_requests {
                if *requested {
                    s.mark_lazy_fetch_canceling(*session_id);
                } else {
                    s.mark_lazy_fetch_cancelled_without_status(*session_id);
                }
            }
            s.refresh_result_edit_controls();
        }
        let status = if lazy_fetch_requests.is_empty() {
            format!("{} running queries...", ResultTabStatus::Canceling.label())
        } else {
            format!(
                "{} running queries and fetches...",
                ResultTabStatus::Canceling.label()
            )
        };
        s.set_status_message(&status);
    }

    fn cancel_active_query_editor_tab(state: &Arc<Mutex<AppState>>) -> bool {
        let active_tab_id = {
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            s.active_editor_tab_id
        };
        Self::cancel_query_editor_tab(state, active_tab_id)
    }

    fn cancel_query_editor_tab(state: &Arc<Mutex<AppState>>, tab_id: QueryTabId) -> bool {
        let Some((editor, query_running, lazy_fetch_session)) = ({
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            s.find_tab_index(tab_id).map(|index| {
                let editor = s.editor_tabs[index].sql_editor.clone();
                (
                    editor.clone(),
                    editor.is_query_running(),
                    editor.active_lazy_fetch_session(),
                )
            })
        }) else {
            return false;
        };

        let mut requested = false;
        if let Some(session_id) = lazy_fetch_session {
            requested |= editor
                .request_lazy_fetch(session_id, crate::ui::sql_editor::LazyFetchRequest::Cancel);
            let mut s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if requested {
                s.mark_lazy_fetch_canceling(session_id);
            } else {
                s.mark_lazy_fetch_cancelled_without_status(session_id);
            }
            s.refresh_result_edit_controls();
        }

        if query_running {
            editor.cancel_current();
            let mut s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            s.mark_progress_context_canceling(tab_id);
            s.set_status_message(&format!(
                "{} running query...",
                ResultTabStatus::Canceling.label()
            ));
            s.refresh_result_edit_controls();
            requested = true;
        }

        requested
    }

    fn focus_existing_tab_with_same_file_path(state: &mut AppState, path: &Path) -> bool {
        let Some(file_name) = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
        else {
            return false;
        };
        let Some(tab_id) = state.find_tab_id_by_file_path(path) else {
            return false;
        };
        if !state.activate_editor_tab(tab_id) {
            return false;
        }
        state.set_status_message(&format!(
            "{} is already open. Switched to existing tab",
            file_name
        ));
        true
    }

    fn save_tab(
        state: &Arc<Mutex<AppState>>,
        tab_id: QueryTabId,
        force_save_as: bool,
    ) -> SaveTabOutcome {
        let (current_file, sql_text) = {
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(sql_text) = s.tab_sql_text(tab_id) else {
                return SaveTabOutcome::Cancelled;
            };
            (s.tab_file_path(tab_id), sql_text)
        };

        let should_record_recent = force_save_as || current_file.is_none();
        let target_path = if force_save_as { None } else { current_file }.or_else(|| {
            let mut dialog = FileDialog::new(FileDialogType::BrowseSaveFile);
            // The native chooser auto-appends an "All Files" entry, so listing
            // it here would show it twice. It lands right after our filters, at
            // index 1 (0 = "SQL Files", 1 = auto "All Files" → skip).
            dialog.set_filter("SQL Files\t*.sql");
            dialog.show();
            let filename = dialog.filename();
            if filename.as_os_str().is_empty() {
                None
            } else {
                Some(Self::apply_default_extension(
                    filename,
                    "sql",
                    dialog.filter_value(),
                    Some(1),
                ))
            }
        });

        let Some(path) = target_path else {
            return SaveTabOutcome::Cancelled;
        };

        if let Err(err) = fs::write(&path, &sql_text) {
            return SaveTabOutcome::Failed(err.to_string());
        }

        let mut s = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        s.set_tab_file_path(tab_id, Some(path.clone()));
        s.set_tab_pristine_text(tab_id, sql_text);
        if should_record_recent {
            MainWindow::record_recent_sql_file(&mut s, &path);
        }
        let file_label = path.file_name().unwrap_or_default().to_string_lossy();
        s.set_status_message(&format!("Saved {}", file_label));
        SaveTabOutcome::Saved
    }

    fn confirm_save_if_dirty(
        state: &Arc<Mutex<AppState>>,
        tab_id: QueryTabId,
        action_verb: &str,
    ) -> bool {
        let (is_dirty, tab_label) = {
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (s.is_tab_dirty(tab_id), s.tab_display_name(tab_id))
        };
        if !is_dirty {
            return true;
        }

        let tab_label = tab_label.unwrap_or_else(|| "Query".to_string());
        let choice = crate::ui::choice2_on_main(
            &format!(
                "Tab '{}' has unsaved changes.\nDo you want to save before {}?",
                tab_label, action_verb
            ),
            "Cancel",
            "Save",
            "Don't Save",
        );

        match choice {
            Some(1) => match Self::save_tab(state, tab_id, false) {
                SaveTabOutcome::Saved => true,
                SaveTabOutcome::Cancelled => false,
                SaveTabOutcome::Failed(err) => {
                    crate::ui::alert_on_main(&format!("Failed to save SQL file: {}", err));
                    false
                }
            },
            Some(2) => true,
            _ => false,
        }
    }

    fn confirm_cancel_running_query_for_close(
        state: &Arc<Mutex<AppState>>,
        tab_id: QueryTabId,
    ) -> bool {
        let tab_label = {
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            s.tab_display_name(tab_id)
                .unwrap_or_else(|| "Query".to_string())
        };
        matches!(
            crate::ui::choice2_on_main(
                &format!(
                    "Tab '{}' has a running query or open lazy fetch.\nCancel it and close the tab?",
                    tab_label
                ),
                "Keep Open",
                "Cancel and Close",
                "",
            ),
            Some(1)
        )
    }

    fn confirm_cancel_running_query_for_exit(state: &Arc<Mutex<AppState>>) -> bool {
        let has_running_work = {
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            s.has_running_query_or_lazy_fetch()
        };
        if !has_running_work {
            return true;
        }

        matches!(
            crate::ui::choice2_on_main(
                "A query is running or a lazy fetch is open.\nCancel it and exit?",
                "Keep Open",
                "Cancel and Exit",
                "",
            ),
            Some(1)
        )
    }

    fn resolve_pooled_session_before_action(
        state: &Arc<Mutex<AppState>>,
        tab_id: QueryTabId,
        action: RetainedSessionPreflightAction,
        action_prompt: &str,
        resolution_context: &str,
        commit_button: &str,
        rollback_button: &str,
    ) -> bool {
        let Some((tab_label, editor, snapshot)) = ({
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            s.find_tab_index(tab_id).and_then(|index| {
                let editor = s.editor_tabs[index].sql_editor.clone();
                let snapshot = editor.pooled_session_activity_snapshot()?;
                Some((
                    s.tab_display_name(tab_id)
                        .unwrap_or_else(|| "Query".to_string()),
                    editor,
                    snapshot,
                ))
            })
        }) else {
            return true;
        };

        if crate::db::retained_session_state_preflight_decision(action, snapshot.retained_state)
            != RetainedSessionPreflightDecision::RequireResolution
        {
            return true;
        }

        let retained_state = snapshot.retained_state();
        let transaction_action_allowed = crate::db::retained_session_resolution_action_allowed(
            retained_state,
            RetainedSessionResolutionAction::Commit,
        );
        let result = if transaction_action_allowed {
            let choice = crate::ui::choice2_on_main(
                &format!(
                    "Tab '{}' has a DB session that may need commit, rollback, or discard.\nChoose how to {}.",
                    tab_label, action_prompt
                ),
                "Cancel",
                "Commit/Rollback",
                "Discard Session",
            );
            match choice {
                Some(1) => {
                    let decision = crate::ui::choice2_on_main(
                        &format!(
                            "Choose how to resolve the DB session before {}.",
                            resolution_context
                        ),
                        "Cancel",
                        commit_button,
                        rollback_button,
                    );
                    match decision {
                        Some(1) => editor.commit_pooled_session_for_close(),
                        Some(2) => editor.rollback_pooled_session_for_close(),
                        _ => return false,
                    }
                }
                Some(2) => editor.discard_pooled_session_for_close(),
                _ => return false,
            }
        } else {
            let choice = crate::ui::choice2_on_main(
                &format!(
                    "Tab '{}' has a {} DB session that commit/rollback cannot resolve.\nDiscard it to {}.",
                    tab_label,
                    retained_state.label(),
                    action_prompt
                ),
                "Cancel",
                "Discard Session",
                "",
            );
            match choice {
                Some(1) => editor.discard_pooled_session_for_close(),
                _ => return false,
            }
        };

        if let Err(err) = result {
            crate::ui::alert_on_main(&format!("Failed to resolve DB session: {}", err));
            return false;
        }

        true
    }

    fn resolve_pooled_session_before_close(
        state: &Arc<Mutex<AppState>>,
        tab_id: QueryTabId,
    ) -> bool {
        Self::resolve_pooled_session_before_action(
            state,
            tab_id,
            RetainedSessionPreflightAction::Close,
            "close it",
            "closing",
            "Commit and Close",
            "Rollback and Close",
        )
    }

    fn confirm_save_for_all_dirty_tabs(state: &Arc<Mutex<AppState>>) -> bool {
        let tab_ids = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .query_tabs
            .tab_ids();
        for tab_id in tab_ids {
            if !Self::confirm_save_if_dirty(state, tab_id, "exiting") {
                return false;
            }
        }
        true
    }

    fn resolve_pooled_sessions_before_exit(state: &Arc<Mutex<AppState>>) -> bool {
        Self::resolve_pooled_sessions_before_retained_action(
            state,
            RetainedSessionPreflightAction::Close,
            "close it",
            "closing",
            "Commit and Close",
            "Rollback and Close",
        )
    }

    fn resolve_pooled_sessions_before_retained_action(
        state: &Arc<Mutex<AppState>>,
        action: RetainedSessionPreflightAction,
        action_prompt: &str,
        resolution_context: &str,
        commit_button: &str,
        rollback_button: &str,
    ) -> bool {
        let tab_ids = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .query_tabs
            .tab_ids();
        for tab_id in tab_ids {
            if !Self::resolve_pooled_session_before_action(
                state,
                tab_id,
                action,
                action_prompt,
                resolution_context,
                commit_button,
                rollback_button,
            ) {
                return false;
            }
        }
        true
    }

    fn resolve_pooled_sessions_before_connection_transition(state: &Arc<Mutex<AppState>>) -> bool {
        Self::resolve_pooled_sessions_before_retained_action(
            state,
            RetainedSessionPreflightAction::ConnectionTransition,
            "change the connection",
            "changing the connection",
            "Commit and Continue",
            "Rollback and Continue",
        )
    }

    fn resolve_pooled_sessions_before_pool_resize(state: &Arc<Mutex<AppState>>) -> bool {
        Self::resolve_pooled_sessions_before_retained_action(
            state,
            RetainedSessionPreflightAction::PoolResize,
            "change connection pool size",
            "changing connection pool size",
            "Commit and Continue",
            "Rollback and Continue",
        )
    }

    pub fn new() -> Self {
        Self::new_with_config(AppConfig::load())
    }

    pub fn new_with_config(config: AppConfig) -> Self {
        let connection = create_shared_connection();
        {
            let mut guard = crate::db::lock_connection(&connection);
            guard.set_connection_pool_size(config.normalized_connection_pool_size());
        }

        let current_group = fltk::group::Group::try_current();

        fltk::group::Group::set_current(None::<&fltk::group::Group>);

        let mut window = Window::default()
            .with_size(1200, 800)
            .with_label(&AppState::app_window_title())
            .center_screen();
        window.set_id("main_window");
        window.set_color(theme::window_bg());
        app_icon::apply_window_icon(&mut window);

        let mut main_flex = Flex::default_fill();
        main_flex.set_type(FlexType::Column);

        let menu_bar = MenuBarBuilder::build_with_recent_sql_files(&config.recent_sql_files);
        main_flex.fixed(&menu_bar, MENU_BAR_HEIGHT);

        let mut query_toolbar = Flex::default();
        query_toolbar.set_type(FlexType::Row);
        query_toolbar.set_margin(TOOLBAR_SPACING);
        query_toolbar.set_spacing(TOOLBAR_SPACING);

        let mut execute_btn = Button::default()
            .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
            .with_label("@> Execute");
        execute_btn.set_color(theme::button_primary());
        execute_btn.set_label_color(theme::text_primary());
        execute_btn.set_frame(FrameType::RFlatBox);
        query_toolbar.fixed(&execute_btn, BUTTON_WIDTH);

        let mut cancel_btn = Button::default()
            .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
            .with_label("Cancel");
        cancel_btn.set_color(theme::button_cancel());
        cancel_btn.set_label_color(theme::text_primary());
        cancel_btn.set_frame(FrameType::RFlatBox);
        query_toolbar.fixed(&cancel_btn, BUTTON_WIDTH);

        let mut explain_btn = Button::default()
            .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
            .with_label("Explain");
        explain_btn.set_color(theme::button_secondary());
        explain_btn.set_label_color(theme::text_primary());
        explain_btn.set_frame(FrameType::RFlatBox);
        query_toolbar.fixed(&explain_btn, BUTTON_WIDTH);

        let mut commit_btn = Button::default()
            .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
            .with_label("Commit");
        commit_btn.set_color(theme::button_success());
        commit_btn.set_label_color(theme::text_primary());
        commit_btn.set_frame(FrameType::RFlatBox);
        query_toolbar.fixed(&commit_btn, BUTTON_WIDTH);

        let mut rollback_btn = Button::default()
            .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
            .with_label("Rollback");
        rollback_btn.set_color(theme::button_danger());
        rollback_btn.set_label_color(theme::text_primary());
        rollback_btn.set_frame(FrameType::RFlatBox);
        query_toolbar.fixed(&rollback_btn, BUTTON_WIDTH);

        let initial_db_type = DatabaseType::default();
        let mut transaction_isolation_choice =
            Choice::default().with_size(TRANSACTION_ISOLATION_CHOICE_WIDTH, BUTTON_HEIGHT);
        transaction_isolation_choice.add_choice(&transaction_isolation_choice_labels(
            initial_db_type,
            TransactionIsolation::Default,
        ));
        transaction_isolation_choice.set_value(0);
        transaction_isolation_choice.set_color(theme::input_bg());
        transaction_isolation_choice.set_text_color(theme::text_primary());
        transaction_isolation_choice.set_tooltip("Transaction isolation for new executions");
        query_toolbar.fixed(
            &transaction_isolation_choice,
            TRANSACTION_ISOLATION_CHOICE_WIDTH,
        );

        let mut transaction_access_choice =
            Choice::default().with_size(TRANSACTION_ACCESS_CHOICE_WIDTH, BUTTON_HEIGHT);
        transaction_access_choice.add_choice("Read write|Read only");
        transaction_access_choice.set_value(0);
        transaction_access_choice.set_color(theme::input_bg());
        transaction_access_choice.set_text_color(theme::text_primary());
        transaction_access_choice.set_tooltip("Transaction access mode for new executions");
        query_toolbar.fixed(&transaction_access_choice, TRANSACTION_ACCESS_CHOICE_WIDTH);

        let toolbar_spacer = Frame::default();
        query_toolbar.resizable(&toolbar_spacer);

        let mut timeout_label = Frame::default().with_size(85, BUTTON_HEIGHT);
        timeout_label.set_label("Timeout(s)");
        timeout_label.set_label_color(theme::text_muted());
        query_toolbar.fixed(&timeout_label, 85);

        let mut timeout_input = IntInput::default().with_size(NUMERIC_INPUT_WIDTH, BUTTON_HEIGHT);
        timeout_input.set_color(theme::input_bg());
        timeout_input.set_text_color(theme::text_primary());
        timeout_input.set_tooltip("Call timeout in seconds (empty = no timeout)");
        timeout_input.set_value("60");
        query_toolbar.fixed(&timeout_input, NUMERIC_INPUT_WIDTH);

        query_toolbar.end();
        main_flex.fixed(&query_toolbar, RESULT_TOOLBAR_HEIGHT);

        let mut content_flex = Flex::default();
        content_flex.set_type(FlexType::Row);
        content_flex.set_spacing(0);

        let object_browser = ObjectBrowserWidget::new(0, 0, 250, 600, connection.clone());
        let obj_browser_widget = object_browser.get_widget();
        content_flex.fixed(&obj_browser_widget, 250);

        let splitter_width = MAIN_SPLITTER_WIDTH;
        let mut split_bar = Frame::default().with_size(splitter_width, 0);
        split_bar.set_frame(FrameType::FlatBox);
        split_bar.set_color(theme::border());
        split_bar.set_tooltip("Drag to resize panels");

        let drag_state = Arc::new(Mutex::new(None::<(i32, i32)>));
        let mut content_flex_for_split = content_flex.clone();
        let obj_browser_for_split = obj_browser_widget.clone();
        let drag_state_for_split = drag_state;
        split_bar.handle(move |_bar, ev| match ev {
            fltk::enums::Event::Enter | fltk::enums::Event::Move => {
                set_cursor(Cursor::WE);
                true
            }
            fltk::enums::Event::Push => {
                *drag_state_for_split
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                    Some((app::event_x(), obj_browser_for_split.w()));
                true
            }
            fltk::enums::Event::Drag => {
                if let Some((start_x, start_w)) = *drag_state_for_split
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                {
                    let delta = app::event_x() - start_x;
                    let min_left = 180;
                    let min_right = 320;
                    let max_left =
                        (content_flex_for_split.w() - splitter_width - min_right).max(min_left);
                    let mut new_width = start_w + delta;
                    if new_width < min_left {
                        new_width = min_left;
                    } else if new_width > max_left {
                        new_width = max_left;
                    }
                    content_flex_for_split.fixed(&obj_browser_for_split, new_width);
                    content_flex_for_split.layout();
                    app::redraw();
                }
                true
            }
            fltk::enums::Event::Released => {
                *drag_state_for_split
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
                set_cursor(Cursor::WE);
                true
            }
            fltk::enums::Event::Leave => {
                set_cursor(Cursor::Default);
                true
            }
            _ => false,
        });
        content_flex.fixed(&split_bar, splitter_width);

        let mut right_flex = Flex::default();
        right_flex.set_type(FlexType::Column);

        let query_split_ratio: Arc<Mutex<Option<f64>>> = Arc::new(Mutex::new(None));
        let mut right_tile = Tile::new(0, 0, 900, 600, None);
        right_tile.set_frame(FrameType::FlatBox);
        right_tile.set_color(theme::panel_bg());
        let tile_x = right_tile.x();
        let tile_y = right_tile.y();
        let tile_w = right_tile.w().max(1);
        let tile_h = right_tile.h().max(1);
        let max_initial_query_height =
            (tile_h - MIN_RESULTS_HEIGHT - QUERY_SPLIT_BAR_HEIGHT).max(MIN_QUERY_HEIGHT);
        let initial_query_height = 250.clamp(MIN_QUERY_HEIGHT, max_initial_query_height);

        right_tile.begin();
        let mut query_top_group = Group::new(tile_x, tile_y, tile_w, initial_query_height, None);
        query_top_group.set_frame(FrameType::FlatBox);
        query_top_group.set_color(theme::panel_bg());
        query_top_group.begin();
        let mut query_top_flex = Flex::new(tile_x, tile_y, tile_w, initial_query_height, None);
        query_top_flex.set_type(FlexType::Column);

        let mut query_tabs = QueryTabsWidget::new(0, 0, 900, 400);
        let query_tabs_widget = query_tabs.get_widget();
        query_top_flex.add(&query_tabs_widget);
        query_top_flex.resizable(&query_tabs_widget);

        let mut query_tab_toolbar = Flex::default();
        query_tab_toolbar.set_type(FlexType::Row);
        query_tab_toolbar.set_margin(TOOLBAR_SPACING);
        query_tab_toolbar.set_spacing(TOOLBAR_SPACING);

        let mut query_close_tab_btn = Button::default()
            .with_size(BUTTON_WIDTH_LARGE, BUTTON_HEIGHT)
            .with_label("Close Current");
        query_close_tab_btn.set_color(theme::button_subtle());
        query_close_tab_btn.set_label_color(theme::text_secondary());
        query_close_tab_btn.set_frame(FrameType::RFlatBox);
        query_close_tab_btn.set_tooltip("Close the current query tab (Cmd/Ctrl+W)");
        query_tab_toolbar.fixed(&query_close_tab_btn, BUTTON_WIDTH_LARGE);

        let close_all_queries_width = BUTTON_WIDTH_LARGE;
        let mut query_close_all_tabs_btn = Button::default()
            .with_size(close_all_queries_width, BUTTON_HEIGHT)
            .with_label("Close All");
        query_close_all_tabs_btn.set_color(theme::button_subtle());
        query_close_all_tabs_btn.set_label_color(theme::text_secondary());
        query_close_all_tabs_btn.set_frame(FrameType::RFlatBox);
        query_close_all_tabs_btn.set_tooltip("Close all query tabs");
        query_tab_toolbar.fixed(&query_close_all_tabs_btn, close_all_queries_width);

        let query_tab_toolbar_spacer = Frame::default();
        query_tab_toolbar.resizable(&query_tab_toolbar_spacer);
        query_tab_toolbar.end();
        query_top_flex.fixed(&query_tab_toolbar, RESULT_TOOLBAR_HEIGHT);
        query_top_flex.end();
        query_top_group.resizable(&query_top_flex);
        query_top_group.end();

        let result_y = tile_y + initial_query_height + QUERY_SPLIT_BAR_HEIGHT;
        let result_h = (tile_h - initial_query_height - QUERY_SPLIT_BAR_HEIGHT).max(1);
        let mut result_bottom_group = Group::new(tile_x, result_y, tile_w, result_h, None);
        result_bottom_group.set_frame(FrameType::FlatBox);
        result_bottom_group.set_color(theme::panel_bg());
        result_bottom_group.begin();

        let mut result_bottom_flex = Flex::new(tile_x, result_y, tile_w, result_h, None);
        result_bottom_flex.set_type(FlexType::Column);

        let mut result_tabs = ResultTabsWidget::new(0, 0, 900, 400);
        let result_widget = result_tabs.get_widget();
        result_bottom_flex.add(&result_widget);
        result_bottom_flex.resizable(&result_widget);

        let mut result_toolbar = Flex::default();
        result_toolbar.set_type(FlexType::Row);
        result_toolbar.set_margin(TOOLBAR_SPACING);
        result_toolbar.set_spacing(TOOLBAR_SPACING);

        let mut clear_current_btn = Button::default()
            .with_size(BUTTON_WIDTH_LARGE + 10, BUTTON_HEIGHT)
            .with_label("Clear Current");
        clear_current_btn.set_color(theme::button_subtle());
        clear_current_btn.set_label_color(theme::text_secondary());
        clear_current_btn.set_frame(FrameType::RFlatBox);
        clear_current_btn
            .set_tooltip("Clear the current result grid, output, message, or plan view");
        result_toolbar.fixed(&clear_current_btn, BUTTON_WIDTH_LARGE + 10);

        let mut clear_all_btn = Button::default()
            .with_size(BUTTON_WIDTH_LARGE, BUTTON_HEIGHT)
            .with_label("Clear All");
        clear_all_btn.set_color(theme::button_subtle());
        clear_all_btn.set_label_color(theme::text_secondary());
        clear_all_btn.set_frame(FrameType::RFlatBox);
        clear_all_btn.set_tooltip("Clear all result grids, output, messages, and plans");
        result_toolbar.fixed(&clear_all_btn, BUTTON_WIDTH_LARGE);

        let spacer = Frame::default();
        result_toolbar.resizable(&spacer);

        let mut one_tab_per_query_check = CheckButton::default()
            .with_size(BUTTON_WIDTH_LARGE + 45, BUTTON_HEIGHT)
            .with_label(RESULT_ONE_TAB_PER_QUERY_LABEL);
        one_tab_per_query_check.set_tooltip(
            "Unchecked: clear existing result tabs before each execution. Checked: append result tabs.",
        );
        result_toolbar.fixed(
            &one_tab_per_query_check,
            result_toolbar_checkbox_width(&one_tab_per_query_check, BUTTON_WIDTH_LARGE + 45),
        );

        let mut one_tab_edit_gap = Frame::default();
        one_tab_edit_gap.hide();
        result_toolbar.fixed(&one_tab_edit_gap, 0);

        let mut edit_mode_check = CheckButton::default()
            .with_size(BUTTON_WIDTH_SMALL, BUTTON_HEIGHT)
            .with_label(" Edit");
        edit_mode_check.set_tooltip("Enable staged edit mode for the current result tab");
        edit_mode_check.hide();
        result_toolbar.fixed(&edit_mode_check, 0);

        let mut edit_insert_btn = Button::default()
            .with_size(BUTTON_WIDTH_SMALL, BUTTON_HEIGHT)
            .with_label("Insert");
        edit_insert_btn.set_color(theme::button_secondary());
        edit_insert_btn.set_label_color(theme::text_primary());
        edit_insert_btn.set_frame(FrameType::RFlatBox);
        edit_insert_btn.set_tooltip("Add a staged row (DB is not changed until Save)");
        result_toolbar.fixed(&edit_insert_btn, BUTTON_WIDTH_SMALL);

        let mut edit_delete_btn = Button::default()
            .with_size(BUTTON_WIDTH_SMALL, BUTTON_HEIGHT)
            .with_label("Delete");
        edit_delete_btn.set_color(theme::button_danger());
        edit_delete_btn.set_label_color(theme::text_primary());
        edit_delete_btn.set_frame(FrameType::RFlatBox);
        edit_delete_btn.set_tooltip("Delete selected row(s) in staged edit mode");
        result_toolbar.fixed(&edit_delete_btn, BUTTON_WIDTH_SMALL);

        let mut edit_save_btn = Button::default()
            .with_size(BUTTON_WIDTH_SMALL, BUTTON_HEIGHT)
            .with_label("Save");
        edit_save_btn.set_color(theme::button_success());
        edit_save_btn.set_label_color(theme::text_primary());
        edit_save_btn.set_frame(FrameType::RFlatBox);
        edit_save_btn.set_tooltip("Apply staged edits to DB");
        result_toolbar.fixed(&edit_save_btn, BUTTON_WIDTH_SMALL);

        let mut edit_cancel_btn = Button::default()
            .with_size(BUTTON_WIDTH_SMALL, BUTTON_HEIGHT)
            .with_label("Cancel");
        edit_cancel_btn.set_color(theme::button_cancel());
        edit_cancel_btn.set_label_color(theme::text_primary());
        edit_cancel_btn.set_frame(FrameType::RFlatBox);
        edit_cancel_btn.set_tooltip("Discard staged edits and restore rows");
        edit_insert_btn.hide();
        edit_delete_btn.hide();
        edit_save_btn.hide();
        edit_cancel_btn.hide();
        result_toolbar.fixed(&edit_insert_btn, 0);
        result_toolbar.fixed(&edit_delete_btn, 0);
        result_toolbar.fixed(&edit_save_btn, 0);
        result_toolbar.fixed(&edit_cancel_btn, 0);
        result_toolbar.end();
        result_bottom_flex.fixed(&result_toolbar, RESULT_TOOLBAR_HEIGHT);
        result_bottom_flex.end();
        result_bottom_group.resizable(&result_bottom_flex);

        result_bottom_group.end();

        let mut query_split_bar = Frame::default().with_size(tile_w, QUERY_SPLIT_BAR_HEIGHT);
        query_split_bar.set_frame(FrameType::FlatBox);
        query_split_bar.set_color(theme::border());
        query_split_bar.set_tooltip("Drag to resize query and result panes");
        query_split_bar.resize(
            tile_x,
            tile_y + initial_query_height,
            tile_w,
            QUERY_SPLIT_BAR_HEIGHT,
        );

        right_tile.end();

        let query_split_ratio_for_tile = query_split_ratio.clone();
        let mut query_top_group_for_tile = query_top_group.clone();
        let mut query_split_bar_for_tile = query_split_bar.clone();
        let split_drag_active = Arc::new(Mutex::new(false));
        let split_drag_active_for_tile = split_drag_active;
        right_tile.handle(move |tile, ev| {
            const SPLIT_GRAB_MARGIN: i32 = 6;
            match ev {
                fltk::enums::Event::Push => {
                    // Avoid event_mouse_button() because FLTK can emit non-standard button
                    // values on some devices, which panics when cast to MouseButton.
                    if app::event_button() == fltk::app::MouseButton::Left as i32 {
                        let split_top = query_split_bar_for_tile.y();
                        let split_bottom = split_top + query_split_bar_for_tile.h();
                        let near_split = (app::event_y() >= split_top - SPLIT_GRAB_MARGIN)
                            && (app::event_y() <= split_bottom + SPLIT_GRAB_MARGIN);
                        if near_split {
                            *split_drag_active_for_tile
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
                            return true;
                        }
                    }
                    false
                }
                fltk::enums::Event::Drag => {
                    if *split_drag_active_for_tile
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                    {
                        let right_height = tile.h();
                        if right_height > 0 {
                            let max_query_height =
                                (right_height - MIN_RESULTS_HEIGHT - QUERY_SPLIT_BAR_HEIGHT)
                                    .max(MIN_QUERY_HEIGHT);
                            let split_pos = app::event_y() - tile.y();
                            let desired_query_height =
                                split_pos.clamp(MIN_QUERY_HEIGHT, max_query_height);
                            query_top_group_for_tile.resize(
                                tile.x(),
                                tile.y(),
                                tile.w(),
                                desired_query_height,
                            );
                            MainWindow::clamp_query_split_with(
                                tile,
                                &mut query_top_group_for_tile,
                                &mut query_split_bar_for_tile,
                            );
                            // Store the ratio for proportional resize.
                            *query_split_ratio_for_tile
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                Some(safe_div(desired_query_height as f64, right_height as f64));
                        }
                        return true;
                    }
                    false
                }
                fltk::enums::Event::Released => {
                    if std::mem::replace(
                        &mut *split_drag_active_for_tile
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()),
                        false,
                    ) {
                        MainWindow::clamp_query_split_with(
                            tile,
                            &mut query_top_group_for_tile,
                            &mut query_split_bar_for_tile,
                        );
                        // Store final ratio after release.
                        let right_height = tile.h();
                        if right_height > 0 {
                            let query_height = query_top_group_for_tile.h();
                            *query_split_ratio_for_tile
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                Some(safe_div(query_height as f64, right_height as f64));
                        }
                        return true;
                    }
                    false
                }
                fltk::enums::Event::Resize => {
                    // Apply the saved split ratio immediately inside the Tile's
                    // own resize handling so the layout is already correct before
                    // the next draw.  This avoids the visible flicker that occurs
                    // when the adjustment is deferred to the window-level handler.
                    let ratio = *query_split_ratio_for_tile
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if let Some(r) = ratio {
                        MainWindow::apply_query_split_ratio(
                            tile,
                            &mut query_top_group_for_tile,
                            &mut query_split_bar_for_tile,
                            r,
                        );
                    } else {
                        MainWindow::adjust_query_layout_with(
                            tile,
                            &mut query_top_group_for_tile,
                            &mut query_split_bar_for_tile,
                        );
                    }
                    // Return false so the default Tile resize still runs for
                    // any children we don't manage here.
                    false
                }
                _ => false,
            }
        });

        let mut first_tab_id = query_tabs.add_tab("Query 1");
        let mut first_tab_group = query_tabs.tab_group(first_tab_id);
        if first_tab_group.is_none() {
            eprintln!(
                "Warning: initial query tab group was missing; attempting recovery by creating a new tab."
            );
            let recovered_tab_id = query_tabs.add_tab("Query 1");
            first_tab_group = query_tabs.tab_group(recovered_tab_id);
            if first_tab_group.is_some() {
                first_tab_id = recovered_tab_id;
            }
        }
        let first_tab_group = first_tab_group.unwrap_or_else(|| query_top_group.clone());
        first_tab_group.begin();
        let schema_intellisense_data = Arc::new(Mutex::new(IntellisenseData::new()));
        let first_editor = SqlEditorWidget::new_with_intellisense_data(
            connection.clone(),
            timeout_input.clone(),
            schema_intellisense_data.clone(),
        );
        first_editor.set_owner_tab_id(first_tab_id);
        let mut first_editor_group = first_editor.get_group().clone();
        first_editor_group.resize(
            first_tab_group.x(),
            first_tab_group.y(),
            first_tab_group.w(),
            first_tab_group.h(),
        );
        first_editor_group.layout();
        first_tab_group.resizable(&first_editor_group);
        first_tab_group.end();
        query_tabs.select(first_tab_id);
        let sql_editor = first_editor.clone();
        let sql_buffer = first_editor.get_buffer();
        let editor_tabs = vec![QueryEditorTab {
            tab_id: first_tab_id,
            base_label: "Query 1".to_string(),
            sql_editor: first_editor,
            sql_buffer: sql_buffer.clone(),
            current_file: None,
            pristine_text: String::new(),
            current_text_len: 0,
            is_dirty: false,
        }];

        right_flex.resizable(&right_tile);
        right_flex.end();

        content_flex.resizable(&right_flex);
        content_flex.end();
        main_flex.resizable(&content_flex);

        let mut status_bar = Frame::default().with_label("Not connected");
        status_bar.set_frame(FrameType::FlatBox);
        status_bar.set_color(theme::accent());
        status_bar.set_label_color(theme::text_primary());
        main_flex.fixed(&status_bar, STATUS_BAR_HEIGHT);
        main_flex.end();
        window.end();
        window.make_resizable(true);

        let state = Arc::new(Mutex::new(AppState {
            connection,
            query_tabs: query_tabs.clone(),
            query_top_group: query_top_group.clone(),
            query_split_bar: query_split_bar.clone(),
            editor_tabs,
            active_editor_tab_id: first_tab_id,
            next_editor_tab_number: 2,
            sql_editor,
            sql_buffer,
            schema_intellisense_data,
            schema_highlight_data: HighlightData::new(),
            query_timeout_input: timeout_input.clone(),
            result_tabs: result_tabs.clone(),
            result_toolbar: result_toolbar.clone(),
            result_one_tab_per_query_check: one_tab_per_query_check.clone(),
            result_one_tab_edit_gap: one_tab_edit_gap.clone(),
            result_edit_check: edit_mode_check.clone(),
            result_insert_btn: edit_insert_btn.clone(),
            result_delete_btn: edit_delete_btn.clone(),
            result_save_btn: edit_save_btn.clone(),
            result_cancel_btn: edit_cancel_btn.clone(),
            execute_btn: execute_btn.clone(),
            query_cancel_btn: cancel_btn.clone(),
            explain_btn: explain_btn.clone(),
            commit_btn: commit_btn.clone(),
            rollback_btn: rollback_btn.clone(),
            transaction_isolation_choice: transaction_isolation_choice.clone(),
            transaction_access_choice: transaction_access_choice.clone(),
            result_grid_execution_target: None,
            progress_contexts: HashMap::new(),
            abandoned_query_operations: HashSet::new(),
            pending_query_canceling_tabs: HashSet::new(),
            pending_lazy_fetch_canceling_sessions: HashSet::new(),
            object_browser,
            status_bar,
            current_file: Arc::new(Mutex::new(None)),
            popups: Arc::new(Mutex::new(Vec::new())),
            window,
            right_tile: right_tile.clone(),
            query_split_ratio,
            connection_info: Arc::new(Mutex::new(None)),
            has_live_connection: false,
            pending_connection_metadata_refresh: false,
            config: Arc::new(Mutex::new(config)),
            status_animation_running: false,
            status_animation_message: String::new(),
            status_animation_frame: 0,
            schema_sender: None,
            file_sender: None,
            schema_refresh_in_progress: Arc::new(Mutex::new(None)),
        }));

        {
            let mut s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let weak_state_for_result_tabs_change = Arc::downgrade(&state);
            s.result_tabs.set_on_change(move || {
                if let Some(state_for_result_tabs_change) =
                    weak_state_for_result_tabs_change.upgrade()
                {
                    if let Ok(mut s) = state_for_result_tabs_change.try_lock() {
                        s.refresh_result_edit_controls();
                    }
                }
            });
            s.refresh_result_edit_controls();
            // Set initial button / menu state: not connected at startup.
            s.refresh_connection_dependent_controls();
        }

        let weak_state_for_grid_edit = Arc::downgrade(&state);
        let grid_edit_callback: ResultGridSqlExecuteCallback =
            Arc::new(Mutex::new(Some(Box::new(move |sql: String| {
                let Some(state_for_grid_edit) = weak_state_for_grid_edit.upgrade() else {
                    return Err("Main window is no longer available.".to_string());
                };
                let mut guard = state_for_grid_edit
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if guard.is_any_query_running() {
                    return Err("Another query is already running.".to_string());
                }
                let target_tab = guard
                    .result_tabs
                    .active_result_id()
                    .ok_or_else(|| "Open a result tab first.".to_string())?;
                guard.result_grid_execution_target = Some(target_tab);
                guard.sql_editor.execute_sql_text(&sql);
                if !guard.sql_editor.is_query_running() {
                    guard.result_grid_execution_target = None;
                    return Err("Failed to start query execution for result-grid edit.".to_string());
                }
                Ok(())
            }))));
        {
            state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .result_tabs
                .set_execute_sql_callback(grid_edit_callback);
        }

        let weak_state_for_lazy_fetch = Arc::downgrade(&state);
        let lazy_fetch_callback = Arc::new(Mutex::new(Some(Box::new(move |session_id, request| {
            let Some(state_for_lazy_fetch) = weak_state_for_lazy_fetch.upgrade() else {
                return false;
            };
            AppState::request_lazy_fetch_on_editors(&state_for_lazy_fetch, session_id, request)
        })
            as Box<dyn FnMut(u64, crate::ui::sql_editor::LazyFetchRequest) -> bool>)));
        {
            state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .result_tabs
                .set_lazy_fetch_callback(lazy_fetch_callback);
        }

        let weak_state_for_execute = Arc::downgrade(&state);
        execute_btn.set_callback(move |_| {
            if let Some(state_for_execute) = weak_state_for_execute.upgrade() {
                execute_sql_request_with_session_pool_slot(
                    &state_for_execute,
                    SqlExecutionRequest::StatementAtCursor,
                );
            }
        });

        let weak_state_for_cancel = Arc::downgrade(&state);
        cancel_btn.set_callback(move |_| {
            if let Some(state_for_cancel) = weak_state_for_cancel.upgrade() {
                MainWindow::cancel_active_query_editor_tab(&state_for_cancel);
            }
        });

        let weak_state_for_explain = Arc::downgrade(&state);
        explain_btn.set_callback(move |_| {
            if let Some(state_for_explain) = weak_state_for_explain.upgrade() {
                if let Some(editor) = acquire_sql_editor_if_idle(&state_for_explain) {
                    editor.explain_current();
                }
            }
        });

        let weak_state_for_commit = Arc::downgrade(&state);
        commit_btn.set_callback(move |_| {
            if let Some(state_for_commit) = weak_state_for_commit.upgrade() {
                if let Some(editor) = acquire_sql_editor_if_idle(&state_for_commit) {
                    editor.commit();
                }
            }
        });

        let weak_state_for_rollback = Arc::downgrade(&state);
        rollback_btn.set_callback(move |_| {
            if let Some(state_for_rollback) = weak_state_for_rollback.upgrade() {
                if let Some(editor) = acquire_sql_editor_if_idle(&state_for_rollback) {
                    editor.rollback();
                }
            }
        });

        let weak_state_for_tx_isolation = Arc::downgrade(&state);
        transaction_isolation_choice.set_callback(move |_| {
            if let Some(state_for_tx_isolation) = weak_state_for_tx_isolation.upgrade() {
                update_transaction_mode_from_controls(&state_for_tx_isolation);
            }
        });

        let weak_state_for_tx_access = Arc::downgrade(&state);
        transaction_access_choice.set_callback(move |_| {
            if let Some(state_for_tx_access) = weak_state_for_tx_access.upgrade() {
                update_transaction_mode_from_controls(&state_for_tx_access);
            }
        });

        let weak_state_for_result_clear_current = Arc::downgrade(&state);
        clear_current_btn.set_callback(move |_| {
            let Some(state_for_result_clear_current) =
                weak_state_for_result_clear_current.upgrade()
            else {
                return;
            };
            MainWindow::clear_current_result_view(&state_for_result_clear_current);
        });

        let weak_state_for_result_clear_all = Arc::downgrade(&state);
        clear_all_btn.set_callback(move |_| {
            let Some(state_for_result_clear_all) = weak_state_for_result_clear_all.upgrade() else {
                return;
            };
            MainWindow::clear_all_result_views(&state_for_result_clear_all);
        });

        let weak_state_for_query_close = Arc::downgrade(&state);
        query_close_tab_btn.set_callback(move |_| {
            let Some(state_for_query_close) = weak_state_for_query_close.upgrade() else {
                return;
            };
            let tab_id = state_for_query_close
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .active_editor_tab_id;
            MainWindow::close_query_editor_tab(&state_for_query_close, tab_id);
            app::redraw();
        });

        let weak_state_for_query_close_all = Arc::downgrade(&state);
        query_close_all_tabs_btn.set_callback(move |_| {
            let Some(state_for_query_close_all) = weak_state_for_query_close_all.upgrade() else {
                return;
            };
            MainWindow::close_all_query_editor_tabs(&state_for_query_close_all);
        });

        let weak_state_for_tab_select = Arc::downgrade(&state);
        query_tabs.set_on_select(move |tab_id| {
            if let Some(state_for_tab_select) = weak_state_for_tab_select.upgrade() {
                MainWindow::select_query_editor_tab_or_retry(&state_for_tab_select, tab_id);
            }
        });

        let weak_state_for_tab_close = Arc::downgrade(&state);
        query_tabs.set_on_close(move |tab_id| {
            let Some(state_for_tab_close) = weak_state_for_tab_close.upgrade() else {
                return;
            };
            MainWindow::close_query_editor_tab(&state_for_tab_close, tab_id);
            app::redraw();
        });

        let weak_state_for_result_tab_close = Arc::downgrade(&state);
        result_tabs.set_on_close(move |target| {
            let Some(state_for_result_tab_close) = weak_state_for_result_tab_close.upgrade() else {
                return;
            };
            MainWindow::close_result_tab_by_target(&state_for_result_tab_close, target);
        });

        {
            let mut state_borrow = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self::adjust_query_layout(&mut state_borrow);
            Self::apply_font_settings(&mut state_borrow);
            Self::apply_lazy_fetch_settings(&mut state_borrow);
        }

        let weak_state_for_edit_check = Arc::downgrade(&state);
        edit_mode_check.set_callback(move |check| {
            let Some(state_for_edit_check) = weak_state_for_edit_check.upgrade() else {
                return;
            };
            let enabled = check.value();
            let mut result_tabs =
                match MainWindow::clone_result_tabs_for_edit_action(&state_for_edit_check) {
                    Ok(tabs) => tabs,
                    Err(err) => {
                        crate::ui::alert_on_main(&err);
                        app::redraw();
                        return;
                    }
                };
            let action_result = if enabled {
                result_tabs.begin_current_edit_mode()
            } else if result_tabs.is_current_edit_mode_enabled() {
                result_tabs.cancel_current_edit_mode()
            } else {
                Ok(String::new())
            };

            let mut error_message = None;
            if let Ok(mut s) = state_for_edit_check.try_lock() {
                match action_result {
                    Ok(msg) => {
                        if !msg.is_empty() {
                            s.set_status_message(&msg);
                        }
                    }
                    Err(err) => {
                        error_message = Some(err);
                    }
                }
                s.refresh_result_edit_controls();
            }
            if let Some(err) = error_message {
                crate::ui::alert_on_main(&err);
            }
            app::redraw();
        });

        let weak_state_for_edit_insert = Arc::downgrade(&state);
        edit_insert_btn.set_callback(move |_| {
            let Some(state_for_edit_insert) = weak_state_for_edit_insert.upgrade() else {
                return;
            };
            let mut result_tabs =
                match MainWindow::clone_result_tabs_for_edit_action(&state_for_edit_insert) {
                    Ok(tabs) => tabs,
                    Err(err) => {
                        crate::ui::alert_on_main(&err);
                        app::redraw();
                        return;
                    }
                };
            let action_result = result_tabs.insert_row_in_current_edit_mode();
            let mut error_message = None;
            {
                let mut s = state_for_edit_insert
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                match action_result {
                    Ok(msg) => s.set_status_message(&msg),
                    Err(err) => {
                        error_message = Some(err);
                    }
                }
                s.refresh_result_edit_controls();
            }
            if let Some(err) = error_message {
                crate::ui::alert_on_main(&err);
            }
            app::redraw();
        });

        let weak_state_for_edit_delete = Arc::downgrade(&state);
        edit_delete_btn.set_callback(move |_| {
            let Some(state_for_edit_delete) = weak_state_for_edit_delete.upgrade() else {
                return;
            };
            let mut result_tabs =
                match MainWindow::clone_result_tabs_for_edit_action(&state_for_edit_delete) {
                    Ok(tabs) => tabs,
                    Err(err) => {
                        crate::ui::alert_on_main(&err);
                        app::redraw();
                        return;
                    }
                };
            let action_result = result_tabs.delete_selected_rows_in_current_edit_mode();
            let mut error_message = None;
            {
                let mut s = state_for_edit_delete
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                match action_result {
                    Ok(msg) => s.set_status_message(&msg),
                    Err(err) => {
                        error_message = Some(err);
                    }
                }
                s.refresh_result_edit_controls();
            }
            if let Some(err) = error_message {
                crate::ui::alert_on_main(&err);
            }
            app::redraw();
        });

        let weak_state_for_edit_save = Arc::downgrade(&state);
        edit_save_btn.set_callback(move |_| {
            let Some(state_for_edit_save) = weak_state_for_edit_save.upgrade() else {
                return;
            };
            let mut result_tabs =
                match MainWindow::clone_result_tabs_for_edit_action(&state_for_edit_save) {
                    Ok(tabs) => tabs,
                    Err(err) => {
                        crate::ui::alert_on_main(&err);
                        app::redraw();
                        return;
                    }
                };
            let save_result = result_tabs.save_current_edit_mode();
            let mut error_message = None;
            {
                let mut s = state_for_edit_save
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                match save_result {
                    Ok(msg) => s.set_status_message(&msg),
                    Err(err) => {
                        error_message = Some(err);
                    }
                }
                s.refresh_result_edit_controls();
            }
            if let Some(err) = error_message {
                crate::ui::alert_on_main(&err);
            }
            app::redraw();
        });

        let weak_state_for_edit_cancel = Arc::downgrade(&state);
        edit_cancel_btn.set_callback(move |_| {
            let Some(state_for_edit_cancel) = weak_state_for_edit_cancel.upgrade() else {
                return;
            };
            let mut result_tabs =
                match MainWindow::clone_result_tabs_for_edit_action(&state_for_edit_cancel) {
                    Ok(tabs) => tabs,
                    Err(err) => {
                        crate::ui::alert_on_main(&err);
                        app::redraw();
                        return;
                    }
                };
            let action_result = result_tabs.cancel_current_edit_mode();
            let mut error_message = None;
            {
                let mut s = state_for_edit_cancel
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                match action_result {
                    Ok(msg) => s.set_status_message(&msg),
                    Err(err) => {
                        error_message = Some(err);
                    }
                }
                s.refresh_result_edit_controls();
            }
            if let Some(err) = error_message {
                crate::ui::alert_on_main(&err);
            }
            app::redraw();
        });

        // Restore current group
        if let Some(ref group) = current_group {
            fltk::group::Group::set_current(Some(group));
        }

        Self { state }
    }

    fn open_query_history_dialog(state: &Arc<Mutex<AppState>>) {
        let popups = {
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            s.popups.clone()
        };
        if let Some(sql) = QueryHistoryDialog::show_with_registry(popups) {
            let (created_tab_id, schema_sender, file_sender, created_editor, created_right_tile) = {
                let mut s = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let mut created_tab_id = None;
                let mut created_editor: Option<SqlEditorWidget> = None;
                let mut created_right_tile: Option<Tile> = None;
                if let Some(tab_id) = MainWindow::create_query_editor_tab(&mut s) {
                    s.sql_buffer.set_text(&sql);
                    s.sql_editor.reset_undo_redo_history();
                    s.set_tab_file_path(tab_id, None);
                    s.set_tab_pristine_text(tab_id, sql);
                    created_editor = Some(s.sql_editor.clone());
                    created_right_tile = Some(s.right_tile.clone());
                    created_tab_id = Some(tab_id);
                }
                (
                    created_tab_id,
                    s.schema_sender.clone(),
                    s.file_sender.clone(),
                    created_editor,
                    created_right_tile,
                )
            };

            if let Some(tab_id) = created_tab_id {
                if let Some(schema_sender) = schema_sender {
                    MainWindow::attach_editor_callbacks(state, tab_id, schema_sender);
                }
                if let Some(file_sender) = file_sender {
                    MainWindow::attach_file_drop_callback(state, tab_id, file_sender);
                }
                if let Some(mut editor) = created_editor {
                    editor.focus();
                }
                if let Some(mut right_tile) = created_right_tile {
                    right_tile.redraw();
                }
                app::redraw();
            } else {
                state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .set_status_message("Failed to create a new query tab");
            }
        }
    }

    fn select_query_editor_tab_or_retry(state: &Arc<Mutex<AppState>>, tab_id: QueryTabId) {
        Self::select_query_editor_tab_or_retry_with_attempt(state, tab_id, 0);
    }

    fn select_query_editor_tab_or_retry_with_attempt(
        state: &Arc<Mutex<AppState>>,
        tab_id: QueryTabId,
        attempt: u8,
    ) {
        const TAB_SELECT_RETRY_INTERVAL_SECONDS: f64 = 0.01;
        const MAX_TAB_SELECT_RETRIES: u8 = 10;

        match state.try_lock() {
            Ok(mut s) => {
                if s.set_active_editor_tab(tab_id) {
                    s.sql_editor.focus();
                }
            }
            Err(_) if attempt < MAX_TAB_SELECT_RETRIES => {
                let state_for_retry = Arc::clone(state);
                crate::ui::ui_timeout::schedule(TAB_SELECT_RETRY_INTERVAL_SECONDS, move || {
                    MainWindow::select_query_editor_tab_or_retry_with_attempt(
                        &state_for_retry,
                        tab_id,
                        attempt.saturating_add(1),
                    );
                });
            }
            Err(_) => {
                crate::utils::logging::log_warning(
                    "main_window::query_tabs",
                    "Skipped query tab selection because AppState stayed busy",
                );
            }
        }
    }

    fn adjust_query_layout(state: &mut AppState) {
        let mut right_tile = state.right_tile.clone();
        let mut query_top_group = state.query_top_group.clone();
        let mut query_split_bar = state.query_split_bar.clone();
        let ratio = *state
            .query_split_ratio
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(r) = ratio {
            Self::apply_query_split_ratio(
                &mut right_tile,
                &mut query_top_group,
                &mut query_split_bar,
                r,
            );
        } else {
            Self::adjust_query_layout_with(
                &mut right_tile,
                &mut query_top_group,
                &mut query_split_bar,
            );
        }
    }

    fn apply_font_settings(state: &mut AppState) {
        let (unified_profile, ui_size, editor_size, result_size, result_cell_max_chars) = {
            let config = state
                .config
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (
                font_settings::profile_by_name(&config.editor_font),
                config.ui_font_size.clamp(8, 24) as i32,
                config.editor_font_size,
                config.result_font_size,
                config.result_cell_max_chars.clamp(
                    RESULT_CELL_MAX_DISPLAY_CHARS_MIN,
                    RESULT_CELL_MAX_DISPLAY_CHARS_MAX,
                ),
            )
        };
        app::set_font(unified_profile.normal);
        app::set_font_size(ui_size);
        fltk::misc::Tooltip::set_font(unified_profile.normal);
        fltk::misc::Tooltip::set_font_size(ui_size);
        fltk::dialog::message_set_font(unified_profile.normal, ui_size);
        for tab in &mut state.editor_tabs {
            tab.sql_editor
                .apply_font_settings(unified_profile, editor_size, ui_size);
        }
        state
            .result_tabs
            .apply_font_settings(unified_profile, result_size);
        state
            .result_tabs
            .set_max_cell_display_chars(result_cell_max_chars as usize);
        state
            .object_browser
            .apply_font_settings(unified_profile, ui_size);
        Self::apply_runtime_ui_font(state, unified_profile.normal, ui_size);
        state.right_tile.redraw();
        state.window.redraw();
        app::redraw();
        // Force FLTK to process the pending redraw immediately, so font
        // changes are visible right after the settings dialog closes
        // instead of requiring multiple save cycles.
        app::flush();
        app::awake();
    }

    fn apply_lazy_fetch_settings(state: &mut AppState) {
        let (
            lazy_fetch_batch_size,
            cancel_timeout_seconds,
            context_window_kib,
            intellisense_popup_delay_ms,
        ) = {
            let config = state
                .config
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (
                config.normalized_lazy_fetch_batch_size(),
                config.normalized_cancel_timeout_seconds(),
                config.normalized_intellisense_context_window_kib(),
                config.normalized_intellisense_popup_delay_ms(),
            )
        };
        for tab in &state.editor_tabs {
            tab.sql_editor
                .set_lazy_fetch_batch_size(lazy_fetch_batch_size);
            tab.sql_editor
                .set_cancel_timeout_seconds(cancel_timeout_seconds);
            tab.sql_editor
                .set_intellisense_context_window_kib(context_window_kib);
            tab.sql_editor
                .set_intellisense_popup_delay_ms(intellisense_popup_delay_ms);
        }
        state
            .sql_editor
            .set_lazy_fetch_batch_size(lazy_fetch_batch_size);
        state
            .sql_editor
            .set_cancel_timeout_seconds(cancel_timeout_seconds);
        state
            .sql_editor
            .set_intellisense_context_window_kib(context_window_kib);
        state
            .sql_editor
            .set_intellisense_popup_delay_ms(intellisense_popup_delay_ms);
    }

    fn apply_runtime_ui_font(state: &mut AppState, font: fltk::enums::Font, ui_size: i32) {
        fn apply_widget_font_recursive(widget: &mut Widget, font: fltk::enums::Font, size: i32) {
            widget.set_label_font(font);
            widget.set_label_size(size);
            if let Some(group) = widget.as_group() {
                for mut child in group.into_iter() {
                    apply_widget_font_recursive(&mut child, font, size);
                }
            }
        }

        let mut window = state.window.clone();
        window.set_label_font(font);
        window.set_label_size(ui_size);
        for mut child in window.clone().into_iter() {
            apply_widget_font_recursive(&mut child, font, ui_size);
        }

        if let Some(mut menu) = app::widget_from_id::<MenuBar>("main_menu") {
            menu.set_text_font(font);
            menu.set_text_size(ui_size);
        }

        for popup in state
            .popups
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter_mut()
        {
            popup.set_label_font(font);
            popup.set_label_size(ui_size);
            for mut child in popup.clone().into_iter() {
                apply_widget_font_recursive(&mut child, font, ui_size);
            }
        }

        state.result_toolbar.fixed(
            &state.result_one_tab_per_query_check,
            result_toolbar_checkbox_width(
                &state.result_one_tab_per_query_check,
                BUTTON_WIDTH_LARGE + 45,
            ),
        );
        if state.result_edit_check.visible() {
            state.result_toolbar.fixed(
                &state.result_edit_check,
                result_toolbar_checkbox_width(&state.result_edit_check, BUTTON_WIDTH_SMALL),
            );
        }
        state.result_toolbar.layout();
    }

    fn clamp_query_split_with(
        right_tile: &mut Tile,
        query_top_group: &mut Group,
        query_split_bar: &mut Frame,
    ) {
        let right_height = right_tile.h();
        if right_height <= 0 {
            return;
        }

        let max_query_height =
            (right_height - MIN_RESULTS_HEIGHT - QUERY_SPLIT_BAR_HEIGHT).max(MIN_QUERY_HEIGHT);
        let desired_query_height = query_top_group
            .h()
            .clamp(MIN_QUERY_HEIGHT, max_query_height);
        Self::apply_query_split_layout(
            right_tile,
            query_top_group,
            query_split_bar,
            desired_query_height,
        );
    }

    /// Apply the saved split ratio to compute the query pane height.
    fn apply_query_split_ratio(
        right_tile: &mut Tile,
        query_top_group: &mut Group,
        query_split_bar: &mut Frame,
        ratio: f64,
    ) {
        let right_height = right_tile.h();
        if right_height <= 0 {
            return;
        }
        let max_height =
            (right_height - MIN_RESULTS_HEIGHT - QUERY_SPLIT_BAR_HEIGHT).max(MIN_QUERY_HEIGHT);
        let desired_height = ((right_height as f64) * ratio).round() as i32;
        let desired_height = desired_height.clamp(MIN_QUERY_HEIGHT, max_height);
        Self::apply_query_split_layout(
            right_tile,
            query_top_group,
            query_split_bar,
            desired_height,
        );
    }

    fn adjust_query_layout_with(
        right_tile: &mut Tile,
        query_top_group: &mut Group,
        query_split_bar: &mut Frame,
    ) {
        let right_height = right_tile.h();
        if right_height <= 0 {
            return;
        }
        let max_height =
            (right_height - MIN_RESULTS_HEIGHT - QUERY_SPLIT_BAR_HEIGHT).max(MIN_QUERY_HEIGHT);
        let mut desired_height = ((right_height as f32) * 0.4).round() as i32;
        if desired_height < MIN_QUERY_HEIGHT {
            desired_height = MIN_QUERY_HEIGHT;
        } else if desired_height > max_height {
            desired_height = max_height;
        }
        Self::apply_query_split_layout(
            right_tile,
            query_top_group,
            query_split_bar,
            desired_height,
        );
    }

    fn apply_query_split_layout(
        right_tile: &mut Tile,
        query_top_group: &mut Group,
        query_split_bar: &mut Frame,
        desired_query_height: i32,
    ) {
        let right_height = right_tile.h().max(1);
        let right_width = right_tile.w();
        let tile_x = right_tile.x();
        let tile_y = right_tile.y();

        let max_query_height =
            (right_height - MIN_RESULTS_HEIGHT - QUERY_SPLIT_BAR_HEIGHT).max(MIN_QUERY_HEIGHT);
        let mut query_height = desired_query_height.clamp(MIN_QUERY_HEIGHT, max_query_height);
        if query_height >= right_height {
            query_height = right_height.saturating_sub(1).max(1);
        }
        let split_bar_height = QUERY_SPLIT_BAR_HEIGHT.min(right_height.max(0));
        let result_y = tile_y + query_height + split_bar_height;
        let result_height = (right_height - query_height - split_bar_height).max(1);
        let top_ptr = query_top_group.as_widget_ptr();

        query_top_group.resize(tile_x, tile_y, right_width, query_height);
        for child in right_tile.clone().into_iter() {
            let Some(mut child_group) = child.as_group() else {
                continue;
            };
            if child_group.as_widget_ptr() == top_ptr {
                continue;
            }
            child_group.resize(tile_x, result_y, right_width, result_height);
        }
        query_split_bar.resize(tile_x, tile_y + query_height, right_width, split_bar_height);
        right_tile.redraw();
    }

    fn create_query_editor_tab(state: &mut AppState) -> Option<QueryTabId> {
        Self::create_query_editor_tab_with_display_stabilization(state, true)
    }

    fn create_query_editor_tab_with_display_stabilization(
        state: &mut AppState,
        stabilize_display: bool,
    ) -> Option<QueryTabId> {
        let label = format!("Query {}", state.next_editor_tab_number);
        state.next_editor_tab_number = state.next_editor_tab_number.saturating_add(1);
        let tab_id = state.query_tabs.add_tab(&label);
        let group = state.query_tabs.tab_group(tab_id)?;
        group.begin();
        let mut editor = SqlEditorWidget::new_with_intellisense_data(
            state.connection.clone(),
            state.query_timeout_input.clone(),
            state.schema_intellisense_data.clone(),
        );
        editor.set_owner_tab_id(tab_id);
        let mut editor_group = editor.get_group().clone();
        editor_group.resize(group.x(), group.y(), group.w(), group.h());
        editor_group.layout();
        group.resizable(&editor_group);
        group.end();
        if stabilize_display {
            editor.stabilize_display_metrics();
        } else {
            editor.mark_display_metrics_pending();
        }
        editor.update_highlight_data(state.schema_highlight_data.clone());
        let buffer = editor.get_buffer();
        state.editor_tabs.push(QueryEditorTab {
            tab_id,
            base_label: label,
            sql_editor: editor,
            sql_buffer: buffer,
            current_file: None,
            pristine_text: String::new(),
            current_text_len: 0,
            is_dirty: false,
        });
        state.query_tabs.select(tab_id);
        let _ = state.set_active_editor_tab_with_display_stabilization(tab_id, stabilize_display);
        Some(tab_id)
    }

    fn close_query_editor_tab(state: &Arc<Mutex<AppState>>, tab_id: QueryTabId) -> bool {
        Self::close_query_editor_tab_with_dirty_check(state, tab_id, true)
            == QueryEditorCloseOutcome::Closed
    }

    fn close_all_query_editor_tabs(state: &Arc<Mutex<AppState>>) {
        let tab_ids = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .editor_tabs
            .iter()
            .map(|tab| tab.tab_id)
            .collect::<Vec<_>>();
        for tab_id in tab_ids {
            let tab_exists = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .find_tab_index(tab_id)
                .is_some();
            if !tab_exists {
                continue;
            }
            match Self::close_query_editor_tab_with_dirty_check(state, tab_id, true) {
                QueryEditorCloseOutcome::Closed | QueryEditorCloseOutcome::Deferred => {}
                QueryEditorCloseOutcome::Cancelled => break,
            }
        }
        app::redraw();
    }

    fn defer_close_query_editor_tab_until_idle(state: &Arc<Mutex<AppState>>, tab_id: QueryTabId) {
        let state_for_retry = Arc::clone(state);
        crate::ui::ui_timeout::schedule(0.2, move || {
            let should_wait = {
                let s = state_for_retry
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                s.find_tab_index(tab_id).is_some()
                    && s.has_running_query_or_lazy_fetch_for_tab(tab_id)
            };
            if should_wait {
                MainWindow::defer_close_query_editor_tab_until_idle(&state_for_retry, tab_id);
                return;
            }
            MainWindow::close_query_editor_tab_with_dirty_check(&state_for_retry, tab_id, false);
        });
    }

    fn close_query_editor_tab_with_dirty_check(
        state: &Arc<Mutex<AppState>>,
        tab_id: QueryTabId,
        check_dirty: bool,
    ) -> QueryEditorCloseOutcome {
        if check_dirty && !Self::confirm_save_if_dirty(state, tab_id, "closing this tab") {
            return QueryEditorCloseOutcome::Cancelled;
        }

        let has_running_work = {
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if s.find_tab_index(tab_id).is_none() {
                return QueryEditorCloseOutcome::Cancelled;
            }
            s.has_running_query_or_lazy_fetch_for_tab(tab_id)
        };
        if has_running_work && !Self::confirm_cancel_running_query_for_close(state, tab_id) {
            return QueryEditorCloseOutcome::Cancelled;
        }
        if has_running_work {
            Self::cancel_query_editor_tab(state, tab_id);
            Self::defer_close_query_editor_tab_until_idle(state, tab_id);
            return QueryEditorCloseOutcome::Deferred;
        }

        if !Self::resolve_pooled_session_before_close(state, tab_id) {
            return QueryEditorCloseOutcome::Cancelled;
        }

        let has_running_session = {
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if s.find_tab_index(tab_id).is_none() {
                return QueryEditorCloseOutcome::Cancelled;
            }
            s.has_running_query_or_lazy_fetch_for_tab(tab_id)
        };
        if has_running_session {
            Self::cancel_query_editor_tab(state, tab_id);
            Self::defer_close_query_editor_tab_until_idle(state, tab_id);
            return QueryEditorCloseOutcome::Deferred;
        }

        let (
            created_tab_id,
            schema_sender,
            file_sender,
            mut editor_to_cleanup,
            lazy_fetch_sessions,
            deferred_display_tab_id,
            focus_after_close,
        ) = {
            let mut s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(index) = s.find_tab_index(tab_id) else {
                return QueryEditorCloseOutcome::Cancelled;
            };

            let was_active = s.active_editor_tab_id == tab_id;
            // Avoid reading FLTK tab selection during the close transaction.
            // `Fl_Tabs::value()` can still observe the closing child and panic.
            let editor_tab_ids = s
                .editor_tabs
                .iter()
                .map(|tab| tab.tab_id)
                .collect::<Vec<_>>();
            let next_active_tab_id = next_active_editor_tab_id_after_close(
                &editor_tab_ids,
                index,
                s.active_editor_tab_id,
            );
            let editor_to_cleanup = s.editor_tabs[index].sql_editor.clone();
            let mut lazy_fetch_sessions = s
                .progress_contexts
                .get(&tab_id)
                .map(|context| {
                    context
                        .lazy_fetch_sessions
                        .keys()
                        .copied()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if let Some(session_id) = editor_to_cleanup.active_lazy_fetch_session() {
                if !lazy_fetch_sessions.contains(&session_id) {
                    lazy_fetch_sessions.push(session_id);
                }
            }
            if !s.query_tabs.close_tab(tab_id) {
                return QueryEditorCloseOutcome::Cancelled;
            }
            for session_id in &lazy_fetch_sessions {
                s.mark_lazy_fetch_result_tab_closed(*session_id);
                s.result_tabs.abort_lazy_fetch_session(*session_id);
            }
            if !lazy_fetch_sessions.is_empty() {
                s.refresh_result_edit_controls();
            }
            s.editor_tabs.remove(index);
            s.finish_progress_context(tab_id);

            let mut created_tab_id = None;
            let mut deferred_display_tab_id = None;
            if s.editor_tabs.is_empty() {
                created_tab_id =
                    MainWindow::create_query_editor_tab_with_display_stabilization(&mut s, false);
            }

            let next_tab_id = created_tab_id
                .or(next_active_tab_id)
                .or_else(|| s.query_tabs.tab_ids().first().copied())
                .or_else(|| s.editor_tabs.first().map(|tab| tab.tab_id));
            let switched_to_next = next_tab_id
                .map(|next_tab_id| {
                    let switched =
                        s.set_active_editor_tab_with_display_stabilization(next_tab_id, false);
                    if switched {
                        deferred_display_tab_id = Some(next_tab_id);
                    }
                    switched
                })
                .unwrap_or(false);

            if !switched_to_next {
                if let Some(fallback_tab) = s.editor_tabs.first().cloned() {
                    // Defensive fallback: if tab/widget selection loses sync, still point
                    // app state to a live editor tab so closed-tab resources are not held
                    // by stale SqlEditorWidget/TextBuffer handles.
                    s.active_editor_tab_id = fallback_tab.tab_id;
                    s.sql_editor = fallback_tab.sql_editor;
                    s.sql_editor.mark_display_metrics_pending();
                    s.sql_buffer = fallback_tab.sql_buffer;
                    *s.current_file
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                        fallback_tab.current_file;
                    s.query_tabs.select(fallback_tab.tab_id);
                    s.refresh_window_title();
                    deferred_display_tab_id = Some(fallback_tab.tab_id);
                } else if was_active {
                    // Defensive fallback: if tab selection cannot be resolved,
                    // clear active editor references so closed-tab resources are
                    // not kept alive by stale handles in application state.
                    let detached_editor = SqlEditorWidget::new_with_intellisense_data(
                        s.connection.clone(),
                        s.query_timeout_input.clone(),
                        s.schema_intellisense_data.clone(),
                    );
                    s.active_editor_tab_id = 0;
                    s.sql_buffer = detached_editor.get_buffer();
                    s.sql_editor = detached_editor;
                    *s.current_file
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
                    s.refresh_window_title();
                }
            }

            s.right_tile.redraw();
            app::redraw();

            // Large SQL buffers are dropped above. Ask allocator to release
            // free pages proactively so RSS reflects the close action sooner.
            malloc_trim_process();
            (
                created_tab_id,
                s.schema_sender.clone(),
                s.file_sender.clone(),
                editor_to_cleanup,
                lazy_fetch_sessions,
                deferred_display_tab_id,
                was_active,
            )
        };

        for session_id in &lazy_fetch_sessions {
            editor_to_cleanup.request_lazy_fetch(
                *session_id,
                crate::ui::sql_editor::LazyFetchRequest::CancelAndDiscard,
            );
        }
        editor_to_cleanup.cleanup_for_close();

        if let Some(tab_id) = created_tab_id {
            if let Some(schema_sender) = schema_sender {
                Self::attach_editor_callbacks(state, tab_id, schema_sender);
            }
            if let Some(file_sender) = file_sender {
                Self::attach_file_drop_callback(state, tab_id, file_sender);
            }
        }

        if let Some(tab_id) = deferred_display_tab_id {
            let state_for_deferred_display = Arc::clone(state);
            crate::ui::ui_timeout::schedule(0.0, move || {
                let editor = {
                    let s = state_for_deferred_display
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if s.active_editor_tab_id == tab_id {
                        Some(s.sql_editor.clone())
                    } else {
                        None
                    }
                };
                let Some(mut editor) = editor else {
                    return;
                };
                // Run after the close transaction so FLTK can finish pending
                // widget deletion before a metric refresh calls into flush().
                editor.stabilize_display_metrics();
                if focus_after_close {
                    editor.focus();
                }
            });
        }

        QueryEditorCloseOutcome::Closed
    }

    fn update_schema_snapshot(
        state: &mut AppState,
        data: IntellisenseData,
        highlight_data: HighlightData,
    ) {
        let mut combined_highlight = highlight_data.clone();
        let columns_from_intellisense = Self::collect_highlight_columns(&data);
        if !columns_from_intellisense.is_empty() {
            let mut seen: HashSet<String> = combined_highlight
                .columns
                .iter()
                .map(|name| name.to_uppercase())
                .collect();
            for name in columns_from_intellisense {
                let upper = name.to_uppercase();
                if seen.insert(upper) {
                    combined_highlight.columns.push(name);
                }
            }
        }

        *state
            .schema_intellisense_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = data;
        state.schema_highlight_data = combined_highlight;
        for tab in &mut state.editor_tabs {
            tab.sql_editor
                .update_highlight_data_deferred(state.schema_highlight_data.clone());
        }
        state
            .sql_editor
            .update_highlight_data_deferred(state.schema_highlight_data.clone());
    }

    fn collect_highlight_columns(data: &IntellisenseData) -> Vec<String> {
        data.get_all_columns_for_highlighting()
    }

    fn merge_unique_names(target: &mut Vec<String>, additions: &[String]) {
        let mut seen: HashSet<String> = target.iter().map(|name| name.to_uppercase()).collect();
        for name in additions {
            if seen.insert(name.to_uppercase()) {
                target.push(name.clone());
            }
        }
    }

    fn intellisense_scope_differs(data: &IntellisenseData, selected_scope: Option<&str>) -> bool {
        match (data.default_qualifier(), selected_scope) {
            (Some(current), Some(next)) => current.trim() != next.trim(),
            (None, Some(_)) => true,
            _ => false,
        }
    }

    fn merge_object_browser_snapshot_into_data(
        mut data: IntellisenseData,
        snapshot: &ObjectBrowserMetadataSnapshot,
    ) -> IntellisenseData {
        Self::merge_unique_names(&mut data.users, &snapshot.available_scopes);
        Self::merge_unique_names(&mut data.tables, &snapshot.tables);
        Self::merge_unique_names(&mut data.views, &snapshot.views);
        Self::merge_unique_names(&mut data.procedures, &snapshot.procedures);
        Self::merge_unique_names(&mut data.functions, &snapshot.functions);
        Self::merge_unique_names(&mut data.sequences, &snapshot.sequences);
        Self::merge_unique_names(&mut data.triggers, &snapshot.triggers);
        Self::merge_unique_names(&mut data.events, &snapshot.events);
        Self::merge_unique_names(&mut data.synonyms, &snapshot.synonyms);
        Self::merge_unique_names(&mut data.packages, &snapshot.packages);
        if let Some(scope) =
            canonical_intellisense_scope(&data, snapshot.selected_scope.clone(), snapshot.db_type)
        {
            data.set_default_qualifier(Some(scope.clone()));
            data.set_members_for_qualifier_with_kinds(&scope, snapshot.qualified_members());
            data.set_relation_members_for_qualifier(&scope, snapshot.relation_members());
        }
        data.rebuild_indices();
        data
    }

    fn apply_object_browser_metadata_snapshot(
        state: &mut AppState,
        snapshot: ObjectBrowserMetadataSnapshot,
    ) {
        let current_data = state
            .schema_intellisense_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let replace_active_scope =
            Self::intellisense_scope_differs(&current_data, snapshot.selected_scope.as_deref());

        let data = if replace_active_scope {
            snapshot.to_intellisense_data()
        } else {
            Self::merge_object_browser_snapshot_into_data(current_data, &snapshot)
        };

        let mut highlight_data = if replace_active_scope {
            snapshot.to_highlight_data()
        } else {
            state.schema_highlight_data.clone()
        };
        Self::merge_unique_names(&mut highlight_data.tables, &snapshot.tables);
        Self::merge_unique_names(&mut highlight_data.views, &snapshot.views);
        Self::merge_unique_names(&mut highlight_data.functions, &snapshot.functions);
        Self::merge_unique_names(&mut highlight_data.procedures, &snapshot.procedures);
        Self::merge_unique_names(&mut highlight_data.packages, &snapshot.packages);
        Self::merge_unique_names(&mut highlight_data.sequences, &snapshot.sequences);
        Self::merge_unique_names(&mut highlight_data.triggers, &snapshot.triggers);
        Self::merge_unique_names(&mut highlight_data.events, &snapshot.events);
        Self::merge_unique_names(&mut highlight_data.synonyms, &snapshot.synonyms);
        Self::merge_unique_names(&mut highlight_data.schemas, &snapshot.available_scopes);

        Self::update_schema_snapshot(state, data, highlight_data);
    }

    fn schema_update_scope_matches(
        db_type: DatabaseType,
        update_scope: Option<&str>,
        current_scope: Option<&str>,
        available_scopes: &[String],
    ) -> bool {
        let Some(current_scope) = current_scope else {
            return true;
        };
        let Some(update_scope) = update_scope else {
            return false;
        };
        if db_type.scope_values_match(Some(update_scope), Some(current_scope)) {
            return true;
        }
        if !db_type.is_mysql_or_mariadb() {
            return false;
        }
        let update_scope = update_scope.trim();
        let current_scope = current_scope.trim();
        if update_scope.is_empty()
            || current_scope.is_empty()
            || !update_scope.eq_ignore_ascii_case(current_scope)
        {
            return false;
        }
        available_scopes
            .iter()
            .filter(|scope| scope.eq_ignore_ascii_case(current_scope))
            .take(2)
            .count()
            == 1
    }

    fn metadata_pool_session_context(
        connection: &SharedConnection,
        activity: &str,
    ) -> Option<crate::db::DbPoolSessionContext> {
        match crate::db::pool_session_context_for_shared_connection(connection, Some(activity)) {
            Ok(context) => Some(context),
            Err(err) => {
                eprintln!("Warning: failed to prepare metadata refresh session: {err}");
                None
            }
        }
    }

    fn load_schema_update_from_pool_context(
        context: crate::db::DbPoolSessionContext,
        requested_scope: Option<String>,
    ) -> Option<SchemaUpdate> {
        context.ensure_current().ok()?;
        let connection_generation = context.connection_generation;
        let db_type = context.connection_info.db_type;
        let activity = db_type.metadata_refresh_activity(requested_scope.as_deref());
        let _activity_guard = crate::db::track_pool_db_activity(activity, db_type);
        let data = schema_metadata_loader_for(db_type).load(context.clone(), requested_scope)?;
        context.ensure_current().ok()?;

        let mut highlight_data = HighlightData::new();
        highlight_data.tables = data.tables.clone();
        highlight_data.views = data.views.clone();
        highlight_data.materialized_views = data.materialized_views.clone();
        highlight_data.functions = data.functions.clone();
        highlight_data.procedures = data.procedures.clone();
        highlight_data.packages = data.packages.clone();
        highlight_data.sequences = data.sequences.clone();
        highlight_data.triggers = data.triggers.clone();
        highlight_data.events = data.events.clone();
        highlight_data.types = data.types.clone();
        highlight_data.indexes = data.indexes.clone();
        highlight_data.synonyms = data.synonyms.clone();
        highlight_data.public_synonyms = data.public_synonyms.clone();
        highlight_data.schemas = data.users.clone();
        let mut data = data;
        data.rebuild_indices();
        highlight_data.columns = MainWindow::collect_highlight_columns(&data);

        Some(SchemaUpdate {
            selected_scope: data
                .default_qualifier_name()
                .or_else(|| data.default_qualifier())
                .map(str::to_string),
            data,
            highlight_data,
            connection_generation,
            db_type,
        })
    }

    fn start_connection_metadata_refresh(
        state: &mut AppState,
        schema_sender: &std::sync::mpsc::Sender<SchemaUpdate>,
    ) -> bool {
        let Some(schema_refresh_token) = try_set_mutex_flag(&state.schema_refresh_in_progress)
        else {
            return false;
        };

        let selected_scope = state.object_browser.selected_scope();
        let Some(context) = Self::metadata_pool_session_context(
            &state.connection,
            "Preparing schema metadata refresh",
        ) else {
            clear_mutex_flag_if_token(&state.schema_refresh_in_progress, schema_refresh_token);
            return false;
        };
        if !state.object_browser.refresh_with_context(context.clone()) {
            clear_mutex_flag_if_token(&state.schema_refresh_in_progress, schema_refresh_token);
            return false;
        }
        let schema_sender = schema_sender.clone();
        let schema_refresh_guard = state.schema_refresh_in_progress.clone();
        thread::spawn(move || {
            let _schema_refresh_guard =
                MutexFlagClearGuard::new(schema_refresh_guard, schema_refresh_token);
            let load_result = panic::catch_unwind(AssertUnwindSafe(|| {
                MainWindow::load_schema_update_from_pool_context(context, selected_scope)
            }));
            match load_result {
                Ok(Some(update)) => {
                    let _ = schema_sender.send(update);
                    app::awake();
                }
                Ok(None) => {}
                Err(payload) => {
                    let panic_msg = panic_payload_to_string(payload.as_ref());
                    crate::utils::logging::log_error(
                        "main_window::schema_metadata_refresh",
                        &format!("schema metadata refresh worker panicked: {panic_msg}"),
                    );
                    eprintln!("schema metadata refresh worker panicked: {panic_msg}");
                }
            }
        });
        true
    }

    fn start_object_browser_metadata_refresh(state: &mut AppState) -> bool {
        let Some(context) = Self::metadata_pool_session_context(
            &state.connection,
            "Preparing object browser metadata refresh",
        ) else {
            return false;
        };
        state.object_browser.refresh_with_context(context)
    }

    fn start_connection_metadata_refresh_for_scope_change(
        state: &mut AppState,
        schema_sender: &std::sync::mpsc::Sender<SchemaUpdate>,
    ) -> bool {
        if mutex_flag_is_set(&state.schema_refresh_in_progress) {
            let _ = Self::start_object_browser_metadata_refresh(state);
            return false;
        }

        Self::start_connection_metadata_refresh(state, schema_sender)
    }

    fn attach_editor_callbacks(
        state: &Arc<Mutex<AppState>>,
        tab_id: QueryTabId,
        schema_sender: std::sync::mpsc::Sender<SchemaUpdate>,
    ) {
        let Some(mut editor) = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .editor_tabs
            .iter()
            .find(|tab| tab.tab_id == tab_id)
            .map(|tab| tab.sql_editor.clone())
        else {
            return;
        };

        let weak_state_for_execute = Arc::downgrade(state);
        editor.set_execute_callback(move |query_result| {
            let Some(state_for_execute) = weak_state_for_execute.upgrade() else {
                return;
            };
            let mut s = state_for_execute
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let base_msg = if query_result.success {
                format!(
                    "{} | Time: {:.3}s",
                    query_result.message,
                    query_result.execution_time.as_secs_f64()
                )
            } else {
                format!(
                    "Error | Time: {:.3}s",
                    query_result.execution_time.as_secs_f64()
                )
            };
            s.set_status_message(&base_msg);
        });

        let weak_state_for_result_tab = Arc::downgrade(state);
        editor.set_result_tab_callback(move |request| {
            let Some(state_for_result_tab) = weak_state_for_result_tab.upgrade() else {
                return;
            };
            let mut s = state_for_result_tab
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            s.append_result_tab_request(request);
        });

        let weak_state_for_status = Arc::downgrade(state);
        editor.set_status_callback(move |message| {
            let Some(state_for_status) = weak_state_for_status.upgrade() else {
                return;
            };
            if let Ok(mut s) = state_for_status.try_lock() {
                s.set_status_message(message);
            };
        });

        let weak_state_for_object_context = Arc::downgrade(state);
        editor.set_object_context_callback(move |selected_text, data| {
            let Some(state_for_object_context) = weak_state_for_object_context.upgrade() else {
                return false;
            };
            let object_browser = {
                let s = state_for_object_context
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                s.object_browser.clone()
            };
            object_browser.show_context_menu_for_sql_selection(&selected_text, &data)
        });

        let weak_state_for_context_action = Arc::downgrade(state);
        editor.set_context_action_callback(move |action| {
            let Some(state_for_context_action) = weak_state_for_context_action.upgrade() else {
                return;
            };
            match action {
                SqlEditorContextAction::Close => {
                    MainWindow::close_query_editor_tab(&state_for_context_action, tab_id);
                }
                SqlEditorContextAction::CloseAll => {
                    MainWindow::close_all_query_editor_tabs(&state_for_context_action);
                }
            }
        });

        let weak_state_for_find = Arc::downgrade(state);
        editor.set_find_callback(move || {
            let Some(state_for_find) = weak_state_for_find.upgrade() else {
                return;
            };
            let (mut editor, mut buffer, popups) = {
                let s = state_for_find
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                (
                    s.sql_editor.get_editor(),
                    s.sql_buffer.clone(),
                    s.popups.clone(),
                )
            };
            FindReplaceDialog::show_find_with_registry(&mut editor, &mut buffer, popups);
        });

        let weak_state_for_replace = Arc::downgrade(state);
        editor.set_replace_callback(move || {
            let Some(state_for_replace) = weak_state_for_replace.upgrade() else {
                return;
            };
            let (mut editor, mut buffer, popups) = {
                let s = state_for_replace
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                (
                    s.sql_editor.get_editor(),
                    s.sql_buffer.clone(),
                    s.popups.clone(),
                )
            };
            FindReplaceDialog::show_replace_with_registry(&mut editor, &mut buffer, popups);
        });

        let weak_state_for_progress = Arc::downgrade(state);
        let schema_sender_for_progress = schema_sender;
        editor.set_progress_callback(move |progress| {
            let Some(state_for_progress) = weak_state_for_progress.upgrade() else {
                return;
            };
            let mut s = state_for_progress
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (progress, operation_token) = match progress {
                QueryProgress::Operation { token, progress } => {
                    if !s.operation_progress_matches(tab_id, token, &progress) {
                        return;
                    }
                    (*progress, Some(token))
                }
                QueryProgress::OperationAbandoned { token } => {
                    if s.operation_abandoned_matches(tab_id, token) {
                        s.mark_operation_abandoned_cancelled(tab_id, token);
                        s.set_status_message(ResultTabStatus::Cancelled.status_bar_message());
                        s.refresh_result_edit_controls();
                        s.sync_transaction_mode_controls();
                    }
                    return;
                }
                progress => (progress, None),
            };
            match progress {
                QueryProgress::Operation { .. } | QueryProgress::OperationAbandoned { .. } => {}
                QueryProgress::BatchStart { activity } => {
                    let has_live_connection = s.has_live_connection;
                    let has_running_queries = s.sql_editor.is_query_running()
                        || s.editor_tabs
                            .iter()
                            .any(|tab| tab.sql_editor.is_query_running());
                    if should_ignore_query_progress_when_disconnected(
                        has_live_connection,
                        has_running_queries,
                    ) {
                        return;
                    }
                    let one_tab_per_query = s.result_one_tab_per_query_check.value();
                    let lazy_fetch_sessions = if !one_tab_per_query
                        && s.result_grid_execution_target.is_none()
                    {
                        s.clear_result_grids_for_new_query_batch()
                    } else {
                        Vec::new()
                    };
                    let mut context = QueryProgressContext::new(
                        s.result_grid_execution_target,
                        activity,
                        operation_token,
                    );
                    if s.pending_query_canceling_tabs.contains(&tab_id) {
                        context.state_label = ResultTabStatus::Canceling.label().to_string();
                    }
                    s.progress_contexts.insert(tab_id, context);
                    s.sync_transaction_mode_controls();
                    drop(s);
                    for session_id in lazy_fetch_sessions {
                        AppState::request_lazy_fetch_on_editors(
                            &state_for_progress,
                            session_id,
                            crate::ui::sql_editor::LazyFetchRequest::CancelAndDiscard,
                        );
                    }
                }
                QueryProgress::StatementStart {
                    index,
                    result_tab_policy,
                } => {
                    let has_live_connection = s.has_live_connection;
                    let has_running_queries = s.sql_editor.is_query_running()
                        || s.editor_tabs
                            .iter()
                            .any(|tab| tab.sql_editor.is_query_running());
                    if should_ignore_query_progress_when_disconnected(
                        has_live_connection,
                        has_running_queries,
                    ) {
                        return;
                    }
                    let mut result_tabs = s.result_tabs.clone();
                    let query_canceling_pending = s.pending_query_canceling_tabs.contains(&tab_id);
                    let (result_tab_id, status, select_tab) = {
                        let Some(context) = s.progress_contexts.get_mut(&tab_id) else {
                            return;
                        };
                        context.fetch_row_counts.remove(&index);
                        context.mark_statement_running(index);
                        let create_result_tab = result_tab_policy.creates_result_tab();
                        let result_tab_id = create_result_tab.then(|| {
                            context.ensure_result_tab_id(index, || result_tabs.reserve_result_tab_id())
                        });
                        let select_tab = if create_result_tab {
                            let _ = context.claim_result_tab_auto_select();
                            true
                        } else {
                            false
                        };
                        let status =
                            statement_start_status(&context.state_label, query_canceling_pending);
                        context.state_label = status.label().to_string();
                        (result_tab_id, status, select_tab)
                    };
                    if status == ResultTabStatus::Canceling {
                        if s.should_show_progress_status_for_tab(tab_id) {
                            s.set_status_message(&format!(
                                "{} running query...",
                                ResultTabStatus::Canceling.label()
                            ));
                        }
                    } else {
                        let was_running = s.status_animation_running;
                        s.start_status_animation(ResultTabStatus::Running.status_bar_message());
                        if !was_running {
                            MainWindow::start_status_animation_timer(&state_for_progress);
                        }
                    }
                    s.refresh_result_edit_controls();
                    drop(s);
                    if let Some(result_tab_id) = result_tab_id {
                        result_tabs.ensure_statement_tab_by_id(result_tab_id, "Result", select_tab);
                        if status == ResultTabStatus::Canceling {
                            result_tabs.mark_statement_canceling_by_id(result_tab_id);
                        }
                    }
                    app::redraw();
                    app::flush();
                }
                QueryProgress::SelectStart {
                    index,
                    columns,
                    null_text,
                } => {
                    if columns.is_empty() {
                        return;
                    }
                    let has_live_connection = s.has_live_connection;
                    let has_running_queries = s.sql_editor.is_query_running()
                        || s.editor_tabs
                            .iter()
                            .any(|tab| tab.sql_editor.is_query_running());
                    if should_ignore_query_progress_when_disconnected(
                        has_live_connection,
                        has_running_queries,
                    ) {
                        return;
                    }
                    let mut result_tabs = s.result_tabs.clone();
                    let pending_canceling_sessions =
                        s.pending_lazy_fetch_canceling_sessions.clone();
                    let should_show_status = s.should_show_progress_status_for_tab(tab_id);
                    let (result_tab_id, lazy_fetch_session, preserve_canceling, select_tab) = {
                        let Some(context) = s.progress_contexts.get_mut(&tab_id) else {
                            return;
                        };
                        if context.closed_statement_indices.contains(&index) {
                            return;
                        }
                        let result_tab_id =
                            context.ensure_result_tab_id(index, || result_tabs.reserve_result_tab_id());
                        let select_tab = context.claim_result_tab_auto_select();
                        context.mark_lazy_fetch_active_for_statement(index);
                        context.fetch_row_counts.insert(index, 0);
                        context.active_statement_index = Some(index);
                        let lazy_fetch_session = context.lazy_fetch_session_for_statement(index);
                        let preserve_canceling = context.state_label
                            == ResultTabStatus::Canceling.label()
                            || lazy_fetch_session.is_some_and(|session_id| {
                                pending_canceling_sessions.contains(&session_id)
                            });
                        if !preserve_canceling {
                            context.state_label = ResultTabStatus::Fetching.label().to_string();
                        }
                        context.last_fetch_status_update = Instant::now();
                        (
                            result_tab_id,
                            lazy_fetch_session,
                            preserve_canceling,
                            select_tab,
                        )
                    };
                    if !preserve_canceling && should_show_status {
                        let was_running = s.status_animation_running;
                        s.start_status_animation(
                            &ResultTabStatus::Fetching.status_bar_message_with_rows(0),
                        );
                        if !was_running {
                            MainWindow::start_status_animation_timer(&state_for_progress);
                        }
                    }
                    s.refresh_result_edit_controls();
                    drop(s);
                    result_tabs.ensure_statement_tab_by_id(result_tab_id, "Result", select_tab);
                    result_tabs.start_streaming_by_id(result_tab_id, &columns, &null_text);
                    if let Some(session_id) = lazy_fetch_session {
                        result_tabs.set_lazy_fetch_session_by_id(result_tab_id, session_id);
                    }
                    if preserve_canceling {
                        result_tabs.mark_statement_canceling_by_id(result_tab_id);
                    }
                }
                QueryProgress::Rows { index, rows } => {
                    let Some(result_tab_id) = resolve_active_progress_tab_id(&s, tab_id, index)
                    else {
                        return;
                    };
                    let rows_len = rows.len();
                    let mut result_tabs = s.result_tabs.clone();
                    let status_animation_was_running = s.status_animation_running;
                    let pending_canceling_sessions =
                        s.pending_lazy_fetch_canceling_sessions.clone();
                    let should_show_status = s.should_show_progress_status_for_tab(tab_id);
                    let Some(context) = s.progress_contexts.get_mut(&tab_id) else {
                        return;
                    };
                    let status_update = {
                        let count = context.fetch_row_counts.entry(index).or_insert(0);
                        let previous_count = *count;
                        *count = previous_count.saturating_add(rows_len);
                        let new_count = *count;
                        context.active_statement_index = Some(index);
                        context.mark_lazy_fetch_active_for_statement(index);
                        let lazy_fetch_session = context.lazy_fetch_session_for_statement(index);
                        let preserve_canceling = context.state_label
                            == ResultTabStatus::Canceling.label()
                            || lazy_fetch_session.is_some_and(|session_id| {
                                pending_canceling_sessions.contains(&session_id)
                            });
                        if preserve_canceling {
                            None
                        } else {
                            let status_message =
                                ResultTabStatus::Fetching.status_bar_message_with_rows(new_count);
                            context.state_label = ResultTabStatus::Fetching.label().to_string();
                            // Throttle active animations, but restart immediately after
                            // lazy fetch waiting has stopped the status animation.
                            if should_refresh_fetch_status_animation(
                                status_animation_was_running,
                                previous_count,
                                context.last_fetch_status_update.elapsed(),
                            ) {
                                context.last_fetch_status_update = Instant::now();
                                Some(status_message)
                            } else {
                                None
                            }
                        }
                    };
                    if should_show_status {
                        if let Some(status_message) = status_update {
                            s.update_status_animation(&status_message);
                            if !status_animation_was_running {
                                MainWindow::start_status_animation_timer(&state_for_progress);
                            }
                        }
                    }
                    drop(s);
                    result_tabs.append_rows_by_id(result_tab_id, rows);
                }
                QueryProgress::LazyFetchSession {
                    index,
                    session_id,
                    operation_id,
                    connection_generation,
                } => {
                    let (active_lazy_fetch_session, event_is_current) = s
                        .find_tab_index(tab_id)
                        .and_then(|tab_index| s.editor_tabs.get(tab_index))
                        .map(|tab| {
                            (
                                tab.sql_editor.active_lazy_fetch_session(),
                                tab.sql_editor.lazy_fetch_progress_event_is_current(
                                    session_id,
                                    operation_id,
                                    connection_generation,
                                ),
                            )
                        })
                        .unwrap_or((None, false));
                    if !should_accept_lazy_fetch_session_event(
                        event_is_current,
                        active_lazy_fetch_session,
                        s.progress_contexts.get(&tab_id),
                        index,
                    ) {
                        return;
                    }
                    let mut result_tabs = s.result_tabs.clone();
                    let preserve_canceling = s.lazy_fetch_canceling_is_pending(session_id);
                    let (result_tab_id, select_tab) = {
                        let Some(context) = s.progress_contexts.get_mut(&tab_id) else {
                            return;
                        };
                        if context.closed_statement_indices.contains(&index) {
                            return;
                        }
                        let result_tab_id =
                            context.ensure_result_tab_id(index, || result_tabs.reserve_result_tab_id());
                        let select_tab = context.claim_result_tab_auto_select();
                        (result_tab_id, select_tab)
                    };
                    if let Some(context) = s.progress_contexts.get_mut(&tab_id) {
                        context.register_lazy_fetch_session(
                            session_id,
                            index,
                            operation_id,
                            connection_generation,
                        );
                        context.active_statement_index = Some(index);
                        context.state_label = if preserve_canceling {
                            ResultTabStatus::Canceling.label().to_string()
                        } else {
                            ResultTabStatus::Fetching.label().to_string()
                        };
                    };
                    drop(s);
                    result_tabs.ensure_statement_tab_by_id(result_tab_id, "Result", select_tab);
                    result_tabs.set_lazy_fetch_session_by_id(result_tab_id, session_id);
                    if preserve_canceling {
                        result_tabs.mark_statement_canceling_by_id(result_tab_id);
                    }
                }
                QueryProgress::LazyFetchWaiting { index, session_id } => {
                    let pending_canceling = s.lazy_fetch_canceling_is_pending(session_id);
                    let mut preserve_canceling = pending_canceling;
                    let should_show_status = s.should_show_progress_status_for_tab(tab_id);
                    let result_tab_id = if let Some(context) = s.progress_contexts.get_mut(&tab_id) {
                        if context.closed_statement_indices.contains(&index) {
                            return;
                        }
                        if !context.mark_lazy_fetch_waiting(session_id, index) {
                            return;
                        }
                        context.active_statement_index = Some(index);
                        preserve_canceling |=
                            context.state_label == ResultTabStatus::Canceling.label();
                        context.state_label = if preserve_canceling {
                            ResultTabStatus::Canceling.label().to_string()
                        } else {
                            ResultTabStatus::Waiting.label().to_string()
                        };
                        let Some(result_tab_id) = context.result_tab_id_for_statement(index) else {
                            return;
                        };
                        result_tab_id
                    } else {
                        return;
                    };
                    let mut result_tabs = s.result_tabs.clone();
                    if preserve_canceling {
                        if should_show_status {
                            s.set_status_message(&format!(
                                "{} lazy fetch...",
                                ResultTabStatus::Canceling.label()
                            ));
                        }
                    } else if should_show_status {
                        s.set_status_message(ResultTabStatus::Waiting.status_bar_message());
                    }
                    drop(s);
                    if preserve_canceling {
                        result_tabs.mark_statement_canceling_by_id(result_tab_id);
                    } else {
                        result_tabs.mark_lazy_fetch_waiting_by_id(result_tab_id, session_id);
                    }
                }
                QueryProgress::LazyFetchCanceling { session_id } => {
                    let should_show_status = !s.is_any_query_running();
                    if s.mark_lazy_fetch_canceling(session_id) {
                        if should_show_status {
                            s.set_status_message(&format!(
                                "{} lazy fetch...",
                                ResultTabStatus::Canceling.label()
                            ));
                        }
                        s.refresh_result_edit_controls();
                    }
                }
                QueryProgress::LazyFetchClosed {
                    index,
                    session_id,
                    operation_id,
                    connection_generation,
                    cancelled,
                    cursor_closed,
                    fetch_worker_done,
                    error_kind,
                } => {
                    let should_show_status = s.should_show_progress_status_for_tab(tab_id);
                    let pending_canceling_close = s.lazy_fetch_canceling_is_pending(session_id);
                    let active_lazy_fetch_still_present =
                        s.lazy_fetch_session_is_active_in_editor(session_id);
                    let mut result_tab_id = None;
                    let mut finished_all_lazy_fetches = false;
                    let mut ignore_result_tab = false;
                    let mut event_matches = false;
                    let mut orphaned_canceling_close = false;
                    let should_abort_result_tab = lazy_fetch_close_should_abort_result_tab(
                        cancelled,
                        cursor_closed,
                        fetch_worker_done,
                        error_kind,
                    );
                    if let Some(context) = s.progress_contexts.get_mut(&tab_id) {
                        let session_was_registered =
                            context.lazy_fetch_sessions.contains_key(&session_id);
                        if context.closed_statement_indices.remove(&index) {
                            ignore_result_tab = true;
                        }
                        event_matches = context.lazy_fetch_event_matches(
                            session_id,
                            index,
                            operation_id,
                            connection_generation,
                        );
                        if event_matches {
                            context.remove_lazy_fetch_session(session_id);
                        } else if !ignore_result_tab {
                            ignore_result_tab = true;
                        }
                        let context_was_canceling =
                            context.state_label == ResultTabStatus::Canceling.label();
                        orphaned_canceling_close = (pending_canceling_close
                            || context_was_canceling)
                            && !active_lazy_fetch_still_present
                            && session_was_registered
                            && !event_matches;
                        if !ignore_result_tab {
                            context.active_statement_index = Some(index);
                            context.state_label = if should_abort_result_tab {
                                ResultTabStatus::Cancelled.label().to_string()
                            } else {
                                ResultTabStatus::Done.label().to_string()
                            };
                            result_tab_id = context.result_tab_id_for_statement(index);
                        }
                        finished_all_lazy_fetches =
                            context.lazy_fetch_sessions.is_empty() && context.batch_finished;
                    }
                    if event_matches {
                        s.pending_lazy_fetch_canceling_sessions.remove(&session_id);
                    }
                    if orphaned_canceling_close {
                        s.mark_lazy_fetch_cancelled_without_status(session_id);
                        if should_show_status {
                            s.set_status_message(ResultTabStatus::Cancelled.status_bar_message());
                        }
                        s.refresh_result_edit_controls();
                        return;
                    }
                    if ignore_result_tab {
                        if finished_all_lazy_fetches {
                            s.finish_progress_context(tab_id);
                            s.refresh_result_edit_controls();
                        }
                        return;
                    }
                    if should_abort_result_tab && should_show_status {
                        s.set_status_message(ResultTabStatus::Cancelled.status_bar_message());
                    } else if finished_all_lazy_fetches && should_show_status {
                        s.set_status_message(ResultTabStatus::Done.status_bar_message());
                    }
                    let Some(result_tab_id) = result_tab_id else {
                        return;
                    };
                    let mut result_tabs = s.result_tabs.clone();
                    drop(s);
                    if should_abort_result_tab {
                        result_tabs.abort_lazy_fetch_session(session_id);
                    } else {
                        result_tabs.clear_lazy_fetch_session_by_id(result_tab_id, session_id, true);
                    }
                    if should_finish_progress_after_lazy_fetch_close(
                        cancelled,
                        finished_all_lazy_fetches,
                    ) {
                        let mut s = state_for_progress
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        s.finish_progress_context(tab_id);
                        s.refresh_result_edit_controls();
                    }
                }
                QueryProgress::ScriptOutput { lines } => {
                    let should_select =
                        !lines.is_empty()
                            && should_select_support_result_pane(
                                s.progress_contexts.get(&tab_id),
                            );
                    let mut result_tabs = s.result_tabs.clone();
                    drop(s);
                    result_tabs.append_script_output_lines(&lines);
                    if should_select {
                        result_tabs.select_script_output();
                    }
                }
                QueryProgress::DbmsOutput { lines } => {
                    let should_select =
                        !lines.is_empty()
                            && should_select_support_result_pane(
                                s.progress_contexts.get(&tab_id),
                            );
                    let mut result_tabs = s.result_tabs.clone();
                    drop(s);
                    result_tabs.append_dbms_output_lines(&lines);
                    if should_select {
                        result_tabs.select_dbms_output();
                    }
                }
                QueryProgress::Message { kind, lines } => {
                    let should_select_info = kind == ResultMessageKind::Info
                        && !lines.is_empty()
                        && should_select_support_result_pane(s.progress_contexts.get(&tab_id));
                    let mut result_tabs = s.result_tabs.clone();
                    drop(s);
                    result_tabs.append_message_lines(kind, &lines);
                    if kind == ResultMessageKind::Error {
                        result_tabs.select_messages_errors();
                    } else if should_select_info {
                        result_tabs.select_messages_info();
                    }
                }
                QueryProgress::ExplainPlanOutput { text } => {
                    let mut result_tabs = s.result_tabs.clone();
                    drop(s);
                    result_tabs.append_explain_plan_tab(&text);
                }
                QueryProgress::PromptInput { .. } => {}
                QueryProgress::RequestCancelOldestLazyFetchForSessionPool { response } => {
                    if let Some(session_id) = s.oldest_lazy_fetch_session() {
                        drop(s);
                        let requested = request_lazy_fetch_cancel_for_session_pool(
                            &state_for_progress,
                            session_id,
                        );
                        let _ = response.send(requested);
                    } else {
                        drop(s);
                        let _ = response.send(false);
                    }
                }
                QueryProgress::NotifyCancelOldestLazyFetchForSessionPool => {
                    if let Some(session_id) = s.oldest_lazy_fetch_session() {
                        drop(s);
                        let _ = request_lazy_fetch_cancel_for_session_pool(
                            &state_for_progress,
                            session_id,
                        );
                    }
                }
                QueryProgress::AutoCommitChanged { enabled } => {
                    s.set_status_message(auto_commit_changed_progress_status(enabled));
                    drop(s);
                }
                QueryProgress::ConnectionChanged { info } => {
                    if let Some(info) = info {
                        let lazy_fetch_sessions =
                            s.abort_lazy_fetch_result_tabs_for_connection_transition();
                        clear_mutex_flag(&s.schema_refresh_in_progress);
                        s.release_all_pooled_db_sessions();
                        let has_running_queries = s.sql_editor.is_query_running()
                            || s.editor_tabs
                                .iter()
                                .any(|tab| tab.sql_editor.is_query_running());
                        *s.connection_info
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(info.clone());
                        s.has_live_connection = true;
                        s.object_browser.reset_selected_scope();
                        s.set_status_message(&format!("Connected | {}", info.name));
                        s.sync_transaction_mode_controls_for_connected_db(info.db_type);
                        s.sql_editor.focus();
                        s.refresh_connection_dependent_controls();
                        s.sync_transaction_mode_controls();
                        if has_running_queries {
                            // CONNECT can appear mid-script. Deferring metadata fetch prevents
                            // object-browser/schema workers from competing with the active batch.
                            s.pending_connection_metadata_refresh = true;
                        } else {
                            let started = MainWindow::start_connection_metadata_refresh(
                                &mut s,
                                &schema_sender_for_progress,
                            );
                            s.update_pending_metadata_refresh_after_start_attempt(started);
                        }
                        drop(s);
                        for session_id in lazy_fetch_sessions {
                            AppState::request_lazy_fetch_on_editors(
                                &state_for_progress,
                                session_id,
                                crate::ui::sql_editor::LazyFetchRequest::CancelAndDiscard,
                            );
                        }
                    } else {
                        let lazy_fetch_sessions =
                            Self::transition_to_disconnected_state(&mut s, None);
                        drop(s);
                        for session_id in lazy_fetch_sessions {
                            AppState::request_lazy_fetch_on_editors(
                                &state_for_progress,
                                session_id,
                                crate::ui::sql_editor::LazyFetchRequest::CancelAndDiscard,
                            );
                        }
                    }
                }
                QueryProgress::DatabaseChanged { info } => {
                    let database = info.service_name.trim().to_string();
                    if database.is_empty() || !s.scope_matches_current_connection(&database) {
                        drop(s);
                        return;
                    }
                    let has_running_queries = s.sql_editor.is_query_running()
                        || s.editor_tabs
                            .iter()
                            .any(|tab| tab.sql_editor.is_query_running());
                    *s.connection_info
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(info.clone());
                    s.has_live_connection = true;
                    s.object_browser.set_selected_scope(Some(database.clone()));
                    let retained_scope_update = s.retained_scope_update(Some(database.clone()));
                    s.set_status_message(&format!("Database selected | {}", database));
                    s.refresh_connection_dependent_controls();
                    s.sync_transaction_mode_controls();
                    if has_running_queries {
                        s.pending_connection_metadata_refresh = true;
                    } else {
                        let started = MainWindow::start_connection_metadata_refresh_for_scope_change(
                            &mut s,
                            &schema_sender_for_progress,
                        );
                        s.update_pending_metadata_refresh_after_start_attempt(started);
                    }
                    drop(s);
                    if let Some(message) = retained_scope_update
                        .map(apply_retained_scope_update)
                        .and_then(|outcomes| first_retained_outcome_message(&outcomes))
                    {
                        crate::ui::alert_on_main(&format!(
                            "Scope was changed, but a retained session could not be updated: \n{}",
                            message
                        ));
                    }
                }
                QueryProgress::ScopeChangedNotice {
                    message,
                    selected_scope,
                } => {
                    let retained_scope_update = if let Some(scope) = selected_scope
                        .map(|scope| scope.trim().to_string())
                        .filter(|scope| !scope.is_empty())
                    {
                        if !s.scope_matches_current_connection(&scope) {
                            drop(s);
                            return;
                        }
                        s.object_browser.set_selected_scope(Some(scope.clone()));
                        s.retained_scope_update(Some(scope))
                    } else {
                        None
                    };
                    let status = message.lines().next().unwrap_or(&message).to_string();
                    s.set_status_message(&status);
                    drop(s);
                    if let Some(message) = retained_scope_update
                        .map(apply_retained_scope_update)
                        .and_then(|outcomes| first_retained_outcome_message(&outcomes))
                    {
                        crate::ui::alert_on_main(&format!(
                            "Scope was changed, but a retained session could not be updated: \n{}",
                            message
                        ));
                    }
                }
                QueryProgress::StatementFinished { index, result, .. } => {
                    let should_display_data_grid = should_display_result_in_data_grid(&result);
                    let has_live_connection = s.has_live_connection;
                    let has_running_queries = s.sql_editor.is_query_running()
                        || s.editor_tabs
                            .iter()
                            .any(|tab| tab.sql_editor.is_query_running());
                    if should_ignore_query_progress_when_disconnected(
                        has_live_connection,
                        has_running_queries,
                    ) {
                        return;
                    }
                    let mut result_tabs = s.result_tabs.clone();
                    let (
                        result_tab_id,
                        script_transcript,
                        has_fetched_rows,
                        context_was_canceling,
                        grid_execution_target,
                        should_select_info_pane,
                        select_tab,
                    ) = {
                        let Some(context) = s.progress_contexts.get_mut(&tab_id) else {
                            return;
                        };
                        if context.closed_statement_indices.remove(&index) {
                            context.fetch_row_counts.remove(&index);
                            context.result_tab_ids.remove(&index);
                            let finished_all_lazy_fetches =
                                context.lazy_fetch_sessions.is_empty() && context.batch_finished;
                            if finished_all_lazy_fetches {
                                s.finish_progress_context(tab_id);
                                s.refresh_result_edit_controls();
                            }
                            return;
                        }
                        let (result_tab_id, select_tab) = if should_display_data_grid {
                            (
                                Some(context.ensure_result_tab_id(index, || {
                                    result_tabs.reserve_result_tab_id()
                                })),
                                context.claim_result_tab_auto_select(),
                            )
                        } else {
                            (context.result_tab_id_for_statement(index), false)
                        };
                        (
                            result_tab_id,
                            script_transcript_owns_success_message(Some(context)),
                            context.fetch_row_counts.get(&index).copied().unwrap_or(0) > 0,
                            context.state_label == ResultTabStatus::Canceling.label(),
                            context.execution_target,
                            should_select_support_result_pane(Some(context)),
                            select_tab,
                        )
                    };
                    let result_status = statement_finished_status(&result, context_was_canceling);
                    let result_routes =
                        statement_finished_result_routes(&result, script_transcript, result_status);
                    let route_to_errors =
                        result_routes.contains(&ResultPaneRoute::MessagesErrors);
                    let route_to_info = result_routes.contains(&ResultPaneRoute::MessagesInfo);
                    let mut error_lines: Vec<String> = Vec::new();
                    let mut info_lines: Vec<String> = Vec::new();
                    if route_to_errors {
                        error_lines = result.message.lines().map(|l| l.to_string()).collect();
                    } else if route_to_info {
                        info_lines = result.message.lines().map(|l| l.to_string()).collect();
                    }
                    let remove_empty_error_grid = result_status == ResultTabStatus::Error
                        && result_tab_id.is_some()
                        && !has_fetched_rows;
                    let remove_empty_success_grid = result_status == ResultTabStatus::Done
                        && result_tab_id.is_some()
                        && !should_display_data_grid
                        && grid_execution_target.is_none()
                        && !has_fetched_rows;
                    let mut removed_last_result_tab = false;
                    if let Some(context) = s.progress_contexts.get_mut(&tab_id) {
                        context.fetch_row_counts.remove(&index);
                        context.mark_lazy_fetch_active_for_statement(index);
                        context.mark_statement_finished(index);
                        context.state_label = result_status.label().to_string();
                        if remove_empty_error_grid || remove_empty_success_grid {
                            context.result_tab_ids.remove(&index);
                            removed_last_result_tab = context.result_tab_ids.is_empty();
                        }
                    }
                    let should_select_info_pane = should_select_info_pane
                        || (remove_empty_success_grid && removed_last_result_tab);
                    if s.should_show_progress_status_for_tab(tab_id) {
                        s.set_status_message(result_status.status_bar_message());
                    }
                    let deferred_lazy_batch_done = s
                        .progress_contexts
                        .get(&tab_id)
                        .map(|context| {
                            context.batch_finished && context.lazy_fetch_sessions.is_empty()
                        })
                        .unwrap_or(false);

                    s.refresh_result_edit_controls();
                    drop(s);

                    if remove_empty_error_grid || remove_empty_success_grid {
                        if let Some(result_tab_id) = result_tab_id {
                            let _ = result_tabs.close_tab_by_id_and_take_lazy_fetch(result_tab_id);
                        }
                    } else if should_display_data_grid {
                        let Some(result_tab_id) = result_tab_id else {
                            return;
                        };
                        result_tabs.ensure_statement_tab_by_id(result_tab_id, "Result", select_tab);
                        result_tabs.finish_streaming_by_id(result_tab_id);
                        result_tabs.display_result_by_id(result_tab_id, &result);
                    } else if let Some(target) = grid_execution_target {
                        // Result-grid edit saves execute only DML, so the
                        // terminal result is non-select and would otherwise be
                        // dropped here (no result-tab index is mapped for it).
                        // Deliver it to the editable tab's table so its
                        // pending-save matching can clear the save and keep the
                        // staged rows; without this the save stays pending and
                        // is later recovered as "Save was interrupted".
                        result_tabs.deliver_result_grid_execution_result_by_id(target, &result);
                    } else if let Some(result_tab_id) = result_tab_id {
                        result_tabs.finish_result_status_by_id(result_tab_id, &result);
                    }
                    if result_status == ResultTabStatus::Cancelled {
                        if let Some(result_tab_id) = result_tab_id {
                            result_tabs.mark_statement_cancelled_by_id(result_tab_id);
                        }
                    }
                    if route_to_errors {
                        result_tabs.append_message_lines(ResultMessageKind::Error, &error_lines);
                        result_tabs.select_messages_errors();
                    } else if !info_lines.is_empty() {
                        result_tabs.append_message_lines(ResultMessageKind::Info, &info_lines);
                        if should_select_info_pane {
                            result_tabs.select_messages_info();
                        }
                    }
                    if deferred_lazy_batch_done {
                        let mut s = state_for_progress
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        s.finish_progress_context(tab_id);
                        s.refresh_result_edit_controls();
                    }
                }
                QueryProgress::WorkerPanicked { message } => {
                    s.set_status_message(&message);
                    s.refresh_result_edit_controls();
                    s.sync_transaction_mode_controls();
                }
                QueryProgress::MetadataRefreshNeeded => {
                    if s.has_live_connection
                        && (s.is_any_query_running() || !s.progress_contexts.is_empty())
                    {
                        s.pending_connection_metadata_refresh = true;
                    } else {
                        let started = MainWindow::start_connection_metadata_refresh(
                            &mut s,
                            &schema_sender_for_progress,
                        );
                        s.update_pending_metadata_refresh_after_start_attempt(started);
                    }
                }
                QueryProgress::ExecutionFinished(event) => {
                    let current_editor = s
                        .find_tab_index(tab_id)
                        .and_then(|index| s.editor_tabs.get(index))
                        .map(|tab| {
                            (
                                tab.sql_editor.editor_instance_id(),
                                tab.sql_editor.current_operation_id_value(),
                                tab.sql_editor.last_completed_operation_id_value(),
                            )
                        });
                    let current_connection_generation = match s.connection.lock() {
                        Ok(connection) => Some(connection.connection_generation()),
                        Err(poisoned) => {
                            eprintln!(
                                "Warning: connection lock was poisoned during progress handling; recovering."
                            );
                            Some(poisoned.into_inner().connection_generation())
                        }
                    };
                    // ExecutionFinished can update status text after cleanup.
                    // Gate that UI-only effect by the captured tab/editor and
                    // connection generation so a late event from a replaced
                    // editor cannot describe the current tab's retained state.
                    if !execution_finished_event_matches_current_editor(
                        &event,
                        tab_id,
                        current_editor.map(|(editor_id, _, _)| editor_id),
                        current_editor
                            .map(|(_, operation_id, _)| operation_id)
                            .unwrap_or_default(),
                        current_editor
                            .map(|(_, _, operation_id)| operation_id)
                            .unwrap_or_default(),
                        current_connection_generation,
                    ) {
                        return;
                    }
                    crate::utils::logging::log_info(
                        "main_window::progress",
                        &format!(
                            "ExecutionFinished: db_type={:?} sql_kind={:?} editor_id={} op_id={} conn_gen={} \
                             cancelled={} timed_out={} recoverable={} conn_err={} timeout_restored={}",
                            event.db_type,
                            event.sql_kind,
                            event.editor_id,
                            event.operation_id,
                            event.connection_generation,
                            event.cancelled,
                            event.timed_out,
                            event.recoverable_timeout,
                            event.has_connection_error,
                            event.timeout_settings_restored,
                        ),
                    );
                    let snapshot = s
                        .find_tab_index(tab_id)
                        .and_then(|index| s.editor_tabs.get(index))
                        .and_then(|tab| tab.sql_editor.pooled_session_activity_snapshot());
                    if let Some(message) = execution_finished_status_override(&event, snapshot) {
                        if s.should_show_progress_status_for_tab(tab_id) {
                            s.set_status_message(message);
                        }
                        s.refresh_result_edit_controls();
                        s.sync_transaction_mode_controls();
                    }
                }
                QueryProgress::BatchFinished => {
                    let pending_canceling_sessions =
                        s.pending_lazy_fetch_canceling_sessions.clone();
                    let should_show_status = s.should_show_progress_status_for_tab(tab_id);
                    let orphaned_canceling_sessions = orphaned_canceling_lazy_fetch_sessions(
                        s.progress_contexts.get(&tab_id),
                        &pending_canceling_sessions,
                        |session_id| s.lazy_fetch_session_is_active_in_editor(session_id),
                    );
                    for session_id in orphaned_canceling_sessions {
                        s.mark_lazy_fetch_cancelled_without_status(session_id);
                    }
                    if let Some(context) = s.progress_contexts.get_mut(&tab_id) {
                        if !context.lazy_fetch_sessions.is_empty() {
                            context.batch_finished = true;
                            let preserve_canceling = context.state_label
                                == ResultTabStatus::Canceling.label()
                                || context.lazy_fetch_sessions.keys().any(|session_id| {
                                    pending_canceling_sessions.contains(session_id)
                                });
                            let has_waiting_lazy_fetch = context.has_waiting_lazy_fetch();
                            if preserve_canceling {
                                context.state_label =
                                    ResultTabStatus::Canceling.label().to_string();
                            } else if has_waiting_lazy_fetch {
                                context.state_label = ResultTabStatus::Waiting.label().to_string();
                            }
                            if preserve_canceling && should_show_status {
                                s.set_status_message(&format!(
                                    "{} lazy fetch...",
                                    ResultTabStatus::Canceling.label()
                                ));
                            } else if has_waiting_lazy_fetch && should_show_status {
                                s.set_status_message(ResultTabStatus::Waiting.status_bar_message());
                            }
                            s.refresh_result_edit_controls();
                            s.sync_transaction_mode_controls();
                            return;
                        }
                    }
                    let canceling_tab_id = {
                        s.progress_contexts.get(&tab_id).and_then(|context| {
                            if context.state_label != ResultTabStatus::Canceling.label() {
                                return None;
                            }
                            context.active_statement_index.and_then(|statement_index| {
                                context.result_tab_id_for_statement(statement_index)
                            })
                        })
                    };
                    if let Some(tab_id) = canceling_tab_id {
                        s.result_tabs.mark_statement_cancelled_by_id(tab_id);
                    }
                    s.finish_progress_context(tab_id);
                    let has_running_queries = s.sql_editor.is_query_running()
                        || s.editor_tabs
                            .iter()
                            .any(|tab| tab.sql_editor.is_query_running());

                    if should_run_global_batch_cleanup(has_running_queries) {
                        let mut result_tabs = s.result_tabs.clone();
                        drop(s);

                        result_tabs.finish_non_lazy_streaming();
                        result_tabs.align_tab_strip_left();
                        let recovered_save_states = result_tabs.clear_orphaned_save_requests();
                        let recovered_edit_states = result_tabs.clear_orphaned_query_edit_backups();

                        let mut s = state_for_progress
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        s.result_grid_execution_target = None;
                        if s.pending_connection_metadata_refresh && s.has_live_connection {
                            let started = MainWindow::start_connection_metadata_refresh(
                                &mut s,
                                &schema_sender_for_progress,
                            );
                            s.update_pending_metadata_refresh_after_start_attempt(started);
                        }
                        // Query execution completed and large temporary buffers may
                        // have been released during result materialization.
                        malloc_trim_process();
                        let current_status = s.status_bar.label().to_ascii_lowercase();
                        let was_canceling = current_status.contains("canceling")
                            || current_status.contains("cancelling");
                        let needs_reset = current_status.contains("running query")
                            || current_status.contains("executing query")
                            || current_status.contains("fetching rows")
                            || current_status.contains("connection is busy")
                            || current_status.contains("query is already running");
                        if recovered_save_states > 0 {
                            s.set_status_message(
                                "Save was interrupted. Staged edits are still available.",
                            );
                        } else if recovered_edit_states > 0 {
                            s.set_status_message(
                                "Query ended before completion. Restored staged result-grid edits.",
                            );
                        } else if was_canceling {
                            s.set_status_message(ResultTabStatus::Cancelled.status_bar_message());
                        } else if needs_reset {
                            s.set_status_message(ResultTabStatus::Done.status_bar_message());
                        }
                        s.refresh_result_edit_controls();
                        s.sync_transaction_mode_controls();
                    } else {
                        s.refresh_result_edit_controls();
                        s.sync_transaction_mode_controls();
                    }
                }
            }
        });

        let weak_state_for_dirty = Arc::downgrade(state);
        let mut buffer_for_dirty = editor.get_buffer();
        buffer_for_dirty.add_modify_callback2(move |buf, pos, ins, del, _restyled, _deleted| {
            let Some(state_for_dirty) = weak_state_for_dirty.upgrade() else {
                return;
            };
            if let Ok(mut s) = state_for_dirty.try_lock() {
                s.on_tab_buffer_modified(tab_id, pos, ins, del, buf)
            };
        });
    }

    fn attach_file_drop_callback(
        state: &Arc<Mutex<AppState>>,
        tab_id: QueryTabId,
        file_sender: std::sync::mpsc::Sender<FileActionResult>,
    ) {
        let Some(mut editor) = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .editor_tabs
            .iter()
            .find(|tab| tab.tab_id == tab_id)
            .map(|tab| tab.sql_editor.clone())
        else {
            return;
        };
        let weak_state_for_file_drop = Arc::downgrade(state);
        let file_sender_for_drop = file_sender;
        editor.set_file_drop_callback(move |path| {
            if let Some(state_for_drop) = weak_state_for_file_drop.upgrade() {
                let mut s = state_for_drop
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if MainWindow::focus_existing_tab_with_same_file_path(&mut s, &path) {
                    MainWindow::record_recent_sql_file(&mut s, &path);
                    return;
                }
                let conn_info = s
                    .connection_info
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                let file_label = path.file_name().unwrap_or_default().to_string_lossy();
                s.status_bar.set_label(&format_status(
                    &format!("Opening {} in new tab", file_label),
                    &conn_info,
                ));
            }

            let sender = file_sender_for_drop.clone();
            thread::spawn(move || {
                let result = fs::read_to_string(&path).map_err(|err| err.to_string());
                let _ = sender.send(FileActionResult::OpenInNewTab { path, result });
                app::awake();
            });
        });
    }

    fn execute_menu_action(
        state: &Arc<Mutex<AppState>>,
        schema_sender: &std::sync::mpsc::Sender<SchemaUpdate>,
        conn_sender: &std::sync::mpsc::Sender<ConnectionResult>,
        file_sender: &std::sync::mpsc::Sender<FileActionResult>,
        choice: &str,
    ) -> bool {
        match choice {
            "File/Connect" => {
                let block_message = {
                    let s = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    connection_transition_block_message(
                        s.is_any_query_running(),
                        s.has_active_lazy_fetches(),
                        "connecting",
                    )
                };
                if let Some(message) = block_message {
                    crate::ui::alert_on_main(&message);
                    return true;
                }

                let (popups, connection, pool_size) = {
                    let s = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let pool_size = s
                        .config
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .normalized_connection_pool_size();
                    (s.popups.clone(), s.connection.clone(), pool_size)
                };
                if let Some(info) = ConnectionDialog::show_with_registry(popups) {
                    if !Self::resolve_pooled_sessions_before_connection_transition(state) {
                        return true;
                    }

                    let conn_sender = conn_sender.clone();
                    {
                        let mut s = state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        s.status_bar
                            .set_label(&format!("Connecting to {}...", info.name));
                    }
                    thread::spawn(move || {
                        let Some(mut db_conn) = try_lock_connection_with_activity(
                            &connection,
                            format!("Connecting to {}", info.name),
                        ) else {
                            let _ = conn_sender
                                .send(ConnectionResult::Failure(format_connection_busy_message()));
                            app::awake();
                            return;
                        };
                        crate::db::clear_pool_session_context_for_shared_connection(&connection);
                        db_conn.set_connection_pool_size(pool_size);
                        match db_conn.connect(info.clone()) {
                            Ok(_) => {
                                db_conn.refresh_tracked_connection();
                                crate::db::refresh_pool_session_context_cache_for_shared_connection(
                                    &connection,
                                    &db_conn,
                                );
                                let session = db_conn.session_state();
                                drop(db_conn);
                                match session.lock() {
                                    Ok(mut guard) => guard.reset(),
                                    Err(poisoned) => {
                                        eprintln!(
                                            "Warning: session state lock was poisoned; recovering."
                                        );
                                        poisoned.into_inner().reset();
                                    }
                                }
                                let mut info = info;
                                info.clear_password();
                                let _ = conn_sender.send(ConnectionResult::Success(Box::new(info)));
                                app::awake();
                            }
                            Err(e) => {
                                crate::db::refresh_pool_session_context_cache_for_shared_connection(
                                    &connection,
                                    &db_conn,
                                );
                                drop(db_conn);
                                let _ = conn_sender.send(ConnectionResult::Failure(e.to_string()));
                                app::awake();
                            }
                        }
                    });
                }
                true
            }
            "File/Disconnect" => {
                let block_message = {
                    let s = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    connection_transition_block_message(
                        s.is_any_query_running(),
                        s.has_active_lazy_fetches(),
                        "disconnecting",
                    )
                };
                if let Some(message) = block_message {
                    crate::ui::alert_on_main(&message);
                    return true;
                }

                if !Self::resolve_pooled_sessions_before_connection_transition(state) {
                    return true;
                }

                let connection = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .connection
                    .clone();
                let Some(mut db_conn) =
                    try_lock_connection_with_activity(&connection, "Disconnecting session")
                else {
                    let busy_message = format_connection_busy_message();
                    crate::ui::alert_on_main(&busy_message);
                    let mut s = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let conn_info = s
                        .connection_info
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clone();
                    s.status_bar
                        .set_label(&format_status(&busy_message, &conn_info));
                    return true;
                };
                crate::db::clear_pool_session_context_for_shared_connection(&connection);
                crate::utils::logging::log_info("connection", "Disconnected from database");
                db_conn.disconnect();
                db_conn.refresh_tracked_connection();
                crate::db::clear_tracked_db_activity();
                // Release the connection lock before locking AppState.
                // Session reset is handled inside transition_to_disconnected_state.
                drop(db_conn);

                let mut s = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let _ = MainWindow::transition_to_disconnected_state(&mut s, None);
                true
            }
            "File/Open SQL File" => {
                let mut dialog = FileDialog::new(FileDialogType::BrowseFile);
                // The native macOS open panel auto-appends an "All Files" entry,
                // so listing it here would show it twice.
                dialog.set_filter("SQL Files\t*.sql");
                dialog.show();
                let filename = dialog.filename();
                MainWindow::open_sql_file_path(state, file_sender, filename);
                true
            }
            "File/Save SQL File" => {
                let tab_id = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .active_editor_tab_id;
                if let SaveTabOutcome::Failed(err) = MainWindow::save_tab(state, tab_id, false) {
                    crate::ui::alert_on_main(&format!("Failed to save SQL file: {}", err));
                }
                true
            }
            "File/Save SQL File As" => {
                let tab_id = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .active_editor_tab_id;
                if let SaveTabOutcome::Failed(err) = MainWindow::save_tab(state, tab_id, true) {
                    crate::ui::alert_on_main(&format!("Failed to save SQL file: {}", err));
                }
                true
            }
            "File/Exit" => {
                let mut window = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .window
                    .clone();
                window.do_callback();
                true
            }
            "Edit/Undo" => {
                state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .sql_editor
                    .undo();
                true
            }
            "Edit/Redo" => {
                state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .sql_editor
                    .redo();
                true
            }
            "Edit/Cut" => {
                state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .sql_editor
                    .get_editor()
                    .cut();
                true
            }
            "Edit/Copy" => {
                let mut s = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let result_tabs_widget = s.result_tabs.get_widget();
                let focus_in_results = if let Some(focus) = app::focus() {
                    focus.as_widget_ptr() == result_tabs_widget.as_widget_ptr()
                        || focus.inside(&result_tabs_widget)
                } else {
                    false
                };
                let focus_in_object_browser = s.object_browser.has_focus();

                if focus_in_results {
                    let cell_count = s.result_tabs.copy();
                    let conn_info = s
                        .connection_info
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clone();
                    if cell_count > 0 {
                        s.status_bar.set_label(&format_status(
                            &format!("Copied {} cells to clipboard", cell_count),
                            &conn_info,
                        ));
                    } else {
                        s.status_bar
                            .set_label(&format_status("No cells selected to copy", &conn_info));
                    }
                } else if focus_in_object_browser {
                    if !s.object_browser.copy_focused_selection_to_clipboard() {
                        let conn_info = s
                            .connection_info
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .clone();
                        s.status_bar.set_label(&format_status(
                            "No object browser item selected to copy",
                            &conn_info,
                        ));
                    }
                } else {
                    s.sql_editor.get_editor().copy();
                }
                true
            }
            "Edit/Copy with Headers" => {
                let mut s = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let result_tabs_widget = s.result_tabs.get_widget();
                let focus_in_results = if let Some(focus) = app::focus() {
                    focus.as_widget_ptr() == result_tabs_widget.as_widget_ptr()
                        || focus.inside(&result_tabs_widget)
                } else {
                    false
                };
                let focus_in_object_browser = s.object_browser.has_focus();

                if focus_in_results {
                    s.result_tabs.copy_with_headers();
                    let conn_info = s
                        .connection_info
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clone();
                    s.status_bar
                        .set_label(&format_status("Copied selection with headers", &conn_info));
                } else if focus_in_object_browser {
                    if !s.object_browser.copy_focused_selection_to_clipboard() {
                        let conn_info = s
                            .connection_info
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .clone();
                        s.status_bar.set_label(&format_status(
                            "No object browser item selected to copy",
                            &conn_info,
                        ));
                    }
                } else {
                    s.sql_editor.get_editor().copy();
                }
                true
            }
            "Edit/Paste" => {
                let s = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let result_tabs_widget = s.result_tabs.get_widget();
                let focus_in_results = if let Some(focus) = app::focus() {
                    focus.as_widget_ptr() == result_tabs_widget.as_widget_ptr()
                        || focus.inside(&result_tabs_widget)
                } else {
                    false
                };

                if focus_in_results {
                    let _ = s.result_tabs.paste_from_clipboard();
                } else {
                    s.sql_editor.get_editor().paste();
                }
                true
            }
            "Edit/Select All" => {
                let mut s = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let result_tabs_widget = s.result_tabs.get_widget();
                let focus_in_results = if let Some(focus) = app::focus() {
                    focus.as_widget_ptr() == result_tabs_widget.as_widget_ptr()
                        || focus.inside(&result_tabs_widget)
                } else {
                    false
                };

                if focus_in_results {
                    s.result_tabs.select_all();
                } else {
                    let len = s.sql_buffer.length();
                    s.sql_buffer.select(0, len);
                }
                true
            }
            "Query/Execute" => {
                execute_sql_request_with_session_pool_slot(state, SqlExecutionRequest::Current);
                true
            }
            "File/New SQL File" => {
                let created_tab_id = {
                    let mut s = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let created = MainWindow::create_query_editor_tab(&mut s);
                    s.right_tile.redraw();
                    created
                };
                if let Some(tab_id) = created_tab_id {
                    MainWindow::attach_editor_callbacks(state, tab_id, schema_sender.clone());
                    MainWindow::attach_file_drop_callback(state, tab_id, file_sender.clone());
                    state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .sql_editor
                        .focus();
                    app::redraw();
                }
                true
            }
            "File/Close SQL File" => {
                let tab_id = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .active_editor_tab_id;
                MainWindow::close_query_editor_tab(state, tab_id);
                true
            }
            "Query/Execute Statement" => {
                execute_sql_request_with_session_pool_slot(
                    state,
                    SqlExecutionRequest::StatementAtCursor,
                );
                true
            }
            "Query/Execute Statement (F9)" => {
                execute_sql_request_with_session_pool_slot(
                    state,
                    SqlExecutionRequest::StatementAtCursor,
                );
                true
            }
            "Query/Execute Selected" => {
                execute_sql_request_with_session_pool_slot(state, SqlExecutionRequest::Selected);
                true
            }
            "Query/Quick Describe" => {
                if let Some(editor) = acquire_sql_editor_if_idle(state) {
                    editor.quick_describe_at_cursor();
                }
                true
            }
            "Query/Explain Plan" => {
                if let Some(editor) = acquire_sql_editor_if_idle(state) {
                    editor.explain_current();
                }
                true
            }
            "Query/Commit" => {
                if let Some(editor) = acquire_sql_editor_if_idle(state) {
                    editor.commit();
                }
                true
            }
            "Query/Rollback" => {
                if let Some(editor) = acquire_sql_editor_if_idle(state) {
                    editor.rollback();
                }
                true
            }
            "Tools/Refresh Objects" => {
                let alert = {
                    let mut s = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if s.is_any_query_running() {
                        Some(crate::db::format_connection_busy_message())
                    } else if !MainWindow::start_connection_metadata_refresh(&mut s, schema_sender)
                    {
                        Some("Object browser refresh already in progress.".to_string())
                    } else {
                        None
                    }
                };
                if let Some(message) = alert {
                    SqlEditorWidget::show_alert_dialog(&message);
                }
                true
            }
            "Tools/Export Results" => {
                MainWindow::export_current_results_to_csv(state, file_sender);
                true
            }
            "Edit/Find" => {
                let (mut editor, mut buffer, popups) = {
                    let s = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    (
                        s.sql_editor.get_editor(),
                        s.sql_buffer.clone(),
                        s.popups.clone(),
                    )
                };
                FindReplaceDialog::show_find_with_registry(&mut editor, &mut buffer, popups);
                true
            }
            "Edit/Find Next" => {
                let (mut editor, mut buffer, popups) = {
                    let s = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    (
                        s.sql_editor.get_editor(),
                        s.sql_buffer.clone(),
                        s.popups.clone(),
                    )
                };
                if !FindReplaceDialog::find_next_from_session(&mut editor, &mut buffer)
                    && !FindReplaceDialog::has_search_text()
                {
                    FindReplaceDialog::show_find_with_registry(&mut editor, &mut buffer, popups);
                }
                true
            }
            "Edit/Replace" => {
                let (mut editor, mut buffer, popups) = {
                    let s = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    (
                        s.sql_editor.get_editor(),
                        s.sql_buffer.clone(),
                        s.popups.clone(),
                    )
                };
                FindReplaceDialog::show_replace_with_registry(&mut editor, &mut buffer, popups);
                true
            }
            "Edit/Format SQL" => {
                state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .sql_editor
                    .format_selected_sql();
                true
            }
            "Edit/Toggle Comment" => {
                state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .sql_editor
                    .toggle_comment();
                true
            }
            "Edit/Uppercase Selection" => {
                state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .sql_editor
                    .convert_selection_case(true);
                true
            }
            "Edit/Lowercase Selection" => {
                state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .sql_editor
                    .convert_selection_case(false);
                true
            }
            "Edit/Intellisense" => {
                state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .sql_editor
                    .show_intellisense();
                true
            }
            "Tools/Query History" => {
                MainWindow::open_query_history_dialog(state);
                true
            }
            "Tools/Session Activity" => {
                let mut s = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let request = s.build_session_activity_result_request();
                s.append_result_tab_request(request);
                true
            }
            "Tools/Application Log" => {
                let popups = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .popups
                    .clone();
                crate::ui::log_viewer::LogViewerDialog::show(popups);
                true
            }
            "Tools/Auto-Commit" => {
                let mut item = app::widget_from_id::<MenuBar>("main_menu")
                    .and_then(|menu| menu.find_item("&Tools/&Auto-Commit"));
                let enabled = item.as_ref().map(|item| item.value()).unwrap_or(false);
                let status = if enabled {
                    "Auto-commit enabled"
                } else {
                    "Auto-commit disabled"
                };
                let connection = {
                    let mut s = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if let Some(message) = transaction_option_block_message(
                        s.is_any_query_running(),
                        s.has_active_lazy_fetches(),
                        "changing auto-commit",
                    ) {
                        crate::ui::alert_on_main(&message);
                        s.set_status_message(&message);
                        if let Some(mut item) = item.take() {
                            if enabled {
                                item.clear();
                            } else {
                                item.set();
                            }
                        }
                        return true;
                    }
                    if let Some(message) = s.retained_transaction_option_blocker("auto-commit") {
                        crate::ui::alert_on_main(&message);
                        if let Some(mut item) = item.take() {
                            if enabled {
                                item.clear();
                            } else {
                                item.set();
                            }
                        }
                        return true;
                    }
                    s.connection.clone()
                };
                let retained_editors = {
                    let s = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    s.retained_session_editors()
                };
                if let Some(mut connection) =
                    try_lock_connection_with_activity(&connection, "Updating auto-commit setting")
                {
                    let retained_plan =
                        RetainedSessionOptionChangePlan::new(&connection, retained_editors);
                    if let Err(err) =
                        retained_plan.validate_transaction_option_change("auto-commit")
                    {
                        crate::ui::alert_on_main(&err);
                        let mut s = state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        let conn_info = s
                            .connection_info
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .clone();
                        s.status_bar
                            .set_label(&format_status("Auto-commit unchanged", &conn_info));
                        if let Some(mut item) = item.take() {
                            if enabled {
                                item.clear();
                            } else {
                                item.set();
                            }
                        }
                        return true;
                    }
                    if let Err(err) = connection.set_auto_commit(enabled) {
                        crate::ui::alert_on_main(&err);
                        let mut s = state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        let conn_info = s
                            .connection_info
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .clone();
                        s.status_bar
                            .set_label(&format_status("Auto-commit unchanged", &conn_info));
                        if let Some(mut item) = item.take() {
                            if enabled {
                                item.clear();
                            } else {
                                item.set();
                            }
                        }
                        return true;
                    }
                    let pool_context_epoch = connection.pool_context_epoch();
                    drop(connection);
                    let retained_outcomes = retained_plan.apply_auto_commit(
                        pool_context_epoch,
                        enabled,
                        "Updating auto-commit setting",
                    );
                    if let Some(message) = first_retained_outcome_message(&retained_outcomes) {
                        crate::ui::alert_on_main(&format!(
                            "Auto-commit was changed, but a retained session could not be updated. It was restored or discarded according to session safety: {}",
                            message
                        ));
                    }
                } else {
                    let busy_message = format_connection_busy_message();
                    crate::ui::alert_on_main(&busy_message);
                    let mut s = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let conn_info = s
                        .connection_info
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clone();
                    s.status_bar
                        .set_label(&format_status(&busy_message, &conn_info));
                    if let Some(mut item) = item.take() {
                        if enabled {
                            item.clear();
                        } else {
                            item.set();
                        }
                    }
                    return true;
                }
                let mut s = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                s.sync_mysql_auto_commit_overrides_with_global_setting(enabled);
                let conn_info = s
                    .connection_info
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                s.status_bar.set_label(&format_status(status, &conn_info));
                true
            }
            "Settings/Preferences" => {
                let config_snapshot = {
                    let s = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let config_snapshot = s
                        .config
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clone();
                    config_snapshot
                };
                if let Some(settings) = show_settings_dialog(&config_snapshot) {
                    let pool_size_changed = settings.connection_pool_size
                        != config_snapshot.normalized_connection_pool_size();
                    if pool_size_changed && !Self::resolve_pooled_sessions_before_pool_resize(state)
                    {
                        return true;
                    }
                    let resize_result = if pool_size_changed {
                        let (connection, blocked) = {
                            let s = state
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            (
                                s.connection.clone(),
                                s.is_any_query_running() || s.has_active_lazy_fetches(),
                            )
                        };
                        if blocked {
                            Err(
                                "Finish or cancel running queries and lazy fetches before changing connection pool size."
                                    .to_string(),
                            )
                        } else if let Some(mut connection_guard) = try_lock_connection_with_activity(
                            &connection,
                            "Updating session pool preference",
                        ) {
                            crate::db::clear_pool_session_context_for_shared_connection(
                                &connection,
                            );
                            let resize_result = connection_guard
                                .resize_current_connection_pool(settings.connection_pool_size);
                            if resize_result.is_ok() {
                                crate::db::refresh_pool_session_context_cache_for_shared_connection(
                                    &connection,
                                    &connection_guard,
                                );
                            }
                            resize_result
                        } else {
                            Err(format_connection_busy_message())
                        }
                    } else {
                        Ok(())
                    };

                    let save_result = resize_result.and_then(|_| {
                        let mut s = state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        let save_result = {
                            let mut config = s
                                .config
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            config.editor_font = settings.font.clone();
                            config.ui_font_size = settings.ui_size;
                            config.editor_font_size = settings.editor_size;
                            config.result_font = settings.font;
                            config.result_font_size = settings.result_size;
                            config.result_cell_max_chars = settings.result_cell_max_chars;
                            config.lazy_fetch_batch_size = settings.lazy_fetch_batch_size;
                            config.intellisense_context_window_kib =
                                settings.intellisense_context_window_kib;
                            config.intellisense_popup_delay_ms =
                                settings.intellisense_popup_delay_ms;
                            config.connection_pool_size = settings.connection_pool_size;
                            config.cancel_timeout_seconds = settings.cancel_timeout_seconds;
                            config.sql_comma_list_layout = settings.sql_comma_list_layout;
                            config.sql_format_right_margin = settings.sql_format_right_margin;
                            config.save()
                        };
                        if pool_size_changed {
                            s.release_all_resolved_pooled_db_sessions()?;
                        }
                        MainWindow::apply_lazy_fetch_settings(&mut s);
                        MainWindow::apply_font_settings(&mut s);
                        save_result.map_err(|err| err.to_string())
                    });
                    if let Err(err) = save_result {
                        crate::ui::alert_on_main(&format!("Failed to save settings: {}", err));
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn strip_menu_label_shortcut(path: &str) -> String {
        let raw = path.split('\t').next().unwrap_or(path).trim();
        let label = if let Some(open_paren) = raw.rfind(" (") {
            if raw.ends_with(')') && raw[open_paren..].starts_with(" (") {
                raw[..open_paren].trim_end()
            } else {
                raw
            }
        } else {
            raw
        };
        label.replace('&', "")
    }

    fn menu_shortcut_for_key(
        key: fltk::enums::Key,
        modifiers: fltk::enums::Shortcut,
    ) -> Option<&'static str> {
        let ctrl_or_cmd = modifiers.contains(fltk::enums::Shortcut::Ctrl)
            || modifiers.contains(fltk::enums::Shortcut::Command);
        let shift = modifiers.contains(fltk::enums::Shortcut::Shift);

        match key {
            k if ctrl_or_cmd
                && (k == fltk::enums::Key::from_char('n')
                    || k == fltk::enums::Key::from_char('N')) =>
            {
                Some("File/Connect")
            }
            k if ctrl_or_cmd
                && (k == fltk::enums::Key::from_char('d')
                    || k == fltk::enums::Key::from_char('D')) =>
            {
                Some("File/Disconnect")
            }
            k if ctrl_or_cmd
                && (k == fltk::enums::Key::from_char('o')
                    || k == fltk::enums::Key::from_char('O')) =>
            {
                Some("File/Open SQL File")
            }
            k if ctrl_or_cmd
                && !shift
                && (k == fltk::enums::Key::from_char('s')
                    || k == fltk::enums::Key::from_char('S')) =>
            {
                Some("File/Save SQL File")
            }
            k if ctrl_or_cmd
                && shift
                && (k == fltk::enums::Key::from_char('s')
                    || k == fltk::enums::Key::from_char('S')) =>
            {
                Some("File/Save SQL File As")
            }
            k if ctrl_or_cmd
                && (k == fltk::enums::Key::from_char('q')
                    || k == fltk::enums::Key::from_char('Q')) =>
            {
                Some("File/Exit")
            }
            k if ctrl_or_cmd
                && shift
                && (k == fltk::enums::Key::from_char('z')
                    || k == fltk::enums::Key::from_char('Z')) =>
            {
                Some("Edit/Redo")
            }
            k if ctrl_or_cmd
                && (k == fltk::enums::Key::from_char('z')
                    || k == fltk::enums::Key::from_char('Z')) =>
            {
                Some("Edit/Undo")
            }
            k if ctrl_or_cmd
                && (k == fltk::enums::Key::from_char('y')
                    || k == fltk::enums::Key::from_char('Y')) =>
            {
                Some("Edit/Redo")
            }
            k if ctrl_or_cmd
                && (k == fltk::enums::Key::from_char('x')
                    || k == fltk::enums::Key::from_char('X')) =>
            {
                Some("Edit/Cut")
            }
            k if ctrl_or_cmd
                && shift
                && (k == fltk::enums::Key::from_char('c')
                    || k == fltk::enums::Key::from_char('C')) =>
            {
                Some("Edit/Copy with Headers")
            }
            k if ctrl_or_cmd
                && (k == fltk::enums::Key::from_char('c')
                    || k == fltk::enums::Key::from_char('C')) =>
            {
                Some("Edit/Copy")
            }
            k if ctrl_or_cmd
                && (k == fltk::enums::Key::from_char('v')
                    || k == fltk::enums::Key::from_char('V')) =>
            {
                Some("Edit/Paste")
            }
            k if ctrl_or_cmd
                && (k == fltk::enums::Key::from_char('a')
                    || k == fltk::enums::Key::from_char('A')) =>
            {
                Some("Edit/Select All")
            }
            fltk::enums::Key::F3 => Some("Edit/Find Next"),
            k if ctrl_or_cmd
                && (k == fltk::enums::Key::from_char('h')
                    || k == fltk::enums::Key::from_char('H')) =>
            {
                Some("Edit/Replace")
            }
            k if ctrl_or_cmd
                && shift
                && (k == fltk::enums::Key::from_char('f')
                    || k == fltk::enums::Key::from_char('F')) =>
            {
                Some("Edit/Format SQL")
            }
            k if ctrl_or_cmd
                && (k == fltk::enums::Key::from_char('f')
                    || k == fltk::enums::Key::from_char('F')) =>
            {
                Some("Edit/Find")
            }
            k if ctrl_or_cmd
                && (k == fltk::enums::Key::from_char('/')
                    || k == fltk::enums::Key::from_char('?')) =>
            {
                Some("Edit/Toggle Comment")
            }
            k if ctrl_or_cmd
                && (k == fltk::enums::Key::from_char('u')
                    || k == fltk::enums::Key::from_char('U')) =>
            {
                Some("Edit/Uppercase Selection")
            }
            k if ctrl_or_cmd
                && (k == fltk::enums::Key::from_char('l')
                    || k == fltk::enums::Key::from_char('L')) =>
            {
                Some("Edit/Lowercase Selection")
            }
            k if ctrl_or_cmd
                && (k == fltk::enums::Key::from_char(' ')
                    || k == fltk::enums::Key::from_char('\u{0020}')) =>
            {
                Some("Edit/Intellisense")
            }
            k if ctrl_or_cmd
                && (k == fltk::enums::Key::from_char('t')
                    || k == fltk::enums::Key::from_char('T')) =>
            {
                Some("File/New SQL File")
            }
            k if ctrl_or_cmd
                && (k == fltk::enums::Key::from_char('w')
                    || k == fltk::enums::Key::from_char('W')) =>
            {
                Some("File/Close SQL File")
            }
            fltk::enums::Key::F5 => Some("Query/Execute"),
            k if ctrl_or_cmd
                && (k == fltk::enums::Key::Enter || k == fltk::enums::Key::KPEnter) =>
            {
                Some("Query/Execute Statement")
            }
            fltk::enums::Key::F9 => Some("Query/Execute Statement (F9)"),
            fltk::enums::Key::F4 => Some("Query/Quick Describe"),
            fltk::enums::Key::F6 => Some("Query/Explain Plan"),
            fltk::enums::Key::F7 => Some("Query/Commit"),
            fltk::enums::Key::F8 => Some("Query/Rollback"),
            k if ctrl_or_cmd
                && (k == fltk::enums::Key::from_char('e')
                    || k == fltk::enums::Key::from_char('E')) =>
            {
                Some("Tools/Export Results")
            }
            _ => None,
        }
    }

    fn resolve_window_shortcut_action(
        event_key: fltk::enums::Key,
        event_original_key: fltk::enums::Key,
        event_state: fltk::enums::Shortcut,
    ) -> Option<&'static str> {
        Self::menu_shortcut_for_key(event_key, event_state)
            .or_else(|| Self::menu_shortcut_for_key(event_original_key, event_state))
    }

    fn handle_window_shortcut(
        state: &Arc<Mutex<AppState>>,
        schema_sender: &std::sync::mpsc::Sender<SchemaUpdate>,
        conn_sender: &std::sync::mpsc::Sender<ConnectionResult>,
        file_sender: &std::sync::mpsc::Sender<FileActionResult>,
    ) -> bool {
        let event_key = app::event_key();
        let event_original_key = app::event_original_key();
        let event_state = app::event_state();
        let Some(action) =
            Self::resolve_window_shortcut_action(event_key, event_original_key, event_state)
        else {
            return false;
        };
        Self::execute_menu_action(state, schema_sender, conn_sender, file_sender, action)
    }

    pub fn setup_callbacks(&mut self) {
        let state = self.state.clone();
        let (schema_sender, schema_receiver) = std::sync::mpsc::channel::<SchemaUpdate>();
        let (conn_sender, conn_receiver) = std::sync::mpsc::channel::<ConnectionResult>();
        let (file_sender, file_receiver) = std::sync::mpsc::channel::<FileActionResult>();

        {
            let weak_state_for_result_context = Arc::downgrade(&state);
            let file_sender_for_result_context = file_sender.clone();
            let callback = Arc::new(Mutex::new(Some(Box::new(
                move |action: ResultTableContextAction| {
                    let Some(state_for_result_context) = weak_state_for_result_context.upgrade()
                    else {
                        return;
                    };
                    match action {
                        ResultTableContextAction::ExportCsv => {
                            MainWindow::export_current_results_to_csv(
                                &state_for_result_context,
                                &file_sender_for_result_context,
                            );
                        }
                        ResultTableContextAction::Close => {
                            MainWindow::close_current_result_tab(&state_for_result_context);
                        }
                        ResultTableContextAction::CloseAll => {
                            MainWindow::close_all_result_tabs(&state_for_result_context);
                        }
                    }
                },
            )
                as Box<dyn FnMut(ResultTableContextAction)>)));
            state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .result_tabs
                .set_context_action_callback(callback);
        }

        let tab_ids: Vec<QueryTabId> = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .editor_tabs
            .iter()
            .map(|tab| tab.tab_id)
            .collect();
        for tab_id in tab_ids {
            Self::attach_editor_callbacks(&state, tab_id, schema_sender.clone());
        }

        let (mut object_browser, mut window) = {
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (s.object_browser.clone(), s.window.clone())
        };

        // Setup object browser callback
        let weak_state_for_browser_status = Arc::downgrade(&state);
        object_browser.set_status_callback(move |message| {
            let Some(state_for_status) = weak_state_for_browser_status.upgrade() else {
                return;
            };

            if let Ok(mut s) = state_for_status.try_lock() {
                let conn_info = s
                    .connection_info
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                s.status_bar.set_label(&format_status(message, &conn_info));
            };
        });

        let weak_state_for_browser_metadata = Arc::downgrade(&state);
        object_browser.set_metadata_callback(move |snapshot| {
            let Some(state_for_metadata) = weak_state_for_browser_metadata.upgrade() else {
                return;
            };

            let mut s = state_for_metadata
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !s.has_live_connection {
                return;
            }

            let connection = s.connection.clone();
            let current_generation = match try_lock_connection_with_activity(
                &connection,
                "Applying object browser metadata",
            ) {
                Some(conn_guard) => conn_guard.connection_generation(),
                None => {
                    s.pending_connection_metadata_refresh = true;
                    return;
                }
            };
            if snapshot.connection_generation != current_generation {
                return;
            }
            let current_scope = s.object_browser.selected_scope();
            if !MainWindow::schema_update_scope_matches(
                snapshot.db_type,
                snapshot.selected_scope.as_deref(),
                current_scope.as_deref(),
                &snapshot.available_scopes,
            ) {
                return;
            }

            MainWindow::apply_object_browser_metadata_snapshot(&mut s, snapshot);
        });

        let weak_state_for_browser = Arc::downgrade(&state);
        let schema_sender_for_browser = schema_sender.clone();
        let file_sender_for_browser = file_sender.clone();
        object_browser.set_sql_callback(move |action| {
            let Some(state_for_browser) = weak_state_for_browser.upgrade() else {
                return;
            };
            let mut created_tab_for_generated_sql: Option<QueryTabId> = None;
            let mut sql_to_execute: Option<String> = None;
            {
                let mut s = state_for_browser
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                match action {
                    SqlAction::Insert(text) => {
                        s.sql_editor.insert_text_at_cursor_position(&text);
                    }
                    SqlAction::OpenInNewTab(sql) => {
                        if let Some(tab_id) = MainWindow::create_query_editor_tab(&mut s) {
                            s.sql_buffer.set_text(&sql);
                            s.sql_editor.reset_undo_redo_history();
                            s.set_tab_file_path(tab_id, None);
                            s.set_tab_pristine_text(tab_id, sql);
                            s.sql_editor.focus();
                            s.right_tile.redraw();
                            created_tab_for_generated_sql = Some(tab_id);
                        }
                    }
                    SqlAction::Execute(sql) => {
                        sql_to_execute = Some(sql);
                    }
                    SqlAction::DisplayResult(request) => {
                        s.append_result_tab_request(request);
                    }
                }
            }

            if let Some(sql) = sql_to_execute {
                if let Some(editor) = acquire_sql_editor_if_idle(&state_for_browser) {
                    editor.execute_sql_text(&sql);
                }
            }

            if let Some(tab_id) = created_tab_for_generated_sql {
                MainWindow::attach_editor_callbacks(
                    &state_for_browser,
                    tab_id,
                    schema_sender_for_browser.clone(),
                );
                MainWindow::attach_file_drop_callback(
                    &state_for_browser,
                    tab_id,
                    file_sender_for_browser.clone(),
                );
                state_for_browser
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .sql_editor
                    .focus();
                app::redraw();
            }
        });

        let weak_state_for_scope_change = Arc::downgrade(&state);
        let schema_sender_for_scope_change = schema_sender.clone();
        object_browser.set_scope_change_callback(move || {
            let Some(state_for_scope_change) = weak_state_for_scope_change.upgrade() else {
                return;
            };

            let retained_scope_update = {
                let mut s = state_for_scope_change
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let connection_info_update = {
                    let connection = s.connection.clone();
                    let update = match try_lock_connection_with_activity(
                        &connection,
                        "Applying object browser scope change",
                    ) {
                        Some(conn_guard) if conn_guard.is_connected() => {
                            Some(Some(conn_guard.get_info().clone()))
                        }
                        Some(_) => Some(None),
                        None => {
                            s.pending_connection_metadata_refresh = true;
                            None
                        }
                    };
                    update
                };
                if let Some(connection_info) = connection_info_update {
                    *s.connection_info
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = connection_info;
                }
                let selected_scope = s.object_browser.selected_scope();
                let retained_scope_update = s.retained_scope_update(selected_scope);
                let started = MainWindow::start_connection_metadata_refresh_for_scope_change(
                    &mut s,
                    &schema_sender_for_scope_change,
                );
                s.update_pending_metadata_refresh_after_start_attempt(started);
                retained_scope_update
            };

            if let Some(message) = retained_scope_update
                .map(apply_retained_scope_update)
                .and_then(|outcomes| first_retained_outcome_message(&outcomes))
            {
                crate::ui::alert_on_main(&format!(
                    "Scope was changed, but a retained session could not be updated: \n{}",
                    message
                ));
            }
        });

        let weak_state_for_scope_preflight = Arc::downgrade(&state);
        object_browser.set_scope_switch_preflight_callback(move || {
            let Some(state_for_scope_preflight) = weak_state_for_scope_preflight.upgrade() else {
                return Ok(());
            };
            let s = state_for_scope_preflight
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(message) = s.retained_scope_change_blocker() {
                Err(message)
            } else {
                Ok(())
            }
        });

        let weak_state_for_window = Arc::downgrade(&state);
        let schema_sender_for_window = schema_sender.clone();
        let conn_sender_for_window = conn_sender.clone();
        let file_sender_for_window = file_sender.clone();
        window.handle(move |_w, ev| {
            let Some(state_for_window) = weak_state_for_window.upgrade() else {
                return false;
            };
            match ev {
                fltk::enums::Event::Resize
                | fltk::enums::Event::Hide
                | fltk::enums::Event::Fullscreen => {
                    if let Ok(s) = state_for_window.try_lock() {
                        s.hide_all_intellisense_popups();
                    }
                    false
                }
                fltk::enums::Event::Deactivate => {
                    // A genuine deactivate (app switch) must hide the popups,
                    // but the completion popup window becoming macOS's key
                    // window can also deactivate the main window for a moment.
                    // Hide the signature hint only after focus settles; the
                    // completion popups keep their existing synchronous hide.
                    if let Ok(s) = state_for_window.try_lock() {
                        s.sql_editor.try_hide_intellisense_popup();
                        s.sql_editor.hide_signature_popup_after_focus_settles();
                        for tab in &s.editor_tabs {
                            tab.sql_editor.try_hide_intellisense_popup();
                            tab.sql_editor.hide_signature_popup_after_focus_settles();
                        }
                    }
                    false
                }
                fltk::enums::Event::KeyDown => {
                    if app::event_key() == fltk::enums::Key::Escape {
                        return true;
                    }
                    if MainWindow::handle_window_shortcut(
                        &state_for_window,
                        &schema_sender_for_window,
                        &conn_sender_for_window,
                        &file_sender_for_window,
                    ) {
                        return true;
                    }
                    false
                }
                fltk::enums::Event::Shortcut => {
                    if MainWindow::handle_window_shortcut(
                        &state_for_window,
                        &schema_sender_for_window,
                        &conn_sender_for_window,
                        &file_sender_for_window,
                    ) {
                        return true;
                    }
                    false
                }
                fltk::enums::Event::Push => {
                    let (sql_editor, object_browser) = {
                        let s = state_for_window
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        (s.sql_editor.clone(), s.object_browser.clone())
                    };
                    object_browser.hide_scope_selector_popup_if_outside(
                        app::event_x_root(),
                        app::event_y_root(),
                    );
                    sql_editor.hide_signature_popup();
                    sql_editor.hide_intellisense_on_outside_click(
                        app::event_x_root(),
                        app::event_y_root(),
                    );
                    false
                }
                _ => false,
            }
        });

        self.setup_menu_callbacks(
            schema_sender,
            schema_receiver,
            conn_sender,
            conn_receiver,
            file_sender,
            file_receiver,
        );
    }

    fn setup_menu_callbacks(
        &mut self,
        schema_sender: std::sync::mpsc::Sender<SchemaUpdate>,
        schema_receiver: std::sync::mpsc::Receiver<SchemaUpdate>,
        conn_sender: std::sync::mpsc::Sender<ConnectionResult>,
        conn_receiver: std::sync::mpsc::Receiver<ConnectionResult>,
        file_sender: std::sync::mpsc::Sender<FileActionResult>,
        file_receiver: std::sync::mpsc::Receiver<FileActionResult>,
    ) {
        let state = self.state.clone();

        // Wrap receivers in Arc<Mutex> to share across timeout callbacks
        let schema_receiver: Arc<Mutex<std::sync::mpsc::Receiver<SchemaUpdate>>> =
            Arc::new(Mutex::new(schema_receiver));
        let conn_receiver: Arc<Mutex<std::sync::mpsc::Receiver<ConnectionResult>>> =
            Arc::new(Mutex::new(conn_receiver));
        let file_receiver: Arc<Mutex<std::sync::mpsc::Receiver<FileActionResult>>> =
            Arc::new(Mutex::new(file_receiver));
        let idle_poll_cycles = Arc::new(AtomicUsize::new(0));

        const CHANNEL_POLL_ACTIVE_INTERVAL_SECONDS: f64 = 0.05;
        const CHANNEL_POLL_IDLE_INTERVAL_SECONDS: f64 = 0.25;
        const MEMORY_TRIM_IDLE_CYCLE_THRESHOLD: usize =
            safe_div_f64_to_usize(60.0, CHANNEL_POLL_IDLE_INTERVAL_SECONDS);

        fn schedule_poll(
            schema_receiver: Arc<Mutex<std::sync::mpsc::Receiver<SchemaUpdate>>>,
            conn_receiver: Arc<Mutex<std::sync::mpsc::Receiver<ConnectionResult>>>,
            file_receiver: Arc<Mutex<std::sync::mpsc::Receiver<FileActionResult>>>,
            state_weak: std::sync::Weak<Mutex<AppState>>,
            schema_sender: std::sync::mpsc::Sender<SchemaUpdate>,
            file_sender: std::sync::mpsc::Sender<FileActionResult>,
            idle_poll_cycles: Arc<AtomicUsize>,
            pending_schema_update: Option<SchemaUpdate>,
        ) {
            let Some(state) = state_weak.upgrade() else {
                return;
            };
            let mut schema_disconnected = false;
            let mut conn_disconnected = false;
            let mut file_disconnected = false;
            let mut deferred_by_borrow_conflict = false;
            let mut processed_message = false;
            let mut pending_schema_update = pending_schema_update;

            // Check for schema updates
            {
                let r = schema_receiver
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let (current_generation, current_scope) = match state.try_lock() {
                    Ok(s) => {
                        let current_scope = s.object_browser.selected_scope();
                        let guard = try_lock_connection_with_activity(
                            &s.connection,
                            "Checking schema update generation",
                        );
                        match guard {
                            Some(connection_guard) => {
                                (connection_guard.connection_generation(), current_scope)
                            }
                            None => {
                                deferred_by_borrow_conflict = true;
                                (0, current_scope)
                            }
                        }
                    }
                    Err(_) => {
                        deferred_by_borrow_conflict = true;
                        (0, None)
                    }
                };

                if !deferred_by_borrow_conflict {
                    let mut latest_update = pending_schema_update.take().filter(|update| {
                        update.connection_generation == current_generation
                            && MainWindow::schema_update_scope_matches(
                                update.db_type,
                                update.selected_scope.as_deref(),
                                current_scope.as_deref(),
                                &update.data.users,
                            )
                    });
                    loop {
                        match r.try_recv() {
                            Ok(update) => {
                                if update.connection_generation != current_generation {
                                    continue;
                                }
                                if !MainWindow::schema_update_scope_matches(
                                    update.db_type,
                                    update.selected_scope.as_deref(),
                                    current_scope.as_deref(),
                                    &update.data.users,
                                ) {
                                    continue;
                                }
                                latest_update = Some(update);
                                processed_message = true;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => break,
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                schema_disconnected = true;
                                break;
                            }
                        }
                    }

                    if let Some(update) = latest_update {
                        match state.try_lock() {
                            Ok(mut s) => {
                                MainWindow::update_schema_snapshot(
                                    &mut s,
                                    update.data,
                                    update.highlight_data,
                                );
                            }
                            Err(_) => {
                                pending_schema_update = Some(update);
                                deferred_by_borrow_conflict = true;
                            }
                        }
                    }
                }
            }

            if !deferred_by_borrow_conflict {
                match state.try_lock() {
                    Ok(mut s) => {
                        if s.pending_connection_metadata_refresh
                            && s.has_live_connection
                            && s.progress_contexts.is_empty()
                        {
                            let started = MainWindow::start_connection_metadata_refresh(
                                &mut s,
                                &schema_sender,
                            );
                            s.update_pending_metadata_refresh_after_start_attempt(started);
                            if started {
                                processed_message = true;
                            }
                        }
                    }
                    Err(_) => {
                        deferred_by_borrow_conflict = true;
                    }
                }
            }

            // Check for connection results
            {
                let r = conn_receiver
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                loop {
                    let Ok(mut s) = state.try_lock() else {
                        deferred_by_borrow_conflict = true;
                        break;
                    };
                    match r.try_recv() {
                        Ok(result) => {
                            processed_message = true;
                            match result {
                                ConnectionResult::Success(info) => {
                                    let info = *info;
                                    crate::utils::logging::log_info(
                                        "connection",
                                        &format!("Connected to {} ({})", info.name, info.db_type),
                                    );
                                    clear_mutex_flag(&s.schema_refresh_in_progress);
                                    s.release_all_pooled_db_sessions();
                                    // Drop the previous connection's schema snapshot before
                                    // touching editors. set_db_type below triggers a rehighlight,
                                    // and without this clear the highlighter would paint with the
                                    // prior DB's tables/columns until the async metadata refresh
                                    // finishes.
                                    MainWindow::update_schema_snapshot(
                                        &mut s,
                                        IntellisenseData::new(),
                                        HighlightData::new(),
                                    );
                                    for tab in &s.editor_tabs {
                                        tab.sql_editor.set_db_type(info.db_type);
                                    }
                                    s.sql_editor.set_db_type(info.db_type);
                                    *s.connection_info
                                        .lock()
                                        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                        Some(info.clone());
                                    s.has_live_connection = true;
                                    s.pending_connection_metadata_refresh = false;
                                    s.object_browser.reset_selected_scope();
                                    s.status_bar.set_label(&format!(
                                        "Connected | {} ({})",
                                        info.name, info.db_type
                                    ));
                                    s.sync_transaction_mode_controls_for_connected_db(info.db_type);
                                    s.sql_editor.focus();
                                    s.refresh_connection_dependent_controls();
                                    s.sync_transaction_mode_controls();
                                    let started = MainWindow::start_connection_metadata_refresh(
                                        &mut s,
                                        &schema_sender,
                                    );
                                    s.update_pending_metadata_refresh_after_start_attempt(started);
                                }
                                ConnectionResult::Failure(err) => {
                                    let current_connection = s
                                        .connection_info
                                        .lock()
                                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                                        .clone();
                                    let current_connection_label =
                                        current_connection.as_ref().map(|info| info.name.clone());

                                    if let Some(current_label) = current_connection_label {
                                        crate::utils::logging::log_error(
                                            "connection",
                                            &format!(
                                                "Connection failed: {} (keeping current connection: {})",
                                                err, current_label
                                            ),
                                        );
                                        s.status_bar.set_label(&format_status(
                                            "Connection failed; keeping current connection",
                                            &current_connection,
                                        ));
                                        let lines = vec![
                                            format!("Connection failed: {}", err),
                                            format!(
                                                "Keeping current connection: {}",
                                                current_label
                                            ),
                                        ];
                                        s.result_tabs
                                            .append_message_lines(ResultMessageKind::Error, &lines);
                                    } else {
                                        crate::utils::logging::log_error(
                                            "connection",
                                            &format!("Connection failed: {}", err),
                                        );
                                        s.status_bar.set_label("Connection failed");
                                        s.result_tabs.append_message_lines(
                                            ResultMessageKind::Error,
                                            &[format!("Connection failed: {}", err)],
                                        );
                                    }
                                    s.result_tabs.select_messages_errors();
                                }
                            }
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            conn_disconnected = true;
                            break;
                        }
                    }
                }
            }

            // Check for file operations
            {
                let r = file_receiver
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let mut deferred_alert: Option<String> = None;
                loop {
                    let Ok(mut s) = state.try_lock() else {
                        deferred_by_borrow_conflict = true;
                        break;
                    };
                    match r.try_recv() {
                        Ok(result) => {
                            processed_message = true;
                            let mut created_tab_for_open: Option<QueryTabId> = None;
                            let mut created_editor_for_open: Option<SqlEditorWidget> = None;
                            let mut created_right_tile_for_open: Option<Tile> = None;
                            match result {
                                FileActionResult::OpenInNewTab { path, result } => match result {
                                    Ok(content) => {
                                        if MainWindow::focus_existing_tab_with_same_file_path(
                                            &mut s, &path,
                                        ) {
                                            MainWindow::record_recent_sql_file(&mut s, &path);
                                            continue;
                                        }
                                        let normalized_content =
                                            MainWindow::normalize_line_endings_for_editor(content);
                                        if let Some(tab_id) =
                                            MainWindow::create_query_editor_tab(&mut s)
                                        {
                                            s.sql_buffer.set_text(&normalized_content);
                                            s.sql_editor.reset_undo_redo_history();
                                            s.set_tab_file_path(tab_id, Some(path.clone()));
                                            s.set_tab_pristine_text(tab_id, normalized_content);
                                            created_editor_for_open = Some(s.sql_editor.clone());
                                            created_right_tile_for_open =
                                                Some(s.right_tile.clone());
                                            created_tab_for_open = Some(tab_id);
                                            MainWindow::record_recent_sql_file(&mut s, &path);
                                        }
                                    }
                                    Err(err) => {
                                        deferred_alert =
                                            Some(format!("Failed to open SQL file: {}", err));
                                    }
                                },
                                FileActionResult::Export {
                                    path,
                                    row_count,
                                    result,
                                } => match result {
                                    Ok(()) => {
                                        let file_label =
                                            path.file_name().unwrap_or_default().to_string_lossy();
                                        let conn_info = s
                                            .connection_info
                                            .lock()
                                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                                            .clone();
                                        s.status_bar.set_label(&format_status(
                                            &format!(
                                                "Exported {} rows to {}",
                                                row_count, file_label
                                            ),
                                            &conn_info,
                                        ));
                                    }
                                    Err(err) => {
                                        deferred_alert =
                                            Some(format!("Failed to export CSV: {}", err));
                                    }
                                },
                            }

                            drop(s);

                            if let Some(alert_msg) = deferred_alert.take() {
                                crate::ui::alert_on_main(&alert_msg);
                            }

                            if let Some(tab_id) = created_tab_for_open {
                                MainWindow::attach_editor_callbacks(
                                    &state,
                                    tab_id,
                                    schema_sender.clone(),
                                );
                                MainWindow::attach_file_drop_callback(
                                    &state,
                                    tab_id,
                                    file_sender.clone(),
                                );
                                if let Some(mut editor) = created_editor_for_open {
                                    editor.focus();
                                }
                                if let Some(mut right_tile) = created_right_tile_for_open {
                                    right_tile.redraw();
                                }
                                app::redraw();
                            }
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            file_disconnected = true;
                            break;
                        }
                    }
                }
            }

            if deferred_by_borrow_conflict {
                crate::ui::ui_timeout::schedule(CHANNEL_POLL_ACTIVE_INTERVAL_SECONDS, move || {
                    schedule_poll(
                        schema_receiver.clone(),
                        conn_receiver.clone(),
                        file_receiver.clone(),
                        state_weak.clone(),
                        schema_sender.clone(),
                        file_sender.clone(),
                        idle_poll_cycles.clone(),
                        pending_schema_update,
                    );
                });
                return;
            }

            // Stop polling if all channels are disconnected
            if schema_disconnected && conn_disconnected && file_disconnected {
                return;
            }

            let delay = if processed_message {
                idle_poll_cycles.store(0, Ordering::Relaxed);
                CHANNEL_POLL_ACTIVE_INTERVAL_SECONDS
            } else {
                let idle_cycles = idle_poll_cycles
                    .fetch_add(1, Ordering::Relaxed)
                    .saturating_add(1);
                if idle_cycles >= MEMORY_TRIM_IDLE_CYCLE_THRESHOLD {
                    idle_poll_cycles.store(0, Ordering::Relaxed);
                    malloc_trim_process();
                }
                CHANNEL_POLL_IDLE_INTERVAL_SECONDS
            };

            // Reschedule for next poll
            crate::ui::ui_timeout::schedule(delay, move || {
                schedule_poll(
                    schema_receiver.clone(),
                    conn_receiver.clone(),
                    file_receiver.clone(),
                    state_weak.clone(),
                    schema_sender.clone(),
                    file_sender.clone(),
                    idle_poll_cycles.clone(),
                    pending_schema_update,
                );
            });
        }

        // Start polling
        let weak_state_for_poll = Arc::downgrade(&state);
        let schema_sender_for_poll = schema_sender.clone();
        {
            let mut s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            s.schema_sender = Some(schema_sender.clone());
            s.file_sender = Some(file_sender.clone());
        }
        schedule_poll(
            schema_receiver,
            conn_receiver,
            file_receiver,
            weak_state_for_poll,
            schema_sender_for_poll,
            file_sender.clone(),
            idle_poll_cycles,
            None,
        );

        let tab_ids_for_drop: Vec<QueryTabId> = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .editor_tabs
            .iter()
            .map(|tab| tab.tab_id)
            .collect();
        for tab_id in tab_ids_for_drop {
            Self::attach_file_drop_callback(&state, tab_id, file_sender.clone());
        }

        if let Some(mut menu) = app::widget_from_id::<MenuBar>("main_menu") {
            let weak_state_for_menu = Arc::downgrade(&state);
            let schema_sender_for_menu = schema_sender;
            let conn_sender_for_menu = conn_sender;
            let file_sender_for_menu = file_sender;
            menu.set_callback(move |m| {
                let Some(state_for_menu) = weak_state_for_menu.upgrade() else {
                    return;
                };
                let recent_sql_file_choice_map = {
                    let s = state_for_menu
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let config = s
                        .config
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    MenuBarBuilder::recent_sql_file_choice_map(&config.recent_sql_files)
                };
                let menu_path = m.item_pathname(None).ok().or_else(|| m.choice());
                if let Some(path) = menu_path {
                    let raw_choice = path.split('\t').next().unwrap_or(&path).trim();
                    let choice = MainWindow::strip_menu_label_shortcut(&path);
                    if let Some(path) = recent_sql_file_choice_map
                        .get(raw_choice)
                        .or_else(|| recent_sql_file_choice_map.get(&choice))
                        .cloned()
                    {
                        let state = state_for_menu.clone();
                        let schema_sender = schema_sender_for_menu.clone();
                        let file_sender = file_sender_for_menu.clone();
                        crate::ui::ui_timeout::schedule(0.0, move || {
                            MainWindow::open_recent_sql_file_path(
                                &state,
                                &schema_sender,
                                &file_sender,
                                path.clone(),
                            );
                        });
                        m.set_value(-1);
                    } else if MainWindow::execute_menu_action(
                        &state_for_menu,
                        &schema_sender_for_menu,
                        &conn_sender_for_menu,
                        &file_sender_for_menu,
                        &choice,
                    ) {
                        // FLTK keeps the last activated menu item selected. When the selection
                        // doesn't change, repeated keyboard shortcuts for the same item may not
                        // trigger again. Clear the current value so Ctrl+N/Ctrl+S can fire
                        // repeatedly without requiring a different shortcut in between.
                        m.set_value(-1);
                    }
                }
            });
        }
    }

    fn continue_application_exit(state: Arc<Mutex<AppState>>, window: Window, check_dirty: bool) {
        if check_dirty && !Self::confirm_save_for_all_dirty_tabs(&state) {
            return;
        }

        let has_running_work = {
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            s.has_running_query_or_lazy_fetch()
        };
        if has_running_work {
            if !Self::confirm_cancel_running_query_for_exit(&state) {
                return;
            }
            Self::cancel_all_running_queries(&state);
            Self::defer_application_exit_until_idle(state, window);
            return;
        }

        if !Self::resolve_pooled_sessions_before_exit(&state) {
            return;
        }

        Self::finish_application_exit(&state, window);
    }

    fn defer_application_exit_until_idle(state: Arc<Mutex<AppState>>, window: Window) {
        crate::ui::ui_timeout::schedule(0.2, move || {
            let should_wait = {
                let s = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                s.has_running_query_or_lazy_fetch()
            };
            if should_wait {
                Self::defer_application_exit_until_idle(state.clone(), window.clone());
                return;
            }
            Self::continue_application_exit(state.clone(), window.clone(), false);
        });
    }

    fn finish_application_exit(state: &Arc<Mutex<AppState>>, mut window: Window) {
        crate::db::clear_tracked_db_activity();
        let (popups, editor_tabs, mut result_tabs) = {
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (
                s.popups.clone(),
                s.editor_tabs.clone(),
                s.result_tabs.clone(),
            )
        };
        let mut popups = popups
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for mut popup in popups.drain(..) {
            if popup.was_deleted() {
                continue;
            }
            popup.hide();
            Window::delete(popup);
        }
        for mut tab in editor_tabs {
            tab.sql_editor.cleanup_for_close();
        }
        result_tabs.clear();
        crate::ui::sql_editor::SqlEditorWidget::shutdown_column_load_workers();
        if let Err(err) = crate::utils::logging::flush_log_writer() {
            eprintln!("Application log flush on exit failed: {err}");
        }
        window.hide();
        app::quit();
    }

    pub fn show(&mut self) {
        let state = self.state.clone();
        let mut window = {
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            s.window.clone()
        };
        let weak_state_for_close = Arc::downgrade(&state);
        window.set_callback(move |w| {
            if let Some(state) = weak_state_for_close.upgrade() {
                MainWindow::continue_application_exit(state, w.clone(), true);
            } else {
                crate::ui::sql_editor::SqlEditorWidget::shutdown_column_load_workers();
                if let Err(err) = crate::utils::logging::flush_log_writer() {
                    eprintln!("Application log flush on exit failed: {err}");
                }
                w.hide();
                app::quit();
            }
        });
        window.show();
        app::flush();
        let _ = app::wait();
        crate::db::clear_tracked_db_activity();
        {
            let mut s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            MainWindow::adjust_query_layout(&mut s);
            s.window.redraw();
            s.sql_editor.focus();
        }
    }

    #[doc(hidden)]
    pub fn capture_tour_set_sql(
        &mut self,
        sql: &str,
        cursor: Option<i32>,
    ) -> fltk::text::TextEditor {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.sql_editor.set_text(sql);
        let mut editor = state.sql_editor.get_editor();
        let position = cursor.unwrap_or_else(|| state.sql_buffer.length());
        editor.set_insert_position(position.clamp(0, state.sql_buffer.length()));
        editor.show_insert_position();
        state.sql_editor.focus();
        state.window.redraw();
        editor
    }

    #[doc(hidden)]
    pub fn capture_tour_format_sql(&mut self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut buffer = state.sql_editor.get_buffer();
        buffer.select(0, buffer.length());
        state.sql_editor.format_selected_sql();
        buffer.unselect();
        let mut editor = state.sql_editor.get_editor();
        editor.set_insert_position(0);
        editor.show_insert_position();
        state.window.redraw();
    }

    #[doc(hidden)]
    pub fn capture_tour_show_object_browser(&mut self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.object_browser.capture_tour_set_example_metadata();
        state
            .status_bar
            .set_label("Connected | Local Oracle (Oracle)");
        state.window.redraw();
    }

    #[doc(hidden)]
    pub fn capture_tour_show_result(
        &mut self,
        label: &str,
        result: crate::db::QueryResult,
        enable_editing: bool,
        selection: Option<(i32, i32, i32, i32)>,
    ) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.result_tabs.clear();
        state.append_result_tab_request(ResultTabRequest {
            label: label.to_string(),
            result,
        });
        if enable_editing {
            let mut result_tabs = state.result_tabs.clone();
            result_tabs.begin_current_edit_mode()?;
        }
        if let Some((row_start, col_start, row_end, col_end)) = selection {
            state
                .result_tabs
                .capture_tour_select_range(row_start, col_start, row_end, col_end);
        }
        state.refresh_result_edit_controls();
        state.window.redraw();
        Ok(())
    }

    pub fn show_previous_crash_report(crash_report: &str) {
        crate::utils::logging::log_warning(
            "app",
            "Previous session ended with a crash. Crash report was shown to user.",
        );
        let crash_message = format!(
            "The previous session ended unexpectedly.

{}

The crash has been recorded in the application log.",
            crash_report
        );
        SqlEditorWidget::show_quick_describe_text_dialog(
            "Previous Session Crash Report",
            &crash_message,
        );
    }

    pub fn run() {
        let app = app::App::default()
            .with_scheme(app::Scheme::Gtk)
            .load_system_fonts();
        let config = AppConfig::load();
        crate::app::configure_fltk_globals(&config);

        let current_group = fltk::group::Group::try_current();

        fltk::group::Group::set_current(None::<&fltk::group::Group>);

        let mut main_window = MainWindow::new_with_config(config);
        main_window.setup_callbacks();
        main_window.show();

        // Check for crash log from a previous session
        if let Some(crash_report) = crate::utils::logging::take_crash_log() {
            Self::show_previous_crash_report(&crash_report);
        }

        match app.run() {
            Ok(()) => {}
            Err(err) => {
                crate::utils::logging::log_error("app", &format!("App run error: {err}"));
                eprintln!("Failed to run app: {err}");
            }
        }
        // Restore current group
        if let Some(ref group) = current_group {
            fltk::group::Group::set_current(Some(group));
        }
    }

    #[allow(dead_code)]
    fn export_results_csv(
        path: &PathBuf,
        result: &QueryResult,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut output = String::new();

        let headers: Vec<String> = result.columns.iter().map(|c| c.name.clone()).collect();
        output.push_str(&Self::csv_row(&headers));
        output.push('\n');

        for row in &result.rows {
            output.push_str(&Self::csv_row(row));
            output.push('\n');
        }

        match fs::write(path, output) {
            Ok(()) => {}
            Err(err) => {
                eprintln!("CSV export error: {err}");
                return Err(Box::new(err));
            }
        }
        Ok(())
    }

    #[allow(dead_code)]
    fn csv_row(values: &[String]) -> String {
        values
            .iter()
            .map(|value| Self::csv_escape(value))
            .collect::<Vec<String>>()
            .join(",")
    }

    #[allow(dead_code)]
    fn csv_escape(value: &str) -> String {
        if value.contains(',') || value.contains('"') || value.contains('\n') {
            format!("\"{}\"", value.replace('"', "\"\""))
        } else {
            value.to_string()
        }
    }

    #[allow(dead_code)]
    fn format_query_history(history: &QueryHistory) -> String {
        if history.queries.is_empty() {
            return "No query history yet.".to_string();
        }

        let mut lines = vec!["Recent Queries (latest first):".to_string()];
        for entry in history.queries.iter().take(20) {
            lines.push(format!(
                "[{}] {} | {} ms | {} rows",
                entry.timestamp, entry.connection_name, entry.execution_time_ms, entry.row_count
            ));
            lines.push(entry.sql.trim().to_string());
            lines.push(String::new());
        }

        lines.join("\n")
    }

    fn normalize_line_endings_for_editor(mut text: String) -> String {
        if !text.contains('\r') {
            return text;
        }

        text = text.replace("\r\n", "\n");
        text.replace('\r', "\n")
    }
}

impl Default for MainWindow {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::configure_fltk_globals;
    use crate::ui::result_table::LazyFetchCallback;
    use crate::ui::sql_editor::LazyFetchRequest;
    use fltk::enums::{Key, Shortcut};
    use std::sync::{Arc, Mutex};

    #[test]
    fn equal_length_paste_avoids_full_dirty_scan_when_local_bytes_decide_state() {
        let pristine = "SELECT employee_name FROM employees;";
        let start = pristine.find("employee_name").expect("column name");

        assert_eq!(
            AppState::dirty_state_from_equal_length_local_edit(
                pristine,
                false,
                start,
                "employee_name",
            ),
            Some(false)
        );
        assert_eq!(
            AppState::dirty_state_from_equal_length_local_edit(
                pristine,
                false,
                start,
                "department_id",
            ),
            Some(true)
        );
        assert_eq!(
            AppState::dirty_state_from_equal_length_local_edit(
                pristine,
                true,
                start,
                "department_id",
            ),
            Some(true)
        );
    }

    #[test]
    fn equal_length_edit_compares_shadow_only_when_it_may_restore_pristine_text() {
        assert_eq!(
            AppState::dirty_state_from_equal_length_local_edit("SELECT 1;", true, 7, "1"),
            None
        );
    }

    #[test]
    fn resolve_window_shortcut_prefers_current_key_match() {
        let action = MainWindow::resolve_window_shortcut_action(
            Key::from_char('f'),
            Key::from_char('x'),
            Shortcut::Ctrl,
        );

        assert_eq!(action, Some("Edit/Find"));
    }

    #[test]
    fn resolve_window_shortcut_uses_original_key_for_non_ascii_layout() {
        let action = MainWindow::resolve_window_shortcut_action(
            Key::from_char('ㄹ'),
            Key::from_char('f'),
            Shortcut::Ctrl,
        );

        assert_eq!(action, Some("Edit/Find"));
    }

    #[test]
    fn normalize_line_endings_for_editor_converts_crlf_and_cr_to_lf() {
        let text = String::from("select 1;\r\nselect 2;\rselect 3;");
        let normalized = MainWindow::normalize_line_endings_for_editor(text);

        assert_eq!(normalized, "select 1;\nselect 2;\nselect 3;");
    }

    #[test]
    fn result_toolbar_checkbox_width_tracks_measured_label_width() {
        let base_width = BUTTON_WIDTH_LARGE + 45;

        assert_eq!(
            result_toolbar_checkbox_width_for_label(1, base_width),
            base_width
        );
        assert!(result_toolbar_checkbox_width_for_label(base_width, base_width) > base_width);
    }

    #[test]
    fn oracle_metadata_maps_to_intellisense_and_highlight_payloads() {
        let mut schema_objects = HashMap::new();
        schema_objects.insert(
            "SYSTEM".to_string(),
            vec![
                ("OQT_THIN_META_T".to_string(), "TABLE".to_string()),
                ("OQT_THIN_META_V".to_string(), "VIEW".to_string()),
                ("OQT_THIN_META_P".to_string(), "PACKAGE".to_string()),
                ("OQT_THIN_META_S".to_string(), "SYNONYM".to_string()),
                ("OQT_THIN_META_DIR".to_string(), "DIRECTORY".to_string()),
                ("OQT_THIN_META_LIB".to_string(), "LIBRARY".to_string()),
                ("OQT_THIN_META_SRC".to_string(), "JAVA SOURCE".to_string()),
            ],
        );
        schema_objects.insert(
            "PUBLIC".to_string(),
            vec![("DUAL".to_string(), "PUBLIC SYNONYM".to_string())],
        );
        let mut relation_members = HashMap::new();
        relation_members.insert(
            "SYSTEM".to_string(),
            vec!["OQT_THIN_META_T".to_string(), "OQT_THIN_META_V".to_string()],
        );

        let mut data = IntellisenseData::new();
        data.users = vec!["SYSTEM".to_string()];
        data.set_default_qualifier(Some("SYSTEM".to_string()));
        apply_schema_objects_to_intellisense(&mut data, &schema_objects);
        apply_relation_members_to_intellisense(&mut data, &relation_members);
        apply_selected_scope_objects_to_intellisense(
            &mut data,
            &schema_objects,
            Some("SYSTEM"),
            crate::db::DatabaseType::Oracle,
        );
        apply_public_synonyms_to_intellisense(&mut data, &schema_objects);

        assert!(data.tables.contains(&"OQT_THIN_META_T".to_string()));
        assert!(data.views.contains(&"OQT_THIN_META_V".to_string()));
        assert!(data.packages.contains(&"OQT_THIN_META_P".to_string()));
        assert!(data.synonyms.contains(&"OQT_THIN_META_S".to_string()));
        assert!(data.directories.contains(&"OQT_THIN_META_DIR".to_string()));
        assert!(data.libraries.contains(&"OQT_THIN_META_LIB".to_string()));
        assert!(data.java_sources.contains(&"OQT_THIN_META_SRC".to_string()));
        assert!(data.public_synonyms.contains(&"DUAL".to_string()));

        let mut highlight_data = HighlightData::new();
        highlight_data.tables = data.tables.clone();
        highlight_data.views = data.views.clone();
        highlight_data.packages = data.packages.clone();
        highlight_data.synonyms = data.synonyms.clone();
        highlight_data.public_synonyms = data.public_synonyms.clone();
        highlight_data.schemas = data.users.clone();

        assert!(highlight_data
            .tables
            .contains(&"OQT_THIN_META_T".to_string()));
        assert!(highlight_data.schemas.contains(&"SYSTEM".to_string()));
    }

    #[test]
    fn mysql_metadata_selected_scope_matches_schema_case_insensitively() {
        let mut schema_objects = HashMap::new();
        schema_objects.insert(
            "SalesDb".to_string(),
            vec![
                ("OrderLine".to_string(), "TABLE".to_string()),
                ("OrderLineView".to_string(), "VIEW".to_string()),
                ("RunBilling".to_string(), "PROCEDURE".to_string()),
                ("CalcTotal".to_string(), "FUNCTION".to_string()),
            ],
        );

        let mut data = IntellisenseData::new();
        data.users = vec!["SalesDb".to_string()];
        data.set_default_qualifier(Some("salesdb".to_string()));
        apply_schema_objects_to_intellisense(&mut data, &schema_objects);
        apply_selected_scope_objects_to_intellisense(
            &mut data,
            &schema_objects,
            Some("salesdb"),
            crate::db::DatabaseType::MySQL,
        );

        assert!(data.tables.contains(&"OrderLine".to_string()));
        assert!(data.views.contains(&"OrderLineView".to_string()));
        assert!(data.procedures.contains(&"RunBilling".to_string()));
        assert!(data.functions.contains(&"CalcTotal".to_string()));
    }

    #[test]
    fn selected_scope_objects_preserve_oracle_case_sensitivity() {
        let mut schema_objects = HashMap::new();
        schema_objects.insert(
            "MixedCase".to_string(),
            vec![("QuotedOnly".to_string(), "TABLE".to_string())],
        );

        let mut data = IntellisenseData::new();
        apply_schema_objects_to_intellisense(&mut data, &schema_objects);
        apply_selected_scope_objects_to_intellisense(
            &mut data,
            &schema_objects,
            Some("mixedcase"),
            crate::db::DatabaseType::Oracle,
        );

        assert!(!data.tables.contains(&"QuotedOnly".to_string()));
    }

    #[test]
    fn mysql_metadata_ambiguous_case_scope_does_not_choose_arbitrary_schema() {
        let mut schema_objects = HashMap::new();
        schema_objects.insert(
            "SalesDb".to_string(),
            vec![("UpperOrderLine".to_string(), "TABLE".to_string())],
        );
        schema_objects.insert(
            "salesdb".to_string(),
            vec![("LowerOrderLine".to_string(), "TABLE".to_string())],
        );

        let mut data = IntellisenseData::new();
        apply_schema_objects_to_intellisense(&mut data, &schema_objects);
        apply_selected_scope_objects_to_intellisense(
            &mut data,
            &schema_objects,
            Some("SALESDB"),
            crate::db::DatabaseType::MySQL,
        );

        assert!(data.tables.is_empty());
    }

    #[test]
    fn canonical_intellisense_scope_uses_catalog_schema_case() {
        let mut data = IntellisenseData::new();
        data.users = vec!["SalesDb".to_string(), "ArchiveDb".to_string()];

        assert_eq!(
            canonical_intellisense_scope(
                &data,
                Some("salesdb".to_string()),
                crate::db::DatabaseType::MySQL,
            ),
            Some("SalesDb".to_string())
        );
        assert_eq!(
            canonical_intellisense_scope(
                &data,
                Some("  archivedb  ".to_string()),
                crate::db::DatabaseType::MariaDB,
            ),
            Some("ArchiveDb".to_string())
        );
    }

    #[test]
    fn canonical_intellisense_scope_preserves_oracle_scope_case() {
        let mut data = IntellisenseData::new();
        data.users = vec!["MixedCase".to_string()];

        assert_eq!(
            canonical_intellisense_scope(
                &data,
                Some("mixedcase".to_string()),
                crate::db::DatabaseType::Oracle,
            ),
            Some("mixedcase".to_string())
        );
    }

    #[test]
    fn canonical_intellisense_scope_preserves_ambiguous_mysql_scope_case() {
        let mut data = IntellisenseData::new();
        data.users = vec!["SalesDb".to_string(), "salesdb".to_string()];

        assert_eq!(
            canonical_intellisense_scope(
                &data,
                Some("SALESDB".to_string()),
                crate::db::DatabaseType::MySQL,
            ),
            Some("SALESDB".to_string())
        );
    }

    #[test]
    fn canonical_intellisense_scope_prefers_exact_mysql_case_match() {
        let mut data = IntellisenseData::new();
        data.users = vec!["SalesDb".to_string(), "salesdb".to_string()];
        data.set_default_qualifier(Some("SalesDb".to_string()));

        assert_eq!(
            canonical_intellisense_scope(
                &data,
                Some("salesdb".to_string()),
                crate::db::DatabaseType::MySQL,
            ),
            Some("salesdb".to_string())
        );
    }

    #[test]
    fn sql_file_paths_match_accepts_same_path() {
        let path = std::env::temp_dir().join("space_query_same.sql");

        assert!(AppState::sql_file_paths_match(&path, &path));
    }

    #[test]
    fn sql_file_paths_match_rejects_same_name_in_different_dirs() {
        let root = std::env::temp_dir();
        let first = root.join("space_query_first").join("query.sql");
        let second = root.join("space_query_second").join("query.sql");

        assert!(!AppState::sql_file_paths_match(&first, &second));
    }

    #[test]
    fn schema_update_scope_matches_global_scope_exactly() {
        assert!(MainWindow::schema_update_scope_matches(
            DatabaseType::Oracle,
            Some(" SCOTT "),
            Some("SCOTT"),
            &[]
        ));
        assert!(!MainWindow::schema_update_scope_matches(
            DatabaseType::Oracle,
            Some(" SCOTT "),
            Some("scott"),
            &[]
        ));
        assert!(!MainWindow::schema_update_scope_matches(
            DatabaseType::MySQL,
            Some(" SCOTT "),
            Some("scott"),
            &[]
        ));
        assert!(MainWindow::schema_update_scope_matches(
            DatabaseType::Oracle,
            Some("SCOTT"),
            None,
            &[]
        ));
        assert!(!MainWindow::schema_update_scope_matches(
            DatabaseType::Oracle,
            Some("SCOTT"),
            Some("HR"),
            &[]
        ));
        assert!(!MainWindow::schema_update_scope_matches(
            DatabaseType::Oracle,
            None,
            Some("HR"),
            &[]
        ));
    }

    #[test]
    fn schema_update_scope_matches_unique_mysql_catalog_case() {
        let available_scopes = vec!["SalesDb".to_string()];

        assert!(MainWindow::schema_update_scope_matches(
            DatabaseType::MySQL,
            Some("SalesDb"),
            Some("salesdb"),
            &available_scopes
        ));
        assert!(MainWindow::schema_update_scope_matches(
            DatabaseType::MariaDB,
            Some("SALESDB"),
            Some("SalesDb"),
            &available_scopes
        ));
        assert!(!MainWindow::schema_update_scope_matches(
            DatabaseType::Oracle,
            Some("SalesDb"),
            Some("salesdb"),
            &available_scopes
        ));
        assert!(!MainWindow::schema_update_scope_matches(
            DatabaseType::MySQL,
            Some("SalesDb"),
            Some("OtherDb"),
            &available_scopes
        ));
    }

    #[test]
    fn schema_update_scope_rejects_ambiguous_mysql_case_variants() {
        let available_scopes = vec!["SalesDb".to_string(), "salesdb".to_string()];

        assert!(!MainWindow::schema_update_scope_matches(
            DatabaseType::MySQL,
            Some("SalesDb"),
            Some("salesdb"),
            &available_scopes
        ));
        assert!(MainWindow::schema_update_scope_matches(
            DatabaseType::MySQL,
            Some("salesdb"),
            Some("salesdb"),
            &available_scopes
        ));
    }

    #[test]
    fn pending_metadata_refresh_survives_failed_start_attempt() {
        assert!(!pending_metadata_refresh_after_start_attempt(true, true));
        assert!(pending_metadata_refresh_after_start_attempt(true, false));
        assert!(!pending_metadata_refresh_after_start_attempt(false, false));
    }

    #[test]
    fn mutex_flag_clear_guard_releases_flag_during_unwind() {
        let flag = Arc::new(Mutex::new(None));
        let token = try_set_mutex_flag(&flag).expect("set metadata refresh flag");
        let result = std::panic::catch_unwind(AssertUnwindSafe({
            let flag = flag.clone();
            move || {
                let _guard = MutexFlagClearGuard::new(flag, token);
                panic!("simulated metadata refresh panic");
            }
        }));

        assert!(result.is_err());
        assert!(!mutex_flag_is_set(&flag));
    }

    #[test]
    fn stale_mutex_flag_guard_does_not_clear_new_owner() {
        let flag = Arc::new(Mutex::new(None));
        let stale_token = try_set_mutex_flag(&flag).expect("set initial metadata refresh flag");
        clear_mutex_flag(&flag);
        let new_token = try_set_mutex_flag(&flag).expect("set replacement metadata refresh flag");
        assert_ne!(stale_token, new_token);

        {
            let _stale_guard = MutexFlagClearGuard::new(flag.clone(), stale_token);
        }

        assert!(mutex_flag_is_set(&flag));
        clear_mutex_flag_if_token(&flag, new_token);
        assert!(!mutex_flag_is_set(&flag));
    }

    #[test]
    fn transaction_isolation_choices_follow_database_backend_capabilities() {
        assert_eq!(
            transaction_isolation_choice_labels(
                DatabaseType::Oracle,
                TransactionIsolation::Default
            ),
            "Default|Read committed|Serializable"
        );
        assert_eq!(
            transaction_isolation_choice_labels(DatabaseType::MySQL, TransactionIsolation::Default),
            "Default|Read uncommitted|Read committed|Repeatable read|Serializable"
        );
    }

    #[test]
    fn transaction_isolation_default_choice_shows_database_default_level() {
        assert_eq!(
            transaction_isolation_choice_labels(
                DatabaseType::Oracle,
                TransactionIsolation::ReadCommitted
            ),
            "Default (Read committed)|Read committed|Serializable"
        );
        assert_eq!(
            transaction_isolation_choice_labels(
                DatabaseType::MySQL,
                TransactionIsolation::RepeatableRead
            ),
            "Default (Repeatable read)|Read uncommitted|Read committed|Repeatable read|Serializable"
        );
    }

    #[test]
    fn transaction_isolation_choice_index_defaults_when_backend_does_not_support_level() {
        assert_eq!(
            transaction_isolation_choice_index(
                DatabaseType::Oracle,
                TransactionIsolation::RepeatableRead
            ),
            0
        );
        assert_eq!(
            transaction_isolation_from_choice_index(DatabaseType::MySQL, 3),
            TransactionIsolation::RepeatableRead
        );
    }

    #[test]
    fn session_pool_slot_action_allows_tab_retained_sessions() {
        assert_eq!(session_pool_slot_action(0, 4), SessionPoolSlotAction::None);
        assert_eq!(session_pool_slot_action(2, 4), SessionPoolSlotAction::None);
    }

    #[test]
    fn session_pool_slot_action_cancels_when_lazy_fetches_fill_pool() {
        assert_eq!(
            session_pool_slot_action(4, 4),
            SessionPoolSlotAction::CancelLazyFetch
        );
        assert_eq!(
            session_pool_slot_action(5, 4),
            SessionPoolSlotAction::CancelLazyFetch
        );
    }

    #[test]
    fn session_activity_result_request_uses_idle_row_when_no_active_entries() {
        let request =
            build_session_activity_result_request("Local", "Oracle", 4, "Idle", Vec::new());

        assert_eq!(request.label, "Session Activity");
        assert_eq!(request.result.message, "No active sessions");
        assert_eq!(request.result.row_count, 1);
        assert_eq!(
            request
                .result
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "Connection",
                "Database",
                "Pool Size",
                "Tab",
                "Result Tab",
                "State",
                "Current Activity",
                "SQL Preview",
                "Fetched Rows",
                "Elapsed"
            ]
        );
        assert_eq!(
            request.result.rows[0],
            vec!["Local", "Oracle", "4", "-", "-", "Idle", "Idle", "-", "-", "-"]
        );
    }

    #[test]
    fn session_activity_result_request_formats_active_rows() {
        let request = build_session_activity_result_request(
            "Local",
            "Oracle",
            4,
            "SELECT running",
            vec![SessionActivityEntry {
                tab_name: "Query 1".to_string(),
                result_tab: Some(2),
                state: ResultTabStatus::Fetching.label().to_string(),
                database: "Oracle".to_string(),
                sql_preview: "select * from employees".to_string(),
                fetched_rows: 42,
                elapsed: "3s".to_string(),
            }],
        );

        assert_eq!(request.result.message, "1 session(s)");
        assert_eq!(
            request.result.rows[0],
            vec![
                "Local",
                "Oracle",
                "4",
                "Query 1",
                "2",
                "Fetching",
                "SELECT running",
                "select * from employees",
                "42",
                "3s"
            ]
        );
    }

    #[test]
    fn normalize_line_endings_for_editor_keeps_lf_only_content() {
        let text = String::from("select 1;\nselect 2;");
        let normalized = MainWindow::normalize_line_endings_for_editor(text.clone());

        assert_eq!(normalized, text);
    }

    #[test]
    fn success_messages_go_to_info_while_errors_use_errors_tab() {
        let dml = QueryResult::new_dml("update t set c = 1", 1, Duration::ZERO, "UPDATE");
        let select = QueryResult::new_select("select 1", Vec::new(), Vec::new(), Duration::ZERO);
        let error = QueryResult::new_error("select missing", "table not found");

        assert!(should_send_success_message_to_info(&dml, false));
        assert!(should_send_success_message_to_info(&select, false));
        assert!(!should_send_success_message_to_info(&dml, true));
        assert!(!should_send_success_message_to_info(&error, false));
    }

    #[test]
    fn support_result_panes_select_only_without_current_data_grid_destination() {
        assert!(should_select_support_result_pane(None));

        let context = QueryProgressContext::new(None, "Executing query".to_string(), None);
        assert!(should_select_support_result_pane(Some(&context)));

        let mut context_with_grid =
            QueryProgressContext::new(None, "Executing query".to_string(), None);
        context_with_grid
            .result_tab_ids
            .insert(0, ResultTabId::new(3));
        assert!(!should_select_support_result_pane(Some(&context_with_grid)));

        let context_with_grid_target = QueryProgressContext::new(
            Some(ResultTabId::new(1)),
            "Saving result grid".to_string(),
            None,
        );
        assert!(!should_select_support_result_pane(Some(
            &context_with_grid_target
        )));
    }

    fn assert_progress_routes_only(
        label: &str,
        progress: QueryProgress,
        expected: &[ResultPaneRoute],
    ) {
        let routes = result_pane_routes_for_progress(&progress);
        assert_eq!(routes, expected, "{label}");
    }

    fn assert_script_progress_routes_only(
        label: &str,
        progress: QueryProgress,
        expected: &[ResultPaneRoute],
    ) {
        let routes = result_pane_routes_for_progress_with_script_context(&progress, true);
        assert_eq!(routes, expected, "{label}");
    }

    fn result_test_column(name: &str) -> ColumnInfo {
        ColumnInfo {
            name: name.to_string(),
            data_type: "NUMBER".to_string(),
        }
    }

    #[test]
    fn result_progress_routes_cover_each_tab_without_unintended_destinations() {
        assert_progress_routes_only(
            "select start with columns",
            QueryProgress::SelectStart {
                index: 0,
                columns: vec!["VALUE".to_string()],
                null_text: "<NULL>".to_string(),
            },
            &[ResultPaneRoute::DataGrid],
        );
        assert_progress_routes_only(
            "data grid rows",
            QueryProgress::Rows {
                index: 0,
                rows: vec![vec!["grid".to_string()]],
            },
            &[ResultPaneRoute::DataGrid],
        );
        assert_progress_routes_only(
            "script output",
            QueryProgress::ScriptOutput {
                lines: vec!["script".to_string()],
            },
            &[ResultPaneRoute::ScriptOutput],
        );
        assert_progress_routes_only(
            "dbms output",
            QueryProgress::DbmsOutput {
                lines: vec!["dbms".to_string()],
            },
            &[ResultPaneRoute::DbmsOutput],
        );
        assert_progress_routes_only(
            "messages info",
            QueryProgress::Message {
                kind: ResultMessageKind::Info,
                lines: vec!["info".to_string()],
            },
            &[ResultPaneRoute::MessagesInfo],
        );
        assert_progress_routes_only(
            "messages errors",
            QueryProgress::Message {
                kind: ResultMessageKind::Error,
                lines: vec!["error".to_string()],
            },
            &[ResultPaneRoute::MessagesErrors],
        );
        assert_progress_routes_only(
            "explain plan",
            QueryProgress::ExplainPlanOutput {
                text: "plan".to_string(),
            },
            &[ResultPaneRoute::DataGrid],
        );

        let select = QueryResult::new_select(
            "select 1",
            vec![result_test_column("VALUE")],
            Vec::new(),
            Duration::ZERO,
        );
        assert_progress_routes_only(
            "select completion",
            QueryProgress::StatementFinished {
                index: 0,
                result: select,
                connection_name: "TEST".to_string(),
                timed_out: false,
            },
            &[ResultPaneRoute::DataGrid, ResultPaneRoute::MessagesInfo],
        );

        let dml = QueryResult::new_dml("update t set c = 1", 1, Duration::ZERO, "UPDATE");
        assert_progress_routes_only(
            "non-select completion",
            QueryProgress::StatementFinished {
                index: 1,
                result: dml,
                connection_name: "TEST".to_string(),
                timed_out: false,
            },
            &[ResultPaneRoute::MessagesInfo],
        );
        assert_script_progress_routes_only(
            "script non-select completion",
            QueryProgress::StatementFinished {
                index: 1,
                result: QueryResult::new_dml("update t set c = 1", 1, Duration::ZERO, "UPDATE"),
                connection_name: "TEST".to_string(),
                timed_out: false,
            },
            &[],
        );

        let error = QueryResult::new_error("select missing", "table not found");
        assert_progress_routes_only(
            "sql error completion",
            QueryProgress::StatementFinished {
                index: 2,
                result: error,
                connection_name: "TEST".to_string(),
                timed_out: false,
            },
            &[ResultPaneRoute::MessagesErrors],
        );

        let mut select_error = QueryResult::new_error("select * from missing", "table not found");
        select_error.is_select = true;
        assert_progress_routes_only(
            "select error completion",
            QueryProgress::StatementFinished {
                index: 3,
                result: select_error,
                connection_name: "TEST".to_string(),
                timed_out: false,
            },
            &[ResultPaneRoute::MessagesErrors],
        );

        let cancelled = QueryResult {
            sql: "select * from t".to_string(),
            columns: vec![result_test_column("ID")],
            rows: Vec::new(),
            row_count: 0,
            execution_time: Duration::ZERO,
            message: "Query cancelled".to_string(),
            is_select: true,
            success: false,
        };
        assert_progress_routes_only(
            "cancelled completion",
            QueryProgress::StatementFinished {
                index: 4,
                result: cancelled,
                connection_name: "TEST".to_string(),
                timed_out: false,
            },
            &[],
        );
    }

    #[test]
    fn result_progress_routes_data_grid_by_columns_not_row_count() {
        assert_progress_routes_only(
            "select start without columns",
            QueryProgress::SelectStart {
                index: 0,
                columns: Vec::new(),
                null_text: "<NULL>".to_string(),
            },
            &[],
        );

        let zero_row_select = QueryResult::new_select(
            "select * from t where 1 = 0",
            vec![result_test_column("ID")],
            Vec::new(),
            Duration::ZERO,
        );
        assert_progress_routes_only(
            "zero-row select with columns",
            QueryProgress::StatementFinished {
                index: 0,
                result: zero_row_select,
                connection_name: "TEST".to_string(),
                timed_out: false,
            },
            &[ResultPaneRoute::DataGrid, ResultPaneRoute::MessagesInfo],
        );

        let mut no_column_select =
            QueryResult::new_select("select 1 into @v", Vec::new(), Vec::new(), Duration::ZERO);
        no_column_select.row_count = 2;
        no_column_select.message = "Statement executed successfully".to_string();
        assert_progress_routes_only(
            "select-like completion without columns but with row count",
            QueryProgress::StatementFinished {
                index: 1,
                result: no_column_select,
                connection_name: "TEST".to_string(),
                timed_out: false,
            },
            &[ResultPaneRoute::MessagesInfo],
        );

        let dml = QueryResult::new_dml("insert into t values (1)", 4, Duration::ZERO, "INSERT");
        assert_progress_routes_only(
            "dml affected rows do not create data grid",
            QueryProgress::StatementFinished {
                index: 2,
                result: dml,
                connection_name: "TEST".to_string(),
                timed_out: false,
            },
            &[ResultPaneRoute::MessagesInfo],
        );
    }

    #[test]
    fn next_active_editor_tab_after_close_keeps_current_active_tab_when_closing_background_tab() {
        let tab_ids = vec![10, 20, 30];

        let next_tab_id = next_active_editor_tab_id_after_close(&tab_ids, 0, 30);

        assert_eq!(next_tab_id, Some(30));
    }

    #[test]
    fn next_active_editor_tab_after_close_moves_to_next_tab_when_closing_active_tab() {
        let tab_ids = vec![10, 20, 30];

        let next_tab_id = next_active_editor_tab_id_after_close(&tab_ids, 1, 20);

        assert_eq!(next_tab_id, Some(30));
    }

    #[test]
    fn next_active_editor_tab_after_close_falls_back_to_previous_tab_at_end() {
        let tab_ids = vec![10, 20, 30];

        let next_tab_id = next_active_editor_tab_id_after_close(&tab_ids, 2, 30);

        assert_eq!(next_tab_id, Some(20));
    }

    #[test]
    fn next_active_editor_tab_after_close_returns_none_for_last_remaining_tab() {
        let tab_ids = vec![10];

        let next_tab_id = next_active_editor_tab_id_after_close(&tab_ids, 0, 10);

        assert_eq!(next_tab_id, None);
    }

    #[test]
    fn validate_result_edit_action_allows_when_no_query_is_running() {
        assert!(validate_result_edit_action_allowed(false).is_ok());
    }

    #[test]
    fn validate_result_edit_action_blocks_when_query_is_running() {
        assert_eq!(
            validate_result_edit_action_allowed(true),
            Err("A query is running. Wait for completion before editing result rows.".to_string())
        );
    }

    #[test]
    fn connection_transition_blocks_running_query_before_lazy_fetch() {
        assert_eq!(
            connection_transition_block_message(true, true, "connecting"),
            Some("A query is running. Stop it before connecting.".to_string())
        );
    }

    #[test]
    fn connection_transition_blocks_active_lazy_fetch() {
        assert_eq!(
            connection_transition_block_message(false, true, "disconnecting"),
            Some(
                "A lazy fetch is still open. Fetch all rows or cancel it before disconnecting."
                    .to_string()
            )
        );
        assert_eq!(
            connection_transition_block_message(false, false, "disconnecting"),
            None
        );
    }

    #[test]
    fn transaction_option_changes_block_running_work() {
        assert_eq!(
            transaction_option_block_message(true, false, "changing auto-commit"),
            Some("A query is running. Stop it before changing auto-commit.".to_string())
        );
        assert_eq!(
            transaction_option_block_message(false, true, "changing transaction mode"),
            Some(
                "A lazy fetch is still open. Fetch all rows or cancel it before changing transaction mode."
                    .to_string()
            )
        );
        assert_eq!(
            transaction_option_block_message(false, false, "changing auto-commit"),
            None
        );
    }

    #[test]
    fn raw_autocommit_progress_is_tab_scoped_status() {
        assert_eq!(
            auto_commit_changed_progress_status(true),
            "Tab auto-commit enabled"
        );
        assert_eq!(
            auto_commit_changed_progress_status(false),
            "Tab auto-commit disabled"
        );
    }

    #[test]
    fn cancelled_lazy_fetch_can_finish_progress_context() {
        assert!(should_finish_progress_after_lazy_fetch_close(true, true));
        assert!(should_finish_progress_after_lazy_fetch_close(false, true));
        assert!(!should_finish_progress_after_lazy_fetch_close(true, false));
    }

    #[test]
    fn orphaned_canceling_lazy_fetch_sessions_require_pending_and_inactive() {
        let mut context = QueryProgressContext::new(None, "Executing query".to_string(), None);
        context.register_lazy_fetch_session(10, 0, 10, 1);
        context.register_lazy_fetch_session(20, 1, 20, 1);
        context.register_lazy_fetch_session(30, 2, 30, 1);
        let pending = HashSet::from([10, 20]);

        let orphaned =
            orphaned_canceling_lazy_fetch_sessions(Some(&context), &pending, |session_id| {
                session_id == 20
            });

        assert_eq!(orphaned, vec![10]);

        context.state_label = ResultTabStatus::Canceling.label().to_string();
        let orphaned_without_pending =
            orphaned_canceling_lazy_fetch_sessions(Some(&context), &HashSet::new(), |session_id| {
                session_id == 20
            });
        assert_eq!(orphaned_without_pending, vec![10, 30]);
    }

    #[test]
    fn statement_finished_error_stays_cancelled_when_context_was_canceling() {
        let error = QueryResult::new_error("select missing", "cleanup failed");
        assert_eq!(
            statement_finished_status(&error, true),
            ResultTabStatus::Cancelled
        );
        assert_eq!(
            statement_finished_status(&error, false),
            ResultTabStatus::Error
        );

        let success = QueryResult::new_dml("update t set c = 1", 1, Duration::ZERO, "UPDATE");
        assert_eq!(
            statement_finished_status(&success, true),
            ResultTabStatus::Done
        );
    }

    #[test]
    fn statement_start_preserves_pending_canceling_status() {
        assert_eq!(
            statement_start_status(ResultTabStatus::Running.label(), true),
            ResultTabStatus::Canceling
        );
        assert_eq!(
            statement_start_status(ResultTabStatus::Canceling.label(), false),
            ResultTabStatus::Canceling
        );
        assert_eq!(
            statement_start_status(ResultTabStatus::Running.label(), false),
            ResultTabStatus::Running
        );
    }

    #[test]
    fn lazy_fetch_canceling_falls_back_to_active_statement_for_active_editor_session() {
        let mut context = QueryProgressContext::new(None, "Fetching".to_string(), None);
        context.active_statement_index = Some(2);

        assert_eq!(
            lazy_fetch_canceling_statement_index(&context, 77, true),
            Some(2)
        );
        assert_eq!(
            lazy_fetch_canceling_statement_index(&context, 77, false),
            None
        );

        context.register_lazy_fetch_session(77, 3, 77, 1);
        assert_eq!(
            lazy_fetch_canceling_statement_index(&context, 77, true),
            Some(3)
        );
    }

    #[test]
    fn cancelled_statement_finished_does_not_use_empty_error_grid_path() {
        let error = QueryResult::new_error("select missing", "cleanup failed");
        let status = statement_finished_status(&error, true);
        let has_fetched_rows = false;
        assert_eq!(status, ResultTabStatus::Cancelled);
        assert!(
            status != ResultTabStatus::Error || has_fetched_rows,
            "cancelled cleanup failures must not be treated as removable empty error grids"
        );
    }

    #[test]
    fn terminal_lazy_fetch_close_aborts_result_tab_even_without_cancel_flag() {
        assert!(lazy_fetch_close_should_abort_result_tab(
            false,
            false,
            true,
            crate::ui::sql_editor::InterruptKind::None
        ));
        assert!(lazy_fetch_close_should_abort_result_tab(
            false,
            true,
            true,
            crate::ui::sql_editor::InterruptKind::ConnectionError
        ));
        assert!(!lazy_fetch_close_should_abort_result_tab(
            false,
            true,
            true,
            crate::ui::sql_editor::InterruptKind::None
        ));
    }

    #[test]
    fn execution_finished_status_reports_transaction_decision_after_cancel() {
        let mut event =
            crate::db::session_policy::ExecutionFinishedEvent::new(crate::db::DatabaseType::MySQL);
        event.cancelled = true;
        let snapshot = crate::db::PooledSessionLeaseSnapshot {
            db_type: crate::db::DatabaseType::MySQL,
            pool_context_epoch: 0,
            transaction_state: crate::db::TransactionSessionState::DecisionRequired,
            retained_state: crate::db::RetainedSessionState::from_transaction_state(
                crate::db::TransactionSessionState::DecisionRequired,
            ),
            current_scope: None,
        };

        assert_eq!(
            execution_finished_status_override(&event, Some(snapshot)),
            Some("Cancelled | Transaction decision required")
        );
    }

    #[test]
    fn execution_finished_event_gate_rejects_replaced_editor_or_connection() {
        let mut event =
            crate::db::session_policy::ExecutionFinishedEvent::new(crate::db::DatabaseType::MySQL);
        event.tab_id = 7;
        event.editor_id = 11;
        event.operation_id = 13;
        event.connection_generation = 17;

        assert!(execution_finished_event_matches_current_editor(
            &event,
            7,
            Some(11),
            0,
            13,
            Some(17),
        ));
        assert!(execution_finished_event_matches_current_editor(
            &event,
            7,
            Some(11),
            13,
            0,
            Some(17),
        ));
        assert!(
            !execution_finished_event_matches_current_editor(&event, 7, Some(12), 0, 13, Some(17)),
            "same tab_id is not enough after an editor widget was recreated"
        );
        assert!(
            !execution_finished_event_matches_current_editor(&event, 8, Some(11), 0, 13, Some(17)),
            "same editor/operation id is not enough if the event belongs to another tab"
        );
        assert!(
            !execution_finished_event_matches_current_editor(&event, 7, Some(11), 14, 0, Some(17)),
            "late completion from an older operation must not update the active operation status"
        );
        assert!(
            !execution_finished_event_matches_current_editor(&event, 7, Some(11), 0, 13, Some(18)),
            "late completion from an older physical connection generation must not update status"
        );
        assert!(
            !execution_finished_event_matches_current_editor(&event, 7, Some(11), 0, 14, Some(17)),
            "zero current operation must still reject events older than the last completed operation"
        );
    }

    #[test]
    fn execution_finished_event_gate_rejects_empty_identity() {
        let event =
            crate::db::session_policy::ExecutionFinishedEvent::new(crate::db::DatabaseType::MySQL);

        assert!(!execution_finished_event_matches_current_editor(
            &event,
            0,
            Some(0),
            0,
            0,
            Some(0),
        ));
    }

    #[test]
    fn operation_progress_token_accepts_late_lazy_events_for_last_completed_operation() {
        let token = QueryOperationToken {
            tab_id: 7,
            editor_id: 11,
            operation_id: 13,
            connection_generation: 17,
        };
        let abandoned = HashSet::new();

        assert!(operation_progress_token_matches_current_editor(
            7,
            token,
            Some(11),
            13,
            0,
            &abandoned,
        ));
        assert!(operation_progress_token_matches_current_editor(
            7,
            token,
            Some(11),
            0,
            13,
            &abandoned,
        ));
    }

    #[test]
    fn operation_progress_token_rejects_abandoned_or_replaced_operation() {
        let token = QueryOperationToken {
            tab_id: 7,
            editor_id: 11,
            operation_id: 13,
            connection_generation: 17,
        };
        let abandoned = HashSet::from([token]);

        assert!(!operation_progress_token_matches_current_editor(
            7,
            token,
            Some(11),
            13,
            0,
            &abandoned,
        ));
        assert!(!operation_progress_token_matches_current_editor(
            7,
            token,
            Some(12),
            13,
            0,
            &HashSet::new(),
        ));
        assert!(!operation_progress_token_matches_current_editor(
            7,
            token,
            Some(11),
            14,
            13,
            &HashSet::new(),
        ));
    }

    #[test]
    fn execution_finished_status_does_not_override_plain_cancel() {
        let mut event =
            crate::db::session_policy::ExecutionFinishedEvent::new(crate::db::DatabaseType::MySQL);
        event.cancelled = true;
        let snapshot = crate::db::PooledSessionLeaseSnapshot {
            db_type: crate::db::DatabaseType::MySQL,
            pool_context_epoch: 0,
            transaction_state: crate::db::TransactionSessionState::Clean,
            retained_state: crate::db::RetainedSessionState::from_transaction_state(
                crate::db::TransactionSessionState::Clean,
            ),
            current_scope: None,
        };

        assert_eq!(
            execution_finished_status_override(&event, Some(snapshot)),
            None
        );
    }

    #[test]
    fn fetch_status_updates_immediately_for_first_row_batch() {
        assert!(should_update_fetch_status(
            0,
            FETCH_STATUS_UPDATE_INTERVAL.saturating_sub(Duration::from_millis(1))
        ));
    }

    #[test]
    fn fetch_status_throttles_after_first_row_batch() {
        assert!(!should_update_fetch_status(
            100,
            FETCH_STATUS_UPDATE_INTERVAL.saturating_sub(Duration::from_millis(1))
        ));
        assert!(should_update_fetch_status(
            100,
            FETCH_STATUS_UPDATE_INTERVAL
        ));
    }

    #[test]
    fn fetch_status_restarts_when_animation_is_stopped() {
        assert!(should_refresh_fetch_status_animation(
            false,
            100,
            Duration::ZERO
        ));
    }

    #[test]
    fn fetch_status_keeps_throttle_when_animation_is_running() {
        assert!(!should_refresh_fetch_status_animation(
            true,
            100,
            FETCH_STATUS_UPDATE_INTERVAL.saturating_sub(Duration::from_millis(1))
        ));
        assert!(should_refresh_fetch_status_animation(
            true,
            100,
            FETCH_STATUS_UPDATE_INTERVAL
        ));
    }

    #[test]
    fn progress_context_marks_closed_statement_and_clears_fetch_tracking() {
        let mut context = QueryProgressContext::new(None, "Executing".to_string(), None);
        context.fetch_row_counts.insert(2, 100);
        context.lazy_fetch_sessions.insert(44, 2);
        context.active_statement_index = Some(2);
        context.running_statement_index = Some(2);

        context.mark_statement_closed(2);

        assert!(context.closed_statement_indices.contains(&2));
        assert!(!context.fetch_row_counts.contains_key(&2));
        assert_eq!(context.active_statement_index, None);
        assert_eq!(context.canceling_statement_index(), None);
        assert_eq!(context.lazy_fetch_sessions.get(&44), Some(&2));
    }

    #[test]
    fn progress_context_canceling_statement_tracks_running_statement_not_active_result() {
        let mut context = QueryProgressContext::new(None, "Executing".to_string(), None);

        context.mark_statement_running(0);
        context.mark_statement_finished(0);
        context.active_statement_index = Some(0);
        context.mark_statement_running(1);

        assert_eq!(context.active_statement_index, Some(1));
        context.active_statement_index = Some(0);
        assert_eq!(context.canceling_statement_index(), Some(1));

        context.mark_statement_finished(1);
        assert_eq!(context.canceling_statement_index(), None);
    }

    #[test]
    fn lazy_fetch_session_event_accepts_fast_completed_worker_for_active_statement() {
        let mut context = QueryProgressContext::new(None, "Executing".to_string(), None);
        context.active_statement_index = Some(0);

        assert!(should_accept_lazy_fetch_session_event(
            false,
            None,
            Some(&context),
            0
        ));
    }

    #[test]
    fn lazy_fetch_session_event_rejects_mismatched_active_worker() {
        let mut context = QueryProgressContext::new(None, "Executing".to_string(), None);
        context.active_statement_index = Some(0);

        assert!(!should_accept_lazy_fetch_session_event(
            false,
            Some(99),
            Some(&context),
            0
        ));
    }

    #[test]
    fn lazy_fetch_session_event_rejects_inactive_statement_fallback() {
        let mut context = QueryProgressContext::new(None, "Executing".to_string(), None);
        context.active_statement_index = Some(1);

        assert!(!should_accept_lazy_fetch_session_event(
            false,
            None,
            Some(&context),
            0
        ));
    }

    #[test]
    fn progress_context_finds_lazy_session_for_statement() {
        let mut context = QueryProgressContext::new(None, "Executing".to_string(), None);
        context.lazy_fetch_sessions.insert(44, 2);
        context.lazy_fetch_sessions.insert(55, 3);

        assert_eq!(context.lazy_fetch_session_for_statement(2), Some(44));
        assert_eq!(context.lazy_fetch_session_for_statement(3), Some(55));
        assert_eq!(context.lazy_fetch_session_for_statement(4), None);
    }

    #[test]
    fn progress_context_distinguishes_registered_and_waiting_lazy_fetch() {
        let mut context = QueryProgressContext::new(None, "Executing".to_string(), None);

        context.register_lazy_fetch_session(44, 2, 44, 7);
        assert!(!context.has_waiting_lazy_fetch());

        assert!(context.mark_lazy_fetch_waiting(44, 2));
        assert!(context.has_waiting_lazy_fetch());

        context.mark_lazy_fetch_active_for_statement(2);
        assert!(!context.has_waiting_lazy_fetch());
    }

    #[test]
    fn progress_context_rejects_stale_lazy_fetch_waiting_event() {
        let mut context = QueryProgressContext::new(None, "Executing".to_string(), None);
        context.register_lazy_fetch_session(44, 2, 44, 7);

        assert!(!context.mark_lazy_fetch_waiting(44, 3));
        assert!(!context.mark_lazy_fetch_waiting(55, 2));
        assert!(!context.has_waiting_lazy_fetch());
    }

    #[test]
    fn progress_context_rejects_stale_lazy_fetch_close_token() {
        let mut context = QueryProgressContext::new(None, "Executing".to_string(), None);
        context.register_lazy_fetch_session(44, 2, 44, 7);

        assert!(context.lazy_fetch_event_matches(44, 2, 44, 7));
        assert!(!context.lazy_fetch_event_matches(44, 2, 45, 7));
        assert!(!context.lazy_fetch_event_matches(44, 2, 44, 8));
        assert!(!context.lazy_fetch_event_matches(44, 3, 44, 7));
    }

    #[test]
    fn progress_context_remove_lazy_fetch_clears_waiting_state() {
        let mut context = QueryProgressContext::new(None, "Executing".to_string(), None);
        context.register_lazy_fetch_session(44, 2, 44, 7);
        assert!(context.mark_lazy_fetch_waiting(44, 2));

        assert_eq!(context.remove_lazy_fetch_session(44), Some(2));
        assert!(!context.has_waiting_lazy_fetch());
        assert_eq!(context.remove_lazy_fetch_session(44), None);
    }

    #[test]
    fn progress_context_keeps_lazy_session_with_matching_result_tab() {
        let mut context = QueryProgressContext::new(None, "Executing".to_string(), None);
        context.lazy_fetch_sessions.insert(44, 2);
        context.result_tab_ids.insert(2, ResultTabId::new(3));

        let unmapped =
            context.lazy_fetch_sessions_without_result_tab_mapping(|tab_id| match tab_id {
                id if id == ResultTabId::new(3) => Some(44),
                _ => None,
            });

        assert!(unmapped.is_empty());
    }

    #[test]
    fn progress_context_finds_lazy_session_without_result_tab() {
        let mut context = QueryProgressContext::new(None, "Executing".to_string(), None);
        context.lazy_fetch_sessions.insert(44, 2);

        let unmapped = context.lazy_fetch_sessions_without_result_tab_mapping(|_| None);

        assert_eq!(unmapped, vec![44]);
    }

    #[test]
    fn progress_context_finds_lazy_session_with_mismatched_result_tab() {
        let mut context = QueryProgressContext::new(None, "Executing".to_string(), None);
        context.lazy_fetch_sessions.insert(44, 2);
        context.result_tab_ids.insert(2, ResultTabId::new(3));

        let unmapped =
            context.lazy_fetch_sessions_without_result_tab_mapping(|tab_id| match tab_id {
                id if id == ResultTabId::new(3) => Some(55),
                _ => None,
            });

        assert_eq!(unmapped, vec![44]);
    }

    #[test]
    fn progress_context_clear_marks_active_statement_before_lazy_session_event() {
        let mut context = QueryProgressContext::new(None, "Executing".to_string(), None);
        context.active_statement_index = Some(0);

        context.mark_all_result_statements_closed();

        assert!(context.closed_statement_indices.contains(&0));
        assert_eq!(context.active_statement_index, None);
        assert!(context.lazy_fetch_sessions.is_empty());
    }

    #[test]
    fn progress_context_clear_marks_known_lazy_sessions_and_fetch_counts() {
        let mut context = QueryProgressContext::new(None, "Executing".to_string(), None);
        context.lazy_fetch_sessions.insert(10, 1);
        context.fetch_row_counts.insert(2, 50);

        context.mark_all_result_statements_closed();

        assert!(context.closed_statement_indices.contains(&1));
        assert!(context.closed_statement_indices.contains(&2));
        assert!(context.lazy_fetch_sessions.is_empty());
        assert!(context.fetch_row_counts.is_empty());
    }

    #[test]
    fn progress_context_assigns_statement_result_tab_ids_once() {
        let mut context = QueryProgressContext::new(None, "Executing".to_string(), None);

        assert_eq!(
            context.ensure_result_tab_id(0, || ResultTabId::new(10)),
            ResultTabId::new(10)
        );
        assert_eq!(
            context.ensure_result_tab_id(0, || ResultTabId::new(11)),
            ResultTabId::new(10)
        );
        assert_eq!(
            context.ensure_result_tab_id(1, || ResultTabId::new(11)),
            ResultTabId::new(11)
        );

        assert_eq!(
            context.result_tab_id_for_statement(0),
            Some(ResultTabId::new(10))
        );
        assert_eq!(
            context.result_tab_id_for_statement(1),
            Some(ResultTabId::new(11))
        );
    }

    #[test]
    fn progress_context_removes_closed_statement_result_tab_id() {
        let mut context = QueryProgressContext::new(None, "Executing".to_string(), None);

        assert_eq!(
            context.ensure_result_tab_id(0, || ResultTabId::new(10)),
            ResultTabId::new(10)
        );
        context.mark_statement_closed(0);

        assert_eq!(context.result_tab_id_for_statement(0), None);
    }

    #[test]
    fn progress_context_auto_selects_only_first_result_tab() {
        let mut context = QueryProgressContext::new(None, "Executing".to_string(), None);

        assert!(context.claim_result_tab_auto_select());
        assert!(!context.claim_result_tab_auto_select());
    }

    #[test]
    fn next_spinner_frame_returns_none_when_frame_count_is_zero() {
        assert_eq!(AppState::next_spinner_frame(0, 0), None);
        assert_eq!(AppState::next_spinner_frame(42, 0), None);
    }

    #[test]
    fn next_spinner_frame_wraps_with_non_zero_frame_count() {
        assert_eq!(AppState::next_spinner_frame(0, 10), Some(1));
        assert_eq!(AppState::next_spinner_frame(9, 10), Some(0));
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn prepare_result_export_releases_app_state_lock_before_lazy_fetch_request() {
        let _app = fltk::app::App::default();
        configure_fltk_globals(&AppConfig::default());
        let window = MainWindow::new_with_config(AppConfig::default());
        let state = window.state.clone();
        let lock_visible = Arc::new(Mutex::new(None::<bool>));

        {
            let mut guard = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let result_tab_id = guard.result_tabs.reserve_result_tab_id();
            guard
                .result_tabs
                .ensure_statement_tab_by_id(result_tab_id, "Result 1", true);
            guard
                .result_tabs
                .start_streaming_by_id(result_tab_id, &["A".to_string()], "NULL");
            guard
                .result_tabs
                .append_rows_by_id(result_tab_id, vec![vec!["1".to_string()]]);
            guard
                .result_tabs
                .set_lazy_fetch_session_by_id(result_tab_id, 77);

            let weak_state = Arc::downgrade(&state);
            let lock_visible_for_callback = lock_visible.clone();
            let callback: LazyFetchCallback =
                Arc::new(Mutex::new(Some(Box::new(move |session_id, request| {
                    *lock_visible_for_callback
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(
                        session_id == 77
                            && request == LazyFetchRequest::All
                            && weak_state
                                .upgrade()
                                .is_some_and(|state| state.try_lock().is_ok()),
                    );
                    true
                }))));
            guard.result_tabs.set_lazy_fetch_callback(callback);
        }

        let export = MainWindow::prepare_result_export(&state, Box::new(|_, _| {}))
            .expect("prepare export should succeed");

        assert!(export.is_none());
        assert_eq!(
            *lock_visible
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            Some(true)
        );
    }
}
