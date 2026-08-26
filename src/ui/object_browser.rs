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

#[derive(Clone, Debug, PartialEq, Eq)]
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
    outcome: RoutineScriptOutcome,
}

/// One routine-script load's answer, and the backend it ran on.
///
/// The backend travels WITH the answer because the delivery rule's
/// could-not-ask road writes a fallback call script in a FAMILY's syntax, and
/// the family is a property of the session the load actually used — not of the
/// widget's snapshot, which a reconnect can leave behind. It is one value
/// because the two arms of this one action used to answer it two ways.
struct RoutineScriptLoad {
    qualified_name: String,
    /// The kind the load RESOLVED — an `UNKNOWN` package member arrives here
    /// as the kind the server's own listing gave it.
    routine_type: String,
    db_type: crate::db::DatabaseType,
    result: RoutineScriptLoadResult,
}

/// Why a routine-script load ended, in the three ways it can.
///
/// The two failures are NOT the same failure. A load that FAILED leaves the app
/// knowing nothing about the routine, so the long-standing simple-call fallback
/// still gives the user something to edit. A load that was STOPPED — cancelled
/// from the activity view, or its cancel timeout fired — leaves the app knowing
/// nothing EITHER, but it was asked to stop: writing a parameterless call for a
/// routine that takes three arguments is acting after being told not to, and it
/// is the very script [`RoutineScriptOutcome::Refused`]'s gate exists to
/// prevent.
///
/// They used to arrive as one `Result<_, String>`, so the delivery point could
/// only treat them alike — and every stop ended with an alert AND a wrong tab.
/// The scope-race message even reads "Retry the action" while the tab it opened
/// says the routine takes nothing.
#[derive(Debug, PartialEq, Eq)]
enum RoutineScriptLoadResult {
    /// The catalog answered: a script, or a refusal.
    Answered(RoutineScriptOutcome),
    /// The app could not ASK — a driver, a session, a connection that went
    /// away, or a worker that panicked.
    Failed(String),
    /// The work was stopped before the catalog answered.
    Stopped(String),
}

impl RoutineScriptLoadResult {
    /// The ONE place a load's failure is read as one of the two things it can
    /// be, so no caller can decide it for itself — or forget to.
    ///
    /// The question is asked of
    /// [`crate::db::session_policy::FailedReadDisposition`], the same
    /// DB-agnostic reader the db layer's own fail-open roads use
    /// (`RoutineDictionaryRead::failed`, `RoutinePresence::failed`), so all four
    /// backends answer it the same way and a new cancel marker is added in one
    /// place. Every producer of this value goes through here: the loader, and
    /// the worker's panic road.
    fn of(result: Result<RoutineScriptOutcome, String>) -> Self {
        use crate::db::session_policy::FailedReadDisposition;
        match result {
            Ok(outcome) => Self::Answered(outcome),
            Err(err) => match FailedReadDisposition::of(&err) {
                FailedReadDisposition::Stop => Self::Stopped(err),
                FailedReadDisposition::FailOpen => Self::Failed(err),
            },
        }
    }
}

/// What a routine-script load produced.
///
/// The two are not the same failure and must not be delivered the same way.
/// An `Err` around this value means the app could not ASK — a session, a
/// driver, a connection that went away — and it still knows nothing about the
/// routine, so the long-standing simple-call fallback gives the user something
/// to edit. [`Self::Refused`] is the catalog's own ANSWER, and a call script is
/// precisely the thing that answer rules out.
///
/// They used to arrive as one `Result<String, String>`, so the delivery point
/// could only treat them alike: it alerted, and then opened the parameterless
/// script anyway — for a routine the catalog had just said takes two
/// arguments, or is not there at all.
#[derive(Debug, PartialEq, Eq)]
enum RoutineScriptOutcome {
    /// The script to open in a new tab.
    Script(String),
    /// Nothing is opened; the sentence is what the user is told instead.
    ///
    /// Two answers reach here and both are the catalog's:
    /// [`crate::db::query::RoutineDefinitionLookup::Unreadable`] — it would
    /// not describe the routine's arguments — and a routine it described in
    /// full whose invocation form no generated script can write
    /// ([`OracleRoutineScript::Unwritable`]). The variant is named for what it
    /// DOES rather than for one of its causes, because naming it after the
    /// first cause is what would send the second down the `Err` road.
    Refused(String),
}

/// One identifier out of a selected object reference, with the fact a bare
/// `String` loses: whether the selection wrote it QUOTED.
///
/// `SCOTT.EMP` and `SCOTT."EMP"` name the same object, but `pkg.myProc` and
/// `pkg."myProc"` do not — Oracle's parser folds the first to `PKG.MYPROC`
/// while the second is a name only a quoted declaration can create. Stripping
/// the quotes and passing the raw text on answers neither question: it makes
/// the two indistinguishable, so every lookup downstream has to guess, and one
/// of them (the package-member reader) has to pick between two real routines.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectedObjectPart {
    /// The identifier's text, quotes removed.
    text: String,
    /// Whether the selection wrote it inside `"…"`, `` `…` `` or `[…]`.
    quoted: bool,
}

impl SelectedObjectPart {
    /// The name this part DENOTES on `db_type` — what every lookup has to be
    /// given.
    ///
    /// A quoted part denotes its text exactly. A bare one denotes whatever the
    /// server's parser folds it to, which each backend answers for itself
    /// ([`ObjectBrowserDbBehavior::denoted_bare_identifier`]).
    ///
    /// Inert for every case-insensitive matcher this feeds, which is all of
    /// them but one: the package-member reader, which needs an EXACT spelling
    /// precisely because a package may hold `MYPROC` and `"myProc"` at once.
    fn denoted_name(&self, db_type: crate::db::DatabaseType) -> String {
        match self.quoted {
            true => self.text.clone(),
            false => object_browser_behavior_for(db_type).denoted_bare_identifier(&self.text),
        }
    }
}

/// What the object browser does with a routine-script load's answer.
#[derive(Debug, PartialEq, Eq)]
struct RoutineScriptDelivery {
    /// What the user is told, if anything.
    alert: Option<String>,
    /// What is opened in a new query tab, if anything.
    open_sql: Option<String>,
    /// What the status line reads once the action is over.
    ///
    /// NOT optional, because "say nothing" is the answer that was wrong: the
    /// action announces `Loading … arguments for X` when it starts and nothing
    /// when it ends, so every road that ends WITHOUT opening a tab — the
    /// catalog's refusal, a routine no script can call, a stopped load — left
    /// the status line claiming a load was still running, with no later event
    /// to correct it (the label has no timer and 17 unrelated writers). A
    /// mandatory field is what makes that unrepresentable: a road added later
    /// cannot end in silence without the compiler asking what it says.
    status: String,
}

/// What a routine does with ONE of its arguments.
///
/// The two facts every `Execute Procedure`/`Execute Function` script is built
/// from: a value the routine READS has to be given a starting value, and a
/// value the routine WRITES has to end up somewhere the user can see. Asking
/// them here — once, for every backend — is what keeps the Oracle and MySQL
/// builders from answering the same question differently.
///
/// The catalogs spell the direction three ways: Oracle's
/// `ALL_ARGUMENTS.IN_OUT` says `IN` / `OUT` / `IN/OUT`, the MySQL family's
/// `PARAMETERS.PARAMETER_MODE` says `IN` / `OUT` / `INOUT`, and a function's
/// return row says `OUT` (Oracle) or `RETURN` (MySQL family).
#[derive(Clone, Copy)]
struct RoutineArgumentDirection {
    reads: bool,
    writes: bool,
}

impl RoutineArgumentDirection {
    fn of(arg: &ProcedureArgument) -> Self {
        let spelling = arg
            .in_out
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("IN")
            .to_ascii_uppercase();
        // `RETURN` contains neither `IN` nor `OUT`, so the substring test
        // below would call a return row an argument the routine neither reads
        // nor writes.
        if spelling.contains("RETURN") {
            return Self {
                reads: false,
                writes: true,
            };
        }
        Self {
            reads: spelling.contains("IN"),
            writes: spelling.contains("OUT"),
        }
    }
}

/// Where a generated Oracle script keeps ONE argument's value.
///
/// A local variable stops existing when the block ends, so a value the
/// routine WROTE into one is lost; a bind is reported back with the result
/// (`| OUT: :v_x = ...`) and a cursor bind opens its own grid. The choice is
/// therefore made from two facts and nothing else — does the routine write
/// the value, and can a bind carry the type — rather than one branch per
/// argument shape, which is how OUT parameters came to be dropped into
/// locals while the return value and OUT ref cursors were bound.
enum OracleValueCarrier {
    /// `VAR name <type>`: visible after the block runs.
    Bind(String),
    /// A `DECLARE` local — the only option for types no bind can carry
    /// (records, collections, object types, `BOOLEAN`, `INTERVAL`, ...).
    Local,
}

/// The statement a generated Oracle script for ONE overload is written as.
///
/// Decided from what the ROUTINE IS — the dictionary's
/// [`crate::db::query::RoutineInvocation`] — never from what its argument list
/// looks like. An `AGGREGATE` function over `NUMBER` reads as
/// `NUMBER f(NUMBER)` in `ALL_ARGUMENTS`, and a SQL macro reads as an ordinary
/// `VARCHAR2` function, so asking the argument list produced a script that
/// could never run in the first case and — worse — one that RUNS and reports
/// the macro's own source text as the routine's value in the second.
///
/// Every invocation form maps onto a shape HERE, in one match, so a form added
/// to the dictionary reader cannot quietly inherit another one's statement by
/// falling into a wildcard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OracleRoutineScript {
    /// The form this app has always written: `BEGIN ... END;`, with
    /// declarations and binds for whatever the routine reads and writes.
    PlSqlBlock,
    /// A query — the only scope that can reach this routine at all.
    Sql(OracleSqlScopeShape),
    /// No script exists. The routine is reachable only with something a
    /// generated argument list cannot supply, so the action says so and opens
    /// nothing; `reason` is the half of the sentence that names why AND what
    /// the user can do instead — both belong to the FORM, so a form added later
    /// cannot inherit another one's remedy.
    Unwritable { reason: &'static str },
}

impl OracleRoutineScript {
    /// The statement shape for the overload the script is being built for.
    ///
    /// `PIPELINED` and `SQL_MACRO(TABLE)` share the `TABLE(...)` shape and
    /// `AGGREGATE` and `SQL_MACRO(SCALAR)` share the select-list shape — all
    /// four live-proven on 23ai/26ai, including the parameterless forms, where
    /// Oracle's "no parentheses on an empty argument list" rule is what makes
    /// `TABLE(f)` the spelling that works for a table macro: the bare name
    /// straight in a `FROM` clause is rejected outright. (The test module
    /// names the exact server errors; this file's production text stays out of
    /// the driver-marker catalogs' way.)
    fn of(overloads: &[crate::db::query::RoutineOverload], overload: Option<i32>) -> Self {
        use crate::db::query::RoutineInvocation;
        match crate::db::query::RoutineOverload::invocation_of(overloads, overload) {
            RoutineInvocation::Ordinary => Self::PlSqlBlock,
            RoutineInvocation::Pipelined | RoutineInvocation::TableMacro => {
                Self::Sql(OracleSqlScopeShape::PipelinedTable)
            }
            RoutineInvocation::Aggregate | RoutineInvocation::ScalarMacro => {
                Self::Sql(OracleSqlScopeShape::Aggregate)
            }
            RoutineInvocation::Polymorphic => Self::Unwritable {
                // The REMEDY belongs to the form, not to the shared sentence:
                // "write the call by hand against the table it reads" is advice
                // only a polymorphic table function can act on, and a second
                // form reaching here would have inherited it from
                // `routine_call_not_writable` and told the user something
                // untrue. The text the user sees is unchanged.
                reason: "it is a polymorphic table function, so it can only be called with a \
                         table argument. Write the call by hand against the table it reads.",
            },
        }
    }
}

/// Whether ANOTHER menu answers the right-click when this object has no action
/// to offer.
///
/// The two callers differ, and folding them made one of them worse. A tree node
/// is a dead end: nothing else opens, so entries filtered away leave the click
/// answered by silence and the user cannot tell "this node has no actions" from
/// "this connection will not run them". An editor selection is not: the editor's
/// own context menu opens whenever the object menu declines, and that is a
/// better answer than a menu holding one entry — so there the object menu keeps
/// declining, exactly as it always has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ObjectMenuFallback {
    /// Nothing else will answer this click.
    None,
    /// The caller opens its own menu when this one declines.
    CallerMenu,
}

/// Why an object's context menu has no action to offer.
///
/// Both roads end in the same menu — the reason, inactive, above `Copy Name` —
/// and the reason is a VALUE so the two cannot drift into two wordings for one
/// situation, and a third road has to name which of them it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ObjectMenuRefusal {
    /// The server was asked which kind of routine a package member is and could
    /// not settle it, so no `Execute` label applies.
    RoutineTypeUnavailable,
    /// Every entry this item has would send something the database must write,
    /// and this connection refuses writes.
    WritesRefused,
}

impl ObjectMenuRefusal {
    fn label(self) -> &'static str {
        match self {
            Self::RoutineTypeUnavailable => "Package routine type unavailable",
            Self::WritesRefused => "Read only connection: no action available",
        }
    }
}

/// One of the two stored-routine groups a selected name can land in.
///
/// A named value rather than the `"PROCEDURES"` / `"FUNCTIONS"` string literals
/// the selection resolvers scan, because the ORDER of these two is now a
/// per-backend answer ([`ObjectBrowserWidget::routine_selection_order`]) and
/// every mapping off it — the catalog label, the intellisense list, the
/// qualified-member kind — has to be TOTAL. Read off a string, each of those
/// mappings needs a wildcard, and a wildcard is what would quietly send one
/// group's name to the other group's list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RoutineSelectionGroup {
    Procedures,
    Functions,
}

impl RoutineSelectionGroup {
    /// The `ObjectItem::Simple` object type, which is also the key
    /// [`ObjectBrowserWidget::cache_name_match`] reads.
    fn object_type(self) -> &'static str {
        match self {
            Self::Procedures => "PROCEDURES",
            Self::Functions => "FUNCTIONS",
        }
    }

    fn qualified_member_kind(self) -> QualifiedMemberKind {
        match self {
            Self::Procedures => QualifiedMemberKind::Procedure,
            Self::Functions => QualifiedMemberKind::Function,
        }
    }

    fn names(self, data: &IntellisenseData) -> &[String] {
        match self {
            Self::Procedures => &data.procedures,
            Self::Functions => &data.functions,
        }
    }
}

/// The two statement shapes a routine only SQL can call is written with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OracleSqlScopeShape {
    /// `SELECT * FROM TABLE(f(...))` — a `PIPELINED` function's rows.
    PipelinedTable,
    /// `SELECT f(...) FROM dual` — an `AGGREGATE` function's value.
    Aggregate,
}

/// The longest variable name a generated Oracle script may declare.
///
/// 128 bytes is the identifier limit of every Oracle release this app can
/// reach — the thin driver refuses to negotiate below protocol 314. (A
/// pre-12.2 server caps identifiers at 30, where a parameter name over 28
/// characters would still overflow; no supported configuration reaches one.)
const ORACLE_GENERATED_NAME_MAX: usize = 128;

/// The longest variable name a generated MySQL/MariaDB script may use.
///
/// User variables are capped at 64 characters — the same cap as the parameter
/// identifiers they are built from, which is exactly why the prefix has to be
/// budgeted for: `@v_` plus a 64-character parameter name is a name the server
/// refuses outright.
const MYSQL_GENERATED_NAME_MAX: usize = 64;

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
    /// The name a BARE identifier denotes on this backend.
    ///
    /// Oracle's parser folds an unquoted name to upper case, so `emp` names
    /// `EMP` and only a quoted declaration can create `emp`. The MySQL family
    /// folds nothing, so `emp` names `emp` and `Emp` really can be a second
    /// object. Asked of the backend rather than tested in place because the
    /// answer is a property OF the backend — a new one has to state its own.
    fn denoted_bare_identifier(&self, identifier: &str) -> String;
    /// Whether stored PROCEDURES and FUNCTIONS live in SEPARATE namespaces
    /// here, so one name can legitimately be both.
    ///
    /// Oracle keeps procedures, functions, packages and types in ONE namespace
    /// — `CREATE FUNCTION calc` beside `CREATE PROCEDURE calc` is refused — so
    /// at most one of the two lists can hold a given name and the order they
    /// are consulted in cannot change an answer. The MySQL family keeps TWO,
    /// where that pair is ordinary, so a bare `calc` in editor text really does
    /// name two objects.
    ///
    /// A backend that says yes gets the FUNCTION, which is not a new
    /// preference: it is the one this app already gives the signature popup and
    /// the bind prompt for the same ambiguity
    /// (`MysqlObjectBrowser::get_routine_arguments_in_schema_any_kind`,
    /// `discovered_kind_for_routine`), because a name written in an expression
    /// is a function call far more often than a `CALL` target. Answering it
    /// here rather than by the order of a candidate array is what stops the two
    /// halves of this app from disagreeing about one question.
    fn routine_namespaces_can_collide(&self) -> bool;
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
    /// The script for a routine whose argument list is empty or could not be
    /// read. Takes the KIND because the procedure and function shapes are not
    /// interchangeable on any backend, and each family spells the empty
    /// argument list differently (Oracle takes no parentheses, the MySQL
    /// family requires them) — one entry point so the kind cannot be
    /// forgotten at a call site.
    fn build_simple_routine_script(&self, qualified_name: &str, routine_type: &str) -> String;
    /// The script for a routine whose definition was READ.
    ///
    /// Takes the whole [`RoutineDefinition`] rather than its argument list: the
    /// statement shape depends on facts an argument row cannot carry, and a
    /// builder that could be handed arguments alone is one that can be made to
    /// write a shape the routine does not support.
    ///
    /// Returns an OUTCOME rather than a `String` because "the definition was
    /// read and there is still no script to write" is a real answer — Oracle's
    /// polymorphic table functions are invoked with a TABLE, which no
    /// generated argument list can supply. A builder that had to return a
    /// `String` could only invent one, and the block it used to invent
    /// "succeeded" while doing nothing the user asked for.
    fn build_routine_script(
        &self,
        qualified_name: &str,
        routine_type: &str,
        definition: &crate::db::query::RoutineDefinition,
    ) -> RoutineScriptOutcome;
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
    /// The package-member half of `Execute Procedure`/`Execute Function`.
    ///
    /// Acquires its own session (an `UNKNOWN` kind has to be resolved against
    /// the server's package listing before the member's arguments can even be
    /// asked for), so it is the only one that can say which backend the work
    /// ran on — hence `load_db_type`, which it overwrites once a session is in
    /// hand and leaves untouched when none was ever acquired.
    fn load_package_routine_script(
        &self,
        connection: &SharedConnection,
        activity: String,
        selected_scope: Option<&str>,
        package_name: &str,
        routine_name: &str,
        routine_type: &str,
        load_db_type: &mut crate::db::DatabaseType,
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
    /// Carries the whole load rather than its parts: the answer and the
    /// backend it ran on are one fact, and splitting them is what let one
    /// producer ship a stale backend beside a fresh answer.
    RoutineScript(RoutineScriptLoad),
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
                        ObjectActionResult::RoutineScript(load) => {
                            // DESTRUCTURED on purpose: every part of the
                            // delivery has to be acted on, and a part dropped
                            // here is what left the status line claiming a load
                            // was still running. Read field by field, dropping
                            // one is an unused binding — a warning this build
                            // denies — instead of a silence nothing notices.
                            let RoutineScriptDelivery {
                                alert,
                                open_sql,
                                status,
                            } = ObjectBrowserWidget::routine_script_delivery(
                                load.db_type,
                                &load.qualified_name,
                                &load.routine_type,
                                load.result,
                            );
                            // FIRST, and unconditionally: the action announced
                            // `Loading … arguments for X` when it started, and
                            // the status line has no timer and no other writer
                            // that would ever correct it. Emitting before the
                            // alert also means the line is already true behind
                            // the modal the user is about to dismiss.
                            ObjectBrowserWidget::emit_status_callback(&status_callback, &status);
                            if let Some(alert) = alert {
                                crate::ui::alert_on_main(&alert);
                            }
                            if let Some(sql) = open_sql {
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
                                    // Both halves say so. This road announces
                                    // `Resolving package routine type for X`
                                    // before it starts, and the RESOLVED half
                                    // used to say nothing back — so a user who
                                    // dismissed the menu without picking an
                                    // entry was left with a status line
                                    // claiming the lookup was still running.
                                    // (The status label has no timer; see
                                    // `RoutineScriptDelivery::status`.)
                                    ObjectBrowserWidget::emit_status_callback(
                                        &status_callback,
                                        &ObjectBrowserWidget::package_routine_resolution_status(
                                            &item, show_menu,
                                        ),
                                    );
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
                                    // No fallback HERE, unlike the synchronous
                                    // editor road this began on: deferring is
                                    // what made the editor decline to open its
                                    // own menu, and that click is long over by
                                    // the time this answer arrives.
                                    ObjectMenuFallback::None,
                                    mouse_x,
                                    mouse_y,
                                );
                            } else {
                                let _ = ObjectBrowserWidget::show_object_menu_refusal_at(
                                    &item,
                                    &status_callback,
                                    db_type,
                                    selected_scope.as_deref(),
                                    ObjectMenuRefusal::RoutineTypeUnavailable,
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
            let result = Self::run_object_action_work("Generate DDL", || {
                ObjectBrowserWidget::with_pooled_object_session(
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
                )
            });
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
            let result = Self::run_object_action_work("Load package routines", || {
                object_browser_behavior_for(db_type).load_package_routines(
                    &connection,
                    activity,
                    scope.as_deref(),
                    &package_name,
                )
            });

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
            let result = Self::run_object_action_work("Load table columns", || {
                ObjectBrowserWidget::with_pooled_object_session(
                    &connection,
                    scope.as_deref(),
                    activity,
                    |context, session| {
                        object_browser_behavior_for(context.connection_info.db_type)
                            .load_table_structure(context, session, scope.as_deref(), &table_name)
                    },
                )
            });
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

    /// One MySQL-family catalog name as a SEGMENT of a dotted path.
    ///
    /// Bare unless writing it bare would let
    /// [`Self::quote_mysql_identifier_path`] read one name as two. That
    /// splitter separates on an unquoted `.` and tracks backticks, so those
    /// are the two characters a segment cannot carry unquoted — and both are
    /// legal inside a MySQL/MariaDB identifier. Everything else is returned
    /// untouched, which is what keeps every ordinary qualified name (and every
    /// status message and completion text built from one) exactly as it was.
    fn quote_mysql_path_segment(name: &str) -> String {
        let name = name.trim();
        match name.contains('.') || name.contains('`') {
            true => format!("`{}`", name.replace('`', "``")),
            false => name.to_string(),
        }
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

    /// `scope.object`, each part written the way Oracle has to read it.
    ///
    /// `object_name` is ONE object's name as the catalog spells it — the tree
    /// and every selection resolver hand bare names — so a `.` inside it is
    /// part of the name (a quoted-created `"MY.PROC"`, live-proven callable as
    /// `SYSTEM."MY.PROC"`) and not a qualifier that is already there. Treating
    /// it as one used to skip quoting entirely, which named a DIFFERENT object:
    /// `MY.PROC` reads as schema `MY`, and the argument lookup — whose splitter
    /// IS quote-aware — went looking in that schema too.
    fn qualify_oracle_object_name(selected_scope: Option<&str>, object_name: &str) -> String {
        let object_name = object_name.trim();
        if object_name.is_empty() {
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

    /// One MySQL/MariaDB column alias, quoted.
    ///
    /// Doubling the backticks is the whole of the job. This used to strip
    /// leading and trailing backticks first, which is a different question —
    /// "is this alias already quoted?" — and the callers never ask it: the
    /// alias is a routine PARAMETER NAME, straight from the catalog and never
    /// quoted. A parameter name may legally start or end with a backtick
    /// (`` `b ``, doubled at creation), and stripping it made the read-back
    /// column claim to be a DIFFERENT parameter — the one thing the alias
    /// exists to say. Ordinary names come back byte-identical either way.
    fn quote_mysql_alias(alias: &str) -> String {
        format!("`{}`", alias.trim().replace('`', "``"))
    }

    /// The Oracle script for a routine whose argument list is empty or could
    /// not be read.
    ///
    /// The routine KIND picks the shape and the two are not interchangeable:
    /// a function called as a statement does not resolve as a procedure, and
    /// a procedure has no value to select. Oracle also takes NO parentheses
    /// on an empty argument list — `SELECT f() FROM dual` is a syntax error —
    /// which is the exact opposite of the MySQL family, where they are
    /// required. (The test module names the exact server errors; this file's
    /// production text stays out of the driver-marker catalogs' way.)
    fn build_simple_oracle_routine_script(qualified_name: &str, routine_type: &str) -> String {
        if routine_type.eq_ignore_ascii_case("FUNCTION") {
            format!("SELECT {} AS result\nFROM dual;\n", qualified_name)
        } else {
            format!("BEGIN\n  {};\nEND;\n/\n", qualified_name)
        }
    }

    /// The routine-call script `Execute Procedure`/`Execute Function` opens,
    /// built from an already-fetched definition — the exact per-backend builder
    /// the context menu uses. `#[doc(hidden)]`, for the live verification
    /// harness (`verify_proc_exec_live`), which reads definitions through the
    /// same db-layer entry points the browser uses and asserts the generated
    /// script really runs on every backend.
    ///
    /// `Err` is the builder's REFUSAL sentence, not a failure to build: a
    /// definition can be read in full and still name a routine no generated
    /// script can call. The harness sees the same two answers the delivery
    /// rule does, so a case that must refuse cannot pass by producing text.
    #[doc(hidden)]
    pub fn routine_script_for_harness(
        db_type: crate::db::DatabaseType,
        qualified_name: &str,
        routine_type: &str,
        definition: &crate::db::query::RoutineDefinition,
    ) -> Result<String, String> {
        match object_browser_behavior_for(db_type).build_routine_script(
            qualified_name,
            routine_type,
            definition,
        ) {
            RoutineScriptOutcome::Script(sql) => Ok(sql),
            RoutineScriptOutcome::Refused(reason) => Err(reason),
        }
    }

    /// The name an object action writes for `scope` + `object_name` on this
    /// backend — the browser's own composition, not a second spelling of it.
    ///
    /// `#[doc(hidden)]`, for the live verification harness
    /// (`verify_proc_exec_live`), which hands the db layer the display name a
    /// refusal has to carry and must not invent its own `schema.name`: that is
    /// the very drift this parameter exists to remove.
    #[doc(hidden)]
    pub fn action_display_name_for_harness(
        db_type: crate::db::DatabaseType,
        selected_scope: Option<&str>,
        object_name: &str,
    ) -> String {
        Self::qualify_object_name_for_scope(db_type, selected_scope, object_name)
    }

    /// Everything `Execute Procedure`/`Execute Function`/`Execute Routine`
    /// loads for ONE item, on a real pooled session.
    ///
    /// The single road every caller takes — both context-menu arms and the
    /// live harness. It used to be three near-copies, which is how the two
    /// menu arms came to answer "which backend did this work run on?"
    /// differently: the standalone one re-read it from the session context,
    /// the package one shipped the widget's snapshot. That fact decides which
    /// family's syntax the could-not-ask road's fallback script is written in,
    /// so it has to be answered once, here, by the code that acquired the
    /// session.
    fn load_routine_script_for_item(
        connection: &SharedConnection,
        db_type: crate::db::DatabaseType,
        selected_scope: Option<&str>,
        item: &ObjectItem,
        routine_type: &str,
        activity: String,
    ) -> RoutineScriptLoad {
        let mut resolved_routine_type = routine_type.to_string();
        // The backend the work ACTUALLY ran on. The caller's snapshot until a
        // session says otherwise, because until one is acquired there is
        // nothing better to go on.
        let mut load_db_type = db_type;
        let (qualified_name, result) = match item {
            ObjectItem::Simple { object_name, .. } => {
                // Scope-qualified UP FRONT, so the simple-script fallback a
                // failed argument load produces still targets the browsed
                // scope. This is the best answer available before a session
                // exists; the closure below replaces it with the action's own
                // as soon as one does, and the load's qualification replaces
                // that on success.
                let mut qualified_name =
                    Self::qualify_object_name_for_scope(db_type, selected_scope, object_name);
                let result = Self::with_pooled_object_session(
                    connection,
                    selected_scope,
                    activity,
                    |context, session| {
                        load_db_type = context.connection_info.db_type;
                        let behavior = object_browser_behavior_for(load_db_type);
                        // Re-answered from the SESSION's own context before any
                        // work is done, so a load that fails half way still
                        // names the object the way the load itself would have.
                        // On the MySQL family that is what fills in a schema the
                        // card never picked.
                        qualified_name = Self::action_object_name(
                            behavior,
                            context,
                            selected_scope,
                            object_name,
                        );
                        let data = behavior.load_routine_script(
                            context,
                            session,
                            selected_scope,
                            object_name,
                            routine_type,
                        )?;
                        // Every fact the load RESOLVED is taken FROM it, on
                        // this road as on the package one. The kind cannot
                        // differ here today — this road is handed the kind the
                        // menu label named — but a road that drops what a
                        // loader returns is one a later loader can be made to
                        // answer into thin air.
                        resolved_routine_type = data.resolved_routine_type;
                        qualified_name = data.qualified_name;
                        Ok(data.outcome)
                    },
                );
                (qualified_name, result)
            }
            ObjectItem::PackageRoutine {
                package_name,
                routine_name,
                ..
            } => {
                let mut qualified_name = Self::qualify_package_member_name(
                    db_type,
                    selected_scope,
                    package_name,
                    routine_name,
                );
                let result = object_browser_behavior_for(db_type)
                    .load_package_routine_script(
                        connection,
                        activity,
                        selected_scope,
                        package_name,
                        routine_name,
                        routine_type,
                        &mut load_db_type,
                    )
                    // Both facts the load RESOLVED are taken from it: an
                    // `UNKNOWN` kind is asked of the package listing, and that
                    // listing answers with the member's own spelling as well
                    // as its kind.
                    .map(|data| {
                        resolved_routine_type = data.resolved_routine_type;
                        qualified_name = data.qualified_name;
                        data.outcome
                    });
                (qualified_name, result)
            }
            // Unreachable through the menu — its Execute arms match only the
            // two shapes above — and answered here rather than left to the
            // delivery rule, whose failed-load road would hand back a call
            // script naming a column. The sentence comes from the shared
            // catalog like every other refusal this action can produce: a road
            // only the harness reaches is still a road the user could be shown
            // if a later menu arm widened, and a hand-written sentence here is
            // the one spelling nobody would keep in step.
            ObjectItem::Column { column_name, .. } => (
                column_name.clone(),
                Ok(RoutineScriptOutcome::Refused(
                    crate::db::query::result_messages::routine_call_not_writable(
                        column_name,
                        "it is a column, not a routine",
                    ),
                )),
            ),
        };
        RoutineScriptLoad {
            qualified_name,
            routine_type: resolved_routine_type,
            db_type: load_db_type,
            // Every road above hands its failure to the ONE reader that tells a
            // load that could not ASK from one that was told to STOP.
            result: RoutineScriptLoadResult::of(result),
        }
    }

    /// The routine kind an Execute menu label asks for.
    ///
    /// `Execute Routine` is the label for an item whose kind nothing has
    /// resolved yet, and `UNKNOWN` is the value that makes the loader ask the
    /// server for it.
    fn execute_label_routine_type(label: &str) -> String {
        match label {
            "Execute Function" => "FUNCTION".to_string(),
            "Execute Procedure" => "PROCEDURE".to_string(),
            _ => "UNKNOWN".to_string(),
        }
    }

    /// The name an Execute action names in what the user reads, scope-qualified
    /// the way the generated script will name it.
    fn routine_action_display_name(
        db_type: crate::db::DatabaseType,
        selected_scope: Option<&str>,
        item: &ObjectItem,
    ) -> String {
        match item {
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
            ObjectItem::Column { column_name, .. } => column_name.clone(),
        }
    }

    /// Run [`Self::load_routine_script_for_item`] on a worker thread and post
    /// its answer to the action loop.
    ///
    /// Every Execute label ends here. The menu arms differ only in the GUARD
    /// that decides which items a label may act on; one road afterwards is what
    /// keeps two arms of one action from answering the same questions
    /// differently — which is exactly how the package arm came to ship the
    /// widget's stale backend snapshot beside a fresh answer.
    fn spawn_routine_script_load(
        connection: &SharedConnection,
        action_sender: &std::sync::mpsc::Sender<ObjectActionResult>,
        status_callback: &StatusCallback,
        db_type: crate::db::DatabaseType,
        selected_scope: Option<String>,
        item: ObjectItem,
        routine_type: String,
    ) {
        let display_name =
            Self::routine_action_display_name(db_type, selected_scope.as_deref(), &item);
        // `Execute Routine` has no kind yet, so the line says what was clicked
        // rather than a kind nobody has answered.
        let status_routine_type = match routine_type.eq_ignore_ascii_case("UNKNOWN") {
            true => "routine",
            false => routine_type.as_str(),
        };
        let activity = format!("Loading {status_routine_type} arguments for {display_name}");
        Self::emit_status_callback(status_callback, &activity);

        let connection = connection.clone();
        let sender = action_sender.clone();
        thread::spawn(move || {
            let loaded = Self::run_object_action_work("Load routine arguments", || {
                Ok(Self::load_routine_script_for_item(
                    &connection,
                    db_type,
                    selected_scope.as_deref(),
                    &item,
                    &routine_type,
                    activity,
                ))
            });
            // A panic leaves the action with nothing loaded, so it takes the
            // could-not-ask road carrying the facts the click already had —
            // through the SAME reader the loader's own failures take, so this
            // road cannot be given a second answer to "was this a stop?".
            let load = loaded.unwrap_or_else(|err| RoutineScriptLoad {
                qualified_name: display_name,
                routine_type,
                db_type,
                result: RoutineScriptLoadResult::of(Err(err)),
            });

            let _ = sender.send(ObjectActionResult::RoutineScript(load));
            app::awake();
        });
    }

    /// What `Execute Procedure`/`Execute Function`/`Execute Routine` does to
    /// one tree item, from the click to what the user is shown: the real
    /// loader on a real pooled session, and the real delivery rule.
    ///
    /// Returns `(qualified name, resolved kind, alert, sql opened)` — an
    /// `alert` with no `sql` is the app refusing, which is the whole point of
    /// the readability gate.
    ///
    /// `#[doc(hidden)]`, for the live verification harness
    /// (`verify_proc_exec_live`). It composes exactly what the context menu's
    /// worker composes — the same loader, then the same delivery rule — which
    /// the menu splits across a thread and the action-result loop, and that
    /// split is why nothing could reach this chain before.
    #[doc(hidden)]
    pub fn routine_script_delivery_for_harness(
        connection: &SharedConnection,
        db_type: crate::db::DatabaseType,
        selected_scope: Option<&str>,
        item: &ObjectItem,
        routine_type: &str,
    ) -> (String, String, Option<String>, Option<String>) {
        let load = Self::load_routine_script_for_item(
            connection,
            db_type,
            selected_scope,
            item,
            routine_type,
            format!("Loading {routine_type} arguments (harness)"),
        );
        let delivery = Self::routine_script_delivery(
            load.db_type,
            &load.qualified_name,
            &load.routine_type,
            load.result,
        );
        (
            load.qualified_name,
            load.routine_type,
            delivery.alert,
            delivery.open_sql,
        )
    }

    fn build_simple_routine_script_for_db(
        db_type: crate::db::DatabaseType,
        qualified_name: &str,
        routine_type: &str,
    ) -> String {
        object_browser_behavior_for(db_type)
            .build_simple_routine_script(qualified_name, routine_type)
    }

    /// What `Execute Procedure`/`Execute Function` does with a load's answer:
    /// what it SAYS, and what it OPENS.
    ///
    /// A value rather than a branch inside the action-result loop, because
    /// this is the rule the whole readability gate exists to enforce and it
    /// was previously unreachable by any test — the loop it lived in needs a
    /// live connection, a worker thread and an FLTK event pump.
    ///
    /// Exactly ONE of the four roads opens a fallback script, and the match
    /// below names all four with no wildcard on purpose: the fallback belongs
    /// to "the app could not ask" alone, and every time another road was let
    /// into it the user got a parameterless call for a routine that takes
    /// arguments ([`RoutineScriptLoadResult`], [`RoutineScriptOutcome`]).
    fn routine_script_delivery(
        db_type: crate::db::DatabaseType,
        qualified_name: &str,
        routine_type: &str,
        result: RoutineScriptLoadResult,
    ) -> RoutineScriptDelivery {
        match result {
            RoutineScriptLoadResult::Answered(RoutineScriptOutcome::Script(sql)) => {
                RoutineScriptDelivery {
                    alert: None,
                    open_sql: Some(sql),
                    status: format!("Opened a call script for {qualified_name}"),
                }
            }
            // The catalog ANSWERED. A parameterless call is the one script
            // that answer rules out, so the user is told and nothing is
            // opened — the same treatment an unresolved kind has always had.
            RoutineScriptLoadResult::Answered(RoutineScriptOutcome::Refused(reason)) => {
                RoutineScriptDelivery {
                    alert: Some(reason),
                    open_sql: None,
                    status: format!("No call script was generated for {qualified_name}"),
                }
            }
            // The work was STOPPED. The app knows nothing about the routine,
            // exactly as on the road below — but it was told to stop, and
            // opening a call script anyway is acting after that. Said out loud
            // rather than left silent, because the stop can be a cancel TIMEOUT
            // the user never asked for and would otherwise see no reason for
            // the missing tab.
            RoutineScriptLoadResult::Stopped(reason) => RoutineScriptDelivery {
                alert: Some(
                    crate::db::query::result_messages::routine_script_load_stopped(
                        qualified_name,
                        &reason,
                    ),
                ),
                open_sql: None,
                // Deliberately NOT "Loading arguments for X was stopped": the
                // status line is one truncatable label, and a terminal line
                // that OPENS with the words the in-progress line opens with
                // reads as still running. The alert says the full sentence.
                status: format!("Stopped loading arguments for {qualified_name}"),
            },
            // The app could not ASK, so it knows nothing about the routine:
            // the simple call script still gives the user something to edit.
            RoutineScriptLoadResult::Failed(err) => RoutineScriptDelivery {
                alert: Some(
                    crate::db::query::result_messages::routine_script_load_failed(
                        qualified_name,
                        &err,
                    ),
                ),
                open_sql: (!routine_type.eq_ignore_ascii_case("UNKNOWN")).then(|| {
                    Self::build_simple_routine_script_for_db(db_type, qualified_name, routine_type)
                }),
                status: format!("Failed to load arguments for {qualified_name}"),
            },
        }
    }

    /// One reading of the catalog's answer, for every backend: a definition
    /// becomes that backend's script, and a refusal is carried through as a
    /// refusal.
    ///
    /// Shared on purpose. The refusal used to be an `Err` each family raised
    /// in its own words, on the same road a lost session takes — so the one
    /// place that decides what to OPEN could not tell "the catalog says this
    /// routine's arguments cannot be read" from "the app could not ask", and
    /// answered both by opening a parameterless call script.
    fn routine_script_outcome(
        behavior: &dyn ObjectBrowserDbBehavior,
        qualified_name: &str,
        routine_type: &str,
        lookup: crate::db::query::RoutineDefinitionLookup,
    ) -> RoutineScriptOutcome {
        match lookup {
            crate::db::query::RoutineDefinitionLookup::Defined(definition) => {
                behavior.build_routine_script(qualified_name, routine_type, &definition)
            }
            crate::db::query::RoutineDefinitionLookup::Unreadable(reason) => {
                RoutineScriptOutcome::Refused(reason)
            }
        }
    }

    /// The listed package member whose name is the one that was asked for.
    ///
    /// EXACT first: `"myProc"` and `MYPROC` are two routines one package may
    /// legally declare, and both answer a case-insensitive test. The
    /// case-insensitive pass is still needed — an `UNKNOWN` kind can arrive
    /// from editor-selected text the caches could not resolve, where the
    /// spelling is the user's — but it only answers when exactly one member
    /// matches, so an ambiguous name is refused rather than settled by the
    /// order the listing happens to be in.
    fn listed_package_routine<'a>(
        routines: &'a [PackageRoutine],
        requested_name: &str,
    ) -> Option<&'a PackageRoutine> {
        Self::identified_by_name(routines, requested_name, |routine| routine.name.as_str())
    }

    /// The member's STORED name and its kind, together — `None` when the
    /// listing does not settle both.
    ///
    /// They are one fact from one row: the listing that says a member is a
    /// FUNCTION is the same row that says how its name is written. Taking the
    /// kind from it while keeping the caller's spelling is what sent
    /// `pkg.myproc` to the dictionary under a name it does not hold — and the
    /// generated call to a name PL/SQL resolves to nothing. Every consumer of
    /// a package listing reads it through here so none of them can take one
    /// half without the other.
    fn listed_package_routine_identity(
        routines: &[PackageRoutine],
        requested_name: &str,
    ) -> Option<(String, String)> {
        Self::listed_package_routine(routines, requested_name).and_then(|routine| {
            Self::normalize_package_routine_type(&routine.routine_type)
                .map(|kind| (routine.name.clone(), kind))
        })
    }

    /// [`Self::listed_package_routine_identity`] for the caller that has to
    /// REFUSE when the listing does not settle it.
    ///
    /// The refusal comes back as [`RoutineScriptOutcome::Refused`] — a
    /// TYPE, not an `Err(String)` — because the listing ANSWERED: no single
    /// member carries that name. `Err` is the could-not-ask road, whose
    /// delivery rule owns a simple-call fallback script; the only thing that
    /// kept this refusal from opening one was the delivery's `UNKNOWN` guard
    /// at the far end, and "the wrong road, saved by a guard somewhere else"
    /// is exactly what the outcome type exists to rule out.
    fn resolve_listed_package_routine(
        routines: &[PackageRoutine],
        requested_name: &str,
        qualified_display_name: &str,
    ) -> Result<(String, String), RoutineScriptOutcome> {
        Self::listed_package_routine_identity(routines, requested_name).ok_or_else(|| {
            RoutineScriptOutcome::Refused(format!(
                "Could not resolve package routine type for {qualified_display_name}"
            ))
        })
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
            "CHAR" | "VARCHAR" | "TEXT" | "TINYTEXT" | "MEDIUMTEXT" | "LONGTEXT" => {
                "''".to_string()
            }
            // Spelled out rather than left to the fallback: these look like
            // string types and are not. MySQL 8 refuses an empty string in
            // all three — ER 3140 for JSON, ER 1265 under STRICT_TRANS_TABLES
            // for an ENUM/SET member that is not literally empty — so the
            // generated call failed on the engine before the user could edit
            // it. MariaDB happens to accept `''` for all three (its JSON is a
            // LONGTEXT alias, and it does not truncate-error here), which is
            // exactly why the placeholder must not be chosen per engine: NULL
            // is what BOTH accept, every routine parameter being nullable.
            "JSON" | "ENUM" | "SET" => "NULL".to_string(),
            _ => "NULL".to_string(),
        }
    }

    /// The MySQL/MariaDB catalog's own spelling of an argument's type.
    ///
    /// `INFORMATION_SCHEMA.PARAMETERS.DTD_IDENTIFIER`, or the `SHOW CREATE`
    /// fallback's parsed type text, verbatim. Deliberately NOT
    /// [`Self::format_argument_type`]: that one reads an ORACLE data
    /// dictionary, and its `CHAR`/`VARCHAR2`/`NUMBER` and composite rules
    /// answered for a catalog this family does not have — `char(3)` came back
    /// out of it as `CHAR(3)(3)`.
    fn mysql_argument_type(arg: &ProcedureArgument) -> String {
        arg.data_type
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .to_string()
    }

    fn build_mysql_routine_script(
        qualified_name: &str,
        routine_type: &str,
        definition: &crate::db::query::RoutineDefinition,
    ) -> String {
        let selected_args = Self::select_overload_arguments(definition, routine_type);
        if selected_args.is_empty() {
            return Self::build_simple_mysql_routine_script(qualified_name, routine_type);
        }

        let target = Self::quote_mysql_identifier_path(qualified_name);
        let mut used_names: HashSet<String> = HashSet::new();
        let mut prelude_lines: Vec<String> = Vec::new();
        let mut call_args: Vec<String> = Vec::new();
        let mut post_lines: Vec<String> = Vec::new();

        for arg in &selected_args {
            // The return value is not a call argument; the caller assigns or
            // selects the call expression itself.
            if Self::is_function_return_row(arg) {
                continue;
            }

            let arg_label = arg
                .name
                .clone()
                .unwrap_or_else(|| format!("arg{}", arg.position.max(1)));
            let direction = RoutineArgumentDirection::of(arg);
            let type_str = Self::mysql_argument_type(arg);

            // Everything the routine WRITES goes through a session variable
            // the trailing SELECT reads back, so the value cannot be lost;
            // IN OUT additionally needs its starting value set first.
            if direction.writes {
                let session_var = format!(
                    "@{}",
                    Self::unique_var_name(
                        &arg_label,
                        arg.position,
                        &mut used_names,
                        MYSQL_GENERATED_NAME_MAX,
                    )
                );
                if direction.reads {
                    prelude_lines.push(format!(
                        "SET {} = {};",
                        session_var,
                        Self::default_value_for_mysql_argument(arg, &type_str)
                    ));
                }
                call_args.push(Self::mysql_call_argument_expr(&arg_label, &session_var));
                post_lines.push(format!(
                    "SELECT {} AS {};",
                    session_var,
                    Self::quote_mysql_alias(&arg_label)
                ));
                continue;
            }

            call_args.push(Self::mysql_call_argument_expr(
                &arg_label,
                &Self::default_value_for_mysql_argument(arg, &type_str),
            ));
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
            let call_expr = if multiline_args.is_empty() {
                format!("{}()", target)
            } else {
                format!("{}{}", target, multiline_args)
            };
            if post_lines.is_empty() {
                script.push_str(&format!("SELECT {} AS result;\n", call_expr));
            } else {
                // A function with OUT/INOUT parameters (MariaDB allows them)
                // cannot be invoked from a SELECT — the server refuses the
                // session-variable argument (ER 4187). SET is the calling
                // shape it accepts, and the trailing SELECTs surface what
                // the call wrote.
                let result_var = format!(
                    "@{}",
                    Self::unique_var_name("result", 0, &mut used_names, MYSQL_GENERATED_NAME_MAX)
                );
                script.push_str(&format!("SET {} = {};\n", result_var, call_expr));
                script.push_str(&format!("SELECT {} AS result;\n", result_var));
                for line in post_lines {
                    script.push_str(&line);
                    script.push('\n');
                }
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

    /// The MySQL/MariaDB script for a routine whose argument list is empty or
    /// could not be read. An empty argument list is written WITH parentheses
    /// on both engines — the opposite of Oracle, where they are a syntax
    /// error.
    fn build_simple_mysql_routine_script(qualified_name: &str, routine_type: &str) -> String {
        let target = Self::quote_mysql_identifier_path(qualified_name);
        if routine_type.eq_ignore_ascii_case("FUNCTION") {
            return format!("SELECT {}() AS result;\n", target);
        }

        format!("CALL {}();\n", target)
    }

    fn build_procedure_script(
        qualified_name: &str,
        routine_type: &str,
        definition: &crate::db::query::RoutineDefinition,
    ) -> RoutineScriptOutcome {
        let selected_args = Self::select_overload_arguments(definition, routine_type);
        if selected_args.is_empty() {
            // No overload number to key the invocation form on — and none is
            // needed. Every form that is not the block belongs to a FUNCTION,
            // and a function always carries its return-value row, so an empty
            // selection is either a parameterless PROCEDURE (which only the
            // block can call) or an argument list that was never read (where
            // the block is the only shape this app can choose).
            return RoutineScriptOutcome::Script(Self::build_simple_oracle_routine_script(
                qualified_name,
                routine_type,
            ));
        }

        // Asked BEFORE anything is written, from the overload actually
        // selected: a PL/SQL block is not a shape every routine supports, and
        // a routine that only SQL can call has no use for the block's
        // declarations, binds or seed lines.
        let shape = OracleRoutineScript::of(
            &definition.overloads,
            selected_args.first().and_then(|arg| arg.overload),
        );
        match shape {
            OracleRoutineScript::PlSqlBlock => {}
            OracleRoutineScript::Sql(shape) => {
                return RoutineScriptOutcome::Script(Self::build_oracle_sql_scope_script(
                    qualified_name,
                    &selected_args,
                    shape,
                ))
            }
            OracleRoutineScript::Unwritable { reason } => {
                return RoutineScriptOutcome::Refused(
                    crate::db::query::result_messages::routine_call_not_writable(
                        qualified_name,
                        reason,
                    ),
                )
            }
        }

        let mut used_names: HashSet<String> = HashSet::new();
        let mut local_decls: Vec<String> = Vec::new();
        let mut call_args: Vec<String> = Vec::new();
        let mut bind_decls: Vec<(String, String)> = Vec::new();
        // A bind starts out empty, so an IN OUT argument carried by one is
        // given its starting value inside the block, before the call.
        let mut seed_lines: Vec<String> = Vec::new();
        // Function return value: assigned via ':=' rather than passed as a
        // call argument.
        let mut return_target: Option<String> = None;

        for arg in &selected_args {
            let direction = RoutineArgumentDirection::of(arg);
            let is_return_value = Self::is_function_return_row(arg);
            let var_base =
                arg.name
                    .as_deref()
                    .unwrap_or(if is_return_value { "RESULT" } else { "ARG" });
            let var_name = Self::unique_var_name(
                var_base,
                arg.position,
                &mut used_names,
                ORACLE_GENERATED_NAME_MAX,
            );
            let type_str = Self::format_argument_type(arg);
            // Everything the routine WRITES — the return value and every
            // OUT/IN OUT parameter alike — goes through a bind whenever a
            // bind can carry the type, because a local would swallow it.
            let carrier = match direction.writes {
                true => Self::bind_type_for_argument(arg, &type_str)
                    .map(OracleValueCarrier::Bind)
                    .unwrap_or(OracleValueCarrier::Local),
                false => OracleValueCarrier::Local,
            };
            // The neutral starting value an IN or IN OUT argument needs, from
            // the one place that knows which types can take one at all.
            let seed = direction
                .reads
                .then(|| Self::in_argument_initializer(arg, &type_str))
                .flatten();

            let target = match carrier {
                OracleValueCarrier::Bind(bind_type) => {
                    bind_decls.push((var_name.clone(), bind_type));
                    if let Some(seed) = seed {
                        seed_lines.push(format!("  :{} := {};", var_name, seed));
                    }
                    format!(":{}", var_name)
                }
                OracleValueCarrier::Local => {
                    match seed {
                        Some(seed) => {
                            local_decls.push(format!("  {} {} := {};", var_name, type_str, seed))
                        }
                        None => local_decls.push(format!("  {} {};", var_name, type_str)),
                    }
                    var_name
                }
            };

            if is_return_value {
                return_target = Some(target);
            } else {
                call_args.push(Self::oracle_call_argument_expr(
                    arg.name.as_deref(),
                    &target,
                ));
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

        for line in &seed_lines {
            script.push_str(line);
            script.push('\n');
        }

        let call_str = Self::oracle_call_expression(qualified_name, &call_args);

        if let Some(ref return_target) = return_target {
            // Function: assign return value via ':='
            script.push_str(&format!("  {} := {};\n", return_target, call_str));
        } else {
            // Procedure: plain call
            script.push_str(&format!("  {};\n", call_str));
        }

        script.push_str("END;\n/\n");

        RoutineScriptOutcome::Script(script)
    }

    /// One Oracle call expression: named association, one argument per line,
    /// or the bare name when there are none — Oracle takes NO parentheses on
    /// an empty argument list.
    ///
    /// Shared by both Oracle shapes so a routine's call reads the same however
    /// the statement around it is written.
    fn oracle_call_expression(qualified_name: &str, call_args: &[String]) -> String {
        if call_args.is_empty() {
            return qualified_name.to_string();
        }
        let mut sql = format!("{}(\n", qualified_name);
        for (idx, arg) in call_args.iter().enumerate() {
            let suffix = if idx + 1 == call_args.len() { "" } else { "," };
            sql.push_str(&format!("    {}{}\n", arg, suffix));
        }
        sql.push_str("  )");
        sql
    }

    /// The script for a routine only SQL can call.
    ///
    /// A `PIPELINED` function produces ROWS and is reachable only through a
    /// query's `FROM` clause; an `AGGREGATE` function is reachable only from a
    /// select list. Both are `PLS-00653` inside a PL/SQL block, which is what
    /// the block builder used to write for them.
    ///
    /// A query has no declarations, so every argument is written as its
    /// neutral literal — the same value [`Self::default_value_for_argument`]
    /// gives a local. That includes an OUT/IN OUT argument, which SQL has no
    /// way to carry at all: writing it keeps the argument list complete, and
    /// the server then refuses the call for the true reason (the function has
    /// OUT arguments) instead of the block's misleading one.
    fn build_oracle_sql_scope_script(
        qualified_name: &str,
        arguments: &[ProcedureArgument],
        shape: OracleSqlScopeShape,
    ) -> String {
        let call_args: Vec<String> = arguments
            .iter()
            .filter(|arg| !Self::is_function_return_row(arg))
            .map(|arg| {
                let type_str = Self::format_argument_type(arg);
                Self::oracle_call_argument_expr(
                    arg.name.as_deref(),
                    &Self::default_value_for_argument(&type_str),
                )
            })
            .collect();
        let call = Self::oracle_call_expression(qualified_name, &call_args);

        // Every variant is named on purpose: a shape added later must not be
        // able to inherit another one's text by falling into a wildcard.
        match shape {
            OracleSqlScopeShape::PipelinedTable => {
                format!("SELECT *\nFROM TABLE(\n  {}\n);\n", call)
            }
            OracleSqlScopeShape::Aggregate => {
                format!("SELECT\n  {} AS result\nFROM dual;\n", call)
            }
        }
    }

    /// The `VAR` type a bind needs to carry this argument, `None` when no
    /// bind can carry it.
    ///
    /// One answer for the function return value and for every OUT/IN OUT
    /// parameter: they are the same question — "can the user be shown what
    /// the routine wrote here?" — and answering it in two places is how the
    /// return value came to be bound while OUT parameters were not.
    fn bind_type_for_argument(arg: &ProcedureArgument, type_str: &str) -> Option<String> {
        if Self::is_ref_cursor(arg) {
            return Some("REFCURSOR".to_string());
        }
        // Records, collections, object types and object references have no
        // bind spelling at all; `composite_argument_type_name` is the single
        // place that knows which types those are.
        if Self::composite_argument_type_name(arg).is_some() {
            return None;
        }

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
            "VARCHAR2" | "NVARCHAR2" | "VARCHAR" | "CHAR" | "NCHAR" => Some(format!(
                "VARCHAR2({})",
                Self::bind_character_capacity(&upper, 1)
            )),
            // A RAW reaches a VARCHAR2 bind through PL/SQL's implicit
            // RAW-to-character conversion, which writes HEX: two characters
            // per declared byte, or the assignment overflows the bind.
            "RAW" => Some(format!(
                "VARCHAR2({})",
                Self::bind_character_capacity(&upper, 2)
            )),
            _ => None,
        }
    }

    /// The character capacity a `VARCHAR2` bind needs for a declared type:
    /// `characters_per_unit` per declared unit, capped at PL/SQL's 32767.
    ///
    /// An unstated length means the type carries its maximum — the same
    /// reading [`Self::clamp_string_length`] gives the declaration itself, so
    /// the bind is never smaller than the variable it has to receive. (The
    /// cap can still be reached by a full-size `RAW`, whose hex form is
    /// simply longer than any `VARCHAR2` can hold.)
    fn bind_character_capacity(upper: &str, characters_per_unit: u32) -> u32 {
        const PLSQL_MAX_LENGTH: u32 = 32767;
        Self::extract_parenthesized_u32(upper)
            .and_then(|declared| declared.checked_mul(characters_per_unit))
            .unwrap_or(PLSQL_MAX_LENGTH)
            .clamp(1, PLSQL_MAX_LENGTH)
    }

    fn extract_parenthesized_u32(value: &str) -> Option<u32> {
        let start = value.find('(')?;
        let end = value[start + 1..].find(')')? + start + 1;
        let inner = value[start + 1..end].trim();
        let head = inner.split(',').next().unwrap_or(inner).trim();
        head.parse::<u32>().ok()
    }

    /// The overload group Execute should call: the FIRST one whose SHAPE
    /// agrees with the menu's routine kind. A package may legally overload
    /// one name across BOTH kinds (`PROCEDURE dup(..)` + `FUNCTION dup(..)`),
    /// so the first overload is not necessarily the kind the user clicked —
    /// a group is a function exactly when it carries the return-value row
    /// (position 0, no name). When no group agrees, or the kind is unknown,
    /// the first group keeps the long-standing behavior. Rows arrive sorted
    /// by overload, so equal overloads are contiguous.
    ///
    /// Takes the whole [`crate::db::query::RoutineDefinition`], not just its
    /// argument rows, because the argument rows alone cannot answer for a
    /// PARAMETERLESS overload: it has no `ALL_ARGUMENTS` rows at all (none on
    /// 18c+, and the pre-18c placeholder row is dropped by the
    /// `data_type IS NOT NULL` filter), while `ALL_PROCEDURES` — carried in
    /// `definition.overloads` — lists one row per overload regardless. When
    /// the wanted kind is PROCEDURE, no argument group has that shape, and the
    /// dictionary lists an overload the rows do not cover, that overload can
    /// only be a parameterless procedure (a function always carries its
    /// return-value row), so the EMPTY selection is returned: the builder's
    /// empty-list path writes the simple call, which the server resolves to
    /// exactly that overload. Falling back to the first group instead is how
    /// `Execute Procedure` on such a member came to run the FUNCTION.
    fn select_overload_arguments(
        definition: &crate::db::query::RoutineDefinition,
        routine_type: &str,
    ) -> Vec<ProcedureArgument> {
        let mut groups: Vec<Vec<ProcedureArgument>> = Vec::new();
        let mut current_overload: Option<Option<i32>> = None;
        for arg in &definition.arguments {
            if current_overload != Some(arg.overload) {
                current_overload = Some(arg.overload);
                groups.push(Vec::new());
            }
            if let Some(group) = groups.last_mut() {
                group.push(arg.clone());
            }
        }

        let wants_function = routine_type.eq_ignore_ascii_case("FUNCTION");
        let wants_procedure = routine_type.eq_ignore_ascii_case("PROCEDURE");
        let matching = (wants_function || wants_procedure).then(|| {
            groups.iter().position(|group| {
                let is_function = group.iter().any(Self::is_function_return_row);
                is_function == wants_function
            })
        });
        if matches!(matching, Some(None))
            && wants_procedure
            && Self::dictionary_lists_argumentless_overload(&definition.overloads, &groups)
        {
            return Vec::new();
        }
        let index = matching.flatten().unwrap_or(0);
        groups.into_iter().nth(index).unwrap_or_default()
    }

    /// Whether the dictionary lists an overload the argument rows do not
    /// cover — i.e. a parameterless overload.
    ///
    /// The dictionary's overload numbers match the argument rows' value for
    /// value including `NULL` (live-proven, see [`RoutineCallForm::of`]), so
    /// an overload with no matching argument group is one the argument query
    /// genuinely returned nothing for, not a numbering mismatch. On the
    /// MySQL family and on Oracle's fail-open road `definition.overloads` is
    /// empty and this can never answer yes.
    fn dictionary_lists_argumentless_overload(
        overloads: &[crate::db::query::RoutineOverload],
        argument_groups: &[Vec<ProcedureArgument>],
    ) -> bool {
        overloads.iter().any(|listed| {
            !argument_groups
                .iter()
                .any(|group| group.first().map(|arg| arg.overload) == Some(listed.overload))
        })
    }

    /// The row that carries a FUNCTION's return value: position 0 with no
    /// argument name. No catalog in any supported family produces such a row
    /// for anything else.
    ///
    /// The DIRECTION is deliberately NOT part of the question. Oracle spells
    /// that row `OUT` and the MySQL family `RETURN`, so a direction test here
    /// is one that has to be written differently per family — which is how
    /// the group picker (position/name) and the Oracle script builder
    /// (position/name/direction) came to be able to disagree about which rows
    /// are return values, leaving the builder free to pass a picked group's
    /// return row along as a call argument.
    fn is_function_return_row(arg: &ProcedureArgument) -> bool {
        arg.position == 0 && arg.name.is_none()
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

    /// The declarable name of a composite-typed argument, `None` for scalars.
    ///
    /// `ALL_ARGUMENTS.DATA_TYPE` spells composites as dictionary keywords —
    /// `PL/SQL RECORD`, `PL/SQL TABLE` (associative array), `TABLE` (nested
    /// table), `VARRAY`, `OBJECT`, `OPAQUE/XMLTYPE`, `REF` (an object
    /// reference, declared `REF owner.type`) — which are not valid PL/SQL
    /// type names, so a DECLARE built from them cannot compile. The name a
    /// declaration can use is TYPE_OWNER/TYPE_NAME(/TYPE_SUBNAME): for a
    /// type declared inside a package the subname is the type itself, and a
    /// `PL/SQL RECORD` with NO subname is a table's implicit record, spelled
    /// `table%ROWTYPE`. TYPE_OWNER `PUBLIC` (a synonym-resolved owner) is
    /// not a schema a reference may name, so it is dropped.
    ///
    /// `REF` is matched EXACTLY: `REF CURSOR` must stay out, because a
    /// cursor row's TYPE_NAME/TYPE_SUBNAME describe the cursor's RETURN
    /// record — declaring by that name would produce a record variable, not
    /// a cursor. Cursors keep their `SYS_REFCURSOR` spelling.
    fn composite_argument_type_name(arg: &ProcedureArgument) -> Option<String> {
        let data_type = arg.data_type.as_deref()?.trim().to_uppercase();
        let is_composite = matches!(
            data_type.as_str(),
            "PL/SQL RECORD"
                | "PL/SQL TABLE"
                | "TABLE"
                | "VARRAY"
                | "OBJECT"
                | "OPAQUE/XMLTYPE"
                | "REF"
        );
        if !is_composite {
            return None;
        }
        let type_name = arg
            .type_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())?;
        let owner = arg
            .type_owner
            .as_deref()
            .map(str::trim)
            .filter(|owner| !owner.is_empty() && !owner.eq_ignore_ascii_case("PUBLIC"));
        let subname = arg
            .type_subname
            .as_deref()
            .map(str::trim)
            .filter(|subname| !subname.is_empty());

        let mut parts: Vec<String> = Vec::new();
        if let Some(owner) = owner {
            parts.push(crate::db::DatabaseConnection::quote_oracle_identifier(
                owner,
            ));
        }
        parts.push(crate::db::DatabaseConnection::quote_oracle_identifier(
            type_name,
        ));
        if let Some(subname) = subname {
            parts.push(crate::db::DatabaseConnection::quote_oracle_identifier(
                subname,
            ));
        }
        let joined = parts.join(".");
        if data_type == "PL/SQL RECORD" && subname.is_none() {
            return Some(format!("{}%ROWTYPE", joined));
        }
        if data_type == "REF" {
            return Some(format!("REF {}", joined));
        }
        Some(joined)
    }

    fn format_argument_type(arg: &ProcedureArgument) -> String {
        if let Some(composite) = Self::composite_argument_type_name(arg) {
            return composite;
        }
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

    /// One call argument as the generated CALL writes it: `label => value`
    /// with the label spelled as an identifier, or just the value when the
    /// dictionary gave no name.
    ///
    /// The label MUST go through identifier quoting: a quoted-created
    /// parameter name (`"my arg"`, `"lower"`) reaches ARGUMENT_NAME verbatim,
    /// and written bare it either fails to parse or normalizes to a DIFFERENT
    /// (uppercase) name than the parameter's. Ordinary uppercase names come
    /// back from [`crate::db::DatabaseConnection::quote_oracle_identifier`]
    /// byte-identical, so this changes nothing for them.
    fn oracle_call_argument_expr(arg_label: Option<&str>, value: &str) -> String {
        match arg_label {
            Some(label) => format!(
                "{} => {}",
                crate::db::DatabaseConnection::quote_oracle_identifier(label),
                value
            ),
            None => value.to_string(),
        }
    }

    /// One MySQL/MariaDB call argument, with the parameter it fills named
    /// beside it.
    ///
    /// Neither engine has named association, so the position is the only thing
    /// binding a value to a parameter — and a generated `CALL db.p(0, '',
    /// NULL)` told the user nothing about which is which, while the Oracle
    /// twin above has always written `NAME => value`. A comment is the only
    /// place the name can go, and it puts the two families' scripts back on
    /// equal terms: the reader can see what each value is for before editing
    /// it.
    ///
    /// The name goes through [`Self::mysql_comment_text`] rather than in raw:
    /// an identifier may legally hold `*/`, which would end the comment early
    /// and leave the rest of the name as SQL.
    fn mysql_call_argument_expr(arg_label: &str, value: &str) -> String {
        format!("/* {} */ {}", Self::mysql_comment_text(arg_label), value)
    }

    /// `text` as it can appear inside a `/* ... */` comment.
    ///
    /// Two characters cannot survive as themselves. `*/` ends the comment, so a
    /// parameter created as `` `a*/b` `` would close it and hand `b` to the
    /// parser as an expression; it is spaced apart instead. A newline does not
    /// break the comment — block comments span lines — but it does break the
    /// one-argument-per-line shape the script is read in, so it becomes a
    /// space. Everything else is left exactly as the catalog spells it, which
    /// is what makes the comment a truthful label; ordinary names come back
    /// byte-identical.
    ///
    /// Note the leading `/* ` this is always written with: `/*!` and `/*+` are
    /// MySQL's executable-comment and optimizer-hint forms, and the space is
    /// what keeps a name starting with `!` or `+` from becoming one.
    fn mysql_comment_text(text: &str) -> String {
        text.replace("*/", "* /").replace(['\n', '\r'], " ")
    }

    /// The starting value an argument the routine READS is given, `None` when
    /// the type cannot take one: `:= NULL` on a cursor variable, a record or
    /// an associative array is a compile error, and declared bare each of
    /// them already holds its neutral value (atomically null / empty).
    ///
    /// The one answer for both carriers — a `DECLARE` local takes it as its
    /// `:=` initializer, a bind as an assignment inside the block — so an
    /// IN OUT argument cannot start out initialized under one carrier and
    /// empty under the other.
    fn in_argument_initializer(arg: &ProcedureArgument, type_str: &str) -> Option<String> {
        if Self::is_ref_cursor(arg) || Self::composite_argument_type_name(arg).is_some() {
            return None;
        }
        Some(Self::default_value_for_argument(type_str))
    }

    /// The neutral value of an Oracle type.
    ///
    /// Derived from the TYPE alone. The dictionary's own
    /// `ALL_ARGUMENTS.DEFAULT_VALUE` is deliberately not consulted: it is a
    /// LONG holding the default expression's source text, which can span
    /// lines and name identifiers that are not in scope at the call site, and
    /// only one of the two Oracle protocols can read it at all.
    fn default_value_for_argument(type_str: &str) -> String {
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

    /// A distinct variable name for one argument, within `max_len`.
    ///
    /// The catalog's own parameter name is already inside ITS database's
    /// identifier limit, but the name generated from it is not the catalog's:
    /// it carries a `v_` prefix and may carry a `_2` disambiguator, and both
    /// are ours to budget for. Both families' limits are reachable from a
    /// perfectly legal routine — a MySQL parameter may be 64 characters, the
    /// identifier maximum, and `@v_<that>` is then an illegal user-variable
    /// name the server refuses outright.
    ///
    /// Truncating is safe: the name is only ever a LOCAL of the generated
    /// script. The routine's own parameter name is what the call is written
    /// with (named association on Oracle, the `AS` alias on the MySQL family),
    /// so the user can still tell which argument a value belongs to.
    fn unique_var_name(
        base_name: &str,
        position: i32,
        used: &mut HashSet<String>,
        max_len: usize,
    ) -> String {
        // ASCII by construction, so every length below is both a byte count
        // and a character count and slicing can never split a character.
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
        let mut candidate = format!("v_{}", cleaned);
        candidate.truncate(max_len);
        if used.insert(candidate.clone()) {
            return candidate;
        }

        let mut suffix = 2;
        loop {
            let tail = format!("_{}", suffix);
            let head = max_len.saturating_sub(tail.len()).min(candidate.len());
            let next = format!("{}{}", &candidate[..head], tail);
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
            // Declining here is not silence: the editor opens its own context
            // menu for any right-click this one does not take, and that beats a
            // menu holding one entry.
            ObjectMenuFallback::CallerMenu,
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

    /// Whether asking the server which kind of routine a package member is can
    /// change what the user is offered.
    ///
    /// Asked BEFORE the round trip, because the menu that lookup feeds holds
    /// `Execute` and nothing else: on a connection that refuses writes it is
    /// empty WHATEVER the answer turns out to be. Resolving anyway spent a
    /// round trip, a pooled session and a DB-activity registration on a menu
    /// that could not appear — and deferring is what makes the editor decline
    /// to open its own menu, so the click was then answered by silence when the
    /// empty result arrived. Declining instead lets the ordinary road run: it
    /// filters the same entries away and falls through to the editor's context
    /// menu, which is a real answer.
    ///
    /// "Can this menu hold anything?" is not a question for the catalog. The
    /// fact it rests on — a package routine's menu is Execute-only on every
    /// backend — is pinned by
    /// `a_package_routine_has_nothing_to_offer_a_write_refusing_connection`.
    fn package_routine_kind_is_worth_resolving(
        db_type: crate::db::DatabaseType,
        writes_are_refused: bool,
    ) -> bool {
        object_browser_behavior_for(db_type).supports_package_routines() && !writes_are_refused
    }

    fn defer_unknown_package_routine_context_menu(
        &self,
        item: ObjectItem,
        selected_scope: Option<String>,
        db_type: crate::db::DatabaseType,
    ) -> bool {
        if !Self::package_routine_kind_is_worth_resolving(
            db_type,
            self.write_refusal.writes_are_refused(),
        ) {
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
            let result = Self::run_object_action_work("Resolve package routine type", || {
                object_browser_behavior_for(db_type).load_package_routines(
                    &connection,
                    activity,
                    selected_scope.as_deref(),
                    &package_name,
                )
            });

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

    /// Turn an `UNKNOWN` package-routine item into a resolved one, from the
    /// listing the server just answered with.
    ///
    /// Both halves come from the SAME listed row: the kind, and the member's
    /// own spelling. Taking only the kind left the item naming the routine the
    /// way it was ASKED about — the user's typing, or a tree that once
    /// upper-cased quoted names — and every action that follows writes the
    /// name it finds here. `listed_package_routine` is what decides which row
    /// that is, so an exact spelling wins and two members differing only in
    /// case leave the item unresolved rather than resolved to a coin flip.
    fn apply_package_routine_type_from_routines(
        item: &mut ObjectItem,
        routines: &[PackageRoutine],
    ) {
        let requested_name = match item {
            ObjectItem::PackageRoutine { routine_name, .. } => routine_name.clone(),
            _ => return,
        };
        let Some((resolved_name, resolved_type)) =
            Self::listed_package_routine_identity(routines, &requested_name)
        else {
            return;
        };
        if let ObjectItem::PackageRoutine {
            routine_name,
            routine_type,
            ..
        } = item
        {
            *routine_name = resolved_name;
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

    /// What the status line reads once `Execute Routine`'s kind lookup is over.
    ///
    /// Both answers, in one place, because the road ANNOUNCES itself when it
    /// starts (`Resolving package routine type for X`) and the status label has
    /// no timer: the resolved half used to say nothing back, so a user who
    /// dismissed the menu without picking an entry kept a line claiming the
    /// lookup was still running. Same rule as
    /// [`RoutineScriptDelivery::status`], for the same reason, on the other
    /// half of the same feature.
    ///
    /// `resolved` is the caller's own answer
    /// ([`Self::package_routine_type_is_resolved`]) rather than re-asked here,
    /// so the sentence cannot disagree with the menu that is or is not shown.
    fn package_routine_resolution_status(item: &ObjectItem, resolved: bool) -> String {
        let ObjectItem::PackageRoutine {
            package_name,
            routine_name,
            routine_type,
        } = item
        else {
            return "Could not resolve package routine type".to_string();
        };
        match resolved {
            true => format!("Resolved {package_name}.{routine_name} as {routine_type}"),
            false => "Could not resolve package routine type".to_string(),
        }
    }

    fn resolve_selected_object_context(
        selected_text: &str,
        data: &IntellisenseData,
        cache: Option<&ObjectCache>,
        db_type: crate::db::DatabaseType,
        current_scope: Option<&str>,
    ) -> Option<ResolvedObjectContext> {
        // Each part becomes the name it DENOTES on this backend before any
        // lookup sees it. The lexer above can only report the text and whether
        // it was quoted; folding those two into one string is what made
        // `pkg.myProc` and `pkg."myProc"` the same value, and they are two
        // different routines.
        let parts: Vec<String> = Self::selected_object_reference_parts(selected_text)?
            .iter()
            .map(|part| part.denoted_name(db_type))
            .collect();
        match parts.as_slice() {
            [name] => Self::resolve_simple_selection_object(name, data, cache, db_type),
            [qualifier, name] => {
                Self::resolve_known_package_routine(qualifier, name, data, cache, db_type)
                    .or_else(|| {
                        Self::resolve_qualified_schema_object(qualifier, name, data, db_type)
                    })
                    .or_else(|| {
                        if Self::scope_matches_current_or_default(
                            qualifier,
                            current_scope,
                            data.default_qualifier(),
                        ) {
                            Self::resolve_simple_selection_object(name, data, cache, db_type)
                                .and_then(|mut context| {
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
                                })
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

    fn selected_object_reference_parts(selected_text: &str) -> Option<Vec<SelectedObjectPart>> {
        let trimmed = selected_text
            .trim()
            .trim_matches(|ch| matches!(ch, ';' | ',' | '(' | ')'))
            .trim();
        if trimmed.is_empty() || trimmed.lines().count() > 1 {
            return None;
        }

        let mut parts: Vec<SelectedObjectPart> = Vec::new();
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

    fn normalize_selected_object_part(part: &str) -> Option<SelectedObjectPart> {
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
        Some(SelectedObjectPart {
            text: unquoted,
            quoted: is_quoted,
        })
    }

    /// The name an object action TARGETS, from the session it actually runs on.
    ///
    /// Composed HERE — not as a trait method — because the composition is not
    /// backend policy: both halves already are. The scope half is
    /// [`ObjectBrowserDbBehavior::action_scope`] (Oracle's browsed scope IS the
    /// scope; the MySQL family fills a missing one from the connection's
    /// current database) and the spelling half is
    /// [`ObjectBrowserDbBehavior::qualify_object_name`].
    ///
    /// One composition because a caller that pre-computes a name for the
    /// FAILURE road used to skip the scope half entirely, so one action
    /// answered "which scope is this on?" two ways: the script said
    /// ``CALL `app`.`p`(...)`` and the fallback said ``CALL `p`()``.
    fn action_object_name(
        behavior: &dyn ObjectBrowserDbBehavior,
        context: &crate::db::DbPoolSessionContext,
        selected_scope: Option<&str>,
        object_name: &str,
    ) -> String {
        behavior.qualify_object_name(behavior.action_scope(selected_scope, context), object_name)
    }

    /// The two stored-routine groups, in the order a selected name that matches
    /// BOTH of them is read.
    ///
    /// Only the MySQL family can produce that name at all
    /// ([`ObjectBrowserDbBehavior::routine_namespaces_can_collide`]), and there
    /// the FUNCTION answers. On Oracle the order is unobservable — the two
    /// lists cannot both hold one name — so the long-standing spelling stays.
    fn routine_selection_order(db_type: crate::db::DatabaseType) -> [RoutineSelectionGroup; 2] {
        match object_browser_behavior_for(db_type).routine_namespaces_can_collide() {
            true => [
                RoutineSelectionGroup::Functions,
                RoutineSelectionGroup::Procedures,
            ],
            false => [
                RoutineSelectionGroup::Procedures,
                RoutineSelectionGroup::Functions,
            ],
        }
    }

    fn resolve_simple_selection_object(
        name: &str,
        data: &IntellisenseData,
        cache: Option<&ObjectCache>,
        db_type: crate::db::DatabaseType,
    ) -> Option<ResolvedObjectContext> {
        let routines = Self::routine_selection_order(db_type).map(|group| {
            (
                group.object_type(),
                Self::selection_name_match(group.names(data), name)
                    .or_else(|| Self::cache_name_match(cache, group.object_type(), name)),
            )
        });
        let leading = [
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
        ];
        let trailing = [
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

        for (object_type, object_name) in leading.into_iter().chain(routines).chain(trailing) {
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
        db_type: crate::db::DatabaseType,
    ) -> Option<ResolvedObjectContext> {
        // `schema.calc` reaches here on the MySQL family too, so the routine
        // pair takes the same backend-decided order the bare-name road takes.
        let routines = Self::routine_selection_order(db_type)
            .map(|group| (group.object_type(), group.qualified_member_kind()));
        let leading = [
            ("TABLES", QualifiedMemberKind::Table),
            ("VIEWS", QualifiedMemberKind::View),
            ("MATERIALIZED VIEWS", QualifiedMemberKind::MaterializedView),
            ("TYPES", QualifiedMemberKind::Type),
        ];
        let trailing = [
            ("PACKAGES", QualifiedMemberKind::Package),
            ("SEQUENCES", QualifiedMemberKind::Sequence),
            ("TRIGGERS", QualifiedMemberKind::Trigger),
            ("INDEXES", QualifiedMemberKind::Index),
            ("EVENTS", QualifiedMemberKind::Event),
        ];

        for (object_type, kind) in leading.into_iter().chain(routines).chain(trailing) {
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

    /// The ONE candidate a requested name identifies, or `None` when the
    /// candidates cannot settle it.
    ///
    /// EXACT first, because the requested name is already the one the
    /// selection DENOTES ([`SelectedObjectPart::denoted_name`]) or a catalog
    /// spelling the tree handed over — either way an exact hit is the answer
    /// the server itself would give.
    ///
    /// The case-insensitive pass is the convenience half: a bare `EMP` finding
    /// a quoted-created `emp`, or a MySQL name typed in another case.
    ///
    /// BOTH passes are settled by the same rule — a name answers when every
    /// candidate it matches is the SAME candidate — which is why `T: PartialEq`
    /// is required. Counting matches instead would be wrong in both
    /// directions: a list of plain names can legitimately hold one name twice
    /// (two entries carrying one answer, which must still resolve), while a
    /// package listing can hold `DUP` twice with two different KINDS, and one
    /// of those two must not be picked by the order the list happens to be in.
    /// Only the exact pass used to skip the question, and a wrapped package's
    /// cross-kind duplicate is exactly the shape that reaches it.
    fn identified_by_name<'a, T: PartialEq>(
        candidates: &'a [T],
        requested_name: &str,
        name_of: impl Fn(&T) -> &str,
    ) -> Option<&'a T> {
        let requested_name = requested_name.trim();
        fn settled<'a, T: PartialEq>(mut matches: impl Iterator<Item = &'a T>) -> Option<&'a T> {
            let first = matches.next()?;
            matches.all(|other| other == first).then_some(first)
        }
        settled(
            candidates
                .iter()
                .filter(|candidate| name_of(candidate) == requested_name),
        )
        .or_else(|| {
            settled(
                candidates
                    .iter()
                    .filter(|candidate| name_of(candidate).eq_ignore_ascii_case(requested_name)),
            )
        })
    }

    fn selection_name_match(names: &[String], candidate: &str) -> Option<String> {
        Self::identified_by_name(names, candidate, String::as_str).cloned()
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
            // The same reader the server-side resolution uses: exact spelling
            // first, and no answer at all when two members differ only in
            // case. `"myProc"` and `MYPROC` are two routines a package may
            // declare side by side, and this cache is now able to hold both —
            // an unresolved name falls through to `UNKNOWN`, which asks the
            // server and gets the same refusal in words.
            .and_then(|(_, routines)| Self::listed_package_routine_identity(routines, routine_name))
    }

    /// The catalog's routine type as one of the two values every consumer
    /// understands, or `None` for anything else.
    pub(crate) fn normalize_package_routine_type(routine_type: &str) -> Option<String> {
        match routine_type.trim().to_ascii_uppercase().as_str() {
            "FUNCTION" => Some("FUNCTION".to_string()),
            "PROCEDURE" => Some("PROCEDURE".to_string()),
            _ => None,
        }
    }

    /// A catalog routine type as the `routine_type` an
    /// [`ObjectItem::PackageRoutine`] may carry.
    ///
    /// Exactly three values mean something downstream: `PROCEDURE` and
    /// `FUNCTION` name an action arm, and `UNKNOWN` is the one that makes the
    /// menu ask the server. Anything else reaches a menu whose single entry
    /// matches no arm — a dead item — so it is folded into `UNKNOWN` HERE,
    /// where the item is built, rather than left for each consumer to notice.
    pub(crate) fn package_routine_item_type(routine_type: &str) -> String {
        Self::normalize_package_routine_type(routine_type).unwrap_or_else(|| "UNKNOWN".to_string())
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
                // A tree node is a dead end: nothing else opens for this
                // click, so a node whose entries were all filtered away has to
                // say WHY rather than do nothing.
                ObjectMenuFallback::None,
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
        fallback: ObjectMenuFallback,
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
            fallback,
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
        fallback: ObjectMenuFallback,
        mouse_x: i32,
        mouse_y: i32,
    ) -> bool {
        let db_type = match current_db_type.lock() {
            Ok(guard) => *guard,
            Err(poisoned) => *poisoned.into_inner(),
        };
        let Some(menu_choices) = Self::menu_choices_for_object_item(&item_info, db_type) else {
            // This item TYPE has no menu at all — a column, a category folder.
            // Nothing to say, so nothing is shown.
            return false;
        };
        let Some(menu_choices) =
            Self::menu_choices_for_read_only(menu_choices, write_refusal.writes_are_refused())
        else {
            // The item HAS a menu and the read-only filter emptied it — which
            // only a package routine can reach, its entries being `Execute`
            // and nothing else. Whether that is answered or declined is the
            // CALLER's fact, not this function's: see [`ObjectMenuFallback`].
            return match fallback {
                ObjectMenuFallback::None => Self::show_object_menu_refusal_at(
                    &item_info,
                    status_callback,
                    db_type,
                    selected_scope.as_deref(),
                    ObjectMenuRefusal::WritesRefused,
                    mouse_x,
                    mouse_y,
                ),
                ObjectMenuFallback::CallerMenu => false,
            };
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
                        // Only the TYPE is read here: which label may act on
                        // this item. The name travels on the item itself, to
                        // the one loader both Execute arms share.
                        ObjectItem::Simple { object_type, .. },
                    ) if (label == "Execute Procedure" && object_type == "PROCEDURES")
                        || (label == "Execute Function" && object_type == "FUNCTIONS") =>
                    {
                        Self::spawn_routine_script_load(
                            connection,
                            action_sender,
                            status_callback,
                            db_type,
                            selected_scope.clone(),
                            item_info.clone(),
                            Self::execute_label_routine_type(label),
                        );
                    }
                    (
                        label @ ("Execute Procedure" | "Execute Function" | "Execute Routine"),
                        ObjectItem::PackageRoutine { routine_type, .. },
                    ) if (label == "Execute Procedure" && routine_type == "PROCEDURE")
                        || (label == "Execute Function" && routine_type == "FUNCTION")
                        || (label == "Execute Routine" && routine_type == "UNKNOWN") =>
                    {
                        Self::spawn_routine_script_load(
                            connection,
                            action_sender,
                            status_callback,
                            db_type,
                            selected_scope.clone(),
                            item_info.clone(),
                            Self::execute_label_routine_type(label),
                        );
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
                            let result = Self::run_object_action_work("Check compilation", || {
                                object_browser_behavior_for(db_type).load_compilation_errors(
                                    &connection,
                                    format!("Checking compilation status for {}", object_name),
                                    selected_scope.as_deref(),
                                    &object_name,
                                    &object_type,
                                )
                            });
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
                            let result = Self::run_object_action_work("Prepare import", || {
                                read_import_file(&path).and_then(|text| {
                                    ObjectBrowserWidget::with_pooled_object_session(
                                        &connection,
                                        selected_scope.as_deref(),
                                        activity,
                                        |context, session| {
                                            object_browser_behavior_for(
                                                context.connection_info.db_type,
                                            )
                                            .load_table_structure(
                                                context,
                                                session,
                                                selected_scope.as_deref(),
                                                &table_name,
                                            )
                                        },
                                    )
                                    .map(|columns| (text, columns))
                                })
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
                            let result = Self::run_object_action_work("Export table", || {
                                ObjectBrowserWidget::with_pooled_object_session(
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
                                )
                            });
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
                            let result =
                                Self::run_object_action_work("Load table structure", || {
                                    ObjectBrowserWidget::with_pooled_object_session(
                                        &connection,
                                        selected_scope.as_deref(),
                                        format!("Loading table structure for {}", table_name),
                                        |context, session| {
                                            object_browser_behavior_for(
                                                context.connection_info.db_type,
                                            )
                                            .load_table_structure(
                                                context,
                                                session,
                                                selected_scope.as_deref(),
                                                &table_name,
                                            )
                                        },
                                    )
                                });
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
                            let result = Self::run_object_action_work("Load indexes", || {
                                ObjectBrowserWidget::with_pooled_object_session(
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
                                )
                            });
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
                            let result = Self::run_object_action_work("Load constraints", || {
                                ObjectBrowserWidget::with_pooled_object_session(
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
                                )
                            });
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

                            let result = Self::run_object_action_work("Load object info", || {
                                ObjectBrowserWidget::with_pooled_object_session(
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
                                )
                            });
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

    /// The menu an object gets when it has actions but none of them can be
    /// offered — with the REASON on it, and the one entry that is still true.
    ///
    /// One function for both reasons, because the alternative is what this
    /// replaced: the road that could not resolve a package routine's kind
    /// showed `Copy Name`, while the road whose entries were all filtered out
    /// showed NOTHING — so the better-informed case gave the user less, and a
    /// right-click could be answered by silence. `reason` is a named value
    /// rather than a caller-supplied label so a third road cannot invent a
    /// third wording for one of these two situations.
    fn show_object_menu_refusal_at(
        item_info: &ObjectItem,
        status_callback: &StatusCallback,
        db_type: crate::db::DatabaseType,
        selected_scope: Option<&str>,
        reason: ObjectMenuRefusal,
        mouse_x: i32,
        mouse_y: i32,
    ) -> bool {
        let current_group = fltk::group::Group::try_current();
        fltk::group::Group::set_current(None::<&fltk::group::Group>);

        let mut menu = MenuButton::new(mouse_x, mouse_y, 0, 0, None);
        menu.set_color(theme::panel_raised());
        menu.set_text_color(theme::text_primary());
        menu.add(reason.label(), Shortcut::None, MenuFlag::Inactive, |_| {});
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

    /// Run one object-action worker's fallible work with the same panic
    /// discipline as the scope-switch worker: a panic becomes the action's
    /// ERROR — reported through the action's own channel — instead of a
    /// dead thread that leaves the status line stuck on "Loading…" with a
    /// reply that never comes.
    fn run_object_action_work<T>(
        action: &str,
        work: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        panic::catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|payload| {
            let panic_msg = Self::panic_payload_to_string(payload.as_ref());
            crate::utils::logging::log_error(
                "object_browser::action",
                &format!("{action} worker panicked: {panic_msg}"),
            );
            eprintln!("{action} worker panicked: {panic_msg}");
            Err(format!("{action} failed internally: {panic_msg}"))
        })
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

    /// Oracle folds an unquoted identifier to upper case — the same reading
    /// `QueryExecutor::normalize_object_name` gives a catalog name.
    fn denoted_bare_identifier(&self, identifier: &str) -> String {
        identifier.to_uppercase()
    }

    /// One namespace: a schema holds `calc` as a procedure or as a function,
    /// never both — the second `CREATE` is refused as a name already used — so
    /// the two lists can never both answer.
    fn routine_namespaces_can_collide(&self) -> bool {
        false
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

    fn build_simple_routine_script(&self, qualified_name: &str, routine_type: &str) -> String {
        ObjectBrowserWidget::build_simple_oracle_routine_script(qualified_name, routine_type)
    }

    fn build_routine_script(
        &self,
        qualified_name: &str,
        routine_type: &str,
        definition: &crate::db::query::RoutineDefinition,
    ) -> RoutineScriptOutcome {
        ObjectBrowserWidget::build_procedure_script(qualified_name, routine_type, definition)
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
        context: &crate::db::DbPoolSessionContext,
        session: &mut crate::db::DbPoolSession,
        selected_scope: Option<&str>,
        object_name: &str,
        routine_type: &str,
    ) -> Result<RoutineScriptData, String> {
        // The same reader the action's own pre-computed name goes through, so
        // the script and the failure road cannot name the object differently.
        // Inert here — this family's `action_scope` returns the browsed scope
        // unchanged — and stated anyway so the two families take one road.
        let qualified_name =
            ObjectBrowserWidget::action_object_name(self, context, selected_scope, object_name);
        let lookup = match session {
            crate::db::DbPoolSession::Oracle(conn) => {
                ObjectBrowser::get_procedure_definition(conn, &qualified_name)?
            }
            crate::db::DbPoolSession::OracleThin(conn) => {
                ObjectBrowser::get_thin_procedure_definition(conn, &qualified_name)?
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
            outcome: ObjectBrowserWidget::routine_script_outcome(
                self,
                &qualified_name,
                routine_type,
                lookup,
            ),
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
        load_db_type: &mut crate::db::DatabaseType,
    ) -> Result<RoutineScriptData, String> {
        // The name the ACTION was asked about. It is the catalog's spelling
        // whenever it came from the tree or a resolved selection, and the
        // user's own whenever the kind arrived UNKNOWN from unresolved editor
        // text — which is why the UNKNOWN road below replaces it with the
        // spelling the package listing carries.
        let requested_display_name = self.qualify_package_member_name(
            selected_scope,
            package_name,
            &crate::db::DatabaseConnection::quote_oracle_identifier(routine_name),
        );
        let package_qualified_name = self.qualify_object_name(selected_scope, package_name);
        ObjectBrowserWidget::with_pooled_object_session(
            connection,
            selected_scope,
            activity,
            |context, session| {
                *load_db_type = context.connection_info.db_type;
                let (member_name, resolved_type) = if routine_type == "UNKNOWN" {
                    let routines = match session {
                        crate::db::DbPoolSession::Oracle(conn) => {
                            ObjectBrowser::get_package_routines(conn, &package_qualified_name)
                                .map_err(|err| err.to_string())?
                        }
                        crate::db::DbPoolSession::OracleThin(conn) => {
                            ObjectBrowser::get_thin_package_routines(conn, &package_qualified_name)?
                        }
                        unexpected @ crate::db::DbPoolSession::MySQL { .. } => {
                            return Err(format!(
                                "Expected Oracle object action session but acquired {}",
                                unexpected.db_type()
                            ))
                        }
                    };
                    match ObjectBrowserWidget::resolve_listed_package_routine(
                        &routines,
                        routine_name,
                        &requested_display_name,
                    ) {
                        Ok(identity) => identity,
                        // The listing ANSWERED and could not settle the
                        // member. That refusal is delivered as the catalog's
                        // own outcome — the `Err` road below is reserved for
                        // reads that could not be made.
                        Err(outcome) => {
                            return Ok(RoutineScriptData {
                                qualified_name: requested_display_name.clone(),
                                resolved_routine_type: routine_type.to_string(),
                                outcome,
                            })
                        }
                    }
                } else {
                    (routine_name.to_string(), routine_type.to_string())
                };

                // ONE spelling of the member's name, used by the script AND by
                // the lookup. They used to be written differently — the script
                // quoted the catalog's name, the lookup passed it bare into a
                // reader that uppercases anything unquoted — so a quoted
                // mixed-case member was looked up under a name the dictionary
                // does not hold and came back with no arguments at all.
                // `quote_oracle_identifier` passes an already-quoted name
                // through unchanged, so composing it once here and again inside
                // `qualify_package_member_name` is the same text.
                let member_sql_name =
                    crate::db::DatabaseConnection::quote_oracle_identifier(&member_name);
                let qualified_name = self.qualify_package_member_name(
                    selected_scope,
                    package_name,
                    &member_sql_name,
                );
                let lookup = match session {
                    crate::db::DbPoolSession::Oracle(conn) => {
                        ObjectBrowser::get_package_procedure_definition(
                            conn,
                            &package_qualified_name,
                            &member_sql_name,
                        )?
                    }
                    crate::db::DbPoolSession::OracleThin(conn) => {
                        ObjectBrowser::get_thin_package_procedure_definition(
                            conn,
                            &package_qualified_name,
                            &member_sql_name,
                        )?
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
                    resolved_routine_type: resolved_type.clone(),
                    outcome: ObjectBrowserWidget::routine_script_outcome(
                        self,
                        &qualified_name,
                        &resolved_type,
                        lookup,
                    ),
                })
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
    /// `scope.object` as a PATH — a value
    /// [`ObjectBrowserWidget::quote_mysql_identifier_path`] later splits back
    /// into its segments.
    ///
    /// `object_name` is one object's own catalog name, so a `.` inside it
    /// belongs to the name (`CREATE PROCEDURE `my.proc`` is accepted by both
    /// engines and the catalog reports ROUTINE_NAME `my.proc`). Written bare
    /// into the path it becomes a separator, and the split then names schema
    /// `my`, object `proc` — a DIFFERENT object, while the argument lookup,
    /// which takes the scope and the name as two values, kept finding the
    /// right one. Backticking the segment is what keeps one name one segment;
    /// the splitter strips and re-adds the quotes, so a name that needs none
    /// comes out byte-identical.
    fn qualify_object_name(&self, selected_scope: Option<&str>, object_name: &str) -> String {
        let object_name = object_name.trim();
        if object_name.is_empty() {
            return object_name.to_string();
        }
        let object_name = ObjectBrowserWidget::quote_mysql_path_segment(object_name);

        selected_scope
            .filter(|scope| !scope.trim().is_empty())
            .map(|scope| {
                format!(
                    "{}.{}",
                    ObjectBrowserWidget::quote_mysql_path_segment(scope.trim()),
                    object_name
                )
            })
            .unwrap_or(object_name)
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

    /// Neither engine folds an identifier's case, so a bare name denotes
    /// itself. Upper-casing here would invent a name the server never
    /// resolves — `Emp` and `emp` can be two tables on a case-sensitive file
    /// system.
    fn denoted_bare_identifier(&self, identifier: &str) -> String {
        identifier.to_string()
    }

    /// Two namespaces: `CREATE PROCEDURE calc` and `CREATE FUNCTION calc` can
    /// both exist in one database, so a bare `calc` really does name two
    /// objects.
    fn routine_namespaces_can_collide(&self) -> bool {
        true
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

    fn build_simple_routine_script(&self, qualified_name: &str, routine_type: &str) -> String {
        ObjectBrowserWidget::build_simple_mysql_routine_script(qualified_name, routine_type)
    }

    /// Always a script: this family has exactly one invocation form, so there
    /// is no routine it can describe and then be unable to call. That is the
    /// same fact its loaders state by leaving `RoutineDefinition::overloads`
    /// empty.
    fn build_routine_script(
        &self,
        qualified_name: &str,
        routine_type: &str,
        definition: &crate::db::query::RoutineDefinition,
    ) -> RoutineScriptOutcome {
        RoutineScriptOutcome::Script(ObjectBrowserWidget::build_mysql_routine_script(
            qualified_name,
            routine_type,
            definition,
        ))
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
        // The menu offered exactly one namespace's routine; carrying that
        // choice into the lookup keeps a same-named function/procedure pair
        // from answering for each other.
        let kind =
            crate::db::query::mysql_executor::MysqlRoutineKind::from_routine_type(routine_type)
                .ok_or_else(|| format!("Unsupported MySQL/MariaDB routine type: {routine_type}"))?;
        let conn = self.take_object_action_session(context, session)?;
        let action_scope = self.action_scope(selected_scope, context);
        // Through the shared reader, not a second spelling of it: this is the
        // name the action's failure road also has to produce, and a card with
        // no schema picked used to make the two disagree.
        let qualified_name =
            ObjectBrowserWidget::action_object_name(self, context, selected_scope, object_name);
        crate::db::query::mysql_executor::MysqlObjectBrowser::get_routine_definition_in_schema(
            conn.as_mut(),
            action_scope,
            object_name,
            kind,
            &qualified_name,
        )
        .map(|lookup| RoutineScriptData {
            qualified_name: qualified_name.clone(),
            resolved_routine_type: routine_type.to_string(),
            outcome: ObjectBrowserWidget::routine_script_outcome(
                self,
                &qualified_name,
                routine_type,
                lookup,
            ),
        })
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

    /// No session is acquired, so `load_db_type` is left as the caller found
    /// it: this family has no package routines at all
    /// (`supports_package_routines()` is false, and the selection resolver
    /// gates on it), so nothing constructs the item that reaches here.
    fn load_package_routine_script(
        &self,
        _connection: &SharedConnection,
        _activity: String,
        _selected_scope: Option<&str>,
        package_name: &str,
        routine_name: &str,
        _routine_type: &str,
        _load_db_type: &mut crate::db::DatabaseType,
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
        MultiObjectBrowserWidget, ObjectBrowserDbBehavior, ObjectBrowserMetadataSnapshot,
        ObjectBrowserWidget, ObjectCache, ObjectDefaultAction, ObjectItem, OracleRoutineScript,
        OracleSqlScopeShape, RoutineScriptOutcome, ScopeSwitchPreflightCallback,
        MYSQL_OBJECT_BROWSER_BEHAVIOR, SCOPE_SELECTOR_ROW_HEIGHT,
        SCOPE_SELECTOR_TABLE_VERTICAL_PADDING,
    };
    use crate::db::{DatabaseType, OracleDriverMode};
    use crate::db::{
        PackageRoutine, ProcedureArgument, RoutineDefinition, RoutineInvocation, RoutineOverload,
    };
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
            type_subname: None,
            pls_type: None,
            overload: None,
            default_value: None,
        }
    }

    /// The definition of an ORDINARY routine: these arguments, and no
    /// dictionary row claiming a call form other than the family's usual one.
    ///
    /// That is what every script-shape test below is about, and what
    /// `RoutineCallForm::of` reads out of an empty overload list — so wrapping
    /// an argument list in this leaves the generated script byte for byte what
    /// it was before the builders started taking a whole definition.
    fn routine_definition(arguments: &[ProcedureArgument]) -> RoutineDefinition {
        RoutineDefinition::from_arguments(arguments.to_vec())
    }

    /// The definition of a routine the dictionary marks with a non-ordinary
    /// invocation form, for the one overload the tests build.
    fn routine_definition_with_call_form(
        arguments: &[ProcedureArgument],
        invocation: RoutineInvocation,
    ) -> RoutineDefinition {
        RoutineDefinition {
            overloads: vec![RoutineOverload {
                overload: arguments.first().and_then(|arg| arg.overload),
                invocation,
            }],
            arguments: arguments.to_vec(),
        }
    }

    /// The Oracle script a builder wrote, for the tests whose subject is the
    /// TEXT. A refusal fails the test loudly instead of being formatted into
    /// an assertion as if it were a script.
    fn oracle_script(
        qualified_name: &str,
        routine_type: &str,
        definition: &RoutineDefinition,
    ) -> String {
        match ObjectBrowserWidget::build_procedure_script(qualified_name, routine_type, definition)
        {
            RoutineScriptOutcome::Script(sql) => sql,
            RoutineScriptOutcome::Refused(reason) => {
                panic!("{qualified_name}: the builder refused with {reason}")
            }
        }
    }

    /// A composite-typed argument as the Oracle dictionary reports it: the
    /// keyword in DATA_TYPE, the declarable name split across
    /// TYPE_OWNER/TYPE_NAME/TYPE_SUBNAME.
    fn composite_procedure_argument(
        name: Option<&str>,
        position: i32,
        data_type: &str,
        in_out: &str,
        type_owner: Option<&str>,
        type_name: Option<&str>,
        type_subname: Option<&str>,
    ) -> ProcedureArgument {
        ProcedureArgument {
            type_owner: type_owner.map(str::to_string),
            type_name: type_name.map(str::to_string),
            type_subname: type_subname.map(str::to_string),
            ..procedure_argument(name, position, Some(data_type), in_out)
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
        let sql =
            ObjectBrowserWidget::preview_select_sql(crate::db::DatabaseType::MySQL, None, "items");
        assert_eq!(sql, "SELECT * FROM `items` LIMIT 100");

        let sql = ObjectBrowserWidget::preview_select_sql(
            crate::db::DatabaseType::MySQL,
            Some("order"),
            "items",
        );
        assert_eq!(sql, "SELECT * FROM `order`.`items` LIMIT 100");
    }

    /// The SCOPE is what says which schema, and it arrives as its own value —
    /// the editor-selection resolver splits `sales.orders` into
    /// `selected_scope = sales` + `object_name = orders` before any action
    /// runs, and the tree only ever holds one object's catalog name.
    ///
    /// So a `.` inside `object_name` is part of that name. This assertion used
    /// to read the other way (`order.items` becoming `` `order`.`items` ``),
    /// which is the premise that made `Select Data (Top 100)` on a table named
    /// `order.items` read a table called `items` in a schema called `order`.
    /// Both engines accept such a name — live-proven on MySQL 8:
    /// ``CREATE PROCEDURE `zq.dot`(IN a INT)`` is created and
    /// `INFORMATION_SCHEMA.ROUTINES` reports ROUTINE_NAME `zq.dot`.
    #[test]
    fn preview_select_sql_keeps_a_dotted_mysql_catalog_name_as_one_object() {
        let sql = ObjectBrowserWidget::preview_select_sql(
            crate::db::DatabaseType::MySQL,
            Some("sales"),
            "order.items",
        );

        assert_eq!(sql, "SELECT * FROM `sales`.`order.items` LIMIT 100");
    }

    /// A dot INSIDE a quoted segment is not a separator. This is the path
    /// splitter's own rule — it is what lets a segment carry a dot at all —
    /// so it is asserted on the splitter rather than through a caller whose
    /// input is a single object name.
    #[test]
    fn quote_mysql_identifier_path_preserves_quoted_dotted_segments() {
        assert_eq!(
            ObjectBrowserWidget::quote_mysql_identifier_path("`sales.ops`.`order.items`"),
            "`sales.ops`.`order.items`"
        );
        // The OTHER character the splitter tracks. A backtick is legal inside
        // a MySQL-family name and the catalog reports it raw; doubled once, it
        // is the spelling the server accepts — live-proven on MariaDB
        // (``CREATE PROCEDURE `zr``tick` `` is created and reported as
        // ``zr`tick``, and the doubled call runs).
        assert_eq!(
            MYSQL_OBJECT_BROWSER_BEHAVIOR.qualify_object_name(Some("app"), "zr`tick"),
            "app.`zr``tick`"
        );
        assert_eq!(
            ObjectBrowserWidget::quote_mysql_identifier_path(
                &MYSQL_OBJECT_BROWSER_BEHAVIOR.qualify_object_name(Some("app"), "zr`tick")
            ),
            "`app`.`zr``tick`"
        );
        // And re-quoting what `qualify_object_name` produced is inert, which
        // is what lets the qualifier quote a segment without the splitter
        // quoting it a second time.
        assert_eq!(
            ObjectBrowserWidget::quote_mysql_identifier_path(
                &MYSQL_OBJECT_BROWSER_BEHAVIOR
                    .qualify_object_name(Some("sales.ops"), "order.items")
            ),
            "`sales.ops`.`order.items`"
        );
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
        // Nothing left means no CHOICE list, rather than an empty one. The
        // caller turns that into the refusal menu — see
        // `a_package_routine_has_nothing_to_offer_a_write_refusing_connection`,
        // which is what makes the right-click answerable at all.
        assert_eq!(
            ObjectBrowserWidget::menu_choices_for_read_only("Execute Procedure", true),
            None
        );
    }

    /// A package routine's menu is `Execute` and nothing else, on every
    /// backend — which is the fact two roads now depend on.
    ///
    /// `defer_unknown_package_routine_context_menu` returns BEFORE asking the
    /// server which kind of routine the member is, because whatever the answer
    /// turns out to be the menu is empty on a write-refusing connection; and
    /// `show_context_menu_for_object_item_at` shows the refusal menu instead of
    /// nothing, because a right-click that resolves to silence cannot be told
    /// from a node that has no actions.
    ///
    /// If an entry that is NOT a write is ever added to this menu, both are
    /// wrong: the kind would decide something again, and the refusal menu would
    /// hide a usable action. This case is what says so.
    #[test]
    fn a_package_routine_has_nothing_to_offer_a_write_refusing_connection() {
        for db_type in [
            crate::db::DatabaseType::Oracle,
            crate::db::DatabaseType::MySQL,
            crate::db::DatabaseType::MariaDB,
        ] {
            for routine_type in ["PROCEDURE", "FUNCTION", "UNKNOWN"] {
                let item = ObjectItem::PackageRoutine {
                    package_name: "PKG".to_string(),
                    routine_name: "R".to_string(),
                    routine_type: routine_type.to_string(),
                };
                let choices = ObjectBrowserWidget::menu_choices_for_object_item(&item, db_type)
                    .unwrap_or_else(|| {
                        panic!("{db_type:?}/{routine_type}: a package routine has a menu")
                    });
                assert_eq!(
                    ObjectBrowserWidget::menu_choices_for_read_only(choices, true),
                    None,
                    "{db_type:?}/{routine_type}: {choices}"
                );
            }
        }
    }

    /// A round trip is spent only when its answer can change what the user is
    /// offered.
    ///
    /// The kind lookup feeds a menu whose only entries are `Execute`, so on a
    /// write-refusing connection the menu is empty whatever comes back — and
    /// deferring is what makes the editor decline to open its own menu, so the
    /// click was answered by silence once the empty result arrived. The
    /// backends that have no package routines at all never ask either.
    #[test]
    fn a_package_routine_kind_is_resolved_only_when_the_answer_can_matter() {
        assert!(
            ObjectBrowserWidget::package_routine_kind_is_worth_resolving(
                crate::db::DatabaseType::Oracle,
                false
            )
        );
        assert!(
            !ObjectBrowserWidget::package_routine_kind_is_worth_resolving(
                crate::db::DatabaseType::Oracle,
                true
            ),
            "a write-refusing connection has no Execute entry to show, whatever the kind is"
        );
        for db_type in [
            crate::db::DatabaseType::MySQL,
            crate::db::DatabaseType::MariaDB,
        ] {
            for writes_are_refused in [false, true] {
                assert!(
                    !ObjectBrowserWidget::package_routine_kind_is_worth_resolving(
                        db_type,
                        writes_are_refused
                    ),
                    "{db_type:?} has no package routines to resolve"
                );
            }
        }
    }

    /// The two reasons a menu can have nothing to offer are one value with two
    /// distinct sentences, so a road cannot tell the user the wrong one.
    #[test]
    fn each_object_menu_refusal_says_which_one_it_is() {
        assert_ne!(
            super::ObjectMenuRefusal::RoutineTypeUnavailable.label(),
            super::ObjectMenuRefusal::WritesRefused.label()
        );
        assert!(super::ObjectMenuRefusal::WritesRefused
            .label()
            .to_lowercase()
            .contains("read only"));
        assert!(super::ObjectMenuRefusal::RoutineTypeUnavailable
            .label()
            .to_lowercase()
            .contains("type"));
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

    /// A `.` inside a catalog name is part of the NAME. Every caller hands one
    /// object's own name, so treating the dot as a qualifier that is already
    /// there skipped quoting and named a different object: `MY.PROC` reads as
    /// schema `MY`, and the browsed scope was dropped on top of that.
    /// Live-proven on 23ai: `"ZQ.DOT"` is creatable and `SYSTEM."ZQ.DOT"(A =>
    /// 1)` runs, while the dictionary holds it under object_name `ZQ.DOT`.
    #[test]
    fn oracle_object_names_quote_a_dot_that_belongs_to_the_name() {
        assert_eq!(
            ObjectBrowserWidget::qualify_oracle_object_name(Some("SCOTT"), "MY.PROC"),
            r#"SCOTT."MY.PROC""#
        );
        assert_eq!(
            ObjectBrowserWidget::qualify_oracle_object_name(None, "MY.PROC"),
            r#""MY.PROC""#
        );
        // The lookup reads that back as one object in one schema — pinned on
        // the db side by
        // `split_normalized_owner_object_name_reads_a_quoted_dot_as_one_name`.
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
                type_subname: None,
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
                type_subname: None,
                pls_type: None,
                overload: None,
                default_value: None,
            },
        ];

        let sql = ObjectBrowserWidget::build_mysql_routine_script(
            "demo_proc",
            "PROCEDURE",
            &routine_definition(&arguments),
        );

        assert!(sql.contains("CALL `demo_proc`("));
        assert!(sql.contains("0,"));
        assert!(sql.contains("@v_p_status"));
        assert!(sql.contains("SELECT @v_p_status AS `p_status`;"));
        assert!(!sql.contains("FROM dual"));
        assert!(!sql.contains("BEGIN\n"));
    }

    #[test]
    fn build_oracle_function_sys_refcursor_return_uses_bind_without_print() {
        // Uppercase, as the dictionary reports ordinary parameter names — a
        // lowercase ARGUMENT_NAME means a quoted-created parameter and is
        // covered by `build_oracle_script_quotes_quoted_parameter_labels`.
        let arguments = vec![
            procedure_argument(None, 0, Some("SYS_REFCURSOR"), "OUT"),
            procedure_argument(Some("P_MIN_SAL"), 1, Some("NUMBER"), "IN"),
        ];

        let sql = oracle_script(
            "DEMO_PKG.GET_ROWS",
            "FUNCTION",
            &routine_definition(&arguments),
        );

        assert!(sql.starts_with("VAR v_result REFCURSOR\n"));
        assert!(sql.contains("  :v_result := DEMO_PKG.GET_ROWS(\n"));
        assert!(sql.contains("P_MIN_SAL => v_p_min_sal"));
        assert!(!sql.contains("PRINT"));
        assert!(!sql.contains("v_result SYS_REFCURSOR"));
    }

    /// A quoted-created parameter's ARGUMENT_NAME arrives verbatim
    /// (`my arg`, `lower`); written bare in named association it either fails
    /// to parse or normalizes to a DIFFERENT (uppercase) name, so the label
    /// must be re-quoted. Ordinary uppercase labels stay unquoted.
    ///
    /// The labels are what this test pins; each one is asserted against the
    /// carrier its argument now takes, so a label change and a carrier change
    /// cannot hide behind each other. `lower` is an OUT scalar and is bound
    /// (see [`build_oracle_script_binds_out_arguments_so_their_values_show`]),
    /// which is why its value reads `:v_lower` and not `v_lower`.
    #[test]
    fn build_oracle_script_quotes_quoted_parameter_labels() {
        let arguments = vec![
            procedure_argument(Some("my arg"), 1, Some("NUMBER"), "IN"),
            procedure_argument(Some("lower"), 2, Some("VARCHAR2"), "OUT"),
            procedure_argument(Some("P_PLAIN"), 3, Some("NUMBER"), "IN"),
            composite_procedure_argument(
                Some("bad cur"),
                4,
                "REF CURSOR",
                "OUT",
                Some("SCOTT"),
                Some("SQ_PKG"),
                Some("T_REC"),
            ),
        ];

        let sql = oracle_script("SQ_QUOTED_P", "PROCEDURE", &routine_definition(&arguments));

        assert!(sql.contains("\"my arg\" => v_my_arg"));
        assert!(sql.contains("\"lower\" => :v_lower"));
        assert!(sql.contains("P_PLAIN => v_p_plain"));
        assert!(sql.contains("\"bad cur\" => :v_bad_cur"));
        assert!(sql.contains("VAR v_bad_cur REFCURSOR\n"));
    }

    /// The dictionary spells composite argument types as keywords
    /// (`PL/SQL RECORD`, `PL/SQL TABLE`, `OBJECT`) that are not PL/SQL; a
    /// declaration must use the qualified name from
    /// TYPE_OWNER/TYPE_NAME/TYPE_SUBNAME, and composites take no `:=`
    /// initializer (`:= NULL` on a record or associative array is a compile
    /// error).
    #[test]
    fn build_oracle_script_declares_composite_arguments_by_qualified_type_name() {
        let arguments = vec![
            composite_procedure_argument(
                Some("p_rec"),
                1,
                "PL/SQL RECORD",
                "IN",
                Some("SCOTT"),
                Some("SQ_PKG"),
                Some("T_REC"),
            ),
            composite_procedure_argument(
                Some("p_tab"),
                2,
                "PL/SQL TABLE",
                "IN",
                Some("SCOTT"),
                Some("SQ_PKG"),
                Some("T_TAB"),
            ),
            composite_procedure_argument(
                Some("p_obj"),
                3,
                "OBJECT",
                "IN",
                Some("SCOTT"),
                Some("SQ_OBJ_T"),
                None,
            ),
            procedure_argument(Some("p_n"), 4, Some("NUMBER"), "IN"),
        ];

        let sql = oracle_script(
            "SCOTT.SQ_PROC",
            "PROCEDURE",
            &routine_definition(&arguments),
        );

        assert!(sql.contains("  v_p_rec SCOTT.SQ_PKG.T_REC;\n"));
        assert!(sql.contains("  v_p_tab SCOTT.SQ_PKG.T_TAB;\n"));
        assert!(sql.contains("  v_p_obj SCOTT.SQ_OBJ_T;\n"));
        assert!(sql.contains("  v_p_n NUMBER := 0;\n"));
        assert!(!sql.contains("PL/SQL"));
        assert!(!sql.contains("v_p_obj OBJECT"));
    }

    /// A `PL/SQL RECORD` with no TYPE_SUBNAME is a table's implicit record:
    /// only the `%ROWTYPE` spelling names it, and a PUBLIC type owner (a
    /// synonym-resolved name) must not be spelled as a schema.
    #[test]
    fn build_oracle_script_spells_rowtype_record_and_drops_public_owner() {
        let arguments = vec![
            composite_procedure_argument(
                Some("p_user"),
                1,
                "PL/SQL RECORD",
                "IN",
                Some("PUBLIC"),
                Some("ALL_USERS"),
                None,
            ),
            composite_procedure_argument(
                Some("p_emp"),
                2,
                "PL/SQL RECORD",
                "OUT",
                Some("SCOTT"),
                Some("EMP"),
                None,
            ),
        ];

        let sql = oracle_script("SQ_ROW_PROC", "PROCEDURE", &routine_definition(&arguments));

        assert!(sql.contains("  v_p_user ALL_USERS%ROWTYPE;\n"));
        assert!(sql.contains("  v_p_emp SCOTT.EMP%ROWTYPE;\n"));
        assert!(!sql.contains("PUBLIC"));
    }

    /// A `REF` argument (an object reference) is declared `REF owner.type`,
    /// bare — the dictionary keyword alone (`v REF;`) and `:= NULL` are both
    /// compile errors. `REF CURSOR` must NOT take this path: a cursor row's
    /// TYPE_NAME/TYPE_SUBNAME name the cursor's RETURN record, so declaring
    /// by them would produce a record variable — cursors stay
    /// `SYS_REFCURSOR`.
    #[test]
    fn build_oracle_script_declares_ref_argument_by_referenced_type() {
        let arguments = vec![
            composite_procedure_argument(
                Some("p_ref"),
                1,
                "REF",
                "IN",
                Some("SCOTT"),
                Some("SQ_OBJ_T"),
                None,
            ),
            // A strong ref cursor: same populated type columns, different
            // DATA_TYPE — it must keep the cursor spelling.
            composite_procedure_argument(
                Some("p_cur"),
                2,
                "REF CURSOR",
                "IN",
                Some("SCOTT"),
                Some("SQ_PKG"),
                Some("T_REC"),
            ),
        ];

        let sql = oracle_script("SQ_REF_PROC", "PROCEDURE", &routine_definition(&arguments));

        assert!(sql.contains("  v_p_ref REF SCOTT.SQ_OBJ_T;\n"));
        assert!(sql.contains("  v_p_cur SYS_REFCURSOR;\n"));
        assert!(!sql.contains(":= NULL"));
        assert!(!sql.contains("v_p_cur SCOTT"));
    }

    /// A package may overload ONE name across BOTH kinds (legal PL/SQL). The
    /// script must call the first overload whose SHAPE matches the clicked
    /// menu label — not blindly the first overload — and an unknown kind
    /// keeps the long-standing first-group behavior.
    #[test]
    fn build_oracle_script_picks_the_overload_matching_the_clicked_kind() {
        fn overloaded(
            name: Option<&str>,
            position: i32,
            data_type: &str,
            in_out: &str,
            overload: i32,
        ) -> ProcedureArgument {
            ProcedureArgument {
                overload: Some(overload),
                ..procedure_argument(name, position, Some(data_type), in_out)
            }
        }
        // Overload 1: PROCEDURE dup(a NUMBER). Overload 2: FUNCTION dup(b
        // VARCHAR2) RETURN NUMBER — as the dictionary reports them, sorted.
        let arguments = vec![
            overloaded(Some("A"), 1, "NUMBER", "IN", 1),
            overloaded(None, 0, "NUMBER", "OUT", 2),
            overloaded(Some("B"), 2, "VARCHAR2", "IN", 2),
        ];

        let as_function =
            oracle_script("SQ_X_OVL.DUP", "FUNCTION", &routine_definition(&arguments));
        assert!(as_function.starts_with("VAR v_result NUMBER\n"));
        assert!(as_function.contains("B => v_b"));
        assert!(!as_function.contains("A => "));

        let as_procedure =
            oracle_script("SQ_X_OVL.DUP", "PROCEDURE", &routine_definition(&arguments));
        assert!(as_procedure.contains("A => v_a"));
        assert!(!as_procedure.contains("VAR "));
        assert!(!as_procedure.contains("B => "));

        let unknown = oracle_script("SQ_X_OVL.DUP", "UNKNOWN", &routine_definition(&arguments));
        assert!(unknown.contains("A => v_a"));
        assert!(!unknown.contains("B => "));
    }

    /// A PARAMETERLESS overload has no `ALL_ARGUMENTS` rows at all (none on
    /// 18c+, and the pre-18c placeholder row is dropped by the
    /// `data_type IS NOT NULL` filter), so the group picker cannot see it in
    /// the argument rows — only `definition.overloads` (`ALL_PROCEDURES`, one
    /// row per overload) can say it exists. With `PROCEDURE dup` +
    /// `FUNCTION dup(b)` in one package, `Execute Procedure` used to fall
    /// back to the function's group and run the routine the user did NOT
    /// click; the empty selection — the simple call `BEGIN pkg.dup; END;` —
    /// is what PL/SQL resolves to the parameterless procedure.
    #[test]
    fn build_oracle_script_calls_the_parameterless_procedure_the_dictionary_lists() {
        // Overload 1: PROCEDURE dup — parameterless, so NO argument rows.
        // Overload 2: FUNCTION dup(b VARCHAR2) RETURN NUMBER.
        let arguments = vec![
            ProcedureArgument {
                overload: Some(2),
                ..procedure_argument(None, 0, Some("NUMBER"), "OUT")
            },
            ProcedureArgument {
                overload: Some(2),
                ..procedure_argument(Some("B"), 1, Some("VARCHAR2"), "IN")
            },
        ];
        let listed = |overload: i32| RoutineOverload {
            overload: Some(overload),
            invocation: RoutineInvocation::Ordinary,
        };
        let definition =
            RoutineDefinition::from_dictionary(arguments.clone(), vec![listed(1), listed(2)]);

        let as_procedure = oracle_script("SQ_X_PL.DUP", "PROCEDURE", &definition);
        assert_eq!(
            as_procedure, "BEGIN\n  SQ_X_PL.DUP;\nEND;\n/\n",
            "the parameterless procedure the dictionary lists is the call, not the function's group"
        );

        // The function's own half is untouched: its group is visible and
        // matches the wanted shape.
        let as_function = oracle_script("SQ_X_PL.DUP", "FUNCTION", &definition);
        assert!(as_function.contains("B => "));
        assert!(as_function.starts_with("VAR v_result NUMBER\n"));

        // When the dictionary's overloads are exactly the ones the argument
        // rows already show, nothing is missing and the long-standing
        // first-group fallback answers for a kind with no matching shape.
        let covered = RoutineDefinition::from_dictionary(arguments, vec![listed(2)]);
        let fallback = oracle_script("SQ_X_PL.DUP", "PROCEDURE", &covered);
        assert!(
            fallback.contains("B => "),
            "no absent overload -> the pre-existing group-0 fallback stays"
        );
    }

    /// Whatever the routine WRITES has to be readable once the block ends.
    /// A local variable stops existing there, so every OUT/IN OUT argument a
    /// bind can carry is bound — the same treatment the return value and OUT
    /// ref cursors already had — and the runtime reports the bind back as
    /// `| OUT: :v_x = ...`. IN arguments are untouched: they stay locals.
    #[test]
    fn build_oracle_script_binds_out_arguments_so_their_values_show() {
        let arguments = vec![
            procedure_argument(Some("P_IN"), 1, Some("NUMBER"), "IN"),
            procedure_argument(Some("P_OUT"), 2, Some("VARCHAR2"), "OUT"),
            procedure_argument(Some("P_BOTH"), 3, Some("NUMBER"), "IN/OUT"),
        ];

        let sql = oracle_script("SQ_OUT_P", "PROCEDURE", &routine_definition(&arguments));

        // Written values: bound, and passed to the call as binds.
        assert!(sql.contains("VAR v_p_out VARCHAR2(32767)\n"));
        assert!(sql.contains("VAR v_p_both NUMBER\n"));
        assert!(sql.contains("P_OUT => :v_p_out"));
        assert!(sql.contains("P_BOTH => :v_p_both"));
        // A bind starts out empty, so the one the routine also READS is given
        // its starting value inside the block, before the call.
        assert!(sql.contains("BEGIN\n  :v_p_both := 0;\n"));
        assert!(!sql.contains(":v_p_out :="));
        // Read-only arguments keep the local declaration and its initializer.
        assert!(sql.contains("  v_p_in NUMBER := 0;\n"));
        assert!(sql.contains("P_IN => v_p_in"));
        assert!(!sql.contains("v_p_out VARCHAR2(32767);"));
    }

    /// No bind can carry a record, a collection, an object type or a
    /// `BOOLEAN`, so an OUT argument of such a type stays a local — declared
    /// bare, and passed by name, exactly as before.
    ///
    /// A regression guard, not a fail-before: it pins the side of the OUT
    /// rule that must NOT move when the bindable side does.
    #[test]
    fn build_oracle_script_keeps_unbindable_out_arguments_local() {
        let arguments = vec![
            composite_procedure_argument(
                Some("P_REC"),
                1,
                "PL/SQL RECORD",
                "OUT",
                Some("SCOTT"),
                Some("SQ_PKG"),
                Some("T_REC"),
            ),
            procedure_argument(Some("P_FLAG"), 2, Some("PL/SQL BOOLEAN"), "OUT"),
        ];

        let sql = oracle_script("SQ_LOCAL_P", "PROCEDURE", &routine_definition(&arguments));

        assert!(!sql.contains("VAR "));
        assert!(sql.contains("  v_p_rec SCOTT.SQ_PKG.T_REC;\n"));
        assert!(sql.contains("P_REC => v_p_rec"));
        assert!(sql.contains("P_FLAG => v_p_flag"));
        assert!(!sql.contains(":v_p_rec"));
    }

    /// A bind must be able to receive everything the declaration can hold: a
    /// `VARCHAR2` with no stated length is 32767 in PL/SQL, and a `RAW`
    /// reaches a character bind as HEX, two characters per byte. A bind
    /// smaller than the value assigned to it is ORA-06502 at run time.
    #[test]
    fn build_oracle_script_binds_are_wide_enough_for_the_declared_type() {
        let unbounded = vec![procedure_argument(None, 0, Some("VARCHAR2"), "OUT")];
        let sql = oracle_script("SQ_LONG_F", "FUNCTION", &routine_definition(&unbounded));
        assert!(sql.starts_with("VAR v_result VARCHAR2(32767)\n"));

        let sized = vec![ProcedureArgument {
            data_length: Some(100),
            ..procedure_argument(None, 0, Some("RAW"), "OUT")
        }];
        let sql = oracle_script("SQ_RAW_F", "FUNCTION", &routine_definition(&sized));
        assert!(sql.starts_with("VAR v_result VARCHAR2(200)\n"));
    }

    /// The catalog's REFUSAL must not be answered with a script.
    ///
    /// A parameterless call is exactly what "this routine's arguments cannot
    /// be read" rules out — the routine may take two, or not be there at all —
    /// and the delivery point used to open one anyway, because the refusal
    /// reached it on the same `Err` road a lost session takes. Of the three
    /// roads only a load that FAILED still gets the fallback; the stop road has
    /// its own test below.
    #[test]
    fn a_refused_routine_opens_nothing_while_a_failed_load_still_falls_back() {
        let refusal = "Arguments for SCOTT.ZQ_BAD could not be read: the object is INVALID, so \
                       the catalog holds no compiled signature for it. Compile it and retry.";

        assert_eq!(
            ObjectBrowserWidget::routine_script_delivery(
                DatabaseType::Oracle,
                "SCOTT.ZQ_BAD",
                "PROCEDURE",
                super::RoutineScriptLoadResult::Answered(super::RoutineScriptOutcome::Refused(
                    refusal.to_string()
                )),
            ),
            super::RoutineScriptDelivery {
                alert: Some(refusal.to_string()),
                open_sql: None,
                status: "No call script was generated for SCOTT.ZQ_BAD".to_string(),
            },
            "the catalog's answer is said, and nothing is opened"
        );

        // Same refusal, MySQL family: one rule, not one per backend.
        assert_eq!(
            ObjectBrowserWidget::routine_script_delivery(
                DatabaseType::MariaDB,
                "app.zq_bad",
                "FUNCTION",
                super::RoutineScriptLoadResult::Answered(super::RoutineScriptOutcome::Refused(
                    refusal.to_string()
                )),
            )
            .open_sql,
            None
        );

        // A load that could not ASK knows nothing about the routine, so the
        // long-standing simple-call fallback stands — per family shape.
        assert_eq!(
            ObjectBrowserWidget::routine_script_delivery(
                DatabaseType::Oracle,
                "SCOTT.ZQ_P",
                "PROCEDURE",
                super::RoutineScriptLoadResult::of(Err("connection lost".to_string())),
            ),
            super::RoutineScriptDelivery {
                // Byte-identical to the shared sentence, like its two
                // neighbours: this road used to own a hand-written text that
                // named neither the routine nor the tab it was about to open,
                // which is the one thing that made it hard to tell from a
                // refusal in a screenshot.
                alert: Some(
                    crate::db::query::result_messages::routine_script_load_failed(
                        "SCOTT.ZQ_P",
                        "connection lost"
                    )
                ),
                open_sql: Some("BEGIN\n  SCOTT.ZQ_P;\nEND;\n/\n".to_string()),
                status: "Failed to load arguments for SCOTT.ZQ_P".to_string(),
            }
        );
        assert_eq!(
            ObjectBrowserWidget::routine_script_delivery(
                DatabaseType::MySQL,
                "app.zq_p",
                "PROCEDURE",
                super::RoutineScriptLoadResult::of(Err("connection lost".to_string())),
            )
            .open_sql,
            Some("CALL `app`.`zq_p`();\n".to_string())
        );
        // An unresolved KIND has no shape to fall back to, as before.
        assert_eq!(
            ObjectBrowserWidget::routine_script_delivery(
                DatabaseType::Oracle,
                "SCOTT.PKG.R",
                "UNKNOWN",
                super::RoutineScriptLoadResult::of(Err("connection lost".to_string())),
            )
            .open_sql,
            None
        );

        // And a definition that WAS read opens its script and says nothing.
        assert_eq!(
            ObjectBrowserWidget::routine_script_delivery(
                DatabaseType::Oracle,
                "SCOTT.ZQ_P",
                "PROCEDURE",
                super::RoutineScriptLoadResult::Answered(super::RoutineScriptOutcome::Script(
                    "BEGIN NULL; END;\n".to_string()
                )),
            ),
            super::RoutineScriptDelivery {
                alert: None,
                open_sql: Some("BEGIN NULL; END;\n".to_string()),
                status: "Opened a call script for SCOTT.ZQ_P".to_string(),
            }
        );
    }

    /// Every road ENDS the status line the action started.
    ///
    /// `spawn_routine_script_load` announces `Loading … arguments for X`, the
    /// status label has no timer, and 17 unrelated writers is not a plan — so a
    /// road that says nothing at the end leaves the app claiming a load is
    /// still running. The three roads that open no tab are where it showed:
    /// the user dismissed the alert and the line still read "Loading".
    ///
    /// Asserted as a PROPERTY over all four roads rather than four literals,
    /// because the thing that must hold is "no road is silent", and a literal
    /// per road is exactly what a fifth road would not have to satisfy.
    #[test]
    fn every_routine_script_road_ends_the_status_line_it_started() {
        let roads = [
            super::RoutineScriptLoadResult::Answered(super::RoutineScriptOutcome::Script(
                "BEGIN NULL; END;\n".to_string(),
            )),
            super::RoutineScriptLoadResult::Answered(super::RoutineScriptOutcome::Refused(
                "nope".to_string(),
            )),
            super::RoutineScriptLoadResult::of(Err("ORA-01013".to_string())),
            super::RoutineScriptLoadResult::of(Err("connection lost".to_string())),
        ];
        let mut seen: Vec<String> = Vec::new();
        for road in roads {
            let delivery = ObjectBrowserWidget::routine_script_delivery(
                DatabaseType::Oracle,
                "SCOTT.ZQ_P",
                "PROCEDURE",
                road,
            );
            assert!(
                !delivery.status.trim().is_empty(),
                "a road ended without saying so: {delivery:?}"
            );
            assert!(
                delivery.status.contains("SCOTT.ZQ_P"),
                "the status must name the routine the action was about: {}",
                delivery.status
            );
            assert!(
                !delivery
                    .status
                    .to_ascii_lowercase()
                    .starts_with("loading arguments for"),
                "a terminal status must not repeat the line that says work is STILL running: {}",
                delivery.status
            );
            seen.push(delivery.status);
        }
        // Four roads, four distinct endings: a shared one would hide which
        // happened from the only line the user is left looking at.
        let mut distinct = seen.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            seen.len(),
            "roads share a status line: {seen:?}"
        );
    }

    /// The other half of the same feature ends its status line too.
    ///
    /// `Execute Routine` on an UNKNOWN package member announces `Resolving
    /// package routine type for X` and then asks the server. The UNRESOLVED
    /// half always answered; the RESOLVED half said nothing, so dismissing the
    /// menu without picking an entry left the line claiming the lookup was
    /// still running.
    #[test]
    fn the_deferred_kind_lookup_ends_its_status_line_either_way() {
        let member = |routine_type: &str| ObjectItem::PackageRoutine {
            package_name: "PKG".to_string(),
            routine_name: "myProc".to_string(),
            routine_type: routine_type.to_string(),
        };

        let resolved = member("PROCEDURE");
        assert!(ObjectBrowserWidget::package_routine_type_is_resolved(
            &resolved
        ));
        assert_eq!(
            ObjectBrowserWidget::package_routine_resolution_status(&resolved, true),
            "Resolved PKG.myProc as PROCEDURE",
            "the resolved half names the member AND the kind the menu is about to offer"
        );

        // Unresolved keeps the sentence it always had, so the failure road's
        // wording is untouched by this fix.
        let unknown = member("UNKNOWN");
        assert!(!ObjectBrowserWidget::package_routine_type_is_resolved(
            &unknown
        ));
        assert_eq!(
            ObjectBrowserWidget::package_routine_resolution_status(&unknown, false),
            "Could not resolve package routine type"
        );

        // An item shape this road cannot produce still ends the line rather
        // than claiming a resolution it did not make.
        assert_eq!(
            ObjectBrowserWidget::package_routine_resolution_status(
                &ObjectItem::Simple {
                    object_type: "PROCEDURES".to_string(),
                    object_name: "P".to_string(),
                },
                true,
            ),
            "Could not resolve package routine type"
        );
    }

    /// A load that was STOPPED opens nothing, on every backend.
    ///
    /// The app knows nothing about the routine here, exactly as on the failed
    /// road — but it was told to stop, and the parameterless call it used to
    /// hand back is the one script the readability gate exists to prevent. It
    /// was reachable two ways a user meets in practice: cancelling the activity,
    /// and a cancel TIMEOUT firing on a slow `ALL_ARGUMENTS` read. The
    /// scope-race message is the sharpest case — it says "Retry the action"
    /// while the tab it opened claimed the routine takes nothing.
    #[test]
    fn a_stopped_routine_load_opens_nothing_on_every_backend() {
        // One cancel spelling per backend, as its own driver reports it, plus
        // the app's own text. Each must reach the SAME road on the SAME rule.
        // (`DatabaseType` has no thin/OCI split — both Oracle protocols report
        // the same `ORA-` code, which is why one entry covers both.)
        let stops = [
            (
                DatabaseType::Oracle,
                "ORA-01013: user requested cancel of current operation",
            ),
            (DatabaseType::MySQL, "Query execution was interrupted"),
            (DatabaseType::MariaDB, "Query was killed"),
            (DatabaseType::Oracle, "Query cancelled by user"),
        ];
        for (db_type, reason) in stops {
            let delivery = ObjectBrowserWidget::routine_script_delivery(
                db_type,
                "SCOTT.ZQ_P",
                "PROCEDURE",
                super::RoutineScriptLoadResult::of(Err(reason.to_string())),
            );
            assert_eq!(
                delivery.open_sql, None,
                "{db_type:?} opened a script after the load was stopped: {reason}"
            );
            assert_eq!(
                delivery.alert,
                Some(
                    crate::db::query::result_messages::routine_script_load_stopped(
                        "SCOTT.ZQ_P",
                        reason
                    )
                ),
                "every backend says the one shared sentence"
            );
        }

        // An UNKNOWN kind takes the same road: there was nothing to fall back
        // to before, and there is nothing to open now either.
        assert_eq!(
            ObjectBrowserWidget::routine_script_delivery(
                DatabaseType::Oracle,
                "SCOTT.PKG.R",
                "UNKNOWN",
                super::RoutineScriptLoadResult::of(Err("ORA-01013".to_string())),
            )
            .open_sql,
            None
        );

        // The classifier is what separates the two failures, and it must not
        // swallow an ordinary one: a lost connection still reaches the
        // fallback road (asserted in full by the test above).
        assert_eq!(
            super::RoutineScriptLoadResult::of(Err("connection lost".to_string())),
            super::RoutineScriptLoadResult::Failed("connection lost".to_string())
        );
        assert_eq!(
            super::RoutineScriptLoadResult::of(Err("ORA-01013".to_string())),
            super::RoutineScriptLoadResult::Stopped("ORA-01013".to_string())
        );
    }

    /// A package member is found by the name the listing SPELLS.
    ///
    /// `"myProc"` and `MYPROC` are two routines one package may declare side
    /// by side, and both answer a case-insensitive test — so the exact match
    /// has to come first. The case-insensitive pass still has a job (an
    /// `UNKNOWN` kind can arrive from editor text the caches could not
    /// resolve, spelled as the user typed it) but must refuse when more than
    /// one member answers it, rather than settle it by listing order.
    #[test]
    fn a_package_member_resolves_by_exact_name_before_case_is_ignored() {
        let routines = vec![
            PackageRoutine {
                name: "MYPROC".to_string(),
                routine_type: "PROCEDURE".to_string(),
            },
            PackageRoutine {
                name: "myProc".to_string(),
                routine_type: "FUNCTION".to_string(),
            },
            PackageRoutine {
                name: "SOLO".to_string(),
                routine_type: "PROCEDURE".to_string(),
            },
        ];

        assert_eq!(
            ObjectBrowserWidget::resolve_listed_package_routine(&routines, "myProc", "PKG.myProc"),
            Ok(("myProc".to_string(), "FUNCTION".to_string())),
            "the quoted member is its own routine, not the upper-cased one"
        );
        assert_eq!(
            ObjectBrowserWidget::resolve_listed_package_routine(&routines, "MYPROC", "PKG.MYPROC"),
            Ok(("MYPROC".to_string(), "PROCEDURE".to_string()))
        );
        // Neither spelling: two members answer, so neither is chosen. The
        // refusal is the catalog's ANSWER, so it must arrive as `Unreadable`
        // (open nothing, say the sentence) — never as the `Err` a failed read
        // travels on, whose delivery rule owns a fallback call script.
        assert_eq!(
            ObjectBrowserWidget::resolve_listed_package_routine(&routines, "myproc", "PKG.myproc"),
            Err(RoutineScriptOutcome::Refused(
                "Could not resolve package routine type for PKG.myproc".to_string()
            )),
            "an ambiguous name must be refused, not settled by list order"
        );
        // One member answers case-insensitively: the listing's spelling wins,
        // which is what the dictionary lookup and the generated call need.
        assert_eq!(
            ObjectBrowserWidget::resolve_listed_package_routine(&routines, "solo", "PKG.solo"),
            Ok(("SOLO".to_string(), "PROCEDURE".to_string()))
        );
        assert_eq!(
            ObjectBrowserWidget::resolve_listed_package_routine(&routines, "GONE", "PKG.GONE"),
            Err(RoutineScriptOutcome::Refused(
                "Could not resolve package routine type for PKG.GONE".to_string()
            ))
        );

        // The EXACT pass has to ask the same question. A listing really can
        // hold one exact name twice with two different KINDS — the wrapped /
        // encrypted package road reads the dictionary, whose
        // `SELECT DISTINCT name, CASE ...` yields (DUP, PROCEDURE) and
        // (DUP, FUNCTION) — and picking one of those by list order is a guess.
        let cross_kind = vec![
            PackageRoutine {
                name: "DUP".to_string(),
                routine_type: "PROCEDURE".to_string(),
            },
            PackageRoutine {
                name: "DUP".to_string(),
                routine_type: "FUNCTION".to_string(),
            },
        ];
        assert_eq!(
            ObjectBrowserWidget::resolve_listed_package_routine(&cross_kind, "DUP", "PKG.DUP"),
            Err(RoutineScriptOutcome::Refused(
                "Could not resolve package routine type for PKG.DUP".to_string()
            )),
            "an exact name matching two DIFFERENT routines settles nothing either"
        );
        // Two entries that are the same routine twice still carry one answer,
        // so they must resolve. This is why the rule is "every match is the
        // same candidate" rather than "exactly one match": lists of plain
        // names (schemas, packages, tables) can repeat a name harmlessly, and
        // counting would refuse those.
        let repeated = vec![
            PackageRoutine {
                name: "DUP".to_string(),
                routine_type: "PROCEDURE".to_string(),
            },
            PackageRoutine {
                name: "DUP".to_string(),
                routine_type: "PROCEDURE".to_string(),
            },
        ];
        assert_eq!(
            ObjectBrowserWidget::resolve_listed_package_routine(&repeated, "DUP", "PKG.DUP"),
            Ok(("DUP".to_string(), "PROCEDURE".to_string()))
        );
        assert_eq!(
            ObjectBrowserWidget::selection_name_match(
                &["EMP".to_string(), "EMP".to_string()],
                "emp"
            ),
            Some("EMP".to_string()),
            "one name listed twice is one answer, not an ambiguity"
        );

        // Resolving a deferred menu item takes BOTH halves from the listed
        // row. Taking only the kind left the item naming the routine the way
        // it was asked about, and every action that follows writes the name it
        // finds on the item.
        let mut item = ObjectItem::PackageRoutine {
            package_name: "PKG".to_string(),
            routine_name: "solo".to_string(),
            routine_type: "UNKNOWN".to_string(),
        };
        ObjectBrowserWidget::apply_package_routine_type_from_routines(&mut item, &routines);
        assert_eq!(
            item,
            ObjectItem::PackageRoutine {
                package_name: "PKG".to_string(),
                routine_name: "SOLO".to_string(),
                routine_type: "PROCEDURE".to_string(),
            }
        );

        // Ambiguous: left UNKNOWN, so the action asks the server and gets the
        // refusal in words rather than acting on a guess.
        let mut ambiguous = ObjectItem::PackageRoutine {
            package_name: "PKG".to_string(),
            routine_name: "myproc".to_string(),
            routine_type: "UNKNOWN".to_string(),
        };
        ObjectBrowserWidget::apply_package_routine_type_from_routines(&mut ambiguous, &routines);
        assert_eq!(
            ambiguous,
            ObjectItem::PackageRoutine {
                package_name: "PKG".to_string(),
                routine_name: "myproc".to_string(),
                routine_type: "UNKNOWN".to_string(),
            }
        );
    }

    /// A routine only SQL can call gets a QUERY, never a PL/SQL block.
    ///
    /// A `PIPELINED` function is reachable through `TABLE(...)` in a `FROM`
    /// clause and an `AGGREGATE` function through a select list; both are
    /// PLS-00653 inside a block, which is what the builder wrote for them
    /// while it decided the shape from the argument list alone. The aggregate
    /// case is the one that proves the argument list cannot answer this: its
    /// rows are a plain `NUMBER` return and a plain `NUMBER` argument,
    /// identical to an ordinary scalar function's.
    #[test]
    fn build_oracle_script_uses_a_query_for_routines_only_sql_can_call() {
        let pipelined = vec![
            composite_procedure_argument(
                None,
                0,
                "TABLE",
                "OUT",
                Some("SCOTT"),
                Some("T_NUMS"),
                None,
            ),
            procedure_argument(Some("N"), 1, Some("NUMBER"), "IN"),
        ];
        assert_eq!(
            oracle_script(
                "SCOTT.ZQ_PIPE",
                "FUNCTION",
                &routine_definition_with_call_form(&pipelined, RoutineInvocation::Pipelined),
            ),
            "SELECT *\nFROM TABLE(\n  SCOTT.ZQ_PIPE(\n    N => 0\n  )\n);\n"
        );

        let aggregate = vec![
            procedure_argument(None, 0, Some("NUMBER"), "OUT"),
            procedure_argument(Some("INPUT"), 1, Some("NUMBER"), "IN"),
        ];
        assert_eq!(
            oracle_script(
                "SCOTT.ZQ_AGG",
                "FUNCTION",
                &routine_definition_with_call_form(&aggregate, RoutineInvocation::Aggregate),
            ),
            "SELECT\n  SCOTT.ZQ_AGG(\n    INPUT => 0\n  ) AS result\nFROM dual;\n"
        );

        // The SAME argument rows, with the dictionary saying "ordinary": the
        // block shape, untouched. This is what makes the flag — and not the
        // arguments — the thing that decides.
        let ordinary = oracle_script("SCOTT.ZQ_AGG", "FUNCTION", &routine_definition(&aggregate));
        assert!(ordinary.starts_with("VAR v_result NUMBER\n"));
        assert!(ordinary.contains("  :v_result := SCOTT.ZQ_AGG(\n"));

        // Oracle takes no parentheses on an empty argument list, in a query
        // exactly as in a block.
        let no_args = vec![composite_procedure_argument(
            None,
            0,
            "TABLE",
            "OUT",
            Some("SCOTT"),
            Some("T_NUMS"),
            None,
        )];
        assert_eq!(
            oracle_script(
                "SCOTT.ZQ_PIPE0",
                "FUNCTION",
                &routine_definition_with_call_form(&no_args, RoutineInvocation::Pipelined),
            ),
            "SELECT *\nFROM TABLE(\n  SCOTT.ZQ_PIPE0\n);\n"
        );
    }

    /// A query has nowhere to put a variable, so an OUT argument is written as
    /// a literal like every other one.
    ///
    /// PL/SQL lets a pipelined function declare an OUT parameter, and SQL
    /// cannot carry one at all — such a routine is callable from nowhere.
    /// Dropping the argument would make the list the wrong LENGTH and the
    /// server would complain about the count; writing it keeps the call
    /// complete, so the refusal names the real problem (the function has OUT
    /// arguments) instead of a made-up one.
    #[test]
    fn build_oracle_sql_scope_script_writes_every_argument_as_a_literal() {
        let arguments = vec![
            composite_procedure_argument(
                None,
                0,
                "TABLE",
                "OUT",
                Some("SCOTT"),
                Some("T_NUMS"),
                None,
            ),
            procedure_argument(Some("N"), 1, Some("NUMBER"), "IN"),
            procedure_argument(Some("O"), 2, Some("VARCHAR2"), "OUT"),
        ];

        let sql = oracle_script(
            "SCOTT.ZQ_PIPE_OUT",
            "FUNCTION",
            &routine_definition_with_call_form(&arguments, RoutineInvocation::Pipelined),
        );

        assert_eq!(
            sql,
            "SELECT *\nFROM TABLE(\n  SCOTT.ZQ_PIPE_OUT(\n    N => 0,\n    O => ''\n  )\n);\n"
        );
        // No declarations, no binds: a query has neither.
        assert!(!sql.contains("VAR "));
        assert!(!sql.contains(":v_"));
    }

    /// The statement shape is read for the overload the script is actually
    /// built for — a package may overload one name with a pipelined and an
    /// ordinary body, and `select_overload_arguments` may pick either — and
    /// EVERY invocation form the dictionary can report has a shape of its own
    /// here.
    ///
    /// The exhaustive half is the point: reading only `PIPELINED`/`AGGREGATE`
    /// is what let a SQL macro reach the PL/SQL block, where the call RUNS and
    /// reports the macro's own source text as the routine's value. A form with
    /// no arm of its own would inherit that silently.
    #[test]
    fn every_invocation_form_maps_to_its_own_oracle_statement() {
        let overloads = vec![
            RoutineOverload {
                overload: Some(1),
                invocation: RoutineInvocation::Ordinary,
            },
            RoutineOverload {
                overload: Some(2),
                invocation: RoutineInvocation::Pipelined,
            },
        ];

        assert_eq!(
            OracleRoutineScript::of(&overloads, Some(1)),
            OracleRoutineScript::PlSqlBlock
        );
        assert_eq!(
            OracleRoutineScript::of(&overloads, Some(2)),
            OracleRoutineScript::Sql(OracleSqlScopeShape::PipelinedTable)
        );
        // A `NULL` overload is its own key, not "the first row".
        assert_eq!(
            OracleRoutineScript::of(&overloads, None),
            OracleRoutineScript::PlSqlBlock
        );
        // Nothing known — every MySQL-family routine, and Oracle's fail-open
        // road — is the block, the only shape choosable without the facts.
        assert_eq!(
            OracleRoutineScript::of(&[], None),
            OracleRoutineScript::PlSqlBlock
        );

        let shape_of = |invocation| {
            OracleRoutineScript::of(
                &[RoutineOverload {
                    overload: None,
                    invocation,
                }],
                None,
            )
        };
        assert_eq!(
            shape_of(RoutineInvocation::Ordinary),
            OracleRoutineScript::PlSqlBlock
        );
        // `PIPELINED` and a TABLE macro are both rows in a `FROM` clause;
        // `AGGREGATE` and a SCALAR macro are both values in a select list.
        // All four live-proven on 23ai/26ai, parameterless forms included.
        for rows in [RoutineInvocation::Pipelined, RoutineInvocation::TableMacro] {
            assert_eq!(
                shape_of(rows),
                OracleRoutineScript::Sql(OracleSqlScopeShape::PipelinedTable),
                "{rows:?} is reachable only from a query's FROM clause"
            );
        }
        for value in [RoutineInvocation::Aggregate, RoutineInvocation::ScalarMacro] {
            assert_eq!(
                shape_of(value),
                OracleRoutineScript::Sql(OracleSqlScopeShape::Aggregate),
                "{value:?} is reachable only from a query's select list"
            );
        }
        // The one form with no statement at all: its argument is a TABLE.
        assert!(matches!(
            shape_of(RoutineInvocation::Polymorphic),
            OracleRoutineScript::Unwritable { .. }
        ));
    }

    /// A routine the dictionary describes in full can still have no script,
    /// and that answer must travel as a REFUSAL — the road that opens nothing
    /// — never as a call script the server accepts and quietly ignores.
    ///
    /// Live-proven on 26ai: the block this used to write for a polymorphic
    /// table function reported "PL/SQL procedure successfully completed" while
    /// doing nothing the user asked for.
    #[test]
    fn a_polymorphic_table_function_refuses_instead_of_writing_a_block() {
        // As `ALL_ARGUMENTS` reports one: the return row and the table
        // argument are both `DBMS_TF` records.
        let arguments = vec![
            composite_procedure_argument(
                None,
                0,
                "PL/SQL RECORD",
                "OUT",
                Some("SYS"),
                Some("DBMS_TF"),
                Some("TABLE_T"),
            ),
            composite_procedure_argument(
                Some("T"),
                1,
                "PL/SQL RECORD",
                "IN",
                Some("SYS"),
                Some("DBMS_TF"),
                Some("TABLE_T"),
            ),
        ];
        let definition =
            routine_definition_with_call_form(&arguments, RoutineInvocation::Polymorphic);

        let outcome =
            ObjectBrowserWidget::build_procedure_script("SCOTT.ZQ_PTF", "FUNCTION", &definition);
        let RoutineScriptOutcome::Refused(reason) = outcome else {
            panic!("a polymorphic table function must refuse, got {outcome:?}");
        };
        // The sentence is the shared catalog's, so all four backends read the
        // same way when they ever have a case of their own. The REMEDY moved
        // into this form's own half — the catalog used to append "write the
        // call by hand against the table it reads" to every refusal, which is
        // advice only this form can act on — so the text handed to the shared
        // function is the whole reason. What the user reads is unchanged, which
        // the literal below pins end to end.
        assert_eq!(
            reason,
            crate::db::query::result_messages::routine_call_not_writable(
                "SCOTT.ZQ_PTF",
                "it is a polymorphic table function, so it can only be called with a table \
                 argument. Write the call by hand against the table it reads.",
            )
        );
        assert_eq!(
            reason,
            "No call script can be generated for SCOTT.ZQ_PTF: it is a polymorphic table \
             function, so it can only be called with a table argument. Write the call by hand \
             against the table it reads."
        );
    }

    /// A SQL macro is spliced into the SQL that names it, so only a query sees
    /// its value.
    ///
    /// This is the case that made the shape question matter: the dictionary
    /// reports `PIPELINED = NO` and `AGGREGATE = NO` for a macro, so reading
    /// only those two produced a PL/SQL block — which RUNS, and reports the
    /// macro's own source text as the routine's value. No error, a wrong
    /// answer (live-proven on 26ai, standalone and package member alike).
    #[test]
    fn a_sql_macro_is_called_from_sql_never_from_a_block() {
        let scalar = vec![
            procedure_argument(None, 0, Some("VARCHAR2"), "OUT"),
            procedure_argument(Some("P"), 1, Some("VARCHAR2"), "IN"),
        ];
        assert_eq!(
            oracle_script(
                "SCOTT.ZQ_MACRO_S",
                "FUNCTION",
                &routine_definition_with_call_form(&scalar, RoutineInvocation::ScalarMacro),
            ),
            "SELECT\n  SCOTT.ZQ_MACRO_S(\n    P => ''\n  ) AS result\nFROM dual;\n"
        );

        let table = vec![
            procedure_argument(None, 0, Some("VARCHAR2"), "OUT"),
            procedure_argument(Some("N"), 1, Some("NUMBER"), "IN"),
        ];
        assert_eq!(
            oracle_script(
                "SCOTT.ZQ_MACRO_T",
                "FUNCTION",
                &routine_definition_with_call_form(&table, RoutineInvocation::TableMacro),
            ),
            "SELECT *\nFROM TABLE(\n  SCOTT.ZQ_MACRO_T(\n    N => 0\n  )\n);\n"
        );

        // A PARAMETERLESS table macro is the shape that pins why `TABLE(...)`
        // is the spelling: Oracle takes no parentheses on an empty argument
        // list, and `SELECT * FROM ZQ_MACRO_T0` — the bare name in a FROM
        // clause — is ORA-04044, while `TABLE(ZQ_MACRO_T0)` runs (both
        // live-proven).
        let parameterless = vec![procedure_argument(None, 0, Some("VARCHAR2"), "OUT")];
        assert_eq!(
            oracle_script(
                "SCOTT.ZQ_MACRO_T0",
                "FUNCTION",
                &routine_definition_with_call_form(&parameterless, RoutineInvocation::TableMacro),
            ),
            "SELECT *\nFROM TABLE(\n  SCOTT.ZQ_MACRO_T0\n);\n"
        );
    }

    /// The routine KIND picks the shape when there are no argument rows to
    /// read — a routine with no parameters, or an argument load that failed.
    /// Calling a function as a statement is PLS-00221, and Oracle rejects an
    /// empty argument list written with parentheses (ORA-00936) where the
    /// MySQL family requires them.
    #[test]
    fn simple_routine_scripts_follow_the_routine_kind_on_every_backend() {
        assert_eq!(
            ObjectBrowserWidget::build_simple_routine_script_for_db(
                DatabaseType::Oracle,
                "SCOTT.SQ_F",
                "FUNCTION",
            ),
            "SELECT SCOTT.SQ_F AS result\nFROM dual;\n"
        );
        assert_eq!(
            ObjectBrowserWidget::build_simple_routine_script_for_db(
                DatabaseType::Oracle,
                "SCOTT.SQ_P",
                "PROCEDURE",
            ),
            "BEGIN\n  SCOTT.SQ_P;\nEND;\n/\n"
        );
        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            assert_eq!(
                ObjectBrowserWidget::build_simple_routine_script_for_db(
                    db_type, "app.fn", "FUNCTION"
                ),
                "SELECT `app`.`fn`() AS result;\n"
            );
            assert_eq!(
                ObjectBrowserWidget::build_simple_routine_script_for_db(
                    db_type,
                    "app.p",
                    "PROCEDURE"
                ),
                "CALL `app`.`p`();\n"
            );
        }

        // The builders reach the same shapes when the argument list they were
        // handed is empty, so a FUNCTION never comes back as a call statement.
        assert_eq!(
            oracle_script("SCOTT.SQ_F", "FUNCTION", &routine_definition(&[])),
            "SELECT SCOTT.SQ_F AS result\nFROM dual;\n"
        );
        assert_eq!(
            ObjectBrowserWidget::build_mysql_routine_script(
                "app.fn",
                "FUNCTION",
                &routine_definition(&[])
            ),
            "SELECT `app`.`fn`() AS result;\n"
        );
    }

    /// `''` is not a value a JSON, ENUM or SET parameter accepts — MySQL
    /// refuses it as JSON (ER 3140) and strict mode refuses it as an
    /// ENUM/SET member — so the generated call would fail on the engine
    /// before the user ever edited it. Real string types keep `''`.
    #[test]
    fn mysql_default_arguments_are_values_the_engine_accepts() {
        let arguments = vec![
            procedure_argument(Some("p_doc"), 1, Some("json"), "IN"),
            procedure_argument(Some("p_kind"), 2, Some("enum('a','b')"), "IN"),
            procedure_argument(Some("p_tags"), 3, Some("set('x','y')"), "IN"),
            procedure_argument(Some("p_name"), 4, Some("varchar(32)"), "IN"),
            procedure_argument(Some("p_code"), 5, Some("char(3)"), "IN"),
            procedure_argument(Some("p_qty"), 6, Some("int"), "IN"),
        ];

        let sql = ObjectBrowserWidget::build_mysql_routine_script(
            "app.p",
            "PROCEDURE",
            &routine_definition(&arguments),
        );

        // Each value carries the parameter it fills. The purpose of this case
        // is the per-type LITERAL, and naming the arguments is what makes the
        // six of them readable as the six they are.
        assert_eq!(
            sql,
            "CALL `app`.`p`(\n    /* p_doc */ NULL,\n    /* p_kind */ NULL,\n    /* p_tags */ \
             NULL,\n    /* p_name */ '',\n    /* p_code */ '',\n    /* p_qty */ 0\n);\n"
        );
    }

    /// The generated variable name is not the catalog's name — it carries a
    /// `v_` prefix and may carry a `_2` disambiguator — so it has to be
    /// budgeted against the backend's identifier limit. Both limits are
    /// reachable from a legal routine: a 64-character MySQL parameter name is
    /// the identifier maximum, and `@v_<that>` is a user-variable name the
    /// server refuses (ER 3061, live-proven). The parameter's OWN name still
    /// carries the call, so truncating the local costs nothing.
    #[test]
    fn generated_variable_names_stay_within_the_backend_identifier_limit() {
        let long_name = format!("p_{}", "a".repeat(62));
        assert_eq!(long_name.len(), 64);

        let mysql = ObjectBrowserWidget::build_mysql_routine_script(
            "app.p",
            "PROCEDURE",
            &routine_definition(&[procedure_argument(Some(&long_name), 1, Some("int"), "OUT")]),
        );
        // The call argument line, whose value is now preceded by the parameter
        // name in a comment. What this case is about is the length of the
        // GENERATED name, so it is read off the line rather than assumed to be
        // the whole of it.
        let session_var = mysql
            .lines()
            .find(|line| line.starts_with("    ") && line.contains('@'))
            .and_then(|line| line.rsplit_once('@').map(|(_, name)| name))
            .expect("the OUT argument is passed as a session variable");
        assert!(
            session_var.len() <= 64,
            "user variable name is {} characters: {session_var}",
            session_var.len()
        );
        // The call still names the parameter, so the value stays identifiable.
        assert!(mysql.contains(&format!("AS `{long_name}`;")));

        // Oracle: 128 bytes, and the disambiguator has to fit too — two names
        // that truncate to the same head must still come out distinct.
        // Uppercase, as the dictionary reports ordinary parameter names: a
        // lowercase one would come back quoted and stop testing length.
        let oracle_name = format!("P_{}", "A".repeat(126));
        let second = format!("{}B", &oracle_name[..127]);
        assert_eq!(oracle_name.len(), 128);
        assert_eq!(second.len(), 128);
        let oracle = oracle_script(
            "SQ_LONG_P",
            "PROCEDURE",
            &routine_definition(&[
                procedure_argument(Some(&oracle_name), 1, Some("NUMBER"), "IN"),
                procedure_argument(Some(&second), 2, Some("NUMBER"), "IN"),
            ]),
        );
        let declared: Vec<&str> = oracle
            .lines()
            .filter_map(|line| line.trim().strip_prefix("v_"))
            .filter_map(|rest| rest.split_whitespace().next())
            .collect();
        assert_eq!(declared.len(), 2, "both arguments declare a local");
        for name in &declared {
            assert!(
                name.len() + 2 <= 128,
                "declared name is {} bytes: v_{name}",
                name.len() + 2
            );
        }
        assert_ne!(declared[0], declared[1], "truncation must stay unique");
        assert!(oracle.contains(&format!("{oracle_name} => ")));
        assert!(oracle.contains(&format!("{second} => ")));
    }

    /// Neither MySQL engine has named association, so a generated `CALL` used
    /// to say nothing at all about which value fills which parameter — while
    /// the Oracle twin has always written `NAME => value`. The name goes in a
    /// comment, which is the only place it can go.
    ///
    /// The comment must be unable to end early. `*/` inside an identifier is
    /// legal (it needs a quoted-created name) and would hand the rest of the
    /// name to the parser as SQL; a newline is legal too and would break the
    /// one-argument-per-line shape the script is read in. Both are neutralised,
    /// and the leading `/* ` — with the space — is what keeps a name starting
    /// with `!` or `+` from becoming MySQL's executable-comment or
    /// optimizer-hint form.
    #[test]
    fn a_mysql_call_names_the_parameter_each_value_fills() {
        let sql = ObjectBrowserWidget::build_mysql_routine_script(
            "app.p",
            "PROCEDURE",
            &routine_definition(&[
                procedure_argument(Some("p_id"), 1, Some("int"), "IN"),
                procedure_argument(Some("p_out"), 2, Some("int"), "OUT"),
            ]),
        );
        // IN and OUT alike: the reader should not have to know which kind an
        // argument is to find out what it is for.
        assert!(sql.contains("    /* p_id */ 0"), "{sql}");
        assert!(sql.contains("    /* p_out */ @v_p_out"), "{sql}");

        // A name that would close the comment, and one that would break the
        // line. Neither may reach the script as itself.
        assert_eq!(ObjectBrowserWidget::mysql_comment_text("a*/b"), "a* /b");
        assert_eq!(ObjectBrowserWidget::mysql_comment_text("a\nb"), "a b");
        assert_eq!(ObjectBrowserWidget::mysql_comment_text("a\r\nb"), "a  b");
        // Everything else is the catalog's spelling, untouched — that is what
        // makes the label truthful.
        for ordinary in ["p_id", "!bang", "+plus", "여러", "a`b"] {
            assert_eq!(ObjectBrowserWidget::mysql_comment_text(ordinary), ordinary);
        }
        // The space after `/*` is what keeps `!` and `+` from being read as
        // MySQL's executable comment / optimizer hint.
        assert_eq!(
            ObjectBrowserWidget::mysql_call_argument_expr("!bang", "0"),
            "/* !bang */ 0"
        );

        // A hostile name end to end: the comment closes exactly once, so the
        // value is still the value.
        let hostile = ObjectBrowserWidget::build_mysql_routine_script(
            "app.p",
            "PROCEDURE",
            &routine_definition(&[procedure_argument(Some("a*/b"), 1, Some("int"), "IN")]),
        );
        assert_eq!(hostile.matches("*/").count(), 1, "{hostile}");
        assert!(hostile.contains("/* a* /b */ 0"), "{hostile}");

        // And the script the user actually runs still splits the way it did
        // before the names went in. A comment is new TEXT inside a statement,
        // so the thing to prove is that the splitter reads it as a comment: a
        // parameter named `a;b` puts a statement terminator inside one, and a
        // splitter that did not know that would cut the CALL in half and send
        // the fragment to the server.
        for (sql, want_statements) in [
            (
                ObjectBrowserWidget::build_mysql_routine_script(
                    "app.p",
                    "PROCEDURE",
                    &routine_definition(&[
                        procedure_argument(Some("p_id"), 1, Some("int"), "IN"),
                        procedure_argument(Some("p_out"), 2, Some("int"), "OUT"),
                    ]),
                ),
                2,
            ),
            (
                ObjectBrowserWidget::build_mysql_routine_script(
                    "app.p",
                    "PROCEDURE",
                    &routine_definition(&[procedure_argument(Some("a;b"), 1, Some("int"), "IN")]),
                ),
                1,
            ),
        ] {
            assert_eq!(
                crate::db::QueryExecutor::split_script_items(&sql).len(),
                want_statements,
                "{sql}"
            );
        }
    }

    /// The read-back alias is the only thing that says WHICH parameter a
    /// value belongs to, so it has to name the parameter exactly. A parameter
    /// name may legally start or end with a backtick (doubled at creation,
    /// and the catalog reports it raw); stripping those made the column claim
    /// to be a different parameter. Ordinary names are unaffected.
    #[test]
    fn mysql_read_back_alias_names_the_parameter_exactly() {
        let sql = ObjectBrowserWidget::build_mysql_routine_script(
            "app.p",
            "PROCEDURE",
            &routine_definition(&[
                procedure_argument(Some("p_plain"), 1, Some("int"), "OUT"),
                procedure_argument(Some("`b"), 2, Some("int"), "OUT"),
                procedure_argument(Some("a`b"), 3, Some("int"), "OUT"),
            ]),
        );

        assert!(sql.contains("SELECT @v_p_plain AS `p_plain`;"));
        // A leading backtick is part of the name, not quoting to be undone.
        assert!(sql.contains("AS ```b`;"));
        assert!(sql.contains("AS `a``b`;"));
    }

    /// A name that IS an Oracle reserved word can only have been created
    /// quoted, and the dictionary hands it back as ordinary uppercase — so
    /// nothing about its shape says it needs quotes, and written bare the
    /// script does not parse (`BEGIN SYSTEM.SELECT; END;` is PLS-00103,
    /// `SELECT SYSTEM.LEVEL FROM dual` is ORA-03050). Ordinary names must
    /// stay unquoted.
    #[test]
    fn oracle_scripts_quote_reserved_word_object_names() {
        let procedure = oracle_script(
            &ObjectBrowserWidget::qualify_object_name_for_scope(
                DatabaseType::Oracle,
                Some("SCOTT"),
                "SELECT",
            ),
            "PROCEDURE",
            &routine_definition(&[]),
        );
        assert_eq!(procedure, "BEGIN\n  SCOTT.\"SELECT\";\nEND;\n/\n");

        let function = ObjectBrowserWidget::build_simple_routine_script_for_db(
            DatabaseType::Oracle,
            &ObjectBrowserWidget::qualify_object_name_for_scope(
                DatabaseType::Oracle,
                Some("SCOTT"),
                "LEVEL",
            ),
            "FUNCTION",
        );
        assert_eq!(function, "SELECT SCOTT.\"LEVEL\" AS result\nFROM dual;\n");

        // A reserved-word SCHEMA and a reserved-word package MEMBER too.
        assert_eq!(
            ObjectBrowserWidget::qualify_package_member_name(
                DatabaseType::Oracle,
                Some("PUBLIC"),
                "PKG",
                "COMMENT",
            ),
            "\"PUBLIC\".PKG.\"COMMENT\""
        );

        // Ordinary names are untouched.
        assert_eq!(
            ObjectBrowserWidget::qualify_object_name_for_scope(
                DatabaseType::Oracle,
                Some("SCOTT"),
                "SQ_PROC",
            ),
            "SCOTT.SQ_PROC"
        );
    }

    /// A cursor variable refuses `:= NULL` too: an IN ref-cursor argument is
    /// declared bare.
    #[test]
    fn build_oracle_script_leaves_in_ref_cursor_uninitialized() {
        let arguments = vec![procedure_argument(
            Some("p_cur"),
            1,
            Some("REF CURSOR"),
            "IN",
        )];

        let sql = oracle_script("SQ_CUR_PROC", "PROCEDURE", &routine_definition(&arguments));

        assert!(sql.contains("  v_p_cur SYS_REFCURSOR;\n"));
        assert!(!sql.contains(":= NULL"));
    }

    /// MariaDB accepts OUT/INOUT parameters on FUNCTIONs but refuses to call
    /// such a function from SELECT (ER 4187): the script must use the SET
    /// calling shape and still surface the OUT values.
    #[test]
    fn build_mysql_function_with_out_parameter_uses_set_call_shape() {
        let arguments = vec![
            procedure_argument(None, 0, Some("INT"), "RETURN"),
            procedure_argument(Some("p_in"), 1, Some("INT"), "IN"),
            procedure_argument(Some("p_out"), 2, Some("INT"), "OUT"),
        ];

        let sql = ObjectBrowserWidget::build_mysql_routine_script(
            "demo_fn",
            "FUNCTION",
            &routine_definition(&arguments),
        );

        assert!(sql.contains("SET @v_result = `demo_fn`(\n"));
        // The parameter each value fills, named beside it: this family has no
        // named association, so the comment is the only thing that says which
        // argument the reader is looking at.
        assert!(sql.contains("    /* p_out */ @v_p_out\n"));
        assert!(sql.contains("SELECT @v_result AS result;\n"));
        assert!(sql.contains("SELECT @v_p_out AS `p_out`;\n"));
        assert!(!sql.contains("SELECT `demo_fn`("));
    }

    #[test]
    fn build_mysql_function_without_out_parameters_keeps_select_shape() {
        let arguments = vec![
            procedure_argument(None, 0, Some("INT"), "RETURN"),
            procedure_argument(Some("p_in"), 1, Some("INT"), "IN"),
        ];

        let sql = ObjectBrowserWidget::build_mysql_routine_script(
            "demo_fn",
            "FUNCTION",
            &routine_definition(&arguments),
        );

        assert!(sql.contains("SELECT `demo_fn`(\n"));
        assert!(sql.contains(") AS result;\n"));
        assert!(!sql.contains("SET @"));
    }

    /// The TEXT each part carries, unchanged from when this test was written:
    /// punctuation trimmed, every quoting form unwrapped, a doubled delimiter
    /// read as one character, and malformed quoting refused outright.
    ///
    /// Each expectation now also states whether the part was QUOTED, which the
    /// lexer always knew and used to discard. Same texts, one more fact — the
    /// one that says whether `myProc` names `MYPROC` or `"myProc"`.
    #[test]
    fn selected_object_reference_parts_trim_sql_punctuation() {
        let bare = |text: &str| super::SelectedObjectPart {
            text: text.to_string(),
            quoted: false,
        };
        let quoted = |text: &str| super::SelectedObjectPart {
            text: text.to_string(),
            quoted: true,
        };

        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts(" (SCOTT.EMP); "),
            Some(vec![bare("SCOTT"), bare("EMP")])
        );
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts("demo_pkg.run_job()"),
            Some(vec![bare("demo_pkg"), bare("run_job")])
        );
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts("\"MixedCase\""),
            Some(vec![quoted("MixedCase")])
        );
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts(r#""Demo.Pkg""#),
            Some(vec![quoted("Demo.Pkg")])
        );
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts(r#""Sales.Ops"."Emp.Table""#),
            Some(vec![quoted("Sales.Ops"), quoted("Emp.Table")])
        );
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts("[Sales.Ops].[Emp.Table]"),
            Some(vec![quoted("Sales.Ops"), quoted("Emp.Table")])
        );
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts(r#""Emp""Name""#),
            Some(vec![quoted("Emp\"Name")])
        );
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts("`emp``name`"),
            Some(vec![quoted("emp`name")])
        );
        assert_eq!(
            ObjectBrowserWidget::selected_object_reference_parts("[Emp]]Name]"),
            Some(vec![quoted("Emp]Name")])
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

    /// A selection denotes the name the SERVER would resolve it to.
    ///
    /// This is the fact `pkg.myProc` and `pkg."myProc"` disagree on, and the
    /// reason a lookup cannot be handed the raw text: Oracle folds a bare
    /// identifier to upper case, so the first names `PKG.MYPROC` and only the
    /// second names the routine a quoted declaration created. The MySQL family
    /// folds nothing, so upper-casing there would invent a name the server
    /// never resolves — `Emp` and `emp` really can be two tables.
    #[test]
    fn a_selected_part_denotes_the_name_the_server_resolves() {
        let bare = super::SelectedObjectPart {
            text: "myProc".to_string(),
            quoted: false,
        };
        let quoted = super::SelectedObjectPart {
            text: "myProc".to_string(),
            quoted: true,
        };

        assert_eq!(bare.denoted_name(DatabaseType::Oracle), "MYPROC");
        assert_eq!(quoted.denoted_name(DatabaseType::Oracle), "myProc");
        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            assert_eq!(bare.denoted_name(db_type), "myProc", "{db_type:?}");
            assert_eq!(quoted.denoted_name(db_type), "myProc", "{db_type:?}");
        }
    }

    /// The STANDALONE half of the same rule: `Execute Procedure` on an editor
    /// selection resolves through `selection_name_match`, which used to take
    /// the first case-insensitive hit — so a schema holding `MYPROC` and
    /// `"myProc"` (two real routines) was separated by list order.
    #[test]
    fn a_standalone_routine_selection_picks_by_denoted_name_and_refuses_a_tie() {
        let names = vec!["MYPROC".to_string(), "myProc".to_string()];

        // Each spelling identifies its own routine.
        assert_eq!(
            ObjectBrowserWidget::selection_name_match(&names, "MYPROC"),
            Some("MYPROC".to_string())
        );
        assert_eq!(
            ObjectBrowserWidget::selection_name_match(&names, "myProc"),
            Some("myProc".to_string())
        );
        // A spelling that is neither is a tie, and a tie is not an answer.
        assert_eq!(
            ObjectBrowserWidget::selection_name_match(&names, "myproc"),
            None,
            "two routines answer this name; list order must not choose"
        );
        // The convenience half is untouched: one candidate, any case.
        assert_eq!(
            ObjectBrowserWidget::selection_name_match(&["EMP".to_string()], "emp"),
            Some("EMP".to_string())
        );
        assert_eq!(
            ObjectBrowserWidget::selection_name_match(&["emp".to_string()], "EMP"),
            Some("emp".to_string())
        );
    }

    /// A name that is BOTH a procedure and a function is a two-namespace fact,
    /// and the backend answers it — not the order of a candidate array.
    ///
    /// `CREATE PROCEDURE calc` beside `CREATE FUNCTION calc` is ordinary on the
    /// MySQL family and impossible on Oracle (ORA-00955, one namespace). The
    /// resolver listed PROCEDURES before FUNCTIONS and took the first hit, so a
    /// MySQL selection of `calc` in `SELECT calc(1)` always meant the
    /// PROCEDURE — the function was unreachable, and `Generate DDL` / `Drop...`
    /// took aim at the other object too. The FUNCTION now answers, which is the
    /// preference this app already applies to the same ambiguity in
    /// `get_routine_arguments_in_schema_any_kind` and
    /// `discovered_kind_for_routine`.
    #[test]
    fn a_name_in_both_routine_namespaces_resolves_the_way_the_backend_says() {
        let mut data = IntellisenseData::new();
        data.procedures = vec!["calc".to_string()];
        data.functions = vec!["calc".to_string()];
        data.rebuild_indices();

        let object_type_of = |db_type| {
            let resolved = ObjectBrowserWidget::resolve_selected_object_context(
                "calc", &data, None, db_type, None,
            )
            .expect("a routine selection should resolve");
            match resolved.item {
                ObjectItem::Simple { object_type, .. } => object_type,
                other => panic!("expected a simple object, got {other:?}"),
            }
        };

        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            assert_eq!(
                object_type_of(db_type),
                "FUNCTIONS",
                "{db_type:?} keeps two routine namespaces, so the function answers"
            );
        }

        // Oracle cannot produce the collision at all, so the order it uses is
        // unobservable — and a name that is only in ONE list is unaffected on
        // every backend, which is what keeps this change to the tie alone.
        let mut only_procedure = IntellisenseData::new();
        only_procedure.procedures = vec!["calc".to_string()];
        only_procedure.rebuild_indices();
        for db_type in [
            DatabaseType::Oracle,
            DatabaseType::MySQL,
            DatabaseType::MariaDB,
        ] {
            let resolved = ObjectBrowserWidget::resolve_selected_object_context(
                "calc",
                &only_procedure,
                None,
                db_type,
                None,
            )
            .expect("a routine selection should resolve");
            match resolved.item {
                ObjectItem::Simple { object_type, .. } => {
                    assert_eq!(object_type, "PROCEDURES", "{db_type:?}")
                }
                other => panic!("expected a simple object, got {other:?}"),
            }
        }

        // The routine pair moved into its own block, so the two BOUNDARIES it
        // sits between are pinned: everything ahead of it still wins, and it
        // still wins over everything behind it. (Only the pair's own internal
        // order was ever meant to change.)
        let mut across_blocks = IntellisenseData::new();
        across_blocks.tables = vec!["calc".to_string()];
        across_blocks.procedures = vec!["calc".to_string()];
        across_blocks.functions = vec!["calc".to_string()];
        across_blocks.packages = vec!["calc".to_string()];
        across_blocks.rebuild_indices();
        for db_type in [
            DatabaseType::Oracle,
            DatabaseType::MySQL,
            DatabaseType::MariaDB,
        ] {
            let resolved = ObjectBrowserWidget::resolve_selected_object_context(
                "calc",
                &across_blocks,
                None,
                db_type,
                None,
            )
            .expect("should resolve");
            match resolved.item {
                ObjectItem::Simple { object_type, .. } => assert_eq!(
                    object_type, "TABLES",
                    "{db_type:?}: a table still outranks both routine groups"
                ),
                other => panic!("expected a simple object, got {other:?}"),
            }
        }
        let mut routine_over_package = IntellisenseData::new();
        routine_over_package.procedures = vec!["calc".to_string()];
        routine_over_package.packages = vec!["calc".to_string()];
        routine_over_package.rebuild_indices();
        let resolved = ObjectBrowserWidget::resolve_selected_object_context(
            "calc",
            &routine_over_package,
            None,
            DatabaseType::Oracle,
            None,
        )
        .expect("should resolve");
        match resolved.item {
            ObjectItem::Simple { object_type, .. } => assert_eq!(
                object_type, "PROCEDURES",
                "a routine still outranks the groups behind it"
            ),
            other => panic!("expected a simple object, got {other:?}"),
        }

        // The order is stated per backend, not per call site: both selection
        // roads read it from here.
        assert_eq!(
            ObjectBrowserWidget::routine_selection_order(DatabaseType::Oracle),
            [
                super::RoutineSelectionGroup::Procedures,
                super::RoutineSelectionGroup::Functions
            ]
        );
        assert_eq!(
            ObjectBrowserWidget::routine_selection_order(DatabaseType::MySQL),
            [
                super::RoutineSelectionGroup::Functions,
                super::RoutineSelectionGroup::Procedures
            ]
        );
    }

    /// The whole point of carrying the quoting: a package may hold `MYPROC`
    /// and `"myProc"` at once, and only the selection's own quoting says which
    /// one `pkg.myProc` meant.
    ///
    /// Reading the raw text made both selections the same value, so the
    /// package-member reader — which needs an exact spelling precisely because
    /// both names are real — had to pick one for both.
    #[test]
    fn a_selection_picks_the_package_member_its_quoting_names() {
        let mut cache = ObjectCache::default();
        cache.packages.push("PKG".to_string());
        cache.package_routines.insert(
            "PKG".to_string(),
            vec![
                PackageRoutine {
                    name: "MYPROC".to_string(),
                    routine_type: "PROCEDURE".to_string(),
                },
                PackageRoutine {
                    name: "myProc".to_string(),
                    routine_type: "FUNCTION".to_string(),
                },
            ],
        );
        let data = IntellisenseData::new();

        let resolved = |selected: &str| {
            ObjectBrowserWidget::resolve_selected_object_context(
                selected,
                &data,
                Some(&cache),
                DatabaseType::Oracle,
                None,
            )
            .map(|context| context.item)
        };

        // Bare: Oracle folds it, so it names the upper-case routine.
        assert_eq!(
            resolved("PKG.myProc"),
            Some(ObjectItem::PackageRoutine {
                package_name: "PKG".to_string(),
                routine_name: "MYPROC".to_string(),
                routine_type: "PROCEDURE".to_string(),
            })
        );
        // Quoted: it names the routine only a quoted declaration can create.
        assert_eq!(
            resolved("PKG.\"myProc\""),
            Some(ObjectItem::PackageRoutine {
                package_name: "PKG".to_string(),
                routine_name: "myProc".to_string(),
                routine_type: "FUNCTION".to_string(),
            })
        );
        // A spelling neither denotes stays UNKNOWN rather than picking one:
        // `PKG.MYPROC` is what a bare `myproc` folds to, and it is exact.
        assert_eq!(
            resolved("PKG.myproc"),
            Some(ObjectItem::PackageRoutine {
                package_name: "PKG".to_string(),
                routine_name: "MYPROC".to_string(),
                routine_type: "PROCEDURE".to_string(),
            })
        );
        assert_eq!(
            resolved("PKG.\"nosuch\""),
            Some(ObjectItem::PackageRoutine {
                package_name: "PKG".to_string(),
                routine_name: "nosuch".to_string(),
                routine_type: "UNKNOWN".to_string(),
            })
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
        // The scope is the name the selection DENOTES, not the text it was
        // typed in: `data.users` does not hold SCOTT here, so this is the
        // fall-through, and it used to hand back the raw `scott`. That is a
        // schema Oracle does not have — `qualify_oracle_object_name` sees a
        // lowercase byte and writes `"scott".EMP`, which resolves to nothing.
        // The metadata-case path one test below is unaffected: it matches
        // case-insensitively and answers with the catalog's own spelling.
        assert_eq!(resolved.selected_scope.as_deref(), Some("SCOTT"));
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

    /// The expected SCOPE is the name `scott` denotes on Oracle. It read
    /// `scott` before, which is the un-normalised fall-through the selection
    /// resolver used to take — see
    /// `resolve_sql_selection_uses_qualified_schema_metadata`. What this test
    /// is about, the object TYPE each qualified name resolves to, is
    /// unchanged.
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
            Some("SCOTT"),
        );
        assert_resolves_simple_object(
            "scott.address_t",
            &data,
            None,
            DatabaseType::Oracle,
            None,
            "TYPES",
            Some("SCOTT"),
        );
        assert_resolves_simple_object(
            "scott.order_seq",
            &data,
            None,
            DatabaseType::Oracle,
            None,
            "SEQUENCES",
            Some("SCOTT"),
        );
        assert_resolves_simple_object(
            "scott.emp_biu",
            &data,
            None,
            DatabaseType::Oracle,
            None,
            "TRIGGERS",
            Some("SCOTT"),
        );
        assert_resolves_simple_object(
            "scott.emp_pk",
            &data,
            None,
            DatabaseType::Oracle,
            None,
            "INDEXES",
            Some("SCOTT"),
        );
        assert_resolves_simple_object(
            "scott.emp_syn",
            &data,
            None,
            DatabaseType::Oracle,
            None,
            "SYNONYMS",
            Some("SCOTT"),
        );
        // MySQL, and the expectation stays as typed on purpose: this family
        // folds no identifier, so `scott` denotes `scott` — `Scott` and
        // `scott` really can be two schemas. Only the Oracle rows above are
        // upper-cased, which is the whole per-family point.
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
                // The member name is what `calc` DENOTES on Oracle. Nothing
                // resolved it — which is this test's point, the dotted
                // literal's type must not be reused — so the name falls
                // through, and it used to fall through as the raw `calc`.
                // `quote_oracle_identifier` then wrote `PKG."calc"`, a member
                // no unquoted declaration can create.
                assert_eq!(routine_name, "CALC");
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
                // Denoted, not typed — the same fall-through as the test
                // above. The type staying UNKNOWN is what this test is for.
                assert_eq!(routine_name, "CALC");
                assert_eq!(routine_type, "UNKNOWN");
            }
            _ => panic!("expected package routine"),
        }
        assert_eq!(resolved.selected_scope.as_deref(), Some("SCOTT"));
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
