use fltk::{
    app,
    browser::Browser,
    button::{Button, CheckButton},
    dialog::{FileDialog, FileDialogType},
    draw::{measure, set_cursor, set_font},
    enums::{Align, Color, Cursor, Event, FrameType},
    frame::Frame,
    group::{Flex, FlexType, Group, Tile},
    input::{Input, IntInput},
    menu::{Choice, MenuBar, MenuButton},
    prelude::*,
    text::{TextBuffer, TextDisplay},
    widget::Widget,
    window::Window,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use crate::app_icon;
use crate::db::session_policy::{CancelTargetSnapshot, ExecutionState};
use crate::db::{
    connect_shared_connection_with_policy, connection_transition_activity,
    create_shared_connection, format_connection_busy_message,
    resize_shared_connection_pool_with_policy, try_lock_connection_with_activity, ColumnInfo,
    ConnectionAttemptPolicy, ConnectionId, ConnectionRegistry, ConnectionRuntime,
    ConnectionRuntimeState, DatabaseType, ObjectBrowser, QueryResult,
    RetainedSessionMutationOutcome, RetainedSessionPreflightAction,
    RetainedSessionPreflightDecision, RetainedSessionResolutionAction, SharedConnection,
    TabConnectionBinding, TransactionAccessMode, TransactionIsolation, TransactionMode,
};
use crate::ui::constants::*;
use crate::ui::grid_sort::NullOrdering;
use crate::ui::result_export::{ExportDestination, ExportFormat};
use crate::ui::result_export_dialog::ExportChoice;
use crate::ui::result_table::{
    ResultGridEditExecuteCallback, ResultGridSqlExecuteCallback, ResultTableContextAction,
};
use crate::ui::theme;
use crate::ui::{
    font_settings, show_settings_dialog, ConnectionDialog, FindReplaceDialog, FontSettings,
    HighlightData, IntellisenseData, MenuBarBuilder, MultiObjectBrowserWidget,
    ObjectBrowserMetadataSnapshot, ObjectBrowserWidget, QualifiedMemberKind, QueryCancelOutcome,
    QueryHistoryDialog, QueryOperationToken, QueryProgress, QueryTabId, QueryTabsWidget,
    ResultMessageKind, ResultTabCloseTarget, ResultTabId, ResultTabRequest, ResultTabStatus,
    ResultTabsWidget, SqlAction, SqlEditorContextAction, SqlEditorWidget,
    TableBrowseExecuteCallback, TableBrowseNavigation, TableBrowsePageRequest, TableBrowseTarget,
};
use crate::utils::arithmetic::{safe_div, safe_div_f64_to_usize, safe_rem};
use crate::utils::{malloc_trim_process, AppConfig, QueryHistory};

type MutexFlag = Arc<Mutex<Option<u64>>>;

const RESULT_ONE_TAB_PER_QUERY_LABEL: &str = " One tab per query";
const RESULT_CHECKBOX_GROUP_GAP: i32 = TOOLBAR_SPACING;
const RESULT_PAGE_UNITS: [usize; 5] = [10, 100, 250, 500, 1000];
const RESULT_PAGE_DEFAULT_UNIT_INDEX: usize = 3;
const RESULT_PAGE_NAV_BUTTON_WIDTH: i32 = 32;
const RESULT_PAGE_UNIT_WIDTH: i32 = 66;
const RESULT_PAGE_CONTROL_SPACING: i32 = TOOLBAR_SPACING;
const RESULT_PAGE_CONTROL_WIDTH: i32 =
    RESULT_PAGE_NAV_BUTTON_WIDTH * 4 + RESULT_PAGE_UNIT_WIDTH + RESULT_PAGE_CONTROL_SPACING * 4;
const UI_SCALE_BUTTON_WIDTH: i32 = 32;
const QUERY_TOOLBAR_COMPACT_BREAKPOINT: i32 = 1050;
const QUERY_TOOLBAR_COMPACT_CHOICE_WIDTH: i32 = 185;
const QUERY_TOOLBAR_COMPACT_ACCESS_WIDTH: i32 = 105;
const QUERY_TOOLBAR_COMPACT_NUMERIC_WIDTH: i32 = 48;
const QUERY_TOOLBAR_COMPACT_SCALE_BUTTON_WIDTH: i32 = 28;
const UI_SCALE_EPSILON: f32 = 0.01;
#[cfg(target_os = "macos")]
const MACOS_FULLSCREEN_EXIT_POLL_SECONDS: f64 = 0.05;
#[cfg(target_os = "macos")]
const MACOS_FULLSCREEN_EXIT_POLL_RETRIES: u8 = 100;
const APPLICATION_EXIT_POLL_SECONDS: f64 = 0.2;
// Once the user chose Cancel and Exit, a stuck database worker must not keep
// FLTK's event loop alive indefinitely.
const APPLICATION_EXIT_CANCEL_GRACE: Duration = Duration::from_secs(5);
const MAX_TOP_LEVEL_WINDOWS_TO_HIDE: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ApplicationExitWaitDecision {
    Continue,
    Retry,
    Force,
}

fn application_exit_wait_decision(
    has_running_work: bool,
    elapsed: Duration,
) -> ApplicationExitWaitDecision {
    if !has_running_work {
        ApplicationExitWaitDecision::Continue
    } else if elapsed < APPLICATION_EXIT_CANCEL_GRACE {
        ApplicationExitWaitDecision::Retry
    } else {
        ApplicationExitWaitDecision::Force
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UiScaleAction {
    In,
    Out,
    Reset,
}

fn next_ui_scale_percent(current: u32, action: UiScaleAction) -> u32 {
    match action {
        UiScaleAction::In => AppConfig::increase_ui_scale_percent(current),
        UiScaleAction::Out => AppConfig::decrease_ui_scale_percent(current),
        UiScaleAction::Reset => crate::utils::DEFAULT_UI_SCALE_PERCENT,
    }
}

fn window_geometry_after_ui_scale(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    old_scale: f32,
    new_scale: f32,
) -> (i32, i32, i32, i32) {
    // Preserve the complete physical frame. If only the logical position is
    // scaled, macOS can constrain an oversized native frame while FLTK keeps
    // the unconstrained logical size, breaking subsequent move coordinates.
    let ratio = safe_div(f64::from(old_scale), f64::from(new_scale));
    let scaled = |value: i32| (f64::from(value) * ratio).round() as i32;
    (
        scaled(x),
        scaled(y),
        scaled(width).max(1),
        scaled(height).max(1),
    )
}

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
    query_tab_id: QueryTabId,
    connection_id: ConnectionId,
    connection_generation: u64,
    binding_revision: u64,
    request_id: u64,
    db_type: DatabaseType,
    requested_scope: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActiveSchemaUpdateTarget {
    query_tab_id: QueryTabId,
    connection_id: ConnectionId,
    connection_generation: u64,
    binding_revision: u64,
    request_id: u64,
    db_type: DatabaseType,
    scope: Option<String>,
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
    connection_binding: TabConnectionBinding,
    sql_editor: SqlEditorWidget,
    sql_buffer: TextBuffer,
    intellisense_data: Arc<Mutex<IntellisenseData>>,
    highlight_data: HighlightData,
    result_tabs: ResultTabsWidget,
    current_file: Option<PathBuf>,
    pristine_text: String,
    current_text_len: usize,
    is_dirty: bool,
}

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
    status_activity: Option<crate::db::DbActivityGuard>,
    status_activity_label: String,
    completed_statement_indices: HashSet<usize>,
    total_units: Option<usize>,
}

#[derive(Clone)]
struct PendingTableBrowseLast {
    request: TableBrowsePageRequest,
    rows: Vec<Vec<String>>,
    error: Option<String>,
}

#[derive(Clone)]
struct PendingTableBrowseRefresh {
    request: TableBrowsePageRequest,
    error: Option<String>,
}

fn status_connection_label(
    connection_info: Option<&crate::db::ConnectionInfo>,
    has_live_connection: bool,
) -> String {
    connection_info
        .filter(|_| has_live_connection)
        .map(|info| format!("{} ({})", info.name, info.db_type))
        .unwrap_or_else(|| "not connected".to_string())
}

fn status_bar_content_label(connection_label: &str, activity: Option<&str>) -> String {
    activity.map_or_else(
        || connection_label.to_string(),
        |activity| format!("{} | {}", connection_label, activity),
    )
}

fn status_connection_color(is_connected: bool) -> Color {
    if is_connected {
        theme::status_connected()
    } else {
        theme::status_disconnected()
    }
}

fn status_bar_pulse_value(pulse_frame: usize) -> f64 {
    let offset = safe_rem(pulse_frame, 200);
    let value = if offset <= 100 { offset } else { 200 - offset };
    value as f64
}

fn activity_pulse_color(pulse_frame: usize, resting_color: Color, active_color: Color) -> Color {
    let progress = safe_div(status_bar_pulse_value(pulse_frame), 100.0);
    let eased_progress = progress * progress * (3.0 - 2.0 * progress);
    let (start_r, start_g, start_b) = resting_color.to_rgb();
    let (end_r, end_g, end_b) = active_color.to_rgb();
    let interpolate = |start: u8, end: u8| {
        (f64::from(start) + (f64::from(end) - f64::from(start)) * eased_progress).round() as u8
    };

    Color::from_rgb(
        interpolate(start_r, end_r),
        interpolate(start_g, end_g),
        interpolate(start_b, end_b),
    )
}

fn query_cancel_activity_color(pulse_frame: usize, active: bool, hovered: bool) -> Color {
    let base = if active {
        activity_pulse_color(
            pulse_frame,
            theme::button_cancel(),
            theme::button_cancel_active(),
        )
    } else {
        theme::button_cancel()
    };
    if hovered {
        theme::hover_feedback_color(base)
    } else {
        base
    }
}

fn latest_status_activity(
    activities: &[crate::db::DbActivitySnapshot],
) -> Option<&crate::db::DbActivitySnapshot> {
    activities
        .iter()
        .max_by_key(|activity| (activity.started_at, activity.id))
}

fn latest_query_cancel_target<I>(snapshots: I) -> Option<CancelTargetSnapshot>
where
    I: IntoIterator<Item = CancelTargetSnapshot>,
{
    snapshots
        .into_iter()
        .filter(|snapshot| {
            matches!(
                snapshot.execution_state,
                ExecutionState::RunningStatement
                    | ExecutionState::RunningScript
                    | ExecutionState::LazyFetchOnly
                    | ExecutionState::CancelRequested
                    | ExecutionState::ClosingCursor
            )
        })
        .max_by_key(|snapshot| (snapshot.operation_id, snapshot.editor_id, snapshot.tab_id))
}

fn cancel_target_is_pending(
    snapshot: &CancelTargetSnapshot,
    pending_queries: &HashMap<QueryOperationToken, QueryCancelPhase>,
    pending_lazy_fetches: &HashSet<u64>,
) -> bool {
    if !matches!(
        snapshot.lazy_state,
        crate::db::session_policy::LazyFetchState::None
    ) {
        pending_lazy_fetches.contains(&snapshot.operation_id)
    } else {
        pending_queries.contains_key(&QueryOperationToken::from_cancel_snapshot(snapshot))
    }
}

#[cfg(test)]
fn latest_query_cancel_tab_id<I>(snapshots: I) -> Option<QueryTabId>
where
    I: IntoIterator<Item = CancelTargetSnapshot>,
{
    latest_query_cancel_target(snapshots).map(|snapshot| snapshot.tab_id)
}

struct StatusBarWidget {
    root: Flex,
    connection_indicator: Frame,
    content: Frame,
    additional_count: Frame,
    pulse_frame: usize,
}

impl StatusBarWidget {
    const HORIZONTAL_MARGIN: i32 = 10;
    const VERTICAL_MARGIN: i32 = 3;
    const TEXT_HORIZONTAL_PADDING: i32 = 12;

    fn new() -> Self {
        let mut root = Flex::default().row();
        root.set_frame(FrameType::FlatBox);
        root.set_color(theme::status_bar_default());
        root.set_margins(
            Self::HORIZONTAL_MARGIN,
            Self::VERTICAL_MARGIN,
            Self::HORIZONTAL_MARGIN,
            Self::VERTICAL_MARGIN,
        );

        let mut connection_indicator = Frame::default().with_label("●");
        connection_indicator.set_frame(FrameType::NoBox);
        connection_indicator.set_label_color(theme::status_disconnected());
        connection_indicator.set_align(Align::Center | Align::Inside);

        let mut content = Frame::default();
        content.set_frame(FrameType::NoBox);
        content.set_label_color(theme::text_primary());
        content.set_align(Align::Inside | Align::Left);

        let mut additional_count = Frame::default();
        additional_count.set_frame(FrameType::NoBox);
        additional_count.set_label_color(theme::text_primary());
        additional_count.set_align(Align::Center | Align::Inside);

        root.fixed(&connection_indicator, 14);
        root.resizable(&content);
        root.fixed(&additional_count, 1);
        root.end();

        Self {
            root,
            connection_indicator,
            content,
            additional_count,
            pulse_frame: 0,
        }
    }

    fn render(
        &mut self,
        connection_info: Option<&crate::db::ConnectionInfo>,
        has_live_connection: bool,
        activity: Option<&crate::db::DbActivitySnapshot>,
        additional_count: usize,
    ) {
        let is_connected = connection_info.is_some() && has_live_connection;
        self.connection_indicator
            .set_label_color(status_connection_color(is_connected));
        let connection_label = status_connection_label(connection_info, has_live_connection);
        let content_label = status_bar_content_label(
            &connection_label,
            activity.map(|activity| activity.activity.as_str()),
        );
        self.content.set_label(&content_label);
        self.content.set_tooltip(&content_label);
        let additional_count_label = if additional_count == 0 {
            String::new()
        } else {
            format!("+{}", additional_count)
        };
        self.additional_count.set_label(&additional_count_label);
        self.resize_additional_count();

        if activity.is_none() {
            self.root.set_color(theme::status_bar_default());
            self.pulse_frame = 0;
            self.redraw();
            return;
        }

        self.root.set_color(activity_pulse_color(
            self.pulse_frame,
            theme::status_bar_default(),
            theme::accent(),
        ));
        self.pulse_frame = self.pulse_frame.wrapping_add(STATUS_ANIMATION_STEP);
        self.redraw();
    }

    fn resize_additional_count(&mut self) {
        let additional_count_width = Self::label_width(&self.additional_count);
        self.root
            .fixed(&self.additional_count, additional_count_width);
        self.root.recalc();
    }

    fn label_width(frame: &Frame) -> i32 {
        let label = frame.label();
        if label.is_empty() {
            return 1;
        }
        set_font(frame.label_font(), frame.label_size());
        measure(&label, false)
            .0
            .max(0)
            .saturating_add(Self::TEXT_HORIZONTAL_PADDING)
    }

    fn was_deleted(&self) -> bool {
        self.root.was_deleted()
            || self.connection_indicator.was_deleted()
            || self.content.was_deleted()
            || self.additional_count.was_deleted()
    }

    fn redraw(&mut self) {
        self.root.redraw();
        self.connection_indicator.redraw();
        self.content.redraw();
        self.additional_count.redraw();
    }

    // Status content is derived from the connection and activity registry.
    // Legacy notification call sites may request a refresh, but cannot replace it.
    fn set_label(&mut self, _label: &str) {}
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueryCancelPhase {
    Requested,
    Dispatched,
}

fn query_cancel_phase_after_outcome(
    current: Option<QueryCancelPhase>,
    outcome: &QueryCancelOutcome,
) -> Option<QueryCancelPhase> {
    let current = current?;
    match outcome {
        QueryCancelOutcome::InterruptSent
        | QueryCancelOutcome::ForceStarted
        | QueryCancelOutcome::ForceCompleted => Some(QueryCancelPhase::Dispatched),
        QueryCancelOutcome::PendingInitialization => Some(current),
        QueryCancelOutcome::InterruptFailed(_) => Some(current),
        QueryCancelOutcome::AlreadyFinished
        | QueryCancelOutcome::StoppedBeforeInterrupt
        | QueryCancelOutcome::Failed(_)
        | QueryCancelOutcome::ForceFailed(_) => None,
    }
}

fn query_cancel_failure_message(outcome: &QueryCancelOutcome) -> Option<String> {
    match outcome {
        QueryCancelOutcome::InterruptFailed(message) => Some(format!(
            "Graceful cancel failed; force cancellation pending: {message}"
        )),
        QueryCancelOutcome::Failed(message) | QueryCancelOutcome::ForceFailed(message) => {
            Some(format!("Cancel failed: {message}"))
        }
        _ => None,
    }
}

fn progress_context_matches_cancel_token(
    context: &QueryProgressContext,
    token: QueryOperationToken,
) -> bool {
    context.operation_token == Some(token)
}

fn prune_abandoned_query_operations(operations: &mut HashSet<QueryOperationToken>) {
    let Some(newest_operation_id) = operations.iter().map(|token| token.operation_id).max() else {
        return;
    };
    let oldest_retained_operation_id =
        newest_operation_id.saturating_sub(MAX_ABANDONED_QUERY_OPERATION_AGE - 1);
    operations.retain(|token| token.operation_id >= oldest_retained_operation_id);
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionActivityEntry {
    connection_id: Option<ConnectionId>,
    connection_name: String,
    connection_state: String,
    scope: Option<String>,
    pool_size: u32,
    tab_name: String,
    result_tab: Option<usize>,
    state: String,
    database: String,
    current_activity: String,
    sql_preview: String,
    fetched_rows: usize,
    elapsed: String,
    active: bool,
}

fn connection_runtime_state_label(state: ConnectionRuntimeState) -> &'static str {
    match state {
        ConnectionRuntimeState::Connecting => "Connecting",
        ConnectionRuntimeState::Connected => "Connected",
        ConnectionRuntimeState::Transitioning => "Transitioning",
        ConnectionRuntimeState::Disconnected => "Disconnected",
        ConnectionRuntimeState::Failed(_) => "Failed",
    }
}

impl QueryProgressContext {
    fn new(
        execution_target: Option<ResultTabId>,
        activity_label: String,
        operation_token: Option<QueryOperationToken>,
    ) -> Self {
        let now = Instant::now();
        let status_activity_label = if execution_target.is_some() {
            "Saving result grid".to_string()
        } else {
            activity_label.clone()
        };
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
            status_activity: None,
            status_activity_label,
            completed_statement_indices: HashSet::new(),
            total_units: None,
        }
    }

    fn start_status_tracking(
        &mut self,
        total_units: Option<usize>,
        db_type: Option<DatabaseType>,
        connection_id: Option<ConnectionId>,
        status_activity: Option<crate::db::DbActivityGuard>,
    ) {
        let status_activity = status_activity
            .unwrap_or_else(|| crate::db::track_db_activity(&self.status_activity_label, db_type));
        if let Some(connection_id) = connection_id {
            status_activity.set_connection_id(connection_id);
        }
        status_activity.set_activity(&self.status_activity_label);
        if let Some(total) = total_units {
            status_activity.set_progress(crate::db::DbActivityProgress::Determinate {
                completed: 0,
                total: total as u64,
            });
        }
        self.total_units = total_units;
        self.status_activity = Some(status_activity);
    }

    fn update_status_activity(&self, state: &str) {
        if let Some(activity) = self.status_activity.as_ref() {
            activity.set_activity(format!("{} | {}", state, self.status_activity_label));
        }
    }

    fn mark_status_unit_complete(&mut self, statement_index: usize) {
        let Some(total) = self.total_units else {
            return;
        };
        self.completed_statement_indices.insert(statement_index);
        if let Some(activity) = self.status_activity.as_ref() {
            activity.set_progress(crate::db::DbActivityProgress::Determinate {
                completed: self.completed_statement_indices.len() as u64,
                total: total as u64,
            });
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
    connection_registry: ConnectionRegistry,
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
    ui_scale_bases: Vec<f32>,
    pub result_tabs: ResultTabsWidget,
    result_workspace_group: Group,
    result_toolbar: Flex,
    result_one_tab_per_query_check: CheckButton,
    result_one_tab_edit_gap: Frame,
    result_edit_check: CheckButton,
    result_insert_btn: Button,
    result_delete_btn: Button,
    result_save_btn: Button,
    result_cancel_btn: Button,
    result_page_unit_choice: Choice,
    execute_btn: Button,
    query_cancel_btn: Button,
    query_cancel_hovered: Arc<AtomicBool>,
    query_cancel_pulse_frame: usize,
    commit_btn: Button,
    rollback_btn: Button,
    transaction_isolation_choice: Choice,
    transaction_access_choice: Choice,
    result_grid_execution_targets: HashMap<QueryTabId, ResultTabId>,
    pending_table_browse_last: HashMap<QueryTabId, PendingTableBrowseLast>,
    pending_table_browse_refresh: HashMap<QueryTabId, PendingTableBrowseRefresh>,
    progress_contexts: HashMap<QueryTabId, QueryProgressContext>,
    abandoned_query_operations: HashSet<QueryOperationToken>,
    pending_query_cancellations: HashMap<QueryOperationToken, QueryCancelPhase>,
    pending_lazy_fetch_canceling_sessions: HashSet<u64>,
    orphaned_lazy_fetch_missing_since: HashMap<u64, Instant>,
    pub object_browser: MultiObjectBrowserWidget,
    status_bar: StatusBarWidget,
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
    pending_metadata_refresh_tabs: HashSet<QueryTabId>,
    latest_schema_request_id: u64,
    pub config: Arc<Mutex<AppConfig>>,
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

fn result_page_unit_for_choice_index(index: i32) -> usize {
    usize::try_from(index)
        .ok()
        .and_then(|index| RESULT_PAGE_UNITS.get(index))
        .copied()
        .unwrap_or(RESULT_PAGE_UNITS[RESULT_PAGE_DEFAULT_UNIT_INDEX])
}

fn result_page_choice_index_for_unit(unit: usize) -> Option<i32> {
    RESULT_PAGE_UNITS
        .iter()
        .position(|candidate| *candidate == unit)
        .and_then(|index| i32::try_from(index).ok())
}

fn result_page_control_center_offsets(available_width: i32) -> (i32, i32) {
    let remaining_width = available_width
        .saturating_sub(RESULT_PAGE_CONTROL_WIDTH)
        .max(0);
    let left = safe_div(remaining_width, 2);
    (left, remaining_width - left)
}

fn result_page_controls_fit(available_width: i32) -> bool {
    available_width >= RESULT_PAGE_CONTROL_WIDTH
}

fn result_page_control_feedback_color(
    event: Event,
    pointer_inside: bool,
    base: Color,
) -> Option<Color> {
    match event {
        Event::Enter | Event::Move => Some(theme::hover_feedback_color(base)),
        Event::Push => Some(theme::selection_soft()),
        Event::Drag => Some(if pointer_inside {
            theme::selection_soft()
        } else {
            base
        }),
        Event::Released => Some(if pointer_inside {
            theme::hover_feedback_color(base)
        } else {
            base
        }),
        Event::Leave | Event::Unfocus => Some(base),
        _ => None,
    }
}

fn install_result_page_control_feedback<W: WidgetBase>(widget: &mut W) {
    let base = widget.color();
    widget.handle(move |widget, event| {
        let event_x = app::event_x();
        let event_y = app::event_y();
        let pointer_inside = event_x >= widget.x()
            && event_x < widget.x() + widget.w()
            && event_y >= widget.y()
            && event_y < widget.y() + widget.h();
        if let Some(color) = result_page_control_feedback_color(event, pointer_inside, base) {
            if widget.color() != color {
                widget.set_color(color);
                widget.redraw();
            }
        }
        false
    });
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

    fn tab_display_label(tab: &QueryEditorTab) -> String {
        let document_label = match &tab.current_file {
            Some(path) => path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            None => tab.base_label.clone(),
        };
        let binding = tab.connection_binding.snapshot();
        let connection_prefix = binding
            .runtime
            .as_ref()
            .map(|runtime| {
                let mut connection = runtime.display_name();
                match runtime.state() {
                    ConnectionRuntimeState::Connecting => connection.push_str(" (connecting)"),
                    ConnectionRuntimeState::Transitioning => {
                        connection.push_str(" (transitioning)")
                    }
                    ConnectionRuntimeState::Disconnected => connection.push_str(" (offline)"),
                    ConnectionRuntimeState::Failed(_) => connection.push_str(" (failed)"),
                    ConnectionRuntimeState::Connected => {}
                }
                connection
            })
            .or_else(|| {
                binding
                    .detached_runtime
                    .as_ref()
                    .map(|runtime| format!("{} (detached)", runtime.display_name()))
            });
        let mut label = connection_prefix.map_or(document_label.clone(), |connection| {
            format!("{connection} · {document_label}")
        });
        if tab.sql_editor.is_query_running() {
            label.push_str(" · running");
        }
        if tab.is_dirty {
            label.push('*');
        }
        label
    }

    fn refresh_tab_label(&mut self, tab_id: QueryTabId) {
        let Some(index) = self.find_tab_index(tab_id) else {
            return;
        };
        let label = Self::tab_display_label(&self.editor_tabs[index]);
        self.query_tabs.set_tab_label(tab_id, &label);
        if self.active_editor_tab_id == tab_id {
            self.refresh_window_title();
        }
    }

    /// Connection state lives on the runtime, so every tab bound to it renders a
    /// stale label until it is redrawn. Refreshing only the tab that triggered
    /// the state change leaves its siblings showing the previous state.
    fn refresh_tab_labels_for_connection(&mut self, connection_id: ConnectionId) {
        let tab_ids = self
            .editor_tabs
            .iter()
            .filter(|tab| tab.connection_binding.snapshot().connection_id() == Some(connection_id))
            .map(|tab| tab.tab_id)
            .collect::<Vec<_>>();
        for tab_id in tab_ids {
            self.refresh_tab_label(tab_id);
        }
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
        self.sql_editor.dismiss_signature_popup();
        for tab in &self.editor_tabs {
            tab.sql_editor.try_hide_intellisense_popup();
            tab.sql_editor.dismiss_signature_popup();
        }
    }

    fn try_hide_all_intellisense_popups(state: &Arc<Mutex<Self>>) -> bool {
        match state.try_lock() {
            Ok(state) => state.hide_all_intellisense_popups(),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                poisoned.into_inner().hide_all_intellisense_popups();
            }
            Err(std::sync::TryLockError::WouldBlock) => return false,
        }
        true
    }

    fn schedule_hide_all_intellisense_popups(
        state: std::sync::Weak<Mutex<Self>>,
        retries_remaining: u8,
    ) {
        const POPUP_LIFECYCLE_RETRY_SECONDS: f64 = 0.01;
        crate::ui::ui_timeout::schedule(POPUP_LIFECYCLE_RETRY_SECONDS, move || {
            let Some(state) = state.upgrade() else {
                return;
            };
            if Self::try_hide_all_intellisense_popups(&state) || retries_remaining == 0 {
                return;
            }
            Self::schedule_hide_all_intellisense_popups(
                Arc::downgrade(&state),
                retries_remaining.saturating_sub(1),
            );
        });
    }

    fn hide_all_intellisense_popups_without_blocking(state: &Arc<Mutex<Self>>) {
        const POPUP_LIFECYCLE_LOCK_RETRIES: u8 = 20;
        if !Self::try_hide_all_intellisense_popups(state) {
            Self::schedule_hide_all_intellisense_popups(
                Arc::downgrade(state),
                POPUP_LIFECYCLE_LOCK_RETRIES,
            );
        }
    }

    fn find_tab_index(&self, tab_id: QueryTabId) -> Option<usize> {
        self.editor_tabs.iter().position(|tab| tab.tab_id == tab_id)
    }

    fn result_tabs_for_tab(&self, tab_id: QueryTabId) -> Option<ResultTabsWidget> {
        self.editor_tabs
            .iter()
            .find(|tab| tab.tab_id == tab_id)
            .map(|tab| tab.result_tabs.clone())
    }

    fn abort_lazy_fetch_session_in_all_workspaces(&self, session_id: u64) -> bool {
        self.editor_tabs.iter().fold(false, |aborted, tab| {
            let mut result_tabs = tab.result_tabs.clone();
            result_tabs.abort_lazy_fetch_session(session_id) || aborted
        })
    }

    fn result_workspaces_contain_lazy_fetch(&self, session_id: u64) -> bool {
        self.editor_tabs
            .iter()
            .any(|tab| tab.result_tabs.lazy_fetch_sessions().contains(&session_id))
    }

    fn normalize_scope_name(scope: Option<String>) -> Option<String> {
        scope
            .map(|scope| scope.trim().to_string())
            .filter(|scope| !scope.is_empty())
    }

    fn synchronize_scope_for_connection(
        &mut self,
        connection_id: ConnectionId,
        scope: Option<String>,
    ) -> bool {
        let scope = Self::normalize_scope_name(scope);
        let tab_ids = self
            .editor_tabs
            .iter()
            .filter(|tab| tab.connection_binding.snapshot().connection_id() == Some(connection_id))
            .map(|tab| tab.tab_id)
            .collect::<Vec<_>>();
        let changed = self.editor_tabs.iter().any(|tab| {
            let binding = tab.connection_binding.snapshot();
            binding.connection_id() == Some(connection_id) && binding.scope != scope
        });

        for tab in self
            .editor_tabs
            .iter()
            .filter(|tab| tab.connection_binding.snapshot().connection_id() == Some(connection_id))
        {
            tab.connection_binding.set_scope(scope.clone());
        }
        self.object_browser
            .set_selected_scope_for_connection(connection_id, scope);

        if changed {
            self.clear_metadata_for_connection(connection_id);
            for tab_id in tab_ids {
                self.mark_metadata_refresh_pending(tab_id);
            }
        }
        changed
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
        let previous_tab_id = self.active_editor_tab_id;
        if previous_tab_id != tab_id && self.pending_connection_metadata_refresh {
            self.pending_metadata_refresh_tabs.insert(previous_tab_id);
        }
        let tab = self.editor_tabs[index].clone();
        self.active_editor_tab_id = tab_id;
        for editor_tab in &self.editor_tabs {
            let mut widget = editor_tab.result_tabs.get_widget();
            if editor_tab.tab_id == tab_id {
                widget.resize(
                    self.result_workspace_group.x(),
                    self.result_workspace_group.y(),
                    self.result_workspace_group.w(),
                    self.result_workspace_group.h(),
                );
                widget.show();
            } else {
                widget.hide();
            }
        }
        self.result_tabs = tab.result_tabs.clone();
        self.schema_intellisense_data = tab.intellisense_data.clone();
        self.schema_highlight_data = tab.highlight_data.clone();
        let binding_snapshot = tab.connection_binding.snapshot();
        if let Some(runtime) = binding_snapshot
            .runtime
            .as_ref()
            .or(binding_snapshot.detached_runtime.as_ref())
        {
            self.object_browser.add_runtime(runtime.clone());
        }
        self.object_browser
            .set_active_connection(binding_snapshot.connection_id());
        if let Some(connection) = binding_snapshot.connection() {
            self.connection = connection.clone();
            let connection_snapshot = crate::db::try_lock_connection(&connection).map(|guard| {
                (
                    guard.is_connected() && guard.has_connection_handle(),
                    guard.get_info().clone(),
                )
            });
            let (has_live_connection, connection_info) = connection_snapshot
                .map(|(live, info)| (live, live.then_some(info)))
                .unwrap_or((false, None));
            self.has_live_connection = has_live_connection;
            *self
                .connection_info
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = connection_info;
        } else {
            self.has_live_connection = false;
            *self
                .connection_info
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        }
        self.pending_connection_metadata_refresh =
            self.has_live_connection && self.pending_metadata_refresh_tabs.remove(&tab_id);
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
        self.refresh_result_edit_controls();
        self.refresh_tab_label(tab_id);
        self.refresh_connection_dependent_controls();
        self.sync_transaction_mode_controls();
        self.render_status_bar();
        self.refresh_window_title();
        self.start_pending_metadata_refresh_if_ready();
        true
    }

    fn is_any_query_running(&self) -> bool {
        self.sql_editor.is_query_running()
            || self
                .editor_tabs
                .iter()
                .any(|tab| tab.sql_editor.is_query_running())
    }

    fn has_cancelable_query_activity(&self) -> bool {
        self.editor_tabs.iter().any(|tab| {
            tab.sql_editor.is_query_running()
                || tab.sql_editor.active_lazy_fetch_session().is_some()
        })
    }

    fn active_connection_id(&self) -> Option<ConnectionId> {
        self.editor_tabs
            .iter()
            .find(|tab| tab.tab_id == self.active_editor_tab_id)
            .and_then(|tab| tab.connection_binding.snapshot().connection_id())
    }

    fn active_connection_runtime(&self) -> Option<Arc<ConnectionRuntime>> {
        self.active_connection_id()
            .and_then(|id| self.connection_registry.get(id))
    }

    fn bind_active_unbound_tab_to_selected_database(&mut self) -> Result<(), String> {
        let tab_id = self.active_editor_tab_id;
        let Some(tab_index) = self.find_tab_index(tab_id) else {
            return Err("No query tab is open".to_string());
        };
        if self.editor_tabs[tab_index].sql_editor.is_query_running() {
            return Ok(());
        }
        let binding = self.editor_tabs[tab_index].connection_binding.clone();
        let binding_snapshot = binding.snapshot();
        if binding_snapshot.runtime.is_some() {
            return Ok(());
        }

        let Some((connection_id, scope)) = self.object_browser.selected_connection_context() else {
            return Err(
                "This query tab is not bound to a database, and no database is selected"
                    .to_string(),
            );
        };
        let Some(runtime) = self.connection_registry.get(connection_id) else {
            return Err("The selected database is no longer available".to_string());
        };
        binding
            .bind_if_revision(binding_snapshot.revision, runtime, scope)
            .map_err(|_| "The query tab connection changed before execution".to_string())?;

        let browser_snapshot = self
            .object_browser
            .metadata_snapshot_for_connection(connection_id);
        let existing_metadata = {
            let tab = &self.editor_tabs[tab_index];
            (
                tab.intellisense_data
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone(),
                tab.highlight_data.clone(),
            )
        };
        let (intellisense_data, highlight_data) =
            MainWindow::editor_metadata_seed(Some(existing_metadata), browser_snapshot.as_ref());
        {
            let tab = &mut self.editor_tabs[tab_index];
            *tab.intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = intellisense_data;
            tab.highlight_data = highlight_data.clone();
            tab.sql_editor
                .update_highlight_data_deferred(highlight_data);
        }
        if browser_snapshot.is_none() {
            self.pending_metadata_refresh_tabs.insert(tab_id);
        }
        let _ = self.set_active_editor_tab(tab_id);
        Ok(())
    }

    fn active_schema_update_target(&self) -> Result<Option<ActiveSchemaUpdateTarget>, ()> {
        let Some(tab) = self
            .editor_tabs
            .iter()
            .find(|tab| tab.tab_id == self.active_editor_tab_id)
        else {
            return Ok(None);
        };
        let binding = tab.connection_binding.snapshot();
        let Some(runtime) = binding.runtime else {
            return Ok(None);
        };
        let connection = runtime.connection();
        let Some(connection) = crate::db::try_lock_connection(&connection) else {
            return Err(());
        };
        Ok(Some(ActiveSchemaUpdateTarget {
            query_tab_id: tab.tab_id,
            connection_id: runtime.id(),
            connection_generation: connection.connection_generation(),
            binding_revision: binding.revision,
            request_id: self.latest_schema_request_id,
            db_type: connection.db_type(),
            scope: binding.scope,
        }))
    }

    fn remove_idle_transient_runtimes(&mut self) -> usize {
        let runtime_ids = self
            .connection_registry
            .runtimes()
            .into_iter()
            .map(|runtime| runtime.id())
            .collect::<Vec<_>>();
        let mut removed = 0;
        for connection_id in runtime_ids {
            if self
                .connection_registry
                .remove_transient_if_idle(connection_id)
            {
                self.object_browser.remove_runtime(connection_id);
                removed += 1;
            }
        }
        removed
    }

    fn clear_metadata_for_connection(&mut self, connection_id: ConnectionId) {
        let active_is_affected = self.active_connection_id() == Some(connection_id);
        for tab in self
            .editor_tabs
            .iter_mut()
            .filter(|tab| tab.connection_binding.snapshot().connection_id() == Some(connection_id))
        {
            *tab.intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = IntellisenseData::new();
            tab.highlight_data = HighlightData::new();
            tab.sql_editor
                .update_highlight_data_deferred(HighlightData::new());
        }
        if active_is_affected {
            self.schema_highlight_data = HighlightData::new();
            self.sql_editor
                .update_highlight_data_deferred(HighlightData::new());
        }
    }

    fn mark_metadata_refresh_pending(&mut self, tab_id: QueryTabId) {
        self.pending_metadata_refresh_tabs.insert(tab_id);
        if self.active_editor_tab_id == tab_id {
            self.pending_connection_metadata_refresh = self.has_live_connection;
        }
    }

    fn clear_pending_metadata_for_connection(&mut self, connection_id: ConnectionId) {
        let tab_ids = self
            .editor_tabs
            .iter()
            .filter(|tab| tab.connection_binding.snapshot().connection_id() == Some(connection_id))
            .map(|tab| tab.tab_id)
            .collect::<Vec<_>>();
        self.pending_metadata_refresh_tabs
            .retain(|tab_id| !tab_ids.contains(tab_id));
        if self.active_connection_id() == Some(connection_id) {
            self.pending_connection_metadata_refresh = false;
        }
    }

    fn has_work_for_connection(&self, connection_id: ConnectionId) -> bool {
        self.editor_tabs.iter().any(|tab| {
            tab.connection_binding.snapshot().connection_id() == Some(connection_id)
                && (tab.sql_editor.is_query_running()
                    || tab.sql_editor.active_lazy_fetch_session().is_some())
        })
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
        self.active_editor_tab_id == tab_id
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

    fn reconcile_orphaned_canceling_lazy_fetches(&mut self) -> bool {
        let missing_sessions = inactive_pending_lazy_fetch_sessions(
            &self.pending_lazy_fetch_canceling_sessions,
            |session_id| self.lazy_fetch_session_is_active_in_editor(session_id),
        );
        let missing_session_set = missing_sessions.iter().copied().collect::<HashSet<_>>();
        self.orphaned_lazy_fetch_missing_since
            .retain(|session_id, _| missing_session_set.contains(session_id));

        let now = Instant::now();
        let mut orphaned_sessions = Vec::new();
        for session_id in missing_sessions {
            let missing_since = self
                .orphaned_lazy_fetch_missing_since
                .entry(session_id)
                .or_insert(now);
            if orphaned_lazy_fetch_grace_expired(*missing_since, now) {
                orphaned_sessions.push(session_id);
            }
        }
        for session_id in &orphaned_sessions {
            self.orphaned_lazy_fetch_missing_since.remove(session_id);
            self.mark_lazy_fetch_cancelled_without_status(*session_id);
        }
        !orphaned_sessions.is_empty()
    }

    fn mark_lazy_fetch_result_tab_cancelled(&mut self, session_id: u64) {
        let mut result_tab_ids = Vec::new();
        for (query_tab_id, context) in self.progress_contexts.iter_mut() {
            let Some(statement_index) = context.lazy_fetch_sessions.get(&session_id).copied()
            else {
                continue;
            };
            context.active_statement_index = Some(statement_index);
            context.state_label = ResultTabStatus::Cancelled.label().to_string();
            context.update_status_activity(ResultTabStatus::Cancelled.label());
            if let Some(result_tab_id) = context.result_tab_id_for_statement(statement_index) {
                result_tab_ids.push((*query_tab_id, result_tab_id));
            }
        }
        result_tab_ids.sort_unstable();
        result_tab_ids.dedup();
        for (query_tab_id, result_tab_id) in result_tab_ids {
            if let Some(mut result_tabs) = self.result_tabs_for_tab(query_tab_id) {
                result_tabs.mark_statement_cancelled_by_id(result_tab_id);
            }
        }
    }

    fn mark_lazy_fetch_result_tab_closed(&mut self, session_id: u64) {
        self.pending_lazy_fetch_canceling_sessions
            .remove(&session_id);
        self.orphaned_lazy_fetch_missing_since.remove(&session_id);
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
        for (query_tab_id, context) in &self.progress_contexts {
            let result_tabs = self.result_tabs_for_tab(*query_tab_id);
            let unmapped = context.lazy_fetch_sessions_without_result_tab_mapping(|tab_id| {
                result_tabs
                    .as_ref()
                    .and_then(|result_tabs| result_tabs.lazy_fetch_session_for_id(tab_id))
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
            self.orphaned_lazy_fetch_missing_since.remove(session_id);
        }
        for session_id in &sessions_to_cancel {
            self.abort_lazy_fetch_session_in_all_workspaces(*session_id);
        }
        for tab_id in finished_contexts {
            self.finish_progress_context(tab_id);
        }
        sessions_to_cancel
    }

    fn finish_progress_context(&mut self, tab_id: QueryTabId) {
        let mut finished_target = None;
        if let Some(context) = self.progress_contexts.remove(&tab_id) {
            finished_target = context.execution_target;
            if let Some(token) = context.operation_token {
                self.pending_query_cancellations.remove(&token);
            }
            for session_id in context.lazy_fetch_sessions.keys() {
                self.pending_lazy_fetch_canceling_sessions
                    .remove(session_id);
                self.orphaned_lazy_fetch_missing_since.remove(session_id);
            }
        }
        // Only the execution this context belongs to may retire its routing.
        // A table page or grid-edit request registered while an older
        // execution was still winding down (its last lazy fetch being
        // cancelled, say) is finalized here too, and clearing by tab id alone
        // would strand it: with no execution target the next batch clears the
        // result grids and renders the page as a brand-new query result.
        if batch_owns_grid_target(
            finished_target,
            self.result_grid_execution_targets.get(&tab_id).copied(),
        ) {
            self.result_grid_execution_targets.remove(&tab_id);
        }
        self.start_pending_metadata_refresh_if_ready();
    }

    fn operation_progress_matches(
        &self,
        tab_id: QueryTabId,
        token: QueryOperationToken,
        progress: &QueryProgress,
    ) -> bool {
        let Some(editor) = self
            .find_tab_index(tab_id)
            .and_then(|index| self.editor_tabs.get(index))
            .map(|tab| &tab.sql_editor)
        else {
            return false;
        };
        if token.tab_id != tab_id
            || token.editor_id != editor.editor_instance_id()
            || token.operation_id == 0
        {
            return false;
        }
        match progress {
            QueryProgress::ExecutionFinished(event)
                if event.tab_id != token.tab_id
                    || event.editor_id != token.editor_id
                    || event.operation_id != token.operation_id
                    || event.connection_generation != token.connection_generation =>
            {
                return false;
            }
            QueryProgress::OperationFinished { token: inner_token }
            | QueryProgress::OperationAbandoned { token: inner_token } => {
                return *inner_token == token;
            }
            QueryProgress::CancelOutcome {
                token: inner_token, ..
            } => return *inner_token == token,
            _ => {}
        }
        if self.abandoned_query_operations.contains(&token) {
            return false;
        }
        let (current_operation_id, last_completed_operation_id) = editor.operation_lifecycle_ids();
        if operation_progress_token_matches_current_editor(
            tab_id,
            token,
            Some(editor.editor_instance_id()),
            current_operation_id,
            last_completed_operation_id,
            &self.abandoned_query_operations,
        ) {
            return true;
        }

        let Some(context) = self.progress_contexts.get(&tab_id) else {
            return false;
        };
        if context.operation_token == Some(token)
            && matches!(progress, QueryProgress::BatchFinished)
        {
            return true;
        }
        if registered_lazy_fetch_progress_matches(context, token, progress) {
            return true;
        }

        match progress {
            QueryProgress::LazyFetchSession {
                session_id,
                operation_id,
                connection_generation,
                ..
            } => unregistered_lazy_fetch_session_matches_context(
                context,
                token,
                *session_id,
                *operation_id,
                *connection_generation,
            ),
            QueryProgress::ExecutionFinished(event) => {
                context.operation_token == Some(token)
                    && event.tab_id == token.tab_id
                    && event.editor_id == token.editor_id
                    && event.operation_id == token.operation_id
                    && event.connection_generation == token.connection_generation
            }
            _ => false,
        }
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
    ) -> bool {
        let context_matches = self
            .progress_contexts
            .get(&tab_id)
            .is_some_and(|context| context.operation_token == Some(token));
        let cancellation_was_pending = self.pending_query_cancellations.remove(&token).is_some();
        self.remember_abandoned_query_operation(token);
        let result_tab_id = self.progress_contexts.get(&tab_id).and_then(|context| {
            if context.operation_token != Some(token) {
                return None;
            }
            context
                .active_statement_index
                .and_then(|statement_index| context.result_tab_id_for_statement(statement_index))
        });
        if let Some(result_tab_id) = result_tab_id {
            if let Some(mut result_tabs) = self.result_tabs_for_tab(tab_id) {
                result_tabs.mark_statement_cancelled_by_id(result_tab_id);
            }
        }
        if context_matches {
            self.finish_progress_context(tab_id);
        }
        cancellation_was_pending || context_matches
    }

    fn schedule_cursor_reset_when_tab_is_idle(&self, tab_id: QueryTabId) {
        let Some(editor) = self
            .find_tab_index(tab_id)
            .and_then(|index| self.editor_tabs.get(index))
            .map(|tab| tab.sql_editor.clone())
        else {
            return;
        };
        crate::ui::ui_timeout::schedule(0.01, move || {
            if !editor.is_query_running() {
                set_cursor(Cursor::Default);
                app::flush();
            }
        });
    }

    fn remember_abandoned_query_operation(&mut self, token: QueryOperationToken) {
        self.abandoned_query_operations.insert(token);
        prune_abandoned_query_operations(&mut self.abandoned_query_operations);
    }

    fn start_pending_metadata_refresh_if_ready(&mut self) {
        if !self.pending_connection_metadata_refresh
            || !self.has_live_connection
            || self.has_running_query_or_lazy_fetch_for_tab(self.active_editor_tab_id)
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
        if self.pending_connection_metadata_refresh {
            self.pending_metadata_refresh_tabs
                .insert(self.active_editor_tab_id);
        } else {
            self.pending_metadata_refresh_tabs
                .remove(&self.active_editor_tab_id);
        }
    }

    fn mark_lazy_fetch_result_tabs_closed<I>(&mut self, session_ids: I)
    where
        I: IntoIterator<Item = u64>,
    {
        for session_id in session_ids {
            self.mark_lazy_fetch_result_tab_closed(session_id);
        }
    }

    fn release_pooled_db_sessions_for_connection(&self, connection_id: ConnectionId) -> bool {
        self.editor_tabs
            .iter()
            .filter(|tab| tab.connection_binding.snapshot().connection_id() == Some(connection_id))
            .fold(false, |released_any, tab| {
                tab.sql_editor.release_pooled_db_session() || released_any
            })
    }

    fn release_all_resolved_pooled_db_sessions(&self) -> Result<bool, String> {
        let mut released_any = self.sql_editor.release_pooled_db_session_if_resolved()?;
        for tab in &self.editor_tabs {
            released_any |= tab.sql_editor.release_pooled_db_session_if_resolved()?;
        }
        Ok(released_any)
    }

    fn sync_mysql_auto_commit_overrides_with_global_setting(&self, enabled: bool) {
        let active_connection_id = self.active_connection_id();
        for tab in self
            .editor_tabs
            .iter()
            .filter(|tab| tab.connection_binding.snapshot().connection_id() == active_connection_id)
        {
            tab.sql_editor
                .sync_mysql_auto_commit_with_global_setting(enabled);
        }
    }

    fn oldest_lazy_fetch_session_for_tab(&self, tab_id: QueryTabId) -> Option<u64> {
        let connection_id = self
            .editor_tabs
            .iter()
            .find(|tab| tab.tab_id == tab_id)
            .and_then(|tab| tab.connection_binding.snapshot().connection_id())?;
        self.lazy_fetch_sessions_for_connection(connection_id)
            .into_iter()
            .min()
    }

    fn mark_lazy_fetch_cancelled_without_status(&mut self, session_id: u64) {
        self.mark_lazy_fetch_result_tab_cancelled(session_id);
        self.mark_lazy_fetch_result_tab_closed(session_id);
        self.abort_lazy_fetch_session_in_all_workspaces(session_id);
    }

    fn mark_lazy_fetch_cancelled(&mut self, session_id: u64, status_message: &str) {
        self.mark_lazy_fetch_cancelled_without_status(session_id);
        self.set_status_message(status_message);
        self.refresh_result_edit_controls();
    }

    fn mark_all_result_tabs_closed_for_clear(&mut self) {
        self.mark_result_tabs_closed_for_clear(self.active_editor_tab_id);
    }

    fn mark_result_tabs_closed_for_clear(&mut self, query_tab_id: QueryTabId) {
        let mut finished_context = false;
        if let Some(context) = self.progress_contexts.get_mut(&query_tab_id) {
            context.mark_all_result_statements_closed();
            if context.batch_finished {
                finished_context = true;
            }
        }
        if finished_context {
            self.finish_progress_context(query_tab_id);
        }
    }

    fn clear_result_grids_for_new_query_batch(&mut self, query_tab_id: QueryTabId) -> Vec<u64> {
        let Some(mut result_tabs) = self.result_tabs_for_tab(query_tab_id) else {
            return Vec::new();
        };
        let had_tabs = result_tabs.tab_count() > 0;
        let mut lazy_fetch_sessions = Vec::new();
        for session_id in result_tabs.lazy_fetch_sessions() {
            Self::push_unique_session_id(&mut lazy_fetch_sessions, session_id);
        }
        if let Some(context) = self.progress_contexts.get(&query_tab_id) {
            for session_id in context.lazy_fetch_sessions.keys().copied() {
                Self::push_unique_session_id(&mut lazy_fetch_sessions, session_id);
            }
        }
        result_tabs.clear_grids();
        self.mark_lazy_fetch_result_tabs_closed(lazy_fetch_sessions.clone());
        self.mark_result_tabs_closed_for_clear(query_tab_id);
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
        for tab in &self.editor_tabs {
            for session_id in tab.result_tabs.lazy_fetch_sessions() {
                Self::push_unique_session_id(&mut session_ids, session_id);
            }
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

    fn lazy_fetch_sessions_for_connection(&self, connection_id: ConnectionId) -> Vec<u64> {
        let query_tab_ids = self
            .editor_tabs
            .iter()
            .filter(|tab| tab.connection_binding.snapshot().connection_id() == Some(connection_id))
            .map(|tab| tab.tab_id)
            .collect::<HashSet<_>>();
        let mut session_ids = Vec::new();
        for tab in self
            .editor_tabs
            .iter()
            .filter(|tab| query_tab_ids.contains(&tab.tab_id))
        {
            for session_id in tab.result_tabs.lazy_fetch_sessions() {
                Self::push_unique_session_id(&mut session_ids, session_id);
            }
            Self::push_unique_session_id_if_some(
                &mut session_ids,
                tab.sql_editor.active_lazy_fetch_session(),
            );
        }
        for (tab_id, context) in &self.progress_contexts {
            if query_tab_ids.contains(tab_id) {
                for session_id in context.lazy_fetch_sessions.keys().copied() {
                    Self::push_unique_session_id(&mut session_ids, session_id);
                }
            }
        }
        session_ids
    }

    fn register_query_cancel_request(&mut self, token: QueryOperationToken) {
        self.pending_query_cancellations
            .insert(token, QueryCancelPhase::Requested);
    }

    fn query_cancel_is_pending(&self, token: QueryOperationToken) -> bool {
        self.pending_query_cancellations.contains_key(&token)
    }

    fn query_cancel_is_dispatched(&self, token: Option<QueryOperationToken>) -> bool {
        token.is_some_and(|token| {
            self.pending_query_cancellations.get(&token) == Some(&QueryCancelPhase::Dispatched)
        })
    }

    fn clear_query_cancel_request(&mut self, token: QueryOperationToken) -> bool {
        self.pending_query_cancellations.remove(&token).is_some()
    }

    fn mark_progress_context_canceling(&mut self, token: QueryOperationToken) -> bool {
        let Some(phase) = self.pending_query_cancellations.get_mut(&token) else {
            return false;
        };
        *phase = QueryCancelPhase::Dispatched;
        let Some(context) = self.progress_contexts.get_mut(&token.tab_id) else {
            return false;
        };
        if !progress_context_matches_cancel_token(context, token) {
            return false;
        }
        context.state_label = ResultTabStatus::Canceling.label().to_string();
        context.update_status_activity("Canceling query");
        let Some(statement_index) = context.canceling_statement_index() else {
            return false;
        };
        let Some(result_tab_id) = context.result_tab_id_for_statement(statement_index) else {
            return false;
        };
        if let Some(mut result_tabs) = self.result_tabs_for_tab(token.tab_id) {
            result_tabs.mark_statement_canceling_by_id(result_tab_id);
        }
        true
    }

    fn apply_query_cancel_outcome(
        &mut self,
        token: QueryOperationToken,
        outcome: &QueryCancelOutcome,
    ) -> bool {
        if !self.pending_query_cancellations.contains_key(&token) {
            return false;
        }
        let current_phase = self.pending_query_cancellations.get(&token).copied();
        let next_phase = query_cancel_phase_after_outcome(current_phase, outcome);
        match next_phase {
            Some(phase) => {
                self.pending_query_cancellations.insert(token, phase);
                if phase == QueryCancelPhase::Dispatched {
                    self.mark_progress_context_canceling(token);
                }
            }
            None => {
                self.pending_query_cancellations.remove(&token);
                let status = if matches!(outcome, QueryCancelOutcome::ForceFailed(_)) {
                    ResultTabStatus::Error
                } else {
                    ResultTabStatus::Running
                };
                self.restore_progress_context_after_cancel_failure(token, status);
            }
        }
        if let Some(message) = query_cancel_failure_message(outcome) {
            if let Some(mut result_tabs) = self.result_tabs_for_tab(token.tab_id) {
                result_tabs.append_message_lines(ResultMessageKind::Error, &[message]);
                result_tabs.select_messages_errors();
            }
        }
        true
    }

    fn restore_progress_context_after_cancel_failure(
        &mut self,
        token: QueryOperationToken,
        status: ResultTabStatus,
    ) {
        let result_tab_id = self
            .progress_contexts
            .get_mut(&token.tab_id)
            .and_then(|context| {
                if !progress_context_matches_cancel_token(context, token)
                    || context.state_label != ResultTabStatus::Canceling.label()
                {
                    return None;
                }
                context.state_label = status.label().to_string();
                context.update_status_activity(status.label());
                context
                    .running_statement_index
                    .and_then(|index| context.result_tab_id_for_statement(index))
            });
        if let Some(result_tab_id) = result_tab_id {
            if let Some(mut result_tabs) = self.result_tabs_for_tab(token.tab_id) {
                result_tabs.mark_statement_status_by_id(result_tab_id, status);
            }
        }
    }

    fn mark_lazy_fetch_canceling(&mut self, session_id: u64) -> bool {
        let active_lazy_fetch_tab_id = self.active_lazy_fetch_tab_id(session_id);
        let known_in_progress = self
            .progress_contexts
            .values()
            .any(|context| context.lazy_fetch_sessions.contains_key(&session_id));
        let known_in_results = self.result_workspaces_contain_lazy_fetch(session_id);
        if active_lazy_fetch_tab_id.is_none() && !known_in_progress && !known_in_results {
            return false;
        }
        self.pending_lazy_fetch_canceling_sessions
            .insert(session_id);
        let mut result_tab_ids = Vec::new();
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
            context.update_status_activity("Canceling lazy fetch");
            if let Some(result_tab_id) = context.result_tab_id_for_statement(statement_index) {
                result_tab_ids.push((*tab_id, result_tab_id));
            }
        }
        result_tab_ids.sort_unstable();
        result_tab_ids.dedup();
        let mut marked = false;
        for (query_tab_id, result_tab_id) in result_tab_ids {
            if let Some(mut result_tabs) = self.result_tabs_for_tab(query_tab_id) {
                result_tabs.mark_statement_canceling_by_id(result_tab_id);
                marked = true;
            }
        }
        for tab in &self.editor_tabs {
            let mut result_tabs = tab.result_tabs.clone();
            marked |= result_tabs.mark_lazy_fetch_canceling(session_id);
        }
        marked
    }

    fn mark_lazy_fetch_cancel_failed(&mut self, session_id: u64) -> bool {
        let was_pending = self
            .pending_lazy_fetch_canceling_sessions
            .remove(&session_id);
        self.orphaned_lazy_fetch_missing_since.remove(&session_id);
        let mut result_tab_updates = Vec::new();
        for (query_tab_id, context) in self.progress_contexts.iter_mut() {
            let Some(statement_index) = context.lazy_fetch_sessions.get(&session_id).copied()
            else {
                continue;
            };
            let status = if context.waiting_lazy_fetch_sessions.contains(&session_id) {
                ResultTabStatus::Waiting
            } else {
                ResultTabStatus::Fetching
            };
            context.state_label = status.label().to_string();
            context.update_status_activity(status.label());
            if let Some(result_tab_id) = context.result_tab_id_for_statement(statement_index) {
                result_tab_updates.push((*query_tab_id, result_tab_id, status));
            }
        }
        for (query_tab_id, result_tab_id, status) in result_tab_updates {
            if let Some(mut result_tabs) = self.result_tabs_for_tab(query_tab_id) {
                result_tabs.mark_statement_status_by_id(result_tab_id, status);
            }
        }
        was_pending
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

    fn render_status_bar(&mut self) -> bool {
        if self.status_bar.was_deleted() {
            return false;
        }
        if self.reconcile_orphaned_canceling_lazy_fetches() {
            self.refresh_result_edit_controls();
        }
        let conn_info = self
            .connection_info
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let activities = crate::db::active_db_activity_snapshots();
        let selected_activity = latest_status_activity(&activities);
        let displayed_registry_count = usize::from(selected_activity.is_some());
        self.status_bar.render(
            conn_info.as_ref(),
            self.has_live_connection,
            selected_activity,
            activities.len().saturating_sub(displayed_registry_count),
        );
        self.render_query_cancel_activity();
        true
    }

    fn render_query_cancel_activity(&mut self) {
        if self.query_cancel_btn.was_deleted() {
            return;
        }
        let was_active = self.query_cancel_pulse_frame != 0;
        let is_active = self.has_cancelable_query_activity();
        let hovered =
            self.query_cancel_hovered.load(Ordering::Relaxed) && self.query_cancel_btn.active_r();
        self.query_cancel_btn.set_color(query_cancel_activity_color(
            self.query_cancel_pulse_frame,
            is_active,
            hovered,
        ));
        if is_active {
            self.query_cancel_pulse_frame = self
                .query_cancel_pulse_frame
                .wrapping_add(STATUS_ANIMATION_STEP);
        } else {
            self.query_cancel_pulse_frame = 0;
        }
        self.query_cancel_btn.redraw();
        if was_active && !is_active {
            if let Some(mut query_toolbar) = self.query_cancel_btn.parent() {
                query_toolbar.redraw();
            }
        }
    }

    fn set_status_message(&mut self, _message: &str) {
        let _ = self.render_status_bar();
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

    fn retained_scope_change_blocker_for_connection(
        &self,
        connection_id: ConnectionId,
    ) -> Option<String> {
        if self.has_work_for_connection(connection_id) {
            return Some(
                "Cannot change scope while a query or lazy fetch is active on this connection."
                    .to_string(),
            );
        }
        for tab in self
            .editor_tabs
            .iter()
            .filter(|tab| tab.connection_binding.snapshot().connection_id() == Some(connection_id))
        {
            let Some(snapshot) = tab.sql_editor.pooled_session_activity_snapshot() else {
                continue;
            };
            let state = snapshot.retained_state;
            if crate::db::retained_session_state_preflight_decision(
                RetainedSessionPreflightAction::ScopeChange,
                state,
            ) == RetainedSessionPreflightDecision::RequireResolution
            {
                return Some(format!(
                    "Cannot change scope while tab '{}' has a {} DB session. Commit, rollback, or discard it first.",
                    Self::tab_display_label(tab),
                    state.label()
                ));
            }
        }
        None
    }

    fn retained_transaction_option_blocker(&self, action: &str) -> Option<String> {
        let action_label = format!("change {action}");
        self.retained_session_transaction_option_blocker(action, &action_label)
    }

    fn retained_session_editors(&self) -> Vec<SqlEditorWidget> {
        let active_connection_id = self.active_connection_id();
        self.editor_tabs
            .iter()
            .filter(|tab| {
                tab.connection_binding.snapshot().connection_id() == active_connection_id
                    && tab.sql_editor.pooled_session_activity_snapshot().is_some()
            })
            .map(|tab| tab.sql_editor.clone())
            .collect()
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
        let active_connection_id = self.active_connection_id();
        self.editor_tabs.iter().filter(|tab| {
            tab.connection_binding.snapshot().connection_id() == active_connection_id
        }).find_map(|tab| {
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
        let active_connection_id = self.active_connection_id();
        self.editor_tabs
            .iter()
            .filter(|tab| {
                tab.connection_binding.snapshot().connection_id() == active_connection_id
                    && tab
                        .sql_editor
                        .pooled_session_activity_snapshot()
                        .is_some_and(|snapshot| {
                            Self::retained_session_transaction_option_decision(action, snapshot)
                                == RetainedSessionPreflightDecision::Allow
                        })
            })
            .map(|tab| tab.sql_editor.clone())
            .collect()
    }

    fn retained_scope_update_for_connection(
        &self,
        connection_id: ConnectionId,
        scope: Option<String>,
    ) -> Option<RetainedScopeUpdate> {
        let scope = Self::normalize_scope_name(scope)?;
        let runtime = self.connection_registry.get(connection_id)?;
        let connection = runtime.connection();
        let conn_guard = crate::db::try_lock_connection(&connection)?;
        if !conn_guard.is_connected() {
            return None;
        }
        let db_type = conn_guard.db_type();
        if !db_type.has_connection_scope() {
            return None;
        }
        Some((
            db_type,
            conn_guard.connection_generation(),
            conn_guard.pool_context_epoch(),
            conn_guard.get_info().advanced.clone(),
            scope,
            self.editor_tabs
                .iter()
                .filter(|tab| {
                    tab.connection_binding.snapshot().connection_id() == Some(connection_id)
                        && tab.sql_editor.pooled_session_activity_snapshot().is_some()
                })
                .map(|tab| tab.sql_editor.clone())
                .collect(),
        ))
    }

    fn append_result_tab_request(&mut self, request: ResultTabRequest) {
        self.append_result_tab_request_for_tab(self.active_editor_tab_id, request);
    }

    fn append_result_tab_request_for_tab(
        &mut self,
        query_tab_id: QueryTabId,
        request: ResultTabRequest,
    ) {
        let Some(mut result_tabs) = self.result_tabs_for_tab(query_tab_id) else {
            return;
        };
        let tab_id = result_tabs.reserve_result_tab_id();
        let status_message = request.result.message.clone();
        result_tabs.ensure_statement_tab_by_id(tab_id, &request.label, true);
        result_tabs.display_result_by_id(tab_id, &request.result);
        if self.active_editor_tab_id == query_tab_id {
            self.refresh_result_edit_controls();
            self.set_status_message(&status_message);
        }
    }

    fn build_session_activity_result_request(&self) -> ResultTabRequest {
        let pool_size = self
            .config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .normalized_connection_pool_size();
        let runtimes = self.connection_registry.runtimes();
        let activities = crate::db::active_db_activity_snapshots();
        let current_activity_for = |connection_id: Option<ConnectionId>| {
            let labels = activities
                .iter()
                .filter(|activity| activity.connection_id == connection_id)
                .map(|activity| activity.activity.as_str())
                .collect::<Vec<_>>();
            if labels.is_empty() {
                "Idle".to_string()
            } else {
                labels.join("; ")
            }
        };
        let runtime_fields = |runtime: &Arc<ConnectionRuntime>| {
            let info = runtime.sanitized_info();
            (
                Some(runtime.id()),
                runtime.display_name(),
                connection_runtime_state_label(runtime.state()).to_string(),
                info.db_type.to_string(),
                current_activity_for(Some(runtime.id())),
            )
        };
        let mut entries = self
            .progress_contexts
            .iter()
            .filter_map(|(tab_id, context)| {
                let tab = self.editor_tabs.iter().find(|tab| tab.tab_id == *tab_id)?;
                let binding = tab.connection_binding.snapshot();
                let runtime = binding
                    .runtime
                    .as_ref()
                    .or(binding.detached_runtime.as_ref());
                let (connection_id, connection_name, connection_state, database, current_activity) =
                    runtime.map_or_else(
                        || {
                            (
                                None,
                                "Unbound".to_string(),
                                "Unbound".to_string(),
                                "-".to_string(),
                                current_activity_for(None),
                            )
                        },
                        &runtime_fields,
                    );
                let result_tab = context
                    .active_statement_index
                    .and_then(|statement_index| {
                        context
                            .result_tab_id_for_statement(statement_index)
                            .and_then(|id| {
                                self.result_tabs_for_tab(*tab_id)
                                    .and_then(|tabs| tabs.result_tab_index_for_id(id))
                            })
                    })
                    .map(|tab_index| tab_index + 1);
                let fetched_rows = context
                    .active_statement_index
                    .and_then(|statement_index| {
                        context.fetch_row_counts.get(&statement_index).copied()
                    })
                    .unwrap_or(0);
                Some((
                    *tab_id,
                    SessionActivityEntry {
                        connection_id,
                        connection_name,
                        connection_state,
                        scope: binding.scope,
                        pool_size,
                        tab_name: self
                            .tab_display_name(*tab_id)
                            .unwrap_or_else(|| format!("Tab {}", tab_id)),
                        result_tab,
                        state: context.state_label.clone(),
                        database,
                        current_activity,
                        sql_preview: context.activity_label.clone(),
                        fetched_rows,
                        elapsed: format_session_activity_elapsed(context.started_at.elapsed()),
                        active: true,
                    },
                ))
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
            let binding = tab.connection_binding.snapshot();
            let runtime = binding
                .runtime
                .as_ref()
                .or(binding.detached_runtime.as_ref());
            let (connection_id, connection_name, connection_state, _, current_activity) = runtime
                .map_or_else(
                    || {
                        (
                            None,
                            "Unbound".to_string(),
                            "Unbound".to_string(),
                            "-".to_string(),
                            current_activity_for(None),
                        )
                    },
                    &runtime_fields,
                );
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
                    connection_id,
                    connection_name,
                    connection_state,
                    scope: binding.scope,
                    pool_size,
                    tab_name: Self::tab_display_label(tab),
                    result_tab: None,
                    state: state.to_string(),
                    database: snapshot.db_type.to_string(),
                    current_activity,
                    sql_preview: "Idle pooled database session".to_string(),
                    fetched_rows: 0,
                    elapsed: "-".to_string(),
                    active: true,
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
                .map(|activity| {
                    let runtime = activity
                        .connection_id
                        .and_then(|connection_id| self.connection_registry.get(connection_id));
                    let (
                        connection_id,
                        connection_name,
                        connection_state,
                        database,
                        current_activity,
                    ) = runtime.as_ref().map_or_else(
                        || {
                            (
                                activity.connection_id,
                                "Unattributed".to_string(),
                                "Unknown".to_string(),
                                activity
                                    .db_type
                                    .map(|db_type| db_type.to_string())
                                    .unwrap_or_else(|| "-".to_string()),
                                current_activity_for(activity.connection_id),
                            )
                        },
                        &runtime_fields,
                    );
                    SessionActivityEntry {
                        connection_id,
                        connection_name,
                        connection_state,
                        scope: None,
                        pool_size,
                        tab_name: "Background".to_string(),
                        result_tab: None,
                        state: "Pool session active".to_string(),
                        database,
                        current_activity,
                        sql_preview: activity.activity,
                        fetched_rows: 0,
                        elapsed: format_session_activity_elapsed(activity.started_at.elapsed()),
                        active: true,
                    }
                }),
        );

        let represented_connections = entries
            .iter()
            .filter_map(|entry| entry.connection_id)
            .collect::<HashSet<_>>();
        entries.extend(runtimes.iter().filter_map(|runtime| {
            if represented_connections.contains(&runtime.id()) {
                return None;
            }
            let (connection_id, connection_name, connection_state, database, current_activity) =
                runtime_fields(runtime);
            Some(SessionActivityEntry {
                connection_id,
                connection_name,
                connection_state,
                scope: None,
                pool_size,
                tab_name: "-".to_string(),
                result_tab: None,
                state: "Idle".to_string(),
                database,
                current_activity,
                sql_preview: "-".to_string(),
                fetched_rows: 0,
                elapsed: "-".to_string(),
                active: false,
            })
        }));
        entries.sort_by(|left, right| {
            (left.connection_id, left.tab_name.as_str(), left.result_tab).cmp(&(
                right.connection_id,
                right.tab_name.as_str(),
                right.result_tab,
            ))
        });

        build_session_activity_result_request(entries)
    }

    fn refresh_result_edit_controls(&mut self) {
        if let Some(page_size) = self.result_tabs.current_table_browse_page_size() {
            if let Some(index) = result_page_choice_index_for_unit(page_size) {
                if self.result_page_unit_choice.value() != index {
                    self.result_page_unit_choice.set_value(index);
                }
            }
        }
        let origin_is_current = self.active_result_origin_is_current();
        let can_edit = origin_is_current && self.result_tabs.can_current_begin_edit_mode();
        let edit_active = self.result_tabs.is_current_edit_mode_enabled();
        let save_pending = self.result_tabs.is_current_save_pending();
        let query_running = self.sql_editor.is_query_running();
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

    fn result_origin_is_current_for_tab(
        &self,
        tab_id: QueryTabId,
        result_tabs: &ResultTabsWidget,
    ) -> bool {
        let current_origin = self
            .editor_tabs
            .iter()
            .find(|tab| tab.tab_id == tab_id)
            .and_then(|tab| tab.connection_binding.snapshot().execution_origin());
        result_tabs
            .active_result_origin()
            .is_some_and(|result_origin| current_origin.as_ref() == Some(&result_origin))
    }

    fn active_result_origin_is_current(&self) -> bool {
        self.result_origin_is_current_for_tab(self.active_editor_tab_id, &self.result_tabs)
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

        let has_query_tab = !self.editor_tabs.is_empty();
        // Regression guard: keep Execute enabled even when disconnected when a
        // tab exists. Scripts may begin with CONNECT (or @script that contains
        // CONNECT), so re-coupling this button to `is_connected` would break
        // reconnect workflows.
        if has_query_tab {
            self.execute_btn.activate();
            // Cancel targets an editor operation snapshot, which can still be
            // active while the primary connection is disconnected or replaced.
            self.query_cancel_btn.activate();
        } else {
            self.execute_btn.deactivate();
            self.query_cancel_btn.deactivate();
        }

        if is_connected {
            self.commit_btn.activate();
            self.rollback_btn.activate();
        } else {
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
const STATUS_ANIMATION_INTERVAL: f64 = 0.05;
const STATUS_ANIMATION_STEP: usize = 2;
const ORPHANED_LAZY_FETCH_GRACE_PERIOD: Duration = Duration::from_millis(250);
const MAX_ABANDONED_QUERY_OPERATION_AGE: u64 = 1_024;

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

fn execution_finished_event_matches_retained_context(
    event: &crate::db::session_policy::ExecutionFinishedEvent,
    callback_tab_id: QueryTabId,
    current_editor_id: Option<u64>,
    current_connection_generation: Option<u64>,
    context: Option<&QueryProgressContext>,
) -> bool {
    let Some(token) = context.and_then(|context| context.operation_token) else {
        return false;
    };
    event.tab_id == callback_tab_id
        && current_editor_id == Some(event.editor_id)
        && event.operation_id != 0
        && event.connection_generation != 0
        && token.tab_id == event.tab_id
        && token.editor_id == event.editor_id
        && token.operation_id == event.operation_id
        && token.connection_generation == event.connection_generation
        && current_connection_generation == Some(event.connection_generation)
}

fn unregistered_lazy_fetch_session_matches_context(
    context: &QueryProgressContext,
    token: QueryOperationToken,
    session_id: u64,
    operation_id: u64,
    connection_generation: u64,
) -> bool {
    context.operation_token == Some(token)
        && session_id != 0
        && operation_id == session_id
        && connection_generation == token.connection_generation
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

fn registered_lazy_fetch_progress_matches(
    context: &QueryProgressContext,
    token: QueryOperationToken,
    progress: &QueryProgress,
) -> bool {
    if context.operation_token != Some(token) {
        return false;
    }

    match progress {
        QueryProgress::SelectStart { index, .. }
        | QueryProgress::ResultEditMetadata { index, .. }
        | QueryProgress::Rows { index, .. }
        | QueryProgress::StatementFinished { index, .. } => context
            .lazy_fetch_sessions
            .values()
            .any(|statement_index| statement_index == index),
        QueryProgress::LazyFetchSession {
            index,
            session_id,
            operation_id,
            connection_generation,
        }
        | QueryProgress::LazyFetchClosed {
            index,
            session_id,
            operation_id,
            connection_generation,
            ..
        } => context.lazy_fetch_event_matches(
            *session_id,
            *index,
            *operation_id,
            *connection_generation,
        ),
        QueryProgress::LazyFetchWaiting { index, session_id } => {
            context.lazy_fetch_sessions.get(session_id) == Some(index)
        }
        QueryProgress::LazyFetchCanceling { session_id } => {
            context.lazy_fetch_sessions.contains_key(session_id)
        }
        QueryProgress::BatchFinished => !context.lazy_fetch_sessions.is_empty(),
        QueryProgress::WorkerPanicked { .. } => !context.lazy_fetch_sessions.is_empty(),
        _ => false,
    }
}

fn should_update_fetch_status(previous_count: usize, elapsed: Duration) -> bool {
    previous_count == 0 || elapsed >= FETCH_STATUS_UPDATE_INTERVAL
}

fn should_fail_table_browse_at_batch_end(
    table_browse_loading: bool,
    last_page_count_pending: bool,
) -> bool {
    table_browse_loading && !last_page_count_pending
}

/// Whether a finishing batch owns the per-tab table-browse state currently
/// registered for its query tab.
///
/// The worker publishes `query_running = false` before it queues
/// `BatchFinished`, so a table-browse or grid-edit request started in that
/// window registers its own routing before the previous batch is finalized.
/// Clearing that state by tab id alone strands the new result: with no
/// execution target, `ensure_result_tab_id` reserves a fresh result tab and
/// the table page renders as an ordinary query result.
fn batch_owns_grid_target(
    finished_target: Option<ResultTabId>,
    registered_target: Option<ResultTabId>,
) -> bool {
    finished_target == registered_target
}

/// Whether a finished statement's result may be offered a `WHERE` / `ORDER BY`
/// bar of its own.
///
/// A table page is this feature's own statement, not a user query. Its
/// filtered, ordered, page-bounded shape must never become the relation a new
/// bar filters: the page would be re-paged and the user's own query would be
/// gone from the chain.
fn result_can_carry_a_filter_bar(sql: &str) -> bool {
    !crate::ui::table_browse::is_materialized_grid_statement(sql)
}

pub struct MainWindow {
    state: Arc<Mutex<AppState>>,
}

#[derive(Clone)]
enum ConnectionResult {
    Success {
        connection_id: ConnectionId,
        info: Box<crate::db::ConnectionInfo>,
    },
    Failure {
        connection_id: ConnectionId,
        message: String,
        preserve_existing_connection: bool,
    },
    PoolResize {
        settings: Box<FontSettings>,
        result: Result<(), String>,
    },
}

enum FileActionResult {
    OpenInNewTab {
        path: PathBuf,
        result: Result<String, String>,
        binding: TabConnectionBinding,
    },
    Export {
        path: PathBuf,
        row_count: usize,
        result: Result<(), String>,
    },
    /// Export text bound for the clipboard: generated SQL that had to read
    /// metadata first, or any format the export dialog sent there. Carries
    /// `(text, status message)`; the copy itself happens on the main thread
    /// when this is drained.
    CopyToClipboard {
        result: Result<(String, String), String>,
    },
}

enum SaveTabOutcome {
    Saved,
    Cancelled,
    Failed(String),
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
        QueryProgress::Operation { progress, .. }
        | QueryProgress::StatementOrigin { progress, .. } => {
            result_pane_routes_for_progress_with_script_context(progress, script_transcript)
        }
        QueryProgress::OperationAbandoned { .. }
        | QueryProgress::OperationFinished { .. }
        | QueryProgress::CancelOutcome { .. } => Vec::new(),
        QueryProgress::StatementStart { .. } => Vec::new(),
        QueryProgress::SelectStart { columns, .. } => {
            if columns.is_empty() {
                Vec::new()
            } else {
                vec![ResultPaneRoute::DataGrid]
            }
        }
        QueryProgress::ResultEditMetadata { .. }
        | QueryProgress::Rows { .. }
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
        QueryProgress::StatementCancelledHistory { .. }
        | QueryProgress::BatchStart { .. }
        | QueryProgress::PromptInput { .. }
        | QueryProgress::RequestCancelOldestLazyFetchForSessionPool { .. }
        | QueryProgress::NotifyCancelOldestLazyFetchForSessionPool
        | QueryProgress::LazyFetchCancelFailed { .. }
        | QueryProgress::AutoCommitChanged { .. }
        | QueryProgress::ConnectionChanged { .. }
        | QueryProgress::DatabaseChanged { .. }
        | QueryProgress::ScopeChangedNotice { .. }
        | QueryProgress::WorkerPanicked { .. }
        | QueryProgress::ExecutionAbandoned { .. }
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

fn inactive_pending_lazy_fetch_sessions<F>(
    pending_canceling_sessions: &HashSet<u64>,
    mut session_is_active: F,
) -> Vec<u64>
where
    F: FnMut(u64) -> bool,
{
    let mut sessions = pending_canceling_sessions
        .iter()
        .copied()
        .filter(|session_id| !session_is_active(*session_id))
        .collect::<Vec<_>>();
    sessions.sort_unstable();
    sessions
}

fn orphaned_lazy_fetch_grace_expired(missing_since: Instant, now: Instant) -> bool {
    now.duration_since(missing_since) >= ORPHANED_LAZY_FETCH_GRACE_PERIOD
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
        if guard.find_tab_index(guard.active_editor_tab_id).is_none() {
            (None, Some("No query tab is open.".to_string()))
        } else if guard.sql_editor.is_query_running() {
            (
                None,
                Some("The active query tab is already running a query.".to_string()),
            )
        } else {
            (Some(guard.sql_editor.clone()), None)
        }
    };

    if let Some(message) = blocked_message {
        SqlEditorWidget::show_alert_dialog(&message);
    }

    editor
}

fn prepare_active_editor_for_execution(state: &Arc<Mutex<AppState>>) -> bool {
    let result = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .bind_active_unbound_tab_to_selected_database();
    if let Err(message) = result {
        SqlEditorWidget::show_alert_dialog(&message);
        return false;
    }
    true
}

fn cancel_oldest_lazy_fetch_if_session_pool_full(state: &Arc<Mutex<AppState>>) -> bool {
    let (connection_id, connection, configured_pool_size) = {
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(runtime) = state.active_connection_runtime() else {
            return false;
        };
        let configured_pool_size = state
            .config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .normalized_connection_pool_size();
        (runtime.id(), runtime.connection(), configured_pool_size)
    };
    let connection_pool_size = crate::db::try_lock_connection(&connection)
        .map(|connection| connection.connection_pool_size())
        .unwrap_or(configured_pool_size);

    let session_id = {
        let guard = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let active_sessions = guard.lazy_fetch_sessions_for_connection(connection_id);
        match session_pool_slot_action(active_sessions.len(), connection_pool_size) {
            SessionPoolSlotAction::None => return false,
            SessionPoolSlotAction::CancelLazyFetch => {}
        }
        let Some(session_id) = active_sessions.into_iter().min() else {
            return false;
        };
        session_id
    };

    request_lazy_fetch_cancel_for_session_pool(state, session_id)
}

fn run_sql_execution_request(state: &Arc<Mutex<AppState>>, request: SqlExecutionRequest) {
    if !prepare_active_editor_for_execution(state) {
        return;
    }
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
    if !prepare_active_editor_for_execution(state) {
        return;
    }
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
        let active_connection_id = s.active_connection_id();
        let connection_has_running_work = active_connection_id
            .is_some_and(|connection_id| s.has_work_for_connection(connection_id));
        let connection_has_lazy_fetch = active_connection_id.is_some_and(|connection_id| {
            !s.lazy_fetch_sessions_for_connection(connection_id)
                .is_empty()
        });
        if let Some(message) = transaction_option_block_message(
            connection_has_running_work,
            connection_has_lazy_fetch,
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
        kind: crate::db::SqlValueKind::Unknown,
    }
}

fn build_session_activity_result_request(entries: Vec<SessionActivityEntry>) -> ResultTabRequest {
    let columns = vec![
        session_activity_column("Connection ID", "NUMBER"),
        session_activity_column("Connection", "VARCHAR2"),
        session_activity_column("Connection State", "VARCHAR2"),
        session_activity_column("Scope", "VARCHAR2"),
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

    let has_active_entries = entries.iter().any(|entry| entry.active);
    let rows = if entries.is_empty() {
        vec![vec![
            "-".to_string(),
            "No connections".to_string(),
            "Unbound".to_string(),
            "-".to_string(),
            "-".to_string(),
            "-".to_string(),
            "-".to_string(),
            "-".to_string(),
            "Idle".to_string(),
            "Idle".to_string(),
            "-".to_string(),
            "-".to_string(),
            "-".to_string(),
        ]]
    } else {
        entries
            .into_iter()
            .map(|entry| {
                vec![
                    entry
                        .connection_id
                        .map(|connection_id| connection_id.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    entry.connection_name,
                    entry.connection_state,
                    entry.scope.unwrap_or_else(|| "-".to_string()),
                    entry.database,
                    entry.pool_size.to_string(),
                    entry.tab_name,
                    entry
                        .result_tab
                        .map(|index| index.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    entry.state,
                    entry.current_activity,
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
    fn clone_result_tabs_for_page_action(state: &Arc<Mutex<AppState>>) -> ResultTabsWidget {
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .result_tabs
            .clone()
    }

    fn clone_result_tabs_for_edit_action(
        state: &Arc<Mutex<AppState>>,
    ) -> Result<ResultTabsWidget, String> {
        let mut guard = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !guard.active_result_origin_is_current() {
            let err =
                "This result belongs to an older connection, reconnect, or scope and is read-only."
                    .to_string();
            guard.set_status_message(&err);
            guard.refresh_result_edit_controls();
            return Err(err);
        }
        if let Err(err) = validate_result_edit_action_allowed(guard.sql_editor.is_query_running()) {
            guard.set_status_message(&err);
            guard.refresh_result_edit_controls();
            return Err(err);
        }
        Ok(guard.result_tabs.clone())
    }

    fn prepare_result_export(
        state: &Arc<Mutex<AppState>>,
        choice: ExportChoice,
        db_type: Option<DatabaseType>,
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

        Ok(result_tabs.export_after_fetch_all(
            choice.format,
            choice.scope,
            choice.destination,
            db_type,
            callback,
        ))
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

        let binding = {
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
            MainWindow::binding_for_selected_database(&s)
        };

        let sender = file_sender.clone();
        thread::spawn(move || {
            let result = fs::read_to_string(&path).map_err(|err| err.to_string());
            let _ = sender.send(FileActionResult::OpenInNewTab {
                path,
                result,
                binding,
            });
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

        let binding = {
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
            MainWindow::binding_for_selected_database(&s)
        };

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
                    if let Some(tab_id) =
                        MainWindow::create_query_editor_tab_for_binding(&mut s, binding, true)
                    {
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

    /// Ask what to export, then write it to a file or put it on the clipboard.
    ///
    /// Both destinations go through the same deferred callback, because an
    /// export of the whole result may have to finish a lazy fetch first and only
    /// then knows what it is writing.
    fn export_current_results(
        state: &Arc<Mutex<AppState>>,
        file_sender: &std::sync::mpsc::Sender<FileActionResult>,
    ) {
        let (db_type, has_selection) = {
            let guard = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !guard.result_tabs.has_data() {
                crate::ui::alert_on_main("No results to export");
                return;
            }
            let has_selection = guard.result_tabs.has_grid_selection();
            let db_type = guard
                .active_connection_runtime()
                .map(|runtime| runtime.sanitized_info().db_type);
            (db_type, has_selection)
        };

        // `SQL Inserts` writes dialect-specific literals, so it is only on offer
        // while a connection can say which dialect that is.
        let formats: Vec<ExportFormat> = ExportFormat::ALL
            .into_iter()
            .filter(|format| db_type.is_some() || *format != ExportFormat::SqlInserts)
            .collect();
        let Some(choice) = crate::ui::result_export_dialog::show(&formats, has_selection) else {
            return;
        };

        let destination = match choice.destination {
            ExportDestination::Clipboard => None,
            ExportDestination::File => {
                let Some(path) = Self::ask_export_file_path(choice.format) else {
                    return;
                };
                Some(path)
            }
        };

        let sender = file_sender.clone();
        let deferred_sender = sender.clone();
        let deferred_destination = destination.clone();
        let format = choice.format;
        let export = match MainWindow::prepare_result_export(
            state,
            choice,
            db_type,
            Box::new(move |content, row_count| {
                Self::deliver_export(
                    &deferred_sender,
                    deferred_destination.clone(),
                    format,
                    content,
                    row_count,
                );
            }),
        ) {
            Ok(export) => export,
            Err(message) => {
                crate::ui::alert_on_main(&message);
                return;
            }
        };
        let Some((content, row_count)) = export else {
            return;
        };
        Self::deliver_export(&sender, destination, format, content, row_count);
    }

    /// Where the exported text should go. `None` means the user cancelled the
    /// save chooser.
    fn ask_export_file_path(format: ExportFormat) -> Option<PathBuf> {
        let mut dialog = FileDialog::new(FileDialogType::BrowseSaveFile);
        dialog.set_filter(&format.file_filter());
        dialog.show();
        let filename = dialog.filename();
        if filename.as_os_str().is_empty() {
            return None;
        }
        // The native chooser auto-appends an "All Files" entry after our single
        // filter, so it sits at index 1 (skip); index 0 → force the extension.
        Some(Self::apply_default_extension(
            filename,
            format.extension(),
            dialog.filter_value(),
            Some(1),
        ))
    }

    /// Hand finished export text to its destination. Writing runs off the main
    /// thread; the clipboard copy is queued so it happens on it.
    fn deliver_export(
        sender: &std::sync::mpsc::Sender<FileActionResult>,
        destination: Option<PathBuf>,
        format: ExportFormat,
        content: String,
        row_count: usize,
    ) {
        match destination {
            Some(path) => {
                let sender = sender.clone();
                thread::spawn(move || {
                    let result = fs::write(&path, content).map_err(|err| err.to_string());
                    let _ = sender.send(FileActionResult::Export {
                        path,
                        row_count,
                        result,
                    });
                    app::awake();
                });
            }
            None => {
                let message = format!("Copied {row_count} rows to clipboard as {}", format.label());
                let _ = sender.send(FileActionResult::CopyToClipboard {
                    result: Ok((content, message)),
                });
                app::awake();
            }
        }
    }

    /// Copy the visible grid's selection to the clipboard as SQL.
    ///
    /// `SQL Inserts` and `Where Clause` are immediate. `SQL Updates` needs the
    /// table's primary key for its WHERE clause, so it reads metadata on a worker
    /// thread and finishes when `FileActionResult::CopyToClipboard` is drained.
    fn copy_result_selection_as_sql(
        state: &Arc<Mutex<AppState>>,
        file_sender: &std::sync::mpsc::Sender<FileActionResult>,
        action: ResultTableContextAction,
    ) {
        let mut s = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let Some(runtime) = s.active_connection_runtime() else {
            return;
        };
        let db_type = runtime.sanitized_info().db_type;
        let Some(selection) = s.result_tabs.sql_export_context(db_type) else {
            return;
        };
        let row_count = selection.rows.len();

        let (sql, message) = match action {
            ResultTableContextAction::CopySqlInserts => (
                crate::ui::grid_sql_export::build_sql_inserts(&selection),
                format!("Copied {row_count} INSERT statements to clipboard"),
            ),
            ResultTableContextAction::CopyWhereClause => (
                crate::ui::grid_sql_export::build_where_clause(&selection),
                "Copied WHERE clause to clipboard".to_string(),
            ),
            ResultTableContextAction::CopySqlUpdates => {
                let Some(table) = selection.table.clone() else {
                    // No base table means no primary key to look up either.
                    let sql = crate::ui::grid_sql_export::build_sql_updates(&selection, &[]);
                    let message = format!(
                        "Copied {row_count} UPDATE statements (table unknown — WHERE omitted)"
                    );
                    Self::finish_clipboard_copy(&mut s, &sql, &message);
                    return;
                };
                let connection = runtime.connection();
                let scope = s
                    .active_connection_id()
                    .and_then(|id| s.object_browser.selected_scope_for_connection(id));
                let sender = file_sender.clone();
                drop(s);
                thread::spawn(move || {
                    let keys = ObjectBrowserWidget::load_primary_key_columns(
                        &connection,
                        scope.as_deref(),
                        &table,
                    );
                    let result = match keys {
                        Ok(keys) => {
                            let sql =
                                crate::ui::grid_sql_export::build_sql_updates(&selection, &keys);
                            let message = if keys.is_empty() {
                                format!(
                                    "Copied {row_count} UPDATE statements \
                                     (no primary key — WHERE omitted)"
                                )
                            } else {
                                format!("Copied {row_count} UPDATE statements to clipboard")
                            };
                            Ok((sql, message))
                        }
                        Err(err) => Err(err),
                    };
                    let _ = sender.send(FileActionResult::CopyToClipboard { result });
                    app::awake();
                });
                return;
            }
            _ => return,
        };

        Self::finish_clipboard_copy(&mut s, &sql, &message);
    }

    /// Put generated SQL on the clipboard and report it in the status bar.
    /// Put a `WHERE` / `ORDER BY` bar on a result that this backend can
    /// re-query, as the result arrives.
    ///
    /// Gating lives here rather than in the grid because the answer depends on
    /// the connection — Oracle can wrap a result whose column names repeat, the
    /// MySQL family cannot — and because a result that did not come from a
    /// SELECT has nothing to re-run.
    ///
    /// Attaching only adds the bar. The tab stays a query tab and keeps the
    /// rows and the grid editing it already had; nothing runs until a filter is
    /// actually applied.
    fn offer_result_filter(
        result_tabs: &mut ResultTabsWidget,
        result_tab_id: ResultTabId,
        db_type: crate::db::DatabaseType,
        scope: Option<String>,
        sql: &str,
        columns: &[String],
        intellisense_data: Arc<Mutex<IntellisenseData>>,
    ) {
        use crate::ui::result_filter::{
            derived_relation_sql, result_filter_support, ResultFilterSupport,
        };

        if !result_can_carry_a_filter_bar(sql) {
            return;
        }
        if matches!(
            result_filter_support(sql, columns, db_type),
            ResultFilterSupport::Blocked(_)
        ) {
            return;
        }

        let target = TableBrowseTarget {
            db_type,
            scope,
            table_name: "Result".to_string(),
            relation_sql: derived_relation_sql(sql),
            completion_name: String::new(),
            editable: false,
            // A derived relation has no name for the metadata engine to
            // resolve, so the result's own headers are what the filter fields
            // complete on.
            result_columns: columns.to_vec(),
        };
        result_tabs.attach_result_filter_bar_by_id(result_tab_id, target, intellisense_data, false);
    }

    fn finish_clipboard_copy(state: &mut AppState, sql: &str, message: &str) {
        if sql.is_empty() {
            return;
        }
        app::copy(sql);
        let conn_info = state
            .connection_info
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        state
            .status_bar
            .set_label(&format_status(message, &conn_info));
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

    fn start_status_activity_timer(state: &Arc<Mutex<AppState>>) {
        let weak_state = Arc::downgrade(state);
        crate::ui::ui_timeout::schedule(STATUS_ANIMATION_INTERVAL, move || {
            let Some(state_for_tick) = weak_state.upgrade() else {
                return;
            };
            let should_reschedule = match state_for_tick.try_lock() {
                Ok(mut state) => state.render_status_bar(),
                Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                    poisoned.into_inner().render_status_bar()
                }
                Err(std::sync::TryLockError::WouldBlock) => true,
            };
            if should_reschedule {
                MainWindow::start_status_activity_timer(&state_for_tick);
            }
        });
    }

    fn cancel_all_running_queries(state: &Arc<Mutex<AppState>>) {
        let (running_query_targets, lazy_fetch_sessions) = {
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut running_query_targets = s
                .editor_tabs
                .iter()
                .filter_map(|tab| {
                    tab.sql_editor
                        .running_operation_cancel_target_snapshot()
                        .filter(|snapshot| snapshot.operation_id != 0)
                })
                .collect::<Vec<_>>();
            if s.find_tab_index(s.active_editor_tab_id).is_none() && s.sql_editor.is_query_running()
            {
                if let Some(snapshot) = s
                    .sql_editor
                    .running_operation_cancel_target_snapshot()
                    .filter(|snapshot| snapshot.operation_id != 0)
                {
                    running_query_targets.push(snapshot);
                }
            }
            (running_query_targets, s.lazy_fetch_sessions_for_abort())
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

        if running_query_targets.is_empty() && lazy_fetch_requests.is_empty() {
            return;
        }

        for target in running_query_targets {
            Self::cancel_query_editor_target(state, target);
        }

        let mut s = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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

    fn cancel_latest_query_editor_tab(state: &Arc<Mutex<AppState>>) -> bool {
        let editors = {
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            s.editor_tabs
                .iter()
                .map(|tab| tab.sql_editor.clone())
                .collect::<Vec<_>>()
        };
        // The newest operation can finish between snapshot selection and the
        // exact-match cancel request. Rescan once so that another still-running
        // operation is not missed by the same button click.
        for _ in 0..2 {
            let target = latest_query_cancel_target(editors.iter().filter_map(|editor| {
                if editor.is_query_running() || editor.active_lazy_fetch_session().is_some() {
                    Some(editor.cancel_target_snapshot())
                } else {
                    None
                }
            }));
            let Some(target) = target else {
                return false;
            };
            if Self::cancel_query_editor_target(state, target) {
                return true;
            }
        }
        false
    }

    fn cancel_query_editor_tab(state: &Arc<Mutex<AppState>>, tab_id: QueryTabId) -> bool {
        let Some(editor) = ({
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            s.find_tab_index(tab_id)
                .map(|index| s.editor_tabs[index].sql_editor.clone())
        }) else {
            return false;
        };
        Self::cancel_query_editor_target(state, editor.cancel_target_snapshot())
    }

    fn cancel_query_editor_target(
        state: &Arc<Mutex<AppState>>,
        snapshot: CancelTargetSnapshot,
    ) -> bool {
        let Some(editor) = ({
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            s.find_tab_index(snapshot.tab_id)
                .map(|index| s.editor_tabs[index].sql_editor.clone())
        }) else {
            return false;
        };
        let target_is_pending = {
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            cancel_target_is_pending(
                &snapshot,
                &s.pending_query_cancellations,
                &s.pending_lazy_fetch_canceling_sessions,
            )
        };
        if target_is_pending {
            return true;
        }

        let mut requested = false;
        if !matches!(
            snapshot.lazy_state,
            crate::db::session_policy::LazyFetchState::None
        ) {
            let session_id = snapshot.operation_id;
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
        } else if editor.is_query_running() && snapshot.operation_id != 0 {
            let token = QueryOperationToken::from_cancel_snapshot(&snapshot);
            {
                let mut s = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if s.query_cancel_is_pending(token) {
                    return true;
                }
                s.register_query_cancel_request(token);
            }
            requested = editor.cancel_snapshot(snapshot);
            let mut s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !requested {
                s.clear_query_cancel_request(token);
            }
            s.refresh_result_edit_controls();
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

    fn resolve_pooled_sessions_before_runtime_disconnect(
        state: &Arc<Mutex<AppState>>,
        connection_id: ConnectionId,
    ) -> bool {
        let tab_ids = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .editor_tabs
            .iter()
            .filter(|tab| tab.connection_binding.snapshot().connection_id() == Some(connection_id))
            .map(|tab| tab.tab_id)
            .collect::<Vec<_>>();
        for tab_id in tab_ids {
            if !Self::resolve_pooled_session_before_action(
                state,
                tab_id,
                RetainedSessionPreflightAction::ConnectionTransition,
                "disconnect it",
                "disconnecting",
                "Commit and Disconnect",
                "Rollback and Disconnect",
            ) {
                return false;
            }
        }
        true
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
        font_settings::update_runtime_font_settings(&config);
        let connection = create_shared_connection();
        {
            let mut guard = crate::db::lock_connection(&connection);
            guard.set_connection_pool_size(config.normalized_connection_pool_size());
        }
        let connection_registry = ConnectionRegistry::new();
        let initial_binding = TabConnectionBinding::unbound();

        let ui_scale_bases = (0..app::screen_count())
            .map(app::screen_scale)
            .collect::<Vec<_>>();

        let current_group = fltk::group::Group::try_current();

        fltk::group::Group::set_current(None::<&fltk::group::Group>);

        let mut window = Window::default()
            .with_size(1200, 800)
            .with_label(&AppState::app_window_title())
            .center_screen();
        window.set_id("main_window");
        window.set_color(theme::window_bg());
        app_icon::apply_window_icon(&mut window);
        Self::apply_ui_scale_percent(
            &ui_scale_bases,
            config.normalized_ui_scale_percent(),
            Some(&window),
        );
        let compact_query_toolbar = window.w() < QUERY_TOOLBAR_COMPACT_BREAKPOINT;
        let query_toolbar_margin = if compact_query_toolbar {
            4
        } else {
            TOOLBAR_SPACING
        };
        let query_toolbar_spacing = if compact_query_toolbar {
            4
        } else {
            TOOLBAR_SPACING
        };
        let query_toolbar_button_width = if compact_query_toolbar {
            BUTTON_WIDTH_SMALL
        } else {
            BUTTON_WIDTH
        };
        let query_toolbar_isolation_width = if compact_query_toolbar {
            QUERY_TOOLBAR_COMPACT_CHOICE_WIDTH
        } else {
            TRANSACTION_ISOLATION_CHOICE_WIDTH
        };
        let query_toolbar_access_width = if compact_query_toolbar {
            QUERY_TOOLBAR_COMPACT_ACCESS_WIDTH
        } else {
            TRANSACTION_ACCESS_CHOICE_WIDTH
        };
        let query_toolbar_timeout_label_width = if compact_query_toolbar { 0 } else { 85 };
        let query_toolbar_timeout_width = if compact_query_toolbar {
            QUERY_TOOLBAR_COMPACT_NUMERIC_WIDTH
        } else {
            NUMERIC_INPUT_WIDTH
        };
        let query_toolbar_scale_button_width = if compact_query_toolbar {
            QUERY_TOOLBAR_COMPACT_SCALE_BUTTON_WIDTH
        } else {
            UI_SCALE_BUTTON_WIDTH
        };

        let toolbar_control_vertical_margin = safe_div(RESULT_TOOLBAR_HEIGHT - BUTTON_HEIGHT, 2);
        let mut main_flex = Flex::default_fill();
        main_flex.set_type(FlexType::Column);

        let menu_bar = MenuBarBuilder::build_with_recent_sql_files(&config.recent_sql_files);
        main_flex.fixed(&menu_bar, MENU_BAR_HEIGHT);

        let mut query_toolbar = Flex::default();
        query_toolbar.set_type(FlexType::Row);
        query_toolbar.set_margins(
            query_toolbar_margin,
            toolbar_control_vertical_margin,
            query_toolbar_margin,
            toolbar_control_vertical_margin,
        );
        query_toolbar.set_spacing(query_toolbar_spacing);

        let mut execute_btn = Button::default()
            .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
            .with_label("@> Execute");
        execute_btn.set_color(theme::selection_soft());
        execute_btn.set_label_color(theme::text_primary());
        execute_btn.set_frame(FrameType::RFlatBox);
        theme::install_button_hover(&mut execute_btn);
        query_toolbar.fixed(&execute_btn, query_toolbar_button_width);

        let mut cancel_btn = Button::default()
            .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
            .with_label("Cancel");
        cancel_btn.set_id("query_cancel");
        cancel_btn.set_color(theme::button_cancel());
        cancel_btn.set_label_color(theme::text_primary());
        cancel_btn.set_frame(FrameType::RFlatBox);
        let query_cancel_hovered = Arc::new(AtomicBool::new(false));
        let query_cancel_hovered_for_handle = Arc::clone(&query_cancel_hovered);
        cancel_btn.handle(move |button, event| {
            let hovered = match event {
                Event::Enter | Event::Move if button.active_r() => Some(true),
                Event::Enter | Event::Move | Event::Leave | Event::Deactivate | Event::Hide => {
                    Some(false)
                }
                _ => None,
            };
            if let Some(hovered) = hovered {
                query_cancel_hovered_for_handle.store(hovered, Ordering::Relaxed);
            }
            false
        });
        query_toolbar.fixed(&cancel_btn, query_toolbar_button_width);

        let mut commit_btn = Button::default()
            .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
            .with_label("Commit");
        commit_btn.set_color(theme::button_success());
        commit_btn.set_label_color(theme::text_primary());
        commit_btn.set_frame(FrameType::RFlatBox);
        theme::install_button_hover(&mut commit_btn);
        query_toolbar.fixed(&commit_btn, query_toolbar_button_width);

        let mut rollback_btn = Button::default()
            .with_size(BUTTON_WIDTH, BUTTON_HEIGHT)
            .with_label("Rollback");
        rollback_btn.set_color(theme::button_danger());
        rollback_btn.set_label_color(theme::text_primary());
        rollback_btn.set_frame(FrameType::RFlatBox);
        theme::install_button_hover(&mut rollback_btn);
        query_toolbar.fixed(&rollback_btn, query_toolbar_button_width);

        let initial_db_type = DatabaseType::default();
        let mut transaction_isolation_choice =
            Choice::default().with_size(TRANSACTION_ISOLATION_CHOICE_WIDTH, BUTTON_HEIGHT);
        transaction_isolation_choice.add_choice(&transaction_isolation_choice_labels(
            initial_db_type,
            TransactionIsolation::Default,
        ));
        transaction_isolation_choice.set_value(0);
        transaction_isolation_choice.set_id("query_transaction_isolation");
        theme::style_choice(&mut transaction_isolation_choice);
        transaction_isolation_choice.set_tooltip("Transaction isolation for new executions");
        theme::install_choice_hover(&mut transaction_isolation_choice);
        query_toolbar.fixed(&transaction_isolation_choice, query_toolbar_isolation_width);

        let mut transaction_access_choice =
            Choice::default().with_size(TRANSACTION_ACCESS_CHOICE_WIDTH, BUTTON_HEIGHT);
        transaction_access_choice.add_choice("Read write|Read only");
        transaction_access_choice.set_value(0);
        transaction_access_choice.set_id("query_transaction_access");
        theme::style_choice(&mut transaction_access_choice);
        transaction_access_choice.set_tooltip("Transaction access mode for new executions");
        theme::install_choice_hover(&mut transaction_access_choice);
        query_toolbar.fixed(&transaction_access_choice, query_toolbar_access_width);

        let toolbar_spacer = Frame::default();
        query_toolbar.resizable(&toolbar_spacer);

        let mut timeout_label = Frame::default().with_size(85, BUTTON_HEIGHT);
        timeout_label.set_label("Timeout(s)");
        timeout_label.set_label_color(theme::text_muted());
        if compact_query_toolbar {
            timeout_label.hide();
        }
        query_toolbar.fixed(&timeout_label, query_toolbar_timeout_label_width);

        let mut timeout_input = IntInput::default().with_size(NUMERIC_INPUT_WIDTH, BUTTON_HEIGHT);
        timeout_input.set_color(theme::input_bg());
        timeout_input.set_text_color(theme::text_primary());
        theme::apply_text_input_inset(&mut timeout_input);
        timeout_input.set_tooltip("Call timeout in seconds (empty = no timeout)");
        timeout_input.set_value("60");
        query_toolbar.fixed(&timeout_input, query_toolbar_timeout_width);

        let mut zoom_out_btn = Button::default()
            .with_size(UI_SCALE_BUTTON_WIDTH, BUTTON_HEIGHT)
            .with_label("-");
        zoom_out_btn.set_color(theme::button_subtle());
        zoom_out_btn.set_label_color(theme::text_primary());
        zoom_out_btn.set_frame(FrameType::RFlatBox);
        zoom_out_btn.set_tooltip("Zoom out (Ctrl/Cmd+-)");
        theme::install_button_hover(&mut zoom_out_btn);
        query_toolbar.fixed(&zoom_out_btn, query_toolbar_scale_button_width);

        let mut zoom_in_btn = Button::default()
            .with_size(UI_SCALE_BUTTON_WIDTH, BUTTON_HEIGHT)
            .with_label("+");
        zoom_in_btn.set_color(theme::button_subtle());
        zoom_in_btn.set_label_color(theme::text_primary());
        zoom_in_btn.set_frame(FrameType::RFlatBox);
        zoom_in_btn.set_tooltip("Zoom in (Ctrl/Cmd++)");
        theme::install_button_hover(&mut zoom_in_btn);
        query_toolbar.fixed(&zoom_in_btn, query_toolbar_scale_button_width);

        query_toolbar.end();
        let execute_btn_for_toolbar_resize = execute_btn.clone();
        let cancel_btn_for_toolbar_resize = cancel_btn.clone();
        let commit_btn_for_toolbar_resize = commit_btn.clone();
        let rollback_btn_for_toolbar_resize = rollback_btn.clone();
        let isolation_for_toolbar_resize = transaction_isolation_choice.clone();
        let access_for_toolbar_resize = transaction_access_choice.clone();
        let mut timeout_label_for_toolbar_resize = timeout_label.clone();
        let timeout_input_for_toolbar_resize = timeout_input.clone();
        let zoom_out_for_toolbar_resize = zoom_out_btn.clone();
        let zoom_in_for_toolbar_resize = zoom_in_btn.clone();
        query_toolbar.handle(move |toolbar, event| {
            if event != Event::Resize {
                return false;
            }
            let compact = toolbar.w() < QUERY_TOOLBAR_COMPACT_BREAKPOINT;
            let horizontal_margin = if compact { 4 } else { TOOLBAR_SPACING };
            toolbar.set_margins(
                horizontal_margin,
                toolbar_control_vertical_margin,
                horizontal_margin,
                toolbar_control_vertical_margin,
            );
            toolbar.set_spacing(if compact { 4 } else { TOOLBAR_SPACING });
            let button_width = if compact {
                BUTTON_WIDTH_SMALL
            } else {
                BUTTON_WIDTH
            };
            toolbar.fixed(&execute_btn_for_toolbar_resize, button_width);
            toolbar.fixed(&cancel_btn_for_toolbar_resize, button_width);
            toolbar.fixed(&commit_btn_for_toolbar_resize, button_width);
            toolbar.fixed(&rollback_btn_for_toolbar_resize, button_width);
            toolbar.fixed(
                &isolation_for_toolbar_resize,
                if compact {
                    QUERY_TOOLBAR_COMPACT_CHOICE_WIDTH
                } else {
                    TRANSACTION_ISOLATION_CHOICE_WIDTH
                },
            );
            toolbar.fixed(
                &access_for_toolbar_resize,
                if compact {
                    QUERY_TOOLBAR_COMPACT_ACCESS_WIDTH
                } else {
                    TRANSACTION_ACCESS_CHOICE_WIDTH
                },
            );
            if compact {
                timeout_label_for_toolbar_resize.hide();
            } else {
                timeout_label_for_toolbar_resize.show();
            }
            toolbar.fixed(
                &timeout_label_for_toolbar_resize,
                if compact { 0 } else { 85 },
            );
            toolbar.fixed(
                &timeout_input_for_toolbar_resize,
                if compact {
                    QUERY_TOOLBAR_COMPACT_NUMERIC_WIDTH
                } else {
                    NUMERIC_INPUT_WIDTH
                },
            );
            let scale_button_width = if compact {
                QUERY_TOOLBAR_COMPACT_SCALE_BUTTON_WIDTH
            } else {
                UI_SCALE_BUTTON_WIDTH
            };
            toolbar.fixed(&zoom_out_for_toolbar_resize, scale_button_width);
            toolbar.fixed(&zoom_in_for_toolbar_resize, scale_button_width);
            false
        });
        main_flex.fixed(&query_toolbar, RESULT_TOOLBAR_HEIGHT);

        let mut content_flex = Flex::default();
        content_flex.set_type(FlexType::Row);
        content_flex.set_spacing(0);

        let object_browser = MultiObjectBrowserWidget::new(0, 0, 250, 600);
        let obj_browser_widget = object_browser.get_widget();
        content_flex.fixed(&obj_browser_widget, 250);

        let splitter_width = MAIN_SPLITTER_WIDTH;
        let mut split_bar = Frame::default().with_size(splitter_width, 0);
        split_bar.set_id("main_vertical_splitter");
        split_bar.set_frame(FrameType::FlatBox);
        split_bar.set_color(theme::border());
        split_bar.set_tooltip("Drag to resize panels");

        let drag_state = Arc::new(Mutex::new(None::<(i32, i32)>));
        let mut content_flex_for_split = content_flex.clone();
        let obj_browser_for_split = obj_browser_widget.clone();
        let drag_state_for_split = drag_state;
        let mut split_hover = theme::HoverFeedbackState::default();
        split_bar.handle(move |bar, ev| {
            split_hover.update(bar, ev);
            match ev {
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
                fltk::enums::Event::Leave
                | fltk::enums::Event::Deactivate
                | fltk::enums::Event::Hide => {
                    set_cursor(Cursor::Default);
                    true
                }
                _ => false,
            }
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

        let mut result_workspace_group = Group::new(0, 0, 900, 400, None);
        result_workspace_group.set_frame(FrameType::FlatBox);
        result_workspace_group.set_color(theme::panel_bg());
        result_workspace_group.begin();
        let result_tabs = ResultTabsWidget::new(0, 0, 900, 400);
        let mut result_widget = result_tabs.get_widget();
        result_widget.hide();
        result_workspace_group.resizable(&result_widget);
        result_workspace_group.end();
        result_bottom_flex.add(&result_workspace_group);
        result_bottom_flex.resizable(&result_workspace_group);

        let mut result_toolbar = Flex::default();
        result_toolbar.set_type(FlexType::Row);
        result_toolbar.set_margins(
            TOOLBAR_SPACING,
            toolbar_control_vertical_margin,
            TOOLBAR_SPACING,
            toolbar_control_vertical_margin,
        );
        result_toolbar.set_spacing(TOOLBAR_SPACING);

        let mut clear_all_btn = Button::default()
            .with_size(BUTTON_WIDTH_LARGE, BUTTON_HEIGHT)
            .with_label("Clear All");
        clear_all_btn.set_id("result_clear_all");
        clear_all_btn.set_color(theme::button_subtle());
        clear_all_btn.set_label_color(theme::text_secondary());
        clear_all_btn.set_frame(FrameType::RFlatBox);
        clear_all_btn.set_tooltip("Clear all result grids, output, messages, and plans");
        theme::install_button_hover(&mut clear_all_btn);
        result_toolbar.fixed(&clear_all_btn, BUTTON_WIDTH_LARGE);

        let mut page_control = Flex::default().with_size(RESULT_PAGE_CONTROL_WIDTH, BUTTON_HEIGHT);
        page_control.set_id("result_page_controls");
        page_control.set_type(FlexType::Row);
        page_control.set_spacing(RESULT_PAGE_CONTROL_SPACING);

        let mut page_first_btn = Button::default()
            .with_size(RESULT_PAGE_NAV_BUTTON_WIDTH, BUTTON_HEIGHT)
            .with_label("«");
        page_first_btn.set_id("result_page_first");
        page_first_btn.set_tooltip("Move to the first result row (Ctrl/Cmd+Up)");
        page_control.fixed(&page_first_btn, RESULT_PAGE_NAV_BUTTON_WIDTH);

        let mut page_previous_btn = Button::default()
            .with_size(RESULT_PAGE_NAV_BUTTON_WIDTH, BUTTON_HEIGHT)
            .with_label("‹");
        page_previous_btn.set_id("result_page_previous");
        page_previous_btn.set_tooltip("Move to the previous page boundary");
        page_control.fixed(&page_previous_btn, RESULT_PAGE_NAV_BUTTON_WIDTH);

        let mut page_unit_choice =
            Choice::default().with_size(RESULT_PAGE_UNIT_WIDTH, BUTTON_HEIGHT);
        page_unit_choice.set_id("result_page_unit");
        page_unit_choice.add_choice("10|100|250|500|1000");
        page_unit_choice.set_value(RESULT_PAGE_DEFAULT_UNIT_INDEX as i32);
        theme::style_choice(&mut page_unit_choice);
        page_unit_choice.set_tooltip("Rows per page for the previous and next buttons");
        page_control.fixed(&page_unit_choice, RESULT_PAGE_UNIT_WIDTH);

        let mut page_next_btn = Button::default()
            .with_size(RESULT_PAGE_NAV_BUTTON_WIDTH, BUTTON_HEIGHT)
            .with_label("›");
        page_next_btn.set_id("result_page_next");
        page_next_btn.set_tooltip("Move to the next page boundary");
        page_control.fixed(&page_next_btn, RESULT_PAGE_NAV_BUTTON_WIDTH);

        let mut page_last_btn = Button::default()
            .with_size(RESULT_PAGE_NAV_BUTTON_WIDTH, BUTTON_HEIGHT)
            .with_label("»");
        page_last_btn.set_id("result_page_last");
        page_last_btn.set_tooltip("Fetch remaining rows and move to the last row (Ctrl/Cmd+Down)");
        page_control.fixed(&page_last_btn, RESULT_PAGE_NAV_BUTTON_WIDTH);

        for button in [
            &mut page_first_btn,
            &mut page_previous_btn,
            &mut page_next_btn,
            &mut page_last_btn,
        ] {
            button.set_color(theme::button_subtle());
            button.set_label_color(theme::text_primary());
            button.set_selection_color(theme::selection_soft());
            button.set_frame(FrameType::RFlatBox);
            install_result_page_control_feedback(button);
        }
        install_result_page_control_feedback(&mut page_unit_choice);
        page_control.end();
        page_control.resize_callback(|control, _, _, width, _| {
            let should_show = result_page_controls_fit(width);
            let mut visibility_changed = false;
            for index in 0..control.children() {
                if let Some(mut child) = control.child(index) {
                    if child.visible() != should_show {
                        visibility_changed = true;
                        if should_show {
                            child.show();
                        } else {
                            child.hide();
                        }
                    }
                }
            }
            let (left, right) = if should_show {
                result_page_control_center_offsets(width)
            } else {
                (0, 0)
            };
            let margins_changed = control.margins() != (left, 0, right, 0);
            if margins_changed {
                control.set_margins(left, 0, right, 0);
            }
            if visibility_changed || margins_changed {
                control.layout();
            }
        });
        result_toolbar.resizable(&page_control);

        let mut one_tab_per_query_check = CheckButton::default()
            .with_size(BUTTON_WIDTH_LARGE + 45, BUTTON_HEIGHT)
            .with_label(RESULT_ONE_TAB_PER_QUERY_LABEL);
        one_tab_per_query_check.set_id("result_one_tab_per_query");
        one_tab_per_query_check.set_color(theme::button_secondary());
        one_tab_per_query_check.set_tooltip(
            "Unchecked: clear existing result tabs before each execution. Checked: append result tabs.",
        );
        theme::install_button_hover(&mut one_tab_per_query_check);
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
        edit_mode_check.set_id("result_edit_mode");
        edit_mode_check.set_color(theme::button_secondary());
        edit_mode_check.set_tooltip("Enable staged edit mode for the current result tab");
        theme::install_button_hover(&mut edit_mode_check);
        edit_mode_check.hide();
        result_toolbar.fixed(&edit_mode_check, 0);

        let mut edit_insert_btn = Button::default()
            .with_size(BUTTON_WIDTH_SMALL, BUTTON_HEIGHT)
            .with_label("Insert");
        edit_insert_btn.set_id("result_edit_insert");
        edit_insert_btn.set_color(theme::button_secondary());
        edit_insert_btn.set_label_color(theme::text_primary());
        edit_insert_btn.set_frame(FrameType::RFlatBox);
        edit_insert_btn.set_tooltip("Add a staged row (DB is not changed until Save)");
        theme::install_button_hover(&mut edit_insert_btn);
        result_toolbar.fixed(&edit_insert_btn, BUTTON_WIDTH_SMALL);

        let mut edit_delete_btn = Button::default()
            .with_size(BUTTON_WIDTH_SMALL, BUTTON_HEIGHT)
            .with_label("Delete");
        edit_delete_btn.set_id("result_edit_delete");
        edit_delete_btn.set_color(theme::button_danger());
        edit_delete_btn.set_label_color(theme::text_primary());
        edit_delete_btn.set_frame(FrameType::RFlatBox);
        edit_delete_btn.set_tooltip("Delete selected row(s) in staged edit mode");
        theme::install_button_hover(&mut edit_delete_btn);
        result_toolbar.fixed(&edit_delete_btn, BUTTON_WIDTH_SMALL);

        let mut edit_save_btn = Button::default()
            .with_size(BUTTON_WIDTH_SMALL, BUTTON_HEIGHT)
            .with_label("Save");
        edit_save_btn.set_id("result_edit_save");
        edit_save_btn.set_color(theme::button_success());
        edit_save_btn.set_label_color(theme::text_primary());
        edit_save_btn.set_frame(FrameType::RFlatBox);
        edit_save_btn.set_tooltip("Apply staged edits to DB");
        theme::install_button_hover(&mut edit_save_btn);
        result_toolbar.fixed(&edit_save_btn, BUTTON_WIDTH_SMALL);

        let mut edit_cancel_btn = Button::default()
            .with_size(BUTTON_WIDTH_SMALL, BUTTON_HEIGHT)
            .with_label("Cancel");
        edit_cancel_btn.set_id("result_edit_cancel");
        edit_cancel_btn.set_color(theme::button_cancel());
        edit_cancel_btn.set_label_color(theme::text_primary());
        edit_cancel_btn.set_frame(FrameType::RFlatBox);
        edit_cancel_btn.set_tooltip("Discard staged edits and restore rows");
        theme::install_button_hover(&mut edit_cancel_btn);
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
        query_split_bar.set_id("query_result_splitter");
        query_split_bar.set_frame(FrameType::FlatBox);
        query_split_bar.set_color(theme::border());
        query_split_bar.set_tooltip("Drag to resize query and result panes");
        query_split_bar.resize(
            tile_x,
            tile_y + initial_query_height,
            tile_w,
            QUERY_SPLIT_BAR_HEIGHT,
        );
        let mut query_split_hover = theme::HoverFeedbackState::default();
        query_split_bar.handle(move |bar, event| {
            query_split_hover.update(bar, event);
            match event {
                Event::Enter | Event::Move => set_cursor(Cursor::NS),
                Event::Leave | Event::Deactivate | Event::Hide => set_cursor(Cursor::Default),
                _ => {}
            }
            false
        });

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

        let schema_intellisense_data = Arc::new(Mutex::new(IntellisenseData::new()));
        let previous_group = Group::try_current();
        query_top_group.begin();
        let mut detached_editor_container = Group::new(0, 0, 1, 1, None);
        detached_editor_container.hide();
        detached_editor_container.begin();
        let detached_editor = SqlEditorWidget::new_with_binding_and_intellisense_data(
            initial_binding,
            timeout_input.clone(),
            schema_intellisense_data.clone(),
        );
        detached_editor_container.end();
        query_top_group.end();
        if let Some(previous_group) = previous_group.as_ref() {
            Group::set_current(Some(previous_group));
        } else {
            Group::set_current(None::<&Group>);
        }
        let sql_buffer = detached_editor.get_buffer();
        let editor_tabs = Vec::new();

        right_flex.resizable(&right_tile);
        right_flex.end();

        content_flex.resizable(&right_flex);
        content_flex.end();
        main_flex.resizable(&content_flex);

        let status_bar = StatusBarWidget::new();
        main_flex.fixed(&status_bar.root, STATUS_BAR_HEIGHT);
        main_flex.end();
        window.end();
        window.make_resizable(true);

        let state = Arc::new(Mutex::new(AppState {
            connection,
            connection_registry,
            query_tabs: query_tabs.clone(),
            query_top_group: query_top_group.clone(),
            query_split_bar: query_split_bar.clone(),
            editor_tabs,
            active_editor_tab_id: 0,
            next_editor_tab_number: 1,
            sql_editor: detached_editor,
            sql_buffer,
            schema_intellisense_data,
            schema_highlight_data: HighlightData::new(),
            query_timeout_input: timeout_input.clone(),
            ui_scale_bases,
            result_tabs: result_tabs.clone(),
            result_workspace_group: result_workspace_group.clone(),
            result_toolbar: result_toolbar.clone(),
            result_one_tab_per_query_check: one_tab_per_query_check.clone(),
            result_one_tab_edit_gap: one_tab_edit_gap.clone(),
            result_edit_check: edit_mode_check.clone(),
            result_insert_btn: edit_insert_btn.clone(),
            result_delete_btn: edit_delete_btn.clone(),
            result_save_btn: edit_save_btn.clone(),
            result_cancel_btn: edit_cancel_btn.clone(),
            result_page_unit_choice: page_unit_choice.clone(),
            execute_btn: execute_btn.clone(),
            query_cancel_btn: cancel_btn.clone(),
            query_cancel_hovered,
            query_cancel_pulse_frame: 0,
            commit_btn: commit_btn.clone(),
            rollback_btn: rollback_btn.clone(),
            transaction_isolation_choice: transaction_isolation_choice.clone(),
            transaction_access_choice: transaction_access_choice.clone(),
            result_grid_execution_targets: HashMap::new(),
            pending_table_browse_last: HashMap::new(),
            pending_table_browse_refresh: HashMap::new(),
            progress_contexts: HashMap::new(),
            abandoned_query_operations: HashSet::new(),
            pending_query_cancellations: HashMap::new(),
            pending_lazy_fetch_canceling_sessions: HashSet::new(),
            orphaned_lazy_fetch_missing_since: HashMap::new(),
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
            pending_metadata_refresh_tabs: HashSet::new(),
            latest_schema_request_id: 0,
            config: Arc::new(Mutex::new(config)),
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
            s.render_status_bar();
        }
        MainWindow::start_status_activity_timer(&state);

        let weak_state_for_execute = Arc::downgrade(&state);
        execute_btn.set_callback(move |_| {
            if let Some(state_for_execute) = weak_state_for_execute.upgrade() {
                execute_sql_request_with_session_pool_slot(
                    &state_for_execute,
                    SqlExecutionRequest::StatementAtCursor,
                );
            }
        });

        let weak_state_for_zoom_out = Arc::downgrade(&state);
        zoom_out_btn.set_callback(move |_| {
            if let Some(state_for_zoom_out) = weak_state_for_zoom_out.upgrade() {
                MainWindow::adjust_ui_scale(&state_for_zoom_out, UiScaleAction::Out);
            }
        });

        let weak_state_for_zoom_in = Arc::downgrade(&state);
        zoom_in_btn.set_callback(move |_| {
            if let Some(state_for_zoom_in) = weak_state_for_zoom_in.upgrade() {
                MainWindow::adjust_ui_scale(&state_for_zoom_in, UiScaleAction::In);
            }
        });

        let weak_state_for_cancel = Arc::downgrade(&state);
        cancel_btn.set_callback(move |_| {
            if let Some(state_for_cancel) = weak_state_for_cancel.upgrade() {
                MainWindow::cancel_latest_query_editor_tab(&state_for_cancel);
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

        let weak_state_for_result_clear_all = Arc::downgrade(&state);
        clear_all_btn.set_callback(move |_| {
            let Some(state_for_result_clear_all) = weak_state_for_result_clear_all.upgrade() else {
                return;
            };
            MainWindow::clear_all_result_views(&state_for_result_clear_all);
        });

        let weak_state_for_page_first = Arc::downgrade(&state);
        page_first_btn.set_callback(move |_| {
            let Some(state_for_page_first) = weak_state_for_page_first.upgrade() else {
                return;
            };
            let mut result_tabs =
                MainWindow::clone_result_tabs_for_page_action(&state_for_page_first);
            result_tabs.page_current_first();
            app::redraw();
        });

        let weak_state_for_page_previous = Arc::downgrade(&state);
        let page_unit_for_previous = page_unit_choice.clone();
        page_previous_btn.set_callback(move |_| {
            let Some(state_for_page_previous) = weak_state_for_page_previous.upgrade() else {
                return;
            };
            let mut result_tabs =
                MainWindow::clone_result_tabs_for_page_action(&state_for_page_previous);
            let unit = result_page_unit_for_choice_index(page_unit_for_previous.value());
            result_tabs.page_current_previous(unit);
            app::redraw();
        });

        let weak_state_for_page_next = Arc::downgrade(&state);
        let page_unit_for_next = page_unit_choice.clone();
        page_next_btn.set_callback(move |_| {
            let Some(state_for_page_next) = weak_state_for_page_next.upgrade() else {
                return;
            };
            let mut result_tabs =
                MainWindow::clone_result_tabs_for_page_action(&state_for_page_next);
            let unit = result_page_unit_for_choice_index(page_unit_for_next.value());
            result_tabs.page_current_next(unit);
            app::redraw();
        });

        let weak_state_for_page_last = Arc::downgrade(&state);
        page_last_btn.set_callback(move |_| {
            let Some(state_for_page_last) = weak_state_for_page_last.upgrade() else {
                return;
            };
            let mut result_tabs =
                MainWindow::clone_result_tabs_for_page_action(&state_for_page_last);
            result_tabs.page_current_last();
            app::redraw();
        });

        let weak_state_for_page_unit = Arc::downgrade(&state);
        page_unit_choice.set_callback(move |choice| {
            let Some(state_for_page_unit) = weak_state_for_page_unit.upgrade() else {
                return;
            };
            let unit = result_page_unit_for_choice_index(choice.value());
            let mut result_tabs =
                MainWindow::clone_result_tabs_for_page_action(&state_for_page_unit);
            result_tabs.set_current_page_unit(unit);
            app::redraw();
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
        let config = state
            .config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        font_settings::update_runtime_font_settings(&config);
        let (editor_profile, result_profile, ui_size, editor_size, result_size) = (
            font_settings::profile_by_name(&config.editor_font),
            font_settings::profile_by_name(&config.result_font),
            config.normalized_ui_font_size() as i32,
            config.normalized_editor_font_size(),
            config.normalized_result_font_size(),
        );
        let result_cell_max_chars = config.result_cell_max_chars.clamp(
            RESULT_CELL_MAX_DISPLAY_CHARS_MIN,
            RESULT_CELL_MAX_DISPLAY_CHARS_MAX,
        );
        font_settings::apply_global_default_font(editor_profile.normal);
        app::set_font_size(ui_size);
        fltk::misc::Tooltip::set_font(editor_profile.normal);
        fltk::misc::Tooltip::set_font_size(ui_size);
        fltk::dialog::message_set_font(editor_profile.normal, ui_size);
        Self::apply_runtime_ui_font(state, editor_profile.normal, ui_size);
        for tab in &mut state.editor_tabs {
            tab.sql_editor
                .apply_font_settings(editor_profile, editor_size, ui_size);
        }
        state.query_tabs.refresh_tab_strip_overflow_mode();
        for tab in &mut state.editor_tabs {
            tab.result_tabs
                .apply_font_settings(result_profile, result_size);
            tab.result_tabs
                .set_max_cell_display_chars(result_cell_max_chars as usize);
        }
        state
            .object_browser
            .apply_font_settings(editor_profile, ui_size);
        state.right_tile.redraw();
        state.window.redraw();
        app::redraw();
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

    fn persist_settings(
        state: &mut AppState,
        settings: FontSettings,
        pool_size_changed: bool,
    ) -> Result<(), String> {
        let save_result = {
            let mut config = state
                .config
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            config.editor_font = settings.font.clone();
            config.ui_font_size = settings.ui_size;
            config.ui_scale_percent = settings.ui_scale_percent;
            config.editor_font_size = settings.editor_size;
            config.result_font = settings.font;
            config.result_font_size = settings.result_size;
            config.result_cell_max_chars = settings.result_cell_max_chars;
            config.lazy_fetch_batch_size = settings.lazy_fetch_batch_size;
            config.intellisense_context_window_kib = settings.intellisense_context_window_kib;
            config.intellisense_popup_delay_ms = settings.intellisense_popup_delay_ms;
            config.connection_pool_size = settings.connection_pool_size;
            config.connect_timeout_seconds = settings.connect_timeout_seconds;
            config.cancel_timeout_seconds = settings.cancel_timeout_seconds;
            config.sql_comma_list_layout = settings.sql_comma_list_layout;
            config.sql_format_right_margin = settings.sql_format_right_margin;
            config.query_history_limit = settings.query_history_limit;
            config.app_log_limit = settings.app_log_limit;
            config.save().map_err(|err| err.to_string())
        };
        // `save` republishes the runtime config, so both writers read the new limits.
        crate::ui::query_history::apply_history_limit();
        crate::utils::logging::apply_log_limit();
        if pool_size_changed {
            state.release_all_resolved_pooled_db_sessions()?;
        }
        Self::apply_lazy_fetch_settings(state);
        Self::apply_font_settings(state);
        save_result
    }

    fn apply_runtime_ui_font(state: &mut AppState, font: fltk::enums::Font, ui_size: i32) {
        fn apply_widget_font_recursive(widget: &mut Widget, font: fltk::enums::Font, size: i32) {
            widget.set_label_font(font);
            widget.set_label_size(size);
            if let Some(mut input) = Input::from_dyn_widget(widget) {
                input.set_text_font(font);
                input.set_text_size(size);
            }
            if let Some(mut choice) = Choice::from_dyn_widget(widget) {
                choice.set_text_font(font);
                choice.set_text_size(size);
            }
            if let Some(mut menu_button) = MenuButton::from_dyn_widget(widget) {
                menu_button.set_text_font(font);
                menu_button.set_text_size(size);
            }
            if let Some(mut browser) = Browser::from_dyn_widget(widget) {
                browser.set_text_size(size);
            }
            if let Some(mut display) = TextDisplay::from_dyn_widget(widget) {
                display.set_text_font(font);
                display.set_text_size(size);
            }
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

        state.transaction_isolation_choice.set_text_font(font);
        state.transaction_isolation_choice.set_text_size(ui_size);
        state.transaction_access_choice.set_text_font(font);
        state.transaction_access_choice.set_text_size(ui_size);

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
        state.render_status_bar();
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
        let binding = state
            .editor_tabs
            .iter()
            .find(|tab| tab.tab_id == state.active_editor_tab_id)
            .filter(|tab| tab.connection_binding.snapshot().connection_id().is_some())
            .map(|tab| tab.connection_binding.fork_for_new_tab())
            .unwrap_or_else(|| Self::binding_for_selected_database(state));
        Self::create_query_editor_tab_for_binding(state, binding, stabilize_display)
    }

    fn create_query_editor_tab_for_selected_database(state: &mut AppState) -> Option<QueryTabId> {
        Self::create_query_editor_tab_for_selected_database_with_display_stabilization(state, true)
    }

    fn create_query_editor_tab_for_selected_database_with_display_stabilization(
        state: &mut AppState,
        stabilize_display: bool,
    ) -> Option<QueryTabId> {
        let binding = Self::binding_for_selected_database(state);
        Self::create_query_editor_tab_for_binding(state, binding, stabilize_display)
    }

    fn binding_for_selected_database(state: &AppState) -> TabConnectionBinding {
        if let Some((connection_id, scope)) = state.object_browser.selected_connection_context() {
            if let Some(runtime) = state.connection_registry.get(connection_id) {
                return TabConnectionBinding::bound_in_registry(
                    state.connection_registry.clone(),
                    runtime,
                    scope,
                );
            }
        }
        state
            .editor_tabs
            .iter()
            .find(|tab| tab.tab_id == state.active_editor_tab_id)
            .map(|tab| tab.connection_binding.fork_for_new_tab())
            .unwrap_or_else(TabConnectionBinding::unbound)
    }

    fn create_query_editor_tab_for_runtime(
        state: &mut AppState,
        runtime: Arc<ConnectionRuntime>,
    ) -> Option<QueryTabId> {
        state.object_browser.add_runtime(runtime.clone());
        Self::create_query_editor_tab_for_binding(
            state,
            TabConnectionBinding::bound_in_registry(
                state.connection_registry.clone(),
                runtime,
                None,
            ),
            true,
        )
    }

    fn select_or_create_query_editor_tab_for_connection(
        state: &mut AppState,
        connection_id: ConnectionId,
    ) -> Option<(QueryTabId, bool)> {
        if state.active_connection_id() == Some(connection_id) {
            return Some((state.active_editor_tab_id, false));
        }
        if let Some(tab_id) = state
            .editor_tabs
            .iter()
            .find(|tab| tab.connection_binding.snapshot().connection_id() == Some(connection_id))
            .map(|tab| tab.tab_id)
        {
            return state
                .set_active_editor_tab(tab_id)
                .then_some((tab_id, false));
        }
        let runtime = state.connection_registry.get(connection_id)?;
        Self::create_query_editor_tab_for_runtime(state, runtime).map(|tab_id| (tab_id, true))
    }

    fn create_query_editor_tab_for_binding(
        state: &mut AppState,
        binding: TabConnectionBinding,
        stabilize_display: bool,
    ) -> Option<QueryTabId> {
        let query_number = state.next_editor_tab_number;
        let binding_snapshot = binding.snapshot();
        let connection_label = binding_snapshot.runtime.as_ref().and_then(|runtime| {
            let label = runtime.display_name();
            (!label.trim().is_empty()).then_some(label)
        });
        let label = connection_label.map_or_else(
            || format!("Query {query_number}"),
            |connection_label| format!("{connection_label} · Query {query_number}"),
        );
        let base_label = format!("Query {query_number}");
        state.next_editor_tab_number = state.next_editor_tab_number.saturating_add(1);
        let tab_id = state.query_tabs.add_tab(&label);
        let group = state.query_tabs.tab_group(tab_id)?;
        let binding_connection_id = binding.snapshot().connection_id();
        let existing_metadata = state
            .editor_tabs
            .iter()
            .find(|tab| tab.connection_binding.snapshot().connection_id() == binding_connection_id)
            .map(|tab| {
                (
                    tab.intellisense_data
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clone(),
                    tab.highlight_data.clone(),
                )
            });
        let browser_snapshot = binding_connection_id.and_then(|connection_id| {
            state
                .object_browser
                .metadata_snapshot_for_connection(connection_id)
        });
        let (seed_data, seed_highlight_data) =
            Self::editor_metadata_seed(existing_metadata, browser_snapshot.as_ref());
        let intellisense_data = Arc::new(Mutex::new(seed_data));
        group.begin();
        let mut editor = SqlEditorWidget::new_with_binding_and_intellisense_data(
            binding.clone(),
            state.query_timeout_input.clone(),
            intellisense_data.clone(),
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
        editor.update_highlight_data(seed_highlight_data.clone());
        let buffer = editor.get_buffer();
        let previous_group = fltk::group::Group::try_current();
        state.result_workspace_group.begin();
        let mut result_tabs = ResultTabsWidget::new(
            state.result_workspace_group.x(),
            state.result_workspace_group.y(),
            state.result_workspace_group.w(),
            state.result_workspace_group.h(),
        );
        let result_cell_max_chars = state
            .config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .result_cell_max_chars
            .clamp(
                RESULT_CELL_MAX_DISPLAY_CHARS_MIN,
                RESULT_CELL_MAX_DISPLAY_CHARS_MAX,
            );
        result_tabs.set_max_cell_display_chars(result_cell_max_chars as usize);
        let mut result_widget = result_tabs.get_widget();
        result_widget.hide();
        state.result_workspace_group.end();
        if let Some(previous_group) = previous_group.as_ref() {
            fltk::group::Group::set_current(Some(previous_group));
        } else {
            fltk::group::Group::set_current(None::<&fltk::group::Group>);
        }
        state.editor_tabs.push(QueryEditorTab {
            tab_id,
            base_label,
            connection_binding: binding,
            sql_editor: editor,
            sql_buffer: buffer,
            intellisense_data,
            highlight_data: seed_highlight_data,
            result_tabs,
            current_file: None,
            pristine_text: String::new(),
            current_text_len: 0,
            is_dirty: false,
        });
        state.query_tabs.select(tab_id);
        state.refresh_tab_label(tab_id);
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
                MainWindow::cancel_query_editor_tab(&state_for_retry, tab_id);
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
            removed_connection_id,
            connection_registry,
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
            let removed_connection_id = s.editor_tabs[index]
                .connection_binding
                .snapshot()
                .connection_id();
            let mut result_workspace_to_cleanup = s.editor_tabs[index].result_tabs.clone();
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
                result_workspace_to_cleanup.abort_lazy_fetch_session(*session_id);
            }
            if !lazy_fetch_sessions.is_empty() {
                s.refresh_result_edit_controls();
            }
            s.editor_tabs.remove(index);
            s.pending_metadata_refresh_tabs.remove(&tab_id);
            s.finish_progress_context(tab_id);
            s.pending_query_cancellations
                .retain(|token, _| token.tab_id != tab_id);
            s.result_grid_execution_targets.remove(&tab_id);
            s.pending_table_browse_last.remove(&tab_id);
            s.pending_table_browse_refresh.remove(&tab_id);

            let mut created_tab_id = None;
            let mut deferred_display_tab_id = None;
            if s.editor_tabs.is_empty() {
                created_tab_id = MainWindow::
                    create_query_editor_tab_for_selected_database_with_display_stabilization(
                        &mut s, false,
                    );
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

            result_workspace_to_cleanup.delete_workspace();

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
                removed_connection_id,
                s.connection_registry.clone(),
            )
        };

        for session_id in &lazy_fetch_sessions {
            editor_to_cleanup.request_lazy_fetch(
                *session_id,
                crate::ui::sql_editor::LazyFetchRequest::CancelAndDiscard,
            );
        }
        editor_to_cleanup.cleanup_for_close();
        drop(editor_to_cleanup);

        if let Some(connection_id) = removed_connection_id {
            if connection_registry.remove_transient_if_idle(connection_id) {
                state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .object_browser
                    .remove_runtime(connection_id);
            }
        }

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
        let active_connection_id = state.active_connection_id();
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

        let data_for_tabs = data.clone();
        *state
            .schema_intellisense_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = data;
        state.schema_highlight_data = combined_highlight;
        let target_tab_ids = state
            .editor_tabs
            .iter()
            .filter(|tab| {
                active_connection_id.map_or(tab.tab_id == state.active_editor_tab_id, |id| {
                    tab.connection_binding.snapshot().connection_id() == Some(id)
                })
            })
            .map(|tab| tab.tab_id)
            .collect::<Vec<_>>();
        for tab in state
            .editor_tabs
            .iter_mut()
            .filter(|tab| target_tab_ids.contains(&tab.tab_id))
        {
            *tab.intellisense_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = data_for_tabs.clone();
            tab.highlight_data = state.schema_highlight_data.clone();
            tab.sql_editor
                .update_highlight_data_deferred(state.schema_highlight_data.clone());
        }
        state
            .pending_metadata_refresh_tabs
            .retain(|tab_id| !target_tab_ids.contains(tab_id));
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

    fn merge_object_browser_snapshot_into_highlight_data(
        mut highlight_data: HighlightData,
        snapshot: &ObjectBrowserMetadataSnapshot,
    ) -> HighlightData {
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
        highlight_data
    }

    fn editor_metadata_seed(
        existing: Option<(IntellisenseData, HighlightData)>,
        browser_snapshot: Option<&ObjectBrowserMetadataSnapshot>,
    ) -> (IntellisenseData, HighlightData) {
        let (mut data, mut highlight_data) =
            existing.unwrap_or_else(|| (IntellisenseData::new(), HighlightData::new()));
        if let Some(snapshot) = browser_snapshot {
            if Self::intellisense_scope_differs(&data, snapshot.selected_scope.as_deref()) {
                data = snapshot.to_intellisense_data();
                highlight_data = snapshot.to_highlight_data();
            } else {
                data = Self::merge_object_browser_snapshot_into_data(data, snapshot);
                highlight_data = Self::merge_object_browser_snapshot_into_highlight_data(
                    highlight_data,
                    snapshot,
                );
            }
        }
        (data, highlight_data)
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

        let highlight_data = if replace_active_scope {
            snapshot.to_highlight_data()
        } else {
            state.schema_highlight_data.clone()
        };
        let highlight_data =
            Self::merge_object_browser_snapshot_into_highlight_data(highlight_data, &snapshot);

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

    fn schema_update_matches_target(
        update: &SchemaUpdate,
        target: &ActiveSchemaUpdateTarget,
    ) -> bool {
        update.query_tab_id == target.query_tab_id
            && update.connection_id == target.connection_id
            && update.connection_generation == target.connection_generation
            && update.binding_revision == target.binding_revision
            && update.request_id == target.request_id
            && update.db_type.is_same_type_as(target.db_type)
            && Self::schema_update_scope_matches(
                update.db_type,
                update.requested_scope.as_deref(),
                target.scope.as_deref(),
                &update.data.users,
            )
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
        query_tab_id: QueryTabId,
        connection_id: ConnectionId,
        binding_revision: u64,
        request_id: u64,
    ) -> Option<SchemaUpdate> {
        context.ensure_current().ok()?;
        let connection_generation = context.connection_generation;
        let db_type = context.connection_info.db_type;
        let activity = db_type.metadata_refresh_activity(requested_scope.as_deref());
        let _activity_guard =
            crate::db::track_pool_db_activity_for_connection(activity, db_type, connection_id);
        let data =
            schema_metadata_loader_for(db_type).load(context.clone(), requested_scope.clone())?;
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
            query_tab_id,
            connection_id,
            data,
            highlight_data,
            connection_generation,
            binding_revision,
            request_id,
            db_type,
            requested_scope,
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

        let Some((query_tab_id, binding_snapshot)) = state
            .editor_tabs
            .iter()
            .find(|tab| tab.tab_id == state.active_editor_tab_id)
            .map(|tab| (tab.tab_id, tab.connection_binding.snapshot()))
        else {
            clear_mutex_flag_if_token(&state.schema_refresh_in_progress, schema_refresh_token);
            return false;
        };
        let Some(runtime) = binding_snapshot.runtime else {
            clear_mutex_flag_if_token(&state.schema_refresh_in_progress, schema_refresh_token);
            return false;
        };
        let connection_id = runtime.id();
        let binding_revision = binding_snapshot.revision;
        let selected_scope = binding_snapshot.scope;
        state.latest_schema_request_id = schema_refresh_token;
        state
            .object_browser
            .set_selected_scope(selected_scope.clone());
        let Some(context) = Self::metadata_pool_session_context(
            &runtime.connection(),
            "Preparing schema metadata refresh",
        )
        .map(|context| context.for_scope(selected_scope.as_deref())) else {
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
                MainWindow::load_schema_update_from_pool_context(
                    context,
                    selected_scope,
                    query_tab_id,
                    connection_id,
                    binding_revision,
                    schema_refresh_token,
                )
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
        let Some(binding_snapshot) = state
            .editor_tabs
            .iter()
            .find(|tab| tab.tab_id == state.active_editor_tab_id)
            .map(|tab| tab.connection_binding.snapshot())
        else {
            return false;
        };
        let Some(runtime) = binding_snapshot.runtime else {
            return false;
        };
        let selected_scope = binding_snapshot.scope;
        state
            .object_browser
            .set_selected_scope(selected_scope.clone());
        let Some(context) = Self::metadata_pool_session_context(
            &runtime.connection(),
            "Preparing object browser metadata refresh",
        )
        .map(|context| context.for_scope(selected_scope.as_deref())) else {
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

    fn execute_table_browse_request(
        state: &Arc<Mutex<AppState>>,
        tab_id: QueryTabId,
        mut request: TableBrowsePageRequest,
    ) -> Result<(), String> {
        let (editor, sql) = {
            let mut state_guard = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let editor = state_guard
                .editor_tabs
                .iter()
                .find(|tab| tab.tab_id == tab_id)
                .map(|tab| tab.sql_editor.clone())
                .ok_or_else(|| "The owning query tab is closed.".to_string())?;
            if editor.is_query_running() {
                return Err("The owning query tab is already running a query.".to_string());
            }
            let mut result_tabs = state_guard
                .result_tabs_for_tab(tab_id)
                .ok_or_else(|| "The result workspace is closed.".to_string())?;
            // A query result the user asked to filter carries a filter bar
            // while still being a plain query tab, so that statement results
            // keep taking the statement path. Applying a filter is the moment a
            // page query really starts, so convert it here.
            // The promoted tab has no applied page size to inherit, so it takes
            // the page-size control's, exactly as opening a table from the
            // object browser does.
            let page_size =
                result_page_unit_for_choice_index(state_guard.result_page_unit_choice.value());
            if !result_tabs.is_table_browse_tab(request.result_tab_id)
                && (!result_tabs.result_tab_has_filter_bar(request.result_tab_id)
                    || !result_tabs.promote_query_tab_to_table_browse(&request, page_size))
            {
                return Err("The table result tab is closed.".to_string());
            }
            if !state_guard.result_origin_is_current_for_tab(tab_id, &result_tabs) {
                return Err(
                    "This table result belongs to an older connection, reconnect, or scope."
                        .to_string(),
                );
            }
            if !result_tabs.normalize_table_browse_request(&mut request) {
                return Err("The table result tab is closed.".to_string());
            }
            let sql = match request.navigation {
                TableBrowseNavigation::Page => request.page_sql()?,
                TableBrowseNavigation::Last => request.count_sql()?,
            };
            result_tabs.begin_table_browse_request(request.clone())?;
            state_guard
                .result_grid_execution_targets
                .insert(tab_id, request.result_tab_id);
            if request.navigation == TableBrowseNavigation::Last {
                state_guard.pending_table_browse_last.insert(
                    tab_id,
                    PendingTableBrowseLast {
                        request: request.clone(),
                        rows: Vec::new(),
                        error: None,
                    },
                );
            }
            (editor, sql)
        };

        if editor.execute_materialized_sql_text(&sql) {
            Ok(())
        } else {
            let mut state_guard = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state_guard.result_grid_execution_targets.remove(&tab_id);
            state_guard.pending_table_browse_last.remove(&tab_id);
            if let Some(mut result_tabs) = state_guard.result_tabs_for_tab(tab_id) {
                result_tabs.fail_table_browse_result_by_id(request.result_tab_id);
            }
            Err("Failed to start table page query execution.".to_string())
        }
    }

    fn configure_result_workspace_callbacks(state: &Arc<Mutex<AppState>>, tab_id: QueryTabId) {
        let Some(mut result_tabs) = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .result_tabs_for_tab(tab_id)
        else {
            return;
        };

        let weak_state_for_change = Arc::downgrade(state);
        result_tabs.set_on_change(move || {
            let Some(state_for_change) = weak_state_for_change.upgrade() else {
                return;
            };
            if let Ok(mut state) = state_for_change.try_lock() {
                if state.active_editor_tab_id == tab_id {
                    state.refresh_result_edit_controls();
                }
            };
        });

        let weak_state_for_table_browse = Arc::downgrade(state);
        let table_browse_callback: TableBrowseExecuteCallback = Arc::new(Mutex::new(Some(
            Box::new(move |request: TableBrowsePageRequest| {
                let Some(state_for_table_browse) = weak_state_for_table_browse.upgrade() else {
                    return Err("Main window is no longer available.".to_string());
                };
                MainWindow::execute_table_browse_request(&state_for_table_browse, tab_id, request)
            }),
        )));
        result_tabs.set_table_browse_callback(table_browse_callback);

        let weak_state_for_grid_edit = Arc::downgrade(state);
        let result_tabs_for_grid_edit = result_tabs.clone();
        let grid_edit_callback: ResultGridSqlExecuteCallback = Arc::new(Mutex::new(Some(
            Box::new(move |sql: String| {
                let Some(state_for_grid_edit) = weak_state_for_grid_edit.upgrade() else {
                    return Err("Main window is no longer available.".to_string());
                };
                let mut state = state_for_grid_edit
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let editor = state
                    .editor_tabs
                    .iter()
                    .find(|tab| tab.tab_id == tab_id)
                    .map(|tab| tab.sql_editor.clone())
                    .ok_or_else(|| "The owning query tab is closed.".to_string())?;
                if editor.is_query_running() {
                    return Err("The owning query tab is already running a query.".to_string());
                }
                if !state.result_origin_is_current_for_tab(tab_id, &result_tabs_for_grid_edit) {
                    return Err(
                        "This result belongs to an older connection, reconnect, or scope and is read-only."
                            .to_string(),
                    );
                }
                let target_tab = result_tabs_for_grid_edit
                    .active_result_id()
                    .ok_or_else(|| "Open a result tab first.".to_string())?;
                if let Some(mut request) =
                    result_tabs_for_grid_edit.table_browse_applied_request(target_tab)
                {
                    request.navigation = TableBrowseNavigation::Page;
                    state.pending_table_browse_refresh.insert(
                        tab_id,
                        PendingTableBrowseRefresh {
                            request,
                            error: None,
                        },
                    );
                }
                state
                    .result_grid_execution_targets
                    .insert(tab_id, target_tab);
                editor.execute_sql_text(&sql);
                if !editor.is_query_running() {
                    state.result_grid_execution_targets.remove(&tab_id);
                    state.pending_table_browse_refresh.remove(&tab_id);
                    return Err("Failed to start query execution for result-grid edit.".to_string());
                }
                Ok(())
            }) as Box<dyn FnMut(String) -> Result<(), String>>,
        )));
        result_tabs.set_execute_sql_callback(grid_edit_callback);

        let weak_state_for_structured_edit = Arc::downgrade(state);
        let result_tabs_for_structured_edit = result_tabs.clone();
        let structured_edit_callback: ResultGridEditExecuteCallback = Arc::new(Mutex::new(Some(
            Box::new(move |request: crate::db::ResultEditRequest| {
                let Some(state_for_grid_edit) = weak_state_for_structured_edit.upgrade() else {
                    return Err("Main window is no longer available.".to_string());
                };
                let mut state = state_for_grid_edit
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let editor = state
                    .editor_tabs
                    .iter()
                    .find(|tab| tab.tab_id == tab_id)
                    .map(|tab| tab.sql_editor.clone())
                    .ok_or_else(|| "The owning query tab is closed.".to_string())?;
                if editor.is_query_running() {
                    return Err("The owning query tab is already running a query.".to_string());
                }
                if !state.result_origin_is_current_for_tab(tab_id, &result_tabs_for_structured_edit)
                {
                    return Err(
                        "This result belongs to an older connection, reconnect, or scope and is read-only."
                            .to_string(),
                    );
                }
                let target_tab = result_tabs_for_structured_edit
                    .active_result_id()
                    .ok_or_else(|| "Open a result tab first.".to_string())?;
                if let Some(mut page_request) =
                    result_tabs_for_structured_edit.table_browse_applied_request(target_tab)
                {
                    page_request.navigation = TableBrowseNavigation::Page;
                    state.pending_table_browse_refresh.insert(
                        tab_id,
                        PendingTableBrowseRefresh {
                            request: page_request,
                            error: None,
                        },
                    );
                }
                state
                    .result_grid_execution_targets
                    .insert(tab_id, target_tab);
                if let Err(error) = editor.execute_result_edit(request) {
                    state.result_grid_execution_targets.remove(&tab_id);
                    state.pending_table_browse_refresh.remove(&tab_id);
                    return Err(error);
                }
                Ok(())
            }) as Box<dyn FnMut(crate::db::ResultEditRequest) -> Result<(), String>>,
        )));
        result_tabs.set_execute_edit_callback(structured_edit_callback);

        let weak_state_for_lazy_fetch = Arc::downgrade(state);
        let lazy_fetch_callback = Arc::new(Mutex::new(Some(Box::new(move |session_id, request| {
            let Some(state_for_lazy_fetch) = weak_state_for_lazy_fetch.upgrade() else {
                return false;
            };
            AppState::request_lazy_fetch_on_editors(&state_for_lazy_fetch, session_id, request)
        })
            as Box<dyn FnMut(u64, crate::ui::sql_editor::LazyFetchRequest) -> bool>)));
        result_tabs.set_lazy_fetch_callback(lazy_fetch_callback);

        let weak_state_for_close = Arc::downgrade(state);
        result_tabs.set_on_close(move |target| {
            let Some(state_for_close) = weak_state_for_close.upgrade() else {
                return;
            };
            {
                let mut state = state_for_close
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if state.active_editor_tab_id != tab_id {
                    state.activate_editor_tab(tab_id);
                }
            }
            MainWindow::close_result_tab_by_target(&state_for_close, target);
        });

        let file_sender = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .file_sender
            .clone();
        if let Some(file_sender) = file_sender {
            let weak_state_for_context = Arc::downgrade(state);
            let callback = Arc::new(Mutex::new(Some(Box::new(
                move |action: ResultTableContextAction| {
                    let Some(state_for_context) = weak_state_for_context.upgrade() else {
                        return;
                    };
                    {
                        let mut state = state_for_context
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        if state.active_editor_tab_id != tab_id {
                            state.activate_editor_tab(tab_id);
                        }
                    }
                    match action {
                        ResultTableContextAction::ExportData => {
                            MainWindow::export_current_results(&state_for_context, &file_sender);
                        }
                        ResultTableContextAction::Close => {
                            MainWindow::close_current_result_tab(&state_for_context);
                        }
                        ResultTableContextAction::CloseAll => {
                            MainWindow::close_all_result_tabs(&state_for_context);
                        }
                        ResultTableContextAction::CopySqlInserts
                        | ResultTableContextAction::CopySqlUpdates
                        | ResultTableContextAction::CopyWhereClause => {
                            MainWindow::copy_result_selection_as_sql(
                                &state_for_context,
                                &file_sender,
                                action,
                            );
                        }
                    }
                },
            )
                as Box<dyn FnMut(ResultTableContextAction)>)));
            result_tabs.set_context_action_callback(callback);
        }
    }

    fn attach_editor_callbacks(
        state: &Arc<Mutex<AppState>>,
        tab_id: QueryTabId,
        schema_sender: std::sync::mpsc::Sender<SchemaUpdate>,
    ) {
        Self::configure_result_workspace_callbacks(state, tab_id);
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
            if s.active_editor_tab_id != tab_id {
                return;
            }
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
            s.append_result_tab_request_for_tab(tab_id, request);
        });

        let weak_state_for_status = Arc::downgrade(state);
        editor.set_status_callback(move |message| {
            let Some(state_for_status) = weak_state_for_status.upgrade() else {
                return;
            };
            if let Ok(mut s) = state_for_status.try_lock() {
                if s.active_editor_tab_id == tab_id {
                    s.set_status_message(message);
                }
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
            let Some(mut owning_result_tabs) = s.result_tabs_for_tab(tab_id) else {
                return;
            };
            let (progress, operation_token) = match progress {
                QueryProgress::Operation { token, progress } => {
                    if !s.operation_progress_matches(tab_id, token, progress.inner()) {
                        return;
                    }
                    (*progress, Some(token))
                }
                QueryProgress::OperationAbandoned { token } => {
                    if s.operation_abandoned_matches(tab_id, token)
                        && s.mark_operation_abandoned_cancelled(tab_id, token)
                    {
                        if s.should_show_progress_status_for_tab(tab_id) {
                            s.set_status_message(ResultTabStatus::Cancelled.status_bar_message());
                        }
                        s.refresh_result_edit_controls();
                        s.sync_transaction_mode_controls();
                        s.schedule_cursor_reset_when_tab_is_idle(tab_id);
                    }
                    return;
                }
                QueryProgress::OperationFinished { token } => {
                    let token_matches_editor = token.tab_id == tab_id
                        && s.find_tab_index(tab_id)
                            .and_then(|index| s.editor_tabs.get(index))
                            .is_some_and(|tab| {
                                tab.sql_editor.editor_instance_id() == token.editor_id
                            });
                    if token_matches_editor && s.clear_query_cancel_request(token) {
                        s.refresh_result_edit_controls();
                    }
                    s.refresh_tab_label(tab_id);
                    drop(s);
                    let state_for_transient_cleanup = state_for_progress.clone();
                    crate::ui::ui_timeout::schedule(0.0, move || {
                        state_for_transient_cleanup
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .remove_idle_transient_runtimes();
                    });
                    return;
                }
                QueryProgress::CancelOutcome { token, outcome } => {
                    let token_matches_editor = token.tab_id == tab_id
                        && s.find_tab_index(tab_id)
                            .and_then(|index| s.editor_tabs.get(index))
                            .is_some_and(|tab| {
                                tab.sql_editor.editor_instance_id() == token.editor_id
                            });
                    if token_matches_editor && s.apply_query_cancel_outcome(token, &outcome) {
                        s.refresh_result_edit_controls();
                    }
                    return;
                }
                progress => (progress, None),
            };
            let (progress, statement_origin) = match progress {
                QueryProgress::StatementOrigin { origin, progress } => {
                    (*progress, Some(origin))
                }
                progress => (progress, None),
            };
            if let Some(origin) = statement_origin {
                owning_result_tabs.set_execution_origin(Some(origin));
            }
            match progress {
                QueryProgress::Operation { .. } => {}
                QueryProgress::StatementOrigin { .. } => {}
                // History-only event: recorded by the editor's progress
                // handler, with no result pane to update here.
                QueryProgress::StatementCancelledHistory { .. } => {}
                QueryProgress::OperationAbandoned { token } => {
                    if operation_token == Some(token)
                        && s.operation_abandoned_matches(tab_id, token)
                        && s.mark_operation_abandoned_cancelled(tab_id, token)
                    {
                        if s.should_show_progress_status_for_tab(tab_id) {
                            s.set_status_message(ResultTabStatus::Cancelled.status_bar_message());
                        }
                        s.refresh_result_edit_controls();
                        s.sync_transaction_mode_controls();
                        s.schedule_cursor_reset_when_tab_is_idle(tab_id);
                    }
                }
                QueryProgress::CancelOutcome { token, outcome } => {
                    if operation_token == Some(token)
                        && s.apply_query_cancel_outcome(token, &outcome)
                    {
                        s.refresh_result_edit_controls();
                    }
                }
                QueryProgress::OperationFinished { token } => {
                    if operation_token.is_none_or(|outer_token| outer_token == token)
                        && s.clear_query_cancel_request(token)
                    {
                        s.refresh_result_edit_controls();
                    }
                    s.refresh_tab_label(tab_id);
                }
                QueryProgress::BatchStart {
                    activity,
                    total_units,
                    status_activity,
                } => {
                    let execution_origin = s
                        .editor_tabs
                        .iter()
                        .find(|tab| tab.tab_id == tab_id)
                        .and_then(|tab| tab.connection_binding.snapshot().execution_origin());
                    owning_result_tabs.set_execution_origin(execution_origin);
                    s.refresh_tab_label(tab_id);
                    let one_tab_per_query = s.result_one_tab_per_query_check.value();
                    let grid_execution_target =
                        s.result_grid_execution_targets.get(&tab_id).copied();
                    let table_browse_page_loading = grid_execution_target.is_some_and(|target| {
                        owning_result_tabs.table_browse_is_loading(target)
                    });
                    let table_browse_counting = s.pending_table_browse_last.contains_key(&tab_id);
                    let lazy_fetch_sessions = if !one_tab_per_query
                        && grid_execution_target.is_none()
                    {
                        s.clear_result_grids_for_new_query_batch(tab_id)
                    } else {
                        Vec::new()
                    };
                    let runtime = s
                        .editor_tabs
                        .iter()
                        .find(|tab| tab.tab_id == tab_id)
                        .and_then(|tab| tab.connection_binding.snapshot().runtime);
                    let db_type = runtime
                        .as_ref()
                        .map(|runtime| runtime.sanitized_info().db_type);
                    let connection_id = runtime.as_ref().map(|runtime| runtime.id());
                    let mut context = QueryProgressContext::new(
                        grid_execution_target,
                        activity,
                        operation_token,
                    );
                    context.start_status_tracking(
                        total_units,
                        db_type,
                        connection_id,
                        status_activity,
                    );
                    if table_browse_counting {
                        context.update_status_activity("Counting rows for last page");
                    } else if table_browse_page_loading {
                        context.update_status_activity("Loading table page");
                    }
                    if s.query_cancel_is_dispatched(operation_token) {
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
                    let mut result_tabs = owning_result_tabs.clone();
                    let query_canceling_pending = s.query_cancel_is_dispatched(operation_token);
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
                        context.update_status_activity(status.label());
                        (result_tab_id, status, select_tab)
                    };
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
                    column_kinds,
                    null_text,
                    sql,
                } => {
                    if columns.is_empty() {
                        return;
                    }
                    if s.pending_table_browse_last.contains_key(&tab_id) {
                        let result_tabs = owning_result_tabs.clone();
                        let Some(context) = s.progress_contexts.get_mut(&tab_id) else {
                            return;
                        };
                        let _ = context.ensure_result_tab_id(index, || {
                            result_tabs.reserve_result_tab_id()
                        });
                        context.fetch_row_counts.insert(index, 0);
                        context.active_statement_index = Some(index);
                        context.state_label = ResultTabStatus::Fetching.label().to_string();
                        context.update_status_activity("Counting rows for last page");
                        return;
                    }
                    let mut result_tabs = owning_result_tabs.clone();
                    let pending_canceling_sessions =
                        s.pending_lazy_fetch_canceling_sessions.clone();
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
                            context.update_status_activity("Fetching rows: 0");
                        }
                        context.last_fetch_status_update = Instant::now();
                        (
                            result_tab_id,
                            lazy_fetch_session,
                            preserve_canceling,
                            select_tab,
                        )
                    };
                    // The grid sorts locally but must agree with the server on
                    // where NULLs go, and it is never told which backend it is
                    // showing — so resolve that from the tab's connection here.
                    // The filter bar needs the same answer, plus the editor
                    // tab's metadata for its own completion.
                    let editor_tab = s.editor_tabs.iter().find(|tab| tab.tab_id == tab_id);
                    let result_db_type = editor_tab
                        .and_then(|tab| tab.connection_binding.snapshot().runtime)
                        .map(|runtime| runtime.sanitized_info().db_type);
                    let filter_intellisense =
                        editor_tab.map(|tab| tab.intellisense_data.clone());
                    let filter_scope = s
                        .active_connection_id()
                        .and_then(|id| s.object_browser.selected_scope_for_connection(id));
                    s.refresh_result_edit_controls();
                    drop(s);
                    result_tabs.ensure_statement_tab_by_id(result_tab_id, "Result", select_tab);
                    if let Some(db_type) = result_db_type {
                        result_tabs.set_sort_null_ordering_by_id(
                            result_tab_id,
                            if db_type.sorts_nulls_last_ascending() {
                                NullOrdering::LastOnAscending
                            } else {
                                NullOrdering::FirstOnAscending
                            },
                        );
                    }
                    result_tabs.start_streaming_by_id(
                        result_tab_id,
                        &columns,
                        &column_kinds,
                        &null_text,
                        &sql,
                    );
                    // Offer the filter only where it can actually run: a result
                    // this backend cannot re-query gets no bar at all.
                    if let (Some(db_type), Some(intellisense_data)) =
                        (result_db_type, filter_intellisense)
                    {
                        MainWindow::offer_result_filter(
                            &mut result_tabs,
                            result_tab_id,
                            db_type,
                            filter_scope,
                            &sql,
                            &columns,
                            intellisense_data,
                        );
                    }
                    if let Some(session_id) = lazy_fetch_session {
                        result_tabs.set_lazy_fetch_session_by_id(result_tab_id, session_id);
                    }
                    if preserve_canceling {
                        result_tabs.mark_statement_canceling_by_id(result_tab_id);
                    }
                }
                QueryProgress::ResultEditMetadata { index, descriptor } => {
                    let Some(result_tab_id) = resolve_active_progress_tab_id(&s, tab_id, index)
                    else {
                        return;
                    };
                    let mut result_tabs = owning_result_tabs.clone();
                    drop(s);
                    result_tabs.set_result_edit_descriptor_by_id(result_tab_id, descriptor);
                }
                QueryProgress::Rows { index, rows } => {
                    if s.pending_table_browse_last.contains_key(&tab_id) {
                        let rows_len = rows.len();
                        if let Some(pending) = s.pending_table_browse_last.get_mut(&tab_id) {
                            pending.rows.extend(rows);
                        }
                        if let Some(context) = s.progress_contexts.get_mut(&tab_id) {
                            let count = context.fetch_row_counts.entry(index).or_insert(0);
                            *count = count.saturating_add(rows_len);
                            context.active_statement_index = Some(index);
                        }
                        return;
                    }
                    let Some(result_tab_id) = resolve_active_progress_tab_id(&s, tab_id, index)
                    else {
                        return;
                    };
                    let rows_len = rows.len();
                    let mut result_tabs = owning_result_tabs.clone();
                    let pending_canceling_sessions =
                        s.pending_lazy_fetch_canceling_sessions.clone();
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
                            if should_update_fetch_status(
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
                    if let Some(status_message) = status_update {
                        context.update_status_activity(&status_message);
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
                    let mut result_tabs = owning_result_tabs.clone();
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
                    let preserve_canceling = pending_canceling;
                    let should_show_status = s.should_show_progress_status_for_tab(tab_id);
                    let result_tab_id = if let Some(context) = s.progress_contexts.get_mut(&tab_id) {
                        if context.closed_statement_indices.contains(&index) {
                            return;
                        }
                        if !context.mark_lazy_fetch_waiting(session_id, index) {
                            return;
                        }
                        context.active_statement_index = Some(index);
                        context.state_label = if preserve_canceling {
                            ResultTabStatus::Canceling.label().to_string()
                        } else {
                            ResultTabStatus::Waiting.label().to_string()
                        };
                        context.update_status_activity(if preserve_canceling {
                            "Canceling lazy fetch"
                        } else {
                            "Waiting for more rows"
                        });
                        let Some(result_tab_id) = context.result_tab_id_for_statement(index) else {
                            return;
                        };
                        result_tab_id
                    } else {
                        return;
                    };
                    let mut result_tabs = owning_result_tabs.clone();
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
                    let should_show_status = s.should_show_progress_status_for_tab(tab_id);
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
                QueryProgress::LazyFetchCancelFailed {
                    session_id,
                    message,
                } => {
                    let failure_is_current = s.mark_lazy_fetch_cancel_failed(session_id);
                    if failure_is_current {
                        s.refresh_result_edit_controls();
                    }
                    crate::utils::logging::log_error("lazy fetch cancel", &message);
                    if failure_is_current {
                        let mut result_tabs = owning_result_tabs.clone();
                        drop(s);
                        result_tabs.append_message_lines(
                            ResultMessageKind::Error,
                            &[format!("Cancel failed: {message}")],
                        );
                        result_tabs.select_messages_errors();
                    }
                }
                QueryProgress::ExecutionAbandoned {
                    materialized_grid_statement,
                    message,
                } => {
                    // The statement was reported as started, so its routing and
                    // the browse tab it left loading are released here or not
                    // at all: nothing else will ever finish them, and a routing
                    // left behind would capture the next query's result.
                    let stranded_target = if materialized_grid_statement {
                        s.pending_table_browse_last.remove(&tab_id);
                        s.pending_table_browse_refresh.remove(&tab_id);
                        s.result_grid_execution_targets.remove(&tab_id)
                    } else {
                        None
                    };
                    if s.should_show_progress_status_for_tab(tab_id) {
                        s.set_status_message(&message);
                    }
                    s.refresh_result_edit_controls();
                    let mut result_tabs = owning_result_tabs.clone();
                    drop(s);
                    if let Some(result_tab_id) = stranded_target {
                        result_tabs.fail_table_browse_result_by_id(result_tab_id);
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
                        orphaned_canceling_close = pending_canceling_close
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
                            context.update_status_activity(if should_abort_result_tab {
                                ResultTabStatus::Cancelled.label()
                            } else {
                                ResultTabStatus::Done.label()
                            });
                            result_tab_id = context.result_tab_id_for_statement(index);
                        }
                        finished_all_lazy_fetches =
                            context.lazy_fetch_sessions.is_empty() && context.batch_finished;
                    }
                    if event_matches || !active_lazy_fetch_still_present {
                        s.pending_lazy_fetch_canceling_sessions.remove(&session_id);
                        s.orphaned_lazy_fetch_missing_since.remove(&session_id);
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
                    let mut result_tabs = owning_result_tabs.clone();
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
                    let mut result_tabs = owning_result_tabs.clone();
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
                    let mut result_tabs = owning_result_tabs.clone();
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
                    let mut result_tabs = owning_result_tabs.clone();
                    drop(s);
                    result_tabs.append_message_lines(kind, &lines);
                    if kind == ResultMessageKind::Error {
                        result_tabs.select_messages_errors();
                    } else if should_select_info {
                        result_tabs.select_messages_info();
                    }
                }
                QueryProgress::ExplainPlanOutput { text } => {
                    let mut result_tabs = owning_result_tabs.clone();
                    drop(s);
                    result_tabs.append_explain_plan_tab(&text);
                }
                QueryProgress::PromptInput { .. } => {}
                QueryProgress::RequestCancelOldestLazyFetchForSessionPool { response } => {
                    if let Some(session_id) = s.oldest_lazy_fetch_session_for_tab(tab_id) {
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
                    if let Some(session_id) = s.oldest_lazy_fetch_session_for_tab(tab_id) {
                        drop(s);
                        let _ = request_lazy_fetch_cancel_for_session_pool(
                            &state_for_progress,
                            session_id,
                        );
                    }
                }
                QueryProgress::AutoCommitChanged { enabled } => {
                    if s.should_show_progress_status_for_tab(tab_id) {
                        s.set_status_message(auto_commit_changed_progress_status(enabled));
                    }
                    drop(s);
                }
                QueryProgress::ConnectionChanged { info } => {
                    if let Some(info) = info {
                        if let Some(runtime) = s
                            .editor_tabs
                            .iter()
                            .find(|tab| tab.tab_id == tab_id)
                            .and_then(|tab| tab.connection_binding.snapshot().runtime)
                        {
                            runtime.update_sanitized_info(info.clone());
                            runtime.set_state(ConnectionRuntimeState::Connected);
                            s.object_browser.add_runtime(runtime.clone());
                            s.synchronize_scope_for_connection(runtime.id(), None);
                            if s.active_editor_tab_id == tab_id {
                                s.object_browser
                                    .set_active_connection(Some(runtime.id()));
                            }
                        }
                        if s.active_editor_tab_id == tab_id {
                            let _ = s.set_active_editor_tab(tab_id);
                            s.set_status_message(&format!("Connected | {}", info.name));
                        }
                        // CONNECT can appear mid-script. Deferring metadata fetch prevents
                        // metadata workers from competing with the active batch.
                        s.mark_metadata_refresh_pending(tab_id);
                        let origin = s
                            .editor_tabs
                            .iter()
                            .find(|tab| tab.tab_id == tab_id)
                            .and_then(|tab| {
                                tab.connection_binding.snapshot().execution_origin()
                            });
                        owning_result_tabs.set_execution_origin(origin);
                        s.refresh_tab_label(tab_id);
                    } else {
                        owning_result_tabs.set_execution_origin(None);
                        if s.active_editor_tab_id == tab_id {
                            let _ = s.set_active_editor_tab(tab_id);
                            s.set_status_message("Query tab detached");
                        }
                        s.refresh_tab_label(tab_id);
                    }
                    drop(s);
                }
                QueryProgress::DatabaseChanged { info } => {
                    let database = info.service_name.trim().to_string();
                    let mut retained_scope_update = None;
                    if !database.is_empty() {
                        let connection_id = s
                            .editor_tabs
                            .iter()
                            .find(|tab| tab.tab_id == tab_id)
                            .and_then(|tab| tab.connection_binding.snapshot().connection_id());
                        if let Some(connection_id) = connection_id {
                            let selected_scope = Some(database.clone());
                            if s.synchronize_scope_for_connection(
                                connection_id,
                                selected_scope.clone(),
                            ) {
                                retained_scope_update = s.retained_scope_update_for_connection(
                                    connection_id,
                                    selected_scope,
                                );
                            }
                        }
                        if s.active_editor_tab_id == tab_id {
                            s.set_status_message(&format!("Database selected | {}", database));
                        }
                    }
                    drop(s);
                    if let Some(message) = retained_scope_update
                        .map(apply_retained_scope_update)
                        .and_then(|outcomes| first_retained_outcome_message(&outcomes))
                    {
                        crate::ui::alert_on_main(&format!(
                            "Database changed for this connection, but a retained tab session could not be updated:\n{}",
                            message
                        ));
                    }
                }
                QueryProgress::ScopeChangedNotice {
                    message,
                    selected_scope,
                } => {
                    let selected_scope = selected_scope
                        .map(|scope| scope.trim().to_string())
                        .filter(|scope| !scope.is_empty());
                    let connection_id = s
                        .editor_tabs
                        .iter()
                        .find(|tab| tab.tab_id == tab_id)
                        .and_then(|tab| tab.connection_binding.snapshot().connection_id());
                    let retained_scope_update = connection_id.and_then(|connection_id| {
                        s.synchronize_scope_for_connection(
                            connection_id,
                            selected_scope.clone(),
                        );
                        s.retained_scope_update_for_connection(
                            connection_id,
                            selected_scope.clone(),
                        )
                    });
                    let origin = s
                        .editor_tabs
                        .iter()
                        .find(|tab| tab.tab_id == tab_id)
                        .and_then(|tab| tab.connection_binding.snapshot().execution_origin());
                    owning_result_tabs.set_execution_origin(origin);
                    if s.active_editor_tab_id == tab_id {
                        let status = message.lines().next().unwrap_or(&message).to_string();
                        s.set_status_message(&status);
                    }
                    drop(s);
                    if let Some(message) = retained_scope_update
                        .map(apply_retained_scope_update)
                        .and_then(|outcomes| first_retained_outcome_message(&outcomes))
                    {
                        crate::ui::alert_on_main(&format!(
                            "Scope changed for this connection, but a retained tab session could not be updated:\n{}",
                            message
                        ));
                    }
                }
                QueryProgress::StatementFinished { index, result, .. } => {
                    if s.pending_table_browse_last.contains_key(&tab_id) {
                        if let Some(pending) = s.pending_table_browse_last.get_mut(&tab_id) {
                            if pending.rows.is_empty() && !result.rows.is_empty() {
                                pending.rows.extend(result.rows.clone());
                            }
                            if !result.success {
                                pending.error = Some(result.message.clone());
                            }
                        }
                        if let Some(context) = s.progress_contexts.get_mut(&tab_id) {
                            context.fetch_row_counts.remove(&index);
                            context.mark_statement_finished(index);
                            context.mark_status_unit_complete(index);
                            let status = ResultTabStatus::from_query_result(&result);
                            context.state_label = status.label().to_string();
                            context.update_status_activity(status.label());
                        }
                        return;
                    }
                    let should_display_data_grid = should_display_result_in_data_grid(&result);
                    let mut result_tabs = owning_result_tabs.clone();
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
                            (
                                context
                                    .result_tab_id_for_statement(index)
                                    .or(context.execution_target),
                                false,
                            )
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
                    let table_browse_result = result_tab_id
                        .is_some_and(|result_tab_id| result_tabs.is_table_browse_tab(result_tab_id));
                    let table_browse_page_loading = result_tab_id.is_some_and(|result_tab_id| {
                        result_tabs.table_browse_is_loading(result_tab_id)
                    });
                    if !result.success {
                        if let Some(pending) = s.pending_table_browse_refresh.get_mut(&tab_id) {
                            if result_tab_id == Some(pending.request.result_tab_id) {
                                pending.error = Some(result.message.clone());
                            }
                        }
                    }
                    let remove_empty_error_grid = result_status == ResultTabStatus::Error
                        && result_tab_id.is_some()
                        && !table_browse_result
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
                        context.mark_status_unit_complete(index);
                        context.state_label = result_status.label().to_string();
                        context.update_status_activity(result_status.label());
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

                    if table_browse_page_loading && !result.success {
                        if let Some(result_tab_id) = result_tab_id {
                            result_tabs.fail_table_browse_result_by_id(result_tab_id);
                        }
                    } else if remove_empty_error_grid || remove_empty_success_grid {
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
                    if let Some(token) = operation_token {
                        s.clear_query_cancel_request(token);
                    }
                    let panicked_grid_target = s
                        .progress_contexts
                        .get(&tab_id)
                        .and_then(|context| context.execution_target);
                    let table_browse_failure_target = panicked_grid_target
                        .filter(|target| owning_result_tabs.table_browse_is_loading(*target));
                    if let Some(pending) =
                        s.pending_table_browse_last.get_mut(&tab_id).filter(|pending| {
                            batch_owns_grid_target(
                                panicked_grid_target,
                                Some(pending.request.result_tab_id),
                            )
                        })
                    {
                        pending.error.get_or_insert_with(|| message.clone());
                    }
                    if s.should_show_progress_status_for_tab(tab_id) {
                        s.set_status_message(&message);
                    }
                    s.refresh_result_edit_controls();
                    s.sync_transaction_mode_controls();
                    drop(s);
                    if let Some(result_tab_id) = table_browse_failure_target {
                        let mut result_tabs = owning_result_tabs.clone();
                        result_tabs.fail_table_browse_result_by_id(result_tab_id);
                    }
                }
                QueryProgress::MetadataRefreshNeeded => {
                    s.mark_metadata_refresh_pending(tab_id);
                    if s.active_editor_tab_id == tab_id
                        && s.has_live_connection
                        && !s.has_running_query_or_lazy_fetch_for_tab(tab_id)
                    {
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
                            let (current_operation_id, last_completed_operation_id) =
                                tab.sql_editor.operation_lifecycle_ids();
                            (
                                tab.sql_editor.editor_instance_id(),
                                current_operation_id,
                                last_completed_operation_id,
                            )
                        });
                    let current_connection_generation = s
                        .find_tab_index(tab_id)
                        .and_then(|index| s.editor_tabs.get(index))
                        .and_then(|tab| tab.connection_binding.snapshot().connection())
                        .and_then(|connection| {
                            crate::db::try_lock_connection(&connection)
                                .map(|connection| connection.connection_generation())
                        });
                    // ExecutionFinished can update status text after cleanup.
                    // This status-only path must neither block on the connection
                    // mutex nor apply an old connection's completion after a
                    // reconnect. A busy connection is therefore treated as an
                    // unverifiable stale event and skipped.
                    let event_matches_current_editor =
                        execution_finished_event_matches_current_editor(
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
                    );
                    let event_matches_retained_context =
                        execution_finished_event_matches_retained_context(
                            &event,
                            tab_id,
                            current_editor.map(|(editor_id, _, _)| editor_id),
                            current_connection_generation,
                            s.progress_contexts.get(&tab_id),
                        );
                    if !event_matches_current_editor && !event_matches_retained_context {
                        return;
                    }
                    if let Some(token) = operation_token {
                        s.clear_query_cancel_request(token);
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
                    if let Some(token) = operation_token {
                        s.clear_query_cancel_request(token);
                    }
                    let pending_canceling_sessions =
                        s.pending_lazy_fetch_canceling_sessions.clone();
                    let should_show_status = s.should_show_progress_status_for_tab(tab_id);
                    if let Some(context) = s.progress_contexts.get_mut(&tab_id) {
                        if !context.lazy_fetch_sessions.is_empty() {
                            context.batch_finished = true;
                            let preserve_canceling =
                                context.lazy_fetch_sessions.keys().any(|session_id| {
                                    pending_canceling_sessions.contains(session_id)
                                });
                            let has_waiting_lazy_fetch = context.has_waiting_lazy_fetch();
                            if preserve_canceling {
                                context.state_label =
                                    ResultTabStatus::Canceling.label().to_string();
                            } else if has_waiting_lazy_fetch {
                                context.state_label = ResultTabStatus::Waiting.label().to_string();
                                context.update_status_activity("Waiting for more rows");
                            } else {
                                context.state_label = ResultTabStatus::Fetching.label().to_string();
                                context.update_status_activity("Fetching rows");
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
                    let finished_grid_target = s
                        .progress_contexts
                        .get(&tab_id)
                        .and_then(|context| context.execution_target);
                    let batch_owns_pending_last =
                        s.pending_table_browse_last.get(&tab_id).is_some_and(|pending| {
                            batch_owns_grid_target(
                                finished_grid_target,
                                Some(pending.request.result_tab_id),
                            )
                        });
                    let unfinished_table_browse_target = finished_grid_target.filter(|target| {
                        should_fail_table_browse_at_batch_end(
                            owning_result_tabs.table_browse_is_loading(*target),
                            batch_owns_pending_last,
                        )
                    });
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
                    if let Some(result_tab_id) = canceling_tab_id {
                        owning_result_tabs.mark_statement_cancelled_by_id(result_tab_id);
                    }
                    s.finish_progress_context(tab_id);
                    let should_trim = !s.is_any_query_running();
                    let mut result_tabs = owning_result_tabs.clone();
                    drop(s);

                    result_tabs.finish_non_lazy_streaming();
                    if let Some(result_tab_id) = unfinished_table_browse_target {
                        result_tabs.fail_table_browse_result_by_id(result_tab_id);
                    }
                    let recovered_save_states = result_tabs.clear_orphaned_save_requests();
                    let recovered_edit_states = result_tabs.clear_orphaned_query_edit_backups();

                    let mut s = state_for_progress
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if batch_owns_grid_target(
                        finished_grid_target,
                        s.result_grid_execution_targets.get(&tab_id).copied(),
                    ) {
                        s.result_grid_execution_targets.remove(&tab_id);
                    }
                    let pending_last = match s.pending_table_browse_last.get(&tab_id) {
                        Some(pending)
                            if batch_owns_grid_target(
                                finished_grid_target,
                                Some(pending.request.result_tab_id),
                            ) =>
                        {
                            s.pending_table_browse_last.remove(&tab_id)
                        }
                        _ => None,
                    };
                    let pending_refresh = match s.pending_table_browse_refresh.get(&tab_id) {
                        Some(pending)
                            if batch_owns_grid_target(
                                finished_grid_target,
                                Some(pending.request.result_tab_id),
                            ) =>
                        {
                            s.pending_table_browse_refresh.remove(&tab_id)
                        }
                        _ => None,
                    };
                    if s.active_editor_tab_id == tab_id
                        && s.pending_connection_metadata_refresh
                        && s.has_live_connection
                    {
                        let started = MainWindow::start_connection_metadata_refresh(
                            &mut s,
                            &schema_sender_for_progress,
                        );
                        s.update_pending_metadata_refresh_after_start_attempt(started);
                    }
                    if should_trim {
                        // Query execution completed and large temporary buffers may
                        // have been released during result materialization.
                        malloc_trim_process();
                    }
                    if recovered_save_states > 0 {
                        result_tabs.append_message_lines(
                            ResultMessageKind::Info,
                            &["Save was interrupted. Staged edits are still available."
                                .to_string()],
                        );
                    } else if recovered_edit_states > 0 {
                        result_tabs.append_message_lines(
                            ResultMessageKind::Info,
                            &["Query ended before completion. Restored staged result-grid edits."
                                .to_string()],
                        );
                    }
                    s.render_status_bar();
                    s.refresh_result_edit_controls();
                    s.sync_transaction_mode_controls();
                    let mut last_page_followup = None;
                    let mut last_page_failure = None;
                    let mut edit_page_followup = None;
                    if let Some(pending) = pending_last {
                        if let Some(message) = pending.error {
                            last_page_failure = Some((pending.request.result_tab_id, message));
                        } else {
                            let total_rows = pending
                                .rows
                                .first()
                                .and_then(|row| row.first())
                                .map(|value| value.trim().replace(',', ""))
                                .and_then(|value| value.parse::<u64>().ok());
                            if let Some(total_rows) = total_rows {
                                let mut request = pending.request;
                                request.offset = crate::ui::table_browse::last_page_offset(
                                    total_rows,
                                    request.page_size,
                                );
                                request.navigation = TableBrowseNavigation::Page;
                                last_page_followup = Some(request);
                            } else {
                                last_page_failure = Some((
                                    pending.request.result_tab_id,
                                    "The last-page row count could not be read.".to_string(),
                                ));
                            }
                        }
                    }
                    if recovered_save_states == 0 {
                        if let Some(pending) = pending_refresh {
                            if pending.error.is_none() {
                                let _ = result_tabs.capture_table_browse_current_page(
                                    pending.request.result_tab_id,
                                );
                                edit_page_followup = Some(pending.request);
                            }
                        }
                    }
                    drop(s);
                    if let Some((result_tab_id, message)) = last_page_failure {
                        result_tabs.fail_table_browse_result_by_id(result_tab_id);
                        result_tabs.append_message_lines(
                            ResultMessageKind::Error,
                            std::slice::from_ref(&message),
                        );
                        result_tabs.select_messages_errors();
                    } else if let Some(request) = last_page_followup {
                        let state_for_last_page = state_for_progress.clone();
                        crate::ui::ui_timeout::schedule(0.0, move || {
                            let result_tab_id = request.result_tab_id;
                            if let Err(message) = MainWindow::execute_table_browse_request(
                                &state_for_last_page,
                                tab_id,
                                request,
                            ) {
                                let state = state_for_last_page
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                if let Some(mut result_tabs) = state.result_tabs_for_tab(tab_id) {
                                    result_tabs.fail_table_browse_result_by_id(result_tab_id);
                                }
                                drop(state);
                                crate::ui::alert_on_main(&message);
                            }
                        });
                    } else if let Some(request) = edit_page_followup {
                        let state_for_refresh = state_for_progress.clone();
                        crate::ui::ui_timeout::schedule(0.0, move || {
                            if let Err(message) = MainWindow::execute_table_browse_request(
                                &state_for_refresh,
                                tab_id,
                                request,
                            ) {
                                crate::ui::alert_on_main(&message);
                            }
                        });
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
            let Some(state_for_drop) = weak_state_for_file_drop.upgrade() else {
                return;
            };
            let binding = {
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
                MainWindow::binding_for_selected_database(&s)
            };

            let sender = file_sender_for_drop.clone();
            thread::spawn(move || {
                let result = fs::read_to_string(&path).map_err(|err| err.to_string());
                let _ = sender.send(FileActionResult::OpenInNewTab {
                    path,
                    result,
                    binding,
                });
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
                let (popups, registry, pool_size, connect_policy) = {
                    let s = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let config = s
                        .config
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let pool_size = config.normalized_connection_pool_size();
                    let connect_policy = ConnectionAttemptPolicy::from_config(&config);
                    (
                        s.popups.clone(),
                        s.connection_registry.clone(),
                        pool_size,
                        connect_policy,
                    )
                };
                if let Some(info) = ConnectionDialog::show_with_registry(popups) {
                    let profile_name = info.name.clone();
                    let runtime = registry.saved_runtime(&profile_name).unwrap_or_else(|| {
                        let connection = create_shared_connection();
                        crate::db::lock_connection(&connection).set_connection_pool_size(pool_size);
                        registry
                            .register_saved(profile_name.clone(), connection)
                            .runtime
                    });
                    let connection = runtime.connection();
                    // `try_lock_connection` also fails while a transition is in
                    // flight or another worker holds the connection, so a
                    // missing guard means "busy", not "not connected". Starting
                    // a second connect worker there would pin the runtime at
                    // "connecting" until the lock frees and then re-login a live
                    // session, so treat it like a connect already in progress.
                    let connection_liveness = crate::db::try_lock_connection(&connection)
                        .map(|guard| guard.is_connected() && guard.has_connection_handle());
                    let already_connected = connection_liveness == Some(true);
                    let connection_in_progress = connection_liveness.is_none()
                        || matches!(
                            runtime.state(),
                            ConnectionRuntimeState::Connecting
                                | ConnectionRuntimeState::Transitioning
                        );
                    if !already_connected && !connection_in_progress {
                        runtime.update_sanitized_info(info.clone());
                    }

                    let (created_tab_id, created_editor, created_right_tile) = {
                        let mut s = state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        s.status_bar
                            .set_label(&format!("Connecting to {}...", info.name));
                        let tab_id =
                            Self::create_query_editor_tab_for_runtime(&mut s, runtime.clone());
                        (
                            tab_id,
                            tab_id.map(|_| s.sql_editor.clone()),
                            tab_id.map(|_| s.right_tile.clone()),
                        )
                    };
                    if let Some(tab_id) = created_tab_id {
                        Self::attach_editor_callbacks(state, tab_id, schema_sender.clone());
                        Self::attach_file_drop_callback(state, tab_id, file_sender.clone());
                    }
                    if let Some(mut editor) = created_editor {
                        editor.focus();
                    }
                    if let Some(mut right_tile) = created_right_tile {
                        right_tile.redraw();
                    }

                    if already_connected {
                        runtime.set_state(ConnectionRuntimeState::Connected);
                        let sanitized_info = runtime.sanitized_info();
                        let mut s = state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        *s.connection_info
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                            Some(sanitized_info.clone());
                        s.has_live_connection = true;
                        s.object_browser.add_runtime(runtime.clone());
                        s.object_browser.refresh_runtime_labels();
                        s.refresh_tab_labels_for_connection(runtime.id());
                        s.status_bar.set_label(&format!(
                            "Connected | {} ({})",
                            sanitized_info.name, sanitized_info.db_type
                        ));
                        s.refresh_connection_dependent_controls();
                        s.sync_transaction_mode_controls();
                        return true;
                    }
                    if connection_in_progress {
                        let mut s = state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        s.object_browser.add_runtime(runtime.clone());
                        s.object_browser.refresh_runtime_labels();
                        s.refresh_tab_labels_for_connection(runtime.id());
                        s.status_bar.set_label(&format!(
                            "{} is already changing connection state. The new tab is bound to it.",
                            runtime.display_name()
                        ));
                        return true;
                    }

                    runtime.set_state(ConnectionRuntimeState::Connecting);
                    if let Ok(mut s) = state.try_lock() {
                        s.object_browser.refresh_runtime_labels();
                        s.refresh_tab_labels_for_connection(runtime.id());
                    }
                    let connection_id = runtime.id();
                    let runtime_for_worker = runtime.clone();
                    let conn_sender = conn_sender.clone();
                    let spawn_failure_sender = conn_sender.clone();
                    if let Err(err) = thread::Builder::new()
                        .name("space-query-connect".to_string())
                        .spawn(move || {
                            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                                connect_shared_connection_with_policy(
                                    &connection,
                                    info.clone(),
                                    pool_size,
                                    connect_policy,
                                )
                            }))
                            .unwrap_or_else(|payload| {
                                Err(format!(
                                    "Connection worker terminated unexpectedly: {}",
                                    panic_payload_to_string(payload.as_ref())
                                ))
                            });
                            match result {
                                Ok(_) => {
                                    runtime_for_worker.refresh_state_from_connection();
                                    let mut info = info;
                                    info.clear_password();
                                    let _ = conn_sender.send(ConnectionResult::Success {
                                        connection_id,
                                        info: Box::new(info),
                                    });
                                    app::awake();
                                }
                                Err(e) => {
                                    let _ = conn_sender.send(ConnectionResult::Failure {
                                        connection_id,
                                        message: e.to_string(),
                                        preserve_existing_connection: false,
                                    });
                                    app::awake();
                                }
                            }
                        })
                    {
                        let _ = spawn_failure_sender.send(ConnectionResult::Failure {
                            connection_id,
                            message: format!("Could not start connection worker: {err}"),
                            preserve_existing_connection: false,
                        });
                        app::awake();
                    }
                }
                true
            }
            "File/Reconnect Active Connection" => {
                let (runtime, mut info, pool_size, policy) = {
                    let s = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let Some(runtime) = s.active_connection_runtime() else {
                        crate::ui::alert_on_main(
                            "The active query tab has no database connection.",
                        );
                        return true;
                    };
                    let Some(profile_name) = s.connection_registry.profile_name_for(runtime.id())
                    else {
                        crate::ui::alert_on_main(
                            "Transient script connections must be re-authenticated with CONNECT.",
                        );
                        return true;
                    };
                    let config = s
                        .config
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let Some(info) = config
                        .recent_connections
                        .iter()
                        .find(|info| info.name == profile_name)
                        .cloned()
                    else {
                        crate::ui::alert_on_main("The saved connection profile no longer exists.");
                        return true;
                    };
                    (
                        runtime,
                        info,
                        config.normalized_connection_pool_size(),
                        ConnectionAttemptPolicy::from_config(&config),
                    )
                };
                if state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .has_work_for_connection(runtime.id())
                {
                    crate::ui::alert_on_main(
                        "Finish or cancel work on the active connection before reconnecting.",
                    );
                    return true;
                }
                if !Self::resolve_pooled_sessions_before_runtime_disconnect(state, runtime.id()) {
                    return true;
                }
                match AppConfig::get_password_for_connection(&info.name) {
                    Ok(Some(password)) => info.password = password,
                    Ok(None) => {
                        crate::ui::alert_on_main(
                            "No password is stored for this saved connection. Use Connect to enter it.",
                        );
                        return true;
                    }
                    Err(err) => {
                        crate::ui::alert_on_main(&err);
                        return true;
                    }
                }

                runtime.set_state(ConnectionRuntimeState::Transitioning);
                {
                    let mut s = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    s.object_browser.refresh_runtime_labels();
                    let matching_tab_ids = s
                        .editor_tabs
                        .iter()
                        .filter(|tab| {
                            tab.connection_binding.snapshot().connection_id() == Some(runtime.id())
                        })
                        .map(|tab| tab.tab_id)
                        .collect::<Vec<_>>();
                    for tab_id in matching_tab_ids {
                        s.refresh_tab_label(tab_id);
                    }
                    s.set_status_message("Reconnecting active connection");
                }
                let connection_id = runtime.id();
                let connection = runtime.connection();
                let runtime_for_worker = runtime.clone();
                let sender = conn_sender.clone();
                let spawn_failure_sender = sender.clone();
                if let Err(err) = thread::Builder::new()
                    .name("space-query-reconnect".to_string())
                    .spawn(move || {
                        let result = panic::catch_unwind(AssertUnwindSafe(|| {
                            connect_shared_connection_with_policy(
                                &connection,
                                info.clone(),
                                pool_size,
                                policy,
                            )
                        }))
                        .unwrap_or_else(|payload| {
                            Err(format!(
                                "Reconnect worker terminated unexpectedly: {}",
                                panic_payload_to_string(payload.as_ref())
                            ))
                        });
                        match result {
                            Ok(_) => {
                                runtime_for_worker.refresh_state_from_connection();
                                info.clear_password();
                                let _ = sender.send(ConnectionResult::Success {
                                    connection_id,
                                    info: Box::new(info),
                                });
                            }
                            Err(message) => {
                                let _ = sender.send(ConnectionResult::Failure {
                                    connection_id,
                                    message,
                                    preserve_existing_connection: true,
                                });
                            }
                        }
                        app::awake();
                    })
                {
                    let _ = spawn_failure_sender.send(ConnectionResult::Failure {
                        connection_id,
                        message: format!("Could not start reconnect worker: {err}"),
                        preserve_existing_connection: true,
                    });
                    app::awake();
                }
                true
            }
            "File/Disconnect" | "File/Disconnect Active Connection" => {
                let Some(runtime) = ({
                    let s = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    s.active_connection_runtime()
                }) else {
                    crate::ui::alert_on_main("The active query tab has no database connection.");
                    return true;
                };
                let connection_id = runtime.id();

                if state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .has_work_for_connection(connection_id)
                {
                    crate::ui::alert_on_main(
                        "A query or lazy fetch is active on this connection. Stop it before disconnecting.",
                    );
                    return true;
                }

                if !Self::resolve_pooled_sessions_before_runtime_disconnect(state, connection_id) {
                    return true;
                }

                let connection = runtime.connection();
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
                drop(db_conn);
                runtime.set_state(ConnectionRuntimeState::Disconnected);

                let mut s = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                s.release_pooled_db_sessions_for_connection(connection_id);
                s.has_live_connection = false;
                *s.connection_info
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
                s.pending_connection_metadata_refresh = false;
                s.clear_pending_metadata_for_connection(connection_id);
                clear_mutex_flag(&s.schema_refresh_in_progress);
                s.set_status_message("Disconnected active connection");
                s.clear_metadata_for_connection(connection_id);
                s.object_browser.refresh_runtime_labels();
                let affected_tab_ids = s
                    .editor_tabs
                    .iter()
                    .filter(|tab| {
                        tab.connection_binding.snapshot().connection_id() == Some(connection_id)
                    })
                    .map(|tab| tab.tab_id)
                    .collect::<Vec<_>>();
                for tab_id in affected_tab_ids {
                    s.refresh_tab_label(tab_id);
                }
                s.refresh_connection_dependent_controls();
                s.sync_transaction_mode_controls();
                true
            }
            "File/Disconnect All" => {
                let runtimes = {
                    let s = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let runtimes = s.connection_registry.runtimes();
                    if let Some(runtime) = runtimes.iter().find(|runtime| {
                        matches!(
                            runtime.state(),
                            ConnectionRuntimeState::Connecting
                                | ConnectionRuntimeState::Transitioning
                        )
                    }) {
                        crate::ui::alert_on_main(&format!(
                            "Connection '{}' is changing state. Wait for it to finish before disconnecting all connections.",
                            runtime.display_name()
                        ));
                        return true;
                    }
                    if let Some(runtime) = runtimes
                        .iter()
                        .find(|runtime| s.has_work_for_connection(runtime.id()))
                    {
                        crate::ui::alert_on_main(&format!(
                            "A query or lazy fetch is active on '{}'. Stop it before disconnecting all connections.",
                            runtime.display_name()
                        ));
                        return true;
                    }
                    runtimes
                        .into_iter()
                        .filter(|runtime| {
                            matches!(runtime.state(), ConnectionRuntimeState::Connected)
                        })
                        .collect::<Vec<_>>()
                };

                if runtimes.is_empty() {
                    state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .set_status_message("No connected databases");
                    return true;
                }
                if !Self::resolve_pooled_sessions_before_exit(state) {
                    return true;
                }

                for runtime in &runtimes {
                    runtime.set_state(ConnectionRuntimeState::Transitioning);
                }
                {
                    let mut s = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    s.object_browser.refresh_runtime_labels();
                    let tab_ids = s
                        .editor_tabs
                        .iter()
                        .map(|tab| tab.tab_id)
                        .collect::<Vec<_>>();
                    for tab_id in tab_ids {
                        s.refresh_tab_label(tab_id);
                    }
                    s.set_status_message("Disconnecting all connections");
                }

                let mut disconnected = Vec::new();
                let mut failures = Vec::new();
                for runtime in runtimes {
                    let connection = runtime.connection();
                    let Some(mut db_conn) = try_lock_connection_with_activity(
                        &connection,
                        "Disconnecting all connections",
                    ) else {
                        runtime.set_state(ConnectionRuntimeState::Connected);
                        failures.push(format!(
                            "{}: {}",
                            runtime.display_name(),
                            format_connection_busy_message()
                        ));
                        continue;
                    };
                    crate::db::clear_pool_session_context_for_shared_connection(&connection);
                    db_conn.disconnect();
                    db_conn.refresh_tracked_connection();
                    drop(db_conn);
                    runtime.set_state(ConnectionRuntimeState::Disconnected);
                    disconnected.push(runtime.id());
                }

                let mut s = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                for connection_id in disconnected.iter().copied() {
                    s.release_pooled_db_sessions_for_connection(connection_id);
                    s.clear_metadata_for_connection(connection_id);
                    s.clear_pending_metadata_for_connection(connection_id);
                }
                s.pending_connection_metadata_refresh = false;
                clear_mutex_flag(&s.schema_refresh_in_progress);
                let active_tab_id = s.active_editor_tab_id;
                let _ = s.set_active_editor_tab(active_tab_id);
                s.object_browser.refresh_runtime_labels();
                let tab_ids = s
                    .editor_tabs
                    .iter()
                    .map(|tab| tab.tab_id)
                    .collect::<Vec<_>>();
                for tab_id in tab_ids {
                    s.refresh_tab_label(tab_id);
                }
                if failures.is_empty() {
                    s.set_status_message(&format!(
                        "Disconnected {} connection(s)",
                        disconnected.len()
                    ));
                } else {
                    s.set_status_message("Some connections could not be disconnected");
                    s.result_tabs
                        .append_message_lines(ResultMessageKind::Error, &failures);
                    s.result_tabs.select_messages_errors();
                }
                s.refresh_connection_dependent_controls();
                s.sync_transaction_mode_controls();
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
                    let created = MainWindow::create_query_editor_tab_for_selected_database(&mut s);
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
                MainWindow::export_current_results(state, file_sender);
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
                    let active_connection_id = s.active_connection_id();
                    let connection_has_running_work = active_connection_id
                        .is_some_and(|connection_id| s.has_work_for_connection(connection_id));
                    let connection_has_lazy_fetch =
                        active_connection_id.is_some_and(|connection_id| {
                            !s.lazy_fetch_sessions_for_connection(connection_id)
                                .is_empty()
                        });
                    if let Some(message) = transaction_option_block_message(
                        connection_has_running_work,
                        connection_has_lazy_fetch,
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
                let (config_snapshot, runtimes) = {
                    let s = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let config_snapshot = s
                        .config
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clone();
                    (config_snapshot, s.connection_registry.runtimes())
                };
                if let Some((runtime, activity)) = runtimes.iter().find_map(|runtime| {
                    connection_transition_activity(&runtime.connection())
                        .map(|activity| (runtime.clone(), activity))
                }) {
                    crate::ui::alert_on_main(&format!(
                        "Connection '{}' is busy. Current DB activity: {activity}",
                        runtime.display_name()
                    ));
                    return true;
                }
                if let Some(settings) = show_settings_dialog(&config_snapshot) {
                    let pool_size_changed = settings.connection_pool_size
                        != config_snapshot.normalized_connection_pool_size();
                    if !pool_size_changed {
                        let save_result = {
                            let mut s = state
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            MainWindow::persist_settings(&mut s, settings, false)
                        };
                        MainWindow::apply_configured_ui_scale(state);
                        app::flush();
                        if let Err(err) = save_result {
                            crate::ui::alert_on_main(&format!("Failed to save settings: {}", err));
                        }
                        return true;
                    }

                    if !Self::resolve_pooled_sessions_before_pool_resize(state) {
                        return true;
                    }
                    let blocked = {
                        let s = state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        s.is_any_query_running() || s.has_active_lazy_fetches()
                    };
                    if blocked {
                        crate::ui::alert_on_main(
                            "Finish or cancel running queries and lazy fetches before changing connection pool size.",
                        );
                        return true;
                    }

                    {
                        let mut s = state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        s.status_bar.set_label("Rebuilding connection pool...");
                    }
                    let sender = conn_sender.clone();
                    let spawn_failure_sender = sender.clone();
                    let spawn_failure_settings = settings.clone();
                    let size = settings.connection_pool_size;
                    let policy =
                        ConnectionAttemptPolicy::from_seconds(settings.connect_timeout_seconds);
                    if let Err(err) = thread::Builder::new()
                        .name("space-query-pool-resize".to_string())
                        .spawn(move || {
                            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                                let mut failures = Vec::new();
                                for runtime in runtimes {
                                    if let Err(err) = resize_shared_connection_pool_with_policy(
                                        &runtime.connection(),
                                        size,
                                        policy,
                                    ) {
                                        failures.push(format!(
                                            "{}: {}",
                                            runtime.display_name(),
                                            err
                                        ));
                                    } else {
                                        runtime.refresh_state_from_connection();
                                    }
                                }
                                if failures.is_empty() {
                                    Ok(())
                                } else {
                                    Err(failures.join("\n"))
                                }
                            }))
                            .unwrap_or_else(|payload| {
                                Err(format!(
                                    "Connection pool resize worker terminated unexpectedly: {}",
                                    panic_payload_to_string(payload.as_ref())
                                ))
                            });
                            let _ = sender.send(ConnectionResult::PoolResize {
                                settings: Box::new(settings),
                                result,
                            });
                            app::awake();
                        })
                    {
                        let _ = spawn_failure_sender.send(ConnectionResult::PoolResize {
                            settings: Box::new(spawn_failure_settings),
                            result: Err(format!("Could not start pool resize worker: {err}")),
                        });
                        app::awake();
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

    fn zoom_shortcut_for_key(
        key: fltk::enums::Key,
        modifiers: fltk::enums::Shortcut,
    ) -> Option<UiScaleAction> {
        let ctrl_or_cmd = modifiers.contains(fltk::enums::Shortcut::Ctrl)
            || modifiers.contains(fltk::enums::Shortcut::Command);
        if !ctrl_or_cmd || modifiers.contains(fltk::enums::Shortcut::Alt) {
            return None;
        }

        match key {
            k if k == fltk::enums::Key::from_char('+') || k == fltk::enums::Key::from_char('=') => {
                Some(UiScaleAction::In)
            }
            k if k == fltk::enums::Key::from_char('-') => Some(UiScaleAction::Out),
            k if k == fltk::enums::Key::from_char('0') => Some(UiScaleAction::Reset),
            _ => None,
        }
    }

    fn resolve_window_zoom_shortcut(
        event_key: fltk::enums::Key,
        event_original_key: fltk::enums::Key,
        event_state: fltk::enums::Shortcut,
    ) -> Option<UiScaleAction> {
        Self::zoom_shortcut_for_key(event_key, event_state)
            .or_else(|| Self::zoom_shortcut_for_key(event_original_key, event_state))
    }

    #[cfg(target_os = "macos")]
    fn defer_ui_scale_until_fullscreen_exit(state: &Arc<Mutex<AppState>>, window: &Window) -> bool {
        if !window.shown() {
            return false;
        }

        let fullscreen_active = window.fullscreen_active();
        let native_fullscreen = crate::ui::macos_window_state::is_fullscreen(window.raw_handle());
        if !fullscreen_active && !native_fullscreen {
            return false;
        }

        if fullscreen_active {
            let mut window = window.clone();
            window.fullscreen(false);
        }
        Self::schedule_ui_scale_after_fullscreen_exit(
            Arc::downgrade(state),
            MACOS_FULLSCREEN_EXIT_POLL_RETRIES,
        );
        true
    }

    #[cfg(target_os = "macos")]
    fn schedule_ui_scale_after_fullscreen_exit(
        weak_state: std::sync::Weak<Mutex<AppState>>,
        retries_remaining: u8,
    ) {
        crate::ui::ui_timeout::schedule(MACOS_FULLSCREEN_EXIT_POLL_SECONDS, move || {
            let Some(state) = weak_state.upgrade() else {
                return;
            };
            let window = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .window
                .clone();
            if !window.shown() {
                return;
            }

            let fullscreen_active = window.fullscreen_active();
            let native_fullscreen =
                crate::ui::macos_window_state::is_fullscreen(window.raw_handle());
            if fullscreen_active || native_fullscreen {
                if retries_remaining == 0 {
                    crate::utils::logging::log_warning(
                        "ui_scale",
                        "Timed out waiting for the macOS fullscreen transition; scale change was not applied",
                    );
                    return;
                }
                Self::schedule_ui_scale_after_fullscreen_exit(
                    Arc::downgrade(&state),
                    retries_remaining - 1,
                );
                return;
            }

            Self::apply_configured_ui_scale(&state);
        });
    }

    fn set_screen_scale_preserving_window_frame(
        screen: i32,
        new_scale: f32,
        window: Option<&Window>,
        affects_all_screens: bool,
    ) -> bool {
        let old_scale = app::screen_scale(screen);
        if !old_scale.is_finite()
            || old_scale <= 0.0
            || !new_scale.is_finite()
            || new_scale <= 0.0
            || (new_scale - old_scale).abs() <= UI_SCALE_EPSILON
        {
            return false;
        }

        #[cfg(target_os = "macos")]
        if affects_all_screens {
            // FLTK resizes mapped NSWindows before returning. Keep the normal
            // AppKit frame exact while updating FLTK's matching geometry.
            if let Some(window) = window.filter(|window| window.shown()) {
                let raw_window = window.raw_handle();
                if window.fullscreen_active()
                    || crate::ui::macos_window_state::is_fullscreen(raw_window)
                {
                    return false;
                }

                if window.maximize_active() {
                    let mut window = window.clone();
                    window.un_maximize();
                }
                if crate::ui::macos_window_state::is_zoomed(raw_window)
                    && !crate::ui::macos_window_state::set_zoomed(raw_window, false)
                {
                    return false;
                }
                let Some(preserved_frame) =
                    crate::ui::macos_window_state::capture_frame(raw_window)
                else {
                    return false;
                };
                let geometry = (window.x(), window.y(), window.w(), window.h());

                app::set_screen_scale(screen, new_scale);

                let (x, y, width, height) = window_geometry_after_ui_scale(
                    geometry.0, geometry.1, geometry.2, geometry.3, old_scale, new_scale,
                );
                let mut window = window.clone();
                window.resize(x, y, width, height);
                let _ = crate::ui::macos_window_state::restore_frame(raw_window, preserved_frame);
                return true;
            }
        }

        let window_geometry = window.and_then(|window| {
            let window_screen = window.screen_num();
            if window.fullscreen_active()
                || window.maximize_active()
                || (!affects_all_screens && window_screen != screen)
            {
                None
            } else {
                Some((window.x(), window.y(), window.w(), window.h()))
            }
        });

        app::set_screen_scale(screen, new_scale);

        if let (Some(window), Some((x, y, width, height))) = (window, window_geometry) {
            let (scaled_x, scaled_y, scaled_width, scaled_height) =
                window_geometry_after_ui_scale(x, y, width, height, old_scale, new_scale);
            let mut window = window.clone();
            window.resize(scaled_x, scaled_y, scaled_width, scaled_height);
        }

        true
    }

    fn apply_ui_scale_percent(scale_bases: &[f32], percent: u32, window: Option<&Window>) -> bool {
        let screen_count = app::screen_count();
        if screen_count <= 0 || !app::Screen::scaling_supported() {
            return false;
        }

        let ratio = safe_div(AppConfig::clamp_ui_scale_percent(percent) as f32, 100.0);
        let separate_screen_scales = app::Screen::scaling_supported_separately();
        let mut changed = false;

        if separate_screen_scales {
            for (screen, base) in scale_bases
                .iter()
                .copied()
                .take(screen_count as usize)
                .enumerate()
            {
                if base.is_finite() && base > 0.0 {
                    changed |= Self::set_screen_scale_preserving_window_frame(
                        screen as i32,
                        base * ratio,
                        window,
                        false,
                    );
                }
            }
        } else if let Some(base) = scale_bases
            .first()
            .copied()
            .filter(|base| base.is_finite() && *base > 0.0)
        {
            changed = Self::set_screen_scale_preserving_window_frame(0, base * ratio, window, true);
        }

        if changed {
            app::redraw();
        }
        changed
    }

    fn schedule_tab_strip_refresh_after_scale(state: &Arc<Mutex<AppState>>) {
        let weak_state = Arc::downgrade(state);
        crate::ui::ui_timeout::schedule(0.0, move || {
            let Some(state) = weak_state.upgrade() else {
                return;
            };
            let (mut query_tabs, mut result_tabs) = {
                let state = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                (state.query_tabs.clone(), state.result_tabs.clone())
            };
            query_tabs.refresh_tab_strip_overflow_mode();
            result_tabs.refresh_tab_strip_overflow_mode();
        });
    }

    fn set_ui_scale_percent(state: &Arc<Mutex<AppState>>, percent: u32) -> bool {
        let (window, scale_bases, config) = {
            let state = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (
                state.window.clone(),
                state.ui_scale_bases.clone(),
                state.config.clone(),
            )
        };
        let percent = AppConfig::clamp_ui_scale_percent(percent);

        let mut config = config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if config.ui_scale_percent != percent {
            config.ui_scale_percent = percent;
            if let Err(err) = config.save() {
                crate::utils::logging::log_warning(
                    "ui_scale",
                    &format!("Failed to save screen scale setting: {err}"),
                );
            }
        }
        drop(config);

        #[cfg(target_os = "macos")]
        if Self::defer_ui_scale_until_fullscreen_exit(state, &window) {
            return true;
        }

        if Self::apply_ui_scale_percent(&scale_bases, percent, Some(&window)) {
            // Screen-scale changes can report their final logical window size
            // after the current event. Recheck once that geometry has settled;
            // normal resize callbacks still handle the synchronous path.
            Self::schedule_tab_strip_refresh_after_scale(state);
        }
        true
    }

    fn adjust_ui_scale(state: &Arc<Mutex<AppState>>, action: UiScaleAction) -> bool {
        let config = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .config
            .clone();
        let current_percent = config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .normalized_ui_scale_percent();
        Self::set_ui_scale_percent(state, next_ui_scale_percent(current_percent, action))
    }

    fn apply_configured_ui_scale(state: &Arc<Mutex<AppState>>) {
        let (window, scale_bases, config) = {
            let state = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (
                state.window.clone(),
                state.ui_scale_bases.clone(),
                state.config.clone(),
            )
        };
        let percent = config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .normalized_ui_scale_percent();
        #[cfg(target_os = "macos")]
        if Self::defer_ui_scale_until_fullscreen_exit(state, &window) {
            return;
        }
        if Self::apply_ui_scale_percent(&scale_bases, percent, Some(&window)) {
            Self::schedule_tab_strip_refresh_after_scale(state);
        }
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
                Some("File/Disconnect Active Connection")
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

    fn handle_window_zoom_shortcut(state: &Arc<Mutex<AppState>>) -> bool {
        let Some(action) = Self::resolve_window_zoom_shortcut(
            app::event_key(),
            app::event_original_key(),
            app::event_state(),
        ) else {
            return false;
        };
        Self::adjust_ui_scale(state, action)
    }

    fn handle_window_menu_shortcut(
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
            let mut state = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.schema_sender = Some(schema_sender.clone());
            state.file_sender = Some(file_sender.clone());
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

            let Some(binding) = s
                .editor_tabs
                .iter()
                .find(|tab| tab.tab_id == s.active_editor_tab_id)
                .map(|tab| tab.connection_binding.snapshot())
            else {
                return;
            };
            let Some(connection) = binding.connection() else {
                return;
            };
            let current_generation = match crate::db::try_lock_connection(&connection) {
                Some(conn_guard) => conn_guard.connection_generation(),
                None => {
                    let tab_id = s.active_editor_tab_id;
                    s.mark_metadata_refresh_pending(tab_id);
                    return;
                }
            };
            if snapshot.connection_generation != current_generation {
                return;
            }
            if !MainWindow::schema_update_scope_matches(
                snapshot.db_type,
                snapshot.selected_scope.as_deref(),
                binding.scope.as_deref(),
                &snapshot.available_scopes,
            ) {
                return;
            }

            MainWindow::apply_object_browser_metadata_snapshot(&mut s, snapshot);
        });

        let weak_state_for_browser = Arc::downgrade(&state);
        let schema_sender_for_browser = schema_sender.clone();
        let file_sender_for_browser = file_sender.clone();
        object_browser.set_sql_callback(move |connection_id, action| {
            let Some(state_for_browser) = weak_state_for_browser.upgrade() else {
                return;
            };
            let mut created_tabs = Vec::new();
            let mut sql_to_execute: Option<String> = None;
            let mut table_browse_to_execute: Option<(QueryTabId, TableBrowsePageRequest)> = None;
            {
                let mut s = state_for_browser
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let Some((source_tab_id, source_tab_created)) =
                    MainWindow::select_or_create_query_editor_tab_for_connection(
                        &mut s,
                        connection_id,
                    )
                else {
                    s.set_status_message("Object action ignored: source connection is unavailable");
                    return;
                };
                if source_tab_created {
                    created_tabs.push(source_tab_id);
                }
                match action {
                    SqlAction::Insert(text) => {
                        s.sql_editor.insert_text_at_cursor_position(&text);
                    }
                    SqlAction::OpenInNewTab(sql) => {
                        let target_tab_id = if source_tab_created {
                            Some(source_tab_id)
                        } else {
                            MainWindow::create_query_editor_tab(&mut s)
                        };
                        if let Some(tab_id) = target_tab_id {
                            s.sql_buffer.set_text(&sql);
                            s.sql_editor.reset_undo_redo_history();
                            s.set_tab_file_path(tab_id, None);
                            s.set_tab_pristine_text(tab_id, sql);
                            s.sql_editor.focus();
                            s.right_tile.redraw();
                            if !created_tabs.contains(&tab_id) {
                                created_tabs.push(tab_id);
                            }
                        }
                    }
                    SqlAction::Execute(sql) => {
                        sql_to_execute = Some(sql);
                    }
                    SqlAction::BrowseTable(target) => {
                        let Some(tab) =
                            s.editor_tabs.iter().find(|tab| tab.tab_id == source_tab_id)
                        else {
                            s.set_status_message(
                                "Table action ignored: owning query tab is unavailable",
                            );
                            return;
                        };
                        let intellisense_data = tab.intellisense_data.clone();
                        let editor = tab.sql_editor.clone();
                        let page_size =
                            result_page_unit_for_choice_index(s.result_page_unit_choice.value());
                        let execution_origin = tab.connection_binding.snapshot().execution_origin();
                        let mut result_tabs = tab.result_tabs.clone();
                        if editor.is_query_running() {
                            s.set_status_message(
                                "Table data was not opened because the owning query tab is busy",
                            );
                            return;
                        }
                        result_tabs.set_execution_origin(execution_origin);
                        let result_tab_id = result_tabs.reserve_result_tab_id();
                        if result_tabs
                            .ensure_table_browse_tab_by_id(
                                result_tab_id,
                                target.clone(),
                                intellisense_data,
                                page_size,
                                true,
                            )
                            .is_none()
                        {
                            s.set_status_message("Failed to create the table data tab");
                            return;
                        }
                        editor.prefetch_intellisense_table_columns(&target.completion_name);
                        if let Some(request) =
                            result_tabs.table_browse_initial_request(result_tab_id)
                        {
                            table_browse_to_execute = Some((source_tab_id, request));
                        }
                    }
                    SqlAction::DisplayResult(request) => {
                        s.append_result_tab_request(request);
                    }
                }
            }

            for tab_id in &created_tabs {
                MainWindow::attach_editor_callbacks(
                    &state_for_browser,
                    *tab_id,
                    schema_sender_for_browser.clone(),
                );
                MainWindow::attach_file_drop_callback(
                    &state_for_browser,
                    *tab_id,
                    file_sender_for_browser.clone(),
                );
            }

            if let Some(sql) = sql_to_execute {
                if let Some(editor) = acquire_sql_editor_if_idle(&state_for_browser) {
                    editor.execute_sql_text(&sql);
                }
            }

            if let Some((tab_id, request)) = table_browse_to_execute {
                if let Err(message) =
                    MainWindow::execute_table_browse_request(&state_for_browser, tab_id, request)
                {
                    crate::ui::alert_on_main(&message);
                }
            }

            if !created_tabs.is_empty() {
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
        object_browser.set_scope_change_callback(move |connection_id| {
            let Some(state_for_scope_change) = weak_state_for_scope_change.upgrade() else {
                return;
            };

            let retained_scope_update = {
                let mut s = state_for_scope_change
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let selected_scope = s
                    .object_browser
                    .selected_scope_for_connection(connection_id);
                s.synchronize_scope_for_connection(connection_id, selected_scope.clone());
                let retained_scope_update =
                    s.retained_scope_update_for_connection(connection_id, selected_scope);
                if s.active_connection_id() == Some(connection_id) {
                    let started = MainWindow::start_connection_metadata_refresh_for_scope_change(
                        &mut s,
                        &schema_sender_for_scope_change,
                    );
                    s.update_pending_metadata_refresh_after_start_attempt(started);
                }
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
        object_browser.set_scope_switch_preflight_callback(move |connection_id| {
            let Some(state_for_scope_preflight) = weak_state_for_scope_preflight.upgrade() else {
                return Ok(());
            };
            let s = state_for_scope_preflight
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(message) = s.retained_scope_change_blocker_for_connection(connection_id) {
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
                    AppState::hide_all_intellisense_popups_without_blocking(&state_for_window);
                    false
                }
                fltk::enums::Event::Deactivate => {
                    // A genuine deactivate (app switch) must hide the popups,
                    // but the completion popup window becoming macOS's key
                    // window can also deactivate the main window for a moment.
                    // Let focus settle before hiding either popup so that the
                    // completion window cannot close itself while being shown.
                    if let Ok(s) = state_for_window.try_lock() {
                        s.sql_editor.hide_intellisense_popup_after_focus_settles();
                        s.sql_editor.hide_signature_popup_after_focus_settles();
                        for tab in &s.editor_tabs {
                            tab.sql_editor.hide_intellisense_popup_after_focus_settles();
                            tab.sql_editor.hide_signature_popup_after_focus_settles();
                        }
                    }
                    false
                }
                fltk::enums::Event::KeyDown => {
                    if app::event_key() == fltk::enums::Key::Escape {
                        AppState::hide_all_intellisense_popups_without_blocking(&state_for_window);
                        return true;
                    }
                    if MainWindow::handle_window_menu_shortcut(
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
                    if MainWindow::handle_window_zoom_shortcut(&state_for_window)
                        || MainWindow::handle_window_menu_shortcut(
                            &state_for_window,
                            &schema_sender_for_window,
                            &conn_sender_for_window,
                            &file_sender_for_window,
                        )
                    {
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
                    if sql_editor
                        .editor_contains_root_point(app::event_x_root(), app::event_y_root())
                    {
                        sql_editor.hide_signature_popup();
                    } else {
                        sql_editor.dismiss_signature_popup();
                    }
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
                let current_target = match state.try_lock() {
                    Ok(s) => match s.active_schema_update_target() {
                        Ok(target) => target,
                        Err(()) => {
                            deferred_by_borrow_conflict = true;
                            None
                        }
                    },
                    Err(_) => {
                        deferred_by_borrow_conflict = true;
                        None
                    }
                };

                if !deferred_by_borrow_conflict {
                    let mut latest_update = pending_schema_update.take().filter(|update| {
                        current_target.as_ref().is_some_and(|target| {
                            MainWindow::schema_update_matches_target(update, target)
                        })
                    });
                    loop {
                        match r.try_recv() {
                            Ok(update) => {
                                processed_message = true;
                                if !current_target.as_ref().is_some_and(|target| {
                                    MainWindow::schema_update_matches_target(&update, target)
                                }) {
                                    continue;
                                }
                                latest_update = Some(update);
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
                            Ok(mut s) => match s.active_schema_update_target() {
                                Ok(Some(target))
                                    if MainWindow::schema_update_matches_target(
                                        &update, &target,
                                    ) =>
                                {
                                    MainWindow::update_schema_snapshot(
                                        &mut s,
                                        update.data,
                                        update.highlight_data,
                                    );
                                }
                                Ok(_) => {}
                                Err(()) => {
                                    pending_schema_update = Some(update);
                                    deferred_by_borrow_conflict = true;
                                }
                            },
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
                                ConnectionResult::Success {
                                    connection_id,
                                    info,
                                } => {
                                    let info = *info;
                                    let Some(runtime) = s.connection_registry.get(connection_id)
                                    else {
                                        continue;
                                    };
                                    runtime.update_sanitized_info(info.clone());
                                    runtime.set_state(ConnectionRuntimeState::Connected);
                                    s.object_browser.add_runtime(runtime.clone());
                                    s.object_browser.refresh_runtime_labels();
                                    s.synchronize_scope_for_connection(connection_id, None);
                                    s.clear_metadata_for_connection(connection_id);
                                    let matching_tab_ids = s
                                        .editor_tabs
                                        .iter()
                                        .filter(|tab| {
                                            tab.connection_binding.snapshot().connection_id()
                                                == Some(connection_id)
                                        })
                                        .map(|tab| tab.tab_id)
                                        .collect::<Vec<_>>();
                                    for tab_id in matching_tab_ids {
                                        s.mark_metadata_refresh_pending(tab_id);
                                        s.refresh_tab_label(tab_id);
                                    }
                                    crate::utils::logging::log_info(
                                        "connection",
                                        &format!("Connected to {} ({})", info.name, info.db_type),
                                    );
                                    let active_matches = s
                                        .editor_tabs
                                        .iter()
                                        .find(|tab| tab.tab_id == s.active_editor_tab_id)
                                        .and_then(|tab| {
                                            tab.connection_binding.snapshot().connection_id()
                                        })
                                        == Some(connection_id);
                                    if !active_matches {
                                        continue;
                                    }
                                    clear_mutex_flag(&s.schema_refresh_in_progress);
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
                                    for tab in s.editor_tabs.iter().filter(|tab| {
                                        tab.connection_binding.snapshot().connection_id()
                                            == Some(connection_id)
                                    }) {
                                        tab.sql_editor.set_db_type(info.db_type);
                                    }
                                    s.sql_editor.set_db_type(info.db_type);
                                    *s.connection_info
                                        .lock()
                                        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                        Some(info.clone());
                                    s.has_live_connection = true;
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
                                ConnectionResult::Failure {
                                    connection_id,
                                    message: err,
                                    preserve_existing_connection,
                                } => {
                                    let mut connection_preserved = false;
                                    if let Some(runtime) = s.connection_registry.get(connection_id)
                                    {
                                        let connection_is_still_live = preserve_existing_connection
                                            && crate::db::try_lock_connection(
                                                &runtime.connection(),
                                            )
                                            .is_some_and(|connection| {
                                                connection.is_connected()
                                                    && connection.has_connection_handle()
                                            });
                                        connection_preserved = connection_is_still_live;
                                        runtime.set_state(if connection_is_still_live {
                                            ConnectionRuntimeState::Connected
                                        } else {
                                            ConnectionRuntimeState::Failed(err.clone())
                                        });
                                        s.object_browser.add_runtime(runtime);
                                    }
                                    s.object_browser.refresh_runtime_labels();
                                    let matching_tab_ids = s
                                        .editor_tabs
                                        .iter()
                                        .filter(|tab| {
                                            tab.connection_binding.snapshot().connection_id()
                                                == Some(connection_id)
                                        })
                                        .map(|tab| tab.tab_id)
                                        .collect::<Vec<_>>();
                                    for tab_id in matching_tab_ids {
                                        s.refresh_tab_label(tab_id);
                                    }
                                    let active_matches = s
                                        .editor_tabs
                                        .iter()
                                        .find(|tab| tab.tab_id == s.active_editor_tab_id)
                                        .and_then(|tab| {
                                            tab.connection_binding.snapshot().connection_id()
                                        })
                                        == Some(connection_id);
                                    if !active_matches {
                                        continue;
                                    }
                                    if connection_preserved {
                                        crate::utils::logging::log_error(
                                            "connection",
                                            &format!(
                                                "Reconnect failed; the existing connection was preserved: {}",
                                                err
                                            ),
                                        );
                                        s.set_status_message(
                                            "Reconnect failed; existing connection preserved",
                                        );
                                        let lines = vec![
                                            format!("Reconnect failed: {}", err),
                                            "The existing connection remains available."
                                                .to_string(),
                                        ];
                                        s.result_tabs
                                            .append_message_lines(ResultMessageKind::Error, &lines);
                                    } else {
                                        crate::utils::logging::log_error(
                                            "connection",
                                            &format!("Connection or reconnect failed: {}", err),
                                        );
                                        s.status_bar.set_label(if preserve_existing_connection {
                                            "Reconnect failed; connection is offline"
                                        } else {
                                            "Connection failed"
                                        });
                                        s.result_tabs.append_message_lines(
                                            ResultMessageKind::Error,
                                            &[if preserve_existing_connection {
                                                format!(
                                                    "Reconnect failed and the previous connection is no longer live: {}",
                                                    err
                                                )
                                            } else {
                                                format!("Connection failed: {}", err)
                                            }],
                                        );
                                    }
                                    s.result_tabs.select_messages_errors();
                                }
                                ConnectionResult::PoolResize { settings, result } => match result {
                                    Ok(()) => {
                                        let save_result =
                                            MainWindow::persist_settings(&mut s, *settings, true);
                                        if let Err(err) = save_result {
                                            crate::utils::logging::log_error(
                                                "settings",
                                                &format!("Failed to save settings: {err}"),
                                            );
                                            s.status_bar.set_label("Failed to save settings");
                                            s.result_tabs.append_message_lines(
                                                ResultMessageKind::Error,
                                                &[format!("Failed to save settings: {err}")],
                                            );
                                            s.result_tabs.select_messages_errors();
                                        } else {
                                            s.status_bar
                                                .set_label("Connection pool preference updated");
                                        }
                                        let state_for_scale = Arc::clone(&state);
                                        crate::ui::ui_timeout::schedule(0.0, move || {
                                            MainWindow::apply_configured_ui_scale(&state_for_scale);
                                            app::flush();
                                        });
                                    }
                                    Err(err) => {
                                        crate::utils::logging::log_error(
                                            "connection pool",
                                            &format!("Failed to resize connection pool: {err}"),
                                        );
                                        s.status_bar.set_label("Connection pool update failed");
                                        s.result_tabs.append_message_lines(
                                            ResultMessageKind::Error,
                                            &[format!("Failed to resize connection pool: {err}")],
                                        );
                                        s.result_tabs.select_messages_errors();
                                    }
                                },
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
                                FileActionResult::OpenInNewTab {
                                    path,
                                    result,
                                    binding,
                                } => match result {
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
                                            MainWindow::create_query_editor_tab_for_binding(
                                                &mut s, binding, true,
                                            )
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
                                            Some(format!("Failed to export results: {}", err));
                                    }
                                },
                                FileActionResult::CopyToClipboard { result } => match result {
                                    Ok((sql, message)) => {
                                        MainWindow::finish_clipboard_copy(&mut s, &sql, &message);
                                    }
                                    Err(err) => {
                                        deferred_alert = Some(format!(
                                            "Failed to read the primary key for SQL Updates: {}",
                                            err
                                        ));
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
            Self::defer_application_exit_until_idle(state, window, Instant::now());
            return;
        }

        if !Self::resolve_pooled_sessions_before_exit(&state) {
            return;
        }

        Self::finish_application_exit(&state, window);
    }

    fn defer_application_exit_until_idle(
        state: Arc<Mutex<AppState>>,
        window: Window,
        started_at: Instant,
    ) {
        crate::ui::ui_timeout::schedule(APPLICATION_EXIT_POLL_SECONDS, move || {
            let has_running_work = {
                let s = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                s.has_running_query_or_lazy_fetch()
            };
            match application_exit_wait_decision(has_running_work, started_at.elapsed()) {
                ApplicationExitWaitDecision::Continue => {
                    Self::continue_application_exit(state.clone(), window.clone(), false);
                }
                ApplicationExitWaitDecision::Retry => {
                    Self::defer_application_exit_until_idle(
                        state.clone(),
                        window.clone(),
                        started_at,
                    );
                }
                ApplicationExitWaitDecision::Force => {
                    crate::utils::logging::log_warning(
                        "app",
                        "Forcing application exit after query cancellation did not become idle",
                    );
                    Self::finish_application_exit(&state, window.clone());
                }
            }
        });
    }

    fn hide_all_visible_windows() {
        for _ in 0..MAX_TOP_LEVEL_WINDOWS_TO_HIDE {
            let Some(mut visible_window) = app::first_window() else {
                return;
            };
            visible_window.hide();
        }
        crate::utils::logging::log_error(
            "app",
            "Failed to hide all top-level windows during application exit",
        );
    }

    fn finish_application_exit(state: &Arc<Mutex<AppState>>, mut window: Window) {
        // FLTK's event loop returns only after every native top-level window is
        // hidden. Establish that exit condition before any resource cleanup
        // that could be delayed by a database driver or worker state.
        window.hide();
        Self::hide_all_visible_windows();

        crate::db::clear_tracked_db_activity();
        let (popups, editor_tabs, mut result_tabs, runtimes) = {
            let s = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (
                s.popups.clone(),
                s.editor_tabs.clone(),
                s.result_tabs.clone(),
                s.connection_registry.runtimes(),
            )
        };
        {
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
        }
        for mut tab in editor_tabs {
            tab.sql_editor.cleanup_for_close();
        }
        for runtime in runtimes {
            let connection = runtime.connection();
            crate::db::clear_pool_session_context_for_shared_connection(&connection);
            if let Some(mut db_conn) = crate::db::try_lock_connection(&connection) {
                db_conn.disconnect();
                db_conn.refresh_tracked_connection();
                runtime.set_state(ConnectionRuntimeState::Disconnected);
            } else {
                crate::utils::logging::log_warning(
                    "app",
                    &format!(
                        "Could not synchronously close connection '{}' during exit",
                        runtime.display_name()
                    ),
                );
            };
        }
        result_tabs.clear();
        crate::ui::sql_editor::SqlEditorWidget::shutdown_column_load_workers();
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
                w.hide();
                MainWindow::hide_all_visible_windows();
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
            if !s.editor_tabs.is_empty() {
                s.sql_editor.focus();
            }
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
        if state.editor_tabs.is_empty() {
            let _ = Self::create_query_editor_tab(&mut state);
        }
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
        let oracle_runtime = state
            .connection_registry
            .register_saved("capture-local-oracle", create_shared_connection())
            .runtime;
        oracle_runtime.update_sanitized_info(crate::db::ConnectionInfo::new_with_type(
            "Local Oracle",
            "",
            "",
            "",
            1521,
            "",
            DatabaseType::Oracle,
        ));
        oracle_runtime.set_state(ConnectionRuntimeState::Connected);
        let maria_runtime = state
            .connection_registry
            .register_saved("capture-analytics-maria", create_shared_connection())
            .runtime;
        maria_runtime.update_sanitized_info(crate::db::ConnectionInfo::new_with_type(
            "Analytics MariaDB",
            "",
            "",
            "",
            3306,
            "",
            DatabaseType::MariaDB,
        ));
        maria_runtime.set_state(ConnectionRuntimeState::Connected);
        state.object_browser.add_runtime(oracle_runtime.clone());
        state.object_browser.add_runtime(maria_runtime.clone());
        let oracle_tab_id = if state.editor_tabs.is_empty() {
            let Some(tab_id) =
                Self::create_query_editor_tab_for_runtime(&mut state, oracle_runtime.clone())
            else {
                return;
            };
            tab_id
        } else {
            let tab_id = state.active_editor_tab_id;
            if let Some(binding) = state
                .editor_tabs
                .iter()
                .find(|tab| tab.tab_id == tab_id)
                .map(|tab| tab.connection_binding.clone())
            {
                binding.bind(oracle_runtime.clone(), Some("SYSTEM".to_string()));
            }
            tab_id
        };
        let _ = Self::create_query_editor_tab_for_runtime(&mut state, maria_runtime.clone());
        state.query_tabs.select(oracle_tab_id);
        let _ = state.set_active_editor_tab(oracle_tab_id);
        state.connection = oracle_runtime.connection();
        state
            .object_browser
            .set_active_connection(Some(oracle_runtime.id()));
        state.object_browser.capture_tour_set_example_metadata();
        *state
            .connection_info
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(crate::db::ConnectionInfo::new_with_type(
                "Local Oracle",
                "",
                "",
                "",
                1521,
                "",
                DatabaseType::Oracle,
            ));
        state.has_live_connection = true;
        state.refresh_tab_label(oracle_tab_id);
        state.render_status_bar();
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
        let execution_origin = state
            .editor_tabs
            .iter()
            .find(|tab| tab.tab_id == state.active_editor_tab_id)
            .and_then(|tab| tab.connection_binding.snapshot().execution_origin());
        state.result_tabs.set_execution_origin(execution_origin);
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

    /// Show the Data Grid popup menu over the current selection.
    ///
    /// Blocks in FLTK's popup loop until the menu is dismissed; a capture has to
    /// take its frame and hide the menu window from a timeout.
    #[doc(hidden)]
    pub fn capture_tour_show_result_context_menu(&mut self) -> Result<(), String> {
        let result_tabs = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.result_tabs.clone()
        };
        result_tabs.capture_tour_show_context_menu()
    }

    /// Whether the visible result grid holds rows an export could cover.
    #[doc(hidden)]
    pub fn capture_tour_result_has_data(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .result_tabs
            .has_data()
    }

    /// Select a cell range in the visible result grid, as a drag would.
    #[doc(hidden)]
    pub fn capture_tour_select_result_range(
        &mut self,
        row_start: i32,
        col_start: i32,
        row_end: i32,
        col_end: i32,
    ) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .result_tabs
            .capture_tour_select_range(row_start, col_start, row_end, col_end);
    }

    /// Drop the visible result grid's selection, as a click outside it would.
    #[doc(hidden)]
    pub fn capture_tour_clear_result_selection(&mut self) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.result_tabs.capture_tour_clear_selection();
    }

    /// Open the export modal exactly as `Ctrl+E` does, with every format on
    /// offer and the selection scope enabled.
    ///
    /// Blocks in the modal's own event loop until it is hidden, so a capture has
    /// to take its frame and hide it from a timeout.
    #[doc(hidden)]
    pub fn capture_tour_show_export_dialog(&mut self) {
        let _ = crate::ui::result_export_dialog::show(&ExportFormat::ALL, true);
    }

    /// What the three Data Grid SQL export items put on the clipboard for the
    /// current selection, in menu order: inserts, updates, where clause.
    ///
    /// Same snapshot and builders `copy_result_selection_as_sql` uses; only the
    /// clipboard write and the primary-key lookup, which needs a server, are
    /// left to the caller.
    #[doc(hidden)]
    pub fn capture_tour_grid_sql_export(
        &mut self,
        db_type: crate::db::DatabaseType,
        primary_key: &[String],
    ) -> Result<(String, String, String), String> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let selection = state
            .result_tabs
            .sql_export_context(db_type)
            .ok_or_else(|| "the result grid has no exportable selection".to_string())?;
        Ok((
            crate::ui::grid_sql_export::build_sql_inserts(&selection),
            crate::ui::grid_sql_export::build_sql_updates(&selection, primary_key),
            crate::ui::grid_sql_export::build_where_clause(&selection),
        ))
    }

    #[doc(hidden)]
    pub fn capture_tour_show_table_browse_popup(
        &mut self,
        result: crate::db::QueryResult,
    ) -> Result<(fltk::input::Input, Arc<Mutex<crate::ui::IntellisensePopup>>), String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.editor_tabs.is_empty() {
            let _ = Self::create_query_editor_tab(&mut state);
        }
        let intellisense_data = state.schema_intellisense_data.clone();
        let mut result_tabs = state.result_tabs.clone();
        result_tabs.clear();
        let result_tab_id = result_tabs.reserve_result_tab_id();
        let target = TableBrowseTarget::new(
            DatabaseType::Oracle,
            Some("SCOTT".to_string()),
            "EMP".to_string(),
            "SCOTT.EMP".to_string(),
            "SCOTT.EMP".to_string(),
        );
        let page_size = result_page_unit_for_choice_index(state.result_page_unit_choice.value());
        result_tabs
            .ensure_table_browse_tab_by_id(
                result_tab_id,
                target,
                intellisense_data,
                page_size,
                true,
            )
            .ok_or_else(|| "Could not create the table browse capture tab.".to_string())?;
        result_tabs.display_result_by_id(result_tab_id, &result);
        state.refresh_result_edit_controls();
        let popup = result_tabs
            .capture_tour_show_table_browse_popup()
            .ok_or_else(|| "Could not show the table browse completion popup.".to_string())?;
        state.window.redraw();
        Ok(popup)
    }

    #[doc(hidden)]
    pub fn capture_tour_show_table_browse_order_popup(
        &mut self,
    ) -> Result<(fltk::input::Input, Arc<Mutex<crate::ui::IntellisensePopup>>), String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut result_tabs = state.result_tabs.clone();
        let popup = result_tabs
            .capture_tour_show_table_browse_order_popup()
            .ok_or_else(|| "Could not show the ORDER BY completion popup.".to_string())?;
        state.window.redraw();
        Ok(popup)
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
        if let Err(err) = crate::utils::logging::flush_log_writer() {
            eprintln!("Application log flush on exit failed: {err}");
        }
        if let Err(err) = crate::ui::query_history::flush_history_writer() {
            eprintln!("Query history flush on exit failed: {err}");
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
    use crate::db::session_policy::{LazyFetchState, SqlKind};
    use crate::ui::result_table::LazyFetchCallback;
    use crate::ui::sql_editor::LazyFetchRequest;
    use fltk::enums::{Key, Shortcut};
    use std::sync::{Arc, Mutex};

    fn cancel_target_snapshot_for_test(
        tab_id: QueryTabId,
        editor_id: u64,
        operation_id: u64,
        execution_state: ExecutionState,
    ) -> CancelTargetSnapshot {
        let lazy_state = if execution_state == ExecutionState::LazyFetchOnly {
            LazyFetchState::Waiting
        } else {
            LazyFetchState::None
        };
        CancelTargetSnapshot {
            tab_id,
            editor_id,
            operation_id,
            connection_generation: 1,
            db_type: DatabaseType::Oracle,
            sql_kind: SqlKind::SelectLike,
            execution_state,
            lazy_state,
            autocommit: true,
            activity_label: "Executing SQL".to_string(),
        }
    }

    #[test]
    fn status_connection_label_uses_exact_disconnected_text_and_connected_identity() {
        let info = crate::db::ConnectionInfo::new_with_type(
            "Local",
            "",
            "",
            "",
            1521,
            "",
            DatabaseType::Oracle,
        );

        assert_eq!(status_connection_label(None, false), "not connected");
        assert_eq!(status_connection_label(Some(&info), true), "Local (Oracle)");
    }

    #[test]
    fn batch_end_only_recovers_unfinished_materialized_page_queries() {
        assert!(should_fail_table_browse_at_batch_end(true, false));
        assert!(!should_fail_table_browse_at_batch_end(false, false));
        assert!(!should_fail_table_browse_at_batch_end(true, true));
    }

    #[test]
    fn batch_end_keeps_table_browse_state_registered_by_a_newer_request() {
        let finished = ResultTabId::new(1);
        let newer = ResultTabId::new(2);

        assert!(batch_owns_grid_target(Some(finished), Some(finished)));
        assert!(batch_owns_grid_target(None, None));
        // A browse started in the window between `query_running = false` and
        // this BatchFinished owns the registration now.
        assert!(!batch_owns_grid_target(Some(finished), Some(newer)));
        assert!(!batch_owns_grid_target(None, Some(newer)));
        assert!(!batch_owns_grid_target(Some(finished), None));
    }

    #[test]
    fn a_table_page_statement_is_not_offered_a_filter_bar_of_its_own() {
        let page_sql = crate::ui::table_browse::build_page_sql(&TableBrowsePageRequest {
            result_tab_id: ResultTabId::new(1),
            target: TableBrowseTarget::new(
                DatabaseType::MySQL,
                Some("APP".to_string()),
                "Result".to_string(),
                "(SELECT * FROM `APP`.`EMP`) sq_src".to_string(),
                String::new(),
            ),
            clauses: crate::ui::table_browse::TableBrowseClauses::new(
                "DEPTNO = 20".to_string(),
                "EMPNO".to_string(),
            ),
            offset: 0,
            page_size: 100,
            navigation: TableBrowseNavigation::Page,
        })
        .unwrap();

        assert!(!result_can_carry_a_filter_bar(&page_sql));
        assert!(result_can_carry_a_filter_bar("SELECT * FROM emp"));
    }

    #[test]
    fn status_connection_color_uses_dark_theme_semantic_colors() {
        assert_eq!(
            status_connection_color(false).to_rgb(),
            theme::status_disconnected().to_rgb()
        );
        assert_eq!(
            status_connection_color(true).to_rgb(),
            theme::status_connected().to_rgb()
        );
    }

    #[test]
    fn status_bar_content_places_connection_and_activity_in_status_bar() {
        assert_eq!(
            status_bar_content_label("Local (Oracle)", Some("Running | Executing SQL: SELECT 1")),
            "Local (Oracle) | Running | Executing SQL: SELECT 1"
        );
        assert_eq!(
            status_bar_content_label("not connected", None),
            "not connected"
        );
    }

    #[test]
    fn status_activity_color_pulse_transitions_between_default_and_active_color() {
        let one_way_frames = 100 / STATUS_ANIMATION_STEP;

        assert_eq!(
            activity_pulse_color(0, theme::status_bar_default(), theme::accent()).to_rgb(),
            theme::status_bar_default().to_rgb()
        );
        assert_eq!(
            activity_pulse_color(
                one_way_frames * STATUS_ANIMATION_STEP,
                theme::status_bar_default(),
                theme::accent()
            )
            .to_rgb(),
            theme::accent().to_rgb()
        );
        assert_eq!(
            activity_pulse_color(
                one_way_frames * STATUS_ANIMATION_STEP * 2,
                theme::status_bar_default(),
                theme::accent()
            )
            .to_rgb(),
            theme::status_bar_default().to_rgb()
        );
        assert!((STATUS_ANIMATION_INTERVAL * one_way_frames as f64 - 2.5).abs() < 0.001);

        assert_eq!(
            activity_pulse_color(
                one_way_frames * STATUS_ANIMATION_STEP,
                theme::button_cancel(),
                theme::button_cancel_active()
            )
            .to_rgb(),
            theme::button_cancel_active().to_rgb()
        );
        assert_eq!(
            query_cancel_activity_color(one_way_frames * STATUS_ANIMATION_STEP, true, true)
                .to_rgb(),
            theme::hover_feedback_color(theme::button_cancel_active()).to_rgb()
        );
        assert_eq!(
            query_cancel_activity_color(0, false, true).to_rgb(),
            theme::hover_feedback_color(theme::button_cancel()).to_rgb()
        );
    }

    #[test]
    fn new_editor_metadata_uses_object_browser_cache_without_an_existing_tab() {
        let snapshot = ObjectBrowserMetadataSnapshot {
            db_type: DatabaseType::Oracle,
            connection_generation: 3,
            available_scopes: vec!["SCOTT".to_string()],
            selected_scope: Some("SCOTT".to_string()),
            tables: vec!["EMP".to_string()],
            views: vec!["EMP_VIEW".to_string()],
            procedures: Vec::new(),
            functions: Vec::new(),
            sequences: Vec::new(),
            triggers: Vec::new(),
            events: Vec::new(),
            synonyms: Vec::new(),
            packages: Vec::new(),
        };

        let (data, highlight_data) = MainWindow::editor_metadata_seed(None, Some(&snapshot));

        assert_eq!(data.default_qualifier(), Some("SCOTT"));
        assert!(data.tables.contains(&"EMP".to_string()));
        assert!(data.views.contains(&"EMP_VIEW".to_string()));
        assert!(highlight_data.tables.contains(&"EMP".to_string()));
        assert!(highlight_data.views.contains(&"EMP_VIEW".to_string()));
    }

    #[test]
    fn status_activity_selects_most_recent_start_across_connections_and_falls_back() {
        let registry = ConnectionRegistry::new();
        let first_connection_id = registry.register_unmanaged(create_shared_connection()).id();
        let second_connection_id = registry.register_unmanaged(create_shared_connection()).id();
        let first_start = Instant::now();
        let second_start = first_start + Duration::from_millis(1);
        let activities = vec![
            crate::db::DbActivitySnapshot {
                id: 10,
                activity: "first".to_string(),
                started_at: first_start,
                db_type: Some(DatabaseType::Oracle),
                connection_id: Some(first_connection_id),
                progress: crate::db::DbActivityProgress::Indeterminate,
            },
            crate::db::DbActivitySnapshot {
                id: 11,
                activity: "second".to_string(),
                started_at: second_start,
                db_type: Some(DatabaseType::Oracle),
                connection_id: Some(second_connection_id),
                progress: crate::db::DbActivityProgress::Indeterminate,
            },
        ];

        assert_eq!(
            latest_status_activity(&activities).map(|activity| activity.activity.as_str()),
            Some("second")
        );
        assert_eq!(
            latest_status_activity(&activities[..1]).map(|activity| activity.activity.as_str()),
            Some("first")
        );
        assert!(latest_status_activity(&[]).is_none());
    }

    #[test]
    fn latest_query_cancel_target_uses_highest_operation_id() {
        let snapshots = vec![
            cancel_target_snapshot_for_test(10, 1, 4, ExecutionState::RunningStatement),
            cancel_target_snapshot_for_test(20, 2, 9, ExecutionState::RunningScript),
            cancel_target_snapshot_for_test(30, 3, 7, ExecutionState::RunningStatement),
        ];

        assert_eq!(latest_query_cancel_tab_id(snapshots), Some(20));
    }

    #[test]
    fn latest_query_cancel_target_ranks_lazy_fetch_with_other_active_work() {
        let snapshots = vec![
            cancel_target_snapshot_for_test(10, 1, 12, ExecutionState::RunningStatement),
            cancel_target_snapshot_for_test(20, 2, 14, ExecutionState::LazyFetchOnly),
        ];

        assert_eq!(latest_query_cancel_tab_id(snapshots), Some(20));
    }

    #[test]
    fn latest_query_cancel_target_prefers_new_explain_over_previous_lazy_fetch() {
        let mut lazy = cancel_target_snapshot_for_test(10, 1, 41, ExecutionState::LazyFetchOnly);
        lazy.activity_label = "Fetching rows".to_string();
        let mut explain =
            cancel_target_snapshot_for_test(10, 1, 42, ExecutionState::RunningStatement);
        explain.activity_label = "Generating explain plan".to_string();

        let target = latest_query_cancel_target(vec![lazy, explain]).expect("cancel target");

        assert_eq!(target.operation_id, 42);
        assert_eq!(target.activity_label, "Generating explain plan");
    }

    #[test]
    fn status_activity_prefers_newer_background_work_over_cancel_target_query() {
        let now = Instant::now();
        let activities = vec![
            crate::db::DbActivitySnapshot {
                id: 42,
                activity: "Executing SQL".to_string(),
                started_at: now,
                db_type: Some(DatabaseType::Oracle),
                connection_id: None,
                progress: crate::db::DbActivityProgress::Indeterminate,
            },
            crate::db::DbActivitySnapshot {
                id: 43,
                activity: "Refreshing metadata".to_string(),
                started_at: now + Duration::from_millis(1),
                db_type: Some(DatabaseType::Oracle),
                connection_id: None,
                progress: crate::db::DbActivityProgress::Indeterminate,
            },
        ];

        assert_eq!(
            latest_status_activity(&activities).map(|item| item.activity.as_str()),
            Some("Refreshing metadata")
        );
    }

    #[test]
    fn latest_query_cancel_target_ignores_inactive_work() {
        let snapshots = vec![
            cancel_target_snapshot_for_test(10, 1, 100, ExecutionState::Finished),
            cancel_target_snapshot_for_test(20, 2, 99, ExecutionState::Idle),
            cancel_target_snapshot_for_test(30, 3, 7, ExecutionState::CancelRequested),
        ];

        assert_eq!(latest_query_cancel_tab_id(snapshots), Some(30));
        assert_eq!(
            latest_query_cancel_tab_id(vec![
                cancel_target_snapshot_for_test(10, 1, 2, ExecutionState::Idle),
                cancel_target_snapshot_for_test(20, 2, 3, ExecutionState::Finished),
            ]),
            None
        );
    }

    #[test]
    fn cancel_outcome_is_operation_scoped_and_only_dispatched_after_interrupt() {
        assert_eq!(
            query_cancel_phase_after_outcome(
                Some(QueryCancelPhase::Requested),
                &QueryCancelOutcome::PendingInitialization,
            ),
            Some(QueryCancelPhase::Requested)
        );
        assert_eq!(
            query_cancel_phase_after_outcome(
                Some(QueryCancelPhase::Requested),
                &QueryCancelOutcome::InterruptSent,
            ),
            Some(QueryCancelPhase::Dispatched)
        );
        assert_eq!(
            query_cancel_phase_after_outcome(
                Some(QueryCancelPhase::Requested),
                &QueryCancelOutcome::ForceStarted,
            ),
            Some(QueryCancelPhase::Dispatched)
        );
        assert_eq!(
            query_cancel_phase_after_outcome(
                Some(QueryCancelPhase::Dispatched),
                &QueryCancelOutcome::ForceCompleted,
            ),
            Some(QueryCancelPhase::Dispatched)
        );
        assert_eq!(
            query_cancel_phase_after_outcome(
                Some(QueryCancelPhase::Requested),
                &QueryCancelOutcome::InterruptFailed("interrupt failed".to_string()),
            ),
            Some(QueryCancelPhase::Requested)
        );
        assert_eq!(
            query_cancel_phase_after_outcome(
                Some(QueryCancelPhase::Requested),
                &QueryCancelOutcome::AlreadyFinished,
            ),
            None
        );
        assert_eq!(
            query_cancel_phase_after_outcome(
                Some(QueryCancelPhase::Dispatched),
                &QueryCancelOutcome::ForceFailed("spawn failed".to_string()),
            ),
            None
        );
    }

    #[test]
    fn cancelling_new_explain_does_not_mark_previous_lazy_context_canceling() {
        let lazy_snapshot =
            cancel_target_snapshot_for_test(10, 1, 41, ExecutionState::LazyFetchOnly);
        let explain_snapshot =
            cancel_target_snapshot_for_test(10, 1, 42, ExecutionState::RunningStatement);
        let lazy_token = QueryOperationToken::from_cancel_snapshot(&lazy_snapshot);
        let explain_token = QueryOperationToken::from_cancel_snapshot(&explain_snapshot);
        let context =
            QueryProgressContext::new(None, "Executing SQL".to_string(), Some(lazy_token));

        assert!(!progress_context_matches_cancel_token(
            &context,
            explain_token
        ));
        assert!(progress_context_matches_cancel_token(&context, lazy_token));
    }

    #[test]
    fn close_cancel_drain_advances_from_auxiliary_operation_to_lazy_fetch() {
        let lazy = cancel_target_snapshot_for_test(10, 1, 41, ExecutionState::LazyFetchOnly);
        let explain = cancel_target_snapshot_for_test(10, 1, 42, ExecutionState::RunningStatement);
        let explain_token = QueryOperationToken::from_cancel_snapshot(&explain);
        let mut pending_queries = HashMap::from([(explain_token, QueryCancelPhase::Requested)]);
        let pending_lazy_fetches = HashSet::new();

        assert!(cancel_target_is_pending(
            &explain,
            &pending_queries,
            &pending_lazy_fetches,
        ));
        pending_queries.remove(&explain_token);
        assert!(!cancel_target_is_pending(
            &lazy,
            &pending_queries,
            &pending_lazy_fetches,
        ));
        assert_eq!(
            latest_query_cancel_target(vec![lazy]).map(|target| target.operation_id),
            Some(41)
        );
    }

    #[test]
    fn queued_initial_lazy_fetch_session_survives_newer_completed_operation() {
        let snapshot = cancel_target_snapshot_for_test(10, 1, 41, ExecutionState::RunningStatement);
        let token = QueryOperationToken::from_cancel_snapshot(&snapshot);
        let mut context = QueryProgressContext::new(None, "Executing SQL".to_string(), Some(token));
        context.active_statement_index = Some(0);

        assert!(unregistered_lazy_fetch_session_matches_context(
            &context, token, 44, 44, 1,
        ));
        assert!(should_accept_lazy_fetch_session_event(
            false,
            None,
            Some(&context),
            0,
        ));
    }

    #[test]
    fn late_execution_finished_matches_retained_lazy_context() {
        let snapshot = cancel_target_snapshot_for_test(10, 1, 41, ExecutionState::RunningStatement);
        let token = QueryOperationToken::from_cancel_snapshot(&snapshot);
        let context = QueryProgressContext::new(None, "Executing SQL".to_string(), Some(token));
        let mut event =
            crate::db::session_policy::ExecutionFinishedEvent::new(DatabaseType::Oracle);
        event.tab_id = 10;
        event.editor_id = 1;
        event.operation_id = 41;
        event.connection_generation = 1;

        assert!(execution_finished_event_matches_retained_context(
            &event,
            10,
            Some(1),
            Some(1),
            Some(&context),
        ));
        assert!(!execution_finished_event_matches_current_editor(
            &event,
            10,
            Some(1),
            0,
            42,
            Some(1),
        ));
    }

    #[test]
    fn abandoned_operation_retention_is_bounded_by_global_operation_age() {
        let mut operations = (1..=2_048)
            .map(|operation_id| QueryOperationToken {
                tab_id: 1,
                editor_id: 1,
                operation_id,
                connection_generation: 1,
            })
            .collect::<HashSet<_>>();

        prune_abandoned_query_operations(&mut operations);

        assert_eq!(operations.len(), MAX_ABANDONED_QUERY_OPERATION_AGE as usize);
        assert_eq!(
            operations.iter().map(|token| token.operation_id).min(),
            Some(1_025)
        );
    }

    #[test]
    fn orphaned_lazy_fetch_requires_a_grace_period() {
        let missing_since = Instant::now();

        assert!(!orphaned_lazy_fetch_grace_expired(
            missing_since,
            missing_since + ORPHANED_LAZY_FETCH_GRACE_PERIOD - Duration::from_millis(1),
        ));
        assert!(orphaned_lazy_fetch_grace_expired(
            missing_since,
            missing_since + ORPHANED_LAZY_FETCH_GRACE_PERIOD,
        ));
    }

    #[test]
    fn application_exit_wait_continues_as_soon_as_work_is_idle() {
        assert_eq!(
            application_exit_wait_decision(false, Duration::ZERO),
            ApplicationExitWaitDecision::Continue
        );
    }

    #[test]
    fn application_exit_wait_retries_only_within_cancel_grace() {
        assert_eq!(
            application_exit_wait_decision(
                true,
                APPLICATION_EXIT_CANCEL_GRACE - Duration::from_millis(1),
            ),
            ApplicationExitWaitDecision::Retry
        );
    }

    #[test]
    fn application_exit_wait_forces_shutdown_at_cancel_deadline() {
        assert_eq!(
            application_exit_wait_decision(true, APPLICATION_EXIT_CANCEL_GRACE),
            ApplicationExitWaitDecision::Force
        );
    }

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
    fn resolve_window_zoom_shortcut_accepts_ctrl_and_command() {
        assert_eq!(
            MainWindow::resolve_window_zoom_shortcut(
                Key::from_char('+'),
                Key::from_char('+'),
                Shortcut::Ctrl,
            ),
            Some(UiScaleAction::In)
        );
        assert_eq!(
            MainWindow::resolve_window_zoom_shortcut(
                Key::from_char('-'),
                Key::from_char('-'),
                Shortcut::Command,
            ),
            Some(UiScaleAction::Out)
        );
    }

    #[test]
    fn resolve_window_zoom_shortcut_accepts_unshifted_plus_key() {
        assert_eq!(
            MainWindow::resolve_window_zoom_shortcut(
                Key::from_char('='),
                Key::from_char('='),
                Shortcut::Ctrl,
            ),
            Some(UiScaleAction::In)
        );
    }

    #[test]
    fn resolve_window_zoom_shortcut_accepts_reset_key() {
        assert_eq!(
            MainWindow::resolve_window_zoom_shortcut(
                Key::from_char('0'),
                Key::from_char('0'),
                Shortcut::Ctrl,
            ),
            Some(UiScaleAction::Reset)
        );
    }

    #[test]
    fn resolve_window_zoom_shortcut_rejects_unmodified_or_alt_keys() {
        assert_eq!(
            MainWindow::resolve_window_zoom_shortcut(
                Key::from_char('+'),
                Key::from_char('+'),
                Shortcut::None,
            ),
            None
        );
        assert_eq!(
            MainWindow::resolve_window_zoom_shortcut(
                Key::from_char('-'),
                Key::from_char('-'),
                Shortcut::Ctrl | Shortcut::Alt,
            ),
            None
        );
    }

    #[test]
    fn ui_scale_actions_use_configured_percent_and_clamp_at_bounds() {
        assert_eq!(next_ui_scale_percent(125, UiScaleAction::In), 135);
        assert_eq!(next_ui_scale_percent(135, UiScaleAction::Out), 125);
        assert_eq!(next_ui_scale_percent(300, UiScaleAction::In), 300);
        assert_eq!(next_ui_scale_percent(50, UiScaleAction::Out), 50);
        assert_eq!(next_ui_scale_percent(175, UiScaleAction::Reset), 100);
    }

    #[test]
    fn window_geometry_uses_old_to_new_scale_ratio() {
        assert_eq!(
            window_geometry_after_ui_scale(300, 200, 800, 600, 1.0, 2.0),
            (150, 100, 400, 300)
        );
    }

    #[test]
    fn window_geometry_preserves_oversized_or_offscreen_physical_frame() {
        assert_eq!(
            window_geometry_after_ui_scale(1700, -200, 2400, 1600, 1.0, 2.0),
            (850, -100, 1200, 800)
        );
    }

    #[test]
    fn window_geometry_scale_round_trip_restores_original_frame() {
        let (x, y, width, height) = window_geometry_after_ui_scale(300, 200, 800, 600, 1.0, 2.0);
        assert_eq!(
            window_geometry_after_ui_scale(x, y, width, height, 2.0, 1.0),
            (300, 200, 800, 600)
        );
    }

    #[test]
    fn fractional_window_geometry_requires_native_frame_restoration() {
        let (_, _, width, _) = window_geometry_after_ui_scale(0, 0, 104, 100, 1.0, 1.1);

        assert_eq!(width, 95);
        assert_eq!((f64::from(width) * 1.1).round() as i32, 105);
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
    fn result_page_unit_choice_supports_only_the_requested_units() {
        assert_eq!(
            (0..RESULT_PAGE_UNITS.len() as i32)
                .map(result_page_unit_for_choice_index)
                .collect::<Vec<_>>(),
            RESULT_PAGE_UNITS
        );
        assert_eq!(
            result_page_unit_for_choice_index(-1),
            RESULT_PAGE_UNITS[RESULT_PAGE_DEFAULT_UNIT_INDEX]
        );
        assert_eq!(
            result_page_unit_for_choice_index(99),
            RESULT_PAGE_UNITS[RESULT_PAGE_DEFAULT_UNIT_INDEX]
        );
        assert_eq!(
            RESULT_PAGE_UNITS[RESULT_PAGE_DEFAULT_UNIT_INDEX], 500,
            "the page unit must default to 500 rows"
        );
        for (index, unit) in RESULT_PAGE_UNITS.into_iter().enumerate() {
            assert_eq!(
                result_page_choice_index_for_unit(unit),
                i32::try_from(index).ok()
            );
        }
        assert_eq!(result_page_choice_index_for_unit(42), None);
    }

    #[test]
    fn result_page_control_centers_fixed_width_children() {
        assert_eq!(result_page_control_center_offsets(100), (0, 0));
        assert_eq!(
            result_page_control_center_offsets(RESULT_PAGE_CONTROL_WIDTH),
            (0, 0)
        );
        assert_eq!(
            result_page_control_center_offsets(RESULT_PAGE_CONTROL_WIDTH + 1),
            (0, 1)
        );
        assert_eq!(
            result_page_control_center_offsets(RESULT_PAGE_CONTROL_WIDTH + 400),
            (200, 200)
        );
        assert!(!result_page_controls_fit(RESULT_PAGE_CONTROL_WIDTH - 1));
        assert!(result_page_controls_fit(RESULT_PAGE_CONTROL_WIDTH));
    }

    #[test]
    fn result_page_control_feedback_distinguishes_hover_press_and_exit() {
        let base = theme::input_bg();
        assert_eq!(
            result_page_control_feedback_color(Event::Enter, true, base),
            Some(theme::hover_feedback_color(base))
        );
        assert_eq!(
            result_page_control_feedback_color(Event::Push, true, base),
            Some(theme::selection_soft())
        );
        assert_eq!(
            result_page_control_feedback_color(Event::Drag, false, base),
            Some(base)
        );
        assert_eq!(
            result_page_control_feedback_color(Event::Released, true, base),
            Some(theme::hover_feedback_color(base))
        );
        assert_eq!(
            result_page_control_feedback_color(Event::Leave, false, base),
            Some(base)
        );
        assert_eq!(
            result_page_control_feedback_color(Event::KeyDown, true, base),
            None
        );
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
        let request = build_session_activity_result_request(Vec::new());

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
                "Connection ID",
                "Connection",
                "Connection State",
                "Scope",
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
            vec![
                "-",
                "No connections",
                "Unbound",
                "-",
                "-",
                "-",
                "-",
                "-",
                "Idle",
                "Idle",
                "-",
                "-",
                "-"
            ]
        );
    }

    #[test]
    fn session_activity_result_request_formats_active_rows() {
        let request = build_session_activity_result_request(vec![SessionActivityEntry {
            connection_id: None,
            connection_name: "Local".to_string(),
            connection_state: "Connected".to_string(),
            scope: Some("HR".to_string()),
            pool_size: 4,
            tab_name: "Query 1".to_string(),
            result_tab: Some(2),
            state: ResultTabStatus::Fetching.label().to_string(),
            database: "Oracle".to_string(),
            current_activity: "SELECT running".to_string(),
            sql_preview: "select * from employees".to_string(),
            fetched_rows: 42,
            elapsed: "3s".to_string(),
            active: true,
        }]);

        assert_eq!(request.result.message, "1 session(s)");
        assert_eq!(
            request.result.rows[0],
            vec![
                "-",
                "Local",
                "Connected",
                "HR",
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
    fn schema_update_requires_exact_tab_connection_generation_revision_request_and_scope() {
        let registry = ConnectionRegistry::new();
        let first_runtime = registry.register_unmanaged(create_shared_connection());
        let second_runtime = registry.register_unmanaged(create_shared_connection());
        let mut data = IntellisenseData::new();
        data.users = vec!["HR".to_string()];
        let update = SchemaUpdate {
            data,
            highlight_data: HighlightData::new(),
            query_tab_id: 7,
            connection_id: first_runtime.id(),
            connection_generation: 11,
            binding_revision: 13,
            request_id: 17,
            db_type: DatabaseType::Oracle,
            requested_scope: Some("HR".to_string()),
        };
        let target = ActiveSchemaUpdateTarget {
            query_tab_id: 7,
            connection_id: first_runtime.id(),
            connection_generation: 11,
            binding_revision: 13,
            request_id: 17,
            db_type: DatabaseType::Oracle,
            scope: Some("HR".to_string()),
        };

        assert!(MainWindow::schema_update_matches_target(&update, &target));

        let mut stale = update.clone();
        stale.connection_id = second_runtime.id();
        assert!(!MainWindow::schema_update_matches_target(&stale, &target));
        stale = update.clone();
        stale.connection_generation += 1;
        assert!(!MainWindow::schema_update_matches_target(&stale, &target));
        stale = update.clone();
        stale.binding_revision += 1;
        assert!(!MainWindow::schema_update_matches_target(&stale, &target));
        stale = update.clone();
        stale.request_id += 1;
        assert!(!MainWindow::schema_update_matches_target(&stale, &target));
        stale = update;
        stale.requested_scope = Some("SALES".to_string());
        assert!(!MainWindow::schema_update_matches_target(&stale, &target));
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
            kind: crate::db::SqlValueKind::Unknown,
        }
    }

    #[test]
    fn result_progress_routes_cover_each_tab_without_unintended_destinations() {
        assert_progress_routes_only(
            "select start with columns",
            QueryProgress::SelectStart {
                index: 0,
                columns: vec!["VALUE".to_string()],
                column_kinds: Vec::new(),
                null_text: "<NULL>".to_string(),
                sql: "select value from t".to_string(),
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
                column_kinds: Vec::new(),
                null_text: "<NULL>".to_string(),
                sql: "select value from t".to_string(),
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
    fn inactive_pending_lazy_fetch_is_reconciled_without_a_progress_context() {
        let pending = HashSet::from([10, 20, 30]);

        let orphaned =
            inactive_pending_lazy_fetch_sessions(&pending, |session_id| session_id == 20);

        assert_eq!(orphaned, vec![10, 30]);

        let orphaned_without_pending =
            inactive_pending_lazy_fetch_sessions(&HashSet::new(), |_| false);
        assert!(orphaned_without_pending.is_empty());
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
    fn execution_finished_event_gate_rejects_replaced_editor_operation_or_connection() {
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
            !execution_finished_event_matches_current_editor(&event, 7, Some(12), 0, 13, Some(17),),
            "same tab_id is not enough after an editor widget was recreated"
        );
        assert!(
            !execution_finished_event_matches_current_editor(&event, 8, Some(11), 0, 13, Some(17),),
            "same editor/operation id is not enough if the event belongs to another tab"
        );
        assert!(
            !execution_finished_event_matches_current_editor(&event, 7, Some(11), 14, 0, Some(17),),
            "late completion from an older operation must not update the active operation status"
        );
        assert!(
            !execution_finished_event_matches_current_editor(
                &event,
                7,
                Some(11),
                0,
                14,
                Some(17),
            ),
            "zero current operation must still reject events older than the last completed operation"
        );
        assert!(
            !execution_finished_event_matches_current_editor(&event, 7, Some(11), 0, 13, Some(18),),
            "a completion from a replaced physical connection must not update status"
        );
        assert!(
            !execution_finished_event_matches_current_editor(&event, 7, Some(11), 0, 13, None,),
            "a busy connection must not force the UI to block to authenticate a late event"
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
    fn registered_lazy_fetch_close_survives_newer_failed_background_operation() {
        let query_token = QueryOperationToken {
            tab_id: 7,
            editor_id: 11,
            operation_id: 13,
            connection_generation: 17,
        };
        let mut context =
            QueryProgressContext::new(None, "Executing SQL".to_string(), Some(query_token));
        context.register_lazy_fetch_session(44, 2, 44, 17);
        let close = QueryProgress::LazyFetchClosed {
            index: 2,
            session_id: 44,
            operation_id: 44,
            connection_generation: 17,
            cancelled: true,
            cursor_closed: true,
            fetch_worker_done: true,
            error_kind: crate::ui::sql_editor::InterruptKind::Cancelled,
        };

        assert!(!operation_progress_token_matches_current_editor(
            7,
            query_token,
            Some(11),
            0,
            45,
            &HashSet::new(),
        ));
        assert!(registered_lazy_fetch_progress_matches(
            &context,
            query_token,
            &close,
        ));
        assert!(registered_lazy_fetch_progress_matches(
            &context,
            query_token,
            &QueryProgress::BatchFinished,
        ));
        assert!(registered_lazy_fetch_progress_matches(
            &context,
            query_token,
            &QueryProgress::Rows {
                index: 2,
                rows: vec![vec!["value".to_string()]],
            },
        ));
    }

    #[test]
    fn registered_lazy_fetch_progress_rejects_wrong_session_generation() {
        let query_token = QueryOperationToken {
            tab_id: 7,
            editor_id: 11,
            operation_id: 13,
            connection_generation: 17,
        };
        let mut context =
            QueryProgressContext::new(None, "Executing SQL".to_string(), Some(query_token));
        context.register_lazy_fetch_session(44, 2, 44, 17);
        let stale_close = QueryProgress::LazyFetchClosed {
            index: 2,
            session_id: 44,
            operation_id: 44,
            connection_generation: 18,
            cancelled: true,
            cursor_closed: true,
            fetch_worker_done: true,
            error_kind: crate::ui::sql_editor::InterruptKind::Cancelled,
        };

        assert!(!registered_lazy_fetch_progress_matches(
            &context,
            query_token,
            &stale_close,
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
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn initial_window_has_no_query_tab_and_execution_can_bind_the_selected_database() {
        let _app = fltk::app::App::default();
        configure_fltk_globals(&AppConfig::default());
        let window = MainWindow::new_with_config(AppConfig::default());
        let mut state = window
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        assert!(state.editor_tabs.is_empty());
        assert!(state.query_tabs.tab_ids().is_empty());
        assert_eq!(state.active_editor_tab_id, 0);

        let registry = state.connection_registry.clone();
        let runtime = registry.register_unmanaged(create_shared_connection());
        let connection_id = runtime.id();
        state.object_browser.add_runtime(runtime);
        let tab_id = MainWindow::create_query_editor_tab_for_binding(
            &mut state,
            TabConnectionBinding::unbound(),
            true,
        )
        .expect("create unbound query tab");
        assert_eq!(state.active_editor_tab_id, tab_id);
        assert_eq!(state.active_connection_id(), None);

        state
            .bind_active_unbound_tab_to_selected_database()
            .expect("bind selected database before execution");
        assert_eq!(state.active_connection_id(), Some(connection_id));
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
            guard.result_tabs.start_streaming_by_id(
                result_tab_id,
                &["A".to_string()],
                &[],
                "NULL",
                "",
            );
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

        let choice = ExportChoice {
            format: ExportFormat::Csv,
            scope: crate::ui::result_export::ExportScope::All,
            destination: ExportDestination::File,
        };
        let export = MainWindow::prepare_result_export(&state, choice, None, Box::new(|_, _| {}))
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
