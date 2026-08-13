use mysql::prelude::*;
use oracle::{
    pool::GetMode, sql_type::OracleType, Connection, Connector, Error as OracleError,
    ErrorKind as OracleErrorKind, InitParams,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fmt;
use std::fs;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};
use tns_thin::exec::{OracleValue, StatementRequest};
use tns_thin::pool::{PoolOptions as OracleThinPoolOptions, PooledThinConnection};
use tns_thin::{ConnectTarget, OracleThinConfig, OracleThinSession, OracleThinSessionPool};

use crate::db::runtime::ConnectionId;
use crate::db::session::SessionState;
use crate::db::session_policy::{
    retained_session_state_preflight_decision, RetainedSessionPreflightAction,
    RetainedSessionPreflightDecision,
};
use crate::db::transaction::{
    RetainedSessionState, TransactionAccessMode, TransactionIsolation, TransactionMode,
    TransactionProbeResult, TransactionSessionState,
};
use crate::utils::arithmetic::safe_div;
use crate::utils::config::{
    AppConfig, DEFAULT_CONNECTION_POOL_SIZE, DEFAULT_CONNECT_TIMEOUT_SECONDS,
    MAX_CONNECTION_POOL_SIZE, MAX_CONNECT_TIMEOUT_SECONDS, MIN_CONNECTION_POOL_SIZE,
    MIN_CONNECT_TIMEOUT_SECONDS,
};
use crate::utils::logging;

pub const NOT_CONNECTED_MESSAGE: &str = "Not connected to database";
const ORACLE_CLIENT_LOAD_HELP_URL: &str =
    "https://oracle.github.io/odpi/doc/installation.html#macos";
const ORACLE_CLIENT_LIB_ENV_VAR: &str = "ORACLE_CLIENT_LIB_DIR";
const ORACLE_THIN_DESIRED_PROTOCOL_ENV_VAR: &str = "ORACLE_THIN_DESIRED_PROTOCOL";
const ORACLE_THIN_MINIMUM_PROTOCOL_ENV_VAR: &str = "ORACLE_THIN_MINIMUM_PROTOCOL";
const ORACLE_THIN_TTC_FIELD_VERSION_ENV_VAR: &str = "ORACLE_THIN_TTC_FIELD_VERSION";
const POOL_SESSION_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);
const STALE_POOL_CONTEXT_MESSAGE: &str =
    "Connection changed before a pooled session could be acquired. Retry the action.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ConnectionAttemptPolicy {
    timeout: Duration,
}

impl ConnectionAttemptPolicy {
    pub(crate) fn from_config(config: &AppConfig) -> Self {
        Self::from_seconds(config.normalized_connect_timeout_seconds())
    }

    pub(crate) fn runtime() -> Self {
        Self::from_config(&AppConfig::runtime())
    }

    pub(crate) fn from_seconds(seconds: u32) -> Self {
        Self {
            timeout: Duration::from_secs(
                seconds.clamp(MIN_CONNECT_TIMEOUT_SECONDS, MAX_CONNECT_TIMEOUT_SECONDS) as u64,
            ),
        }
    }

    pub(crate) fn timeout(self) -> Duration {
        self.timeout
    }
}

impl Default for ConnectionAttemptPolicy {
    fn default() -> Self {
        Self::from_seconds(DEFAULT_CONNECT_TIMEOUT_SECONDS)
    }
}

type ConnectionCleanupTask = Box<dyn FnOnce() + Send + 'static>;

/// Connection incarnations are numbered process-wide, so a generation names
/// one incarnation of one connection. Zero is reserved for "never connected".
static NEXT_CONNECTION_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Every lease slot that has ever held a retained session, weakly.
///
/// A retained session is the one physical session no pool can reclaim on its
/// own: the pool handed it out and the tab is holding it, so tearing the
/// connection down does not close it -- and on the MySQL family it does not
/// close the pool's IDLE sessions either, because the outstanding `PooledConn`
/// owns a clone of the pool. This registry is what lets the teardown paths
/// find those leases without every owner having to remember to hand them back.
static RETAINED_POOL_SESSION_LEASES: OnceLock<Mutex<Vec<Weak<Mutex<DbSessionLeaseSlot>>>>> =
    OnceLock::new();

fn next_connection_generation() -> u64 {
    NEXT_CONNECTION_GENERATION.fetch_add(1, Ordering::Relaxed)
}

fn lock_retained_pool_session_leases() -> MutexGuard<'static, Vec<Weak<Mutex<DbSessionLeaseSlot>>>>
{
    RETAINED_POOL_SESSION_LEASES
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|poisoned| {
            logging::log_warning(
                "db::connection",
                "retained session registry lock was poisoned; recovering",
            );
            poisoned.into_inner()
        })
}

/// Release, physically, every retained session left over from a connection
/// incarnation that has ended.
///
/// Backend-independent by construction: it goes through the same
/// `discard_physical` choke point every other discard uses, so a backend
/// cannot join the app without joining this guarantee.
fn release_retained_sessions_for_retired_connection(retired_generation: u64) -> usize {
    // Generation 0 is "never connected", so nothing can have been retained
    // under it, and matching on it would hit every lease of a fresh slot.
    if retired_generation == 0 {
        return 0;
    }
    let leases = {
        let mut registry = lock_retained_pool_session_leases();
        registry.retain(|lease| lease.strong_count() > 0);
        registry.clone()
    };
    // Collect first, discard after: closing a session talks to the server, and
    // no registry or lease lock may be held while that happens.
    let stale = leases
        .iter()
        .filter_map(|lease| lease.upgrade())
        .map(SharedDbSessionLease::from_inner)
        .filter_map(|lease| lease.take_entry_for_connection_generation(retired_generation))
        .collect::<Vec<_>>();
    let released = stale.len();
    for entry in stale {
        entry.discard_physical("db::session_lease");
    }
    released
}

/// Reclaim what a connection incarnation leaves behind.
///
/// The teardown paths run under the connection lock and closing a session does
/// network I/O, so the work happens on the cleanup worker. Two things have to
/// go, and neither can be left to whoever notices first: the sessions retained
/// under the incarnation that ended, and any cached pool context still holding
/// a clone of its pool.
fn reclaim_retired_connection_sessions_in_background(retired_generation: u64) {
    if retired_generation == 0 {
        return;
    }
    spawn_connection_cleanup(move || {
        prune_stale_pool_session_context_cache();
        let released = release_retained_sessions_for_retired_connection(retired_generation);
        if released > 0 {
            logging::log_info(
                "db::connection",
                &format!(
                    "Released {released} retained DB session(s) left by a replaced connection"
                ),
            );
        }
    });
}

static PENDING_CONNECTION_CLEANUPS: OnceLock<Mutex<Vec<ConnectionCleanupTask>>> = OnceLock::new();

fn run_connection_attempt<T, F>(
    policy: ConnectionAttemptPolicy,
    description: String,
    task: F,
) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let worker_description = description.clone();
    std::thread::Builder::new()
        .name("space-query-connection-attempt".to_string())
        .spawn(move || {
            let worker = || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task))
                    .map_err(|_| format!("{worker_description} worker terminated unexpectedly"))
                    .and_then(|result| result);
                let _ = sender.send(result);
            };
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(worker)).is_err() {
                logging::log_error(
                    "db::connection",
                    &format!("{worker_description} worker cleanup panicked"),
                );
            }
        })
        .map_err(|err| format!("{description} worker could not start: {err}"))?;

    match receiver.recv_timeout(policy.timeout()) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "{description} timed out after {} seconds",
            policy.timeout().as_secs()
        )),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err(format!("{description} worker terminated unexpectedly"))
        }
    }
}

fn lock_pending_connection_cleanups() -> MutexGuard<'static, Vec<ConnectionCleanupTask>> {
    PENDING_CONNECTION_CLEANUPS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|poisoned| {
            logging::log_warning(
                "db::connection",
                "pending connection cleanup lock was poisoned; recovering",
            );
            poisoned.into_inner()
        })
}

fn run_connection_cleanup_task(task: Arc<Mutex<Option<ConnectionCleanupTask>>>) {
    let task = task
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    let Some(task) = task else {
        return;
    };
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(task)).is_err() {
        logging::log_error("db::connection", "Connection cleanup worker panicked");
    }
}

fn try_start_connection_cleanup_with<E, F>(
    task: ConnectionCleanupTask,
    start: F,
) -> Result<(), (E, Option<ConnectionCleanupTask>)>
where
    F: FnOnce(Arc<Mutex<Option<ConnectionCleanupTask>>>) -> Result<(), E>,
{
    let task = Arc::new(Mutex::new(Some(task)));
    match start(Arc::clone(&task)) {
        Ok(()) => Ok(()),
        Err(err) => {
            let pending = task
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            Err((err, pending))
        }
    }
}

fn spawn_connection_cleanup(task: impl FnOnce() + Send + 'static) {
    let mut tasks = {
        let mut pending = lock_pending_connection_cleanups();
        let mut tasks = std::mem::take(&mut *pending);
        tasks.push(Box::new(task) as ConnectionCleanupTask);
        tasks
    };

    while let Some(task) = tasks.pop() {
        let start_result = try_start_connection_cleanup_with(task, |task| {
            std::thread::Builder::new()
                .name("space-query-connection-cleanup".to_string())
                .spawn(move || run_connection_cleanup_task(task))
                .map(|_| ())
        });
        if let Err((err, pending_task)) = start_result {
            logging::log_error(
                "db::connection",
                &format!("Failed to start connection cleanup worker: {err}"),
            );
            let mut pending = lock_pending_connection_cleanups();
            if let Some(task) = pending_task {
                pending.push(task);
            }
            pending.append(&mut tasks);
            return;
        }
    }
}

fn update_session_state_without_blocking<F>(
    session: &Arc<Mutex<SessionState>>,
    epoch_token: &Arc<AtomicU64>,
    expected_epoch: u64,
    update: F,
) where
    F: FnOnce(&mut SessionState) + Send + 'static,
{
    match session.try_lock() {
        Ok(mut guard) => update(&mut guard),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            logging::log_warning(
                "db::connection",
                "session state lock was poisoned; recovering",
            );
            update(&mut poisoned.into_inner());
        }
        Err(std::sync::TryLockError::WouldBlock) => {
            let session = Arc::clone(session);
            let epoch_token = Arc::clone(epoch_token);
            spawn_connection_cleanup(move || {
                let mut guard = session.lock().unwrap_or_else(|poisoned| {
                    logging::log_warning(
                        "db::connection",
                        "session state lock was poisoned; recovering deferred update",
                    );
                    poisoned.into_inner()
                });
                if epoch_token.load(Ordering::Acquire) == expected_epoch {
                    update(&mut guard);
                }
            });
        }
    }
}

pub(crate) fn discard_mysql_pooled_connection(conn: mysql::PooledConn) {
    // `PooledConn::unwrap()` looks like the discard API but leaks the pool
    // slot: it takes the `Conn` out, so the pool's `Drop` never runs its
    // `decrease()` and the connection stays counted as live forever. Enough
    // discards (every non-retained session takes this path) and the pool is
    // permanently "full" of ghosts — `try_get_conn` then times out with
    // "connection pool appears exhausted" while only a couple of real
    // sessions exist. The correct discard is to make the pool's own cleanup
    // fail: break the connection first, then drop the `PooledConn` normally,
    // and the crate's Drop takes its broken-connection branch, which does
    // decrement the count.
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            // Kill the socket without touching the protocol state — safe on a
            // connection in ANY state, including mid-resultset after a cancel.
            // The fd stays owned by the `Conn`, so there is no double close;
            // cleanup-for-pool then fails immediately on the dead socket.
            unsafe { libc::shutdown(conn.as_raw_fd(), libc::SHUT_RDWR) };
            drop(conn);
        }
        #[cfg(not(unix))]
        {
            // The mysql crate exposes no raw socket handle off unix. Ask the
            // server to drop us instead: KILL of the connection's own id makes
            // the server close the socket, after which cleanup-for-pool fails
            // the same way. On a mid-protocol connection the KILL write itself
            // errors (commands out of sync) and cleanup's reset then fails on
            // the desynced stream — either way the pool's count is released.
            let mut conn = conn;
            let connection_id = conn.connection_id();
            let _ =
                mysql::prelude::Queryable::query_drop(&mut conn, format!("KILL {connection_id}"));
            drop(conn);
        }
    }))
    .is_err()
    {
        logging::log_error(
            "db::connection",
            "MySQL pooled connection panicked while being discarded",
        );
    }
}

/// Route oracle_thin connect/auth phase events into the app log so the user
/// can see exactly where a connect attempt stalls (especially useful for
/// legacy protocol 314 servers where a TCP read can time out silently).
fn ensure_oracle_thin_connect_logger_installed() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = tns_thin::set_connect_phase_logger(Box::new(|phase, detail| {
            // The crate emits a phase event for every TTC round-trip, including
            // the high-frequency data-plane ones (fetch/execute/commit/...) that
            // fire on every statement and row batch. Those would flood the log,
            // so keep only the connect/auth establishment phases this logger
            // exists to diagnose.
            if is_oracle_thin_runtime_phase(phase) {
                return;
            }
            let message = if detail.is_empty() {
                phase.to_string()
            } else {
                format!("{phase} | {detail}")
            };
            logging::log_info("oracle_thin/connect", &message);
        }));
    });
}

/// True for the per-statement / per-fetch TTC phases that fire on every query,
/// row batch, commit, rollback, ping or logoff. None of these occur during the
/// connect/auth handshake, so dropping them keeps connect diagnostics intact.
fn is_oracle_thin_runtime_phase(phase: &str) -> bool {
    phase.contains("fetch")
        || phase.contains("execute")
        || phase.contains("commit")
        || phase.contains("rollback")
        || phase.contains("ping")
        || phase.contains("logoff")
}

fn apply_oracle_thin_protocol_env(config: &mut OracleThinConfig) -> Result<(), String> {
    if let Some(version) = oracle_thin_protocol_env(ORACLE_THIN_DESIRED_PROTOCOL_ENV_VAR)? {
        config.connect_options.desired_protocol_version = version;
    }
    if let Some(version) = oracle_thin_protocol_env(ORACLE_THIN_MINIMUM_PROTOCOL_ENV_VAR)? {
        config.connect_options.minimum_protocol_version = version;
    }
    if config.connect_options.minimum_protocol_version
        > config.connect_options.desired_protocol_version
    {
        return Err(format!(
            "{} ({}) cannot be greater than {} ({})",
            ORACLE_THIN_MINIMUM_PROTOCOL_ENV_VAR,
            config.connect_options.minimum_protocol_version,
            ORACLE_THIN_DESIRED_PROTOCOL_ENV_VAR,
            config.connect_options.desired_protocol_version
        ));
    }
    if let Some(version) = oracle_thin_ttc_field_version_env(ORACLE_THIN_TTC_FIELD_VERSION_ENV_VAR)?
    {
        config.connect_options.desired_ttc_field_version = Some(version);
    }
    Ok(())
}

fn apply_oracle_thin_debug_protocol(
    config: &mut OracleThinConfig,
    protocol_version: Option<u16>,
) -> Result<(), String> {
    let Some(protocol_version) = protocol_version else {
        return Ok(());
    };
    if !(314..=319).contains(&protocol_version) {
        return Err(format!(
            "Oracle Thin debug protocol version must be between 314 and 319, got {protocol_version}"
        ));
    }
    config.connect_options.desired_protocol_version = protocol_version;
    config.connect_options.minimum_protocol_version = protocol_version;
    Ok(())
}

fn oracle_thin_protocol_env(name: &str) -> Result<Option<u16>, String> {
    let Some(value) = env::var_os(name) else {
        return Ok(None);
    };
    let value = value.to_string_lossy();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse::<u16>()
        .map(Some)
        .map_err(|err| format!("invalid {name} value `{trimmed}`: {err}"))
}

fn oracle_thin_ttc_field_version_env(name: &str) -> Result<Option<u8>, String> {
    let Some(value) = env::var_os(name) else {
        return Ok(None);
    };
    let value = value.to_string_lossy();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let version = trimmed
        .parse::<u8>()
        .map_err(|err| format!("invalid {name} value `{trimmed}`: {err}"))?;
    if !(6..=24).contains(&version) {
        return Err(format!("{name} must be between 6 and 24, got {version}"));
    }
    Ok(Some(version))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DatabaseType {
    #[default]
    Oracle,
    MySQL,
    MariaDB,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqlDialect {
    Oracle,
    MySql,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatabaseBackendKind {
    Oracle,
    MySql,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionSslMode {
    #[default]
    Disabled,
    Required,
    VerifyCa,
    VerifyIdentity,
}

/// The colour a saved connection is tagged with, so the window says which
/// database is on the other end before a statement runs.
///
/// This is a client-side label, not a session setting: it never reaches the
/// server and it survives switching the connection's database type.
///
/// There is no blue: `theme::selection_soft()` already means "selected" here,
/// so a blue tag stops reading as a tag on any surface the UI paints as chosen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum ConnectionColor {
    #[default]
    None,
    Red,
    Orange,
    Yellow,
    Green,
    Purple,
    Gray,
}

/// A tag saved by a build that offered a colour this one does not loads as
/// `None`, so one retired colour cannot make a saved connection unreadable.
impl<'de> Deserialize<'de> for ConnectionColor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let label = String::deserialize(deserializer)?;
        Ok(Self::from_label(&label).unwrap_or_default())
    }
}

impl ConnectionColor {
    /// Every colour in menu order, `None` first.
    pub const ALL: [Self; 7] = [
        Self::None,
        Self::Red,
        Self::Orange,
        Self::Yellow,
        Self::Green,
        Self::Purple,
        Self::Gray,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Red => "Red",
            Self::Orange => "Orange",
            Self::Yellow => "Yellow",
            Self::Green => "Green",
            Self::Purple => "Purple",
            Self::Gray => "Gray",
        }
    }

    /// The 24-bit value the UI paints with, or `None` for an untagged
    /// connection, which keeps whatever colour it had before.
    ///
    /// The tones are picked to stay legible on the dark palette; the widgets
    /// that use them are in `src/ui/theme.rs`.
    pub fn rgb(self) -> Option<(u8, u8, u8)> {
        match self {
            Self::None => None,
            Self::Red => Some((0xE8, 0x64, 0x64)),
            Self::Orange => Some((0xE8, 0x95, 0x40)),
            Self::Yellow => Some((0xE0, 0xC2, 0x4A)),
            Self::Green => Some((0x5C, 0xC2, 0x7A)),
            Self::Purple => Some((0xA9, 0x7B, 0xE0)),
            Self::Gray => Some((0x9A, 0xA0, 0xA8)),
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.label() == label)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OracleNetworkProtocol {
    #[default]
    Tcp,
    Tcps,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OracleDriverMode {
    #[default]
    Oci,
    Thin,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionAdvancedSettings {
    #[serde(default)]
    pub ssl_mode: ConnectionSslMode,
    #[serde(default = "ConnectionAdvancedSettings::default_transaction_isolation")]
    pub default_transaction_isolation: TransactionIsolation,
    #[serde(default)]
    pub default_transaction_access_mode: TransactionAccessMode,
    #[serde(default)]
    pub session_time_zone: String,
    #[serde(default = "ConnectionAdvancedSettings::default_mysql_sql_mode")]
    pub mysql_sql_mode: String,
    #[serde(default = "ConnectionAdvancedSettings::default_mysql_charset")]
    pub mysql_charset: String,
    #[serde(default)]
    pub mysql_collation: String,
    #[serde(default)]
    pub mysql_ssl_ca_path: String,
    #[serde(default)]
    pub oracle_protocol: OracleNetworkProtocol,
    #[serde(default)]
    pub oracle_driver_mode: OracleDriverMode,
    #[serde(default = "ConnectionAdvancedSettings::default_oracle_nls_date_format")]
    pub oracle_nls_date_format: String,
    #[serde(default = "ConnectionAdvancedSettings::default_oracle_nls_timestamp_format")]
    pub oracle_nls_timestamp_format: String,
}

impl ConnectionSslMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disabled => "Disabled",
            Self::Required => "Required",
            Self::VerifyCa => "Verify CA",
            Self::VerifyIdentity => "Verify identity",
        }
    }
}

impl OracleNetworkProtocol {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tcp => "TCP",
            Self::Tcps => "TCPS",
        }
    }
}

impl OracleDriverMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Oci => "OCI",
            Self::Thin => "Thin",
        }
    }

    pub fn is_thin(self) -> bool {
        matches!(self, Self::Thin)
    }
}

impl ConnectionAdvancedSettings {
    fn default_transaction_isolation() -> TransactionIsolation {
        TransactionIsolation::ReadCommitted
    }

    fn default_mysql_sql_mode() -> String {
        "TRADITIONAL".to_string()
    }

    fn default_mysql_charset() -> String {
        "utf8mb4".to_string()
    }

    fn default_oracle_nls_date_format() -> String {
        "yyyy-mm-dd hh24:mi:ss".to_string()
    }

    fn default_oracle_nls_timestamp_format() -> String {
        "yyyy-mm-dd hh24:mi:ss.ff6".to_string()
    }

    pub fn default_for(db_type: DatabaseType) -> Self {
        backend_for(db_type).default_advanced_settings()
    }

    /// Produce a settings value appropriate for `new_db_type` while keeping
    /// cross-database fields the user has already customized (isolation,
    /// access mode, SSL mode, time zone). DB-specific fields fall back to
    /// the defaults for `new_db_type` because the `self` value holds fields
    /// for the other backend.
    pub fn migrate_for_db_type(
        &self,
        previous_db_type: DatabaseType,
        new_db_type: DatabaseType,
    ) -> Self {
        if previous_db_type.is_same_type_as(new_db_type) {
            return self.clone();
        }

        let mut settings = Self::default_for(new_db_type);
        let previous_defaults = Self::default_for(previous_db_type);

        if self.default_transaction_isolation != previous_defaults.default_transaction_isolation
            && new_db_type
                .supported_transaction_isolations()
                .contains(&self.default_transaction_isolation)
        {
            settings.default_transaction_isolation = self.default_transaction_isolation;
        }
        if self.default_transaction_access_mode != previous_defaults.default_transaction_access_mode
        {
            settings.default_transaction_access_mode = self.default_transaction_access_mode;
        }
        if self.session_time_zone != previous_defaults.session_time_zone
            && validate_session_time_zone_for_db(new_db_type, self.session_time_zone.trim()).is_ok()
        {
            settings.session_time_zone = self.session_time_zone.clone();
        }

        if self.ssl_mode != previous_defaults.ssl_mode {
            settings.ssl_mode = new_db_type.normalize_ssl_mode(self.ssl_mode);
        }

        settings
    }

    pub fn validate_for_db(
        &self,
        db_type: DatabaseType,
        using_tns_alias: bool,
    ) -> Result<(), String> {
        if !db_type
            .supported_transaction_isolations()
            .contains(&self.default_transaction_isolation)
        {
            return Err(format!(
                "{} does not support {} as a default transaction isolation",
                db_type,
                self.default_transaction_isolation.label()
            ));
        }

        if !self.session_time_zone.trim().is_empty() {
            validate_session_time_zone_for_db(db_type, self.session_time_zone.trim())?;
        }

        backend_for(db_type).validate_advanced_settings(self, using_tns_alias)
    }

    fn validate_oracle(&self, using_tns_alias: bool) -> Result<(), String> {
        if self.oracle_driver_mode == OracleDriverMode::Thin {
            if using_tns_alias {
                return Err(
                    "Oracle Thin currently supports Host + Port + Service connections only"
                        .to_string(),
                );
            }
            if self.oracle_effective_protocol() == OracleNetworkProtocol::Tcps {
                return Err("Oracle Thin currently supports TCP only".to_string());
            }
        }
        if !using_tns_alias
            && matches!(
                self.ssl_mode,
                ConnectionSslMode::VerifyCa | ConnectionSslMode::VerifyIdentity
            )
        {
            return Err(
                "Oracle SSL certificate verification is not configured in this dialog; use Required/TCPS or configure verification through a TNS alias"
                    .to_string(),
            );
        }
        if let Some(reason) = DatabaseConnection::transaction_mode_selection_error(
            DatabaseType::Oracle,
            TransactionMode::new(
                self.default_transaction_isolation,
                self.default_transaction_access_mode,
            ),
        ) {
            return Err(reason);
        }
        validate_oracle_nls_format("Oracle NLS date format", self.oracle_nls_date_format.trim())?;
        validate_oracle_nls_format(
            "Oracle NLS timestamp format",
            self.oracle_nls_timestamp_format.trim(),
        )?;
        Ok(())
    }

    fn validate_mysql(&self) -> Result<(), String> {
        let charset = self.mysql_charset.trim();
        let collation = self.mysql_collation.trim();
        validate_mysql_sql_mode(self.mysql_sql_mode.trim())?;
        validate_mysql_identifier("MySQL character set", charset, false)?;
        validate_mysql_identifier("MySQL collation", collation, true)?;
        if !collation.is_empty() && !mysql_collation_matches_charset(collation, charset) {
            return Err(format!(
                "MySQL collation `{collation}` does not match character set `{charset}`"
            ));
        }
        Ok(())
    }

    fn oracle_effective_protocol(&self) -> OracleNetworkProtocol {
        if self.ssl_mode == ConnectionSslMode::Disabled {
            self.oracle_protocol
        } else {
            OracleNetworkProtocol::Tcps
        }
    }
}

impl Default for ConnectionAdvancedSettings {
    fn default() -> Self {
        Self {
            ssl_mode: ConnectionSslMode::Disabled,
            default_transaction_isolation: Self::default_transaction_isolation(),
            default_transaction_access_mode: TransactionAccessMode::ReadWrite,
            session_time_zone: String::new(),
            mysql_sql_mode: Self::default_mysql_sql_mode(),
            mysql_charset: Self::default_mysql_charset(),
            mysql_collation: String::new(),
            mysql_ssl_ca_path: String::new(),
            oracle_protocol: OracleNetworkProtocol::Tcp,
            oracle_driver_mode: OracleDriverMode::Oci,
            oracle_nls_date_format: Self::default_oracle_nls_date_format(),
            oracle_nls_timestamp_format: Self::default_oracle_nls_timestamp_format(),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SessionTimeZoneOffset {
    sign: u8,
    hour: u8,
    minute: u8,
}

fn parse_session_time_zone_offset(value: &str) -> Option<SessionTimeZoneOffset> {
    let bytes = value.as_bytes();
    if bytes.len() != 6 || !matches!(bytes[0], b'+' | b'-') || bytes[3] != b':' {
        return None;
    }
    let hour = value[1..3].parse::<u8>().ok()?;
    let minute = value[4..6].parse::<u8>().ok()?;
    if minute > 59 {
        return None;
    }
    Some(SessionTimeZoneOffset {
        sign: bytes[0],
        hour,
        minute,
    })
}

fn oracle_session_time_zone_in_range(offset: SessionTimeZoneOffset) -> bool {
    match offset.sign {
        b'+' => offset.hour < 14 || (offset.hour == 14 && offset.minute == 0),
        b'-' => offset.hour < 12 || (offset.hour == 12 && offset.minute == 0),
        _ => false,
    }
}

fn mysql_session_time_zone_in_range(offset: SessionTimeZoneOffset) -> bool {
    match offset.sign {
        b'+' => offset.hour < 14 || (offset.hour == 14 && offset.minute == 0),
        b'-' => offset.hour < 14,
        _ => false,
    }
}

fn mariadb_session_time_zone_in_range(offset: SessionTimeZoneOffset) -> bool {
    match offset.sign {
        b'+' => offset.hour < 13 || (offset.hour == 13 && offset.minute == 0),
        b'-' => offset.hour < 13,
        _ => false,
    }
}

fn validate_session_time_zone_for_db(db_type: DatabaseType, value: &str) -> Result<(), String> {
    backend_for(db_type).validate_session_time_zone(value)
}

fn validate_oracle_nls_format(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{label} is required"));
    }
    if !value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(byte, b' ' | b':' | b'.' | b'-' | b'_' | b'/' | b',' | b';')
    }) {
        return Err(format!("{label} contains invalid characters"));
    }
    Ok(())
}

fn validate_mysql_sql_mode(value: &str) -> Result<(), String> {
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b',' | b'_'))
    {
        return Err("MySQL sql_mode contains invalid characters".to_string());
    }
    Ok(())
}

fn validate_mysql_identifier(label: &str, value: &str, allow_empty: bool) -> Result<(), String> {
    if value.is_empty() {
        return if allow_empty {
            Ok(())
        } else {
            Err(format!("{label} is required"))
        };
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(format!("{label} contains invalid characters"));
    }
    Ok(())
}

fn mysql_collation_matches_charset(collation: &str, charset: &str) -> bool {
    let collation = collation.to_ascii_lowercase();
    let charset = charset.to_ascii_lowercase();
    if collation.starts_with(&format!("{charset}_")) {
        return true;
    }
    if charset == "binary" && collation == "binary" {
        return true;
    }

    matches!(charset.as_str(), "utf8" | "utf8mb3")
        && (collation.starts_with("utf8_") || collation.starts_with("utf8mb3_"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DbConnectionFormSpec {
    pub show_driver_mode: bool,
    pub service_name_form_label: &'static str,
    pub service_name_value_label: &'static str,
    pub service_name_required: bool,
    pub default_host: &'static str,
    pub default_port: u16,
    pub default_service_name: &'static str,
    pub supports_tns_alias: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DbAdvancedSettingsFormSpec {
    pub show_oracle_protocol: bool,
    pub show_oracle_nls_formats: bool,
    pub show_mysql_session_options: bool,
    pub show_mysql_ssl_ca_path: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbTableBrowsePagination {
    Rownum,
    LimitOffset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DbTableBrowseSpec {
    pub pagination: DbTableBrowsePagination,
    pub strips_page_helper_column: bool,
}

impl DatabaseType {
    pub const ALL: [Self; 3] = [Self::Oracle, Self::MySQL, Self::MariaDB];

    pub fn supported() -> &'static [Self] {
        &Self::ALL
    }

    pub fn choice_label(self) -> &'static str {
        backend_for(self).choice_label()
    }

    pub fn display_name(self) -> &'static str {
        backend_for(self).display_name()
    }

    pub fn connection_form_spec(self) -> DbConnectionFormSpec {
        backend_for(self).connection_form_spec()
    }

    pub fn advanced_settings_form_spec(self) -> DbAdvancedSettingsFormSpec {
        backend_for(self).advanced_settings_form_spec()
    }

    pub fn table_browse_spec(self) -> DbTableBrowseSpec {
        backend_for(self).table_browse_spec()
    }

    pub fn sorts_nulls_last_ascending(self) -> bool {
        backend_for(self).sorts_nulls_last_ascending()
    }

    pub fn supports_tns_alias(self) -> bool {
        self.connection_form_spec().supports_tns_alias
    }

    pub fn supported_transaction_isolations(self) -> &'static [TransactionIsolation] {
        backend_for(self).supported_transaction_isolations()
    }

    pub(crate) fn transaction_isolation_choice_labels(
        self,
        default_isolation: Option<TransactionIsolation>,
    ) -> String {
        self.supported_transaction_isolations()
            .iter()
            .map(|isolation| match default_isolation {
                Some(default_isolation)
                    if *isolation == TransactionIsolation::Default
                        && default_isolation != TransactionIsolation::Default =>
                {
                    format!("Default ({})", default_isolation.label())
                }
                _ => isolation.label().to_string(),
            })
            .collect::<Vec<_>>()
            .join("|")
    }

    pub(crate) fn transaction_isolation_from_choice_index(
        self,
        index: i32,
        fallback: TransactionIsolation,
    ) -> TransactionIsolation {
        self.supported_transaction_isolations()
            .get(index.max(0) as usize)
            .copied()
            .unwrap_or(fallback)
    }

    pub(crate) fn choice_index_from_transaction_isolation(
        self,
        isolation: TransactionIsolation,
        fallback: TransactionIsolation,
    ) -> i32 {
        self.supported_transaction_isolations()
            .iter()
            .position(|candidate| *candidate == isolation)
            .or_else(|| {
                self.supported_transaction_isolations()
                    .iter()
                    .position(|candidate| *candidate == fallback)
            })
            .unwrap_or_default() as i32
    }

    pub fn transaction_mode_requires_first_statement(self, mode: TransactionMode) -> bool {
        backend_for(self).transaction_mode_requires_first_statement(mode)
    }

    fn fallback_default_transaction_isolation(self) -> TransactionIsolation {
        backend_for(self).fallback_default_transaction_isolation()
    }

    pub fn sql_dialect(self) -> SqlDialect {
        backend_for(self).sql_dialect()
    }

    pub(crate) fn supports_mysql_delimiter_commands(self) -> bool {
        backend_for(self).supports_mysql_delimiter_commands()
    }

    pub(crate) fn supports_explicit_analytic_null_treatment(self) -> bool {
        backend_for(self).supports_explicit_analytic_null_treatment()
    }

    pub(crate) fn uses_mysql_analytic_null_treatment_rules(self) -> bool {
        backend_for(self).uses_mysql_analytic_null_treatment_rules()
    }

    pub(crate) fn supports_trailing_select_into_after_set_limit(self) -> bool {
        backend_for(self).supports_trailing_select_into_after_set_limit()
    }

    pub(crate) fn preserves_quoted_routine_lookup_spelling(self) -> bool {
        backend_for(self).preserves_quoted_routine_lookup_spelling()
    }

    pub fn backend_kind(self) -> DatabaseBackendKind {
        backend_for(self).backend_kind()
    }

    pub fn cache_key(self) -> u8 {
        backend_for(self).cache_key()
    }

    pub(crate) fn has_connection_scope(self) -> bool {
        backend_for(self).has_connection_scope()
    }

    pub(crate) fn can_apply_empty_scope_to_retained_session(self) -> bool {
        backend_for(self).can_apply_empty_scope_to_retained_session()
    }

    pub(crate) fn retained_session_blocks_transaction_mode_change(
        self,
        retained_state: RetainedSessionState,
    ) -> bool {
        backend_for(self).retained_session_blocks_transaction_mode_change(retained_state)
    }

    pub(crate) fn can_replace_retained_transaction_mode(
        self,
        retained_state: RetainedSessionState,
    ) -> bool {
        backend_for(self).can_replace_retained_transaction_mode(retained_state)
    }

    pub(crate) fn scope_values_match(self, left: Option<&str>, right: Option<&str>) -> bool {
        backend_for(self).scope_values_match(left, right)
    }

    pub(crate) fn metadata_refresh_activity(self, requested_scope: Option<&str>) -> String {
        backend_for(self).metadata_refresh_activity(requested_scope)
    }

    pub(crate) fn metadata_refresh_activity_with_base(
        self,
        base_activity: &str,
        requested_scope: Option<&str>,
    ) -> String {
        backend_for(self).metadata_refresh_activity_with_base(base_activity, requested_scope)
    }

    pub(crate) fn scope_switch_activity_message(self, target_scope: &str) -> String {
        backend_for(self).scope_switch_activity_message(target_scope)
    }

    pub(crate) fn scope_switch_failure_message(self, target_scope: &str, err: &str) -> String {
        backend_for(self).scope_switch_failure_message(target_scope, err)
    }

    pub(crate) fn ssl_choice_labels(self) -> String {
        backend_for(self)
            .supported_ssl_choices()
            .iter()
            .map(|(_, label)| *label)
            .collect::<Vec<_>>()
            .join("|")
    }

    pub(crate) fn ssl_mode_from_choice_index(self, idx: i32) -> ConnectionSslMode {
        let choices = backend_for(self).supported_ssl_choices();
        usize::try_from(idx)
            .ok()
            .and_then(|idx| choices.get(idx))
            .map(|(mode, _)| *mode)
            .unwrap_or(ConnectionSslMode::Disabled)
    }

    pub(crate) fn choice_index_from_ssl_mode(self, mode: ConnectionSslMode) -> i32 {
        let normalized = self.normalize_ssl_mode(mode);
        backend_for(self)
            .supported_ssl_choices()
            .iter()
            .position(|(choice, _)| *choice == normalized)
            .map(|idx| idx as i32)
            .unwrap_or_default()
    }

    pub(crate) fn normalize_ssl_mode(self, mode: ConnectionSslMode) -> ConnectionSslMode {
        backend_for(self).normalize_ssl_mode(mode)
    }

    pub(crate) fn is_recoverable_timeout_message(self, trimmed: &str, lower: &str) -> bool {
        backend_for(self).is_recoverable_timeout_message(trimmed, lower)
    }

    pub(crate) fn is_same_type_as(self, expected: Self) -> bool {
        self == expected
    }

    pub(crate) fn is_mysql_or_mariadb(self) -> bool {
        self == Self::MySQL || self == Self::MariaDB
    }

    pub fn from_cache_key(raw: u8) -> Self {
        Self::supported()
            .iter()
            .copied()
            .find(|db_type| db_type.cache_key() == raw)
            .unwrap_or_default()
    }
}

impl fmt::Display for DatabaseType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", backend_for(*self).display_name())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ConnectionInfo {
    pub name: String,
    pub username: String,
    #[serde(skip_serializing)]
    pub password: String,
    pub host: String,
    pub port: u16,
    pub service_name: String,
    pub db_type: DatabaseType,
    pub advanced: ConnectionAdvancedSettings,
    /// Client-side tag, not a session setting — see [`ConnectionColor`].
    #[serde(default)]
    pub color: ConnectionColor,
    /// When set, the application refuses to send anything that writes over this
    /// connection. It is a guard in this process, not a server-side lock.
    #[serde(default)]
    pub read_only: bool,
    #[serde(skip)]
    pub debug_oracle_thin_protocol_version: Option<u16>,
}

#[derive(Deserialize)]
struct ConnectionInfoSerde {
    name: String,
    username: String,
    #[serde(default)]
    password: String,
    host: String,
    port: u16,
    service_name: String,
    #[serde(default)]
    db_type: DatabaseType,
    advanced: Option<ConnectionAdvancedSettingsPatch>,
    // `ConnectionInfo` deserialises through this struct, so a field missing
    // here is a field silently dropped on load no matter what the real struct
    // says.
    #[serde(default)]
    color: ConnectionColor,
    #[serde(default)]
    read_only: bool,
}

#[derive(Default, Deserialize)]
struct ConnectionAdvancedSettingsPatch {
    ssl_mode: Option<ConnectionSslMode>,
    default_transaction_isolation: Option<TransactionIsolation>,
    default_transaction_access_mode: Option<TransactionAccessMode>,
    session_time_zone: Option<String>,
    mysql_sql_mode: Option<String>,
    mysql_charset: Option<String>,
    mysql_collation: Option<String>,
    mysql_ssl_ca_path: Option<String>,
    oracle_protocol: Option<OracleNetworkProtocol>,
    oracle_driver_mode: Option<OracleDriverMode>,
    oracle_nls_date_format: Option<String>,
    oracle_nls_timestamp_format: Option<String>,
}

impl ConnectionAdvancedSettings {
    fn default_for_with_patch(
        db_type: DatabaseType,
        patch: Option<ConnectionAdvancedSettingsPatch>,
    ) -> Self {
        let mut settings = Self::default_for(db_type);
        let Some(patch) = patch else {
            return settings;
        };

        if let Some(value) = patch.ssl_mode {
            settings.ssl_mode = value;
        }
        if let Some(value) = patch.default_transaction_isolation {
            settings.default_transaction_isolation = value;
        }
        if let Some(value) = patch.default_transaction_access_mode {
            settings.default_transaction_access_mode = value;
        }
        if let Some(value) = patch.session_time_zone {
            settings.session_time_zone = value;
        }
        if let Some(value) = patch.mysql_sql_mode {
            settings.mysql_sql_mode = value;
        }
        if let Some(value) = patch.mysql_charset {
            settings.mysql_charset = value;
        }
        if let Some(value) = patch.mysql_collation {
            settings.mysql_collation = value;
        }
        if let Some(value) = patch.mysql_ssl_ca_path {
            settings.mysql_ssl_ca_path = value;
        }
        if let Some(value) = patch.oracle_protocol {
            settings.oracle_protocol = value;
        }
        if let Some(value) = patch.oracle_driver_mode {
            settings.oracle_driver_mode = value;
        }
        if let Some(value) = patch.oracle_nls_date_format {
            settings.oracle_nls_date_format = value;
        }
        if let Some(value) = patch.oracle_nls_timestamp_format {
            settings.oracle_nls_timestamp_format = value;
        }
        settings
    }
}

impl<'de> Deserialize<'de> for ConnectionInfo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let fields = ConnectionInfoSerde::deserialize(deserializer)?;
        Ok(Self {
            name: fields.name,
            username: fields.username,
            password: fields.password,
            host: fields.host,
            port: fields.port,
            service_name: fields.service_name,
            db_type: fields.db_type,
            advanced: ConnectionAdvancedSettings::default_for_with_patch(
                fields.db_type,
                fields.advanced,
            ),
            color: fields.color,
            read_only: fields.read_only,
            debug_oracle_thin_protocol_version: None,
        })
    }
}

impl ConnectionInfo {
    pub fn uses_oracle_tns_alias(&self) -> bool {
        self.db_type.supports_tns_alias()
            && self.host.trim().is_empty()
            && !self.service_name.trim().is_empty()
    }

    pub(crate) fn clear_secret(secret: &mut String) {
        // Overwrite the secret bytes with zeros before releasing the allocation.
        // SAFETY: 0x00 bytes are valid UTF-8 code points, so the String's UTF-8
        // invariant is preserved during zeroing. We immediately clear and shrink the
        // Vec to release the underlying allocation that held the secret.
        let vec = unsafe { secret.as_mut_vec() };
        for b in vec.iter_mut() {
            // SAFETY: `b` is a unique mutable reference to one initialized byte
            // in `vec`; its derived pointer is valid and aligned for a `u8`
            // volatile write for the duration of this iteration.
            unsafe { std::ptr::write_volatile(b as *mut u8, 0) };
        }
        vec.clear();
        vec.shrink_to_fit();
    }

    pub fn new(
        name: &str,
        username: &str,
        password: &str,
        host: &str,
        port: u16,
        service_name: &str,
    ) -> Self {
        let db_type = DatabaseType::default();
        Self {
            name: name.to_string(),
            username: username.to_string(),
            password: password.to_string(),
            host: host.to_string(),
            port,
            service_name: service_name.to_string(),
            db_type,
            advanced: ConnectionAdvancedSettings::default_for(db_type),
            color: ConnectionColor::default(),
            read_only: false,
            debug_oracle_thin_protocol_version: None,
        }
    }

    pub fn new_with_type(
        name: &str,
        username: &str,
        password: &str,
        host: &str,
        port: u16,
        service_name: &str,
        db_type: DatabaseType,
    ) -> Self {
        Self {
            name: name.to_string(),
            username: username.to_string(),
            password: password.to_string(),
            host: host.to_string(),
            port,
            service_name: service_name.to_string(),
            db_type,
            advanced: ConnectionAdvancedSettings::default_for(db_type),
            color: ConnectionColor::default(),
            read_only: false,
            debug_oracle_thin_protocol_version: None,
        }
    }

    pub fn connection_string(&self) -> String {
        backend_for(self.db_type).connection_string(self)
    }

    fn connection_attempt_description(&self, action: &str) -> String {
        let endpoint = if self.uses_oracle_tns_alias() {
            self.service_name.trim().to_string()
        } else {
            let service = self.service_name.trim();
            if service.is_empty() {
                format!("{}:{}", self.host, self.port)
            } else {
                format!("{}:{}/{}", self.host, self.port, service)
            }
        };
        format!("{} {} connection to {}", action, self.db_type, endpoint)
    }

    pub fn default_for(db_type: DatabaseType) -> Self {
        backend_for(db_type).default_connection_info()
    }

    /// The label used for the service_name field depending on database type.
    pub fn service_name_label(&self) -> &'static str {
        backend_for(self.db_type).service_name_label()
    }

    /// Securely clear the password from memory by overwriting with zeros
    /// then releasing the allocation.
    pub fn clear_password(&mut self) {
        Self::clear_secret(&mut self.password);
    }
}

impl Default for ConnectionInfo {
    fn default() -> Self {
        Self::default_for(DatabaseType::default())
    }
}

pub enum DbConnection {
    Oracle(Arc<Connection>),
    OracleThin(Arc<Mutex<OracleThinSession>>),
    MySQL {
        conn: mysql::Conn,
        db_type: DatabaseType,
    },
}

impl DbConnection {
    pub fn db_type(&self) -> DatabaseType {
        match self {
            DbConnection::Oracle(_) | DbConnection::OracleThin(_) => DatabaseType::Oracle,
            DbConnection::MySQL { db_type, .. } => *db_type,
        }
    }
}

#[derive(Clone)]
pub enum DbConnectionPool {
    Oracle {
        pool: oracle::pool::Pool,
        advanced: ConnectionAdvancedSettings,
    },
    OracleThin {
        pool: Arc<OracleThinSessionPool>,
        advanced: ConnectionAdvancedSettings,
    },
    MySQL {
        pool: mysql::Pool,
        advanced: ConnectionAdvancedSettings,
        db_type: DatabaseType,
    },
}

pub enum DbPoolSession {
    Oracle(Arc<Connection>),
    OracleThin(Box<PooledThinConnection<OracleThinSession>>),
    MySQL {
        conn: mysql::PooledConn,
        db_type: DatabaseType,
    },
}

pub enum DbSessionLease {
    Oracle(Arc<Connection>),
    OracleThin(Box<PooledThinConnection<OracleThinSession>>),
    MySQL {
        conn: mysql::PooledConn,
        db_type: DatabaseType,
    },
}

pub struct DbSessionLeaseEntry {
    connection_generation: u64,
    pool_context_epoch: u64,
    /// `None` only while the session is being handed out or discarded. It is
    /// an `Option` so this entry can own a `Drop` that closes a session nobody
    /// took responsibility for -- see that impl.
    lease: Option<DbSessionLease>,
    retained_state: RetainedSessionState,
    current_scope: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetainedSessionDisposition {
    Retain(RetainedSessionState),
    DiscardPhysical,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetainedSessionMutationOutcome {
    NoSession,
    Applied,
    AppliedWithWarning(String),
    DiscardedBecauseStale,
    BlockedRequiresResolution(String),
    FailedRestored(String),
    FailedDiscarded(String),
}

impl RetainedSessionMutationOutcome {
    pub fn message(&self) -> Option<&str> {
        match self {
            Self::AppliedWithWarning(message)
            | Self::BlockedRequiresResolution(message)
            | Self::FailedRestored(message)
            | Self::FailedDiscarded(message) => Some(message.as_str()),
            Self::NoSession | Self::Applied | Self::DiscardedBecauseStale => None,
        }
    }

    pub fn should_alert_user(&self) -> bool {
        !matches!(self, Self::NoSession | Self::Applied)
    }

    pub fn status_label(&self) -> &'static str {
        match self {
            Self::NoSession => "no retained session",
            Self::Applied => "applied",
            Self::AppliedWithWarning(_) => "applied with cleanup warning",
            Self::DiscardedBecauseStale => "discarded stale retained session",
            Self::BlockedRequiresResolution(_) => "blocked pending resolution",
            Self::FailedRestored(_) => "failed and restored",
            Self::FailedDiscarded(_) => "failed and discarded",
        }
    }
}

impl RetainedSessionTakeOutcome {
    /// Move the taken session's cancel registration somewhere that outlives the
    /// frame that took it, so the session stays cancelable while it is in use.
    pub fn hold_cancel_registration_in(&mut self, holder: &impl HoldsSessionCancelRegistration) {
        if let RetainedSessionTakeOutcome::Reusable(taken) = self {
            if let Some(registration) = taken.cancel_registration.take() {
                holder.hold_session_registration(registration);
            }
        }
    }

    /// Same, for the common case where the holder is optional and the outcome
    /// is being matched on directly.
    #[must_use]
    pub fn held_in(mut self, holder: Option<&impl HoldsSessionCancelRegistration>) -> Self {
        if let Some(holder) = holder {
            self.hold_cancel_registration_in(holder);
        }
        self
    }
}

/// Somewhere a session's cancel registration can live for as long as the work
/// using that session runs.
pub trait HoldsSessionCancelRegistration {
    fn hold_session_registration(&self, registration: DbSessionCancelRegistration);
}

pub enum RetainedSessionTakeOutcome {
    NoSession,
    Reusable(Box<TakenDbSessionLease>),
    DiscardedBecauseStale,
    BlockedContextMismatch(RetainedSessionState),
}

/// What one lease slot holds, plus whether its owner still exists.
///
/// `closed` is the difference between "empty because idle" and "empty because
/// the tab is gone". Work that outlives its tab (a statement whose cancel was
/// requested but never landed) hands its session back through the same store
/// path a live tab uses, and an `Option` alone cannot refuse it — the session
/// would sit in a slot nobody will ever clear again, holding a server session
/// for as long as any clone of the slot exists. Live-observed on Oracle thin:
/// a `DBMS_SESSION.SLEEP` outlasting its cancelled tab came back healthy,
/// was retained into the closed slot, and survived every teardown.
#[derive(Default)]
struct DbSessionLeaseSlot {
    entry: Option<DbSessionLeaseEntry>,
    closed: bool,
}

/// One editor tab's owned DB session slot.
///
/// Oracle and MySQL/MariaDB both use this same lifecycle: take the lease for
/// execution, retain it in the tab slot after cleanup, and clear it on close,
/// disconnect, cancel, or stale connection generation.
#[derive(Clone, Default)]
pub struct SharedDbSessionLease {
    inner: Arc<Mutex<DbSessionLeaseSlot>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RetainedLeaseConflictResolution {
    KeepExisting,
    ReplaceExisting,
    KeepExistingMarkedInvalid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RetainedLeaseContextDecision {
    Reusable,
    BlockContextMismatch,
}

pub struct TakenDbSessionLease {
    owner: SharedDbSessionLease,
    connection_generation: u64,
    pool_context_epoch: u64,
    lease: Option<DbSessionLease>,
    retained_state: RetainedSessionState,
    current_scope: Option<String>,
    /// Keeps this session reachable by the cancel button while the caller is
    /// using it; detaches when the lease is handed back.
    cancel_registration: Option<DbSessionCancelRegistration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PooledSessionLeaseSnapshot {
    pub db_type: DatabaseType,
    pub pool_context_epoch: u64,
    pub transaction_state: TransactionSessionState,
    pub retained_state: RetainedSessionState,
    pub current_scope: Option<String>,
}

impl PooledSessionLeaseSnapshot {
    pub fn transaction_state(&self) -> TransactionSessionState {
        self.transaction_state
    }

    pub fn retained_state(&self) -> RetainedSessionState {
        self.retained_state
    }

    pub fn current_scope(&self) -> Option<&str> {
        self.current_scope.as_deref()
    }
}

#[derive(Clone)]
pub struct DbPoolSessionContext {
    pub connection_generation: u64,
    pub connection_id: Option<ConnectionId>,
    pub connection_info: ConnectionInfo,
    pub pool: DbConnectionPool,
    pub connection_pool_size: u32,
    pub current_service_name: String,
    pub oracle_current_schema: Option<String>,
    pub auto_commit: bool,
    pub transaction_mode: TransactionMode,
    pub default_transaction_isolation: TransactionIsolation,
    cache_epoch: u64,
    cache_epoch_token: Arc<AtomicU64>,
}

impl DbPoolSessionContext {
    pub fn pool_context_epoch(&self) -> u64 {
        self.cache_epoch
    }

    /// The lifetime an activity running on this context should be bound to, so
    /// the registry retires it once this connection's sessions are gone.
    pub fn activity_lifetime(&self) -> DbActivityLifetime {
        DbActivityLifetime {
            epoch_token: Arc::clone(&self.cache_epoch_token),
            epoch: self.cache_epoch,
        }
    }

    fn cache_epoch_is_current(&self) -> bool {
        self.cache_epoch_token.load(Ordering::Acquire) == self.cache_epoch
    }

    pub fn is_current(&self) -> bool {
        self.cache_epoch_is_current()
    }

    pub fn ensure_current(&self) -> Result<(), String> {
        if self.cache_epoch_is_current() {
            Ok(())
        } else {
            Err(STALE_POOL_CONTEXT_MESSAGE.to_string())
        }
    }

    /// Acquire a session for this context's scope.
    ///
    /// The activity guard is required, not optional: binding the session to a
    /// tracked activity here is what makes the app-wide guarantees hold. Every
    /// acquired session is therefore visible in the status bar, reachable by
    /// the cancel button, and retired by the stale sweep when its connection
    /// goes away — with no way for a new call site to opt out.
    pub fn acquire_session_for_current_scope(
        &self,
        activity: &DbActivityGuard,
    ) -> Result<(DbPoolSession, DbSessionCancelRegistration), String> {
        self.acquire_session_with_scope_context(self, activity)
    }

    pub fn acquire_session_for_scope(
        &self,
        scope: Option<&str>,
        activity: &DbActivityGuard,
    ) -> Result<(DbPoolSession, DbSessionCancelRegistration), String> {
        let scoped = self.for_scope(scope);
        self.acquire_session_with_scope_context(&scoped, activity)
    }

    pub fn for_scope(&self, scope: Option<&str>) -> Self {
        let mut scoped = self.clone();
        let scope = scope.map(str::trim).filter(|scope| !scope.is_empty());
        if self.connection_info.db_type.is_mysql_or_mariadb() {
            scoped.current_service_name = scope
                .unwrap_or(self.connection_info.service_name.trim())
                .to_string();
        } else {
            // Resolved to a concrete schema, never left empty: pooled
            // sessions are recycled between query tabs, and applying "no
            // schema" is a no-op — so a tab with no scope of its own would
            // keep whichever schema the previous tab left on the session it
            // just picked up. The MySQL branch above has always been total
            // for the same reason.
            scoped.oracle_current_schema =
                oracle_session_schema(scope, self.oracle_current_schema.as_deref());
        }
        scoped
    }

    fn acquire_session_with_scope_context(
        &self,
        scope_context: &DbPoolSessionContext,
        activity: &DbActivityGuard,
    ) -> Result<(DbPoolSession, DbSessionCancelRegistration), String> {
        self.ensure_current()?;
        // Tie the activity to this connection before the session exists, so a
        // teardown that lands mid-acquire still retires this work.
        activity.bind_lifetime(self.activity_lifetime());
        let (mut session, registration) =
            self.pool.acquire_session(&self.connection_info, activity)?;
        if let Err(err) = self.ensure_current() {
            Self::discard_stale_session(session);
            return Err(err);
        }
        if let Err(err) = scope_context.apply_current_scope_to_session(&mut session) {
            Self::discard_stale_session(session);
            return Err(err);
        }
        if let Err(err) = self.ensure_current() {
            Self::discard_stale_session(session);
            return Err(err);
        }
        Ok((session, registration))
    }

    pub fn apply_current_scope_to_session(
        &self,
        session: &mut DbPoolSession,
    ) -> Result<(), String> {
        backend_for(self.connection_info.db_type).apply_current_scope_to_session(self, session)
    }

    /// Throw away a session that was acquired but could not be handed over.
    ///
    /// Either the connection it came from is already gone, or applying the
    /// scope to it failed part way, so what the session carries is unknown.
    /// Every backend gets rid of it the same way, through the one discard
    /// choke point: returning a half-configured session to the pool would hand
    /// the next caller state nobody has accounted for, and on a connection
    /// that is being retired it would keep the pool -- and on the MySQL family
    /// every idle session in it -- alive for as long as something holds it.
    fn discard_stale_session(session: DbPoolSession) {
        session.into_lease().discard_physical("db::pool_session");
    }
}

/// Whether an Oracle force close failed only because the call was already gone.
pub fn oracle_force_close_already_completed(error: &OracleError) -> bool {
    if matches!(error.dpi_code(), Some(1002 | 1010 | 1080))
        || matches!(error.oci_code(), Some(3113 | 3114 | 3135))
    {
        return true;
    }
    crate::db::session_policy::message_indicates_connection_loss(&error.to_string())
}

/// Cancels whatever call a pooled session is currently blocked in.
///
/// Lives in the DB layer so session acquisition can build one without the UI:
/// a canceler the UI had to supply would be a canceler a call site could
/// forget.
enum PoolSessionCanceler {
    Oracle {
        conn: Arc<Connection>,
        /// Drop-close is only valid for a session checked out of a session
        /// pool; on the main connection ODPI-C rejects it with DPI-1011.
        from_pool: bool,
    },
    OracleThin(tns_thin::OracleThinCancelHandle),
    MySql {
        connection_info: Box<ConnectionInfo>,
        connection_id: u32,
        db_type: DatabaseType,
    },
}

impl Drop for PoolSessionCanceler {
    fn drop(&mut self) {
        if let PoolSessionCanceler::MySql {
            connection_info, ..
        } = self
        {
            connection_info.clear_password();
        }
    }
}

/// A canceler for work running on the main connection rather than a pooled
/// session — scope switches, commits, `ALTER SESSION`, health checks.
///
/// These hold the connection mutex while they block, so leaving them
/// uncancelable would leave the whole connection wedged behind them.
fn main_connection_canceler(
    connection: &DatabaseConnection,
) -> Option<Arc<dyn DbActivityCanceler>> {
    let info = connection.get_info();
    Some(Arc::new(match connection.get_db_connection()? {
        DbConnection::Oracle(conn) => PoolSessionCanceler::Oracle {
            conn,
            from_pool: false,
        },
        DbConnection::OracleThin(session) => {
            // try_lock: the session mutex is held for the duration of a call,
            // and blocking here would deadlock the very work we want to be able
            // to cancel. A busy session simply stays uncancelable this round.
            let session = session.try_lock().ok()?;
            PoolSessionCanceler::OracleThin(session.cancel_handle())
        }
        DbConnection::MySQL { conn, db_type } => PoolSessionCanceler::MySql {
            connection_info: Box::new(info.clone()),
            connection_id: conn.connection_id(),
            db_type,
        },
    }))
}

/// The canceler for a session a tab is holding on to.
///
/// Retained sessions never go through `acquire_session`, so without this the
/// work that runs on them — including the ROLLBACK / SET autocommit / COM_INIT_DB
/// round trips that prepare them for reuse — would have nothing to cancel.
fn session_lease_canceler(
    lease: &DbSessionLease,
    connection_info: &ConnectionInfo,
) -> Arc<dyn DbActivityCanceler> {
    Arc::new(match lease {
        DbSessionLease::Oracle(conn) => PoolSessionCanceler::Oracle {
            conn: Arc::clone(conn),
            from_pool: true,
        },
        DbSessionLease::OracleThin(conn) => {
            conn.reset_pending_cancel();
            PoolSessionCanceler::OracleThin(conn.cancel_handle())
        }
        DbSessionLease::MySQL { conn, db_type } => PoolSessionCanceler::MySql {
            connection_info: Box::new(connection_info.clone()),
            connection_id: conn.connection_id(),
            db_type: *db_type,
        },
    })
}

fn pool_session_canceler(
    session: &DbPoolSession,
    connection_info: &ConnectionInfo,
) -> Arc<dyn DbActivityCanceler> {
    Arc::new(match session {
        DbPoolSession::Oracle(conn) => PoolSessionCanceler::Oracle {
            conn: Arc::clone(conn),
            from_pool: true,
        },
        DbPoolSession::OracleThin(conn) => {
            // A pooled session can still carry a cancel that was queued but
            // never delivered on an earlier call; clear it so this caller is
            // not broken by someone else's cancel.
            conn.reset_pending_cancel();
            PoolSessionCanceler::OracleThin(conn.cancel_handle())
        }
        DbPoolSession::MySQL { conn, db_type } => PoolSessionCanceler::MySql {
            connection_info: Box::new(connection_info.clone()),
            connection_id: conn.connection_id(),
            db_type: *db_type,
        },
    })
}

impl DbActivityCanceler for PoolSessionCanceler {
    fn interrupt(&self) -> Result<(), String> {
        match self {
            PoolSessionCanceler::Oracle { conn, .. } => {
                conn.break_execution().map_err(|err| err.to_string())
            }
            PoolSessionCanceler::OracleThin(handle) => {
                handle.break_execution().map_err(|err| err.to_string())
            }
            PoolSessionCanceler::MySql {
                connection_info,
                connection_id,
                ..
            } => crate::db::query::mysql_executor::MysqlExecutor::cancel_running_query(
                connection_info,
                *connection_id,
            )
            .map_err(|err| err.to_string()),
        }
    }

    fn force(&self) -> Result<(), String> {
        match self {
            PoolSessionCanceler::Oracle { conn, from_pool } => {
                // The main connection has no drop-close: ODPI-C only accepts it
                // for pool-checked-out sessions. Re-breaking is the strongest
                // tier available there, and it is not a failure to report.
                if !from_pool {
                    return conn.break_execution().map_err(|err| err.to_string());
                }
                match conn.close_with_mode(oracle::conn::CloseMode::Drop) {
                    Ok(()) => Ok(()),
                    Err(error) if oracle_force_close_already_completed(&error) => Ok(()),
                    Err(error) => Err(format!("Oracle force close failed: {error}")),
                }
            }
            PoolSessionCanceler::OracleThin(handle) => handle
                .force_close()
                .map_err(|err| format!("Oracle thin force close failed: {err}")),
            PoolSessionCanceler::MySql {
                connection_info,
                connection_id,
                ..
            } => crate::db::query::mysql_executor::MysqlExecutor::cancel_connection(
                connection_info,
                *connection_id,
            )
            .map_err(|err| format!("MySQL KILL CONNECTION {connection_id} failed: {err}")),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            PoolSessionCanceler::Oracle { .. } => "Oracle",
            PoolSessionCanceler::OracleThin(_) => "Oracle thin",
            PoolSessionCanceler::MySql { db_type, .. } => db_type.display_name(),
        }
    }
}

impl DbConnectionPool {
    /// Acquire a pooled session under a tracked activity.
    ///
    /// This is the only way to get a pooled session anywhere in the app, and it
    /// always publishes the session to the activity registry. That is what
    /// makes the guarantees total rather than per-call-site: work the status
    /// bar cannot show, the cancel button cannot reach, or a teardown cannot
    /// retire is not expressible.
    pub fn acquire_session(
        &self,
        connection_info: &ConnectionInfo,
        activity: &DbActivityGuard,
    ) -> Result<(DbPoolSession, DbSessionCancelRegistration), String> {
        let session = self.acquire_session_untracked()?;
        let registration =
            activity.attach_canceler(pool_session_canceler(&session, connection_info));
        Ok((session, registration))
    }

    fn acquire_session_untracked(&self) -> Result<DbPoolSession, String> {
        let mut session = match self {
            DbConnectionPool::Oracle { pool, .. } => DbPoolSession::Oracle(Arc::new(
                pool.get()
                    .map_err(|err| Self::format_oracle_pool_acquire_error(pool, &err))?,
            )),
            DbConnectionPool::OracleThin { pool, .. } => DbPoolSession::OracleThin(Box::new(
                pool.acquire()
                    .map_err(|err| Self::format_oracle_thin_pool_acquire_error(&err))?,
            )),
            DbConnectionPool::MySQL { pool, db_type, .. } => DbPoolSession::MySQL {
                conn: pool
                    .try_get_conn(POOL_SESSION_ACQUIRE_TIMEOUT)
                    .map_err(|err| Self::format_mysql_pool_acquire_error(*db_type, &err))?,
                db_type: *db_type,
            },
        };
        backend_for(self.db_type()).apply_pool_session_settings(&mut session, self.advanced())?;
        Ok(session)
    }

    pub fn db_type(&self) -> DatabaseType {
        match self {
            DbConnectionPool::Oracle { .. } | DbConnectionPool::OracleThin { .. } => {
                DatabaseType::Oracle
            }
            DbConnectionPool::MySQL { db_type, .. } => *db_type,
        }
    }

    fn advanced(&self) -> &ConnectionAdvancedSettings {
        match self {
            DbConnectionPool::Oracle { advanced, .. }
            | DbConnectionPool::OracleThin { advanced, .. }
            | DbConnectionPool::MySQL { advanced, .. } => advanced,
        }
    }

    fn format_oracle_pool_acquire_error(pool: &oracle::pool::Pool, err: &OracleError) -> String {
        let message = err.to_string();
        let lower = message.to_ascii_lowercase();
        let looks_pool_exhausted = lower.contains("ora-24418")
            || lower.contains("ora-24496")
            || lower.contains("ocisessionget timed out")
            || lower.contains("waiting for pool")
            || lower.contains("connection pool");
        if !looks_pool_exhausted {
            return message;
        }

        let pool_counts = match (pool.busy_count(), pool.open_count()) {
            (Ok(busy), Ok(open)) => format!(" busy/open sessions: {busy}/{open}."),
            _ => String::new(),
        };

        format!(
            "{}. Oracle session pool appears exhausted.{} Finish or cancel lazy fetches in other result tabs, close unused query tabs, or increase Settings > Connection pool size.",
            message, pool_counts
        )
    }

    fn format_mysql_pool_acquire_error(db_type: DatabaseType, err: &mysql::Error) -> String {
        let message = err.to_string();
        let looks_pool_exhausted =
            matches!(err, mysql::Error::DriverError(mysql::DriverError::Timeout));
        if !looks_pool_exhausted {
            return message;
        }

        format!(
            "{}. {} connection pool appears exhausted. Finish or cancel lazy fetches in other result tabs, close unused query tabs, or increase Settings > Connection pool size.",
            message, db_type
        )
    }

    fn format_oracle_thin_pool_acquire_error(err: &tns_thin::OracleThinError) -> String {
        let message = err.to_string();
        if !message
            .to_ascii_lowercase()
            .contains("timed out waiting for a pooled oracle thin connection")
        {
            return message;
        }

        format!(
            "{}. Oracle thin connection pool appears exhausted. Finish or cancel lazy fetches in other result tabs, close unused query tabs, or increase Settings > Connection pool size.",
            message
        )
    }

    fn close(&self) {
        match self {
            DbConnectionPool::Oracle { .. } | DbConnectionPool::MySQL { .. } => {}
            DbConnectionPool::OracleThin { pool, .. } => {
                pool.close();
            }
        }
    }
}

impl DbPoolSession {
    pub fn db_type(&self) -> DatabaseType {
        match self {
            DbPoolSession::Oracle(_) | DbPoolSession::OracleThin(_) => DatabaseType::Oracle,
            DbPoolSession::MySQL { db_type, .. } => *db_type,
        }
    }

    pub fn is_db_type(&self, expected: DatabaseType) -> bool {
        self.db_type().is_same_type_as(expected)
    }

    pub fn ensure_db_type(self, expected: DatabaseType) -> Result<Self, String> {
        if self.is_db_type(expected) {
            Ok(self)
        } else {
            Err(format!(
                "Expected {} pool session but acquired {}",
                expected,
                self.db_type()
            ))
        }
    }

    pub fn into_lease(self) -> DbSessionLease {
        match self {
            DbPoolSession::Oracle(conn) => DbSessionLease::Oracle(conn),
            DbPoolSession::OracleThin(conn) => DbSessionLease::OracleThin(conn),
            DbPoolSession::MySQL { conn, db_type } => DbSessionLease::MySQL { conn, db_type },
        }
    }
}

impl DbSessionLease {
    pub fn db_type(&self) -> DatabaseType {
        match self {
            DbSessionLease::Oracle(_) | DbSessionLease::OracleThin(_) => DatabaseType::Oracle,
            DbSessionLease::MySQL { db_type, .. } => *db_type,
        }
    }

    pub fn is_db_type(&self, expected: DatabaseType) -> bool {
        self.db_type().is_same_type_as(expected)
    }

    pub fn into_oracle_connection(self) -> Option<Arc<Connection>> {
        match self {
            DbSessionLease::Oracle(conn) => Some(conn),
            DbSessionLease::OracleThin(_) | DbSessionLease::MySQL { .. } => None,
        }
    }

    pub fn into_oracle_thin_connection(self) -> Option<PooledThinConnection<OracleThinSession>> {
        match self {
            DbSessionLease::OracleThin(conn) => Some(*conn),
            DbSessionLease::Oracle(_) | DbSessionLease::MySQL { .. } => None,
        }
    }

    pub fn into_mysql_connection(self) -> Option<mysql::PooledConn> {
        match self {
            DbSessionLease::MySQL { conn, .. } => Some(conn),
            DbSessionLease::Oracle(_) | DbSessionLease::OracleThin(_) => None,
        }
    }

    pub fn apply_scope(
        &mut self,
        db_type: DatabaseType,
        target_scope: &str,
        advanced: &ConnectionAdvancedSettings,
        preserve_existing_session_state: bool,
    ) -> Result<(), String> {
        backend_for(db_type).apply_scope_to_lease(
            self,
            target_scope,
            advanced,
            preserve_existing_session_state,
        )
    }

    pub fn discard_physical(self, log_context: &str) {
        match self {
            DbSessionLease::Oracle(conn) => {
                if let Err(err) = conn.close_with_mode(oracle::conn::CloseMode::Drop) {
                    logging::log_warning(
                        log_context,
                        &format!("Failed to drop Oracle pooled session from pool: {err}"),
                    );
                }
            }
            DbSessionLease::OracleThin(conn) => {
                let mut conn = *conn;
                conn.mark_broken();
                conn.discard();
            }
            DbSessionLease::MySQL { conn, .. } => {
                discard_mysql_pooled_connection(conn);
            }
        }
    }
}

impl TakenDbSessionLease {
    fn new_with_retained_state(
        owner: SharedDbSessionLease,
        connection_generation: u64,
        pool_context_epoch: u64,
        lease: DbSessionLease,
        retained_state: RetainedSessionState,
        current_scope: Option<String>,
    ) -> Self {
        Self {
            owner,
            connection_generation,
            pool_context_epoch,
            lease: Some(lease),
            retained_state,
            current_scope,
            cancel_registration: None,
        }
    }

    /// Publish the retained session so a cancel can break whatever the caller
    /// is about to run on it.
    fn track_under(mut self, connection_info: &ConnectionInfo, activity: &DbActivityGuard) -> Self {
        if let Some(lease) = self.lease.as_ref() {
            self.cancel_registration =
                Some(activity.attach_canceler(session_lease_canceler(lease, connection_info)));
        }
        self
    }

    pub fn retained_state(&self) -> RetainedSessionState {
        self.retained_state
    }

    pub fn pool_context_epoch(&self) -> u64 {
        self.pool_context_epoch
    }

    pub fn current_scope(&self) -> Option<&str> {
        self.current_scope.as_deref()
    }

    pub fn lease_mut(&mut self) -> Option<&mut DbSessionLease> {
        self.lease.as_mut()
    }

    pub fn into_lease_with_retained_state(
        mut self,
    ) -> Option<(DbSessionLease, RetainedSessionState)> {
        self.lease.take().map(|lease| (lease, self.retained_state))
    }

    pub fn into_oracle_connection_with_retained_state(
        mut self,
    ) -> Option<(Arc<Connection>, RetainedSessionState)> {
        self.lease.take().and_then(|lease| {
            lease
                .into_oracle_connection()
                .map(|conn| (conn, self.retained_state))
        })
    }

    pub fn into_mysql_connection_with_retained_state(
        mut self,
    ) -> Option<(mysql::PooledConn, RetainedSessionState)> {
        self.lease.take().and_then(|lease| {
            lease
                .into_mysql_connection()
                .map(|conn| (conn, self.retained_state))
        })
    }

    pub fn into_oracle_thin_connection_with_retained_state(
        mut self,
    ) -> Option<(
        PooledThinConnection<OracleThinSession>,
        RetainedSessionState,
    )> {
        self.lease.take().and_then(|lease| {
            lease
                .into_oracle_thin_connection()
                .map(|conn| (conn, self.retained_state))
        })
    }

    pub fn restore(self) -> bool {
        let retained_state = self.retained_state;
        self.restore_with_retained_state(retained_state)
    }

    pub fn restore_with_retained_state(mut self, retained_state: RetainedSessionState) -> bool {
        if let Some(lease) = self.lease.take() {
            self.owner.apply_retained_session_disposition_with_scope(
                self.connection_generation,
                self.pool_context_epoch,
                lease,
                RetainedSessionDisposition::Retain(retained_state),
                "db::session_lease",
                self.current_scope.clone(),
            )
        } else {
            false
        }
    }

    pub fn restore_with_context_epoch(
        mut self,
        pool_context_epoch: u64,
        retained_state: RetainedSessionState,
    ) -> bool {
        if let Some(lease) = self.lease.take() {
            self.owner.apply_retained_session_disposition_with_scope(
                self.connection_generation,
                pool_context_epoch,
                lease,
                RetainedSessionDisposition::Retain(retained_state),
                "db::session_lease",
                self.current_scope.clone(),
            )
        } else {
            false
        }
    }

    pub fn restore_with_context_epoch_and_scope(
        mut self,
        pool_context_epoch: u64,
        retained_state: RetainedSessionState,
        current_scope: Option<String>,
    ) -> bool {
        if let Some(lease) = self.lease.take() {
            self.owner.apply_retained_session_disposition_with_scope(
                self.connection_generation,
                pool_context_epoch,
                lease,
                RetainedSessionDisposition::Retain(retained_state),
                "db::session_lease",
                current_scope,
            )
        } else {
            false
        }
    }

    pub fn discard(mut self) {
        if let Some(lease) = self.lease.take() {
            let _ = self.owner.apply_retained_session_disposition(
                self.connection_generation,
                self.pool_context_epoch,
                lease,
                RetainedSessionDisposition::DiscardPhysical,
                "db::session_lease",
            );
        }
    }
}

impl Drop for TakenDbSessionLease {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            let _ = self.owner.apply_retained_session_disposition(
                self.connection_generation,
                self.pool_context_epoch,
                lease,
                RetainedSessionDisposition::DiscardPhysical,
                "db::session_lease",
            );
        }
    }
}

impl DbSessionLeaseEntry {
    fn new_with_retained_state(
        connection_generation: u64,
        pool_context_epoch: u64,
        lease: DbSessionLease,
        retained_state: RetainedSessionState,
        current_scope: Option<String>,
    ) -> Self {
        Self {
            connection_generation,
            pool_context_epoch,
            lease: Some(lease),
            retained_state,
            current_scope,
        }
    }

    fn lease(&self) -> Option<&DbSessionLease> {
        self.lease.as_ref()
    }

    fn take_lease(&mut self) -> Option<DbSessionLease> {
        self.lease.take()
    }

    fn matches_connection(&self, connection_generation: u64, db_type: DatabaseType) -> bool {
        self.connection_generation == connection_generation
            && self.lease().is_some_and(|lease| lease.is_db_type(db_type))
    }

    fn matches_context(
        &self,
        connection_generation: u64,
        pool_context_epoch: u64,
        db_type: DatabaseType,
    ) -> bool {
        self.matches_connection(connection_generation, db_type)
            && self.pool_context_epoch == pool_context_epoch
    }

    fn discard_physical(mut self, log_context: &str) {
        if let Some(lease) = self.lease.take() {
            lease.discard_physical(log_context);
        }
    }
}

/// The last resort for an orphaned session.
///
/// A retained session belongs to a query tab, and every ordinary path hands it
/// back deliberately -- the tab closes it, a teardown releases it, a reuse
/// takes it. If a slot is dropped with a session still in it, there is nobody
/// left to do any of that: the tab is gone, so the session would drift back
/// into the pool carrying whatever transaction, temporary table or lock it was
/// holding, and on the MySQL family it would keep the pool it came from alive
/// with it. Closing it here means no session can outlive its owner, on any
/// backend, however the owner went away.
impl Drop for DbSessionLeaseEntry {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            logging::log_info(
                "db::session_lease",
                "Closing a retained DB session whose owner is gone",
            );
            lease.discard_physical("db::session_lease");
        }
    }
}

fn retained_lease_conflict_resolution(
    existing_state: RetainedSessionState,
    incoming_state: RetainedSessionState,
) -> RetainedLeaseConflictResolution {
    match (
        existing_state.requires_physical_session_preservation(),
        incoming_state.requires_physical_session_preservation(),
    ) {
        (false, true) => RetainedLeaseConflictResolution::ReplaceExisting,
        (true, true) => RetainedLeaseConflictResolution::KeepExistingMarkedInvalid,
        _ => RetainedLeaseConflictResolution::KeepExisting,
    }
}

fn retained_lease_context_decision(
    context_matches: bool,
    retained_state: RetainedSessionState,
) -> RetainedLeaseContextDecision {
    if context_matches || !retained_state.requires_physical_session_preservation() {
        RetainedLeaseContextDecision::Reusable
    } else {
        RetainedLeaseContextDecision::BlockContextMismatch
    }
}

impl SharedDbSessionLease {
    /// The lease's own mutex, tracked so the app-wide lock order is observable.
    fn lock_inner(&self) -> TrackedGuard<'_, DbSessionLeaseSlot> {
        let _order = crate::db::lock_order::LockOrderScope::enter(
            crate::db::lock_order::names::SESSION_LEASE,
        );
        TrackedGuard {
            guard: self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            _order,
        }
    }

    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(DbSessionLeaseSlot::default())),
        }
    }

    fn from_inner(inner: Arc<Mutex<DbSessionLeaseSlot>>) -> Self {
        Self { inner }
    }

    /// Publish this slot to the retained-session registry.
    ///
    /// Called from the one place a session becomes retained, so a connection
    /// teardown can reclaim it without the owner having to take part.
    fn register_for_connection_teardown(&self) {
        let handle = Arc::downgrade(&self.inner);
        let mut registry = lock_retained_pool_session_leases();
        registry.retain(|lease| lease.strong_count() > 0);
        if !registry.iter().any(|lease| lease.ptr_eq(&handle)) {
            registry.push(handle);
        }
    }

    /// Take the retained session if it belongs to the given (ended) connection
    /// incarnation. The caller discards it with no lock held.
    fn take_entry_for_connection_generation(
        &self,
        connection_generation: u64,
    ) -> Option<DbSessionLeaseEntry> {
        let mut lease = self.lock_inner();
        let matches = lease
            .entry
            .as_ref()
            .is_some_and(|entry| entry.connection_generation == connection_generation);
        if matches {
            lease.entry.take()
        } else {
            None
        }
    }

    pub fn clear(&self) -> bool {
        let lease_to_drop = { self.lock_inner().entry.take() };
        if let Some(entry) = lease_to_drop {
            entry.discard_physical("db::session_lease");
            true
        } else {
            false
        }
    }

    /// Close this slot for good: its owner is going away.
    ///
    /// Beyond `clear`, this refuses every store from now on. A cancelled
    /// statement can outlive its tab, and when it finally hands its session
    /// back there is nobody left to clear the slot again — so the store path
    /// closes the session physically instead of retaining it. Every backend
    /// shares this refusal because every backend shares the store path.
    pub fn close_for_owner_shutdown(&self) -> bool {
        let lease_to_drop = {
            let mut lease = self.lock_inner();
            lease.closed = true;
            lease.entry.take()
        };
        if let Some(entry) = lease_to_drop {
            entry.discard_physical("db::session_lease");
            true
        } else {
            false
        }
    }

    pub fn snapshot(&self) -> Option<PooledSessionLeaseSnapshot> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry
            .as_ref()
            .and_then(|entry| entry.lease().map(|lease| (entry, lease)))
            .map(|(entry, lease)| PooledSessionLeaseSnapshot {
                db_type: lease.db_type(),
                pool_context_epoch: entry.pool_context_epoch,
                transaction_state: entry.retained_state.summary_transaction_state(),
                retained_state: entry.retained_state,
                current_scope: entry.current_scope.clone(),
            })
    }

    fn take_reusable_lease_matching_connection(
        &self,
        connection_generation: u64,
        db_type: DatabaseType,
        tracking: Option<(&ConnectionInfo, &DbActivityGuard)>,
    ) -> Option<TakenDbSessionLease> {
        let mut stale_lease_to_drop = None;
        let reusable_lease = {
            let mut lease = self.lock_inner();
            let reusable = lease.entry.as_ref().is_some_and(|existing| {
                existing.matches_connection(connection_generation, db_type)
            });
            if reusable {
                lease.entry.take().and_then(|mut entry| {
                    let pool_context_epoch = entry.pool_context_epoch;
                    let retained_state = entry.retained_state;
                    let current_scope = entry.current_scope.take();
                    let taken = TakenDbSessionLease::new_with_retained_state(
                        self.clone(),
                        connection_generation,
                        pool_context_epoch,
                        entry.take_lease()?,
                        retained_state,
                        current_scope,
                    );
                    Some(match tracking {
                        Some((connection_info, activity)) => {
                            taken.track_under(connection_info, activity)
                        }
                        None => taken,
                    })
                })
            } else {
                if lease.entry.is_some() {
                    stale_lease_to_drop = lease.entry.take();
                }
                None
            }
        };
        if let Some(entry) = stale_lease_to_drop {
            entry.discard_physical("db::session_lease");
        }
        reusable_lease
    }

    pub fn take_reusable_lease_for_context_update(
        &self,
        connection_generation: u64,
        db_type: DatabaseType,
        connection_info: &ConnectionInfo,
        activity: &DbActivityGuard,
    ) -> Option<TakenDbSessionLease> {
        self.take_reusable_lease_matching_connection(
            connection_generation,
            db_type,
            Some((connection_info, activity)),
        )
    }

    pub fn take_reusable_lease_for_resolution(
        &self,
        connection_generation: u64,
        db_type: DatabaseType,
        connection_info: &ConnectionInfo,
        activity: &DbActivityGuard,
    ) -> Option<TakenDbSessionLease> {
        self.take_reusable_lease_matching_connection(
            connection_generation,
            db_type,
            Some((connection_info, activity)),
        )
    }

    /// Take the tab's retained session for reuse.
    ///
    /// The activity guard is required for the same reason it is on
    /// `acquire_session`: a retained session skips acquire entirely, so this is
    /// the only place that can publish it to the activity registry.
    pub fn take_reusable_lease(
        &self,
        connection_generation: u64,
        pool_context_epoch: u64,
        db_type: DatabaseType,
        connection_info: &ConnectionInfo,
        activity: &DbActivityGuard,
    ) -> RetainedSessionTakeOutcome {
        let mut stale_lease_to_drop = None;
        let reusable_lease = {
            let mut lease = self.lock_inner();
            let Some(existing) = lease.entry.as_ref() else {
                return RetainedSessionTakeOutcome::NoSession;
            };
            if !existing.matches_connection(connection_generation, db_type) {
                stale_lease_to_drop = lease.entry.take();
                None
            } else if retained_lease_context_decision(
                existing.matches_context(connection_generation, pool_context_epoch, db_type),
                existing.retained_state,
            ) == RetainedLeaseContextDecision::Reusable
            {
                lease.entry.take().and_then(|mut entry| {
                    let restore_epoch = if entry.pool_context_epoch == pool_context_epoch {
                        entry.pool_context_epoch
                    } else {
                        pool_context_epoch
                    };
                    let retained_state = entry.retained_state;
                    let current_scope = entry.current_scope.take();
                    Some(
                        TakenDbSessionLease::new_with_retained_state(
                            self.clone(),
                            connection_generation,
                            restore_epoch,
                            entry.take_lease()?,
                            retained_state,
                            current_scope,
                        )
                        .track_under(connection_info, activity),
                    )
                })
            } else {
                return RetainedSessionTakeOutcome::BlockedContextMismatch(existing.retained_state);
            }
        };
        if let Some(entry) = stale_lease_to_drop {
            entry.discard_physical("db::session_lease");
            return RetainedSessionTakeOutcome::DiscardedBecauseStale;
        }
        reusable_lease
            .map(Box::new)
            .map(RetainedSessionTakeOutcome::Reusable)
            .unwrap_or(RetainedSessionTakeOutcome::NoSession)
    }

    pub fn discard_oracle_if_current_connection(
        &self,
        connection_generation: u64,
        expected_conn: &Arc<Connection>,
        log_context: &str,
    ) -> bool {
        let lease_to_drop = {
            let mut lease = self.lock_inner();
            let should_clear = lease.entry.as_ref().is_some_and(|existing| {
                existing.connection_generation == connection_generation
                    && match existing.lease() {
                        Some(DbSessionLease::Oracle(conn)) => Arc::ptr_eq(conn, expected_conn),
                        Some(DbSessionLease::OracleThin(_))
                        | Some(DbSessionLease::MySQL { .. })
                        | None => false,
                    }
            });
            if should_clear {
                lease.entry.take()
            } else {
                None
            }
        };
        if let Some(entry) = lease_to_drop {
            entry.discard_physical(log_context);
            true
        } else {
            false
        }
    }

    pub fn apply_retained_session_disposition(
        &self,
        connection_generation: u64,
        pool_context_epoch: u64,
        lease: DbSessionLease,
        disposition: RetainedSessionDisposition,
        log_context: &str,
    ) -> bool {
        self.apply_retained_session_disposition_with_scope(
            connection_generation,
            pool_context_epoch,
            lease,
            disposition,
            log_context,
            None,
        )
    }

    pub fn apply_retained_session_disposition_with_scope(
        &self,
        connection_generation: u64,
        pool_context_epoch: u64,
        lease: DbSessionLease,
        disposition: RetainedSessionDisposition,
        log_context: &str,
        current_scope: Option<String>,
    ) -> bool {
        match disposition {
            RetainedSessionDisposition::Retain(retained_state) => self
                .store_if_empty_with_retained_state_and_scope(
                    connection_generation,
                    pool_context_epoch,
                    lease,
                    retained_state,
                    current_scope,
                ),
            RetainedSessionDisposition::DiscardPhysical => {
                lease.discard_physical(log_context);
                true
            }
        }
    }

    pub fn store_if_empty_with_retained_state(
        &self,
        connection_generation: u64,
        pool_context_epoch: u64,
        lease_to_store: DbSessionLease,
        retained_state: RetainedSessionState,
    ) -> bool {
        self.store_if_empty_with_retained_state_and_scope(
            connection_generation,
            pool_context_epoch,
            lease_to_store,
            retained_state,
            None,
        )
    }

    pub fn store_if_empty_with_retained_state_and_scope(
        &self,
        connection_generation: u64,
        pool_context_epoch: u64,
        lease_to_store: DbSessionLease,
        retained_state: RetainedSessionState,
        current_scope: Option<String>,
    ) -> bool {
        let lease_db_type = lease_to_store.db_type();
        let mut lease_to_store = Some(lease_to_store);
        let mut stored = false;
        let mut refused_because_closed = false;
        let old_lease_to_drop = {
            let mut lease = self.lock_inner();
            if lease.closed {
                // The owner is gone. Retaining here would park a live server
                // session in a slot nobody will ever clear again, so the
                // session is closed instead — on every backend, because every
                // backend hands sessions back through this one path.
                refused_because_closed = true;
                None
            } else {
                let should_store = match lease.entry.as_mut() {
                    None => true,
                    Some(existing) => {
                        if existing.connection_generation != connection_generation
                            || existing.pool_context_epoch != pool_context_epoch
                            || !existing
                                .lease()
                                .is_some_and(|lease| lease.is_db_type(lease_db_type))
                        {
                            true
                        } else {
                            match retained_lease_conflict_resolution(
                                existing.retained_state,
                                retained_state,
                            ) {
                                RetainedLeaseConflictResolution::KeepExisting => false,
                                RetainedLeaseConflictResolution::ReplaceExisting => true,
                                RetainedLeaseConflictResolution::KeepExistingMarkedInvalid => {
                                    existing.retained_state = existing
                                        .retained_state
                                        .conservative_merge(retained_state)
                                        .with_transaction_state(
                                            TransactionSessionState::InvalidSession,
                                        );
                                    false
                                }
                            }
                        }
                    }
                };
                if should_store {
                    let old_lease = lease.entry.take();
                    if let Some(lease_to_store) = lease_to_store.take() {
                        lease.entry = Some(DbSessionLeaseEntry::new_with_retained_state(
                            connection_generation,
                            pool_context_epoch,
                            lease_to_store,
                            retained_state,
                            current_scope,
                        ));
                        stored = true;
                    }
                    old_lease
                } else {
                    None
                }
            }
        };
        if stored {
            // The one place a session becomes retained is the one place the
            // teardown paths have to learn about it.
            self.register_for_connection_teardown();
        }
        if let Some(entry) = old_lease_to_drop {
            entry.discard_physical("db::session_lease");
        }
        if let Some(lease_to_store) = lease_to_store.take() {
            if refused_because_closed {
                logging::log_info(
                    "db::session_lease",
                    &format!("Closing a {lease_db_type} session handed back to a closed query tab"),
                );
            } else {
                logging::log_warning(
                    "db::session_lease",
                    &format!(
                        "Discarded conflicting retained {} session for generation {} because an active retained session already exists",
                        lease_db_type, connection_generation
                    ),
                );
            }
            lease_to_store.discard_physical("db::session_lease");
            return false;
        }
        true
    }
}

pub(crate) trait DbBackend: Sync {
    fn db_type(&self) -> DatabaseType;
    fn display_name(&self) -> &'static str;
    fn choice_label(&self) -> &'static str {
        self.display_name()
    }
    fn connection_form_spec(&self) -> DbConnectionFormSpec;
    fn advanced_settings_form_spec(&self) -> DbAdvancedSettingsFormSpec;
    fn table_browse_spec(&self) -> DbTableBrowseSpec;
    /// Where the server puts NULLs on an ascending `ORDER BY` with no explicit
    /// `NULLS FIRST` / `NULLS LAST`. The result grid's local header sort mirrors
    /// this so a locally sorted column lands in the same order the server would
    /// have produced.
    fn sorts_nulls_last_ascending(&self) -> bool;
    fn sql_dialect(&self) -> SqlDialect;
    fn supports_mysql_delimiter_commands(&self) -> bool;
    fn supports_explicit_analytic_null_treatment(&self) -> bool;
    fn uses_mysql_analytic_null_treatment_rules(&self) -> bool;
    fn supports_trailing_select_into_after_set_limit(&self) -> bool;
    fn preserves_quoted_routine_lookup_spelling(&self) -> bool;
    fn backend_kind(&self) -> DatabaseBackendKind;
    fn cache_key(&self) -> u8;
    fn default_connection_info(&self) -> ConnectionInfo;
    fn default_advanced_settings(&self) -> ConnectionAdvancedSettings;
    fn connection_string(&self, info: &ConnectionInfo) -> String;
    fn service_name_label(&self) -> &'static str;
    fn validate_advanced_settings(
        &self,
        settings: &ConnectionAdvancedSettings,
        using_tns_alias: bool,
    ) -> Result<(), String>;
    fn validate_session_time_zone(&self, value: &str) -> Result<(), String> {
        let Some(offset) = parse_session_time_zone_offset(value) else {
            return Err(
                "Session time zone must be blank or an offset like +00:00 or -05:30".to_string(),
            );
        };

        if self.session_time_zone_in_range(offset) {
            Ok(())
        } else {
            Err(self.session_time_zone_error_message().to_string())
        }
    }
    fn session_time_zone_in_range(&self, offset: SessionTimeZoneOffset) -> bool;
    fn session_time_zone_error_message(&self) -> &'static str;
    fn connect(
        &self,
        info: &ConnectionInfo,
        pool_size: u32,
        auto_commit: bool,
        policy: ConnectionAttemptPolicy,
    ) -> Result<(DbConnection, DbConnectionPool), String>;
    fn build_pool(
        &self,
        info: &ConnectionInfo,
        pool_size: u32,
        policy: ConnectionAttemptPolicy,
    ) -> Result<DbConnectionPool, String>;
    fn apply_pool_session_settings(
        &self,
        session: &mut DbPoolSession,
        advanced: &ConnectionAdvancedSettings,
    ) -> Result<(), String>;
    fn apply_current_scope_to_session(
        &self,
        context: &DbPoolSessionContext,
        session: &mut DbPoolSession,
    ) -> Result<(), String>;
    fn test_connection(
        &self,
        info: &ConnectionInfo,
        policy: ConnectionAttemptPolicy,
    ) -> Result<(), String>;
    // Transaction/session behavior methods below have no default bodies on
    // purpose: a silent no-op default (e.g. auto-commit toggles that do
    // nothing) is exactly the kind of omission a new backend must not be able
    // to compile with. Each backend states its behavior explicitly, even when
    // that behavior is "nothing to do".
    fn after_connect(&self, connection: &mut DatabaseConnection);
    fn current_scope_name(&self, connection: &DatabaseConnection) -> Option<String>;
    fn switch_scope(
        &self,
        connection: &mut DatabaseConnection,
        target_scope: &str,
    ) -> Result<(), String>;
    fn apply_scope_to_lease(
        &self,
        lease: &mut DbSessionLease,
        target_scope: &str,
        advanced: &ConnectionAdvancedSettings,
        preserve_existing_session_state: bool,
    ) -> Result<(), String>;
    fn has_connection_scope(&self) -> bool;
    fn can_apply_empty_scope_to_retained_session(&self) -> bool;
    fn retained_session_blocks_transaction_mode_change(
        &self,
        retained_state: RetainedSessionState,
    ) -> bool;
    fn can_replace_retained_transaction_mode(&self, retained_state: RetainedSessionState) -> bool;
    fn scope_values_match(&self, left: Option<&str>, right: Option<&str>) -> bool;
    fn metadata_scope_noun(&self) -> &'static str;
    fn switch_scope_noun(&self) -> &'static str;
    fn metadata_refresh_activity(&self, requested_scope: Option<&str>) -> String {
        self.metadata_refresh_activity_with_base("Loading schema metadata", requested_scope)
    }
    fn metadata_refresh_activity_with_base(
        &self,
        base_activity: &str,
        requested_scope: Option<&str>,
    ) -> String {
        match requested_scope
            .map(str::trim)
            .filter(|scope| !scope.is_empty())
        {
            Some(scope) => format!(
                "{} for {} {}",
                base_activity,
                self.metadata_scope_noun(),
                scope
            ),
            None => base_activity.to_string(),
        }
    }
    fn scope_switch_activity_message(&self, target_scope: &str) -> String {
        format!("Switching {} to {}", self.switch_scope_noun(), target_scope)
    }
    fn scope_switch_failure_message(&self, target_scope: &str, err: &str) -> String {
        format!(
            "Failed to switch {} to {}: {}",
            self.switch_scope_noun(),
            target_scope,
            err
        )
    }
    fn supported_ssl_choices(&self) -> &'static [(ConnectionSslMode, &'static str)];
    fn normalize_ssl_mode(&self, mode: ConnectionSslMode) -> ConnectionSslMode {
        if self
            .supported_ssl_choices()
            .iter()
            .any(|(choice, _)| *choice == mode)
        {
            mode
        } else {
            ConnectionSslMode::Disabled
        }
    }
    fn is_recoverable_timeout_message(&self, trimmed: &str, lower: &str) -> bool;
    fn apply_auto_commit(&self, connection: &mut DbConnection, enabled: bool)
        -> Result<(), String>;
    fn supported_transaction_isolations(&self) -> &'static [TransactionIsolation];
    fn fallback_default_transaction_isolation(&self) -> TransactionIsolation;
    fn read_current_default_transaction_isolation(
        &self,
        connection: &mut Option<DbConnection>,
    ) -> Result<Option<TransactionIsolation>, String>;
    fn apply_transaction_mode_to_live_connection(
        &self,
        connection: &mut Option<DbConnection>,
        mode: TransactionMode,
        default_isolation: TransactionIsolation,
    ) -> Result<(), String>;
    fn transaction_mode_requires_first_statement(&self, mode: TransactionMode) -> bool;
    fn transaction_mode_statements(&self, mode: TransactionMode) -> Result<Vec<String>, String>;
}

struct OracleBackend;

impl OracleBackend {
    fn connection_string_with_policy(
        info: &ConnectionInfo,
        policy: ConnectionAttemptPolicy,
    ) -> String {
        if info.uses_oracle_tns_alias() {
            return info.service_name.trim().to_string();
        }

        let protocol = if info.advanced.oracle_effective_protocol() == OracleNetworkProtocol::Tcps {
            "TCPS"
        } else {
            "TCP"
        };
        let timeout_seconds = policy.timeout().as_secs().max(1);
        format!(
            "(DESCRIPTION=(CONNECT_TIMEOUT={timeout_seconds}sec)(TRANSPORT_CONNECT_TIMEOUT={timeout_seconds}sec)(RETRY_COUNT=0)(ADDRESS=(PROTOCOL={protocol})(HOST={})(PORT={}))(CONNECT_DATA=(SERVICE_NAME={})))",
            info.host, info.port, info.service_name
        )
    }
}

struct MysqlBackend {
    db_type: DatabaseType,
    display_name: &'static str,
    choice_label: &'static str,
    cache_key: u8,
    supports_explicit_analytic_null_treatment: bool,
    uses_mysql_analytic_null_treatment_rules: bool,
    supports_trailing_select_into_after_set_limit: bool,
    session_time_zone_in_range: fn(SessionTimeZoneOffset) -> bool,
    session_time_zone_error_message: &'static str,
}

const ORACLE_TRANSACTION_ISOLATIONS: [TransactionIsolation; 3] = [
    TransactionIsolation::Default,
    TransactionIsolation::ReadCommitted,
    TransactionIsolation::Serializable,
];
const MYSQL_TRANSACTION_ISOLATIONS: [TransactionIsolation; 5] = [
    TransactionIsolation::Default,
    TransactionIsolation::ReadUncommitted,
    TransactionIsolation::ReadCommitted,
    TransactionIsolation::RepeatableRead,
    TransactionIsolation::Serializable,
];
const ORACLE_SSL_CHOICES: [(ConnectionSslMode, &str); 2] = [
    (ConnectionSslMode::Disabled, "Disabled"),
    (ConnectionSslMode::Required, "Required (TCPS)"),
];
const MYSQL_SSL_CHOICES: [(ConnectionSslMode, &str); 4] = [
    (ConnectionSslMode::Disabled, "Disabled"),
    (ConnectionSslMode::Required, "Required"),
    (ConnectionSslMode::VerifyCa, "Verify CA"),
    (ConnectionSslMode::VerifyIdentity, "Verify identity"),
];

static ORACLE_BACKEND: OracleBackend = OracleBackend;
static MYSQL_BACKEND: MysqlBackend = MysqlBackend {
    db_type: DatabaseType::MySQL,
    display_name: "MySQL",
    choice_label: "MySQL",
    cache_key: 1,
    supports_explicit_analytic_null_treatment: true,
    uses_mysql_analytic_null_treatment_rules: true,
    supports_trailing_select_into_after_set_limit: true,
    session_time_zone_in_range: mysql_session_time_zone_in_range,
    session_time_zone_error_message:
        "MySQL session time zone must be blank or an offset from -13:59 through +14:00",
};
static MARIADB_BACKEND: MysqlBackend = MysqlBackend {
    db_type: DatabaseType::MariaDB,
    display_name: "MariaDB",
    choice_label: "MariaDB",
    cache_key: 2,
    supports_explicit_analytic_null_treatment: false,
    uses_mysql_analytic_null_treatment_rules: false,
    supports_trailing_select_into_after_set_limit: false,
    session_time_zone_in_range: mariadb_session_time_zone_in_range,
    session_time_zone_error_message:
        "MariaDB session time zone must be blank or an offset from -12:59 through +13:00",
};

impl MysqlBackend {
    fn ensure_concrete_db_type(&self, actual: DatabaseType, resource: &str) -> Result<(), String> {
        if actual.is_same_type_as(self.db_type) {
            Ok(())
        } else {
            Err(format!(
                "Expected {} {} but found {}",
                self.display_name, resource, actual
            ))
        }
    }
}

pub(crate) fn backend_for(db_type: DatabaseType) -> &'static dyn DbBackend {
    match db_type {
        DatabaseType::Oracle => &ORACLE_BACKEND,
        DatabaseType::MySQL => &MYSQL_BACKEND,
        DatabaseType::MariaDB => &MARIADB_BACKEND,
    }
}

/// The schema a session prepared for `scope` must be put in: the tab's scope,
/// else the connection's own schema (read from the server at connect).
///
/// One rule, used by session acquisition (both Oracle drivers) and by the
/// per-statement application, so a tab's session can never be left in the
/// schema another tab put it in. It resolves to a concrete name because the
/// connection always knows its own — applying nothing would be a no-op, and a
/// pooled session is recycled between tabs.
fn oracle_session_schema(scope: Option<&str>, connection_schema: Option<&str>) -> Option<String> {
    scope
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .or_else(|| {
            connection_schema
                .map(str::trim)
                .filter(|schema| !schema.is_empty())
        })
        .map(str::to_string)
}

fn scope_values_match_exact(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.trim() == right.trim(),
        (None, None) => true,
        (Some(value), None) | (None, Some(value)) => value.trim().is_empty(),
    }
}

pub(crate) fn retained_scope_matches_target(
    db_type: DatabaseType,
    retained_scope: Option<&str>,
    target_scope: &str,
) -> bool {
    retained_scope.is_some_and(|scope| db_type.scope_values_match(Some(scope), Some(target_scope)))
}

impl DbBackend for OracleBackend {
    fn db_type(&self) -> DatabaseType {
        DatabaseType::Oracle
    }

    fn display_name(&self) -> &'static str {
        "Oracle"
    }

    fn connection_form_spec(&self) -> DbConnectionFormSpec {
        DbConnectionFormSpec {
            show_driver_mode: true,
            service_name_form_label: "Service:",
            service_name_value_label: "Service name",
            service_name_required: true,
            default_host: "localhost",
            default_port: 1521,
            default_service_name: "",
            supports_tns_alias: true,
        }
    }

    fn advanced_settings_form_spec(&self) -> DbAdvancedSettingsFormSpec {
        DbAdvancedSettingsFormSpec {
            show_oracle_protocol: true,
            show_oracle_nls_formats: true,
            show_mysql_session_options: false,
            show_mysql_ssl_ca_path: false,
        }
    }

    fn table_browse_spec(&self) -> DbTableBrowseSpec {
        DbTableBrowseSpec {
            pagination: DbTableBrowsePagination::Rownum,
            strips_page_helper_column: true,
        }
    }

    fn sorts_nulls_last_ascending(&self) -> bool {
        true
    }

    fn sql_dialect(&self) -> SqlDialect {
        SqlDialect::Oracle
    }

    fn supports_mysql_delimiter_commands(&self) -> bool {
        false
    }

    fn supports_explicit_analytic_null_treatment(&self) -> bool {
        true
    }

    fn uses_mysql_analytic_null_treatment_rules(&self) -> bool {
        false
    }

    fn supports_trailing_select_into_after_set_limit(&self) -> bool {
        false
    }

    fn preserves_quoted_routine_lookup_spelling(&self) -> bool {
        true
    }

    fn backend_kind(&self) -> DatabaseBackendKind {
        DatabaseBackendKind::Oracle
    }

    fn cache_key(&self) -> u8 {
        0
    }

    fn default_connection_info(&self) -> ConnectionInfo {
        let form = self.connection_form_spec();
        ConnectionInfo {
            name: String::new(),
            username: String::new(),
            password: String::new(),
            host: form.default_host.to_string(),
            port: form.default_port,
            service_name: form.default_service_name.to_string(),
            db_type: self.db_type(),
            advanced: ConnectionAdvancedSettings::default_for(self.db_type()),
            color: ConnectionColor::default(),
            read_only: false,
            debug_oracle_thin_protocol_version: None,
        }
    }

    fn default_advanced_settings(&self) -> ConnectionAdvancedSettings {
        ConnectionAdvancedSettings::default()
    }

    fn validate_advanced_settings(
        &self,
        settings: &ConnectionAdvancedSettings,
        using_tns_alias: bool,
    ) -> Result<(), String> {
        settings.validate_oracle(using_tns_alias)
    }

    fn session_time_zone_in_range(&self, offset: SessionTimeZoneOffset) -> bool {
        oracle_session_time_zone_in_range(offset)
    }

    fn session_time_zone_error_message(&self) -> &'static str {
        "Oracle session time zone must be blank or an offset from -12:00 through +14:00"
    }

    fn connection_string(&self, info: &ConnectionInfo) -> String {
        if info.uses_oracle_tns_alias() {
            info.service_name.trim().to_string()
        } else if info.advanced.oracle_effective_protocol() == OracleNetworkProtocol::Tcps {
            format!(
                "(DESCRIPTION=(ADDRESS=(PROTOCOL=TCPS)(HOST={})(PORT={}))(CONNECT_DATA=(SERVICE_NAME={})))",
                info.host, info.port, info.service_name
            )
        } else {
            format!("//{}:{}/{}", info.host, info.port, info.service_name)
        }
    }

    fn service_name_label(&self) -> &'static str {
        "Service Name"
    }

    fn connect(
        &self,
        info: &ConnectionInfo,
        pool_size: u32,
        _auto_commit: bool,
        policy: ConnectionAttemptPolicy,
    ) -> Result<(DbConnection, DbConnectionPool), String> {
        if info.advanced.oracle_driver_mode == OracleDriverMode::Thin {
            let config = DatabaseConnection::build_oracle_thin_config(info, policy)?;
            let requested_minimum_protocol = config.connect_options.minimum_protocol_version;
            let requested_desired_protocol = config.connect_options.desired_protocol_version;
            let mut session = OracleThinSession::connect(config).map_err(|err| err.to_string())?;
            DatabaseConnection::log_oracle_thin_protocol_acceptance(
                &session,
                requested_minimum_protocol,
                requested_desired_protocol,
            );
            DatabaseConnection::apply_oracle_thin_session_settings(&mut session, &info.advanced)?;
            return Ok((
                DbConnection::OracleThin(Arc::new(Mutex::new(session))),
                self.build_pool(info, pool_size, policy)?,
            ));
        }

        ensure_oracle_client_initialized().map_err(|e| e.to_string())?;
        let conn_str = Self::connection_string_with_policy(info, policy);
        let connection = Arc::new(
            Connector::new(&info.username, &info.password, &conn_str)
                .connect()
                .map_err(|err| err.to_string())?,
        );
        DatabaseConnection::apply_oracle_session_settings(connection.as_ref(), &info.advanced)?;
        Ok((
            DbConnection::Oracle(connection),
            self.build_pool(info, pool_size, policy)?,
        ))
    }

    fn build_pool(
        &self,
        info: &ConnectionInfo,
        pool_size: u32,
        policy: ConnectionAttemptPolicy,
    ) -> Result<DbConnectionPool, String> {
        if info.advanced.oracle_driver_mode == OracleDriverMode::Thin {
            return DatabaseConnection::build_oracle_thin_pool(info, pool_size, policy).map(
                |pool| DbConnectionPool::OracleThin {
                    pool: Arc::new(pool),
                    advanced: info.advanced.clone(),
                },
            );
        }

        DatabaseConnection::build_oracle_pool(info, pool_size, policy).map(|pool| {
            DbConnectionPool::Oracle {
                pool,
                advanced: info.advanced.clone(),
            }
        })
    }

    fn apply_pool_session_settings(
        &self,
        session: &mut DbPoolSession,
        advanced: &ConnectionAdvancedSettings,
    ) -> Result<(), String> {
        match session {
            DbPoolSession::Oracle(conn) => {
                DatabaseConnection::apply_oracle_session_settings(conn, advanced)
            }
            DbPoolSession::OracleThin(conn) => {
                DatabaseConnection::apply_oracle_thin_session_settings(conn, advanced)
            }
            DbPoolSession::MySQL { .. } => Err(format!(
                "Expected Oracle pool session but acquired {}",
                session.db_type()
            )),
        }
    }

    fn apply_current_scope_to_session(
        &self,
        context: &DbPoolSessionContext,
        session: &mut DbPoolSession,
    ) -> Result<(), String> {
        let result = match session {
            DbPoolSession::Oracle(conn) => DatabaseConnection::apply_oracle_current_schema(
                conn,
                context.oracle_current_schema.as_deref(),
            ),
            DbPoolSession::OracleThin(conn) => {
                DatabaseConnection::apply_oracle_thin_current_schema(
                    conn,
                    context.oracle_current_schema.as_deref(),
                )
            }
            DbPoolSession::MySQL { .. } => Err(format!(
                "Expected Oracle pool session but acquired {}",
                session.db_type()
            )),
        };
        match result {
            Err(message) if DatabaseConnection::oracle_missing_current_schema_error(&message) => {
                // Same rule as apply_tracked_oracle_current_schema: a dropped
                // tracked schema must not make fresh sessions unusable; the
                // session falls back to the login schema.
                logging::log_warning(
                    "oracle pool session",
                    &format!(
                        "Tracked Oracle current schema {:?} is not available; acquiring the session without it",
                        context.oracle_current_schema.as_deref().unwrap_or_default()
                    ),
                );
                Ok(())
            }
            other => other.map_err(|err| format!("Failed to apply Oracle current schema: {err}")),
        }
    }

    fn test_connection(
        &self,
        info: &ConnectionInfo,
        policy: ConnectionAttemptPolicy,
    ) -> Result<(), String> {
        if info.advanced.oracle_driver_mode == OracleDriverMode::Thin {
            let config = DatabaseConnection::build_oracle_thin_config(info, policy)?;
            let requested_minimum_protocol = config.connect_options.minimum_protocol_version;
            let requested_desired_protocol = config.connect_options.desired_protocol_version;
            let mut session = OracleThinSession::connect(config).map_err(|err| err.to_string())?;
            DatabaseConnection::log_oracle_thin_protocol_acceptance(
                &session,
                requested_minimum_protocol,
                requested_desired_protocol,
            );
            DatabaseConnection::apply_oracle_thin_session_settings(&mut session, &info.advanced)?;
            return Ok(());
        }

        ensure_oracle_client_initialized().map_err(|e| e.to_string())?;
        let conn_str = Self::connection_string_with_policy(info, policy);
        let connection = Connector::new(&info.username, &info.password, &conn_str)
            .connect()
            .map_err(|err| err.to_string())?;
        DatabaseConnection::apply_oracle_session_settings(&connection, &info.advanced)?;
        Ok(())
    }

    fn current_scope_name(&self, connection: &DatabaseConnection) -> Option<String> {
        connection
            .tracked_oracle_current_schema()
            .map(str::trim)
            .filter(|scope| !scope.is_empty())
            .map(str::to_string)
    }

    fn switch_scope(
        &self,
        connection: &mut DatabaseConnection,
        target_scope: &str,
    ) -> Result<(), String> {
        connection.switch_oracle_current_schema(target_scope)
    }

    fn apply_scope_to_lease(
        &self,
        lease: &mut DbSessionLease,
        target_scope: &str,
        _advanced: &ConnectionAdvancedSettings,
        _preserve_existing_session_state: bool,
    ) -> Result<(), String> {
        match lease {
            DbSessionLease::Oracle(conn) => {
                DatabaseConnection::apply_oracle_current_schema(conn.as_ref(), Some(target_scope))
            }
            DbSessionLease::OracleThin(conn) => {
                DatabaseConnection::apply_oracle_thin_current_schema(conn, Some(target_scope))
            }
            DbSessionLease::MySQL { .. } => Err(format!(
                "Expected Oracle retained session but found {}",
                lease.db_type()
            )),
        }
    }

    fn has_connection_scope(&self) -> bool {
        true
    }

    fn can_apply_empty_scope_to_retained_session(&self) -> bool {
        false
    }

    fn retained_session_blocks_transaction_mode_change(
        &self,
        retained_state: RetainedSessionState,
    ) -> bool {
        retained_state.requires_physical_session_preservation()
    }

    fn can_replace_retained_transaction_mode(&self, _retained_state: RetainedSessionState) -> bool {
        false
    }

    fn scope_values_match(&self, left: Option<&str>, right: Option<&str>) -> bool {
        scope_values_match_exact(left, right)
    }

    fn metadata_scope_noun(&self) -> &'static str {
        "owner"
    }

    fn switch_scope_noun(&self) -> &'static str {
        "current schema"
    }

    fn supported_ssl_choices(&self) -> &'static [(ConnectionSslMode, &'static str)] {
        &ORACLE_SSL_CHOICES
    }

    fn normalize_ssl_mode(&self, mode: ConnectionSslMode) -> ConnectionSslMode {
        match mode {
            ConnectionSslMode::VerifyCa | ConnectionSslMode::VerifyIdentity => {
                ConnectionSslMode::Required
            }
            mode => mode,
        }
    }

    fn is_recoverable_timeout_message(&self, trimmed: &str, lower: &str) -> bool {
        trimmed.contains("DPI-1067") || lower.contains("dpi-1067")
    }

    fn after_connect(&self, connection: &mut DatabaseConnection) {
        // Read the schema the session actually logged into, the twin of the
        // MySQL branch below. Without it this connection has no schema of its
        // own, and preparing a session for a tab with no scope would have
        // nothing concrete to apply — leaving a recycled pooled session in
        // whichever schema the previous tab put it in. Guessing from the
        // typed username does not work: it is quoted when it contains
        // lowercase, so `system` becomes `"system"`, which Oracle rejects.
        if let Err(err) = connection.sync_oracle_current_schema_after_connect() {
            eprintln!("Warning: failed to read Oracle current schema after connect: {err}");
        }
    }

    fn apply_auto_commit(
        &self,
        connection: &mut DbConnection,
        _enabled: bool,
    ) -> Result<(), String> {
        match connection {
            DbConnection::Oracle(_) | DbConnection::OracleThin(_) => {
                // Oracle has no session-level autocommit flag to push; the
                // executor consults the logical auto-commit setting per statement.
                Ok(())
            }
            unexpected @ DbConnection::MySQL { .. } => Err(format!(
                "Expected Oracle live connection but found {}",
                unexpected.db_type()
            )),
        }
    }

    fn supported_transaction_isolations(&self) -> &'static [TransactionIsolation] {
        &ORACLE_TRANSACTION_ISOLATIONS
    }

    fn fallback_default_transaction_isolation(&self) -> TransactionIsolation {
        TransactionIsolation::ReadCommitted
    }

    fn apply_transaction_mode_to_live_connection(
        &self,
        connection: &mut Option<DbConnection>,
        _mode: TransactionMode,
        _default_isolation: TransactionIsolation,
    ) -> Result<(), String> {
        match connection.as_mut() {
            Some(DbConnection::Oracle(_)) | Some(DbConnection::OracleThin(_)) | None => {
                // Oracle applies transaction mode through SET TRANSACTION as the
                // first statement of each transaction (`transaction_mode_statements`),
                // never against the live session.
                Ok(())
            }
            Some(unexpected @ DbConnection::MySQL { .. }) => Err(format!(
                "Expected Oracle live connection but found {}",
                unexpected.db_type()
            )),
        }
    }

    fn read_current_default_transaction_isolation(
        &self,
        connection: &mut Option<DbConnection>,
    ) -> Result<Option<TransactionIsolation>, String> {
        match connection.as_mut() {
            Some(DbConnection::Oracle(conn)) => {
                DatabaseConnection::read_oracle_default_transaction_isolation(conn.as_ref())
            }
            Some(DbConnection::OracleThin(conn)) => {
                let mut guard = conn
                    .lock()
                    .map_err(|_| "Oracle thin connection mutex poisoned".to_string())?;
                let raw = DatabaseConnection::oracle_thin_select_one_text(
                    &mut guard,
                    "SELECT value FROM v$ses_optimizer_env WHERE sid = SYS_CONTEXT('USERENV', 'SID') AND name = 'transaction_isolation_level'",
                )?;
                Ok(raw
                    .as_deref()
                    .and_then(TransactionIsolation::from_sql_level))
            }
            Some(unexpected @ DbConnection::MySQL { .. }) => Err(format!(
                "Expected Oracle live connection but found {}",
                unexpected.db_type()
            )),
            None => Ok(None),
        }
    }

    fn transaction_mode_requires_first_statement(&self, mode: TransactionMode) -> bool {
        !mode.is_default()
    }

    fn transaction_mode_statements(&self, mode: TransactionMode) -> Result<Vec<String>, String> {
        if !self
            .supported_transaction_isolations()
            .contains(&mode.isolation)
        {
            return Err(format!(
                "Oracle does not support {} transaction isolation",
                mode.isolation.label()
            ));
        }

        if mode.access_mode == TransactionAccessMode::ReadOnly {
            // An Oracle read-only transaction reads one consistent snapshot —
            // exactly the SERIALIZABLE read guarantee, with writes forbidden.
            // So "Serializable + Read only" IS `SET TRANSACTION READ ONLY`,
            // while statement-level Read committed consistency cannot exist
            // inside one: that pair has no Oracle behavior to map to.
            if !matches!(
                mode.isolation,
                TransactionIsolation::Default | TransactionIsolation::Serializable
            ) {
                return Err(format!(
                    "Oracle cannot combine {} isolation with READ ONLY: a read-only transaction always reads a single consistent snapshot (Serializable)",
                    mode.isolation.label()
                ));
            }
            return Ok(vec![format!(
                "SET TRANSACTION {}",
                mode.access_mode.sql_clause()
            )]);
        }

        if let Some(level) = mode.isolation.sql_level() {
            return Ok(vec![format!("SET TRANSACTION ISOLATION LEVEL {level}")]);
        }

        Ok(Vec::new())
    }
}

impl DbBackend for MysqlBackend {
    fn db_type(&self) -> DatabaseType {
        self.db_type
    }

    fn display_name(&self) -> &'static str {
        self.display_name
    }

    fn choice_label(&self) -> &'static str {
        self.choice_label
    }

    fn connection_form_spec(&self) -> DbConnectionFormSpec {
        DbConnectionFormSpec {
            show_driver_mode: false,
            service_name_form_label: "Database:",
            service_name_value_label: "Database name",
            service_name_required: false,
            default_host: "localhost",
            default_port: 3306,
            default_service_name: "",
            supports_tns_alias: false,
        }
    }

    fn advanced_settings_form_spec(&self) -> DbAdvancedSettingsFormSpec {
        DbAdvancedSettingsFormSpec {
            show_oracle_protocol: false,
            show_oracle_nls_formats: false,
            show_mysql_session_options: true,
            show_mysql_ssl_ca_path: true,
        }
    }

    fn table_browse_spec(&self) -> DbTableBrowseSpec {
        DbTableBrowseSpec {
            pagination: DbTableBrowsePagination::LimitOffset,
            strips_page_helper_column: false,
        }
    }

    fn sorts_nulls_last_ascending(&self) -> bool {
        false
    }

    fn sql_dialect(&self) -> SqlDialect {
        SqlDialect::MySql
    }

    fn supports_mysql_delimiter_commands(&self) -> bool {
        true
    }

    fn supports_explicit_analytic_null_treatment(&self) -> bool {
        self.supports_explicit_analytic_null_treatment
    }

    fn uses_mysql_analytic_null_treatment_rules(&self) -> bool {
        self.uses_mysql_analytic_null_treatment_rules
    }

    fn supports_trailing_select_into_after_set_limit(&self) -> bool {
        self.supports_trailing_select_into_after_set_limit
    }

    fn preserves_quoted_routine_lookup_spelling(&self) -> bool {
        false
    }

    fn backend_kind(&self) -> DatabaseBackendKind {
        DatabaseBackendKind::MySql
    }

    fn cache_key(&self) -> u8 {
        self.cache_key
    }

    fn default_connection_info(&self) -> ConnectionInfo {
        let form = self.connection_form_spec();
        ConnectionInfo {
            name: String::new(),
            username: String::new(),
            password: String::new(),
            host: form.default_host.to_string(),
            port: form.default_port,
            service_name: form.default_service_name.to_string(),
            db_type: self.db_type(),
            advanced: ConnectionAdvancedSettings::default_for(self.db_type()),
            color: ConnectionColor::default(),
            read_only: false,
            debug_oracle_thin_protocol_version: None,
        }
    }

    fn default_advanced_settings(&self) -> ConnectionAdvancedSettings {
        ConnectionAdvancedSettings {
            session_time_zone: "+00:00".to_string(),
            ..Default::default()
        }
    }

    fn validate_advanced_settings(
        &self,
        settings: &ConnectionAdvancedSettings,
        _using_tns_alias: bool,
    ) -> Result<(), String> {
        settings.validate_mysql()
    }

    fn session_time_zone_in_range(&self, offset: SessionTimeZoneOffset) -> bool {
        (self.session_time_zone_in_range)(offset)
    }

    fn session_time_zone_error_message(&self) -> &'static str {
        self.session_time_zone_error_message
    }

    fn connection_string(&self, info: &ConnectionInfo) -> String {
        let database = info.service_name.trim();
        if database.is_empty() {
            format!("mysql://{}:{}", info.host, info.port)
        } else {
            format!("mysql://{}:{}/{}", info.host, info.port, database)
        }
    }

    fn service_name_label(&self) -> &'static str {
        "Database"
    }

    fn connect(
        &self,
        info: &ConnectionInfo,
        pool_size: u32,
        _auto_commit: bool,
        policy: ConnectionAttemptPolicy,
    ) -> Result<(DbConnection, DbConnectionPool), String> {
        let opts = DatabaseConnection::build_mysql_opts(info, policy);
        let mut conn = mysql::Conn::new(opts).map_err(|err| err.to_string())?;
        DatabaseConnection::apply_mysql_session_settings_for_db_type(
            &mut conn,
            &info.advanced,
            self.db_type,
        )?;
        // The live connection only ever runs app metadata queries; user SQL
        // always executes on pooled sessions, which apply the logical
        // auto-commit setting on every acquisition. Keep the live session on
        // autocommit=1: under autocommit=0 every metadata table read leaves an
        // implicitly opened transaction on it, which the dirty probe then
        // truthfully reports, permanently refusing the auto-commit toggle.
        DatabaseConnection::apply_mysql_autocommit_setting_for_db_type(
            &mut conn,
            true,
            self.db_type,
        )?;
        Ok((
            DbConnection::MySQL {
                conn,
                db_type: self.db_type,
            },
            self.build_pool(info, pool_size, policy)?,
        ))
    }

    fn build_pool(
        &self,
        info: &ConnectionInfo,
        pool_size: u32,
        policy: ConnectionAttemptPolicy,
    ) -> Result<DbConnectionPool, String> {
        DatabaseConnection::build_mysql_pool(info, pool_size, policy).map(|pool| {
            DbConnectionPool::MySQL {
                pool,
                advanced: info.advanced.clone(),
                db_type: self.db_type,
            }
        })
    }

    fn apply_pool_session_settings(
        &self,
        session: &mut DbPoolSession,
        advanced: &ConnectionAdvancedSettings,
    ) -> Result<(), String> {
        let DbPoolSession::MySQL { conn, db_type } = session else {
            return Err(format!(
                "Expected {} pool session but acquired {}",
                self.display_name,
                session.db_type()
            ));
        };
        self.ensure_concrete_db_type(*db_type, "pool session")?;
        DatabaseConnection::apply_mysql_session_settings_for_db_type(conn, advanced, self.db_type)
    }

    fn apply_current_scope_to_session(
        &self,
        context: &DbPoolSessionContext,
        session: &mut DbPoolSession,
    ) -> Result<(), String> {
        let DbPoolSession::MySQL { conn, db_type } = session else {
            return Err(format!(
                "Expected {} pool session but acquired {}",
                self.display_name,
                session.db_type()
            ));
        };
        self.ensure_concrete_db_type(*db_type, "pool session")?;
        let current_database = context.current_service_name.trim();
        if current_database.is_empty() {
            DatabaseConnection::reset_mysql_session_to_no_database_for_db_type(
                conn.as_mut(),
                self.db_type,
            )?;
            DatabaseConnection::apply_mysql_session_settings_for_db_type(
                conn,
                &context.connection_info.advanced,
                self.db_type,
            )
            .map_err(|err| {
                format!(
                    "Failed to reapply {} session settings after database reset: {err}",
                    self.display_name()
                )
            })?;
            return DatabaseConnection::apply_mysql_session_transaction_options(
                conn,
                context.auto_commit,
                context.transaction_mode,
                context.connection_info.db_type,
                context.default_transaction_isolation,
            );
        }

        conn.as_mut().select_db(current_database).map_err(|err| {
            format!(
                "Failed to apply {} current database `{current_database}`: {err}",
                self.display_name()
            )
        })?;
        DatabaseConnection::apply_mysql_connection_encoding_with_settings_for_db_type(
            conn,
            &context.connection_info.advanced,
            self.db_type,
        )
        .map_err(|err| {
            format!(
                "Failed to refresh {} session encoding after database switch: {err}",
                self.display_name()
            )
        })?;
        DatabaseConnection::apply_mysql_session_transaction_options(
            conn,
            context.auto_commit,
            context.transaction_mode,
            context.connection_info.db_type,
            context.default_transaction_isolation,
        )
    }

    fn test_connection(
        &self,
        info: &ConnectionInfo,
        policy: ConnectionAttemptPolicy,
    ) -> Result<(), String> {
        let opts = DatabaseConnection::build_mysql_opts(info, policy);
        let mut conn = mysql::Conn::new(opts).map_err(|err| err.to_string())?;
        DatabaseConnection::apply_mysql_session_settings_for_db_type(
            &mut conn,
            &info.advanced,
            self.db_type,
        )?;
        Ok(())
    }

    fn current_scope_name(&self, connection: &DatabaseConnection) -> Option<String> {
        let scope = connection.get_info().service_name.trim();
        (!scope.is_empty()).then(|| scope.to_string())
    }

    fn switch_scope(
        &self,
        connection: &mut DatabaseConnection,
        target_scope: &str,
    ) -> Result<(), String> {
        connection.switch_mysql_database(target_scope)
    }

    fn apply_scope_to_lease(
        &self,
        lease: &mut DbSessionLease,
        target_scope: &str,
        advanced: &ConnectionAdvancedSettings,
        preserve_existing_session_state: bool,
    ) -> Result<(), String> {
        let actual_db_type = lease.db_type();
        let DbSessionLease::MySQL { conn, db_type } = lease else {
            return Err(format!(
                "Expected {} retained session but found {}",
                self.display_name, actual_db_type
            ));
        };
        self.ensure_concrete_db_type(*db_type, "retained session")?;
        let target_scope = target_scope.trim();
        if target_scope.is_empty() {
            if preserve_existing_session_state {
                return Err(
                    DatabaseConnection::mysql_empty_scope_requires_resolved_session_error(),
                );
            }
            DatabaseConnection::reset_mysql_session_to_no_database_for_db_type(
                conn.as_mut(),
                self.db_type,
            )?;
            return DatabaseConnection::apply_mysql_session_settings_for_db_type(
                conn,
                advanced,
                self.db_type,
            );
        }
        conn.as_mut()
            .select_db(target_scope)
            .map_err(|err| err.to_string())?;
        if preserve_existing_session_state {
            return Ok(());
        }
        DatabaseConnection::apply_mysql_connection_encoding_with_settings_for_db_type(
            conn,
            advanced,
            self.db_type,
        )
    }

    fn has_connection_scope(&self) -> bool {
        true
    }

    fn can_apply_empty_scope_to_retained_session(&self) -> bool {
        true
    }

    fn retained_session_blocks_transaction_mode_change(
        &self,
        _retained_state: RetainedSessionState,
    ) -> bool {
        false
    }

    fn can_replace_retained_transaction_mode(&self, retained_state: RetainedSessionState) -> bool {
        retained_state.allows_transaction_mode_replacement()
    }

    fn scope_values_match(&self, left: Option<&str>, right: Option<&str>) -> bool {
        scope_values_match_exact(left, right)
    }

    fn metadata_scope_noun(&self) -> &'static str {
        "database"
    }

    fn switch_scope_noun(&self) -> &'static str {
        "database"
    }

    fn supported_ssl_choices(&self) -> &'static [(ConnectionSslMode, &'static str)] {
        &MYSQL_SSL_CHOICES
    }

    fn is_recoverable_timeout_message(&self, _trimmed: &str, lower: &str) -> bool {
        lower.contains("error 3024")
            || lower.contains("er_query_timeout")
            || lower.contains("max_execution_time")
            || lower.contains("max_statement_time")
            || lower.contains("max statement time exceeded")
            || lower.contains("maximum statement execution time exceeded")
    }

    fn after_connect(&self, connection: &mut DatabaseConnection) {
        if let Err(err) = connection.sync_mysql_current_database_name() {
            eprintln!(
                "Warning: failed to sync {} current database after connect: {err}",
                self.display_name()
            );
        }
    }

    fn apply_auto_commit(
        &self,
        connection: &mut DbConnection,
        _enabled: bool,
    ) -> Result<(), String> {
        match connection {
            DbConnection::MySQL { conn: _, db_type } => {
                // The live connection stays on autocommit=1 (see `connect`):
                // it only runs app metadata queries, and pooled sessions apply
                // the logical setting on every acquisition. Only validate the
                // dispatch here.
                self.ensure_concrete_db_type(*db_type, "live connection")
            }
            unexpected @ (DbConnection::Oracle(_) | DbConnection::OracleThin(_)) => Err(format!(
                "Expected {} live connection but found {}",
                self.display_name,
                unexpected.db_type()
            )),
        }
    }

    fn supported_transaction_isolations(&self) -> &'static [TransactionIsolation] {
        &MYSQL_TRANSACTION_ISOLATIONS
    }

    fn fallback_default_transaction_isolation(&self) -> TransactionIsolation {
        TransactionIsolation::ReadCommitted
    }

    fn transaction_mode_requires_first_statement(&self, _mode: TransactionMode) -> bool {
        false
    }

    fn read_current_default_transaction_isolation(
        &self,
        connection: &mut Option<DbConnection>,
    ) -> Result<Option<TransactionIsolation>, String> {
        match connection.as_mut() {
            Some(DbConnection::MySQL { conn, db_type }) => {
                self.ensure_concrete_db_type(*db_type, "live connection")?;
                DatabaseConnection::read_mysql_default_transaction_isolation(conn)
            }
            Some(unexpected @ (DbConnection::Oracle(_) | DbConnection::OracleThin(_))) => {
                Err(format!(
                    "Expected {} live connection but found {}",
                    self.display_name,
                    unexpected.db_type()
                ))
            }
            None => Ok(None),
        }
    }

    fn apply_transaction_mode_to_live_connection(
        &self,
        connection: &mut Option<DbConnection>,
        mode: TransactionMode,
        default_isolation: TransactionIsolation,
    ) -> Result<(), String> {
        match connection.as_mut() {
            Some(DbConnection::MySQL { conn, db_type }) => {
                self.ensure_concrete_db_type(*db_type, "live connection")?;
                DatabaseConnection::apply_mysql_transaction_mode_for_db_with_default(
                    conn,
                    mode,
                    self.db_type,
                    default_isolation,
                )
            }
            Some(unexpected @ (DbConnection::Oracle(_) | DbConnection::OracleThin(_))) => {
                Err(format!(
                    "Expected {} live connection but found {}",
                    self.display_name,
                    unexpected.db_type()
                ))
            }
            None => Ok(()),
        }
    }

    fn transaction_mode_statements(&self, mode: TransactionMode) -> Result<Vec<String>, String> {
        if !self
            .supported_transaction_isolations()
            .contains(&mode.isolation)
        {
            return Err(format!(
                "MySQL/MariaDB does not support {} transaction isolation",
                mode.isolation.label()
            ));
        }

        let mut characteristics = Vec::new();
        if let Some(level) = mode.isolation.sql_level() {
            characteristics.push(format!("ISOLATION LEVEL {level}"));
        }
        characteristics.push(mode.access_mode.sql_clause().to_string());

        Ok(vec![format!(
            "SET SESSION TRANSACTION {}",
            characteristics.join(", ")
        )])
    }
}

pub struct DatabaseConnection {
    connection: Option<DbConnection>,
    pool: Option<DbConnectionPool>,
    info: ConnectionInfo,
    session_password: String,
    oracle_current_schema: Option<String>,
    connected: bool,
    auto_commit: bool,
    transaction_mode: TransactionMode,
    default_transaction_isolation: TransactionIsolation,
    session: Arc<Mutex<SessionState>>,
    last_disconnect_reason: Option<String>,
    connection_generation: u64,
    pool_context_epoch: Arc<AtomicU64>,
    /// A shared mirror of `connection_generation`.
    ///
    /// Work on the main connection is bound to THIS rather than to the pool
    /// context epoch: the epoch is bumped by ordinary operations that run while
    /// holding the connection lock (`set_auto_commit`, `set_transaction_mode`,
    /// `switch_mysql_database`), so binding to it would make the stale sweep
    /// cancel those operations mid-flight. The generation moves only when the
    /// connection itself is replaced or closed, which is the real signal.
    connection_generation_token: Arc<AtomicU64>,
    /// Which registered connection this is, once a runtime has claimed it.
    ///
    /// Stamped here so every activity started on this connection can be tagged
    /// automatically: without it, a cancel or a teardown cannot tell one
    /// connection's work from another's.
    connection_id: Option<ConnectionId>,
    connection_pool_size: u32,
}

impl DatabaseConnection {
    fn clamp_connection_pool_size(size: u32) -> u32 {
        size.clamp(MIN_CONNECTION_POOL_SIZE, MAX_CONNECTION_POOL_SIZE)
    }

    fn build_mysql_opts(
        info: &ConnectionInfo,
        policy: ConnectionAttemptPolicy,
    ) -> mysql::OptsBuilder {
        Self::build_mysql_opts_with_pool_size(info, None, policy)
    }

    pub(crate) fn build_mysql_opts_without_database(info: &ConnectionInfo) -> mysql::OptsBuilder {
        Self::build_mysql_opts_with_pool_size_and_database(
            info,
            None,
            false,
            ConnectionAttemptPolicy::runtime(),
        )
    }

    fn build_mysql_opts_with_pool_size(
        info: &ConnectionInfo,
        pool_size: Option<u32>,
        policy: ConnectionAttemptPolicy,
    ) -> mysql::OptsBuilder {
        Self::build_mysql_opts_with_pool_size_and_database(info, pool_size, true, policy)
    }

    fn build_mysql_pool_opts(
        info: &ConnectionInfo,
        pool_size: u32,
        policy: ConnectionAttemptPolicy,
    ) -> mysql::OptsBuilder {
        Self::build_mysql_opts_with_pool_size_and_database(info, Some(pool_size), false, policy)
    }

    fn build_mysql_opts_with_pool_size_and_database(
        info: &ConnectionInfo,
        pool_size: Option<u32>,
        include_database: bool,
        policy: ConnectionAttemptPolicy,
    ) -> mysql::OptsBuilder {
        let mut opts = mysql::OptsBuilder::new()
            .ip_or_hostname(Some(&info.host))
            .tcp_port(info.port)
            .user(Some(&info.username))
            .pass(Some(&info.password))
            .tcp_connect_timeout(Some(policy.timeout()))
            .prefer_socket(false);

        let database = info.service_name.trim();
        if include_database && !database.is_empty() {
            opts = opts.db_name(Some(database));
        }

        opts = Self::apply_mysql_driver_options(opts, &info.advanced);

        if let Some(pool_size) = pool_size {
            let pool_size = Self::clamp_connection_pool_size(pool_size) as usize;
            if let Some(constraints) = mysql::PoolConstraints::new(0, pool_size) {
                opts = opts.pool_opts(Some(
                    mysql::PoolOpts::default().with_constraints(constraints),
                ));
            }
        }

        opts
    }

    fn apply_mysql_driver_options(
        mut opts: mysql::OptsBuilder,
        advanced: &ConnectionAdvancedSettings,
    ) -> mysql::OptsBuilder {
        if advanced.ssl_mode != ConnectionSslMode::Disabled {
            let mut ssl_opts = mysql::SslOpts::default();
            let ca_path = advanced.mysql_ssl_ca_path.trim();
            if !ca_path.is_empty() {
                ssl_opts = ssl_opts.with_root_cert_path(Some(std::path::PathBuf::from(ca_path)));
            }
            ssl_opts = match advanced.ssl_mode {
                ConnectionSslMode::Disabled => ssl_opts,
                ConnectionSslMode::Required => ssl_opts
                    .with_danger_skip_domain_validation(true)
                    .with_danger_accept_invalid_certs(true),
                ConnectionSslMode::VerifyCa => ssl_opts.with_danger_skip_domain_validation(true),
                ConnectionSslMode::VerifyIdentity => ssl_opts,
            };
            opts = opts.ssl_opts(ssl_opts);
        }
        opts
    }

    fn build_oracle_pool(
        info: &ConnectionInfo,
        pool_size: u32,
        policy: ConnectionAttemptPolicy,
    ) -> Result<oracle::pool::Pool, String> {
        let conn_str = OracleBackend::connection_string_with_policy(info, policy);
        let pool_size = Self::clamp_connection_pool_size(pool_size);
        let mut builder =
            oracle::pool::PoolBuilder::new(info.username.clone(), info.password.clone(), conn_str);
        builder
            .min_connections(1)
            .max_connections(pool_size)
            .connection_increment(1)
            .get_mode(GetMode::TimedWait(POOL_SESSION_ACQUIRE_TIMEOUT));
        builder.build().map_err(|err| err.to_string())
    }

    fn build_oracle_thin_config(
        info: &ConnectionInfo,
        policy: ConnectionAttemptPolicy,
    ) -> Result<OracleThinConfig, String> {
        if info.uses_oracle_tns_alias() {
            return Err(
                "Oracle Thin currently supports Host + Port + Service connections only".to_string(),
            );
        }
        if info.advanced.oracle_effective_protocol() != OracleNetworkProtocol::Tcp {
            return Err("Oracle Thin currently supports TCP only".to_string());
        }

        ensure_oracle_thin_connect_logger_installed();
        let mut config = OracleThinConfig::new(
            ConnectTarget::service_name(info.host.clone(), info.port, info.service_name.clone()),
            info.username.clone(),
            info.password.clone(),
        );
        config.program = "space-query-thin".to_string();
        config.connect_options.tcp_connect_timeout = policy.timeout();
        config.connect_options.connect_io_timeout = policy.timeout();
        config.connect_options.retry_count = 0;
        config.connect_options.retry_delay = Duration::ZERO;
        // Skip the connect-time out-of-band probe on the interactive connect
        // path. Some Oracle 318+ listeners advertise OOB (supports_oob_check)
        // but then stall the protocol handshake when the urgent-data probe is
        // sent, hanging login at `ttc-protocol-read`. Without the probe, query
        // cancel falls back to the in-band interrupt marker (the two-tier
        // model still works). Matches the diagnostic/debug connect paths.
        config.connect_options.disable_oob_probe = true;
        apply_oracle_thin_protocol_env(&mut config)?;
        apply_oracle_thin_debug_protocol(&mut config, info.debug_oracle_thin_protocol_version)?;
        Ok(config)
    }

    fn format_oracle_thin_protocol_acceptance_log(
        accepted_protocol_version: Option<u16>,
        requested_minimum_protocol: u16,
        requested_desired_protocol: u16,
        ttc_field_version: u8,
    ) -> String {
        let accepted = accepted_protocol_version
            .map(|version| version.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let requested = if requested_minimum_protocol == requested_desired_protocol {
            requested_minimum_protocol.to_string()
        } else {
            format!("{requested_minimum_protocol}..{requested_desired_protocol}")
        };
        format!(
            "Oracle Thin accepted TNS protocol version {accepted} (requested {requested}); TTC field version {ttc_field_version}"
        )
    }

    fn log_oracle_thin_protocol_acceptance(
        session: &OracleThinSession,
        requested_minimum_protocol: u16,
        requested_desired_protocol: u16,
    ) {
        logging::log_info(
            "oracle_thin",
            &Self::format_oracle_thin_protocol_acceptance_log(
                session.capabilities().protocol_version,
                requested_minimum_protocol,
                requested_desired_protocol,
                session.capabilities().ttc_field_version,
            ),
        );
    }

    fn build_oracle_thin_pool(
        info: &ConnectionInfo,
        pool_size: u32,
        policy: ConnectionAttemptPolicy,
    ) -> Result<OracleThinSessionPool, String> {
        let config = Self::build_oracle_thin_config(info, policy)?;
        let options = OracleThinPoolOptions {
            max_size: Self::clamp_connection_pool_size(pool_size) as usize,
            acquire_timeout: POOL_SESSION_ACQUIRE_TIMEOUT,
        };
        Ok(OracleThinSessionPool::new(config, options))
    }

    fn build_mysql_pool(
        info: &ConnectionInfo,
        pool_size: u32,
        policy: ConnectionAttemptPolicy,
    ) -> Result<mysql::Pool, String> {
        let opts = Self::build_mysql_pool_opts(info, pool_size, policy);
        mysql::Pool::new(opts).map_err(|err| err.to_string())
    }

    fn build_pool_for_info(
        info: &ConnectionInfo,
        pool_size: u32,
        policy: ConnectionAttemptPolicy,
    ) -> Result<DbConnectionPool, String> {
        backend_for(info.db_type).build_pool(info, pool_size, policy)
    }

    pub fn new() -> Self {
        Self {
            connection: None,
            pool: None,
            info: ConnectionInfo::default(),
            session_password: String::new(),
            oracle_current_schema: None,
            connected: false,
            auto_commit: false,
            transaction_mode: TransactionMode::default(),
            default_transaction_isolation: TransactionIsolation::Default,
            session: Arc::new(Mutex::new(SessionState::default())),
            last_disconnect_reason: None,
            connection_generation: 0,
            pool_context_epoch: Arc::new(AtomicU64::new(0)),
            connection_generation_token: Arc::new(AtomicU64::new(0)),
            connection_id: None,
            connection_pool_size: DEFAULT_CONNECTION_POOL_SIZE,
        }
    }

    /// End this connection's current incarnation and start the next one.
    ///
    /// Called from exactly the places where the physical connection or its
    /// pool is replaced or closed, so the generation is the app-wide answer to
    /// "is this session still ours". Two things hang off that:
    ///
    /// * The new generation comes from a process-wide counter, so a
    ///   generation identifies one incarnation of ONE connection — two
    ///   connections can never hold the same value and be mistaken for each
    ///   other.
    /// * Every session retained from the incarnation that just ended is
    ///   released here, physically, instead of being left for whichever tab
    ///   happens to notice the mismatch next. A retained session keeps its
    ///   whole pool alive (a `PooledConn` owns a clone of the MySQL pool, an
    ///   `Arc<Connection>` keeps the OCI pool from being destroyed), so one
    ///   forgotten lease pins every idle session in that pool on the server.
    fn bump_connection_generation(&mut self) {
        let retired_generation = self.connection_generation;
        self.connection_generation = next_connection_generation();
        self.connection_generation_token
            .store(self.connection_generation, Ordering::Release);
        reclaim_retired_connection_sessions_in_background(retired_generation);
    }

    pub fn connection_id(&self) -> Option<ConnectionId> {
        self.connection_id
    }

    /// The lifetime work on the main connection should be bound to.
    pub fn activity_lifetime(&self) -> DbActivityLifetime {
        DbActivityLifetime {
            epoch_token: Arc::clone(&self.connection_generation_token),
            epoch: self.connection_generation,
        }
    }

    fn bump_pool_context_epoch(&self) {
        self.pool_context_epoch.fetch_add(1, Ordering::AcqRel);
    }

    fn current_pool_context_epoch(&self) -> u64 {
        self.pool_context_epoch.load(Ordering::Acquire)
    }

    pub fn connect(&mut self, info: ConnectionInfo) -> Result<(), String> {
        self.connect_with_policy(info, ConnectionAttemptPolicy::runtime())
    }

    pub(crate) fn connect_with_policy(
        &mut self,
        info: ConnectionInfo,
        policy: ConnectionAttemptPolicy,
    ) -> Result<(), String> {
        let prepared =
            Self::prepare_connection(info, self.connection_pool_size, self.auto_commit, policy)?;
        let retired = self.install_prepared_connection(prepared)?;
        Self::retire_connection_in_background(retired);
        Ok(())
    }

    fn prepare_connection(
        info: ConnectionInfo,
        pool_size: u32,
        auto_commit: bool,
        policy: ConnectionAttemptPolicy,
    ) -> Result<Self, String> {
        info.advanced
            .validate_for_db(info.db_type, info.uses_oracle_tns_alias())?;
        let description = info.connection_attempt_description("Establishing");
        run_connection_attempt(policy, description, move || {
            let mut prepared = Self::new();
            prepared.connection_pool_size = Self::clamp_connection_pool_size(pool_size);
            prepared.auto_commit = auto_commit;
            prepared.connect_blocking_with_policy(info, policy)?;
            Ok(prepared)
        })
    }

    fn connect_blocking_with_policy(
        &mut self,
        info: ConnectionInfo,
        policy: ConnectionAttemptPolicy,
    ) -> Result<(), String> {
        let (db_conn, pool) = backend_for(info.db_type).connect(
            &info,
            self.connection_pool_size,
            self.auto_commit,
            policy,
        )?;

        // Swap in the new connection only after a successful handshake.
        // This preserves the active session when users mistype credentials
        // during reconnect attempts.
        self.connection = Some(db_conn);
        self.pool = Some(pool);
        let db_type = info.db_type;
        let new_session_password = info.password.clone();
        ConnectionInfo::clear_secret(&mut self.session_password);
        self.session_password = new_session_password;
        self.info = info;
        self.oracle_current_schema = None;
        self.sync_default_transaction_isolation(db_type);
        self.transaction_mode = TransactionMode::new(
            TransactionIsolation::Default,
            self.info.advanced.default_transaction_access_mode,
        );
        self.connected = true;
        backend_for(db_type).after_connect(self);
        self.last_disconnect_reason = None;
        self.bump_connection_generation();
        self.bump_pool_context_epoch();

        // Keep SessionState::reset() backend-preserving for same-DB resets;
        // successful connection transitions must explicitly stamp the new
        // backend here so delimiter/bind scanning and SQL*Plus substitution
        // defaults follow the live database.
        match self.session.lock() {
            Ok(mut guard) => guard.reset_for_connection(db_type),
            Err(poisoned) => poisoned.into_inner().reset_for_connection(db_type),
        }

        Ok(())
    }

    fn install_prepared_connection(&mut self, mut prepared: Self) -> Result<Self, String> {
        if !prepared.connected || prepared.connection.is_none() || prepared.pool.is_none() {
            Self::retire_connection_in_background(prepared);
            return Err("Prepared database connection is incomplete".to_string());
        }
        std::mem::swap(&mut self.connection, &mut prepared.connection);
        std::mem::swap(&mut self.pool, &mut prepared.pool);
        std::mem::swap(&mut self.info, &mut prepared.info);
        std::mem::swap(&mut self.session_password, &mut prepared.session_password);
        std::mem::swap(
            &mut self.oracle_current_schema,
            &mut prepared.oracle_current_schema,
        );
        std::mem::swap(&mut self.connected, &mut prepared.connected);
        std::mem::swap(&mut self.auto_commit, &mut prepared.auto_commit);
        std::mem::swap(&mut self.transaction_mode, &mut prepared.transaction_mode);
        std::mem::swap(
            &mut self.default_transaction_isolation,
            &mut prepared.default_transaction_isolation,
        );
        std::mem::swap(
            &mut self.last_disconnect_reason,
            &mut prepared.last_disconnect_reason,
        );
        std::mem::swap(
            &mut self.connection_pool_size,
            &mut prepared.connection_pool_size,
        );

        self.bump_connection_generation();
        self.bump_pool_context_epoch();
        let db_type = self.info.db_type;
        update_session_state_without_blocking(
            &self.session,
            &self.pool_context_epoch,
            self.current_pool_context_epoch(),
            move |session| session.reset_for_connection(db_type),
        );
        Ok(prepared)
    }

    fn retire_connection_in_background(mut retired: Self) {
        let connection = retired.connection.take();
        let pool = retired.pool.take();
        ConnectionInfo::clear_secret(&mut retired.session_password);
        retired.info.clear_password();
        drop(retired);
        Self::retire_connection_resources_in_background(connection, pool);
    }

    /// The one place a connection's physical resources go away, whichever path
    /// got here: disconnect, reconnect, pool resize, a failed install, or the
    /// connection simply being dropped.
    ///
    /// Pruning the pool-context cache is part of retiring, not an extra: the
    /// cache holds a CLONE of the pool, and a pool with a clone outstanding is
    /// not destroyed -- ODPI keeps the OCI session pool (and every session in
    /// it) alive on its own refcount, and the MySQL pool keeps its idle
    /// connections. Live-observed as "dropping a connection nobody
    /// disconnected leaves its sessions open" on Oracle OCI.
    fn retire_connection_resources_in_background(
        connection: Option<DbConnection>,
        pool: Option<DbConnectionPool>,
    ) {
        if connection.is_none() && pool.is_none() {
            return;
        }
        spawn_connection_cleanup(move || {
            prune_stale_pool_session_context_cache();
            if let Some(pool) = pool.as_ref() {
                pool.close();
            }
            drop(connection);
            drop(pool);
        });
    }

    pub(crate) fn apply_oracle_session_settings(
        conn: &Connection,
        advanced: &ConnectionAdvancedSettings,
    ) -> Result<(), String> {
        let statements = Self::oracle_session_setting_statements(advanced);

        for statement in statements {
            if let Err(err) = conn.execute(statement.as_str(), &[]) {
                return Err(format!(
                    "Failed to apply Oracle session setting `{statement}`: {err}"
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn apply_oracle_thin_session_settings(
        session: &mut OracleThinSession,
        advanced: &ConnectionAdvancedSettings,
    ) -> Result<(), String> {
        let statements = Self::oracle_session_setting_statements(advanced);

        for statement in statements {
            if let Err(err) = session.query_drop(&statement) {
                return Err(format!(
                    "Failed to apply Oracle thin session setting `{statement}`: {err}"
                ));
            }
        }
        session
            .flush_pending_cursor_closes()
            .map_err(|err| format!("Failed to close Oracle thin session setting cursors: {err}"))?;
        Ok(())
    }

    fn oracle_session_setting_statements(advanced: &ConnectionAdvancedSettings) -> Vec<String> {
        let mut statements = vec![
            format!(
                "ALTER SESSION SET NLS_TIMESTAMP_FORMAT = '{}'",
                advanced.oracle_nls_timestamp_format.trim()
            ),
            format!(
                "ALTER SESSION SET NLS_DATE_FORMAT = '{}'",
                advanced.oracle_nls_date_format.trim()
            ),
        ];

        if let Some(level) = advanced.default_transaction_isolation.sql_level() {
            statements.push(format!("ALTER SESSION SET ISOLATION_LEVEL = {level}"));
        }
        let time_zone = advanced.session_time_zone.trim();
        if !time_zone.is_empty() {
            statements.push(format!("ALTER SESSION SET TIME_ZONE = '{time_zone}'"));
        }
        statements
    }

    fn normalize_oracle_current_schema_name(schema: &str) -> Option<String> {
        let trimmed = schema.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    fn set_tracked_oracle_current_schema(&mut self, schema: Option<String>) {
        let normalized = schema
            .as_deref()
            .and_then(Self::normalize_oracle_current_schema_name);
        if self.oracle_current_schema != normalized {
            self.oracle_current_schema = normalized;
            self.bump_pool_context_epoch();
        }
    }

    pub(crate) fn apply_mysql_session_settings_for_db_type<C: Queryable>(
        conn: &mut C,
        advanced: &ConnectionAdvancedSettings,
        db_type: DatabaseType,
    ) -> Result<(), String> {
        Self::apply_mysql_session_settings_for_db_type_with_isolation(conn, advanced, db_type, true)
    }

    /// The same session settings without the connection's default isolation
    /// level, for a session an execution is about to put into the requesting
    /// tab's own transaction mode.
    ///
    /// The tab's mode already resolves `Default` to the connection default, so
    /// re-asserting it here is redundant — and harmful: it leaves the session
    /// on a level the execution then has to change, and changing it means
    /// ending the transaction the tab's own reads had opened (MySQL fixes a
    /// transaction's isolation at its start). Two plain SELECTs of one script
    /// could not share a snapshot under a pinned isolation level because of it.
    pub(crate) fn apply_mysql_session_settings_without_default_isolation_for_db_type<
        C: Queryable,
    >(
        conn: &mut C,
        advanced: &ConnectionAdvancedSettings,
        db_type: DatabaseType,
    ) -> Result<(), String> {
        Self::apply_mysql_session_settings_for_db_type_with_isolation(
            conn, advanced, db_type, false,
        )
    }

    fn apply_mysql_session_settings_for_db_type_with_isolation<C: Queryable>(
        conn: &mut C,
        advanced: &ConnectionAdvancedSettings,
        db_type: DatabaseType,
        include_default_transaction_isolation: bool,
    ) -> Result<(), String> {
        let display_name = db_type.display_name();
        Self::validate_mysql_session_time_zone_for_server(conn, advanced.session_time_zone.trim())?;
        let statements = Self::mysql_session_setting_statements_with_isolation(
            advanced,
            include_default_transaction_isolation,
        );

        for statement in statements {
            if let Err(err) = conn.query_drop(statement.as_str()) {
                return Err(format!(
                    "Failed to apply {display_name} session setting `{statement}`: {err}"
                ));
            }
        }

        Self::apply_mysql_connection_encoding_with_settings_for_db_type(conn, advanced, db_type)
    }

    pub(crate) fn reset_mysql_session_to_no_database_for_db_type(
        conn: &mut mysql::Conn,
        db_type: DatabaseType,
    ) -> Result<(), String> {
        let display_name = db_type.display_name();
        conn.change_user(mysql::ChangeUserOpts::new().with_db_name(None))
            .map_err(|err| format!("Failed to reset {display_name} session database scope: {err}"))
    }

    pub(crate) fn mysql_empty_scope_requires_resolved_session_error() -> String {
        "Cannot clear the MySQL/MariaDB database scope while the retained session has transaction or session state. Resolve or discard the retained session first.".to_string()
    }

    fn validate_mysql_session_time_zone_for_server<C: Queryable>(
        conn: &mut C,
        time_zone: &str,
    ) -> Result<(), String> {
        let Some(offset) = parse_session_time_zone_offset(time_zone) else {
            return Ok(());
        };
        if mariadb_session_time_zone_in_range(offset) {
            return Ok(());
        }

        if let Ok(Some(version)) = conn.query_first::<String, _>("SELECT VERSION()") {
            Self::validate_mysql_session_time_zone_for_server_version(time_zone, &version)?;
        }
        Ok(())
    }

    fn validate_mysql_session_time_zone_for_server_version(
        time_zone: &str,
        server_version: &str,
    ) -> Result<(), String> {
        let Some(offset) = parse_session_time_zone_offset(time_zone) else {
            return Ok(());
        };
        if mariadb_session_time_zone_in_range(offset)
            || !server_version.to_ascii_lowercase().contains("mariadb")
        {
            return Ok(());
        }

        Err(format!(
            "MariaDB session time zone `{time_zone}` is outside MariaDB's supported offset range (-12:59 through +13:00)"
        ))
    }

    #[cfg(test)]
    fn mysql_session_setting_statements(advanced: &ConnectionAdvancedSettings) -> Vec<String> {
        Self::mysql_session_setting_statements_with_isolation(advanced, true)
    }

    fn mysql_session_setting_statements_with_isolation(
        advanced: &ConnectionAdvancedSettings,
        include_default_transaction_isolation: bool,
    ) -> Vec<String> {
        let mut statements = Vec::new();
        statements.push(format!(
            "SET SESSION sql_mode = '{}'",
            advanced.mysql_sql_mode.trim()
        ));
        let time_zone = advanced.session_time_zone.trim();
        if !time_zone.is_empty() {
            statements.push(format!("SET SESSION time_zone = '{time_zone}'"));
        }
        if include_default_transaction_isolation {
            if let Some(level) = advanced.default_transaction_isolation.sql_level() {
                statements.push(format!("SET SESSION TRANSACTION ISOLATION LEVEL {level}"));
            }
        }
        statements
    }

    pub(crate) fn apply_mysql_connection_encoding_with_settings_for_db_type<C: Queryable>(
        conn: &mut C,
        advanced: &ConnectionAdvancedSettings,
        db_type: DatabaseType,
    ) -> Result<(), String> {
        let display_name = db_type.display_name();
        let database_collation = Self::mysql_current_database_collation_for_db_type(conn, db_type);
        let statement =
            Self::mysql_set_names_statement_with_settings(database_collation.as_deref(), advanced);

        if let Err(err) = conn.query_drop(statement.as_str()) {
            return Err(format!(
                "Failed to apply {display_name} session setting `{statement}`: {err}"
            ));
        }
        Ok(())
    }

    fn mysql_current_database_collation_for_db_type<C: Queryable>(
        conn: &mut C,
        db_type: DatabaseType,
    ) -> Option<String> {
        let display_name = db_type.display_name();
        match conn.query_first::<String, _>(
            "SELECT DEFAULT_COLLATION_NAME \
             FROM INFORMATION_SCHEMA.SCHEMATA \
             WHERE SCHEMA_NAME = DATABASE()",
        ) {
            Ok(Some(collation)) => return Some(collation.trim().to_string()),
            Ok(None) => {}
            Err(err) => {
                eprintln!(
                    "Warning: failed to read {display_name} current database collation for session setup: {err}"
                );
            }
        }

        match conn.query_first::<String, _>("SELECT @@collation_database") {
            Ok(value) => value.map(|collation| collation.trim().to_string()),
            Err(err) => {
                eprintln!(
                    "Warning: failed to read {display_name} database collation for session setup: {err}"
                );
                None
            }
        }
    }

    #[cfg(test)]
    fn mysql_set_names_statement(database_collation: Option<&str>) -> String {
        Self::mysql_set_names_statement_with_settings(
            database_collation,
            &ConnectionAdvancedSettings::default_for(DatabaseType::MySQL),
        )
    }

    fn mysql_set_names_statement_with_settings(
        database_collation: Option<&str>,
        advanced: &ConnectionAdvancedSettings,
    ) -> String {
        let charset = advanced.mysql_charset.trim();
        let configured_collation = advanced.mysql_collation.trim();
        if !configured_collation.is_empty()
            && Self::mysql_collation_name_is_safe(configured_collation)
        {
            return format!("SET NAMES {charset} COLLATE {configured_collation}");
        }

        match database_collation.map(str::trim) {
            Some(collation)
                if !collation.is_empty()
                    && Self::mysql_collation_name_is_safe(collation)
                    && mysql_collation_matches_charset(collation, charset) =>
            {
                format!("SET NAMES {charset} COLLATE {collation}")
            }
            _ => format!("SET NAMES {charset}"),
        }
    }

    fn mysql_collation_name_is_safe(collation: &str) -> bool {
        collation
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    }

    fn oracle_identifier_needs_quotes(identifier: &str) -> bool {
        let mut chars = identifier.chars();
        let Some(first) = chars.next() else {
            return true;
        };
        if !(first.is_ascii_alphabetic() || matches!(first, '_' | '$' | '#')) {
            return true;
        }
        if identifier.bytes().any(|byte| byte.is_ascii_lowercase()) {
            return true;
        }
        !chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$' | '#'))
    }

    pub(crate) fn quote_oracle_identifier(identifier: &str) -> String {
        let trimmed = identifier.trim();
        if trimmed.is_empty() {
            return "\"\"".to_string();
        }
        if trimmed.starts_with('"') && trimmed.ends_with('"') {
            return trimmed.to_string();
        }
        if Self::oracle_identifier_needs_quotes(trimmed) {
            format!("\"{}\"", trimmed.replace('"', "\"\""))
        } else {
            trimmed.to_string()
        }
    }

    fn oracle_set_current_schema_statement(schema: &str) -> String {
        format!(
            "ALTER SESSION SET CURRENT_SCHEMA = {}",
            Self::quote_oracle_identifier(schema)
        )
    }

    fn read_oracle_current_schema(conn: &Connection) -> Result<String, String> {
        let sql = "SELECT SYS_CONTEXT('USERENV', 'CURRENT_SCHEMA') FROM dual";
        let mut stmt = conn.statement(sql).build().map_err(|err| err.to_string())?;
        let row = stmt.query_row(&[]).map_err(|err| err.to_string())?;
        row.get::<_, Option<String>>(0)
            .map_err(|err| err.to_string())
            .map(|value| value.unwrap_or_default().trim().to_string())
    }

    fn read_oracle_thin_current_schema(session: &mut OracleThinSession) -> Result<String, String> {
        Self::oracle_thin_select_one_text(
            session,
            "SELECT SYS_CONTEXT('USERENV', 'CURRENT_SCHEMA') FROM dual",
        )
        .map(|value| value.unwrap_or_default().trim().to_string())
    }

    fn read_oracle_default_transaction_isolation(
        conn: &Connection,
    ) -> Result<Option<TransactionIsolation>, String> {
        let sql = "\
            SELECT value \
            FROM v$ses_optimizer_env \
            WHERE sid = SYS_CONTEXT('USERENV', 'SID') \
              AND name = 'transaction_isolation_level'";
        let mut stmt = conn.statement(sql).build().map_err(|err| err.to_string())?;
        let row = stmt.query_row(&[]).map_err(|err| err.to_string())?;
        let raw = row
            .get::<_, Option<String>>(0)
            .map_err(|err| err.to_string())?
            .unwrap_or_default();
        Ok(TransactionIsolation::from_sql_level(&raw))
    }

    pub(crate) fn apply_oracle_current_schema(
        conn: &Connection,
        schema: Option<&str>,
    ) -> Result<(), String> {
        let Some(schema) = schema.and_then(Self::normalize_oracle_current_schema_name) else {
            return Ok(());
        };

        let statement = Self::oracle_set_current_schema_statement(&schema);
        conn.execute(&statement, &[])
            .map(|_| ())
            .map_err(|err| err.to_string())
    }

    pub(crate) fn apply_oracle_thin_current_schema(
        session: &mut OracleThinSession,
        schema: Option<&str>,
    ) -> Result<(), String> {
        let Some(schema) = schema.and_then(Self::normalize_oracle_current_schema_name) else {
            return Ok(());
        };

        let statement = Self::oracle_set_current_schema_statement(&schema);
        session
            .query_drop(&statement)
            .map_err(|err| err.to_string())?;
        session
            .flush_pending_cursor_closes()
            .map_err(|err| err.to_string())
    }

    /// Tracked-schema variant of `apply_oracle_thin_current_schema`: a
    /// dropped tracked schema is skipped instead of failing the caller, same
    /// as `apply_tracked_oracle_current_schema` on the OCI side.
    pub(crate) fn apply_tracked_oracle_thin_current_schema(
        session: &mut OracleThinSession,
        schema: Option<&str>,
    ) -> Result<(), String> {
        match Self::apply_oracle_thin_current_schema(session, schema) {
            Err(message) if Self::oracle_missing_current_schema_error(&message) => {
                logging::log_warning(
                    "oracle pool session",
                    &format!(
                        "Tracked Oracle current schema {:?} is not available; continuing without re-applying it",
                        schema.unwrap_or_default()
                    ),
                );
                Ok(())
            }
            other => other,
        }
    }

    pub(crate) fn oracle_thin_select_one_text(
        session: &mut OracleThinSession,
        sql: &str,
    ) -> Result<Option<String>, String> {
        let request = StatementRequest::query(sql, 1);
        let result = session
            .query_described_fetch_all_request(&request)
            .map_err(|err| err.to_string())?;
        Ok(result
            .result
            .rows
            .first()
            .and_then(|row| match row.first() {
                Some(OracleValue::Text(value)) => Some(value.trim().to_string()),
                Some(OracleValue::Number(value)) => Some(value.trim().to_string()),
                Some(OracleValue::Boolean(value)) => Some(if *value {
                    "TRUE".to_string()
                } else {
                    "FALSE".to_string()
                }),
                Some(OracleValue::DateTime(value)) => Some(format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                    value.year, value.month, value.day, value.hour, value.minute, value.second
                )),
                Some(OracleValue::Timestamp(value)) => {
                    let mut text = format!(
                        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:06}",
                        value.year,
                        value.month,
                        value.day,
                        value.hour,
                        value.minute,
                        value.second,
                        value.nanosecond / 1_000
                    );
                    if let Some(suffix) = value.timezone_suffix() {
                        text.push_str(&suffix);
                    }
                    Some(text)
                }
                Some(OracleValue::Null) | None => None,
                Some(OracleValue::Lob(_))
                | Some(OracleValue::Bytes(_))
                | Some(OracleValue::JsonId(_))
                | Some(OracleValue::Cursor(_))
                | Some(OracleValue::Object(_))
                | Some(OracleValue::Array(_))
                | Some(OracleValue::IndexedArray(_)) => None,
            }))
    }

    fn apply_mysql_autocommit_setting_for_db_type<C: Queryable>(
        conn: &mut C,
        enabled: bool,
        db_type: DatabaseType,
    ) -> Result<(), String> {
        let statement = if enabled {
            "SET autocommit = 1"
        } else {
            "SET autocommit = 0"
        };
        let display_name = db_type.display_name();

        conn.query_drop(statement).map_err(|err| {
            format!("Failed to apply {display_name} autocommit setting `{statement}`: {err}")
        })
    }

    pub(crate) fn apply_mysql_session_transaction_options<C: Queryable>(
        conn: &mut C,
        auto_commit: bool,
        transaction_mode: TransactionMode,
        db_type: DatabaseType,
        default_transaction_isolation: TransactionIsolation,
    ) -> Result<(), String> {
        Self::apply_mysql_autocommit_setting_for_db_type(conn, auto_commit, db_type)?;
        Self::apply_mysql_transaction_mode_for_db_with_default(
            conn,
            transaction_mode,
            db_type,
            default_transaction_isolation,
        )
    }

    pub(crate) fn oracle_session_may_have_uncommitted_work(
        conn: &Connection,
        log_context: &str,
    ) -> bool {
        let result = (|| -> Result<bool, OracleError> {
            let stmt = conn.execute_named(
                Self::oracle_session_transaction_probe_sql(),
                &[("transaction_id", &OracleType::Varchar2(128))],
            )?;
            let transaction_id: Option<String> = stmt.bind_value("transaction_id")?;
            Ok(transaction_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()))
        })();

        match result {
            Ok(has_transaction) => has_transaction,
            Err(err) => {
                logging::log_error(
                    log_context,
                    &format!("Failed to inspect Oracle session transaction state: {err}"),
                );
                true
            }
        }
    }

    pub(crate) fn oracle_thin_session_may_have_uncommitted_work(
        session: &mut OracleThinSession,
        _log_context: &str,
    ) -> bool {
        // Match python-oracledb thin: transaction state is tracked from the
        // server call-status flags, not by issuing a SQL probe. Oracle SQL
        // treats LOCAL_TRANSACTION_ID(FALSE)'s PL/SQL boolean argument as an
        // identifier on older versions, which raises ORA-00904 during cleanup.
        session.transaction_in_progress()
    }

    fn oracle_session_transaction_probe_sql() -> &'static str {
        "BEGIN :transaction_id := DBMS_TRANSACTION.LOCAL_TRANSACTION_ID(FALSE); END;"
    }

    pub(crate) fn mysql_session_uncommitted_work_probe<C: Queryable>(
        conn: &mut C,
        log_context: &str,
        fallback_on_error: bool,
        db_type: DatabaseType,
    ) -> TransactionProbeResult {
        let display_name = db_type.display_name();
        let mut errors: Vec<String> = Vec::new();
        for probe_sql in Self::mysql_transaction_probe_sql_order(db_type) {
            match conn.query_first::<u64, _>(*probe_sql) {
                Ok(Some(value)) => {
                    return TransactionProbeResult {
                        may_have_uncommitted_work: value != 0,
                        used_fallback: false,
                    }
                }
                // A probe that yields no row did not answer (the
                // performance_schema probe fails closed this way when the
                // instrumentation is unavailable) — try the next one.
                Ok(None) => errors.push(format!("probe returned no row: {probe_sql}")),
                Err(err) => errors.push(err.to_string()),
            }
        }
        logging::log_error(
            log_context,
            &format!(
                "Failed to inspect {display_name} session transaction state; every probe failed ({}). \
                 The probes need SELECT on performance_schema (MySQL) or the PROCESS privilege \
                 (information_schema.innodb_trx); without one of them the session is treated as possibly dirty.",
                errors.join("; ")
            ),
        );
        TransactionProbeResult {
            may_have_uncommitted_work: fallback_on_error,
            used_fallback: true,
        }
    }

    /// Dialect-ordered probes; the first that answers wins.
    ///
    /// - `@@in_transaction` exists only on MariaDB (accurate there; an
    ///   implicit read-only transaction under `autocommit=0` reports 0).
    /// - The `performance_schema` transaction event is the accurate MySQL
    ///   equivalent (instrumentation is on by default since 8.0) — verified
    ///   live: implicit read-only tx → 0, uncommitted DML → 1, no stale
    ///   entries after COMMIT/ROLLBACK.
    /// - `innodb_trx` is the last resort only: self-probing it from inside a
    ///   transaction leaves a stale RUNNING entry on MySQL 8.0 that outlives
    ///   ROLLBACK, so it must never rank above the accurate probes.
    ///
    /// Each dialect keeps the other's probe in its chain so a server
    /// connected under the wrong profile type still gets an accurate answer.
    ///
    /// Public so the live verification harness probes with exactly the SQL
    /// the app ships instead of a copy that could drift.
    pub fn mysql_transaction_probe_sql_order(db_type: DatabaseType) -> &'static [&'static str] {
        const MARIADB_PROBES: [&str; 3] = [
            DatabaseConnection::mysql_session_transaction_probe_sql(),
            DatabaseConnection::mysql_performance_schema_transaction_probe_sql(),
            DatabaseConnection::mysql_innodb_transaction_probe_sql(),
        ];
        const MYSQL_PROBES: [&str; 3] = [
            DatabaseConnection::mysql_performance_schema_transaction_probe_sql(),
            DatabaseConnection::mysql_session_transaction_probe_sql(),
            DatabaseConnection::mysql_innodb_transaction_probe_sql(),
        ];
        match db_type {
            DatabaseType::MariaDB => &MARIADB_PROBES,
            // Oracle never reaches the MySQL probe; listed only to keep the
            // DatabaseType dispatch exhaustive.
            DatabaseType::MySQL | DatabaseType::Oracle => &MYSQL_PROBES,
        }
    }

    const fn mysql_session_transaction_probe_sql() -> &'static str {
        "SELECT @@in_transaction"
    }

    /// The HAVING guard makes the probe fail closed: with the Performance
    /// Schema disabled `PS_CURRENT_THREAD_ID()` is NULL and an unguarded
    /// COUNT(*) would "answer" 0 — a false clean that would stop the probe
    /// chain before the fallbacks run. With the guard the query returns no
    /// row instead, which the caller treats as unanswered.
    const fn mysql_performance_schema_transaction_probe_sql() -> &'static str {
        "\
            SELECT COUNT(*) \
            FROM performance_schema.events_transactions_current \
            WHERE THREAD_ID = PS_CURRENT_THREAD_ID() \
              AND STATE = 'ACTIVE' \
            HAVING PS_CURRENT_THREAD_ID() IS NOT NULL"
    }

    /// Counts only transactions with something to lose (modified rows or held
    /// locks). Under `autocommit=0` every statement — including this probe —
    /// registers an implicit read-only transaction in `innodb_trx`, so an
    /// unfiltered count reports a permanently dirty session and the
    /// auto-commit toggle can never be enabled again (verified live on MySQL
    /// 8.0 and MariaDB; MariaDB's own `@@in_transaction` likewise reports 0
    /// for such implicit read transactions).
    const fn mysql_innodb_transaction_probe_sql() -> &'static str {
        "\
            SELECT COUNT(*) \
            FROM information_schema.innodb_trx \
            WHERE trx_mysql_thread_id = CONNECTION_ID() \
              AND (trx_rows_modified > 0 OR trx_rows_locked > 0)"
    }

    pub(crate) fn mysql_session_may_have_uncommitted_work<C: Queryable>(
        conn: &mut C,
        log_context: &str,
        fallback_on_error: bool,
        db_type: DatabaseType,
    ) -> bool {
        Self::mysql_session_uncommitted_work_probe(conn, log_context, fallback_on_error, db_type)
            .may_have_uncommitted_work
    }

    pub fn ensure_transaction_option_change_allowed(
        transaction_state: TransactionSessionState,
        action: &str,
    ) -> Result<(), String> {
        Self::ensure_retained_session_option_change_allowed(
            RetainedSessionState::from_transaction_state(transaction_state),
            action,
        )
    }

    pub fn ensure_retained_session_option_change_allowed(
        retained_state: RetainedSessionState,
        action: &str,
    ) -> Result<(), String> {
        if retained_session_state_preflight_decision(
            RetainedSessionPreflightAction::TransactionOptionChange,
            retained_state,
        ) == RetainedSessionPreflightDecision::Allow
        {
            Ok(())
        } else {
            Err(format!(
                "Cannot change {action} while the current DB session is {}. Commit, rollback, or discard it first.",
                retained_state.label()
            ))
        }
    }

    fn live_transaction_session_state(&mut self, log_context: &str) -> TransactionSessionState {
        if !self.connected || self.connection.is_none() {
            return TransactionSessionState::Clean;
        }

        match self.connection.as_mut() {
            Some(DbConnection::Oracle(conn)) => TransactionSessionState::from_flags(
                Self::oracle_session_may_have_uncommitted_work(conn.as_ref(), log_context),
                false,
            ),
            Some(DbConnection::OracleThin(conn)) => {
                let has_uncommitted = match conn.lock() {
                    Ok(mut guard) => {
                        Self::oracle_thin_session_may_have_uncommitted_work(&mut guard, log_context)
                    }
                    Err(_) => {
                        logging::log_error(
                            log_context,
                            "Failed to inspect Oracle thin session transaction state: mutex poisoned",
                        );
                        true
                    }
                };
                TransactionSessionState::from_flags(has_uncommitted, false)
            }
            Some(DbConnection::MySQL { conn, db_type }) => TransactionSessionState::from_flags(
                Self::mysql_session_may_have_uncommitted_work(conn, log_context, true, *db_type),
                false,
            ),
            None => TransactionSessionState::Clean,
        }
    }

    fn ensure_live_transaction_option_change_allowed(
        &mut self,
        action: &str,
    ) -> Result<(), String> {
        let transaction_state = self.live_transaction_session_state(action);
        Self::ensure_transaction_option_change_allowed(transaction_state, action)
    }

    pub fn disconnect(&mut self) {
        self.clear_connection_state(None);
    }

    fn clear_connection_state(&mut self, disconnect_reason: Option<String>) {
        let had_connection = self.connection.is_some() || self.connected;
        let retired_connection = self.connection.take();
        let retired_pool = self.pool.take();
        self.connected = false;
        self.last_disconnect_reason = disconnect_reason;
        self.info.clear_password();
        self.info = ConnectionInfo::default();
        ConnectionInfo::clear_secret(&mut self.session_password);
        self.oracle_current_schema = None;
        self.auto_commit = false;
        self.transaction_mode = TransactionMode::default();
        self.default_transaction_isolation = TransactionIsolation::Default;
        if had_connection {
            self.bump_connection_generation();
            self.bump_pool_context_epoch();
        }
        update_session_state_without_blocking(
            &self.session,
            &self.pool_context_epoch,
            self.current_pool_context_epoch(),
            |session| session.reset_for_connection(DatabaseType::default()),
        );
        Self::retire_connection_resources_in_background(retired_connection, retired_pool);
    }

    fn disconnect_message(&self) -> String {
        self.last_disconnect_reason
            .clone()
            .unwrap_or_else(|| NOT_CONNECTED_MESSAGE.to_string())
    }

    /// Returns the Oracle connection if connected to Oracle.
    /// For backward compatibility with existing Oracle-specific code paths.
    pub fn require_live_connection(&mut self) -> Result<Arc<Connection>, String> {
        let db_conn = self.require_live_db_connection()?;
        match db_conn {
            DbConnection::Oracle(conn) => {
                self.apply_tracked_oracle_current_schema(conn.as_ref())?;
                Ok(conn)
            }
            DbConnection::OracleThin(_) => {
                Err("Expected Oracle OCI connection but found Oracle Thin connection".to_string())
            }
            DbConnection::MySQL { .. } => {
                Err("Expected Oracle connection but found MySQL-family connection".to_string())
            }
        }
    }

    /// Returns the underlying DbConnection enum for dispatch-based code.
    pub fn require_live_db_connection(&mut self) -> Result<DbConnection, String> {
        if !self.connected {
            if self.connection.is_some() {
                self.clear_connection_state(Some(NOT_CONNECTED_MESSAGE.to_string()));
            }
            return Err(self.disconnect_message());
        }

        if self.connection.is_none() {
            self.clear_connection_state(Some(NOT_CONNECTED_MESSAGE.to_string()));
            return Err(self.disconnect_message());
        }

        self.get_db_connection()
            .ok_or_else(|| self.disconnect_message())
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    pub fn has_connection_handle(&self) -> bool {
        self.connection.is_some()
    }

    /// Returns the Oracle connection (backward compat).
    pub fn get_connection(&self) -> Option<Arc<Connection>> {
        match &self.connection {
            Some(DbConnection::Oracle(conn)) => Some(Arc::clone(conn)),
            Some(DbConnection::OracleThin(_)) | Some(DbConnection::MySQL { .. }) | None => None,
        }
    }

    pub fn get_oracle_thin_connection(&self) -> Option<Arc<Mutex<OracleThinSession>>> {
        match &self.connection {
            Some(DbConnection::OracleThin(conn)) => Some(Arc::clone(conn)),
            Some(DbConnection::Oracle(_)) | Some(DbConnection::MySQL { .. }) | None => None,
        }
    }

    /// Returns the DbConnection enum clone.
    pub fn get_db_connection(&self) -> Option<DbConnection> {
        match &self.connection {
            Some(DbConnection::Oracle(conn)) => Some(DbConnection::Oracle(Arc::clone(conn))),
            Some(DbConnection::OracleThin(conn)) => {
                Some(DbConnection::OracleThin(Arc::clone(conn)))
            }
            Some(DbConnection::MySQL { .. }) => {
                // MySQL connections are not Arc-wrapped; return None here.
                // Use get_mysql_connection_mut() via mutable access instead.
                None
            }
            None => None,
        }
    }

    /// Returns a mutable reference to the MySQL connection, if connected to MySQL.
    pub fn get_mysql_connection_mut(&mut self) -> Option<&mut mysql::Conn> {
        match &mut self.connection {
            Some(DbConnection::MySQL { conn, .. }) => Some(conn),
            Some(DbConnection::Oracle(_)) | Some(DbConnection::OracleThin(_)) | None => None,
        }
    }

    pub fn db_type(&self) -> DatabaseType {
        self.info.db_type
    }

    pub fn get_info(&self) -> &ConnectionInfo {
        &self.info
    }

    pub fn tracked_oracle_current_schema(&self) -> Option<&str> {
        self.oracle_current_schema.as_deref()
    }

    pub fn current_scope_name(&self) -> Option<String> {
        backend_for(self.info.db_type).current_scope_name(self)
    }

    pub fn switch_scope(&mut self, target_scope: &str) -> Result<(), String> {
        let db_type = self.info.db_type;
        backend_for(db_type).switch_scope(self, target_scope)
    }

    pub fn runtime_connection_info_for(&self, db_type: DatabaseType) -> Option<ConnectionInfo> {
        if !self.info.db_type.is_same_type_as(db_type) {
            return None;
        }

        self.runtime_connection_info()
    }

    pub fn runtime_connection_info(&self) -> Option<ConnectionInfo> {
        if !self.connected || self.connection.is_none() {
            return None;
        }

        let mut info = self.info.clone();
        info.password = self.session_password.clone();
        Some(info)
    }

    pub fn pool_session_context_for(
        &self,
        db_type: DatabaseType,
    ) -> Result<DbPoolSessionContext, String> {
        if !self.can_reuse_pool_session(self.connection_generation, db_type) {
            return Err(NOT_CONNECTED_MESSAGE.to_string());
        }

        let pool = self
            .get_pool()
            .ok_or_else(|| format!("{} connection pool is not available", db_type))?;
        let mut connection_info = self.info.clone();
        connection_info.password = self.session_password.clone();

        Ok(DbPoolSessionContext {
            connection_generation: self.connection_generation,
            connection_id: self.connection_id,
            connection_info,
            pool,
            connection_pool_size: self.connection_pool_size,
            current_service_name: self.info.service_name.clone(),
            oracle_current_schema: self.oracle_current_schema.clone(),
            auto_commit: self.auto_commit,
            transaction_mode: self.transaction_mode,
            default_transaction_isolation: self.default_transaction_isolation,
            cache_epoch: self.current_pool_context_epoch(),
            cache_epoch_token: Arc::clone(&self.pool_context_epoch),
        })
    }

    pub fn pool_session_context(&self) -> Result<DbPoolSessionContext, String> {
        self.pool_session_context_for(self.info.db_type)
    }

    pub fn get_pool(&self) -> Option<DbConnectionPool> {
        self.pool.clone()
    }

    /// Acquire a session without publishing it to the activity registry.
    ///
    /// Test-only: production code goes through [`DbPoolSessionContext`] or
    /// [`DbConnectionPool::acquire_session`], both of which require an activity
    /// so the work stays visible, cancelable, and sweepable.
    #[cfg(test)]
    pub fn acquire_pool_session(&self) -> Result<Option<DbPoolSession>, String> {
        let mut session = self
            .pool
            .as_ref()
            .map(DbConnectionPool::acquire_session_untracked)
            .transpose()?;

        if let Some(session) = session.as_mut() {
            self.pool_session_context()?
                .apply_current_scope_to_session(session)?;
        }

        Ok(session)
    }

    pub fn connection_pool_size(&self) -> u32 {
        self.connection_pool_size
    }

    pub fn set_connection_pool_size(&mut self, size: u32) {
        self.connection_pool_size = Self::clamp_connection_pool_size(size);
    }

    pub fn resize_current_connection_pool(&mut self, size: u32) -> Result<(), String> {
        self.resize_current_connection_pool_with_policy(size, ConnectionAttemptPolicy::runtime())
    }

    pub(crate) fn resize_current_connection_pool_with_policy(
        &mut self,
        size: u32,
        policy: ConnectionAttemptPolicy,
    ) -> Result<(), String> {
        let size = Self::clamp_connection_pool_size(size);
        if self.connection_pool_size == size {
            return Ok(());
        }

        if !self.connected || self.connection.is_none() {
            self.connection_pool_size = size;
            return Ok(());
        }

        let mut info = self.info.clone();
        info.password = self.session_password.clone();
        let description = info.connection_attempt_description("Rebuilding");
        let pool = run_connection_attempt(policy, description, move || {
            Self::build_pool_for_info(&info, size, policy)
        })?;
        let retired_pool = self.pool.replace(pool);
        self.connection_pool_size = size;
        self.bump_connection_generation();
        self.bump_pool_context_epoch();
        if let Some(retired_pool) = retired_pool {
            Self::retire_connection_resources_in_background(None, Some(retired_pool));
        }
        Ok(())
    }

    pub fn connection_generation(&self) -> u64 {
        self.connection_generation
    }

    pub fn pool_context_epoch(&self) -> u64 {
        self.current_pool_context_epoch()
    }

    pub fn can_reuse_pool_session(
        &self,
        connection_generation: u64,
        db_type: DatabaseType,
    ) -> bool {
        self.info.db_type.is_same_type_as(db_type)
            && self.connected
            && self.connection.is_some()
            && self.connection_generation == connection_generation
    }

    pub fn set_auto_commit(&mut self, enabled: bool) -> Result<(), String> {
        if self.auto_commit == enabled {
            return Ok(());
        }

        self.ensure_live_transaction_option_change_allowed("auto-commit")?;
        let db_type = self.info.db_type;
        if let Some(connection) = self.connection.as_mut() {
            backend_for(db_type).apply_auto_commit(connection, enabled)?;
        }
        self.auto_commit = enabled;
        self.bump_pool_context_epoch();
        Ok(())
    }

    pub fn auto_commit(&self) -> bool {
        self.auto_commit
    }

    fn sync_default_transaction_isolation(&mut self, db_type: DatabaseType) {
        let configured = self.info.advanced.default_transaction_isolation;
        if configured != TransactionIsolation::Default
            && db_type
                .supported_transaction_isolations()
                .contains(&configured)
        {
            self.default_transaction_isolation = configured;
            return;
        }

        self.default_transaction_isolation = self
            .read_current_default_transaction_isolation(db_type)
            .ok()
            .flatten()
            .unwrap_or_else(|| db_type.fallback_default_transaction_isolation());
    }

    fn read_current_default_transaction_isolation(
        &mut self,
        db_type: DatabaseType,
    ) -> Result<Option<TransactionIsolation>, String> {
        backend_for(db_type).read_current_default_transaction_isolation(&mut self.connection)
    }

    fn ensure_connected_db_type(&self, expected: DatabaseType) -> Result<(), String> {
        if !self.connected {
            return Err(format!(
                "Expected {} connection but none is active",
                expected
            ));
        }

        if self.info.db_type.is_same_type_as(expected) {
            Ok(())
        } else {
            Err(format!(
                "Expected {} connection but {} is active",
                expected, self.info.db_type
            ))
        }
    }

    fn ensure_connected_mysql_family(&self) -> Result<(), String> {
        if !self.connected {
            return Err("Expected MySQL/MariaDB connection but none is active".to_string());
        }

        match self.info.db_type {
            DatabaseType::MySQL | DatabaseType::MariaDB => Ok(()),
            DatabaseType::Oracle => Err(format!(
                "Expected MySQL/MariaDB connection but {} is active",
                self.info.db_type
            )),
        }
    }

    fn expected_connection_missing_message(&self) -> String {
        format!(
            "Expected {} connection but none is active",
            self.info.db_type
        )
    }

    pub fn set_transaction_mode(&mut self, mode: TransactionMode) -> Result<(), String> {
        let db_type = self.info.db_type;
        let backend = backend_for(db_type);
        backend.transaction_mode_statements(mode)?;
        if self.transaction_mode == mode {
            return Ok(());
        }
        self.ensure_live_transaction_option_change_allowed("transaction mode")?;
        backend.apply_transaction_mode_to_live_connection(
            &mut self.connection,
            mode,
            self.default_transaction_isolation,
        )?;
        self.transaction_mode = mode;
        self.bump_pool_context_epoch();
        Ok(())
    }

    pub fn transaction_mode(&self) -> TransactionMode {
        self.transaction_mode
    }

    pub fn default_transaction_isolation(&self) -> TransactionIsolation {
        self.default_transaction_isolation
    }

    pub fn transaction_mode_statements_for(
        db_type: DatabaseType,
        mode: TransactionMode,
    ) -> Result<Vec<String>, String> {
        backend_for(db_type).transaction_mode_statements(mode)
    }

    /// The statement that puts an Oracle session's isolation level back to the
    /// connection default, when one is needed.
    ///
    /// `ALTER SESSION SET ISOLATION_LEVEL` is SESSION persistent, so a user
    /// statement — which the tab adopts and shows on the toolbar — leaves the
    /// session on that level for good. `SET TRANSACTION ISOLATION LEVEL`
    /// cannot express "whatever the connection default is", and Oracle's
    /// statement list for the default mode is empty, so without this the tab
    /// would keep running on the abandoned level while the toolbar reads
    /// "Default". Only a tab that has actively selected a mode can be in that
    /// position: a tab that never touched the controls has adopted nothing.
    pub fn oracle_session_isolation_reset_statement(
        tab_selected_mode: Option<TransactionMode>,
        default_isolation: TransactionIsolation,
    ) -> Option<String> {
        let mode = tab_selected_mode?;
        if mode.isolation != TransactionIsolation::Default {
            // A non-default isolation is issued per transaction anyway, and
            // that overrides whatever the session carries.
            return None;
        }
        let level = default_isolation.sql_level()?;
        Some(format!("ALTER SESSION SET ISOLATION_LEVEL = {level}"))
    }

    /// Every statement an Oracle execution must issue to put the session into
    /// the tab's transaction mode: the session-level reset above (when the tab
    /// asks for the connection default) followed by the mode itself. Both
    /// Oracle drivers go through here so they cannot drift apart.
    pub fn oracle_transaction_mode_statements_for_tab(
        tab_selected_mode: Option<TransactionMode>,
        mode: TransactionMode,
        default_isolation: TransactionIsolation,
    ) -> Result<Vec<String>, String> {
        let mut statements = Vec::new();
        statements.extend(Self::oracle_session_isolation_reset_statement(
            tab_selected_mode,
            default_isolation,
        ));
        statements.extend(Self::transaction_mode_statements_for(
            DatabaseType::Oracle,
            mode,
        )?);
        Ok(statements)
    }

    /// Why this isolation/access pair cannot be applied on `db_type`, if it
    /// cannot. The toolbar exposes isolation and access mode as two
    /// independent choices, so a user can select a pair the backend has no
    /// statement for (Oracle cannot combine READ ONLY with an explicit
    /// isolation level). Reporting it where the pair is chosen keeps a mode
    /// that can never run off the tab, instead of failing every statement.
    pub fn transaction_mode_selection_error(
        db_type: DatabaseType,
        mode: TransactionMode,
    ) -> Option<String> {
        Self::transaction_mode_statements_for(db_type, mode).err()
    }

    pub(crate) fn transaction_mode_statements_for_with_default(
        db_type: DatabaseType,
        mode: TransactionMode,
        default_isolation: TransactionIsolation,
    ) -> Result<Vec<String>, String> {
        Self::transaction_mode_statements_for(
            db_type,
            Self::transaction_mode_with_default_substituted(db_type, mode, default_isolation),
        )
    }

    /// The mode the MySQL family will really SET: `Default` isolation means
    /// "the connection's configured default", so it is substituted here rather
    /// than left to the server's own default. Callers that need to know what a
    /// session should already carry (see
    /// `mysql_pooled_session_settings_already_applied`) must resolve it the
    /// same way the statements do, so both go through this.
    pub(crate) fn transaction_mode_with_default_substituted(
        db_type: DatabaseType,
        mode: TransactionMode,
        default_isolation: TransactionIsolation,
    ) -> TransactionMode {
        let mysql_family = match db_type {
            DatabaseType::MySQL | DatabaseType::MariaDB => true,
            DatabaseType::Oracle => false,
        };
        if mysql_family
            && mode.isolation == TransactionIsolation::Default
            && default_isolation != TransactionIsolation::Default
        {
            TransactionMode::new(default_isolation, mode.access_mode)
        } else {
            mode
        }
    }

    pub fn apply_oracle_transaction_mode(
        conn: &Connection,
        mode: TransactionMode,
    ) -> Result<(), String> {
        for statement in Self::transaction_mode_statements_for(DatabaseType::Oracle, mode)? {
            conn.execute(&statement, &[])
                .map_err(|err| format!("Failed to apply transaction mode: {err}"))?;
        }
        Ok(())
    }

    pub fn apply_oracle_thin_transaction_mode(
        session: &mut OracleThinSession,
        mode: TransactionMode,
    ) -> Result<(), String> {
        for statement in Self::transaction_mode_statements_for(DatabaseType::Oracle, mode)? {
            let request = StatementRequest::statement(statement.clone());
            session
                .execute_typed(&request, &[])
                .map_err(|err| format!("Failed to apply transaction mode: {err}"))?;
        }
        Ok(())
    }

    pub fn apply_mysql_transaction_mode<C: Queryable>(
        conn: &mut C,
        mode: TransactionMode,
    ) -> Result<(), String> {
        Self::apply_mysql_transaction_mode_for_db(conn, mode, DatabaseType::MySQL)
    }

    pub fn apply_mysql_transaction_mode_for_db<C: Queryable>(
        conn: &mut C,
        mode: TransactionMode,
        db_type: DatabaseType,
    ) -> Result<(), String> {
        Self::apply_mysql_transaction_mode_for_db_with_default(
            conn,
            mode,
            db_type,
            TransactionIsolation::Default,
        )
    }

    pub(crate) fn apply_mysql_transaction_mode_for_db_with_default<C: Queryable>(
        conn: &mut C,
        mode: TransactionMode,
        db_type: DatabaseType,
        default_isolation: TransactionIsolation,
    ) -> Result<(), String> {
        for statement in
            Self::transaction_mode_statements_for_with_default(db_type, mode, default_isolation)?
        {
            conn.query_drop(statement.as_str())
                .map_err(|err| format!("Failed to apply transaction mode: {err}"))?;
        }
        Ok(())
    }

    fn read_mysql_default_transaction_isolation<C: Queryable>(
        conn: &mut C,
    ) -> Result<Option<TransactionIsolation>, String> {
        let raw = match conn.query_first::<String, _>("SELECT @@transaction_isolation") {
            Ok(value) => value,
            Err(_) => conn
                .query_first::<String, _>("SELECT @@tx_isolation")
                .map_err(|err| err.to_string())?,
        };

        Ok(raw
            .as_deref()
            .and_then(TransactionIsolation::from_sql_level))
    }

    pub fn apply_tracked_oracle_current_schema(&self, conn: &Connection) -> Result<(), String> {
        self.apply_oracle_current_schema_for_scope(conn, None)
    }

    /// Put `conn` in the schema an operation with this `scope` runs in: the
    /// scope when it has one, this connection's tracked schema otherwise —
    /// the same rule as [`Self::oracle_session_schema_for_scope`],
    /// `mysql_database_for_scope` and `DbPoolSessionContext::for_scope`.
    ///
    /// Executions MUST go through this rather than the tracked schema alone.
    /// Scope is per query tab, and the tracked schema is per connection: a
    /// session moved by one tab (an `ALTER SESSION SET CURRENT_SCHEMA`, whose
    /// result is synced back here) would otherwise be forced onto every other
    /// tab's session at its next statement, and those tabs would run
    /// somewhere their own selector never pointed.
    pub fn apply_oracle_current_schema_for_scope(
        &self,
        conn: &Connection,
        scope: Option<&str>,
    ) -> Result<(), String> {
        Self::apply_tracked_oracle_current_schema_on_session(
            conn,
            self.oracle_session_schema_for_scope(scope).as_deref(),
        )
    }

    /// The schema a session prepared for `scope` must be put in: the tab's
    /// scope, else this connection's own schema, else the login user.
    ///
    /// The last fallback is what makes preparation total. A pooled session is
    /// recycled between tabs and keeps whatever schema its previous user left
    /// it in, and applying "no schema" is a no-op — so without a concrete
    /// name a tab with no scope of its own would silently inherit the last
    /// tab's schema. The MySQL twin has always been total for the same
    /// reason: `mysql_database_for_scope` never resolves to nothing.
    pub fn oracle_session_schema_for_scope(&self, scope: Option<&str>) -> Option<String> {
        oracle_session_schema(scope, self.oracle_current_schema.as_deref())
    }

    /// Tracked-schema variant of `apply_oracle_current_schema`, the OCI twin of
    /// [`Self::apply_tracked_oracle_thin_current_schema`].
    pub(crate) fn apply_tracked_oracle_current_schema_on_session(
        conn: &Connection,
        schema: Option<&str>,
    ) -> Result<(), String> {
        match Self::apply_oracle_current_schema(conn, schema) {
            Err(message) if Self::oracle_missing_current_schema_error(&message) => {
                // The tracked schema's user was dropped. The schema setting is
                // only a name-resolution namespace and the session itself is
                // still valid, so keep using it instead of failing every
                // statement (including the recovery ALTER SESSION) on
                // ORA-01435.
                logging::log_warning(
                    "oracle pool session",
                    &format!(
                        "Tracked Oracle current schema {:?} is not available; keeping the session without re-applying it",
                        schema.unwrap_or_default()
                    ),
                );
                Ok(())
            }
            other => other,
        }
    }

    /// The schema an operation runs under: the tab's selected scope when it
    /// has one, otherwise this connection's tracked schema.
    pub(crate) fn oracle_missing_current_schema_error(message: &str) -> bool {
        message.to_ascii_lowercase().contains("ora-01435")
    }

    pub fn clear_tracked_oracle_current_schema(&mut self) {
        self.set_tracked_oracle_current_schema(None);
    }

    pub fn apply_tracked_mysql_current_database(&mut self) -> Result<(), String> {
        self.apply_mysql_current_database_for_scope(None)
    }

    /// Point the live session at the database a tab-initiated operation must
    /// run in. A query tab carries its own selected database, so the tab's
    /// scope wins over the connection's tracked one. Applying it on every such
    /// operation is also what keeps the shared live session honest: the next
    /// operation re-applies its own scope instead of inheriting this one.
    pub fn apply_mysql_current_database_for_scope(
        &mut self,
        scope: Option<&str>,
    ) -> Result<(), String> {
        self.ensure_connected_mysql_family()?;

        let db_type = self.info.db_type;
        let target_database = self.mysql_database_for_scope(scope).to_string();
        let advanced = self.info.advanced.clone();
        let Some(conn) = self.get_mysql_connection_mut() else {
            return Err(self.expected_connection_missing_message());
        };

        if target_database.is_empty() {
            Self::reset_mysql_session_to_no_database_for_db_type(conn, db_type)?;
            return Self::apply_mysql_session_settings_for_db_type(conn, &advanced, db_type);
        }

        conn.select_db(target_database.as_str())
            .map_err(|err| err.to_string())?;
        Self::apply_mysql_connection_encoding_with_settings_for_db_type(conn, &advanced, db_type)
    }

    /// The database an operation runs in: the tab's selected scope when it has
    /// one, otherwise this connection's tracked database. Same rule as
    /// [`Self::oracle_session_schema_for_scope`] and
    /// `DbPoolSessionContext::for_scope`.
    pub fn mysql_database_for_scope<'a>(&'a self, scope: Option<&'a str>) -> &'a str {
        scope
            .map(str::trim)
            .filter(|scope| !scope.is_empty())
            .unwrap_or_else(|| self.info.service_name.trim())
    }

    pub fn sync_mysql_current_database_name(&mut self) -> Result<String, String> {
        self.ensure_connected_mysql_family()?;

        let db_type = self.info.db_type;
        let advanced = self.info.advanced.clone();
        let Some(conn) = self.get_mysql_connection_mut() else {
            return Err(self.expected_connection_missing_message());
        };

        let current_database = conn
            .query_first::<Option<String>, _>("SELECT DATABASE()")
            .map_err(|err| err.to_string())?
            .flatten()
            .map(|database| database.trim().to_string())
            .unwrap_or_default();
        Self::apply_mysql_connection_encoding_with_settings_for_db_type(conn, &advanced, db_type)?;
        if self.info.service_name != current_database {
            self.info.service_name = current_database.clone();
            self.bump_pool_context_epoch();
        }
        Ok(current_database)
    }

    /// The database a TAB's session is now in, read from that session, with
    /// its encoding refreshed.
    ///
    /// Read-only with respect to this connection: a tab's `USE` moves that
    /// tab's session, not the connection's. The connection's stored database
    /// is its own (the profile's), and it is what a tab with NO scope of its
    /// own falls back to — so recording one tab's `USE` there, and moving the
    /// shared live connection with it, dragged every such tab along. When an
    /// event really is the connection's (its database was dropped), the
    /// caller records it with
    /// [`Self::sync_mysql_current_database_name_from_known_name`].
    pub fn read_mysql_session_current_database<C: Queryable>(
        &self,
        conn: &mut C,
        refresh_encoding: bool,
    ) -> Result<String, String> {
        self.ensure_connected_mysql_family()?;

        let db_type = self.info.db_type;
        let advanced = self.info.advanced.clone();
        let current_database = conn
            .query_first::<Option<String>, _>("SELECT DATABASE()")
            .map_err(|err| err.to_string())?
            .flatten()
            .map(|database| database.trim().to_string())
            .unwrap_or_default();
        if refresh_encoding {
            Self::apply_mysql_connection_encoding_with_settings_for_db_type(
                conn, &advanced, db_type,
            )?;
        }
        Ok(current_database)
    }

    pub fn sync_mysql_current_database_name_from_known_name(
        &mut self,
        current_database: &str,
    ) -> Result<String, String> {
        self.ensure_connected_mysql_family()?;

        let db_type = self.info.db_type;
        let advanced = self.info.advanced.clone();
        let current_database = current_database.trim().to_string();
        let Some(primary_conn) = self.get_mysql_connection_mut() else {
            return Err(self.expected_connection_missing_message());
        };
        if current_database.is_empty() {
            Self::reset_mysql_session_to_no_database_for_db_type(primary_conn, db_type)?;
        } else {
            primary_conn
                .select_db(current_database.as_str())
                .map_err(|err| err.to_string())?;
        }
        Self::apply_mysql_connection_encoding_with_settings_for_db_type(
            primary_conn,
            &advanced,
            db_type,
        )?;
        if self.info.service_name != current_database {
            self.info.service_name = current_database.clone();
            self.bump_pool_context_epoch();
        }
        Ok(current_database)
    }

    /// Switches the primary connection's current database. Per session.md §2.6
    /// the caller is responsible for propagating the change to all retained
    /// pooled sessions that share the same `connection_generation` (typically
    /// via `apply_retained_scope_update` from the main window). Retained
    /// sessions that are not propagated immediately will receive the new scope
    /// at next lease via `apply_current_scope_to_session`.
    pub fn switch_mysql_database(&mut self, database: &str) -> Result<(), String> {
        self.ensure_connected_mysql_family()?;

        let db_type = self.info.db_type;
        let target_database = database.trim();
        let advanced = self.info.advanced.clone();
        let Some(conn) = self.get_mysql_connection_mut() else {
            return Err(self.expected_connection_missing_message());
        };

        if target_database.is_empty() {
            Self::reset_mysql_session_to_no_database_for_db_type(conn, db_type)?;
            Self::apply_mysql_session_settings_for_db_type(conn, &advanced, db_type)?;
        } else {
            conn.select_db(target_database)
                .map_err(|err| err.to_string())?;
            Self::apply_mysql_connection_encoding_with_settings_for_db_type(
                conn, &advanced, db_type,
            )?;
        }
        if self.info.service_name != target_database {
            self.info.service_name = target_database.to_string();
            self.bump_pool_context_epoch();
        }
        Ok(())
    }

    /// The schema a TAB's session is now in, read from that session.
    ///
    /// Deliberately read-only: a tab moving its own session must not write
    /// this connection's schema. That value is the connection's own (its
    /// login/configured schema), and it is what a tab with no scope of its
    /// own falls back to — so recording one tab's `ALTER SESSION` there
    /// dragged every scope-less tab along with it.
    pub fn read_oracle_session_current_schema(&self, conn: &Connection) -> Result<String, String> {
        self.ensure_connected_db_type(DatabaseType::Oracle)?;
        Self::read_oracle_current_schema(conn)
    }

    pub fn read_oracle_thin_session_current_schema(
        &self,
        session: &mut OracleThinSession,
    ) -> Result<String, String> {
        self.ensure_connected_db_type(DatabaseType::Oracle)?;
        Self::read_oracle_thin_current_schema(session)
    }

    /// Record the schema this connection logged into, read from the server.
    pub fn sync_oracle_current_schema_after_connect(&mut self) -> Result<(), String> {
        match self.require_live_db_connection()? {
            DbConnection::Oracle(conn) => {
                let schema = Self::read_oracle_current_schema(conn.as_ref())?;
                self.set_tracked_oracle_current_schema(Some(schema));
                Ok(())
            }
            DbConnection::OracleThin(conn) => {
                let schema = {
                    let mut session = conn
                        .lock()
                        .map_err(|_| "Oracle Thin connection lock was poisoned".to_string())?;
                    Self::read_oracle_thin_current_schema(&mut session)?
                };
                self.set_tracked_oracle_current_schema(Some(schema));
                Ok(())
            }
            DbConnection::MySQL { .. } => {
                Err("Expected Oracle connection but found MySQL-family connection".to_string())
            }
        }
    }

    /// Switches the primary Oracle connection's `CURRENT_SCHEMA`. Per
    /// session.md §2.6 the caller is responsible for propagating the change
    /// to retained pooled sessions for the same `connection_generation` (via
    /// `apply_retained_scope_update`). Retained sessions that are not
    /// propagated immediately will receive the new schema at next lease via
    /// `apply_oracle_tracked_schema_to_pooled_session_if_current`.
    pub fn switch_oracle_current_schema(&mut self, schema: &str) -> Result<(), String> {
        self.ensure_connected_db_type(DatabaseType::Oracle)?;

        let target_schema = schema.trim();
        if target_schema.is_empty() {
            return Err("Schema name cannot be empty".to_string());
        }

        match self.require_live_db_connection()? {
            DbConnection::Oracle(conn) => {
                Self::apply_oracle_current_schema(conn.as_ref(), Some(target_schema))?;
            }
            DbConnection::OracleThin(conn) => {
                let mut session = conn
                    .lock()
                    .map_err(|_| "Oracle Thin connection lock was poisoned".to_string())?;
                Self::apply_oracle_thin_current_schema(&mut session, Some(target_schema))?;
            }
            DbConnection::MySQL { .. } => {
                return Err(
                    "Expected Oracle connection but found MySQL-family connection".to_string(),
                );
            }
        }
        self.set_tracked_oracle_current_schema(Some(target_schema.to_string()));
        Ok(())
    }

    pub fn session_state(&self) -> Arc<Mutex<SessionState>> {
        Arc::clone(&self.session)
    }

    pub fn test_connection(info: &ConnectionInfo) -> Result<(), String> {
        Self::test_connection_with_policy(info, ConnectionAttemptPolicy::runtime())
    }

    pub(crate) fn test_connection_with_policy(
        info: &ConnectionInfo,
        policy: ConnectionAttemptPolicy,
    ) -> Result<(), String> {
        info.advanced
            .validate_for_db(info.db_type, info.uses_oracle_tns_alias())?;
        let info = info.clone();
        let description = info.connection_attempt_description("Testing");
        run_connection_attempt(policy, description, move || {
            backend_for(info.db_type).test_connection(&info, policy)
        })
    }

    #[cfg(test)]
    fn simulate_connected_metadata_for_test(&mut self, info: ConnectionInfo) {
        self.connected = true;
        self.session_password = info.password.clone();
        self.oracle_current_schema = None;
        self.info = info;
    }
}

impl Default for DatabaseConnection {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for DatabaseConnection {
    fn drop(&mut self) {
        let connection = self.connection.take();
        let pool = self.pool.take();
        ConnectionInfo::clear_secret(&mut self.session_password);
        self.info.clear_password();
        // Dropping is a teardown like any other -- a script CONNECT's
        // connection is torn down this way, by the tab that owned it going
        // away -- so the sessions retained from it have to go with it. There
        // is no generation bump on this path to carry that for us.
        reclaim_retired_connection_sessions_in_background(self.connection_generation);
        Self::retire_connection_resources_in_background(connection, pool);
    }
}

pub type SharedConnection = Arc<Mutex<DatabaseConnection>>;

#[derive(Clone)]
struct ActiveConnectionTransition {
    owner: Weak<Mutex<DatabaseConnection>>,
    attempt_id: u64,
    activity: String,
}

#[derive(Default)]
struct ConnectionTransitionRegistry {
    active: Mutex<HashMap<usize, ActiveConnectionTransition>>,
    changed: Condvar,
}

struct ConnectionTransitionGuard {
    connection: SharedConnection,
    key: usize,
    attempt_id: u64,
    expected_generation: u64,
    finished: bool,
}

static ACTIVE_DB_ACTIVITY: OnceLock<Mutex<Vec<TrackedDbActivity>>> = OnceLock::new();
static DB_POOL_SESSION_CONTEXT_CACHE: OnceLock<Mutex<HashMap<usize, CachedDbPoolSessionContext>>> =
    OnceLock::new();
static CONNECTION_TRANSITIONS: OnceLock<ConnectionTransitionRegistry> = OnceLock::new();
static NEXT_DB_ACTIVITY_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_DB_CANCELER_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_CONNECTION_ATTEMPT_ID: AtomicU64 = AtomicU64::new(1);
static ORACLE_CLIENT_INIT_SUCCESS: OnceLock<()> = OnceLock::new();
static ORACLE_CLIENT_INIT_ATTEMPT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Clone)]
struct CachedDbPoolSessionContext {
    owner: Weak<Mutex<DatabaseConnection>>,
    context: DbPoolSessionContext,
}

fn shared_connection_cache_key(connection: &SharedConnection) -> usize {
    Arc::as_ptr(connection) as usize
}

fn connection_transition_registry() -> &'static ConnectionTransitionRegistry {
    CONNECTION_TRANSITIONS.get_or_init(ConnectionTransitionRegistry::default)
}

/// Deliberately NOT tracked: this guard is handed to a `Condvar`, which
/// releases the mutex while waiting. A held-scope around it would claim the
/// lock is held during the wait, which is exactly wrong.
fn lock_connection_transition_state(
) -> MutexGuard<'static, HashMap<usize, ActiveConnectionTransition>> {
    connection_transition_registry()
        .active
        .lock()
        .unwrap_or_else(|poisoned| {
            logging::log_warning(
                "db::connection",
                "connection transition registry lock was poisoned; recovering",
            );
            poisoned.into_inner()
        })
}

fn remove_stale_connection_transitions(
    transitions: &mut HashMap<usize, ActiveConnectionTransition>,
) {
    transitions.retain(|_, transition| transition.owner.upgrade().is_some());
}

fn active_connection_transition(connection: &SharedConnection) -> Option<String> {
    let key = shared_connection_cache_key(connection);
    let mut transitions = lock_connection_transition_state();
    remove_stale_connection_transitions(&mut transitions);
    transitions.get(&key).map(|state| state.activity.clone())
}

pub(crate) fn connection_transition_activity(connection: &SharedConnection) -> Option<String> {
    active_connection_transition(connection)
}

fn connection_transition_is_current(key: usize, attempt_id: u64) -> bool {
    let mut transitions = lock_connection_transition_state();
    remove_stale_connection_transitions(&mut transitions);
    transitions
        .get(&key)
        .is_some_and(|state| state.attempt_id == attempt_id)
}

fn finish_connection_transition(key: usize, attempt_id: u64) {
    let registry = connection_transition_registry();
    let removed = {
        let mut transitions = lock_connection_transition_state();
        let should_remove = transitions
            .get(&key)
            .is_some_and(|state| state.attempt_id == attempt_id);
        if should_remove {
            transitions.remove(&key);
        }
        should_remove
    };
    if removed {
        registry.changed.notify_all();
    }
}

fn wait_for_connection_transition(connection: &SharedConnection) {
    let key = shared_connection_cache_key(connection);
    let registry = connection_transition_registry();
    let mut transitions = lock_connection_transition_state();
    loop {
        remove_stale_connection_transitions(&mut transitions);
        if !transitions.contains_key(&key) {
            return;
        }
        transitions = registry
            .changed
            .wait(transitions)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
}

fn lock_database_connection_raw(connection: &SharedConnection) -> DatabaseConnectionGuard<'_> {
    let _order =
        crate::db::lock_order::LockOrderScope::enter(crate::db::lock_order::names::DB_CONNECTION);
    DatabaseConnectionGuard {
        guard: lock_database_connection_unchecked(connection),
        _order,
    }
}

pub(crate) struct DatabaseConnectionGuard<'a> {
    guard: MutexGuard<'a, DatabaseConnection>,
    _order: crate::db::lock_order::LockOrderScope,
}

impl std::ops::Deref for DatabaseConnectionGuard<'_> {
    type Target = DatabaseConnection;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl std::ops::DerefMut for DatabaseConnectionGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

fn lock_database_connection_unchecked(
    connection: &SharedConnection,
) -> MutexGuard<'_, DatabaseConnection> {
    match connection.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            logging::log_warning(
                "db::connection",
                "database connection lock was poisoned; recovering",
            );
            poisoned.into_inner()
        }
    }
}

fn begin_connection_transition(
    connection: &SharedConnection,
    activity: impl Into<String>,
) -> Result<ConnectionTransitionGuard, String> {
    let key = shared_connection_cache_key(connection);
    let attempt_id = NEXT_CONNECTION_ATTEMPT_ID.fetch_add(1, Ordering::Relaxed);
    let activity = activity.into();
    {
        let mut transitions = lock_connection_transition_state();
        remove_stale_connection_transitions(&mut transitions);
        if let Some(active) = transitions.get(&key) {
            return Err(format!(
                "Connection is busy. Current DB activity: {}",
                active.activity
            ));
        }
        transitions.insert(
            key,
            ActiveConnectionTransition {
                owner: Arc::downgrade(connection),
                attempt_id,
                activity,
            },
        );
    }

    let connection_guard = match connection.try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::WouldBlock) => {
            finish_connection_transition(key, attempt_id);
            return Err(format_connection_busy_message());
        }
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            logging::log_warning(
                "db::connection",
                "database connection lock was poisoned; recovering",
            );
            poisoned.into_inner()
        }
    };
    let expected_generation = connection_guard.connection_generation();
    connection_guard.bump_pool_context_epoch();
    clear_pool_session_context_for_shared_connection(connection);
    drop(connection_guard);

    Ok(ConnectionTransitionGuard {
        connection: Arc::clone(connection),
        key,
        attempt_id,
        expected_generation,
        finished: false,
    })
}

impl ConnectionTransitionGuard {
    fn is_current(&self) -> bool {
        connection_transition_is_current(self.key, self.attempt_id)
    }

    fn finish(mut self) {
        finish_connection_transition(self.key, self.attempt_id);
        self.finished = true;
    }
}

impl Drop for ConnectionTransitionGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        // During panic unwinding, restoring this optional cache is not worth
        // waiting for the database mutex. The cache was invalidated when the
        // transition began and will be rebuilt on the next successful access.
        if !std::thread::panicking() && self.is_current() {
            let connection_guard = lock_database_connection_raw(&self.connection);
            refresh_pool_session_context_cache_for_shared_connection(
                &self.connection,
                &connection_guard,
            );
            drop(connection_guard);
        }
        finish_connection_transition(self.key, self.attempt_id);
    }
}

fn pool_context_cache_slot() -> &'static Mutex<HashMap<usize, CachedDbPoolSessionContext>> {
    DB_POOL_SESSION_CONTEXT_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_pool_context_cache() -> TrackedGuard<'static, HashMap<usize, CachedDbPoolSessionContext>> {
    let _order = crate::db::lock_order::LockOrderScope::enter(
        crate::db::lock_order::names::POOL_CONTEXT_CACHE,
    );
    TrackedGuard {
        guard: lock_pool_context_cache_raw(),
        _order,
    }
}

fn lock_pool_context_cache_raw() -> MutexGuard<'static, HashMap<usize, CachedDbPoolSessionContext>>
{
    match pool_context_cache_slot().lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            logging::log_warning(
                "db::connection",
                "DB pool context cache lock was poisoned; recovering",
            );
            poisoned.into_inner()
        }
    }
}

fn cache_pool_session_context(connection: &SharedConnection, context: &DbPoolSessionContext) {
    let key = shared_connection_cache_key(connection);
    if !context.cache_epoch_is_current() {
        remove_cached_pool_session_context(key);
        return;
    }

    let mut cached = context.clone();
    cached.connection_info.clear_password();
    lock_pool_context_cache().insert(
        key,
        CachedDbPoolSessionContext {
            owner: Arc::downgrade(connection),
            context: cached,
        },
    );
}

/// Drop every cached pool context whose connection has moved on.
///
/// The cache holds a CLONE of the connection's pool, so an entry left behind
/// by a disconnect keeps that pool alive -- and with it every idle session the
/// pool still owns, on a connection the user has already closed. Entries are
/// checked on read too, but nothing guarantees another read ever comes.
fn prune_stale_pool_session_context_cache() -> usize {
    // Take the stale entries out under the lock and drop them outside it:
    // dropping the last clone of a pool closes its sessions, which talks to
    // the server.
    let stale = {
        let mut cache = lock_pool_context_cache();
        let stale_keys = cache
            .iter()
            .filter(|(_, cached)| {
                cached.owner.upgrade().is_none() || !cached.context.cache_epoch_is_current()
            })
            .map(|(key, _)| *key)
            .collect::<Vec<_>>();
        stale_keys
            .into_iter()
            .filter_map(|key| cache.remove(&key))
            .collect::<Vec<_>>()
    };
    let pruned = stale.len();
    drop(stale);
    pruned
}

fn remove_cached_pool_session_context(key: usize) {
    lock_pool_context_cache().remove(&key);
}

fn cached_pool_session_context(key: usize) -> Option<DbPoolSessionContext> {
    let mut cache = lock_pool_context_cache();
    let cached = cache.get(&key)?;
    if cached.owner.upgrade().is_none() || !cached.context.cache_epoch_is_current() {
        cache.remove(&key);
        return None;
    }
    Some(cached.context.clone())
}

fn pool_session_context_identity_matches(
    left: &DbPoolSessionContext,
    right: &DbPoolSessionContext,
) -> bool {
    left.cache_epoch_is_current()
        && right.cache_epoch_is_current()
        && Arc::ptr_eq(&left.cache_epoch_token, &right.cache_epoch_token)
        && left.cache_epoch == right.cache_epoch
        && left.connection_generation == right.connection_generation
        && left
            .connection_info
            .db_type
            .is_same_type_as(right.connection_info.db_type)
        && left.connection_pool_size == right.connection_pool_size
        && left.current_service_name == right.current_service_name
        && left.oracle_current_schema == right.oracle_current_schema
        && left.auto_commit == right.auto_commit
        && left.transaction_mode == right.transaction_mode
        && left.default_transaction_isolation == right.default_transaction_isolation
}

pub fn clear_pool_session_context_for_shared_connection(connection: &SharedConnection) {
    remove_cached_pool_session_context(shared_connection_cache_key(connection));
}

pub fn cache_pool_session_context_for_shared_connection(
    connection: &SharedConnection,
    context: &DbPoolSessionContext,
) {
    cache_pool_session_context(connection, context);
}

pub fn refresh_pool_session_context_cache_for_shared_connection(
    connection: &SharedConnection,
    db_conn: &DatabaseConnection,
) -> Option<DbPoolSessionContext> {
    match db_conn.pool_session_context() {
        Ok(context) => {
            cache_pool_session_context(connection, &context);
            Some(context)
        }
        Err(_) => {
            clear_pool_session_context_for_shared_connection(connection);
            None
        }
    }
}

pub fn cached_pool_session_context_matches_shared_connection(
    connection: &SharedConnection,
    context: &DbPoolSessionContext,
) -> bool {
    cached_pool_session_context(shared_connection_cache_key(connection))
        .as_ref()
        .is_some_and(|cached| pool_session_context_identity_matches(cached, context))
}

pub fn pool_session_context_for_shared_connection(
    connection: &SharedConnection,
    activity: Option<&str>,
) -> Result<DbPoolSessionContext, String> {
    if let Some(activity) = active_connection_transition(connection) {
        return Err(format!(
            "Connection is busy. Current DB activity: {activity}"
        ));
    }
    let key = shared_connection_cache_key(connection);
    let conn_guard = match activity {
        Some(activity) => try_lock_connection_with_activity(connection, activity),
        None => try_lock_connection(connection),
    };

    let Some(conn_guard) = conn_guard else {
        if let Some(activity) = active_connection_transition(connection) {
            return Err(format!(
                "Connection is busy. Current DB activity: {activity}"
            ));
        }
        return cached_pool_session_context(key).ok_or_else(format_connection_busy_message);
    };

    match conn_guard.pool_session_context() {
        Ok(context) => {
            cache_pool_session_context(connection, &context);
            Ok(context)
        }
        Err(err) => {
            remove_cached_pool_session_context(key);
            Err(err)
        }
    }
}

fn ensure_oracle_client_initialized() -> Result<(), OracleError> {
    if ORACLE_CLIENT_INIT_SUCCESS.get().is_some() {
        return Ok(());
    }

    let attempt_lock = ORACLE_CLIENT_INIT_ATTEMPT_LOCK.get_or_init(|| Mutex::new(()));
    let _attempt_guard = match attempt_lock.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            logging::log_warning(
                "db::connection",
                "oracle init lock was poisoned; recovering",
            );
            poisoned.into_inner()
        }
    };

    if ORACLE_CLIENT_INIT_SUCCESS.get().is_some() {
        return Ok(());
    }

    match init_oracle_client() {
        Ok(_) => {
            ORACLE_CLIENT_INIT_SUCCESS.get_or_init(|| ());
            Ok(())
        }
        Err(err) => Err(OracleError::new(
            OracleErrorKind::InternalError,
            format_oracle_client_init_error(&err),
        )),
    }
}

fn init_oracle_client() -> Result<(), OracleError> {
    let candidate_dirs = oracle_client_lib_dir_candidates();
    let mut last_error: Option<OracleError> = None;

    for dir in candidate_dirs {
        if !dir_has_oracle_client_lib(&dir) {
            continue;
        }

        let mut params = InitParams::new();
        params.load_error_url(ORACLE_CLIENT_LOAD_HELP_URL)?;
        params.oracle_client_lib_dir(&dir)?;

        match params.init() {
            Ok(_) => return Ok(()),
            Err(err) => last_error = Some(err),
        }
    }

    if let Some(err) = last_error {
        return Err(err);
    }

    let mut params = InitParams::new();
    params.load_error_url(ORACLE_CLIENT_LOAD_HELP_URL)?;
    params.init().map(|_| ())
}

fn oracle_client_lib_dir_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(env_dir) = env::var_os(ORACLE_CLIENT_LIB_ENV_VAR) {
        push_oracle_client_dir_candidate(&mut candidates, PathBuf::from(env_dir));
    }

    if let Some(home_dir) = oracle_home_lib_dir() {
        push_oracle_client_dir_candidate(&mut candidates, home_dir);
    }

    for root in oracle_client_search_roots() {
        for dir in collect_instantclient_dirs(&root) {
            push_oracle_client_dir_candidate(&mut candidates, dir);
        }
    }

    candidates
}

/// Library directory for a full Oracle Client / Database install exposed via
/// the `ORACLE_HOME` environment variable. On Windows `oci.dll` lives in
/// `%ORACLE_HOME%\bin`; on Unix `libclntsh` lives in `$ORACLE_HOME/lib`.
fn oracle_home_lib_dir() -> Option<PathBuf> {
    let home = PathBuf::from(env::var_os("ORACLE_HOME")?);

    #[cfg(target_os = "windows")]
    {
        Some(home.join("bin"))
    }

    #[cfg(not(target_os = "windows"))]
    {
        Some(home.join("lib"))
    }
}

fn oracle_client_search_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        vec![PathBuf::from("/opt/oracle")]
    }

    #[cfg(target_os = "linux")]
    {
        vec![
            PathBuf::from("/opt/oracle"),
            PathBuf::from("/usr/local/oracle"),
        ]
    }

    #[cfg(target_os = "windows")]
    {
        let mut roots = vec![PathBuf::from(r"C:\oracle")];
        if let Some(program_files) = env::var_os("ProgramFiles") {
            roots.push(PathBuf::from(program_files).join("Oracle"));
        }
        roots
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Vec::new()
    }
}

fn collect_instantclient_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };

    let mut dirs = Vec::new();
    for entry_result in entries {
        let Ok(entry) = entry_result else {
            continue;
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with("instantclient_") {
            dirs.push(path);
        }
    }

    dirs.sort_unstable_by(|left, right| right.as_os_str().cmp(left.as_os_str()));
    dirs
}

fn push_oracle_client_dir_candidate(candidates: &mut Vec<PathBuf>, dir: PathBuf) {
    if candidates.iter().any(|existing| existing == &dir) {
        return;
    }
    candidates.push(dir);
}

/// Whether `dir` contains an Oracle client shared library for the current
/// platform. Linux ships versioned files (e.g. `libclntsh.so.23.1`) and the
/// unversioned symlink may be absent in zip installs, so match by prefix there.
fn dir_has_oracle_client_lib(dir: &Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        dir.join("oci.dll").is_file()
    }

    #[cfg(target_os = "macos")]
    {
        dir.join("libclntsh.dylib").is_file()
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let Ok(entries) = fs::read_dir(dir) else {
            return false;
        };
        entries.flatten().any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("libclntsh.so"))
        })
    }
}

fn format_oracle_client_init_error(err: &OracleError) -> String {
    let err_text = err.to_string();
    let mut message = format!("Failed to initialize Oracle client library: {err_text}");

    if is_oracle_client_architecture_mismatch(&err_text) {
        message.push_str(
            " Detected an Oracle Client CPU architecture mismatch. Install an Oracle Instant Client that matches this app's architecture. On Apple Silicon, use an arm64 client and set ORACLE_CLIENT_LIB_DIR if you need to override auto-detection.",
        );
    } else if err_text.contains("DPI-1047") {
        message.push_str(
            " Set ORACLE_CLIENT_LIB_DIR to the directory that contains the Oracle Client library (oci.dll on Windows, libclntsh.so on Linux, libclntsh.dylib on macOS) if the client is installed in a non-default location.",
        );
    }

    message
}

fn is_oracle_client_architecture_mismatch(err_text: &str) -> bool {
    err_text.contains("incompatible architecture")
        || (err_text.contains("have 'x86_64'") && err_text.contains("need 'arm64"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DbActivityKind {
    ConnectionLock,
    PoolSession,
    Operation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbActivityProgress {
    Indeterminate,
    Determinate { completed: u64, total: u64 },
}

impl DbActivityProgress {
    pub fn percentage(self) -> Option<u8> {
        match self {
            Self::Indeterminate | Self::Determinate { total: 0, .. } => None,
            Self::Determinate { completed, total } => {
                let percentage = safe_div(
                    u128::from(completed.min(total)).saturating_mul(100),
                    u128::from(total),
                );
                Some(percentage as u8)
            }
        }
    }
}

/// Two-tier cancel contract for anything the activity registry tracks.
///
/// Same shape the query cancel button uses: ask the server to abort the call,
/// then tear the session down if it does not let go within the cancel timeout.
pub trait DbActivityCanceler: Send + Sync {
    fn interrupt(&self) -> Result<(), String>;
    fn force(&self) -> Result<(), String>;
    fn label(&self) -> &'static str;
}

/// Ties an activity to the pool context it runs on.
///
/// Every path that ends a session — disconnect, reconnect, pool resize — bumps
/// the pool context epoch, so a stale lifetime is the registry's reliable
/// signal that the connection behind an activity is gone. That is what makes
/// "nothing stays running after the session ends" enforceable centrally
/// instead of at each of the callers.
#[derive(Clone, Debug)]
pub struct DbActivityLifetime {
    epoch_token: Arc<AtomicU64>,
    epoch: u64,
}

impl DbActivityLifetime {
    pub fn is_current(&self) -> bool {
        self.epoch_token.load(Ordering::Acquire) == self.epoch
    }
}

struct TrackedDbActivity {
    id: u64,
    activity: String,
    started_at: Instant,
    db_type: Option<DatabaseType>,
    connection_id: Option<ConnectionId>,
    kind: DbActivityKind,
    progress: DbActivityProgress,
    /// None means the activity is not bound to a pool context, so the registry
    /// cannot tell on its own when it went stale.
    lifetime: Option<DbActivityLifetime>,
    /// Every session currently open under this activity. A list rather than a
    /// slot because one activity can fan out across several sessions, and a
    /// cancel has to reach all of them.
    cancelers: Vec<(u64, Arc<dyn DbActivityCanceler>)>,
    /// Run when the registry retires this activity.
    ///
    /// Breaking the session stops the work, but only the owner knows how to
    /// *report* it. A query whose session is broken by a disconnect would
    /// otherwise surface the driver's error instead of "Cancelled".
    on_cancel: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Alive for exactly as long as the operation still holds its guard, which
    /// is how the force tier knows the work ignored the graceful break.
    guard: Weak<DbActivityGuardInner>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DbActivitySnapshot {
    pub id: u64,
    pub activity: String,
    pub started_at: Instant,
    pub db_type: Option<DatabaseType>,
    pub connection_id: Option<ConnectionId>,
    pub progress: DbActivityProgress,
    /// Whether this activity carries a canceler, so the UI can offer a cancel
    /// for it rather than leaving the user with a status entry they cannot end.
    pub cancelable: bool,
}

#[derive(Clone)]
pub struct DbActivityGuard {
    inner: Arc<DbActivityGuardInner>,
}

struct DbActivityGuardInner {
    id: u64,
    finished: AtomicBool,
}

impl DbActivityGuardInner {
    fn finish(&self) {
        if !self.finished.swap(true, Ordering::AcqRel) {
            remove_db_activity(self.id);
        }
    }
}

#[derive(Clone)]
pub(crate) struct DbActivityFinishHandle {
    inner: Weak<DbActivityGuardInner>,
}

impl DbActivityFinishHandle {
    pub(crate) fn finish(&self) {
        if let Some(inner) = self.inner.upgrade() {
            inner.finish();
        }
    }

    /// Whether the activity this handle points at is still showing in the
    /// registry. False once the guard was dropped or finished, which makes a
    /// stored handle self-clearing: callers do not have to track completion
    /// separately to know the work is over.
    pub(crate) fn is_active(&self) -> bool {
        self.inner
            .upgrade()
            .is_some_and(|inner| !inner.finished.load(Ordering::Acquire))
    }
}

impl DbActivityGuard {
    pub(crate) fn finish_handle(&self) -> DbActivityFinishHandle {
        DbActivityFinishHandle {
            inner: Arc::downgrade(&self.inner),
        }
    }

    pub fn id(&self) -> u64 {
        self.inner.id
    }

    /// A guard that is not in the registry. Only used as a fallback so handing
    /// out a connection can never panic; nothing is tracked under it.
    fn detached() -> Self {
        Self {
            inner: Arc::new(DbActivityGuardInner {
                id: 0,
                finished: AtomicBool::new(true),
            }),
        }
    }

    /// Whether this activity has been retired — by a cancel, by the stale
    /// sweep, or because it already completed.
    ///
    /// Workers check this to bail out: the registry retires an activity the
    /// moment it is cancelled, so this is the one flag that means "stop", no
    /// matter which path asked for it.
    pub fn is_finished(&self) -> bool {
        self.inner.finished.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn untracked_for_test() -> Self {
        Self {
            inner: Arc::new(DbActivityGuardInner {
                id: 0,
                finished: AtomicBool::new(false),
            }),
        }
    }

    pub fn set_activity(&self, activity: impl Into<String>) {
        let activity = activity.into();
        if let Some(tracked) = lock_db_activities()
            .iter_mut()
            .find(|tracked| tracked.id == self.inner.id)
        {
            tracked.activity = activity;
        }
    }

    pub fn set_progress(&self, progress: DbActivityProgress) {
        if let Some(tracked) = lock_db_activities()
            .iter_mut()
            .find(|tracked| tracked.id == self.inner.id)
        {
            tracked.progress = progress;
        }
    }

    fn set_db_type(&self, db_type: DatabaseType) {
        if let Some(tracked) = lock_db_activities()
            .iter_mut()
            .find(|tracked| tracked.id == self.inner.id)
        {
            tracked.db_type = Some(db_type);
        }
    }

    pub fn set_connection_id(&self, connection_id: ConnectionId) {
        if let Some(tracked) = lock_db_activities()
            .iter_mut()
            .find(|tracked| tracked.id == self.inner.id)
        {
            tracked.connection_id = Some(connection_id);
        }
    }

    /// Bind this activity to the pool context it runs on, so the registry can
    /// retire it by itself once that connection's sessions are gone.
    pub fn bind_lifetime(&self, lifetime: DbActivityLifetime) {
        if let Some(tracked) = lock_db_activities()
            .iter_mut()
            .find(|tracked| tracked.id == self.inner.id)
        {
            tracked.lifetime = Some(lifetime);
        }
    }

    /// Register what to do when the registry retires this activity, so the
    /// owner can report it as a cancel rather than as a failure.
    pub fn on_cancel(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        let replaced = {
            let mut activities = lock_db_activities();
            activities
                .iter_mut()
                .find(|tracked| tracked.id == self.inner.id)
                .and_then(|tracked| tracked.on_cancel.replace(hook))
        };
        // The previous hook is dropped outside the lock; it is caller code.
        drop(replaced);
    }

    /// Publish how to stop a session running under this activity.
    ///
    /// The returned registration detaches on drop, so a cancel can never land
    /// on a session that has already gone back to the pool and been handed to
    /// someone else.
    pub fn attach_canceler(
        &self,
        canceler: Arc<dyn DbActivityCanceler>,
    ) -> DbSessionCancelRegistration {
        let canceler_id = NEXT_DB_CANCELER_ID.fetch_add(1, Ordering::Relaxed);
        if let Some(tracked) = lock_db_activities()
            .iter_mut()
            .find(|tracked| tracked.id == self.inner.id)
        {
            tracked.cancelers.push((canceler_id, canceler));
        }
        DbSessionCancelRegistration {
            activity_id: self.inner.id,
            canceler_id,
        }
    }
}

/// Keeps a session reachable by the cancel button for exactly as long as the
/// caller holds it. Dropping it retires the session's canceler.
pub struct DbSessionCancelRegistration {
    activity_id: u64,
    canceler_id: u64,
}

impl Drop for DbSessionCancelRegistration {
    fn drop(&mut self) {
        // Same reason as `remove_db_activity`: the canceler is dropped after
        // the lock is released, never under it.
        let detached = {
            let mut activities = lock_db_activities();
            activities
                .iter_mut()
                .find(|tracked| tracked.id == self.activity_id)
                .and_then(|tracked| {
                    tracked
                        .cancelers
                        .iter()
                        .position(|(id, _)| *id == self.canceler_id)
                        .map(|index| tracked.cancelers.swap_remove(index))
                })
        };
        drop(detached);
    }
}

impl Drop for DbActivityGuardInner {
    fn drop(&mut self) {
        self.finish();
    }
}

fn db_activity_slot() -> &'static Mutex<Vec<TrackedDbActivity>> {
    ACTIVE_DB_ACTIVITY.get_or_init(|| Mutex::new(Vec::new()))
}

/// Guard plus its lock-order scope, so the tracker sees exactly the window the
/// lock is held for.
pub(crate) struct TrackedGuard<'a, T> {
    guard: MutexGuard<'a, T>,
    _order: crate::db::lock_order::LockOrderScope,
}

impl<T> std::ops::Deref for TrackedGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl<T> std::ops::DerefMut for TrackedGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

fn lock_db_activities() -> TrackedGuard<'static, Vec<TrackedDbActivity>> {
    let _order = crate::db::lock_order::LockOrderScope::enter(
        crate::db::lock_order::names::ACTIVITY_REGISTRY,
    );
    let guard = lock_db_activities_raw();
    TrackedGuard { guard, _order }
}

fn lock_db_activities_raw() -> MutexGuard<'static, Vec<TrackedDbActivity>> {
    match db_activity_slot().lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            logging::log_warning(
                "db::connection",
                "DB activity lock was poisoned; recovering",
            );
            poisoned.into_inner()
        }
    }
}

fn track_db_activity_entry(
    activity: String,
    db_type: Option<DatabaseType>,
    connection_id: Option<ConnectionId>,
    kind: DbActivityKind,
) -> DbActivityGuard {
    let id = NEXT_DB_ACTIVITY_ID.fetch_add(1, Ordering::Relaxed);
    let inner = Arc::new(DbActivityGuardInner {
        id,
        finished: AtomicBool::new(false),
    });
    let mut guard = lock_db_activities();
    guard.push(TrackedDbActivity {
        id,
        activity,
        started_at: Instant::now(),
        db_type,
        connection_id,
        kind,
        progress: DbActivityProgress::Indeterminate,
        lifetime: None,
        cancelers: Vec::new(),
        on_cancel: None,
        guard: Arc::downgrade(&inner),
    });
    DbActivityGuard { inner }
}

fn remove_db_activity(id: u64) {
    // Move the entry out before dropping it. It owns caller-supplied values —
    // the cancel hook's closure and the session cancelers — and running any of
    // their destructors while the registry lock is held would deadlock the
    // moment one of them touched the registry back.
    let removed = {
        let mut guard = lock_db_activities();
        guard
            .iter()
            .position(|activity| activity.id == id)
            .map(|index| guard.swap_remove(index))
    };
    drop(removed);
}

pub fn track_pool_db_activity(
    activity: impl Into<String>,
    db_type: DatabaseType,
) -> DbActivityGuard {
    track_db_activity_entry(
        activity.into(),
        Some(db_type),
        None,
        DbActivityKind::PoolSession,
    )
}

pub fn track_pool_db_activity_for_connection(
    activity: impl Into<String>,
    db_type: DatabaseType,
    connection_id: ConnectionId,
) -> DbActivityGuard {
    track_db_activity_entry(
        activity.into(),
        Some(db_type),
        Some(connection_id),
        DbActivityKind::PoolSession,
    )
}

pub fn track_db_activity(
    activity: impl Into<String>,
    db_type: Option<DatabaseType>,
) -> DbActivityGuard {
    track_db_activity_entry(activity.into(), db_type, None, DbActivityKind::Operation)
}

pub fn track_db_activity_for_connection(
    activity: impl Into<String>,
    db_type: Option<DatabaseType>,
    connection_id: ConnectionId,
) -> DbActivityGuard {
    track_db_activity_entry(
        activity.into(),
        db_type,
        Some(connection_id),
        DbActivityKind::Operation,
    )
}

fn current_db_activity_for_kind(kind: Option<DbActivityKind>) -> Option<String> {
    let guard = lock_db_activities();
    let activities = guard
        .iter()
        .filter(|activity| kind.is_none_or(|kind| activity.kind == kind))
        .map(|activity| activity.activity.as_str())
        .collect::<Vec<_>>();
    if activities.is_empty() {
        return None;
    }
    Some(activities.join("; "))
}

pub fn current_db_activity() -> Option<String> {
    current_db_activity_for_kind(None)
}

fn current_connection_lock_activity() -> Option<String> {
    current_db_activity_for_kind(Some(DbActivityKind::ConnectionLock))
}

impl TrackedDbActivity {
    fn snapshot(&self) -> DbActivitySnapshot {
        DbActivitySnapshot {
            id: self.id,
            activity: self.activity.clone(),
            started_at: self.started_at,
            db_type: self.db_type,
            connection_id: self.connection_id,
            progress: self.progress,
            cancelable: !self.cancelers.is_empty(),
        }
    }

    /// Stale means the pool context this activity runs on is gone, so whatever
    /// it is blocked in cannot produce a usable result any more.
    fn is_stale(&self) -> bool {
        self.lifetime
            .as_ref()
            .is_some_and(|lifetime| !lifetime.is_current())
    }
}

pub fn active_pool_db_activity_snapshots() -> Vec<DbActivitySnapshot> {
    let guard = lock_db_activities();
    guard
        .iter()
        .filter(|activity| activity.kind == DbActivityKind::PoolSession)
        .map(TrackedDbActivity::snapshot)
        .collect()
}

pub fn active_db_activity_snapshots() -> Vec<DbActivitySnapshot> {
    lock_db_activities()
        .iter()
        .map(TrackedDbActivity::snapshot)
        .collect()
}

/// Wait out the graceful tier of a two-tier cancel.
///
/// Polls `still_pending` until `timeout` elapses. Returns true when the
/// deadline passes with the cancel still pending — the caller's cue to escalate
/// to a force close — and false as soon as the graceful break lands.
///
/// Shared so every cancel in the app waits the same way on the same configured
/// cancel timeout instead of each carrying its own loop.
pub fn wait_for_graceful_cancel(timeout: Duration, still_pending: impl Fn() -> bool) -> bool {
    // `Instant + Duration` panics on overflow, and this is a public entry point.
    let Some(force_deadline) = Instant::now().checked_add(timeout) else {
        // A deadline that far out means "never force"; wait for the break.
        while still_pending() {
            std::thread::sleep(CANCEL_WATCHDOG_POLL_INTERVAL);
        }
        return false;
    };
    loop {
        let remaining = force_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return true;
        }
        if !still_pending() {
            return false;
        }
        std::thread::sleep(remaining.min(CANCEL_WATCHDOG_POLL_INTERVAL));
    }
}

/// How often a cancel watchdog rechecks whether the graceful break landed.
const CANCEL_WATCHDOG_POLL_INTERVAL: Duration = Duration::from_millis(100);

fn panic_payload_to_string(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// Runs a driver call or an owner callback that must never take the caller
/// down: cancels run on the UI thread (the status tick sweeps there) and on the
/// shared watchdog thread, so one misbehaving backend must not stop the rest.
fn run_guarded(what: &str, activity: &str, call: impl FnOnce() -> Result<(), String>) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(call))
        .unwrap_or_else(|payload| Err(panic_payload_to_string(payload.as_ref())));
    if let Err(err) = outcome {
        logging::log_warning(
            "db::connection",
            &format!("{what} failed for '{activity}': {err}"),
        );
    }
}

/// One cancel that has been dispatched and is waiting out its graceful tier.
struct DispatchedCancel {
    canceler: Arc<dyn DbActivityCanceler>,
    guard: Weak<DbActivityGuardInner>,
    activity: String,
}

/// Break the sessions, then escalate to the force tier for whatever is still
/// running at `force_timeout`. Runs off the caller's thread because both tiers
/// make network calls.
fn spawn_force_cancel_watchdog(dispatched: Vec<DispatchedCancel>, force_timeout: Duration) {
    if dispatched.is_empty() {
        return;
    }
    // Shared so the work can be taken back if the thread never starts: the
    // closure owns it on success, and on failure it is still here to run
    // inline rather than silently leaving the sessions running.
    let pending = Arc::new(Mutex::new(Some(dispatched)));
    let pending_in_thread = Arc::clone(&pending);
    let spawned = std::thread::Builder::new()
        .name("db-activity-cancel-watchdog".to_string())
        .spawn(move || {
            let Some(dispatched) = pending_in_thread
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
            else {
                return;
            };
            for dispatched in &dispatched {
                let label = dispatched.canceler.label();
                run_guarded(&format!("{label} cancel"), &dispatched.activity, || {
                    dispatched.canceler.interrupt()
                });
            }
            // ONE deadline for the whole batch. These sessions were all
            // interrupted at the same moment, so they have all had the same
            // grace period; restarting the timeout per session would make the
            // last of N sessions wait N * timeout to be force closed.
            let deadline = Instant::now().checked_add(force_timeout);
            for dispatched in dispatched {
                let remaining = deadline.map_or(force_timeout, |deadline| {
                    deadline.saturating_duration_since(Instant::now())
                });
                // The guard lives exactly as long as the operation holds it, so
                // an upgrade that still succeeds means the break was ignored.
                let escalate =
                    wait_for_graceful_cancel(remaining, || dispatched.guard.upgrade().is_some());
                if !escalate {
                    continue;
                }
                let label = dispatched.canceler.label();
                run_guarded(
                    &format!("{label} force cancel"),
                    &dispatched.activity,
                    || dispatched.canceler.force(),
                );
            }
        });
    if let Err(err) = spawned {
        logging::log_error(
            "db::connection",
            &format!("failed to start DB activity cancel watchdog: {err}"),
        );
        // Last resort: without the watchdog neither tier would run at all, so
        // break the sessions on this thread rather than leave them running.
        let dispatched = pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .unwrap_or_default();
        for dispatched in dispatched {
            let label = dispatched.canceler.label();
            run_guarded(&format!("{label} cancel"), &dispatched.activity, || {
                dispatched.canceler.interrupt()
            });
        }
    }
}

/// Cancel every tracked activity matching `select`, removing their entries.
/// Returns how many were retired.
fn cancel_db_activities_where(
    force_timeout: Duration,
    select: impl Fn(&TrackedDbActivity) -> bool,
) -> usize {
    // Nothing that can block or re-enter runs while the registry lock is held:
    // `interrupt` makes a network call (MySQL cancels over a second
    // connection), and a cancel hook calls back into the owner, which may touch
    // the registry itself. Both happen after the lock is released.
    let mut selected = Vec::new();
    let mut retired = 0usize;
    {
        let mut activities = lock_db_activities();
        // `retain_mut` so the hook and the cancelers are MOVED out rather than
        // cloned: dropping them is caller-controlled code, and running any of it
        // while the registry lock is held would deadlock the moment it touched
        // the registry back.
        activities.retain_mut(|tracked| {
            if !select(tracked) {
                return true;
            }
            retired += 1;
            selected.push((
                tracked.on_cancel.take(),
                std::mem::take(&mut tracked.cancelers),
                tracked.guard.clone(),
                std::mem::take(&mut tracked.activity),
            ));
            // Mark the guard finished so its later drop is a no-op and nothing
            // re-adds the entry.
            if let Some(inner) = tracked.guard.upgrade() {
                inner.finished.store(true, Ordering::Release);
            }
            false
        });
    }

    let mut dispatched = Vec::new();
    for (hook, cancelers, guard, activity) in selected {
        // Tell the owner first: it must see the cancel before the work it owns
        // comes back with a broken-session error.
        if let Some(hook) = hook {
            run_guarded("cancel notification", &activity, || {
                hook();
                Ok(())
            });
        }
        for (_, canceler) in cancelers {
            // The break itself is NOT run here. The stale sweep calls this from
            // the UI thread, and a MySQL-family cancel opens a second connection
            // to issue KILL QUERY — against an unreachable server that blocks
            // for the connect timeout, which would freeze the UI. The registry
            // entry is already gone, so the screen is correct immediately; the
            // break and its escalation both happen on the watchdog thread.
            dispatched.push(DispatchedCancel {
                canceler,
                guard: guard.clone(),
                activity: activity.clone(),
            });
        }
    }
    spawn_force_cancel_watchdog(dispatched, force_timeout);
    retired
}

/// Cancel one activity by id. Used by the cancel button for work that has no
/// query tab behind it.
pub fn cancel_db_activity(id: u64, force_timeout: Duration) -> bool {
    cancel_db_activities_where(force_timeout, |tracked| tracked.id == id) > 0
}

/// Retire every activity whose connection is gone.
///
/// This is the guarantee that a finished session leaves nothing behind: it runs
/// on the status bar tick, so a disconnect clears within one UI frame no matter
/// which code path started the work.
pub fn sweep_stale_db_activities(force_timeout: Duration) -> usize {
    cancel_db_activities_where(force_timeout, TrackedDbActivity::is_stale)
}

/// Retire every activity belonging to a connection that is being closed.
pub fn cancel_db_activities_for_connection(
    connection_id: ConnectionId,
    force_timeout: Duration,
) -> usize {
    cancel_db_activities_where(force_timeout, |tracked| {
        tracked.connection_id == Some(connection_id)
    })
}

pub fn format_connection_busy_message() -> String {
    match current_connection_lock_activity() {
        Some(activity) => format!("Connection is busy. Current DB activity: {}", activity),
        None => "Connection is busy. Try again after the current operation finishes.".to_string(),
    }
}

pub fn clear_tracked_db_activity() {
    // Moved out, then dropped: the entries own caller-supplied values (the
    // cancel hook's closure, the session cancelers) and running their
    // destructors under the registry lock would break the leaf-lock invariant.
    let cleared = std::mem::take(&mut *lock_db_activities());
    drop(cleared);
}

pub struct ConnectionLockGuard<'a> {
    guard: DatabaseConnectionGuard<'a>,
    activity_guard: Option<DbActivityGuard>,
    /// Detaches when the lock is released, so a cancel cannot land on the
    /// connection after this operation stopped using it.
    cancel_registration: Option<DbSessionCancelRegistration>,
}

impl<'a> ConnectionLockGuard<'a> {
    pub fn refresh_tracked_connection(&self) {}

    /// Publish the live connection to the registry before it is handed out.
    ///
    /// These four shadow the `DatabaseConnection` accessors reached through
    /// `Deref`, and inherent methods win over `Deref`, so every guard-based
    /// caller goes through them whether it knows about them or not. That is
    /// what makes "a connection handle is never handed out untracked" hold
    /// without auditing call sites.
    pub fn require_live_connection(&mut self) -> Result<Arc<Connection>, String> {
        let _ = self.activity();
        self.guard.require_live_connection()
    }

    pub fn require_live_db_connection(&mut self) -> Result<DbConnection, String> {
        let _ = self.activity();
        self.guard.require_live_db_connection()
    }

    pub fn get_connection(&mut self) -> Option<Arc<Connection>> {
        let _ = self.activity();
        self.guard.get_connection()
    }

    pub fn get_db_connection(&mut self) -> Option<DbConnection> {
        let _ = self.activity();
        self.guard.get_db_connection()
    }

    /// The activity this lock is tracked under, creating one if the lock was
    /// taken without a label.
    ///
    /// Acquiring a session needs an activity to hang it on, and returning a
    /// clone rather than an `Option` is what keeps that requirement
    /// unconditional: there is no lock state from which a caller can acquire a
    /// session that nothing is tracking.
    pub fn activity(&mut self) -> DbActivityGuard {
        if self.activity_guard.is_none() {
            let db_type = self.guard.db_type();
            let activity_guard = track_db_activity_entry(
                current_db_activity().unwrap_or_else(|| "Database operation".to_string()),
                Some(db_type),
                None,
                DbActivityKind::ConnectionLock,
            );
            activity_guard.bind_lifetime(self.guard.activity_lifetime());
            if let Some(connection_id) = self.guard.connection_id() {
                activity_guard.set_connection_id(connection_id);
            }
            self.cancel_registration = main_connection_canceler(&self.guard)
                .map(|canceler| activity_guard.attach_canceler(canceler));
            self.activity_guard = Some(activity_guard);
        }
        self.activity_guard
            .as_ref()
            .map_or_else(DbActivityGuard::detached, DbActivityGuard::clone)
    }
}

impl<'a> Deref for ConnectionLockGuard<'a> {
    type Target = DatabaseConnection;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl<'a> DerefMut for ConnectionLockGuard<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

pub fn create_shared_connection() -> SharedConnection {
    Arc::new(Mutex::new(DatabaseConnection::new()))
}

pub(crate) fn connect_shared_connection_with_policy(
    connection: &SharedConnection,
    info: ConnectionInfo,
    pool_size: u32,
    policy: ConnectionAttemptPolicy,
) -> Result<(), String> {
    info.advanced
        .validate_for_db(info.db_type, info.uses_oracle_tns_alias())?;
    let activity = format!("Connecting to {}", info.name);
    let transition = begin_connection_transition(connection, activity.clone())?;
    let _activity_guard = track_db_activity(activity, Some(info.db_type));
    let auto_commit = {
        let connection_guard = lock_database_connection_raw(connection);
        connection_guard.auto_commit()
    };

    let prepared = DatabaseConnection::prepare_connection(info, pool_size, auto_commit, policy)?;
    if !transition.is_current() {
        DatabaseConnection::retire_connection_in_background(prepared);
        return Err("Connection attempt is no longer current".to_string());
    }

    let retired = {
        let mut connection_guard = lock_database_connection_raw(connection);
        if connection_guard.connection_generation() != transition.expected_generation {
            drop(connection_guard);
            DatabaseConnection::retire_connection_in_background(prepared);
            return Err(
                "Connection changed before the new connection could be installed".to_string(),
            );
        }
        let retired = connection_guard.install_prepared_connection(prepared)?;
        refresh_pool_session_context_cache_for_shared_connection(connection, &connection_guard);
        retired
    };
    transition.finish();
    DatabaseConnection::retire_connection_in_background(retired);
    Ok(())
}

pub(crate) fn resize_shared_connection_pool_with_policy(
    connection: &SharedConnection,
    size: u32,
    policy: ConnectionAttemptPolicy,
) -> Result<(), String> {
    let size = DatabaseConnection::clamp_connection_pool_size(size);
    let transition = begin_connection_transition(connection, "Rebuilding connection pool")?;
    let (info, current_size, connected) = {
        let connection_guard = lock_database_connection_raw(connection);
        (
            connection_guard.runtime_connection_info(),
            connection_guard.connection_pool_size(),
            connection_guard.is_connected() && connection_guard.has_connection_handle(),
        )
    };

    if current_size == size {
        let connection_guard = lock_database_connection_raw(connection);
        refresh_pool_session_context_cache_for_shared_connection(connection, &connection_guard);
        drop(connection_guard);
        transition.finish();
        return Ok(());
    }
    if !connected {
        let mut connection_guard = lock_database_connection_raw(connection);
        connection_guard.set_connection_pool_size(size);
        refresh_pool_session_context_cache_for_shared_connection(connection, &connection_guard);
        drop(connection_guard);
        transition.finish();
        return Ok(());
    }

    let info = info.ok_or_else(|| "Connected session credentials are unavailable".to_string())?;
    let description = info.connection_attempt_description("Rebuilding");
    let pool = run_connection_attempt(policy, description, move || {
        DatabaseConnection::build_pool_for_info(&info, size, policy)
    })?;
    if !transition.is_current() {
        DatabaseConnection::retire_connection_resources_in_background(None, Some(pool));
        return Err("Connection pool resize attempt is no longer current".to_string());
    }

    let retired_pool = {
        let mut connection_guard = lock_database_connection_raw(connection);
        if connection_guard.connection_generation() != transition.expected_generation {
            drop(connection_guard);
            DatabaseConnection::retire_connection_resources_in_background(None, Some(pool));
            return Err(
                "Connection changed before the new connection pool could be installed".to_string(),
            );
        }
        let retired_pool = connection_guard.pool.replace(pool);
        connection_guard.connection_pool_size = size;
        connection_guard.bump_connection_generation();
        connection_guard.bump_pool_context_epoch();
        refresh_pool_session_context_cache_for_shared_connection(connection, &connection_guard);
        retired_pool
    };
    transition.finish();
    if let Some(retired_pool) = retired_pool {
        DatabaseConnection::retire_connection_resources_in_background(None, Some(retired_pool));
    }
    Ok(())
}

/// Record which registered connection this is, so work started on it is tagged
/// with the connection it belongs to.
pub(crate) fn stamp_connection_id(connection: &SharedConnection, connection_id: ConnectionId) {
    lock_database_connection_raw(connection).connection_id = Some(connection_id);
}

pub fn lock_connection(connection: &SharedConnection) -> ConnectionLockGuard<'_> {
    loop {
        wait_for_connection_transition(connection);
        let guard = lock_database_connection_raw(connection);
        if active_connection_transition(connection).is_some() {
            drop(guard);
            continue;
        }
        return ConnectionLockGuard {
            guard,
            activity_guard: None,
            cancel_registration: None,
        };
    }
}

pub fn lock_connection_with_activity(
    connection: &SharedConnection,
    activity: impl Into<String>,
) -> ConnectionLockGuard<'_> {
    let activity = activity.into();
    let activity_guard =
        track_db_activity_entry(activity, None, None, DbActivityKind::ConnectionLock);
    let mut connection_guard = lock_connection(connection);
    activity_guard.set_db_type(connection_guard.db_type());
    // Bound to the connection generation so a disconnect retires this entry
    // even if the call it is tracking never returns.
    activity_guard.bind_lifetime(connection_guard.activity_lifetime());
    if let Some(connection_id) = connection_guard.connection_id() {
        activity_guard.set_connection_id(connection_id);
    }
    connection_guard.cancel_registration = main_connection_canceler(&connection_guard)
        .map(|canceler| activity_guard.attach_canceler(canceler));
    connection_guard.activity_guard = Some(activity_guard);
    connection_guard
}

/// Try to acquire the connection lock without blocking.
/// Returns None if the lock is already held (query is running).
pub fn try_lock_connection(connection: &SharedConnection) -> Option<ConnectionLockGuard<'_>> {
    if active_connection_transition(connection).is_some() {
        return None;
    }
    let order =
        crate::db::lock_order::LockOrderScope::enter(crate::db::lock_order::names::DB_CONNECTION);
    let guard = match connection.try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::WouldBlock) => return None,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            logging::log_warning(
                "db::connection",
                "database connection lock was poisoned; recovering",
            );
            poisoned.into_inner()
        }
    };
    let guard = DatabaseConnectionGuard {
        guard,
        _order: order,
    };
    if active_connection_transition(connection).is_some() {
        drop(guard);
        return None;
    }
    Some(ConnectionLockGuard {
        guard,
        activity_guard: None,
        cancel_registration: None,
    })
}

pub fn try_lock_connection_with_activity(
    connection: &SharedConnection,
    activity: impl Into<String>,
) -> Option<ConnectionLockGuard<'_>> {
    let mut guard = try_lock_connection(connection)?;
    let db_type = guard.db_type();
    let activity_guard = track_db_activity_entry(
        activity.into(),
        Some(db_type),
        None,
        DbActivityKind::ConnectionLock,
    );
    activity_guard.bind_lifetime(guard.activity_lifetime());
    if let Some(connection_id) = guard.connection_id() {
        activity_guard.set_connection_id(connection_id);
    }
    guard.cancel_registration =
        main_connection_canceler(&guard).map(|canceler| activity_guard.attach_canceler(canceler));
    guard.activity_guard = Some(activity_guard);
    Some(guard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_null_sort_order_matches_each_backend() {
        // Oracle puts NULLs last on an ascending ORDER BY; the MySQL family
        // puts them first. The result grid's local sort mirrors this.
        assert!(DatabaseType::Oracle.sorts_nulls_last_ascending());
        assert!(!DatabaseType::MySQL.sorts_nulls_last_ascending());
        assert!(!DatabaseType::MariaDB.sorts_nulls_last_ascending());
    }

    #[test]
    fn every_supported_backend_states_its_null_sort_order() {
        // Exercises the accessor for every variant so a new backend cannot be
        // added without deciding this.
        for db_type in DatabaseType::ALL {
            let _ = db_type.sorts_nulls_last_ascending();
        }
    }

    #[test]
    fn common_connection_deadline_returns_before_late_worker_result() {
        let completed = Arc::new(AtomicBool::new(false));
        let completed_for_worker = Arc::clone(&completed);
        let policy = ConnectionAttemptPolicy {
            timeout: Duration::from_millis(50),
        };
        let started = Instant::now();

        let result = run_connection_attempt(policy, "test connection".to_string(), move || {
            std::thread::sleep(Duration::from_millis(300));
            completed_for_worker.store(true, Ordering::Release);
            Ok(())
        });

        assert!(result
            .expect_err("attempt should time out")
            .contains("timed out"));
        assert!(started.elapsed() < Duration::from_millis(250));
        std::thread::sleep(Duration::from_millis(300));
        assert!(completed.load(Ordering::Acquire));
    }

    #[test]
    fn connection_attempt_worker_panic_is_returned_as_an_error() {
        let result = run_connection_attempt(
            ConnectionAttemptPolicy {
                timeout: Duration::from_millis(250),
            },
            "panic test connection".to_string(),
            || -> Result<(), String> { panic!("simulated connection worker panic") },
        );

        assert!(result
            .expect_err("worker panic should become an ordinary error")
            .contains("worker terminated unexpectedly"));
    }

    #[test]
    fn cleanup_task_is_recovered_when_worker_start_fails() {
        let executed = Arc::new(AtomicBool::new(false));
        let executed_for_task = Arc::clone(&executed);
        let task: ConnectionCleanupTask = Box::new(move || {
            executed_for_task.store(true, Ordering::Release);
        });

        let result = try_start_connection_cleanup_with(task, |_task| {
            Err::<(), _>("simulated worker start failure")
        });
        let (err, pending_task) = match result {
            Ok(()) => panic!("simulated worker start should fail"),
            Err(failure) => failure,
        };

        assert_eq!(err, "simulated worker start failure");
        assert!(!executed.load(Ordering::Acquire));
        let pending_task = pending_task.expect("failed start must return cleanup ownership");
        pending_task();
        assert!(executed.load(Ordering::Acquire));
    }

    #[test]
    fn cleanup_task_panic_is_contained() {
        let task: ConnectionCleanupTask = Box::new(|| panic!("simulated cleanup panic"));
        let task = Arc::new(Mutex::new(Some(task)));

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_connection_cleanup_task(task);
        }));

        assert!(result.is_ok());
    }

    #[test]
    fn connection_transition_is_released_during_panic_unwind() {
        let connection = create_shared_connection();
        let unwind_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _transition = begin_connection_transition(&connection, "PANIC_TRANSITION")
                .expect("transition should start");
            panic!("simulated transition panic");
        }));

        assert!(unwind_result.is_err());
        assert!(connection_transition_activity(&connection).is_none());
        assert!(try_lock_connection(&connection).is_some());
    }

    #[test]
    fn panicking_transition_drop_does_not_wait_for_database_mutex() {
        let connection = create_shared_connection();
        let transition = begin_connection_transition(&connection, "PANIC_WITH_BUSY_MUTEX")
            .expect("transition should start");
        let connection_for_holder = Arc::clone(&connection);
        let (locked_sender, locked_receiver) = std::sync::mpsc::channel();
        let (release_sender, release_receiver) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _guard = connection_for_holder
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            locked_sender.send(()).expect("report held database mutex");
            let _ = release_receiver.recv_timeout(Duration::from_secs(2));
        });
        locked_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("holder should acquire database mutex");

        let started = Instant::now();
        let unwind_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _transition = transition;
            panic!("simulated panic while raw database mutex is held");
        }));
        let elapsed = started.elapsed();

        let _ = release_sender.send(());
        holder.join().expect("database mutex holder");
        assert!(unwind_result.is_err());
        assert!(
            elapsed < Duration::from_millis(250),
            "panic cleanup waited for the database mutex: {elapsed:?}"
        );
        assert!(connection_transition_activity(&connection).is_none());
    }

    #[test]
    fn connection_lock_releases_database_mutex_before_activity_mutex() {
        let _activity_test_guard = db_activity_test_lock();
        clear_tracked_db_activity();
        let connection = create_shared_connection();
        let connection_for_worker = Arc::clone(&connection);
        let (ready_sender, ready_receiver) = std::sync::mpsc::channel();
        let (drop_sender, drop_receiver) = std::sync::mpsc::channel();
        let (done_sender, done_receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let guard =
                lock_connection_with_activity(&connection_for_worker, "LOCK_DROP_ORDER_TEST");
            ready_sender.send(()).expect("report acquired lock");
            drop_receiver.recv().expect("wait for drop signal");
            drop(guard);
            done_sender.send(()).expect("report dropped lock");
        });
        ready_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("worker should acquire connection lock");

        let activity_lock = lock_db_activities();
        drop_sender.send(()).expect("request guard drop");
        let deadline = Instant::now() + Duration::from_millis(500);
        let database_lock_released = loop {
            match connection.try_lock() {
                Ok(guard) => {
                    drop(guard);
                    break true;
                }
                Err(std::sync::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::yield_now();
                }
                Err(std::sync::TryLockError::WouldBlock) => break false,
                Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                    drop(poisoned.into_inner());
                    break true;
                }
            }
        };
        drop(activity_lock);

        done_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("guard drop should finish after activity lock is released");
        worker.join().expect("lock drop worker");
        assert!(database_lock_released);
        clear_tracked_db_activity();
    }

    #[test]
    fn connection_transition_rejects_try_lock_and_releases_blocking_waiters() {
        let connection = create_shared_connection();
        let transition = begin_connection_transition(&connection, "TEST_CONNECT_TRANSITION")
            .expect("transition should start");
        assert!(try_lock_connection(&connection).is_none());

        let acquired = Arc::new(AtomicBool::new(false));
        let acquired_for_worker = Arc::clone(&acquired);
        let connection_for_worker = Arc::clone(&connection);
        let worker = std::thread::spawn(move || {
            let _guard = lock_connection(&connection_for_worker);
            acquired_for_worker.store(true, Ordering::Release);
        });
        std::thread::sleep(Duration::from_millis(25));
        assert!(!acquired.load(Ordering::Acquire));

        transition.finish();
        worker.join().expect("waiting worker should finish");
        assert!(acquired.load(Ordering::Acquire));
    }

    #[test]
    fn incomplete_prepared_connection_returns_error_without_replacing_current_state() {
        let mut connection = DatabaseConnection::new();
        connection.simulate_connected_metadata_for_test(ConnectionInfo::new_with_type(
            "preserved",
            "system",
            "old-password",
            "old-host",
            1521,
            "OLD",
            DatabaseType::Oracle,
        ));
        let generation = connection.connection_generation();

        let result = connection.install_prepared_connection(DatabaseConnection::new());

        let err = match result {
            Ok(_) => panic!("incomplete prepared connection should be rejected"),
            Err(err) => err,
        };
        assert!(err.contains("incomplete"));
        assert_eq!(connection.get_info().name, "preserved");
        assert_eq!(connection.connection_generation(), generation);
        assert!(connection.is_connected());
    }

    #[test]
    fn disconnect_does_not_wait_for_a_held_session_state_mutex() {
        let connection = create_shared_connection();
        let session = {
            let mut guard = lock_connection(&connection);
            guard.simulate_connected_metadata_for_test(ConnectionInfo::new_with_type(
                "connected",
                "root",
                "password",
                "localhost",
                3306,
                "test",
                DatabaseType::MySQL,
            ));
            guard.session_state()
        };
        let mut held_session = session.lock().expect("session state lock");
        held_session.set_connection_db_type(DatabaseType::MySQL);

        let connection_for_worker = Arc::clone(&connection);
        let (done_sender, done_receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut guard = lock_connection(&connection_for_worker);
            guard.disconnect();
            done_sender.send(()).expect("report disconnect completion");
        });

        done_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("disconnect should not wait for the held session mutex");
        drop(held_session);
        worker.join().expect("disconnect worker");

        let reset_deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Ok(guard) = session.try_lock() {
                if guard.db_type == DatabaseType::default() {
                    break;
                }
            }
            assert!(
                Instant::now() < reset_deadline,
                "deferred session reset should complete"
            );
            std::thread::yield_now();
        }

        let reset_session = session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(reset_session.define_enabled);
    }

    #[test]
    fn stale_deferred_session_update_cannot_overwrite_a_newer_epoch() {
        struct CompletionOnDrop(Option<std::sync::mpsc::Sender<()>>);

        impl Drop for CompletionOnDrop {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }

        let session = Arc::new(Mutex::new(SessionState::default()));
        let epoch_token = Arc::new(AtomicU64::new(7));
        let mut held_session = session.lock().expect("hold session state lock");
        let (done_sender, done_receiver) = std::sync::mpsc::channel();
        let completion = CompletionOnDrop(Some(done_sender));

        update_session_state_without_blocking(&session, &epoch_token, 7, move |session| {
            let _completion = completion;
            session.reset_for_connection(DatabaseType::default());
        });
        epoch_token.store(8, Ordering::Release);
        held_session.set_connection_db_type(DatabaseType::MySQL);
        drop(held_session);

        done_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("deferred update should finish or be discarded");
        let session = session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(session.db_type, DatabaseType::MySQL);
        assert!(!session.define_enabled);
    }

    #[test]
    fn failed_shared_connect_preserves_existing_connection_metadata() {
        let _activity_test_guard = db_activity_test_lock();
        clear_tracked_db_activity();
        let listener =
            std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind stalled MariaDB server");
        let port = listener.local_addr().expect("listener address").port();
        let (accepted_sender, accepted_receiver) = std::sync::mpsc::channel();
        let (release_sender, release_receiver) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept MariaDB test client");
            accepted_sender.send(()).expect("report accepted client");
            let _ = release_receiver.recv_timeout(Duration::from_secs(4));
            drop(stream);
        });

        let connection = create_shared_connection();
        {
            let mut guard = lock_connection(&connection);
            guard.simulate_connected_metadata_for_test(ConnectionInfo::new_with_type(
                "preserved",
                "system",
                "old-password",
                "old-host",
                1521,
                "OLD",
                DatabaseType::Oracle,
            ));
        }
        let replacement = ConnectionInfo::new_with_type(
            "replacement",
            "root",
            "bad-password",
            "127.0.0.1",
            port,
            "test-service",
            DatabaseType::MariaDB,
        );

        let connection_for_attempt = Arc::clone(&connection);
        let attempt = std::thread::spawn(move || {
            connect_shared_connection_with_policy(
                &connection_for_attempt,
                replacement,
                MIN_CONNECTION_POOL_SIZE,
                ConnectionAttemptPolicy::from_seconds(1),
            )
        });
        accepted_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("MariaDB client should reach the stalled server");

        let ui_probe_started = Instant::now();
        assert!(try_lock_connection(&connection).is_none());
        assert!(ui_probe_started.elapsed() < Duration::from_millis(250));

        let result = attempt.join().expect("connection attempt worker");
        let _ = release_sender.send(());
        server.join().expect("stalled MariaDB server");
        assert!(result
            .expect_err("stalled MariaDB connection should time out")
            .contains("timed out"));

        let guard = lock_connection(&connection);
        assert_eq!(guard.get_info().name, "preserved");
        assert_eq!(guard.db_type(), DatabaseType::Oracle);
        assert!(guard.is_connected());
    }

    fn assert_stalled_server_obeys_connection_deadline(db_type: DatabaseType) {
        let listener =
            std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind stalled test server");
        listener
            .set_nonblocking(true)
            .expect("make stalled test server nonblocking");
        let port = listener.local_addr().expect("listener address").port();
        let (release_sender, release_receiver) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let accept_deadline = Instant::now() + Duration::from_secs(4);
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = release_receiver.recv_timeout(Duration::from_secs(4));
                        drop(stream);
                        return true;
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        match release_receiver.try_recv() {
                            Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                return false;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                        }
                        if Instant::now() >= accept_deadline {
                            return false;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return false,
                }
            }
        });

        let mut info = ConnectionInfo::new_with_type(
            "stalled",
            "test-user",
            "test-password",
            "127.0.0.1",
            port,
            "test-service",
            db_type,
        );
        if db_type == DatabaseType::Oracle {
            info.advanced.oracle_driver_mode = OracleDriverMode::Thin;
        }

        let started = Instant::now();
        let result = DatabaseConnection::test_connection_with_policy(
            &info,
            ConnectionAttemptPolicy::from_seconds(1),
        );
        let elapsed = started.elapsed();
        drop(release_sender);
        let accepted = server.join().expect("stalled test server");

        assert!(accepted, "{db_type} client should reach the stalled server");
        assert!(result.is_err(), "{db_type} stalled connection should fail");
        assert!(
            elapsed >= Duration::from_millis(700),
            "{db_type} failed before exercising the deadline: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(4),
            "{db_type} exceeded the common connection deadline: {elapsed:?}"
        );
    }

    #[test]
    fn mysql_family_stalled_handshakes_obey_common_connection_deadline() {
        assert_stalled_server_obeys_connection_deadline(DatabaseType::MySQL);
        assert_stalled_server_obeys_connection_deadline(DatabaseType::MariaDB);
    }

    #[test]
    fn oracle_thin_stalled_handshake_obeys_common_connection_deadline() {
        assert_stalled_server_obeys_connection_deadline(DatabaseType::Oracle);
    }

    #[test]
    fn blocking_connection_lock_registers_activity_before_waiting() {
        let _activity_test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        let connection = create_shared_connection();
        let held_lock = connection.lock().expect("connection lock");
        let connection_for_worker = connection.clone();
        let worker = std::thread::spawn(move || {
            let _guard = lock_connection_with_activity(
                &connection_for_worker,
                "WAITING_LOCK_ACTIVITY_REGRESSION",
            );
        });

        let mut registered_while_waiting = false;
        for _ in 0..100 {
            if active_db_activity_snapshots()
                .iter()
                .any(|activity| activity.activity == "WAITING_LOCK_ACTIVITY_REGRESSION")
            {
                registered_while_waiting = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(registered_while_waiting);

        drop(held_lock);
        worker.join().expect("connection lock worker");
        assert!(active_db_activity_snapshots().is_empty());
    }

    struct RegistryLockProbe<'a> {
        converted_without_registry_lock: &'a std::sync::atomic::AtomicBool,
    }

    impl From<RegistryLockProbe<'_>> for String {
        fn from(probe: RegistryLockProbe<'_>) -> Self {
            let registry_was_unlocked = db_activity_slot().try_lock().is_ok();
            probe
                .converted_without_registry_lock
                .store(registry_was_unlocked, Ordering::Relaxed);
            "Updated activity".to_string()
        }
    }

    fn oracle_test_connection_info_from_env() -> ConnectionInfo {
        let username =
            std::env::var("ORACLE_TEST_USERNAME").expect("ORACLE_TEST_USERNAME must be set");
        let password =
            std::env::var("ORACLE_TEST_PASSWORD").expect("ORACLE_TEST_PASSWORD must be set");
        let service_name = std::env::var("ORACLE_TEST_SERVICE_NAME")
            .expect("ORACLE_TEST_SERVICE_NAME must be set");
        let host = std::env::var("ORACLE_TEST_HOST").unwrap_or_else(|_| "localhost".to_string());
        let port = std::env::var("ORACLE_TEST_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(1521);

        ConnectionInfo::new_with_type(
            "local",
            &username,
            &password,
            &host,
            port,
            &service_name,
            DatabaseType::Oracle,
        )
    }

    fn oracle_thin_test_connection_info_from_env() -> ConnectionInfo {
        let mut info = oracle_test_connection_info_from_env();
        info.advanced.oracle_driver_mode = OracleDriverMode::Thin;
        info
    }

    fn read_oracle_thin_session_parameter(
        session: &mut OracleThinSession,
        parameter: &str,
    ) -> String {
        let parameter = parameter.replace('\'', "''");
        let sql =
            format!("SELECT value FROM nls_session_parameters WHERE parameter = '{parameter}'");
        DatabaseConnection::oracle_thin_select_one_text(session, &sql)
            .expect("read Oracle thin session parameter")
            .expect("Oracle thin session parameter value")
    }

    fn read_oracle_thin_session_time_zone(session: &mut OracleThinSession) -> String {
        DatabaseConnection::oracle_thin_select_one_text(session, "SELECT SESSIONTIMEZONE FROM dual")
            .expect("read Oracle thin session time zone")
            .expect("Oracle thin session time zone")
    }

    fn read_oracle_thin_default_transaction_isolation(
        session: &mut OracleThinSession,
    ) -> Option<TransactionIsolation> {
        let raw = DatabaseConnection::oracle_thin_select_one_text(
            session,
            "SELECT value FROM v$ses_optimizer_env WHERE sid = SYS_CONTEXT('USERENV', 'SID') AND name = 'transaction_isolation_level'",
        )
        .expect("read Oracle thin current transaction isolation");
        raw.as_deref()
            .and_then(TransactionIsolation::from_sql_level)
    }

    fn mysql_test_connection_info_from_env() -> ConnectionInfo {
        mysql_test_connection_info_from_env_for(DatabaseType::MySQL)
    }

    fn mysql_test_connection_info_from_env_for(db_type: DatabaseType) -> ConnectionInfo {
        let host = std::env::var("SPACE_QUERY_TEST_MYSQL_HOST")
            .expect("SPACE_QUERY_TEST_MYSQL_HOST must be set");
        let database = std::env::var("SPACE_QUERY_TEST_MYSQL_DATABASE")
            .expect("SPACE_QUERY_TEST_MYSQL_DATABASE must be set");
        let user = std::env::var("SPACE_QUERY_TEST_MYSQL_USER")
            .expect("SPACE_QUERY_TEST_MYSQL_USER must be set");
        let password = std::env::var("SPACE_QUERY_TEST_MYSQL_PASSWORD")
            .expect("SPACE_QUERY_TEST_MYSQL_PASSWORD must be set");
        let port = std::env::var("SPACE_QUERY_TEST_MYSQL_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(3306);

        ConnectionInfo::new_with_type("local", &user, &password, &host, port, &database, db_type)
    }

    /// The one leak-freedom claim every backend must honour, proven through
    /// the one discard choke point they all share
    /// (`DbSessionLease::discard_physical`): a discarded session hands its
    /// pool slot back. A backend that violates it accumulates ghost
    /// connections until acquire times out with "pool appears exhausted"
    /// while almost no real sessions exist — live-observed on the MySQL
    /// family, whose `PooledConn::unwrap` discard skipped the pool's Drop
    /// accounting. Each backend joins by handing this engine its acquire
    /// function; the discard side is deliberately NOT pluggable.
    fn assert_discarded_sessions_release_their_pool_slots(
        label: &str,
        acquire: &dyn Fn(usize) -> DbSessionLease,
    ) {
        // More discard rounds than the pool has slots (2): with a slot leak,
        // round 3 already finds the pool full of ghosts and times out.
        for round in 0..4 {
            acquire(round).discard_physical("pool slot probe");
        }

        // And the freed slots must be genuinely usable, both at once.
        let first = acquire(4);
        let second = acquire(5);
        drop(first);
        drop(second);
        let _ = label;
    }

    fn assert_mysql_family_discarded_sessions_release_their_pool_slots(db_type: DatabaseType) {
        let info = mysql_test_connection_info_from_env_for(db_type);
        let pool = DatabaseConnection::build_mysql_pool(
            &info,
            2,
            ConnectionAttemptPolicy::from_seconds(5),
        )
        .expect("build MySQL-family test pool");

        assert_discarded_sessions_release_their_pool_slots(db_type.display_name(), &|round| {
            DbSessionLease::MySQL {
                conn: pool
                    .try_get_conn(Duration::from_secs(3))
                    .unwrap_or_else(|err| {
                        panic!("round {round} could not acquire a pooled connection: {err}")
                    }),
                db_type,
            }
        });
    }

    #[test]
    #[ignore = "requires local MySQL test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_discarded_sessions_release_their_pool_slots() {
        assert_mysql_family_discarded_sessions_release_their_pool_slots(DatabaseType::MySQL);
    }

    #[test]
    #[ignore = "requires local MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mariadb_discarded_sessions_release_their_pool_slots() {
        assert_mysql_family_discarded_sessions_release_their_pool_slots(DatabaseType::MariaDB);
    }

    #[test]
    #[ignore = "requires local Oracle database via ORACLE_TEST_* env vars and an Oracle client"]
    fn oracle_oci_discarded_sessions_release_their_pool_slots() {
        ensure_oracle_client_initialized().expect("Oracle client should initialize");
        let info = oracle_test_connection_info_from_env();
        let pool = DatabaseConnection::build_oracle_pool(
            &info,
            2,
            ConnectionAttemptPolicy::from_seconds(5),
        )
        .expect("build Oracle OCI test pool");

        assert_discarded_sessions_release_their_pool_slots("Oracle OCI", &|round| {
            DbSessionLease::Oracle(Arc::new(pool.get().unwrap_or_else(|err| {
                panic!("round {round} could not acquire an OCI pooled session: {err}")
            })))
        });
    }

    #[test]
    #[ignore = "requires local Oracle database via ORACLE_TEST_* env vars"]
    fn oracle_thin_discarded_sessions_release_their_pool_slots() {
        let info = oracle_test_connection_info_from_env();
        let config = DatabaseConnection::build_oracle_thin_config(
            &info,
            ConnectionAttemptPolicy::from_seconds(5),
        )
        .expect("build Oracle thin config");
        let pool = OracleThinSessionPool::new(
            config,
            tns_thin::pool::PoolOptions {
                max_size: 2,
                acquire_timeout: Duration::from_secs(3),
            },
        );

        assert_discarded_sessions_release_their_pool_slots("Oracle thin", &|round| {
            DbSessionLease::OracleThin(Box::new(pool.acquire().unwrap_or_else(|err| {
                panic!("round {round} could not acquire a thin pooled session: {err}")
            })))
        });
    }

    /// The server's own view of how many sessions this app has open.
    ///
    /// Pool-slot accounting is the app's bookkeeping; this is the database's.
    /// Only the server can prove that a lifecycle event actually *closed* a
    /// session rather than merely losing track of it, which is why every
    /// backend implements this the same way for the one lifecycle engine
    /// below.
    trait ServerSessionCensus {
        fn count_sessions(&mut self) -> usize;
    }

    /// The census must see this test's sessions and nobody else's, or another
    /// test opening a connection at the same time reads as a leak. Each entry
    /// point therefore gives its probe connection an identity of its own and
    /// counts by that: a database of its own on the MySQL family, a user of
    /// its own on Oracle. The identity belongs to the entry point rather than
    /// to the backend, so two of these can run side by side.
    fn session_census_probe_name(entry_point: &str) -> String {
        format!("sq_session_probe_{entry_point}")
    }
    const SESSION_CENSUS_PROBE_ORACLE_PASSWORD: &str = "sq_probe_2026";

    struct MySqlFamilySessionCensus {
        conn: mysql::Conn,
        database: String,
    }

    impl ServerSessionCensus for MySqlFamilySessionCensus {
        fn count_sessions(&mut self) -> usize {
            use mysql::prelude::Queryable;
            let database = self.database.replace('\'', "''");
            let sql = format!(
                "SELECT COUNT(*) FROM information_schema.processlist WHERE db = '{database}'"
            );
            self.conn
                .query_first::<i64, _>(sql)
                .expect("count MySQL-family server sessions")
                .unwrap_or_default()
                .max(0) as usize
        }
    }

    struct OracleOciSessionCensus {
        conn: Connection,
        user: String,
    }

    /// Give the Oracle probe a user of its own. In a CDB root the name has to
    /// be a common one, so fall back to the `C##` form and report which name
    /// the census should count.
    fn create_oracle_session_census_probe_user(
        entry_point: &str,
        mut execute: impl FnMut(&str) -> Result<(), String>,
    ) -> String {
        let mut user = session_census_probe_name(entry_point).to_uppercase();
        let create = |user: &str| {
            format!("CREATE USER {user} IDENTIFIED BY \"{SESSION_CENSUS_PROBE_ORACLE_PASSWORD}\"")
        };
        if let Err(err) = execute(&create(&user)) {
            let message = err.to_ascii_lowercase();
            if message.contains("ora-65096") {
                // "invalid common user or role name": this is a CDB root.
                user = format!("C##{user}");
                if let Err(err) = execute(&create(&user)) {
                    assert!(
                        err.to_ascii_lowercase().contains("ora-01920"),
                        "create the Oracle session census probe user: {err}"
                    );
                }
            } else {
                assert!(
                    message.contains("ora-01920"),
                    "create the Oracle session census probe user: {err}"
                );
            }
        }
        for grant in [
            format!("GRANT CREATE SESSION TO {user}"),
            format!("GRANT SELECT ANY DICTIONARY TO {user}"),
        ] {
            execute(&grant).unwrap_or_else(|err| panic!("{grant}: {err}"));
        }
        user
    }

    impl ServerSessionCensus for OracleOciSessionCensus {
        fn count_sessions(&mut self) -> usize {
            let count: i64 = self
                .conn
                .query_row_as(ORACLE_SESSION_CENSUS_SQL, &[&self.user.to_uppercase()])
                .expect("count Oracle server sessions");
            count.max(0) as usize
        }
    }

    struct OracleThinSessionCensus {
        session: OracleThinSession,
        user: String,
    }

    impl ServerSessionCensus for OracleThinSessionCensus {
        fn count_sessions(&mut self) -> usize {
            let sql = format!(
                "SELECT COUNT(*) FROM v$session WHERE username = '{}'",
                self.user.to_uppercase().replace('\'', "''")
            );
            DatabaseConnection::oracle_thin_select_one_text(&mut self.session, &sql)
                .expect("count Oracle server sessions")
                .and_then(|value| value.trim().parse::<i64>().ok())
                .unwrap_or_default()
                .max(0) as usize
        }
    }

    const ORACLE_SESSION_CENSUS_SQL: &str = "SELECT COUNT(*) FROM v$session WHERE username = :1";

    /// Poll until the server's session count comes down to the limit, so a
    /// backend that closes its sockets asynchronously (the server still has to
    /// notice the FIN) is judged on where it ends up, not on the instant after
    /// the call returned.
    fn settled_server_session_count(census: &mut dyn ServerSessionCensus, limit: usize) -> usize {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut observed = census.count_sessions();
        while observed > limit && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(250));
            observed = census.count_sessions();
        }
        observed
    }

    /// Read a count that has stopped moving, for the reference points the
    /// engine measures rather than predicts.
    fn stable_server_session_count(census: &mut dyn ServerSessionCensus) -> usize {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut previous = census.count_sessions();
        loop {
            std::thread::sleep(Duration::from_millis(400));
            let observed = census.count_sessions();
            if observed == previous || Instant::now() >= deadline {
                return observed;
            }
            previous = observed;
        }
    }

    /// The leak claim is one-sided: after the event, no more than `limit`
    /// sessions may still be open. Closing more than that is not a leak -- a
    /// discard legitimately takes the pool below the size it kept idle before.
    fn assert_server_sessions_at_most(
        census: &mut dyn ServerSessionCensus,
        limit: usize,
        label: &str,
    ) {
        let observed = settled_server_session_count(census, limit);
        assert!(
            observed <= limit,
            "{label}: the server still has {observed} sessions open, expected at most {limit}"
        );
    }

    /// The second leak-freedom claim every backend must honour, and the one
    /// pool-slot accounting cannot see: when a connection is torn down or
    /// replaced, every session it opened is *closed on the server* — including
    /// the pool's idle sessions and the one a query tab is still holding.
    ///
    /// A backend that violates it leaves real sessions on the database for as
    /// long as the app runs: the user disconnects, reconnects or resizes the
    /// pool and the server keeps counting connections nobody can reach any
    /// more. Each backend joins by handing this engine a connection and a
    /// census of the server's own session list.
    fn assert_connection_lifecycle_closes_every_server_session(
        info: ConnectionInfo,
        census: &mut dyn ServerSessionCensus,
    ) {
        const POOL_SIZE: u32 = 4;
        let policy = ConnectionAttemptPolicy::from_seconds(30);
        let db_type = info.db_type;
        let activity = track_pool_db_activity("server session census", db_type);

        // Reference point 1: nothing of ours is connected but the census.
        let disconnected_baseline = stable_server_session_count(census);

        let shared = create_shared_connection();
        connect_shared_connection_with_policy(&shared, info.clone(), POOL_SIZE, policy)
            .expect("connect the probe connection");

        // Reference point 2: connected, but no pooled work has run yet.
        let connected_baseline = stable_server_session_count(census);
        assert!(
            connected_baseline > disconnected_baseline,
            "connecting should open at least one server session ({disconnected_baseline} -> {connected_baseline})"
        );

        let context = |shared: &SharedConnection| {
            shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pool_session_context()
                .expect("pool session context")
        };
        let acquire = |context: &DbPoolSessionContext| {
            context
                .acquire_session_for_current_scope(&activity)
                .expect("acquire a pooled session")
                .0
                .into_lease()
        };

        // L1: a discarded session is gone from the server, not just from the
        // pool's slot count.
        let ctx = context(&shared);
        let first = acquire(&ctx);
        let second = acquire(&ctx);
        let working = stable_server_session_count(census);
        assert!(
            working > connected_baseline,
            "two pooled sessions should be visible on the server ({connected_baseline} -> {working})"
        );
        first.discard_physical("session census probe");
        second.discard_physical("session census probe");
        assert_server_sessions_at_most(
            census,
            connected_baseline,
            "L1 discarding a pooled session closes it on the server",
        );

        // L2: a session returned to the pool stays open (that is the point of
        // pooling) and is reused rather than re-opened.
        let returned_first = acquire(&ctx);
        let returned_second = acquire(&ctx);
        let pooled = stable_server_session_count(census);
        drop(returned_first);
        drop(returned_second);
        let reacquired_first = acquire(&ctx);
        let reacquired_second = acquire(&ctx);
        assert_server_sessions_at_most(
            census,
            pooled,
            "L2 a returned pooled session is reused, not re-opened",
        );

        // L3: closing a query tab closes the session that tab was holding.
        let tab_lease = SharedDbSessionLease::new();
        assert!(
            tab_lease.apply_retained_session_disposition(
                ctx.connection_generation,
                ctx.pool_context_epoch(),
                reacquired_first,
                RetainedSessionDisposition::Retain(RetainedSessionState::default()),
                "session census probe",
            ),
            "the probe tab should retain its session"
        );
        tab_lease.clear();
        assert_server_sessions_at_most(
            census,
            pooled - 1,
            "L3 closing a query tab closes its retained session",
        );

        // L4: a tab that goes away WITHOUT closing its session leaves nobody
        // to hand it back. The session must not drift on regardless.
        let orphaned_tab_lease = SharedDbSessionLease::new();
        assert!(
            orphaned_tab_lease.apply_retained_session_disposition(
                ctx.connection_generation,
                ctx.pool_context_epoch(),
                reacquired_second,
                RetainedSessionDisposition::Retain(RetainedSessionState::default()),
                "session census probe",
            ),
            "the probe tab should retain its session"
        );
        drop(orphaned_tab_lease);
        assert_server_sessions_at_most(
            census,
            pooled - 2,
            "L4 a retained session whose owner is gone is closed, not orphaned",
        );

        // L11: a session handed back to a slot that was CLOSED, not merely
        // cleared. A cancelled statement can outlive its tab and hand its
        // session back afterwards; the closed slot must close that session
        // rather than retain it where nobody will ever clear it again.
        // Live-observed on Oracle thin before the closed flag existed.
        let before_late_handback = stable_server_session_count(census);
        let late = acquire(&ctx);
        let closed_tab_lease = SharedDbSessionLease::new();
        closed_tab_lease.close_for_owner_shutdown();
        assert!(
            !closed_tab_lease.apply_retained_session_disposition(
                ctx.connection_generation,
                ctx.pool_context_epoch(),
                late,
                RetainedSessionDisposition::Retain(RetainedSessionState::default()),
                "session census probe",
            ),
            "a closed slot must refuse to retain a session"
        );
        assert!(
            closed_tab_lease.snapshot().is_none(),
            "a closed slot must stay empty after a refused store"
        );
        assert_server_sessions_at_most(
            census,
            before_late_handback,
            "L11 a session handed back to a closed tab slot is closed",
        );

        // L5: disconnect closes every session the connection opened, including
        // the pool's idle ones and the one a query tab is still holding.
        let retained = acquire(&ctx);
        let idle = acquire(&ctx);
        drop(idle);
        let tab_lease = SharedDbSessionLease::new();
        assert!(
            tab_lease.apply_retained_session_disposition(
                ctx.connection_generation,
                ctx.pool_context_epoch(),
                retained,
                RetainedSessionDisposition::Retain(RetainedSessionState::default()),
                "session census probe",
            ),
            "the probe tab should retain its session"
        );
        drop(ctx);
        shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .disconnect();
        assert_server_sessions_at_most(
            census,
            disconnected_baseline,
            "L5 disconnect closes every session, including a tab's retained one",
        );
        tab_lease.clear();

        // L6: reconnecting replaces the connection, and the replaced one must
        // not keep sessions alive — again with a tab still holding one.
        connect_shared_connection_with_policy(&shared, info.clone(), POOL_SIZE, policy)
            .expect("reconnect the probe connection");
        let ctx = context(&shared);
        let retained = acquire(&ctx);
        let idle = acquire(&ctx);
        drop(idle);
        let tab_lease = SharedDbSessionLease::new();
        assert!(
            tab_lease.apply_retained_session_disposition(
                ctx.connection_generation,
                ctx.pool_context_epoch(),
                retained,
                RetainedSessionDisposition::Retain(RetainedSessionState::default()),
                "session census probe",
            ),
            "the probe tab should retain its session"
        );
        drop(ctx);
        connect_shared_connection_with_policy(&shared, info.clone(), POOL_SIZE, policy)
            .expect("reconnect the probe connection again");
        assert_server_sessions_at_most(
            census,
            connected_baseline,
            "L6 reconnecting closes the replaced connection's sessions",
        );
        tab_lease.clear();

        // L7: resizing the pool retires the old pool, which must take its
        // sessions with it — again with a tab still holding one.
        let ctx = context(&shared);
        let retained = acquire(&ctx);
        let idle = acquire(&ctx);
        drop(idle);
        let tab_lease = SharedDbSessionLease::new();
        assert!(
            tab_lease.apply_retained_session_disposition(
                ctx.connection_generation,
                ctx.pool_context_epoch(),
                retained,
                RetainedSessionDisposition::Retain(RetainedSessionState::default()),
                "session census probe",
            ),
            "the probe tab should retain its session"
        );
        drop(ctx);
        resize_shared_connection_pool_with_policy(&shared, POOL_SIZE + 2, policy)
            .expect("resize the probe connection pool");
        assert_server_sessions_at_most(
            census,
            connected_baseline,
            "L7 resizing the pool closes the retired pool's sessions",
        );
        tab_lease.clear();

        // L10: a connection attempt that never becomes a live connection. The
        // Test button opens one and throws it away, and an attempt that is no
        // longer current is retired before it is ever installed -- neither has
        // an owner to close it later, so neither may leave a session behind.
        {
            DatabaseConnection::test_connection_with_policy(&info, policy)
                .expect("test the probe connection");
            let abandoned =
                DatabaseConnection::prepare_connection(info.clone(), POOL_SIZE, false, policy)
                    .expect("prepare a connection the way a connect attempt does");
            DatabaseConnection::retire_connection_in_background(abandoned);
            assert_server_sessions_at_most(
                census,
                connected_baseline,
                "L10 a connection attempt that is thrown away leaves no session",
            );
        }

        // L9: a connection nobody disconnected. A script CONNECT builds a whole
        // connection and pool behind a query tab, and closing the tab drops it
        // rather than disconnecting it -- so dropping the last handle has to
        // close its sessions just as thoroughly as a disconnect would.
        {
            let orphan = create_shared_connection();
            connect_shared_connection_with_policy(&orphan, info.clone(), POOL_SIZE, policy)
                .expect("connect the orphan probe connection");
            let orphan_context = context(&orphan);
            let first = acquire(&orphan_context);
            let second = acquire(&orphan_context);
            let working = stable_server_session_count(census);
            assert!(
                working > connected_baseline,
                "the orphan connection should have opened sessions of its own \
                 ({connected_baseline} -> {working})"
            );
            drop(first);
            drop(second);
            drop(orphan_context);
            drop(orphan);
            assert_server_sessions_at_most(
                census,
                connected_baseline,
                "L9 dropping a connection nobody disconnected closes its sessions",
            );
        }

        // L8: connect/disconnect cycles that do real pooled work leave nothing
        // behind, so a session-per-cycle leak cannot hide under a single pass.
        shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .disconnect();
        assert_server_sessions_at_most(
            census,
            disconnected_baseline,
            "L8 the probe connection is fully closed before the cycles",
        );
        for cycle in 0..3 {
            connect_shared_connection_with_policy(&shared, info.clone(), POOL_SIZE, policy)
                .unwrap_or_else(|err| panic!("cycle {cycle} connect: {err}"));
            let ctx = context(&shared);
            let held = acquire(&ctx);
            let returned = acquire(&ctx);
            drop(returned);
            let tab_lease = SharedDbSessionLease::new();
            tab_lease.apply_retained_session_disposition(
                ctx.connection_generation,
                ctx.pool_context_epoch(),
                held,
                RetainedSessionDisposition::Retain(RetainedSessionState::default()),
                "session census probe",
            );
            drop(ctx);
            shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .disconnect();
            assert_server_sessions_at_most(
                census,
                disconnected_baseline,
                &format!("L8 connect/disconnect cycle {cycle} leaves no session behind"),
            );
            tab_lease.clear();
        }

        // L12: two live connections at once. Teardown is keyed on a
        // process-wide connection generation precisely so that one
        // connection's disconnect can never reach another connection's
        // sessions -- and must still take every one of its own. Both claims
        // in one event: the survivor's retained session stays, the departing
        // connection's retained session is reclaimed, and the server count
        // comes down to exactly the survivor's footprint.
        {
            let survivor = create_shared_connection();
            connect_shared_connection_with_policy(&survivor, info.clone(), POOL_SIZE, policy)
                .expect("connect the survivor connection");
            let survivor_ctx = context(&survivor);
            let survivor_retained = acquire(&survivor_ctx);
            let survivor_lease = SharedDbSessionLease::new();
            assert!(
                survivor_lease.apply_retained_session_disposition(
                    survivor_ctx.connection_generation,
                    survivor_ctx.pool_context_epoch(),
                    survivor_retained,
                    RetainedSessionDisposition::Retain(RetainedSessionState::default()),
                    "session census probe",
                ),
                "the survivor tab should retain its session"
            );
            let survivor_only = stable_server_session_count(census);

            connect_shared_connection_with_policy(&shared, info.clone(), POOL_SIZE, policy)
                .expect("connect the departing connection");
            let departing_ctx = context(&shared);
            let departing_retained = acquire(&departing_ctx);
            let departing_lease = SharedDbSessionLease::new();
            assert!(
                departing_lease.apply_retained_session_disposition(
                    departing_ctx.connection_generation,
                    departing_ctx.pool_context_epoch(),
                    departing_retained,
                    RetainedSessionDisposition::Retain(RetainedSessionState::default()),
                    "session census probe",
                ),
                "the departing tab should retain its session"
            );
            let both = stable_server_session_count(census);
            assert!(
                both > survivor_only,
                "the departing connection should have opened sessions of its own \
                 ({survivor_only} -> {both})"
            );

            drop(departing_ctx);
            shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .disconnect();
            assert_server_sessions_at_most(
                census,
                survivor_only,
                "L12 disconnecting one connection closes only that connection's sessions",
            );
            wait_for_lease_reclaim(
                &departing_lease,
                "L12 the departing connection's retained session is reclaimed",
            );
            assert!(
                survivor_lease.snapshot().is_some(),
                "L12 the surviving connection's retained session must not be \
                 reclaimed by another connection's teardown"
            );

            drop(survivor_ctx);
            survivor
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .disconnect();
            assert_server_sessions_at_most(
                census,
                disconnected_baseline,
                "L12 disconnecting the second connection closes the rest",
            );
            wait_for_lease_reclaim(
                &survivor_lease,
                "L12 the surviving connection's retained session is reclaimed by its own teardown",
            );
        }
    }

    /// A retained lease is reclaimed by a background thread after its
    /// connection's teardown; wait for that to land rather than asserting on
    /// the instant after disconnect returned.
    fn wait_for_lease_reclaim(lease: &SharedDbSessionLease, label: &str) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while lease.snapshot().is_some() {
            assert!(
                Instant::now() < deadline,
                "{label}: the retained lease was never reclaimed"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn assert_mysql_family_connection_lifecycle_closes_every_server_session(
        db_type: DatabaseType,
        entry_point: &str,
    ) {
        use mysql::prelude::Queryable;
        let mut info = mysql_test_connection_info_from_env_for(db_type);
        let opts =
            DatabaseConnection::build_mysql_opts(&info, ConnectionAttemptPolicy::from_seconds(30));
        let mut conn = mysql::Conn::new(opts).expect("connect the MySQL-family census");
        let probe_database = session_census_probe_name(entry_point);
        conn.query_drop(format!("CREATE DATABASE IF NOT EXISTS {probe_database}"))
            .expect("create the MySQL-family session census probe database");
        // The probe connection lives in a database of its own, so the census
        // can count its sessions and only its sessions.
        info.service_name = probe_database.clone();
        let mut census = MySqlFamilySessionCensus {
            conn,
            database: probe_database,
        };
        assert_connection_lifecycle_closes_every_server_session(info, &mut census);
    }

    #[test]
    #[ignore = "requires local MySQL test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_connection_lifecycle_closes_every_server_session() {
        assert_mysql_family_connection_lifecycle_closes_every_server_session(
            DatabaseType::MySQL,
            "mysql",
        );
    }

    #[test]
    #[ignore = "requires local MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mariadb_connection_lifecycle_closes_every_server_session() {
        assert_mysql_family_connection_lifecycle_closes_every_server_session(
            DatabaseType::MariaDB,
            "mariadb",
        );
    }

    #[test]
    #[ignore = "requires local Oracle database via ORACLE_TEST_* env vars and an Oracle client"]
    fn oracle_oci_connection_lifecycle_closes_every_server_session() {
        ensure_oracle_client_initialized().expect("Oracle client should initialize");
        let mut info = oracle_test_connection_info_from_env();
        let conn = Connection::connect(&info.username, &info.password, info.connection_string())
            .expect("connect the Oracle OCI census");
        let probe_user = create_oracle_session_census_probe_user("oci", |sql| {
            conn.execute(sql, &[])
                .map(|_| ())
                .map_err(|err| err.to_string())
        });
        info.username = probe_user.clone();
        info.password = SESSION_CENSUS_PROBE_ORACLE_PASSWORD.to_string();
        let mut census = OracleOciSessionCensus {
            conn,
            user: probe_user,
        };
        assert_connection_lifecycle_closes_every_server_session(info, &mut census);
    }

    #[test]
    #[ignore = "requires local Oracle database via ORACLE_TEST_* env vars"]
    fn oracle_thin_connection_lifecycle_closes_every_server_session() {
        let mut info = oracle_thin_test_connection_info_from_env();
        let config = DatabaseConnection::build_oracle_thin_config(
            &info,
            ConnectionAttemptPolicy::from_seconds(30),
        )
        .expect("build Oracle thin census config");
        let mut session =
            OracleThinSession::connect(config).expect("connect the Oracle thin census");
        let probe_user = create_oracle_session_census_probe_user("thin", |sql| {
            session.query_drop(sql).map_err(|err| err.to_string())
        });
        info.username = probe_user.clone();
        info.password = SESSION_CENSUS_PROBE_ORACLE_PASSWORD.to_string();
        let mut census = OracleThinSessionCensus {
            session,
            user: probe_user,
        };
        assert_connection_lifecycle_closes_every_server_session(info, &mut census);
    }

    fn db_activity_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The guarantees rest on one claim: a caller cannot get hold of a live DB
    /// session or connection without an activity for it to hang off. The
    /// compiler enforces that for the entry points that exist today — but not
    /// that the set of entry points stays closed. This test does.
    ///
    /// It reads this file and requires every public API that hands out a
    /// session or a live connection handle to take a `DbActivityGuard`. Adding
    /// a new one without tracking fails here rather than silently reopening the
    /// hole that retained leases had: enforcement was never the weak part, the
    /// enumeration was.
    #[test]
    fn every_way_to_get_a_session_requires_an_activity() {
        /// Types whose value IS a usable session or live connection handle.
        /// Matched whole, so `DbConnectionPool` and `DbPoolSessionContext` —
        /// which only let you acquire, and acquiring is already enforced — do
        /// not count.
        const HANDS_OUT_A_SESSION: [&str; 5] = [
            "DbPoolSession",
            "DbSessionLease",
            "DbConnection",
            "TakenDbSessionLease",
            "RetainedSessionTakeOutcome",
        ];
        /// Conversions and accessors on a handle that is already tracked, plus
        /// the `DatabaseConnection` accessors that `ConnectionLockGuard`
        /// shadows to attach before delegating.
        const ALREADY_TRACKED: [&str; 8] = [
            "fn into_lease",
            "fn into_oracle_connection",
            "fn lease_mut",
            "fn acquire_session_untracked",
            "fn require_live_connection",
            "fn require_live_db_connection",
            "fn get_connection",
            "fn get_db_connection",
        ];

        fn mentions_type(return_type: &str, wanted: &str) -> bool {
            let mut rest = return_type;
            while let Some(at) = rest.find(wanted) {
                let after = &rest[at + wanted.len()..];
                let next_is_boundary = after
                    .chars()
                    .next()
                    .is_none_or(|ch| !ch.is_alphanumeric() && ch != '_');
                if next_is_boundary {
                    return true;
                }
                rest = &rest[at + wanted.len()..];
            }
            false
        }

        let source = include_str!("connection.rs");
        let lines: Vec<&str> = source.lines().collect();
        let mut untracked = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            let is_public_fn = trimmed.starts_with("pub fn ")
                || trimmed.starts_with("pub(crate) fn ")
                || trimmed.starts_with("pub(super) fn ");
            if !is_public_fn {
                continue;
            }
            let Some((_, return_type)) = trimmed.split_once("->") else {
                continue;
            };
            if !HANDS_OUT_A_SESSION
                .iter()
                .any(|handle| mentions_type(return_type, handle))
            {
                continue;
            }
            if ALREADY_TRACKED
                .iter()
                .any(|exempt| trimmed.contains(exempt))
            {
                continue;
            }
            // Test-only helpers are not a production entry point.
            let is_test_only = lines[index.saturating_sub(3)..index]
                .iter()
                .any(|preceding| preceding.trim() == "#[cfg(test)]");
            if is_test_only {
                continue;
            }
            // Signatures wrap, so look at the whole parameter list.
            let signature: String = lines
                .iter()
                .skip(index)
                .take(16)
                .copied()
                .collect::<Vec<_>>()
                .join(" ");
            if !signature.contains("DbActivityGuard") {
                untracked.push(format!("line {}: {trimmed}", index + 1));
            }
        }

        assert!(
            untracked.is_empty(),
            "these hand out a DB session or connection without requiring a DbActivityGuard, so \
             work started through them would be invisible to the status bar, unreachable by the \
             cancel button, and immune to the stale sweep:\n{}",
            untracked.join("\n")
        );
    }

    #[derive(Default)]
    struct TestCanceler {
        interrupted: AtomicBool,
        forced: AtomicBool,
    }

    impl DbActivityCanceler for TestCanceler {
        fn interrupt(&self) -> Result<(), String> {
            self.interrupted.store(true, Ordering::Release);
            Ok(())
        }

        fn force(&self) -> Result<(), String> {
            self.forced.store(true, Ordering::Release);
            Ok(())
        }

        fn label(&self) -> &'static str {
            "test"
        }
    }

    /// The registry retires an activity synchronously but performs the break on
    /// the watchdog thread, so tests wait for it rather than assuming it landed
    /// before `cancel_db_activity` returned.
    fn wait_for(what: &str, ready: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if ready() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for {what}");
    }

    fn stale_lifetime() -> (DbActivityLifetime, Arc<AtomicU64>) {
        let token = Arc::new(AtomicU64::new(4));
        let lifetime = DbActivityLifetime {
            epoch_token: Arc::clone(&token),
            epoch: 4,
        };
        (lifetime, token)
    }

    #[test]
    fn an_activity_without_a_session_is_not_offered_as_cancelable() {
        let _test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        let _activity = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);

        assert!(!active_db_activity_snapshots()[0].cancelable);
        clear_tracked_db_activity();
    }

    #[test]
    fn attaching_a_session_makes_the_activity_cancelable_and_cancel_breaks_it() {
        let _test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        let activity = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);
        let canceler = Arc::new(TestCanceler::default());
        let _registration = activity.attach_canceler(canceler.clone());

        assert!(active_db_activity_snapshots()[0].cancelable);
        assert!(cancel_db_activity(activity.id(), Duration::from_secs(60)));
        wait_for("the session to be broken", || {
            canceler.interrupted.load(Ordering::Acquire)
        });
        assert!(
            active_db_activity_snapshots().is_empty(),
            "a cancelled activity must stop showing as in progress at once"
        );
        assert!(
            activity.is_finished(),
            "the worker must be able to see that it was cancelled"
        );
        clear_tracked_db_activity();
    }

    #[test]
    fn releasing_a_session_stops_a_cancel_landing_on_it() {
        let _test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        let activity = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);
        let canceler = Arc::new(TestCanceler::default());
        {
            let _registration = activity.attach_canceler(canceler.clone());
            assert!(active_db_activity_snapshots()[0].cancelable);
        }

        // The session went back to the pool, so it may now belong to someone
        // else and must not be broken by this activity's cancel.
        assert!(!active_db_activity_snapshots()[0].cancelable);
        cancel_db_activity(activity.id(), Duration::from_secs(60));
        // Give the watchdog thread a chance to act, so this proves the cancel
        // does not reach a released session rather than just racing it.
        std::thread::sleep(Duration::from_millis(200));
        assert!(!canceler.interrupted.load(Ordering::Acquire));
        clear_tracked_db_activity();
    }

    #[test]
    fn a_session_that_fans_out_is_cancelled_on_every_branch() {
        let _test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        let activity = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);
        let first = Arc::new(TestCanceler::default());
        let second = Arc::new(TestCanceler::default());
        let _first_registration = activity.attach_canceler(first.clone());
        let _second_registration = activity.attach_canceler(second.clone());

        cancel_db_activity(activity.id(), Duration::from_secs(60));

        wait_for("both sessions to be broken", || {
            first.interrupted.load(Ordering::Acquire) && second.interrupted.load(Ordering::Acquire)
        });
        clear_tracked_db_activity();
    }

    #[test]
    fn a_session_that_ends_leaves_nothing_showing_as_in_progress() {
        let _test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        let activity = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);
        let canceler = Arc::new(TestCanceler::default());
        let _registration = activity.attach_canceler(canceler.clone());
        let (lifetime, epoch_token) = stale_lifetime();
        activity.bind_lifetime(lifetime);

        assert_eq!(sweep_stale_db_activities(Duration::from_secs(60)), 0);
        assert_eq!(active_db_activity_snapshots().len(), 1);

        // Every teardown path bumps the pool context epoch; this is what the
        // registry sees when a connection goes away.
        epoch_token.fetch_add(1, Ordering::AcqRel);

        assert_eq!(sweep_stale_db_activities(Duration::from_secs(60)), 1);
        assert!(
            active_db_activity_snapshots().is_empty(),
            "a finished session must never leave work showing as in progress"
        );
        wait_for("the session to be broken", || {
            canceler.interrupted.load(Ordering::Acquire)
        });
        clear_tracked_db_activity();
    }

    #[test]
    fn an_activity_with_no_lifetime_is_left_alone_by_the_sweep() {
        let _test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        let _activity = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);

        assert_eq!(sweep_stale_db_activities(Duration::from_secs(60)), 0);
        assert_eq!(active_db_activity_snapshots().len(), 1);
        clear_tracked_db_activity();
    }

    #[test]
    fn closing_a_connection_retires_its_work_only() {
        let _test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        let registry = crate::db::ConnectionRegistry::new();
        let closing = registry.register_unmanaged(create_shared_connection()).id();
        let other = registry.register_unmanaged(create_shared_connection()).id();
        let _closing_activity = track_pool_db_activity_for_connection(
            "Loading metadata",
            DatabaseType::Oracle,
            closing,
        );
        let _other_activity =
            track_pool_db_activity_for_connection("Loading metadata", DatabaseType::Oracle, other);

        assert_eq!(
            cancel_db_activities_for_connection(closing, Duration::from_secs(60)),
            1
        );

        let remaining = active_db_activity_snapshots();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].connection_id, Some(other));
        clear_tracked_db_activity();
    }

    #[test]
    fn a_cancel_that_is_ignored_escalates_to_the_force_tier() {
        let _test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        let activity = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);
        let canceler = Arc::new(TestCanceler::default());
        let _registration = activity.attach_canceler(canceler.clone());

        // Holding the guard is what "the worker never let go" looks like.
        cancel_db_activity(activity.id(), Duration::ZERO);

        let deadline = Instant::now() + Duration::from_secs(5);
        while !canceler.forced.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            canceler.forced.load(Ordering::Acquire),
            "a break the work ignores must be escalated to a force close"
        );
        clear_tracked_db_activity();
    }

    #[test]
    fn a_cancel_the_work_honors_is_not_escalated() {
        let _test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        let activity = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);
        let canceler = Arc::new(TestCanceler::default());
        let _registration = activity.attach_canceler(canceler.clone());

        cancel_db_activity(activity.id(), Duration::from_secs(30));
        // Dropping the guard is what "the worker returned" looks like.
        drop(_registration);
        drop(activity);

        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !canceler.forced.load(Ordering::Acquire),
            "work that stopped on the graceful break must not be force closed"
        );
        clear_tracked_db_activity();
    }

    #[test]
    fn graceful_cancel_wait_returns_as_soon_as_the_break_lands() {
        let started = Instant::now();
        let landed = AtomicBool::new(false);

        let escalate = wait_for_graceful_cancel(Duration::from_secs(60), || {
            !landed.swap(true, Ordering::AcqRel)
        });

        assert!(!escalate);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn graceful_cancel_wait_escalates_when_the_break_never_lands() {
        assert!(wait_for_graceful_cancel(Duration::ZERO, || true));
    }

    #[derive(Default)]
    struct TestRegistrationHolder {
        held: Mutex<Vec<DbSessionCancelRegistration>>,
    }

    impl HoldsSessionCancelRegistration for TestRegistrationHolder {
        fn hold_session_registration(&self, registration: DbSessionCancelRegistration) {
            self.held
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(registration);
        }
    }

    #[test]
    fn a_session_stays_cancelable_after_the_frame_that_acquired_it_returns() {
        let _test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let canceler = Arc::new(TestCanceler::default());
        let holder = TestRegistrationHolder::default();

        // A session is acquired in one frame and used by the rest of the
        // execution, so the registration has to outlive the acquiring frame.
        {
            let registration = activity.attach_canceler(canceler.clone());
            holder.hold_session_registration(registration);
        }

        assert!(
            active_db_activity_snapshots()[0].cancelable,
            "a query must stay cancelable for its whole run, not only while its session is acquired"
        );
        cancel_db_activity(activity.id(), Duration::from_secs(60));
        wait_for("the session to be broken", || {
            canceler.interrupted.load(Ordering::Acquire)
        });
        clear_tracked_db_activity();
    }

    #[test]
    fn work_on_the_main_connection_is_retired_when_the_connection_goes() {
        let _test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        // Main-connection work (scope switch, commit, ALTER SESSION) is bound to
        // the connection generation, not the pool context epoch: ordinary
        // operations bump the epoch while holding the lock, and binding to that
        // would make the sweep cancel them mid-flight.
        let generation = Arc::new(AtomicU64::new(3));
        let activity = track_db_activity("Switching schema", Some(DatabaseType::Oracle));
        activity.bind_lifetime(DbActivityLifetime {
            epoch_token: Arc::clone(&generation),
            epoch: 3,
        });

        assert_eq!(sweep_stale_db_activities(Duration::from_secs(60)), 0);

        generation.store(4, Ordering::Release);

        assert_eq!(sweep_stale_db_activities(Duration::from_secs(60)), 1);
        assert!(
            active_db_activity_snapshots().is_empty(),
            "a closed connection must not leave its own work showing as in progress"
        );
        clear_tracked_db_activity();
    }

    struct PanickingCanceler;

    impl DbActivityCanceler for PanickingCanceler {
        fn interrupt(&self) -> Result<(), String> {
            panic!("driver exploded during interrupt");
        }

        fn force(&self) -> Result<(), String> {
            panic!("driver exploded during force");
        }

        fn label(&self) -> &'static str {
            "panicking"
        }
    }

    #[test]
    fn a_backend_that_panics_while_cancelling_does_not_take_the_caller_down() {
        let _test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        // Cancels run on the UI thread (the status tick sweeps there), so a
        // driver that panics must not unwind into the caller, and must not stop
        // the other sessions from being cancelled.
        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let survivor = Arc::new(TestCanceler::default());
        let _panicking = activity.attach_canceler(Arc::new(PanickingCanceler));
        let _survivor_registration = activity.attach_canceler(survivor.clone());

        assert!(cancel_db_activity(activity.id(), Duration::from_secs(60)));
        assert!(active_db_activity_snapshots().is_empty());
        wait_for("the surviving session to be broken", || {
            survivor.interrupted.load(Ordering::Acquire)
        });
        clear_tracked_db_activity();
    }

    #[test]
    fn the_force_tier_gives_the_whole_batch_one_grace_period_not_one_each() {
        let _test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        // One activity can hold several sessions (parallel metadata jobs). They
        // are all interrupted at the same moment, so the last one must not wait
        // sessions * timeout to be force closed.
        let activity = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);
        let cancelers: Vec<_> = (0..3).map(|_| Arc::new(TestCanceler::default())).collect();
        let registrations: Vec<_> = cancelers
            .iter()
            .map(|canceler| activity.attach_canceler(canceler.clone()))
            .collect();

        let started = Instant::now();
        cancel_db_activity(activity.id(), Duration::from_secs(1));

        // Hold the guard so the graceful tier never "lands" and every session
        // has to be escalated.
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline
            && !cancelers
                .iter()
                .all(|canceler| canceler.forced.load(Ordering::Acquire))
        {
            std::thread::sleep(Duration::from_millis(20));
        }

        assert!(
            cancelers
                .iter()
                .all(|canceler| canceler.forced.load(Ordering::Acquire)),
            "every session in the batch must reach the force tier"
        );
        assert!(
            started.elapsed() < Duration::from_millis(2500),
            "the batch took {:?}, which means the timeout restarted per session",
            started.elapsed()
        );
        drop(registrations);
        drop(activity);
        clear_tracked_db_activity();
    }

    #[test]
    fn a_cancel_hook_that_panics_does_not_stop_the_cancel() {
        let _test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let canceler = Arc::new(TestCanceler::default());
        let _registration = activity.attach_canceler(canceler.clone());
        activity.on_cancel(Arc::new(|| panic!("owner callback exploded")));

        assert!(cancel_db_activity(activity.id(), Duration::from_secs(60)));
        wait_for(
            "the session to be broken despite the panicking callback",
            || canceler.interrupted.load(Ordering::Acquire),
        );
        clear_tracked_db_activity();
    }

    /// A canceler whose destructor reads the registry back.
    ///
    /// The whole design rests on the activity registry being a LEAF lock:
    /// nothing caller-supplied may run while it is held. Every path that drops
    /// an entry drops caller-owned values (hooks, cancelers), so if any of them
    /// still did that under the lock, this type turns it into a hang.
    struct ReentrantDropCanceler {
        dropped: Arc<AtomicBool>,
    }

    impl DbActivityCanceler for ReentrantDropCanceler {
        fn interrupt(&self) -> Result<(), String> {
            Ok(())
        }

        fn force(&self) -> Result<(), String> {
            Ok(())
        }

        fn label(&self) -> &'static str {
            "reentrant-drop"
        }
    }

    impl Drop for ReentrantDropCanceler {
        fn drop(&mut self) {
            let _ = active_db_activity_snapshots();
            let _ = current_db_activity();
            self.dropped.store(true, Ordering::Release);
        }
    }

    fn reentrant_canceler() -> (Arc<dyn DbActivityCanceler>, Arc<AtomicBool>) {
        let dropped = Arc::new(AtomicBool::new(false));
        (
            Arc::new(ReentrantDropCanceler {
                dropped: dropped.clone(),
            }),
            dropped,
        )
    }

    #[test]
    fn the_activity_registry_is_a_leaf_lock_on_every_path_that_drops_an_entry() {
        let _test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        // 1. releasing one session
        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let (canceler, released) = reentrant_canceler();
        drop(activity.attach_canceler(canceler));
        assert!(released.load(Ordering::Acquire));

        // 2. the activity finishing normally
        let (canceler, finished) = reentrant_canceler();
        let registration = activity.attach_canceler(canceler);
        drop(activity);
        drop(registration);
        assert!(finished.load(Ordering::Acquire));

        // 3. a cancel retiring the entry
        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let (canceler, cancelled) = reentrant_canceler();
        let registration = activity.attach_canceler(canceler);
        cancel_db_activity(activity.id(), Duration::from_secs(60));
        drop(registration);
        drop(activity);
        wait_for("the cancelled session's canceler to drop", || {
            cancelled.load(Ordering::Acquire)
        });

        // 4. replacing a cancel hook, whose closure is caller code too
        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let (canceler, hook_dropped) = reentrant_canceler();
        let hook_canceler = canceler.clone();
        activity.on_cancel(Arc::new(move || {
            let _ = hook_canceler.label();
        }));
        drop(canceler);
        activity.on_cancel(Arc::new(|| {}));
        assert!(hook_dropped.load(Ordering::Acquire));

        // 5. clearing the whole registry
        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let (canceler, wiped) = reentrant_canceler();
        std::mem::forget(activity.attach_canceler(canceler));
        clear_tracked_db_activity();
        assert!(wiped.load(Ordering::Acquire));

        clear_tracked_db_activity();
    }

    #[test]
    fn releasing_a_registration_while_cancelling_does_not_deadlock() {
        let _test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        // Both `remove_db_activity` and a registration's own drop take the
        // registry lock, and both drop caller-supplied values. Dropping a guard
        // and its registrations together must not re-enter the lock.
        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let canceler = Arc::new(TestCanceler::default());
        let registration = activity.attach_canceler(canceler);
        drop(activity);
        drop(registration);

        assert!(active_db_activity_snapshots().is_empty());
        clear_tracked_db_activity();
    }

    #[test]
    fn a_cancel_hook_may_re_enter_the_registry_without_deadlocking() {
        let _test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        // Hooks run after the registry lock is released, so an owner that
        // reacts by touching the registry — reading it, or cancelling something
        // else — must not deadlock. `std::sync::Mutex` is not reentrant, so this
        // would hang rather than fail if the hook ran under the lock.
        let other = track_pool_db_activity("Loading metadata", DatabaseType::Oracle);
        let other_id = other.id();
        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let observed = Arc::new(AtomicBool::new(false));
        let observed_by_hook = observed.clone();
        activity.on_cancel(Arc::new(move || {
            let _ = active_db_activity_snapshots();
            cancel_db_activity(other_id, Duration::from_secs(60));
            observed_by_hook.store(true, Ordering::Release);
        }));

        cancel_db_activity(activity.id(), Duration::from_secs(60));

        assert!(observed.load(Ordering::Acquire));
        clear_tracked_db_activity();
    }

    #[test]
    fn a_cancel_timeout_that_cannot_be_added_to_now_does_not_panic() {
        // `Instant + Duration` panics on overflow and this is a public entry
        // point, so an absurd timeout must degrade rather than abort.
        assert!(!wait_for_graceful_cancel(Duration::MAX, || false));
    }

    #[test]
    fn a_registry_cancel_tells_the_owner_so_it_can_report_a_cancel() {
        let _test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let reported = Arc::new(AtomicBool::new(false));
        let reported_by_hook = reported.clone();
        activity.on_cancel(Arc::new(move || {
            reported_by_hook.store(true, Ordering::Release);
        }));

        cancel_db_activity(activity.id(), Duration::from_secs(60));

        assert!(
            reported.load(Ordering::Acquire),
            "without this the query surfaces the broken-session error instead of Cancelled"
        );
        clear_tracked_db_activity();
    }

    #[test]
    fn an_operation_that_ends_releases_the_sessions_it_was_holding() {
        let _test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        let activity = track_pool_db_activity("Executing query", DatabaseType::Oracle);
        let canceler = Arc::new(TestCanceler::default());
        {
            let holder = TestRegistrationHolder::default();
            holder.hold_session_registration(activity.attach_canceler(canceler.clone()));
            assert!(active_db_activity_snapshots()[0].cancelable);
        }

        // The operation finished, so its sessions went back to the pool and must
        // no longer be reachable by a cancel.
        assert!(!active_db_activity_snapshots()[0].cancelable);
        cancel_db_activity(activity.id(), Duration::from_secs(60));
        assert!(!canceler.interrupted.load(Ordering::Acquire));
        clear_tracked_db_activity();
    }

    #[test]
    fn retained_session_lease_conflict_keeps_existing_when_neither_requires_physical_preservation()
    {
        let existing = RetainedSessionState::default();
        let incoming = RetainedSessionState::default();

        assert_eq!(
            retained_lease_conflict_resolution(existing, incoming),
            RetainedLeaseConflictResolution::KeepExisting
        );
    }

    #[test]
    fn retained_session_lease_conflict_replaces_clean_existing_with_preserved_incoming() {
        let existing = RetainedSessionState::default();
        let incoming =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);

        assert_eq!(
            retained_lease_conflict_resolution(existing, incoming),
            RetainedLeaseConflictResolution::ReplaceExisting
        );
    }

    #[test]
    fn retained_session_lease_conflict_keeps_preserved_existing_over_clean_incoming() {
        let existing =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);
        let incoming = RetainedSessionState::default();

        assert_eq!(
            retained_lease_conflict_resolution(existing, incoming),
            RetainedLeaseConflictResolution::KeepExisting
        );
    }

    #[test]
    fn retained_session_lease_conflict_invalidates_when_both_sessions_need_preservation() {
        let existing =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);
        let incoming =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);

        assert_eq!(
            retained_lease_conflict_resolution(existing, incoming),
            RetainedLeaseConflictResolution::KeepExistingMarkedInvalid
        );
    }

    #[test]
    fn retained_lease_context_mismatch_blocks_preserved_sessions() {
        let dirty =
            RetainedSessionState::from_transaction_state(TransactionSessionState::MaybeDirty);

        assert_eq!(
            retained_lease_context_decision(false, dirty),
            RetainedLeaseContextDecision::BlockContextMismatch
        );
        assert_eq!(
            retained_lease_context_decision(true, dirty),
            RetainedLeaseContextDecision::Reusable
        );
    }

    #[test]
    fn retained_lease_context_mismatch_allows_clean_sessions_only() {
        assert_eq!(
            retained_lease_context_decision(false, RetainedSessionState::default()),
            RetainedLeaseContextDecision::Reusable
        );

        let post_processor =
            crate::db::statement_session_post_processor_for(crate::db::DatabaseType::MySQL);
        let transaction_mode_override = crate::db::retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
            false,
            false,
            false,
            false,
        );
        assert!(transaction_mode_override.requires_physical_session_preservation());
        assert_eq!(
            retained_lease_context_decision(false, transaction_mode_override),
            RetainedLeaseContextDecision::BlockContextMismatch
        );
    }

    #[test]
    fn transaction_option_guard_allows_only_clean_sessions() {
        assert!(
            DatabaseConnection::ensure_transaction_option_change_allowed(
                TransactionSessionState::Clean,
                "auto-commit",
            )
            .is_ok()
        );

        for state in [
            TransactionSessionState::MaybeDirty,
            TransactionSessionState::BlockedDirty,
            TransactionSessionState::DecisionRequired,
            TransactionSessionState::InvalidSession,
        ] {
            let err =
                DatabaseConnection::ensure_transaction_option_change_allowed(state, "auto-commit")
                    .expect_err("non-clean transaction state should block option changes");
            assert!(err.contains("auto-commit"));
            assert!(err.contains(state.label()));
        }
    }

    /// session.md §27.6: the global auto-commit toggle must consult retained
    /// session state across all editor tabs, not just the global UI flag. A
    /// dirty retained transaction or a clean retained transaction that still
    /// holds a session-level lock must reject the option change so the user
    /// is forced to commit, rollback, or discard it first.
    #[test]
    fn retained_session_option_change_guard_rejects_dirty_or_locked_sessions() {
        use crate::db::{SessionLockState, SessionResidueState};

        let clean = RetainedSessionState::default();
        assert!(
            DatabaseConnection::ensure_retained_session_option_change_allowed(clean, "auto-commit")
                .is_ok(),
            "Clean retained session must allow auto-commit toggle",
        );

        // A `Clean + typed session residue` editor (e.g. leftover SET @var = ...)
        // is OK because typed residue is not transaction-bound; the autocommit
        // toggle can still proceed.
        let clean_with_residue = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            SessionResidueState::user_variable_for_test(),
            SessionLockState::default(),
        );
        assert!(
            DatabaseConnection::ensure_retained_session_option_change_allowed(
                clean_with_residue,
                "auto-commit",
            )
            .is_ok(),
            "Clean + residue must still allow auto-commit toggle",
        );

        // Unknown residue may include transaction-option side effects from a
        // routine or unsupported SET form, so it must block option changes.
        let clean_with_unknown_residue = RetainedSessionState::from_parts(
            TransactionSessionState::Clean,
            SessionResidueState::new(true),
            SessionLockState::default(),
        );
        let err = DatabaseConnection::ensure_retained_session_option_change_allowed(
            clean_with_unknown_residue,
            "auto-commit",
        )
        .expect_err("unknown retained session state must block auto-commit toggle");
        assert!(err.contains("auto-commit"));
        assert!(err.contains(clean_with_unknown_residue.label()));

        // A retained dirty transaction MUST block the toggle no matter how
        // bare the rest of the state is.
        for transaction_state in [
            TransactionSessionState::MaybeDirty,
            TransactionSessionState::BlockedDirty,
            TransactionSessionState::DecisionRequired,
            TransactionSessionState::InvalidSession,
        ] {
            let state = RetainedSessionState::from_transaction_state(transaction_state);
            let err = DatabaseConnection::ensure_retained_session_option_change_allowed(
                state,
                "auto-commit",
            )
            .expect_err("dirty retained transaction must block auto-commit toggle");
            assert!(err.contains("auto-commit"));
            assert!(err.contains(state.label()));
        }

        // Even a Clean transaction must block the toggle when a session lock
        // is still held — the lock would otherwise outlive the editor.
        for &(table_lock, named_lock) in &[(true, false), (false, true), (true, true)] {
            let state = RetainedSessionState::from_parts(
                TransactionSessionState::Clean,
                SessionResidueState::default(),
                SessionLockState::new(table_lock, named_lock),
            );
            let err = DatabaseConnection::ensure_retained_session_option_change_allowed(
                state,
                "auto-commit",
            )
            .expect_err("session lock must block auto-commit toggle");
            assert!(err.contains("auto-commit"));
            assert!(err.contains(state.label()));
        }

        let post_processor =
            crate::db::statement_session_post_processor_for(crate::db::DatabaseType::MySQL);
        let pending_transaction_mode = crate::db::retained_session_state_after_statement(
            post_processor,
            RetainedSessionState::default(),
            post_processor.effects_for_sql("SET TRANSACTION READ ONLY"),
            false,
            false,
            false,
            false,
        );
        let err = DatabaseConnection::ensure_retained_session_option_change_allowed(
            pending_transaction_mode,
            "transaction mode",
        )
        .expect_err("pending transaction-mode override must block transaction option changes");
        assert!(err.contains("transaction mode"));
        assert!(err.contains(pending_transaction_mode.label()));
    }

    #[test]
    fn mysql_transaction_probe_uses_session_in_transaction_flag() {
        let sql = DatabaseConnection::mysql_session_transaction_probe_sql();
        assert!(sql.contains("@@in_transaction"));
    }

    #[test]
    fn mysql_transaction_probe_keeps_innodb_metadata_fallback() {
        let sql = DatabaseConnection::mysql_innodb_transaction_probe_sql();
        assert!(sql.contains("COUNT(*)"));
        assert!(sql.contains("trx_mysql_thread_id = CONNECTION_ID()"));
        // The work filter is required: under autocommit=0 the probe itself
        // registers an implicit read-only transaction in innodb_trx, and an
        // unfiltered count would block the auto-commit toggle forever
        // (verified live on MySQL 8.0 / MariaDB).
        assert!(sql.contains("trx_rows_modified > 0"));
        assert!(sql.contains("trx_rows_locked > 0"));
    }

    #[test]
    fn mysql_transaction_probe_order_matches_server_dialect() {
        // Each dialect's accurate probe must come first; the stale-prone
        // innodb_trx probe is strictly the last resort (live-verified: a
        // self-probe of innodb_trx inside a transaction leaves a stale
        // RUNNING entry on MySQL 8.0 that outlives ROLLBACK).
        let mariadb = DatabaseConnection::mysql_transaction_probe_sql_order(DatabaseType::MariaDB);
        assert!(mariadb[0].contains("@@in_transaction"));
        assert!(mariadb.last().unwrap().contains("innodb_trx"));

        let mysql = DatabaseConnection::mysql_transaction_probe_sql_order(DatabaseType::MySQL);
        assert!(mysql[0].contains("performance_schema.events_transactions_current"));
        assert!(mysql[0].contains("STATE = 'ACTIVE'"));
        assert!(mysql[1].contains("@@in_transaction"));
        assert!(mysql.last().unwrap().contains("innodb_trx"));
    }

    // A query tab browses its own database/schema (the object browser's scope
    // selection is tab-local), so every tab-initiated operation — quick
    // describe, explain plan — must resolve names there and not in whatever
    // the connection was opened with.
    #[test]
    fn mysql_operation_database_prefers_the_tabs_scope() {
        let mut connection = DatabaseConnection::new();
        connection.info.service_name = "connection_db".to_string();

        assert_eq!(
            connection.mysql_database_for_scope(Some("tab_db")),
            "tab_db"
        );
        assert_eq!(connection.mysql_database_for_scope(None), "connection_db");
        assert_eq!(
            connection.mysql_database_for_scope(Some("   ")),
            "connection_db"
        );
    }

    #[test]
    fn oracle_operation_schema_prefers_the_tabs_scope() {
        let mut connection = DatabaseConnection::new();
        connection.set_tracked_oracle_current_schema(Some("CONNECTION_SCHEMA".to_string()));

        assert_eq!(
            connection.oracle_session_schema_for_scope(Some("TAB_SCHEMA")),
            Some("TAB_SCHEMA".to_string())
        );
        assert_eq!(
            connection.oracle_session_schema_for_scope(None),
            Some("CONNECTION_SCHEMA".to_string())
        );
        assert_eq!(
            connection.oracle_session_schema_for_scope(Some("   ")),
            Some("CONNECTION_SCHEMA".to_string())
        );

        connection.clear_tracked_oracle_current_schema();
        assert_eq!(connection.oracle_session_schema_for_scope(None), None);
    }

    #[test]
    fn mysql_empty_scope_preserved_session_error_requires_user_resolution() {
        let message = DatabaseConnection::mysql_empty_scope_requires_resolved_session_error();

        assert!(message.contains("Cannot clear the MySQL/MariaDB database scope"));
        assert!(message.contains("retained session has transaction or session state"));
        assert!(message.contains("Resolve or discard"));
    }

    fn mysql_pool_session_context_for_cache_test(
        cache_epoch: u64,
        cache_epoch_token: Arc<AtomicU64>,
    ) -> DbPoolSessionContext {
        let connection_info = ConnectionInfo::new_with_type(
            "cache-test",
            "root",
            "secret",
            "127.0.0.1",
            3306,
            "cache_test",
            DatabaseType::MySQL,
        );
        let pool = DatabaseConnection::build_mysql_pool(
            &connection_info,
            MIN_CONNECTION_POOL_SIZE,
            ConnectionAttemptPolicy::default(),
        )
        .expect("create test MySQL pool without opening a connection");
        DbPoolSessionContext {
            connection_generation: 1,
            connection_id: None,
            pool: DbConnectionPool::MySQL {
                pool,
                advanced: connection_info.advanced.clone(),
                db_type: connection_info.db_type,
            },
            connection_pool_size: MIN_CONNECTION_POOL_SIZE,
            current_service_name: connection_info.service_name.clone(),
            oracle_current_schema: None,
            auto_commit: true,
            transaction_mode: TransactionMode::default(),
            default_transaction_isolation: TransactionIsolation::RepeatableRead,
            connection_info,
            cache_epoch,
            cache_epoch_token,
        }
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_empty_current_scope_resets_reused_pool_session_database() {
        let _guard = db_activity_test_lock();
        let info = mysql_test_connection_info_from_env();
        if info.service_name.trim().is_empty() {
            eprintln!("skipping: SPACE_QUERY_TEST_MYSQL_DATABASE must be non-empty");
            return;
        }

        let pool = DatabaseConnection::build_mysql_pool(
            &info,
            MIN_CONNECTION_POOL_SIZE,
            ConnectionAttemptPolicy::default(),
        )
        .expect("create MySQL pool");
        let db_pool = DbConnectionPool::MySQL {
            pool,
            advanced: info.advanced.clone(),
            db_type: info.db_type,
        };
        let pool_activity = track_pool_db_activity("MySQL pool session test", info.db_type);
        let (mut session, _cancel_registration) = db_pool
            .acquire_session(&info, &pool_activity)
            .expect("acquire MySQL pool session");
        let DbPoolSession::MySQL { conn, .. } = &mut session else {
            panic!("expected MySQL pool session");
        };
        conn.as_mut()
            .select_db(info.service_name.as_str())
            .expect("select test database before empty-scope reset");
        let selected = conn
            .query_first::<Option<String>, _>("SELECT DATABASE()")
            .expect("read selected database")
            .flatten();
        assert_eq!(selected.as_deref(), Some(info.service_name.as_str()));

        let mut empty_info = info.clone();
        empty_info.service_name.clear();
        let context = DbPoolSessionContext {
            connection_generation: 1,
            connection_id: None,
            connection_info: empty_info,
            pool: db_pool,
            connection_pool_size: MIN_CONNECTION_POOL_SIZE,
            current_service_name: String::new(),
            oracle_current_schema: None,
            auto_commit: true,
            transaction_mode: TransactionMode::default(),
            default_transaction_isolation: TransactionIsolation::RepeatableRead,
            cache_epoch: 0,
            cache_epoch_token: Arc::new(AtomicU64::new(0)),
        };
        backend_for(DatabaseType::MySQL)
            .apply_current_scope_to_session(&context, &mut session)
            .expect("empty MySQL current scope should reset stale database state");

        let DbPoolSession::MySQL { conn, .. } = &mut session else {
            panic!("expected MySQL pool session");
        };
        let current_database = conn
            .query_first::<Option<String>, _>("SELECT DATABASE()")
            .expect("read database after empty-scope reset")
            .flatten();
        assert_eq!(current_database, None);
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_empty_primary_scope_resets_current_database() {
        let _guard = db_activity_test_lock();
        let info = mysql_test_connection_info_from_env();
        if info.service_name.trim().is_empty() {
            eprintln!("skipping: SPACE_QUERY_TEST_MYSQL_DATABASE must be non-empty");
            return;
        }

        let mut connection = DatabaseConnection::new();
        connection.connect(info.clone()).expect("connect to MySQL");
        assert_eq!(connection.get_info().service_name, info.service_name);

        connection
            .switch_mysql_database("")
            .expect("empty MySQL primary scope should reset current database");

        assert_eq!(connection.get_info().service_name, "");
        let current_database = connection
            .get_mysql_connection_mut()
            .expect("MySQL connection")
            .query_first::<Option<String>, _>("SELECT DATABASE()")
            .expect("read database after empty primary-scope reset")
            .flatten();
        assert_eq!(current_database, None);
    }

    fn read_oracle_session_parameter(conn: &Connection, parameter: &str) -> String {
        let mut stmt = conn
            .statement("SELECT value FROM nls_session_parameters WHERE parameter = :1")
            .build()
            .expect("build Oracle session parameter query");
        let row = stmt
            .query_row(&[&parameter])
            .expect("read Oracle session parameter");
        row.get::<_, String>(0)
            .expect("Oracle session parameter value")
    }

    fn read_oracle_session_time_zone(conn: &Connection) -> String {
        let mut stmt = conn
            .statement("SELECT SESSIONTIMEZONE FROM dual")
            .build()
            .expect("build Oracle session time zone query");
        let row = stmt.query_row(&[]).expect("read Oracle session time zone");
        row.get::<_, String>(0).expect("Oracle session time zone")
    }

    #[test]
    fn require_live_connection_returns_default_message_when_never_connected() {
        let mut conn = DatabaseConnection::new();
        let err = conn
            .require_live_connection()
            .expect_err("must be disconnected");
        assert_eq!(err, NOT_CONNECTED_MESSAGE);
    }

    #[test]
    fn pool_context_cache_rejects_epoch_invalidated_context() {
        let _activity_test_guard = db_activity_test_lock();
        lock_pool_context_cache().clear();

        let connection = Arc::new(Mutex::new(DatabaseConnection::new()));
        let key = shared_connection_cache_key(&connection);
        let epoch_token = Arc::new(AtomicU64::new(7));
        let context = mysql_pool_session_context_for_cache_test(7, Arc::clone(&epoch_token));

        cache_pool_session_context_for_shared_connection(&connection, &context);
        assert!(cached_pool_session_context(key).is_some());

        epoch_token.fetch_add(1, Ordering::AcqRel);
        assert!(cached_pool_session_context(key).is_none());
    }

    #[test]
    fn stale_pool_session_context_rejects_acquire_before_touching_pool() {
        let epoch_token = Arc::new(AtomicU64::new(8));
        let context = mysql_pool_session_context_for_cache_test(7, epoch_token);

        let activity = track_pool_db_activity("stale acquire test", DatabaseType::MySQL);
        let err = match context.acquire_session_for_current_scope(&activity) {
            Ok(_) => panic!("stale context must not acquire a pooled session"),
            Err(err) => err,
        };

        assert_eq!(err, STALE_POOL_CONTEXT_MESSAGE);
    }

    #[test]
    fn pool_context_cache_rejects_dropped_shared_connection_owner() {
        let _activity_test_guard = db_activity_test_lock();
        lock_pool_context_cache().clear();

        let key = {
            let connection = Arc::new(Mutex::new(DatabaseConnection::new()));
            let key = shared_connection_cache_key(&connection);
            let epoch_token = Arc::new(AtomicU64::new(0));
            let context = mysql_pool_session_context_for_cache_test(0, epoch_token);

            cache_pool_session_context_for_shared_connection(&connection, &context);
            assert!(cached_pool_session_context(key).is_some());
            key
        };

        assert!(cached_pool_session_context(key).is_none());
    }

    #[test]
    fn pool_context_identity_includes_auto_commit() {
        let _activity_test_guard = db_activity_test_lock();
        lock_pool_context_cache().clear();

        let connection = Arc::new(Mutex::new(DatabaseConnection::new()));
        let epoch_token = Arc::new(AtomicU64::new(0));
        let context = mysql_pool_session_context_for_cache_test(0, epoch_token);
        let mut changed = context.clone();
        changed.auto_commit = !context.auto_commit;

        cache_pool_session_context_for_shared_connection(&connection, &context);

        assert!(!cached_pool_session_context_matches_shared_connection(
            &connection,
            &changed,
        ));
    }

    #[test]
    fn set_auto_commit_invalidates_pool_context_epoch() {
        let mut connection = DatabaseConnection::new();
        let initial_epoch = connection.current_pool_context_epoch();

        connection
            .set_auto_commit(true)
            .expect("auto-commit toggle should update disconnected preference");

        assert_ne!(connection.current_pool_context_epoch(), initial_epoch);
    }

    #[test]
    fn missing_live_connection_handle_invalidates_pool_context_cache() {
        let _activity_test_guard = db_activity_test_lock();
        lock_pool_context_cache().clear();

        let mut connection = DatabaseConnection::new();
        connection.connected = true;
        let context = mysql_pool_session_context_for_cache_test(
            connection.current_pool_context_epoch(),
            Arc::clone(&connection.pool_context_epoch),
        );
        let shared_connection = Arc::new(Mutex::new(connection));
        let key = shared_connection_cache_key(&shared_connection);

        cache_pool_session_context_for_shared_connection(&shared_connection, &context);
        assert!(cached_pool_session_context(key).is_some());

        let err = match shared_connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .require_live_db_connection()
        {
            Ok(_) => panic!("missing connection handle should be reported as disconnected"),
            Err(err) => err,
        };

        assert_eq!(err, NOT_CONNECTED_MESSAGE);
        assert!(cached_pool_session_context(key).is_none());
    }

    #[test]
    fn db_activity_tracking_keeps_pool_activity_out_of_busy_message() {
        let _activity_test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        let pool_activity = track_pool_db_activity("Loading object metadata", DatabaseType::Oracle);
        let second_pool_activity =
            track_pool_db_activity("Generating object DDL", DatabaseType::MySQL);

        let snapshots = active_pool_db_activity_snapshots();
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].activity, "Loading object metadata");
        assert_eq!(snapshots[0].db_type, Some(DatabaseType::Oracle));
        assert_eq!(snapshots[1].activity, "Generating object DDL");
        assert_eq!(snapshots[1].db_type, Some(DatabaseType::MySQL));

        let combined_activity = current_db_activity().expect("activity should be tracked");
        assert!(combined_activity.contains("Loading object metadata"));
        assert!(combined_activity.contains("Generating object DDL"));
        assert_eq!(
            format_connection_busy_message(),
            "Connection is busy. Try again after the current operation finishes."
        );

        let connection_activity = track_db_activity_entry(
            "Switching schema".to_string(),
            None,
            None,
            DbActivityKind::ConnectionLock,
        );
        assert_eq!(
            format_connection_busy_message(),
            "Connection is busy. Current DB activity: Switching schema"
        );

        drop(connection_activity);
        assert_eq!(active_pool_db_activity_snapshots().len(), 2);
        drop(pool_activity);
        assert_eq!(active_pool_db_activity_snapshots().len(), 1);
        drop(second_pool_activity);
        assert!(active_pool_db_activity_snapshots().is_empty());
        assert_eq!(current_db_activity(), None);
        clear_tracked_db_activity();
    }

    #[test]
    fn db_activity_tracking_preserves_connection_identity() {
        let _activity_test_guard = db_activity_test_lock();
        clear_tracked_db_activity();
        let registry = crate::db::ConnectionRegistry::new();
        let runtime = registry.register_unmanaged(create_shared_connection());
        let activity = track_db_activity_for_connection(
            "Executing on one runtime",
            Some(DatabaseType::Oracle),
            runtime.id(),
        );

        let snapshots = active_db_activity_snapshots();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].connection_id, Some(runtime.id()));

        drop(activity);
        clear_tracked_db_activity();
    }

    #[test]
    fn db_activity_guard_updates_summary_and_exact_progress() {
        let _activity_test_guard = db_activity_test_lock();
        clear_tracked_db_activity();

        let activity = track_db_activity("Executing script", Some(DatabaseType::Oracle));
        let handed_off_activity = activity.clone();
        activity.set_activity("Fetching rows: 25 | Executing script");
        activity.set_progress(DbActivityProgress::Determinate {
            completed: 1,
            total: 4,
        });

        let snapshots = active_db_activity_snapshots();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(
            snapshots[0].activity,
            "Fetching rows: 25 | Executing script"
        );
        assert_eq!(snapshots[0].progress.percentage(), Some(25));
        assert_eq!(
            DbActivityProgress::Determinate {
                completed: 9,
                total: 4,
            }
            .percentage(),
            Some(100)
        );
        assert_eq!(DbActivityProgress::Indeterminate.percentage(), None);

        drop(activity);
        assert_eq!(active_db_activity_snapshots().len(), 1);
        drop(handed_off_activity);
        assert!(active_db_activity_snapshots().is_empty());

        let stuck_activity = track_db_activity("Stuck operation", Some(DatabaseType::Oracle));
        stuck_activity.finish_handle().finish();
        assert!(active_db_activity_snapshots().is_empty());
        drop(stuck_activity);
        clear_tracked_db_activity();
    }

    #[test]
    fn db_activity_guard_converts_summary_before_locking_registry() {
        let _activity_test_guard = db_activity_test_lock();
        clear_tracked_db_activity();
        let converted_without_registry_lock = std::sync::atomic::AtomicBool::new(false);
        let activity = track_db_activity("Initial activity", Some(DatabaseType::Oracle));

        activity.set_activity(RegistryLockProbe {
            converted_without_registry_lock: &converted_without_registry_lock,
        });

        assert!(converted_without_registry_lock.load(Ordering::Relaxed));
        assert_eq!(
            active_db_activity_snapshots()[0].activity,
            "Updated activity"
        );
        drop(activity);
        clear_tracked_db_activity();
    }

    #[test]
    fn disconnect_resets_connection_metadata_auto_commit_and_transaction_mode() {
        let mut conn = DatabaseConnection::new();
        conn.info = ConnectionInfo::new("Prod", "scott", "pw", "db", 1521, "FREE");
        conn.connected = true;
        conn.auto_commit = true;
        conn.transaction_mode = TransactionMode::new(
            TransactionIsolation::Serializable,
            TransactionAccessMode::ReadOnly,
        );
        conn.disconnect();

        assert!(!conn.connected);
        assert!(!conn.auto_commit);
        assert_eq!(conn.transaction_mode(), TransactionMode::default());
        assert!(conn.info.name.is_empty());
        assert!(conn.info.username.is_empty());
        assert_eq!(conn.info.host, "localhost");
    }

    #[test]
    fn connected_metadata_retains_password_until_disconnect() {
        let mut conn = DatabaseConnection::new();
        conn.simulate_connected_metadata_for_test(ConnectionInfo::new(
            "Prod", "scott", "pw", "db", 1521, "FREE",
        ));

        assert_eq!(conn.get_info().password, "pw");

        conn.disconnect();

        assert!(conn.get_info().password.is_empty());
        assert!(conn.session_password.is_empty());
    }

    #[test]
    fn connection_pool_size_defaults_and_clamps() {
        let mut conn = DatabaseConnection::new();

        assert_eq!(conn.connection_pool_size(), DEFAULT_CONNECTION_POOL_SIZE);

        conn.set_connection_pool_size(0);
        assert_eq!(conn.connection_pool_size(), MIN_CONNECTION_POOL_SIZE);

        conn.set_connection_pool_size(99);
        assert_eq!(conn.connection_pool_size(), MAX_CONNECTION_POOL_SIZE);
    }

    #[test]
    fn resize_disconnected_connection_pool_size_clamps_preference() {
        let mut conn = DatabaseConnection::new();

        conn.resize_current_connection_pool(0)
            .expect("disconnected resize should not require a live pool");
        assert_eq!(conn.connection_pool_size(), MIN_CONNECTION_POOL_SIZE);

        conn.resize_current_connection_pool(99)
            .expect("disconnected resize should not require a live pool");
        assert_eq!(conn.connection_pool_size(), MAX_CONNECTION_POOL_SIZE);
    }

    #[test]
    fn disconnect_resets_session_state() {
        let mut conn = DatabaseConnection::new();
        conn.connected = true;
        conn.info.db_type = DatabaseType::MySQL;
        if let Ok(mut session) = conn.session.lock() {
            session.db_type = DatabaseType::MySQL;
            session.continue_on_error = true;
            session.colsep = ",".to_string();
        }

        conn.disconnect();

        let (db_type, continue_on_error, colsep) = match conn.session.lock() {
            Ok(guard) => (guard.db_type, guard.continue_on_error, guard.colsep.clone()),
            Err(poisoned) => {
                let guard = poisoned.into_inner();
                (guard.db_type, guard.continue_on_error, guard.colsep.clone())
            }
        };
        assert_eq!(db_type, DatabaseType::default());
        assert!(!continue_on_error);
        assert_eq!(colsep, " | ");
    }

    #[test]
    fn mysql_connection_string_omits_database_segment_when_empty() {
        let info = ConnectionInfo::new_with_type(
            "local",
            "root",
            "pw",
            "localhost",
            3306,
            "",
            DatabaseType::MySQL,
        );

        assert_eq!(info.connection_string(), "mysql://localhost:3306");
    }

    #[test]
    fn mysql_interactive_connection_opts_keep_requested_database() {
        let info = ConnectionInfo::new_with_type(
            "local",
            "root",
            "pw",
            "localhost",
            3306,
            "initial_db",
            DatabaseType::MySQL,
        );
        let opts = mysql::Opts::from(DatabaseConnection::build_mysql_opts(
            &info,
            ConnectionAttemptPolicy::default(),
        ));

        assert_eq!(opts.get_db_name(), Some("initial_db"));
    }

    #[test]
    fn mysql_family_connection_opts_use_common_transport_timeout_only() {
        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            let info = ConnectionInfo::new_with_type(
                "local",
                "root",
                "pw",
                "localhost",
                3306,
                "initial_db",
                db_type,
            );
            let policy = ConnectionAttemptPolicy::from_seconds(7);
            let opts = mysql::Opts::from(DatabaseConnection::build_mysql_opts(&info, policy));
            let pool_opts =
                mysql::Opts::from(DatabaseConnection::build_mysql_pool_opts(&info, 4, policy));

            assert_eq!(opts.get_tcp_connect_timeout(), Some(Duration::from_secs(7)));
            assert_eq!(
                pool_opts.get_tcp_connect_timeout(),
                Some(Duration::from_secs(7))
            );
            assert_eq!(opts.get_read_timeout(), None);
            assert_eq!(opts.get_write_timeout(), None);
            assert_eq!(pool_opts.get_read_timeout(), None);
            assert_eq!(pool_opts.get_write_timeout(), None);
        }
    }

    #[test]
    fn mysql_pool_opts_do_not_pin_initial_database() {
        let info = ConnectionInfo::new_with_type(
            "local",
            "root",
            "pw",
            "localhost",
            3306,
            "initial_db",
            DatabaseType::MySQL,
        );
        let opts = mysql::Opts::from(DatabaseConnection::build_mysql_pool_opts(
            &info,
            4,
            ConnectionAttemptPolicy::default(),
        ));

        assert_eq!(opts.get_db_name(), None);
    }

    #[test]
    fn oracle_connection_string_uses_tns_alias_when_host_is_empty() {
        let info = ConnectionInfo::new_with_type(
            "local",
            "system",
            "pw",
            "",
            0,
            "LOCAL_FREE",
            DatabaseType::Oracle,
        );

        assert_eq!(info.connection_string(), "LOCAL_FREE");
    }

    #[test]
    fn oracle_transaction_mode_generates_first_statement_sql() {
        let mode = TransactionMode::new(
            TransactionIsolation::Serializable,
            TransactionAccessMode::ReadWrite,
        );

        assert_eq!(
            DatabaseConnection::transaction_mode_statements_for(DatabaseType::Oracle, mode)
                .expect("Oracle mode should be supported"),
            vec!["SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"]
        );
        assert!(DatabaseType::Oracle.transaction_mode_requires_first_statement(mode));
    }

    #[test]
    fn oracle_transaction_mode_generates_read_only_sql() {
        let mode = TransactionMode::new(
            TransactionIsolation::Default,
            TransactionAccessMode::ReadOnly,
        );

        assert_eq!(
            DatabaseConnection::transaction_mode_statements_for(DatabaseType::Oracle, mode)
                .expect("Oracle read-only mode should be supported"),
            vec!["SET TRANSACTION READ ONLY"]
        );
        assert!(DatabaseType::Oracle.transaction_mode_requires_first_statement(mode));
    }

    #[test]
    fn oracle_transaction_mode_read_only_isolation_pairs() {
        // A read-only Oracle transaction IS a serializable snapshot, so the
        // Serializable + Read only pair maps to SET TRANSACTION READ ONLY.
        let serializable_read_only = TransactionMode::new(
            TransactionIsolation::Serializable,
            TransactionAccessMode::ReadOnly,
        );
        assert_eq!(
            DatabaseConnection::transaction_mode_statements_for(
                DatabaseType::Oracle,
                serializable_read_only
            )
            .expect("Serializable + Read only is expressible on Oracle"),
            vec!["SET TRANSACTION READ ONLY"]
        );

        // Statement-level Read committed consistency cannot exist inside a
        // read-only transaction; that pair stays refused.
        let read_committed_read_only = TransactionMode::new(
            TransactionIsolation::ReadCommitted,
            TransactionAccessMode::ReadOnly,
        );
        let err = DatabaseConnection::transaction_mode_statements_for(
            DatabaseType::Oracle,
            read_committed_read_only,
        )
        .expect_err("Oracle cannot run a read-committed read-only transaction");
        assert!(err.contains("READ ONLY"));
        assert!(err.contains("isolation"));
    }

    #[test]
    fn oracle_transaction_mode_rejects_unsupported_isolation() {
        let mode = TransactionMode::new(
            TransactionIsolation::RepeatableRead,
            TransactionAccessMode::ReadWrite,
        );

        assert!(
            DatabaseConnection::transaction_mode_statements_for(DatabaseType::Oracle, mode)
                .is_err()
        );
    }

    #[test]
    fn unsupported_transaction_mode_pairs_are_reported_where_they_are_selected() {
        let awkward = TransactionMode::new(
            TransactionIsolation::ReadCommitted,
            TransactionAccessMode::ReadOnly,
        );

        let reason =
            DatabaseConnection::transaction_mode_selection_error(DatabaseType::Oracle, awkward)
                .expect("Oracle cannot run a read-committed read-only transaction");
        assert!(reason.contains("READ ONLY"));
        // The MySQL family expresses the same pair in one statement, so
        // nothing is refused there.
        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            assert_eq!(
                DatabaseConnection::transaction_mode_selection_error(db_type, awkward),
                None
            );
        }
        // Serializable + Read only is expressible everywhere: on Oracle it is
        // exactly what SET TRANSACTION READ ONLY provides.
        for db_type in [
            DatabaseType::Oracle,
            DatabaseType::MySQL,
            DatabaseType::MariaDB,
        ] {
            assert_eq!(
                DatabaseConnection::transaction_mode_selection_error(
                    db_type,
                    TransactionMode::new(
                        TransactionIsolation::Serializable,
                        TransactionAccessMode::ReadOnly,
                    )
                ),
                None
            );
        }
        assert_eq!(
            DatabaseConnection::transaction_mode_selection_error(
                DatabaseType::Oracle,
                TransactionMode::default()
            ),
            None
        );
    }

    #[test]
    fn oracle_returning_a_tab_to_the_default_isolation_resets_the_session() {
        let default_mode = TransactionMode::default();
        let read_only = TransactionMode::new(
            TransactionIsolation::Default,
            TransactionAccessMode::ReadOnly,
        );
        let serializable = TransactionMode::new(
            TransactionIsolation::Serializable,
            TransactionAccessMode::ReadWrite,
        );

        // A tab that never selected anything cannot have adopted a session
        // level change, so it needs no reset — and Oracle's statement list for
        // the default mode stays empty.
        assert_eq!(
            DatabaseConnection::oracle_transaction_mode_statements_for_tab(
                None,
                default_mode,
                TransactionIsolation::ReadCommitted,
            )
            .expect("the default mode is always supported"),
            Vec::<String>::new()
        );

        // A tab that selected the default explicitly may be sitting on a
        // session an ALTER SESSION left elsewhere; put it back.
        assert_eq!(
            DatabaseConnection::oracle_transaction_mode_statements_for_tab(
                Some(default_mode),
                default_mode,
                TransactionIsolation::ReadCommitted,
            )
            .expect("the default mode is always supported"),
            vec!["ALTER SESSION SET ISOLATION_LEVEL = READ COMMITTED"]
        );

        // The reset comes first, so the mode statements apply on top of it.
        assert_eq!(
            DatabaseConnection::oracle_transaction_mode_statements_for_tab(
                Some(read_only),
                read_only,
                TransactionIsolation::ReadCommitted,
            )
            .expect("read-only with the default isolation is supported"),
            vec![
                "ALTER SESSION SET ISOLATION_LEVEL = READ COMMITTED",
                "SET TRANSACTION READ ONLY",
            ]
        );

        // An explicit isolation is issued per transaction and overrides the
        // session anyway, so no reset is needed.
        assert_eq!(
            DatabaseConnection::oracle_transaction_mode_statements_for_tab(
                Some(serializable),
                serializable,
                TransactionIsolation::ReadCommitted,
            )
            .expect("serializable is supported"),
            vec!["SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"]
        );
    }

    #[test]
    fn oracle_transaction_probe_uses_plsql_boolean_context() {
        let sql = DatabaseConnection::oracle_session_transaction_probe_sql();

        assert_eq!(
            sql,
            "BEGIN :transaction_id := DBMS_TRANSACTION.LOCAL_TRANSACTION_ID(FALSE); END;"
        );
        assert!(!sql.to_ascii_uppercase().starts_with("SELECT "));
    }

    #[test]
    fn mysql_transaction_mode_generates_session_sql() {
        let mode = TransactionMode::new(
            TransactionIsolation::ReadCommitted,
            TransactionAccessMode::ReadOnly,
        );

        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            assert_eq!(
                DatabaseConnection::transaction_mode_statements_for(db_type, mode)
                    .expect("MySQL-family mode should be supported"),
                vec!["SET SESSION TRANSACTION ISOLATION LEVEL READ COMMITTED, READ ONLY"]
            );
        }
    }

    #[test]
    fn mysql_default_transaction_mode_resets_access_mode_to_read_write() {
        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            assert_eq!(
                DatabaseConnection::transaction_mode_statements_for(
                    db_type,
                    TransactionMode::default()
                )
                .expect("MySQL-family default mode should be supported"),
                vec!["SET SESSION TRANSACTION READ WRITE"]
            );
        }
    }

    #[test]
    fn mysql_default_transaction_mode_with_known_default_resets_isolation_too() {
        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            assert_eq!(
                DatabaseConnection::transaction_mode_statements_for_with_default(
                    db_type,
                    TransactionMode::default(),
                    TransactionIsolation::RepeatableRead,
                )
                .expect("MySQL-family default mode should reset to known default isolation"),
                vec!["SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ WRITE"]
            );
        }
    }

    #[test]
    fn transaction_isolation_parses_database_reported_values() {
        assert_eq!(
            TransactionIsolation::from_sql_level("READ-COMMITTED"),
            Some(TransactionIsolation::ReadCommitted)
        );
        assert_eq!(
            TransactionIsolation::from_sql_level("read_commited"),
            Some(TransactionIsolation::ReadCommitted)
        );
        assert_eq!(
            TransactionIsolation::from_sql_level("REPEATABLE-READ"),
            Some(TransactionIsolation::RepeatableRead)
        );
    }

    #[test]
    fn database_form_specs_keep_connection_defaults_in_backend_metadata() {
        let oracle = DatabaseType::Oracle.connection_form_spec();
        assert_eq!(oracle.default_port, 1521);
        assert!(oracle.default_service_name.is_empty());
        assert!(oracle.show_driver_mode);
        assert!(oracle.service_name_required);
        assert!(oracle.supports_tns_alias);
        let oracle_advanced = DatabaseType::Oracle.advanced_settings_form_spec();
        assert!(oracle_advanced.show_oracle_protocol);
        assert!(oracle_advanced.show_oracle_nls_formats);
        assert!(!oracle_advanced.show_mysql_session_options);
        assert!(!oracle_advanced.show_mysql_ssl_ca_path);
        assert_eq!(
            DatabaseType::Oracle.table_browse_spec(),
            DbTableBrowseSpec {
                pagination: DbTableBrowsePagination::Rownum,
                strips_page_helper_column: true,
            }
        );

        let mysql = DatabaseType::MySQL.connection_form_spec();
        assert_eq!(mysql.default_port, 3306);
        assert!(!mysql.show_driver_mode);
        assert!(!mysql.service_name_required);
        assert!(!mysql.supports_tns_alias);
        let mysql_advanced = DatabaseType::MySQL.advanced_settings_form_spec();
        assert!(!mysql_advanced.show_oracle_protocol);
        assert!(!mysql_advanced.show_oracle_nls_formats);
        assert!(mysql_advanced.show_mysql_session_options);
        assert!(mysql_advanced.show_mysql_ssl_ca_path);
        assert_eq!(
            DatabaseType::MySQL.table_browse_spec(),
            DbTableBrowseSpec {
                pagination: DbTableBrowsePagination::LimitOffset,
                strips_page_helper_column: false,
            }
        );

        let mariadb = DatabaseType::MariaDB.connection_form_spec();
        assert_eq!(mariadb.default_port, 3306);
        assert!(!mariadb.show_driver_mode);
        assert!(!mariadb.service_name_required);
        assert!(!mariadb.supports_tns_alias);
        let mariadb_advanced = DatabaseType::MariaDB.advanced_settings_form_spec();
        assert_eq!(mariadb_advanced, mysql_advanced);
        assert_eq!(
            DatabaseType::MariaDB.table_browse_spec(),
            DatabaseType::MySQL.table_browse_spec()
        );
    }

    #[test]
    fn connection_info_defaults_follow_default_database_backend() {
        let db_type = DatabaseType::default();
        let form = db_type.connection_form_spec();

        let default_info = ConnectionInfo::default();
        assert_eq!(default_info.db_type, db_type);
        assert_eq!(default_info.host, form.default_host);
        assert_eq!(default_info.port, form.default_port);
        assert_eq!(default_info.service_name, form.default_service_name);
        assert_eq!(
            default_info.advanced,
            ConnectionAdvancedSettings::default_for(db_type)
        );
        assert_eq!(default_info.debug_oracle_thin_protocol_version, None);

        let new_info = ConnectionInfo::new("Prod", "scott", "pw", "db", 1521, "FREE");
        assert_eq!(new_info.db_type, db_type);
        assert_eq!(
            new_info.advanced,
            ConnectionAdvancedSettings::default_for(db_type)
        );
        assert_eq!(new_info.debug_oracle_thin_protocol_version, None);
    }

    #[test]
    fn oracle_thin_debug_protocol_version_pins_connect_options() {
        let mut info = ConnectionInfo::new_with_type(
            "local",
            "system",
            "pw",
            "localhost",
            1521,
            "FREE",
            DatabaseType::Oracle,
        );
        info.advanced.oracle_driver_mode = OracleDriverMode::Thin;
        info.debug_oracle_thin_protocol_version = Some(314);

        let config =
            DatabaseConnection::build_oracle_thin_config(&info, ConnectionAttemptPolicy::default())
                .expect("debug protocol version should build a Thin config");

        assert_eq!(config.connect_options.desired_protocol_version, 314);
        assert_eq!(config.connect_options.minimum_protocol_version, 314);
    }

    #[test]
    fn oracle_thin_config_uses_common_connect_policy_without_retries() {
        let mut info = ConnectionInfo::new_with_type(
            "local",
            "system",
            "pw",
            "localhost",
            1521,
            "FREE",
            DatabaseType::Oracle,
        );
        info.advanced.oracle_driver_mode = OracleDriverMode::Thin;
        let config = DatabaseConnection::build_oracle_thin_config(
            &info,
            ConnectionAttemptPolicy::from_seconds(9),
        )
        .expect("Thin config should build");

        assert_eq!(
            config.connect_options.tcp_connect_timeout,
            Duration::from_secs(9)
        );
        assert_eq!(
            config.connect_options.connect_io_timeout,
            Duration::from_secs(9)
        );
        assert_eq!(config.connect_options.retry_count, 0);
        assert_eq!(config.connect_options.retry_delay, Duration::ZERO);
    }

    #[test]
    fn connection_color_and_read_only_survive_a_save_and_load() {
        // ConnectionInfo deserialises through ConnectionInfoSerde, so a field
        // present on the struct but missing from that mirror would save fine
        // and come back gone. This is the test that catches it.
        let mut info = ConnectionInfo::new_with_type(
            "prod",
            "system",
            "pw",
            "localhost",
            1521,
            "FREE",
            DatabaseType::Oracle,
        );
        info.color = ConnectionColor::Red;
        info.read_only = true;

        let serialized = serde_json::to_string(&info).expect("ConnectionInfo should serialize");
        let restored: ConnectionInfo =
            serde_json::from_str(&serialized).expect("ConnectionInfo should deserialize");

        assert_eq!(restored.color, ConnectionColor::Red);
        assert!(restored.read_only);
    }

    #[test]
    fn connection_color_and_read_only_default_for_connections_saved_before_them() {
        let stored = r#"{"name":"old","username":"u","host":"h","port":1521,
            "service_name":"FREE","db_type":"Oracle"}"#;
        let restored: ConnectionInfo =
            serde_json::from_str(stored).expect("an older saved connection still loads");

        assert_eq!(restored.color, ConnectionColor::None);
        assert!(!restored.read_only);
    }

    #[test]
    fn a_connection_tagged_with_a_retired_colour_still_loads() {
        let stored = r#"{"name":"old","username":"u","host":"h","port":1521,
            "service_name":"FREE","db_type":"Oracle","color":"Blue"}"#;
        let restored: ConnectionInfo =
            serde_json::from_str(stored).expect("a retired tag must not fail the whole connection");

        assert_eq!(restored.color, ConnectionColor::None);
    }

    #[test]
    fn connection_colors_have_distinct_labels_and_only_none_is_unpainted() {
        let mut labels: Vec<&str> = ConnectionColor::ALL.iter().map(|c| c.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), ConnectionColor::ALL.len());

        for color in ConnectionColor::ALL {
            assert_eq!(ConnectionColor::from_label(color.label()), Some(color));
            assert_eq!(
                color.rgb().is_none(),
                color == ConnectionColor::None,
                "only None leaves the widget colour alone"
            );
        }
        assert_eq!(ConnectionColor::from_label("Chartreuse"), None);
    }

    #[test]
    fn oracle_thin_debug_protocol_version_is_not_serialized() {
        let mut info = ConnectionInfo::new_with_type(
            "local",
            "system",
            "pw",
            "localhost",
            1521,
            "FREE",
            DatabaseType::Oracle,
        );
        info.debug_oracle_thin_protocol_version = Some(314);

        let serialized = serde_json::to_string(&info).expect("ConnectionInfo should serialize");
        let restored: ConnectionInfo =
            serde_json::from_str(&serialized).expect("ConnectionInfo should deserialize");

        assert!(!serialized.contains("debug_oracle_thin_protocol_version"));
        assert_eq!(restored.debug_oracle_thin_protocol_version, None);
    }

    #[test]
    fn oracle_thin_debug_protocol_version_rejects_unknown_versions() {
        let mut info = ConnectionInfo::new_with_type(
            "local",
            "system",
            "pw",
            "localhost",
            1521,
            "FREE",
            DatabaseType::Oracle,
        );
        info.advanced.oracle_driver_mode = OracleDriverMode::Thin;
        info.debug_oracle_thin_protocol_version = Some(313);

        let err =
            DatabaseConnection::build_oracle_thin_config(&info, ConnectionAttemptPolicy::default())
                .expect_err("unsupported debug protocol should be rejected");

        assert!(err.contains("between 314 and 319"));
    }

    #[test]
    fn oracle_thin_protocol_acceptance_log_shows_forced_protocol() {
        assert_eq!(
            DatabaseConnection::format_oracle_thin_protocol_acceptance_log(Some(314), 314, 314, 6),
            "Oracle Thin accepted TNS protocol version 314 (requested 314); TTC field version 6"
        );
    }

    #[test]
    fn oracle_thin_protocol_acceptance_log_shows_default_range_and_unknown_accept() {
        assert_eq!(
            DatabaseConnection::format_oracle_thin_protocol_acceptance_log(Some(319), 314, 319, 24),
            "Oracle Thin accepted TNS protocol version 319 (requested 314..319); TTC field version 24"
        );
        assert_eq!(
            DatabaseConnection::format_oracle_thin_protocol_acceptance_log(None, 314, 319, 17),
            "Oracle Thin accepted TNS protocol version unknown (requested 314..319); TTC field version 17"
        );
    }

    #[test]
    fn database_backend_metadata_covers_dialect_flags_and_cache_keys() {
        assert_eq!(
            DatabaseType::supported(),
            &[
                DatabaseType::Oracle,
                DatabaseType::MySQL,
                DatabaseType::MariaDB
            ]
        );
        let mut cache_keys = std::collections::HashSet::new();
        for db_type in DatabaseType::supported().iter().copied() {
            assert!(
                cache_keys.insert(db_type.cache_key()),
                "duplicate cache key {} for {}",
                db_type.cache_key(),
                db_type
            );
            assert_eq!(DatabaseType::from_cache_key(db_type.cache_key()), db_type);
        }

        assert_eq!(DatabaseType::Oracle.sql_dialect(), SqlDialect::Oracle);
        assert_eq!(
            DatabaseType::Oracle.backend_kind(),
            DatabaseBackendKind::Oracle
        );
        assert_eq!(
            DatabaseType::from_cache_key(DatabaseType::Oracle.cache_key()),
            DatabaseType::Oracle
        );
        assert!(DatabaseType::Oracle.supports_explicit_analytic_null_treatment());
        assert!(!DatabaseType::Oracle.uses_mysql_analytic_null_treatment_rules());
        assert!(!DatabaseType::Oracle.supports_trailing_select_into_after_set_limit());

        assert_eq!(DatabaseType::MySQL.sql_dialect(), SqlDialect::MySql);
        assert_eq!(
            DatabaseType::MySQL.backend_kind(),
            DatabaseBackendKind::MySql
        );
        assert_eq!(
            DatabaseType::from_cache_key(DatabaseType::MySQL.cache_key()),
            DatabaseType::MySQL
        );
        assert!(DatabaseType::MySQL.supports_explicit_analytic_null_treatment());
        assert!(DatabaseType::MySQL.uses_mysql_analytic_null_treatment_rules());
        assert!(DatabaseType::MySQL.supports_trailing_select_into_after_set_limit());

        assert_eq!(DatabaseType::MariaDB.sql_dialect(), SqlDialect::MySql);
        assert_eq!(
            DatabaseType::MariaDB.backend_kind(),
            DatabaseBackendKind::MySql
        );
        assert_eq!(
            DatabaseType::from_cache_key(DatabaseType::MariaDB.cache_key()),
            DatabaseType::MariaDB
        );
        assert_eq!(DatabaseType::MariaDB.choice_label(), "MariaDB");
        assert!(!DatabaseType::MariaDB.supports_explicit_analytic_null_treatment());
        assert!(!DatabaseType::MariaDB.uses_mysql_analytic_null_treatment_rules());
        assert!(!DatabaseType::MariaDB.supports_trailing_select_into_after_set_limit());
    }

    #[test]
    fn backend_retained_session_policies_are_explicit_per_database_type() {
        let clean = RetainedSessionState::default();
        let dirty_transaction =
            RetainedSessionState::from_transaction_state(TransactionSessionState::DecisionRequired);
        let post_processor = crate::db::statement_session_post_processor_for(DatabaseType::MySQL);
        let transaction_mode_override = crate::db::retained_session_state_after_statement(
            post_processor,
            clean,
            post_processor.effects_for_sql("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
            false,
            false,
            false,
            false,
        );

        assert!(!DatabaseType::Oracle.can_apply_empty_scope_to_retained_session());
        assert!(!DatabaseType::Oracle.supports_mysql_delimiter_commands());
        assert!(
            DatabaseType::Oracle.retained_session_blocks_transaction_mode_change(dirty_transaction)
        );
        assert!(
            !DatabaseType::Oracle.can_replace_retained_transaction_mode(transaction_mode_override)
        );

        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            assert!(db_type.can_apply_empty_scope_to_retained_session());
            assert!(db_type.supports_mysql_delimiter_commands());
            assert!(!db_type.retained_session_blocks_transaction_mode_change(dirty_transaction));
            assert!(db_type.can_replace_retained_transaction_mode(transaction_mode_override));
            assert!(!db_type.can_replace_retained_transaction_mode(dirty_transaction));
        }
    }

    #[test]
    fn mysql_backend_rejects_concrete_db_type_mismatch() {
        assert!(MYSQL_BACKEND
            .ensure_concrete_db_type(DatabaseType::MySQL, "pool session")
            .is_ok());
        assert!(MARIADB_BACKEND
            .ensure_concrete_db_type(DatabaseType::MariaDB, "pool session")
            .is_ok());

        let mysql_err = MYSQL_BACKEND
            .ensure_concrete_db_type(DatabaseType::MariaDB, "pool session")
            .expect_err("MySQL backend must reject a MariaDB session");
        assert_eq!(mysql_err, "Expected MySQL pool session but found MariaDB");

        let mariadb_err = MARIADB_BACKEND
            .ensure_concrete_db_type(DatabaseType::MySQL, "live connection")
            .expect_err("MariaDB backend must reject a MySQL live connection");
        assert_eq!(
            mariadb_err,
            "Expected MariaDB live connection but found MySQL"
        );
    }

    #[test]
    fn advanced_defaults_preserve_existing_db_specific_session_settings() {
        let oracle = ConnectionAdvancedSettings::default_for(DatabaseType::Oracle);
        assert_eq!(
            oracle.default_transaction_isolation,
            TransactionIsolation::ReadCommitted
        );
        assert_eq!(
            oracle.default_transaction_access_mode,
            TransactionAccessMode::ReadWrite
        );
        assert!(oracle.session_time_zone.is_empty());
        assert_eq!(
            oracle.oracle_nls_timestamp_format,
            "yyyy-mm-dd hh24:mi:ss.ff6"
        );
        assert_eq!(oracle.oracle_nls_date_format, "yyyy-mm-dd hh24:mi:ss");

        let mysql = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        assert_eq!(
            mysql.default_transaction_isolation,
            TransactionIsolation::ReadCommitted
        );
        assert_eq!(
            mysql.default_transaction_access_mode,
            TransactionAccessMode::ReadWrite
        );
        assert_eq!(mysql.session_time_zone, "+00:00");
        assert_eq!(mysql.mysql_sql_mode, "TRADITIONAL");
        assert_eq!(mysql.mysql_charset, "utf8mb4");
    }

    #[test]
    fn sync_default_transaction_isolation_trusts_applied_advanced_setting() {
        let mut connection = DatabaseConnection::new();
        connection.info = ConnectionInfo::new_with_type(
            "local",
            "system",
            "pw",
            "localhost",
            1521,
            "FREE",
            DatabaseType::Oracle,
        );
        connection.info.advanced.default_transaction_isolation = TransactionIsolation::Serializable;

        connection.sync_default_transaction_isolation(DatabaseType::Oracle);

        assert_eq!(
            connection.default_transaction_isolation(),
            TransactionIsolation::Serializable
        );
    }

    #[test]
    fn oracle_advanced_session_statements_use_configured_values() {
        let mut advanced = ConnectionAdvancedSettings::default_for(DatabaseType::Oracle);
        advanced.default_transaction_isolation = TransactionIsolation::Serializable;
        advanced.session_time_zone = "+09:00".to_string();
        advanced.oracle_nls_date_format = "YYYY/MM/DD HH24:MI:SS".to_string();
        advanced.oracle_nls_timestamp_format = "YYYY/MM/DD HH24:MI:SS.FF3".to_string();

        assert_eq!(
            DatabaseConnection::oracle_session_setting_statements(&advanced),
            vec![
                "ALTER SESSION SET NLS_TIMESTAMP_FORMAT = 'YYYY/MM/DD HH24:MI:SS.FF3'",
                "ALTER SESSION SET NLS_DATE_FORMAT = 'YYYY/MM/DD HH24:MI:SS'",
                "ALTER SESSION SET ISOLATION_LEVEL = SERIALIZABLE",
                "ALTER SESSION SET TIME_ZONE = '+09:00'",
            ]
        );
    }

    #[test]
    fn mysql_advanced_session_statements_use_configured_values() {
        let mut advanced = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        advanced.default_transaction_isolation = TransactionIsolation::RepeatableRead;
        advanced.session_time_zone = "+09:00".to_string();
        advanced.mysql_sql_mode = "ANSI_QUOTES,STRICT_TRANS_TABLES".to_string();

        assert_eq!(
            DatabaseConnection::mysql_session_setting_statements(&advanced),
            vec![
                "SET SESSION sql_mode = 'ANSI_QUOTES,STRICT_TRANS_TABLES'",
                "SET SESSION time_zone = '+09:00'",
                "SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ",
            ]
        );
    }

    #[test]
    fn oracle_direct_connection_string_uses_tcps_for_ssl_or_protocol() {
        let mut info = ConnectionInfo::new_with_type(
            "local",
            "system",
            "pw",
            "localhost",
            2484,
            "FREE",
            DatabaseType::Oracle,
        );
        info.advanced.ssl_mode = ConnectionSslMode::Required;

        assert_eq!(
            info.connection_string(),
            "(DESCRIPTION=(ADDRESS=(PROTOCOL=TCPS)(HOST=localhost)(PORT=2484))(CONNECT_DATA=(SERVICE_NAME=FREE)))"
        );

        info.advanced.ssl_mode = ConnectionSslMode::Disabled;
        info.advanced.oracle_protocol = OracleNetworkProtocol::Tcps;
        assert_eq!(
            info.connection_string(),
            "(DESCRIPTION=(ADDRESS=(PROTOCOL=TCPS)(HOST=localhost)(PORT=2484))(CONNECT_DATA=(SERVICE_NAME=FREE)))"
        );
    }

    #[test]
    fn oracle_oci_direct_descriptor_uses_common_timeout_and_no_retry() {
        let mut info = ConnectionInfo::new_with_type(
            "local",
            "system",
            "pw",
            "dbhost",
            1521,
            "FREE",
            DatabaseType::Oracle,
        );
        let policy = ConnectionAttemptPolicy::from_seconds(8);

        let tcp = OracleBackend::connection_string_with_policy(&info, policy);
        assert!(tcp.contains("(CONNECT_TIMEOUT=8sec)"));
        assert!(tcp.contains("(TRANSPORT_CONNECT_TIMEOUT=8sec)"));
        assert!(tcp.contains("(RETRY_COUNT=0)"));
        assert!(tcp.contains("(PROTOCOL=TCP)"));

        info.advanced.oracle_protocol = OracleNetworkProtocol::Tcps;
        let tcps = OracleBackend::connection_string_with_policy(&info, policy);
        assert!(tcps.contains("(PROTOCOL=TCPS)"));

        info.host.clear();
        info.service_name = "LOCAL_FREE".to_string();
        assert_eq!(
            OracleBackend::connection_string_with_policy(&info, policy),
            "LOCAL_FREE"
        );
    }

    #[test]
    fn mysql_driver_ssl_options_follow_advanced_mode() {
        let mut info = ConnectionInfo::new_with_type(
            "local",
            "root",
            "pw",
            "localhost",
            3306,
            "initial_db",
            DatabaseType::MySQL,
        );
        let opts = mysql::Opts::from(DatabaseConnection::build_mysql_opts(
            &info,
            ConnectionAttemptPolicy::default(),
        ));
        assert!(opts.get_ssl_opts().is_none());

        info.advanced.ssl_mode = ConnectionSslMode::Required;
        let opts = mysql::Opts::from(DatabaseConnection::build_mysql_opts(
            &info,
            ConnectionAttemptPolicy::default(),
        ));
        let ssl = opts.get_ssl_opts().expect("required SSL should be enabled");
        assert!(ssl.skip_domain_validation());
        assert!(ssl.accept_invalid_certs());

        info.advanced.ssl_mode = ConnectionSslMode::VerifyCa;
        info.advanced.mysql_ssl_ca_path = "/tmp/mysql-ca.pem".to_string();
        let opts = mysql::Opts::from(DatabaseConnection::build_mysql_opts(
            &info,
            ConnectionAttemptPolicy::default(),
        ));
        let ssl = opts.get_ssl_opts().expect("Verify CA should enable SSL");
        assert!(ssl.skip_domain_validation());
        assert!(!ssl.accept_invalid_certs());
        assert_eq!(
            ssl.root_cert_path(),
            Some(std::path::Path::new("/tmp/mysql-ca.pem"))
        );

        info.advanced.ssl_mode = ConnectionSslMode::VerifyIdentity;
        let opts = mysql::Opts::from(DatabaseConnection::build_mysql_opts(
            &info,
            ConnectionAttemptPolicy::default(),
        ));
        let ssl = opts
            .get_ssl_opts()
            .expect("Verify identity should enable SSL");
        assert!(!ssl.skip_domain_validation());
        assert!(!ssl.accept_invalid_certs());
    }

    #[test]
    fn advanced_validation_rejects_unsafe_values() {
        let mut mysql = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        mysql.session_time_zone = "UTC".to_string();
        assert!(mysql.validate_for_db(DatabaseType::MySQL, false).is_err());

        mysql.session_time_zone = "+00:00".to_string();
        mysql.mysql_sql_mode = "TRADITIONAL;DROP".to_string();
        assert!(mysql.validate_for_db(DatabaseType::MySQL, false).is_err());

        let mut oracle = ConnectionAdvancedSettings::default_for(DatabaseType::Oracle);
        oracle.ssl_mode = ConnectionSslMode::Required;
        oracle.oracle_protocol = OracleNetworkProtocol::Tcps;
        assert!(oracle.validate_for_db(DatabaseType::Oracle, true).is_ok());

        oracle.ssl_mode = ConnectionSslMode::VerifyCa;
        assert!(oracle.validate_for_db(DatabaseType::Oracle, false).is_err());
    }

    #[test]
    fn oracle_advanced_validation_rejects_read_only_with_explicit_isolation() {
        let mut oracle = ConnectionAdvancedSettings::default_for(DatabaseType::Oracle);
        oracle.default_transaction_access_mode = TransactionAccessMode::ReadOnly;
        oracle.default_transaction_isolation = TransactionIsolation::ReadCommitted;

        let err = oracle
            .validate_for_db(DatabaseType::Oracle, false)
            .expect_err("Oracle cannot run a read-committed read-only transaction");

        assert!(err.contains("READ ONLY"));
        assert!(err.contains("isolation"));

        // Serializable + Read only is what SET TRANSACTION READ ONLY provides,
        // so it validates.
        oracle.default_transaction_isolation = TransactionIsolation::Serializable;
        assert!(oracle.validate_for_db(DatabaseType::Oracle, false).is_ok());

        oracle.default_transaction_isolation = TransactionIsolation::Default;
        assert!(oracle.validate_for_db(DatabaseType::Oracle, false).is_ok());
    }

    #[test]
    fn mysql_advanced_validation_allows_read_only_with_explicit_isolation() {
        let mut mysql = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        mysql.default_transaction_access_mode = TransactionAccessMode::ReadOnly;
        mysql.default_transaction_isolation = TransactionIsolation::ReadCommitted;

        assert!(mysql.validate_for_db(DatabaseType::MySQL, false).is_ok());
    }

    #[test]
    fn session_time_zone_validation_matches_database_ranges() {
        let mut mysql = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        mysql.session_time_zone = "+14:00".to_string();
        assert!(mysql.validate_for_db(DatabaseType::MySQL, false).is_ok());
        mysql.session_time_zone = "-13:59".to_string();
        assert!(mysql.validate_for_db(DatabaseType::MySQL, false).is_ok());
        mysql.session_time_zone = "+14:01".to_string();
        assert!(mysql.validate_for_db(DatabaseType::MySQL, false).is_err());
        mysql.session_time_zone = "-14:00".to_string();
        assert!(mysql.validate_for_db(DatabaseType::MySQL, false).is_err());

        let mut oracle = ConnectionAdvancedSettings::default_for(DatabaseType::Oracle);
        oracle.session_time_zone = "+14:00".to_string();
        assert!(oracle.validate_for_db(DatabaseType::Oracle, false).is_ok());
        oracle.session_time_zone = "-12:00".to_string();
        assert!(oracle.validate_for_db(DatabaseType::Oracle, false).is_ok());
        oracle.session_time_zone = "+14:01".to_string();
        assert!(oracle.validate_for_db(DatabaseType::Oracle, false).is_err());
        oracle.session_time_zone = "-12:01".to_string();
        assert!(oracle.validate_for_db(DatabaseType::Oracle, false).is_err());
        oracle.session_time_zone = "-13:00".to_string();
        assert!(oracle.validate_for_db(DatabaseType::Oracle, false).is_err());
    }

    #[test]
    fn migrate_for_db_type_drops_session_time_zone_unsupported_by_target_db() {
        let mut mysql = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        mysql.session_time_zone = "-13:00".to_string();

        let migrated = mysql.migrate_for_db_type(DatabaseType::MySQL, DatabaseType::Oracle);

        assert_eq!(
            migrated.session_time_zone,
            ConnectionAdvancedSettings::default_for(DatabaseType::Oracle).session_time_zone
        );
    }

    #[test]
    fn mariadb_time_zone_range_is_narrower_than_mysql() {
        let mysql_only_positive = parse_session_time_zone_offset("+13:01").unwrap();
        assert!(mysql_session_time_zone_in_range(mysql_only_positive));
        assert!(!mariadb_session_time_zone_in_range(mysql_only_positive));

        let mysql_only_negative = parse_session_time_zone_offset("-13:00").unwrap();
        assert!(mysql_session_time_zone_in_range(mysql_only_negative));
        assert!(!mariadb_session_time_zone_in_range(mysql_only_negative));

        assert!(mariadb_session_time_zone_in_range(
            parse_session_time_zone_offset("+13:00").unwrap()
        ));
        assert!(mariadb_session_time_zone_in_range(
            parse_session_time_zone_offset("-12:59").unwrap()
        ));
    }

    #[test]
    fn mysql_server_version_time_zone_validation_handles_mariadb_only_limits() {
        assert!(
            DatabaseConnection::validate_mysql_session_time_zone_for_server_version(
                "+13:01", "8.0.46"
            )
            .is_ok()
        );
        assert!(
            DatabaseConnection::validate_mysql_session_time_zone_for_server_version(
                "-13:00", "8.0.46"
            )
            .is_ok()
        );

        let positive_err = DatabaseConnection::validate_mysql_session_time_zone_for_server_version(
            "+13:01",
            "12.2.2-MariaDB",
        )
        .expect_err("MariaDB should reject offsets above +13:00");
        assert!(positive_err.contains("outside MariaDB's supported offset range"));

        let negative_err = DatabaseConnection::validate_mysql_session_time_zone_for_server_version(
            "-13:00",
            "12.2.2-MariaDB",
        )
        .expect_err("MariaDB should reject offsets below -12:59");
        assert!(negative_err.contains("outside MariaDB's supported offset range"));
    }

    #[test]
    fn mysql_advanced_validation_rejects_charset_collation_mismatch() {
        let mut mysql = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        mysql.mysql_charset = "utf8mb4".to_string();
        mysql.mysql_collation = "latin1_swedish_ci".to_string();

        let err = mysql
            .validate_for_db(DatabaseType::MySQL, false)
            .expect_err("collation must belong to the selected character set");

        assert!(err.contains("does not match character set"));
    }

    #[test]
    fn mysql_advanced_validation_accepts_utf8_utf8mb3_alias_collations() {
        let mut mysql = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);

        mysql.mysql_charset = "utf8".to_string();
        mysql.mysql_collation = "utf8mb3_general_ci".to_string();
        assert!(mysql.validate_for_db(DatabaseType::MySQL, false).is_ok());

        mysql.mysql_charset = "utf8mb3".to_string();
        mysql.mysql_collation = "utf8_general_ci".to_string();
        assert!(mysql.validate_for_db(DatabaseType::MySQL, false).is_ok());
    }

    #[test]
    fn mysql_advanced_validation_accepts_binary_charset_collation() {
        let mut mysql = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        mysql.mysql_charset = "binary".to_string();
        mysql.mysql_collation = "binary".to_string();

        assert!(mysql.validate_for_db(DatabaseType::MySQL, false).is_ok());
    }

    #[test]
    fn mysql_set_names_statement_uses_configured_charset_and_collation() {
        let mut advanced = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        advanced.mysql_charset = "utf8mb4".to_string();
        advanced.mysql_collation = "utf8mb4_0900_ai_ci".to_string();

        assert_eq!(
            DatabaseConnection::mysql_set_names_statement_with_settings(
                Some("utf8mb4_unicode_ci"),
                &advanced,
            ),
            "SET NAMES utf8mb4 COLLATE utf8mb4_0900_ai_ci"
        );
    }

    #[test]
    fn mysql_set_names_statement_uses_utf8mb4_database_collation_when_available() {
        assert_eq!(
            DatabaseConnection::mysql_set_names_statement(Some("utf8mb4_unicode_ci")),
            "SET NAMES utf8mb4 COLLATE utf8mb4_unicode_ci"
        );
    }

    #[test]
    fn mysql_set_names_statement_matches_database_collation_case_insensitively() {
        let mut advanced = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        advanced.mysql_charset = "UTF8MB4".to_string();

        assert_eq!(
            DatabaseConnection::mysql_set_names_statement_with_settings(
                Some("utf8mb4_unicode_ci"),
                &advanced,
            ),
            "SET NAMES UTF8MB4 COLLATE utf8mb4_unicode_ci"
        );
    }

    #[test]
    fn mysql_set_names_statement_accepts_utf8_utf8mb3_alias_collations() {
        let mut advanced = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        advanced.mysql_charset = "utf8".to_string();

        assert_eq!(
            DatabaseConnection::mysql_set_names_statement_with_settings(
                Some("utf8mb3_general_ci"),
                &advanced,
            ),
            "SET NAMES utf8 COLLATE utf8mb3_general_ci"
        );

        advanced.mysql_charset = "utf8mb3".to_string();

        assert_eq!(
            DatabaseConnection::mysql_set_names_statement_with_settings(
                Some("utf8_general_ci"),
                &advanced,
            ),
            "SET NAMES utf8mb3 COLLATE utf8_general_ci"
        );
    }

    #[test]
    fn mysql_set_names_statement_accepts_binary_database_collation() {
        let mut advanced = ConnectionAdvancedSettings::default_for(DatabaseType::MySQL);
        advanced.mysql_charset = "binary".to_string();

        assert_eq!(
            DatabaseConnection::mysql_set_names_statement_with_settings(Some("binary"), &advanced,),
            "SET NAMES binary COLLATE binary"
        );
    }

    #[test]
    fn mysql_set_names_statement_falls_back_for_non_utf8mb4_database_collation() {
        assert_eq!(
            DatabaseConnection::mysql_set_names_statement(Some("latin1_swedish_ci")),
            "SET NAMES utf8mb4"
        );
    }

    #[test]
    fn mysql_set_names_statement_falls_back_for_unsafe_collation_name() {
        assert_eq!(
            DatabaseConnection::mysql_set_names_statement(Some("utf8mb4_unicode_ci;DROP")),
            "SET NAMES utf8mb4"
        );
    }

    #[test]
    fn oracle_set_current_schema_statement_keeps_simple_identifier_unquoted() {
        assert_eq!(
            DatabaseConnection::oracle_set_current_schema_statement("SCOTT"),
            "ALTER SESSION SET CURRENT_SCHEMA = SCOTT"
        );
    }

    #[test]
    fn oracle_set_current_schema_statement_quotes_schema_when_needed() {
        assert_eq!(
            DatabaseConnection::oracle_set_current_schema_statement("Sales Ops"),
            r#"ALTER SESSION SET CURRENT_SCHEMA = "Sales Ops""#
        );
    }

    #[test]
    fn oracle_set_current_schema_statement_quotes_lowercase_schema() {
        assert_eq!(
            DatabaseConnection::oracle_set_current_schema_statement("app_user"),
            r#"ALTER SESSION SET CURRENT_SCHEMA = "app_user""#
        );
    }

    #[test]
    fn normalize_oracle_current_schema_name_trims_blank_values() {
        assert_eq!(
            DatabaseConnection::normalize_oracle_current_schema_name("   "),
            None
        );
        assert_eq!(
            DatabaseConnection::normalize_oracle_current_schema_name(" sys "),
            Some("sys".to_string())
        );
    }

    #[test]
    fn disconnect_clears_tracked_oracle_current_schema() {
        let mut conn = DatabaseConnection::new();
        conn.info = ConnectionInfo::new("Prod", "scott", "pw", "db", 1521, "FREE");
        conn.connected = true;
        conn.oracle_current_schema = Some("SYS".to_string());

        conn.disconnect();

        assert!(conn.oracle_current_schema.is_none());
    }

    #[test]
    fn mysql_pool_timeout_error_gets_actionable_exhaustion_message_for_db_type() {
        let message = DbConnectionPool::format_mysql_pool_acquire_error(
            DatabaseType::MySQL,
            &mysql::Error::DriverError(mysql::DriverError::Timeout),
        );

        assert!(message.contains("MySQL connection pool appears exhausted"));

        let message = DbConnectionPool::format_mysql_pool_acquire_error(
            DatabaseType::MariaDB,
            &mysql::Error::DriverError(mysql::DriverError::Timeout),
        );

        assert!(message.contains("MariaDB connection pool appears exhausted"));
        assert!(!message.contains("MySQL connection pool appears exhausted"));
    }

    #[test]
    fn mysql_network_timeout_error_is_not_reported_as_pool_exhaustion() {
        let err = mysql::Error::IoError(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Operation timed out",
        ));
        let message =
            DbConnectionPool::format_mysql_pool_acquire_error(DatabaseType::MariaDB, &err);

        assert!(!message.contains("MySQL connection pool appears exhausted"));
        assert!(!message.contains("MariaDB connection pool appears exhausted"));
    }

    #[test]
    #[ignore = "requires local Oracle XE plus TNS_ADMIN/ORACLE_TEST_* environment variables"]
    fn oracle_test_connection_supports_tns_alias_from_tns_admin() {
        let username =
            std::env::var("ORACLE_TEST_USERNAME").expect("ORACLE_TEST_USERNAME must be set");
        let password =
            std::env::var("ORACLE_TEST_PASSWORD").expect("ORACLE_TEST_PASSWORD must be set");
        let alias =
            std::env::var("ORACLE_TEST_TNS_ALIAS").expect("ORACLE_TEST_TNS_ALIAS must be set");

        let info = ConnectionInfo::new_with_type(
            "local",
            &username,
            &password,
            "",
            0,
            &alias,
            DatabaseType::Oracle,
        );

        DatabaseConnection::test_connection(&info)
            .expect("TNS alias connection should succeed against local Oracle XE");
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_test_connection_supports_direct_local_xe() {
        let username =
            std::env::var("ORACLE_TEST_USERNAME").expect("ORACLE_TEST_USERNAME must be set");
        let password =
            std::env::var("ORACLE_TEST_PASSWORD").expect("ORACLE_TEST_PASSWORD must be set");
        let service_name = std::env::var("ORACLE_TEST_SERVICE_NAME")
            .expect("ORACLE_TEST_SERVICE_NAME must be set");
        let host = std::env::var("ORACLE_TEST_HOST").unwrap_or_else(|_| "localhost".to_string());
        let port = std::env::var("ORACLE_TEST_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(1521);

        let info = ConnectionInfo::new_with_type(
            "local",
            &username,
            &password,
            &host,
            port,
            &service_name,
            DatabaseType::Oracle,
        );

        DatabaseConnection::test_connection(&info)
            .expect("Direct localhost Oracle connection should succeed against local Oracle XE");
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_connect_sets_read_committed_as_default_transaction_isolation() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(oracle_test_connection_info_from_env())
            .expect("Direct localhost Oracle connection should succeed");

        assert_eq!(
            connection.default_transaction_isolation(),
            TransactionIsolation::ReadCommitted
        );
        assert_eq!(connection.transaction_mode(), TransactionMode::default());
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_thin_connect_sets_read_committed_as_default_transaction_isolation() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(oracle_thin_test_connection_info_from_env())
            .expect("Direct localhost Oracle Thin connection should succeed");

        assert_eq!(
            connection.default_transaction_isolation(),
            TransactionIsolation::ReadCommitted
        );
        assert_eq!(connection.transaction_mode(), TransactionMode::default());
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_thin_select_one_text_reads_non_text_scalars() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(oracle_thin_test_connection_info_from_env())
            .expect("Direct localhost Oracle Thin connection should succeed");
        let conn = connection
            .get_oracle_thin_connection()
            .expect("Oracle Thin connection should be live");
        let mut conn = conn.lock().expect("Oracle Thin connection lock");

        let value =
            DatabaseConnection::oracle_thin_select_one_text(&mut conn, "SELECT 1 FROM dual")
                .expect("Oracle Thin numeric scalar probe should succeed")
                .expect("Oracle Thin numeric scalar probe should return a row");
        assert_eq!(value, "1");
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_connect_applies_advanced_session_settings_from_local_xe() {
        let mut info = oracle_test_connection_info_from_env();
        info.advanced.default_transaction_isolation = TransactionIsolation::Serializable;
        info.advanced.session_time_zone = "+09:00".to_string();
        info.advanced.oracle_nls_date_format = "YYYY-MM-DD HH24:MI:SS".to_string();
        info.advanced.oracle_nls_timestamp_format = "YYYY-MM-DD HH24:MI:SS.FF3".to_string();

        let mut connection = DatabaseConnection::new();
        connection
            .connect(info)
            .expect("Direct localhost Oracle connection should succeed");
        let conn = connection
            .require_live_connection()
            .expect("Oracle connection should be live");

        assert_eq!(
            DatabaseConnection::read_oracle_default_transaction_isolation(conn.as_ref())
                .expect("read Oracle current transaction isolation"),
            Some(TransactionIsolation::Serializable)
        );
        assert_eq!(
            read_oracle_session_parameter(conn.as_ref(), "NLS_DATE_FORMAT"),
            "YYYY-MM-DD HH24:MI:SS"
        );
        assert_eq!(
            read_oracle_session_parameter(conn.as_ref(), "NLS_TIMESTAMP_FORMAT"),
            "YYYY-MM-DD HH24:MI:SS.FF3"
        );
        assert_eq!(read_oracle_session_time_zone(conn.as_ref()), "+09:00");
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_thin_connect_applies_advanced_session_settings_from_local_xe() {
        let mut info = oracle_thin_test_connection_info_from_env();
        info.advanced.default_transaction_isolation = TransactionIsolation::Serializable;
        info.advanced.session_time_zone = "+09:00".to_string();
        info.advanced.oracle_nls_date_format = "YYYY-MM-DD HH24:MI:SS".to_string();
        info.advanced.oracle_nls_timestamp_format = "YYYY-MM-DD HH24:MI:SS.FF3".to_string();

        let mut connection = DatabaseConnection::new();
        connection
            .connect(info)
            .expect("Direct localhost Oracle Thin connection should succeed");
        let conn = connection
            .get_oracle_thin_connection()
            .expect("Oracle Thin connection should be live");
        let mut conn = conn.lock().expect("Oracle Thin connection lock");

        assert_eq!(
            read_oracle_thin_default_transaction_isolation(&mut conn),
            Some(TransactionIsolation::Serializable)
        );
        assert_eq!(
            read_oracle_thin_session_parameter(&mut conn, "NLS_DATE_FORMAT"),
            "YYYY-MM-DD HH24:MI:SS"
        );
        assert_eq!(
            read_oracle_thin_session_parameter(&mut conn, "NLS_TIMESTAMP_FORMAT"),
            "YYYY-MM-DD HH24:MI:SS.FF3"
        );
        assert_eq!(read_oracle_thin_session_time_zone(&mut conn), "+09:00");
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_pool_session_applies_advanced_session_settings_from_local_xe() {
        let mut info = oracle_test_connection_info_from_env();
        info.advanced.default_transaction_isolation = TransactionIsolation::Serializable;
        info.advanced.session_time_zone = "+09:00".to_string();
        info.advanced.oracle_nls_date_format = "YYYY-MM-DD HH24:MI:SS".to_string();
        info.advanced.oracle_nls_timestamp_format = "YYYY-MM-DD HH24:MI:SS.FF3".to_string();

        let mut connection = DatabaseConnection::new();
        connection
            .connect(info)
            .expect("Direct localhost Oracle connection should succeed");

        let Some(DbPoolSession::Oracle(conn)) = connection
            .acquire_pool_session()
            .expect("Oracle pool session should be acquired")
        else {
            panic!("expected Oracle pool session");
        };

        assert_eq!(
            DatabaseConnection::read_oracle_default_transaction_isolation(&conn)
                .expect("read Oracle current transaction isolation"),
            Some(TransactionIsolation::Serializable)
        );
        assert_eq!(
            read_oracle_session_parameter(&conn, "NLS_DATE_FORMAT"),
            "YYYY-MM-DD HH24:MI:SS"
        );
        assert_eq!(
            read_oracle_session_parameter(&conn, "NLS_TIMESTAMP_FORMAT"),
            "YYYY-MM-DD HH24:MI:SS.FF3"
        );
        assert_eq!(read_oracle_session_time_zone(&conn), "+09:00");
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_thin_pool_session_applies_advanced_session_settings_from_local_xe() {
        let mut info = oracle_thin_test_connection_info_from_env();
        info.advanced.default_transaction_isolation = TransactionIsolation::Serializable;
        info.advanced.session_time_zone = "+09:00".to_string();
        info.advanced.oracle_nls_date_format = "YYYY-MM-DD HH24:MI:SS".to_string();
        info.advanced.oracle_nls_timestamp_format = "YYYY-MM-DD HH24:MI:SS.FF3".to_string();

        let mut connection = DatabaseConnection::new();
        connection
            .connect(info)
            .expect("Direct localhost Oracle Thin connection should succeed");

        let Some(DbPoolSession::OracleThin(mut conn)) = connection
            .acquire_pool_session()
            .expect("Oracle Thin pool session should be acquired")
        else {
            panic!("expected Oracle Thin pool session");
        };

        assert_eq!(
            read_oracle_thin_default_transaction_isolation(&mut conn),
            Some(TransactionIsolation::Serializable)
        );
        assert_eq!(
            read_oracle_thin_session_parameter(&mut conn, "NLS_DATE_FORMAT"),
            "YYYY-MM-DD HH24:MI:SS"
        );
        assert_eq!(
            read_oracle_thin_session_parameter(&mut conn, "NLS_TIMESTAMP_FORMAT"),
            "YYYY-MM-DD HH24:MI:SS.FF3"
        );
        assert_eq!(read_oracle_thin_session_time_zone(&mut conn), "+09:00");
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_thin_switch_current_schema_uses_thin_connection_from_local_xe() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(oracle_thin_test_connection_info_from_env())
            .expect("Direct localhost Oracle Thin connection should succeed");
        let current_schema = {
            let conn = connection
                .get_oracle_thin_connection()
                .expect("Oracle Thin connection should be live");
            let mut conn = conn.lock().expect("Oracle Thin connection lock");
            DatabaseConnection::read_oracle_thin_current_schema(&mut conn)
                .expect("read Oracle Thin current schema")
        };

        connection
            .switch_oracle_current_schema(&current_schema)
            .expect("Oracle Thin schema switch should not use OCI-only connection path");

        assert_eq!(
            connection.tracked_oracle_current_schema(),
            Some(current_schema.as_str())
        );
    }

    #[test]
    #[ignore = "requires local Oracle TCPS listener plus ORACLE_TEST_* environment variables"]
    fn oracle_tcps_connection_uses_advanced_ssl_protocol() {
        let mut info = oracle_test_connection_info_from_env();
        info.port = std::env::var("ORACLE_TEST_TCPS_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(2484);
        info.advanced.ssl_mode = ConnectionSslMode::Required;

        DatabaseConnection::test_connection(&info)
            .expect("Oracle TCPS connection should succeed against configured listener");
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_transaction_mode_applies_every_supported_isolation_from_local_xe() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(oracle_test_connection_info_from_env())
            .expect("Direct localhost Oracle connection should succeed");
        let conn = connection
            .require_live_connection()
            .expect("Oracle connection should be live");

        for isolation in [
            TransactionIsolation::ReadCommitted,
            TransactionIsolation::Serializable,
        ] {
            DatabaseConnection::apply_oracle_transaction_mode(
                conn.as_ref(),
                TransactionMode::new(isolation, TransactionAccessMode::ReadWrite),
            )
            .unwrap_or_else(|err| panic!("Oracle should apply {}: {err}", isolation.label()));

            let observed =
                DatabaseConnection::read_oracle_default_transaction_isolation(conn.as_ref())
                    .expect("read Oracle current transaction isolation")
                    .expect("Oracle should report a transaction isolation");
            assert_eq!(observed, isolation);
            let _ = conn.rollback();
        }
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_thin_transaction_mode_applies_every_supported_isolation_from_local_xe() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(oracle_thin_test_connection_info_from_env())
            .expect("Direct localhost Oracle Thin connection should succeed");
        let conn = connection
            .get_oracle_thin_connection()
            .expect("Oracle Thin connection should be live");
        let mut conn = conn.lock().expect("Oracle Thin connection lock");

        for isolation in [
            TransactionIsolation::ReadCommitted,
            TransactionIsolation::Serializable,
        ] {
            DatabaseConnection::apply_oracle_thin_transaction_mode(
                &mut conn,
                TransactionMode::new(isolation, TransactionAccessMode::ReadWrite),
            )
            .unwrap_or_else(|err| panic!("Oracle Thin should apply {}: {err}", isolation.label()));

            let observed = read_oracle_thin_default_transaction_isolation(&mut conn)
                .expect("Oracle Thin should report a transaction isolation");
            assert_eq!(observed, isolation);
            let _ = conn.rollback();
        }
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_transaction_mode_serializable_applies_from_local_xe() {
        let username =
            std::env::var("ORACLE_TEST_USERNAME").expect("ORACLE_TEST_USERNAME must be set");
        let password =
            std::env::var("ORACLE_TEST_PASSWORD").expect("ORACLE_TEST_PASSWORD must be set");
        let service_name = std::env::var("ORACLE_TEST_SERVICE_NAME")
            .expect("ORACLE_TEST_SERVICE_NAME must be set");
        let host = std::env::var("ORACLE_TEST_HOST").unwrap_or_else(|_| "localhost".to_string());
        let port = std::env::var("ORACLE_TEST_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(1521);

        let mut connection = DatabaseConnection::new();
        connection
            .connect(ConnectionInfo::new_with_type(
                "local",
                &username,
                &password,
                &host,
                port,
                &service_name,
                DatabaseType::Oracle,
            ))
            .expect("Direct localhost Oracle connection should succeed");
        let conn = connection
            .require_live_connection()
            .expect("Oracle connection should be live");

        DatabaseConnection::apply_oracle_transaction_mode(
            conn.as_ref(),
            TransactionMode::new(
                TransactionIsolation::Serializable,
                TransactionAccessMode::ReadWrite,
            ),
        )
        .expect("Oracle serializable transaction mode should apply");

        let mut stmt = conn
            .statement("SELECT 1 FROM dual")
            .build()
            .expect("build serializable probe statement");
        let value = stmt
            .query_row_as::<i64>(&[])
            .expect("serializable transaction should allow SELECT");
        assert_eq!(value, 1);
        let _ = conn.rollback();
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_thin_transaction_mode_serializable_applies_from_local_xe() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(oracle_thin_test_connection_info_from_env())
            .expect("Direct localhost Oracle Thin connection should succeed");
        let conn = connection
            .get_oracle_thin_connection()
            .expect("Oracle Thin connection should be live");
        let mut conn = conn.lock().expect("Oracle Thin connection lock");

        DatabaseConnection::apply_oracle_thin_transaction_mode(
            &mut conn,
            TransactionMode::new(
                TransactionIsolation::Serializable,
                TransactionAccessMode::ReadWrite,
            ),
        )
        .expect("Oracle Thin serializable transaction mode should apply");

        let value = DatabaseConnection::oracle_thin_select_one_text(
            &mut conn,
            "SELECT TO_CHAR(1) FROM dual",
        )
        .expect("serializable transaction should allow SELECT")
        .expect("serializable SELECT should return a row");
        assert_eq!(value, "1");
        let _ = conn.rollback();
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_transaction_mode_read_only_blocks_dml_from_local_xe() {
        let username =
            std::env::var("ORACLE_TEST_USERNAME").expect("ORACLE_TEST_USERNAME must be set");
        let password =
            std::env::var("ORACLE_TEST_PASSWORD").expect("ORACLE_TEST_PASSWORD must be set");
        let service_name = std::env::var("ORACLE_TEST_SERVICE_NAME")
            .expect("ORACLE_TEST_SERVICE_NAME must be set");
        let host = std::env::var("ORACLE_TEST_HOST").unwrap_or_else(|_| "localhost".to_string());
        let port = std::env::var("ORACLE_TEST_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(1521);

        let info = ConnectionInfo::new_with_type(
            "local",
            &username,
            &password,
            &host,
            port,
            &service_name,
            DatabaseType::Oracle,
        );

        {
            let mut setup = DatabaseConnection::new();
            setup
                .connect(info.clone())
                .expect("Direct localhost Oracle connection should succeed");
            let conn = setup
                .require_live_connection()
                .expect("Oracle setup connection should be live");
            let _ = conn.execute("DROP TABLE qt_tx_mode_probe PURGE", &[]);
            conn.execute("CREATE TABLE qt_tx_mode_probe (id NUMBER)", &[])
                .expect("create transaction mode probe table");
            conn.commit().expect("commit probe table DDL");
        }

        {
            let mut connection = DatabaseConnection::new();
            connection
                .connect(info.clone())
                .expect("Direct localhost Oracle connection should succeed");
            let conn = connection
                .require_live_connection()
                .expect("Oracle connection should be live");

            DatabaseConnection::apply_oracle_transaction_mode(
                conn.as_ref(),
                TransactionMode::new(
                    TransactionIsolation::Default,
                    TransactionAccessMode::ReadOnly,
                ),
            )
            .expect("Oracle transaction mode should apply");

            let mut stmt = conn
                .statement("SELECT 1 FROM dual")
                .build()
                .expect("build read probe statement");
            let value = stmt
                .query_row_as::<i64>(&[])
                .expect("read-only transaction should allow SELECT");
            assert_eq!(value, 1);
            drop(stmt);

            let insert_err = conn
                .execute("INSERT INTO qt_tx_mode_probe (id) VALUES (1)", &[])
                .expect_err("read-only transaction should reject DML");
            let insert_message = insert_err.to_string();
            assert!(
                insert_message.contains("ORA-01456")
                    || insert_message.to_ascii_lowercase().contains("read only"),
                "unexpected Oracle read-only DML error: {insert_message}"
            );
            let _ = conn.rollback();
        }

        {
            let mut cleanup = DatabaseConnection::new();
            cleanup
                .connect(info)
                .expect("Direct localhost Oracle connection should succeed for cleanup");
            if let Ok(conn) = cleanup.require_live_connection() {
                let _ = conn.execute("DROP TABLE qt_tx_mode_probe PURGE", &[]);
            }
        }
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_thin_transaction_mode_read_only_blocks_dml_from_local_xe() {
        let info = oracle_thin_test_connection_info_from_env();

        {
            let mut setup = DatabaseConnection::new();
            setup
                .connect(info.clone())
                .expect("Direct localhost Oracle Thin connection should succeed");
            let conn = setup
                .get_oracle_thin_connection()
                .expect("Oracle Thin setup connection should be live");
            let mut conn = conn.lock().expect("Oracle Thin setup connection lock");
            let _ = conn.execute_typed(
                &StatementRequest::statement("DROP TABLE qt_tx_mode_probe PURGE"),
                &[],
            );
            conn.execute_typed(
                &StatementRequest::statement("CREATE TABLE qt_tx_mode_probe (id NUMBER)"),
                &[],
            )
            .expect("create transaction mode probe table");
            conn.commit().expect("commit probe table DDL");
        }

        {
            let mut connection = DatabaseConnection::new();
            connection
                .connect(info.clone())
                .expect("Direct localhost Oracle Thin connection should succeed");
            let conn = connection
                .get_oracle_thin_connection()
                .expect("Oracle Thin connection should be live");
            let mut conn = conn.lock().expect("Oracle Thin connection lock");

            DatabaseConnection::apply_oracle_thin_transaction_mode(
                &mut conn,
                TransactionMode::new(
                    TransactionIsolation::Default,
                    TransactionAccessMode::ReadOnly,
                ),
            )
            .expect("Oracle Thin transaction mode should apply");

            let value = DatabaseConnection::oracle_thin_select_one_text(
                &mut conn,
                "SELECT TO_CHAR(1) FROM dual",
            )
            .expect("read-only transaction should allow SELECT")
            .expect("read-only SELECT should return a row");
            assert_eq!(value, "1");

            let insert_err = conn
                .execute_typed(
                    &StatementRequest::statement("INSERT INTO qt_tx_mode_probe (id) VALUES (1)"),
                    &[],
                )
                .expect_err("read-only transaction should reject DML");
            let insert_message = insert_err.to_string();
            assert!(
                insert_message.contains("ORA-01456")
                    || insert_message.to_ascii_lowercase().contains("read only"),
                "unexpected Oracle Thin read-only DML error: {insert_message}"
            );
            let _ = conn.rollback();
        }

        {
            let mut cleanup = DatabaseConnection::new();
            cleanup
                .connect(info)
                .expect("Direct localhost Oracle Thin connection should succeed for cleanup");
            if let Some(conn) = cleanup.get_oracle_thin_connection() {
                let mut conn = conn.lock().expect("Oracle Thin cleanup connection lock");
                let _ = conn.execute_typed(
                    &StatementRequest::statement("DROP TABLE qt_tx_mode_probe PURGE"),
                    &[],
                );
            }
        }
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_read_only_transaction_can_be_reapplied_after_rollback_from_local_xe() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(oracle_test_connection_info_from_env())
            .expect("Direct localhost Oracle connection should succeed");
        let conn = connection
            .require_live_connection()
            .expect("Oracle connection should be live");
        let read_only_mode = TransactionMode::new(
            TransactionIsolation::Default,
            TransactionAccessMode::ReadOnly,
        );

        for attempt in 1..=2 {
            DatabaseConnection::apply_oracle_transaction_mode(conn.as_ref(), read_only_mode)
                .unwrap_or_else(|err| {
                    panic!("Oracle read-only mode should apply on attempt {attempt}: {err}")
                });

            let mut stmt = conn
                .statement("SELECT 1 FROM dual")
                .build()
                .unwrap_or_else(|err| panic!("build read-only probe on attempt {attempt}: {err}"));
            let value = stmt
                .query_row_as::<i64>(&[])
                .unwrap_or_else(|err| panic!("run read-only probe on attempt {attempt}: {err}"));
            assert_eq!(value, 1);
            drop(stmt);

            conn.rollback().unwrap_or_else(|err| {
                panic!("close read-only transaction on attempt {attempt}: {err}")
            });
        }
    }

    #[test]
    #[ignore = "requires local Oracle XE plus ORACLE_TEST_* environment variables"]
    fn oracle_thin_read_only_transaction_can_be_reapplied_after_rollback_from_local_xe() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(oracle_thin_test_connection_info_from_env())
            .expect("Direct localhost Oracle Thin connection should succeed");
        let conn = connection
            .get_oracle_thin_connection()
            .expect("Oracle Thin connection should be live");
        let mut conn = conn.lock().expect("Oracle Thin connection lock");
        let read_only_mode = TransactionMode::new(
            TransactionIsolation::Default,
            TransactionAccessMode::ReadOnly,
        );

        for attempt in 1..=2 {
            DatabaseConnection::apply_oracle_thin_transaction_mode(&mut conn, read_only_mode)
                .unwrap_or_else(|err| {
                    panic!("Oracle Thin read-only mode should apply on attempt {attempt}: {err}")
                });

            let value = DatabaseConnection::oracle_thin_select_one_text(
                &mut conn,
                "SELECT TO_CHAR(1) FROM dual",
            )
            .unwrap_or_else(|err| panic!("run read-only probe on attempt {attempt}: {err}"))
            .unwrap_or_else(|| panic!("read-only probe should return a row on attempt {attempt}"));
            assert_eq!(value, "1");

            conn.rollback().unwrap_or_else(|err| {
                panic!("close read-only transaction on attempt {attempt}: {err}")
            });
        }
    }

    #[test]
    #[ignore = "requires local MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_pool_session_applies_default_session_settings_from_local_mariadb() {
        let host = std::env::var("SPACE_QUERY_TEST_MYSQL_HOST")
            .expect("SPACE_QUERY_TEST_MYSQL_HOST must be set");
        let database = std::env::var("SPACE_QUERY_TEST_MYSQL_DATABASE")
            .expect("SPACE_QUERY_TEST_MYSQL_DATABASE must be set");
        let user = std::env::var("SPACE_QUERY_TEST_MYSQL_USER")
            .expect("SPACE_QUERY_TEST_MYSQL_USER must be set");
        let password = std::env::var("SPACE_QUERY_TEST_MYSQL_PASSWORD")
            .expect("SPACE_QUERY_TEST_MYSQL_PASSWORD must be set");
        let port = std::env::var("SPACE_QUERY_TEST_MYSQL_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(3306);

        let mut connection = DatabaseConnection::new();
        connection
            .connect(ConnectionInfo::new_with_type(
                "local",
                &user,
                &password,
                &host,
                port,
                &database,
                DatabaseType::MariaDB,
            ))
            .expect("MariaDB connection should succeed");

        let Some(DbPoolSession::MySQL { mut conn, .. }) = connection
            .acquire_pool_session()
            .expect("MySQL pool session should be acquired")
        else {
            panic!("expected MySQL pool session");
        };
        let sql_mode = conn
            .query_first::<String, _>("SELECT @@SESSION.sql_mode")
            .expect("read sql_mode")
            .unwrap_or_default();
        let time_zone = conn
            .query_first::<String, _>("SELECT @@SESSION.time_zone")
            .expect("read time_zone")
            .unwrap_or_default();
        let character_set_client = conn
            .query_first::<String, _>("SELECT @@SESSION.character_set_client")
            .expect("read character_set_client")
            .unwrap_or_default();
        let isolation = DatabaseConnection::read_mysql_default_transaction_isolation(&mut conn)
            .expect("read transaction isolation")
            .expect("transaction isolation should be available");

        assert!(sql_mode.contains("STRICT_TRANS_TABLES"));
        assert_eq!(time_zone, "+00:00");
        assert_eq!(character_set_client, "utf8mb4");
        assert_eq!(isolation, TransactionIsolation::ReadCommitted);
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_pool_session_applies_advanced_session_settings() {
        assert_mysql_pool_session_applies_advanced_session_settings(DatabaseType::MySQL);
    }

    #[test]
    #[ignore = "requires local MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mariadb_pool_session_applies_advanced_session_settings() {
        assert_mysql_pool_session_applies_advanced_session_settings(DatabaseType::MariaDB);
    }

    fn assert_mysql_pool_session_applies_advanced_session_settings(db_type: DatabaseType) {
        let mut info = mysql_test_connection_info_from_env_for(db_type);
        info.advanced.default_transaction_isolation = TransactionIsolation::RepeatableRead;
        info.advanced.session_time_zone = "+09:00".to_string();
        info.advanced.mysql_sql_mode = "ANSI_QUOTES,STRICT_TRANS_TABLES".to_string();
        info.advanced.mysql_charset = "utf8mb4".to_string();
        info.advanced.mysql_collation = "utf8mb4_unicode_ci".to_string();

        let mut connection = DatabaseConnection::new();
        connection
            .connect(info)
            .expect("MySQL/MariaDB connection should succeed");

        let Some(DbPoolSession::MySQL { mut conn, .. }) = connection
            .acquire_pool_session()
            .expect("MySQL pool session should be acquired")
        else {
            panic!("expected MySQL pool session");
        };

        let sql_mode = conn
            .query_first::<String, _>("SELECT @@SESSION.sql_mode")
            .expect("read sql_mode")
            .unwrap_or_default();
        let time_zone = conn
            .query_first::<String, _>("SELECT @@SESSION.time_zone")
            .expect("read time_zone")
            .unwrap_or_default();
        let character_set_client = conn
            .query_first::<String, _>("SELECT @@SESSION.character_set_client")
            .expect("read character_set_client")
            .unwrap_or_default();
        let collation_connection = conn
            .query_first::<String, _>("SELECT @@SESSION.collation_connection")
            .expect("read collation_connection")
            .unwrap_or_default();
        let isolation = DatabaseConnection::read_mysql_default_transaction_isolation(&mut conn)
            .expect("read transaction isolation")
            .expect("transaction isolation should be available");

        assert!(sql_mode.contains("ANSI_QUOTES"));
        assert!(sql_mode.contains("STRICT_TRANS_TABLES"));
        assert_eq!(time_zone, "+09:00");
        assert_eq!(character_set_client, "utf8mb4");
        assert_eq!(collation_connection, "utf8mb4_unicode_ci");
        assert_eq!(isolation, TransactionIsolation::RepeatableRead);
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_pool_session_context_applies_global_auto_commit() {
        let mut connection = DatabaseConnection::new();
        connection
            .set_auto_commit(true)
            .expect("set initial MySQL/MariaDB auto-commit");
        connection
            .connect(mysql_test_connection_info_from_env())
            .expect("MySQL/MariaDB connection should succeed");
        connection
            .set_auto_commit(false)
            .expect("disable global MySQL/MariaDB auto-commit");

        let context = connection
            .pool_session_context()
            .expect("MySQL pool context should be available");
        assert!(!context.auto_commit);

        let Some(DbPoolSession::MySQL { mut conn, .. }) = connection
            .acquire_pool_session()
            .expect("MySQL pool session should be acquired")
        else {
            panic!("expected MySQL pool session");
        };
        let autocommit = conn
            .query_first::<u8, _>("SELECT @@autocommit")
            .expect("read MySQL/MariaDB autocommit")
            .expect("autocommit variable should be available");

        assert_eq!(autocommit, 0);
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_connect_applies_advanced_session_settings() {
        assert_mysql_connect_applies_advanced_session_settings(DatabaseType::MySQL);
    }

    #[test]
    #[ignore = "requires local MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mariadb_connect_applies_advanced_session_settings() {
        assert_mysql_connect_applies_advanced_session_settings(DatabaseType::MariaDB);
    }

    fn assert_mysql_connect_applies_advanced_session_settings(db_type: DatabaseType) {
        let mut info = mysql_test_connection_info_from_env_for(db_type);
        info.advanced.default_transaction_isolation = TransactionIsolation::RepeatableRead;
        info.advanced.default_transaction_access_mode = TransactionAccessMode::ReadOnly;
        info.advanced.session_time_zone = "+09:00".to_string();
        info.advanced.mysql_sql_mode = "ANSI_QUOTES,STRICT_TRANS_TABLES".to_string();
        info.advanced.mysql_charset = "utf8mb4".to_string();
        info.advanced.mysql_collation = "utf8mb4_unicode_ci".to_string();

        let mut connection = DatabaseConnection::new();
        connection
            .connect(info)
            .expect("MySQL/MariaDB connection should succeed");
        assert_eq!(
            connection.default_transaction_isolation(),
            TransactionIsolation::RepeatableRead
        );
        assert_eq!(
            connection.transaction_mode(),
            TransactionMode::new(
                TransactionIsolation::Default,
                TransactionAccessMode::ReadOnly
            )
        );

        let conn = connection
            .get_mysql_connection_mut()
            .expect("MySQL connection should be live");
        let sql_mode = conn
            .query_first::<String, _>("SELECT @@SESSION.sql_mode")
            .expect("read sql_mode")
            .unwrap_or_default();
        let time_zone = conn
            .query_first::<String, _>("SELECT @@SESSION.time_zone")
            .expect("read time_zone")
            .unwrap_or_default();
        let character_set_client = conn
            .query_first::<String, _>("SELECT @@SESSION.character_set_client")
            .expect("read character_set_client")
            .unwrap_or_default();
        let collation_connection = conn
            .query_first::<String, _>("SELECT @@SESSION.collation_connection")
            .expect("read collation_connection")
            .unwrap_or_default();
        let isolation = DatabaseConnection::read_mysql_default_transaction_isolation(conn)
            .expect("read transaction isolation")
            .expect("transaction isolation should be available");

        assert!(sql_mode.contains("ANSI_QUOTES"));
        assert!(sql_mode.contains("STRICT_TRANS_TABLES"));
        assert_eq!(time_zone, "+09:00");
        assert_eq!(character_set_client, "utf8mb4");
        assert_eq!(collation_connection, "utf8mb4_unicode_ci");
        assert_eq!(isolation, TransactionIsolation::RepeatableRead);
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_connect_reports_invalid_advanced_session_setting() {
        let mut info = mysql_test_connection_info_from_env();
        info.advanced.mysql_collation = "utf8mb4_not_a_real_ci".to_string();

        let mut connection = DatabaseConnection::new();
        let err = connection
            .connect(info)
            .expect_err("invalid collation should fail connection setup");

        assert!(err.contains("Failed to apply MySQL session setting"));
        assert!(err.contains("SET NAMES"));
    }

    #[test]
    #[ignore = "requires MySQL or MariaDB TLS config via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_ssl_required_connects_when_server_tls_is_configured() {
        let mut info = mysql_test_connection_info_from_env();
        info.advanced.ssl_mode = ConnectionSslMode::Required;
        if let Ok(ca_path) = std::env::var("SPACE_QUERY_TEST_MYSQL_SSL_CA") {
            info.advanced.ssl_mode = ConnectionSslMode::VerifyCa;
            info.advanced.mysql_ssl_ca_path = ca_path;
        }

        let mut connection = DatabaseConnection::new();
        connection
            .connect(info)
            .expect("MySQL/MariaDB TLS connection should succeed");
        let conn = connection
            .get_mysql_connection_mut()
            .expect("MySQL connection should be live");
        let ssl_cipher = conn
            .query_first::<(String, String), _>("SHOW STATUS LIKE 'Ssl_cipher'")
            .expect("read SSL cipher")
            .map(|(_, value)| value)
            .unwrap_or_default();

        assert!(!ssl_cipher.is_empty());
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_connect_sets_read_committed_as_default_transaction_isolation() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(mysql_test_connection_info_from_env())
            .expect("MySQL/MariaDB connection should succeed");

        assert_eq!(
            connection.default_transaction_isolation(),
            TransactionIsolation::ReadCommitted
        );
        assert_eq!(connection.transaction_mode(), TransactionMode::default());
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_transaction_mode_applies_every_supported_isolation() {
        let mut connection = DatabaseConnection::new();
        connection
            .connect(mysql_test_connection_info_from_env())
            .expect("MySQL/MariaDB connection should succeed");
        let conn = connection
            .get_mysql_connection_mut()
            .expect("MySQL connection should be live");

        for isolation in [
            TransactionIsolation::ReadUncommitted,
            TransactionIsolation::ReadCommitted,
            TransactionIsolation::RepeatableRead,
            TransactionIsolation::Serializable,
        ] {
            DatabaseConnection::apply_mysql_transaction_mode(
                conn,
                TransactionMode::new(isolation, TransactionAccessMode::ReadWrite),
            )
            .unwrap_or_else(|err| {
                panic!("MySQL/MariaDB should apply {}: {err}", isolation.label())
            });

            let observed = DatabaseConnection::read_mysql_default_transaction_isolation(conn)
                .expect("read MySQL/MariaDB transaction isolation")
                .expect("MySQL/MariaDB should report a transaction isolation");
            assert_eq!(observed, isolation);
        }
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_read_only_transaction_mode_blocks_dml() {
        let mut connection = DatabaseConnection::new();
        connection
            .set_auto_commit(true)
            .expect("set initial MySQL/MariaDB auto-commit");
        connection
            .connect(mysql_test_connection_info_from_env())
            .expect("MySQL/MariaDB connection should succeed");
        let conn = connection
            .get_mysql_connection_mut()
            .expect("MySQL connection should be live");

        let _ = conn.query_drop("DROP TABLE IF EXISTS qt_tx_mode_probe_mysql");
        conn.query_drop("CREATE TABLE qt_tx_mode_probe_mysql (id INT)")
            .expect("create transaction mode probe table");

        DatabaseConnection::apply_mysql_transaction_mode(
            conn,
            TransactionMode::new(
                TransactionIsolation::ReadCommitted,
                TransactionAccessMode::ReadOnly,
            ),
        )
        .expect("MySQL/MariaDB read-only mode should apply");

        let insert_err = conn
            .query_drop("INSERT INTO qt_tx_mode_probe_mysql (id) VALUES (1)")
            .expect_err("read-only transaction should reject DML");
        let insert_message = insert_err.to_string();
        assert!(
            insert_message.to_ascii_lowercase().contains("read only")
                || insert_message.contains("1792"),
            "unexpected MySQL/MariaDB read-only DML error: {insert_message}"
        );

        let _ = conn.query_drop("ROLLBACK");
        let _ = conn.query_drop("SET SESSION TRANSACTION READ WRITE");
        let _ = conn.query_drop("DROP TABLE IF EXISTS qt_tx_mode_probe_mysql");
    }

    #[test]
    fn retained_scope_matches_target_only_when_scope_is_known_and_equal() {
        assert!(retained_scope_matches_target(
            DatabaseType::MariaDB,
            Some(" test "),
            "test"
        ));
        assert!(retained_scope_matches_target(
            DatabaseType::MySQL,
            Some("test"),
            "test"
        ));
        assert!(retained_scope_matches_target(
            DatabaseType::Oracle,
            Some("HR"),
            "HR"
        ));

        assert!(!retained_scope_matches_target(
            DatabaseType::MariaDB,
            None,
            "test"
        ));
        assert!(!retained_scope_matches_target(
            DatabaseType::MySQL,
            Some("test"),
            "other"
        ));
        assert!(!retained_scope_matches_target(
            DatabaseType::Oracle,
            Some("HR"),
            "SYS"
        ));
    }

    #[test]
    fn architecture_mismatch_detection_identifies_x86_client_on_arm_runtime() {
        let err = "DPI-1047: Cannot locate a 64-bit Oracle Client library: \"dlopen(libclntsh.dylib, 0x0001): tried: '/opt/homebrew/libclntsh.dylib' (mach-o file, but is an incompatible architecture (have 'x86_64', need 'arm64'))\"";
        assert!(is_oracle_client_architecture_mismatch(err));
    }

    #[test]
    fn formatted_init_error_adds_actionable_architecture_hint() {
        let err = OracleError::new(
            OracleErrorKind::InternalError,
            "DPI-1047: incompatible architecture (have 'x86_64', need 'arm64')".to_string(),
        );
        let message = format_oracle_client_init_error(&err);
        assert!(message.contains("CPU architecture mismatch"));
        assert!(message.contains("ORACLE_CLIENT_LIB_DIR"));
    }
}
