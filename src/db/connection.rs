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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};
use tns_thin::exec::{OracleValue, StatementRequest};
use tns_thin::pool::{PoolOptions as OracleThinPoolOptions, PooledThinConnection};
use tns_thin::{ConnectTarget, OracleThinConfig, OracleThinSession, OracleThinSessionPool};

use crate::db::session::SessionState;
use crate::db::session_policy::{
    retained_session_state_preflight_decision, RetainedSessionPreflightAction,
    RetainedSessionPreflightDecision,
};
use crate::db::transaction::{
    RetainedSessionState, TransactionAccessMode, TransactionIsolation, TransactionMode,
    TransactionProbeResult, TransactionSessionState,
};
use crate::utils::config::{
    DEFAULT_CONNECTION_POOL_SIZE, MAX_CONNECTION_POOL_SIZE, MIN_CONNECTION_POOL_SIZE,
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

pub(crate) fn discard_mysql_pooled_connection(conn: mysql::PooledConn) {
    drop(mysql::PooledConn::unwrap(conn));
}

/// Route oracle_thin connect/auth phase events into the app log so the user
/// can see exactly where a connect attempt stalls (especially useful for
/// legacy protocol 314 servers where a TCP read can time out silently).
fn ensure_oracle_thin_connect_logger_installed() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = tns_thin::set_connect_phase_logger(Box::new(|phase, detail| {
            let message = if detail.is_empty() {
                phase.to_string()
            } else {
                format!("{phase} | {detail}")
            };
            logging::log_info("oracle_thin/connect", &message);
        }));
    });
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
        if self.default_transaction_access_mode == TransactionAccessMode::ReadOnly
            && self.default_transaction_isolation != TransactionIsolation::Default
        {
            return Err(
                "Oracle does not support combining READ ONLY with an explicit transaction isolation level"
                    .to_string(),
            );
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
            // write_volatile prevents the compiler from optimizing away the zeroing.
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
            debug_oracle_thin_protocol_version: None,
        }
    }

    pub fn connection_string(&self) -> String {
        backend_for(self.db_type).connection_string(self)
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
    Oracle(Connection),
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
    lease: DbSessionLease,
    retained_state: RetainedSessionState,
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

pub enum RetainedSessionTakeOutcome {
    NoSession,
    Reusable(Box<TakenDbSessionLease>),
    DiscardedBecauseStale,
    BlockedContextMismatch(RetainedSessionState),
}

/// One editor tab's owned DB session slot.
///
/// Oracle and MySQL/MariaDB both use this same lifecycle: take the lease for
/// execution, retain it in the tab slot after cleanup, and clear it on close,
/// disconnect, cancel, or stale connection generation.
#[derive(Clone, Default)]
pub struct SharedDbSessionLease {
    inner: Arc<Mutex<Option<DbSessionLeaseEntry>>>,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PooledSessionLeaseSnapshot {
    pub db_type: DatabaseType,
    pub pool_context_epoch: u64,
    pub transaction_state: TransactionSessionState,
    pub retained_state: RetainedSessionState,
}

impl PooledSessionLeaseSnapshot {
    pub fn transaction_state(self) -> TransactionSessionState {
        self.transaction_state
    }

    pub fn retained_state(self) -> RetainedSessionState {
        self.retained_state
    }
}

#[derive(Clone)]
pub struct DbPoolSessionContext {
    pub connection_generation: u64,
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

    pub fn acquire_session_for_current_scope(&self) -> Result<DbPoolSession, String> {
        self.ensure_current()?;
        let mut session = self.pool.acquire_session()?;
        if let Err(err) = self.ensure_current() {
            Self::discard_stale_session(session);
            return Err(err);
        }
        if let Err(err) = self.apply_current_scope_to_session(&mut session) {
            Self::discard_stale_session(session);
            return Err(err);
        }
        if let Err(err) = self.ensure_current() {
            Self::discard_stale_session(session);
            return Err(err);
        }
        Ok(session)
    }

    pub fn apply_current_scope_to_session(
        &self,
        session: &mut DbPoolSession,
    ) -> Result<(), String> {
        backend_for(self.connection_info.db_type).apply_current_scope_to_session(self, session)
    }

    fn discard_stale_session(session: DbPoolSession) {
        match session {
            DbPoolSession::OracleThin(conn) => {
                let conn = *conn;
                conn.discard();
            }
            DbPoolSession::Oracle(_) | DbPoolSession::MySQL { .. } => {}
        }
    }
}

impl DbConnectionPool {
    pub fn acquire_session(&self) -> Result<DbPoolSession, String> {
        let mut session = match self {
            DbConnectionPool::Oracle { pool, .. } => DbPoolSession::Oracle(
                pool.get()
                    .map_err(|err| Self::format_oracle_pool_acquire_error(pool, &err))?,
            ),
            DbConnectionPool::OracleThin { pool, .. } => DbPoolSession::OracleThin(Box::new(
                pool.acquire()
                    .map_err(|err| Self::format_oracle_thin_pool_acquire_error(&err))?,
            )),
            DbConnectionPool::MySQL { pool, db_type, .. } => DbPoolSession::MySQL {
                conn: pool
                    .try_get_conn(POOL_SESSION_ACQUIRE_TIMEOUT)
                    .map_err(|err| Self::format_mysql_pool_acquire_error(&err))?,
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

    fn format_mysql_pool_acquire_error(err: &mysql::Error) -> String {
        let message = err.to_string();
        let looks_pool_exhausted =
            matches!(err, mysql::Error::DriverError(mysql::DriverError::Timeout));
        if !looks_pool_exhausted {
            return message;
        }

        format!(
            "{}. MySQL connection pool appears exhausted. Finish or cancel lazy fetches in other result tabs, close unused query tabs, or increase Settings > Connection pool size.",
            message
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
            DbPoolSession::Oracle(conn) => DbSessionLease::Oracle(Arc::new(conn)),
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
    ) -> Self {
        Self {
            owner,
            connection_generation,
            pool_context_epoch,
            lease: Some(lease),
            retained_state,
        }
    }

    pub fn retained_state(&self) -> RetainedSessionState {
        self.retained_state
    }

    pub fn pool_context_epoch(&self) -> u64 {
        self.pool_context_epoch
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
            self.owner.apply_retained_session_disposition(
                self.connection_generation,
                self.pool_context_epoch,
                lease,
                RetainedSessionDisposition::Retain(retained_state),
                "db::session_lease",
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
            self.owner.apply_retained_session_disposition(
                self.connection_generation,
                pool_context_epoch,
                lease,
                RetainedSessionDisposition::Retain(retained_state),
                "db::session_lease",
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
    ) -> Self {
        Self {
            connection_generation,
            pool_context_epoch,
            lease,
            retained_state,
        }
    }

    fn matches_connection(&self, connection_generation: u64, db_type: DatabaseType) -> bool {
        self.connection_generation == connection_generation && self.lease.is_db_type(db_type)
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

    fn discard_physical(self, log_context: &str) {
        self.lease.discard_physical(log_context);
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
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
        }
    }

    pub fn clear(&self) -> bool {
        let lease_to_drop = {
            self.inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
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
            .as_ref()
            .map(|entry| PooledSessionLeaseSnapshot {
                db_type: entry.lease.db_type(),
                pool_context_epoch: entry.pool_context_epoch,
                transaction_state: entry.retained_state.summary_transaction_state(),
                retained_state: entry.retained_state,
            })
    }

    fn take_reusable_lease_matching_connection(
        &self,
        connection_generation: u64,
        db_type: DatabaseType,
    ) -> Option<TakenDbSessionLease> {
        let mut stale_lease_to_drop = None;
        let reusable_lease = {
            let mut lease = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let reusable = lease.as_ref().is_some_and(|existing| {
                existing.matches_connection(connection_generation, db_type)
            });
            if reusable {
                lease.take().map(|entry| {
                    TakenDbSessionLease::new_with_retained_state(
                        self.clone(),
                        connection_generation,
                        entry.pool_context_epoch,
                        entry.lease,
                        entry.retained_state,
                    )
                })
            } else {
                if lease.is_some() {
                    stale_lease_to_drop = lease.take();
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
    ) -> Option<TakenDbSessionLease> {
        self.take_reusable_lease_matching_connection(connection_generation, db_type)
    }

    pub fn take_reusable_lease_for_resolution(
        &self,
        connection_generation: u64,
        db_type: DatabaseType,
    ) -> Option<TakenDbSessionLease> {
        self.take_reusable_lease_matching_connection(connection_generation, db_type)
    }

    pub fn take_reusable_lease(
        &self,
        connection_generation: u64,
        pool_context_epoch: u64,
        db_type: DatabaseType,
    ) -> RetainedSessionTakeOutcome {
        let mut stale_lease_to_drop = None;
        let reusable_lease = {
            let mut lease = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(existing) = lease.as_ref() else {
                return RetainedSessionTakeOutcome::NoSession;
            };
            if !existing.matches_connection(connection_generation, db_type) {
                stale_lease_to_drop = lease.take();
                None
            } else if retained_lease_context_decision(
                existing.matches_context(connection_generation, pool_context_epoch, db_type),
                existing.retained_state,
            ) == RetainedLeaseContextDecision::Reusable
            {
                lease.take().map(|entry| {
                    let restore_epoch = if entry.pool_context_epoch == pool_context_epoch {
                        entry.pool_context_epoch
                    } else {
                        pool_context_epoch
                    };
                    TakenDbSessionLease::new_with_retained_state(
                        self.clone(),
                        connection_generation,
                        restore_epoch,
                        entry.lease,
                        entry.retained_state,
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
            let mut lease = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let should_clear = lease.as_ref().is_some_and(|existing| {
                existing.connection_generation == connection_generation
                    && match &existing.lease {
                        DbSessionLease::Oracle(conn) => Arc::ptr_eq(conn, expected_conn),
                        DbSessionLease::OracleThin(_) | DbSessionLease::MySQL { .. } => false,
                    }
            });
            if should_clear {
                lease.take()
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
        match disposition {
            RetainedSessionDisposition::Retain(retained_state) => self
                .store_if_empty_with_retained_state(
                    connection_generation,
                    pool_context_epoch,
                    lease,
                    retained_state,
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
        let lease_db_type = lease_to_store.db_type();
        let mut lease_to_store = Some(lease_to_store);
        let old_lease_to_drop = {
            let mut lease = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let should_store = match lease.as_mut() {
                None => true,
                Some(existing) => {
                    if existing.connection_generation != connection_generation
                        || existing.pool_context_epoch != pool_context_epoch
                        || !existing.lease.is_db_type(lease_db_type)
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
                let old_lease = lease.take();
                if let Some(lease_to_store) = lease_to_store.take() {
                    *lease = Some(DbSessionLeaseEntry::new_with_retained_state(
                        connection_generation,
                        pool_context_epoch,
                        lease_to_store,
                        retained_state,
                    ));
                }
                old_lease
            } else {
                None
            }
        };
        if let Some(entry) = old_lease_to_drop {
            entry.discard_physical("db::session_lease");
        }
        if lease_to_store.is_some() {
            logging::log_warning(
                "db::session_lease",
                &format!(
                    "Discarded conflicting retained {} session for generation {} because an active retained session already exists",
                    lease_db_type, connection_generation
                ),
            );
            if let Some(lease_to_store) = lease_to_store.take() {
                lease_to_store.discard_physical("db::session_lease");
            }
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
    fn sql_dialect(&self) -> SqlDialect;
    fn supports_mysql_delimiter_commands(&self) -> bool;
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
    ) -> Result<(DbConnection, DbConnectionPool), String>;
    fn build_pool(&self, info: &ConnectionInfo, pool_size: u32)
        -> Result<DbConnectionPool, String>;
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
    fn test_connection(&self, info: &ConnectionInfo) -> Result<(), String>;
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
struct MysqlBackend {
    db_type: DatabaseType,
    display_name: &'static str,
    choice_label: &'static str,
    cache_key: u8,
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
    session_time_zone_in_range: mysql_session_time_zone_in_range,
    session_time_zone_error_message:
        "MySQL session time zone must be blank or an offset from -13:59 through +14:00",
};
static MARIADB_BACKEND: MysqlBackend = MysqlBackend {
    db_type: DatabaseType::MariaDB,
    display_name: "MariaDB",
    choice_label: "MariaDB",
    cache_key: 2,
    session_time_zone_in_range: mariadb_session_time_zone_in_range,
    session_time_zone_error_message:
        "MariaDB session time zone must be blank or an offset from -12:59 through +13:00",
};

pub(crate) fn backend_for(db_type: DatabaseType) -> &'static dyn DbBackend {
    match db_type {
        DatabaseType::Oracle => &ORACLE_BACKEND,
        DatabaseType::MySQL => &MYSQL_BACKEND,
        DatabaseType::MariaDB => &MARIADB_BACKEND,
    }
}

fn scope_values_match_exact(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.trim() == right.trim(),
        (None, None) => true,
        (Some(value), None) | (None, Some(value)) => value.trim().is_empty(),
    }
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
            default_service_name: "ORCL",
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

    fn sql_dialect(&self) -> SqlDialect {
        SqlDialect::Oracle
    }

    fn supports_mysql_delimiter_commands(&self) -> bool {
        false
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
    ) -> Result<(DbConnection, DbConnectionPool), String> {
        if info.advanced.oracle_driver_mode == OracleDriverMode::Thin {
            let config = DatabaseConnection::build_oracle_thin_config(info)?;
            let requested_minimum_protocol = config.connect_options.minimum_protocol_version;
            let requested_desired_protocol = config.connect_options.desired_protocol_version;
            let mut session = OracleThinSession::connect(config).map_err(|err| {
                eprintln!("Oracle thin connection error: {err}");
                err.to_string()
            })?;
            DatabaseConnection::log_oracle_thin_protocol_acceptance(
                &session,
                requested_minimum_protocol,
                requested_desired_protocol,
            );
            DatabaseConnection::apply_oracle_thin_session_settings(&mut session, &info.advanced)?;
            return Ok((
                DbConnection::OracleThin(Arc::new(Mutex::new(session))),
                self.build_pool(info, pool_size)?,
            ));
        }

        ensure_oracle_client_initialized().map_err(|e| e.to_string())?;
        let conn_str = info.connection_string();
        let connection = Arc::new(
            Connector::new(&info.username, &info.password, &conn_str)
                .connect()
                .map_err(|err| {
                    eprintln!("Connection error: {err}");
                    err.to_string()
                })?,
        );
        DatabaseConnection::apply_oracle_session_settings(connection.as_ref(), &info.advanced)?;
        Ok((
            DbConnection::Oracle(connection),
            self.build_pool(info, pool_size)?,
        ))
    }

    fn build_pool(
        &self,
        info: &ConnectionInfo,
        pool_size: u32,
    ) -> Result<DbConnectionPool, String> {
        if info.advanced.oracle_driver_mode == OracleDriverMode::Thin {
            return DatabaseConnection::build_oracle_thin_pool(info, pool_size).map(|pool| {
                DbConnectionPool::OracleThin {
                    pool: Arc::new(pool),
                    advanced: info.advanced.clone(),
                }
            });
        }

        DatabaseConnection::build_oracle_pool(info, pool_size).map(|pool| {
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
        match session {
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
        }
        .map_err(|err| format!("Failed to apply Oracle current schema: {err}"))
    }

    fn test_connection(&self, info: &ConnectionInfo) -> Result<(), String> {
        if info.advanced.oracle_driver_mode == OracleDriverMode::Thin {
            let config = DatabaseConnection::build_oracle_thin_config(info)?;
            let requested_minimum_protocol = config.connect_options.minimum_protocol_version;
            let requested_desired_protocol = config.connect_options.desired_protocol_version;
            let mut session = OracleThinSession::connect(config).map_err(|err| {
                eprintln!("Oracle thin connection error: {err}");
                err.to_string()
            })?;
            DatabaseConnection::log_oracle_thin_protocol_acceptance(
                &session,
                requested_minimum_protocol,
                requested_desired_protocol,
            );
            DatabaseConnection::apply_oracle_thin_session_settings(&mut session, &info.advanced)?;
            return Ok(());
        }

        ensure_oracle_client_initialized().map_err(|e| e.to_string())?;
        let conn_str = info.connection_string();
        let connection = Connector::new(&info.username, &info.password, &conn_str)
            .connect()
            .map_err(|err| {
                eprintln!("Connection error: {err}");
                err.to_string()
            })?;
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

    fn after_connect(&self, _connection: &mut DatabaseConnection) {}

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
            if mode.isolation != TransactionIsolation::Default {
                return Err(
                    "Oracle does not support combining READ ONLY with an explicit transaction isolation level"
                        .to_string(),
                );
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

    fn sql_dialect(&self) -> SqlDialect {
        SqlDialect::MySql
    }

    fn supports_mysql_delimiter_commands(&self) -> bool {
        match self.db_type {
            DatabaseType::MySQL | DatabaseType::MariaDB => true,
            DatabaseType::Oracle => unreachable!("Oracle does not use MysqlBackend"),
        }
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
        auto_commit: bool,
    ) -> Result<(DbConnection, DbConnectionPool), String> {
        let opts = DatabaseConnection::build_mysql_opts(info);
        let mut conn = mysql::Conn::new(opts).map_err(|err| {
            eprintln!("MySQL connection error: {err}");
            err.to_string()
        })?;
        DatabaseConnection::apply_mysql_session_settings(&mut conn, &info.advanced)?;
        DatabaseConnection::apply_mysql_autocommit_setting(&mut conn, auto_commit)?;
        Ok((
            DbConnection::MySQL {
                conn,
                db_type: self.db_type,
            },
            self.build_pool(info, pool_size)?,
        ))
    }

    fn build_pool(
        &self,
        info: &ConnectionInfo,
        pool_size: u32,
    ) -> Result<DbConnectionPool, String> {
        DatabaseConnection::build_mysql_pool(info, pool_size).map(|pool| DbConnectionPool::MySQL {
            pool,
            advanced: info.advanced.clone(),
            db_type: self.db_type,
        })
    }

    fn apply_pool_session_settings(
        &self,
        session: &mut DbPoolSession,
        advanced: &ConnectionAdvancedSettings,
    ) -> Result<(), String> {
        let DbPoolSession::MySQL { conn, .. } = session else {
            return Err(format!(
                "Expected MySQL pool session but acquired {}",
                session.db_type()
            ));
        };
        DatabaseConnection::apply_mysql_session_settings(conn, advanced)
    }

    fn apply_current_scope_to_session(
        &self,
        context: &DbPoolSessionContext,
        session: &mut DbPoolSession,
    ) -> Result<(), String> {
        let DbPoolSession::MySQL { conn, .. } = session else {
            return Err(format!(
                "Expected MySQL pool session but acquired {}",
                session.db_type()
            ));
        };
        let current_database = context.current_service_name.trim();
        if current_database.is_empty() {
            DatabaseConnection::reset_mysql_session_to_no_database(conn.as_mut())?;
            DatabaseConnection::apply_mysql_session_settings(
                conn,
                &context.connection_info.advanced,
            )
            .map_err(|err| {
                format!("Failed to reapply MySQL session settings after database reset: {err}")
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
            format!("Failed to apply MySQL current database `{current_database}`: {err}")
        })?;
        DatabaseConnection::apply_mysql_connection_encoding_with_settings(
            conn,
            &context.connection_info.advanced,
        )
        .map_err(|err| {
            format!("Failed to refresh MySQL session encoding after database switch: {err}")
        })?;
        DatabaseConnection::apply_mysql_session_transaction_options(
            conn,
            context.auto_commit,
            context.transaction_mode,
            context.connection_info.db_type,
            context.default_transaction_isolation,
        )
    }

    fn test_connection(&self, info: &ConnectionInfo) -> Result<(), String> {
        let opts = DatabaseConnection::build_mysql_opts(info);
        let mut conn = mysql::Conn::new(opts).map_err(|err| {
            eprintln!("MySQL connection error: {err}");
            err.to_string()
        })?;
        DatabaseConnection::apply_mysql_session_settings(&mut conn, &info.advanced)?;
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
        let DbSessionLease::MySQL { conn, .. } = lease else {
            return Err(format!(
                "Expected MySQL retained session but found {}",
                lease.db_type()
            ));
        };
        let target_scope = target_scope.trim();
        if target_scope.is_empty() {
            if preserve_existing_session_state {
                return Err(
                    DatabaseConnection::mysql_empty_scope_requires_resolved_session_error(),
                );
            }
            DatabaseConnection::reset_mysql_session_to_no_database(conn.as_mut())?;
            return DatabaseConnection::apply_mysql_session_settings(conn, advanced);
        }
        conn.as_mut()
            .select_db(target_scope)
            .map_err(|err| err.to_string())?;
        if preserve_existing_session_state {
            return Ok(());
        }
        DatabaseConnection::apply_mysql_connection_encoding_with_settings(conn, advanced)
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
            eprintln!("Warning: failed to sync MySQL current database after connect: {err}");
        }
    }

    fn apply_auto_commit(
        &self,
        connection: &mut DbConnection,
        enabled: bool,
    ) -> Result<(), String> {
        match connection {
            DbConnection::MySQL { conn, .. } => {
                DatabaseConnection::apply_mysql_autocommit_setting(conn, enabled)
            }
            unexpected @ (DbConnection::Oracle(_) | DbConnection::OracleThin(_)) => Err(format!(
                "Expected MySQL live connection but found {}",
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
            Some(DbConnection::MySQL { conn, .. }) => {
                DatabaseConnection::read_mysql_default_transaction_isolation(conn)
            }
            Some(unexpected @ (DbConnection::Oracle(_) | DbConnection::OracleThin(_))) => {
                Err(format!(
                    "Expected MySQL live connection but found {}",
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
            Some(DbConnection::MySQL { conn, .. }) => {
                DatabaseConnection::apply_mysql_transaction_mode_for_db_with_default(
                    conn,
                    mode,
                    self.db_type,
                    default_isolation,
                )
            }
            Some(unexpected @ (DbConnection::Oracle(_) | DbConnection::OracleThin(_))) => {
                Err(format!(
                    "Expected MySQL live connection but found {}",
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
    connection_pool_size: u32,
}

impl DatabaseConnection {
    fn clamp_connection_pool_size(size: u32) -> u32 {
        size.clamp(MIN_CONNECTION_POOL_SIZE, MAX_CONNECTION_POOL_SIZE)
    }

    fn build_mysql_opts(info: &ConnectionInfo) -> mysql::OptsBuilder {
        Self::build_mysql_opts_with_pool_size(info, None)
    }

    pub(crate) fn build_mysql_opts_without_database(info: &ConnectionInfo) -> mysql::OptsBuilder {
        Self::build_mysql_opts_with_pool_size_and_database(info, None, false)
    }

    fn build_mysql_opts_with_pool_size(
        info: &ConnectionInfo,
        pool_size: Option<u32>,
    ) -> mysql::OptsBuilder {
        Self::build_mysql_opts_with_pool_size_and_database(info, pool_size, true)
    }

    fn build_mysql_pool_opts(info: &ConnectionInfo, pool_size: u32) -> mysql::OptsBuilder {
        Self::build_mysql_opts_with_pool_size_and_database(info, Some(pool_size), false)
    }

    fn build_mysql_opts_with_pool_size_and_database(
        info: &ConnectionInfo,
        pool_size: Option<u32>,
        include_database: bool,
    ) -> mysql::OptsBuilder {
        let mut opts = mysql::OptsBuilder::new()
            .ip_or_hostname(Some(&info.host))
            .tcp_port(info.port)
            .user(Some(&info.username))
            .pass(Some(&info.password))
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
    ) -> Result<oracle::pool::Pool, String> {
        let conn_str = info.connection_string();
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

    fn build_oracle_thin_config(info: &ConnectionInfo) -> Result<OracleThinConfig, String> {
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
    ) -> Result<OracleThinSessionPool, String> {
        let config = Self::build_oracle_thin_config(info)?;
        let options = OracleThinPoolOptions {
            max_size: Self::clamp_connection_pool_size(pool_size) as usize,
            acquire_timeout: POOL_SESSION_ACQUIRE_TIMEOUT,
        };
        Ok(OracleThinSessionPool::new(config, options))
    }

    fn build_mysql_pool(info: &ConnectionInfo, pool_size: u32) -> Result<mysql::Pool, String> {
        let opts = Self::build_mysql_pool_opts(info, pool_size);
        mysql::Pool::new(opts).map_err(|err| err.to_string())
    }

    fn build_pool_for_info(
        info: &ConnectionInfo,
        pool_size: u32,
    ) -> Result<DbConnectionPool, String> {
        backend_for(info.db_type).build_pool(info, pool_size)
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
            connection_pool_size: DEFAULT_CONNECTION_POOL_SIZE,
        }
    }

    fn bump_pool_context_epoch(&self) {
        self.pool_context_epoch.fetch_add(1, Ordering::AcqRel);
    }

    fn current_pool_context_epoch(&self) -> u64 {
        self.pool_context_epoch.load(Ordering::Acquire)
    }

    pub fn connect(&mut self, info: ConnectionInfo) -> Result<(), String> {
        info.advanced
            .validate_for_db(info.db_type, info.uses_oracle_tns_alias())?;
        let (db_conn, pool) = backend_for(info.db_type).connect(
            &info,
            self.connection_pool_size,
            self.auto_commit,
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
        self.connection_generation = self.connection_generation.wrapping_add(1);
        self.bump_pool_context_epoch();

        // Keep SessionState::reset() backend-preserving for same-DB resets;
        // successful connection transitions must explicitly stamp the new
        // backend here so delimiter/bind scanning follows the live database.
        match self.session.lock() {
            Ok(mut guard) => guard.db_type = db_type,
            Err(poisoned) => poisoned.into_inner().db_type = db_type,
        }

        Ok(())
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

    pub(crate) fn apply_mysql_session_settings<C: Queryable>(
        conn: &mut C,
        advanced: &ConnectionAdvancedSettings,
    ) -> Result<(), String> {
        Self::validate_mysql_session_time_zone_for_server(conn, advanced.session_time_zone.trim())?;
        let statements = Self::mysql_session_setting_statements(advanced);

        for statement in statements {
            if let Err(err) = conn.query_drop(statement.as_str()) {
                return Err(format!(
                    "Failed to apply MySQL session setting `{statement}`: {err}"
                ));
            }
        }

        Self::apply_mysql_connection_encoding_with_settings(conn, advanced)
    }

    pub(crate) fn reset_mysql_session_to_no_database(conn: &mut mysql::Conn) -> Result<(), String> {
        conn.change_user(mysql::ChangeUserOpts::new().with_db_name(None))
            .map_err(|err| format!("Failed to reset MySQL session database scope: {err}"))
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

    fn mysql_session_setting_statements(advanced: &ConnectionAdvancedSettings) -> Vec<String> {
        let mut statements = Vec::new();
        statements.push(format!(
            "SET SESSION sql_mode = '{}'",
            advanced.mysql_sql_mode.trim()
        ));
        let time_zone = advanced.session_time_zone.trim();
        if !time_zone.is_empty() {
            statements.push(format!("SET SESSION time_zone = '{time_zone}'"));
        }
        if let Some(level) = advanced.default_transaction_isolation.sql_level() {
            statements.push(format!("SET SESSION TRANSACTION ISOLATION LEVEL {level}"));
        }
        statements
    }

    pub(crate) fn apply_mysql_connection_encoding_with_settings<C: Queryable>(
        conn: &mut C,
        advanced: &ConnectionAdvancedSettings,
    ) -> Result<(), String> {
        let database_collation = Self::mysql_current_database_collation(conn);
        let statement =
            Self::mysql_set_names_statement_with_settings(database_collation.as_deref(), advanced);

        if let Err(err) = conn.query_drop(statement.as_str()) {
            return Err(format!(
                "Failed to apply MySQL session setting `{statement}`: {err}"
            ));
        }
        Ok(())
    }

    fn mysql_current_database_collation<C: Queryable>(conn: &mut C) -> Option<String> {
        match conn.query_first::<String, _>(
            "SELECT DEFAULT_COLLATION_NAME \
             FROM INFORMATION_SCHEMA.SCHEMATA \
             WHERE SCHEMA_NAME = DATABASE()",
        ) {
            Ok(Some(collation)) => return Some(collation.trim().to_string()),
            Ok(None) => {}
            Err(err) => {
                eprintln!(
                    "Warning: failed to read MySQL current database collation for session setup: {err}"
                );
            }
        }

        match conn.query_first::<String, _>("SELECT @@collation_database") {
            Ok(value) => value.map(|collation| collation.trim().to_string()),
            Err(err) => {
                eprintln!(
                    "Warning: failed to read MySQL database collation for session setup: {err}"
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

    fn apply_mysql_autocommit_setting<C: Queryable>(
        conn: &mut C,
        enabled: bool,
    ) -> Result<(), String> {
        let statement = if enabled {
            "SET autocommit = 1"
        } else {
            "SET autocommit = 0"
        };

        conn.query_drop(statement)
            .map_err(|err| format!("Failed to apply MySQL autocommit setting `{statement}`: {err}"))
    }

    pub(crate) fn apply_mysql_session_transaction_options<C: Queryable>(
        conn: &mut C,
        auto_commit: bool,
        transaction_mode: TransactionMode,
        db_type: DatabaseType,
        default_transaction_isolation: TransactionIsolation,
    ) -> Result<(), String> {
        Self::apply_mysql_autocommit_setting(conn, auto_commit)?;
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
    ) -> TransactionProbeResult {
        match conn.query_first::<u64, _>(Self::mysql_session_transaction_probe_sql()) {
            Ok(Some(value)) => TransactionProbeResult {
                may_have_uncommitted_work: value != 0,
                used_fallback: false,
            },
            Ok(None) => TransactionProbeResult {
                may_have_uncommitted_work: false,
                used_fallback: false,
            },
            Err(primary_err) => {
                match conn.query_first::<u64, _>(Self::mysql_innodb_transaction_probe_sql()) {
                    Ok(Some(value)) => TransactionProbeResult {
                        may_have_uncommitted_work: value != 0,
                        used_fallback: false,
                    },
                    Ok(None) => TransactionProbeResult {
                        may_have_uncommitted_work: false,
                        used_fallback: false,
                    },
                    Err(fallback_err) => {
                        logging::log_error(
                            log_context,
                            &format!(
                                "Failed to inspect MySQL session transaction state: {primary_err}; fallback probe failed: {fallback_err}"
                            ),
                        );
                        TransactionProbeResult {
                            may_have_uncommitted_work: fallback_on_error,
                            used_fallback: true,
                        }
                    }
                }
            }
        }
    }

    fn mysql_session_transaction_probe_sql() -> &'static str {
        "SELECT @@in_transaction"
    }

    fn mysql_innodb_transaction_probe_sql() -> &'static str {
        "\
            SELECT COUNT(*) \
            FROM information_schema.innodb_trx \
            WHERE trx_mysql_thread_id = CONNECTION_ID()"
    }

    pub(crate) fn mysql_session_may_have_uncommitted_work<C: Queryable>(
        conn: &mut C,
        log_context: &str,
        fallback_on_error: bool,
    ) -> bool {
        Self::mysql_session_uncommitted_work_probe(conn, log_context, fallback_on_error)
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
            Some(DbConnection::MySQL { conn, .. }) => TransactionSessionState::from_flags(
                Self::mysql_session_may_have_uncommitted_work(conn, log_context, true),
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
        if let Some(pool) = &self.pool {
            pool.close();
        }
        self.connection = None;
        self.pool = None;
        self.connected = false;
        self.last_disconnect_reason = disconnect_reason;
        self.info.clear_password();
        self.info = ConnectionInfo::default();
        ConnectionInfo::clear_secret(&mut self.session_password);
        self.oracle_current_schema = None;
        self.auto_commit = false;
        self.transaction_mode = TransactionMode::default();
        self.default_transaction_isolation = TransactionIsolation::Default;
        match self.session.lock() {
            Ok(mut guard) => {
                guard.reset();
                guard.db_type = DatabaseType::default();
            }
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                guard.reset();
                guard.db_type = DatabaseType::default();
            }
        }
        if had_connection {
            self.connection_generation = self.connection_generation.wrapping_add(1);
            self.bump_pool_context_epoch();
        }
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

    pub fn acquire_pool_session(&self) -> Result<Option<DbPoolSession>, String> {
        let mut session = self
            .pool
            .as_ref()
            .map(DbConnectionPool::acquire_session)
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
        let pool = Self::build_pool_for_info(&info, size)?;
        self.pool = Some(pool);
        self.connection_pool_size = size;
        self.connection_generation = self.connection_generation.wrapping_add(1);
        self.bump_pool_context_epoch();
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
            return Err("Expected MySQL-family connection but none is active".to_string());
        }

        match self.info.db_type.backend_kind() {
            DatabaseBackendKind::MySql => Ok(()),
            DatabaseBackendKind::Oracle => Err(format!(
                "Expected MySQL-family connection but {} is active",
                self.info.db_type
            )),
        }
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

    pub(crate) fn transaction_mode_statements_for_with_default(
        db_type: DatabaseType,
        mode: TransactionMode,
        default_isolation: TransactionIsolation,
    ) -> Result<Vec<String>, String> {
        let mysql_family = match db_type.backend_kind() {
            DatabaseBackendKind::MySql => true,
            DatabaseBackendKind::Oracle => false,
        };
        let mode = if mysql_family
            && mode.isolation == TransactionIsolation::Default
            && default_isolation != TransactionIsolation::Default
        {
            TransactionMode::new(default_isolation, mode.access_mode)
        } else {
            mode
        };
        Self::transaction_mode_statements_for(db_type, mode)
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
        Self::apply_oracle_current_schema(conn, self.oracle_current_schema.as_deref())
    }

    pub fn apply_tracked_mysql_current_database(&mut self) -> Result<(), String> {
        self.ensure_connected_mysql_family()?;

        let target_database = self.info.service_name.trim().to_string();
        let advanced = self.info.advanced.clone();
        let Some(conn) = self.get_mysql_connection_mut() else {
            return Err("Expected MySQL connection but none is active".to_string());
        };

        if target_database.is_empty() {
            Self::reset_mysql_session_to_no_database(conn)?;
            return Self::apply_mysql_session_settings(conn, &advanced);
        }

        conn.select_db(target_database.as_str())
            .map_err(|err| err.to_string())?;
        Self::apply_mysql_connection_encoding_with_settings(conn, &advanced)
    }

    pub fn sync_mysql_current_database_name(&mut self) -> Result<String, String> {
        self.ensure_connected_mysql_family()?;

        let advanced = self.info.advanced.clone();
        let Some(conn) = self.get_mysql_connection_mut() else {
            return Err("Expected MySQL connection but none is active".to_string());
        };

        let current_database = conn
            .query_first::<Option<String>, _>("SELECT DATABASE()")
            .map_err(|err| err.to_string())?
            .flatten()
            .map(|database| database.trim().to_string())
            .unwrap_or_default();
        Self::apply_mysql_connection_encoding_with_settings(conn, &advanced)?;
        if self.info.service_name != current_database {
            self.info.service_name = current_database.clone();
            self.bump_pool_context_epoch();
        }
        Ok(current_database)
    }

    pub fn sync_mysql_current_database_name_from_session<C: Queryable>(
        &mut self,
        conn: &mut C,
        refresh_encoding: bool,
    ) -> Result<String, String> {
        self.ensure_connected_mysql_family()?;

        let advanced = self.info.advanced.clone();
        let current_database = conn
            .query_first::<Option<String>, _>("SELECT DATABASE()")
            .map_err(|err| err.to_string())?
            .flatten()
            .map(|database| database.trim().to_string())
            .unwrap_or_default();
        if refresh_encoding {
            Self::apply_mysql_connection_encoding_with_settings(conn, &advanced)?;
        }
        let Some(primary_conn) = self.get_mysql_connection_mut() else {
            return Err("Expected MySQL connection but none is active".to_string());
        };
        if current_database.is_empty() {
            Self::reset_mysql_session_to_no_database(primary_conn)?;
        } else {
            primary_conn
                .select_db(current_database.as_str())
                .map_err(|err| err.to_string())?;
        }
        Self::apply_mysql_connection_encoding_with_settings(primary_conn, &advanced)?;
        if self.info.service_name != current_database {
            self.info.service_name = current_database.clone();
            self.bump_pool_context_epoch();
        }
        Ok(current_database)
    }

    pub fn sync_mysql_current_database_name_from_known_name(
        &mut self,
        current_database: &str,
    ) -> Result<String, String> {
        self.ensure_connected_mysql_family()?;

        let advanced = self.info.advanced.clone();
        let current_database = current_database.trim().to_string();
        let Some(primary_conn) = self.get_mysql_connection_mut() else {
            return Err("Expected MySQL connection but none is active".to_string());
        };
        if current_database.is_empty() {
            Self::reset_mysql_session_to_no_database(primary_conn)?;
        } else {
            primary_conn
                .select_db(current_database.as_str())
                .map_err(|err| err.to_string())?;
        }
        Self::apply_mysql_connection_encoding_with_settings(primary_conn, &advanced)?;
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

        let target_database = database.trim();
        let advanced = self.info.advanced.clone();
        let Some(conn) = self.get_mysql_connection_mut() else {
            return Err("Expected MySQL connection but none is active".to_string());
        };

        if target_database.is_empty() {
            Self::reset_mysql_session_to_no_database(conn)?;
            Self::apply_mysql_session_settings(conn, &advanced)?;
        } else {
            conn.select_db(target_database)
                .map_err(|err| err.to_string())?;
            Self::apply_mysql_connection_encoding_with_settings(conn, &advanced)?;
        }
        if self.info.service_name != target_database {
            self.info.service_name = target_database.to_string();
            self.bump_pool_context_epoch();
        }
        Ok(())
    }

    pub fn sync_oracle_current_schema_from_session(
        &mut self,
        conn: &Connection,
    ) -> Result<String, String> {
        self.ensure_connected_db_type(DatabaseType::Oracle)?;

        let current_schema = Self::read_oracle_current_schema(conn)?;
        self.set_tracked_oracle_current_schema(Some(current_schema.clone()));
        self.require_live_connection()?;

        Ok(current_schema)
    }

    pub fn sync_oracle_thin_current_schema_from_session(
        &mut self,
        session: &mut OracleThinSession,
    ) -> Result<String, String> {
        self.ensure_connected_db_type(DatabaseType::Oracle)?;

        let current_schema = Self::read_oracle_thin_current_schema(session)?;
        self.set_tracked_oracle_current_schema(Some(current_schema.clone()));
        match self.require_live_db_connection()? {
            DbConnection::OracleThin(conn) => {
                let mut primary_session = conn
                    .lock()
                    .map_err(|_| "Oracle Thin connection lock was poisoned".to_string())?;
                Self::apply_oracle_thin_current_schema(
                    &mut primary_session,
                    self.oracle_current_schema.as_deref(),
                )?;
            }
            DbConnection::Oracle(_) => {
                return Err(
                    "Expected Oracle Thin connection but found Oracle OCI connection".to_string(),
                );
            }
            DbConnection::MySQL { .. } => {
                return Err(
                    "Expected Oracle connection but found MySQL-family connection".to_string(),
                );
            }
        }

        Ok(current_schema)
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
        info.advanced
            .validate_for_db(info.db_type, info.uses_oracle_tns_alias())?;
        backend_for(info.db_type).test_connection(info)
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

pub type SharedConnection = Arc<Mutex<DatabaseConnection>>;

static ACTIVE_DB_ACTIVITY: OnceLock<Mutex<Vec<TrackedDbActivity>>> = OnceLock::new();
static DB_POOL_SESSION_CONTEXT_CACHE: OnceLock<Mutex<HashMap<usize, CachedDbPoolSessionContext>>> =
    OnceLock::new();
static NEXT_DB_ACTIVITY_ID: AtomicU64 = AtomicU64::new(1);
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

fn pool_context_cache_slot() -> &'static Mutex<HashMap<usize, CachedDbPoolSessionContext>> {
    DB_POOL_SESSION_CONTEXT_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_pool_context_cache() -> MutexGuard<'static, HashMap<usize, CachedDbPoolSessionContext>> {
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
    let key = shared_connection_cache_key(connection);
    let conn_guard = match activity {
        Some(activity) => try_lock_connection_with_activity(connection, activity),
        None => try_lock_connection(connection),
    };

    let Some(conn_guard) = conn_guard else {
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
}

#[derive(Clone, Debug)]
struct TrackedDbActivity {
    id: u64,
    activity: String,
    started_at: Instant,
    db_type: Option<DatabaseType>,
    kind: DbActivityKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DbActivitySnapshot {
    pub activity: String,
    pub started_at: Instant,
    pub db_type: Option<DatabaseType>,
}

pub struct DbActivityGuard {
    id: u64,
}

impl Drop for DbActivityGuard {
    fn drop(&mut self) {
        remove_db_activity(self.id);
    }
}

fn db_activity_slot() -> &'static Mutex<Vec<TrackedDbActivity>> {
    ACTIVE_DB_ACTIVITY.get_or_init(|| Mutex::new(Vec::new()))
}

fn lock_db_activities() -> MutexGuard<'static, Vec<TrackedDbActivity>> {
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
    kind: DbActivityKind,
) -> DbActivityGuard {
    let id = NEXT_DB_ACTIVITY_ID.fetch_add(1, Ordering::Relaxed);
    let mut guard = lock_db_activities();
    guard.push(TrackedDbActivity {
        id,
        activity,
        started_at: Instant::now(),
        db_type,
        kind,
    });
    DbActivityGuard { id }
}

fn remove_db_activity(id: u64) {
    let mut guard = lock_db_activities();
    guard.retain(|activity| activity.id != id);
}

pub fn track_pool_db_activity(
    activity: impl Into<String>,
    db_type: DatabaseType,
) -> DbActivityGuard {
    track_db_activity_entry(activity.into(), Some(db_type), DbActivityKind::PoolSession)
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

pub fn active_pool_db_activity_snapshots() -> Vec<DbActivitySnapshot> {
    let guard = lock_db_activities();
    guard
        .iter()
        .filter(|activity| activity.kind == DbActivityKind::PoolSession)
        .map(|activity| DbActivitySnapshot {
            activity: activity.activity.clone(),
            started_at: activity.started_at,
            db_type: activity.db_type,
        })
        .collect()
}

pub fn format_connection_busy_message() -> String {
    match current_connection_lock_activity() {
        Some(activity) => format!("Connection is busy. Current DB activity: {}", activity),
        None => "Connection is busy. Try again after the current operation finishes.".to_string(),
    }
}

pub fn clear_tracked_db_activity() {
    lock_db_activities().clear();
}

pub struct ConnectionLockGuard<'a> {
    guard: MutexGuard<'a, DatabaseConnection>,
    activity_guard: Option<DbActivityGuard>,
}

impl<'a> ConnectionLockGuard<'a> {
    fn with_activity(mut self, activity: String) -> Self {
        self.activity_guard = Some(track_db_activity_entry(
            activity,
            None,
            DbActivityKind::ConnectionLock,
        ));
        self
    }

    pub fn refresh_tracked_connection(&self) {}
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

impl<'a> Drop for ConnectionLockGuard<'a> {
    fn drop(&mut self) {
        self.activity_guard.take();
    }
}

pub fn create_shared_connection() -> SharedConnection {
    Arc::new(Mutex::new(DatabaseConnection::new()))
}

pub fn lock_connection(connection: &SharedConnection) -> ConnectionLockGuard<'_> {
    let guard = match connection.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            logging::log_warning(
                "db::connection",
                "database connection lock was poisoned; recovering",
            );
            poisoned.into_inner()
        }
    };
    ConnectionLockGuard {
        guard,
        activity_guard: None,
    }
}

pub fn lock_connection_with_activity(
    connection: &SharedConnection,
    activity: impl Into<String>,
) -> ConnectionLockGuard<'_> {
    lock_connection(connection).with_activity(activity.into())
}

/// Try to acquire the connection lock without blocking.
/// Returns None if the lock is already held (query is running).
pub fn try_lock_connection(connection: &SharedConnection) -> Option<ConnectionLockGuard<'_>> {
    match connection.try_lock() {
        Ok(guard) => Some(ConnectionLockGuard {
            guard,
            activity_guard: None,
        }),
        Err(std::sync::TryLockError::WouldBlock) => None,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            logging::log_warning(
                "db::connection",
                "database connection lock was poisoned; recovering",
            );
            Some(ConnectionLockGuard {
                guard: poisoned.into_inner(),
                activity_guard: None,
            })
        }
    }
}

pub fn try_lock_connection_with_activity(
    connection: &SharedConnection,
    activity: impl Into<String>,
) -> Option<ConnectionLockGuard<'_>> {
    match connection.try_lock() {
        Ok(guard) => Some(ConnectionLockGuard {
            guard,
            activity_guard: Some(track_db_activity_entry(
                activity.into(),
                None,
                DbActivityKind::ConnectionLock,
            )),
        }),
        Err(std::sync::TryLockError::WouldBlock) => None,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            logging::log_warning(
                "db::connection",
                "database connection lock was poisoned; recovering",
            );
            let guard = poisoned.into_inner();
            Some(ConnectionLockGuard {
                guard,
                activity_guard: Some(track_db_activity_entry(
                    activity.into(),
                    None,
                    DbActivityKind::ConnectionLock,
                )),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        ConnectionInfo::new_with_type(
            "local",
            &user,
            &password,
            &host,
            port,
            &database,
            DatabaseType::MySQL,
        )
    }

    fn db_activity_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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
        assert!(!sql.contains("trx_rows_modified"));
        assert!(!sql.contains("trx_rows_locked"));
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
        let pool = DatabaseConnection::build_mysql_pool(&connection_info, MIN_CONNECTION_POOL_SIZE)
            .expect("create test MySQL pool without opening a connection");
        DbPoolSessionContext {
            connection_generation: 1,
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

        let pool = DatabaseConnection::build_mysql_pool(&info, MIN_CONNECTION_POOL_SIZE)
            .expect("create MySQL pool");
        let db_pool = DbConnectionPool::MySQL {
            pool,
            advanced: info.advanced.clone(),
            db_type: info.db_type,
        };
        let mut session = db_pool
            .acquire_session()
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

        let err = match context.acquire_session_for_current_scope() {
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
        let opts = mysql::Opts::from(DatabaseConnection::build_mysql_opts(&info));

        assert_eq!(opts.get_db_name(), Some("initial_db"));
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
        let opts = mysql::Opts::from(DatabaseConnection::build_mysql_pool_opts(&info, 4));

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
    fn oracle_transaction_mode_rejects_read_only_with_explicit_isolation() {
        let mode = TransactionMode::new(
            TransactionIsolation::Serializable,
            TransactionAccessMode::ReadOnly,
        );

        let err = DatabaseConnection::transaction_mode_statements_for(DatabaseType::Oracle, mode)
            .expect_err("Oracle cannot combine read-only and explicit isolation");
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
        assert!(oracle.show_driver_mode);
        assert!(oracle.service_name_required);
        assert!(oracle.supports_tns_alias);
        let oracle_advanced = DatabaseType::Oracle.advanced_settings_form_spec();
        assert!(oracle_advanced.show_oracle_protocol);
        assert!(oracle_advanced.show_oracle_nls_formats);
        assert!(!oracle_advanced.show_mysql_session_options);
        assert!(!oracle_advanced.show_mysql_ssl_ca_path);

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

        let mariadb = DatabaseType::MariaDB.connection_form_spec();
        assert_eq!(mariadb.default_port, 3306);
        assert!(!mariadb.show_driver_mode);
        assert!(!mariadb.service_name_required);
        assert!(!mariadb.supports_tns_alias);
        let mariadb_advanced = DatabaseType::MariaDB.advanced_settings_form_spec();
        assert_eq!(mariadb_advanced, mysql_advanced);
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

        let config = DatabaseConnection::build_oracle_thin_config(&info)
            .expect("debug protocol version should build a Thin config");

        assert_eq!(config.connect_options.desired_protocol_version, 314);
        assert_eq!(config.connect_options.minimum_protocol_version, 314);
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

        let err = DatabaseConnection::build_oracle_thin_config(&info)
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

        assert_eq!(DatabaseType::MySQL.sql_dialect(), SqlDialect::MySql);
        assert_eq!(
            DatabaseType::MySQL.backend_kind(),
            DatabaseBackendKind::MySql
        );
        assert_eq!(
            DatabaseType::from_cache_key(DatabaseType::MySQL.cache_key()),
            DatabaseType::MySQL
        );

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
        let opts = mysql::Opts::from(DatabaseConnection::build_mysql_opts(&info));
        assert!(opts.get_ssl_opts().is_none());

        info.advanced.ssl_mode = ConnectionSslMode::Required;
        let opts = mysql::Opts::from(DatabaseConnection::build_mysql_opts(&info));
        let ssl = opts.get_ssl_opts().expect("required SSL should be enabled");
        assert!(ssl.skip_domain_validation());
        assert!(ssl.accept_invalid_certs());

        info.advanced.ssl_mode = ConnectionSslMode::VerifyCa;
        info.advanced.mysql_ssl_ca_path = "/tmp/mysql-ca.pem".to_string();
        let opts = mysql::Opts::from(DatabaseConnection::build_mysql_opts(&info));
        let ssl = opts.get_ssl_opts().expect("Verify CA should enable SSL");
        assert!(ssl.skip_domain_validation());
        assert!(!ssl.accept_invalid_certs());
        assert_eq!(
            ssl.root_cert_path(),
            Some(std::path::Path::new("/tmp/mysql-ca.pem"))
        );

        info.advanced.ssl_mode = ConnectionSslMode::VerifyIdentity;
        let opts = mysql::Opts::from(DatabaseConnection::build_mysql_opts(&info));
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
            .expect_err("Oracle READ ONLY must not be combined with explicit isolation");

        assert!(err.contains("combining READ ONLY with an explicit transaction isolation level"));

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
    fn mysql_pool_timeout_error_gets_actionable_exhaustion_message() {
        let message = DbConnectionPool::format_mysql_pool_acquire_error(
            &mysql::Error::DriverError(mysql::DriverError::Timeout),
        );

        assert!(message.contains("MySQL connection pool appears exhausted"));
    }

    #[test]
    fn mysql_network_timeout_error_is_not_reported_as_pool_exhaustion() {
        let err = mysql::Error::IoError(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Operation timed out",
        ));
        let message = DbConnectionPool::format_mysql_pool_acquire_error(&err);

        assert!(!message.contains("MySQL connection pool appears exhausted"));
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
                DatabaseType::MySQL,
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
        let mut info = mysql_test_connection_info_from_env();
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
        let mut info = mysql_test_connection_info_from_env();
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
