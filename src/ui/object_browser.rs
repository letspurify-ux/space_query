use fltk::{
    app, draw,
    enums::{Align, Color, Event, Font, FrameType, Key, Shortcut},
    frame::Frame,
    group::{Flex, FlexType, Group},
    input::Input,
    menu::{Choice, MenuButton, MenuFlag},
    prelude::*,
    tree::{Tree, TreeItem, TreeSelect},
    valuator::{Scrollbar, ScrollbarType},
    window::Window,
};
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::Duration;

use crate::db::{
    format_connection_busy_message, lock_connection_with_activity, try_lock_connection, ColumnInfo,
    CompilationError, ConnectionId, ConnectionRuntime, ConnectionRuntimeState, ConstraintInfo,
    IndexInfo, ObjectBrowser, PackageRoutine, ProcedureArgument, QueryResult, SequenceInfo,
    SharedConnection, SynonymInfo, TableColumnDetail,
};
use crate::ui::constants::*;
use crate::ui::font_settings::FontProfile;
use crate::ui::object_drag_payload;
use crate::ui::query_tabs::QueryTabId;
use crate::ui::theme;
use crate::ui::{
    HighlightData, IntellisenseData, PopupAnchorSnapshot, QualifiedMemberKind, ResultTabRequest,
    TableBrowseTarget,
};
use crate::utils::arithmetic::safe_div;

const SCOPE_SELECTOR_MAX_VISIBLE_ROWS: usize = 18;
const SCOPE_SELECTOR_ROW_HEIGHT: i32 = 24;
const SCOPE_SELECTOR_TABLE_VERTICAL_PADDING: i32 = 4;
const SCOPE_SELECTOR_SCREEN_MARGIN: i32 = 8;
const SCOPE_SELECTOR_TEXT_PADDING: i32 = 8;

struct ScopeChoiceMenuBusyGuard<'a> {
    flag: &'a AtomicBool,
}

impl<'a> ScopeChoiceMenuBusyGuard<'a> {
    fn new(flag: &'a AtomicBool) -> Self {
        flag.store(true, Ordering::Release);
        Self { flag }
    }
}

impl Drop for ScopeChoiceMenuBusyGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

struct CurrentGroupGuard {
    previous: Option<Group>,
}

impl CurrentGroupGuard {
    fn suspend() -> Self {
        let previous = Group::try_current();
        Group::set_current(None::<&Group>);
        Self { previous }
    }
}

impl Drop for CurrentGroupGuard {
    fn drop(&mut self) {
        if let Some(ref group) = self.previous {
            if !group.was_deleted() {
                Group::set_current(Some(group));
                return;
            }
        }
        Group::set_current(None::<&Group>);
    }
}

#[derive(Clone)]
pub enum SqlAction {
    Insert(String),
    OpenInNewTab(String),
    Execute(String),
    /// Multi-statement SQL the app generated, run the way F5 runs a script.
    ExecuteScript(String),
    BrowseTable(TableBrowseTarget),
    DisplayResult(ResultTabRequest),
    /// Rendered export bytes looking for a destination.
    ///
    /// The object browser renders but does not deliver: the file chooser, the
    /// write, the clipboard and the status line all stay in the main window, so
    /// a table exported from the tree lands exactly the way a grid export does.
    ExportData(ObjectExportDelivery),
}

/// A finished tree export, ready to be written or copied.
#[derive(Clone, Debug)]
pub struct ObjectExportDelivery {
    pub text: String,
    pub format: crate::ui::result_export::ExportFormat,
    pub destination: crate::ui::result_export::ExportDestination,
    pub row_count: usize,
    /// Base name to offer in the save panel, without an extension.
    pub suggested_name: String,
}

/// The object-browser actions that destroy something.
///
/// Both are shown with the exact statement they would run and are refused
/// unless the user confirms that statement: this app does not change a schema
/// out of sight of the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DestructiveObjectAction {
    Drop,
    Truncate,
}

impl DestructiveObjectAction {
    /// The menu label. `...` says a dialog comes before anything runs.
    const DROP_LABEL: &'static str = "Drop...";
    const TRUNCATE_LABEL: &'static str = "Truncate...";

    fn from_menu_label(label: &str) -> Option<Self> {
        match label {
            Self::DROP_LABEL => Some(Self::Drop),
            Self::TRUNCATE_LABEL => Some(Self::Truncate),
            _ => None,
        }
    }

    fn confirm_button(self) -> &'static str {
        match self {
            Self::Drop => "Drop",
            Self::Truncate => "Truncate",
        }
    }

    /// What the user is agreeing to, above the statement itself.
    fn confirm_prompt(self, qualified_name: &str) -> String {
        match self {
            Self::Drop => format!(
                "Drop {}?\n\nThe object and everything in it are removed, and this cannot be \
                 rolled back.",
                qualified_name
            ),
            Self::Truncate => format!(
                "Truncate {}?\n\nEvery row is removed, and this cannot be rolled back.",
                qualified_name
            ),
        }
    }
}

/// Whether the confirmation dialog for `action` on `sql` was accepted.
fn confirm_destructive_object_action(
    action: DestructiveObjectAction,
    qualified_name: &str,
    sql: &str,
) -> bool {
    let message = format!(
        "{}\n\nThis statement will run:\n{}",
        action.confirm_prompt(qualified_name),
        sql
    );
    matches!(
        crate::ui::choice2_on_main(&message, "Cancel", action.confirm_button(), ""),
        Some(1)
    )
}

/// Show the destructive-action confirmation the context menu shows, for the
/// feature-tour capture.
///
/// It runs the real prompt over the real statement, so the screenshot cannot
/// drift from what a right-click actually asks. Returns the built statement
/// alongside the answer; hiding the window from a timeout reads as Cancel.
#[doc(hidden)]
pub fn capture_tour_confirm_destructive_object_action(
    db_type: crate::db::DatabaseType,
    menu_label: &str,
    selected_scope: Option<&str>,
    object_type: &str,
    object_name: &str,
) -> Result<(String, bool), String> {
    let action = DestructiveObjectAction::from_menu_label(menu_label)
        .ok_or_else(|| format!("{menu_label} is not a destructive action"))?;
    let sql = ObjectBrowserWidget::destructive_object_sql(
        db_type,
        action,
        selected_scope,
        object_type,
        object_name,
    )
    .ok_or_else(|| format!("{menu_label} has no statement for {object_type}"))?;
    let qualified_name =
        ObjectBrowserWidget::qualify_object_name_for_scope(db_type, selected_scope, object_name);
    let accepted = confirm_destructive_object_action(action, &qualified_name, &sql);
    Ok((sql, accepted))
}

/// What Enter and double-click do on a tree node. Both key paths resolve
/// through this so the object browser has one default action per node.
#[derive(Debug, PartialEq, Eq)]
enum ObjectDefaultAction {
    Browse(TableBrowseTarget),
    GenerateDdl {
        object_type: &'static str,
        object_name: String,
    },
    /// Load package members on first activation, expand/collapse afterwards.
    PackageNode,
    /// Put text straight into the editor, for a node that names something
    /// rather than being an object of its own.
    InsertText(String),
    ToggleNode,
    None,
}

#[derive(Clone)]
pub struct ObjectBrowserMetadataSnapshot {
    pub db_type: crate::db::DatabaseType,
    pub connection_generation: u64,
    pub available_scopes: Vec<String>,
    pub selected_scope: Option<String>,
    pub tables: Vec<String>,
    pub views: Vec<String>,
    pub procedures: Vec<String>,
    pub functions: Vec<String>,
    pub sequences: Vec<String>,
    pub triggers: Vec<String>,
    pub events: Vec<String>,
    pub synonyms: Vec<String>,
    pub packages: Vec<String>,
}

impl ObjectBrowserMetadataSnapshot {
    fn from_cache(
        db_type: crate::db::DatabaseType,
        connection_generation: u64,
        available_scopes: Vec<String>,
        selected_scope: Option<String>,
        cache: &ObjectCache,
    ) -> Self {
        Self {
            db_type,
            connection_generation,
            available_scopes,
            selected_scope,
            tables: cache.tables.clone(),
            views: cache.views.clone(),
            procedures: cache.procedures.clone(),
            functions: cache.functions.clone(),
            sequences: cache.sequences.clone(),
            triggers: cache.triggers.clone(),
            events: cache.events.clone(),
            synonyms: cache.synonyms.clone(),
            packages: cache.packages.clone(),
        }
    }

    pub fn qualified_members(&self) -> Vec<(String, Option<QualifiedMemberKind>)> {
        let mut members = Vec::new();
        members.extend(
            self.tables
                .iter()
                .cloned()
                .map(|name| (name, Some(QualifiedMemberKind::Table))),
        );
        members.extend(
            self.views
                .iter()
                .cloned()
                .map(|name| (name, Some(QualifiedMemberKind::View))),
        );
        members.extend(
            self.procedures
                .iter()
                .cloned()
                .map(|name| (name, Some(QualifiedMemberKind::Procedure))),
        );
        members.extend(
            self.functions
                .iter()
                .cloned()
                .map(|name| (name, Some(QualifiedMemberKind::Function))),
        );
        members.extend(
            self.sequences
                .iter()
                .cloned()
                .map(|name| (name, Some(QualifiedMemberKind::Sequence))),
        );
        members.extend(
            self.triggers
                .iter()
                .cloned()
                .map(|name| (name, Some(QualifiedMemberKind::Trigger))),
        );
        members.extend(
            self.events
                .iter()
                .cloned()
                .map(|name| (name, Some(QualifiedMemberKind::Event))),
        );
        members.extend(
            self.synonyms
                .iter()
                .cloned()
                .map(|name| (name, Some(QualifiedMemberKind::Synonym))),
        );
        members.extend(
            self.packages
                .iter()
                .cloned()
                .map(|name| (name, Some(QualifiedMemberKind::Package))),
        );
        members
    }

    pub fn relation_members(&self) -> Vec<String> {
        self.tables
            .iter()
            .chain(self.views.iter())
            .chain(self.synonyms.iter())
            .cloned()
            .collect()
    }

    pub fn to_intellisense_data(&self) -> IntellisenseData {
        let mut data = IntellisenseData::new();
        data.users = self.available_scopes.clone();
        let selected_scope = self
            .selected_scope
            .as_deref()
            .map(str::trim)
            .filter(|scope| !scope.is_empty())
            .map(|scope| {
                if crate::sql_text::mysql_compatibility_for_sql("", Some(self.db_type)) {
                    data.canonical_qualifier_name(scope)
                        .unwrap_or_else(|| scope.to_string())
                } else {
                    scope.to_string()
                }
            });
        data.set_default_qualifier(selected_scope.clone());
        data.tables = self.tables.clone();
        data.views = self.views.clone();
        data.procedures = self.procedures.clone();
        data.functions = self.functions.clone();
        data.sequences = self.sequences.clone();
        data.triggers = self.triggers.clone();
        data.events = self.events.clone();
        data.synonyms = self.synonyms.clone();
        data.packages = self.packages.clone();
        if let Some(scope) = selected_scope.as_deref() {
            data.set_members_for_qualifier_with_kinds(scope, self.qualified_members());
            data.set_relation_members_for_qualifier(scope, self.relation_members());
        }
        data.rebuild_indices();
        data
    }

    pub fn to_highlight_data(&self) -> HighlightData {
        let mut highlight_data = HighlightData::new();
        highlight_data.tables = self.tables.clone();
        highlight_data.views = self.views.clone();
        highlight_data.functions = self.functions.clone();
        highlight_data.procedures = self.procedures.clone();
        highlight_data.packages = self.packages.clone();
        highlight_data.sequences = self.sequences.clone();
        highlight_data.triggers = self.triggers.clone();
        highlight_data.events = self.events.clone();
        highlight_data.synonyms = self.synonyms.clone();
        highlight_data.schemas = self.available_scopes.clone();
        highlight_data
    }
}

/// Callback type for executing SQL from object browser
pub type SqlExecuteCallback = Arc<Mutex<Option<Box<dyn FnMut(SqlAction)>>>>;
type StatusCallback = Arc<Mutex<Option<Box<dyn FnMut(&str)>>>>;
type ScopeChangeCallback = Arc<Mutex<Option<Box<dyn FnMut()>>>>;
type ScopeSwitchPreflightCallback = Arc<Mutex<Option<Box<dyn FnMut() -> Result<(), String>>>>>;
/// The tab whose card raised an action, when the card belongs to one.
///
/// An object-browser action can be delivered LONG after the click (the import
/// dialog reads a file and loads the target's columns on a worker first), and
/// the user may have switched tabs meanwhile. Resolving the target tab from
/// "whichever is active on delivery" put one tab's INSERTs inside another
/// tab's open transaction, so the raising tab travels with the action.
/// `None` is a connection-preview card, which owns no tab and keeps the
/// connection-level routing.
type ConnectionSqlExecuteCallback =
    Arc<Mutex<Option<Box<dyn FnMut(Option<QueryTabId>, ConnectionId, SqlAction)>>>>;
type ConnectionScopeChangeCallback = Arc<Mutex<Option<Box<dyn FnMut(ConnectionId)>>>>;
type ConnectionScopeSwitchPreflightCallback =
    Arc<Mutex<Option<Box<dyn FnMut(ConnectionId) -> Result<(), String>>>>>;
type MetadataCallback = Arc<Mutex<Option<Box<dyn FnMut(ObjectBrowserMetadataSnapshot)>>>>;
/// The app-facing metadata callback carries the tab whose card produced the
/// catalog: delivery can be deferred, and by the time it lands the active tab
/// may be another one.
type TabMetadataCallback =
    Arc<Mutex<Option<Box<dyn FnMut(QueryTabId, ObjectBrowserMetadataSnapshot)>>>>;

#[derive(Clone)]
pub enum ObjectItem {
    Simple {
        object_type: String,
        object_name: String,
    },
    PackageRoutine {
        package_name: String,
        routine_name: String,
        routine_type: String,
    },
    /// A column under a table node.
    ///
    /// It answers to Copy Name and to a drag into the editor, and to nothing
    /// else: it is not an object the catalog can describe, drop or open.
    Column {
        table_name: String,
        column_name: String,
    },
}

#[derive(Clone)]
struct ResolvedObjectContext {
    item: ObjectItem,
    selected_scope: Option<String>,
}

struct RoutineScriptData {
    qualified_name: String,
    resolved_routine_type: String,
    sql: String,
}

enum ObjectInfoPayload {
    Sequence(SequenceInfo),
    Synonym(SynonymInfo),
}

/// Stores original object lists for filtering
#[derive(Clone, Default)]
pub struct ObjectCache {
    pub tables: Vec<String>,
    pub views: Vec<String>,
    pub procedures: Vec<String>,
    pub functions: Vec<String>,
    pub sequences: Vec<String>,
    pub triggers: Vec<String>,
    pub events: Vec<String>,
    pub synonyms: Vec<String>,
    pub packages: Vec<String>,
    pub package_routines: HashMap<String, Vec<PackageRoutine>>,
    /// Columns per table, filled the first time a table node is expanded.
    ///
    /// Lives here rather than on the tree because the tree is rebuilt from this
    /// cache on every filter keystroke and every refresh — anything added
    /// straight to the widget would disappear on the next character typed.
    pub table_columns: HashMap<String, Vec<TableColumnDetail>>,
}

trait ObjectBrowserDbBehavior: Sync {
    fn qualify_object_name(&self, selected_scope: Option<&str>, object_name: &str) -> String;
    fn qualify_package_member_name(
        &self,
        selected_scope: Option<&str>,
        package_name: &str,
        routine_name: &str,
    ) -> String;
    fn preview_select_sql(&self, selected_scope: Option<&str>, object_name: &str) -> String;
    /// The statement `Export Data...` runs: the same relation the preview
    /// shows, with no row limit.
    fn export_select_sql(&self, selected_scope: Option<&str>, object_name: &str) -> String;
    /// The statement `action` would run on this object, or `None` when the
    /// backend has no such statement for that object type.
    ///
    /// This is the only place the DDL is spelled: the context menu offers the
    /// action exactly when this returns `Some`, so the menu and what runs
    /// cannot drift apart.
    fn destructive_object_sql(
        &self,
        action: DestructiveObjectAction,
        selected_scope: Option<&str>,
        object_type: &str,
        object_name: &str,
    ) -> Option<String>;
    fn build_simple_procedure_script(&self, qualified_name: &str) -> String;
    fn build_simple_function_script(&self, qualified_name: &str) -> String;
    fn build_routine_script(
        &self,
        qualified_name: &str,
        routine_type: &str,
        arguments: &[ProcedureArgument],
    ) -> String;
    fn action_scope<'a>(
        &self,
        selected_scope: Option<&'a str>,
        context: &'a crate::db::DbPoolSessionContext,
    ) -> Option<&'a str>;
    fn load_routine_script(
        &self,
        context: &crate::db::DbPoolSessionContext,
        session: &mut crate::db::DbPoolSession,
        selected_scope: Option<&str>,
        object_name: &str,
        routine_type: &str,
    ) -> Result<RoutineScriptData, String>;
    fn load_table_structure(
        &self,
        context: &crate::db::DbPoolSessionContext,
        session: &mut crate::db::DbPoolSession,
        selected_scope: Option<&str>,
        table_name: &str,
    ) -> Result<Vec<TableColumnDetail>, String>;
    /// Read a whole table for `Export Data...`.
    ///
    /// Every row: an export that quietly stopped at some limit would produce a
    /// file that looks complete and is not.
    fn load_table_rows(
        &self,
        context: &crate::db::DbPoolSessionContext,
        session: &mut crate::db::DbPoolSession,
        selected_scope: Option<&str>,
        table_name: &str,
    ) -> Result<crate::db::QueryResult, String>;
    fn load_table_indexes(
        &self,
        context: &crate::db::DbPoolSessionContext,
        session: &mut crate::db::DbPoolSession,
        selected_scope: Option<&str>,
        table_name: &str,
    ) -> Result<Vec<IndexInfo>, String>;
    fn load_table_constraints(
        &self,
        context: &crate::db::DbPoolSessionContext,
        session: &mut crate::db::DbPoolSession,
        selected_scope: Option<&str>,
        table_name: &str,
    ) -> Result<Vec<ConstraintInfo>, String>;
    fn load_object_info(
        &self,
        context: &crate::db::DbPoolSessionContext,
        session: &mut crate::db::DbPoolSession,
        selected_scope: Option<&str>,
        object_type: &str,
        object_name: &str,
    ) -> Result<ObjectInfoPayload, String>;
    fn generate_object_ddl(
        &self,
        context: &crate::db::DbPoolSessionContext,
        session: &mut crate::db::DbPoolSession,
        selected_scope: Option<&str>,
        object_type: &str,
        object_name: &str,
    ) -> Result<String, String>;
    fn supports_package_routines(&self) -> bool;
    fn load_package_routines(
        &self,
        connection: &SharedConnection,
        activity: String,
        selected_scope: Option<&str>,
        package_name: &str,
    ) -> Result<Vec<PackageRoutine>, String>;
    fn load_package_routine_script(
        &self,
        connection: &SharedConnection,
        activity: String,
        selected_scope: Option<&str>,
        package_name: &str,
        routine_name: &str,
        routine_type: &str,
    ) -> Result<RoutineScriptData, String>;
    fn load_compilation_errors(
        &self,
        connection: &SharedConnection,
        activity: String,
        selected_scope: Option<&str>,
        object_name: &str,
        object_type: &str,
    ) -> Result<(String, Vec<CompilationError>), String>;
    fn menu_choices_for_object_item(&self, item_info: &ObjectItem) -> Option<&'static str>;
    fn root_categories(&self, cache: &ObjectCache) -> Vec<&'static str>;
    fn load_metadata_cache(
        &self,
        context: crate::db::DbPoolSessionContext,
        requested_scope: Option<String>,
        activity: &crate::db::DbActivityGuard,
    ) -> Option<(
        crate::db::DatabaseType,
        ObjectCache,
        Vec<String>,
        Option<String>,
    )>;
}

struct OracleObjectBrowserBehavior;
struct MysqlObjectBrowserBehavior;

static ORACLE_OBJECT_BROWSER_BEHAVIOR: OracleObjectBrowserBehavior = OracleObjectBrowserBehavior;
static MYSQL_OBJECT_BROWSER_BEHAVIOR: MysqlObjectBrowserBehavior = MysqlObjectBrowserBehavior;

fn object_browser_behavior_for(
    db_type: crate::db::DatabaseType,
) -> &'static dyn ObjectBrowserDbBehavior {
    match db_type {
        crate::db::DatabaseType::Oracle => &ORACLE_OBJECT_BROWSER_BEHAVIOR,
        crate::db::DatabaseType::MySQL => &MYSQL_OBJECT_BROWSER_BEHAVIOR,
        crate::db::DatabaseType::MariaDB => &MYSQL_OBJECT_BROWSER_BEHAVIOR,
    }
}

impl MysqlObjectBrowserBehavior {
    fn take_object_action_session<'a>(
        &self,
        context: &crate::db::DbPoolSessionContext,
        session: &'a mut crate::db::DbPoolSession,
    ) -> Result<&'a mut mysql::PooledConn, String> {
        let expected_db_type = context.connection_info.db_type;
        let actual_db_type = session.db_type();
        let crate::db::DbPoolSession::MySQL { conn, db_type } = session else {
            return Err(format!(
                "Expected {} object action session but acquired {}",
                expected_db_type.display_name(),
                actual_db_type
            ));
        };
        if db_type.is_same_type_as(expected_db_type) {
            Ok(conn)
        } else {
            Err(format!(
                "Expected {} object action session but acquired {}",
                expected_db_type.display_name(),
                db_type
            ))
        }
    }
}

enum RefreshEvent {
    Finished {
        cache: Box<ObjectCache>,
        db_type: crate::db::DatabaseType,
        available_scopes: Vec<String>,
        selected_scope: Option<String>,
        scope_generation: u64,
        connection_generation: u64,
        activity_guard: crate::db::DbActivityGuard,
    },
    Failed {
        message: String,
        scope_generation: u64,
        connection_generation: u64,
        activity_guard: crate::db::DbActivityGuard,
    },
}

enum RefreshRequest {
    Metadata {
        selected_scope: Option<String>,
        scope_generation: u64,
        context: crate::db::DbPoolSessionContext,
        activity_guard: crate::db::DbActivityGuard,
    },
}

const REFRESH_TREE_BATCH_SIZE: usize = 300;
type ObjectMetadataLoadJob = Box<dyn FnOnce() -> ObjectCache + Send + 'static>;

struct PendingTreeRefresh {
    paths: Vec<String>,
    next_index: usize,
    activity_guard: crate::db::DbActivityGuard,
}

/// The handles that let the UI end a metadata load it started.
///
/// `activity` doubles as the liveness flag: it reports active for exactly as
/// long as the load still owns a status-bar entry, so a finished refresh needs
/// no separate bookkeeping to stop looking cancelable.
struct InFlightMetadataRefresh {
    activity_id: u64,
    activity: crate::db::DbActivityFinishHandle,
}

enum ObjectActionResult {
    TableStructure {
        table_name: String,
        result: Result<Vec<TableColumnDetail>, String>,
    },
    ImportTarget {
        qualified_name: String,
        file_label: String,
        db_type: crate::db::DatabaseType,
        format: crate::ui::result_export::ExportFormat,
        /// The file's text and the table's columns, loaded together so one
        /// failure reports one message.
        result: Result<(String, Vec<TableColumnDetail>), String>,
    },
    TableIndexes {
        table_name: String,
        result: Result<Vec<IndexInfo>, String>,
    },
    TableConstraints {
        table_name: String,
        result: Result<Vec<ConstraintInfo>, String>,
    },
    SequenceInfo(Result<SequenceInfo, String>),
    SynonymInfo(Result<SynonymInfo, String>),
    Ddl(Result<String, String>),
    RoutineScript {
        qualified_name: String,
        routine_type: String,
        db_type: crate::db::DatabaseType,
        result: Result<String, String>,
    },
    PackageRoutines {
        package_name: String,
        result: Result<Vec<PackageRoutine>, String>,
        scope_generation: u64,
        select_first_child_after_load: bool,
    },
    PackageRoutineContextMenu {
        item: ObjectItem,
        db_type: crate::db::DatabaseType,
        selected_scope: Option<String>,
        package_name: String,
        result: Result<Vec<PackageRoutine>, String>,
        mouse_x: i32,
        mouse_y: i32,
        scope_generation: u64,
    },
    ScopeSwitchFinished {
        db_type: crate::db::DatabaseType,
        target_scope: String,
        previous_scope: Option<String>,
        generation: u64,
        result: Result<(), String>,
    },
    CompilationErrors {
        object_name: String,
        object_type: String,
        status: String,
        result: Result<Vec<CompilationError>, String>,
    },
    TableColumns {
        table_name: String,
        result: Result<Vec<TableColumnDetail>, String>,
        scope_generation: u64,
    },
    ExportedTable {
        qualified_name: String,
        db_type: crate::db::DatabaseType,
        choice: crate::ui::result_export_dialog::ExportChoice,
        result: Result<crate::db::QueryResult, String>,
    },
}

/// Whether a write started from one browser card would be refused — and the
/// two independent sources that can say so.
///
/// They are kept apart because they are written by different people at
/// different times: the connection's read-only flag belongs to the saved
/// profile and is re-stated for EVERY card whenever the runtimes are
/// re-labelled, while the READ ONLY transaction mode is pinned per TAB and is
/// only known where the tab's mode is resolved. Holding one combined flag meant
/// the connection-wide writer erased the tab's answer every time it ran, and
/// the menus offered Drop, Truncate and Import on a tab whose next write the
/// statement gate was going to refuse. There is deliberately no setter for the
/// combined value: neither source can be spelled over the other.
///
/// Both halves are atomics because a context menu must never wait on the
/// connection mutex — it would hang the UI while a query runs, or guess
/// "writable" while it waits.
#[derive(Clone, Default)]
pub struct CardWriteRefusal {
    connection: Arc<AtomicBool>,
    tab_mode: Arc<AtomicBool>,
}

impl CardWriteRefusal {
    /// The connection's own read-only flag: the same answer for every card on
    /// that connection.
    fn set_connection(&self, refused: bool) {
        self.connection.store(refused, Ordering::Release);
    }

    /// The READ ONLY transaction mode pinned on the tab this card belongs to.
    fn set_tab_mode(&self, refused: bool) {
        self.tab_mode.store(refused, Ordering::Release);
    }

    /// The answer the menus ask for.
    fn writes_are_refused(&self) -> bool {
        self.connection.load(Ordering::Acquire) || self.tab_mode.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
pub struct ObjectBrowserWidget {
    flex: Flex,
    tree: Tree,
    connection: SharedConnection,
    sql_callback: SqlExecuteCallback,
    status_callback: StatusCallback,
    scope_change_callback: ScopeChangeCallback,
    scope_switch_preflight_callback: ScopeSwitchPreflightCallback,
    metadata_callback: MetadataCallback,
    scope_label: Frame,
    scope_choice: Choice,
    filter_input: Input,
    object_cache: Arc<Mutex<ObjectCache>>,
    current_db_type: Arc<Mutex<crate::db::DatabaseType>>,
    write_refusal: CardWriteRefusal,
    scope_options: Arc<Mutex<Vec<String>>>,
    selected_scope: Arc<Mutex<Option<String>>>,
    suppress_scope_events: Arc<Mutex<bool>>,
    scope_choice_menu_busy: Arc<AtomicBool>,
    active_scope_selector_popup: Arc<Mutex<Option<Window>>>,
    scope_generation: Arc<AtomicU64>,
    scope_switch_in_progress: Arc<AtomicBool>,
    tab_local_scope_selection: Arc<AtomicBool>,
    refresh_connection_generation: Arc<AtomicU64>,
    pending_tree_refresh: Arc<Mutex<Option<PendingTreeRefresh>>>,
    in_flight_metadata_refresh: Arc<Mutex<Option<InFlightMetadataRefresh>>>,
    /// Set when a metadata load has actually delivered a catalog for this
    /// card, cleared when that catalog is thrown away (a new load, a
    /// disconnect). It is what tells a new card whether a sibling has
    /// anything worth inheriting — the object cache alone cannot say so, since
    /// a schema may legitimately be empty, and the scope list cannot either,
    /// because selecting a scope inserts it there before anything is loaded.
    /// What catalog this card holds and what question it answers. One value,
    /// so no caller can move the ask without deciding the catalog's fate.
    catalog: CardCatalogState,
    /// The card holds a catalog its tree has not been drawn from yet. Set
    /// when a hidden card adopts one: drawing a whole schema is slow enough
    /// that doing it for every hidden card the moment a load lands freezes
    /// the UI, and nothing can see the tree until the card is shown anyway.
    tree_rebuild_pending: Arc<AtomicBool>,
    poll_lifecycle: Arc<()>,
    refresh_request_sender: Sender<RefreshRequest>,
    action_sender: std::sync::mpsc::Sender<ObjectActionResult>,
}

impl ObjectBrowserWidget {
    pub fn new(x: i32, y: i32, w: i32, h: i32, connection: SharedConnection) -> Self {
        let initial_db_type = crate::db::try_lock_connection(&connection)
            .map(|guard| guard.db_type())
            .unwrap_or_default();

        // Create a flex container for the filter input and tree
        let mut flex = Flex::default().with_pos(x, y).with_size(w, h);
        flex.set_type(FlexType::Column);
        flex.set_margins(TOOLBAR_SPACING, 0, TOOLBAR_SPACING, 0);
        flex.set_spacing(DIALOG_SPACING);

        let mut scope_row = Flex::default();
        scope_row.set_type(FlexType::Row);
        scope_row.set_spacing(0);

        let mut scope_label = Frame::default().with_label(Self::scope_label_text(initial_db_type));
        scope_label.set_label_color(theme::text_primary());
        scope_label.hide();
        scope_row.fixed(&scope_label, 0);

        let mut scope_choice = Choice::default();
        theme::style_choice(&mut scope_choice);
        scope_choice.deactivate();
        scope_row.resizable(&scope_choice);
        scope_row.end();
        flex.fixed(&scope_row, FILTER_INPUT_HEIGHT);

        let mut filter_row = Flex::default();
        filter_row.set_type(FlexType::Row);
        filter_row.set_spacing(DIALOG_SPACING);

        // Filter input with modern styling
        let mut filter_input = Input::default();
        filter_input.set_color(theme::input_bg());
        filter_input.set_text_color(theme::text_primary());
        theme::apply_text_input_inset(&mut filter_input);
        filter_input.set_tooltip("Type to filter objects...");
        filter_row.resizable(&filter_input);
        filter_row.end();
        flex.fixed(&filter_row, FILTER_INPUT_HEIGHT);

        // Tree view with modern styling
        let mut tree = Tree::default();

        tree.set_color(theme::panel_bg());
        tree.set_selection_color(theme::selection_soft());
        tree.set_item_label_fgcolor(theme::text_secondary());
        tree.set_connector_color(theme::tree_connector());
        tree.set_select_mode(TreeSelect::Single);
        theme::style_tree_scrollbars(&mut tree);

        // Initialize tree structure
        tree.set_show_root(false);
        Self::rebuild_root_categories_for_db_type(
            &mut tree,
            initial_db_type,
            &ObjectCache::default(),
        );

        // Make tree resizable (takes remaining space after filter input)
        flex.resizable(&tree);
        flex.end();

        let sql_callback: SqlExecuteCallback = Arc::new(Mutex::new(None));
        let status_callback: StatusCallback = Arc::new(Mutex::new(None));
        let scope_change_callback: ScopeChangeCallback = Arc::new(Mutex::new(None));
        let scope_switch_preflight_callback: ScopeSwitchPreflightCallback =
            Arc::new(Mutex::new(None));
        let metadata_callback: MetadataCallback = Arc::new(Mutex::new(None));
        let object_cache = Arc::new(Mutex::new(ObjectCache::default()));
        let current_db_type = Arc::new(Mutex::new(initial_db_type));
        let write_refusal = CardWriteRefusal::default();
        let scope_options = Arc::new(Mutex::new(Vec::new()));
        let selected_scope = Arc::new(Mutex::new(None));
        let suppress_scope_events = Arc::new(Mutex::new(false));
        let scope_choice_menu_busy = Arc::new(AtomicBool::new(false));
        let active_scope_selector_popup = Arc::new(Mutex::new(None));
        let scope_generation = Arc::new(AtomicU64::new(0));
        let scope_switch_in_progress = Arc::new(AtomicBool::new(false));
        let tab_local_scope_selection = Arc::new(AtomicBool::new(false));
        let refresh_connection_generation = Arc::new(AtomicU64::new(0));
        let pending_tree_refresh = Arc::new(Mutex::new(None));
        let in_flight_metadata_refresh = Arc::new(Mutex::new(None));
        let catalog = CardCatalogState::new();
        let tree_rebuild_pending = Arc::new(AtomicBool::new(false));
        let poll_lifecycle = Arc::new(());

        let (refresh_sender, refresh_receiver) = std::sync::mpsc::channel::<RefreshEvent>();
        let (refresh_request_sender, refresh_request_receiver) =
            std::sync::mpsc::channel::<RefreshRequest>();
        let (action_sender, action_receiver) = std::sync::mpsc::channel::<ObjectActionResult>();

        Self::spawn_refresh_worker(refresh_request_receiver, refresh_sender);

        let mut widget = Self {
            flex,
            tree,
            connection,
            scope_change_callback,
            scope_switch_preflight_callback,
            metadata_callback,
            scope_label,
            scope_choice,
            filter_input,
            object_cache,
            current_db_type,
            write_refusal,
            scope_options,
            selected_scope,
            suppress_scope_events,
            scope_choice_menu_busy,
            active_scope_selector_popup,
            scope_generation,
            scope_switch_in_progress,
            tab_local_scope_selection,
            refresh_connection_generation,
            pending_tree_refresh,
            in_flight_metadata_refresh,
            catalog,
            tree_rebuild_pending,
            poll_lifecycle,
            sql_callback,
            status_callback,
            refresh_request_sender,
            action_sender,
        };
        widget.setup_callbacks();
        widget.setup_scope_choice_popup_handler();
        widget.setup_scope_callback();
        widget.setup_filter_callback();
        widget.setup_refresh_handler(refresh_receiver);
        widget.setup_action_handler(action_receiver);
        widget
    }

    pub fn get_widget(&self) -> Flex {
        self.flex.clone()
    }

    pub fn apply_font_settings(&mut self, profile: FontProfile, ui_size: i32) {
        self.scope_label.set_label_size(ui_size);
        self.scope_choice.set_text_font(profile.normal);
        self.scope_choice.set_text_size(ui_size);
        self.filter_input.set_text_font(profile.normal);
        self.filter_input.set_text_size(ui_size);
        self.tree.set_item_label_font(profile.normal);
        self.tree.set_item_label_size(ui_size);
        let canceled_pending_refresh = self.clear_pending_tree_refresh();
        let filter_text = self.filter_input.value().to_lowercase();
        let cache_snapshot = self
            .object_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let db_type = self
            .current_db_type
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .to_owned();
        Self::rebuild_root_categories_for_db_type(&mut self.tree, db_type, &cache_snapshot);
        Self::populate_tree(&mut self.tree, &cache_snapshot, &filter_text);
        // Force layout recalculation so new font metrics take effect immediately.
        let (x, y, w, h) = (self.tree.x(), self.tree.y(), self.tree.w(), self.tree.h());
        self.tree.resize(x, y, w, h);
        self.flex.layout();
        self.filter_input.redraw();
        self.scope_choice.redraw();
        self.tree.redraw();
        if canceled_pending_refresh {
            self.emit_status("Object browser metadata refresh completed");
        }
    }

    #[doc(hidden)]
    fn example_column(
        name: &str,
        data_type: &str,
        data_length: i32,
        data_precision: Option<i32>,
        data_scale: Option<i32>,
        nullable: bool,
        is_primary_key: bool,
    ) -> TableColumnDetail {
        TableColumnDetail {
            name: name.to_string(),
            data_type: data_type.to_string(),
            data_length,
            data_precision,
            data_scale,
            nullable,
            default_value: None,
            is_primary_key,
        }
    }

    #[doc(hidden)]
    pub fn capture_tour_set_example_metadata(&mut self) {
        let cache = ObjectCache {
            tables: vec![
                "DEPT".to_string(),
                "EMP".to_string(),
                "SALGRADE".to_string(),
                "SQ_CUSTOMERS".to_string(),
                "SQ_ORDERS".to_string(),
            ],
            views: vec![
                "EMP_DETAILS_VIEW".to_string(),
                "SQ_ORDER_SUMMARY".to_string(),
            ],
            procedures: vec!["RAISE_SALARY".to_string()],
            functions: vec!["ANNUAL_SALARY".to_string()],
            sequences: vec!["SQ_ORDER_SEQ".to_string()],
            triggers: vec!["EMP_AUDIT_TRG".to_string()],
            packages: vec!["EMP_API".to_string()],
            // One expanded table, so the capture tour can show what a table
            // node looks like once its columns have been read.
            table_columns: [(
                "EMP".to_string(),
                vec![
                    Self::example_column("EMPNO", "NUMBER", 22, Some(4), Some(0), false, true),
                    Self::example_column("ENAME", "VARCHAR2", 10, None, None, true, false),
                    Self::example_column("JOB", "VARCHAR2", 9, None, None, true, false),
                    Self::example_column("MGR", "NUMBER", 22, Some(4), Some(0), true, false),
                    Self::example_column("HIREDATE", "DATE", 7, None, None, true, false),
                    Self::example_column("SAL", "NUMBER", 22, Some(7), Some(2), true, false),
                    Self::example_column("DEPTNO", "NUMBER", 22, Some(2), Some(0), true, false),
                ],
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        *self
            .current_db_type
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = crate::db::DatabaseType::Oracle;
        *self
            .object_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = cache.clone();
        *self
            .scope_options
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec!["SYSTEM".to_string(), "SYS".to_string()];
        *self
            .selected_scope
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some("SYSTEM".to_string());
        self.scope_choice.clear();
        self.scope_choice.add_choice("SYSTEM|SYS");
        self.scope_choice.set_value(0);
        self.scope_choice.activate();
        // The example catalog stands in for a completed load of the
        // connection's default schema — which is what a tab bound by
        // connecting asks for — so a new tab may inherit it instead of asking
        // a database that does not exist here.
        self.catalog.load_started(None);
        self.catalog.catalog_arrived();
        Self::rebuild_root_categories_for_db_type(
            &mut self.tree,
            crate::db::DatabaseType::Oracle,
            &cache,
        );
        Self::populate_tree(&mut self.tree, &cache, "");
        let _ = self.tree.open("Tables", false);
        let _ = self.tree.open("Views", false);
        let _ = self.tree.select("Tables/EMP", false);
        self.flex.layout();
        self.scope_choice.redraw();
        self.tree.redraw();
    }

    pub fn selected_scope(&self) -> Option<String> {
        self.selected_scope
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Whether a metadata load has filled this card. A card that answers
    /// `false` has an empty tree and nothing an editor can complete with.
    fn has_loaded_metadata(&self) -> bool {
        self.catalog.is_loaded()
    }

    fn metadata_requested_scope(&self) -> Option<String> {
        self.catalog.requested_scope()
    }

    fn metadata_serial(&self) -> u64 {
        self.catalog.serial()
    }

    /// Forget that this card's catalog is current, without disturbing what is
    /// on screen. Used when the server behind the card changed.
    fn invalidate_loaded_metadata(&self) {
        self.catalog.invalidate();
    }

    /// Whether this card is showing `scope`, by the database's own rule for
    /// comparing schema/database names — the same comparison the retained
    /// session and the scope selector use, so nothing can disagree about
    /// whether two cards describe the same place.
    /// Whether the catalog this card holds was read for `scope`.
    fn requested_scope_matches(&self, scope: Option<&str>) -> bool {
        let db_type = *self
            .current_db_type
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.scope_names_match(db_type, self.metadata_requested_scope().as_deref(), scope)
    }

    /// Whether `other` is asking the same question this card's catalog
    /// answers. Judged entirely with THIS card's database type and option
    /// list, since `other` may know neither yet.
    fn request_matches_request_of(&self, other: &ObjectBrowserWidget) -> bool {
        let db_type = *self
            .current_db_type
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.scope_names_match(
            db_type,
            self.metadata_requested_scope().as_deref(),
            other.metadata_requested_scope().as_deref(),
        )
    }

    fn scope_matches(&self, scope: Option<&str>) -> bool {
        let db_type = *self
            .current_db_type
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.scope_matches_for_db_type(db_type, scope)
    }

    fn scope_matches_for_db_type(
        &self,
        db_type: crate::db::DatabaseType,
        scope: Option<&str>,
    ) -> bool {
        self.scope_names_match(db_type, self.selected_scope().as_deref(), scope)
    }

    /// Compare two schema/database names by this database's rule, with THIS
    /// card's option list settling the MySQL case question — the caller may be
    /// asking on behalf of a card that knows neither.
    fn scope_names_match(
        &self,
        db_type: crate::db::DatabaseType,
        card_scope: Option<&str>,
        scope: Option<&str>,
    ) -> bool {
        if Self::scope_values_match_for_db_type(db_type, card_scope, scope) {
            return true;
        }
        // MySQL and MariaDB report a database in the catalog's own case, which
        // need not be the case the tab recorded, and the app's schema-update
        // gate already accepts that difference when the name is unambiguous.
        // Being stricter here would call a tab "out of sync" with its own card
        // on every activation and reload the whole catalog each time.
        if !db_type.is_mysql_or_mariadb() {
            return false;
        }
        let trimmed = |scope: Option<&str>| {
            scope
                .map(str::trim)
                .filter(|scope| !scope.is_empty())
                .map(str::to_string)
        };
        let (Some(card_scope), Some(scope)) = (trimmed(card_scope), trimmed(scope)) else {
            return false;
        };
        if !card_scope.eq_ignore_ascii_case(&scope) {
            return false;
        }
        // Two databases differing only in case are two databases: refuse.
        self.scope_options
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|option| option.eq_ignore_ascii_case(&scope))
            .take(2)
            .count()
            == 1
    }

    /// Copies another card's already-loaded metadata into this one and draws
    /// the tree from it. A new query tab's card is born empty, and filling it
    /// from the database would mean a second full metadata load plus an empty
    /// tree until it lands; a sibling card on the same connection already
    /// holds exactly the same server-side truth, so take it verbatim
    /// (`object_cache` and not the lossy snapshot, so table columns that were
    /// already expanded come along).
    ///
    /// Returns whether anything was adopted — `false` means the source is
    /// itself empty and the caller still has to run a real refresh.
    fn adopt_metadata_from(&mut self, source: &ObjectBrowserWidget) -> bool {
        if !source.has_loaded_metadata() {
            return false;
        }
        let db_type = *source
            .current_db_type
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let cache = source
            .object_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let scope_options = source
            .scope_options
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        *self
            .current_db_type
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = db_type;
        *self
            .object_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = cache.clone();
        *self
            .scope_options
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = scope_options;
        // `fetch_max`, never `store`: `cancel_metadata_refresh` invalidates a
        // cancelled worker's late result by bumping this counter, and moving
        // it back would let that result through.
        self.refresh_connection_generation.fetch_max(
            source.refresh_connection_generation.load(Ordering::Acquire),
            Ordering::AcqRel,
        );
        // Claim the scope FIRST: `set_selected_scope` treats a change of
        // scope as "the catalog no longer describes this card" and clears the
        // loaded flag, which would undo the stamp below.
        self.set_selected_scope(source.selected_scope());
        // The adopted catalog is as real as a loaded one: without this the
        // card would show a full tree and still report itself empty, so the
        // next tab switch would throw the tree away and reload it. It is
        // exactly as fresh as what it was copied from.
        self.catalog.adopt_from(&source.catalog);
        if self.tree.visible_r() {
            self.rebuild_tree_from_cache(db_type, &cache);
        } else {
            // Drawing a full catalog is the expensive half. A hidden card
            // draws when it is shown, so a load landing with several empty
            // sibling cards does not stall the UI thread once per card.
            self.tree_rebuild_pending.store(true, Ordering::Release);
        }
        true
    }

    fn rebuild_tree_from_cache(&mut self, db_type: crate::db::DatabaseType, cache: &ObjectCache) {
        Self::rebuild_root_categories_for_db_type(&mut self.tree, db_type, cache);
        // The user may have typed a filter while this card was still empty;
        // the adopted catalog has to arrive through it, exactly as a load
        // would (`setup_filter_callback` uses the same pair).
        let filter_text = self.filter_input.value().to_lowercase();
        Self::populate_tree(&mut self.tree, cache, &filter_text);
        self.tree.redraw();
        self.tree_rebuild_pending.store(false, Ordering::Release);
    }

    /// Draw the adopted catalog if this card put it off while hidden. Called
    /// when a card is shown.
    fn rebuild_tree_if_pending(&mut self) {
        if !self.tree_rebuild_pending.swap(false, Ordering::AcqRel) {
            return;
        }
        if self.tree.was_deleted() {
            return;
        }
        let db_type = *self
            .current_db_type
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let cache = self
            .object_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        self.rebuild_tree_from_cache(db_type, &cache);
    }

    fn metadata_snapshot(&self) -> ObjectBrowserMetadataSnapshot {
        let db_type = *self
            .current_db_type
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let connection_generation = self.refresh_connection_generation.load(Ordering::Acquire);
        let available_scopes = self
            .scope_options
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let selected_scope = self.selected_scope();
        let cache = self
            .object_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        ObjectBrowserMetadataSnapshot::from_cache(
            db_type,
            connection_generation,
            available_scopes,
            selected_scope,
            &cache,
        )
    }

    pub fn reset_selected_scope(&mut self) {
        self.scope_switch_in_progress
            .store(false, Ordering::Release);
        self.scope_generation.fetch_add(1, Ordering::Relaxed);
        *self
            .selected_scope
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    pub fn set_selected_scope(&mut self, scope: Option<String>) {
        let normalized_scope = scope
            .map(|scope| scope.trim().to_string())
            .filter(|scope| !scope.is_empty());
        let db_type = *self
            .current_db_type
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Compared against what the held catalog was ASKED for, not against
        // the name a load resolved that ask to. Writing a tab's own scope back
        // into its card — which the refresh path and every scope
        // synchronisation do — hands `None` to a card that resolved
        // `SYSTEM`; judging that a change would throw away a catalog that
        // still answers exactly the question it was asked, and the reload
        // that was supposed to replace it does not even start while the
        // connection is busy. The card would then sit there showing a full
        // tree while reporting itself empty — and the next sibling load would
        // "fill" it, collapsing the user's expanded tree under them.
        let previous_request = self.metadata_requested_scope();
        let answers_new_ask = self.scope_names_match(
            db_type,
            previous_request.as_deref(),
            normalized_scope.as_deref(),
        );
        if !answers_new_ask {
            self.scope_generation.fetch_add(1, Ordering::Relaxed);
            self.scope_switch_in_progress
                .store(false, Ordering::Release);
            // The catalog still describes the PREVIOUS database/schema, and
            // until the reload lands this card is not a catalog of the scope
            // it now claims — a tab opened in that window (the reload cannot
            // even start while the connection is busy) would inherit one
            // schema's objects under another schema's name and never correct
            // itself. That holds for a move to "unspecified" too: a catalog
            // read in a named schema is not "wherever the session lands".
        }
        // One call moves the ask AND settles the catalog's fate: they cannot
        // drift apart, whatever a future caller here forgets.
        self.catalog
            .ask_for(normalized_scope.clone(), answers_new_ask);
        *self
            .selected_scope
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = normalized_scope.clone();

        let available_scopes = {
            let mut options = self
                .scope_options
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(ref scope) = normalized_scope {
                if !options.iter().any(|option| {
                    Self::scope_values_match_for_db_type(
                        db_type,
                        Some(option.as_str()),
                        Some(scope.as_str()),
                    )
                }) {
                    options.push(scope.clone());
                    options.sort();
                    options.dedup();
                }
            }
            options.clone()
        };
        Self::sync_scope_choice_widget(
            &mut self.scope_choice,
            &self.suppress_scope_events,
            &self.scope_choice_menu_busy,
            db_type,
            &available_scopes,
            normalized_scope.as_deref(),
            self.scope_switch_in_progress.load(Ordering::Acquire),
        );
    }

    /// Tells this card that its CONNECTION refuses writes (the saved profile's
    /// read-only flag). The tab's own READ ONLY pin is the other half of the
    /// answer and is stated separately — see [`CardWriteRefusal`].
    pub fn set_connection_refuses_writes(&self, refused: bool) {
        self.write_refusal.set_connection(refused);
    }

    /// Tells this card that the tab it belongs to is pinned READ ONLY, so a
    /// write from its menus would be refused by
    /// `transaction_mode_refusal_for_statement` once the statement was built.
    pub fn set_tab_mode_refuses_writes(&self, refused: bool) {
        self.write_refusal.set_tab_mode(refused);
    }

    pub fn set_tab_local_scope_selection(&mut self, enabled: bool) {
        self.tab_local_scope_selection
            .store(enabled, Ordering::Release);
    }

    pub fn set_scope_change_callback<F>(&mut self, callback: F)
    where
        F: FnMut() + 'static,
    {
        *self
            .scope_change_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    pub fn set_scope_switch_preflight_callback<F>(&mut self, callback: F)
    where
        F: FnMut() -> Result<(), String> + 'static,
    {
        *self
            .scope_switch_preflight_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    fn setup_filter_callback(&mut self) {
        let mut tree = self.tree.clone();
        let object_cache = self.object_cache.clone();
        let pending_tree_refresh = self.pending_tree_refresh.clone();
        let current_db_type = self.current_db_type.clone();
        let status_callback = self.status_callback.clone();

        self.filter_input.set_callback(move |input| {
            let canceled_pending_refresh = {
                let mut pending = pending_tree_refresh
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let had_pending = pending.is_some();
                *pending = None;
                had_pending
            };
            let filter_text = input.value().to_lowercase();
            let cache = object_cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let db_type = *current_db_type
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            ObjectBrowserWidget::rebuild_root_categories_for_db_type(&mut tree, db_type, &cache);
            ObjectBrowserWidget::populate_tree(&mut tree, &cache, &filter_text);
            tree.redraw();
            if canceled_pending_refresh {
                ObjectBrowserWidget::emit_status_callback(
                    &status_callback,
                    "Object browser metadata refresh completed",
                );
            }
        });
    }

    fn setup_scope_choice_popup_handler(&mut self) {
        let scope_choice_menu_busy = self.scope_choice_menu_busy.clone();
        let active_scope_selector_popup = self.active_scope_selector_popup.clone();
        let mut hover_feedback = theme::HoverFeedbackState::default();

        self.scope_choice.super_handle_first(false);
        self.scope_choice.handle(move |choice, event| {
            hover_feedback.update(choice, event);
            match event {
                Event::Push if choice.active() => {
                    Self::show_scope_selector_popup(
                        choice,
                        &scope_choice_menu_busy,
                        &active_scope_selector_popup,
                    );
                    true
                }
                Event::KeyDown if choice.active() && Self::is_plain_space_key_event() => {
                    Self::show_scope_selector_popup(
                        choice,
                        &scope_choice_menu_busy,
                        &active_scope_selector_popup,
                    );
                    true
                }
                Event::Shortcut if choice.active() && Self::is_plain_space_key_event() => true,
                _ => false,
            }
        });
    }

    fn is_plain_space_key_event() -> bool {
        let blocked_modifiers = Shortcut::Shift | Shortcut::Ctrl | Shortcut::Alt | Shortcut::Meta;
        app::event_key() == Key::from_char(' ') && !app::event_state().intersects(blocked_modifiers)
    }

    fn show_scope_selector_popup(
        choice: &mut Choice,
        scope_choice_menu_busy: &Arc<AtomicBool>,
        active_scope_selector_popup: &Arc<Mutex<Option<Window>>>,
    ) {
        if scope_choice_menu_busy.load(Ordering::Acquire) {
            return;
        }

        let options = Self::scope_choice_values(choice);
        if options.is_empty() {
            return;
        }

        let busy = ScopeChoiceMenuBusyGuard::new(scope_choice_menu_busy.as_ref());
        if app::visible_focus() {
            let _ = choice.take_focus();
        }

        let initial_row = Self::scope_selector_initial_row(choice.value(), options.len());
        let Some(selected_row) = Self::run_scope_selector_popup(
            choice,
            options,
            initial_row,
            active_scope_selector_popup,
        ) else {
            return;
        };
        if choice.was_deleted() {
            return;
        }

        if choice.set_value(selected_row) {
            choice.redraw();
        }
        drop(busy);
        choice.do_callback();
    }

    fn run_scope_selector_popup(
        choice: &Choice,
        options: Vec<String>,
        initial_row: i32,
        active_scope_selector_popup: &Arc<Mutex<Option<Window>>>,
    ) -> Option<i32> {
        let parent_window = choice.top_window()?;
        let anchor_snapshot = PopupAnchorSnapshot::capture(choice);
        let row_count = options.len() as i32;
        let (popup_x, popup_y, popup_w, popup_h) =
            Self::scope_selector_popup_geometry(choice, options.len());
        let (popup_x, popup_y, popup_w, popup_h) = Self::scope_selector_fit_popup_to_parent(
            parent_window.x_root(),
            parent_window.y_root(),
            parent_window.w(),
            parent_window.h(),
            popup_x,
            popup_y,
            popup_w,
            popup_h,
        );
        let popup_h = Self::scope_selector_popup_height_for_available_height(
            popup_h,
            Self::scope_selector_requested_visible_rows(options.len()),
        );
        let (popup_x, popup_y) = Self::scope_selector_parent_relative_position(
            parent_window.x_root(),
            parent_window.y_root(),
            popup_x,
            popup_y,
        );
        let needs_scrollbar = Self::scope_selector_needs_scrollbar(popup_h, row_count);
        let scrollbar_size = Self::scope_selector_scrollbar_size();
        let list_w = Self::scope_selector_list_width(popup_w, needs_scrollbar);
        let (mut popup, mut list, mut scrollbar) = {
            let _group_guard = CurrentGroupGuard::suspend();

            parent_window.begin();
            let mut popup = Window::new(popup_x, popup_y, popup_w, popup_h, None);
            popup.set_color(theme::panel_bg());
            popup.set_border(false);

            let mut list = Frame::default().with_pos(0, 0).with_size(list_w, popup_h);
            list.set_frame(FrameType::FlatBox);
            list.set_color(theme::table_cell_bg());

            let scrollbar = if needs_scrollbar {
                let mut scrollbar =
                    Scrollbar::new(popup_w - scrollbar_size, 0, scrollbar_size, popup_h, None);
                scrollbar.set_type(ScrollbarType::Vertical);
                scrollbar.set_frame(FrameType::FlatBox);
                scrollbar.set_linesize(1);
                theme::style_scrollbar(&mut scrollbar);
                Some(scrollbar)
            } else {
                None
            };

            popup.end();
            parent_window.end();

            (popup, list, scrollbar)
        };

        let selected_result = Arc::new(Mutex::new(None::<i32>));
        let current_row = Arc::new(Mutex::new(initial_row));
        let scroll_row = Arc::new(Mutex::new(0));
        let options_for_draw = Arc::new(options);
        let text_font = choice.text_font();
        let text_size = choice.text_size();
        let cell_bg = theme::table_cell_bg();
        let selected_bg = theme::selection_soft();
        let text_color = theme::text_primary();

        let current_for_draw = current_row.clone();
        let scroll_for_draw = scroll_row.clone();
        list.draw(move |list| {
            let selected_row = *current_for_draw
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let scroll_row = *scroll_for_draw
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self::draw_scope_selector_list(
                list,
                &options_for_draw,
                selected_row,
                scroll_row,
                text_font,
                text_size,
                cell_bg,
                selected_bg,
                text_color,
            );
        });

        let selected_for_handle = selected_result.clone();
        let current_for_handle = current_row.clone();
        let scroll_for_handle = scroll_row.clone();
        let mut popup_for_handle = popup.clone();
        let mut scrollbar_for_handle = scrollbar.clone();
        list.handle(move |list, event| match event {
            Event::Focus => true,
            Event::Push => {
                let scroll_row = *scroll_for_handle
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(row) = Self::scope_selector_row_at_event(list, scroll_row, row_count) {
                    Self::select_scope_selector_row(
                        &current_for_handle,
                        &scroll_for_handle,
                        row_count,
                        Self::scope_selector_visible_row_count(list),
                        row,
                    );
                    Self::sync_scope_selector_scrollbar(
                        scrollbar_for_handle.as_mut(),
                        &scroll_for_handle,
                        row_count,
                        Self::scope_selector_visible_row_count(list),
                    );
                    list.redraw();
                }
                true
            }
            Event::Drag => {
                let scroll_row = *scroll_for_handle
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(row) = Self::scope_selector_row_at_event(list, scroll_row, row_count) {
                    Self::select_scope_selector_row(
                        &current_for_handle,
                        &scroll_for_handle,
                        row_count,
                        Self::scope_selector_visible_row_count(list),
                        row,
                    );
                    Self::sync_scope_selector_scrollbar(
                        scrollbar_for_handle.as_mut(),
                        &scroll_for_handle,
                        row_count,
                        Self::scope_selector_visible_row_count(list),
                    );
                    list.redraw();
                }
                true
            }
            Event::Released => {
                let scroll_row = *scroll_for_handle
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(row) = Self::scope_selector_row_at_event(list, scroll_row, row_count) {
                    Self::select_scope_selector_row(
                        &current_for_handle,
                        &scroll_for_handle,
                        row_count,
                        Self::scope_selector_visible_row_count(list),
                        row,
                    );
                    Self::sync_scope_selector_scrollbar(
                        scrollbar_for_handle.as_mut(),
                        &scroll_for_handle,
                        row_count,
                        Self::scope_selector_visible_row_count(list),
                    );
                    Self::accept_scope_selector_row(
                        &selected_for_handle,
                        &mut popup_for_handle,
                        row,
                    );
                    list.redraw();
                }
                true
            }
            Event::MouseWheel => {
                Self::scroll_scope_selector_list(
                    &scroll_for_handle,
                    row_count,
                    Self::scope_selector_visible_row_count(list),
                    app::event_dy_value(),
                );
                Self::sync_scope_selector_scrollbar(
                    scrollbar_for_handle.as_mut(),
                    &scroll_for_handle,
                    row_count,
                    Self::scope_selector_visible_row_count(list),
                );
                list.redraw();
                true
            }
            Event::KeyDown => {
                let key = app::event_key();
                let visible_rows = Self::scope_selector_visible_row_count(list);
                let handled =
                    if let Some(delta) = Self::scope_selector_key_move_delta(key, visible_rows) {
                        Self::move_scope_selector_row(
                            &current_for_handle,
                            &scroll_for_handle,
                            row_count,
                            visible_rows,
                            delta,
                        );
                        true
                    } else {
                        match key {
                            Key::Escape => {
                                popup_for_handle.hide();
                                true
                            }
                            Key::Enter | Key::KPEnter => {
                                let row = *current_for_handle
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                Self::accept_scope_selector_row(
                                    &selected_for_handle,
                                    &mut popup_for_handle,
                                    row,
                                );
                                true
                            }
                            Key::Home => {
                                Self::select_scope_selector_row(
                                    &current_for_handle,
                                    &scroll_for_handle,
                                    row_count,
                                    visible_rows,
                                    0,
                                );
                                true
                            }
                            Key::End => {
                                Self::select_scope_selector_row(
                                    &current_for_handle,
                                    &scroll_for_handle,
                                    row_count,
                                    visible_rows,
                                    row_count - 1,
                                );
                                true
                            }
                            _ => false,
                        }
                    };
                if handled {
                    Self::sync_scope_selector_scrollbar(
                        scrollbar_for_handle.as_mut(),
                        &scroll_for_handle,
                        row_count,
                        visible_rows,
                    );
                    list.redraw();
                }
                handled
            }
            Event::Unfocus => {
                let pointer_inside_popup = Self::scope_selector_popup_contains_root_point(
                    &popup_for_handle,
                    app::event_x_root(),
                    app::event_y_root(),
                );
                if Self::scope_selector_should_hide_on_unfocus(pointer_inside_popup, false) {
                    popup_for_handle.hide();
                    true
                } else {
                    false
                }
            }
            _ => false,
        });

        let selected_for_popup_handle = selected_result.clone();
        let current_for_popup_handle = current_row.clone();
        let scroll_for_popup_handle = scroll_row.clone();
        let mut list_for_popup_handle = list.clone();
        let mut scrollbar_for_popup_handle = scrollbar.clone();
        popup.handle(move |popup, event| match event {
            Event::KeyDown => {
                let key = app::event_key();
                let visible_rows = Self::scope_selector_visible_row_count(&list_for_popup_handle);
                let handled = if let Some(delta) =
                    Self::scope_selector_key_move_delta(key, visible_rows)
                {
                    Self::move_scope_selector_row(
                        &current_for_popup_handle,
                        &scroll_for_popup_handle,
                        row_count,
                        visible_rows,
                        delta,
                    );
                    true
                } else {
                    match key {
                        Key::Escape => {
                            popup.hide();
                            true
                        }
                        Key::Enter | Key::KPEnter => {
                            let row = *current_for_popup_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            Self::accept_scope_selector_row(&selected_for_popup_handle, popup, row);
                            true
                        }
                        Key::Home => {
                            Self::select_scope_selector_row(
                                &current_for_popup_handle,
                                &scroll_for_popup_handle,
                                row_count,
                                visible_rows,
                                0,
                            );
                            true
                        }
                        Key::End => {
                            Self::select_scope_selector_row(
                                &current_for_popup_handle,
                                &scroll_for_popup_handle,
                                row_count,
                                visible_rows,
                                row_count - 1,
                            );
                            true
                        }
                        _ => false,
                    }
                };
                if handled {
                    Self::sync_scope_selector_scrollbar(
                        scrollbar_for_popup_handle.as_mut(),
                        &scroll_for_popup_handle,
                        row_count,
                        visible_rows,
                    );
                    list_for_popup_handle.redraw();
                }
                handled
            }
            Event::Unfocus => {
                let pointer_inside_popup = Self::scope_selector_popup_contains_root_point(
                    popup,
                    app::event_x_root(),
                    app::event_y_root(),
                );
                if Self::scope_selector_should_hide_on_unfocus(pointer_inside_popup, true) {
                    popup.hide();
                    true
                } else {
                    false
                }
            }
            _ => false,
        });

        popup.show();
        let _ = popup.take_focus();
        *active_scope_selector_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(popup.clone());
        Self::select_scope_selector_row(
            &current_row,
            &scroll_row,
            row_count,
            Self::scope_selector_visible_row_count(&list),
            initial_row,
        );
        Self::sync_scope_selector_scrollbar(
            scrollbar.as_mut(),
            &scroll_row,
            row_count,
            Self::scope_selector_visible_row_count(&list),
        );
        list.redraw();
        if let Some(mut scrollbar) = scrollbar {
            let mut list_for_scrollbar = list.clone();
            let scroll_for_scrollbar = scroll_row.clone();
            scrollbar.set_callback(move |scrollbar| {
                let visible_rows = Self::scope_selector_visible_row_count(&list_for_scrollbar);
                let max_row = Self::scope_selector_max_scroll_row(row_count, visible_rows);
                let row = (scrollbar.value().round() as i32).clamp(0, max_row);
                *scroll_for_scrollbar
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = row;
                list_for_scrollbar.redraw();
            });
        }
        let _ = list.take_focus();

        while popup.shown() {
            if popup.was_deleted() || !app::wait() {
                break;
            }
            if !anchor_snapshot.is_some_and(|snapshot| snapshot.still_matches(choice)) {
                popup.hide();
                break;
            }
        }

        let popup_deleted = popup.was_deleted();
        if !popup_deleted {
            popup.hide();
        }
        *active_scope_selector_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        let selected_row = *selected_result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !popup_deleted {
            Window::delete(popup);
        }
        selected_row
    }

    fn scope_selector_initial_row(choice_value: i32, option_count: usize) -> i32 {
        if option_count == 0 {
            return 0;
        }
        choice_value.clamp(0, option_count as i32 - 1)
    }

    fn scope_selector_popup_geometry(choice: &Choice, option_count: usize) -> (i32, i32, i32, i32) {
        let anchor_x = choice
            .top_window()
            .map(|window| window.x_root())
            .unwrap_or(0)
            + choice.x();
        let anchor_y = choice
            .top_window()
            .map(|window| window.y_root())
            .unwrap_or(0)
            + choice.y();
        let screen_num = app::screen_num(anchor_x, anchor_y);
        let (screen_x, _screen_y, screen_w, screen_h) = app::screen_work_area(screen_num);

        let popup_w = Self::scope_selector_popup_width(choice.w());

        let visible_rows = Self::scope_selector_requested_visible_rows(option_count);
        let popup_h = Self::scope_selector_popup_height_for_available_height(
            (screen_h - SCOPE_SELECTOR_SCREEN_MARGIN * 2).max(1),
            visible_rows,
        );

        let min_x = screen_x + SCOPE_SELECTOR_SCREEN_MARGIN;
        let max_x = screen_x + screen_w - popup_w - SCOPE_SELECTOR_SCREEN_MARGIN;
        let popup_x = if min_x <= max_x {
            anchor_x.clamp(min_x, max_x)
        } else {
            screen_x
        };
        let popup_y = anchor_y + choice.h();

        (popup_x, popup_y, popup_w, popup_h)
    }

    fn scope_selector_parent_relative_position(
        parent_x: i32,
        parent_y: i32,
        popup_x: i32,
        popup_y: i32,
    ) -> (i32, i32) {
        (popup_x - parent_x, popup_y - parent_y)
    }

    fn scope_selector_popup_width(choice_width: i32) -> i32 {
        choice_width.max(1)
    }

    fn scope_selector_fit_popup_to_parent(
        parent_x: i32,
        _parent_y: i32,
        parent_w: i32,
        parent_h: i32,
        popup_x: i32,
        popup_y: i32,
        popup_w: i32,
        popup_h: i32,
    ) -> (i32, i32, i32, i32) {
        let popup_w = popup_w.min(parent_w.max(1)).max(1);
        let popup_h = popup_h.min(parent_h.max(1)).max(1);
        let min_x = parent_x;
        let max_x = parent_x + parent_w - popup_w;
        let popup_x = if min_x <= max_x {
            popup_x.clamp(min_x, max_x)
        } else {
            parent_x
        };
        (popup_x, popup_y, popup_w, popup_h)
    }

    fn draw_scope_selector_list(
        list: &Frame,
        options: &[String],
        selected_row: i32,
        scroll_row: i32,
        text_font: Font,
        text_size: i32,
        cell_bg: Color,
        selected_bg: Color,
        text_color: Color,
    ) {
        draw::draw_box(
            FrameType::BorderBox,
            list.x(),
            list.y(),
            list.w(),
            list.h(),
            cell_bg,
        );
        let (x, y, w, h) = Self::scope_selector_list_inner_bounds(list);
        draw::push_clip(x, y, w, h);
        draw::draw_box(FrameType::FlatBox, x, y, w, h, cell_bg);
        draw::set_font(text_font, text_size);

        let visible_rows = Self::scope_selector_visible_row_count(list);
        for offset in 0..visible_rows {
            let row = scroll_row + offset;
            let Some(text) = options.get(row as usize) else {
                break;
            };
            let row_y = y + offset * SCOPE_SELECTOR_ROW_HEIGHT;
            let bg = if row == selected_row {
                selected_bg
            } else {
                cell_bg
            };
            draw::draw_box(
                FrameType::FlatBox,
                x,
                row_y,
                w,
                SCOPE_SELECTOR_ROW_HEIGHT,
                bg,
            );
            draw::set_draw_color(text_color);
            draw::draw_text2(
                text,
                x + SCOPE_SELECTOR_TEXT_PADDING,
                row_y,
                (w - SCOPE_SELECTOR_TEXT_PADDING * 2).max(1),
                SCOPE_SELECTOR_ROW_HEIGHT,
                Align::Left,
            );
        }
        draw::pop_clip();
    }

    fn scope_selector_list_inner_bounds(list: &Frame) -> (i32, i32, i32, i32) {
        let y_padding = safe_div(SCOPE_SELECTOR_TABLE_VERTICAL_PADDING, 2);
        (
            list.x() + 1,
            list.y() + y_padding,
            (list.w() - 2).max(1),
            (list.h() - SCOPE_SELECTOR_TABLE_VERTICAL_PADDING).max(1),
        )
    }

    fn scope_selector_row_at_event(list: &Frame, scroll_row: i32, row_count: i32) -> Option<i32> {
        let (mouse_x, mouse_y) = (app::event_x(), app::event_y());
        let (x, y, w, h) = Self::scope_selector_list_inner_bounds(list);
        if mouse_x < x || mouse_x >= x + w || mouse_y < y || mouse_y >= y + h {
            return None;
        }
        let row = scroll_row + safe_div(mouse_y - y, SCOPE_SELECTOR_ROW_HEIGHT);
        (row >= 0 && row < row_count).then_some(row)
    }

    fn select_scope_selector_row(
        current_row: &Arc<Mutex<i32>>,
        scroll_row: &Arc<Mutex<i32>>,
        row_count: i32,
        visible_rows: i32,
        row: i32,
    ) {
        if row_count <= 0 {
            return;
        }
        let row = row.clamp(0, row_count - 1);
        *current_row
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = row;
        Self::ensure_scope_selector_row_visible(scroll_row, row, row_count, visible_rows);
    }

    fn move_scope_selector_row(
        current_row: &Arc<Mutex<i32>>,
        scroll_row: &Arc<Mutex<i32>>,
        row_count: i32,
        visible_rows: i32,
        delta: i32,
    ) {
        if row_count <= 0 {
            return;
        }
        let current = *current_row
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let row = (current + delta).clamp(0, row_count - 1);
        Self::select_scope_selector_row(current_row, scroll_row, row_count, visible_rows, row);
    }

    fn scope_selector_key_move_delta(key: Key, visible_rows: i32) -> Option<i32> {
        match key {
            Key::Up => Some(-1),
            Key::Down => Some(1),
            Key::PageUp => Some(-visible_rows.max(1)),
            Key::PageDown => Some(visible_rows.max(1)),
            _ => None,
        }
    }

    fn ensure_scope_selector_row_visible(
        scroll_row: &Arc<Mutex<i32>>,
        row: i32,
        row_count: i32,
        visible_rows: i32,
    ) {
        let max_row = Self::scope_selector_max_scroll_row(row_count, visible_rows);
        let mut top_row = scroll_row
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if row < *top_row {
            *top_row = row.clamp(0, max_row);
        } else if row >= *top_row + visible_rows {
            *top_row = (row - visible_rows + 1).clamp(0, max_row);
        }
    }

    fn scope_selector_visible_row_count(list: &Frame) -> i32 {
        Self::scope_selector_visible_rows_for_height(list.h())
    }

    fn scope_selector_visible_rows_for_height(height: i32) -> i32 {
        safe_div(
            (height - SCOPE_SELECTOR_TABLE_VERTICAL_PADDING).max(1),
            SCOPE_SELECTOR_ROW_HEIGHT,
        )
        .max(1)
    }

    fn scope_selector_requested_visible_rows(option_count: usize) -> i32 {
        option_count.clamp(1, SCOPE_SELECTOR_MAX_VISIBLE_ROWS) as i32
    }

    fn scope_selector_popup_height_for_rows(visible_rows: i32) -> i32 {
        visible_rows.max(1) * SCOPE_SELECTOR_ROW_HEIGHT + SCOPE_SELECTOR_TABLE_VERTICAL_PADDING
    }

    fn scope_selector_popup_height_for_available_height(
        available_height: i32,
        requested_visible_rows: i32,
    ) -> i32 {
        let visible_rows = requested_visible_rows
            .min(Self::scope_selector_visible_rows_for_height(
                available_height,
            ))
            .max(1);
        Self::scope_selector_popup_height_for_rows(visible_rows).min(available_height.max(1))
    }

    fn scope_selector_needs_scrollbar(popup_h: i32, row_count: i32) -> bool {
        row_count > Self::scope_selector_visible_rows_for_height(popup_h)
    }

    fn scope_selector_scrollbar_size() -> i32 {
        app::scrollbar_size().max(1)
    }

    fn scope_selector_list_width(popup_w: i32, needs_scrollbar: bool) -> i32 {
        if needs_scrollbar {
            (popup_w - Self::scope_selector_scrollbar_size()).max(1)
        } else {
            popup_w.max(1)
        }
    }

    fn scope_selector_max_scroll_row(row_count: i32, visible_rows: i32) -> i32 {
        (row_count - visible_rows).max(0)
    }

    fn sync_scope_selector_scrollbar(
        scrollbar: Option<&mut Scrollbar>,
        scroll_row: &Arc<Mutex<i32>>,
        row_count: i32,
        visible_rows: i32,
    ) {
        let Some(scrollbar) = scrollbar else {
            return;
        };

        let visible_rows = visible_rows.min(row_count).max(1);
        let max_row = Self::scope_selector_max_scroll_row(row_count, visible_rows);
        let row_position = scroll_row
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clamp(0, max_row);
        scrollbar.scroll_value(row_position, visible_rows, 0, row_count.max(1));
        scrollbar.redraw();
    }

    fn scroll_scope_selector_list(
        scroll_row: &Arc<Mutex<i32>>,
        row_count: i32,
        visible_rows: i32,
        row_delta: i32,
    ) {
        if row_delta == 0 {
            return;
        }

        let max_row = Self::scope_selector_max_scroll_row(row_count, visible_rows);
        let mut row = scroll_row
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *row = (*row + row_delta).clamp(0, max_row);
    }

    fn accept_scope_selector_row(
        selected_result: &Arc<Mutex<Option<i32>>>,
        popup: &mut Window,
        row: i32,
    ) {
        *selected_result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(row);
        popup.hide();
    }

    fn scope_selector_popup_contains_root_point(popup: &Window, x: i32, y: i32) -> bool {
        let popup_x = popup.x_root();
        let popup_y = popup.y_root();
        x >= popup_x && x < popup_x + popup.w() && y >= popup_y && y < popup_y + popup.h()
    }

    fn scope_selector_should_hide_on_unfocus(
        pointer_inside_popup: bool,
        popup_window_lost_focus: bool,
    ) -> bool {
        popup_window_lost_focus || !pointer_inside_popup
    }

    fn scope_selector_should_hide_on_pointer_push(pointer_inside_popup: bool) -> bool {
        !pointer_inside_popup
    }

    pub fn hide_scope_selector_popup_if_outside(&self, root_x: i32, root_y: i32) {
        let active_popup = self
            .active_scope_selector_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(mut popup) = active_popup else {
            return;
        };

        if popup.was_deleted() || !popup.shown() {
            *self
                .active_scope_selector_popup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            return;
        }

        let pointer_inside_popup =
            Self::scope_selector_popup_contains_root_point(&popup, root_x, root_y);
        if Self::scope_selector_should_hide_on_pointer_push(pointer_inside_popup) {
            popup.hide();
        }
    }

    fn setup_scope_callback(&mut self) {
        let connection = self.connection.clone();
        let current_db_type = self.current_db_type.clone();
        let selected_scope = self.selected_scope.clone();
        let catalog = self.catalog.clone();
        let suppress_scope_events = self.suppress_scope_events.clone();
        let status_callback = self.status_callback.clone();
        let scope_change_callback = self.scope_change_callback.clone();
        let scope_switch_preflight_callback = self.scope_switch_preflight_callback.clone();
        let scope_generation = self.scope_generation.clone();
        let scope_switch_in_progress = self.scope_switch_in_progress.clone();
        let tab_local_scope_selection = self.tab_local_scope_selection.clone();
        let refresh_connection_generation = self.refresh_connection_generation.clone();
        let action_sender = self.action_sender.clone();

        self.scope_choice.set_callback(move |choice| {
            let suppressed = *suppress_scope_events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if suppressed {
                return;
            }

            let next_scope = choice.choice().map(|value| value.trim().to_string());
            let next_scope = next_scope.filter(|value| !value.is_empty());
            let previous_scope = selected_scope
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();

            let db_type = *current_db_type
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());

            if Self::scope_values_match_for_db_type(
                db_type,
                next_scope.as_deref(),
                previous_scope.as_deref(),
            ) {
                return;
            }

            if db_type.has_connection_scope() {
                let preflight_result =
                    Self::invoke_scope_switch_preflight_callback(&scope_switch_preflight_callback);
                if let Err(err) = preflight_result {
                    Self::restore_previous_scope_choice(
                        choice,
                        &suppress_scope_events,
                        db_type,
                        previous_scope.as_deref(),
                    );
                    Self::emit_status_callback(&status_callback, &err);
                    crate::ui::alert_on_main(&err);
                    return;
                }
            }

            if db_type.has_connection_scope() && !tab_local_scope_selection.load(Ordering::Acquire)
            {
                let Some(target_scope) = next_scope else {
                    return;
                };
                if scope_switch_in_progress.swap(true, Ordering::AcqRel) {
                    Self::restore_previous_scope_choice(
                        choice,
                        &suppress_scope_events,
                        db_type,
                        previous_scope.as_deref(),
                    );
                    Self::emit_status_callback(
                        &status_callback,
                        "Scope switch already in progress",
                    );
                    return;
                }
                choice.deactivate();
                let generation = scope_generation
                    .fetch_add(1, Ordering::Relaxed)
                    .wrapping_add(1);
                let activity = Self::scope_switch_activity_message(db_type, &target_scope);
                Self::emit_status_callback(&status_callback, &activity);
                let expected_connection_generation =
                    refresh_connection_generation.load(Ordering::Acquire);
                let connection = connection.clone();
                let sender = action_sender.clone();
                thread::spawn(move || {
                    let result = panic::catch_unwind(AssertUnwindSafe(|| {
                        let mut conn_guard =
                            lock_connection_with_activity(&connection, activity.clone());
                        let result = if conn_guard.connection_generation()
                            != expected_connection_generation
                            || !conn_guard.db_type().is_same_type_as(db_type)
                        {
                            Err("Scope switch was superseded by a connection change.".to_string())
                        } else {
                            crate::db::clear_pool_session_context_for_shared_connection(
                                &connection,
                            );
                            conn_guard.switch_scope(&target_scope)
                        };
                        if result.is_ok() {
                            crate::db::refresh_pool_session_context_cache_for_shared_connection(
                                &connection,
                                &conn_guard,
                            );
                        }
                        result
                    }))
                    .unwrap_or_else(|payload| {
                        let panic_msg = Self::panic_payload_to_string(payload.as_ref());
                        crate::utils::logging::log_error(
                            "object_browser::scope_switch",
                            &format!("scope switch worker panicked: {panic_msg}"),
                        );
                        eprintln!("scope switch worker panicked: {panic_msg}");
                        Err(format!("Scope switch failed internally: {panic_msg}"))
                    });

                    let _ = sender.send(ObjectActionResult::ScopeSwitchFinished {
                        db_type,
                        target_scope,
                        previous_scope,
                        generation,
                        result,
                    });
                    app::awake();
                });
                return;
            }

            scope_generation.fetch_add(1, Ordering::Relaxed);
            Self::complete_scope_change(
                &selected_scope,
                &catalog,
                &status_callback,
                &scope_change_callback,
                db_type,
                next_scope,
            );
        });
    }

    fn setup_refresh_handler(&mut self, refresh_receiver: std::sync::mpsc::Receiver<RefreshEvent>) {
        let tree = self.tree.clone();
        let object_cache = self.object_cache.clone();
        let current_db_type = self.current_db_type.clone();
        let scope_label = self.scope_label.clone();
        let scope_choice = self.scope_choice.clone();
        let scope_options = self.scope_options.clone();
        let selected_scope = self.selected_scope.clone();
        let suppress_scope_events = self.suppress_scope_events.clone();
        let scope_choice_menu_busy = self.scope_choice_menu_busy.clone();
        let scope_generation = self.scope_generation.clone();
        let scope_switch_in_progress = self.scope_switch_in_progress.clone();
        let refresh_connection_generation = self.refresh_connection_generation.clone();
        let filter_input = self.filter_input.clone();
        let pending_tree_refresh = self.pending_tree_refresh.clone();
        let catalog = self.catalog.clone();
        let metadata_callback = self.metadata_callback.clone();

        let lifecycle = Arc::downgrade(&self.poll_lifecycle);

        // Wrap receiver in Arc<Mutex> to share across timeout callbacks
        let receiver: Arc<Mutex<std::sync::mpsc::Receiver<RefreshEvent>>> =
            Arc::new(Mutex::new(refresh_receiver));

        fn schedule_poll(
            receiver: Arc<Mutex<Receiver<RefreshEvent>>>,
            mut tree: Tree,
            object_cache: Arc<Mutex<ObjectCache>>,
            current_db_type: Arc<Mutex<crate::db::DatabaseType>>,
            mut scope_label: Frame,
            mut scope_choice: Choice,
            scope_options: Arc<Mutex<Vec<String>>>,
            selected_scope: Arc<Mutex<Option<String>>>,
            suppress_scope_events: Arc<Mutex<bool>>,
            scope_choice_menu_busy: Arc<AtomicBool>,
            scope_generation: Arc<AtomicU64>,
            scope_switch_in_progress: Arc<AtomicBool>,
            refresh_connection_generation: Arc<AtomicU64>,
            filter_input: Input,
            pending_tree_refresh: Arc<Mutex<Option<PendingTreeRefresh>>>,
            catalog: CardCatalogState,
            metadata_callback: MetadataCallback,
            status_callback: StatusCallback,
            lifecycle: Weak<()>,
        ) {
            if lifecycle.upgrade().is_none() {
                return;
            }

            if tree.was_deleted() || filter_input.was_deleted() || scope_choice.was_deleted() {
                return;
            }

            let mut disconnected = false;
            // Keep receiver lock scope minimal: drain messages first, then perform UI work.
            // This prevents long lock hold while rebuilding tree widgets.
            let mut latest_cache: Option<(
                crate::db::DatabaseType,
                ObjectCache,
                Vec<String>,
                Option<String>,
                u64,
                crate::db::DbActivityGuard,
            )> = None;
            let mut latest_failure: Option<(String, crate::db::DbActivityGuard)> = None;

            {
                let r = receiver
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                loop {
                    match r.try_recv() {
                        Ok(RefreshEvent::Finished {
                            cache,
                            db_type,
                            available_scopes,
                            selected_scope,
                            scope_generation: event_scope_generation,
                            connection_generation: event_connection_generation,
                            activity_guard,
                        }) => {
                            if !ObjectBrowserWidget::refresh_result_matches_generations(
                                event_scope_generation,
                                scope_generation.load(Ordering::Relaxed),
                                event_connection_generation,
                                refresh_connection_generation.load(Ordering::Relaxed),
                            ) {
                                continue;
                            }
                            let cache = *cache;
                            latest_cache = Some((
                                db_type,
                                cache,
                                available_scopes,
                                selected_scope,
                                event_connection_generation,
                                activity_guard,
                            ));
                            latest_failure = None;
                            match current_db_type.lock() {
                                Ok(mut guard) => *guard = db_type,
                                Err(poisoned) => *poisoned.into_inner() = db_type,
                            }
                        }
                        Ok(RefreshEvent::Failed {
                            message,
                            scope_generation: event_scope_generation,
                            connection_generation: event_connection_generation,
                            activity_guard,
                        }) => {
                            if !ObjectBrowserWidget::refresh_result_matches_generations(
                                event_scope_generation,
                                scope_generation.load(Ordering::Relaxed),
                                event_connection_generation,
                                refresh_connection_generation.load(Ordering::Relaxed),
                            ) {
                                continue;
                            }
                            latest_cache = None;
                            latest_failure = Some((message, activity_guard));
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            disconnected = true;
                            break;
                        }
                    }
                }
            }

            if let Some((
                db_type,
                cache,
                available_scopes,
                resolved_scope,
                connection_generation,
                activity_guard,
            )) = latest_cache
            {
                let filter_text = filter_input.value().to_lowercase();
                let paths = ObjectBrowserWidget::collect_tree_paths(&cache, &filter_text);
                let cache_snapshot = cache.clone();

                {
                    let mut cache_guard = object_cache
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    *cache_guard = cache;
                }
                // This card now holds a real catalog, so a new card may
                // inherit it instead of loading again.
                catalog.catalog_arrived();

                ObjectBrowserWidget::rebuild_root_categories_for_db_type(
                    &mut tree,
                    db_type,
                    &cache_snapshot,
                );
                scope_label.set_label(ObjectBrowserWidget::scope_label_text(db_type));
                {
                    let mut options_guard = scope_options
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    *options_guard = available_scopes.clone();
                }
                {
                    let mut selected_guard = selected_scope
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    *selected_guard = resolved_scope.clone();
                }
                let snapshot = ObjectBrowserMetadataSnapshot::from_cache(
                    db_type,
                    connection_generation,
                    available_scopes.clone(),
                    resolved_scope.clone(),
                    &cache_snapshot,
                );
                ObjectBrowserWidget::emit_metadata_callback(&metadata_callback, snapshot);
                // The delivery runs application code that can open a modal,
                // and a modal pumps the timers that tear cards down. Anything
                // below touches this card's widgets, so re-check them.
                if tree.was_deleted() || scope_choice.was_deleted() {
                    return;
                }
                ObjectBrowserWidget::clear_tree_items(&mut tree);
                activity_guard.set_activity("Applying object browser metadata");
                activity_guard.set_progress(crate::db::DbActivityProgress::Determinate {
                    completed: 0,
                    total: paths.len() as u64,
                });
                {
                    let mut pending = pending_tree_refresh
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    *pending = Some(PendingTreeRefresh {
                        paths,
                        next_index: 0,
                        activity_guard,
                    });
                }
            } else if let Some((message, _activity_guard)) = latest_failure {
                {
                    let mut pending = pending_tree_refresh
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    *pending = None;
                }
                ObjectBrowserWidget::emit_status_callback(&status_callback, &message);
            }

            // Only the card on screen needs its selector kept in step. This
            // runs 20 times a second per card, and reading the menu back to
            // decide whether it changed costs one FFI call and one String per
            // item — with a card per editor tab and a server full of
            // databases, doing that for hidden cards is pure waste. A card
            // that becomes visible is synced by its next tick.
            if scope_choice.visible_r() {
                let desired_scopes = scope_options
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                let desired_scope = selected_scope
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                ObjectBrowserWidget::sync_scope_choice_widget(
                    &mut scope_choice,
                    &suppress_scope_events,
                    &scope_choice_menu_busy,
                    *current_db_type
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()),
                    &desired_scopes,
                    desired_scope.as_deref(),
                    scope_switch_in_progress.load(Ordering::Acquire),
                );
            }

            let mut next_paths = Vec::new();
            let mut finished_refresh = false;
            {
                let mut pending = pending_tree_refresh
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(task) = pending.as_mut() {
                    let end = task
                        .next_index
                        .saturating_add(REFRESH_TREE_BATCH_SIZE)
                        .min(task.paths.len());
                    if task.next_index < end {
                        next_paths.extend(task.paths[task.next_index..end].iter().cloned());
                        task.next_index = end;
                        task.activity_guard.set_progress(
                            crate::db::DbActivityProgress::Determinate {
                                completed: task.next_index as u64,
                                total: task.paths.len() as u64,
                            },
                        );
                    }
                    if task.next_index >= task.paths.len() {
                        *pending = None;
                        finished_refresh = true;
                    }
                }
            }

            if !next_paths.is_empty() {
                for path in next_paths {
                    tree.add(&path);
                }
                tree.redraw();
            }

            if finished_refresh {
                tree.redraw();
                ObjectBrowserWidget::emit_status_callback(
                    &status_callback,
                    "Object browser metadata refresh completed",
                );
            }

            if disconnected {
                return;
            }

            // Reschedule for next poll
            crate::ui::ui_timeout::schedule(0.05, move || {
                schedule_poll(
                    receiver.clone(),
                    tree.clone(),
                    object_cache.clone(),
                    current_db_type.clone(),
                    scope_label.clone(),
                    scope_choice.clone(),
                    scope_options.clone(),
                    selected_scope.clone(),
                    suppress_scope_events.clone(),
                    scope_choice_menu_busy.clone(),
                    scope_generation.clone(),
                    scope_switch_in_progress.clone(),
                    refresh_connection_generation.clone(),
                    filter_input.clone(),
                    pending_tree_refresh.clone(),
                    catalog.clone(),
                    metadata_callback.clone(),
                    status_callback.clone(),
                    lifecycle.clone(),
                );
            });
        }

        // Start polling
        schedule_poll(
            receiver,
            tree,
            object_cache,
            current_db_type,
            scope_label,
            scope_choice,
            scope_options,
            selected_scope,
            suppress_scope_events,
            scope_choice_menu_busy,
            scope_generation,
            scope_switch_in_progress,
            refresh_connection_generation,
            filter_input,
            pending_tree_refresh,
            catalog,
            metadata_callback,
            self.status_callback.clone(),
            lifecycle,
        );
    }

    fn setup_action_handler(
        &mut self,
        action_receiver: std::sync::mpsc::Receiver<ObjectActionResult>,
    ) {
        let sql_callback = self.sql_callback.clone();
        let status_callback = self.status_callback.clone();
        let tree = self.tree.clone();
        let object_cache = self.object_cache.clone();
        let filter_input = self.filter_input.clone();
        let connection = self.connection.clone();
        let current_db_type = self.current_db_type.clone();
        let write_refusal = self.write_refusal.clone();
        let action_sender = self.action_sender.clone();
        let selected_scope = self.selected_scope.clone();
        let catalog = self.catalog.clone();
        let scope_change_callback = self.scope_change_callback.clone();
        let scope_choice = self.scope_choice.clone();
        let suppress_scope_events = self.suppress_scope_events.clone();
        let scope_generation = self.scope_generation.clone();
        let scope_switch_in_progress = self.scope_switch_in_progress.clone();
        let lifecycle = Arc::downgrade(&self.poll_lifecycle);

        let receiver: Arc<Mutex<std::sync::mpsc::Receiver<ObjectActionResult>>> =
            Arc::new(Mutex::new(action_receiver));

        fn schedule_poll(
            receiver: Arc<Mutex<std::sync::mpsc::Receiver<ObjectActionResult>>>,
            sql_callback: SqlExecuteCallback,
            status_callback: StatusCallback,
            mut tree: Tree,
            object_cache: Arc<Mutex<ObjectCache>>,
            filter_input: Input,
            connection: SharedConnection,
            current_db_type: Arc<Mutex<crate::db::DatabaseType>>,
            write_refusal: CardWriteRefusal,
            action_sender: std::sync::mpsc::Sender<ObjectActionResult>,
            selected_scope: Arc<Mutex<Option<String>>>,
            catalog: CardCatalogState,
            scope_change_callback: ScopeChangeCallback,
            mut scope_choice: Choice,
            suppress_scope_events: Arc<Mutex<bool>>,
            scope_generation: Arc<AtomicU64>,
            scope_switch_in_progress: Arc<AtomicBool>,
            lifecycle: Weak<()>,
        ) {
            if lifecycle.upgrade().is_none() {
                return;
            }

            if tree.was_deleted() || filter_input.was_deleted() || scope_choice.was_deleted() {
                return;
            }

            let mut disconnected = false;
            loop {
                // Re-check every round, not just once at the top: handling a
                // message can open an alert or a context menu, and those pump
                // the FLTK loop — which dispatches the deferred tab-close
                // timer, which deletes this card's widgets. The next message
                // would then draw into freed memory.
                if tree.was_deleted() || filter_input.was_deleted() || scope_choice.was_deleted() {
                    return;
                }
                let message = {
                    let r = receiver
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    r.try_recv()
                };

                match message {
                    Ok(action) => match action {
                        ObjectActionResult::TableStructure { table_name, result } => match result {
                            Ok(columns) => {
                                ObjectBrowserWidget::emit_sql_callback(
                                    &sql_callback,
                                    SqlAction::DisplayResult(
                                        ObjectBrowserWidget::build_table_structure_result_request(
                                            &table_name,
                                            &columns,
                                        ),
                                    ),
                                );
                            }
                            Err(err) => {
                                crate::ui::alert_on_main(&format!(
                                    "Failed to get table structure: {}",
                                    err
                                ));
                            }
                        },
                        ObjectActionResult::ExportedTable {
                            qualified_name,
                            db_type,
                            choice,
                            result,
                        } => match result {
                            Ok(query_result) => {
                                let delivery = ObjectBrowserWidget::render_table_export(
                                    &qualified_name,
                                    db_type,
                                    choice,
                                    &query_result,
                                );
                                ObjectBrowserWidget::emit_status_callback(
                                    &status_callback,
                                    &format!(
                                        "Exporting {qualified_name}: {} rows as {}",
                                        delivery.row_count,
                                        choice.format.label()
                                    ),
                                );
                                ObjectBrowserWidget::emit_sql_callback(
                                    &sql_callback,
                                    SqlAction::ExportData(delivery),
                                );
                            }
                            Err(err) => {
                                crate::ui::alert_on_main(&format!(
                                    "Failed to read {qualified_name} for export: {err}"
                                ));
                            }
                        },
                        ObjectActionResult::ImportTarget {
                            qualified_name,
                            file_label,
                            db_type,
                            format,
                            result,
                        } => match result {
                            Ok((text, columns)) => {
                                if let Some((sql, summary)) =
                                    ObjectBrowserWidget::build_import_script_from_dialog(
                                        &file_label,
                                        &text,
                                        &qualified_name,
                                        db_type,
                                        &columns,
                                        format,
                                    )
                                {
                                    ObjectBrowserWidget::emit_status_callback(
                                        &status_callback,
                                        &format!("Importing {file_label}: {summary}"),
                                    );
                                    ObjectBrowserWidget::emit_sql_callback(
                                        &sql_callback,
                                        SqlAction::ExecuteScript(sql),
                                    );
                                } else {
                                    ObjectBrowserWidget::emit_status_callback(
                                        &status_callback,
                                        &format!("Cancelled: {} was not imported", file_label),
                                    );
                                }
                            }
                            Err(err) => {
                                crate::ui::alert_on_main(&format!(
                                    "Failed to prepare the import: {}",
                                    err
                                ));
                            }
                        },
                        ObjectActionResult::TableIndexes { table_name, result } => match result {
                            Ok(indexes) => {
                                ObjectBrowserWidget::emit_sql_callback(
                                    &sql_callback,
                                    SqlAction::DisplayResult(
                                        ObjectBrowserWidget::build_table_indexes_result_request(
                                            &table_name,
                                            &indexes,
                                        ),
                                    ),
                                );
                            }
                            Err(err) => {
                                crate::ui::alert_on_main(&format!(
                                    "Failed to get indexes: {}",
                                    err
                                ));
                            }
                        },
                        ObjectActionResult::TableConstraints { table_name, result } => match result
                        {
                            Ok(constraints) => {
                                ObjectBrowserWidget::emit_sql_callback(
                                    &sql_callback,
                                    SqlAction::DisplayResult(
                                        ObjectBrowserWidget::build_table_constraints_result_request(
                                            &table_name,
                                            &constraints,
                                        ),
                                    ),
                                );
                            }
                            Err(err) => {
                                crate::ui::alert_on_main(&format!(
                                    "Failed to get constraints: {}",
                                    err
                                ));
                            }
                        },
                        ObjectActionResult::SequenceInfo(result) => match result {
                            Ok(info) => {
                                ObjectBrowserWidget::emit_sql_callback(
                                    &sql_callback,
                                    SqlAction::DisplayResult(
                                        ObjectBrowserWidget::build_sequence_info_result_request(
                                            &info,
                                        ),
                                    ),
                                );
                            }
                            Err(err) => {
                                crate::ui::alert_on_main(&format!(
                                    "Failed to get sequence info: {}",
                                    err
                                ));
                            }
                        },
                        ObjectActionResult::SynonymInfo(result) => match result {
                            Ok(info) => {
                                ObjectBrowserWidget::emit_sql_callback(
                                    &sql_callback,
                                    SqlAction::DisplayResult(
                                        ObjectBrowserWidget::build_synonym_info_result_request(
                                            &info,
                                        ),
                                    ),
                                );
                            }
                            Err(err) => {
                                crate::ui::alert_on_main(&format!(
                                    "Failed to get synonym info: {}",
                                    err
                                ));
                            }
                        },
                        ObjectActionResult::Ddl(result) => match result {
                            Ok(ddl) => {
                                ObjectBrowserWidget::emit_sql_callback(
                                    &sql_callback,
                                    SqlAction::OpenInNewTab(ddl),
                                );
                            }
                            Err(err) => {
                                crate::ui::alert_on_main(&format!(
                                    "Failed to generate DDL: {}",
                                    err
                                ));
                            }
                        },
                        ObjectActionResult::RoutineScript {
                            qualified_name,
                            routine_type,
                            db_type,
                            result,
                        } => {
                            let sql = match result {
                                Ok(sql) => Some(sql),
                                Err(err) => {
                                    crate::ui::alert_on_main(&format!(
                                        "Failed to load routine arguments: {}",
                                        err
                                    ));
                                    if routine_type.eq_ignore_ascii_case("UNKNOWN") {
                                        None
                                    } else {
                                        Some(
                                            ObjectBrowserWidget::build_simple_routine_script_for_db(
                                                db_type,
                                                &qualified_name,
                                                &routine_type,
                                            ),
                                        )
                                    }
                                }
                            };
                            if let Some(sql) = sql {
                                ObjectBrowserWidget::emit_sql_callback(
                                    &sql_callback,
                                    SqlAction::OpenInNewTab(sql),
                                );
                            }
                        }
                        ObjectActionResult::TableColumns {
                            table_name,
                            result,
                            scope_generation: action_scope_generation,
                        } => {
                            if action_scope_generation != scope_generation.load(Ordering::Relaxed) {
                                continue;
                            }
                            match result {
                                Ok(columns) => {
                                    let open_paths = ObjectBrowserWidget::open_tree_paths(&tree);
                                    let mut cache = object_cache
                                        .lock()
                                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                                    cache.table_columns.insert(table_name.clone(), columns);
                                    let filter_text = filter_input.value().to_lowercase();
                                    ObjectBrowserWidget::populate_tree(
                                        &mut tree,
                                        &cache,
                                        &filter_text,
                                    );
                                    drop(cache);
                                    ObjectBrowserWidget::restore_tree_open_paths(
                                        &mut tree,
                                        &open_paths,
                                    );
                                    // The node the user pressed Right on is the
                                    // one that should now be open.
                                    if let Some(mut item) =
                                        tree.find_item(&format!("Tables/{}", table_name))
                                    {
                                        item.open();
                                    }
                                    tree.redraw();
                                }
                                Err(err) => {
                                    ObjectBrowserWidget::emit_status_callback(
                                        &status_callback,
                                        &format!("Failed to load columns for {table_name}: {err}"),
                                    );
                                }
                            }
                        }
                        ObjectActionResult::PackageRoutines {
                            package_name,
                            result,
                            scope_generation: action_scope_generation,
                            select_first_child_after_load,
                        } => {
                            if action_scope_generation != scope_generation.load(Ordering::Relaxed) {
                                continue;
                            }
                            match result {
                                Ok(routines) => {
                                    let package_open_paths =
                                        ObjectBrowserWidget::open_tree_paths(&tree);
                                    let mut cache = object_cache
                                        .lock()
                                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                                    cache
                                        .package_routines
                                        .insert(package_name.clone(), routines);
                                    let filter_text = filter_input.value().to_lowercase();
                                    ObjectBrowserWidget::populate_tree(
                                        &mut tree,
                                        &cache,
                                        &filter_text,
                                    );
                                    ObjectBrowserWidget::restore_tree_open_paths(
                                        &mut tree,
                                        &package_open_paths,
                                    );
                                    if select_first_child_after_load {
                                        if let Some(item) =
                                            tree.find_item(&format!("Packages/{}", package_name))
                                        {
                                            ObjectBrowserWidget::select_first_child_item(
                                                &mut tree, &item,
                                            );
                                        }
                                    }
                                    tree.redraw();
                                }
                                Err(err) => {
                                    crate::ui::alert_on_main(&format!(
                                        "Failed to load package routines: {}",
                                        err
                                    ));
                                }
                            }
                        }
                        ObjectActionResult::PackageRoutineContextMenu {
                            mut item,
                            db_type,
                            selected_scope,
                            package_name,
                            result,
                            mouse_x,
                            mouse_y,
                            scope_generation: action_scope_generation,
                        } => {
                            if action_scope_generation != scope_generation.load(Ordering::Relaxed) {
                                continue;
                            }
                            let mut show_menu = false;
                            match result {
                                Ok(routines) => {
                                    ObjectBrowserWidget::apply_package_routine_type_from_routines(
                                        &mut item, &routines,
                                    );
                                    show_menu =
                                        ObjectBrowserWidget::package_routine_type_is_resolved(
                                            &item,
                                        );
                                    let mut cache = object_cache
                                        .lock()
                                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                                    cache.package_routines.insert(package_name, routines);
                                    if !show_menu {
                                        ObjectBrowserWidget::emit_status_callback(
                                            &status_callback,
                                            "Could not resolve package routine type",
                                        );
                                    }
                                }
                                Err(err) => {
                                    ObjectBrowserWidget::emit_status_callback(
                                        &status_callback,
                                        &format!("Could not resolve package routine type: {}", err),
                                    );
                                }
                            }

                            if show_menu {
                                let _ = ObjectBrowserWidget::show_context_menu_for_object_item_at(
                                    &connection,
                                    &current_db_type,
                                    &write_refusal,
                                    item,
                                    &sql_callback,
                                    &status_callback,
                                    &action_sender,
                                    selected_scope,
                                    mouse_x,
                                    mouse_y,
                                );
                            } else {
                                let _ =
                                    ObjectBrowserWidget::show_unresolved_package_routine_menu_at(
                                        &item,
                                        &status_callback,
                                        db_type,
                                        selected_scope.as_deref(),
                                        mouse_x,
                                        mouse_y,
                                    );
                            }
                        }
                        ObjectActionResult::ScopeSwitchFinished {
                            db_type,
                            target_scope,
                            previous_scope,
                            generation,
                            result,
                        } => {
                            if generation != scope_generation.load(Ordering::Relaxed) {
                                continue;
                            }
                            scope_switch_in_progress.store(false, Ordering::Release);

                            match result {
                                Ok(()) => {
                                    ObjectBrowserWidget::complete_scope_change(
                                        &selected_scope,
                                        &catalog,
                                        &status_callback,
                                        &scope_change_callback,
                                        db_type,
                                        Some(target_scope),
                                    );
                                    ObjectBrowserWidget::apply_scope_choice_enabled_state(
                                        &mut scope_choice,
                                        false,
                                    );
                                }
                                Err(err) => {
                                    ObjectBrowserWidget::restore_previous_scope_choice(
                                        &mut scope_choice,
                                        &suppress_scope_events,
                                        db_type,
                                        previous_scope.as_deref(),
                                    );
                                    ObjectBrowserWidget::emit_status_callback(
                                        &status_callback,
                                        &err,
                                    );
                                    crate::ui::alert_on_main(
                                        &ObjectBrowserWidget::scope_switch_failure_message(
                                            db_type,
                                            &target_scope,
                                            &err,
                                        ),
                                    );
                                    ObjectBrowserWidget::apply_scope_choice_enabled_state(
                                        &mut scope_choice,
                                        false,
                                    );
                                }
                            }
                        }
                        ObjectActionResult::CompilationErrors {
                            object_name,
                            object_type,
                            status,
                            result,
                        } => match result {
                            Ok(errors) => {
                                ObjectBrowserWidget::emit_sql_callback(
                                    &sql_callback,
                                    SqlAction::DisplayResult(
                                        ObjectBrowserWidget::build_compilation_result_request(
                                            &object_name,
                                            &object_type,
                                            &status,
                                            &errors,
                                        ),
                                    ),
                                );
                            }
                            Err(err) => {
                                crate::ui::alert_on_main(&format!(
                                    "Failed to check compilation status: {}",
                                    err
                                ));
                            }
                        },
                    },
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }

            if disconnected {
                return;
            }

            crate::ui::ui_timeout::schedule(0.05, move || {
                schedule_poll(
                    receiver.clone(),
                    sql_callback.clone(),
                    status_callback.clone(),
                    tree.clone(),
                    object_cache.clone(),
                    filter_input.clone(),
                    connection.clone(),
                    current_db_type.clone(),
                    write_refusal.clone(),
                    action_sender.clone(),
                    selected_scope.clone(),
                    catalog.clone(),
                    scope_change_callback.clone(),
                    scope_choice.clone(),
                    suppress_scope_events.clone(),
                    scope_generation.clone(),
                    scope_switch_in_progress.clone(),
                    lifecycle.clone(),
                );
            });
        }

        schedule_poll(
            receiver,
            sql_callback,
            status_callback,
            tree,
            object_cache,
            filter_input,
            connection,
            current_db_type,
            write_refusal,
            action_sender,
            selected_scope,
            catalog,
            scope_change_callback,
            scope_choice,
            suppress_scope_events,
            scope_generation,
            scope_switch_in_progress,
            lifecycle,
        );
    }

    fn setup_callbacks(&mut self) {
        let connection = self.connection.clone();
        let sql_callback = self.sql_callback.clone();
        let status_callback = self.status_callback.clone();
        let action_sender = self.action_sender.clone();
        let object_cache = self.object_cache.clone();
        let current_db_type = self.current_db_type.clone();
        let write_refusal = self.write_refusal.clone();
        let selected_scope = self.selected_scope.clone();
        let scope_generation = self.scope_generation.clone();
        let mut pending_drag_text: Option<String> = None;
        // FLTK routes KeyUp to whatever holds focus at that moment, which is not
        // necessarily the widget that received the matching KeyDown (Fl.cxx
        // documents this). Only act on a KeyUp whose KeyDown this tree owned.
        let mut owned_keydown: Option<Key> = None;

        self.tree.handle(move |t, ev| {
            if !t.active() {
                return false;
            }
            match ev {
                Event::Push => {
                    let mouse_button = fltk::app::event_button();
                    if mouse_button == fltk::app::MouseButton::Right as i32 {
                        let clicked_item = t
                            .find_clicked(false)
                            .or_else(|| t.find_clicked(true))
                            .or_else(|| Self::item_at_mouse(t));

                        if let Some(item) = clicked_item {
                            let _ = t.select_only(&item, false);
                            t.set_item_focus(&item);
                            Self::show_context_menu(
                                &connection,
                                &current_db_type,
                                &write_refusal,
                                &item,
                                &sql_callback,
                                &status_callback,
                                &action_sender,
                                &selected_scope,
                                &object_cache,
                            );
                        } else if let Some(item) = t.first_selected_item() {
                            Self::show_context_menu(
                                &connection,
                                &current_db_type,
                                &write_refusal,
                                &item,
                                &sql_callback,
                                &status_callback,
                                &action_sender,
                                &selected_scope,
                                &object_cache,
                            );
                        }
                        return true;
                    }

                    if mouse_button == fltk::app::MouseButton::Left as i32
                        && !fltk::app::event_clicks()
                    {
                        let clicked_item = t
                            .find_clicked(false)
                            .or_else(|| t.find_clicked(true))
                            .or_else(|| Self::item_at_mouse(t));
                        pending_drag_text = None;

                        if let Some(item) = clicked_item.as_ref() {
                            let db_type = *current_db_type
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            let scope = Self::scope_snapshot(&selected_scope);
                            if let Some(insert_text) = Self::get_insert_text(
                                item,
                                db_type,
                                scope.as_deref(),
                                &object_cache,
                            ) {
                                pending_drag_text = Some(insert_text);
                            }
                        }
                    }

                    if mouse_button == fltk::app::MouseButton::Left as i32
                        && fltk::app::event_clicks()
                    {
                        pending_drag_text = None;
                        let clicked_item = t
                            .find_clicked(false)
                            .or_else(|| t.find_clicked(true))
                            .or_else(|| Self::item_at_mouse(t));

                        if let (Some(item), Some(selected_item)) =
                            (clicked_item, t.first_selected_item())
                        {
                            if item != selected_item {
                                return false;
                            }

                            return Self::activate_tree_item(
                                t,
                                &item,
                                &connection,
                                &current_db_type,
                                &selected_scope,
                                &scope_generation,
                                &object_cache,
                                &status_callback,
                                &action_sender,
                                &sql_callback,
                            );
                        }
                    }

                    false
                }
                Event::Drag => {
                    if fltk::app::event_state().contains(Shortcut::Button1) {
                        if let Some(insert_text) = pending_drag_text.take() {
                            object_drag_payload::start_drag(&insert_text);
                            return true;
                        }
                    }
                    false
                }
                Event::Released | Event::Leave => {
                    pending_drag_text = None;
                    false
                }
                Event::KeyDown => {
                    if !widget_has_focus(t) {
                        owned_keydown = None;
                        return false;
                    }
                    owned_keydown = Some(fltk::app::event_key());

                    match fltk::app::event_key() {
                        Key::Up => {
                            Self::select_focused_tree_item(t);
                            true
                        }
                        Key::Down => {
                            Self::select_focused_tree_item(t);
                            true
                        }
                        Key::Right => {
                            if let Some(item) = Self::current_tree_item(t) {
                                if let Some(package_name) =
                                    Self::package_name_requiring_routine_load(&item, &object_cache)
                                {
                                    Self::load_package_routines_async(
                                        &connection,
                                        &current_db_type,
                                        &selected_scope,
                                        &scope_generation,
                                        &status_callback,
                                        &action_sender,
                                        package_name,
                                        true,
                                    );
                                } else if let Some(table_name) =
                                    Self::table_requiring_column_load(&item, &object_cache)
                                {
                                    // Right expands; double-click still browses
                                    // the rows, which is the more common thing
                                    // to want from a table.
                                    Self::load_table_columns_async(
                                        &connection,
                                        &selected_scope,
                                        &scope_generation,
                                        &status_callback,
                                        &action_sender,
                                        table_name,
                                    );
                                } else {
                                    Self::select_first_child_item(t, &item);
                                }
                            }
                            true
                        }
                        Key::Left => {
                            if let Some(item) = Self::current_tree_item(t) {
                                Self::select_parent_item(t, &item);
                            }
                            true
                        }
                        _ => false,
                    }
                }
                Event::KeyUp => {
                    let key = fltk::app::event_key();
                    if !consume_owned_key_up(&mut owned_keydown, key) {
                        return false;
                    }

                    if matches!(key, Key::Up | Key::Down) && widget_has_focus(t) {
                        Self::select_focused_tree_item(t);
                        return true;
                    }

                    // Enter/KPEnter runs the same default action as a
                    // double-click - only if tree has focus
                    if matches!(key, Key::Enter | Key::KPEnter) && widget_has_focus(t) {
                        if let Some(item) = t.first_selected_item() {
                            Self::activate_tree_item(
                                t,
                                &item,
                                &connection,
                                &current_db_type,
                                &selected_scope,
                                &scope_generation,
                                &object_cache,
                                &status_callback,
                                &action_sender,
                                &sql_callback,
                            );
                        }
                        return true;
                    }
                    false
                }

                _ => false,
            }
        });
    }

    fn item_at_mouse(tree: &Tree) -> Option<TreeItem> {
        let mouse_y = fltk::app::event_y();
        let mut current = tree.first_visible_item();
        while let Some(item) = current {
            let item_y = item.y();
            let item_h = item.h();
            if mouse_y >= item_y && mouse_y < item_y + item_h {
                return Some(item);
            }
            current = tree.next_visible_item(&item, Key::Down);
        }
        None
    }

    fn current_tree_item(tree: &Tree) -> Option<TreeItem> {
        tree.get_item_focus()
            .or_else(|| tree.first_selected_item())
            .or_else(|| tree.first_visible_item())
    }

    /// Maps a tree category to the object type `generate_object_ddl` expects.
    fn ddl_object_type(object_type: &str) -> Option<&'static str> {
        match object_type {
            "TABLES" => Some("TABLE"),
            "VIEWS" => Some("VIEW"),
            "MATERIALIZED VIEWS" => Some("MATERIALIZED_VIEW"),
            "PROCEDURES" => Some("PROCEDURE"),
            "FUNCTIONS" => Some("FUNCTION"),
            "SEQUENCES" => Some("SEQUENCE"),
            "TRIGGERS" => Some("TRIGGER"),
            "EVENTS" => Some("EVENT"),
            "TYPES" => Some("TYPE"),
            "INDEXES" => Some("INDEX"),
            "SYNONYMS" => Some("SYNONYM"),
            "PACKAGES" => Some("PACKAGE"),
            _ => None,
        }
    }

    fn spawn_generate_ddl(
        connection: &SharedConnection,
        action_sender: &std::sync::mpsc::Sender<ObjectActionResult>,
        status_callback: &StatusCallback,
        selected_scope: Option<String>,
        object_type: &str,
        object_name: &str,
    ) {
        let connection = connection.clone();
        let sender = action_sender.clone();
        let object_type = object_type.to_string();
        let object_name = object_name.to_string();
        Self::emit_status_callback(
            status_callback,
            &format!("Generating {} DDL for {}", object_type, object_name),
        );
        thread::spawn(move || {
            let activity = format!("Generating {} DDL for {}", object_type, object_name);
            let result = ObjectBrowserWidget::with_pooled_object_session(
                &connection,
                selected_scope.as_deref(),
                activity,
                |context, session| {
                    object_browser_behavior_for(context.connection_info.db_type)
                        .generate_object_ddl(
                            context,
                            session,
                            selected_scope.as_deref(),
                            &object_type,
                            &object_name,
                        )
                },
            );
            let _ = sender.send(ObjectActionResult::Ddl(result));
            app::awake();
        });
    }

    /// Primary-key column names of `table_name`, in key order.
    ///
    /// Blocking: callers run it on a worker thread. Used by the result grid's
    /// "SQL Updates" export to build the WHERE clause. Goes through the same
    /// per-DB behavior the object browser uses for table structure, so Oracle
    /// (OCI and thin), MySQL, and MariaDB all resolve the real key.
    #[doc(hidden)]
    pub fn load_primary_key_columns(
        connection: &SharedConnection,
        selected_scope: Option<&str>,
        table_name: &str,
    ) -> Result<Vec<String>, String> {
        // A qualified name carries its own schema, which the per-DB loaders take
        // as the scope rather than as part of the table name.
        let (scope, table_name) = match table_name.trim().rsplit_once('.') {
            Some((schema, table)) if !schema.trim().is_empty() && !table.trim().is_empty() => {
                (Some(schema.trim().to_string()), table.trim().to_string())
            }
            _ => (
                selected_scope.map(str::to_string),
                table_name.trim().to_string(),
            ),
        };
        let activity = format!("Reading primary key of {}", table_name);
        let columns =
            Self::with_pooled_object_session(
                connection,
                scope.as_deref(),
                activity,
                |context, session| {
                    object_browser_behavior_for(context.connection_info.db_type)
                        .load_table_structure(context, session, scope.as_deref(), &table_name)
                },
            )?;
        Ok(columns
            .into_iter()
            .filter(|column| column.is_primary_key)
            .map(|column| column.name)
            .collect())
    }

    fn browse_target_for_object(
        object_name: &str,
        db_type: crate::db::DatabaseType,
        selected_scope: Option<&str>,
    ) -> TableBrowseTarget {
        let completion_name =
            Self::qualify_object_name_for_scope(db_type, selected_scope, object_name);
        let relation_sql = if db_type.is_mysql_or_mariadb() {
            Self::quote_mysql_identifier_path(&completion_name)
        } else {
            completion_name.clone()
        };
        TableBrowseTarget::new(
            db_type,
            selected_scope.map(str::to_string),
            object_name.to_string(),
            relation_sql,
            completion_name,
        )
    }

    /// The default action a tree node performs, shared by Enter and
    /// double-click so the two can never drift apart.
    fn default_action_for_item(
        item_info: Option<&ObjectItem>,
        db_type: crate::db::DatabaseType,
        selected_scope: Option<&str>,
    ) -> ObjectDefaultAction {
        let Some(item_info) = item_info else {
            // Category folders and package member groups carry no object info.
            return ObjectDefaultAction::ToggleNode;
        };

        match item_info {
            // A column drops its bare name into the editor, which is what the
            // caret is usually waiting for; the table it belongs to is already
            // in the statement by the time a column is being picked.
            ObjectItem::Column { column_name, .. } => {
                ObjectDefaultAction::InsertText(column_name.clone())
            }
            ObjectItem::Simple {
                object_type,
                object_name,
            } => match object_type.as_str() {
                "TABLES" => ObjectDefaultAction::Browse(Self::browse_target_for_object(
                    object_name,
                    db_type,
                    selected_scope,
                )),
                "VIEWS" | "MATERIALIZED VIEWS" => ObjectDefaultAction::Browse(
                    Self::browse_target_for_object(object_name, db_type, selected_scope)
                        .read_only(),
                ),
                "PACKAGES" => ObjectDefaultAction::PackageNode,
                other => match Self::ddl_object_type(other) {
                    Some(ddl_type) => ObjectDefaultAction::GenerateDdl {
                        object_type: ddl_type,
                        object_name: object_name.clone(),
                    },
                    None => ObjectDefaultAction::None,
                },
            },
            // A package routine has no DDL of its own: its source lives in the
            // package, which is what DataGrip opens for a package member too.
            ObjectItem::PackageRoutine { package_name, .. } => ObjectDefaultAction::GenerateDdl {
                object_type: "PACKAGE",
                object_name: package_name.clone(),
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn activate_tree_item(
        tree: &mut Tree,
        item: &TreeItem,
        connection: &SharedConnection,
        current_db_type: &Arc<Mutex<crate::db::DatabaseType>>,
        selected_scope: &Arc<Mutex<Option<String>>>,
        scope_generation: &Arc<AtomicU64>,
        object_cache: &Arc<Mutex<ObjectCache>>,
        status_callback: &StatusCallback,
        action_sender: &std::sync::mpsc::Sender<ObjectActionResult>,
        sql_callback: &SqlExecuteCallback,
    ) -> bool {
        let db_type = *current_db_type
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let scope = Self::scope_snapshot(selected_scope);
        let item_info = Self::resolve_tree_item(item, object_cache);

        match Self::default_action_for_item(item_info.as_ref(), db_type, scope.as_deref()) {
            ObjectDefaultAction::Browse(target) => {
                Self::emit_sql_callback(sql_callback, SqlAction::BrowseTable(target));
                true
            }
            ObjectDefaultAction::GenerateDdl {
                object_type,
                object_name,
            } => {
                Self::spawn_generate_ddl(
                    connection,
                    action_sender,
                    status_callback,
                    scope,
                    object_type,
                    &object_name,
                );
                true
            }
            ObjectDefaultAction::InsertText(text) => {
                Self::emit_sql_callback(sql_callback, SqlAction::Insert(text));
                true
            }
            ObjectDefaultAction::PackageNode => {
                match Self::package_name_requiring_routine_load(item, object_cache) {
                    Some(package_name) => Self::load_package_routines_async(
                        connection,
                        current_db_type,
                        selected_scope,
                        scope_generation,
                        status_callback,
                        action_sender,
                        package_name,
                        false,
                    ),
                    None => {
                        Self::toggle_tree_item(tree, item);
                    }
                }
                true
            }
            ObjectDefaultAction::ToggleNode => Self::toggle_tree_item(tree, item),
            ObjectDefaultAction::None => false,
        }
    }

    fn toggle_tree_item(tree: &mut Tree, item: &TreeItem) -> bool {
        if !item.has_children() {
            return false;
        }
        let mut item = item.clone();
        if item.is_close() {
            item.open();
        } else {
            item.close();
        }
        tree.redraw();
        true
    }

    fn select_tree_item(tree: &mut Tree, item: &TreeItem) {
        let _ = tree.select_only(item, false);
        tree.set_item_focus(item);
        Self::show_selected_item_like_tree_navigation(tree, item);
        tree.redraw();
    }

    fn select_tree_item_without_scroll(tree: &mut Tree, item: &TreeItem) {
        let _ = tree.select_only(item, false);
        tree.set_item_focus(item);
        tree.redraw();
    }

    fn select_focused_tree_item(tree: &mut Tree) -> bool {
        if let Some(item) = Self::current_tree_item(tree) {
            Self::select_tree_item(tree, &item);
            true
        } else {
            false
        }
    }

    fn show_selected_item_like_tree_navigation(tree: &mut Tree, item: &TreeItem) {
        let item_top = item.y();
        let item_bottom = item.y() + item.h();
        if item_top < tree.y() {
            tree.show_item_top(item);
        }
        if item_bottom > tree.y() + tree.h() {
            tree.show_item_bottom(item);
        }
    }

    /// Every expanded node, so a rebuild can put the tree back as it was.
    ///
    /// Not limited to packages any more: expanded tables have children too, and
    /// leaving them out would collapse them on every filter keystroke.
    fn open_tree_paths(tree: &Tree) -> HashSet<String> {
        tree.get_items()
            .unwrap_or_default()
            .into_iter()
            .filter(|item| item.has_children() && item.is_open())
            .filter_map(|item| tree.item_pathname(&item).ok())
            .collect()
    }

    fn restore_tree_open_paths(tree: &mut Tree, open_paths: &HashSet<String>) {
        for mut item in tree.get_items().unwrap_or_default() {
            let Ok(path) = tree.item_pathname(&item) else {
                continue;
            };
            if !item.has_children() {
                continue;
            }

            if open_paths.contains(&path) {
                item.open();
            } else {
                item.close();
            }
        }
    }

    fn select_first_child_item(tree: &mut Tree, item: &TreeItem) -> bool {
        if !item.has_children() {
            return false;
        }

        let mut item = item.clone();
        if item.is_close() {
            item.open();
        }

        if let Some(child) = item.child(0) {
            Self::select_tree_item_without_scroll(tree, &child);
            true
        } else {
            false
        }
    }

    fn select_parent_item(tree: &mut Tree, item: &TreeItem) -> bool {
        let Some(parent) = item.parent() else {
            return false;
        };
        if parent.is_root() {
            return false;
        }

        Self::select_tree_item_without_scroll(tree, &parent);
        true
    }

    fn package_name_requiring_routine_load(
        item: &TreeItem,
        object_cache: &Arc<Mutex<ObjectCache>>,
    ) -> Option<String> {
        let Some(ObjectItem::Simple {
            object_type,
            object_name,
        }) = Self::get_item_info(item)
        else {
            return None;
        };
        if object_type != "PACKAGES" {
            return None;
        }

        let cache = object_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if cache.package_routines.contains_key(&object_name) {
            None
        } else {
            Some(object_name)
        }
    }

    fn load_package_routines_async(
        connection: &SharedConnection,
        current_db_type: &Arc<Mutex<crate::db::DatabaseType>>,
        selected_scope: &Arc<Mutex<Option<String>>>,
        scope_generation: &Arc<AtomicU64>,
        status_callback: &StatusCallback,
        action_sender: &std::sync::mpsc::Sender<ObjectActionResult>,
        package_name: String,
        select_first_child_after_load: bool,
    ) {
        let connection = connection.clone();
        let sender = action_sender.clone();
        let selected_scope = selected_scope.clone();
        let db_type = *current_db_type
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let action_scope_generation = scope_generation.load(Ordering::Relaxed);
        Self::emit_status_callback(
            status_callback,
            &format!("Loading package members for {}", package_name),
        );
        thread::spawn(move || {
            let activity = format!("Loading package members for {}", package_name);
            let scope = ObjectBrowserWidget::scope_snapshot(&selected_scope);
            let result = object_browser_behavior_for(db_type).load_package_routines(
                &connection,
                activity,
                scope.as_deref(),
                &package_name,
            );

            let _ = sender.send(ObjectActionResult::PackageRoutines {
                package_name,
                result,
                scope_generation: action_scope_generation,
                select_first_child_after_load,
            });
            app::awake();
        });
    }

    fn get_item_info(item: &TreeItem) -> Option<ObjectItem> {
        let object_name = match item.label() {
            Some(label) => label.trim().to_string(),
            None => return None,
        };
        let parent = item.parent()?;
        let parent_label = match parent.label() {
            Some(label) => label.trim().to_string(),
            None => return None,
        };
        let parent_type_upper = parent_label.to_uppercase();

        // Package member item: Packages/<pkg>/(Procedures|Functions)/<name>
        if parent_type_upper == "PROCEDURES" || parent_type_upper == "FUNCTIONS" {
            if let Some(grandparent) = parent.parent() {
                if let Some(package_label) = grandparent.label() {
                    if let Some(root) = grandparent.parent() {
                        if let Some(root_label) = root.label() {
                            if root_label.trim().eq_ignore_ascii_case("Packages") {
                                let routine_type = if parent_type_upper == "FUNCTIONS" {
                                    "FUNCTION"
                                } else {
                                    "PROCEDURE"
                                };
                                return Some(ObjectItem::PackageRoutine {
                                    package_name: package_label.trim().to_string(),
                                    routine_name: object_name,
                                    routine_type: routine_type.to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }

        match parent_type_upper.as_str() {
            "TABLES" | "VIEWS" | "PROCEDURES" | "FUNCTIONS" | "SEQUENCES" | "TRIGGERS"
            | "EVENTS" | "SYNONYMS" | "PACKAGES" => Some(ObjectItem::Simple {
                object_type: parent_type_upper,
                object_name,
            }),
            _ => None,
        }
    }

    /// What a tree node is, columns included.
    ///
    /// Columns are resolved *first*. [`Self::get_item_info`] decides by the
    /// parent's label alone, so a column of a table legitimately named `VIEWS`
    /// would otherwise come back as a view — its parent's label is `VIEWS`,
    /// which is also a category name. `column_item_info` is the specific test
    /// (the grandparent must be `Tables`, and the label must be one this cache
    /// generated), so asking it first settles the collision.
    fn resolve_tree_item(
        item: &TreeItem,
        object_cache: &Arc<Mutex<ObjectCache>>,
    ) -> Option<ObjectItem> {
        Self::column_item_info(item, object_cache).or_else(|| Self::get_item_info(item))
    }

    /// A column node, resolved against the cache the tree was built from.
    ///
    /// Separate from [`Self::get_item_info`] so that callers which must not see
    /// columns — the context menu, the package loader — keep getting `None` for
    /// them without having to say so.
    fn column_item_info(
        item: &TreeItem,
        object_cache: &Arc<Mutex<ObjectCache>>,
    ) -> Option<ObjectItem> {
        let label = item.label()?.trim().to_string();
        let parent = item.parent()?;
        let table_name = parent.label()?.trim().to_string();
        let root_label = parent.parent()?.label()?;
        if !root_label.trim().eq_ignore_ascii_case("Tables") {
            return None;
        }
        let cache = object_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let columns = cache.table_columns.get(&table_name)?;
        let column_name = Self::column_name_for_node_label(columns, &label)?;
        Some(ObjectItem::Column {
            table_name,
            column_name,
        })
    }

    /// The table whose columns still have to be read before it can expand.
    fn table_requiring_column_load(
        item: &TreeItem,
        object_cache: &Arc<Mutex<ObjectCache>>,
    ) -> Option<String> {
        let Some(ObjectItem::Simple {
            object_type,
            object_name,
        }) = Self::get_item_info(item)
        else {
            return None;
        };
        if object_type != "TABLES" {
            return None;
        }
        let cache = object_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if cache.table_columns.contains_key(&object_name) {
            None
        } else {
            Some(object_name)
        }
    }

    /// Read a table's columns so its node can be expanded.
    ///
    /// Uses the same per-backend `load_table_structure` that `View Structure`
    /// and `Import Data...` use, so the tree cannot disagree with them about
    /// what the columns are.
    fn load_table_columns_async(
        connection: &SharedConnection,
        selected_scope: &Arc<Mutex<Option<String>>>,
        scope_generation: &Arc<AtomicU64>,
        status_callback: &StatusCallback,
        action_sender: &std::sync::mpsc::Sender<ObjectActionResult>,
        table_name: String,
    ) {
        let connection = connection.clone();
        let sender = action_sender.clone();
        let selected_scope = selected_scope.clone();
        let action_scope_generation = scope_generation.load(Ordering::Relaxed);
        Self::emit_status_callback(
            status_callback,
            &format!("Loading columns for {}", table_name),
        );
        thread::spawn(move || {
            let activity = format!("Loading columns for {}", table_name);
            let scope = ObjectBrowserWidget::scope_snapshot(&selected_scope);
            let result = ObjectBrowserWidget::with_pooled_object_session(
                &connection,
                scope.as_deref(),
                activity,
                |context, session| {
                    object_browser_behavior_for(context.connection_info.db_type)
                        .load_table_structure(context, session, scope.as_deref(), &table_name)
                },
            );
            let _ = sender.send(ObjectActionResult::TableColumns {
                table_name,
                result,
                scope_generation: action_scope_generation,
            });
            app::awake();
        });
    }

    fn get_insert_text(
        item: &TreeItem,
        db_type: crate::db::DatabaseType,
        selected_scope: Option<&str>,
        object_cache: &Arc<Mutex<ObjectCache>>,
    ) -> Option<String> {
        Self::resolve_tree_item(item, object_cache)
            .as_ref()
            .map(|item_info| {
                Self::copy_text_for_object_item_with_scope(item_info, db_type, selected_scope)
            })
    }

    fn copy_text_for_selected_item(
        item: &TreeItem,
        object_cache: &Arc<Mutex<ObjectCache>>,
    ) -> Option<String> {
        Self::resolve_tree_item(item, object_cache)
            .as_ref()
            .map(copy_text_for_object_item)
            .or_else(|| {
                item.label().and_then(|label| {
                    let trimmed = label.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                })
            })
    }

    fn quote_mysql_identifier_path(identifier: &str) -> String {
        let mut segments = Vec::new();
        let mut start = 0usize;
        let mut active_quote = false;
        let trimmed = identifier.trim();
        let mut chars = trimmed.char_indices().peekable();

        while let Some((idx, ch)) = chars.next() {
            if ch == '`' {
                if active_quote {
                    if chars.peek().is_some_and(|(_, next)| *next == '`') {
                        chars.next();
                    } else {
                        active_quote = false;
                    }
                } else {
                    active_quote = true;
                }
                continue;
            }
            if ch == '.' && !active_quote {
                if let Some(segment) = trimmed.get(start..idx) {
                    segments.push(segment);
                }
                start = idx + ch.len_utf8();
            }
        }

        if let Some(segment) = trimmed.get(start..) {
            segments.push(segment);
        }

        segments
            .into_iter()
            .filter_map(|segment| {
                let trimmed = segment.trim();
                if trimmed.is_empty() {
                    return None;
                }
                let unquoted = crate::sql_text::strip_identifier_quotes(trimmed);
                Some(format!("`{}`", unquoted.replace('`', "``")))
            })
            .collect::<Vec<_>>()
            .join(".")
    }

    fn scope_label_text(_db_type: crate::db::DatabaseType) -> &'static str {
        ""
    }

    fn scope_choice_values(choice: &Choice) -> Vec<String> {
        (0..choice.size())
            .filter_map(|index| choice.text(index))
            .collect()
    }

    fn scope_options_match_for_db_type(
        db_type: crate::db::DatabaseType,
        current_options: &[String],
        desired_options: &[String],
    ) -> bool {
        current_options.len() == desired_options.len()
            && current_options
                .iter()
                .zip(desired_options.iter())
                .all(|(current, desired)| {
                    Self::scope_values_match_for_db_type(
                        db_type,
                        Some(current.as_str()),
                        Some(desired.as_str()),
                    )
                })
    }

    fn sync_scope_choice_widget(
        scope_choice: &mut Choice,
        suppress_scope_events: &Arc<Mutex<bool>>,
        scope_choice_menu_busy: &Arc<AtomicBool>,
        db_type: crate::db::DatabaseType,
        available_scopes: &[String],
        resolved_scope: Option<&str>,
        switch_in_progress: bool,
    ) {
        // A card can be torn down while an alert or a popup pumps the event
        // loop, and every line below writes to its widget.
        if scope_choice.was_deleted() {
            return;
        }
        // Choice still owns the backing menu items, so avoid rebuilding them
        // while the custom selector popup or a FLTK grab is active.
        if Self::scope_choice_sync_should_defer(
            app::grab().is_some(),
            scope_choice_menu_busy.load(Ordering::Acquire),
        ) {
            return;
        }

        let current_options = Self::scope_choice_values(scope_choice);
        let needs_rebuild =
            !Self::scope_options_match_for_db_type(db_type, &current_options, available_scopes);
        // Falling straight through to the first option would make the
        // selector announce the alphabetically first database whenever the
        // caller has no scope to pass — a rebuild of the item list must not
        // silently move the user's selection. Keep what is already selected
        // as long as the new option list still contains it.
        let previously_selected = scope_choice.choice().and_then(|selected| {
            let selected = selected.trim().to_string();
            (!selected.is_empty()
                && available_scopes.iter().any(|option| {
                    Self::scope_values_match_for_db_type(
                        db_type,
                        Some(option.as_str()),
                        Some(selected.as_str()),
                    )
                }))
            .then_some(selected)
        });
        let desired_scope = resolved_scope
            .map(str::trim)
            .filter(|scope| !scope.is_empty())
            .map(str::to_string)
            .or(previously_selected)
            .or_else(|| available_scopes.first().cloned());

        *suppress_scope_events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;

        if needs_rebuild {
            scope_choice.clear();
            if !available_scopes.is_empty() {
                scope_choice.add_choice(&available_scopes.join("|"));
            }
        }

        if let Some(ref desired_scope) = desired_scope {
            if let Some(index) =
                Self::choice_index_for_value_for_db_type(db_type, scope_choice, desired_scope)
            {
                scope_choice.set_value(index);
            } else if !available_scopes.is_empty() {
                scope_choice.set_value(0);
            }
        } else if !available_scopes.is_empty() {
            scope_choice.set_value(0);
        }

        Self::apply_scope_choice_enabled_state(scope_choice, switch_in_progress);

        *suppress_scope_events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
    }

    fn scope_choice_should_be_active(option_count: i32, switch_in_progress: bool) -> bool {
        option_count > 0 && !switch_in_progress
    }

    fn apply_scope_choice_enabled_state(scope_choice: &mut Choice, switch_in_progress: bool) {
        // Callers reach here after alerts, which pump the event loop and can
        // take this card down with them.
        if scope_choice.was_deleted() {
            return;
        }
        if Self::scope_choice_should_be_active(scope_choice.size(), switch_in_progress) {
            scope_choice.activate();
        } else {
            scope_choice.deactivate();
        }
    }

    fn choice_index_for_value_for_db_type(
        db_type: crate::db::DatabaseType,
        choice: &Choice,
        value: &str,
    ) -> Option<i32> {
        let options = (0..choice.size())
            .filter_map(|index| choice.text(index))
            .collect::<Vec<_>>();
        Self::scope_option_index_for_db_type(db_type, &options, value)
    }

    fn scope_choice_sync_should_defer(menu_grab_active: bool, selector_popup_busy: bool) -> bool {
        menu_grab_active || selector_popup_busy
    }

    fn scope_option_index_for_db_type(
        db_type: crate::db::DatabaseType,
        options: &[String],
        value: &str,
    ) -> Option<i32> {
        let target = value.trim();
        options
            .iter()
            .position(|entry| {
                Self::scope_values_match_for_db_type(db_type, Some(entry.as_str()), Some(target))
            })
            .map(|index| index as i32)
    }

    fn restore_previous_scope_choice(
        scope_choice: &mut Choice,
        suppress_scope_events: &Arc<Mutex<bool>>,
        db_type: crate::db::DatabaseType,
        previous_scope: Option<&str>,
    ) {
        // Reached after an alert on the failure paths; the alert pumps the
        // event loop, which can delete this card's widgets.
        if scope_choice.was_deleted() {
            return;
        }
        let Some(previous_scope) = previous_scope else {
            return;
        };

        *suppress_scope_events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        if let Some(index) =
            Self::choice_index_for_value_for_db_type(db_type, scope_choice, previous_scope)
        {
            scope_choice.set_value(index);
        }
        *suppress_scope_events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
    }

    fn scope_values_match_for_db_type(
        db_type: crate::db::DatabaseType,
        left: Option<&str>,
        right: Option<&str>,
    ) -> bool {
        db_type.scope_values_match(left, right)
    }

    fn scope_switch_activity_message(
        db_type: crate::db::DatabaseType,
        target_scope: &str,
    ) -> String {
        db_type.scope_switch_activity_message(target_scope)
    }

    fn scope_switch_failure_message(
        db_type: crate::db::DatabaseType,
        target_scope: &str,
        err: &str,
    ) -> String {
        db_type.scope_switch_failure_message(target_scope, err)
    }

    fn refresh_result_matches_generations(
        event_scope_generation: u64,
        current_scope_generation: u64,
        event_connection_generation: u64,
        current_refresh_connection_generation: u64,
    ) -> bool {
        event_scope_generation == current_scope_generation
            && event_connection_generation == current_refresh_connection_generation
    }

    fn object_action_pool_session_context(
        connection: &SharedConnection,
    ) -> Result<crate::db::DbPoolSessionContext, String> {
        crate::db::pool_session_context_for_shared_connection(connection, None)
    }

    fn with_pooled_object_session<T>(
        connection: &SharedConnection,
        selected_scope: Option<&str>,
        activity: String,
        action: impl FnOnce(
            &crate::db::DbPoolSessionContext,
            &mut crate::db::DbPoolSession,
        ) -> Result<T, String>,
    ) -> Result<T, String> {
        let base_context = Self::object_action_pool_session_context(connection)?;
        let context = base_context.for_scope(selected_scope);
        let activity_guard = context.track_activity(activity);
        Self::ensure_object_action_context_current(connection, &base_context)?;
        // session.md §3 / §27 — narrow the race between scope validation and
        // session acquire. `ensure_object_action_context_current` uses a
        // try_lock that silently passes when the connection mutex is held by
        // another caller, so an extra cache match check before acquire keeps
        // a disconnect that landed mid-call from leasing a stale pool session.
        if !crate::db::cached_pool_session_context_matches_shared_connection(
            connection,
            &base_context,
        ) {
            return Err(
                "Connection scope changed before object metadata query started. Retry the action."
                    .to_string(),
            );
        }
        // Session and cancel reach as ONE value, so the reach lasts exactly as
        // long as the action's use of the session and ends BEFORE it goes back
        // to the pool -- `AcquiredPoolSession`'s own drop, which runs after
        // this frame's borrow of the session ends.
        let mut acquired = base_context.acquire_session_for_scope(
            selected_scope,
            crate::db::PooledSessionPurpose::AppRead,
            &activity_guard,
        )?;
        if !crate::db::cached_pool_session_context_matches_shared_connection(
            connection,
            &base_context,
        ) {
            return Err(
                "Connection scope changed before object metadata query started. Retry the action."
                    .to_string(),
            );
        }
        Self::ensure_object_action_context_current(connection, &base_context)?;
        let Some(session) = acquired.session_mut() else {
            return Err(
                "Object metadata session was taken before the action started. Retry the action."
                    .to_string(),
            );
        };
        action(&context, session)
    }

    fn ensure_object_action_context_current(
        connection: &SharedConnection,
        context: &crate::db::DbPoolSessionContext,
    ) -> Result<(), String> {
        let Some(conn_guard) = try_lock_connection(connection) else {
            return Ok(());
        };
        if !conn_guard.can_reuse_pool_session(
            context.connection_generation,
            context.connection_info.db_type,
        ) {
            return Err(
                "Connection changed before object metadata query started. Retry the action."
                    .to_string(),
            );
        }
        Ok(())
    }

    fn acquire_oracle_metadata_session(
        context: &crate::db::DbPoolSessionContext,
        activity: &crate::db::DbActivityGuard,
    ) -> Option<crate::db::HeldSession<Arc<oracle::Connection>>> {
        if activity.is_finished() {
            return None;
        }
        context.ensure_current().ok()?;
        match context
            .acquire_session_for_current_scope(crate::db::PooledSessionPurpose::AppRead, activity)
        {
            Ok(acquired) => match acquired.into_oracle() {
                Ok(conn) => Some(conn),
                Err(other) => {
                    eprintln!(
                        "Warning: expected Oracle object-browser metadata session but acquired {}",
                        other.describe_session()
                    );
                    None
                }
            },
            Err(err) => {
                eprintln!(
                    "Warning: failed to acquire Oracle object-browser metadata session: {err}"
                );
                None
            }
        }
    }

    fn acquire_oracle_thin_metadata_session(
        context: &crate::db::DbPoolSessionContext,
        activity: &crate::db::DbActivityGuard,
    ) -> Option<
        crate::db::HeldSession<tns_thin::pool::PooledThinConnection<tns_thin::OracleThinSession>>,
    > {
        if activity.is_finished() {
            return None;
        }
        context.ensure_current().ok()?;
        match context
            .acquire_session_for_current_scope(crate::db::PooledSessionPurpose::AppRead, activity)
        {
            Ok(acquired) => match acquired.into_oracle_thin() {
                Ok(conn) => Some(conn),
                Err(other) => {
                    eprintln!(
                        "Warning: expected Oracle Thin object-browser metadata session but acquired {}",
                        other.describe_session()
                    );
                    None
                }
            },
            Err(err) => {
                eprintln!(
                    "Warning: failed to acquire Oracle Thin object-browser metadata session: {err}"
                );
                None
            }
        }
    }

    fn acquire_mysql_metadata_session(
        context: &crate::db::DbPoolSessionContext,
        selected_scope: &str,
        activity: &crate::db::DbActivityGuard,
    ) -> Option<crate::db::HeldSession<mysql::PooledConn>> {
        if activity.is_finished() {
            return None;
        }
        context.ensure_current().ok()?;
        let expected_db_type = context.connection_info.db_type;
        let display_name = expected_db_type.display_name();
        let mut mysql_conn = match context
            .acquire_session_for_current_scope(crate::db::PooledSessionPurpose::AppRead, activity)
        {
            Ok(acquired) => match acquired.into_mysql(expected_db_type) {
                Ok(conn) => conn,
                Err(other) => {
                    eprintln!(
                        "Warning: expected {display_name} object-browser metadata session but acquired {}",
                        other.describe_session()
                    );
                    return None;
                }
            },
            Err(err) => {
                eprintln!(
                    "Warning: failed to acquire {display_name} object-browser metadata session: {err}"
                );
                return None;
            }
        };

        // Preparing this session is SEVERAL steps -- the database, then the
        // connection encoding -- so a failure between them leaves state nobody
        // has accounted for. `return None` on its own DROPPED the `HeldSession`,
        // which puts exactly that session back in the pool for the next tab;
        // the same rule the DB layer's own `acquire_session_with_scope_context`
        // states for the same multi-step premise.
        if let Err(err) = mysql_conn.as_mut().select_db(selected_scope) {
            eprintln!(
                "Warning: failed to select {display_name} object-browser metadata database `{selected_scope}`: {err}"
            );
            mysql_conn.discard();
            return None;
        }

        if let Err(err) =
            crate::db::DatabaseConnection::apply_mysql_connection_encoding_with_settings_for_db_type(
                &mut *mysql_conn,
                &context.connection_info.advanced,
                expected_db_type,
            )
        {
            eprintln!(
                "Warning: failed to refresh {display_name} object-browser metadata encoding: {err}"
            );
            mysql_conn.discard();
            return None;
        }

        Some(mysql_conn)
    }

    fn object_metadata_worker_limit(context: &crate::db::DbPoolSessionContext) -> usize {
        (context.connection_pool_size as usize).max(1)
    }

    fn load_object_metadata_jobs(
        context: &crate::db::DbPoolSessionContext,
        mut jobs: Vec<ObjectMetadataLoadJob>,
        worker_limit: usize,
        activity: &crate::db::DbActivityGuard,
    ) -> ObjectCache {
        let worker_limit = worker_limit.max(1);
        let mut cache = ObjectCache::default();
        thread::scope(|scope| {
            while !jobs.is_empty() {
                if !context.is_current() || activity.is_finished() {
                    break;
                }
                let batch_len = worker_limit.min(jobs.len());
                let batch: Vec<_> = jobs.drain(..batch_len).collect();
                let mut handles = Vec::with_capacity(batch_len);
                for job in batch {
                    handles.push(scope.spawn(job));
                }
                for handle in handles {
                    if let Ok(partial) = handle.join() {
                        if !context.is_current() || activity.is_finished() {
                            break;
                        }
                        ObjectBrowserWidget::merge_object_metadata_cache(&mut cache, partial);
                    }
                }
            }
        });
        cache
    }

    fn merge_object_metadata_cache(target: &mut ObjectCache, partial: ObjectCache) {
        if !partial.tables.is_empty() {
            target.tables = partial.tables;
            // A refresh is a request for current metadata, and a cached column
            // list survives an ALTER without noticing. Dropping them costs one
            // query the next time a table is expanded.
            target.table_columns.clear();
        }
        if !partial.views.is_empty() {
            target.views = partial.views;
        }
        if !partial.procedures.is_empty() {
            target.procedures = partial.procedures;
        }
        if !partial.functions.is_empty() {
            target.functions = partial.functions;
        }
        if !partial.sequences.is_empty() {
            target.sequences = partial.sequences;
        }
        if !partial.triggers.is_empty() {
            target.triggers = partial.triggers;
        }
        if !partial.events.is_empty() {
            target.events = partial.events;
        }
        if !partial.synonyms.is_empty() {
            target.synonyms = partial.synonyms;
        }
        if !partial.packages.is_empty() {
            target.packages = partial.packages;
        }
        if !partial.package_routines.is_empty() {
            target.package_routines.extend(partial.package_routines);
        }
        if !partial.table_columns.is_empty() {
            target.table_columns.extend(partial.table_columns);
        }
    }

    fn scope_refresh_status_message(
        db_type: crate::db::DatabaseType,
        next_scope: Option<&str>,
    ) -> String {
        db_type.metadata_refresh_activity_with_base("Loading object browser metadata", next_scope)
    }

    fn complete_scope_change(
        selected_scope: &Arc<Mutex<Option<String>>>,
        catalog: &CardCatalogState,
        status_callback: &StatusCallback,
        scope_change_callback: &ScopeChangeCallback,
        db_type: crate::db::DatabaseType,
        next_scope: Option<String>,
    ) {
        {
            let mut scope_guard = selected_scope
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *scope_guard = next_scope.clone();
        }
        // The ask moves with the selection, and the catalog it no longer
        // answers stops counting — one call, so this path cannot drift from
        // `set_selected_scope`. The refresh that replaces the cache is
        // started by the app, and may not start at all while the connection
        // is busy; until then no other card may inherit from this one.
        catalog.ask_for(next_scope.clone(), false);

        Self::emit_status_callback(
            status_callback,
            &Self::scope_refresh_status_message(db_type, next_scope.as_deref()),
        );
        Self::invoke_scope_change_callback(scope_change_callback);
    }

    fn invoke_scope_change_callback(scope_change_callback: &ScopeChangeCallback) {
        let callback = {
            let mut slot = scope_change_callback
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            slot.take()
        };

        if let Some(mut callback) = callback {
            let call_result = panic::catch_unwind(AssertUnwindSafe(&mut callback));
            let mut slot = scope_change_callback
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot.is_none() {
                *slot = Some(callback);
            }
            if let Err(payload) = call_result {
                Self::log_callback_panic("scope change callback", payload.as_ref());
            }
        }
    }

    fn invoke_scope_switch_preflight_callback(
        callback_slot: &ScopeSwitchPreflightCallback,
    ) -> Result<(), String> {
        let callback = {
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            slot.take()
        };

        let Some(mut callback) = callback else {
            return Ok(());
        };

        let call_result = panic::catch_unwind(AssertUnwindSafe(&mut callback));
        let mut slot = callback_slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_none() {
            *slot = Some(callback);
        }

        match call_result {
            Ok(result) => result,
            Err(payload) => {
                Self::log_callback_panic("scope switch preflight callback", payload.as_ref());
                Err(
                    "Scope switch preflight failed internally. Retry the action or reconnect."
                        .to_string(),
                )
            }
        }
    }

    fn scope_snapshot(selected_scope: &Arc<Mutex<Option<String>>>) -> Option<String> {
        selected_scope
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn qualify_oracle_object_name(selected_scope: Option<&str>, object_name: &str) -> String {
        let object_name = object_name.trim();
        if object_name.is_empty() || object_name.contains('.') {
            return object_name.to_string();
        }

        let object_name = crate::db::DatabaseConnection::quote_oracle_identifier(object_name);
        selected_scope
            .filter(|scope| !scope.trim().is_empty())
            .map(|scope| {
                format!(
                    "{}.{}",
                    crate::db::DatabaseConnection::quote_oracle_identifier(scope),
                    object_name
                )
            })
            .unwrap_or(object_name)
    }

    fn qualify_object_name_for_scope(
        db_type: crate::db::DatabaseType,
        selected_scope: Option<&str>,
        object_name: &str,
    ) -> String {
        object_browser_behavior_for(db_type).qualify_object_name(selected_scope, object_name)
    }

    fn mysql_scope_for_context<'a>(
        selected_scope: Option<&'a str>,
        current_service_name: &'a str,
    ) -> Option<&'a str> {
        selected_scope
            .filter(|scope| !scope.trim().is_empty())
            .or_else(|| {
                let current_service_name = current_service_name.trim();
                (!current_service_name.is_empty()).then_some(current_service_name)
            })
    }

    fn qualify_package_member_name(
        db_type: crate::db::DatabaseType,
        selected_scope: Option<&str>,
        package_name: &str,
        routine_name: &str,
    ) -> String {
        object_browser_behavior_for(db_type).qualify_package_member_name(
            selected_scope,
            package_name,
            routine_name,
        )
    }

    fn copy_text_for_object_item_with_scope(
        item_info: &ObjectItem,
        db_type: crate::db::DatabaseType,
        selected_scope: Option<&str>,
    ) -> String {
        match item_info {
            // Not scope-qualified: a column name is only ever used inside a
            // statement that has already named its table.
            ObjectItem::Column { column_name, .. } => column_name.clone(),
            ObjectItem::Simple { object_name, .. } => {
                Self::qualify_object_name_for_scope(db_type, selected_scope, object_name)
            }
            ObjectItem::PackageRoutine {
                package_name,
                routine_name,
                ..
            } => Self::qualify_package_member_name(
                db_type,
                selected_scope,
                package_name,
                routine_name,
            ),
        }
    }

    fn preview_select_sql(
        db_type: crate::db::DatabaseType,
        selected_scope: Option<&str>,
        object_name: &str,
    ) -> String {
        object_browser_behavior_for(db_type).preview_select_sql(selected_scope, object_name)
    }

    fn destructive_object_sql(
        db_type: crate::db::DatabaseType,
        action: DestructiveObjectAction,
        selected_scope: Option<&str>,
        object_type: &str,
        object_name: &str,
    ) -> Option<String> {
        object_browser_behavior_for(db_type).destructive_object_sql(
            action,
            selected_scope,
            object_type,
            object_name,
        )
    }

    fn quote_mysql_alias(alias: &str) -> String {
        format!("`{}`", alias.trim().trim_matches('`').replace('`', "``"))
    }

    fn build_simple_procedure_script(qualified_name: &str) -> String {
        format!("BEGIN\n  {};\nEND;\n/\n", qualified_name)
    }

    fn build_simple_function_script(qualified_name: &str) -> String {
        format!(
            "SELECT {} AS result\nFROM dual;\n",
            if qualified_name.contains('(') {
                qualified_name.to_string()
            } else {
                format!("{}()", qualified_name)
            }
        )
    }

    fn build_simple_procedure_script_for_db(
        db_type: crate::db::DatabaseType,
        qualified_name: &str,
    ) -> String {
        object_browser_behavior_for(db_type).build_simple_procedure_script(qualified_name)
    }

    fn build_simple_function_script_for_db(
        db_type: crate::db::DatabaseType,
        qualified_name: &str,
    ) -> String {
        object_browser_behavior_for(db_type).build_simple_function_script(qualified_name)
    }

    fn build_simple_routine_script_for_db(
        db_type: crate::db::DatabaseType,
        qualified_name: &str,
        routine_type: &str,
    ) -> String {
        if routine_type.eq_ignore_ascii_case("FUNCTION") {
            Self::build_simple_function_script_for_db(db_type, qualified_name)
        } else {
            Self::build_simple_procedure_script_for_db(db_type, qualified_name)
        }
    }

    fn default_value_for_mysql_argument(arg: &ProcedureArgument, type_str: &str) -> String {
        if let Some(default_value) = arg.default_value.as_deref() {
            let trimmed = default_value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }

        let base = Self::normalize_type_base(type_str);
        match base.as_str() {
            "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "INTEGER" | "BIGINT" | "DECIMAL"
            | "NUMERIC" | "FLOAT" | "DOUBLE" | "REAL" | "BIT" => "0".to_string(),
            "DATE" => "CURRENT_DATE".to_string(),
            "DATETIME" | "TIMESTAMP" => "CURRENT_TIMESTAMP".to_string(),
            "TIME" => "CURRENT_TIME".to_string(),
            "BOOLEAN" | "BOOL" => "FALSE".to_string(),
            "CHAR" | "VARCHAR" | "TEXT" | "TINYTEXT" | "MEDIUMTEXT" | "LONGTEXT" | "ENUM"
            | "SET" | "JSON" => "''".to_string(),
            _ => "NULL".to_string(),
        }
    }

    fn build_mysql_routine_script(
        qualified_name: &str,
        routine_type: &str,
        arguments: &[ProcedureArgument],
    ) -> String {
        let selected_args = Self::select_overload_arguments(arguments);
        if selected_args.is_empty() {
            return Self::build_simple_mysql_routine_script(qualified_name, routine_type);
        }

        let target = Self::quote_mysql_identifier_path(qualified_name);
        let mut used_names: HashSet<String> = HashSet::new();
        let mut prelude_lines: Vec<String> = Vec::new();
        let mut call_args: Vec<String> = Vec::new();
        let mut post_lines: Vec<String> = Vec::new();

        for arg in &selected_args {
            if arg.position == 0 && arg.name.is_none() {
                continue;
            }

            let arg_label = arg
                .name
                .clone()
                .unwrap_or_else(|| format!("arg{}", arg.position.max(1)));
            let direction = arg
                .in_out
                .clone()
                .unwrap_or_else(|| "IN".to_string())
                .replace('/', " ")
                .to_uppercase();
            let type_str = Self::format_argument_type(arg);

            if direction.contains("OUT") && !direction.contains("IN") {
                let session_var = format!(
                    "@{}",
                    Self::unique_var_name(&arg_label, arg.position, &mut used_names)
                );
                call_args.push(session_var.clone());
                post_lines.push(format!(
                    "SELECT {} AS {};",
                    session_var,
                    Self::quote_mysql_alias(&arg_label)
                ));
                continue;
            }

            if direction.contains("IN") && direction.contains("OUT") {
                let session_var = format!(
                    "@{}",
                    Self::unique_var_name(&arg_label, arg.position, &mut used_names)
                );
                prelude_lines.push(format!(
                    "SET {} = {};",
                    session_var,
                    Self::default_value_for_mysql_argument(arg, &type_str)
                ));
                call_args.push(session_var.clone());
                post_lines.push(format!(
                    "SELECT {} AS {};",
                    session_var,
                    Self::quote_mysql_alias(&arg_label)
                ));
                continue;
            }

            call_args.push(Self::default_value_for_mysql_argument(arg, &type_str));
        }

        let multiline_args = if call_args.is_empty() {
            String::new()
        } else {
            let mut args_sql = String::from("(\n");
            for (index, arg) in call_args.iter().enumerate() {
                let suffix = if index + 1 == call_args.len() {
                    ""
                } else {
                    ","
                };
                args_sql.push_str(&format!("    {}{}\n", arg, suffix));
            }
            args_sql.push(')');
            args_sql
        };

        let mut script = String::new();
        for line in prelude_lines {
            script.push_str(&line);
            script.push('\n');
        }

        if routine_type.eq_ignore_ascii_case("FUNCTION") {
            if multiline_args.is_empty() {
                script.push_str(&format!("SELECT {}() AS result;\n", target));
            } else {
                script.push_str(&format!("SELECT {}{} AS result;\n", target, multiline_args));
            }
            return script;
        }

        if multiline_args.is_empty() {
            script.push_str(&format!("CALL {}();\n", target));
        } else {
            script.push_str(&format!("CALL {}{};\n", target, multiline_args));
        }

        for line in post_lines {
            script.push_str(&line);
            script.push('\n');
        }

        script
    }

    fn build_simple_mysql_routine_script(qualified_name: &str, routine_type: &str) -> String {
        if routine_type.eq_ignore_ascii_case("FUNCTION") {
            return format!(
                "SELECT {} AS result;\n",
                if qualified_name.contains('(') {
                    qualified_name.to_string()
                } else {
                    format!("{}()", Self::quote_mysql_identifier_path(qualified_name))
                }
            );
        }

        format!(
            "CALL {}();\n",
            Self::quote_mysql_identifier_path(qualified_name)
        )
    }

    fn build_procedure_script(qualified_name: &str, arguments: &[ProcedureArgument]) -> String {
        if arguments.is_empty() {
            return Self::build_simple_procedure_script(qualified_name);
        }

        let selected_args = Self::select_overload_arguments(arguments);
        if selected_args.is_empty() {
            return Self::build_simple_procedure_script(qualified_name);
        }

        let mut used_names: HashSet<String> = HashSet::new();
        let mut local_decls: Vec<String> = Vec::new();
        let mut call_args: Vec<String> = Vec::new();
        let mut bind_decls: Vec<(String, String)> = Vec::new();
        // Function return value (position=0, name=NULL) must be assigned
        // via ':=' rather than passed as a call argument.
        let mut return_var: Option<String> = None;

        for arg in &selected_args {
            let arg_label = arg.name.clone();
            let direction = arg
                .in_out
                .clone()
                .unwrap_or_else(|| "IN".to_string())
                .replace('/', " ")
                .to_uppercase();
            let is_out = direction.contains("OUT");
            let is_in = direction.contains("IN");

            // Detect function return value: position=0 with no argument name
            // and direction is OUT (not IN OUT).
            let is_return_value = arg.position == 0 && arg.name.is_none() && is_out && !is_in;

            let var_base =
                arg_label
                    .as_deref()
                    .unwrap_or(if is_return_value { "RESULT" } else { "ARG" });
            let var_name = Self::unique_var_name(var_base, arg.position, &mut used_names);

            if is_return_value {
                let type_str = Self::format_argument_type(arg);
                if Self::is_ref_cursor(arg) {
                    bind_decls.push((var_name.clone(), "REFCURSOR".to_string()));
                    return_var = Some(format!(":{}", var_name));
                } else if let Some(bind_type) = Self::bind_type_for_return(&type_str) {
                    bind_decls.push((var_name.clone(), bind_type));
                    return_var = Some(format!(":{}", var_name));
                } else {
                    // Fallback for unsupported return types: keep local variable assignment.
                    local_decls.push(format!("  {} {};", var_name, type_str));
                    return_var = Some(var_name);
                }
            } else if is_out && Self::is_ref_cursor(arg) {
                bind_decls.push((var_name.clone(), "REFCURSOR".to_string()));
                let target = format!(":{}", var_name);
                let call_expr = match &arg_label {
                    Some(label) => format!("{} => {}", label, target),
                    None => target,
                };
                call_args.push(call_expr);
            } else {
                let type_str = Self::format_argument_type(arg);
                if is_in {
                    let default_expr = Self::default_value_for_argument(arg, &type_str);
                    local_decls.push(format!("  {} {} := {};", var_name, type_str, default_expr));
                } else {
                    local_decls.push(format!("  {} {};", var_name, type_str));
                }
                let call_expr = match &arg_label {
                    Some(label) => format!("{} => {}", label, var_name),
                    None => var_name,
                };
                call_args.push(call_expr);
            }
        }

        let mut script = String::new();
        for (name, bind_type) in &bind_decls {
            script.push_str(&format!("VAR {} {}\n", name, bind_type));
        }

        if !local_decls.is_empty() {
            script.push_str("DECLARE\n");
            for decl in &local_decls {
                script.push_str(decl);
                script.push('\n');
            }
        }

        script.push_str("BEGIN\n");

        // Build the call expression (with or without arguments)
        let call_str = if call_args.is_empty() {
            qualified_name.to_string()
        } else {
            let mut s = format!("{}(\n", qualified_name);
            for (idx, arg) in call_args.iter().enumerate() {
                let suffix = if idx + 1 == call_args.len() { "" } else { "," };
                s.push_str(&format!("    {}{}\n", arg, suffix));
            }
            s.push_str("  )");
            s
        };

        if let Some(ref ret_var) = return_var {
            // Function: assign return value via ':='
            script.push_str(&format!("  {} := {};\n", ret_var, call_str));
        } else {
            // Procedure: plain call
            script.push_str(&format!("  {};\n", call_str));
        }

        script.push_str("END;\n/\n");

        script
    }

    fn bind_type_for_return(type_str: &str) -> Option<String> {
        let upper = type_str.trim().to_uppercase();
        if upper.is_empty() {
            return None;
        }
        let base = Self::normalize_type_base(&upper);
        if base.contains('.') {
            return None;
        }

        match base.as_str() {
            "NUMBER" | "NUMERIC" | "DECIMAL" | "INTEGER" | "INT" | "PLS_INTEGER"
            | "BINARY_INTEGER" | "NATURAL" | "NATURALN" | "POSITIVE" | "POSITIVEN"
            | "SIMPLE_INTEGER" | "FLOAT" | "BINARY_FLOAT" | "BINARY_DOUBLE" => {
                Some("NUMBER".to_string())
            }
            "DATE" => Some("DATE".to_string()),
            "TIMESTAMP" => {
                let precision = Self::extract_parenthesized_u32(&upper)
                    .unwrap_or(6)
                    .clamp(0, 9);
                Some(format!("TIMESTAMP({})", precision))
            }
            "CLOB" | "NCLOB" => Some("CLOB".to_string()),
            "VARCHAR2" | "NVARCHAR2" | "VARCHAR" | "CHAR" | "NCHAR" | "RAW" => {
                let size = Self::extract_parenthesized_u32(&upper)
                    .unwrap_or(4000)
                    .clamp(1, 4000);
                Some(format!("VARCHAR2({})", size))
            }
            _ => None,
        }
    }

    fn extract_parenthesized_u32(value: &str) -> Option<u32> {
        let start = value.find('(')?;
        let end = value[start + 1..].find(')')? + start + 1;
        let inner = value[start + 1..end].trim();
        let head = inner.split(',').next().unwrap_or(inner).trim();
        head.parse::<u32>().ok()
    }

    fn select_overload_arguments(arguments: &[ProcedureArgument]) -> Vec<ProcedureArgument> {
        let mut selected: Vec<ProcedureArgument> = Vec::new();
        let mut selected_overload: Option<i32> = None;
        for arg in arguments {
            if selected_overload.is_none() {
                selected_overload = arg.overload;
            }
            if arg.overload == selected_overload {
                selected.push(arg.clone());
            } else {
                break;
            }
        }
        selected
    }

    fn is_ref_cursor(arg: &ProcedureArgument) -> bool {
        let data_type = arg.data_type.as_deref().unwrap_or("").to_uppercase();
        if data_type.contains("REF CURSOR") || data_type.contains("REFCURSOR") {
            return true;
        }
        if data_type == "SYS_REFCURSOR" {
            return true;
        }
        if let Some(pls_type) = arg.pls_type.as_deref() {
            let upper = pls_type.to_uppercase();
            if upper.contains("REF CURSOR") || upper.contains("REFCURSOR") {
                return true;
            }
        }
        if let Some(type_name) = arg.type_name.as_deref() {
            if type_name.eq_ignore_ascii_case("REFCURSOR") {
                return true;
            }
        }
        false
    }

    fn format_argument_type(arg: &ProcedureArgument) -> String {
        if let Some(pls_type) = arg.pls_type.as_deref() {
            let trimmed = pls_type.trim();
            if !trimmed.is_empty() {
                if trimmed.contains('%') {
                    return trimmed.to_string();
                }
                let upper = trimmed.to_uppercase();
                if Self::is_string_type_without_length(&upper) {
                    let len = Self::clamp_string_length(arg.data_length);
                    return format!("{}({})", upper, len);
                }
                return trimmed.to_string();
            }
        }
        if let Some(data_type) = arg.data_type.as_deref() {
            let upper = data_type.to_uppercase();
            if upper.contains("REF CURSOR") || upper.contains("REFCURSOR") {
                return "SYS_REFCURSOR".to_string();
            }
            if upper.starts_with("NUMBER") {
                if let Some(precision) = arg.data_precision {
                    if let Some(scale) = arg.data_scale {
                        return format!("NUMBER({}, {})", precision, scale);
                    }
                    return format!("NUMBER({})", precision);
                }
                return "NUMBER".to_string();
            }
            if upper.starts_with("VARCHAR2")
                || upper.starts_with("NVARCHAR2")
                || upper.starts_with("CHAR")
                || upper.starts_with("NCHAR")
                || upper.starts_with("RAW")
            {
                let len = Self::clamp_string_length(arg.data_length);
                return format!("{}({})", upper, len);
            }
            return upper;
        }

        if let Some(type_name) = arg.type_name.as_deref() {
            if let Some(owner) = arg.type_owner.as_deref() {
                return format!("{}.{}", owner, type_name);
            }
            return type_name.to_string();
        }

        "VARCHAR2(4000)".to_string()
    }

    fn is_string_type_without_length(upper: &str) -> bool {
        if upper.contains('(') {
            return false;
        }
        matches!(
            upper,
            "VARCHAR2" | "NVARCHAR2" | "VARCHAR" | "CHAR" | "NCHAR" | "RAW"
        )
    }

    fn clamp_string_length(length: Option<i32>) -> i32 {
        let fallback = 32767;
        let len = length.unwrap_or(fallback);
        let len = if len <= 0 { fallback } else { len };
        len.clamp(1, 32767)
    }

    fn default_value_for_argument(arg: &ProcedureArgument, type_str: &str) -> String {
        if let Some(default_value) = arg.default_value.as_deref() {
            let trimmed = default_value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        if Self::is_ref_cursor(arg) {
            return "NULL".to_string();
        }

        let base = Self::normalize_type_base(type_str);
        if base.contains('.') {
            return "NULL".to_string();
        }

        match base.as_str() {
            "NUMBER" | "NUMERIC" | "DECIMAL" | "INTEGER" | "INT" | "PLS_INTEGER"
            | "BINARY_INTEGER" | "NATURAL" | "NATURALN" | "POSITIVE" | "POSITIVEN"
            | "SIMPLE_INTEGER" => "0".to_string(),
            "FLOAT" | "BINARY_FLOAT" | "BINARY_DOUBLE" => "0".to_string(),
            "VARCHAR2" | "NVARCHAR2" | "VARCHAR" | "CHAR" | "NCHAR" => "''".to_string(),
            "CLOB" | "NCLOB" => "EMPTY_CLOB()".to_string(),
            "BLOB" => "EMPTY_BLOB()".to_string(),
            "RAW" => "HEXTORAW('')".to_string(),
            "DATE" => "SYSDATE".to_string(),
            "TIMESTAMP" => "SYSTIMESTAMP".to_string(),
            "BOOLEAN" => "FALSE".to_string(),
            _ => "NULL".to_string(),
        }
    }

    fn normalize_type_base(type_str: &str) -> String {
        let mut upper = type_str.trim().to_uppercase();
        if let Some(idx) = upper.find('(') {
            upper.truncate(idx);
        }
        if let Some(idx) = upper.find(' ') {
            upper.truncate(idx);
        }
        upper
    }

    fn unique_var_name(base_name: &str, position: i32, used: &mut HashSet<String>) -> String {
        let mut cleaned = base_name
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect::<String>();
        if cleaned.is_empty() {
            cleaned = format!("arg{}", position.max(1));
        }
        if cleaned
            .chars()
            .next()
            .map(|ch| ch.is_ascii_digit())
            .unwrap_or(false)
        {
            cleaned.insert(0, '_');
        }
        let candidate = format!("v_{}", cleaned);
        if used.insert(candidate.clone()) {
            return candidate;
        }

        let mut suffix = 2;
        loop {
            let next = format!("{}_{}", candidate, suffix);
            if used.insert(next.clone()) {
                return next;
            }
            suffix += 1;
        }
    }

    pub fn show_context_menu_for_sql_selection(
        &self,
        selected_text: &str,
        intellisense_data: &IntellisenseData,
    ) -> bool {
        let db_type = match self.current_db_type.lock() {
            Ok(guard) => *guard,
            Err(poisoned) => *poisoned.into_inner(),
        };
        let current_scope = Self::scope_snapshot(&self.selected_scope);
        let cache_snapshot = self
            .object_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(resolved) = Self::resolve_selected_object_context(
            selected_text,
            intellisense_data,
            Some(&cache_snapshot),
            db_type,
            current_scope.as_deref(),
        ) else {
            return false;
        };
        let selected_scope = Self::scope_for_sql_selection_action(
            &resolved.item,
            resolved.selected_scope.as_deref(),
            intellisense_data,
            current_scope.as_deref(),
        );
        if self.defer_unknown_package_routine_context_menu(
            resolved.item.clone(),
            selected_scope.clone(),
            db_type,
        ) {
            return true;
        }

        Self::show_context_menu_for_object_item(
            &self.connection,
            &self.current_db_type,
            &self.write_refusal,
            resolved.item,
            &self.sql_callback,
            &self.status_callback,
            &self.action_sender,
            selected_scope,
        )
    }

    /// Object type and name whose source *is* the declaration of `item`.
    ///
    /// Deliberately not `default_action_for_item`: double-clicking a table in
    /// the tree browses its rows, but "go to declaration" always wants the
    /// definition. A package member has no DDL of its own, so the package is
    /// what opens — the same choice the tree makes for package children.
    fn declaration_target_for_item(item: &ObjectItem) -> Option<(&'static str, String)> {
        match item {
            ObjectItem::Simple {
                object_type,
                object_name,
            } => Self::ddl_object_type(object_type).map(|ddl_type| (ddl_type, object_name.clone())),
            // A column has no source of its own to open.
            ObjectItem::Column { .. } => None,
            ObjectItem::PackageRoutine { package_name, .. } => {
                Some(("PACKAGE", package_name.clone()))
            }
        }
    }

    /// Resolve `selected_text` to an object and open its source in a new editor
    /// tab. Returns `false` when the name matches nothing in the cached scope,
    /// so the caller can try the next candidate.
    pub fn open_declaration_for_sql_selection(
        &self,
        selected_text: &str,
        intellisense_data: &IntellisenseData,
    ) -> bool {
        let db_type = match self.current_db_type.lock() {
            Ok(guard) => *guard,
            Err(poisoned) => *poisoned.into_inner(),
        };
        let current_scope = Self::scope_snapshot(&self.selected_scope);
        let cache_snapshot = self
            .object_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(resolved) = Self::resolve_selected_object_context(
            selected_text,
            intellisense_data,
            Some(&cache_snapshot),
            db_type,
            current_scope.as_deref(),
        ) else {
            return false;
        };
        let selected_scope = Self::scope_for_sql_selection_action(
            &resolved.item,
            resolved.selected_scope.as_deref(),
            intellisense_data,
            current_scope.as_deref(),
        );
        self.open_declaration_for_object_item(&resolved.item, selected_scope)
    }

    /// Object names cached for the current scope.
    pub fn object_cache_snapshot(&self) -> ObjectCache {
        self.object_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Open the source of an already-resolved object in a new editor tab.
    pub fn open_declaration_for_object_item(
        &self,
        item: &ObjectItem,
        selected_scope: Option<String>,
    ) -> bool {
        let Some((object_type, object_name)) = Self::declaration_target_for_item(item) else {
            return false;
        };
        Self::spawn_generate_ddl(
            &self.connection,
            &self.action_sender,
            &self.status_callback,
            selected_scope,
            object_type,
            &object_name,
        );
        true
    }

    fn scope_for_sql_selection_action(
        item: &ObjectItem,
        resolved_scope: Option<&str>,
        intellisense_data: &IntellisenseData,
        current_scope: Option<&str>,
    ) -> Option<String> {
        if let Some(scope) = resolved_scope {
            return Some(scope.to_string());
        }

        if let Some(scope) = current_scope {
            return Some(scope.to_string());
        }

        Self::intellisense_default_scope_for_object(intellisense_data, item)
    }

    fn intellisense_default_scope_for_object(
        data: &IntellisenseData,
        item: &ObjectItem,
    ) -> Option<String> {
        let qualifier = data.default_qualifier()?;
        let object_name = match item {
            ObjectItem::Simple { object_name, .. } => object_name.as_str(),
            ObjectItem::PackageRoutine { package_name, .. } => package_name.as_str(),
            // The scope belongs to the table, so resolve against that.
            ObjectItem::Column { table_name, .. } => table_name.as_str(),
        };

        if !data.qualifier_has_member(qualifier, object_name, false) {
            return None;
        }

        data.default_qualifier_name()
            .or_else(|| data.default_qualifier())
            .map(str::to_string)
    }

    fn defer_unknown_package_routine_context_menu(
        &self,
        item: ObjectItem,
        selected_scope: Option<String>,
        db_type: crate::db::DatabaseType,
    ) -> bool {
        if !object_browser_behavior_for(db_type).supports_package_routines() {
            return false;
        }

        let (package_name, routine_name) = match &item {
            ObjectItem::PackageRoutine {
                package_name,
                routine_name,
                routine_type,
            } if routine_type == "UNKNOWN" => (package_name.clone(), routine_name.clone()),
            _ => return false,
        };

        let qualified_package = object_browser_behavior_for(db_type)
            .qualify_object_name(selected_scope.as_deref(), &package_name);
        let connection = self.connection.clone();
        let sender = self.action_sender.clone();
        let mouse_x = fltk::app::event_x();
        let mouse_y = fltk::app::event_y();
        let scope_generation = self.scope_generation.load(Ordering::Relaxed);
        Self::emit_status_callback(
            &self.status_callback,
            &format!(
                "Resolving package routine type for {}.{}",
                qualified_package, routine_name
            ),
        );

        thread::spawn(move || {
            let activity = format!(
                "Resolving package routine type for {}.{}",
                qualified_package, routine_name
            );
            let result = object_browser_behavior_for(db_type).load_package_routines(
                &connection,
                activity,
                selected_scope.as_deref(),
                &package_name,
            );

            let _ = sender.send(ObjectActionResult::PackageRoutineContextMenu {
                item,
                db_type,
                selected_scope,
                package_name: qualified_package,
                result,
                mouse_x,
                mouse_y,
                scope_generation,
            });
            app::awake();
        });
        true
    }

    fn apply_package_routine_type_from_routines(
        item: &mut ObjectItem,
        routines: &[PackageRoutine],
    ) {
        let routine_name = match item {
            ObjectItem::PackageRoutine { routine_name, .. } => routine_name.clone(),
            _ => return,
        };
        let Some(resolved_type) = routines
            .iter()
            .find(|routine| routine.name.eq_ignore_ascii_case(&routine_name))
            .and_then(|routine| Self::normalize_package_routine_type(&routine.routine_type))
        else {
            return;
        };
        if let ObjectItem::PackageRoutine { routine_type, .. } = item {
            *routine_type = resolved_type;
        }
    }

    fn package_routine_type_is_resolved(item: &ObjectItem) -> bool {
        matches!(
            item,
            ObjectItem::PackageRoutine { routine_type, .. }
                if routine_type.eq_ignore_ascii_case("PROCEDURE")
                    || routine_type.eq_ignore_ascii_case("FUNCTION")
        )
    }

    fn resolve_selected_object_context(
        selected_text: &str,
        data: &IntellisenseData,
        cache: Option<&ObjectCache>,
        db_type: crate::db::DatabaseType,
        current_scope: Option<&str>,
    ) -> Option<ResolvedObjectContext> {
        let parts = Self::selected_object_reference_parts(selected_text)?;
        match parts.as_slice() {
            [name] => Self::resolve_simple_selection_object(name, data, cache),
            [qualifier, name] => {
                Self::resolve_known_package_routine(qualifier, name, data, cache, db_type)
                    .or_else(|| Self::resolve_qualified_schema_object(qualifier, name, data))
                    .or_else(|| {
                        if Self::scope_matches_current_or_default(
                            qualifier,
                            current_scope,
                            data.default_qualifier(),
                        ) {
                            Self::resolve_simple_selection_object(name, data, cache).and_then(
                                |mut context| {
                                    if context
                                        .selected_scope
                                        .as_deref()
                                        .is_some_and(|scope| !scope.eq_ignore_ascii_case(qualifier))
                                    {
                                        return None;
                                    }
                                    if context.selected_scope.is_none() {
                                        context.selected_scope = Some(Self::canonical_scope_name(
                                            data,
                                            qualifier,
                                            current_scope,
                                        ));
                                    }
                                    Some(context)
                                },
                            )
                        } else {
                            None
                        }
                    })
                    .or_else(|| {
                        object_browser_behavior_for(db_type)
                            .supports_package_routines()
                            .then(|| Self::package_name_match(data, cache, qualifier))
                            .flatten()
                            .map(|package_name| {
                                Self::package_routine_context(
                                    None,
                                    &package_name,
                                    name,
                                    data,
                                    cache,
                                )
                            })
                    })
            }
            [owner, package_name, routine_name] => {
                if !object_browser_behavior_for(db_type).supports_package_routines() {
                    return None;
                }
                let resolved_package_name = data
                    .qualifier_member_name_matching_kinds(
                        owner,
                        package_name,
                        &[QualifiedMemberKind::Package],
                    )
                    .or_else(|| {
                        (Self::scope_matches_current_or_default(
                            owner,
                            current_scope,
                            data.default_qualifier(),
                        ))
                        .then(|| Self::cache_name_match(cache, "PACKAGES", package_name))
                        .flatten()
                    });
                if let Some(package_name) = resolved_package_name {
                    let owner = Self::canonical_scope_name(data, owner, current_scope);
                    Some(Self::package_routine_context(
                        Some(owner),
                        &package_name,
                        routine_name,
                        data,
                        cache,
                    ))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn resolve_known_package_routine(
        package_name: &str,
        routine_name: &str,
        data: &IntellisenseData,
        cache: Option<&ObjectCache>,
        db_type: crate::db::DatabaseType,
    ) -> Option<ResolvedObjectContext> {
        if !object_browser_behavior_for(db_type).supports_package_routines() {
            return None;
        }
        let package_name = Self::package_name_match(data, cache, package_name)?;

        let known_package_member =
            Self::qualified_member_matches_kind(
                data,
                &package_name,
                routine_name,
                QualifiedMemberKind::Function,
            ) || Self::qualified_member_matches_kind(
                data,
                &package_name,
                routine_name,
                QualifiedMemberKind::Procedure,
            ) || Self::cached_package_routine_match(cache, None, &package_name, routine_name)
                .is_some();
        known_package_member
            .then(|| Self::package_routine_context(None, &package_name, routine_name, data, cache))
    }

    fn selected_object_reference_parts(selected_text: &str) -> Option<Vec<String>> {
        let trimmed = selected_text
            .trim()
            .trim_matches(|ch| matches!(ch, ';' | ',' | '(' | ')'))
            .trim();
        if trimmed.is_empty() || trimmed.lines().count() > 1 {
            return None;
        }

        let mut parts = Vec::new();
        for part in Self::split_selected_object_reference_parts(trimmed)? {
            parts.push(Self::normalize_selected_object_part(part)?);
        }

        if parts.is_empty() || parts.len() > 3 {
            None
        } else {
            Some(parts)
        }
    }

    fn split_selected_object_reference_parts(selected_text: &str) -> Option<Vec<&str>> {
        let mut parts = Vec::new();
        let mut start = 0usize;
        let mut active_quote: Option<char> = None;
        let mut chars = selected_text.char_indices().peekable();

        while let Some((idx, ch)) = chars.next() {
            if let Some(quote) = active_quote {
                if ch == quote {
                    if chars.peek().is_some_and(|(_, next)| *next == quote) {
                        chars.next();
                    } else {
                        active_quote = None;
                    }
                }
                continue;
            }

            if matches!(ch, '"' | '`') {
                active_quote = Some(ch);
            } else if ch == '[' {
                active_quote = Some(']');
            } else if ch == '.' {
                parts.push(selected_text[start..idx].trim());
                start = idx + ch.len_utf8();
            }
        }

        if active_quote.is_some() {
            return None;
        }

        parts.push(selected_text[start..].trim());
        Some(parts)
    }

    fn normalize_selected_object_part(part: &str) -> Option<String> {
        let part = part
            .trim()
            .trim_matches(|ch| matches!(ch, ';' | ',' | '(' | ')'))
            .trim();
        if part.is_empty() {
            return None;
        }

        let is_quoted = crate::sql_text::is_quoted_identifier(part);
        if !is_quoted
            && (part.starts_with('"')
                || part.ends_with('"')
                || part.contains('"')
                || part.starts_with('`')
                || part.ends_with('`')
                || part.contains('`')
                || part.starts_with('[')
                || part.ends_with(']')
                || part.contains('[')
                || part.contains(']'))
        {
            return None;
        }
        let unquoted = if is_quoted {
            crate::sql_text::strip_identifier_quotes(part)
        } else {
            part.to_string()
        };

        if unquoted.is_empty() {
            return None;
        }
        if !is_quoted && unquoted.chars().any(char::is_whitespace) {
            return None;
        }
        Some(unquoted)
    }

    fn resolve_simple_selection_object(
        name: &str,
        data: &IntellisenseData,
        cache: Option<&ObjectCache>,
    ) -> Option<ResolvedObjectContext> {
        let candidates = [
            (
                "TABLES",
                Self::selection_name_match(&data.tables, name)
                    .or_else(|| Self::cache_name_match(cache, "TABLES", name)),
            ),
            (
                "VIEWS",
                Self::selection_name_match(&data.views, name)
                    .or_else(|| Self::cache_name_match(cache, "VIEWS", name)),
            ),
            (
                "MATERIALIZED VIEWS",
                Self::selection_name_match(&data.materialized_views, name),
            ),
            ("TYPES", Self::selection_name_match(&data.types, name)),
            (
                "PROCEDURES",
                Self::selection_name_match(&data.procedures, name)
                    .or_else(|| Self::cache_name_match(cache, "PROCEDURES", name)),
            ),
            (
                "FUNCTIONS",
                Self::selection_name_match(&data.functions, name)
                    .or_else(|| Self::cache_name_match(cache, "FUNCTIONS", name)),
            ),
            (
                "PACKAGES",
                Self::selection_name_match(&data.packages, name)
                    .or_else(|| Self::cache_name_match(cache, "PACKAGES", name)),
            ),
            (
                "SEQUENCES",
                Self::selection_name_match(&data.sequences, name)
                    .or_else(|| Self::cache_name_match(cache, "SEQUENCES", name)),
            ),
            (
                "TRIGGERS",
                Self::selection_name_match(&data.triggers, name)
                    .or_else(|| Self::cache_name_match(cache, "TRIGGERS", name)),
            ),
            ("INDEXES", Self::selection_name_match(&data.indexes, name)),
            (
                "EVENTS",
                Self::selection_name_match(&data.events, name)
                    .or_else(|| Self::cache_name_match(cache, "EVENTS", name)),
            ),
            (
                "SYNONYMS",
                Self::selection_name_match(&data.synonyms, name)
                    .or_else(|| Self::cache_name_match(cache, "SYNONYMS", name)),
            ),
        ];

        for (object_type, object_name) in candidates {
            if let Some(object_name) = object_name {
                return Some(ResolvedObjectContext {
                    item: ObjectItem::Simple {
                        object_type: object_type.to_string(),
                        object_name,
                    },
                    selected_scope: None,
                });
            }
        }

        if let Some(object_name) = Self::selection_name_match(&data.public_synonyms, name) {
            return Some(ResolvedObjectContext {
                item: ObjectItem::Simple {
                    object_type: "SYNONYMS".to_string(),
                    object_name,
                },
                selected_scope: Some("PUBLIC".to_string()),
            });
        }

        None
    }

    fn resolve_qualified_schema_object(
        qualifier: &str,
        name: &str,
        data: &IntellisenseData,
    ) -> Option<ResolvedObjectContext> {
        let object_kinds = [
            ("TABLES", QualifiedMemberKind::Table),
            ("VIEWS", QualifiedMemberKind::View),
            ("MATERIALIZED VIEWS", QualifiedMemberKind::MaterializedView),
            ("TYPES", QualifiedMemberKind::Type),
            ("PROCEDURES", QualifiedMemberKind::Procedure),
            ("FUNCTIONS", QualifiedMemberKind::Function),
            ("PACKAGES", QualifiedMemberKind::Package),
            ("SEQUENCES", QualifiedMemberKind::Sequence),
            ("TRIGGERS", QualifiedMemberKind::Trigger),
            ("INDEXES", QualifiedMemberKind::Index),
            ("EVENTS", QualifiedMemberKind::Event),
        ];

        for (object_type, kind) in object_kinds {
            if let Some(object_name) =
                data.qualifier_member_name_matching_kinds(qualifier, name, &[kind])
            {
                return Some(ResolvedObjectContext {
                    item: ObjectItem::Simple {
                        object_type: object_type.to_string(),
                        object_name,
                    },
                    selected_scope: Some(Self::canonical_scope_name(data, qualifier, None)),
                });
            }
        }

        let object_name = data.qualifier_member_name_matching_kinds(
            qualifier,
            name,
            &[
                QualifiedMemberKind::Synonym,
                QualifiedMemberKind::PublicSynonym,
            ],
        )?;

        Some(ResolvedObjectContext {
            item: ObjectItem::Simple {
                object_type: "SYNONYMS".to_string(),
                object_name,
            },
            selected_scope: Some(Self::canonical_scope_name(data, qualifier, None)),
        })
    }

    fn package_routine_context(
        owner: Option<String>,
        package_name: &str,
        routine_name: &str,
        data: &IntellisenseData,
        cache: Option<&ObjectCache>,
    ) -> ResolvedObjectContext {
        let package_qualifier = owner
            .as_deref()
            .map(|owner| format!("{}.{}", owner, package_name))
            .unwrap_or_else(|| package_name.to_string());
        let (routine_name, routine_type) = if let Some(routine_name) = data
            .qualifier_member_name_matching_kinds(
                &package_qualifier,
                routine_name,
                &[QualifiedMemberKind::Function],
            ) {
            (routine_name, "FUNCTION".to_string())
        } else if let Some(routine_name) = data.qualifier_member_name_matching_kinds(
            &package_qualifier,
            routine_name,
            &[QualifiedMemberKind::Procedure],
        ) {
            (routine_name, "PROCEDURE".to_string())
        } else if let Some(cached_routine) =
            Self::cached_package_routine_match(cache, owner.as_deref(), package_name, routine_name)
        {
            cached_routine
        } else {
            (routine_name.to_string(), "UNKNOWN".to_string())
        };

        ResolvedObjectContext {
            item: ObjectItem::PackageRoutine {
                package_name: package_name.to_string(),
                routine_name,
                routine_type,
            },
            selected_scope: owner,
        }
    }

    fn selection_name_match(names: &[String], candidate: &str) -> Option<String> {
        let candidate = candidate.trim();
        names
            .iter()
            .find(|name| name.eq_ignore_ascii_case(candidate))
            .cloned()
    }

    fn package_name_match(
        data: &IntellisenseData,
        cache: Option<&ObjectCache>,
        candidate: &str,
    ) -> Option<String> {
        Self::selection_name_match(&data.packages, candidate)
            .or_else(|| Self::cache_name_match(cache, "PACKAGES", candidate))
    }

    fn canonical_scope_name(
        data: &IntellisenseData,
        qualifier: &str,
        current_scope: Option<&str>,
    ) -> String {
        current_scope
            .filter(|scope| scope.eq_ignore_ascii_case(qualifier))
            .map(str::to_string)
            .or_else(|| Self::selection_name_match(&data.users, qualifier))
            .or_else(|| {
                data.default_qualifier_name()
                    .filter(|scope| scope.eq_ignore_ascii_case(qualifier))
                    .map(str::to_string)
            })
            .unwrap_or_else(|| qualifier.to_string())
    }

    fn cache_name_match(
        cache: Option<&ObjectCache>,
        object_type: &str,
        candidate: &str,
    ) -> Option<String> {
        let cache = cache?;
        let names = match object_type {
            "TABLES" => &cache.tables,
            "VIEWS" => &cache.views,
            "PROCEDURES" => &cache.procedures,
            "FUNCTIONS" => &cache.functions,
            "SEQUENCES" => &cache.sequences,
            "TRIGGERS" => &cache.triggers,
            "EVENTS" => &cache.events,
            "SYNONYMS" => &cache.synonyms,
            "PACKAGES" => &cache.packages,
            _ => return None,
        };
        Self::selection_name_match(names, candidate)
    }

    fn cached_package_routine_match(
        cache: Option<&ObjectCache>,
        owner: Option<&str>,
        package_name: &str,
        routine_name: &str,
    ) -> Option<(String, String)> {
        let cache = cache?;
        let owner_qualified_package = owner.map(|owner| format!("{owner}.{package_name}"));
        let package_name_is_literal =
            Self::selection_name_match(&cache.packages, package_name).is_some();
        cache
            .package_routines
            .iter()
            .find(|(cached_package, _)| {
                if let Some(owner_qualified_package) = owner_qualified_package.as_deref() {
                    cached_package.eq_ignore_ascii_case(owner_qualified_package)
                } else {
                    let cached_package_is_literal =
                        Self::selection_name_match(&cache.packages, cached_package).is_some();
                    cached_package.eq_ignore_ascii_case(package_name)
                        || (!package_name_is_literal
                            && package_name.rsplit('.').next().is_some_and(|short_name| {
                                short_name != package_name
                                    && cached_package.eq_ignore_ascii_case(short_name)
                            }))
                        || (!cached_package_is_literal
                            && cached_package.rsplit('.').next().is_some_and(|short_name| {
                                short_name != cached_package.as_str()
                                    && short_name.eq_ignore_ascii_case(package_name)
                            }))
                }
            })
            .and_then(|(_, routines)| {
                routines
                    .iter()
                    .find(|routine| routine.name.eq_ignore_ascii_case(routine_name))
            })
            .and_then(|routine| {
                Self::normalize_package_routine_type(&routine.routine_type)
                    .map(|routine_type| (routine.name.clone(), routine_type))
            })
    }

    fn normalize_package_routine_type(routine_type: &str) -> Option<String> {
        match routine_type.trim().to_ascii_uppercase().as_str() {
            "FUNCTION" => Some("FUNCTION".to_string()),
            "PROCEDURE" => Some("PROCEDURE".to_string()),
            _ => None,
        }
    }

    fn qualified_member_matches_kind(
        data: &IntellisenseData,
        qualifier: &str,
        name: &str,
        kind: QualifiedMemberKind,
    ) -> bool {
        data.qualifier_member_matches_kinds(qualifier, name, &[kind]) == Some(true)
    }

    fn scope_matches_current_or_default(
        qualifier: &str,
        current_scope: Option<&str>,
        default_qualifier: Option<&str>,
    ) -> bool {
        current_scope.is_some_and(|scope| scope.eq_ignore_ascii_case(qualifier))
            || default_qualifier.is_some_and(|scope| scope.eq_ignore_ascii_case(qualifier))
    }

    fn show_context_menu(
        connection: &SharedConnection,
        current_db_type: &Arc<Mutex<crate::db::DatabaseType>>,
        write_refusal: &CardWriteRefusal,
        item: &TreeItem,
        sql_callback: &SqlExecuteCallback,
        status_callback: &StatusCallback,
        action_sender: &std::sync::mpsc::Sender<ObjectActionResult>,
        selected_scope: &Arc<Mutex<Option<String>>>,
        object_cache: &Arc<Mutex<ObjectCache>>,
    ) {
        // Resolved the same way every other tree action resolves it, so a
        // column never reaches the object menu — including a column of a table
        // whose name happens to match a category.
        let item_info = Self::resolve_tree_item(item, object_cache);
        if let Some(item_info) = item_info.filter(|info| !matches!(info, ObjectItem::Column { .. }))
        {
            let selected_scope = Self::scope_snapshot(selected_scope);
            let _ = Self::show_context_menu_for_object_item(
                connection,
                current_db_type,
                write_refusal,
                item_info,
                sql_callback,
                status_callback,
                action_sender,
                selected_scope,
            );
        }
    }

    /// Ask how to import, then build the script that does it.
    ///
    /// `None` means the user cancelled, or said yes to something the builder
    /// refused — either way nothing runs.
    ///
    /// Public only so `verify_import_ui` can drive the production modal; not
    /// part of the supported surface.
    #[doc(hidden)]
    /// Turn a freshly read table into export bytes.
    ///
    /// Goes through the same `render_export_content` the result grid uses, so a
    /// table exported from the tree is byte-identical to the same table
    /// exported from a `Select Data` grid. The one thing that has to be
    /// translated is NULL: a driver reports it as a sentinel, and the
    /// serializers expect the grid's NULL display text.
    fn render_table_export(
        qualified_name: &str,
        db_type: crate::db::DatabaseType,
        choice: crate::ui::result_export_dialog::ExportChoice,
        result: &crate::db::QueryResult,
    ) -> ObjectExportDelivery {
        // No grid here to ask for its NULL text, so use the one a grid
        // starts with; a session `SET NULL` cannot reach this path.
        let null_text = crate::ui::result_table::ResultTableWidget::DEFAULT_NULL_TEXT.to_string();
        let columns: Vec<String> = result
            .columns
            .iter()
            .map(|column| column.name.clone())
            .collect();
        let column_kinds: Vec<crate::db::SqlValueKind> =
            result.columns.iter().map(|column| column.kind).collect();
        let rows: Vec<Vec<String>> = result
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|value| crate::db::QueryCell::display_result_text(value, &null_text))
                    .collect()
            })
            .collect();
        let grid = crate::ui::result_export::ExportGrid {
            columns: columns.clone(),
            column_kinds: column_kinds.clone(),
            rows: rows.clone(),
            null_text: null_text.clone(),
        };
        let sql_selection = crate::ui::grid_sql_export::GridSqlSelection {
            db_type,
            table: Some(qualified_name.to_string()),
            selected_columns: (0..columns.len()).collect(),
            all_columns: columns,
            column_kinds,
            rows,
            null_text,
        };
        let (text, row_count) = crate::ui::result_export::render_export_content(
            choice.format,
            &grid,
            Some(&sql_selection),
        );
        ObjectExportDelivery {
            text: crate::ui::result_export::with_destination_prelude(
                choice.format,
                choice.destination,
                text,
            ),
            format: choice.format,
            destination: choice.destination,
            row_count,
            suggested_name: Self::export_file_stem(qualified_name),
        }
    }

    /// A file name for an exported table: the object name with anything a file
    /// system would argue about replaced.
    fn export_file_stem(qualified_name: &str) -> String {
        let base = qualified_name
            .rsplit('.')
            .next()
            .unwrap_or(qualified_name)
            .trim()
            .trim_matches('"')
            .trim_matches('`');
        let cleaned: String = base
            .chars()
            .map(|ch| if ch.is_alphanumeric() { ch } else { '_' })
            .collect();
        let cleaned = cleaned.trim_matches('_').to_string();
        if cleaned.is_empty() {
            "export".to_string()
        } else {
            cleaned
        }
    }

    pub fn build_import_script_from_dialog(
        file_label: &str,
        text: &str,
        qualified_name: &str,
        db_type: crate::db::DatabaseType,
        columns: &[TableColumnDetail],
        format: crate::ui::result_export::ExportFormat,
    ) -> Option<(String, String)> {
        let targets = Self::import_target_columns(db_type, columns);
        let outcome = crate::ui::table_import_dialog::show(
            file_label,
            text,
            qualified_name,
            &targets,
            format,
        )?;
        let request = crate::ui::table_import::ImportRequest {
            db_type,
            table: qualified_name,
            targets: &targets,
            mapping: &outcome.mapping,
            data: &outcome.data,
            batch_rows: crate::ui::table_import::DEFAULT_BATCH_ROWS,
        };
        match crate::ui::table_import::build_insert_script(&request) {
            Ok(sql) => {
                let summary = crate::ui::table_import::describe(&request)
                    .unwrap_or_else(|_| qualified_name.to_string());
                Some((sql, summary))
            }
            Err(error) => {
                crate::ui::alert_on_main(&error);
                None
            }
        }
    }

    /// The table's columns as the import builder needs them: the declared type
    /// resolved to the literal kind it accepts.
    #[doc(hidden)]
    pub fn import_target_columns(
        db_type: crate::db::DatabaseType,
        columns: &[TableColumnDetail],
    ) -> Vec<crate::ui::table_import::TargetColumn> {
        columns
            .iter()
            .map(|column| crate::ui::table_import::TargetColumn {
                name: column.name.clone(),
                kind: crate::ui::table_import::column_kind_for_data_type(
                    db_type,
                    &column.data_type,
                ),
                nullable: column.nullable,
            })
            .collect()
    }

    /// Which file to import. `None` means the user cancelled the chooser.
    fn ask_import_file_path() -> Option<std::path::PathBuf> {
        let mut dialog = fltk::dialog::FileDialog::new(fltk::dialog::FileDialogType::BrowseFile);
        // Deliberately unfiltered, for the same reason as `File/Open SQL File`:
        // a filter makes FLTK attach an open-panel delegate that dereferences a
        // panel item's missing path and crashes.
        dialog.show();
        let filename = dialog.filename();
        (!filename.as_os_str().is_empty()).then_some(filename)
    }

    fn menu_choices_for_object_item(
        item_info: &ObjectItem,
        db_type: crate::db::DatabaseType,
    ) -> Option<&'static str> {
        object_browser_behavior_for(db_type).menu_choices_for_object_item(item_info)
    }

    /// Menu entries that would send something the database has to write, or
    /// run code that could.
    ///
    /// `Check Compilation`, `View Info`, `View Structure` and `Generate DDL`
    /// are not here: they only read catalog metadata.
    const WRITE_CAPABLE_MENU_LABELS: [&'static str; 6] = [
        "Import Data...",
        DestructiveObjectAction::TRUNCATE_LABEL,
        DestructiveObjectAction::DROP_LABEL,
        "Execute Procedure",
        "Execute Function",
        "Execute Routine",
    ];

    /// `choices` with the write-capable entries removed when the connection is
    /// read-only, or `None` when nothing is left to show.
    ///
    /// The alternative — leaving them in and refusing on click — would put
    /// items in the menu that are guaranteed to fail, which is the thing the
    /// destructive-action design set out to avoid.
    fn menu_choices_for_read_only(choices: &str, read_only: bool) -> Option<String> {
        if !read_only {
            return (!choices.is_empty()).then(|| choices.to_string());
        }
        let kept: Vec<&str> = choices
            .split('|')
            .filter(|label| !Self::WRITE_CAPABLE_MENU_LABELS.contains(label))
            .collect();
        (!kept.is_empty()).then(|| kept.join("|"))
    }

    fn show_context_menu_for_object_item(
        connection: &SharedConnection,
        current_db_type: &Arc<Mutex<crate::db::DatabaseType>>,
        write_refusal: &CardWriteRefusal,
        item_info: ObjectItem,
        sql_callback: &SqlExecuteCallback,
        status_callback: &StatusCallback,
        action_sender: &std::sync::mpsc::Sender<ObjectActionResult>,
        selected_scope: Option<String>,
    ) -> bool {
        Self::show_context_menu_for_object_item_at(
            connection,
            current_db_type,
            write_refusal,
            item_info,
            sql_callback,
            status_callback,
            action_sender,
            selected_scope,
            fltk::app::event_x(),
            fltk::app::event_y(),
        )
    }

    fn show_context_menu_for_object_item_at(
        connection: &SharedConnection,
        current_db_type: &Arc<Mutex<crate::db::DatabaseType>>,
        write_refusal: &CardWriteRefusal,
        item_info: ObjectItem,
        sql_callback: &SqlExecuteCallback,
        status_callback: &StatusCallback,
        action_sender: &std::sync::mpsc::Sender<ObjectActionResult>,
        selected_scope: Option<String>,
        mouse_x: i32,
        mouse_y: i32,
    ) -> bool {
        let db_type = match current_db_type.lock() {
            Ok(guard) => *guard,
            Err(poisoned) => *poisoned.into_inner(),
        };
        let Some(menu_choices) = Self::menu_choices_for_object_item(&item_info, db_type) else {
            return false;
        };
        let Some(menu_choices) =
            Self::menu_choices_for_read_only(menu_choices, write_refusal.writes_are_refused())
        else {
            return false;
        };

        // Prevent menu from being added to parent container
        let current_group = fltk::group::Group::try_current();
        fltk::group::Group::set_current(None::<&fltk::group::Group>);

        let mut menu = fltk::menu::MenuButton::new(mouse_x, mouse_y, 0, 0, None);
        menu.set_color(theme::panel_raised());
        menu.set_text_color(theme::text_primary());
        menu.add_choice(&menu_choices);

        if let Some(ref group) = current_group {
            fltk::group::Group::set_current(Some(group));
        }

        if let Some(choice_item) = menu.popup() {
            let choice_label = choice_item.label().unwrap_or_default();

            let handle_choice = || {
                match (choice_label.as_str(), &item_info) {
                    (
                        label,
                        ObjectItem::Simple {
                            object_name,
                            object_type,
                        },
                    ) if DestructiveObjectAction::from_menu_label(label).is_some() => {
                        let Some(action) = DestructiveObjectAction::from_menu_label(label) else {
                            return;
                        };
                        let qualified_name = Self::qualify_object_name_for_scope(
                            db_type,
                            selected_scope.as_deref(),
                            object_name,
                        );
                        let Some(sql) = Self::destructive_object_sql(
                            db_type,
                            action,
                            selected_scope.as_deref(),
                            object_type,
                            object_name,
                        ) else {
                            Self::emit_status_callback(
                                status_callback,
                                &format!("{} cannot be run on this object", label),
                            );
                            return;
                        };
                        if !confirm_destructive_object_action(action, &qualified_name, &sql) {
                            Self::emit_status_callback(
                                status_callback,
                                &format!("Cancelled: {} was left alone", qualified_name),
                            );
                            return;
                        }
                        Self::emit_status_callback(
                            status_callback,
                            &format!(
                                "Running {} for {}",
                                label.trim_end_matches('.'),
                                qualified_name
                            ),
                        );
                        // Executed through the editor like any other statement,
                        // so the user is left holding the exact SQL that ran.
                        ObjectBrowserWidget::emit_sql_callback(
                            sql_callback,
                            SqlAction::Execute(sql),
                        );
                    }
                    ("Select Data (Top 100)", ObjectItem::Simple { object_name, .. }) => {
                        let qualified_name = Self::qualify_object_name_for_scope(
                            db_type,
                            selected_scope.as_deref(),
                            object_name,
                        );
                        Self::emit_status_callback(
                            status_callback,
                            &format!("Preparing SELECT TOP 100 for {}", qualified_name),
                        );
                        let sql = ObjectBrowserWidget::preview_select_sql(
                            db_type,
                            selected_scope.as_deref(),
                            object_name,
                        );
                        ObjectBrowserWidget::emit_sql_callback(
                            sql_callback,
                            SqlAction::Execute(sql),
                        );
                    }
                    (
                        label @ ("Execute Procedure" | "Execute Function"),
                        ObjectItem::Simple {
                            object_name,
                            object_type,
                        },
                    ) if (label == "Execute Procedure" && object_type == "PROCEDURES")
                        || (label == "Execute Function" && object_type == "FUNCTIONS") =>
                    {
                        let connection = connection.clone();
                        let sender = action_sender.clone();
                        let object_name = object_name.clone();
                        let routine_type = if label == "Execute Function" {
                            "FUNCTION".to_string()
                        } else {
                            "PROCEDURE".to_string()
                        };
                        let selected_scope = selected_scope.clone();
                        Self::emit_status_callback(
                            status_callback,
                            &format!("Loading {} arguments for {}", routine_type, object_name),
                        );
                        thread::spawn(move || {
                            let activity =
                                format!("Loading {} arguments for {}", routine_type, object_name);
                            let mut qualified_name = object_name.clone();
                            let mut db_type = db_type;
                            let result = ObjectBrowserWidget::with_pooled_object_session(
                                &connection,
                                selected_scope.as_deref(),
                                activity,
                                |context, session| {
                                    db_type = context.connection_info.db_type;
                                    let data = object_browser_behavior_for(db_type)
                                        .load_routine_script(
                                            context,
                                            session,
                                            selected_scope.as_deref(),
                                            &object_name,
                                            &routine_type,
                                        )?;
                                    qualified_name = data.qualified_name;
                                    Ok(data.sql)
                                },
                            );

                            let _ = sender.send(ObjectActionResult::RoutineScript {
                                qualified_name,
                                routine_type,
                                db_type,
                                result,
                            });
                            app::awake();
                        });
                    }
                    (
                        label @ ("Execute Procedure" | "Execute Function" | "Execute Routine"),
                        ObjectItem::PackageRoutine {
                            package_name,
                            routine_name,
                            routine_type,
                        },
                    ) if (label == "Execute Procedure" && routine_type == "PROCEDURE")
                        || (label == "Execute Function" && routine_type == "FUNCTION")
                        || (label == "Execute Routine" && routine_type == "UNKNOWN") =>
                    {
                        let connection = connection.clone();
                        let sender = action_sender.clone();
                        let package_name = package_name.clone();
                        let routine_name = routine_name.clone();
                        let routine_type = match label {
                            "Execute Function" => "FUNCTION".to_string(),
                            "Execute Procedure" => "PROCEDURE".to_string(),
                            _ => "UNKNOWN".to_string(),
                        };
                        let selected_scope = selected_scope.clone();
                        let qualified_name = Self::qualify_package_member_name(
                            db_type,
                            selected_scope.as_deref(),
                            &package_name,
                            &routine_name,
                        );
                        let status_routine_type = if routine_type == "UNKNOWN" {
                            "routine".to_string()
                        } else {
                            routine_type.clone()
                        };
                        Self::emit_status_callback(
                            status_callback,
                            &format!(
                                "Loading {} arguments for {}",
                                status_routine_type, qualified_name
                            ),
                        );
                        thread::spawn(move || {
                            let activity = format!(
                                "Loading {} arguments for {}",
                                status_routine_type, qualified_name
                            );
                            let mut resolved_routine_type = routine_type.clone();
                            let result = object_browser_behavior_for(db_type)
                                .load_package_routine_script(
                                    &connection,
                                    activity,
                                    selected_scope.as_deref(),
                                    &package_name,
                                    &routine_name,
                                    &routine_type,
                                )
                                .map(|data| {
                                    resolved_routine_type = data.resolved_routine_type;
                                    data.sql
                                });

                            let _ = sender.send(ObjectActionResult::RoutineScript {
                                qualified_name,
                                routine_type: resolved_routine_type,
                                db_type,
                                result,
                            });
                            app::awake();
                        });
                    }
                    (
                        "Check Compilation",
                        ObjectItem::Simple {
                            object_type,
                            object_name,
                        },
                    ) => {
                        let db_object_type = match object_type.as_str() {
                            "PROCEDURES" => "PROCEDURE",
                            "FUNCTIONS" => "FUNCTION",
                            "PACKAGES" => "PACKAGE",
                            "TRIGGERS" => "TRIGGER",
                            _ => return,
                        };
                        let connection = connection.clone();
                        let sender = action_sender.clone();
                        let object_name = object_name.clone();
                        let object_type = db_object_type.to_string();
                        let selected_scope = selected_scope.clone();
                        Self::emit_status_callback(
                            status_callback,
                            &format!("Checking compilation status for {}", object_name),
                        );
                        thread::spawn(move || {
                            let result = object_browser_behavior_for(db_type)
                                .load_compilation_errors(
                                    &connection,
                                    format!("Checking compilation status for {}", object_name),
                                    selected_scope.as_deref(),
                                    &object_name,
                                    &object_type,
                                );
                            let (status, result) = match result {
                                Ok((status, errors)) => (status, Ok(errors)),
                                Err(err) => (String::new(), Err(err)),
                            };
                            let _ = sender.send(ObjectActionResult::CompilationErrors {
                                object_name,
                                object_type,
                                status,
                                result,
                            });
                            app::awake();
                        });
                    }
                    (
                        "Import Data...",
                        ObjectItem::Simple {
                            object_name,
                            object_type,
                        },
                    ) if object_type == "TABLES" => {
                        let qualified_name = Self::qualify_object_name_for_scope(
                            db_type,
                            selected_scope.as_deref(),
                            object_name,
                        );
                        // The chooser runs here, on the UI thread, so the user
                        // picks a file before any session is taken.
                        let Some(path) = Self::ask_import_file_path() else {
                            return;
                        };
                        let file_label = path
                            .file_name()
                            .map(|name| name.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.to_string_lossy().to_string());
                        let format = crate::ui::result_import::detect_format(&path)
                            .unwrap_or(crate::ui::result_export::ExportFormat::Csv);
                        Self::emit_status_callback(
                            status_callback,
                            &format!("Reading {} for import into {}", file_label, qualified_name),
                        );

                        let connection = connection.clone();
                        let sender = action_sender.clone();
                        let table_name = object_name.clone();
                        let selected_scope = selected_scope.clone();
                        let qualified_for_thread = qualified_name.clone();
                        let file_label_for_thread = file_label.clone();
                        thread::spawn(move || {
                            let activity = format!("Loading table structure for {}", table_name);
                            let result = read_import_file(&path).and_then(|text| {
                                ObjectBrowserWidget::with_pooled_object_session(
                                    &connection,
                                    selected_scope.as_deref(),
                                    activity,
                                    |context, session| {
                                        object_browser_behavior_for(context.connection_info.db_type)
                                            .load_table_structure(
                                                context,
                                                session,
                                                selected_scope.as_deref(),
                                                &table_name,
                                            )
                                    },
                                )
                                .map(|columns| (text, columns))
                            });
                            let _ = sender.send(ObjectActionResult::ImportTarget {
                                qualified_name: qualified_for_thread,
                                file_label: file_label_for_thread,
                                db_type,
                                format,
                                result,
                            });
                            app::awake();
                        });
                    }
                    ("Export Data...", ObjectItem::Simple { object_name, .. }) => {
                        let qualified_name = Self::qualify_object_name_for_scope(
                            db_type,
                            selected_scope.as_deref(),
                            object_name,
                        );
                        // Ask before reading: a whole table is expensive, and a
                        // cancelled dialog must cost nothing. The tree always
                        // knows the dialect, so `SQL Inserts` is on offer here
                        // even though a grid without a connection loses it.
                        let formats = crate::ui::result_export::ExportFormat::ALL.to_vec();
                        let Some(choice) = crate::ui::result_export_dialog::show(&formats, false)
                        else {
                            return;
                        };
                        let connection = connection.clone();
                        let sender = action_sender.clone();
                        let table_name = object_name.clone();
                        let selected_scope = selected_scope.clone();
                        let qualified_for_thread = qualified_name.clone();
                        Self::emit_status_callback(
                            status_callback,
                            &format!("Reading {} for export", qualified_name),
                        );
                        thread::spawn(move || {
                            let activity = format!("Exporting {}", table_name);
                            let result = ObjectBrowserWidget::with_pooled_object_session(
                                &connection,
                                selected_scope.as_deref(),
                                activity,
                                |context, session| {
                                    object_browser_behavior_for(context.connection_info.db_type)
                                        .load_table_rows(
                                            context,
                                            session,
                                            selected_scope.as_deref(),
                                            &table_name,
                                        )
                                },
                            );
                            let _ = sender.send(ObjectActionResult::ExportedTable {
                                qualified_name: qualified_for_thread,
                                db_type,
                                choice,
                                result,
                            });
                            app::awake();
                        });
                    }
                    ("View Structure", ObjectItem::Simple { object_name, .. }) => {
                        let connection = connection.clone();
                        let sender = action_sender.clone();
                        let table_name = object_name.clone();
                        let selected_scope = selected_scope.clone();
                        Self::emit_status_callback(
                            status_callback,
                            &format!("Loading table structure for {}", table_name),
                        );
                        thread::spawn(move || {
                            let result = ObjectBrowserWidget::with_pooled_object_session(
                                &connection,
                                selected_scope.as_deref(),
                                format!("Loading table structure for {}", table_name),
                                |context, session| {
                                    object_browser_behavior_for(context.connection_info.db_type)
                                        .load_table_structure(
                                            context,
                                            session,
                                            selected_scope.as_deref(),
                                            &table_name,
                                        )
                                },
                            );
                            let _ = sender
                                .send(ObjectActionResult::TableStructure { table_name, result });
                            app::awake();
                        });
                    }
                    ("View Indexes", ObjectItem::Simple { object_name, .. }) => {
                        let connection = connection.clone();
                        let sender = action_sender.clone();
                        let table_name = object_name.clone();
                        let selected_scope = selected_scope.clone();
                        Self::emit_status_callback(
                            status_callback,
                            &format!("Loading indexes for {}", table_name),
                        );
                        thread::spawn(move || {
                            let result = ObjectBrowserWidget::with_pooled_object_session(
                                &connection,
                                selected_scope.as_deref(),
                                format!("Loading indexes for {}", table_name),
                                |context, session| {
                                    object_browser_behavior_for(context.connection_info.db_type)
                                        .load_table_indexes(
                                            context,
                                            session,
                                            selected_scope.as_deref(),
                                            &table_name,
                                        )
                                },
                            );
                            let _ = sender
                                .send(ObjectActionResult::TableIndexes { table_name, result });
                            app::awake();
                        });
                    }
                    ("View Constraints", ObjectItem::Simple { object_name, .. }) => {
                        let connection = connection.clone();
                        let sender = action_sender.clone();
                        let table_name = object_name.clone();
                        let selected_scope = selected_scope.clone();
                        Self::emit_status_callback(
                            status_callback,
                            &format!("Loading constraints for {}", table_name),
                        );
                        thread::spawn(move || {
                            let result = ObjectBrowserWidget::with_pooled_object_session(
                                &connection,
                                selected_scope.as_deref(),
                                format!("Loading constraints for {}", table_name),
                                |context, session| {
                                    object_browser_behavior_for(context.connection_info.db_type)
                                        .load_table_constraints(
                                            context,
                                            session,
                                            selected_scope.as_deref(),
                                            &table_name,
                                        )
                                },
                            );
                            let _ = sender
                                .send(ObjectActionResult::TableConstraints { table_name, result });
                            app::awake();
                        });
                    }
                    (
                        "View Info",
                        ObjectItem::Simple {
                            object_type,
                            object_name,
                        },
                    ) => {
                        let connection = connection.clone();
                        let sender = action_sender.clone();
                        let name = object_name.clone();
                        let obj_type = object_type.clone();
                        let selected_scope = selected_scope.clone();
                        Self::emit_status_callback(
                            status_callback,
                            &format!("Loading {} info for {}", obj_type, name),
                        );
                        thread::spawn(move || {
                            let send_err = |sender: &std::sync::mpsc::Sender<
                                ObjectActionResult,
                            >,
                                            obj_type: &str,
                                            msg: &str| {
                                match obj_type {
                                    "SYNONYMS" => {
                                        let _ = sender.send(ObjectActionResult::SynonymInfo(Err(
                                            msg.to_string(),
                                        )));
                                    }
                                    "SEQUENCES" => {
                                        let _ = sender.send(ObjectActionResult::SequenceInfo(Err(
                                            msg.to_string(),
                                        )));
                                    }
                                    other => {
                                        eprintln!("Unexpected object type for View Info: {other}");
                                    }
                                }
                            };

                            let result = ObjectBrowserWidget::with_pooled_object_session(
                                &connection,
                                selected_scope.as_deref(),
                                format!("Loading {} info for {}", obj_type, name),
                                |context, session| {
                                    object_browser_behavior_for(context.connection_info.db_type)
                                        .load_object_info(
                                            context,
                                            session,
                                            selected_scope.as_deref(),
                                            &obj_type,
                                            &name,
                                        )
                                },
                            );
                            match result {
                                Ok(ObjectInfoPayload::Synonym(info)) => {
                                    let _ = sender.send(ObjectActionResult::SynonymInfo(Ok(info)));
                                }
                                Ok(ObjectInfoPayload::Sequence(info)) => {
                                    let _ = sender.send(ObjectActionResult::SequenceInfo(Ok(info)));
                                }
                                Err(err) => {
                                    send_err(&sender, &obj_type, &err);
                                }
                            }
                            app::awake();
                        });
                    }
                    (
                        "Generate DDL",
                        ObjectItem::Simple {
                            object_type,
                            object_name,
                        },
                    ) => {
                        if let Some(obj_type) = Self::ddl_object_type(object_type) {
                            Self::spawn_generate_ddl(
                                connection,
                                action_sender,
                                status_callback,
                                selected_scope.clone(),
                                obj_type,
                                object_name,
                            );
                        }
                    }
                    _ => {}
                };
            };
            handle_choice();
        }

        // FLTK memory management: widgets created without a parent must be deleted.
        fltk::menu::MenuButton::delete(menu);
        true
    }

    fn show_unresolved_package_routine_menu_at(
        item_info: &ObjectItem,
        status_callback: &StatusCallback,
        db_type: crate::db::DatabaseType,
        selected_scope: Option<&str>,
        mouse_x: i32,
        mouse_y: i32,
    ) -> bool {
        let current_group = fltk::group::Group::try_current();
        fltk::group::Group::set_current(None::<&fltk::group::Group>);

        let mut menu = MenuButton::new(mouse_x, mouse_y, 0, 0, None);
        menu.set_color(theme::panel_raised());
        menu.set_text_color(theme::text_primary());
        menu.add(
            "Package routine type unavailable",
            Shortcut::None,
            MenuFlag::Inactive,
            |_| {},
        );
        menu.add("Copy Name", Shortcut::None, MenuFlag::Normal, |_| {});

        if let Some(ref group) = current_group {
            fltk::group::Group::set_current(Some(group));
        }

        if let Some(choice_item) = menu.popup() {
            if choice_item.label().as_deref() == Some("Copy Name") {
                let text =
                    Self::copy_text_for_object_item_with_scope(item_info, db_type, selected_scope);
                app::copy(&text);
                Self::emit_status_callback(
                    status_callback,
                    &format!("Copied '{}' to clipboard", text),
                );
            }
        }

        MenuButton::delete(menu);
        true
    }

    fn result_column(name: &str, data_type: &str) -> ColumnInfo {
        ColumnInfo {
            name: name.to_string(),
            data_type: data_type.to_string(),
            kind: crate::db::SqlValueKind::Unknown,
        }
    }

    fn build_result_tab_request(
        label: String,
        columns: Vec<ColumnInfo>,
        rows: Vec<Vec<String>>,
        message: String,
    ) -> ResultTabRequest {
        ResultTabRequest {
            label,
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

    fn build_table_structure_result_request(
        table_name: &str,
        columns: &[TableColumnDetail],
    ) -> ResultTabRequest {
        let rows = columns
            .iter()
            .map(|column| {
                vec![
                    column.name.clone(),
                    column.get_type_display(),
                    if column.nullable {
                        "YES".to_string()
                    } else {
                        "NO".to_string()
                    },
                    if column.is_primary_key {
                        "PK".to_string()
                    } else {
                        String::new()
                    },
                ]
            })
            .collect();
        Self::build_result_tab_request(
            format!("Structure: {table_name}"),
            vec![
                Self::result_column("Column Name", "VARCHAR2"),
                Self::result_column("Data Type", "VARCHAR2"),
                Self::result_column("Nullable", "VARCHAR2"),
                Self::result_column("PK", "VARCHAR2"),
            ],
            rows,
            format!("Loaded table structure for {table_name}"),
        )
    }

    fn build_table_indexes_result_request(
        table_name: &str,
        indexes: &[IndexInfo],
    ) -> ResultTabRequest {
        let rows = indexes
            .iter()
            .map(|index| {
                vec![
                    index.name.clone(),
                    if index.is_unique {
                        "YES".to_string()
                    } else {
                        "NO".to_string()
                    },
                    index.columns.clone(),
                ]
            })
            .collect();
        Self::build_result_tab_request(
            format!("Indexes: {table_name}"),
            vec![
                Self::result_column("Index Name", "VARCHAR2"),
                Self::result_column("Unique", "VARCHAR2"),
                Self::result_column("Columns", "VARCHAR2"),
            ],
            rows,
            format!("Loaded table indexes for {table_name}"),
        )
    }

    fn build_table_constraints_result_request(
        table_name: &str,
        constraints: &[ConstraintInfo],
    ) -> ResultTabRequest {
        let rows = constraints
            .iter()
            .map(|constraint| {
                vec![
                    constraint.name.clone(),
                    constraint.constraint_type.clone(),
                    constraint.columns.clone(),
                    constraint.ref_table.clone().unwrap_or_default(),
                ]
            })
            .collect();
        Self::build_result_tab_request(
            format!("Constraints: {table_name}"),
            vec![
                Self::result_column("Constraint Name", "VARCHAR2"),
                Self::result_column("Type", "VARCHAR2"),
                Self::result_column("Columns", "VARCHAR2"),
                Self::result_column("Ref Table", "VARCHAR2"),
            ],
            rows,
            format!("Loaded table constraints for {table_name}"),
        )
    }

    fn build_sequence_info_result_request(info: &SequenceInfo) -> ResultTabRequest {
        let rows = vec![
            vec!["Name".to_string(), info.name.clone()],
            vec!["Min Value".to_string(), info.min_value.clone()],
            vec!["Max Value".to_string(), info.max_value.clone()],
            vec!["Increment By".to_string(), info.increment_by.clone()],
            vec!["Cycle".to_string(), info.cycle_flag.clone()],
            vec!["Order".to_string(), info.order_flag.clone()],
            vec!["Cache Size".to_string(), info.cache_size.clone()],
            vec!["Last Number".to_string(), info.last_number.clone()],
            vec![
                "Note".to_string(),
                "LAST_NUMBER is the next value to be generated.".to_string(),
            ],
        ];
        Self::build_result_tab_request(
            format!("Sequence: {}", info.name),
            vec![
                Self::result_column("Property", "VARCHAR2"),
                Self::result_column("Value", "VARCHAR2"),
            ],
            rows,
            format!("Loaded sequence info for {}", info.name),
        )
    }

    fn build_synonym_info_result_request(info: &SynonymInfo) -> ResultTabRequest {
        let mut rows = vec![
            vec!["Name".to_string(), info.name.clone()],
            vec!["Table Owner".to_string(), info.table_owner.clone()],
            vec!["Table Name".to_string(), info.table_name.clone()],
        ];
        if !info.db_link.is_empty() {
            rows.push(vec!["DB Link".to_string(), info.db_link.clone()]);
        }
        Self::build_result_tab_request(
            format!("Synonym: {}", info.name),
            vec![
                Self::result_column("Property", "VARCHAR2"),
                Self::result_column("Value", "VARCHAR2"),
            ],
            rows,
            format!("Loaded synonym info for {}", info.name),
        )
    }

    fn build_compilation_result_request(
        object_name: &str,
        object_type: &str,
        status: &str,
        errors: &[CompilationError],
    ) -> ResultTabRequest {
        if errors.is_empty() {
            return Self::build_result_tab_request(
                format!("Compile: {object_name}"),
                vec![
                    Self::result_column("Status", "VARCHAR2"),
                    Self::result_column("Message", "VARCHAR2"),
                ],
                vec![vec![
                    status.to_string(),
                    format!("No compilation errors found for {object_type}."),
                ]],
                format!("Loaded compilation status for {object_name}"),
            );
        }

        let rows = errors
            .iter()
            .map(|error| {
                vec![
                    error.line.to_string(),
                    error.position.to_string(),
                    error.attribute.clone(),
                    error.text.clone(),
                ]
            })
            .collect();
        Self::build_result_tab_request(
            format!("Compile: {object_name}"),
            vec![
                Self::result_column("Line", "NUMBER"),
                Self::result_column("Position", "NUMBER"),
                Self::result_column("Type", "VARCHAR2"),
                Self::result_column("Message", "VARCHAR2"),
            ],
            rows,
            format!("Loaded compilation status for {object_name} ({status})"),
        )
    }

    pub fn set_sql_callback<F>(&mut self, callback: F)
    where
        F: FnMut(SqlAction) + 'static,
    {
        *self
            .sql_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    fn panic_payload_to_string(payload: &(dyn Any + Send)) -> String {
        if let Some(msg) = payload.downcast_ref::<&str>() {
            (*msg).to_string()
        } else if let Some(msg) = payload.downcast_ref::<String>() {
            msg.clone()
        } else {
            "unknown panic payload".to_string()
        }
    }

    fn log_callback_panic(context: &str, payload: &(dyn Any + Send)) {
        let panic_payload = Self::panic_payload_to_string(payload);
        crate::utils::logging::log_error(
            "object_browser::callback",
            &format!("{context} panicked: {panic_payload}"),
        );
        eprintln!("{context} panicked: {panic_payload}");
    }

    fn emit_sql_callback(callback_slot: &SqlExecuteCallback, action: SqlAction) {
        let callback = {
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            slot.take()
        };

        if let Some(mut cb) = callback {
            let call_result = panic::catch_unwind(AssertUnwindSafe(|| cb(action)));
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot.is_none() {
                *slot = Some(cb);
            }
            if let Err(payload) = call_result {
                Self::log_callback_panic("SQL callback", payload.as_ref());
            }
        }
    }

    fn emit_status_callback(callback_slot: &StatusCallback, message: &str) {
        let callback = {
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            slot.take()
        };

        if let Some(mut cb) = callback {
            let call_result = panic::catch_unwind(AssertUnwindSafe(|| cb(message)));
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot.is_none() {
                *slot = Some(cb);
            }
            if let Err(payload) = call_result {
                Self::log_callback_panic("status callback", payload.as_ref());
            }
        }
    }

    fn emit_metadata_callback(
        callback_slot: &MetadataCallback,
        snapshot: ObjectBrowserMetadataSnapshot,
    ) {
        let callback = {
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            slot.take()
        };

        if let Some(mut cb) = callback {
            let call_result = panic::catch_unwind(AssertUnwindSafe(|| cb(snapshot)));
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot.is_none() {
                *slot = Some(cb);
            }
            if let Err(payload) = call_result {
                Self::log_callback_panic("metadata callback", payload.as_ref());
            }
        }
    }

    fn emit_status(&self, message: &str) {
        Self::emit_status_callback(&self.status_callback, message);
    }

    pub fn set_status_callback<F>(&mut self, callback: F)
    where
        F: FnMut(&str) + 'static,
    {
        *self
            .status_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    pub fn set_metadata_callback<F>(&mut self, callback: F)
    where
        F: FnMut(ObjectBrowserMetadataSnapshot) + 'static,
    {
        *self
            .metadata_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    /// Clear the object browser tree and cache without triggering a network refetch.
    /// Called when the database connection is closed or lost.
    pub fn clear_on_disconnect(&mut self) {
        // A disconnect does not unblock a metadata job on its own: the job holds
        // a leased session, and the OCI and MySQL pools close to a no-op. Break
        // the sessions here so the load stops instead of running to completion
        // against a connection the user already closed.
        self.cancel_metadata_refresh();
        self.scope_generation.fetch_add(1, Ordering::Relaxed);
        self.scope_switch_in_progress
            .store(false, Ordering::Release);
        self.refresh_connection_generation
            .fetch_add(1, Ordering::Relaxed);
        // The catalog goes away with the connection, so this card is no
        // longer something a new card may inherit from, and there is nothing
        // left for a deferred draw to draw.
        self.catalog.invalidate();
        self.tree_rebuild_pending.store(false, Ordering::Release);
        self.clear_pending_tree_refresh();
        self.clear_items();
        self.filter_input.set_value("");
        self.scope_choice.clear();
        self.scope_choice.deactivate();
        self.scope_label
            .set_label(Self::scope_label_text(crate::db::DatabaseType::default()));
        *self
            .scope_options
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Vec::new();
        *self
            .selected_scope
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .object_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = ObjectCache::default();
        self.tree.redraw();
    }

    pub fn refresh(&mut self) -> bool {
        let Some(context) = Self::metadata_pool_session_context(&self.connection) else {
            self.emit_status(&format_connection_busy_message());
            return false;
        };
        self.refresh_with_context(context)
    }

    pub fn refresh_with_context(&mut self, context: crate::db::DbPoolSessionContext) -> bool {
        if !context.is_current() {
            return false;
        }
        let db_type = context.connection_info.db_type;
        let requested_scope = self.selected_scope();
        let activity_guard = context.track_activity(Self::scope_refresh_status_message(
            db_type,
            requested_scope.as_deref(),
        ));
        // A refresh that is already running is superseded by this one, so it is
        // cancelled rather than left to finish against the old scope.
        self.cancel_metadata_refresh();
        *self
            .in_flight_metadata_refresh
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(InFlightMetadataRefresh {
            activity_id: activity_guard.id(),
            activity: activity_guard.finish_handle(),
        });
        let connection_generation = context.connection_generation;
        // Load started: the ask is recorded and the old catalog stops
        // counting, in one step.
        self.catalog.load_started(requested_scope.clone());
        self.refresh_connection_generation
            .store(connection_generation, Ordering::Relaxed);
        self.clear_pending_tree_refresh();
        // Any deferred draw of an ADOPTED catalog is cancelled with it: the
        // poll fills the tree in batches from here on, and a deferred draw
        // firing in the middle of that would double up its nodes.
        self.tree_rebuild_pending.store(false, Ordering::Release);
        // First clear items and filter
        self.clear_items();
        self.filter_input.set_value("");
        *self
            .object_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = ObjectCache::default();
        self.emit_status("Refreshing object browser metadata");

        let _ = self.refresh_request_sender.send(RefreshRequest::Metadata {
            selected_scope: requested_scope,
            scope_generation: self.scope_generation.load(Ordering::Relaxed),
            context,
            activity_guard,
        });
        true
    }

    fn cancel_timeout(&self) -> Duration {
        crate::utils::config::AppConfig::runtime_cancel_timeout()
    }

    /// Drop the record of a refresh that the activity registry already retired,
    /// so a load cancelled through the registry is not offered again.
    pub fn forget_cancelled_metadata_refresh(&mut self) {
        let mut in_flight = self
            .in_flight_metadata_refresh
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if in_flight
            .as_ref()
            .is_some_and(|refresh| !refresh.activity.is_active())
        {
            *in_flight = None;
            drop(in_flight);
            self.clear_pending_tree_refresh();
        }
    }

    /// Whether a metadata load is still running and still owns a status entry.
    pub fn metadata_refresh_in_flight(&self) -> bool {
        self.in_flight_metadata_refresh
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_some_and(|refresh| refresh.activity.is_active())
    }

    /// Stop the in-flight metadata load. Returns whether there was one to stop.
    ///
    /// The status entry is released here rather than when the worker returns:
    /// the worker can still be unwinding a broken session for a while, and the
    /// user has already been told the load is over.
    pub fn cancel_metadata_refresh(&mut self) -> bool {
        let refresh = self
            .in_flight_metadata_refresh
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some(refresh) = refresh.filter(|refresh| refresh.activity.is_active()) else {
            return false;
        };
        // The registry breaks every session still open under this activity and
        // retires the entry, so the status bar stops showing the load even
        // though the worker can still be unwinding a broken session.
        crate::db::cancel_db_activity(refresh.activity_id, self.cancel_timeout());
        refresh.activity.finish();
        // Anything the worker still delivers belongs to a load the user ended,
        // so make the result fail the generation check on arrival.
        self.refresh_connection_generation
            .fetch_add(1, Ordering::Relaxed);
        self.clear_pending_tree_refresh();
        true
    }

    fn spawn_refresh_worker(
        refresh_request_receiver: Receiver<RefreshRequest>,
        refresh_sender: Sender<RefreshEvent>,
    ) {
        thread::spawn(move || {
            while let Ok(request) = Self::recv_latest_refresh_request(&refresh_request_receiver) {
                match request {
                    RefreshRequest::Metadata {
                        selected_scope,
                        scope_generation,
                        context,
                        activity_guard,
                    } => {
                        let connection_generation = context.connection_generation;
                        match panic::catch_unwind(AssertUnwindSafe(|| {
                            Self::load_metadata_cache(context, selected_scope, &activity_guard)
                        })) {
                            Ok(Some((db_type, cache, available_scopes, selected_scope))) => {
                                let _ = refresh_sender.send(RefreshEvent::Finished {
                                    cache: Box::new(cache),
                                    db_type,
                                    available_scopes,
                                    selected_scope,
                                    scope_generation,
                                    connection_generation,
                                    activity_guard,
                                });
                                app::awake();
                            }
                            Ok(None) => {
                                Self::send_refresh_failure(
                                    &refresh_sender,
                                    "Object browser metadata refresh failed.".to_string(),
                                    scope_generation,
                                    connection_generation,
                                    activity_guard,
                                );
                            }
                            Err(payload) => {
                                let panic_msg = Self::panic_payload_to_string(payload.as_ref());
                                crate::utils::logging::log_error(
                                    "object_browser::metadata_refresh",
                                    &format!("metadata refresh worker panicked: {panic_msg}"),
                                );
                                eprintln!("metadata refresh worker panicked: {panic_msg}");
                                Self::send_refresh_failure(
                                    &refresh_sender,
                                    format!("Object browser metadata refresh failed: {panic_msg}"),
                                    scope_generation,
                                    connection_generation,
                                    activity_guard,
                                );
                            }
                        }
                    }
                }
            }
        });
    }

    fn send_refresh_failure(
        refresh_sender: &Sender<RefreshEvent>,
        message: String,
        scope_generation: u64,
        connection_generation: u64,
        activity_guard: crate::db::DbActivityGuard,
    ) {
        let _ = refresh_sender.send(RefreshEvent::Failed {
            message,
            scope_generation,
            connection_generation,
            activity_guard,
        });
        app::awake();
    }

    fn recv_latest_refresh_request(
        refresh_request_receiver: &Receiver<RefreshRequest>,
    ) -> Result<RefreshRequest, RecvError> {
        let mut latest_request = refresh_request_receiver.recv()?;
        loop {
            match refresh_request_receiver.try_recv() {
                Ok(next_request) => {
                    latest_request = next_request;
                }
                Err(TryRecvError::Empty) => return Ok(latest_request),
                Err(TryRecvError::Disconnected) => return Ok(latest_request),
            }
        }
    }

    fn metadata_pool_session_context(
        connection: &SharedConnection,
    ) -> Option<crate::db::DbPoolSessionContext> {
        match crate::db::pool_session_context_for_shared_connection(
            connection,
            Some("Preparing object browser metadata refresh"),
        ) {
            Ok(context) => Some(context),
            Err(err) => {
                eprintln!("Warning: failed to prepare object browser metadata session: {err}");
                None
            }
        }
    }

    fn load_metadata_cache(
        context: crate::db::DbPoolSessionContext,
        requested_scope: Option<String>,
        activity: &crate::db::DbActivityGuard,
    ) -> Option<(
        crate::db::DatabaseType,
        ObjectCache,
        Vec<String>,
        Option<String>,
    )> {
        let db_type = context.connection_info.db_type;
        if activity.is_finished() {
            return None;
        }
        object_browser_behavior_for(db_type).load_metadata_cache(context, requested_scope, activity)
    }

    fn clear_items(&mut self) {
        Self::clear_tree_items(&mut self.tree);
    }

    fn clear_pending_tree_refresh(&self) -> bool {
        let mut pending = self
            .pending_tree_refresh
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let had_pending = pending.is_some();
        *pending = None;
        had_pending
    }

    fn clear_tree_items(tree: &mut Tree) {
        for category in Self::all_root_categories() {
            if let Some(item) = tree.find_item(category) {
                while item.has_children() {
                    if let Some(child) = item.child(0) {
                        let _ = tree.remove(&child);
                    } else {
                        break;
                    }
                }
            }
        }
    }

    fn all_root_categories() -> &'static [&'static str] {
        &[
            "Tables",
            "Views",
            "Procedures",
            "Functions",
            "Sequences",
            "Triggers",
            "Events",
            "Synonyms",
            "Packages",
        ]
    }

    fn root_categories_for_db_type(
        db_type: crate::db::DatabaseType,
        cache: &ObjectCache,
    ) -> Vec<&'static str> {
        object_browser_behavior_for(db_type).root_categories(cache)
    }

    fn rebuild_root_categories_for_db_type(
        tree: &mut Tree,
        db_type: crate::db::DatabaseType,
        cache: &ObjectCache,
    ) {
        for category in Self::all_root_categories() {
            if let Some(item) = tree.find_item(category) {
                let _ = tree.remove(&item);
            }
        }

        for category in Self::root_categories_for_db_type(db_type, cache) {
            tree.add(category);
            if let Some(mut item) = tree.find_item(category) {
                item.close();
            }
        }
    }

    fn populate_tree(tree: &mut Tree, cache: &ObjectCache, filter_text: &str) {
        Self::clear_tree_items(tree);
        for path in Self::collect_tree_paths(cache, filter_text) {
            tree.add(&path);
        }
    }

    /// What a column node reads as in the tree.
    ///
    /// The type is part of the label because seeing it is the reason to expand
    /// a table at all — otherwise `View Structure` would still be the only way.
    fn column_node_label(column: &TableColumnDetail) -> String {
        let type_display = column.get_type_display();
        if type_display.is_empty() {
            column.name.clone()
        } else {
            format!("{}  {}", column.name, type_display)
        }
    }

    /// The column a node label names, found by regenerating each cached
    /// column's label and comparing.
    ///
    /// Deliberately not parsed back out of the label: a column name can contain
    /// spaces, and a parsing rule would be a second definition of the format
    /// that could drift from [`Self::column_node_label`].
    fn column_name_for_node_label(columns: &[TableColumnDetail], label: &str) -> Option<String> {
        columns
            .iter()
            .find(|column| Self::column_node_label(column) == label)
            .map(|column| column.name.clone())
    }

    fn collect_tree_paths(cache: &ObjectCache, filter_text: &str) -> Vec<String> {
        let mut paths: Vec<String> = Vec::new();
        for table in &cache.tables {
            if filter_text.is_empty() || table.to_lowercase().contains(filter_text) {
                paths.push(format!("Tables/{}", table));
                // Columns only exist here once the table has been expanded
                // once. Emitting them from the cache is what keeps them alive
                // across the rebuild that every filter keystroke triggers.
                for column in cache.table_columns.get(table).into_iter().flatten() {
                    paths.push(format!(
                        "Tables/{}/{}",
                        table,
                        Self::column_node_label(column)
                    ));
                }
            }
        }
        for view in &cache.views {
            if filter_text.is_empty() || view.to_lowercase().contains(filter_text) {
                paths.push(format!("Views/{}", view));
            }
        }
        for procedure in &cache.procedures {
            if filter_text.is_empty() || procedure.to_lowercase().contains(filter_text) {
                paths.push(format!("Procedures/{}", procedure));
            }
        }
        for func in &cache.functions {
            if filter_text.is_empty() || func.to_lowercase().contains(filter_text) {
                paths.push(format!("Functions/{}", func));
            }
        }
        for seq in &cache.sequences {
            if filter_text.is_empty() || seq.to_lowercase().contains(filter_text) {
                paths.push(format!("Sequences/{}", seq));
            }
        }
        for trig in &cache.triggers {
            if filter_text.is_empty() || trig.to_lowercase().contains(filter_text) {
                paths.push(format!("Triggers/{}", trig));
            }
        }
        for event in &cache.events {
            if filter_text.is_empty() || event.to_lowercase().contains(filter_text) {
                paths.push(format!("Events/{}", event));
            }
        }
        for syn in &cache.synonyms {
            if filter_text.is_empty() || syn.to_lowercase().contains(filter_text) {
                paths.push(format!("Synonyms/{}", syn));
            }
        }

        for package in &cache.packages {
            let routines = cache
                .package_routines
                .get(package)
                .cloned()
                .unwrap_or_default();
            let package_matches =
                filter_text.is_empty() || package.to_lowercase().contains(filter_text);
            let matching_routines: Vec<PackageRoutine> = routines
                .into_iter()
                .filter(|routine| {
                    filter_text.is_empty()
                        || routine.name.to_lowercase().contains(filter_text)
                        || package_matches
                })
                .collect();

            if package_matches || !matching_routines.is_empty() {
                paths.push(format!("Packages/{}", package));
                for routine in matching_routines {
                    if routine.routine_type == "FUNCTION" {
                        paths.push(format!("Packages/{}/Functions/{}", package, routine.name));
                    } else {
                        paths.push(format!("Packages/{}/Procedures/{}", package, routine.name));
                    }
                }
            }
        }

        paths
    }

    /// Every node path in this browser's tree, for the capture tour.
    #[doc(hidden)]
    #[doc(hidden)]
    pub fn capture_tour_pick_scope(&self, scope: Option<String>) {
        let db_type = *self
            .current_db_type
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self::complete_scope_change(
            &self.selected_scope,
            &self.catalog,
            &self.status_callback,
            &self.scope_change_callback,
            db_type,
            scope,
        );
    }

    pub fn capture_tour_tree_paths(&self) -> Vec<String> {
        self.tree
            .get_items()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|item| self.tree.item_pathname(&item).ok())
            .collect()
    }

    /// Open one tree node by path, for the capture tour.
    #[doc(hidden)]
    pub fn capture_tour_expand_path(&self, path: &str) -> bool {
        let Some(mut item) = self.tree.find_item(path) else {
            return false;
        };
        item.open();
        let mut tree = self.tree.clone();
        tree.redraw();
        true
    }

    #[allow(dead_code)]
    pub fn get_selected_item(&self) -> Option<String> {
        self.tree
            .first_selected_item()
            .and_then(|item| Self::copy_text_for_selected_item(&item, &self.object_cache))
    }

    pub fn has_focus(&self) -> bool {
        widget_has_focus(&self.flex)
    }

    pub fn copy_focused_selection_to_clipboard(&self) -> bool {
        if widget_has_focus(&self.filter_input) {
            let mut filter_input = self.filter_input.clone();
            return filter_input.copy().is_ok();
        }

        if !widget_has_focus(&self.tree) {
            return false;
        }

        let Some(item) = self.tree.first_selected_item() else {
            return false;
        };
        let Some(text) = Self::copy_text_for_selected_item(&item, &self.object_cache) else {
            return false;
        };

        app::copy(&text);
        Self::emit_status_callback(
            &self.status_callback,
            &format!("Copied '{}' to clipboard", text),
        );
        true
    }
}

impl ObjectBrowserDbBehavior for OracleObjectBrowserBehavior {
    fn qualify_object_name(&self, selected_scope: Option<&str>, object_name: &str) -> String {
        ObjectBrowserWidget::qualify_oracle_object_name(selected_scope, object_name)
    }

    fn qualify_package_member_name(
        &self,
        selected_scope: Option<&str>,
        package_name: &str,
        routine_name: &str,
    ) -> String {
        let package_name = self.qualify_object_name(selected_scope, package_name);
        format!(
            "{}.{}",
            package_name,
            crate::db::DatabaseConnection::quote_oracle_identifier(routine_name)
        )
    }

    fn preview_select_sql(&self, selected_scope: Option<&str>, object_name: &str) -> String {
        let qualified_name = self.qualify_object_name(selected_scope, object_name);
        format!("SELECT * FROM {} WHERE ROWNUM <= 100", qualified_name)
    }

    fn export_select_sql(&self, selected_scope: Option<&str>, object_name: &str) -> String {
        format!(
            "SELECT * FROM {}",
            self.qualify_object_name(selected_scope, object_name)
        )
    }

    fn destructive_object_sql(
        &self,
        action: DestructiveObjectAction,
        selected_scope: Option<&str>,
        object_type: &str,
        object_name: &str,
    ) -> Option<String> {
        // No CASCADE CONSTRAINTS and no PURGE: a wider statement than the one
        // the user read is exactly what the confirmation is there to prevent.
        // A table that will not drop reports its own error instead.
        let keyword = match action {
            DestructiveObjectAction::Truncate => match object_type {
                "TABLES" => "TABLE",
                _ => return None,
            },
            DestructiveObjectAction::Drop => match object_type {
                "TABLES" => "TABLE",
                "VIEWS" => "VIEW",
                "MATERIALIZED VIEWS" => "MATERIALIZED VIEW",
                "PROCEDURES" => "PROCEDURE",
                "FUNCTIONS" => "FUNCTION",
                "SEQUENCES" => "SEQUENCE",
                "TRIGGERS" => "TRIGGER",
                "SYNONYMS" => "SYNONYM",
                "PACKAGES" => "PACKAGE",
                _ => return None,
            },
        };
        let qualified_name = self.qualify_object_name(selected_scope, object_name);
        Some(match action {
            DestructiveObjectAction::Truncate => format!("TRUNCATE {} {}", keyword, qualified_name),
            DestructiveObjectAction::Drop => format!("DROP {} {}", keyword, qualified_name),
        })
    }

    fn build_simple_procedure_script(&self, qualified_name: &str) -> String {
        ObjectBrowserWidget::build_simple_procedure_script(qualified_name)
    }

    fn build_simple_function_script(&self, qualified_name: &str) -> String {
        ObjectBrowserWidget::build_simple_function_script(qualified_name)
    }

    fn build_routine_script(
        &self,
        qualified_name: &str,
        _routine_type: &str,
        arguments: &[ProcedureArgument],
    ) -> String {
        ObjectBrowserWidget::build_procedure_script(qualified_name, arguments)
    }

    fn action_scope<'a>(
        &self,
        selected_scope: Option<&'a str>,
        _context: &'a crate::db::DbPoolSessionContext,
    ) -> Option<&'a str> {
        selected_scope
    }

    fn load_routine_script(
        &self,
        _context: &crate::db::DbPoolSessionContext,
        session: &mut crate::db::DbPoolSession,
        selected_scope: Option<&str>,
        object_name: &str,
        routine_type: &str,
    ) -> Result<RoutineScriptData, String> {
        let qualified_name = self.qualify_object_name(selected_scope, object_name);
        let arguments = match session {
            crate::db::DbPoolSession::Oracle(conn) => {
                ObjectBrowser::get_procedure_arguments(conn, &qualified_name)
                    .map_err(|err| err.to_string())?
            }
            crate::db::DbPoolSession::OracleThin(conn) => {
                ObjectBrowser::get_thin_procedure_arguments(conn, &qualified_name)?
            }
            unexpected @ crate::db::DbPoolSession::MySQL { .. } => {
                return Err(format!(
                    "Expected Oracle object action session but acquired {}",
                    unexpected.db_type()
                ))
            }
        };
        Ok(RoutineScriptData {
            qualified_name: qualified_name.clone(),
            resolved_routine_type: routine_type.to_string(),
            sql: self.build_routine_script(&qualified_name, routine_type, &arguments),
        })
    }

    fn load_table_structure(
        &self,
        _context: &crate::db::DbPoolSessionContext,
        session: &mut crate::db::DbPoolSession,
        selected_scope: Option<&str>,
        table_name: &str,
    ) -> Result<Vec<TableColumnDetail>, String> {
        let qualified_name = self.qualify_object_name(selected_scope, table_name);
        match session {
            crate::db::DbPoolSession::Oracle(conn) => {
                ObjectBrowser::get_table_structure(conn, &qualified_name)
                    .map_err(|err| err.to_string())
            }
            crate::db::DbPoolSession::OracleThin(conn) => {
                ObjectBrowser::get_thin_table_structure(conn, &qualified_name)
            }
            unexpected @ crate::db::DbPoolSession::MySQL { .. } => Err(format!(
                "Expected Oracle object action session but acquired {}",
                unexpected.db_type()
            )),
        }
    }

    fn load_table_rows(
        &self,
        _context: &crate::db::DbPoolSessionContext,
        session: &mut crate::db::DbPoolSession,
        selected_scope: Option<&str>,
        table_name: &str,
    ) -> Result<crate::db::QueryResult, String> {
        let sql = self.export_select_sql(selected_scope, table_name);
        match session {
            crate::db::DbPoolSession::Oracle(conn) => {
                ObjectBrowser::execute_oci_query(conn, &sql).map_err(|err| err.to_string())
            }
            crate::db::DbPoolSession::OracleThin(conn) => {
                ObjectBrowser::execute_thin_query(conn, &sql)
            }
            unexpected @ crate::db::DbPoolSession::MySQL { .. } => Err(format!(
                "Expected Oracle object action session but acquired {}",
                unexpected.db_type()
            )),
        }
    }

    fn load_table_indexes(
        &self,
        _context: &crate::db::DbPoolSessionContext,
        session: &mut crate::db::DbPoolSession,
        selected_scope: Option<&str>,
        table_name: &str,
    ) -> Result<Vec<IndexInfo>, String> {
        let qualified_name = self.qualify_object_name(selected_scope, table_name);
        match session {
            crate::db::DbPoolSession::Oracle(conn) => {
                ObjectBrowser::get_table_indexes(conn, &qualified_name)
                    .map_err(|err| err.to_string())
            }
            crate::db::DbPoolSession::OracleThin(conn) => {
                ObjectBrowser::get_thin_table_indexes(conn, &qualified_name)
            }
            unexpected @ crate::db::DbPoolSession::MySQL { .. } => Err(format!(
                "Expected Oracle object action session but acquired {}",
                unexpected.db_type()
            )),
        }
    }

    fn load_table_constraints(
        &self,
        _context: &crate::db::DbPoolSessionContext,
        session: &mut crate::db::DbPoolSession,
        selected_scope: Option<&str>,
        table_name: &str,
    ) -> Result<Vec<ConstraintInfo>, String> {
        let qualified_name = self.qualify_object_name(selected_scope, table_name);
        match session {
            crate::db::DbPoolSession::Oracle(conn) => {
                ObjectBrowser::get_table_constraints(conn, &qualified_name)
                    .map_err(|err| err.to_string())
            }
            crate::db::DbPoolSession::OracleThin(conn) => {
                ObjectBrowser::get_thin_table_constraints(conn, &qualified_name)
            }
            unexpected @ crate::db::DbPoolSession::MySQL { .. } => Err(format!(
                "Expected Oracle object action session but acquired {}",
                unexpected.db_type()
            )),
        }
    }

    fn load_object_info(
        &self,
        _context: &crate::db::DbPoolSessionContext,
        session: &mut crate::db::DbPoolSession,
        selected_scope: Option<&str>,
        object_type: &str,
        object_name: &str,
    ) -> Result<ObjectInfoPayload, String> {
        let qualified_name = self.qualify_object_name(selected_scope, object_name);
        match session {
            crate::db::DbPoolSession::Oracle(conn) => match object_type {
                "SYNONYMS" => ObjectBrowser::get_synonym_info(conn, &qualified_name)
                    .map(ObjectInfoPayload::Synonym)
                    .map_err(|err| err.to_string()),
                "SEQUENCES" => ObjectBrowser::get_sequence_info(conn, &qualified_name)
                    .map(ObjectInfoPayload::Sequence)
                    .map_err(|err| err.to_string()),
                other => Err(format!("Unexpected object type for View Info: {other}")),
            },
            crate::db::DbPoolSession::OracleThin(conn) => match object_type {
                "SYNONYMS" => ObjectBrowser::get_thin_synonym_info(conn, &qualified_name)
                    .map(ObjectInfoPayload::Synonym),
                "SEQUENCES" => ObjectBrowser::get_thin_sequence_info(conn, &qualified_name)
                    .map(ObjectInfoPayload::Sequence),
                other => Err(format!("Unexpected object type for View Info: {other}")),
            },
            unexpected @ crate::db::DbPoolSession::MySQL { .. } => Err(format!(
                "Expected Oracle object action session but acquired {}",
                unexpected.db_type()
            )),
        }
    }

    fn generate_object_ddl(
        &self,
        _context: &crate::db::DbPoolSessionContext,
        session: &mut crate::db::DbPoolSession,
        selected_scope: Option<&str>,
        object_type: &str,
        object_name: &str,
    ) -> Result<String, String> {
        let qualified_name = self.qualify_object_name(selected_scope, object_name);
        match session {
            crate::db::DbPoolSession::Oracle(conn) => match object_type {
                "TABLE" => ObjectBrowser::get_table_ddl(conn, &qualified_name),
                "VIEW" => ObjectBrowser::get_view_ddl(conn, &qualified_name),
                "MATERIALIZED_VIEW" => {
                    ObjectBrowser::get_object_ddl(conn, "MATERIALIZED_VIEW", &qualified_name)
                }
                "PROCEDURE" => ObjectBrowser::get_procedure_ddl(conn, &qualified_name),
                "FUNCTION" => ObjectBrowser::get_function_ddl(conn, &qualified_name),
                "SEQUENCE" => ObjectBrowser::get_sequence_ddl(conn, &qualified_name),
                "TRIGGER" => ObjectBrowser::get_object_ddl(conn, "TRIGGER", &qualified_name),
                "TYPE" => ObjectBrowser::get_object_ddl(conn, "TYPE", &qualified_name),
                "INDEX" => ObjectBrowser::get_object_ddl(conn, "INDEX", &qualified_name),
                "SYNONYM" => ObjectBrowser::get_synonym_ddl(conn, &qualified_name),
                "PACKAGE" => ObjectBrowser::get_package_ddl(conn, &qualified_name),
                other => {
                    return Err(format!(
                        "{other} DDL is not supported for Oracle connections"
                    ))
                }
            }
            .map_err(|err| err.to_string()),
            crate::db::DbPoolSession::OracleThin(conn) => match object_type {
                "TABLE" => ObjectBrowser::get_thin_object_ddl(conn, "TABLE", &qualified_name),
                "VIEW" => ObjectBrowser::get_thin_object_ddl(conn, "VIEW", &qualified_name),
                "MATERIALIZED_VIEW" => {
                    ObjectBrowser::get_thin_object_ddl(conn, "MATERIALIZED_VIEW", &qualified_name)
                }
                "PROCEDURE" => {
                    ObjectBrowser::get_thin_object_ddl(conn, "PROCEDURE", &qualified_name)
                }
                "FUNCTION" => ObjectBrowser::get_thin_object_ddl(conn, "FUNCTION", &qualified_name),
                "SEQUENCE" => ObjectBrowser::get_thin_object_ddl(conn, "SEQUENCE", &qualified_name),
                "TRIGGER" => ObjectBrowser::get_thin_object_ddl(conn, "TRIGGER", &qualified_name),
                "TYPE" => ObjectBrowser::get_thin_object_ddl(conn, "TYPE", &qualified_name),
                "INDEX" => ObjectBrowser::get_thin_object_ddl(conn, "INDEX", &qualified_name),
                "SYNONYM" => ObjectBrowser::get_thin_object_ddl(conn, "SYNONYM", &qualified_name),
                "PACKAGE" => ObjectBrowser::get_thin_package_ddl(conn, &qualified_name),
                other => Err(format!(
                    "{other} DDL is not supported for Oracle connections"
                )),
            },
            unexpected @ crate::db::DbPoolSession::MySQL { .. } => Err(format!(
                "Expected Oracle object action session but acquired {}",
                unexpected.db_type()
            )),
        }
    }

    fn supports_package_routines(&self) -> bool {
        true
    }

    fn load_package_routines(
        &self,
        connection: &SharedConnection,
        activity: String,
        selected_scope: Option<&str>,
        package_name: &str,
    ) -> Result<Vec<PackageRoutine>, String> {
        let qualified_package = self.qualify_object_name(selected_scope, package_name);
        ObjectBrowserWidget::with_pooled_object_session(
            connection,
            selected_scope,
            activity,
            |_context, session| match session {
                crate::db::DbPoolSession::Oracle(conn) => {
                    ObjectBrowser::get_package_routines(conn, &qualified_package)
                        .map_err(|err| err.to_string())
                }
                crate::db::DbPoolSession::OracleThin(conn) => {
                    ObjectBrowser::get_thin_package_routines(conn, &qualified_package)
                }
                unexpected @ crate::db::DbPoolSession::MySQL { .. } => Err(format!(
                    "Expected Oracle object action session but acquired {}",
                    unexpected.db_type()
                )),
            },
        )
    }

    fn load_package_routine_script(
        &self,
        connection: &SharedConnection,
        activity: String,
        selected_scope: Option<&str>,
        package_name: &str,
        routine_name: &str,
        routine_type: &str,
    ) -> Result<RoutineScriptData, String> {
        let qualified_name =
            self.qualify_package_member_name(selected_scope, package_name, routine_name);
        let package_qualified_name = self.qualify_object_name(selected_scope, package_name);
        ObjectBrowserWidget::with_pooled_object_session(
            connection,
            selected_scope,
            activity,
            |_context, session| match session {
                crate::db::DbPoolSession::Oracle(conn) => {
                    let resolved_type = if routine_type == "UNKNOWN" {
                        let routines =
                            ObjectBrowser::get_package_routines(conn, &package_qualified_name)
                                .map_err(|err| err.to_string())?;
                        routines
                            .iter()
                            .find(|routine| routine.name.eq_ignore_ascii_case(routine_name))
                            .and_then(|routine| {
                                ObjectBrowserWidget::normalize_package_routine_type(
                                    &routine.routine_type,
                                )
                            })
                            .ok_or_else(|| {
                                format!(
                                    "Could not resolve package routine type for {}",
                                    qualified_name
                                )
                            })
                    } else {
                        Ok(routine_type.to_string())
                    }?;

                    let arguments = ObjectBrowser::get_package_procedure_arguments(
                        conn,
                        &package_qualified_name,
                        routine_name,
                    )
                    .map_err(|err| err.to_string())?;
                    Ok(RoutineScriptData {
                        qualified_name: qualified_name.clone(),
                        resolved_routine_type: resolved_type.clone(),
                        sql: self.build_routine_script(&qualified_name, &resolved_type, &arguments),
                    })
                }
                crate::db::DbPoolSession::OracleThin(conn) => {
                    let resolved_type = if routine_type == "UNKNOWN" {
                        let routines = ObjectBrowser::get_thin_package_routines(
                            conn,
                            &package_qualified_name,
                        )?;
                        routines
                            .iter()
                            .find(|routine| routine.name.eq_ignore_ascii_case(routine_name))
                            .and_then(|routine| {
                                ObjectBrowserWidget::normalize_package_routine_type(
                                    &routine.routine_type,
                                )
                            })
                            .ok_or_else(|| {
                                format!(
                                    "Could not resolve package routine type for {}",
                                    qualified_name
                                )
                            })
                    } else {
                        Ok(routine_type.to_string())
                    }?;

                    let arguments = ObjectBrowser::get_thin_package_procedure_arguments(
                        conn,
                        &package_qualified_name,
                        routine_name,
                    )?;
                    Ok(RoutineScriptData {
                        qualified_name: qualified_name.clone(),
                        resolved_routine_type: resolved_type.clone(),
                        sql: self.build_routine_script(&qualified_name, &resolved_type, &arguments),
                    })
                }
                unexpected @ crate::db::DbPoolSession::MySQL { .. } => Err(format!(
                    "Expected Oracle object action session but acquired {}",
                    unexpected.db_type()
                )),
            },
        )
    }

    fn load_compilation_errors(
        &self,
        connection: &SharedConnection,
        activity: String,
        selected_scope: Option<&str>,
        object_name: &str,
        object_type: &str,
    ) -> Result<(String, Vec<CompilationError>), String> {
        let qualified_name = self.qualify_object_name(selected_scope, object_name);
        ObjectBrowserWidget::with_pooled_object_session(
            connection,
            selected_scope,
            activity,
            |_context, session| {
                let (status, body_status, errors) = match session {
                    crate::db::DbPoolSession::Oracle(conn) => {
                        let status =
                            ObjectBrowser::get_object_status(conn, &qualified_name, object_type)
                                .unwrap_or_else(|_| "UNKNOWN".to_string());
                        let body_status = if object_type == "PACKAGE" {
                            ObjectBrowser::get_object_status(conn, &qualified_name, "PACKAGE BODY")
                                .ok()
                        } else {
                            None
                        };
                        let mut errors = ObjectBrowser::get_compilation_errors(
                            conn,
                            &qualified_name,
                            object_type,
                        )
                        .unwrap_or_default();
                        if object_type == "PACKAGE" {
                            if let Ok(body_errors) = ObjectBrowser::get_compilation_errors(
                                conn,
                                &qualified_name,
                                "PACKAGE BODY",
                            ) {
                                errors.extend(body_errors);
                            }
                        }
                        (status, body_status, errors)
                    }
                    crate::db::DbPoolSession::OracleThin(conn) => {
                        let status = ObjectBrowser::get_thin_object_status(
                            conn,
                            &qualified_name,
                            object_type,
                        )
                        .unwrap_or_else(|_| "UNKNOWN".to_string());
                        let body_status = if object_type == "PACKAGE" {
                            ObjectBrowser::get_thin_object_status(
                                conn,
                                &qualified_name,
                                "PACKAGE BODY",
                            )
                            .ok()
                        } else {
                            None
                        };
                        let mut errors = ObjectBrowser::get_thin_compilation_errors(
                            conn,
                            &qualified_name,
                            object_type,
                        )
                        .unwrap_or_default();
                        if object_type == "PACKAGE" {
                            if let Ok(body_errors) = ObjectBrowser::get_thin_compilation_errors(
                                conn,
                                &qualified_name,
                                "PACKAGE BODY",
                            ) {
                                errors.extend(body_errors);
                            }
                        }
                        (status, body_status, errors)
                    }
                    unexpected @ crate::db::DbPoolSession::MySQL { .. } => {
                        return Err(format!(
                            "Expected Oracle object action session but acquired {}",
                            unexpected.db_type()
                        ))
                    }
                };

                let combined_status = if let Some(body_status) = body_status {
                    format!("Spec: {} / Body: {}", status, body_status)
                } else {
                    status
                };

                Ok((combined_status, errors))
            },
        )
    }

    fn menu_choices_for_object_item(&self, item_info: &ObjectItem) -> Option<&'static str> {
        match item_info {
            ObjectItem::Simple { object_type, .. } if object_type == "TABLES" => Some(
                "Select Data (Top 100)|Import Data...|Export Data...|View Structure|View \
                 Indexes|View Constraints|Generate DDL|Truncate...|Drop...",
            ),
            ObjectItem::Simple { object_type, .. }
                if object_type == "VIEWS" || object_type == "MATERIALIZED VIEWS" =>
            {
                Some("Select Data (Top 100)|Generate DDL|Drop...")
            }
            ObjectItem::Simple { object_type, .. } if object_type == "PROCEDURES" => {
                Some("Execute Procedure|Check Compilation|Generate DDL|Drop...")
            }
            ObjectItem::Simple { object_type, .. } if object_type == "FUNCTIONS" => {
                Some("Execute Function|Check Compilation|Generate DDL|Drop...")
            }
            ObjectItem::Simple { object_type, .. } if object_type == "SEQUENCES" => {
                Some("View Info|Generate DDL|Drop...")
            }
            ObjectItem::Simple { object_type, .. } if object_type == "TRIGGERS" => {
                Some("Check Compilation|Generate DDL|Drop...")
            }
            ObjectItem::Simple { object_type, .. } if object_type == "EVENTS" => {
                Some("Generate DDL")
            }
            ObjectItem::Simple { object_type, .. }
                if object_type == "TYPES" || object_type == "INDEXES" =>
            {
                Some("Generate DDL")
            }
            ObjectItem::Simple { object_type, .. } if object_type == "SYNONYMS" => {
                Some("View Info|Generate DDL|Drop...")
            }
            ObjectItem::PackageRoutine { routine_type, .. } => match routine_type.as_str() {
                "FUNCTION" => Some("Execute Function"),
                "PROCEDURE" => Some("Execute Procedure"),
                _ => Some("Execute Routine"),
            },
            ObjectItem::Simple { object_type, .. } if object_type == "PACKAGES" => {
                Some("Check Compilation|Generate DDL|Drop...")
            }
            _ => None,
        }
    }

    fn root_categories(&self, _cache: &ObjectCache) -> Vec<&'static str> {
        vec![
            "Tables",
            "Views",
            "Procedures",
            "Functions",
            "Sequences",
            "Triggers",
            "Synonyms",
            "Packages",
        ]
    }

    fn load_metadata_cache(
        &self,
        context: crate::db::DbPoolSessionContext,
        requested_scope: Option<String>,
        activity: &crate::db::DbActivityGuard,
    ) -> Option<(
        crate::db::DatabaseType,
        ObjectCache,
        Vec<String>,
        Option<String>,
    )> {
        let db_type = context.connection_info.db_type;
        context.ensure_current().ok()?;
        let acquired = match context
            .acquire_session_for_current_scope(crate::db::PooledSessionPurpose::AppRead, activity)
        {
            Ok(acquired) => acquired,
            Err(err) => {
                eprintln!(
                    "Warning: failed to acquire Oracle object-browser metadata session: {err}"
                );
                return None;
            }
        };
        let (current_schema, mut available_scopes, use_thin_metadata) = match acquired.into_oracle()
        {
            Ok(conn) => {
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
                let available_scopes = ObjectBrowser::get_users(&conn).unwrap_or_default();
                (current_schema, available_scopes, false)
            }
            Err(acquired) => match acquired.into_oracle_thin() {
                Ok(mut conn) => {
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
                    let available_scopes =
                        ObjectBrowser::get_thin_users(&mut conn).unwrap_or_default();
                    (current_schema, available_scopes, true)
                }
                Err(other) => {
                    eprintln!(
                        "Warning: expected Oracle object-browser metadata session but acquired {}",
                        other.describe_session()
                    );
                    return None;
                }
            },
        };
        context.ensure_current().ok()?;
        if let Some(ref current_schema) = current_schema {
            if !available_scopes.iter().any(|scope| scope == current_schema) {
                available_scopes.push(current_schema.clone());
            }
        }
        available_scopes.sort();
        available_scopes.dedup();
        let selected_scope = requested_scope
            .filter(|scope| !scope.trim().is_empty())
            .or(current_schema)
            .or_else(|| available_scopes.first().cloned());

        let worker_limit = ObjectBrowserWidget::object_metadata_worker_limit(&context);
        let cache = if let Some(ref selected_scope) = selected_scope {
            let selected_scope = selected_scope.clone();
            let mut jobs: Vec<ObjectMetadataLoadJob> = Vec::new();

            let context_for_tables = context.clone();
            let activity_for_tables = activity.clone();
            let scope_for_tables = selected_scope.clone();
            jobs.push(Box::new(move || {
                let mut cache = ObjectCache::default();
                cache.tables = if use_thin_metadata {
                    let Some(mut db_conn) =
                        ObjectBrowserWidget::acquire_oracle_thin_metadata_session(
                            &context_for_tables,
                            &activity_for_tables,
                        )
                    else {
                        return cache;
                    };
                    ObjectBrowser::get_thin_tables_by_owner(&mut db_conn, &scope_for_tables)
                        .unwrap_or_default()
                } else {
                    let Some(db_conn) = ObjectBrowserWidget::acquire_oracle_metadata_session(
                        &context_for_tables,
                        &activity_for_tables,
                    ) else {
                        return cache;
                    };
                    ObjectBrowser::get_tables_by_owner(&db_conn, &scope_for_tables)
                        .unwrap_or_default()
                };
                cache
            }));

            let context_for_views = context.clone();
            let activity_for_views = activity.clone();
            let scope_for_views = selected_scope.clone();
            jobs.push(Box::new(move || {
                let mut cache = ObjectCache::default();
                cache.views = if use_thin_metadata {
                    let Some(mut db_conn) =
                        ObjectBrowserWidget::acquire_oracle_thin_metadata_session(
                            &context_for_views,
                            &activity_for_views,
                        )
                    else {
                        return cache;
                    };
                    ObjectBrowser::get_thin_views_by_owner(&mut db_conn, &scope_for_views)
                        .unwrap_or_default()
                } else {
                    let Some(db_conn) = ObjectBrowserWidget::acquire_oracle_metadata_session(
                        &context_for_views,
                        &activity_for_views,
                    ) else {
                        return cache;
                    };
                    ObjectBrowser::get_views_by_owner(&db_conn, &scope_for_views)
                        .unwrap_or_default()
                };
                cache
            }));

            let context_for_procedures = context.clone();
            let activity_for_procedures = activity.clone();
            let scope_for_procedures = selected_scope.clone();
            jobs.push(Box::new(move || {
                let mut cache = ObjectCache::default();
                cache.procedures = if use_thin_metadata {
                    let Some(mut db_conn) =
                        ObjectBrowserWidget::acquire_oracle_thin_metadata_session(
                            &context_for_procedures,
                            &activity_for_procedures,
                        )
                    else {
                        return cache;
                    };
                    ObjectBrowser::get_thin_procedures_by_owner(&mut db_conn, &scope_for_procedures)
                        .unwrap_or_default()
                } else {
                    let Some(db_conn) = ObjectBrowserWidget::acquire_oracle_metadata_session(
                        &context_for_procedures,
                        &activity_for_procedures,
                    ) else {
                        return cache;
                    };
                    ObjectBrowser::get_procedures_by_owner(&db_conn, &scope_for_procedures)
                        .unwrap_or_default()
                };
                cache
            }));

            let context_for_functions = context.clone();
            let activity_for_functions = activity.clone();
            let scope_for_functions = selected_scope.clone();
            jobs.push(Box::new(move || {
                let mut cache = ObjectCache::default();
                cache.functions = if use_thin_metadata {
                    let Some(mut db_conn) =
                        ObjectBrowserWidget::acquire_oracle_thin_metadata_session(
                            &context_for_functions,
                            &activity_for_functions,
                        )
                    else {
                        return cache;
                    };
                    ObjectBrowser::get_thin_functions_by_owner(&mut db_conn, &scope_for_functions)
                        .unwrap_or_default()
                } else {
                    let Some(db_conn) = ObjectBrowserWidget::acquire_oracle_metadata_session(
                        &context_for_functions,
                        &activity_for_functions,
                    ) else {
                        return cache;
                    };
                    ObjectBrowser::get_functions_by_owner(&db_conn, &scope_for_functions)
                        .unwrap_or_default()
                };
                cache
            }));

            let context_for_sequences = context.clone();
            let activity_for_sequences = activity.clone();
            let scope_for_sequences = selected_scope.clone();
            jobs.push(Box::new(move || {
                let mut cache = ObjectCache::default();
                cache.sequences = if use_thin_metadata {
                    let Some(mut db_conn) =
                        ObjectBrowserWidget::acquire_oracle_thin_metadata_session(
                            &context_for_sequences,
                            &activity_for_sequences,
                        )
                    else {
                        return cache;
                    };
                    ObjectBrowser::get_thin_sequences_by_owner(&mut db_conn, &scope_for_sequences)
                        .unwrap_or_default()
                } else {
                    let Some(db_conn) = ObjectBrowserWidget::acquire_oracle_metadata_session(
                        &context_for_sequences,
                        &activity_for_sequences,
                    ) else {
                        return cache;
                    };
                    ObjectBrowser::get_sequences_by_owner(&db_conn, &scope_for_sequences)
                        .unwrap_or_default()
                };
                cache
            }));

            let context_for_triggers = context.clone();
            let activity_for_triggers = activity.clone();
            let scope_for_triggers = selected_scope.clone();
            jobs.push(Box::new(move || {
                let mut cache = ObjectCache::default();
                cache.triggers = if use_thin_metadata {
                    let Some(mut db_conn) =
                        ObjectBrowserWidget::acquire_oracle_thin_metadata_session(
                            &context_for_triggers,
                            &activity_for_triggers,
                        )
                    else {
                        return cache;
                    };
                    ObjectBrowser::get_thin_triggers_by_owner(&mut db_conn, &scope_for_triggers)
                        .unwrap_or_default()
                } else {
                    let Some(db_conn) = ObjectBrowserWidget::acquire_oracle_metadata_session(
                        &context_for_triggers,
                        &activity_for_triggers,
                    ) else {
                        return cache;
                    };
                    ObjectBrowser::get_triggers_by_owner(&db_conn, &scope_for_triggers)
                        .unwrap_or_default()
                };
                cache
            }));

            let context_for_synonyms = context.clone();
            let activity_for_synonyms = activity.clone();
            let scope_for_synonyms = selected_scope.clone();
            jobs.push(Box::new(move || {
                let mut cache = ObjectCache::default();
                cache.synonyms = if use_thin_metadata {
                    let Some(mut db_conn) =
                        ObjectBrowserWidget::acquire_oracle_thin_metadata_session(
                            &context_for_synonyms,
                            &activity_for_synonyms,
                        )
                    else {
                        return cache;
                    };
                    ObjectBrowser::get_thin_synonyms_by_owner(&mut db_conn, &scope_for_synonyms)
                        .unwrap_or_default()
                } else {
                    let Some(db_conn) = ObjectBrowserWidget::acquire_oracle_metadata_session(
                        &context_for_synonyms,
                        &activity_for_synonyms,
                    ) else {
                        return cache;
                    };
                    ObjectBrowser::get_synonyms_by_owner(&db_conn, &scope_for_synonyms)
                        .unwrap_or_default()
                };
                cache
            }));

            let context_for_packages = context.clone();
            let activity_for_packages = activity.clone();
            let scope_for_packages = selected_scope;
            jobs.push(Box::new(move || {
                let mut cache = ObjectCache::default();
                cache.packages = if use_thin_metadata {
                    let Some(mut db_conn) =
                        ObjectBrowserWidget::acquire_oracle_thin_metadata_session(
                            &context_for_packages,
                            &activity_for_packages,
                        )
                    else {
                        return cache;
                    };
                    ObjectBrowser::get_thin_packages_by_owner(&mut db_conn, &scope_for_packages)
                        .unwrap_or_default()
                } else {
                    let Some(db_conn) = ObjectBrowserWidget::acquire_oracle_metadata_session(
                        &context_for_packages,
                        &activity_for_packages,
                    ) else {
                        return cache;
                    };
                    ObjectBrowser::get_packages_by_owner(&db_conn, &scope_for_packages)
                        .unwrap_or_default()
                };
                cache
            }));

            ObjectBrowserWidget::load_object_metadata_jobs(&context, jobs, worker_limit, activity)
        } else {
            ObjectCache::default()
        };

        context.ensure_current().ok()?;
        Some((db_type, cache, available_scopes, selected_scope))
    }
}

impl ObjectBrowserDbBehavior for MysqlObjectBrowserBehavior {
    fn qualify_object_name(&self, selected_scope: Option<&str>, object_name: &str) -> String {
        let object_name = object_name.trim();
        if object_name.is_empty() || object_name.contains('.') {
            return object_name.to_string();
        }

        selected_scope
            .filter(|scope| !scope.trim().is_empty())
            .map(|scope| format!("{}.{}", scope.trim(), object_name))
            .unwrap_or_else(|| object_name.to_string())
    }

    fn qualify_package_member_name(
        &self,
        selected_scope: Option<&str>,
        package_name: &str,
        routine_name: &str,
    ) -> String {
        let package_name = self.qualify_object_name(selected_scope, package_name);
        format!("{}.{}", package_name, routine_name.trim())
    }

    fn preview_select_sql(&self, selected_scope: Option<&str>, object_name: &str) -> String {
        let qualified_name = self.qualify_object_name(selected_scope, object_name);
        format!(
            "SELECT * FROM {} LIMIT 100",
            ObjectBrowserWidget::quote_mysql_identifier_path(&qualified_name)
        )
    }

    fn export_select_sql(&self, selected_scope: Option<&str>, object_name: &str) -> String {
        let qualified_name = self.qualify_object_name(selected_scope, object_name);
        format!(
            "SELECT * FROM {}",
            ObjectBrowserWidget::quote_mysql_identifier_path(&qualified_name)
        )
    }

    fn destructive_object_sql(
        &self,
        action: DestructiveObjectAction,
        selected_scope: Option<&str>,
        object_type: &str,
        object_name: &str,
    ) -> Option<String> {
        // Indexes are deliberately absent: `DROP INDEX` needs the table the
        // index belongs to, which a tree node does not carry.
        let keyword = match action {
            DestructiveObjectAction::Truncate => match object_type {
                "TABLES" => "TABLE",
                _ => return None,
            },
            DestructiveObjectAction::Drop => match object_type {
                "TABLES" => "TABLE",
                "VIEWS" => "VIEW",
                "PROCEDURES" => "PROCEDURE",
                "FUNCTIONS" => "FUNCTION",
                "SEQUENCES" => "SEQUENCE",
                "TRIGGERS" => "TRIGGER",
                "EVENTS" => "EVENT",
                _ => return None,
            },
        };
        let qualified_name = ObjectBrowserWidget::quote_mysql_identifier_path(
            &self.qualify_object_name(selected_scope, object_name),
        );
        Some(match action {
            DestructiveObjectAction::Truncate => format!("TRUNCATE {} {}", keyword, qualified_name),
            DestructiveObjectAction::Drop => format!("DROP {} {}", keyword, qualified_name),
        })
    }

    fn build_simple_procedure_script(&self, qualified_name: &str) -> String {
        format!(
            "CALL {}();\n",
            ObjectBrowserWidget::quote_mysql_identifier_path(qualified_name)
        )
    }

    fn build_simple_function_script(&self, qualified_name: &str) -> String {
        format!(
            "SELECT {} AS result;\n",
            if qualified_name.contains('(') {
                qualified_name.to_string()
            } else {
                format!(
                    "{}()",
                    ObjectBrowserWidget::quote_mysql_identifier_path(qualified_name)
                )
            }
        )
    }

    fn build_routine_script(
        &self,
        qualified_name: &str,
        routine_type: &str,
        arguments: &[ProcedureArgument],
    ) -> String {
        ObjectBrowserWidget::build_mysql_routine_script(qualified_name, routine_type, arguments)
    }

    fn action_scope<'a>(
        &self,
        selected_scope: Option<&'a str>,
        context: &'a crate::db::DbPoolSessionContext,
    ) -> Option<&'a str> {
        ObjectBrowserWidget::mysql_scope_for_context(selected_scope, &context.current_service_name)
    }

    fn load_routine_script(
        &self,
        context: &crate::db::DbPoolSessionContext,
        session: &mut crate::db::DbPoolSession,
        selected_scope: Option<&str>,
        object_name: &str,
        routine_type: &str,
    ) -> Result<RoutineScriptData, String> {
        let conn = self.take_object_action_session(context, session)?;
        let action_scope = self.action_scope(selected_scope, context);
        let qualified_name = self.qualify_object_name(action_scope, object_name);
        crate::db::query::mysql_executor::MysqlObjectBrowser::get_routine_arguments_in_schema(
            conn.as_mut(),
            action_scope,
            object_name,
        )
        .map(|arguments| RoutineScriptData {
            qualified_name: qualified_name.clone(),
            resolved_routine_type: routine_type.to_string(),
            sql: self.build_routine_script(&qualified_name, routine_type, &arguments),
        })
        .map_err(|err| err.to_string())
    }

    fn load_table_structure(
        &self,
        context: &crate::db::DbPoolSessionContext,
        session: &mut crate::db::DbPoolSession,
        selected_scope: Option<&str>,
        table_name: &str,
    ) -> Result<Vec<TableColumnDetail>, String> {
        let conn = self.take_object_action_session(context, session)?;
        crate::db::query::mysql_executor::MysqlObjectBrowser::get_table_structure_in_schema(
            conn.as_mut(),
            self.action_scope(selected_scope, context),
            table_name,
        )
        .map_err(|err| err.to_string())
    }

    fn load_table_rows(
        &self,
        context: &crate::db::DbPoolSessionContext,
        session: &mut crate::db::DbPoolSession,
        selected_scope: Option<&str>,
        table_name: &str,
    ) -> Result<crate::db::QueryResult, String> {
        let sql = self.export_select_sql(selected_scope, table_name);
        // The concrete type, not the family: MariaDB and MySQL classify some
        // statements differently, and defaulting to MySQL would quietly give a
        // MariaDB connection MySQL's answer.
        let db_type = context.connection_info.db_type;
        let conn = self.take_object_action_session(context, session)?;
        let results = crate::db::query::mysql_executor::MysqlExecutor::execute_for_db_type(
            conn.as_mut(),
            &sql,
            db_type,
        )
        .map_err(|err| err.to_string())?;
        results
            .into_iter()
            .find(|result| result.is_select)
            .ok_or_else(|| format!("{sql} returned no result set"))
    }

    fn load_table_indexes(
        &self,
        context: &crate::db::DbPoolSessionContext,
        session: &mut crate::db::DbPoolSession,
        selected_scope: Option<&str>,
        table_name: &str,
    ) -> Result<Vec<IndexInfo>, String> {
        let conn = self.take_object_action_session(context, session)?;
        crate::db::query::mysql_executor::MysqlObjectBrowser::get_index_details_in_schema(
            conn.as_mut(),
            self.action_scope(selected_scope, context),
            table_name,
        )
        .map_err(|err| err.to_string())
    }

    fn load_table_constraints(
        &self,
        context: &crate::db::DbPoolSessionContext,
        session: &mut crate::db::DbPoolSession,
        selected_scope: Option<&str>,
        table_name: &str,
    ) -> Result<Vec<ConstraintInfo>, String> {
        let conn = self.take_object_action_session(context, session)?;
        crate::db::query::mysql_executor::MysqlObjectBrowser::get_table_constraints_in_schema(
            conn.as_mut(),
            self.action_scope(selected_scope, context),
            table_name,
        )
        .map_err(|err| err.to_string())
    }

    fn load_object_info(
        &self,
        _context: &crate::db::DbPoolSessionContext,
        _session: &mut crate::db::DbPoolSession,
        _selected_scope: Option<&str>,
        object_type: &str,
        _object_name: &str,
    ) -> Result<ObjectInfoPayload, String> {
        Err(format!(
            "{} info is not supported for MySQL/MariaDB connections",
            object_type
        ))
    }

    fn generate_object_ddl(
        &self,
        context: &crate::db::DbPoolSessionContext,
        session: &mut crate::db::DbPoolSession,
        selected_scope: Option<&str>,
        object_type: &str,
        object_name: &str,
    ) -> Result<String, String> {
        let conn = self.take_object_action_session(context, session)?;
        match object_type {
            "MATERIALIZED_VIEW" | "SEQUENCE" | "SYNONYM" | "PACKAGE" | "TYPE" | "INDEX" => {
                Err(format!(
                    "{} DDL is not supported for MySQL/MariaDB connections",
                    object_type
                ))
            }
            _ => crate::db::query::mysql_executor::MysqlObjectBrowser::get_create_object_in_schema(
                conn.as_mut(),
                self.action_scope(selected_scope, context),
                object_type,
                object_name,
            )
            .map_err(|err| err.to_string()),
        }
    }

    fn supports_package_routines(&self) -> bool {
        false
    }

    fn load_package_routines(
        &self,
        _connection: &SharedConnection,
        _activity: String,
        _selected_scope: Option<&str>,
        package_name: &str,
    ) -> Result<Vec<PackageRoutine>, String> {
        Err(format!(
            "Package routines are not supported for {}",
            package_name
        ))
    }

    fn load_package_routine_script(
        &self,
        _connection: &SharedConnection,
        _activity: String,
        _selected_scope: Option<&str>,
        package_name: &str,
        routine_name: &str,
        _routine_type: &str,
    ) -> Result<RoutineScriptData, String> {
        Err(format!(
            "Package routine execution is not supported for {}.{}",
            package_name, routine_name
        ))
    }

    fn load_compilation_errors(
        &self,
        _connection: &SharedConnection,
        _activity: String,
        _selected_scope: Option<&str>,
        object_name: &str,
        object_type: &str,
    ) -> Result<(String, Vec<CompilationError>), String> {
        Err(format!(
            "Compilation status is not supported for {} {}",
            object_type, object_name
        ))
    }

    fn menu_choices_for_object_item(&self, item_info: &ObjectItem) -> Option<&'static str> {
        match item_info {
            ObjectItem::Simple { object_type, .. } if object_type == "TABLES" => Some(
                "Select Data (Top 100)|Import Data...|Export Data...|View Structure|View \
                 Indexes|View Constraints|Generate DDL|Truncate...|Drop...",
            ),
            ObjectItem::Simple { object_type, .. } if object_type == "VIEWS" => {
                Some("Select Data (Top 100)|Generate DDL|Drop...")
            }
            // No `Drop...`: this family has no materialized views, so there is
            // no statement to offer if such a node ever reaches here.
            ObjectItem::Simple { object_type, .. } if object_type == "MATERIALIZED VIEWS" => {
                Some("Select Data (Top 100)|Generate DDL")
            }
            ObjectItem::Simple { object_type, .. } if object_type == "PROCEDURES" => {
                Some("Execute Procedure|Generate DDL|Drop...")
            }
            ObjectItem::Simple { object_type, .. } if object_type == "FUNCTIONS" => {
                Some("Execute Function|Generate DDL|Drop...")
            }
            ObjectItem::Simple { object_type, .. } if object_type == "SEQUENCES" => {
                Some("View Info|Generate DDL|Drop...")
            }
            ObjectItem::Simple { object_type, .. }
                if object_type == "TRIGGERS" || object_type == "EVENTS" =>
            {
                Some("Generate DDL|Drop...")
            }
            ObjectItem::Simple { object_type, .. } if object_type == "SYNONYMS" => {
                Some("View Info|Generate DDL")
            }
            ObjectItem::PackageRoutine { routine_type, .. } => match routine_type.as_str() {
                "FUNCTION" => Some("Execute Function"),
                "PROCEDURE" => Some("Execute Procedure"),
                _ => Some("Execute Routine"),
            },
            ObjectItem::Simple { object_type, .. } if object_type == "PACKAGES" => {
                Some("Check Compilation|Generate DDL")
            }
            _ => None,
        }
    }

    fn root_categories(&self, cache: &ObjectCache) -> Vec<&'static str> {
        let mut categories = vec![
            "Tables",
            "Views",
            "Procedures",
            "Functions",
            "Triggers",
            "Events",
        ];
        if !cache.sequences.is_empty() {
            categories.insert(4, "Sequences");
        }
        categories
    }

    fn load_metadata_cache(
        &self,
        context: crate::db::DbPoolSessionContext,
        requested_scope: Option<String>,
        activity: &crate::db::DbActivityGuard,
    ) -> Option<(
        crate::db::DatabaseType,
        ObjectCache,
        Vec<String>,
        Option<String>,
    )> {
        use crate::db::query::mysql_executor::MysqlObjectBrowser;

        let db_type = context.connection_info.db_type;
        let display_name = db_type.display_name();
        context.ensure_current().ok()?;
        let requested_scope = requested_scope
            .map(|scope| scope.trim().to_string())
            .filter(|scope| !scope.is_empty());
        let mut mysql_conn = match context
            .acquire_session_for_current_scope(crate::db::PooledSessionPurpose::AppRead, activity)
        {
            Ok(acquired) => match acquired.into_mysql(db_type) {
                Ok(conn) => conn,
                Err(other) => {
                    eprintln!(
                        "Warning: expected {display_name} object-browser metadata session but acquired {}",
                        other.describe_session()
                    );
                    return None;
                }
            },
            Err(err) => {
                eprintln!(
                    "Warning: failed to acquire {display_name} object-browser metadata session: {err}"
                );
                return None;
            }
        };
        let current_database = context.current_service_name.trim().to_string();
        let mut available_scopes =
            MysqlObjectBrowser::get_schemas(mysql_conn.as_mut()).unwrap_or_default();
        context.ensure_current().ok()?;
        if !current_database.is_empty()
            && !available_scopes
                .iter()
                .any(|scope| scope.eq_ignore_ascii_case(&current_database))
        {
            available_scopes.push(current_database.clone());
        }
        available_scopes.sort();
        available_scopes.dedup();
        let selected_scope = requested_scope
            .filter(|scope| !scope.trim().is_empty())
            .or_else(|| (!current_database.is_empty()).then_some(current_database.clone()))
            .or_else(|| available_scopes.first().cloned())
            .map(|scope| {
                let mut matches = available_scopes
                    .iter()
                    .filter(|available| available.eq_ignore_ascii_case(&scope));
                match (matches.next(), matches.next()) {
                    (Some(available), None) => available.clone(),
                    _ => scope,
                }
            });

        if let Some(ref selected_scope) = selected_scope {
            if let Err(err) = mysql_conn.as_mut().select_db(selected_scope) {
                eprintln!(
                    "Warning: failed to select {display_name} object-browser metadata database `{selected_scope}`: {err}"
                );
                return None;
            }

            if let Err(err) =
                crate::db::DatabaseConnection::apply_mysql_connection_encoding_with_settings_for_db_type(
                    &mut *mysql_conn,
                    &context.connection_info.advanced,
                    db_type,
                )
            {
                eprintln!(
                    "Warning: failed to refresh {display_name} object-browser metadata encoding: {err}"
                );
                return None;
            }
        }

        drop(mysql_conn);

        let worker_limit = ObjectBrowserWidget::object_metadata_worker_limit(&context);
        let cache = if let Some(ref selected_scope) = selected_scope {
            let selected_scope = selected_scope.clone();
            let mut jobs: Vec<ObjectMetadataLoadJob> = Vec::new();

            let context_for_tables = context.clone();
            let activity_for_tables = activity.clone();
            let scope_for_tables = selected_scope.clone();
            jobs.push(Box::new(move || {
                let mut cache = ObjectCache::default();
                let Some(mut mysql_conn) = ObjectBrowserWidget::acquire_mysql_metadata_session(
                    &context_for_tables,
                    &scope_for_tables,
                    &activity_for_tables,
                ) else {
                    return cache;
                };
                cache.tables =
                    MysqlObjectBrowser::get_tables(mysql_conn.as_mut()).unwrap_or_default();
                cache
            }));

            let context_for_views = context.clone();
            let activity_for_views = activity.clone();
            let scope_for_views = selected_scope.clone();
            jobs.push(Box::new(move || {
                let mut cache = ObjectCache::default();
                let Some(mut mysql_conn) = ObjectBrowserWidget::acquire_mysql_metadata_session(
                    &context_for_views,
                    &scope_for_views,
                    &activity_for_views,
                ) else {
                    return cache;
                };
                cache.views =
                    MysqlObjectBrowser::get_views(mysql_conn.as_mut()).unwrap_or_default();
                cache
            }));

            let context_for_procedures = context.clone();
            let activity_for_procedures = activity.clone();
            let scope_for_procedures = selected_scope.clone();
            jobs.push(Box::new(move || {
                let mut cache = ObjectCache::default();
                let Some(mut mysql_conn) = ObjectBrowserWidget::acquire_mysql_metadata_session(
                    &context_for_procedures,
                    &scope_for_procedures,
                    &activity_for_procedures,
                ) else {
                    return cache;
                };
                cache.procedures =
                    MysqlObjectBrowser::get_procedures(mysql_conn.as_mut()).unwrap_or_default();
                cache
            }));

            let context_for_functions = context.clone();
            let activity_for_functions = activity.clone();
            let scope_for_functions = selected_scope.clone();
            jobs.push(Box::new(move || {
                let mut cache = ObjectCache::default();
                let Some(mut mysql_conn) = ObjectBrowserWidget::acquire_mysql_metadata_session(
                    &context_for_functions,
                    &scope_for_functions,
                    &activity_for_functions,
                ) else {
                    return cache;
                };
                cache.functions =
                    MysqlObjectBrowser::get_functions(mysql_conn.as_mut()).unwrap_or_default();
                cache
            }));

            let context_for_sequences = context.clone();
            let activity_for_sequences = activity.clone();
            let scope_for_sequences = selected_scope.clone();
            jobs.push(Box::new(move || {
                let mut cache = ObjectCache::default();
                let Some(mut mysql_conn) = ObjectBrowserWidget::acquire_mysql_metadata_session(
                    &context_for_sequences,
                    &scope_for_sequences,
                    &activity_for_sequences,
                ) else {
                    return cache;
                };
                cache.sequences =
                    MysqlObjectBrowser::get_sequences(mysql_conn.as_mut()).unwrap_or_default();
                cache
            }));

            let context_for_triggers = context.clone();
            let activity_for_triggers = activity.clone();
            let scope_for_triggers = selected_scope.clone();
            jobs.push(Box::new(move || {
                let mut cache = ObjectCache::default();
                let Some(mut mysql_conn) = ObjectBrowserWidget::acquire_mysql_metadata_session(
                    &context_for_triggers,
                    &scope_for_triggers,
                    &activity_for_triggers,
                ) else {
                    return cache;
                };
                cache.triggers =
                    MysqlObjectBrowser::get_triggers(mysql_conn.as_mut()).unwrap_or_default();
                cache
            }));

            let context_for_events = context.clone();
            let activity_for_events = activity.clone();
            let scope_for_events = selected_scope;
            jobs.push(Box::new(move || {
                let mut cache = ObjectCache::default();
                let Some(mut mysql_conn) = ObjectBrowserWidget::acquire_mysql_metadata_session(
                    &context_for_events,
                    &scope_for_events,
                    &activity_for_events,
                ) else {
                    return cache;
                };
                cache.events =
                    MysqlObjectBrowser::get_events(mysql_conn.as_mut()).unwrap_or_default();
                cache
            }));

            ObjectBrowserWidget::load_object_metadata_jobs(&context, jobs, worker_limit, activity)
        } else {
            ObjectCache::default()
        };

        context.ensure_current().ok()?;
        Some((db_type, cache, available_scopes, selected_scope))
    }
}

impl ObjectBrowserWidget {
    /// Release the card's callbacks. Safe to call more than once, and safe
    /// after the widgets are gone: a clone can outlive the deletion (a card
    /// torn down while a nested event loop holds one), and touching a deleted
    /// FLTK widget asserts inside the wrapper — a panic that would land in a
    /// `Drop`.
    fn detach_callbacks(&mut self) {
        if !self.filter_input.was_deleted() {
            self.filter_input.set_callback(|_| {});
        }
        if !self.scope_choice.was_deleted() {
            self.scope_choice.set_callback(|_| {});
            self.scope_choice.handle(|_, _| false);
        }
        if !self.tree.was_deleted() {
            self.tree.handle(|_, _| false);
        }
        self.clear_callback_slots();
    }

    fn clear_callback_slots(&self) {
        *self
            .sql_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .status_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .scope_change_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .scope_switch_preflight_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .metadata_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

impl Drop for ObjectBrowserWidget {
    fn drop(&mut self) {
        // Clones share the same underlying FLTK widgets and callback slots.
        // Only the last owner may detach handlers, otherwise dropping a
        // temporary clone can disable interactions in the live widget.
        if Arc::strong_count(&self.poll_lifecycle) != 1 {
            return;
        }
        self.detach_callbacks();
    }
}

fn widget_has_focus<W: WidgetExt>(widget: &W) -> bool {
    if let Some(focus) = app::focus() {
        return focus.as_widget_ptr() == widget.as_widget_ptr() || focus.inside(widget);
    }

    false
}

/// FLTK delivers KeyUp to whatever holds focus at that moment, which need not
/// be the widget that received the matching KeyDown (`Fl.cxx`, `case FL_KEYUP`
/// documents this). Returns true only for a KeyUp whose KeyDown was recorded
/// here, and clears the record either way so one KeyDown arms one KeyUp.
fn consume_owned_key_up(owned_keydown: &mut Option<Key>, key: Key) -> bool {
    owned_keydown.take() == Some(key)
}

fn copy_text_for_object_item(item_info: &ObjectItem) -> String {
    match item_info {
        ObjectItem::Column { column_name, .. } => column_name.clone(),
        ObjectItem::Simple { object_name, .. } => object_name.clone(),
        ObjectItem::PackageRoutine {
            package_name,
            routine_name,
            ..
        } => format!("{}.{}", package_name, routine_name),
    }
}

/// What a card's catalog is, and what question it answers.
///
/// These four values only ever make sense together: the scope the card ASKS
/// for, whether the catalog it holds answers that ask, when that catalog
/// arrived, and which connection incarnation produced it. Every bug this
/// module has had in this area came from a writer moving one and leaving the
/// others behind — a scope change that kept `loaded`, a load that stamped a
/// serial without a cache, a comparison that read the resolved name where the
/// ask was meant. Holding them in one type with named transitions is what
/// makes those states unwritable: there is no way to change the ask without
/// deciding what happens to the catalog, because `ask_for` does both.
///
/// The RESOLVED name (what the selector shows, and what qualifies generated
/// SQL) deliberately stays outside: a load rewrites it, and it must survive
/// an invalidation so the panel keeps showing where it is looking.
#[derive(Clone)]
struct CardCatalogState {
    requested_scope: Arc<Mutex<Option<String>>>,
    loaded: Arc<AtomicBool>,
    serial: Arc<AtomicU64>,
}

impl CardCatalogState {
    fn new() -> Self {
        Self {
            requested_scope: Arc::new(Mutex::new(None)),
            loaded: Arc::new(AtomicBool::new(false)),
            serial: Arc::new(AtomicU64::new(0)),
        }
    }

    fn requested_scope(&self) -> Option<String> {
        self.requested_scope
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn is_loaded(&self) -> bool {
        self.loaded.load(Ordering::Acquire)
    }

    fn serial(&self) -> u64 {
        self.serial.load(Ordering::Acquire)
    }

    /// The card is now asking `scope`. If that is a different question from
    /// the one the held catalog answers, the catalog stops counting as an
    /// answer — nothing else may inherit it, and the card must load again.
    fn ask_for(&self, scope: Option<String>, answers_current_ask: bool) {
        if !answers_current_ask {
            self.loaded.store(false, Ordering::Release);
        }
        *self
            .requested_scope
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = scope;
    }

    /// A load for `scope` has started: the old catalog is gone.
    fn load_started(&self, scope: Option<String>) {
        self.loaded.store(false, Ordering::Release);
        *self
            .requested_scope
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = scope;
    }

    /// A catalog answering the current ask has arrived.
    fn catalog_arrived(&self) {
        self.serial.store(next_metadata_serial(), Ordering::Release);
        self.loaded.store(true, Ordering::Release);
    }

    /// This card now holds `source`'s catalog, and answers the same question
    /// it answers — inheriting the ask, never the name it resolved to.
    fn adopt_from(&self, source: &CardCatalogState) {
        *self
            .requested_scope
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = source.requested_scope();
        self.serial.store(source.serial(), Ordering::Release);
        self.loaded.store(true, Ordering::Release);
    }

    /// The catalog no longer describes anything usable (the connection went
    /// away, or was rebuilt under the card).
    fn invalidate(&self) {
        self.loaded.store(false, Ordering::Release);
    }
}

/// Orders catalogs by when they were read, across every card in the process.
fn next_metadata_serial() -> u64 {
    static NEXT_METADATA_SERIAL: AtomicU64 = AtomicU64::new(1);
    NEXT_METADATA_SERIAL.fetch_add(1, Ordering::Relaxed)
}

/// Who a browser card belongs to. Every query editor tab gets its OWN card —
/// tree, filter, scope selector, metadata cache, and worker lifecycle — so
/// each tab keeps its expansion state and scope independently; the card is
/// created when the tab first becomes active on a connection and torn down
/// when the tab closes. Every connection additionally keeps one PREVIEW card,
/// which the dropdown shows when the active tab is not bound to that
/// connection; its selection only seeds what a new or unbound tab binds to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BrowserOwner {
    Tab(QueryTabId),
    ConnectionPreview(ConnectionId),
}

#[derive(Clone)]
struct ConnectionBrowserEntry {
    owner: BrowserOwner,
    connection_id: ConnectionId,
    runtime: Arc<ConnectionRuntime>,
    browser: ObjectBrowserWidget,
    /// Whether this card was born with a sibling card's metadata. A card that
    /// was not needs a real metadata load before its tab can show a tree or
    /// highlight identifiers.
    seeded_from_sibling: bool,
}

/// Tab- and connection-aware Object Browser host. Cards are per query tab
/// (plus one preview per connection); the connection choice is a compact root
/// selector and changing it never changes the active query tab.
#[derive(Clone)]
pub struct MultiObjectBrowserWidget {
    flex: Flex,
    connection_choice: Choice,
    browser_stack: Group,
    entries: Arc<Mutex<Vec<ConnectionBrowserEntry>>>,
    visible_owner: Arc<Mutex<Option<BrowserOwner>>>,
    active_tab: Arc<Mutex<Option<(QueryTabId, ConnectionId)>>>,
    sql_callback: ConnectionSqlExecuteCallback,
    status_callback: StatusCallback,
    scope_change_callback: ConnectionScopeChangeCallback,
    scope_switch_preflight_callback: ConnectionScopeSwitchPreflightCallback,
    metadata_callback: TabMetadataCallback,
    font_settings: Arc<Mutex<Option<(FontProfile, i32)>>>,
}

impl MultiObjectBrowserWidget {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        let mut flex = Flex::default().with_pos(x, y).with_size(w, h);
        flex.set_type(FlexType::Column);
        flex.set_spacing(DIALOG_SPACING);

        let mut connection_row = Flex::default();
        connection_row.set_type(FlexType::Row);
        connection_row.set_spacing(0);
        let left_margin = Frame::default();
        connection_row.fixed(&left_margin, TOOLBAR_SPACING);
        let mut connection_choice = Choice::default();
        theme::style_choice(&mut connection_choice);
        connection_choice
            .set_tooltip("Object Browser connection. Changing this does not rebind the query tab.");
        theme::install_choice_hover(&mut connection_choice);
        connection_choice.deactivate();
        connection_row.resizable(&connection_choice);
        let right_margin = Frame::default();
        connection_row.fixed(&right_margin, TOOLBAR_SPACING);
        connection_row.end();
        flex.fixed(&connection_row, FILTER_INPUT_HEIGHT);

        let browser_stack = Group::default();
        flex.resizable(&browser_stack);
        flex.end();

        let mut widget = Self {
            flex,
            connection_choice,
            browser_stack,
            entries: Arc::new(Mutex::new(Vec::new())),
            visible_owner: Arc::new(Mutex::new(None)),
            active_tab: Arc::new(Mutex::new(None)),
            sql_callback: Arc::new(Mutex::new(None)),
            status_callback: Arc::new(Mutex::new(None)),
            scope_change_callback: Arc::new(Mutex::new(None)),
            scope_switch_preflight_callback: Arc::new(Mutex::new(None)),
            metadata_callback: Arc::new(Mutex::new(None)),
            font_settings: Arc::new(Mutex::new(None)),
        };
        widget.setup_connection_choice_callback();
        widget
    }

    /// FLTK's `Fl_Menu_::add()` parses the label instead of taking it verbatim:
    /// `|` splits it into several items, `/` builds a submenu path, a leading `_`
    /// becomes a divider flag, `&` marks a mnemonic, `\` escapes the next
    /// character and a tab cuts the label off at a shortcut. Every one of those
    /// desynchronizes the flat menu index from `entries`, so a connection name
    /// carrying them would select (or display) the wrong browser. `|` and tabs
    /// are consumed before escapes are honoured, so they must be substituted.
    fn escape_choice_label(label: &str) -> String {
        let mut escaped = String::with_capacity(label.len());
        for character in label.chars() {
            match character {
                '|' | '\t' => escaped.push(' '),
                '/' | '\\' | '&' => {
                    escaped.push('\\');
                    escaped.push(character);
                }
                '_' if escaped.is_empty() => escaped.push_str("\\_"),
                _ => escaped.push(character),
            }
        }
        escaped
    }

    /// `Fl_Menu_::add()` reuses an existing item when the label matches exactly,
    /// so two connections sharing a display name would collapse into one entry
    /// and shift every following index. Keep the labels distinct instead.
    fn disambiguate_choice_labels(labels: &mut [String]) {
        let mut used = std::collections::HashSet::new();
        for label in labels.iter_mut() {
            if used.insert(label.clone()) {
                continue;
            }
            let mut suffix = 2;
            let mut candidate = format!("{label} #{suffix}");
            while !used.insert(candidate.clone()) {
                suffix += 1;
                candidate = format!("{label} #{suffix}");
            }
            *label = candidate;
        }
    }

    fn runtime_label(runtime: &ConnectionRuntime) -> String {
        let mut label = Self::escape_choice_label(&runtime.display_name());
        match runtime.state() {
            ConnectionRuntimeState::Connecting => label.push_str(" (connecting)"),
            ConnectionRuntimeState::Transitioning => label.push_str(" (transitioning)"),
            ConnectionRuntimeState::Disconnected => label.push_str(" (offline)"),
            ConnectionRuntimeState::Failed(_) => label.push_str(" (failed)"),
            ConnectionRuntimeState::Connected => {}
        }
        label
    }

    /// The dropdown lists CONNECTIONS (each once), not cards. Picking one
    /// shows the active tab's own card when the active tab is bound to that
    /// connection, and the connection's preview card otherwise.
    fn dropdown_connections(entries: &[ConnectionBrowserEntry]) -> Vec<ConnectionId> {
        let mut seen = std::collections::HashSet::new();
        entries
            .iter()
            .filter(|entry| seen.insert(entry.connection_id))
            .map(|entry| entry.connection_id)
            .collect()
    }

    fn card_owner_for_connection(
        entries: &[ConnectionBrowserEntry],
        active_tab: Option<(QueryTabId, ConnectionId)>,
        connection_id: ConnectionId,
    ) -> Option<BrowserOwner> {
        if let Some((tab_id, active_connection_id)) = active_tab {
            if active_connection_id == connection_id
                && entries
                    .iter()
                    .any(|entry| entry.owner == BrowserOwner::Tab(tab_id))
            {
                return Some(BrowserOwner::Tab(tab_id));
            }
        }
        entries
            .iter()
            .find(|entry| entry.owner == BrowserOwner::ConnectionPreview(connection_id))
            .or_else(|| {
                entries
                    .iter()
                    .find(|entry| entry.connection_id == connection_id)
            })
            .map(|entry| entry.owner)
    }

    fn show_owner_card(entries: &[ConnectionBrowserEntry], owner: BrowserOwner) {
        for entry in entries {
            let mut root = entry.browser.get_widget();
            if entry.owner == owner {
                root.show();
                // A card that adopted a catalog while hidden put off drawing
                // it; now that it is on screen it has to catch up.
                entry.browser.clone().rebuild_tree_if_pending();
            } else {
                root.hide();
            }
        }
    }

    fn setup_connection_choice_callback(&mut self) {
        let entries = self.entries.clone();
        let visible_owner = self.visible_owner.clone();
        let active_tab = self.active_tab.clone();
        self.connection_choice.set_callback(move |choice| {
            let index = choice.value().max(0) as usize;
            let entries_snapshot = entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            let connections = Self::dropdown_connections(&entries_snapshot);
            let Some(connection_id) = connections.get(index).copied() else {
                return;
            };
            let active = *active_tab
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(owner) =
                Self::card_owner_for_connection(&entries_snapshot, active, connection_id)
            else {
                return;
            };
            *visible_owner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(owner);
            Self::show_owner_card(&entries_snapshot, owner);
            // Only a card with nothing to show is loaded here. Refreshing
            // unconditionally would throw away the tree, filter and expansion
            // of whichever card the dropdown lands on — including the active
            // tab's own card, which is exactly what "a plain switch reloads
            // nothing" promises not to do.
            if let Some(mut browser) = entries_snapshot
                .iter()
                .find(|entry| entry.owner == owner)
                .map(|entry| entry.browser.clone())
                .filter(|browser| {
                    // A card that is empty *because its load is running* must
                    // not be loaded again: `refresh_with_context` starts by
                    // cancelling, which breaks that load's session and throws
                    // its result away, so revisiting the dropdown would keep
                    // restarting it and the card would never fill.
                    !browser.has_loaded_metadata() && !browser.metadata_refresh_in_flight()
                })
            {
                let _ = browser.refresh();
            }
            app::redraw();
        });
    }

    /// Hand a finished catalog to the application, naming the tab whose card
    /// produced it.
    ///
    /// The call is caught the way every other callback emit in this file is:
    /// the boxed closure is taken out of its slot to make the call, and a
    /// panic on the way through would drop it on the unwinding frame. There
    /// is exactly one of these slots for the whole panel, installed once, so
    /// losing it would end metadata delivery for every card until restart.
    fn emit_tab_metadata_callback(
        callback_slot: &TabMetadataCallback,
        tab_id: QueryTabId,
        snapshot: ObjectBrowserMetadataSnapshot,
    ) {
        let callback = {
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            slot.take()
        };
        let Some(mut callback) = callback else {
            return;
        };
        let call_result = panic::catch_unwind(AssertUnwindSafe(|| callback(tab_id, snapshot)));
        let mut slot = callback_slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_none() {
            *slot = Some(callback);
        }
        drop(slot);
        if let Err(payload) = call_result {
            ObjectBrowserWidget::log_callback_panic("metadata callback", payload.as_ref());
        }
    }

    fn wire_callbacks(
        &self,
        owner: BrowserOwner,
        connection_id: ConnectionId,
        browser: &mut ObjectBrowserWidget,
    ) {
        let sql_callback = self.sql_callback.clone();
        // Captured now, not read at delivery: this card's tab is the one the
        // user acted from, whatever tab is active by the time an async action
        // (Import Data...) comes back.
        let owner_tab_id = match owner {
            BrowserOwner::Tab(tab_id) => Some(tab_id),
            BrowserOwner::ConnectionPreview(_) => None,
        };
        browser.set_sql_callback(move |action| {
            if let Some(callback) = sql_callback
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_mut()
            {
                callback(owner_tab_id, connection_id, action);
            }
        });

        let status_callback = self.status_callback.clone();
        let visible_owner = self.visible_owner.clone();
        browser.set_status_callback(move |message| {
            let visible = *visible_owner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if visible == Some(owner) {
                ObjectBrowserWidget::emit_status_callback(&status_callback, message);
            }
        });

        // Metadata feeds the editor's intellisense, and every tab has its own
        // card: only the ACTIVE TAB's card may deliver. Preview cards refresh
        // their tree but never feed an editor.
        let metadata_callback = self.metadata_callback.clone();
        let active_tab = self.active_tab.clone();
        // Weak, not strong: the card table owns the card, the card owns this
        // callback, and a strong handle here would close that loop — every
        // card (and its worker thread, parked on a channel whose sender never
        // drops) would outlive the panel itself.
        let entries = Arc::downgrade(&self.entries);
        browser.set_metadata_callback(move |snapshot| {
            // A load is the truth for the whole connection at this scope, so
            // hand it to the cards that are still empty — a tab created while
            // this load was running never gets stuck with a blank tree
            // waiting for a refresh of its own.
            if let Some(entries) = entries.upgrade() {
                Self::fill_empty_sibling_cards(&entries, owner, connection_id);
            }
            let active = *active_tab
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let delivers = match owner {
                BrowserOwner::Tab(tab_id) => {
                    active.is_some_and(|(active_tab_id, _)| active_tab_id == tab_id)
                }
                BrowserOwner::ConnectionPreview(_) => false,
            };
            if let (true, BrowserOwner::Tab(tab_id)) = (delivers, owner) {
                Self::emit_tab_metadata_callback(&metadata_callback, tab_id, snapshot);
            }
        });

        let scope_change_callback = self.scope_change_callback.clone();
        browser.set_scope_change_callback(move || {
            if let Some(callback) = scope_change_callback
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_mut()
            {
                callback(connection_id);
            }
        });

        let scope_switch_preflight_callback = self.scope_switch_preflight_callback.clone();
        browser.set_scope_switch_preflight_callback(move || {
            scope_switch_preflight_callback
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_mut()
                .map_or(Ok(()), |callback| callback(connection_id))
        });
    }

    /// Copies a card's freshly loaded metadata into every card of the same
    /// connection that has none yet and is on the same scope. Called when a
    /// load lands, so cards created while it was running do not each need a
    /// load of their own.
    fn fill_empty_sibling_cards(
        entries: &Arc<Mutex<Vec<ConnectionBrowserEntry>>>,
        source_owner: BrowserOwner,
        connection_id: ConnectionId,
    ) {
        let (source, targets) = {
            let entries = entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // Match the connection too: a card can be torn down and rebuilt
            // on another server under the same owner, and a late callback
            // from the old one must not copy that server's catalog here.
            let Some(source) = entries
                .iter()
                .find(|entry| entry.owner == source_owner && entry.connection_id == connection_id)
                .map(|entry| entry.browser.clone())
            else {
                return;
            };
            let targets = entries
                .iter()
                .filter(|entry| {
                    entry.connection_id == connection_id
                        && entry.owner != source_owner
                        && !entry.browser.has_loaded_metadata()
                        // Ask against ask. A card that has loaded before keeps
                        // the question it asked, which is not the name that
                        // answer resolved to — comparing the source's request
                        // with the target's RESOLVED name would refuse every
                        // inheritance after a reconnect, and worse, could
                        // match a card whose tab is really asking for
                        // something else. Judged with the SOURCE's database
                        // type and option list: a card built while the
                        // connection was locked knows neither.
                        && source.request_matches_request_of(&entry.browser)
                })
                .map(|entry| entry.browser.clone())
                .collect::<Vec<_>>();
            (source, targets)
        };
        for mut target in targets {
            target.adopt_metadata_from(&source);
        }
    }

    /// Builds one browser card for `owner`, wired and font-styled, hidden by
    /// default.
    fn create_browser_entry(
        &mut self,
        owner: BrowserOwner,
        runtime: Arc<ConnectionRuntime>,
        scope: Option<String>,
    ) -> ConnectionBrowserEntry {
        let previous_group = Group::try_current();
        self.browser_stack.begin();
        let mut browser = ObjectBrowserWidget::new(
            self.browser_stack.x(),
            self.browser_stack.y(),
            self.browser_stack.w(),
            self.browser_stack.h(),
            runtime.connection(),
        );
        browser.set_tab_local_scope_selection(true);
        browser.set_connection_refuses_writes(runtime.sanitized_info().read_only);
        self.browser_stack.end();
        if let Some(previous_group) = previous_group.as_ref() {
            Group::set_current(Some(previous_group));
        } else {
            Group::set_current(None::<&Group>);
        }
        self.wire_callbacks(owner, runtime.id(), &mut browser);
        if let Some((profile, size)) = *self
            .font_settings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            browser.apply_font_settings(profile, size);
        }
        browser.get_widget().hide();
        // Start from what the connection already knows instead of an empty
        // tree: the metadata is a property of the connection and the scope it
        // was read in, not of the tab that first asked for it.
        let source = self.metadata_source_for_scope(runtime.id(), scope.as_deref());
        let seeded = source.is_some_and(|source| browser.adopt_metadata_from(&source));
        if !seeded {
            // Nothing to inherit: still make the card report the tab's scope,
            // so the selector is right while the first load runs. This is
            // also the question the card is asking until a load answers it.
            browser.set_selected_scope(scope);
        }
        ConnectionBrowserEntry {
            owner,
            connection_id: runtime.id(),
            runtime,
            browser,
            seeded_from_sibling: seeded,
        }
    }

    /// A card of the same connection AND the same scope whose metadata a new
    /// card can inherit — the richest one, so an expanded table's columns are
    /// carried over too. A card on another scope is not a source: its tree
    /// describes a different database/schema.
    fn metadata_source_for_scope(
        &self,
        connection_id: ConnectionId,
        scope: Option<&str>,
    ) -> Option<ObjectBrowserWidget> {
        let scope = scope.map(str::trim).filter(|scope| !scope.is_empty());
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        entries
            .iter()
            .filter(|entry| {
                entry.connection_id == connection_id
                    // A catalog read before the connection was rebuilt
                    // describes a server that may no longer be there.
                    && entry
                        .browser
                        .refresh_connection_generation
                        .load(Ordering::Acquire)
                        == entry.runtime.connection_generation()
            })
            .map(|entry| entry.browser.clone())
            .filter(|browser| browser.has_loaded_metadata())
            // Match what was ASKED for. A tab bound by connecting has no
            // scope and therefore asks for the session default; the card that
            // already read that default resolved it to a concrete name, and
            // comparing against the name would refuse the inheritance and
            // force a redundant load.
            .filter(|browser| browser.requested_scope_matches(scope))
            // The NEWEST catalog, not the one with the most expanded tables:
            // a sibling refreshed after a DDL must win over an older card
            // that merely has more columns cached, or the new tab would
            // inherit the pre-DDL catalog and schedule no load of its own.
            .max_by_key(|browser| browser.metadata_serial())
    }

    /// Ensures the CONNECTION's preview card exists (the card the dropdown
    /// shows when the active tab is not bound to this connection, and the
    /// place a new/unbound tab reads its binding from). Tab cards are created
    /// by `set_active_tab`.
    pub fn add_runtime(&mut self, runtime: Arc<ConnectionRuntime>) {
        // A runtime is identified by its id, but an unmanaged or transient
        // runtime can wrap a connection that a registered runtime already owns.
        // Browsing the same connection twice would only duplicate the preview,
        // so keep the one that is already installed.
        if self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .any(|entry| {
                matches!(entry.owner, BrowserOwner::ConnectionPreview(_))
                    && (entry.connection_id == runtime.id()
                        || Arc::ptr_eq(&entry.runtime.connection(), &runtime.connection()))
            })
        {
            self.refresh_runtime_labels();
            return;
        }

        let entry =
            self.create_browser_entry(BrowserOwner::ConnectionPreview(runtime.id()), runtime, None);
        let is_first = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty();
        if is_first {
            entry.browser.get_widget().show();
            *self
                .visible_owner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(entry.owner);
        }
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(entry);
        self.refresh_runtime_labels();
    }

    /// The active query tab changed (or its binding did): make sure that tab
    /// has its OWN card on its connection, show it, and route metadata to it.
    /// An unbound tab (`None`) keeps the current card visible, like before.
    /// Returns whether the tab's card still has to be loaded from the
    /// database — true only when a card was created and had no sibling
    /// catalog to inherit, so the caller must schedule a refresh.
    pub fn set_active_tab(
        &mut self,
        tab_id: QueryTabId,
        runtime: Option<Arc<ConnectionRuntime>>,
        scope: Option<String>,
    ) -> bool {
        let Some(runtime) = runtime else {
            *self
                .active_tab
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            return false;
        };
        let connection_id = runtime.id();
        *self
            .active_tab
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((tab_id, connection_id));

        let owner = BrowserOwner::Tab(tab_id);
        let stale_entry = {
            let entries = self
                .entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            entries
                .iter()
                .find(|entry| entry.owner == owner)
                .map(|entry| (entry.connection_id, entry.browser.get_widget()))
        };
        let create_card = |host: &mut Self| {
            let entry = host.create_browser_entry(owner, runtime.clone(), scope.clone());
            // A card seeded from a sibling already shows the connection's
            // metadata, so it needs no load; only a first card on a
            // connection does.
            let needs_refresh = !entry.seeded_from_sibling;
            host.entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(entry);
            needs_refresh
        };
        let needs_metadata_refresh = match stale_entry {
            Some((existing_connection_id, _)) if existing_connection_id == connection_id => {
                // The tab keeps the card it already arranged: same tree, same
                // expansion, same filter, and no reload.
                false
            }
            Some((_, root)) => {
                // The tab moved to another connection: its old card shows the
                // wrong server. Tear it down and start fresh.
                self.remove_entry_widget(owner, root);
                create_card(self)
            }
            None => create_card(self),
        };

        *self
            .visible_owner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(owner);
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        Self::show_owner_card(&entries, owner);
        self.refresh_runtime_labels();
        app::redraw();
        needs_metadata_refresh
    }

    /// A connection was replaced under its cards (a script `CONNECT` keeps the
    /// same runtime, so nothing else clears them): the catalogs they show
    /// belong to the previous server. Stop them being inherited from — the
    /// trees stay on screen until the reconnect's own refresh replaces them,
    /// but a new tab must load rather than copy them.
    pub fn invalidate_metadata_for_connection(&mut self, connection_id: ConnectionId) {
        let browsers = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|entry| entry.connection_id == connection_id)
            .map(|entry| entry.browser.clone())
            .collect::<Vec<_>>();
        for browser in browsers {
            browser.invalidate_loaded_metadata();
        }
    }

    /// How many browser cards exist, and whether a given tab still owns one.
    /// A closed tab must leave neither behind — its card owns a worker thread,
    /// a poll timer and an FLTK widget tree.
    pub fn card_count(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    pub fn has_card_for_tab(&self, tab_id: QueryTabId) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .any(|entry| entry.owner == BrowserOwner::Tab(tab_id))
    }

    /// Whether the active tab's card has metadata to show. `false` means its
    /// tree is empty and the editor has nothing to highlight with, so the
    /// caller must run (or retry) a metadata refresh.
    pub fn active_tab_has_metadata(&self) -> bool {
        self.bound_browser()
            .is_some_and(|browser| browser.has_loaded_metadata())
    }

    /// Whether the active tab's card still has to be loaded from the
    /// database. A card that is empty *because its load is running* must
    /// answer `false`: scheduling another one would cancel the load in
    /// flight and start it over, so switching away and back during a load
    /// would keep restarting it.
    pub fn active_tab_needs_metadata_load(&self) -> bool {
        self.bound_browser().is_some_and(|browser| {
            !browser.has_loaded_metadata() && !browser.metadata_refresh_in_flight()
        })
    }

    /// The metadata the ACTIVE tab's card holds, for seeding that tab's
    /// editor (intellisense and highlighting) without a database round trip.
    pub fn active_tab_metadata_snapshot(&self) -> Option<ObjectBrowserMetadataSnapshot> {
        let browser = self.bound_browser()?;
        browser
            .has_loaded_metadata()
            .then(|| browser.metadata_snapshot())
    }

    /// The stamp of the catalog the active tab's card holds.
    pub fn active_tab_metadata_serial(&self) -> Option<u64> {
        let browser = self.bound_browser()?;
        browser
            .has_loaded_metadata()
            .then(|| browser.metadata_serial())
    }

    /// The catalog the active tab's card holds, together with the stamp that
    /// says WHICH catalog it is. The editor records the stamp it took, so a
    /// card that has since been refreshed can be recognised and copied
    /// across — no database round trip, and nothing that would clear the
    /// tree the user has arranged.
    pub fn active_tab_metadata_snapshot_with_serial(
        &self,
    ) -> Option<(u64, ObjectBrowserMetadataSnapshot)> {
        let browser = self.bound_browser()?;
        browser
            .has_loaded_metadata()
            .then(|| (browser.metadata_serial(), browser.metadata_snapshot()))
    }

    /// Tears one card down: stops what it is doing, then deletes its widgets.
    ///
    /// The cancel is the load-bearing part. A card's metadata load runs on its
    /// own worker over a pooled DB session, and dropping the card only stops
    /// the UI side of it (the poll loop's `Weak` fails to upgrade and the
    /// callbacks are detached) — the session would stay checked out and
    /// working for nobody until the load finished on its own. Cards used to be
    /// removed only when a connection went away; they are now removed on every
    /// tab close, so cancelling here is what keeps a closed tab from leaving a
    /// server session behind. `cancel_db_activity` never blocks this thread:
    /// it retires the registry entry and hands the actual break to the
    /// watchdog.
    fn remove_entry_widget(&mut self, owner: BrowserOwner, mut root: Flex) {
        let removed = {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            entries
                .iter()
                .position(|entry| entry.owner == owner)
                .map(|index| entries.remove(index))
        };
        let Some(mut removed) = removed else {
            // Nothing was removed, so this owner's card is already gone (a
            // re-entrant teardown through a nested event loop). Deleting the
            // widget again would assert on a dangling pointer.
            return;
        };
        removed.browser.cancel_metadata_refresh();
        // Detach here rather than leaving it to `Drop`: a clone held across a
        // nested event loop would make the card's own `Drop` run AFTER the
        // widgets below are deleted, and its first widget touch would panic
        // inside a destructor.
        removed.browser.detach_callbacks();
        drop(removed);
        root.hide();
        Flex::delete(root);
    }

    /// A query tab closed: tear its card down (the drop stops the card's
    /// worker and poll loop). The connection stays in the dropdown through
    /// its preview card.
    pub fn remove_tab(&mut self, tab_id: QueryTabId) {
        let owner = BrowserOwner::Tab(tab_id);
        let (root, connection_id) = {
            let entries = self
                .entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(entry) = entries.iter().find(|entry| entry.owner == owner) else {
                return;
            };
            (entry.browser.get_widget(), entry.connection_id)
        };
        self.remove_entry_widget(owner, root);
        {
            let mut active = self
                .active_tab
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if active.is_some_and(|(active_tab_id, _)| active_tab_id == tab_id) {
                *active = None;
            }
        }
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let mut visible = self
            .visible_owner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *visible == Some(owner) {
            // Stay on the same connection if it still has a card, so closing
            // a tab does not jump the panel to an unrelated server for the
            // moment before the next tab is activated.
            *visible = entries
                .iter()
                .find(|entry| {
                    entry.connection_id == connection_id
                        && matches!(entry.owner, BrowserOwner::ConnectionPreview(_))
                })
                .or_else(|| {
                    entries
                        .iter()
                        .find(|entry| entry.connection_id == connection_id)
                })
                .or_else(|| entries.first())
                .map(|entry| entry.owner);
            if let Some(next_owner) = *visible {
                Self::show_owner_card(&entries, next_owner);
            }
        }
        drop(visible);
        self.refresh_runtime_labels();
        app::redraw();
    }

    /// A connection went away: tear down its preview card AND every tab card
    /// on it.
    pub fn remove_runtime(&mut self, connection_id: ConnectionId) -> bool {
        let owners = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|entry| entry.connection_id == connection_id)
            .map(|entry| (entry.owner, entry.browser.get_widget()))
            .collect::<Vec<_>>();
        if owners.is_empty() {
            return false;
        }
        for (owner, root) in owners {
            self.remove_entry_widget(owner, root);
        }

        {
            let mut active = self
                .active_tab
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if active.is_some_and(|(_, active_connection_id)| active_connection_id == connection_id)
            {
                *active = None;
            }
        }
        let active_owner = self
            .active_tab
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .map(|(tab_id, _)| BrowserOwner::Tab(tab_id));
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        // Only the removed connection loses its selection. Falling back to
        // the first remaining card unconditionally would show one
        // connection's tree while `visible_owner` still reports another one —
        // and it would drop the user out of the tab they are working in, so
        // the active tab's own card is the first choice.
        {
            let mut visible = self
                .visible_owner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let visible_is_gone =
                !visible.is_some_and(|owner| entries.iter().any(|entry| entry.owner == owner));
            if visible_is_gone {
                *visible = active_owner
                    .filter(|owner| entries.iter().any(|entry| entry.owner == *owner))
                    .or_else(|| entries.first().map(|entry| entry.owner));
            }
            if let Some(owner) = *visible {
                Self::show_owner_card(&entries, owner);
            }
        }
        self.refresh_runtime_labels();
        app::redraw();
        true
    }

    pub fn refresh_runtime_labels(&mut self) {
        let current_value = self.connection_choice.value();
        let visible = *self
            .visible_owner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (mut labels, visible_index) = {
            let entries = self
                .entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let connections = Self::dropdown_connections(&entries);
            let labels = connections
                .iter()
                .filter_map(|connection_id| {
                    entries
                        .iter()
                        .find(|entry| entry.connection_id == *connection_id)
                        .map(|entry| Self::runtime_label(&entry.runtime))
                })
                .collect::<Vec<_>>();
            let visible_connection_id = visible.and_then(|owner| {
                entries
                    .iter()
                    .find(|entry| entry.owner == owner)
                    .map(|entry| entry.connection_id)
            });
            let visible_index = connections
                .iter()
                .position(|connection_id| Some(*connection_id) == visible_connection_id);
            (labels, visible_index)
        };
        // The read-only flag is a property of the saved profile, and a reconnect
        // reuses the same runtime — so the browsers have to be re-told, or a
        // connection the user just marked read-only would keep offering Drop
        // and Truncate for the rest of the session.
        {
            let entries = self
                .entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for entry in entries.iter() {
                entry
                    .browser
                    .set_connection_refuses_writes(entry.runtime.sanitized_info().read_only);
            }
        }
        Self::disambiguate_choice_labels(&mut labels);
        self.connection_choice.clear();
        for label in &labels {
            self.connection_choice.add_choice(label);
        }
        if labels.is_empty() {
            self.connection_choice.deactivate();
            return;
        }
        // `Fl_Menu_::size()` counts the terminating item as well, so clamping
        // against it can select a phantom entry that maps to no connection.
        let last_index = labels.len() as i32 - 1;
        let selected_index = visible_index
            .map(|index| index as i32)
            .unwrap_or_else(|| current_value.clamp(0, last_index));
        self.connection_choice.set_value(selected_index);
        self.connection_choice.activate();
    }

    /// The connection and scope the VISIBLE card shows — what a new or
    /// unbound tab binds to.
    pub fn selected_connection_context(&self) -> Option<(ConnectionId, Option<String>)> {
        let visible = (*self
            .visible_owner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()))?;
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = entries.iter().find(|entry| entry.owner == visible)?;
        Some((entry.connection_id, entry.browser.selected_scope()))
    }

    /// The ACTIVE TAB's own card — where editor-driven reads and writes (its
    /// scope, its metadata refresh) go.
    fn bound_browser(&self) -> Option<ObjectBrowserWidget> {
        let (tab_id, _) = (*self
            .active_tab
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()))?;
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find(|entry| entry.owner == BrowserOwner::Tab(tab_id))
            .map(|entry| entry.browser.clone())
    }

    fn visible_browser(&self) -> Option<ObjectBrowserWidget> {
        let owner = *self
            .visible_owner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find(|entry| Some(entry.owner) == owner)
            .map(|entry| entry.browser.clone())
    }

    pub fn get_widget(&self) -> Flex {
        self.flex.clone()
    }

    pub fn apply_font_settings(&mut self, profile: FontProfile, ui_size: i32) {
        *self
            .font_settings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((profile, ui_size));
        self.connection_choice.set_text_font(profile.normal);
        self.connection_choice.set_text_size(ui_size);
        for entry in self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter_mut()
        {
            entry.browser.apply_font_settings(profile, ui_size);
        }
    }

    pub fn selected_scope(&self) -> Option<String> {
        self.bound_browser()
            .and_then(|browser| browser.selected_scope())
    }

    /// Every node path across the connections' browsers, for the capture tour.
    #[doc(hidden)]
    /// Pick a scope on the card the user is looking at, exactly as the
    /// selector does: the card records it and the app's scope-change callback
    /// fires. For harnesses that need the real end-to-end path.
    #[doc(hidden)]
    pub fn capture_tour_pick_scope(&mut self, scope: Option<String>) {
        let Some(browser) = self.visible_browser() else {
            return;
        };
        browser.capture_tour_pick_scope(scope);
    }

    pub fn capture_tour_tree_paths(&self) -> Vec<String> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .flat_map(|entry| entry.browser.capture_tour_tree_paths())
            .collect()
    }

    /// The ACTIVE TAB's own card: what its tree holds and which of those nodes
    /// are expanded. Cards are per tab, so a harness checking that a tab
    /// switch restored a tab's view must ask this and not the flattened
    /// all-cards list above.
    pub fn active_tab_tree_paths(&self) -> Vec<String> {
        self.bound_browser()
            .map(|browser| browser.capture_tour_tree_paths())
            .unwrap_or_default()
    }

    pub fn active_tab_expanded_paths(&self) -> Vec<String> {
        let Some(browser) = self.bound_browser() else {
            return Vec::new();
        };
        let mut paths = ObjectBrowserWidget::open_tree_paths(&browser.tree)
            .into_iter()
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }

    /// Whether the tab's card is already on `scope`, by the database's own
    /// name comparison. A tab whose card has no entry answers `true`: there is
    /// nothing to bring in sync.
    ///
    /// A tab with NO scope of its own also answers `true`. Its binding says
    /// "wherever the session lands", and a load resolves exactly that — the
    /// server's default schema/database — into the card. Reporting those two
    /// as different would push the resolved name back out of the card and
    /// order a reload on EVERY activation of such a tab (which is every tab
    /// that was bound by connecting), throwing the tree, the filter and the
    /// expansion away each time and never converging, because the next load
    /// resolves the same name again. `schema_update_scope_matches` treats an
    /// unset scope the same way.
    pub fn tab_scope_matches(&self, tab_id: QueryTabId, scope: Option<&str>) -> bool {
        if scope.map(str::trim).is_none_or(str::is_empty) {
            return true;
        }
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find(|entry| entry.owner == BrowserOwner::Tab(tab_id))
            .is_none_or(|entry| entry.browser.scope_matches(scope))
    }

    /// The scope (Oracle schema / MySQL database) the active tab's card is
    /// showing in its selector — the value the user reads, not the stored one.
    pub fn active_tab_displayed_scope(&self) -> Option<String> {
        self.bound_browser()
            .and_then(|browser| browser.scope_choice.choice())
            .map(|scope| scope.trim().to_string())
            .filter(|scope| !scope.is_empty())
    }

    /// Open one tree node by path on every connection's browser, for the
    /// capture tour.
    #[doc(hidden)]
    pub fn capture_tour_expand_path(&self, path: &str) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .any(|entry| entry.browser.capture_tour_expand_path(path))
    }

    /// The card a CONNECTION-keyed read should answer from: the active tab's
    /// own card when it is on this connection, else the connection's preview
    /// card, else any card of the connection.
    fn representative_entry_for_connection(
        &self,
        connection_id: ConnectionId,
    ) -> Option<ConnectionBrowserEntry> {
        let active = *self
            .active_tab
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self::card_owner_for_connection(&entries, active, connection_id)
            .and_then(|owner| entries.iter().find(|entry| entry.owner == owner).cloned())
    }

    pub fn selected_scope_for_connection(&self, connection_id: ConnectionId) -> Option<String> {
        self.representative_entry_for_connection(connection_id)
            .and_then(|entry| entry.browser.selected_scope())
    }

    /// Tells ONE tab's card that its tab is pinned READ ONLY.
    ///
    /// Per tab because the pin is per tab, and stated on its own half of
    /// [`CardWriteRefusal`] because the connection-wide re-labelling states the
    /// other half for every card — including this one — whenever a connection
    /// changes state. Writing one combined answer meant that re-labelling
    /// erased the pin and the card started offering writes again.
    pub fn set_tab_mode_refuses_writes(&mut self, tab_id: QueryTabId, refused: bool) -> bool {
        let browser = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find(|entry| entry.owner == BrowserOwner::Tab(tab_id))
            .map(|entry| entry.browser.clone());
        let Some(browser) = browser else {
            return false;
        };
        browser.set_tab_mode_refuses_writes(refused);
        true
    }

    pub fn set_selected_scope_for_tab(
        &mut self,
        tab_id: QueryTabId,
        scope: Option<String>,
    ) -> bool {
        let browser = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find(|entry| entry.owner == BrowserOwner::Tab(tab_id))
            .map(|entry| entry.browser.clone());
        let Some(mut browser) = browser else {
            return false;
        };
        browser.set_selected_scope(scope);
        true
    }

    pub fn metadata_snapshot_for_connection(
        &self,
        connection_id: ConnectionId,
    ) -> Option<ObjectBrowserMetadataSnapshot> {
        let entry = self.representative_entry_for_connection(connection_id)?;
        let snapshot = entry.browser.metadata_snapshot();
        (snapshot.connection_generation == entry.runtime.connection_generation())
            .then_some(snapshot)
    }

    /// Connection-wide scope write: every card of the connection (each tab's
    /// and the preview) takes the value. Used by connection lifecycle resets;
    /// a user's scope pick goes through the visible card / the tab setter.
    pub fn set_selected_scope_for_connection(
        &mut self,
        connection_id: ConnectionId,
        scope: Option<String>,
    ) -> bool {
        let browsers = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|entry| entry.connection_id == connection_id)
            .map(|entry| entry.browser.clone())
            .collect::<Vec<_>>();
        if browsers.is_empty() {
            return false;
        }
        for mut browser in browsers {
            browser.set_selected_scope(scope.clone());
        }
        true
    }

    pub fn reset_selected_scope(&mut self) {
        if let Some(mut browser) = self.bound_browser() {
            browser.reset_selected_scope();
        }
    }

    pub fn set_selected_scope(&mut self, scope: Option<String>) {
        if let Some(mut browser) = self.bound_browser() {
            browser.set_selected_scope(scope);
        }
    }

    pub fn clear_on_disconnect(&mut self) {
        if let Some(mut browser) = self.bound_browser() {
            browser.clear_on_disconnect();
        }
        self.refresh_runtime_labels();
    }

    /// Clear every card of the connection that just went away — keyed by
    /// connection rather than by what is bound or visible, so disconnecting a
    /// background connection stops its metadata loads too (tab cards
    /// included).
    pub fn clear_on_disconnect_for_connection(&mut self, connection_id: ConnectionId) {
        let browsers = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|entry| entry.connection_id == connection_id)
            .map(|entry| entry.browser.clone())
            .collect::<Vec<_>>();
        for mut browser in browsers {
            browser.clear_on_disconnect();
        }
        self.refresh_runtime_labels();
    }

    fn browsers(&self) -> Vec<ObjectBrowserWidget> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|entry| entry.browser.clone())
            .collect()
    }

    pub fn forget_cancelled_metadata_refresh(&mut self) {
        for mut browser in self.browsers() {
            browser.forget_cancelled_metadata_refresh();
        }
    }

    /// Whether any connection's object browser is still loading metadata.
    pub fn metadata_refresh_in_flight(&self) -> bool {
        self.browsers()
            .iter()
            .any(ObjectBrowserWidget::metadata_refresh_in_flight)
    }

    /// Cancel every in-flight metadata load. Returns whether one was stopped.
    pub fn cancel_metadata_refresh(&mut self) -> bool {
        self.browsers()
            .into_iter()
            .fold(false, |cancelled, mut browser| {
                browser.cancel_metadata_refresh() || cancelled
            })
    }

    pub fn refresh_with_context(&mut self, context: crate::db::DbPoolSessionContext) -> bool {
        self.bound_browser()
            .is_some_and(|mut browser| browser.refresh_with_context(context))
    }

    /// The connection and scope of the card on screen, when that card is NOT
    /// the active tab's — i.e. a preview the user is browsing. Such a card
    /// owns no tab, so nothing else can ask it to reload. The scope comes
    /// from THAT card, not from the connection: another card of the same
    /// connection may be looking somewhere else.
    pub fn visible_preview_context(&self) -> Option<(ConnectionId, Option<String>)> {
        // A PREVIEW specifically, not merely "not the active tab's card":
        // another tab's card can be the visible one transiently, and
        // reloading that would wipe a tree its own tab is still using — and
        // its editor would never receive the result, because delivery is
        // gated on the card's tab being active.
        let visible = match *self
            .visible_owner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            Some(owner @ BrowserOwner::ConnectionPreview(_)) => owner,
            _ => return None,
        };
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = entries.iter().find(|entry| entry.owner == visible)?;
        // The ASK, not the name a load resolved it to: refreshing with the
        // resolved name would turn "wherever the session lands" into an
        // explicit schema, and this card would stop answering the question
        // new tabs ask.
        Some((
            entry.connection_id,
            entry.browser.metadata_requested_scope(),
        ))
    }

    /// Reload the card on screen. Used by Refresh Objects when what the user
    /// is looking at is a preview rather than their own tab's card.
    pub fn refresh_visible_card_with_context(
        &mut self,
        context: crate::db::DbPoolSessionContext,
    ) -> bool {
        self.visible_browser()
            .is_some_and(|mut browser| browser.refresh_with_context(context))
    }

    pub fn show_context_menu_for_sql_selection(
        &self,
        selected_text: &str,
        intellisense_data: &IntellisenseData,
    ) -> bool {
        self.bound_browser().is_some_and(|browser| {
            browser.show_context_menu_for_sql_selection(selected_text, intellisense_data)
        })
    }

    pub fn open_declaration_for_sql_selection(
        &self,
        selected_text: &str,
        intellisense_data: &IntellisenseData,
    ) -> bool {
        self.bound_browser().is_some_and(|browser| {
            browser.open_declaration_for_sql_selection(selected_text, intellisense_data)
        })
    }

    /// Cached object names of the bound connection's current scope, for the
    /// object search dialog.
    pub fn object_cache_snapshot(&self) -> Option<(ObjectCache, Option<String>)> {
        self.bound_browser().map(|browser| {
            (
                browser
                    .object_cache
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone(),
                ObjectBrowserWidget::scope_snapshot(&browser.selected_scope),
            )
        })
    }

    pub fn open_declaration_for_object_item(
        &self,
        item: &ObjectItem,
        selected_scope: Option<String>,
    ) -> bool {
        self.bound_browser()
            .is_some_and(|browser| browser.open_declaration_for_object_item(item, selected_scope))
    }

    pub fn hide_scope_selector_popup_if_outside(&self, root_x: i32, root_y: i32) {
        for entry in self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
        {
            entry
                .browser
                .hide_scope_selector_popup_if_outside(root_x, root_y);
        }
    }

    pub fn has_focus(&self) -> bool {
        widget_has_focus(&self.connection_choice)
            || self
                .visible_browser()
                .is_some_and(|browser| browser.has_focus())
    }

    pub fn copy_focused_selection_to_clipboard(&self) -> bool {
        self.visible_browser()
            .is_some_and(|browser| browser.copy_focused_selection_to_clipboard())
    }

    pub fn capture_tour_set_example_metadata(&mut self) {
        if let Some(mut browser) = self.bound_browser() {
            browser.capture_tour_set_example_metadata();
        }
    }

    pub fn set_sql_callback<F>(&mut self, callback: F)
    where
        F: FnMut(Option<QueryTabId>, ConnectionId, SqlAction) + 'static,
    {
        *self
            .sql_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    pub fn set_status_callback<F>(&mut self, callback: F)
    where
        F: FnMut(&str) + 'static,
    {
        *self
            .status_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    pub fn set_metadata_callback<F>(&mut self, callback: F)
    where
        F: FnMut(QueryTabId, ObjectBrowserMetadataSnapshot) + 'static,
    {
        *self
            .metadata_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    pub fn set_scope_change_callback<F>(&mut self, callback: F)
    where
        F: FnMut(ConnectionId) + 'static,
    {
        *self
            .scope_change_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    pub fn set_scope_switch_preflight_callback<F>(&mut self, callback: F)
    where
        F: FnMut(ConnectionId) -> Result<(), String> + 'static,
    {
        *self
            .scope_switch_preflight_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }
}

/// Read an import file as text.
///
/// Only UTF-8 is accepted, and a file that is not UTF-8 is refused by name
/// rather than mangled into replacement characters that would then be inserted
/// into a table.
fn read_import_file(path: &std::path::Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    String::from_utf8(bytes).map_err(|_| {
        format!(
            "{} is not UTF-8 text. Save it as UTF-8 and import it again.",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{
        consume_owned_key_up, copy_text_for_object_item, CardWriteRefusal, DestructiveObjectAction,
        MultiObjectBrowserWidget, ObjectBrowserMetadataSnapshot, ObjectBrowserWidget, ObjectCache,
        ObjectDefaultAction, ObjectItem, ScopeSwitchPreflightCallback, SCOPE_SELECTOR_ROW_HEIGHT,
        SCOPE_SELECTOR_TABLE_VERTICAL_PADDING,
    };
    use crate::db::{DatabaseType, OracleDriverMode};
    use crate::db::{PackageRoutine, ProcedureArgument};
    use crate::ui::{IntellisenseData, QualifiedMemberKind};
    use fltk::enums::Key;
    use std::sync::{Arc, Mutex};

    use tns_thin::exec::StatementRequest as OracleThinStatementRequest;

    #[test]
    fn neither_source_of_a_write_refusal_can_erase_the_other() {
        // The connection's read-only flag is re-stated for EVERY card whenever
        // the runtimes are re-labelled (a connect, a reconnect, a card added, a
        // disconnect elsewhere). The tab's READ ONLY pin is stated only where
        // the tab's mode is resolved. While they shared one flag, the first
        // writer erased the second's answer and the card started offering
        // Drop, Truncate and Import that the statement gate then refused.
        let refusal = CardWriteRefusal::default();
        assert!(!refusal.writes_are_refused(), "nothing refuses writes yet");

        refusal.set_tab_mode(true);
        assert!(refusal.writes_are_refused(), "the tab is pinned READ ONLY");

        refusal.set_connection(false);
        assert!(
            refusal.writes_are_refused(),
            "re-stating the connection's own flag must not answer for the tab"
        );

        refusal.set_tab_mode(false);
        assert!(!refusal.writes_are_refused(), "the pin is gone");

        refusal.set_connection(true);
        refusal.set_tab_mode(false);
        assert!(
            refusal.writes_are_refused(),
            "and a read-only CONNECTION still refuses writes on an unpinned tab"
        );
    }

    #[test]
    fn key_up_without_matching_key_down_is_not_owned() {
        // An Enter typed in the table browse filter bar deactivates the input,
        // FLTK bounces focus back to this tree, and only the KeyUp arrives here.
        let mut owned = None;
        assert!(!consume_owned_key_up(&mut owned, Key::Enter));
    }

    #[test]
    fn key_up_matching_recorded_key_down_is_owned_once() {
        let mut owned = Some(Key::Enter);
        assert!(consume_owned_key_up(&mut owned, Key::Enter));
        assert_eq!(owned, None);
        assert!(!consume_owned_key_up(&mut owned, Key::Enter));
    }

    #[test]
    fn key_up_for_a_different_key_is_not_owned() {
        let mut owned = Some(Key::Down);
        assert!(!consume_owned_key_up(&mut owned, Key::Enter));
        assert_eq!(owned, None);
    }

    fn oracle_thin_live_connection_info() -> crate::db::ConnectionInfo {
        let host = std::env::var("ORACLE_TEST_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let port = std::env::var("ORACLE_TEST_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(1521);
        let service_name = std::env::var("ORACLE_TEST_SERVICE")
            .or_else(|_| std::env::var("ORACLE_TEST_SERVICE_NAME"))
            .unwrap_or_else(|_| "FREE".to_string());
        let username =
            std::env::var("ORACLE_TEST_USERNAME").unwrap_or_else(|_| "system".to_string());
        let password =
            std::env::var("ORACLE_TEST_PASSWORD").unwrap_or_else(|_| "password".to_string());
        let mut info = crate::db::ConnectionInfo::new_with_type(
            "oracle-thin-live",
            &username,
            &password,
            &host,
            port,
            &service_name,
            DatabaseType::Oracle,
        );
        info.advanced.oracle_driver_mode = OracleDriverMode::Thin;
        info
    }

    fn oracle_thin_live_config() -> tns_thin::OracleThinConfig {
        let info = oracle_thin_live_connection_info();
        let mut config = tns_thin::OracleThinConfig::new(
            tns_thin::ConnectTarget::service_name(info.host, info.port, info.service_name),
            info.username,
            info.password,
        );
        if let Ok(version) = std::env::var("ORACLE_THIN_DESIRED_PROTOCOL") {
            if let Ok(version) = version.trim().parse::<u16>() {
                config.connect_options.desired_protocol_version = version;
            }
        }
        if let Ok(version) = std::env::var("ORACLE_THIN_MINIMUM_PROTOCOL") {
            if let Ok(version) = version.trim().parse::<u16>() {
                config.connect_options.minimum_protocol_version = version;
            }
        }
        config.connect_options.disable_oob_probe = true;
        config
    }

    fn execute_oracle_thin_live_sql(
        session: &mut tns_thin::OracleThinSession,
        sql: impl Into<String>,
    ) {
        session
            .execute(&OracleThinStatementRequest::statement(sql), 0)
            .expect("execute Oracle Thin object-browser UI metadata setup SQL");
    }

    fn procedure_argument(
        name: Option<&str>,
        position: i32,
        data_type: Option<&str>,
        in_out: &str,
    ) -> ProcedureArgument {
        ProcedureArgument {
            name: name.map(str::to_string),
            position,
            sequence: position,
            data_type: data_type.map(str::to_string),
            in_out: Some(in_out.to_string()),
            data_length: None,
            data_precision: None,
            data_scale: None,
            type_owner: None,
            type_name: None,
            pls_type: None,
            overload: None,
            default_value: None,
        }
    }

    fn assert_resolves_simple_object(
        selected_text: &str,
        data: &IntellisenseData,
        cache: Option<&ObjectCache>,
        db_type: DatabaseType,
        current_scope: Option<&str>,
        expected_type: &str,
        expected_scope: Option<&str>,
    ) {
        let resolved = ObjectBrowserWidget::resolve_selected_object_context(
            selected_text,
            data,
            cache,
            db_type,
            current_scope,
        )
        .expect("selection should resolve");

        match resolved.item {
            ObjectItem::Simple { object_type, .. } => assert_eq!(object_type, expected_type),
            _ => panic!("expected simple object"),
        }
        assert_eq!(resolved.selected_scope.as_deref(), expected_scope);
    }

    #[test]
    #[ignore = "requires a reachable Oracle listener"]
    fn oracle_thin_ui_metadata_cache_loads_all_object_categories() {
        let suffix = std::process::id();
        let table = format!("OQT_UI_META_{suffix}");
        let view = format!("OQT_UI_META_V_{suffix}");
        let procedure = format!("OQT_UI_META_P_{suffix}");
        let function = format!("OQT_UI_META_F_{suffix}");
        let sequence = format!("OQT_UI_META_S_{suffix}");
        let trigger = format!("OQT_UI_META_T_{suffix}");
        let synonym = format!("OQT_UI_META_Y_{suffix}");
        let package = format!("OQT_UI_META_G_{suffix}");

        let mut setup_session =
            tns_thin::OracleThinSession::connect(oracle_thin_live_config()).expect("thin login");
        let _ = setup_session.execute(
            &OracleThinStatementRequest::statement(format!("DROP SYNONYM {synonym}")),
            0,
        );
        let _ = setup_session.execute(
            &OracleThinStatementRequest::statement(format!("DROP PACKAGE {package}")),
            0,
        );
        let _ = setup_session.execute(
            &OracleThinStatementRequest::statement(format!("DROP FUNCTION {function}")),
            0,
        );
        let _ = setup_session.execute(
            &OracleThinStatementRequest::statement(format!("DROP PROCEDURE {procedure}")),
            0,
        );
        let _ = setup_session.execute(
            &OracleThinStatementRequest::statement(format!("DROP SEQUENCE {sequence}")),
            0,
        );
        let _ = setup_session.execute(
            &OracleThinStatementRequest::statement(format!("DROP VIEW {view}")),
            0,
        );
        let _ = setup_session.execute(
            &OracleThinStatementRequest::statement(format!("DROP TRIGGER {trigger}")),
            0,
        );
        let _ = setup_session.execute(
            &OracleThinStatementRequest::statement(format!("DROP TABLE {table} PURGE")),
            0,
        );

        execute_oracle_thin_live_sql(
            &mut setup_session,
            format!("CREATE TABLE {table} (id NUMBER PRIMARY KEY, name VARCHAR2(30))"),
        );
        execute_oracle_thin_live_sql(
            &mut setup_session,
            format!("CREATE VIEW {view} AS SELECT id, name FROM {table}"),
        );
        execute_oracle_thin_live_sql(
            &mut setup_session,
            format!("CREATE OR REPLACE PROCEDURE {procedure} IS BEGIN NULL; END;"),
        );
        execute_oracle_thin_live_sql(
            &mut setup_session,
            format!("CREATE OR REPLACE FUNCTION {function} RETURN NUMBER IS BEGIN RETURN 1; END;"),
        );
        execute_oracle_thin_live_sql(
            &mut setup_session,
            format!("CREATE SEQUENCE {sequence} START WITH 1"),
        );
        execute_oracle_thin_live_sql(
            &mut setup_session,
            format!(
                "CREATE OR REPLACE TRIGGER {trigger} BEFORE INSERT ON {table} \
                 FOR EACH ROW BEGIN NULL; END;"
            ),
        );
        execute_oracle_thin_live_sql(
            &mut setup_session,
            format!("CREATE SYNONYM {synonym} FOR {table}"),
        );
        execute_oracle_thin_live_sql(
            &mut setup_session,
            format!("CREATE OR REPLACE PACKAGE {package} AS PROCEDURE P; END;"),
        );

        let shared_connection = crate::db::create_shared_connection();
        {
            let mut connection = crate::db::lock_connection(&shared_connection);
            connection
                .connect(oracle_thin_live_connection_info())
                .expect("connect through UI Oracle Thin connection info");
        }
        let context = crate::db::pool_session_context_for_shared_connection(
            &shared_connection,
            Some("Load object browser metadata"),
        )
        .expect("load UI object browser pool context");
        let (db_type, cache, available_scopes, selected_scope) =
            ObjectBrowserWidget::load_metadata_cache(
                context,
                None,
                &crate::db::track_pool_db_activity(
                    "Load object browser metadata",
                    DatabaseType::Oracle,
                ),
            )
            .expect("load UI object browser metadata cache");

        assert_eq!(db_type, DatabaseType::Oracle);
        let selected_scope = selected_scope.expect("selected Oracle scope");
        assert!(
            available_scopes.contains(&selected_scope),
            "selected scope should be listed in available scopes"
        );
        assert!(cache.tables.contains(&table), "table should load");
        assert!(cache.views.contains(&view), "view should load");
        assert!(
            cache.procedures.contains(&procedure),
            "procedure should load"
        );
        assert!(cache.functions.contains(&function), "function should load");
        assert!(cache.sequences.contains(&sequence), "sequence should load");
        assert!(cache.triggers.contains(&trigger), "trigger should load");
        assert!(cache.synonyms.contains(&synonym), "synonym should load");
        assert!(cache.packages.contains(&package), "package should load");

        {
            let mut connection = crate::db::lock_connection(&shared_connection);
            connection.disconnect();
        }
        crate::db::clear_pool_session_context_for_shared_connection(&shared_connection);

        let _ = setup_session.execute(
            &OracleThinStatementRequest::statement(format!("DROP SYNONYM {synonym}")),
            0,
        );
        let _ = setup_session.execute(
            &OracleThinStatementRequest::statement(format!("DROP PACKAGE {package}")),
            0,
        );
        let _ = setup_session.execute(
            &OracleThinStatementRequest::statement(format!("DROP FUNCTION {function}")),
            0,
        );
        let _ = setup_session.execute(
            &OracleThinStatementRequest::statement(format!("DROP PROCEDURE {procedure}")),
            0,
        );
        let _ = setup_session.execute(
            &OracleThinStatementRequest::statement(format!("DROP SEQUENCE {sequence}")),
            0,
        );
        let _ = setup_session.execute(
            &OracleThinStatementRequest::statement(format!("DROP VIEW {view}")),
            0,
        );
        let _ = setup_session.execute(
            &OracleThinStatementRequest::statement(format!("DROP TRIGGER {trigger}")),
            0,
        );
        let _ = setup_session.execute(
            &OracleThinStatementRequest::statement(format!("DROP TABLE {table} PURGE")),
            0,
        );
    }

    #[test]
    fn copy_text_for_package_routine_uses_qualified_name() {
        let item = ObjectItem::PackageRoutine {
            package_name: "DEMO_PKG".to_string(),
            routine_name: "RUN_JOB".to_string(),
            routine_type: "PROCEDURE".to_string(),
        };

        assert_eq!(copy_text_for_object_item(&item), "DEMO_PKG.RUN_JOB");
    }

    #[test]
    fn metadata_snapshot_reuses_object_browser_cache_for_editor_metadata() {
        let cache = ObjectCache {
            tables: vec!["EMP".to_string()],
            views: vec!["EMP_VIEW".to_string()],
            procedures: vec!["RUN_JOB".to_string()],
            packages: vec!["EMP_API".to_string()],
            ..Default::default()
        };

        let snapshot = ObjectBrowserMetadataSnapshot::from_cache(
            DatabaseType::Oracle,
            7,
            vec!["SCOTT".to_string()],
            Some("SCOTT".to_string()),
            &cache,
        );

        let mut data = snapshot.to_intellisense_data();
        assert_eq!(data.default_qualifier(), Some("SCOTT"));
        assert!(data
            .get_relation_suggestions("EMP")
            .contains(&"EMP".to_string()));
        assert!(data
            .get_member_suggestions("SCOTT", "EMP", true)
            .contains(&"EMP_VIEW".to_string()));
        assert!(!data
            .get_member_suggestions("SCOTT", "RUN", true)
            .contains(&"RUN_JOB".to_string()));
        assert!(data
            .get_member_suggestions("SCOTT", "RUN", false)
            .contains(&"RUN_JOB".to_string()));

        let highlight_data = snapshot.to_highlight_data();
        assert_eq!(highlight_data.tables, vec!["EMP".to_string()]);
        assert_eq!(highlight_data.views, vec!["EMP_VIEW".to_string()]);
    }

    #[test]
    fn metadata_snapshot_canonicalizes_selected_scope_for_editor_metadata() {
        let cache = ObjectCache {
            tables: vec!["OrderLine".to_string()],
            views: vec!["OrderLineView".to_string()],
            ..Default::default()
        };

        let snapshot = ObjectBrowserMetadataSnapshot::from_cache(
            DatabaseType::MySQL,
            7,
            vec!["SalesDb".to_string()],
            Some("salesdb".to_string()),
            &cache,
        );

        let mut data = snapshot.to_intellisense_data();
        assert_eq!(data.default_qualifier_name(), Some("SalesDb"));
        assert!(data
            .get_member_suggestions("SalesDb", "Order", true)
            .contains(&"OrderLine".to_string()));
        assert!(data
            .get_member_suggestions("salesdb", "Order", true)
            .contains(&"OrderLineView".to_string()));
    }

    #[test]
    fn metadata_snapshot_preserves_oracle_selected_scope_case_for_editor_metadata() {
        let cache = ObjectCache {
            tables: vec!["EMP".to_string()],
            ..Default::default()
        };

        let snapshot = ObjectBrowserMetadataSnapshot::from_cache(
            DatabaseType::Oracle,
            7,
            vec!["MixedCase".to_string()],
            Some("mixedcase".to_string()),
            &cache,
        );

        let data = snapshot.to_intellisense_data();
        assert_eq!(data.default_qualifier_name(), Some("mixedcase"));
    }

    #[test]
    fn scope_switch_preflight_callback_restores_after_panic() {
        let callback_slot: ScopeSwitchPreflightCallback = Arc::new(Mutex::new(None));
        let calls = Arc::new(Mutex::new(0));
        let calls_for_callback = calls.clone();
        *callback_slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(move || {
            *calls_for_callback
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) += 1;
            panic!("expected test panic");
        }));

        let result = ObjectBrowserWidget::invoke_scope_switch_preflight_callback(&callback_slot);
        assert!(result.is_err());
        let result = ObjectBrowserWidget::invoke_scope_switch_preflight_callback(&callback_slot);
        assert!(result.is_err());
        assert_eq!(
            *calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            2
        );
    }

    #[test]
    fn scope_switch_preflight_callback_reports_callback_error() {
        let callback_slot: ScopeSwitchPreflightCallback = Arc::new(Mutex::new(None));
        *callback_slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(Box::new(|| Err("blocked".to_string())));

        let result = ObjectBrowserWidget::invoke_scope_switch_preflight_callback(&callback_slot);
        assert_eq!(result, Err("blocked".to_string()));
    }

    #[test]
    fn connection_choice_labels_escape_fltk_menu_syntax() {
        assert_eq!(
            MultiObjectBrowserWidget::escape_choice_label("dev/qa"),
            "dev\\/qa"
        );
        assert_eq!(
            MultiObjectBrowserWidget::escape_choice_label("dev|qa"),
            "dev qa"
        );
        assert_eq!(
            MultiObjectBrowserWidget::escape_choice_label("_staging"),
            "\\_staging"
        );
        assert_eq!(
            MultiObjectBrowserWidget::escape_choice_label("a&b\\c"),
            "a\\&b\\\\c"
        );
        assert_eq!(
            MultiObjectBrowserWidget::escape_choice_label("dev\tqa"),
            "dev qa"
        );
        assert_eq!(
            MultiObjectBrowserWidget::escape_choice_label("prod db_1"),
            "prod db_1"
        );
    }

    #[test]
    fn connection_choice_labels_stay_distinct_for_equal_connection_names() {
        let mut labels = vec![
            "prod".to_string(),
            "prod".to_string(),
            "prod #2".to_string(),
            "dev".to_string(),
            "prod".to_string(),
        ];
        MultiObjectBrowserWidget::disambiguate_choice_labels(&mut labels);

        assert_eq!(
            labels,
            vec![
                "prod".to_string(),
                "prod #2".to_string(),
                "prod #2 #2".to_string(),
                "dev".to_string(),
                "prod #3".to_string(),
            ]
        );
    }

    #[test]
    fn preview_select_sql_uses_mysql_limit_and_identifier_quotes() {
        let sql = ObjectBrowserWidget::preview_select_sql(
            crate::db::DatabaseType::MySQL,
            None,
            "order.items",
        );

        assert_eq!(sql, "SELECT * FROM `order`.`items` LIMIT 100");
    }

    #[test]
    fn preview_select_sql_preserves_mysql_quoted_dotted_identifier_segments() {
        let sql = ObjectBrowserWidget::preview_select_sql(
            crate::db::DatabaseType::MySQL,
            None,
            "`sales.ops`.`order.items`",
        );

        assert_eq!(sql, "SELECT * FROM `sales.ops`.`order.items` LIMIT 100");
    }

    #[test]
    fn preview_select_sql_qualifies_oracle_object_name_with_selected_owner() {
        let sql = ObjectBrowserWidget::preview_select_sql(
            crate::db::DatabaseType::Oracle,
            Some("SCOTT"),
            "EMP",
        );

        assert_eq!(sql, "SELECT * FROM SCOTT.EMP WHERE ROWNUM <= 100");
    }

    #[test]
    fn preview_select_sql_qualifies_mysql_object_name_with_selected_database() {
        let sql = ObjectBrowserWidget::preview_select_sql(
            crate::db::DatabaseType::MySQL,
            Some("sales"),
            "orders",
        );

        assert_eq!(sql, "SELECT * FROM `sales`.`orders` LIMIT 100");
    }

    #[test]
    fn drop_statements_name_the_object_kind_per_backend() {
        let oracle = |object_type: &str| {
            ObjectBrowserWidget::destructive_object_sql(
                DatabaseType::Oracle,
                DestructiveObjectAction::Drop,
                Some("SCOTT"),
                object_type,
                "EMP",
            )
        };

        assert_eq!(oracle("TABLES").as_deref(), Some("DROP TABLE SCOTT.EMP"));
        assert_eq!(oracle("VIEWS").as_deref(), Some("DROP VIEW SCOTT.EMP"));
        assert_eq!(
            oracle("MATERIALIZED VIEWS").as_deref(),
            Some("DROP MATERIALIZED VIEW SCOTT.EMP")
        );
        assert_eq!(
            oracle("PACKAGES").as_deref(),
            Some("DROP PACKAGE SCOTT.EMP")
        );

        assert_eq!(
            ObjectBrowserWidget::destructive_object_sql(
                DatabaseType::MySQL,
                DestructiveObjectAction::Drop,
                Some("sales"),
                "TABLES",
                "orders",
            )
            .as_deref(),
            Some("DROP TABLE `sales`.`orders`")
        );
        assert_eq!(
            ObjectBrowserWidget::destructive_object_sql(
                DatabaseType::MariaDB,
                DestructiveObjectAction::Drop,
                Some("sales"),
                "EVENTS",
                "nightly",
            )
            .as_deref(),
            Some("DROP EVENT `sales`.`nightly`")
        );
    }

    #[test]
    fn truncate_is_offered_for_tables_only() {
        for db_type in [DatabaseType::Oracle, DatabaseType::MySQL] {
            assert!(ObjectBrowserWidget::destructive_object_sql(
                db_type,
                DestructiveObjectAction::Truncate,
                None,
                "VIEWS",
                "v_sales",
            )
            .is_none());
        }

        assert_eq!(
            ObjectBrowserWidget::destructive_object_sql(
                DatabaseType::Oracle,
                DestructiveObjectAction::Truncate,
                Some("SCOTT"),
                "TABLES",
                "EMP",
            )
            .as_deref(),
            Some("TRUNCATE TABLE SCOTT.EMP")
        );
        assert_eq!(
            ObjectBrowserWidget::destructive_object_sql(
                DatabaseType::MySQL,
                DestructiveObjectAction::Truncate,
                Some("sales"),
                "TABLES",
                "orders",
            )
            .as_deref(),
            Some("TRUNCATE TABLE `sales`.`orders`")
        );
    }

    #[test]
    fn drop_statement_carries_no_cascade_or_purge_the_user_did_not_read() {
        let sql = ObjectBrowserWidget::destructive_object_sql(
            DatabaseType::Oracle,
            DestructiveObjectAction::Drop,
            Some("SCOTT"),
            "TABLES",
            "EMP",
        )
        .expect("oracle tables drop");

        assert_eq!(sql, "DROP TABLE SCOTT.EMP");
    }

    /// The context menu must offer a destructive action exactly when there is a
    /// statement behind it, or a user picks an item that can only fail.
    #[test]
    fn every_offered_destructive_menu_item_has_a_statement_behind_it() {
        const OBJECT_TYPES: [&str; 12] = [
            "TABLES",
            "VIEWS",
            "MATERIALIZED VIEWS",
            "PROCEDURES",
            "FUNCTIONS",
            "SEQUENCES",
            "TRIGGERS",
            "SYNONYMS",
            "PACKAGES",
            "EVENTS",
            "TYPES",
            "INDEXES",
        ];

        for db_type in [
            DatabaseType::Oracle,
            DatabaseType::MySQL,
            DatabaseType::MariaDB,
        ] {
            for object_type in OBJECT_TYPES {
                let item = simple_item(object_type, "OBJ");
                let choices = ObjectBrowserWidget::menu_choices_for_object_item(&item, db_type)
                    .unwrap_or_default();
                for (label, action) in [
                    (
                        DestructiveObjectAction::DROP_LABEL,
                        DestructiveObjectAction::Drop,
                    ),
                    (
                        DestructiveObjectAction::TRUNCATE_LABEL,
                        DestructiveObjectAction::Truncate,
                    ),
                ] {
                    let offered = choices.split('|').any(|choice| choice == label);
                    let runnable = ObjectBrowserWidget::destructive_object_sql(
                        db_type,
                        action,
                        None,
                        object_type,
                        "OBJ",
                    )
                    .is_some();
                    assert_eq!(
                        offered, runnable,
                        "{:?} / {} / {}",
                        db_type, object_type, label
                    );
                }
            }
        }
    }

    fn simple_item(object_type: &str, object_name: &str) -> ObjectItem {
        ObjectItem::Simple {
            object_type: object_type.to_string(),
            object_name: object_name.to_string(),
        }
    }

    fn default_action(
        item: &ObjectItem,
        db_type: DatabaseType,
        scope: Option<&str>,
    ) -> ObjectDefaultAction {
        ObjectBrowserWidget::default_action_for_item(Some(item), db_type, scope)
    }

    #[test]
    fn table_default_action_browses_an_editable_target() {
        let ObjectDefaultAction::Browse(target) = default_action(
            &simple_item("TABLES", "EMP"),
            DatabaseType::Oracle,
            Some("SCOTT"),
        ) else {
            panic!("tables should browse data");
        };

        assert_eq!(target.table_name, "EMP");
        assert_eq!(target.relation_sql, "SCOTT.EMP");
        assert!(target.editable);
    }

    fn column_detail(name: &str, data_type: &str) -> super::TableColumnDetail {
        super::TableColumnDetail {
            name: name.to_string(),
            data_type: data_type.to_string(),
            data_length: 0,
            data_precision: None,
            data_scale: None,
            nullable: true,
            default_value: None,
            is_primary_key: false,
        }
    }

    #[test]
    fn a_column_node_reads_as_its_name_and_type() {
        let column = column_detail("EMPNO", "NUMBER");
        let label = ObjectBrowserWidget::column_node_label(&column);
        assert!(label.starts_with("EMPNO"));
        assert!(label.contains("NUMBER"));
    }

    #[test]
    fn a_column_is_found_from_its_label_without_parsing_it() {
        // A name holding the separator is exactly what a parsing rule would get
        // wrong, so the lookup regenerates labels and compares instead.
        let columns = vec![
            column_detail("EMPNO", "NUMBER"),
            column_detail("ODD  NAME", "VARCHAR2"),
        ];
        for column in &columns {
            let label = ObjectBrowserWidget::column_node_label(column);
            assert_eq!(
                ObjectBrowserWidget::column_name_for_node_label(&columns, &label).as_deref(),
                Some(column.name.as_str()),
                "{label}"
            );
        }
        assert!(
            ObjectBrowserWidget::column_name_for_node_label(&columns, "NOT A COLUMN").is_none()
        );
    }

    #[test]
    fn only_expanded_tables_contribute_column_paths() {
        let mut cache = ObjectCache {
            tables: vec!["EMP".to_string(), "DEPT".to_string()],
            ..ObjectCache::default()
        };
        let paths = ObjectBrowserWidget::collect_tree_paths(&cache, "");
        assert!(paths.contains(&"Tables/EMP".to_string()));
        assert!(!paths.iter().any(|path| path.starts_with("Tables/EMP/")));

        cache.table_columns.insert(
            "EMP".to_string(),
            vec![
                column_detail("EMPNO", "NUMBER"),
                column_detail("ENAME", "VARCHAR2"),
            ],
        );
        let paths = ObjectBrowserWidget::collect_tree_paths(&cache, "");
        assert_eq!(
            paths
                .iter()
                .filter(|path| path.starts_with("Tables/EMP/"))
                .count(),
            2
        );
        // A table that was never expanded still has no children.
        assert!(!paths.iter().any(|path| path.starts_with("Tables/DEPT/")));
    }

    #[test]
    fn filtering_a_table_out_takes_its_columns_with_it() {
        let mut cache = ObjectCache {
            tables: vec!["EMP".to_string(), "DEPT".to_string()],
            ..ObjectCache::default()
        };
        cache
            .table_columns
            .insert("EMP".to_string(), vec![column_detail("EMPNO", "NUMBER")]);
        let paths = ObjectBrowserWidget::collect_tree_paths(&cache, "dept");
        assert!(!paths.iter().any(|path| path.starts_with("Tables/EMP")));
    }

    #[test]
    fn refreshed_metadata_drops_cached_columns_so_an_alter_is_not_missed() {
        let mut target = ObjectCache {
            tables: vec!["EMP".to_string()],
            ..ObjectCache::default()
        };
        target
            .table_columns
            .insert("EMP".to_string(), vec![column_detail("EMPNO", "NUMBER")]);
        let partial = ObjectCache {
            tables: vec!["EMP".to_string()],
            ..ObjectCache::default()
        };
        ObjectBrowserWidget::merge_object_metadata_cache(&mut target, partial);
        assert!(target.table_columns.is_empty());
    }

    #[test]
    fn a_column_copies_and_drags_as_its_bare_name() {
        let item = ObjectItem::Column {
            table_name: "EMP".to_string(),
            column_name: "ENAME".to_string(),
        };
        assert_eq!(copy_text_for_object_item(&item), "ENAME");
        assert_eq!(
            ObjectBrowserWidget::copy_text_for_object_item_with_scope(
                &item,
                crate::db::DatabaseType::Oracle,
                Some("HR"),
            ),
            "ENAME"
        );
        // Not something "go to declaration" can open.
        assert!(ObjectBrowserWidget::declaration_target_for_item(&item).is_none());
    }

    #[test]
    fn export_file_stem_uses_the_object_name_and_stays_a_legal_file_name() {
        assert_eq!(ObjectBrowserWidget::export_file_stem("HR.EMP"), "EMP");
        assert_eq!(
            ObjectBrowserWidget::export_file_stem("`shop`.`order items`"),
            "order_items"
        );
        assert_eq!(
            ObjectBrowserWidget::export_file_stem("\"odd/name\""),
            "odd_name"
        );
        assert_eq!(ObjectBrowserWidget::export_file_stem("사원"), "사원");
        // Nothing usable left is still a usable file name.
        assert_eq!(ObjectBrowserWidget::export_file_stem("..."), "export");
    }

    #[test]
    fn a_tree_export_renders_the_same_bytes_a_grid_export_would() {
        use crate::db::{ColumnInfo, DatabaseType, QueryCell, QueryResult, SqlValueKind};
        use crate::ui::result_export::{ExportDestination, ExportFormat, ExportGrid, ExportScope};
        use crate::ui::result_export_dialog::ExportChoice;

        let null_text = crate::ui::result_table::ResultTableWidget::DEFAULT_NULL_TEXT.to_string();
        let result = QueryResult::new_select(
            "SELECT * FROM HR.EMP",
            vec![
                ColumnInfo {
                    name: "EMPNO".to_string(),
                    data_type: "NUMBER".to_string(),
                    kind: SqlValueKind::Number,
                },
                ColumnInfo {
                    name: "ENAME".to_string(),
                    data_type: "VARCHAR2".to_string(),
                    kind: SqlValueKind::String,
                },
            ],
            vec![
                vec!["7369".to_string(), "SMITH".to_string()],
                vec!["7499".to_string(), QueryCell::null_result_text()],
            ],
            std::time::Duration::from_millis(1),
        );

        for format in ExportFormat::ALL {
            let delivery = ObjectBrowserWidget::render_table_export(
                "HR.EMP",
                DatabaseType::Oracle,
                ExportChoice {
                    format,
                    scope: ExportScope::All,
                    destination: ExportDestination::Clipboard,
                },
                &result,
            );
            let grid = ExportGrid {
                columns: vec!["EMPNO".to_string(), "ENAME".to_string()],
                column_kinds: vec![SqlValueKind::Number, SqlValueKind::String],
                rows: vec![
                    vec!["7369".to_string(), "SMITH".to_string()],
                    vec!["7499".to_string(), null_text.clone()],
                ],
                null_text: null_text.clone(),
            };
            let selection = crate::ui::grid_sql_export::GridSqlSelection {
                db_type: DatabaseType::Oracle,
                table: Some("HR.EMP".to_string()),
                all_columns: grid.columns.clone(),
                column_kinds: grid.column_kinds.clone(),
                selected_columns: vec![0, 1],
                rows: grid.rows.clone(),
                null_text: null_text.clone(),
            };
            let (expected, expected_rows) =
                crate::ui::result_export::render_export_content(format, &grid, Some(&selection));
            assert_eq!(delivery.text, expected, "{format:?} bytes differ");
            assert_eq!(
                delivery.row_count, expected_rows,
                "{format:?} row count differs"
            );
        }
    }

    #[test]
    fn a_file_export_carries_the_byte_order_mark_and_the_clipboard_does_not() {
        use crate::db::{ColumnInfo, DatabaseType, QueryResult, SqlValueKind};
        use crate::ui::result_export::{ExportDestination, ExportFormat, ExportScope};
        use crate::ui::result_export_dialog::ExportChoice;

        let result = QueryResult::new_select(
            "SELECT * FROM HR.EMP",
            vec![ColumnInfo {
                name: "ENAME".to_string(),
                data_type: "VARCHAR2".to_string(),
                kind: SqlValueKind::String,
            }],
            vec![vec!["SMITH".to_string()]],
            std::time::Duration::from_millis(1),
        );
        let choice = |destination| ExportChoice {
            format: ExportFormat::Csv,
            scope: ExportScope::All,
            destination,
        };
        let to_file = ObjectBrowserWidget::render_table_export(
            "HR.EMP",
            DatabaseType::Oracle,
            choice(ExportDestination::File),
            &result,
        );
        let to_clipboard = ObjectBrowserWidget::render_table_export(
            "HR.EMP",
            DatabaseType::Oracle,
            choice(ExportDestination::Clipboard),
            &result,
        );
        assert_eq!(&to_file.text.as_bytes()[..3], &[0xEF, 0xBB, 0xBF]);
        assert!(!to_clipboard.text.starts_with('\u{feff}'));
    }

    #[test]
    fn every_backend_exports_the_whole_table_without_a_row_limit() {
        // The preview caps rows on purpose; an export that inherited that cap
        // would write a file that looks complete and is not.
        for db_type in [
            crate::db::DatabaseType::Oracle,
            crate::db::DatabaseType::MySQL,
            crate::db::DatabaseType::MariaDB,
        ] {
            let behavior = super::object_browser_behavior_for(db_type);
            let export = behavior.export_select_sql(Some("HR"), "EMP");
            let upper = export.to_uppercase();
            assert!(!upper.contains("ROWNUM"), "{db_type:?}: {export}");
            assert!(!upper.contains("LIMIT"), "{db_type:?}: {export}");
            assert!(upper.starts_with("SELECT * FROM "), "{db_type:?}: {export}");
        }
    }

    #[test]
    fn read_only_menu_drops_every_write_capable_entry() {
        let table_menu = "Select Data (Top 100)|Import Data...|Export Data...|View Structure|\
                          View Indexes|View Constraints|Generate DDL|Truncate...|Drop...";
        assert_eq!(
            ObjectBrowserWidget::menu_choices_for_read_only(table_menu, true).as_deref(),
            // Export survives: it only reads, so a read-only connection keeps it.
            Some(
                "Select Data (Top 100)|Export Data...|View Structure|View Indexes|View \
                 Constraints|Generate DDL"
            )
        );
        // Reading the catalog is still on offer.
        assert_eq!(
            ObjectBrowserWidget::menu_choices_for_read_only(
                "Check Compilation|Generate DDL|Drop...",
                true
            )
            .as_deref(),
            Some("Check Compilation|Generate DDL")
        );
        // Nothing left means no menu at all, rather than an empty one.
        assert_eq!(
            ObjectBrowserWidget::menu_choices_for_read_only("Execute Procedure", true),
            None
        );
    }

    #[test]
    fn a_writable_connection_keeps_every_menu_entry() {
        for db_type in [
            crate::db::DatabaseType::Oracle,
            crate::db::DatabaseType::MySQL,
            crate::db::DatabaseType::MariaDB,
        ] {
            for object_type in [
                "TABLES",
                "VIEWS",
                "PROCEDURES",
                "FUNCTIONS",
                "SEQUENCES",
                "TRIGGERS",
                "PACKAGES",
                "SYNONYMS",
            ] {
                let item = ObjectItem::Simple {
                    object_type: object_type.to_string(),
                    object_name: "X".to_string(),
                };
                let Some(choices) =
                    ObjectBrowserWidget::menu_choices_for_object_item(&item, db_type)
                else {
                    continue;
                };
                assert_eq!(
                    ObjectBrowserWidget::menu_choices_for_read_only(choices, false).as_deref(),
                    Some(choices),
                    "{db_type:?}/{object_type} lost an entry while writable"
                );
                // And every read-only menu is a subset of the writable one.
                if let Some(read_only) =
                    ObjectBrowserWidget::menu_choices_for_read_only(choices, true)
                {
                    for label in read_only.split('|') {
                        assert!(
                            choices.split('|').any(|candidate| candidate == label),
                            "{db_type:?}/{object_type} invented the entry {label:?}"
                        );
                        assert!(
                            !ObjectBrowserWidget::WRITE_CAPABLE_MENU_LABELS.contains(&label),
                            "{db_type:?}/{object_type} kept the write-capable entry {label:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn view_default_action_browses_read_only() {
        let ObjectDefaultAction::Browse(target) = default_action(
            &simple_item("VIEWS", "EMP_VIEW"),
            DatabaseType::Oracle,
            Some("SCOTT"),
        ) else {
            panic!("views should browse data");
        };

        assert_eq!(target.relation_sql, "SCOTT.EMP_VIEW");
        assert!(!target.editable);
    }

    #[test]
    fn mysql_view_browse_target_quotes_the_relation() {
        let ObjectDefaultAction::Browse(target) = default_action(
            &simple_item("VIEWS", "order summary"),
            DatabaseType::MySQL,
            Some("sales"),
        ) else {
            panic!("views should browse data");
        };

        assert_eq!(target.relation_sql, "`sales`.`order summary`");
        assert!(!target.editable);
    }

    #[test]
    fn source_objects_default_to_opening_their_ddl() {
        for (db_type, object_type, expected_ddl_type) in [
            (DatabaseType::Oracle, "PROCEDURES", "PROCEDURE"),
            (DatabaseType::Oracle, "FUNCTIONS", "FUNCTION"),
            (DatabaseType::Oracle, "SEQUENCES", "SEQUENCE"),
            (DatabaseType::Oracle, "TRIGGERS", "TRIGGER"),
            (DatabaseType::Oracle, "SYNONYMS", "SYNONYM"),
            (DatabaseType::MySQL, "PROCEDURES", "PROCEDURE"),
            (DatabaseType::MySQL, "EVENTS", "EVENT"),
            (DatabaseType::MariaDB, "SEQUENCES", "SEQUENCE"),
            (DatabaseType::MariaDB, "TRIGGERS", "TRIGGER"),
        ] {
            assert_eq!(
                default_action(&simple_item(object_type, "OBJ"), db_type, Some("APP")),
                ObjectDefaultAction::GenerateDdl {
                    object_type: expected_ddl_type,
                    object_name: "OBJ".to_string(),
                },
                "{object_type} on {db_type:?}"
            );
        }
    }

    #[test]
    fn package_node_expands_and_its_routines_open_the_package_ddl() {
        assert_eq!(
            default_action(
                &simple_item("PACKAGES", "DEMO_PKG"),
                DatabaseType::Oracle,
                Some("SCOTT")
            ),
            ObjectDefaultAction::PackageNode
        );

        let routine = ObjectItem::PackageRoutine {
            package_name: "DEMO_PKG".to_string(),
            routine_name: "RUN_JOB".to_string(),
            routine_type: "PROCEDURE".to_string(),
        };
        assert_eq!(
            default_action(&routine, DatabaseType::Oracle, Some("SCOTT")),
            ObjectDefaultAction::GenerateDdl {
                object_type: "PACKAGE",
                object_name: "DEMO_PKG".to_string(),
            }
        );
    }

    #[test]
    fn category_nodes_toggle_instead_of_acting_on_an_object() {
        // Category folders and package member groups carry no object info.
        assert_eq!(
            ObjectBrowserWidget::default_action_for_item(None, DatabaseType::Oracle, Some("SCOTT")),
            ObjectDefaultAction::ToggleNode
        );
    }

    #[test]
    fn mysql_scope_for_context_falls_back_to_current_database() {
        assert_eq!(
            ObjectBrowserWidget::mysql_scope_for_context(None, "sales"),
            Some("sales")
        );
        assert_eq!(
            ObjectBrowserWidget::mysql_scope_for_context(Some("hr"), "sales"),
            Some("hr")
        );
        assert_eq!(
            ObjectBrowserWidget::mysql_scope_for_context(None, " "),
            None
        );
    }

    #[test]
    fn oracle_object_names_quote_case_sensitive_scope_and_object_names() {
        assert_eq!(
            ObjectBrowserWidget::qualify_oracle_object_name(Some("app_user"), "ORDERS"),
            r#""app_user".ORDERS"#
        );
        assert_eq!(
            ObjectBrowserWidget::qualify_oracle_object_name(Some("SCOTT"), "orderLines"),
            r#"SCOTT."orderLines""#
        );
    }

    #[test]
    fn oracle_package_member_names_quote_case_sensitive_routines() {
        assert_eq!(
            ObjectBrowserWidget::qualify_package_member_name(
                DatabaseType::Oracle,
                Some("app_user"),
                "pkg_api",
                "runJob",
            ),
            r#""app_user"."pkg_api"."runJob""#
        );
    }

    #[test]
    fn copy_text_for_object_item_with_scope_qualifies_package_routine() {
        let item = ObjectItem::PackageRoutine {
            package_name: "DEMO_PKG".to_string(),
            routine_name: "RUN_JOB".to_string(),
            routine_type: "PROCEDURE".to_string(),
        };

        let text = ObjectBrowserWidget::copy_text_for_object_item_with_scope(
            &item,
            DatabaseType::Oracle,
            Some("SCOTT"),
        );

        assert_eq!(text, "SCOTT.DEMO_PKG.RUN_JOB");
    }

    #[test]
    fn oracle_scope_option_index_keeps_case_distinct() {
        let options = vec!["SCOTT".to_string(), "scott".to_string(), "APP".to_string()];

        assert_eq!(
            ObjectBrowserWidget::scope_option_index_for_db_type(
                DatabaseType::Oracle,
                &options,
                " SCOTT "
            ),
            Some(0)
        );
        assert_eq!(
            ObjectBrowserWidget::scope_option_index_for_db_type(
                DatabaseType::Oracle,
                &options,
                "scott"
            ),
            Some(1)
        );
        assert_eq!(
            ObjectBrowserWidget::scope_option_index_for_db_type(
                DatabaseType::Oracle,
                &options,
                "missing"
            ),
            None
        );
    }

    #[test]
    fn oracle_scope_options_match_uses_exact_case() {
        let current = vec![" SCOTT ".to_string(), "HR".to_string()];
        let desired = vec!["SCOTT".to_string(), "HR".to_string()];

        assert!(ObjectBrowserWidget::scope_options_match_for_db_type(
            DatabaseType::Oracle,
            &current,
            &desired
        ));
        assert!(!ObjectBrowserWidget::scope_options_match_for_db_type(
            DatabaseType::Oracle,
            &["SCOTT".to_string()],
            &["scott".to_string()]
        ));
    }

    #[test]
    fn database_type_scope_matching_trims_and_treats_empty_as_same_scope() {
        assert!(DatabaseType::Oracle.scope_values_match(Some(" SCOTT "), Some("SCOTT")));
        assert!(DatabaseType::MySQL.scope_values_match(Some(" "), None));
        assert!(!DatabaseType::Oracle.scope_values_match(Some("SCOTT"), Some("scott")));
    }

    #[test]
    fn mysql_scope_matching_keeps_case_distinct() {
        assert!(!ObjectBrowserWidget::scope_values_match_for_db_type(
            DatabaseType::MySQL,
            Some("sales"),
            Some("Sales")
        ));
        assert!(ObjectBrowserWidget::scope_values_match_for_db_type(
            DatabaseType::MySQL,
            Some(" sales "),
            Some("sales")
        ));

        let options = vec!["sales".to_string(), "Sales".to_string()];
        assert_eq!(
            ObjectBrowserWidget::scope_option_index_for_db_type(
                DatabaseType::MySQL,
                &options,
                "Sales"
            ),
            Some(1)
        );
        assert!(!ObjectBrowserWidget::scope_options_match_for_db_type(
            DatabaseType::MySQL,
            &["sales".to_string()],
            &["Sales".to_string()]
        ));
    }

    #[test]
    fn scope_choice_is_disabled_while_scope_switch_is_in_progress() {
        assert!(ObjectBrowserWidget::scope_choice_should_be_active(2, false));
        assert!(!ObjectBrowserWidget::scope_choice_should_be_active(2, true));
        assert!(!ObjectBrowserWidget::scope_choice_should_be_active(
            0, false
        ));
    }

    #[test]
    fn scope_choice_sync_defers_during_menu_grab_or_selector_popup() {
        assert!(ObjectBrowserWidget::scope_choice_sync_should_defer(
            true, false
        ));
        assert!(ObjectBrowserWidget::scope_choice_sync_should_defer(
            false, true
        ));
        assert!(!ObjectBrowserWidget::scope_choice_sync_should_defer(
            false, false
        ));
    }

    #[test]
    fn scope_selector_initial_row_is_clamped_to_available_options() {
        assert_eq!(ObjectBrowserWidget::scope_selector_initial_row(-1, 3), 0);
        assert_eq!(ObjectBrowserWidget::scope_selector_initial_row(1, 3), 1);
        assert_eq!(ObjectBrowserWidget::scope_selector_initial_row(99, 3), 2);
        assert_eq!(ObjectBrowserWidget::scope_selector_initial_row(99, 0), 0);
    }

    #[test]
    fn scope_selector_popup_width_matches_choice_width() {
        assert_eq!(ObjectBrowserWidget::scope_selector_popup_width(180), 180);
        assert_eq!(ObjectBrowserWidget::scope_selector_popup_width(420), 420);
        assert_eq!(ObjectBrowserWidget::scope_selector_popup_width(0), 1);
    }

    #[test]
    fn scope_selector_list_width_reserves_space_for_vertical_scrollbar() {
        assert_eq!(
            ObjectBrowserWidget::scope_selector_list_width(260, false),
            260
        );
        assert_eq!(
            ObjectBrowserWidget::scope_selector_list_width(260, true),
            260 - ObjectBrowserWidget::scope_selector_scrollbar_size()
        );
    }

    #[test]
    fn scope_selector_scrollbar_is_needed_only_when_rows_overflow() {
        let popup_h = ObjectBrowserWidget::scope_selector_popup_height_for_rows(3);
        assert!(!ObjectBrowserWidget::scope_selector_needs_scrollbar(
            popup_h, 3
        ));
        assert!(ObjectBrowserWidget::scope_selector_needs_scrollbar(
            popup_h, 4
        ));
    }

    #[test]
    fn scope_selector_visible_rows_excludes_table_padding() {
        let popup_h = SCOPE_SELECTOR_ROW_HEIGHT * 3 + SCOPE_SELECTOR_TABLE_VERTICAL_PADDING - 1;
        assert_eq!(
            ObjectBrowserWidget::scope_selector_visible_rows_for_height(popup_h),
            2
        );
        assert_eq!(
            ObjectBrowserWidget::scope_selector_visible_rows_for_height(
                ObjectBrowserWidget::scope_selector_popup_height_for_rows(3)
            ),
            3
        );
    }

    #[test]
    fn scope_selector_popup_height_snaps_to_full_rows() {
        assert_eq!(
            ObjectBrowserWidget::scope_selector_popup_height_for_available_height(
                SCOPE_SELECTOR_ROW_HEIGHT * 3 + SCOPE_SELECTOR_TABLE_VERTICAL_PADDING - 1,
                3,
            ),
            ObjectBrowserWidget::scope_selector_popup_height_for_rows(2)
        );
        assert_eq!(
            ObjectBrowserWidget::scope_selector_popup_height_for_available_height(
                SCOPE_SELECTOR_ROW_HEIGHT * 20,
                3,
            ),
            ObjectBrowserWidget::scope_selector_popup_height_for_rows(3)
        );
    }

    #[test]
    fn scope_selector_max_scroll_row_is_never_negative() {
        assert_eq!(ObjectBrowserWidget::scope_selector_max_scroll_row(3, 10), 0);
        assert_eq!(
            ObjectBrowserWidget::scope_selector_max_scroll_row(20, 6),
            14
        );
    }

    #[test]
    fn scope_selector_navigation_keys_move_by_row_or_page() {
        assert_eq!(
            ObjectBrowserWidget::scope_selector_key_move_delta(Key::Up, 8),
            Some(-1)
        );
        assert_eq!(
            ObjectBrowserWidget::scope_selector_key_move_delta(Key::Down, 8),
            Some(1)
        );
        assert_eq!(
            ObjectBrowserWidget::scope_selector_key_move_delta(Key::PageUp, 8),
            Some(-8)
        );
        assert_eq!(
            ObjectBrowserWidget::scope_selector_key_move_delta(Key::PageDown, 8),
            Some(8)
        );
        assert_eq!(
            ObjectBrowserWidget::scope_selector_key_move_delta(Key::PageDown, 0),
            Some(1)
        );
        assert_eq!(
            ObjectBrowserWidget::scope_selector_key_move_delta(Key::Enter, 8),
            None
        );
    }

    #[test]
    fn scope_selector_popup_position_is_parent_relative() {
        assert_eq!(
            ObjectBrowserWidget::scope_selector_parent_relative_position(100, 200, 135, 260),
            (35, 60)
        );
    }

    #[test]
    fn scope_selector_popup_keeps_requested_vertical_anchor() {
        assert_eq!(
            ObjectBrowserWidget::scope_selector_fit_popup_to_parent(
                100, 200, 300, 400, 360, 550, 260, 180
            ),
            (140, 550, 260, 180)
        );
    }

    #[test]
    fn scope_selector_popup_size_is_capped_to_parent_window() {
        assert_eq!(
            ObjectBrowserWidget::scope_selector_fit_popup_to_parent(
                100, 200, 120, 90, 100, 200, 260, 180
            ),
            (100, 200, 120, 90)
        );
    }

    #[test]
    fn scope_selector_unfocus_hides_when_popup_window_loses_focus() {
        assert!(!ObjectBrowserWidget::scope_selector_should_hide_on_unfocus(
            true, false
        ));
        assert!(ObjectBrowserWidget::scope_selector_should_hide_on_unfocus(
            false, false
        ));
        assert!(ObjectBrowserWidget::scope_selector_should_hide_on_unfocus(
            true, true
        ));
    }

    #[test]
    fn scope_selector_pointer_push_hides_only_outside_popup() {
        assert!(!ObjectBrowserWidget::scope_selector_should_hide_on_pointer_push(true));
        assert!(ObjectBrowserWidget::scope_selector_should_hide_on_pointer_push(false));
    }

    #[test]
    fn refresh_result_requires_matching_scope_and_connection_generations() {
        assert!(ObjectBrowserWidget::refresh_result_matches_generations(
            7, 7, 12, 12
        ));
        assert!(!ObjectBrowserWidget::refresh_result_matches_generations(
            6, 7, 12, 12
        ));
        assert!(!ObjectBrowserWidget::refresh_result_matches_generations(
            7, 7, 11, 12
        ));
    }

    #[test]
    fn scope_switch_messages_are_dialect_specific() {
        assert_eq!(
            ObjectBrowserWidget::scope_switch_activity_message(DatabaseType::Oracle, "SCOTT"),
            "Switching current schema to SCOTT"
        );
        assert_eq!(
            ObjectBrowserWidget::scope_switch_activity_message(DatabaseType::MySQL, "sales"),
            "Switching database to sales"
        );
        assert_eq!(
            ObjectBrowserWidget::scope_switch_failure_message(
                DatabaseType::Oracle,
                "SCOTT",
                "denied"
            ),
            "Failed to switch current schema to SCOTT: denied"
        );
        assert_eq!(
            ObjectBrowserWidget::scope_switch_failure_message(
                DatabaseType::MySQL,
                "sales",
                "denied"
            ),
            "Failed to switch database to sales: denied"
        );
    }

    #[test]
    fn scope_options_match_rejects_different_scope_sets() {
        let current = vec!["SCOTT".to_string(), "HR".to_string()];
        let reordered = vec!["HR".to_string(), "SCOTT".to_string()];
        let shortened = vec!["SCOTT".to_string()];

        assert!(!ObjectBrowserWidget::scope_options_match_for_db_type(
            DatabaseType::Oracle,
            &current,
            &reordered
        ));
        assert!(!ObjectBrowserWidget::scope_options_match_for_db_type(
            DatabaseType::Oracle,
            &current,
            &shortened
        ));
    }

    #[test]
    fn build_mysql_routine_script_uses_call_and_session_variables() {
        let arguments = vec![
            ProcedureArgument {
                name: Some("p_id".to_string()),
                position: 1,
                sequence: 1,
                data_type: Some("INT".to_string()),
                in_out: Some("IN".to_string()),
                data_length: None,
                data_precision: Some(10),
                data_scale: Some(0),
                type_owner: None,
                type_name: None,
                pls_type: None,
                overload: None,
                default_value: None,
            },
            ProcedureArgument {
                name: Some("p_status".to_string()),
                position: 2,
                sequence: 2,
                data_type: Some("VARCHAR(32)".to_string()),
                in_out: Some("OUT".to_string()),
                data_length: Some(32),
                data_precision: None,
                data_scale: None,
                type_owner: None,
                type_name: None,
                pls_type: None,
                overload: None,
                default_value: None,
            },
        ];

        let sql =
            ObjectBrowserWidget::build_mysql_routine_script("demo_proc", "PROCEDURE", &arguments);

        assert!(sql.contains("CALL `demo_proc`("));
        assert!(sql.contains("0,"));
        assert!(sql.contains("@v_p_status"));
        assert!(sql.contains("SELECT @v_p_status AS `p_status`;"));
        assert!(!sql.contains("FROM dual"));
        assert!(!sql.contains("BEGIN\n"));
    }

    #[test]
    fn build_oracle_function_sys_refcursor_return_uses_bind_without_print() {
        let arguments = vec![
            procedure_argument(None, 0, Some("SYS_REFCURSOR"), "OUT"),
            procedure_argument(Some("p_min_sal"), 1, Some("NUMBER"), "IN"),
        ];

        let sql = ObjectBrowserWidget::build_procedure_script("DEMO_PKG.GET_ROWS", &arguments);

        assert!(sql.starts_with("VAR v_result REFCURSOR\n"));
        assert!(sql.contains("  :v_result := DEMO_PKG.GET_ROWS(\n"));
        assert!(sql.contains("p_min_sal => v_p_min_sal"));
        assert!(!sql.contains("PRINT"));
        assert!(!sql.contains("v_result SYS_REFCURSOR"));
    }

    #[test]
    fn selected_object_reference_parts_trim_sql_punctuation() {
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts(" (SCOTT.EMP); "),
            Some(vec!["SCOTT".to_string(), "EMP".to_string()])
        );
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts("demo_pkg.run_job()"),
            Some(vec!["demo_pkg".to_string(), "run_job".to_string()])
        );
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts("\"MixedCase\""),
            Some(vec!["MixedCase".to_string()])
        );
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts(r#""Demo.Pkg""#),
            Some(vec!["Demo.Pkg".to_string()])
        );
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts(r#""Sales.Ops"."Emp.Table""#),
            Some(vec!["Sales.Ops".to_string(), "Emp.Table".to_string()])
        );
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts("[Sales.Ops].[Emp.Table]"),
            Some(vec!["Sales.Ops".to_string(), "Emp.Table".to_string()])
        );
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts(r#""Emp""Name""#),
            Some(vec!["Emp\"Name".to_string()])
        );
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts("`emp``name`"),
            Some(vec!["emp`name".to_string()])
        );
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts("[Emp]]Name]"),
            Some(vec!["Emp]Name".to_string()])
        );
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts(r#""Bad"Name""#),
            None
        );
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts(r#"Bad"Name"#),
            None
        );
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts("bad`name"),
            None
        );
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts("[bad.name"),
            None
        );
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts("SELECT EMP"),
            None
        );
    }

    #[test]
    fn resolve_sql_selection_uses_simple_table_metadata() {
        let mut data = IntellisenseData::new();
        data.tables = vec!["EMP".to_string()];
        data.rebuild_indices();

        let resolved = ObjectBrowserWidget::resolve_selected_object_context(
            "emp",
            &data,
            None,
            DatabaseType::Oracle,
            Some("SCOTT"),
        )
        .expect("table selection should resolve");

        match resolved.item {
            ObjectItem::Simple {
                object_type,
                object_name,
            } => {
                assert_eq!(object_type, "TABLES");
                assert_eq!(object_name, "EMP");
            }
            _ => panic!("expected simple object"),
        }
        assert!(resolved.selected_scope.is_none());
    }

    #[test]
    fn resolve_sql_selection_uses_metadata_case_for_oracle_object_actions() {
        let mut data = IntellisenseData::new();
        data.tables = vec!["HELP".to_string()];
        data.rebuild_indices();

        let resolved = ObjectBrowserWidget::resolve_selected_object_context(
            "help",
            &data,
            None,
            DatabaseType::Oracle,
            Some("SYSTEM"),
        )
        .expect("lowercase SQL selection should resolve through uppercase metadata");

        let ObjectItem::Simple {
            object_type,
            object_name,
        } = &resolved.item
        else {
            panic!("expected simple object");
        };
        assert_eq!(object_type, "TABLES");
        assert_eq!(object_name, "HELP");

        let selected_scope = ObjectBrowserWidget::scope_for_sql_selection_action(
            &resolved.item,
            resolved.selected_scope.as_deref(),
            &data,
            Some("SYSTEM"),
        );
        let sql = ObjectBrowserWidget::preview_select_sql(
            DatabaseType::Oracle,
            selected_scope.as_deref(),
            object_name,
        );

        assert_eq!(sql, "SELECT * FROM SYSTEM.HELP WHERE ROWNUM <= 100");
    }

    #[test]
    fn resolve_sql_selection_uses_metadata_case_for_other_object_actions() {
        let mut data = IntellisenseData::new();
        data.views = vec!["EMP_VIEW".to_string()];
        data.procedures = vec!["RUN_JOB".to_string()];
        data.functions = vec!["CALC_TOTAL".to_string()];
        data.packages = vec!["DEMO_PKG".to_string()];
        data.indexes = vec!["EMP_PK".to_string()];
        data.rebuild_indices();

        for (selected_text, expected_type, expected_name) in [
            ("emp_view", "VIEWS", "EMP_VIEW"),
            ("run_job", "PROCEDURES", "RUN_JOB"),
            ("calc_total", "FUNCTIONS", "CALC_TOTAL"),
            ("demo_pkg", "PACKAGES", "DEMO_PKG"),
            ("emp_pk", "INDEXES", "EMP_PK"),
        ] {
            let resolved = ObjectBrowserWidget::resolve_selected_object_context(
                selected_text,
                &data,
                None,
                DatabaseType::Oracle,
                Some("SCOTT"),
            )
            .expect("selection should resolve");

            match resolved.item {
                ObjectItem::Simple {
                    object_type,
                    object_name,
                } => {
                    assert_eq!(object_type, expected_type);
                    assert_eq!(object_name, expected_name);
                }
                _ => panic!("expected simple object"),
            }
        }
    }

    #[test]
    fn sql_selection_action_scope_prefers_current_scope_over_intellisense_default() {
        let mut data = IntellisenseData::new();
        data.set_default_qualifier(Some("HR".to_string()));
        data.set_members_for_qualifier_with_kinds(
            "HR",
            vec![("EMP".to_string(), Some(QualifiedMemberKind::Table))],
        );
        let item = ObjectItem::Simple {
            object_type: "TABLES".to_string(),
            object_name: "emp".to_string(),
        };

        let scope =
            ObjectBrowserWidget::scope_for_sql_selection_action(&item, None, &data, Some("SCOTT"));

        assert_eq!(scope.as_deref(), Some("SCOTT"));
    }

    #[test]
    fn sql_selection_action_scope_returns_current_scope_unchanged() {
        let mut data = IntellisenseData::new();
        data.set_default_qualifier(Some("sales".to_string()));
        data.set_members_for_qualifier_with_kinds(
            "sales",
            vec![("orders".to_string(), Some(QualifiedMemberKind::Table))],
        );
        let item = ObjectItem::Simple {
            object_type: "TABLES".to_string(),
            object_name: "orders".to_string(),
        };

        let scope = ObjectBrowserWidget::scope_for_sql_selection_action(
            &item,
            None,
            &data,
            Some("legacy_db"),
        );

        assert_eq!(scope.as_deref(), Some("legacy_db"));
    }

    #[test]
    fn sql_selection_action_scope_keeps_current_scope_when_default_lacks_object() {
        let mut data = IntellisenseData::new();
        data.set_default_qualifier(Some("HR".to_string()));
        data.set_members_for_qualifier_with_kinds(
            "HR",
            vec![("DEPT".to_string(), Some(QualifiedMemberKind::Table))],
        );
        let item = ObjectItem::Simple {
            object_type: "TABLES".to_string(),
            object_name: "emp".to_string(),
        };

        let scope =
            ObjectBrowserWidget::scope_for_sql_selection_action(&item, None, &data, Some("SCOTT"));

        assert_eq!(scope.as_deref(), Some("SCOTT"));
    }

    #[test]
    fn sql_selection_action_scope_applies_current_scope_to_package_routines() {
        let mut data = IntellisenseData::new();
        data.set_default_qualifier(Some("APP".to_string()));
        data.set_members_for_qualifier_with_kinds(
            "APP",
            vec![("DEMO_PKG".to_string(), Some(QualifiedMemberKind::Package))],
        );
        let item = ObjectItem::PackageRoutine {
            package_name: "demo_pkg".to_string(),
            routine_name: "run_job".to_string(),
            routine_type: "PROCEDURE".to_string(),
        };

        let scope = ObjectBrowserWidget::scope_for_sql_selection_action(
            &item,
            None,
            &data,
            Some("OLD_APP"),
        );

        assert_eq!(scope.as_deref(), Some("OLD_APP"));
    }

    #[test]
    fn sql_selection_action_scope_falls_back_to_intellisense_default_when_no_current_scope() {
        let mut data = IntellisenseData::new();
        data.set_default_qualifier(Some("HR".to_string()));
        data.set_members_for_qualifier_with_kinds(
            "HR",
            vec![("EMP".to_string(), Some(QualifiedMemberKind::Table))],
        );
        let item = ObjectItem::Simple {
            object_type: "TABLES".to_string(),
            object_name: "emp".to_string(),
        };

        let scope = ObjectBrowserWidget::scope_for_sql_selection_action(&item, None, &data, None);

        assert_eq!(scope.as_deref(), Some("HR"));
    }

    #[test]
    fn resolve_sql_selection_uses_qualified_schema_metadata() {
        let mut data = IntellisenseData::new();
        data.set_members_for_qualifier_with_kinds(
            "SCOTT",
            vec![("EMP".to_string(), Some(QualifiedMemberKind::Table))],
        );

        let resolved = ObjectBrowserWidget::resolve_selected_object_context(
            "scott.emp",
            &data,
            None,
            DatabaseType::Oracle,
            None,
        )
        .expect("schema-qualified table should resolve");

        match resolved.item {
            ObjectItem::Simple {
                object_type,
                object_name,
            } => {
                assert_eq!(object_type, "TABLES");
                assert_eq!(object_name, "EMP");
            }
            _ => panic!("expected simple object"),
        }
        assert_eq!(resolved.selected_scope.as_deref(), Some("scott"));
    }

    #[test]
    fn resolve_sql_selection_uses_metadata_case_for_qualified_oracle_selection() {
        let mut data = IntellisenseData::new();
        data.users = vec!["SYSTEM".to_string()];
        data.set_members_for_qualifier_with_kinds(
            "SYSTEM",
            vec![("HELP".to_string(), Some(QualifiedMemberKind::Table))],
        );

        let resolved = ObjectBrowserWidget::resolve_selected_object_context(
            "system.help",
            &data,
            None,
            DatabaseType::Oracle,
            None,
        )
        .expect("qualified lowercase SQL selection should resolve through uppercase metadata");

        let ObjectItem::Simple { object_name, .. } = &resolved.item else {
            panic!("expected simple object");
        };
        assert_eq!(object_name, "HELP");
        assert_eq!(resolved.selected_scope.as_deref(), Some("SYSTEM"));

        let sql = ObjectBrowserWidget::preview_select_sql(
            DatabaseType::Oracle,
            resolved.selected_scope.as_deref(),
            object_name,
        );
        assert_eq!(sql, "SELECT * FROM SYSTEM.HELP WHERE ROWNUM <= 100");
    }

    #[test]
    fn resolve_sql_selection_covers_object_browser_object_types() {
        let mut data = IntellisenseData::new();
        data.materialized_views = vec!["MV_SALES".to_string()];
        data.types = vec!["ADDRESS_T".to_string()];
        data.sequences = vec!["ORDER_SEQ".to_string()];
        data.triggers = vec!["EMP_BIU".to_string()];
        data.indexes = vec!["EMP_PK".to_string()];
        data.synonyms = vec!["EMP_SYN".to_string()];
        data.public_synonyms = vec!["PUBLIC_EMP".to_string()];
        data.events = vec!["NIGHTLY_EVENT".to_string()];
        data.rebuild_indices();

        assert_resolves_simple_object(
            "mv_sales",
            &data,
            None,
            DatabaseType::Oracle,
            Some("SCOTT"),
            "MATERIALIZED VIEWS",
            None,
        );
        assert_resolves_simple_object(
            "address_t",
            &data,
            None,
            DatabaseType::Oracle,
            Some("SCOTT"),
            "TYPES",
            None,
        );
        assert_resolves_simple_object(
            "order_seq",
            &data,
            None,
            DatabaseType::Oracle,
            Some("SCOTT"),
            "SEQUENCES",
            None,
        );
        assert_resolves_simple_object(
            "emp_biu",
            &data,
            None,
            DatabaseType::Oracle,
            Some("SCOTT"),
            "TRIGGERS",
            None,
        );
        assert_resolves_simple_object(
            "emp_pk",
            &data,
            None,
            DatabaseType::Oracle,
            Some("SCOTT"),
            "INDEXES",
            None,
        );
        assert_resolves_simple_object(
            "emp_syn",
            &data,
            None,
            DatabaseType::Oracle,
            Some("SCOTT"),
            "SYNONYMS",
            None,
        );
        assert_resolves_simple_object(
            "public_emp",
            &data,
            None,
            DatabaseType::Oracle,
            Some("SCOTT"),
            "SYNONYMS",
            Some("PUBLIC"),
        );
        assert_resolves_simple_object(
            "nightly_event",
            &data,
            None,
            DatabaseType::MySQL,
            Some("app"),
            "EVENTS",
            None,
        );
    }

    #[test]
    fn resolve_sql_selection_uses_qualified_metadata_for_supported_object_types() {
        let mut data = IntellisenseData::new();
        data.set_members_for_qualifier_with_kinds(
            "SCOTT",
            vec![
                (
                    "MV_SALES".to_string(),
                    Some(QualifiedMemberKind::MaterializedView),
                ),
                ("ADDRESS_T".to_string(), Some(QualifiedMemberKind::Type)),
                ("ORDER_SEQ".to_string(), Some(QualifiedMemberKind::Sequence)),
                ("EMP_BIU".to_string(), Some(QualifiedMemberKind::Trigger)),
                ("EMP_PK".to_string(), Some(QualifiedMemberKind::Index)),
                ("EMP_SYN".to_string(), Some(QualifiedMemberKind::Synonym)),
                (
                    "NIGHTLY_EVENT".to_string(),
                    Some(QualifiedMemberKind::Event),
                ),
            ],
        );

        assert_resolves_simple_object(
            "scott.mv_sales",
            &data,
            None,
            DatabaseType::Oracle,
            None,
            "MATERIALIZED VIEWS",
            Some("scott"),
        );
        assert_resolves_simple_object(
            "scott.address_t",
            &data,
            None,
            DatabaseType::Oracle,
            None,
            "TYPES",
            Some("scott"),
        );
        assert_resolves_simple_object(
            "scott.order_seq",
            &data,
            None,
            DatabaseType::Oracle,
            None,
            "SEQUENCES",
            Some("scott"),
        );
        assert_resolves_simple_object(
            "scott.emp_biu",
            &data,
            None,
            DatabaseType::Oracle,
            None,
            "TRIGGERS",
            Some("scott"),
        );
        assert_resolves_simple_object(
            "scott.emp_pk",
            &data,
            None,
            DatabaseType::Oracle,
            None,
            "INDEXES",
            Some("scott"),
        );
        assert_resolves_simple_object(
            "scott.emp_syn",
            &data,
            None,
            DatabaseType::Oracle,
            None,
            "SYNONYMS",
            Some("scott"),
        );
        assert_resolves_simple_object(
            "scott.nightly_event",
            &data,
            None,
            DatabaseType::MySQL,
            None,
            "EVENTS",
            Some("scott"),
        );
    }

    #[test]
    fn resolve_sql_selection_uses_object_browser_cache_for_events_and_package_routines() {
        let data = IntellisenseData::new();
        let mut cache = ObjectCache {
            events: vec!["NIGHTLY_EVENT".to_string()],
            packages: vec!["DEMO_PKG".to_string()],
            ..Default::default()
        };
        cache.package_routines.insert(
            "DEMO_PKG".to_string(),
            vec![PackageRoutine {
                name: "CALC".to_string(),
                routine_type: "FUNCTION".to_string(),
            }],
        );

        assert_resolves_simple_object(
            "nightly_event",
            &data,
            Some(&cache),
            DatabaseType::MySQL,
            Some("app"),
            "EVENTS",
            None,
        );

        let resolved = ObjectBrowserWidget::resolve_selected_object_context(
            "demo_pkg.calc",
            &data,
            Some(&cache),
            DatabaseType::Oracle,
            Some("SCOTT"),
        )
        .expect("cached package routine should resolve");

        match resolved.item {
            ObjectItem::PackageRoutine {
                package_name,
                routine_name,
                routine_type,
            } => {
                assert_eq!(package_name, "DEMO_PKG");
                assert_eq!(routine_name, "CALC");
                assert_eq!(routine_type, "FUNCTION");
            }
            _ => panic!("expected package routine"),
        }
    }

    #[test]
    fn package_routine_cache_does_not_short_match_dotted_literal_package() {
        let mut cache = ObjectCache {
            packages: vec!["PKG".to_string(), "SALES.PKG".to_string()],
            ..Default::default()
        };
        cache.package_routines.insert(
            "SALES.PKG".to_string(),
            vec![PackageRoutine {
                name: "CALC".to_string(),
                routine_type: "FUNCTION".to_string(),
            }],
        );

        assert_eq!(
            ObjectBrowserWidget::cached_package_routine_match(Some(&cache), None, "PKG", "CALC"),
            None
        );
        assert_eq!(
            ObjectBrowserWidget::cached_package_routine_match(
                Some(&cache),
                None,
                "SALES.PKG",
                "CALC"
            ),
            Some(("CALC".to_string(), "FUNCTION".to_string()))
        );
    }

    #[test]
    fn resolve_sql_selection_does_not_reuse_dotted_literal_package_routine_type_for_short_name() {
        let data = IntellisenseData::new();
        let mut cache = ObjectCache {
            packages: vec!["PKG".to_string(), "SALES.PKG".to_string()],
            ..Default::default()
        };
        cache.package_routines.insert(
            "SALES.PKG".to_string(),
            vec![PackageRoutine {
                name: "CALC".to_string(),
                routine_type: "FUNCTION".to_string(),
            }],
        );

        let resolved = ObjectBrowserWidget::resolve_selected_object_context(
            "pkg.calc",
            &data,
            Some(&cache),
            DatabaseType::Oracle,
            None,
        )
        .expect("package routine should still resolve with unknown type");

        match resolved.item {
            ObjectItem::PackageRoutine {
                package_name,
                routine_name,
                routine_type,
            } => {
                assert_eq!(package_name, "PKG");
                assert_eq!(routine_name, "calc");
                assert_eq!(routine_type, "UNKNOWN");
            }
            _ => panic!("expected package routine"),
        }

        let resolved = ObjectBrowserWidget::resolve_selected_object_context(
            r#""SALES.PKG".calc"#,
            &data,
            Some(&cache),
            DatabaseType::Oracle,
            None,
        )
        .expect("quoted dotted package routine should resolve from exact cache entry");

        match resolved.item {
            ObjectItem::PackageRoutine {
                package_name,
                routine_name,
                routine_type,
            } => {
                assert_eq!(package_name, "SALES.PKG");
                assert_eq!(routine_name, "CALC");
                assert_eq!(routine_type, "FUNCTION");
            }
            _ => panic!("expected package routine"),
        }
    }

    #[test]
    fn context_menu_choices_cover_materialized_views_and_unknown_package_routines() {
        let materialized_view = ObjectItem::Simple {
            object_type: "MATERIALIZED VIEWS".to_string(),
            object_name: "MV_SALES".to_string(),
        };
        assert_eq!(
            ObjectBrowserWidget::menu_choices_for_object_item(
                &materialized_view,
                DatabaseType::Oracle
            ),
            Some("Select Data (Top 100)|Generate DDL|Drop...")
        );

        let type_item = ObjectItem::Simple {
            object_type: "TYPES".to_string(),
            object_name: "ADDRESS_T".to_string(),
        };
        assert_eq!(
            ObjectBrowserWidget::menu_choices_for_object_item(&type_item, DatabaseType::Oracle),
            Some("Generate DDL")
        );
        assert_eq!(
            ObjectBrowserWidget::menu_choices_for_object_item(&type_item, DatabaseType::MySQL),
            None
        );

        let index_item = ObjectItem::Simple {
            object_type: "INDEXES".to_string(),
            object_name: "EMP_PK".to_string(),
        };
        assert_eq!(
            ObjectBrowserWidget::menu_choices_for_object_item(&index_item, DatabaseType::Oracle),
            Some("Generate DDL")
        );
        assert_eq!(
            ObjectBrowserWidget::menu_choices_for_object_item(&index_item, DatabaseType::MySQL),
            None
        );

        let unknown_routine = ObjectItem::PackageRoutine {
            package_name: "DEMO_PKG".to_string(),
            routine_name: "DO_WORK".to_string(),
            routine_type: "UNKNOWN".to_string(),
        };
        assert_eq!(
            ObjectBrowserWidget::menu_choices_for_object_item(
                &unknown_routine,
                DatabaseType::Oracle
            ),
            Some("Execute Routine")
        );
    }

    #[test]
    fn apply_package_routine_type_from_routines_updates_unknown_menu_item() {
        let mut item = ObjectItem::PackageRoutine {
            package_name: "DEMO_PKG".to_string(),
            routine_name: "RUN_JOB".to_string(),
            routine_type: "UNKNOWN".to_string(),
        };
        let routines = vec![PackageRoutine {
            name: "RUN_JOB".to_string(),
            routine_type: "PROCEDURE".to_string(),
        }];

        ObjectBrowserWidget::apply_package_routine_type_from_routines(&mut item, &routines);

        match item {
            ObjectItem::PackageRoutine { routine_type, .. } => {
                assert_eq!(routine_type, "PROCEDURE");
            }
            _ => panic!("expected package routine"),
        }
    }

    #[test]
    fn package_routine_type_is_unresolved_until_function_or_procedure_is_known() {
        let mut item = ObjectItem::PackageRoutine {
            package_name: "DEMO_PKG".to_string(),
            routine_name: "RUN_JOB".to_string(),
            routine_type: "UNKNOWN".to_string(),
        };

        assert!(!ObjectBrowserWidget::package_routine_type_is_resolved(
            &item
        ));

        if let ObjectItem::PackageRoutine { routine_type, .. } = &mut item {
            *routine_type = "PROCEDURE".to_string();
        }

        assert!(ObjectBrowserWidget::package_routine_type_is_resolved(&item));
    }

    #[test]
    fn resolve_sql_selection_recognizes_package_routine_metadata() {
        let mut data = IntellisenseData::new();
        data.packages = vec!["DEMO_PKG".to_string()];
        data.set_members_for_qualifier_with_kinds(
            "DEMO_PKG",
            vec![("RUN_JOB".to_string(), Some(QualifiedMemberKind::Procedure))],
        );
        data.rebuild_indices();

        let resolved = ObjectBrowserWidget::resolve_selected_object_context(
            "demo_pkg.run_job",
            &data,
            None,
            DatabaseType::Oracle,
            Some("SCOTT"),
        )
        .expect("package routine should resolve");

        match resolved.item {
            ObjectItem::PackageRoutine {
                package_name,
                routine_name,
                routine_type,
            } => {
                assert_eq!(package_name, "DEMO_PKG");
                assert_eq!(routine_name, "RUN_JOB");
                assert_eq!(routine_type, "PROCEDURE");
            }
            _ => panic!("expected package routine"),
        }
        assert!(resolved.selected_scope.is_none());
    }

    #[test]
    fn resolve_sql_selection_recognizes_owner_qualified_package_function() {
        let mut data = IntellisenseData::new();
        data.users = vec!["SCOTT".to_string()];
        data.set_members_for_qualifier_with_kinds(
            "SCOTT",
            vec![("DEMO_PKG".to_string(), Some(QualifiedMemberKind::Package))],
        );
        data.set_members_for_qualifier_with_kinds(
            "SCOTT.DEMO_PKG",
            vec![("CALC".to_string(), Some(QualifiedMemberKind::Function))],
        );

        let resolved = ObjectBrowserWidget::resolve_selected_object_context(
            "scott.demo_pkg.calc",
            &data,
            None,
            DatabaseType::Oracle,
            None,
        )
        .expect("owner-qualified package function should resolve");

        match resolved.item {
            ObjectItem::PackageRoutine {
                package_name,
                routine_name,
                routine_type,
            } => {
                assert_eq!(package_name, "DEMO_PKG");
                assert_eq!(routine_name, "CALC");
                assert_eq!(routine_type, "FUNCTION");
            }
            _ => panic!("expected package routine"),
        }
        assert_eq!(resolved.selected_scope.as_deref(), Some("SCOTT"));
    }

    #[test]
    fn owner_qualified_package_routine_does_not_use_unqualified_cache_type() {
        let mut data = IntellisenseData::new();
        data.set_members_for_qualifier_with_kinds(
            "SCOTT",
            vec![("DEMO_PKG".to_string(), Some(QualifiedMemberKind::Package))],
        );

        let mut cache = ObjectCache {
            packages: vec!["DEMO_PKG".to_string()],
            ..Default::default()
        };
        cache.package_routines.insert(
            "DEMO_PKG".to_string(),
            vec![PackageRoutine {
                name: "CALC".to_string(),
                routine_type: "FUNCTION".to_string(),
            }],
        );

        let resolved = ObjectBrowserWidget::resolve_selected_object_context(
            "scott.demo_pkg.calc",
            &data,
            Some(&cache),
            DatabaseType::Oracle,
            None,
        )
        .expect("owner-qualified package routine should resolve");

        match resolved.item {
            ObjectItem::PackageRoutine {
                package_name,
                routine_name,
                routine_type,
            } => {
                assert_eq!(package_name, "DEMO_PKG");
                assert_eq!(routine_name, "calc");
                assert_eq!(routine_type, "UNKNOWN");
            }
            _ => panic!("expected package routine"),
        }
        assert_eq!(resolved.selected_scope.as_deref(), Some("scott"));
    }

    #[test]
    fn mysql_root_categories_hide_oracle_only_groups_and_keep_events() {
        let categories = ObjectBrowserWidget::root_categories_for_db_type(
            DatabaseType::MySQL,
            &Default::default(),
        );

        assert!(categories.contains(&"Tables"));
        assert!(categories.contains(&"Views"));
        assert!(categories.contains(&"Procedures"));
        assert!(categories.contains(&"Functions"));
        assert!(categories.contains(&"Triggers"));
        assert!(categories.contains(&"Events"));
        assert!(!categories.contains(&"Synonyms"));
        assert!(!categories.contains(&"Packages"));
    }
}
