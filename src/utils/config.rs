use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::db::ConnectionInfo;
use crate::utils::credential_store;

const APP_DIR_NAME: &str = "space_query";
const LEGACY_APP_DIR_NAME: &str = "oracle_query_tool";
const MAX_RECENT_CONNECTIONS: usize = 50;
pub const MAX_RECENT_SQL_FILES: usize = 10;
const MAX_QUERY_HISTORY_ENTRIES: usize = 100;
const DEFAULT_RESULT_CELL_MAX_CHARS: u32 = 150;
pub const DEFAULT_UI_SCALE_PERCENT: u32 = 100;
pub const MIN_UI_SCALE_PERCENT: u32 = 50;
pub const MAX_UI_SCALE_PERCENT: u32 = 300;
pub const UI_SCALE_INCREMENT_PERCENT: u32 = 10;
pub const DEFAULT_CONNECTION_POOL_SIZE: u32 = 12;
pub const MIN_CONNECTION_POOL_SIZE: u32 = 1;
pub const MAX_CONNECTION_POOL_SIZE: u32 = 16;
pub const DEFAULT_CONNECT_TIMEOUT_SECONDS: u32 = 10;
pub const MIN_CONNECT_TIMEOUT_SECONDS: u32 = 1;
pub const MAX_CONNECT_TIMEOUT_SECONDS: u32 = 120;
pub const DEFAULT_LAZY_FETCH_BATCH_SIZE: u32 = 100;
pub const MIN_LAZY_FETCH_BATCH_SIZE: u32 = 1;
pub const MAX_LAZY_FETCH_BATCH_SIZE: u32 = 10_000;
pub const DEFAULT_INTELLISENSE_CONTEXT_WINDOW_KIB: u32 = 16;
pub const MIN_INTELLISENSE_CONTEXT_WINDOW_KIB: u32 = 8;
pub const MAX_INTELLISENSE_CONTEXT_WINDOW_KIB: u32 = 4 * 1024;
pub const DEFAULT_INTELLISENSE_POPUP_DELAY_MS: u32 = 250;
pub const MIN_INTELLISENSE_POPUP_DELAY_MS: u32 = 0;
pub const MAX_INTELLISENSE_POPUP_DELAY_MS: u32 = 5_000;
pub const DEFAULT_CANCEL_TIMEOUT_SECONDS: u32 = 60;
pub const MIN_CANCEL_TIMEOUT_SECONDS: u32 = 1;
pub const MAX_CANCEL_TIMEOUT_SECONDS: u32 = 120;
pub const DEFAULT_SQL_FORMAT_RIGHT_MARGIN: u32 = 120;
pub const MIN_SQL_FORMAT_RIGHT_MARGIN: u32 = 60;
pub const MAX_SQL_FORMAT_RIGHT_MARGIN: u32 = 300;
pub const DEFAULT_FONT_SIZE: u32 = 16;
pub const MIN_FONT_SIZE: u32 = 8;
pub const MAX_FONT_SIZE: u32 = 48;
pub const DEFAULT_UI_FONT_SIZE: u32 = 16;
pub const MIN_UI_FONT_SIZE: u32 = 8;
pub const MAX_UI_FONT_SIZE: u32 = 24;

static RUNTIME_CONFIG: OnceLock<RwLock<AppConfig>> = OnceLock::new();

#[derive(Debug, Serialize, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SqlCommaListLayout {
    Stacked,
    #[default]
    Wrapped,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct AppConfig {
    pub recent_connections: Vec<ConnectionInfo>,
    pub recent_sql_files: Vec<PathBuf>,
    pub last_connection: Option<String>,
    pub editor_font: String,
    pub ui_font_size: u32,
    pub ui_scale_percent: u32,
    pub editor_font_size: u32,
    pub result_font: String,
    pub result_font_size: u32,
    pub result_cell_max_chars: u32,
    pub lazy_fetch_batch_size: u32,
    pub intellisense_context_window_kib: u32,
    pub intellisense_popup_delay_ms: u32,
    pub max_rows: u32,
    pub auto_commit: bool,
    pub connection_pool_size: u32,
    pub connect_timeout_seconds: u32,
    pub cancel_timeout_seconds: u32,
    pub sql_comma_list_layout: SqlCommaListLayout,
    pub sql_format_right_margin: u32,
}

impl AppConfig {
    fn config_base_dir() -> Option<PathBuf> {
        std::env::var_os("SPACE_QUERY_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(dirs::config_dir)
    }

    fn app_file_path(base: Option<PathBuf>, app_dir: &str, file_name: &str) -> Option<PathBuf> {
        base.map(|mut path| {
            path.push(app_dir);
            path.push(file_name);
            path
        })
    }

    fn load_from_path(path: &PathBuf) -> Option<Self> {
        if !path.exists() {
            return None;
        }
        let content = fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }

    pub fn new() -> Self {
        Self {
            recent_connections: Vec::new(),
            recent_sql_files: Vec::new(),
            last_connection: None,
            editor_font: "Courier".to_string(),
            ui_font_size: DEFAULT_UI_FONT_SIZE,
            ui_scale_percent: DEFAULT_UI_SCALE_PERCENT,
            editor_font_size: DEFAULT_FONT_SIZE,
            result_font: "Courier".to_string(),
            result_font_size: DEFAULT_FONT_SIZE,
            result_cell_max_chars: DEFAULT_RESULT_CELL_MAX_CHARS,
            lazy_fetch_batch_size: DEFAULT_LAZY_FETCH_BATCH_SIZE,
            intellisense_context_window_kib: DEFAULT_INTELLISENSE_CONTEXT_WINDOW_KIB,
            intellisense_popup_delay_ms: DEFAULT_INTELLISENSE_POPUP_DELAY_MS,
            max_rows: 1000,
            auto_commit: false,
            connection_pool_size: DEFAULT_CONNECTION_POOL_SIZE,
            connect_timeout_seconds: DEFAULT_CONNECT_TIMEOUT_SECONDS,
            cancel_timeout_seconds: DEFAULT_CANCEL_TIMEOUT_SECONDS,
            sql_comma_list_layout: SqlCommaListLayout::Wrapped,
            sql_format_right_margin: DEFAULT_SQL_FORMAT_RIGHT_MARGIN,
        }
    }

    pub fn clamp_connection_pool_size(size: u32) -> u32 {
        size.clamp(MIN_CONNECTION_POOL_SIZE, MAX_CONNECTION_POOL_SIZE)
    }

    pub fn clamp_ui_scale_percent(percent: u32) -> u32 {
        percent.clamp(MIN_UI_SCALE_PERCENT, MAX_UI_SCALE_PERCENT)
    }

    pub fn increase_ui_scale_percent(percent: u32) -> u32 {
        Self::clamp_ui_scale_percent(percent)
            .saturating_add(UI_SCALE_INCREMENT_PERCENT)
            .min(MAX_UI_SCALE_PERCENT)
    }

    pub fn decrease_ui_scale_percent(percent: u32) -> u32 {
        Self::clamp_ui_scale_percent(percent)
            .saturating_sub(UI_SCALE_INCREMENT_PERCENT)
            .max(MIN_UI_SCALE_PERCENT)
    }

    pub fn normalized_ui_scale_percent(&self) -> u32 {
        Self::clamp_ui_scale_percent(self.ui_scale_percent)
    }

    pub fn clamp_font_size(size: u32) -> u32 {
        size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
    }

    pub fn normalized_editor_font_size(&self) -> u32 {
        Self::clamp_font_size(self.editor_font_size)
    }

    pub fn normalized_result_font_size(&self) -> u32 {
        Self::clamp_font_size(self.result_font_size)
    }

    pub fn clamp_ui_font_size(size: u32) -> u32 {
        size.clamp(MIN_UI_FONT_SIZE, MAX_UI_FONT_SIZE)
    }

    pub fn normalized_ui_font_size(&self) -> u32 {
        Self::clamp_ui_font_size(self.ui_font_size)
    }

    pub fn normalized_connection_pool_size(&self) -> u32 {
        Self::clamp_connection_pool_size(self.connection_pool_size)
    }

    pub fn clamp_connect_timeout_seconds(seconds: u32) -> u32 {
        seconds.clamp(MIN_CONNECT_TIMEOUT_SECONDS, MAX_CONNECT_TIMEOUT_SECONDS)
    }

    pub fn normalized_connect_timeout_seconds(&self) -> u32 {
        Self::clamp_connect_timeout_seconds(self.connect_timeout_seconds)
    }

    pub fn clamp_lazy_fetch_batch_size(size: u32) -> u32 {
        size.clamp(MIN_LAZY_FETCH_BATCH_SIZE, MAX_LAZY_FETCH_BATCH_SIZE)
    }

    pub fn normalized_lazy_fetch_batch_size(&self) -> u32 {
        Self::clamp_lazy_fetch_batch_size(self.lazy_fetch_batch_size)
    }

    pub fn clamp_intellisense_context_window_kib(size: u32) -> u32 {
        size.clamp(
            MIN_INTELLISENSE_CONTEXT_WINDOW_KIB,
            MAX_INTELLISENSE_CONTEXT_WINDOW_KIB,
        )
    }

    pub fn intellisense_context_window_bytes(size_kib: u32) -> usize {
        (Self::clamp_intellisense_context_window_kib(size_kib) as usize).saturating_mul(1024)
    }

    pub fn normalized_intellisense_context_window_kib(&self) -> u32 {
        Self::clamp_intellisense_context_window_kib(self.intellisense_context_window_kib)
    }

    pub fn normalized_intellisense_context_window_bytes(&self) -> usize {
        Self::intellisense_context_window_bytes(self.intellisense_context_window_kib)
    }

    pub fn clamp_intellisense_popup_delay_ms(delay_ms: u32) -> u32 {
        delay_ms.clamp(
            MIN_INTELLISENSE_POPUP_DELAY_MS,
            MAX_INTELLISENSE_POPUP_DELAY_MS,
        )
    }

    pub fn normalized_intellisense_popup_delay_ms(&self) -> u32 {
        Self::clamp_intellisense_popup_delay_ms(self.intellisense_popup_delay_ms)
    }

    pub fn clamp_cancel_timeout_seconds(seconds: u32) -> u32 {
        seconds.clamp(MIN_CANCEL_TIMEOUT_SECONDS, MAX_CANCEL_TIMEOUT_SECONDS)
    }

    pub fn normalized_cancel_timeout_seconds(&self) -> u32 {
        Self::clamp_cancel_timeout_seconds(self.cancel_timeout_seconds)
    }

    pub fn clamp_sql_format_right_margin(margin: u32) -> u32 {
        margin.clamp(MIN_SQL_FORMAT_RIGHT_MARGIN, MAX_SQL_FORMAT_RIGHT_MARGIN)
    }

    pub fn normalized_sql_format_right_margin(&self) -> u32 {
        Self::clamp_sql_format_right_margin(self.sql_format_right_margin)
    }

    pub fn config_path() -> Option<PathBuf> {
        Self::app_file_path(Self::config_base_dir(), APP_DIR_NAME, "config.json")
    }

    fn legacy_config_path() -> Option<PathBuf> {
        Self::app_file_path(Self::config_base_dir(), LEGACY_APP_DIR_NAME, "config.json")
    }

    pub fn load() -> Self {
        let mut loaded_from_legacy = false;
        let mut config = if let Some(path) = Self::config_path() {
            if let Some(loaded) = Self::load_from_path(&path) {
                loaded
            } else if let Some(legacy_path) = Self::legacy_config_path() {
                if let Some(loaded) = Self::load_from_path(&legacy_path) {
                    loaded_from_legacy = true;
                    loaded
                } else {
                    Self::new()
                }
            } else {
                Self::new()
            }
        } else {
            Self::new()
        };

        config.normalize_font_settings();

        if loaded_from_legacy {
            // Migrate config location from legacy app folder to new app folder.
            if let Err(e) = config.save() {
                eprintln!("Failed to migrate config path: {}", e);
            }
        }

        config
    }

    fn normalize_font_settings(&mut self) {
        self.ui_font_size = self.normalized_ui_font_size();
        self.editor_font_size = self.normalized_editor_font_size();
        self.result_font_size = self.normalized_result_font_size();
        if self.editor_font.trim().is_empty() {
            self.editor_font = "Courier".to_string();
        }
        if self.result_font.trim().is_empty() {
            self.result_font = self.editor_font.clone();
        }
    }

    /// Updates the process-local configuration used by UI hot paths.
    /// Disk reads remain explicit at application/bootstrap boundaries.
    pub fn update_runtime(config: &Self) {
        let runtime = RUNTIME_CONFIG.get_or_init(|| RwLock::new(config.clone()));
        *runtime
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = config.clone();
    }

    pub fn runtime() -> Self {
        RUNTIME_CONFIG
            // Runtime consumers must never perform disk I/O. Application and
            // window bootstrap load the persisted config, then publish it via
            // `update_runtime`; the default only covers isolated/test widgets.
            .get_or_init(|| RwLock::new(Self::new()))
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let path = Self::config_path().ok_or_else(|| {
            let err = std::io::Error::other("Config directory is unavailable");
            crate::utils::logging::log_error("config", &format!("Config persistence error: {err}"));
            eprintln!("Config persistence error: {err}");
            err
        })?;

        if let Some(parent) = path.parent() {
            match fs::create_dir_all(parent) {
                Ok(()) => {}
                Err(err) => {
                    crate::utils::logging::log_error(
                        "config",
                        &format!("Config persistence error: {err}"),
                    );
                    eprintln!("Config persistence error: {err}");
                    return Err(Box::new(err));
                }
            }
        }
        let content = match serde_json::to_string_pretty(self) {
            Ok(content) => content,
            Err(err) => {
                crate::utils::logging::log_error(
                    "config",
                    &format!("Config persistence error: {err}"),
                );
                eprintln!("Config persistence error: {err}");
                return Err(Box::new(err));
            }
        };
        match fs::write(&path, content) {
            Ok(()) => {}
            Err(err) => {
                crate::utils::logging::log_error(
                    "config",
                    &format!("Config persistence error: {err}"),
                );
                eprintln!("Config persistence error: {err}");
                return Err(Box::new(err));
            }
        }

        // Restrict file permissions to owner-only (0600) on Unix
        #[cfg(unix)]
        {
            let permissions = fs::Permissions::from_mode(0o600);
            if let Err(e) = fs::set_permissions(&path, permissions) {
                eprintln!("Warning: could not set config file permissions: {}", e);
            }
        }
        Self::update_runtime(self);
        Ok(())
    }

    pub fn add_recent_connection(&mut self, mut info: ConnectionInfo) -> Result<(), String> {
        // Store password in OS keyring, then clear from memory
        if !info.password.is_empty() {
            credential_store::store_password(&info.name, &info.password)
                .map_err(|e| format!("Failed to store password in keyring: {e}"))?;
        }
        info.clear_password();
        info.debug_oracle_thin_protocol_version = None;

        // Remove existing connection with same name
        self.recent_connections.retain(|c| c.name != info.name);

        // Add to front
        self.recent_connections.insert(0, info);

        // Keep only last 10 connections
        self.recent_connections.truncate(MAX_RECENT_CONNECTIONS);
        Ok(())
    }

    pub fn add_recent_sql_file(&mut self, path: &std::path::Path) {
        let normalized_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.recent_sql_files
            .retain(|existing| existing != &normalized_path);
        self.recent_sql_files.insert(0, normalized_path);
        self.recent_sql_files.truncate(MAX_RECENT_SQL_FILES);
    }

    pub fn get_connection_by_name(&self, name: &str) -> Option<&ConnectionInfo> {
        self.recent_connections.iter().find(|c| c.name == name)
    }

    /// Retrieve the password for a saved connection from the OS keyring on demand.
    /// Returns None if no password is stored or the connection name is not found.
    pub fn get_password_for_connection(name: &str) -> Result<Option<String>, String> {
        match credential_store::get_password(name) {
            Ok(Some(password)) => Ok(Some(password)),
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Failed to load password from keyring: {e}")),
        }
    }

    pub fn remove_connection(&mut self, name: &str) -> Result<(), String> {
        self.remove_connection_with(name, credential_store::delete_password)
    }

    pub fn get_all_connections(&self) -> &Vec<ConnectionInfo> {
        &self.recent_connections
    }
}

impl AppConfig {
    fn remove_connection_with<F>(&mut self, name: &str, delete_password: F) -> Result<(), String>
    where
        F: FnOnce(&str) -> Result<(), String>,
    {
        let removed = self.recent_connections.iter().any(|c| c.name == name);
        self.recent_connections.retain(|c| c.name != name);

        if self.last_connection.as_deref() == Some(name) {
            self.last_connection = None;
        }

        if !removed {
            return Ok(());
        }

        // Remove password from OS keyring after config list cleanup.
        // Keyring failures are logged but do not block removal from config.
        if let Err(err) = delete_password(name) {
            crate::utils::logging::log_warning(
                "config",
                &format!(
                    "Connection removed from config, but failed to remove password from keyring: {err}"
                ),
            );
            eprintln!(
                "Connection removed from config, but failed to remove password from keyring: {}",
                err
            );
        }

        Ok(())
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct QueryHistory {
    pub queries: VecDeque<QueryHistoryEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct QueryHistoryEntry {
    pub sql: String,
    pub timestamp: String,
    pub execution_time_ms: u64,
    pub row_count: usize,
    pub connection_name: String,
    #[serde(default)]
    pub connection_id: Option<u64>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default = "default_query_success")]
    pub success: bool,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub error_line: Option<usize>,
}

fn default_query_success() -> bool {
    true
}

impl QueryHistory {
    pub fn new() -> Self {
        Self {
            queries: VecDeque::new(),
        }
    }

    pub fn load() -> Self {
        Self::new()
    }

    pub fn add_entry(&mut self, entry: QueryHistoryEntry) {
        self.queries.push_front(entry);
        // Keep only last 100 queries
        self.queries.truncate(MAX_QUERY_HISTORY_ENTRIES);
    }
}

impl Default for QueryHistory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{AppConfig, QueryHistoryEntry};
    use crate::db::{ConnectionInfo, DatabaseType};

    fn sample_connection(name: &str) -> ConnectionInfo {
        ConnectionInfo {
            name: name.to_string(),
            host: "localhost".to_string(),
            port: 1521,
            service_name: "orcl".to_string(),
            username: "scott".to_string(),
            password: String::new(),
            db_type: crate::db::DatabaseType::Oracle,
            advanced: crate::db::ConnectionAdvancedSettings::default_for(
                crate::db::DatabaseType::Oracle,
            ),
            debug_oracle_thin_protocol_version: None,
        }
    }

    #[test]
    fn remove_connection_clears_last_selected_connection() {
        let mut config = AppConfig::new();
        config.recent_connections.push(sample_connection("primary"));
        config.last_connection = Some("primary".to_string());

        let result = config.remove_connection_with("primary", |_| Ok(()));

        assert!(result.is_ok());
        assert!(config.recent_connections.is_empty());
        assert!(config.last_connection.is_none());
    }

    #[test]
    fn remove_connection_ignores_keyring_error_after_list_cleanup() {
        let mut config = AppConfig::new();
        config.recent_connections.push(sample_connection("primary"));

        let result =
            config.remove_connection_with("primary", |_| Err("keyring backend unavailable".into()));

        assert!(result.is_ok());
        assert!(config.recent_connections.is_empty());
    }

    #[test]
    fn remove_connection_skips_keyring_delete_when_entry_does_not_exist() {
        let mut config = AppConfig::new();
        config.recent_connections.push(sample_connection("primary"));
        let mut delete_called = false;

        let result = config.remove_connection_with("missing", |_| {
            delete_called = true;
            Ok(())
        });

        assert!(result.is_ok());
        assert!(!delete_called);
        assert_eq!(config.recent_connections.len(), 1);
    }

    #[test]
    fn add_recent_connection_drops_runtime_debug_protocol_choice() {
        let mut config = AppConfig::new();
        let mut connection = sample_connection("debug-protocol");
        connection.debug_oracle_thin_protocol_version = Some(314);

        config
            .add_recent_connection(connection)
            .expect("empty-password connection should save without keyring access");

        assert_eq!(
            config.recent_connections[0].debug_oracle_thin_protocol_version,
            None
        );
    }

    #[test]
    fn app_config_serialization_preserves_mysql_db_type() {
        let mut config = AppConfig::new();
        config.recent_connections.push(ConnectionInfo {
            name: "maria".to_string(),
            host: "localhost".to_string(),
            port: 3306,
            service_name: String::new(),
            username: "root".to_string(),
            password: String::new(),
            db_type: DatabaseType::MySQL,
            advanced: crate::db::ConnectionAdvancedSettings::default_for(DatabaseType::MySQL),
            debug_oracle_thin_protocol_version: None,
        });

        let serialized =
            serde_json::to_string(&config).expect("config with MySQL db_type should serialize");
        let restored: AppConfig =
            serde_json::from_str(&serialized).expect("serialized config should deserialize");

        assert_eq!(restored.recent_connections.len(), 1);
        assert_eq!(restored.recent_connections[0].db_type, DatabaseType::MySQL);
    }

    #[test]
    fn query_history_origin_fields_are_backward_compatible() {
        let restored: QueryHistoryEntry = serde_json::from_str(
            r#"{
                "sql":"SELECT 1",
                "timestamp":"2026-08-03 12:00:00",
                "execution_time_ms":1,
                "row_count":1,
                "connection_name":"Local Oracle"
            }"#,
        )
        .expect("legacy query history should deserialize");

        assert_eq!(restored.connection_id, None);
        assert_eq!(restored.scope, None);
        assert!(restored.success);
    }

    #[test]
    fn app_config_defaults_connection_pool_size_to_twelve() {
        assert_eq!(
            AppConfig::new().connection_pool_size,
            super::DEFAULT_CONNECTION_POOL_SIZE
        );
    }

    #[test]
    fn app_config_defaults_connect_timeout_to_ten_seconds() {
        assert_eq!(
            AppConfig::new().connect_timeout_seconds,
            super::DEFAULT_CONNECT_TIMEOUT_SECONDS
        );
    }

    #[test]
    fn app_config_defaults_lazy_fetch_batch_size_to_one_hundred() {
        assert_eq!(
            AppConfig::new().lazy_fetch_batch_size,
            super::DEFAULT_LAZY_FETCH_BATCH_SIZE
        );
    }

    #[test]
    fn app_config_defaults_intellisense_context_window_to_sixteen_kib() {
        assert_eq!(
            AppConfig::new().intellisense_context_window_kib,
            super::DEFAULT_INTELLISENSE_CONTEXT_WINDOW_KIB
        );
    }

    #[test]
    fn app_config_defaults_intellisense_popup_delay_to_250_ms() {
        assert_eq!(
            AppConfig::new().intellisense_popup_delay_ms,
            super::DEFAULT_INTELLISENSE_POPUP_DELAY_MS
        );
    }

    #[test]
    fn app_config_defaults_result_cell_max_chars_to_one_hundred_fifty() {
        assert_eq!(
            AppConfig::new().result_cell_max_chars,
            super::DEFAULT_RESULT_CELL_MAX_CHARS
        );
    }

    #[test]
    fn app_config_defaults_cancel_timeout_to_sixty_seconds() {
        assert_eq!(
            AppConfig::new().cancel_timeout_seconds,
            super::DEFAULT_CANCEL_TIMEOUT_SECONDS
        );
    }

    #[test]
    fn app_config_defaults_sql_comma_lists_to_wrapped_with_120_column_margin() {
        let config = AppConfig::new();

        assert_eq!(
            config.sql_comma_list_layout,
            super::SqlCommaListLayout::Wrapped
        );
        assert_eq!(
            config.sql_format_right_margin,
            super::DEFAULT_SQL_FORMAT_RIGHT_MARGIN
        );
    }

    #[test]
    fn app_config_round_trips_wrapped_sql_comma_list_layout() {
        let mut config = AppConfig::new();
        config.sql_comma_list_layout = super::SqlCommaListLayout::Wrapped;
        config.sql_format_right_margin = 140;

        let serialized = serde_json::to_string(&config).expect("config should serialize");
        let restored: AppConfig =
            serde_json::from_str(&serialized).expect("config should deserialize");

        assert!(serialized.contains("\"sql_comma_list_layout\":\"wrapped\""));
        assert_eq!(
            restored.sql_comma_list_layout,
            super::SqlCommaListLayout::Wrapped
        );
        assert_eq!(restored.sql_format_right_margin, 140);
    }

    #[test]
    fn app_config_clamps_connection_pool_size_to_supported_range() {
        assert_eq!(AppConfig::clamp_connection_pool_size(0), 1);
        assert_eq!(AppConfig::clamp_connection_pool_size(4), 4);
        assert_eq!(AppConfig::clamp_connection_pool_size(99), 16);
    }

    #[test]
    fn app_config_clamps_ui_scale_to_supported_range() {
        assert_eq!(AppConfig::clamp_ui_scale_percent(0), 50);
        assert_eq!(AppConfig::clamp_ui_scale_percent(100), 100);
        assert_eq!(AppConfig::clamp_ui_scale_percent(999), 300);
    }

    #[test]
    fn app_config_adjusts_arbitrary_ui_scale_by_ten_percentage_points() {
        assert_eq!(AppConfig::increase_ui_scale_percent(125), 135);
        assert_eq!(AppConfig::decrease_ui_scale_percent(135), 125);
    }

    #[test]
    fn app_config_ui_scale_adjustments_clamp_at_bounds() {
        assert_eq!(AppConfig::increase_ui_scale_percent(300), 300);
        assert_eq!(AppConfig::increase_ui_scale_percent(u32::MAX), 300);
        assert_eq!(AppConfig::decrease_ui_scale_percent(50), 50);
        assert_eq!(AppConfig::decrease_ui_scale_percent(0), 50);
    }

    #[test]
    fn app_config_normalizes_font_settings_from_untrusted_config_values() {
        let mut config = AppConfig::new();
        config.editor_font.clear();
        config.result_font.clear();
        config.ui_font_size = u32::MAX;
        config.editor_font_size = 0;
        config.result_font_size = u32::MAX;

        config.normalize_font_settings();

        assert_eq!(config.editor_font, "Courier");
        assert_eq!(config.result_font, "Courier");
        assert_eq!(config.ui_font_size, super::MAX_UI_FONT_SIZE);
        assert_eq!(config.editor_font_size, super::MIN_FONT_SIZE);
        assert_eq!(config.result_font_size, super::MAX_FONT_SIZE);
    }

    #[test]
    fn app_config_uses_a_cross_platform_builtin_font_by_default() {
        let config = AppConfig::new();
        assert_eq!(config.editor_font, "Courier");
        assert_eq!(config.result_font, "Courier");
    }

    #[test]
    fn app_config_clamps_lazy_fetch_batch_size_to_supported_range() {
        assert_eq!(AppConfig::clamp_lazy_fetch_batch_size(0), 1);
        assert_eq!(AppConfig::clamp_lazy_fetch_batch_size(100), 100);
        assert_eq!(AppConfig::clamp_lazy_fetch_batch_size(50_000), 10_000);
    }

    #[test]
    fn app_config_clamps_intellisense_context_window_to_supported_range() {
        assert_eq!(AppConfig::clamp_intellisense_context_window_kib(0), 8);
        assert_eq!(AppConfig::clamp_intellisense_context_window_kib(128), 128);
        assert_eq!(
            AppConfig::clamp_intellisense_context_window_kib(10_000),
            4 * 1024
        );
        assert_eq!(
            AppConfig::intellisense_context_window_bytes(128),
            128 * 1024
        );
    }

    #[test]
    fn app_config_clamps_intellisense_popup_delay_to_supported_range() {
        assert_eq!(AppConfig::clamp_intellisense_popup_delay_ms(0), 0);
        assert_eq!(AppConfig::clamp_intellisense_popup_delay_ms(250), 250);
        assert_eq!(AppConfig::clamp_intellisense_popup_delay_ms(10_000), 5_000);
    }

    #[test]
    fn app_config_clamps_cancel_timeout_to_supported_range() {
        assert_eq!(AppConfig::clamp_cancel_timeout_seconds(0), 1);
        assert_eq!(AppConfig::clamp_cancel_timeout_seconds(30), 30);
        assert_eq!(AppConfig::clamp_cancel_timeout_seconds(999), 120);
    }

    #[test]
    fn app_config_clamps_connect_timeout_to_supported_range() {
        assert_eq!(AppConfig::clamp_connect_timeout_seconds(0), 1);
        assert_eq!(AppConfig::clamp_connect_timeout_seconds(10), 10);
        assert_eq!(AppConfig::clamp_connect_timeout_seconds(999), 120);
    }

    #[test]
    fn app_config_clamps_sql_format_right_margin_to_supported_range() {
        assert_eq!(AppConfig::clamp_sql_format_right_margin(0), 60);
        assert_eq!(AppConfig::clamp_sql_format_right_margin(120), 120);
        assert_eq!(AppConfig::clamp_sql_format_right_margin(999), 300);
    }

    #[test]
    fn app_config_deserializes_missing_pool_size_with_default() {
        let restored: AppConfig = serde_json::from_str(
            r#"{
                "recent_connections": [],
                "last_connection": null,
                "editor_font": "Courier",
                "ui_font_size": 16,
                "editor_font_size": 16,
                "result_font": "Courier",
                "result_font_size": 16,
                "result_cell_max_chars": 50,
                "max_rows": 1000,
                "auto_commit": false
            }"#,
        )
        .expect("old config should deserialize");

        assert_eq!(
            restored.connection_pool_size,
            super::DEFAULT_CONNECTION_POOL_SIZE
        );
        assert_eq!(restored.ui_scale_percent, super::DEFAULT_UI_SCALE_PERCENT);
        assert_eq!(
            restored.lazy_fetch_batch_size,
            super::DEFAULT_LAZY_FETCH_BATCH_SIZE
        );
        assert_eq!(
            restored.intellisense_context_window_kib,
            super::DEFAULT_INTELLISENSE_CONTEXT_WINDOW_KIB
        );
        assert_eq!(
            restored.intellisense_popup_delay_ms,
            super::DEFAULT_INTELLISENSE_POPUP_DELAY_MS
        );
        assert_eq!(
            restored.cancel_timeout_seconds,
            super::DEFAULT_CANCEL_TIMEOUT_SECONDS
        );
        assert_eq!(
            restored.connect_timeout_seconds,
            super::DEFAULT_CONNECT_TIMEOUT_SECONDS
        );
        assert_eq!(
            restored.sql_comma_list_layout,
            super::SqlCommaListLayout::Wrapped
        );
        assert_eq!(
            restored.sql_format_right_margin,
            super::DEFAULT_SQL_FORMAT_RIGHT_MARGIN
        );
    }

    #[test]
    fn app_config_deserializes_old_connection_without_advanced_settings() {
        let restored: AppConfig = serde_json::from_str(
            r#"{
                "recent_connections": [{
                    "name": "maria",
                    "host": "localhost",
                    "port": 3306,
                    "service_name": "query_tool_test",
                    "username": "root",
                    "db_type": "MySQL"
                }],
                "last_connection": null,
                "editor_font": "Courier",
                "ui_font_size": 16,
                "editor_font_size": 16,
                "result_font": "Courier",
                "result_font_size": 16,
                "result_cell_max_chars": 50,
                "max_rows": 1000,
                "auto_commit": false
            }"#,
        )
        .expect("old config should deserialize");

        let advanced = &restored.recent_connections[0].advanced;
        assert_eq!(advanced.mysql_sql_mode, "TRADITIONAL");
        assert_eq!(advanced.mysql_charset, "utf8mb4");
        assert_eq!(advanced.session_time_zone, "+00:00");
        assert_eq!(
            advanced.default_transaction_isolation,
            crate::db::transaction::TransactionIsolation::ReadCommitted
        );
    }

    #[test]
    fn app_config_merges_partial_advanced_settings_with_db_defaults() {
        let restored: AppConfig = serde_json::from_str(
            r#"{
                "recent_connections": [{
                    "name": "maria",
                    "host": "localhost",
                    "port": 3306,
                    "service_name": "query_tool_test",
                    "username": "root",
                    "db_type": "MySQL",
                    "advanced": {
                        "mysql_sql_mode": "ANSI_QUOTES"
                    }
                }],
                "last_connection": null,
                "editor_font": "Courier",
                "ui_font_size": 16,
                "editor_font_size": 16,
                "result_font": "Courier",
                "result_font_size": 16,
                "result_cell_max_chars": 50,
                "max_rows": 1000,
                "auto_commit": false
            }"#,
        )
        .expect("partial advanced config should deserialize");

        let advanced = &restored.recent_connections[0].advanced;
        assert_eq!(advanced.mysql_sql_mode, "ANSI_QUOTES");
        assert_eq!(advanced.mysql_charset, "utf8mb4");
        assert_eq!(advanced.session_time_zone, "+00:00");
    }

    #[test]
    fn recent_sql_files_are_deduplicated_and_limited_to_ten() {
        let mut config = AppConfig::new();
        for idx in 0..12 {
            config.add_recent_sql_file(std::path::Path::new(&format!("/tmp/query_{idx}.sql")));
        }
        config.add_recent_sql_file(std::path::Path::new("/tmp/query_5.sql"));

        assert_eq!(config.recent_sql_files.len(), super::MAX_RECENT_SQL_FILES);
        assert_eq!(
            config.recent_sql_files.first(),
            Some(&std::path::PathBuf::from("/tmp/query_5.sql"))
        );
        assert_eq!(
            config
                .recent_sql_files
                .iter()
                .filter(|path| path.as_path() == std::path::Path::new("/tmp/query_5.sql"))
                .count(),
            1
        );
    }

    #[test]
    fn app_config_serializes_connection_pool_size_without_passwords() {
        let mut config = AppConfig::new();
        config.connection_pool_size = 8;
        config.lazy_fetch_batch_size = 500;
        config.intellisense_context_window_kib = 256;
        config.intellisense_popup_delay_ms = 400;
        config.connect_timeout_seconds = 11;
        config.cancel_timeout_seconds = 9;
        config.recent_connections.push(ConnectionInfo {
            name: "prod".to_string(),
            host: "localhost".to_string(),
            port: 1521,
            service_name: "FREE".to_string(),
            username: "scott".to_string(),
            password: "secret".to_string(),
            db_type: DatabaseType::Oracle,
            advanced: crate::db::ConnectionAdvancedSettings::default_for(DatabaseType::Oracle),
            debug_oracle_thin_protocol_version: Some(314),
        });

        let serialized = serde_json::to_string(&config).expect("config should serialize");

        assert!(serialized.contains("\"connection_pool_size\":8"));
        assert!(serialized.contains("\"lazy_fetch_batch_size\":500"));
        assert!(serialized.contains("\"intellisense_context_window_kib\":256"));
        assert!(serialized.contains("\"intellisense_popup_delay_ms\":400"));
        assert!(serialized.contains("\"connect_timeout_seconds\":11"));
        assert!(serialized.contains("\"cancel_timeout_seconds\":9"));
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("debug_oracle_thin_protocol_version"));
    }
}
