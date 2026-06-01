use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use aes::{Aes192, Aes256};
use cbc::cipher::{
    block_padding::{NoPadding, Pkcs7},
    BlockDecryptMut, BlockEncryptMut, KeyIvInit,
};
use pbkdf2::pbkdf2_hmac;
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha512};

use crate::connect::{AcceptInfo, ConnectOptions, ConnectTarget, OracleNetConnector};
use crate::exec::{
    BindInputValue, BindValue, ColumnMetadata, DescribedQueryResult, ExecuteWithImplicitResult,
    OracleColumnType, OracleValue, OutBindResult, QueryResult, RefCursorValue, StatementRequest,
};
use crate::{log_connect_phase, OracleThinError};

const TNS_PACKET_TYPE_DATA: u8 = 6;
const TNS_PACKET_TYPE_MARKER: u8 = 12;
const TNS_PACKET_TYPE_CONTROL: u8 = 14;
const TNS_MARKER_TYPE_INTERRUPT: u8 = 3;
const TNS_DEFAULT_SDU: usize = 8192;
const TNS_DATA_PACKET_CHUNK_SIZE: usize = TNS_DEFAULT_SDU - 64;
const TNS_DATA_FLAGS_EOF: u16 = 0x0040;
const TNS_DATA_FLAGS_END_OF_RESPONSE: u16 = 0x2000;
const TNS_MSG_TYPE_PROTOCOL: u8 = 1;
const TNS_MSG_TYPE_DATA_TYPES: u8 = 2;
const TNS_MSG_TYPE_FUNCTION: u8 = 3;
const TNS_MSG_TYPE_ERROR: u8 = 4;
const TNS_MSG_TYPE_ROW_HEADER: u8 = 6;
const TNS_MSG_TYPE_ROW_DATA: u8 = 7;
const TNS_MSG_TYPE_PARAMETER: u8 = 8;
const TNS_MSG_TYPE_STATUS: u8 = 9;
const TNS_MSG_TYPE_IO_VECTOR: u8 = 11;
const TNS_MSG_TYPE_WARNING: u8 = 15;
const TNS_MSG_TYPE_DESCRIBE_INFO: u8 = 16;
const TNS_MSG_TYPE_PIGGYBACK: u8 = 17;
const TNS_MSG_TYPE_FLUSH_OUT_BINDS: u8 = 19;
const TNS_MSG_TYPE_BIT_VECTOR: u8 = 21;
const TNS_MSG_TYPE_SERVER_SIDE_PIGGYBACK: u8 = 23;
const TNS_MSG_TYPE_IMPLICIT_RESULTSET: u8 = 27;
const TNS_MSG_TYPE_END_OF_RESPONSE: u8 = 29;
const TNS_MSG_TYPE_TOKEN: u8 = 33;
const TNS_SERVER_PIGGYBACK_QUERY_CACHE_INVALIDATION: u8 = 1;
const TNS_SERVER_PIGGYBACK_OS_PID_MTS: u8 = 2;
const TNS_SERVER_PIGGYBACK_TRACE_EVENT: u8 = 3;
const TNS_SERVER_PIGGYBACK_SESS_RET: u8 = 4;
const TNS_SERVER_PIGGYBACK_SYNC: u8 = 5;
const TNS_SERVER_PIGGYBACK_LTXID: u8 = 7;
const TNS_SERVER_PIGGYBACK_AC_REPLAY_CONTEXT: u8 = 8;
const TNS_SERVER_PIGGYBACK_EXT_SYNC: u8 = 9;
const TNS_SERVER_PIGGYBACK_SESS_SIGNATURE: u8 = 10;
const TNS_FUNC_COMMIT: u8 = 14;
const TNS_FUNC_ROLLBACK: u8 = 15;
const TNS_FUNC_FETCH: u8 = 5;
const TNS_FUNC_EXECUTE: u8 = 94;
const TNS_FUNC_CLOSE_CURSORS: u8 = 105;
const TNS_FUNC_AUTH_PHASE_ONE: u8 = 118;
const TNS_FUNC_AUTH_PHASE_TWO: u8 = 115;
const TNS_FUNC_PING: u8 = 147;
const TNS_EXEC_OPTION_PARSE: u32 = 0x0000_0001;
const TNS_EXEC_OPTION_BIND: u32 = 0x0000_0008;
const TNS_EXEC_OPTION_DEFINE: u32 = 0x0000_0010;
const TNS_EXEC_OPTION_EXECUTE: u32 = 0x0000_0020;
const TNS_EXEC_OPTION_FETCH: u32 = 0x0000_0040;
const TNS_EXEC_OPTION_COMMIT: u32 = 0x0000_0100;
const TNS_EXEC_OPTION_PLSQL_BIND: u32 = 0x0000_0400;
const TNS_EXEC_OPTION_NOT_PLSQL: u32 = 0x0000_8000;
const TNS_EXEC_OPTION_DESCRIBE: u32 = 0x0002_0000;
const TNS_EXEC_FLAGS_IMPLICIT_RESULTSET: u32 = 0x0000_8000;
const TNS_BIND_USE_INDICATORS: u8 = 0x01;
const TNS_BIND_DIR_INPUT: u8 = 32;
const TNS_MAX_LONG_LENGTH: u32 = 0x7fff_ffff;
const TNS_CHARSET_UTF8: u16 = 873;
const TNS_ERR_NO_DATA_FOUND: u32 = 1403;
const TNS_AUTH_MODE_LOGON: u32 = 0x0000_0001;
const TNS_AUTH_MODE_WITH_PASSWORD: u32 = 0x0000_0100;
const TNS_VERIFIER_TYPE_11G_1: u32 = 0xb152;
const TNS_VERIFIER_TYPE_11G_2: u32 = 0x1b25;
const TNS_VERIFIER_TYPE_12C: u32 = 0x4815;
const TNS_CCAP_FIELD_VERSION: usize = 7;
const TNS_CCAP_FIELD_VERSION_12_2: u8 = 8;
const TNS_CCAP_FIELD_VERSION_12_2_EXT1: u8 = 9;
const TNS_CCAP_FIELD_VERSION_20_1: u8 = 14;
const TNS_CCAP_FIELD_VERSION_23_1: u8 = 17;
const TNS_CCAP_FIELD_VERSION_23_1_EXT_3: u8 = 20;
const TNS_CCAP_FIELD_VERSION_23_4: u8 = 24;
const TNS_CCAP_TTC4: usize = 40;
const TNS_CCAP_EXPLICIT_BOUNDARY: u8 = 0x40;
const TNS_CCAP_END_OF_RESPONSE: u8 = 0x20;
const TNS_RCAP_TTC: usize = 6;
const TNS_RCAP_TTC_32K: u8 = 0x04;
const ORA_TYPE_NUM_VARCHAR: u8 = 1;
const ORA_TYPE_NUM_NUMBER: u8 = 2;
const ORA_TYPE_NUM_LONG: u8 = 8;
const ORA_TYPE_NUM_ROWID: u8 = 11;
const ORA_TYPE_NUM_DATE: u8 = 12;
const ORA_TYPE_NUM_RAW: u8 = 23;
const ORA_TYPE_NUM_LONG_RAW: u8 = 24;
const ORA_TYPE_NUM_CHAR: u8 = 96;
const ORA_TYPE_NUM_BINARY_FLOAT: u8 = 100;
const ORA_TYPE_NUM_BINARY_DOUBLE: u8 = 101;
const ORA_TYPE_NUM_CURSOR: u8 = 102;
const ORA_TYPE_NUM_CLOB: u8 = 112;
const ORA_TYPE_NUM_BLOB: u8 = 113;
const ORA_TYPE_NUM_TIMESTAMP: u8 = 180;
const ORA_TYPE_NUM_TIMESTAMP_TZ: u8 = 181;
const ORA_TYPE_NUM_TIMESTAMP_DTY: u8 = 187;
const ORA_TYPE_NUM_UROWID: u8 = 208;
const ORA_TYPE_NUM_BOOLEAN: u8 = 252;
const CS_FORM_NCHAR: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OracleThinConfig {
    pub target: ConnectTarget,
    pub username: String,
    pub password: String,
    pub connect_options: ConnectOptions,
    pub program: String,
    pub machine: String,
    pub os_user: String,
}

impl OracleThinConfig {
    pub fn new(
        target: ConnectTarget,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self {
            target,
            username: username.into(),
            password: password.into(),
            connect_options: ConnectOptions::default(),
            program: "space-query-thin".to_string(),
            machine: "localhost".to_string(),
            os_user: "space-query".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OracleThinCapabilities {
    pub protocol_version: Option<u16>,
    pub ttc_field_version: u8,
    pub charset_id: u16,
    pub ncharset_id: u16,
    pub max_string_size: u32,
    pub supports_sql_boolean: bool,
    pub supports_end_of_response: bool,
    pub supports_request_boundaries: bool,
    pub supports_fast_auth: bool,
    pub supports_oob: bool,
    pub supports_big_clr_chunks: bool,
    pub supports_implicit_resultsets: bool,
}

impl Default for OracleThinCapabilities {
    fn default() -> Self {
        Self {
            protocol_version: None,
            ttc_field_version: 6,
            charset_id: 0,
            ncharset_id: 0,
            max_string_size: 4000,
            supports_sql_boolean: false,
            supports_end_of_response: false,
            supports_request_boundaries: false,
            supports_fast_auth: false,
            supports_oob: false,
            supports_big_clr_chunks: false,
            supports_implicit_resultsets: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OracleThinCancelHandle {
    cancelled: Arc<AtomicBool>,
    break_stream: Option<Arc<Mutex<TcpStream>>>,
    protocol_version: u16,
    supports_oob: bool,
}

impl OracleThinCancelHandle {
    pub fn break_execution(&self) -> Result<(), OracleThinError> {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(stream) = &self.break_stream {
            let mut stream = stream
                .lock()
                .map_err(|_| OracleThinError::new("Oracle thin cancel stream lock poisoned"))?;
            if self.supports_oob {
                let _ = send_oob_break(&stream);
            }
            write_marker_packet(
                &mut stream,
                self.protocol_version,
                TNS_MARKER_TYPE_INTERRUPT,
            )?;
            let _ = stream.shutdown(Shutdown::Both);
        }
        Ok(())
    }

    pub fn force_close(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

#[derive(Debug)]
pub struct OracleThinSession {
    #[allow(dead_code)]
    stream: TcpStream,
    #[allow(dead_code)]
    config: OracleThinConfig,
    capabilities: OracleThinCapabilities,
    server_version: Option<String>,
    broken: bool,
    call_timeout: Option<Duration>,
    pending_cursor_closes: Vec<u32>,
    cancel_flag: Arc<AtomicBool>,
    ttc_sequence: u8,
}

impl OracleThinSession {
    pub fn connect(config: OracleThinConfig) -> Result<Self, OracleThinError> {
        log_connect_phase("session-connect", &config.target.easy_connect_string());
        let connector = OracleNetConnector::new(config.connect_options.clone());
        let (mut stream, accept) = connector.connect_tcp(&config.target)?;
        let mut capabilities = capabilities_from_accept(&config.connect_options, &accept);
        negotiate_protocol(&mut stream, &mut capabilities)?;
        negotiate_data_types(&mut stream, &capabilities)?;
        let auth = authenticate(&mut stream, &config, &capabilities)?;
        Ok(Self {
            stream,
            config,
            capabilities,
            server_version: auth.server_version,
            broken: false,
            call_timeout: None,
            pending_cursor_closes: Vec::new(),
            cancel_flag: Arc::new(AtomicBool::new(false)),
            ttc_sequence: 3,
        })
    }

    pub fn capabilities(&self) -> &OracleThinCapabilities {
        &self.capabilities
    }

    pub fn server_version(&self) -> Option<&str> {
        self.server_version.as_deref()
    }

    pub fn connection_id(&self) -> u64 {
        self as *const Self as usize as u64
    }

    pub fn cancel_handle(&self) -> OracleThinCancelHandle {
        OracleThinCancelHandle {
            cancelled: Arc::clone(&self.cancel_flag),
            break_stream: self
                .stream
                .try_clone()
                .ok()
                .map(|stream| Arc::new(Mutex::new(stream))),
            protocol_version: self.capabilities.protocol_version.unwrap_or(319),
            supports_oob: self.capabilities.supports_oob,
        }
    }

    pub fn reset_pending_cancel(&self) {
        self.cancel_flag.store(false, Ordering::SeqCst);
    }

    pub fn break_execution(&self) -> Result<(), OracleThinError> {
        self.cancel_flag.store(true, Ordering::SeqCst);
        if self.capabilities.supports_oob {
            let _ = send_oob_break(&self.stream);
        }
        let mut stream = self.stream.try_clone()?;
        write_marker_packet(
            &mut stream,
            self.capabilities.protocol_version.unwrap_or(319),
            TNS_MARKER_TYPE_INTERRUPT,
        )?;
        let _ = stream.shutdown(Shutdown::Both);
        Ok(())
    }

    pub fn set_call_timeout(&mut self, timeout: Option<Duration>) -> Result<(), OracleThinError> {
        self.call_timeout = timeout;
        Ok(())
    }

    pub fn call_timeout(&self) -> Result<Option<Duration>, OracleThinError> {
        Ok(self.call_timeout)
    }

    pub fn is_broken(&self) -> bool {
        self.broken
    }

    pub fn mark_broken(&mut self) {
        self.broken = true;
    }

    pub fn is_healthy(&mut self) -> bool {
        !self.broken
    }

    pub fn reset_before_reuse(&mut self) -> Result<(), OracleThinError> {
        self.reset_pending_cancel();
        self.call_timeout = None;
        if self.broken {
            self.pending_cursor_closes.clear();
            Err(OracleThinError::new("Oracle thin session is broken"))
        } else {
            self.flush_pending_cursor_closes()?;
            Ok(())
        }
    }

    pub fn ping(&mut self) -> Result<(), OracleThinError> {
        if self.broken {
            Err(OracleThinError::new("Oracle thin session is broken"))
        } else {
            Ok(())
        }
    }

    pub fn status(&mut self) -> Result<(), OracleThinError> {
        self.ping()
    }

    pub fn commit(&mut self) -> Result<(), OracleThinError> {
        self.simple_ttc_call(TNS_FUNC_COMMIT, "commit")
    }

    pub fn rollback(&mut self) -> Result<(), OracleThinError> {
        self.simple_ttc_call(TNS_FUNC_ROLLBACK, "rollback")
    }

    pub fn transaction_in_progress(&self) -> bool {
        false
    }

    pub fn query_drop(&mut self, sql: &str) -> Result<(), OracleThinError> {
        self.execute_typed(&StatementRequest::statement(sql), &[])
            .map(|_| ())
    }

    pub fn query(
        &mut self,
        sql: &str,
        fetch_array_size: u32,
    ) -> Result<QueryResult, OracleThinError> {
        self.execute_typed(&StatementRequest::query(sql, fetch_array_size), &[])
    }

    pub fn execute(
        &mut self,
        request: &StatementRequest,
        _prefetch_rows: usize,
    ) -> Result<QueryResult, OracleThinError> {
        self.execute_typed(request, &[])
    }

    pub fn execute_typed(
        &mut self,
        request: &StatementRequest,
        _column_types: &[OracleColumnType],
    ) -> Result<QueryResult, OracleThinError> {
        self.execute_request(request)
            .map(|response| response.result)
    }

    pub fn execute_typed_with_implicit(
        &mut self,
        request: &StatementRequest,
        _column_types: &[OracleColumnType],
    ) -> Result<ExecuteWithImplicitResult, OracleThinError> {
        let response = self.execute_request(request)?;
        let result = response.result;
        self.remember_last_row_for_open_fetch(&result);
        Ok(ExecuteWithImplicitResult {
            result,
            implicit_results: response.implicit_results,
        })
    }

    pub fn execute_typed_fetch_all(
        &mut self,
        request: &StatementRequest,
        column_types: &[OracleColumnType],
    ) -> Result<QueryResult, OracleThinError> {
        let mut result = self.execute_typed(request, column_types)?;
        let Some(cursor_id) = result.cursor_id else {
            return Ok(result);
        };
        let mut needs_define_fetch = column_types.iter().any(|column_type| {
            matches!(
                column_type,
                OracleColumnType::Long
                    | OracleColumnType::Clob
                    | OracleColumnType::Nclob
                    | OracleColumnType::Blob
            )
        });
        while !result.exhausted {
            let batch = if needs_define_fetch {
                needs_define_fetch = false;
                self.define_and_fetch_typed(cursor_id, request.fetch_array_size, column_types)?
            } else {
                self.fetch_typed(cursor_id, request.fetch_array_size, column_types)?
            };
            let no_rows = batch.rows.is_empty();
            result.rows.extend(batch.rows);
            result.exhausted = batch.exhausted || batch.cursor_id.is_none() || no_rows;
        }
        result.cursor_id = None;
        Ok(result)
    }

    pub fn execute_out_binds(
        &mut self,
        request: &StatementRequest,
        bind_types: &[OracleColumnType],
    ) -> Result<Vec<OracleValue>, OracleThinError> {
        self.execute_out_binds_with_implicit(request, bind_types)
            .map(|result| result.values)
    }

    pub fn execute_out_binds_with_implicit(
        &mut self,
        request: &StatementRequest,
        _bind_types: &[OracleColumnType],
    ) -> Result<OutBindResult, OracleThinError> {
        let response = self.execute_request(request)?;
        let values = response
            .out_bind_values
            .or_else(|| response.result.rows.first().cloned())
            .unwrap_or_default();
        Ok(OutBindResult {
            values,
            statement_cursor_id: response.result.cursor_id,
            implicit_results: response.implicit_results,
        })
    }

    pub fn describe(&mut self, sql: &str) -> Result<Vec<ColumnMetadata>, OracleThinError> {
        self.describe_request(&StatementRequest::query(sql, 1))
    }

    pub fn describe_request(
        &mut self,
        request: &StatementRequest,
    ) -> Result<Vec<ColumnMetadata>, OracleThinError> {
        let mut describe_request = request.clone();
        describe_request.prefetch_rows = 0;
        self.execute_request(&describe_request)
            .map(|response| response.columns)
    }

    pub fn query_described_fetch_all(
        &mut self,
        sql: impl Into<String>,
        fetch_array_size: u32,
    ) -> Result<DescribedQueryResult, OracleThinError> {
        let request = StatementRequest::query(sql.into(), fetch_array_size);
        self.query_described_fetch_all_request(&request)
    }

    pub fn query_described_fetch_all_request(
        &mut self,
        request: &StatementRequest,
    ) -> Result<DescribedQueryResult, OracleThinError> {
        let mut result = self.query_described_fetch_all_request_legacy(request)?;
        self.remember_last_row_for_open_fetch(&result);
        let Some(cursor_id) = result.result.cursor_id else {
            return Ok(result);
        };
        while !result.result.exhausted {
            let batch = self.fetch_ref_cursor_batch(
                cursor_id,
                &result.columns,
                request.fetch_array_size,
                false,
            )?;
            let no_rows = batch.rows.is_empty();
            result.result.rows.extend(batch.rows);
            result.result.exhausted = batch.exhausted || batch.cursor_id.is_none() || no_rows;
        }
        result.result.cursor_id = None;
        Ok(result)
    }

    fn query_described_fetch_all_request_legacy(
        &mut self,
        request: &StatementRequest,
    ) -> Result<DescribedQueryResult, OracleThinError> {
        let response = self.execute_request(request)?;
        Ok(DescribedQueryResult {
            columns: response.columns,
            result: response.result,
        })
    }

    pub fn query_described_initial_request(
        &mut self,
        request: &StatementRequest,
    ) -> Result<DescribedQueryResult, OracleThinError> {
        let result = self.query_described_initial_request_legacy(request)?;
        self.remember_last_row_for_open_fetch(&result);
        Ok(result)
    }

    fn query_described_initial_request_legacy(
        &mut self,
        request: &StatementRequest,
    ) -> Result<DescribedQueryResult, OracleThinError> {
        self.query_described_fetch_all_request_legacy(request)
    }

    pub fn fetch_ref_cursor_all(
        &mut self,
        cursor_id: u32,
        columns: Vec<ColumnMetadata>,
        fetch_array_size: u32,
    ) -> Result<DescribedQueryResult, OracleThinError> {
        let mut rows = Vec::new();
        loop {
            let result =
                self.fetch_ref_cursor_batch(cursor_id, &columns, fetch_array_size, false)?;
            rows.extend(result.rows);
            if result.exhausted || result.cursor_id.is_none() {
                break;
            }
        }
        Ok(DescribedQueryResult {
            columns,
            result: QueryResult {
                cursor_id: None,
                exhausted: true,
                rows,
            },
        })
    }

    pub fn fetch_nested_cursor_all(
        &mut self,
        cursor_id: u32,
        columns: Vec<ColumnMetadata>,
        fetch_array_size: u32,
    ) -> Result<DescribedQueryResult, OracleThinError> {
        self.fetch_ref_cursor_all(cursor_id, columns, fetch_array_size)
    }

    pub fn fetch_ref_cursor_batch(
        &mut self,
        cursor_id: u32,
        columns: &[ColumnMetadata],
        fetch_array_size: u32,
        needs_define_fetch: bool,
    ) -> Result<QueryResult, OracleThinError> {
        let row_count = fetch_array_size.max(1);
        let pending_cursor_closes = self.drain_pending_cursor_closes();
        let close_sequence = if pending_cursor_closes.is_empty() {
            None
        } else {
            Some(self.next_ttc_sequence())
        };
        let sequence = self.next_ttc_sequence();
        if needs_define_fetch {
            log_connect_phase("ttc-define-fetch-write", "");
            write_define_fetch_request(
                &mut self.stream,
                &self.capabilities,
                cursor_id,
                row_count,
                sequence,
                close_sequence,
                &pending_cursor_closes,
                columns,
            )?;
        } else {
            log_connect_phase("ttc-fetch-write", "");
            write_fetch_request(
                &mut self.stream,
                &self.capabilities,
                cursor_id,
                row_count,
                sequence,
                close_sequence,
                &pending_cursor_closes,
            )?;
        }
        log_connect_phase("ttc-fetch-read", "");
        let mut state = ExecuteReadState::default();
        state.columns = columns
            .iter()
            .map(thin_column_from_column_metadata)
            .collect();
        let request = StatementRequest::query("", row_count);
        read_execute_response_with_state(
            &mut self.stream,
            &self.capabilities,
            &request,
            state,
            close_sequence.is_some(),
        )
        .map(|response| response.result)
    }

    pub fn define_and_fetch_typed(
        &mut self,
        cursor_id: u32,
        row_count: u32,
        column_types: &[OracleColumnType],
    ) -> Result<QueryResult, OracleThinError> {
        let columns = column_types
            .iter()
            .enumerate()
            .map(|(index, column_type)| ColumnMetadata {
                name: format!("COL{}", index + 1),
                column_type: *column_type,
            })
            .collect::<Vec<_>>();
        self.fetch_ref_cursor_batch(cursor_id, &columns, row_count, true)
    }

    pub fn fetch_typed(
        &mut self,
        cursor_id: u32,
        row_count: u32,
        column_types: &[OracleColumnType],
    ) -> Result<QueryResult, OracleThinError> {
        let columns = column_types
            .iter()
            .enumerate()
            .map(|(index, column_type)| ColumnMetadata {
                name: format!("COL{}", index + 1),
                column_type: *column_type,
            })
            .collect::<Vec<_>>();
        self.fetch_ref_cursor_batch(cursor_id, &columns, row_count, false)
    }

    pub fn close_cursor_later(&mut self, cursor_id: Option<u32>) {
        if let Some(cursor_id) = cursor_id {
            self.pending_cursor_closes.push(cursor_id);
        }
    }

    pub fn close_cursor_on_next_call(&mut self, cursor_id: Option<u32>) {
        self.close_cursor_later(cursor_id);
    }

    pub fn flush_pending_cursor_closes(&mut self) -> Result<(), OracleThinError> {
        if self.pending_cursor_closes.is_empty() {
            return Ok(());
        }
        let cursor_ids = self.drain_pending_cursor_closes();
        let close_sequence = self.next_ttc_sequence();
        let ping_sequence = self.next_ttc_sequence();
        let mut payload = Vec::new();
        write_close_cursors_piggyback(
            &mut payload,
            &self.capabilities,
            close_sequence,
            &cursor_ids,
        )?;
        write_function_code(
            &mut payload,
            TNS_FUNC_PING,
            ping_sequence,
            &self.capabilities,
        );
        write_data_packet(
            &mut self.stream,
            self.capabilities.protocol_version.unwrap_or(319),
            &payload,
        )?;
        read_simple_response(&mut self.stream, &self.capabilities, true)
    }

    pub fn described_columns_require_define_fetch(columns: &[ColumnMetadata]) -> bool {
        columns.iter().any(|column| {
            matches!(
                column.column_type,
                OracleColumnType::Long
                    | OracleColumnType::Clob
                    | OracleColumnType::Nclob
                    | OracleColumnType::Blob
                    | OracleColumnType::Cursor
            )
        })
    }

    fn remember_last_row_for_open_fetch<T>(&mut self, _result: &T) {}

    fn execute_request(
        &mut self,
        request: &StatementRequest,
    ) -> Result<ExecuteResponse, OracleThinError> {
        log_connect_phase("ttc-execute-write", &request.sql);
        let pending_cursor_closes = self.drain_pending_cursor_closes();
        let close_sequence = if pending_cursor_closes.is_empty() {
            None
        } else {
            Some(self.next_ttc_sequence())
        };
        let sequence = self.next_ttc_sequence();
        write_execute_request(
            &mut self.stream,
            &self.capabilities,
            request,
            sequence,
            0,
            close_sequence,
            &pending_cursor_closes,
        )?;
        log_connect_phase("ttc-execute-read", "");
        let skip_empty_end_of_response = close_sequence.is_some()
            || (request.is_query
                && request.prefetch_rows == 0
                && request.sql.contains("SQ_INTERNAL_ROWID"));
        let response = read_execute_response(
            &mut self.stream,
            &self.capabilities,
            request,
            skip_empty_end_of_response,
        );
        match response {
            Ok(response) => {
                self.cancel_flag.store(false, Ordering::SeqCst);
                Ok(response)
            }
            Err(error) if self.cancel_flag.swap(false, Ordering::SeqCst) => {
                self.broken = true;
                Err(OracleThinError::new(
                    "ORA-01013: user requested cancel of current operation",
                ))
            }
            Err(error) => {
                self.close_cursor_later(error.cursor_id());
                Err(error)
            }
        }
    }

    fn simple_ttc_call(
        &mut self,
        function_code: u8,
        operation: &str,
    ) -> Result<(), OracleThinError> {
        log_connect_phase(&format!("ttc-{operation}-write"), "");
        let mut payload = Vec::new();
        let pending_cursor_closes = self.drain_pending_cursor_closes();
        if !pending_cursor_closes.is_empty() {
            let close_sequence = self.next_ttc_sequence();
            write_close_cursors_piggyback(
                &mut payload,
                &self.capabilities,
                close_sequence,
                &pending_cursor_closes,
            )?;
        }
        let sequence = self.next_ttc_sequence();
        write_function_code(&mut payload, function_code, sequence, &self.capabilities);
        write_data_packet(
            &mut self.stream,
            self.capabilities.protocol_version.unwrap_or(319),
            &payload,
        )?;
        log_connect_phase(&format!("ttc-{operation}-read"), "");
        read_simple_response(
            &mut self.stream,
            &self.capabilities,
            !pending_cursor_closes.is_empty(),
        )
    }

    fn next_ttc_sequence(&mut self) -> u8 {
        if self.ttc_sequence == 0 || self.ttc_sequence == u8::MAX {
            self.ttc_sequence = 1;
        }
        let sequence = self.ttc_sequence;
        self.ttc_sequence = self.ttc_sequence.wrapping_add(1);
        sequence
    }

    fn drain_pending_cursor_closes(&mut self) -> Vec<u32> {
        if self.pending_cursor_closes.is_empty() {
            return Vec::new();
        }
        let mut cursor_ids = std::mem::take(&mut self.pending_cursor_closes);
        cursor_ids.retain(|cursor_id| *cursor_id != 0);
        cursor_ids.sort_unstable();
        cursor_ids.dedup();
        cursor_ids
    }
}

#[cfg(unix)]
fn send_oob_break(stream: &TcpStream) -> Result<(), OracleThinError> {
    let value = b"!";
    let written = unsafe {
        libc::send(
            stream.as_raw_fd(),
            value.as_ptr().cast(),
            value.len(),
            libc::MSG_OOB,
        )
    };
    if written == value.len() as isize {
        Ok(())
    } else {
        Err(OracleThinError::from(std::io::Error::last_os_error()))
    }
}

#[cfg(not(unix))]
fn send_oob_break(_stream: &TcpStream) -> Result<(), OracleThinError> {
    Err(OracleThinError::new(
        "Oracle thin out-of-band break is not supported on this platform",
    ))
}

fn capabilities_from_accept(
    options: &ConnectOptions,
    accept: &AcceptInfo,
) -> OracleThinCapabilities {
    let ttc_field_version = options
        .desired_ttc_field_version
        .unwrap_or_else(|| default_ttc_field_version(accept.protocol_version));
    OracleThinCapabilities {
        protocol_version: Some(accept.protocol_version),
        ttc_field_version,
        charset_id: 0,
        ncharset_id: 0,
        max_string_size: 4000,
        supports_sql_boolean: ttc_field_version >= 23,
        supports_end_of_response: accept.supports_end_of_response(),
        supports_request_boundaries: false,
        supports_fast_auth: accept.supports_fast_auth(),
        supports_oob: accept.supports_oob_check(),
        supports_big_clr_chunks: false,
        supports_implicit_resultsets: accept.protocol_version >= 315,
    }
}

fn negotiate_protocol(
    stream: &mut TcpStream,
    capabilities: &mut OracleThinCapabilities,
) -> Result<(), OracleThinError> {
    log_connect_phase("ttc-protocol-write", "");
    let mut payload = vec![TNS_MSG_TYPE_PROTOCOL, 6, 0];
    payload.extend_from_slice(b"space-query-thin");
    payload.push(0);
    write_data_packet(
        stream,
        capabilities.protocol_version.unwrap_or(319),
        &payload,
    )?;

    log_connect_phase("ttc-protocol-read", "");
    let packet = read_data_packet(stream, capabilities.protocol_version.unwrap_or(319))?;
    let mut cursor = PacketCursor::new(&packet);
    let message_type = cursor.read_u8()?;
    if message_type != TNS_MSG_TYPE_PROTOCOL {
        return Err(OracleThinError::new(format!(
            "expected TTC protocol message, got message type {message_type}"
        )));
    }
    process_protocol_message(&mut cursor, capabilities)?;
    while cursor.remaining() > 0 {
        let message_type = cursor.read_u8()?;
        if message_type == TNS_MSG_TYPE_END_OF_RESPONSE {
            break;
        }
    }
    Ok(())
}

fn negotiate_data_types(
    stream: &mut TcpStream,
    capabilities: &OracleThinCapabilities,
) -> Result<(), OracleThinError> {
    log_connect_phase("ttc-data-types-write", "");
    let mut payload = Vec::new();
    payload.push(TNS_MSG_TYPE_DATA_TYPES);
    put_u16_le_vec(&mut payload, 873);
    put_u16_le_vec(&mut payload, 873);
    payload.push(0x01 | 0x02);
    write_len_bytes(&mut payload, &client_compile_caps(capabilities)?)?;
    write_len_bytes(&mut payload, &client_runtime_caps())?;
    for (data_type, conv_data_type, representation) in DATA_TYPE_REPRESENTATIONS {
        put_u16_be_vec(&mut payload, *data_type);
        put_u16_be_vec(&mut payload, *conv_data_type);
        put_u16_be_vec(&mut payload, *representation);
        put_u16_be_vec(&mut payload, 0);
    }
    put_u16_be_vec(&mut payload, 0);
    write_data_packet(
        stream,
        capabilities.protocol_version.unwrap_or(319),
        &payload,
    )?;

    log_connect_phase("ttc-data-types-read", "");
    let packet = read_data_packet(stream, capabilities.protocol_version.unwrap_or(319))?;
    let mut cursor =
        PacketCursor::with_big_clr_chunks(&packet, capabilities.supports_big_clr_chunks);
    let message_type = cursor.read_u8()?;
    if message_type != TNS_MSG_TYPE_DATA_TYPES {
        return Err(OracleThinError::new(format!(
            "expected TTC data type negotiation message, got message type {message_type}"
        )));
    }
    loop {
        let data_type = cursor.read_u16_be()?;
        if data_type == 0 {
            break;
        }
        let conv_data_type = cursor.read_u16_be()?;
        if conv_data_type != 0 {
            cursor.skip(4)?;
        }
    }
    log_connect_phase("ttc-data-types-accept", "");
    Ok(())
}

#[derive(Debug, Clone)]
struct ExecuteResponse {
    columns: Vec<ColumnMetadata>,
    result: QueryResult,
    out_bind_values: Option<Vec<OracleValue>>,
    implicit_results: Vec<RefCursorValue>,
}

#[derive(Debug, Clone)]
struct ThinColumn {
    name: String,
    column_type: OracleColumnType,
    ora_type_num: u8,
    charset_form: u8,
    buffer_size: u32,
}

#[derive(Debug, Default)]
struct ExecuteReadState {
    columns: Vec<ThinColumn>,
    rows: Vec<Vec<OracleValue>>,
    out_bind_columns: Vec<ThinColumn>,
    out_bind_rows: Vec<Vec<OracleValue>>,
    implicit_results: Vec<RefCursorValue>,
    last_row: Option<Vec<OracleValue>>,
    bit_vector: Option<Vec<u8>>,
    reading_out_binds: bool,
    cursor_id: Option<u32>,
    exhausted: bool,
    done: bool,
}

#[derive(Debug, Default)]
struct ExecuteError {
    code: u32,
    cursor_id: u32,
    _rowcount: u64,
    message: Option<String>,
}

fn write_execute_request(
    stream: &mut TcpStream,
    capabilities: &OracleThinCapabilities,
    request: &StatementRequest,
    sequence: u8,
    cursor_id: u32,
    close_sequence: Option<u8>,
    close_cursor_ids: &[u32],
) -> Result<(), OracleThinError> {
    if capabilities.ttc_field_version <= 6 {
        return write_legacy_execute_request(
            stream,
            capabilities,
            request,
            sequence,
            cursor_id,
            close_sequence,
            close_cursor_ids,
        );
    }

    let sql_bytes = request.sql.as_bytes();
    let parse_only_describe = request.is_query && request.prefetch_rows == 0;
    let mut options = TNS_EXEC_OPTION_PARSE;
    let num_params = if parse_only_describe {
        0
    } else {
        request.binds.len() as u32
    };
    let num_iters = if request.is_query {
        if parse_only_describe {
            options |= TNS_EXEC_OPTION_DESCRIBE;
            1
        } else {
            options |= TNS_EXEC_OPTION_EXECUTE;
            let rows = request.prefetch_rows;
            if rows > 0 {
                options |= TNS_EXEC_OPTION_FETCH;
            }
            rows
        }
    } else {
        options |= TNS_EXEC_OPTION_EXECUTE;
        1
    };
    if !request.is_plsql && !parse_only_describe {
        options |= TNS_EXEC_OPTION_NOT_PLSQL;
    } else if request.is_plsql && num_params > 0 {
        options |= TNS_EXEC_OPTION_PLSQL_BIND;
    }
    if num_params > 0 {
        options |= TNS_EXEC_OPTION_BIND;
    }
    if request.auto_commit && !parse_only_describe {
        options |= TNS_EXEC_OPTION_COMMIT;
    }
    let exec_flags = if !parse_only_describe {
        TNS_EXEC_FLAGS_IMPLICIT_RESULTSET
    } else {
        0
    };

    let mut payload = Vec::new();
    if let Some(close_sequence) = close_sequence {
        write_close_cursors_piggyback(
            &mut payload,
            capabilities,
            close_sequence,
            close_cursor_ids,
        )?;
    }
    write_function_code(&mut payload, TNS_FUNC_EXECUTE, sequence, capabilities);
    write_ub4(&mut payload, options);
    write_ub4(&mut payload, cursor_id);
    payload.push(1);
    write_ub4(&mut payload, sql_bytes.len() as u32);
    payload.push(1);
    write_ub4(&mut payload, 13);
    payload.push(0);
    payload.push(0);
    write_ub4(&mut payload, 0);
    write_ub4(&mut payload, num_iters);
    write_ub4(&mut payload, TNS_MAX_LONG_LENGTH);
    if num_params == 0 {
        payload.push(0);
        write_ub4(&mut payload, 0);
    } else {
        payload.push(1);
        write_ub4(&mut payload, num_params);
    }
    payload.push(0);
    payload.push(0);
    payload.push(0);
    payload.push(0);
    payload.push(0);
    payload.push(0);
    write_ub4(&mut payload, 0);
    write_ub4(&mut payload, 0);
    payload.push(0);
    payload.push(1);
    payload.push(0);
    write_ub4(&mut payload, 0);
    payload.push(0);
    write_ub4(&mut payload, 0);
    write_ub4(&mut payload, 0);
    payload.push(0);
    write_ub4(&mut payload, 0);
    payload.push(0);
    if capabilities.ttc_field_version >= TNS_CCAP_FIELD_VERSION_12_2 {
        payload.push(0);
        write_ub4(&mut payload, 0);
        payload.push(0);
        write_ub4(&mut payload, 0);
        payload.push(0);
        if capabilities.ttc_field_version >= TNS_CCAP_FIELD_VERSION_12_2_EXT1 {
            payload.push(0);
            write_ub4(&mut payload, 0);
        }
    }
    write_bytes_with_length(&mut payload, sql_bytes)?;
    write_ub4(&mut payload, 1);
    if request.is_query {
        write_ub4(&mut payload, 0);
    } else {
        write_ub4(&mut payload, 1);
    }
    write_ub4(&mut payload, 0);
    write_ub4(&mut payload, 0);
    write_ub4(&mut payload, 0);
    write_ub4(&mut payload, 0);
    write_ub4(&mut payload, 0);
    write_ub4(&mut payload, u32::from(request.is_query));
    write_ub4(&mut payload, 0);
    write_ub4(&mut payload, exec_flags);
    write_ub4(&mut payload, 0);
    write_ub4(&mut payload, 0);
    write_ub4(&mut payload, 0);
    if num_params > 0 {
        write_bind_metadata(&mut payload, capabilities, &request.binds)?;
        write_bind_rows(&mut payload, &request.binds)?;
    }
    if std::env::var_os("ORACLE_THIN_TRACE_EXEC").is_some() {
        eprintln!(
            "thin exec request options=0x{options:08x} binds={} sql={} payload={}",
            request.binds.len(),
            request.sql,
            hex_encode_upper(&payload)
        );
    }
    write_data_packet(
        stream,
        capabilities.protocol_version.unwrap_or(319),
        &payload,
    )
}

fn write_legacy_execute_request(
    stream: &mut TcpStream,
    capabilities: &OracleThinCapabilities,
    request: &StatementRequest,
    sequence: u8,
    cursor_id: u32,
    close_sequence: Option<u8>,
    close_cursor_ids: &[u32],
) -> Result<(), OracleThinError> {
    let sql_bytes = request.sql.as_bytes();
    let parse_only_describe = request.is_query && request.prefetch_rows == 0;
    let num_iters = if request.is_query {
        if parse_only_describe {
            1
        } else {
            request.prefetch_rows
        }
    } else {
        1
    };
    let num_params = if parse_only_describe {
        0
    } else {
        request.binds.len() as u16
    };
    let mut options = TNS_EXEC_OPTION_PARSE;
    if parse_only_describe {
        options |= TNS_EXEC_OPTION_DESCRIBE;
    } else {
        options |= TNS_EXEC_OPTION_EXECUTE;
    }
    if !request.is_plsql && !parse_only_describe {
        options |= TNS_EXEC_OPTION_NOT_PLSQL;
    } else if request.is_plsql && num_params > 0 {
        options |= TNS_EXEC_OPTION_PLSQL_BIND;
    }
    if num_params > 0 {
        options |= TNS_EXEC_OPTION_BIND;
    }
    if request.auto_commit && !parse_only_describe {
        options |= TNS_EXEC_OPTION_COMMIT;
    }

    let mut payload = Vec::new();
    if let Some(close_sequence) = close_sequence {
        write_close_cursors_piggyback(
            &mut payload,
            capabilities,
            close_sequence,
            close_cursor_ids,
        )?;
    }
    write_function_code(&mut payload, TNS_FUNC_EXECUTE, sequence, capabilities);
    write_ub4(&mut payload, options);
    write_ub2(&mut payload, cursor_id as u16);
    payload.push(if cursor_id == 0 { 1 } else { 0 });
    write_ub4(&mut payload, sql_bytes.len() as u32);
    payload.push(1);
    write_ub2(&mut payload, 13);
    payload.push(0);
    payload.push(0);
    if request.is_query {
        payload.push(0);
        write_ub4(&mut payload, num_iters);
    } else {
        payload.push(0);
        payload.push(0);
    }
    write_ub4(&mut payload, TNS_MAX_LONG_LENGTH);
    if num_params == 0 {
        payload.push(0);
        write_ub2(&mut payload, 0);
    } else {
        payload.push(1);
        write_ub2(&mut payload, num_params);
    }
    payload.push(0);
    payload.push(0);
    payload.push(0);
    payload.push(0);
    payload.push(0);
    payload.push(0);
    write_ub2(&mut payload, 0);
    payload.push(0);
    payload.push(0);
    payload.push(1);
    payload.push(0);
    payload.push(0);
    payload.push(0);
    payload.push(0);
    payload.push(0);
    write_bytes_with_length(&mut payload, sql_bytes)?;

    let mut al8i4 = [0u32; 13];
    al8i4[0] = 1;
    al8i4[1] = if request.is_query { 0 } else { 1 };
    al8i4[7] = u32::from(request.is_query);
    al8i4[9] = if !parse_only_describe {
        TNS_EXEC_FLAGS_IMPLICIT_RESULTSET
    } else {
        0
    };
    for value in al8i4 {
        write_ub4(&mut payload, value);
    }
    if num_params > 0 {
        write_bind_metadata(&mut payload, capabilities, &request.binds)?;
        write_bind_rows(&mut payload, &request.binds)?;
    }
    if std::env::var_os("ORACLE_THIN_TRACE_EXEC").is_some() {
        eprintln!(
            "thin legacy exec request options=0x{options:08x} binds={} sql={} payload={}",
            request.binds.len(),
            request.sql,
            hex_encode_upper(&payload)
        );
    }
    write_data_packet(
        stream,
        capabilities.protocol_version.unwrap_or(319),
        &payload,
    )
}

fn write_fetch_request(
    stream: &mut TcpStream,
    capabilities: &OracleThinCapabilities,
    cursor_id: u32,
    row_count: u32,
    sequence: u8,
    close_sequence: Option<u8>,
    close_cursor_ids: &[u32],
) -> Result<(), OracleThinError> {
    let mut payload = Vec::new();
    if let Some(close_sequence) = close_sequence {
        write_close_cursors_piggyback(
            &mut payload,
            capabilities,
            close_sequence,
            close_cursor_ids,
        )?;
    }
    write_function_code(&mut payload, TNS_FUNC_FETCH, sequence, capabilities);
    if capabilities.ttc_field_version <= 6 {
        write_ub2(&mut payload, cursor_id as u16);
        write_ub2(&mut payload, row_count as u16);
    } else {
        write_ub4(&mut payload, cursor_id);
        write_ub4(&mut payload, row_count);
    }
    if std::env::var_os("ORACLE_THIN_TRACE_EXEC").is_some() {
        eprintln!(
            "thin fetch request cursor={} rows={} payload={}",
            cursor_id,
            row_count,
            hex_encode_upper(&payload)
        );
    }
    write_data_packet(
        stream,
        capabilities.protocol_version.unwrap_or(319),
        &payload,
    )
}

fn write_define_fetch_request(
    stream: &mut TcpStream,
    capabilities: &OracleThinCapabilities,
    cursor_id: u32,
    row_count: u32,
    sequence: u8,
    close_sequence: Option<u8>,
    close_cursor_ids: &[u32],
    columns: &[ColumnMetadata],
) -> Result<(), OracleThinError> {
    let mut payload = Vec::new();
    if let Some(close_sequence) = close_sequence {
        write_close_cursors_piggyback(
            &mut payload,
            capabilities,
            close_sequence,
            close_cursor_ids,
        )?;
    }
    write_function_code(&mut payload, TNS_FUNC_EXECUTE, sequence, capabilities);
    let options = TNS_EXEC_OPTION_NOT_PLSQL | TNS_EXEC_OPTION_DEFINE | TNS_EXEC_OPTION_FETCH;
    write_ub4(&mut payload, options);
    if capabilities.ttc_field_version <= 6 {
        write_ub2(&mut payload, cursor_id as u16);
        payload.push(0);
        payload.push(0);
        payload.push(1);
        write_ub2(&mut payload, 13);
        payload.push(0);
        payload.push(0);
        payload.push(0);
        payload.push(0);
        write_ub4(&mut payload, TNS_MAX_LONG_LENGTH);
        payload.push(0);
        write_ub2(&mut payload, 0);
        payload.push(0);
        payload.push(0);
        payload.push(0);
        payload.push(0);
        payload.push(0);
        payload.push(1);
        write_ub2(&mut payload, columns.len() as u16);
        payload.push(0);
        payload.push(0);
        payload.push(1);
        payload.push(0);
        payload.push(0);
        payload.push(0);
        payload.push(0);
        payload.push(0);
        let mut al8i4 = [0u32; 13];
        al8i4[1] = row_count;
        al8i4[7] = 1;
        for value in al8i4 {
            write_ub4(&mut payload, value);
        }
    } else {
        write_ub4(&mut payload, cursor_id);
        payload.push(0);
        write_ub4(&mut payload, 0);
        payload.push(1);
        write_ub4(&mut payload, 13);
        payload.push(0);
        payload.push(0);
        write_ub4(&mut payload, 0);
        write_ub4(&mut payload, row_count);
        write_ub4(&mut payload, TNS_MAX_LONG_LENGTH);
        payload.push(0);
        write_ub4(&mut payload, 0);
        payload.push(0);
        payload.push(0);
        payload.push(0);
        payload.push(0);
        payload.push(0);
        payload.push(1);
        write_ub4(&mut payload, columns.len() as u32);
        write_ub4(&mut payload, 0);
        payload.push(0);
        payload.push(1);
        payload.push(0);
        write_ub4(&mut payload, 0);
        payload.push(0);
        write_ub4(&mut payload, 0);
        write_ub4(&mut payload, 0);
        payload.push(0);
        write_ub4(&mut payload, 0);
        payload.push(0);
        if capabilities.ttc_field_version >= TNS_CCAP_FIELD_VERSION_12_2 {
            payload.push(0);
            write_ub4(&mut payload, 0);
            payload.push(0);
            write_ub4(&mut payload, 0);
            payload.push(0);
            if capabilities.ttc_field_version >= TNS_CCAP_FIELD_VERSION_12_2_EXT1 {
                payload.push(0);
                write_ub4(&mut payload, 0);
            }
        }
        let mut al8i4 = [0u32; 13];
        al8i4[1] = row_count;
        al8i4[7] = 1;
        for value in al8i4 {
            write_ub4(&mut payload, value);
        }
    }
    write_define_metadata(&mut payload, capabilities, columns)?;
    if std::env::var_os("ORACLE_THIN_TRACE_EXEC").is_some() {
        eprintln!(
            "thin define fetch request cursor={} rows={} columns={} payload={}",
            cursor_id,
            row_count,
            columns.len(),
            hex_encode_upper(&payload)
        );
    }
    write_data_packet(
        stream,
        capabilities.protocol_version.unwrap_or(319),
        &payload,
    )
}

fn write_close_cursors_piggyback(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    sequence: u8,
    cursor_ids: &[u32],
) -> Result<(), OracleThinError> {
    if cursor_ids.is_empty() {
        return Ok(());
    }
    write_piggyback_code(payload, TNS_FUNC_CLOSE_CURSORS, sequence, capabilities);
    payload.push(1);
    write_ub4(payload, cursor_ids.len() as u32);
    for cursor_id in cursor_ids {
        write_ub4(payload, *cursor_id);
    }
    Ok(())
}

fn write_bind_metadata(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    binds: &[BindValue],
) -> Result<(), OracleThinError> {
    for bind in binds {
        let column = bind_column_metadata(bind);
        write_column_metadata(payload, capabilities, &column)?;
    }
    Ok(())
}

fn write_define_metadata(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    columns: &[ColumnMetadata],
) -> Result<(), OracleThinError> {
    for column in columns {
        let column = define_column_metadata(column);
        write_column_metadata(payload, capabilities, &column)?;
    }
    Ok(())
}

fn write_column_metadata(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    column: &ThinColumn,
) -> Result<(), OracleThinError> {
    payload.push(column.ora_type_num);
    payload.push(TNS_BIND_USE_INDICATORS);
    payload.push(0);
    payload.push(0);
    write_ub4(payload, column.buffer_size);
    write_ub4(payload, 0);
    if capabilities.ttc_field_version <= 6 {
        write_ub4(payload, 0);
        payload.push(0);
    } else {
        write_ub8(payload, 0);
        write_ub4(payload, 0);
    }
    write_ub2(payload, 0);
    if column.charset_form != 0 {
        write_ub2(payload, TNS_CHARSET_UTF8);
    } else {
        write_ub2(payload, 0);
    }
    payload.push(column.charset_form);
    write_ub4(payload, 0);
    if capabilities.ttc_field_version >= TNS_CCAP_FIELD_VERSION_12_2 {
        write_ub4(payload, 0);
    }
    Ok(())
}

fn write_bind_rows(payload: &mut Vec<u8>, binds: &[BindValue]) -> Result<(), OracleThinError> {
    payload.push(TNS_MSG_TYPE_ROW_DATA);
    for bind in binds {
        write_bind_value(payload, bind)?;
    }
    Ok(())
}

fn write_bind_value(payload: &mut Vec<u8>, bind: &BindValue) -> Result<(), OracleThinError> {
    match bind {
        BindValue::Null(_) | BindValue::Out { .. } => {
            payload.push(0);
            Ok(())
        }
        BindValue::Number(value) => write_oracle_number(payload, value),
        BindValue::Text(value) => write_bytes_with_length(payload, value.as_bytes()),
        BindValue::Boolean(value) => {
            if *value {
                write_bytes_with_length(payload, &[1, 1])
            } else {
                write_bytes_with_length(payload, &[0])
            }
        }
        BindValue::Date(value) => write_bytes_with_length(payload, &encode_oracle_date(value, 7)),
        BindValue::Timestamp(value) => {
            write_bytes_with_length(payload, &encode_oracle_date(value, 11))
        }
        BindValue::InOut { value, .. } => match value {
            Some(BindInputValue::Number(value)) => write_oracle_number(payload, value),
            Some(BindInputValue::Text(value)) => write_bytes_with_length(payload, value.as_bytes()),
            Some(BindInputValue::Boolean(value)) => {
                if *value {
                    write_bytes_with_length(payload, &[1, 1])
                } else {
                    write_bytes_with_length(payload, &[0])
                }
            }
            Some(BindInputValue::Date(value)) => {
                write_bytes_with_length(payload, &encode_oracle_date(value, 7))
            }
            Some(BindInputValue::Timestamp(value)) => {
                write_bytes_with_length(payload, &encode_oracle_date(value, 11))
            }
            None => {
                payload.push(0);
                Ok(())
            }
        },
    }
}

fn bind_column_metadata(bind: &BindValue) -> ThinColumn {
    let (column_type, max_len) = match bind {
        BindValue::Null(column_type) => (*column_type, default_bind_len(*column_type)),
        BindValue::Number(_) => (OracleColumnType::Number, 22),
        BindValue::Text(value) => (
            OracleColumnType::Varchar,
            value.len().saturating_mul(4).max(1) as u32,
        ),
        BindValue::Boolean(_) => (OracleColumnType::Boolean, 4),
        BindValue::Date(_) => (OracleColumnType::Date, 7),
        BindValue::Timestamp(_) => (OracleColumnType::Timestamp, 11),
        BindValue::Out {
            column_type,
            max_len,
        } => (*column_type, (*max_len).max(default_bind_len(*column_type))),
        BindValue::InOut {
            column_type,
            max_len,
            ..
        } => (*column_type, (*max_len).max(default_bind_len(*column_type))),
    };
    let ora_type_num = match column_type {
        OracleColumnType::Varchar | OracleColumnType::Long | OracleColumnType::Clob => {
            ORA_TYPE_NUM_VARCHAR
        }
        OracleColumnType::Number => ORA_TYPE_NUM_NUMBER,
        OracleColumnType::Date => ORA_TYPE_NUM_DATE,
        OracleColumnType::Timestamp => ORA_TYPE_NUM_TIMESTAMP,
        OracleColumnType::Boolean => ORA_TYPE_NUM_BOOLEAN,
        OracleColumnType::Raw | OracleColumnType::Blob => ORA_TYPE_NUM_RAW,
        OracleColumnType::Nclob => ORA_TYPE_NUM_VARCHAR,
        OracleColumnType::Cursor => ORA_TYPE_NUM_CURSOR,
    };
    let charset_form = if matches!(
        column_type,
        OracleColumnType::Varchar | OracleColumnType::Long | OracleColumnType::Clob
    ) {
        1
    } else {
        0
    };
    ThinColumn {
        name: String::new(),
        column_type,
        ora_type_num,
        charset_form,
        buffer_size: max_len,
    }
}

fn thin_column_from_column_metadata(column: &ColumnMetadata) -> ThinColumn {
    let bind_like = match column.column_type {
        OracleColumnType::Varchar => BindValue::Null(OracleColumnType::Varchar),
        OracleColumnType::Number => BindValue::Null(OracleColumnType::Number),
        OracleColumnType::Date => BindValue::Null(OracleColumnType::Date),
        OracleColumnType::Timestamp => BindValue::Null(OracleColumnType::Timestamp),
        OracleColumnType::Boolean => BindValue::Null(OracleColumnType::Boolean),
        OracleColumnType::Raw => BindValue::Null(OracleColumnType::Raw),
        OracleColumnType::Long => BindValue::Null(OracleColumnType::Long),
        OracleColumnType::Clob => BindValue::Null(OracleColumnType::Clob),
        OracleColumnType::Nclob => BindValue::Null(OracleColumnType::Nclob),
        OracleColumnType::Blob => BindValue::Null(OracleColumnType::Blob),
        OracleColumnType::Cursor => BindValue::Null(OracleColumnType::Cursor),
    };
    let mut thin = bind_column_metadata(&bind_like);
    thin.name = column.name.clone();
    if column.column_type == OracleColumnType::Long {
        thin.ora_type_num = ORA_TYPE_NUM_LONG;
    }
    thin
}

fn define_column_metadata(column: &ColumnMetadata) -> ThinColumn {
    let mut thin = thin_column_from_column_metadata(column);
    match column.column_type {
        OracleColumnType::Long => {
            thin.ora_type_num = ORA_TYPE_NUM_LONG;
            thin.buffer_size = TNS_MAX_LONG_LENGTH;
            thin.charset_form = 1;
        }
        OracleColumnType::Clob | OracleColumnType::Nclob => {
            thin.ora_type_num = ORA_TYPE_NUM_LONG;
            thin.buffer_size = TNS_MAX_LONG_LENGTH;
            thin.charset_form = 1;
            thin.column_type = OracleColumnType::Long;
        }
        OracleColumnType::Blob => {
            thin.ora_type_num = ORA_TYPE_NUM_LONG_RAW;
            thin.buffer_size = TNS_MAX_LONG_LENGTH;
            thin.column_type = OracleColumnType::Raw;
        }
        _ => {}
    }
    thin
}

fn default_bind_len(column_type: OracleColumnType) -> u32 {
    match column_type {
        OracleColumnType::Varchar | OracleColumnType::Long | OracleColumnType::Clob => 4000,
        OracleColumnType::Number => 22,
        OracleColumnType::Date => 7,
        OracleColumnType::Timestamp => 11,
        OracleColumnType::Boolean => 4,
        OracleColumnType::Raw | OracleColumnType::Blob => 2000,
        OracleColumnType::Nclob => 4000,
        OracleColumnType::Cursor => 1,
    }
}

fn write_oracle_number(payload: &mut Vec<u8>, value: &str) -> Result<(), OracleThinError> {
    let bytes = encode_oracle_number(value)?;
    write_bytes_with_length(payload, &bytes)
}

fn encode_oracle_date(value: &crate::OracleDateTime, length: usize) -> Vec<u8> {
    let mut bytes = vec![
        (value.year / 100) as u8 + 100,
        (value.year % 100) as u8 + 100,
        value.month,
        value.day,
        value.hour + 1,
        value.minute + 1,
        value.second + 1,
    ];
    if length > 7 {
        bytes.extend_from_slice(&value.nanosecond.to_be_bytes());
    }
    bytes
}

fn encode_oracle_number(value: &str) -> Result<Vec<u8>, OracleThinError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(OracleThinError::new("empty Oracle NUMBER bind value"));
    }
    let bytes = value.as_bytes();
    let mut pos = 0usize;
    let mut is_negative = false;
    if bytes[pos] == b'-' {
        is_negative = true;
        pos += 1;
    } else if bytes[pos] == b'+' {
        pos += 1;
    }
    if pos >= bytes.len() {
        return Err(OracleThinError::new(format!(
            "invalid Oracle NUMBER bind value: {value}"
        )));
    }

    let mut digits = Vec::new();
    let mut decimal_point_index: i16;
    while pos < bytes.len() {
        match bytes[pos] {
            b'0'..=b'9' => {
                let digit = bytes[pos] - b'0';
                pos += 1;
                if digit == 0 && digits.is_empty() {
                    continue;
                }
                digits.push(digit);
            }
            b'.' | b'e' | b'E' => break,
            _ => {
                return Err(OracleThinError::new(format!(
                    "invalid Oracle NUMBER bind value: {value}"
                )));
            }
        }
    }
    decimal_point_index = digits.len() as i16;

    if pos < bytes.len() && bytes[pos] == b'.' {
        pos += 1;
        while pos < bytes.len() {
            match bytes[pos] {
                b'0'..=b'9' => {
                    let digit = bytes[pos] - b'0';
                    pos += 1;
                    if digit == 0 && digits.is_empty() {
                        decimal_point_index -= 1;
                        continue;
                    }
                    digits.push(digit);
                }
                b'e' | b'E' => break,
                _ => {
                    return Err(OracleThinError::new(format!(
                        "invalid Oracle NUMBER bind value: {value}"
                    )));
                }
            }
        }
    }

    if pos < bytes.len() && matches!(bytes[pos], b'e' | b'E') {
        pos += 1;
        let mut exponent_negative = false;
        if pos < bytes.len() && bytes[pos] == b'-' {
            exponent_negative = true;
            pos += 1;
        } else if pos < bytes.len() && bytes[pos] == b'+' {
            pos += 1;
        }
        let exponent_start = pos;
        while pos < bytes.len() && bytes[pos].is_ascii_digit() {
            pos += 1;
        }
        if exponent_start == pos || pos < bytes.len() {
            return Err(OracleThinError::new(format!(
                "invalid Oracle NUMBER exponent in bind value: {value}"
            )));
        }
        let exponent_text = std::str::from_utf8(&bytes[exponent_start..pos])
            .map_err(|err| OracleThinError::new(err.to_string()))?;
        let mut exponent = exponent_text
            .parse::<i16>()
            .map_err(|err| OracleThinError::new(err.to_string()))?;
        if exponent_negative {
            exponent = -exponent;
        }
        decimal_point_index += exponent;
    } else if pos < bytes.len() {
        return Err(OracleThinError::new(format!(
            "invalid Oracle NUMBER bind value: {value}"
        )));
    }

    while digits.last() == Some(&0) {
        digits.pop();
    }
    if digits.is_empty() {
        return Ok(vec![0x80]);
    }
    if digits.len() > 38 || decimal_point_index > 126 || decimal_point_index < -129 {
        return Err(OracleThinError::new(format!(
            "Oracle NUMBER bind value out of range: {value}"
        )));
    }

    let mut prepend_zero = false;
    if decimal_point_index % 2 != 0 {
        prepend_zero = true;
        digits.push(0);
        decimal_point_index += 1;
    }
    if digits.len() % 2 != 0 {
        digits.push(0);
    }

    let num_pairs = digits.len() / 2;
    let mut out = Vec::with_capacity(num_pairs + 2);
    let exponent_on_wire = (decimal_point_index / 2 + 192) as u8;
    out.push(if is_negative {
        !exponent_on_wire
    } else {
        exponent_on_wire
    });

    let mut digits_pos = 0usize;
    for pair_num in 0..num_pairs {
        let mut digit = if pair_num == 0 && prepend_zero {
            let digit = digits[digits_pos];
            digits_pos += 1;
            digit
        } else {
            let digit = digits[digits_pos] * 10 + digits[digits_pos + 1];
            digits_pos += 2;
            digit
        };
        if is_negative {
            digit = 101 - digit;
        } else {
            digit += 1;
        }
        out.push(digit);
    }
    if is_negative && digits.len() < 38 {
        out.push(102);
    }
    Ok(out)
}

fn read_execute_response(
    stream: &mut TcpStream,
    capabilities: &OracleThinCapabilities,
    request: &StatementRequest,
    skip_empty_end_of_response: bool,
) -> Result<ExecuteResponse, OracleThinError> {
    read_execute_response_with_state(
        stream,
        capabilities,
        request,
        ExecuteReadState::default(),
        skip_empty_end_of_response,
    )
}

fn read_execute_response_with_state(
    stream: &mut TcpStream,
    capabilities: &OracleThinCapabilities,
    request: &StatementRequest,
    mut state: ExecuteReadState,
    mut skip_empty_end_of_response: bool,
) -> Result<ExecuteResponse, OracleThinError> {
    let mut pending_error = None;
    let mut pending_fragment = Vec::new();
    let mut pending_fragment_error = None;
    let mut response_had_content = false;
    while !state.done {
        let (data_flags, packet) =
            read_data_packet_with_flags(stream, capabilities.protocol_version.unwrap_or(319))?;
        if std::env::var_os("ORACLE_THIN_TRACE_EXEC").is_some() {
            eprintln!(
                "thin exec response data_flags=0x{data_flags:04x} packet={}",
                hex_encode_upper(&packet)
            );
        }
        let packet = if pending_fragment.is_empty() {
            packet
        } else {
            pending_fragment.extend_from_slice(&packet);
            std::mem::take(&mut pending_fragment)
        };
        let mut cursor =
            PacketCursor::with_big_clr_chunks(&packet, capabilities.supports_big_clr_chunks);
        let mut skipped_empty_end_of_response = false;
        while cursor.remaining() > 0 && !state.done {
            let message_offset = cursor.pos;
            let message_type = match cursor.read_u8() {
                Ok(message_type) => message_type,
                Err(error) if is_incomplete_ttc_packet_error(&error) => {
                    pending_fragment = packet[message_offset..].to_vec();
                    pending_fragment_error = Some(error);
                    break;
                }
                Err(error) => return Err(error),
            };
            if std::env::var_os("ORACLE_THIN_TRACE_EXEC").is_some() {
                eprintln!(
                    "thin exec response message type={} offset={} remaining={}",
                    message_type,
                    message_offset,
                    cursor.remaining()
                );
            }
            if message_type != TNS_MSG_TYPE_END_OF_RESPONSE {
                response_had_content = true;
            }
            let result = (|| -> Result<(), OracleThinError> {
                match message_type {
                    TNS_MSG_TYPE_ROW_HEADER => process_row_header(&mut cursor, &mut state),
                    TNS_MSG_TYPE_ROW_DATA => {
                        process_row_data(&mut cursor, capabilities, &mut state)
                    }
                    TNS_MSG_TYPE_IO_VECTOR => process_io_vector(&mut cursor, request, &mut state),
                    TNS_MSG_TYPE_FLUSH_OUT_BINDS => write_data_packet(
                        stream,
                        capabilities.protocol_version.unwrap_or(319),
                        &[TNS_MSG_TYPE_FLUSH_OUT_BINDS],
                    ),
                    TNS_MSG_TYPE_DESCRIBE_INFO => {
                        process_describe_info(&mut cursor, capabilities, &mut state)
                    }
                    TNS_MSG_TYPE_BIT_VECTOR => process_bit_vector(&mut cursor, &mut state),
                    TNS_MSG_TYPE_IMPLICIT_RESULTSET => {
                        process_implicit_results(&mut cursor, capabilities, &mut state)
                    }
                    TNS_MSG_TYPE_PARAMETER => process_return_parameters(&mut cursor),
                    TNS_MSG_TYPE_STATUS => {
                        let _ = cursor.read_ub4()?;
                        let _ = cursor.read_ub2()?;
                        if !capabilities.supports_end_of_response {
                            state.done = true;
                        }
                        Ok(())
                    }
                    TNS_MSG_TYPE_TOKEN => {
                        let _ = cursor.read_ub8()?;
                        Ok(())
                    }
                    TNS_MSG_TYPE_WARNING => process_warning(&mut cursor),
                    TNS_MSG_TYPE_ERROR => {
                        let error = process_execute_error(&mut cursor, capabilities)?;
                        if error.cursor_id != 0 {
                            state.cursor_id = Some(error.cursor_id);
                        }
                        if error.code == TNS_ERR_NO_DATA_FOUND && request.is_query {
                            state.exhausted = true;
                            if !capabilities.supports_end_of_response {
                                state.done = true;
                            }
                        } else if error.code != 0 {
                            let cursor_id = if error.cursor_id != 0 {
                                Some(error.cursor_id)
                            } else {
                                state.cursor_id
                            };
                            pending_error.get_or_insert_with(|| {
                                OracleThinError::with_cursor_id(
                                    error.message.unwrap_or_else(|| {
                                        format!("Oracle error ORA-{:05}", error.code)
                                    }),
                                    cursor_id,
                                )
                            });
                            if !capabilities.supports_end_of_response {
                                state.done = true;
                            }
                        } else if !capabilities.supports_end_of_response {
                            state.done = true;
                        }
                        Ok(())
                    }
                    TNS_MSG_TYPE_SERVER_SIDE_PIGGYBACK => {
                        process_server_side_piggyback(&mut cursor)
                    }
                    TNS_MSG_TYPE_END_OF_RESPONSE => {
                        if !response_had_content {
                            skip_empty_end_of_response = false;
                            skipped_empty_end_of_response = true;
                        } else {
                            state.done = true;
                        }
                        Ok(())
                    }
                    other => {
                        let context_end = packet.len().min(message_offset + 32);
                        Err(OracleThinError::new(format!(
                            "unexpected Oracle execute response message type {other} at offset {message_offset}; context={}",
                            hex_encode_upper(&packet[message_offset..context_end])
                        )))
                    }
                }
            })();
            if let Err(error) = result {
                if is_incomplete_ttc_packet_error(&error) {
                    pending_fragment = packet[message_offset..].to_vec();
                    pending_fragment_error = Some(error);
                    break;
                }
                return Err(error);
            }
        }
        let has_end_flag = data_flags & (TNS_DATA_FLAGS_END_OF_RESPONSE | TNS_DATA_FLAGS_EOF) != 0;
        if !pending_fragment.is_empty() {
            if has_end_flag {
                return Err(pending_fragment_error.take().unwrap_or_else(|| {
                    OracleThinError::new("incomplete TTC message at end of response")
                }));
            }
            continue;
        }
        if has_end_flag {
            if skip_empty_end_of_response && !response_had_content {
                skip_empty_end_of_response = false;
                skipped_empty_end_of_response = true;
            }
            if !skipped_empty_end_of_response {
                state.done = true;
            }
        }
    }

    if let Some(error) = pending_error {
        return Err(error);
    }

    let columns = state
        .columns
        .iter()
        .map(|column| ColumnMetadata {
            name: column.name.clone(),
            column_type: column.column_type,
        })
        .collect();
    Ok(ExecuteResponse {
        columns,
        result: QueryResult {
            cursor_id: state.cursor_id.filter(|_| !state.exhausted),
            exhausted: state.exhausted || !request.is_query,
            rows: state.rows,
        },
        out_bind_values: state.out_bind_rows.into_iter().next(),
        implicit_results: state.implicit_results,
    })
}

fn is_incomplete_ttc_packet_error(error: &OracleThinError) -> bool {
    let message = error.to_string();
    message.starts_with("short TTC packet")
        || message.starts_with("unterminated TTC null-terminated byte field")
}

fn read_simple_response(
    stream: &mut TcpStream,
    capabilities: &OracleThinCapabilities,
    mut skip_empty_end_of_response: bool,
) -> Result<(), OracleThinError> {
    let mut done = false;
    let mut response_had_content = false;
    while !done {
        let (data_flags, packet) =
            read_data_packet_with_flags(stream, capabilities.protocol_version.unwrap_or(319))?;
        let mut cursor =
            PacketCursor::with_big_clr_chunks(&packet, capabilities.supports_big_clr_chunks);
        let mut skipped_empty_end_of_response = false;
        while cursor.remaining() > 0 && !done {
            let message_type = cursor.read_u8()?;
            if message_type != TNS_MSG_TYPE_END_OF_RESPONSE {
                response_had_content = true;
            }
            match message_type {
                TNS_MSG_TYPE_STATUS => {
                    let _ = cursor.read_ub4()?;
                    let _ = cursor.read_ub2()?;
                    if !capabilities.supports_end_of_response {
                        done = true;
                    }
                }
                TNS_MSG_TYPE_ERROR => {
                    let error = process_execute_error(&mut cursor, capabilities)?;
                    if error.code != 0 {
                        return Err(OracleThinError::new(
                            error
                                .message
                                .unwrap_or_else(|| format!("Oracle error ORA-{:05}", error.code)),
                        ));
                    }
                    if !capabilities.supports_end_of_response {
                        done = true;
                    }
                }
                TNS_MSG_TYPE_TOKEN => {
                    let _ = cursor.read_ub8()?;
                }
                TNS_MSG_TYPE_WARNING => process_warning(&mut cursor)?,
                TNS_MSG_TYPE_PARAMETER => process_return_parameters(&mut cursor)?,
                TNS_MSG_TYPE_SERVER_SIDE_PIGGYBACK => process_server_side_piggyback(&mut cursor)?,
                TNS_MSG_TYPE_END_OF_RESPONSE => {
                    if !response_had_content {
                        skip_empty_end_of_response = false;
                        skipped_empty_end_of_response = true;
                    } else {
                        done = true;
                    }
                }
                other => {
                    return Err(OracleThinError::new(format!(
                        "unexpected Oracle response message type {other}"
                    )));
                }
            }
        }
        if data_flags & (TNS_DATA_FLAGS_END_OF_RESPONSE | TNS_DATA_FLAGS_EOF) != 0 {
            if skip_empty_end_of_response && !response_had_content {
                skip_empty_end_of_response = false;
                skipped_empty_end_of_response = true;
            }
            if !skipped_empty_end_of_response {
                done = true;
            }
        }
    }
    Ok(())
}

fn process_describe_info(
    cursor: &mut PacketCursor<'_>,
    capabilities: &OracleThinCapabilities,
    state: &mut ExecuteReadState,
) -> Result<(), OracleThinError> {
    let _ = cursor.read_bytes()?;
    process_describe_body(cursor, capabilities, state)
}

fn process_describe_body(
    cursor: &mut PacketCursor<'_>,
    capabilities: &OracleThinCapabilities,
    state: &mut ExecuteReadState,
) -> Result<(), OracleThinError> {
    let _ = cursor.read_ub4()?;
    let num_columns = cursor.read_ub4()? as usize;
    if num_columns > 0 {
        cursor.skip(1)?;
    }
    let previous_columns = state.columns.clone();
    let mut columns = Vec::with_capacity(num_columns);
    for _ in 0..num_columns {
        columns.push(process_column_metadata(cursor, capabilities)?);
    }
    adjust_columns_after_define(&previous_columns, &mut columns);
    cursor.skip_bytes_with_ub4_length()?;
    let _ = cursor.read_ub4()?;
    let _ = cursor.read_ub4()?;
    let _ = cursor.read_ub4()?;
    let _ = cursor.read_ub4()?;
    cursor.skip_bytes_with_ub4_length()?;
    state.columns = columns;
    Ok(())
}

fn adjust_columns_after_define(previous_columns: &[ThinColumn], columns: &mut [ThinColumn]) {
    for (previous, column) in previous_columns.iter().zip(columns.iter_mut()) {
        match (previous.ora_type_num, column.ora_type_num) {
            (ORA_TYPE_NUM_LONG, ORA_TYPE_NUM_CLOB) => {
                column.ora_type_num = ORA_TYPE_NUM_LONG;
                column.column_type = OracleColumnType::Long;
                column.charset_form = previous.charset_form;
            }
            (ORA_TYPE_NUM_LONG_RAW, ORA_TYPE_NUM_BLOB) => {
                column.ora_type_num = ORA_TYPE_NUM_LONG_RAW;
                column.column_type = OracleColumnType::Raw;
            }
            _ => {}
        }
    }
}

fn process_io_vector(
    cursor: &mut PacketCursor<'_>,
    request: &StatementRequest,
    state: &mut ExecuteReadState,
) -> Result<(), OracleThinError> {
    cursor.skip(1)?;
    let low_binds = cursor.read_ub2()? as u32;
    let high_binds = cursor.read_ub4()?;
    let num_binds = high_binds.saturating_mul(256).saturating_add(low_binds) as usize;
    let _ = cursor.read_ub4()?;
    let _ = cursor.read_ub2()?;
    let num_bytes = cursor.read_ub2()? as usize;
    if num_bytes > 0 {
        cursor.skip(num_bytes)?;
    }
    let num_bytes = cursor.read_ub2()? as usize;
    if num_bytes > 0 {
        cursor.skip(num_bytes)?;
    }

    let mut out_bind_columns = Vec::new();
    for index in 0..num_binds {
        let bind_dir = cursor.read_u8()?;
        if bind_dir == TNS_BIND_DIR_INPUT {
            continue;
        }
        let Some(bind) = request.binds.get(index) else {
            continue;
        };
        out_bind_columns.push(bind_column_metadata(bind));
    }
    state.out_bind_columns = out_bind_columns;
    state.reading_out_binds = !state.out_bind_columns.is_empty();
    Ok(())
}

fn process_implicit_results(
    cursor: &mut PacketCursor<'_>,
    capabilities: &OracleThinCapabilities,
    state: &mut ExecuteReadState,
) -> Result<(), OracleThinError> {
    let num_results = cursor.read_ub4()? as usize;
    let mut implicit_results = Vec::with_capacity(num_results);
    for _ in 0..num_results {
        let num_bytes = cursor.read_u8()? as usize;
        if num_bytes > 0 {
            cursor.skip(num_bytes)?;
        }
        let mut child_state = ExecuteReadState::default();
        process_describe_body(cursor, capabilities, &mut child_state)?;
        let cursor_id = cursor.read_ub2()? as u32;
        let columns = child_state
            .columns
            .into_iter()
            .map(|column| ColumnMetadata {
                name: column.name,
                column_type: column.column_type,
            })
            .collect();
        implicit_results.push(RefCursorValue { cursor_id, columns });
    }
    state.implicit_results.extend(implicit_results);
    Ok(())
}

fn process_column_metadata(
    cursor: &mut PacketCursor<'_>,
    capabilities: &OracleThinCapabilities,
) -> Result<ThinColumn, OracleThinError> {
    let ora_type_num = cursor.read_u8()?;
    cursor.skip(1)?;
    let _ = cursor.read_i8()?;
    let _ = cursor.read_i8()?;
    let buffer_size = cursor.read_ub4()?;
    let _ = cursor.read_ub4()?;
    let _ = cursor.read_ub8()?;
    let _ = cursor.read_bytes()?;
    let _ = cursor.read_ub2()?;
    let _ = cursor.read_ub2()?;
    let charset_form = cursor.read_u8()?;
    let _ = cursor.read_ub4()?;
    if capabilities.ttc_field_version >= TNS_CCAP_FIELD_VERSION_12_2 {
        let _ = cursor.read_ub4()?;
    }
    let _ = cursor.read_u8()?;
    cursor.skip(1)?;
    let name = cursor.read_str_with_length()?.unwrap_or_default();
    let _ = cursor.read_str_with_length()?;
    let _ = cursor.read_str_with_length()?;
    let _ = cursor.read_ub2()?;
    let _ = cursor.read_ub4()?;
    if capabilities.ttc_field_version >= TNS_CCAP_FIELD_VERSION_23_1 {
        let _ = cursor.read_str_with_length()?;
        let _ = cursor.read_str_with_length()?;
    }
    if capabilities.ttc_field_version >= TNS_CCAP_FIELD_VERSION_23_1_EXT_3 {
        let num_annotations = cursor.read_ub4()?;
        if num_annotations > 0 {
            cursor.skip(1)?;
            let repeated_annotations = cursor.read_ub4()?;
            cursor.skip(1)?;
            for _ in 0..repeated_annotations {
                let _ = cursor.read_str_with_length()?;
                let _ = cursor.read_str_with_length()?;
                let _ = cursor.read_ub4()?;
            }
            let _ = cursor.read_ub4()?;
        }
    }
    if capabilities.ttc_field_version >= TNS_CCAP_FIELD_VERSION_23_4 {
        let _ = cursor.read_ub4()?;
        cursor.skip(2)?;
    }
    Ok(ThinColumn {
        name,
        column_type: oracle_column_type_from_ora_type(ora_type_num),
        ora_type_num,
        charset_form,
        buffer_size,
    })
}

fn process_row_header(
    cursor: &mut PacketCursor<'_>,
    state: &mut ExecuteReadState,
) -> Result<(), OracleThinError> {
    cursor.skip(1)?;
    let _ = cursor.read_ub2()?;
    let _ = cursor.read_ub4()?;
    let _ = cursor.read_ub4()?;
    let _ = cursor.read_ub2()?;
    let num_bytes = cursor.read_ub4()? as usize;
    let mut bit_vector = None;
    if num_bytes > 0 {
        cursor.skip(1)?;
        bit_vector = Some(cursor.read_raw(num_bytes)?.to_vec());
    }
    cursor.skip_bytes_with_ub4_length()?;
    state.bit_vector = bit_vector;
    Ok(())
}

fn process_bit_vector(
    cursor: &mut PacketCursor<'_>,
    state: &mut ExecuteReadState,
) -> Result<(), OracleThinError> {
    let _ = cursor.read_ub2()?;
    let num_bytes = (state.columns.len() + 7) / 8;
    let bit_vector = cursor.read_raw(num_bytes)?.to_vec();
    state.bit_vector = Some(bit_vector);
    Ok(())
}

fn process_row_data(
    cursor: &mut PacketCursor<'_>,
    capabilities: &OracleThinCapabilities,
    state: &mut ExecuteReadState,
) -> Result<(), OracleThinError> {
    let columns = if state.reading_out_binds {
        &state.out_bind_columns
    } else {
        &state.columns
    };
    let mut row = Vec::with_capacity(columns.len());
    for (index, column) in columns.iter().enumerate() {
        if is_duplicate_column(index, state.bit_vector.as_deref()) {
            let value = state
                .last_row
                .as_ref()
                .and_then(|last_row| last_row.get(index))
                .cloned()
                .unwrap_or(OracleValue::Null);
            row.push(value);
        } else {
            row.push(read_column_value(
                cursor,
                capabilities,
                column,
                state.reading_out_binds,
            )?);
        }
    }
    state.last_row = Some(row.clone());
    if state.reading_out_binds {
        state.out_bind_rows.push(row);
    } else {
        state.rows.push(row);
    }
    state.bit_vector = None;
    Ok(())
}

fn is_duplicate_column(index: usize, bit_vector: Option<&[u8]>) -> bool {
    let Some(bit_vector) = bit_vector else {
        return false;
    };
    let byte = bit_vector.get(index / 8).copied().unwrap_or(0);
    byte & (1 << (index % 8)) == 0
}

fn read_column_value(
    cursor: &mut PacketCursor<'_>,
    capabilities: &OracleThinCapabilities,
    column: &ThinColumn,
    out_bind: bool,
) -> Result<OracleValue, OracleThinError> {
    let value = if column.buffer_size == 0
        && !matches!(
            column.ora_type_num,
            ORA_TYPE_NUM_LONG | ORA_TYPE_NUM_LONG_RAW | ORA_TYPE_NUM_UROWID
        ) {
        OracleValue::Null
    } else {
        match column.ora_type_num {
            ORA_TYPE_NUM_NUMBER => {
                let Some(bytes) = cursor.read_bytes()? else {
                    return finish_column_value(cursor, OracleValue::Null, out_bind);
                };
                OracleValue::Number(decode_oracle_number(&bytes)?)
            }
            ORA_TYPE_NUM_LONG => {
                let value = match cursor.read_bytes()? {
                    Some(bytes) => {
                        OracleValue::Text(decode_oracle_text(&bytes, column.charset_form)?)
                    }
                    None => OracleValue::Null,
                };
                if !out_bind {
                    let _ = cursor.read_sb4()?;
                    let _ = cursor.read_ub4()?;
                    return Ok(value);
                }
                value
            }
            ORA_TYPE_NUM_VARCHAR | ORA_TYPE_NUM_CHAR => {
                let Some(bytes) = cursor.read_bytes()? else {
                    return finish_column_value(cursor, OracleValue::Null, out_bind);
                };
                OracleValue::Text(decode_oracle_text(&bytes, column.charset_form)?)
            }
            ORA_TYPE_NUM_DATE => {
                let Some(bytes) = cursor.read_bytes()? else {
                    return finish_column_value(cursor, OracleValue::Null, out_bind);
                };
                OracleValue::DateTime(decode_oracle_datetime(&bytes)?)
            }
            ORA_TYPE_NUM_TIMESTAMP | ORA_TYPE_NUM_TIMESTAMP_TZ | ORA_TYPE_NUM_TIMESTAMP_DTY => {
                let Some(bytes) = cursor.read_bytes()? else {
                    return finish_column_value(cursor, OracleValue::Null, out_bind);
                };
                OracleValue::Timestamp(decode_oracle_datetime(&bytes)?)
            }
            ORA_TYPE_NUM_RAW => cursor
                .read_bytes()?
                .map(OracleValue::Bytes)
                .unwrap_or(OracleValue::Null),
            ORA_TYPE_NUM_LONG_RAW => {
                let value = cursor
                    .read_bytes()?
                    .map(OracleValue::Bytes)
                    .unwrap_or(OracleValue::Null);
                if !out_bind {
                    let _ = cursor.read_sb4()?;
                    let _ = cursor.read_ub4()?;
                    return Ok(value);
                }
                value
            }
            ORA_TYPE_NUM_BOOLEAN => {
                let Some(bytes) = cursor.read_bytes()? else {
                    return finish_column_value(cursor, OracleValue::Null, out_bind);
                };
                OracleValue::Boolean(bytes == [1, 1])
            }
            ORA_TYPE_NUM_CLOB | ORA_TYPE_NUM_BLOB => cursor
                .read_bytes()?
                .map(OracleValue::Lob)
                .unwrap_or(OracleValue::Null),
            ORA_TYPE_NUM_ROWID | ORA_TYPE_NUM_UROWID => {
                let Some(bytes) = cursor.read_bytes()? else {
                    return finish_column_value(cursor, OracleValue::Null, out_bind);
                };
                OracleValue::Text(String::from_utf8_lossy(&bytes).into_owned())
            }
            ORA_TYPE_NUM_BINARY_FLOAT | ORA_TYPE_NUM_BINARY_DOUBLE => {
                let Some(bytes) = cursor.read_bytes()? else {
                    return finish_column_value(cursor, OracleValue::Null, out_bind);
                };
                OracleValue::Bytes(bytes)
            }
            ORA_TYPE_NUM_CURSOR => {
                let _ = cursor.read_u8()?;
                let mut child_state = ExecuteReadState::default();
                process_describe_body(cursor, capabilities, &mut child_state)?;
                let cursor_id = cursor.read_ub2()? as u32;
                let columns = child_state
                    .columns
                    .into_iter()
                    .map(|column| ColumnMetadata {
                        name: column.name,
                        column_type: column.column_type,
                    })
                    .collect();
                OracleValue::Cursor(RefCursorValue { cursor_id, columns })
            }
            other => {
                return Err(OracleThinError::new(format!(
                    "Oracle thin TTC cannot decode Oracle type {other}"
                )))
            }
        }
    };
    finish_column_value(cursor, value, out_bind)
}

fn finish_column_value(
    cursor: &mut PacketCursor<'_>,
    value: OracleValue,
    out_bind: bool,
) -> Result<OracleValue, OracleThinError> {
    if out_bind {
        let _ = cursor.read_sb4()?;
    }
    Ok(value)
}

fn oracle_column_type_from_ora_type(ora_type_num: u8) -> OracleColumnType {
    match ora_type_num {
        ORA_TYPE_NUM_NUMBER => OracleColumnType::Number,
        ORA_TYPE_NUM_DATE => OracleColumnType::Date,
        ORA_TYPE_NUM_TIMESTAMP | ORA_TYPE_NUM_TIMESTAMP_TZ | ORA_TYPE_NUM_TIMESTAMP_DTY => {
            OracleColumnType::Timestamp
        }
        ORA_TYPE_NUM_RAW | ORA_TYPE_NUM_LONG_RAW => OracleColumnType::Raw,
        ORA_TYPE_NUM_LONG => OracleColumnType::Long,
        ORA_TYPE_NUM_CLOB => OracleColumnType::Clob,
        ORA_TYPE_NUM_BLOB => OracleColumnType::Blob,
        ORA_TYPE_NUM_CURSOR => OracleColumnType::Cursor,
        ORA_TYPE_NUM_BOOLEAN => OracleColumnType::Boolean,
        _ => OracleColumnType::Varchar,
    }
}

fn decode_oracle_text(bytes: &[u8], charset_form: u8) -> Result<String, OracleThinError> {
    if charset_form == CS_FORM_NCHAR {
        if bytes.len() % 2 != 0 {
            return Err(OracleThinError::new("odd-length Oracle NCHAR data"));
        }
        let units = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]));
        String::from_utf16(&units.collect::<Vec<_>>())
            .map_err(|err| OracleThinError::new(format!("invalid UTF-16 Oracle text: {err}")))
    } else {
        String::from_utf8(bytes.to_vec())
            .map_err(|err| OracleThinError::new(format!("invalid UTF-8 Oracle text: {err}")))
    }
}

fn decode_oracle_datetime(bytes: &[u8]) -> Result<crate::OracleDateTime, OracleThinError> {
    if bytes.len() < 7 {
        return Err(OracleThinError::new("short Oracle date/time value"));
    }
    let year =
        (u16::from(bytes[0]).saturating_sub(100) * 100) + u16::from(bytes[1]).saturating_sub(100);
    let nanosecond = if bytes.len() > 10 {
        u32::from_be_bytes([bytes[7], bytes[8], bytes[9], bytes[10]])
    } else {
        0
    };
    let timezone_offset_minutes = if bytes.len() > 12 && bytes[11] & 0x80 == 0 {
        let hour = i16::from(bytes[11] & 0x3f) - 20;
        let minute = i16::from(bytes[12]) - 60;
        Some(hour * 60 + minute)
    } else {
        None
    };
    Ok(crate::OracleDateTime {
        year,
        month: bytes[2],
        day: bytes[3],
        hour: bytes[4].saturating_sub(1),
        minute: bytes[5].saturating_sub(1),
        second: bytes[6].saturating_sub(1),
        nanosecond,
        timezone_offset_minutes,
    })
}

fn decode_oracle_number(bytes: &[u8]) -> Result<String, OracleThinError> {
    if bytes.is_empty() {
        return Err(OracleThinError::new("empty Oracle NUMBER value"));
    }
    if bytes == [0x80] {
        return Ok("0".to_string());
    }
    if bytes == [0x00] {
        return Ok("-Infinity".to_string());
    }
    if bytes == [0xff, 0x65] {
        return Ok("Infinity".to_string());
    }
    let negative = bytes[0] & 0x80 == 0;
    let exponent = if negative {
        i32::from(bytes[0] ^ 0x7f) - 64
    } else {
        i32::from(bytes[0] & 0x7f) - 64
    };
    let mut mantissa = &bytes[1..];
    if mantissa.is_empty() {
        return Err(OracleThinError::new("invalid Oracle NUMBER value"));
    }
    if negative && mantissa.last() == Some(&0x66) {
        mantissa = &mantissa[..mantissa.len() - 1];
    }

    let mut digits = String::with_capacity(mantissa.len() * 2);
    for byte in mantissa {
        let mut digit = byte.saturating_sub(1);
        if negative {
            digit = 100u8.saturating_sub(digit);
        }
        digits.push(char::from(b'0' + digit / 10));
        digits.push(char::from(b'0' + digit % 10));
    }
    let scale = exponent * 2 - i32::try_from(digits.len()).unwrap_or(i32::MAX);
    if digits.len() > 1 {
        let trimmed = digits.trim_start_matches('0');
        digits = if trimmed.is_empty() {
            "0".to_string()
        } else {
            trimmed.to_string()
        };
    }
    if scale > 0 {
        digits.push_str(&"0".repeat(scale as usize));
    } else if scale < 0 {
        let point_pos = digits.len() as i32 + scale;
        if point_pos <= 0 {
            let zeros = "0".repeat(point_pos.unsigned_abs() as usize);
            digits = format!("0.{zeros}{digits}");
        } else {
            digits.insert(point_pos as usize, '.');
        }
    }
    if digits.contains('.') {
        while digits.ends_with('0') {
            digits.pop();
        }
        if digits.ends_with('.') {
            digits.pop();
        }
    }
    if digits.starts_with('.') {
        digits.insert(0, '0');
    }
    if negative && digits != "0" {
        digits.insert(0, '-');
    }
    Ok(digits)
}

fn process_return_parameters(cursor: &mut PacketCursor<'_>) -> Result<(), OracleThinError> {
    let num_params = cursor.read_ub2()? as usize;
    for _ in 0..num_params {
        let _ = cursor.read_ub4()?;
    }
    let num_bytes = cursor.read_ub2()? as usize;
    if num_bytes > 0 {
        cursor.skip(num_bytes)?;
    }
    let num_pairs = cursor.read_ub2()? as usize;
    for _ in 0..num_pairs {
        let text_len = cursor.read_ub2()?;
        if text_len > 0 {
            let _ = cursor.read_bytes()?;
        }
        let binary_len = cursor.read_ub2()?;
        if binary_len > 0 {
            let _ = cursor.read_bytes()?;
        }
        let _ = cursor.read_ub2()?;
    }
    let num_bytes = cursor.read_ub2()? as usize;
    if num_bytes > 0 {
        cursor.skip(num_bytes)?;
    }
    Ok(())
}

fn process_server_side_piggyback(cursor: &mut PacketCursor<'_>) -> Result<(), OracleThinError> {
    let opcode = cursor.read_u8()?;
    match opcode {
        TNS_SERVER_PIGGYBACK_LTXID => {
            cursor.skip_bytes_with_ub4_length()?;
        }
        TNS_SERVER_PIGGYBACK_QUERY_CACHE_INVALIDATION | TNS_SERVER_PIGGYBACK_TRACE_EVENT => {}
        TNS_SERVER_PIGGYBACK_OS_PID_MTS => {
            let _ = cursor.read_ub2()?;
            cursor.skip_bytes()?;
        }
        TNS_SERVER_PIGGYBACK_SYNC => {
            let _ = cursor.read_ub2()?;
            cursor.skip(1)?;
            let num_elements = cursor.read_ub2()? as usize;
            cursor.skip(1)?;
            process_keyword_value_pairs(cursor, num_elements)?;
            let _ = cursor.read_ub4()?;
        }
        TNS_SERVER_PIGGYBACK_EXT_SYNC => {
            let _ = cursor.read_ub2()?;
            cursor.skip(1)?;
        }
        TNS_SERVER_PIGGYBACK_AC_REPLAY_CONTEXT => {
            let _ = cursor.read_ub2()?;
            cursor.skip(1)?;
            let _ = cursor.read_ub4()?;
            let _ = cursor.read_ub4()?;
            cursor.skip(1)?;
            cursor.skip_bytes_with_ub4_length()?;
        }
        TNS_SERVER_PIGGYBACK_SESS_RET => {
            let _ = cursor.read_ub2()?;
            cursor.skip(1)?;
            let num_elements = cursor.read_ub2()? as usize;
            if num_elements > 0 {
                cursor.skip(1)?;
                for _ in 0..num_elements {
                    if cursor.read_ub2()? > 0 {
                        cursor.skip_bytes()?;
                    }
                    if cursor.read_ub2()? > 0 {
                        cursor.skip_bytes()?;
                    }
                    let _ = cursor.read_ub2()?;
                }
            }
            let _ = cursor.read_ub4()?;
            let _ = cursor.read_ub4()?;
            let _ = cursor.read_ub2()?;
        }
        TNS_SERVER_PIGGYBACK_SESS_SIGNATURE => {
            let _ = cursor.read_ub2()?;
            cursor.skip(1)?;
            let _ = cursor.read_ub8()?;
            let _ = cursor.read_ub8()?;
            let _ = cursor.read_ub8()?;
        }
        _ => {
            return Err(OracleThinError::new(format!(
                "unknown Oracle server-side piggyback opcode {opcode}"
            )));
        }
    }
    Ok(())
}

fn process_keyword_value_pairs(
    cursor: &mut PacketCursor<'_>,
    num_pairs: usize,
) -> Result<(), OracleThinError> {
    for _ in 0..num_pairs {
        if cursor.read_ub2()? > 0 {
            cursor.skip_bytes()?;
        }
        if cursor.read_ub2()? > 0 {
            cursor.skip_bytes()?;
        }
        let _ = cursor.read_ub2()?;
    }
    Ok(())
}

fn process_warning(cursor: &mut PacketCursor<'_>) -> Result<(), OracleThinError> {
    let _ = cursor.read_ub2()?;
    let num_bytes = cursor.read_ub2()?;
    let _ = cursor.read_ub2()?;
    if num_bytes > 0 {
        let _ = cursor.read_bytes()?;
    }
    Ok(())
}

fn process_execute_error(
    cursor: &mut PacketCursor<'_>,
    capabilities: &OracleThinCapabilities,
) -> Result<ExecuteError, OracleThinError> {
    let _ = cursor.read_ub4()?;
    let _ = cursor.read_ub2()?;
    let _ = cursor.read_ub4()?;
    let _ = cursor.read_ub2()?;
    let _ = cursor.read_ub2()?;
    let _ = cursor.read_ub2()?;
    let cursor_id = cursor.read_ub2()? as u32;
    let error_pos = cursor.read_sb2()?;
    cursor.skip(6)?;
    let _ = cursor.read_ub4()?;
    let _ = cursor.read_ub2()?;
    cursor.skip(1)?;
    let _ = cursor.read_ub4()?;
    let _ = cursor.read_ub2()?;
    let _ = cursor.read_ub4()?;
    cursor.skip(2)?;
    let _ = cursor.read_ub2()?;
    let _ = cursor.read_ub4()?;
    cursor.skip_bytes_with_ub4_length()?;

    let num_errors = cursor.read_ub2()? as usize;
    if num_errors > 0 {
        let first_byte = cursor.read_u8()?;
        for _ in 0..num_errors {
            if first_byte == 0xfe {
                let _ = cursor.read_ub4()?;
            }
            let _ = cursor.read_ub2()?;
        }
        if first_byte == 0xfe {
            cursor.skip(1)?;
        }
    }

    let num_offsets = cursor.read_ub4()? as usize;
    if num_offsets > 0 {
        let first_byte = cursor.read_u8()?;
        for _ in 0..num_offsets {
            if first_byte == 0xfe {
                let _ = cursor.read_ub4()?;
            }
            let _ = cursor.read_ub4()?;
        }
        if first_byte == 0xfe {
            cursor.skip(1)?;
        }
    }

    let num_messages = cursor.read_ub2()? as usize;
    if num_messages > 0 {
        cursor.skip(1)?;
        for _ in 0..num_messages {
            let _ = cursor.read_ub2()?;
            let _ = cursor.read_bytes()?;
            cursor.skip(2)?;
        }
    }

    let code = cursor.read_ub4()?;
    let rowcount = cursor.read_ub8()?;
    if capabilities.ttc_field_version >= TNS_CCAP_FIELD_VERSION_20_1 {
        let _ = cursor.read_ub4()?;
        let _ = cursor.read_ub4()?;
    }
    let message = if code != 0 {
        let raw_message = cursor.read_str()?.unwrap_or_default();
        let has_visible_message = raw_message
            .chars()
            .any(|ch| !ch.is_control() && !ch.is_whitespace());
        let mut message = if has_visible_message {
            raw_message
        } else {
            format!("ORA-{:05}", code)
        };
        while message.ends_with(char::is_whitespace) {
            message.pop();
        }
        if !message.contains("ORA-") {
            message = format!("ORA-{:05}: {message}", code);
        }
        if error_pos > 0 && !message.contains("position") {
            message.push_str(&format!(" (position {error_pos})"));
        }
        Some(message)
    } else {
        None
    };
    Ok(ExecuteError {
        code,
        cursor_id,
        _rowcount: rowcount,
        message,
    })
}

#[derive(Debug, Default)]
struct AuthResult {
    server_version: Option<String>,
}

#[derive(Debug, Default)]
struct AuthState {
    session_data: HashMap<String, String>,
    verifier_type: u32,
    combo_key: Option<Vec<u8>>,
    server_version: Option<String>,
    saw_auth_parameters: bool,
    saw_error: bool,
}

fn authenticate(
    stream: &mut TcpStream,
    config: &OracleThinConfig,
    capabilities: &OracleThinCapabilities,
) -> Result<AuthResult, OracleThinError> {
    let mut state = AuthState::default();
    log_connect_phase("ttc-auth-phase-one-write", "");
    write_auth_phase_one(stream, config, capabilities)?;
    log_connect_phase("ttc-auth-phase-one-read", "");
    process_auth_response(stream, capabilities, &mut state)?;
    if !state.session_data.contains_key("AUTH_SESSKEY")
        || !state.session_data.contains_key("AUTH_VFR_DATA")
    {
        return Err(OracleThinError::new(
            "Oracle authentication phase one did not return verifier data",
        ));
    }

    let credentials = generate_auth_credentials(config, &mut state)?;
    log_connect_phase("ttc-auth-phase-two-write", "");
    write_auth_phase_two(stream, config, capabilities, &credentials)?;
    log_connect_phase("ttc-auth-phase-two-read", "");
    process_auth_response(stream, capabilities, &mut state)?;
    verify_server_response(&state)?;
    log_connect_phase("ttc-auth-accept", "");
    Ok(AuthResult {
        server_version: state.server_version,
    })
}

fn write_auth_phase_one(
    stream: &mut TcpStream,
    config: &OracleThinConfig,
    capabilities: &OracleThinCapabilities,
) -> Result<(), OracleThinError> {
    let mut payload = Vec::new();
    write_function_code(&mut payload, TNS_FUNC_AUTH_PHASE_ONE, 1, capabilities);
    write_auth_header(&mut payload, &config.username, TNS_AUTH_MODE_LOGON, 5)?;
    write_key_value(&mut payload, "AUTH_TERMINAL", "unknown", 0)?;
    write_key_value(&mut payload, "AUTH_PROGRAM_NM", &config.program, 0)?;
    write_key_value(&mut payload, "AUTH_MACHINE", &config.machine, 0)?;
    write_key_value(&mut payload, "AUTH_PID", &std::process::id().to_string(), 0)?;
    write_key_value(&mut payload, "AUTH_SID", &config.os_user, 0)?;
    write_data_packet(
        stream,
        capabilities.protocol_version.unwrap_or(319),
        &payload,
    )
}

fn write_auth_phase_two(
    stream: &mut TcpStream,
    config: &OracleThinConfig,
    capabilities: &OracleThinCapabilities,
    credentials: &AuthCredentials,
) -> Result<(), OracleThinError> {
    let mut payload = Vec::new();
    write_function_code(&mut payload, TNS_FUNC_AUTH_PHASE_TWO, 2, capabilities);
    let mut num_pairs = 7;
    if credentials.speedy_key.is_some() {
        num_pairs += 1;
    }
    write_auth_header(
        &mut payload,
        &config.username,
        TNS_AUTH_MODE_LOGON | TNS_AUTH_MODE_WITH_PASSWORD,
        num_pairs,
    )?;
    write_key_value(&mut payload, "AUTH_SESSKEY", &credentials.session_key, 1)?;
    if let Some(speedy_key) = credentials.speedy_key.as_deref() {
        write_key_value(&mut payload, "AUTH_PBKDF2_SPEEDY_KEY", speedy_key, 0)?;
    }
    write_key_value(&mut payload, "AUTH_PASSWORD", &credentials.password, 0)?;
    write_key_value(&mut payload, "SESSION_CLIENT_CHARSET", "873", 0)?;
    write_key_value(
        &mut payload,
        "SESSION_CLIENT_DRIVER_NAME",
        "space-query-thin thn : 0.1.0",
        0,
    )?;
    write_key_value(&mut payload, "SESSION_CLIENT_VERSION", "0", 0)?;
    write_key_value(
        &mut payload,
        "AUTH_ALTER_SESSION",
        &alter_session_timezone_statement(),
        1,
    )?;
    write_key_value(
        &mut payload,
        "AUTH_CONNECT_STRING",
        &auth_connect_string(&config.target),
        0,
    )?;
    write_data_packet(
        stream,
        capabilities.protocol_version.unwrap_or(319),
        &payload,
    )
}

fn write_auth_header(
    payload: &mut Vec<u8>,
    username: &str,
    auth_mode: u32,
    num_pairs: u32,
) -> Result<(), OracleThinError> {
    let user_bytes = username.as_bytes();
    payload.push(if user_bytes.is_empty() { 0 } else { 1 });
    write_ub4(payload, user_bytes.len() as u32);
    write_ub4(payload, auth_mode);
    payload.push(1);
    write_ub4(payload, num_pairs);
    payload.push(1);
    payload.push(1);
    if !user_bytes.is_empty() {
        write_len_bytes(payload, user_bytes)?;
    }
    Ok(())
}

fn write_function_code(
    payload: &mut Vec<u8>,
    function_code: u8,
    sequence: u8,
    capabilities: &OracleThinCapabilities,
) {
    payload.push(TNS_MSG_TYPE_FUNCTION);
    payload.push(function_code);
    payload.push(sequence);
    if capabilities.ttc_field_version >= 18 {
        write_ub8(payload, 0);
    }
}

fn write_piggyback_code(
    payload: &mut Vec<u8>,
    function_code: u8,
    sequence: u8,
    capabilities: &OracleThinCapabilities,
) {
    payload.push(TNS_MSG_TYPE_PIGGYBACK);
    payload.push(function_code);
    payload.push(sequence);
    if capabilities.ttc_field_version >= 18 {
        write_ub8(payload, 0);
    }
}

fn write_key_value(
    payload: &mut Vec<u8>,
    key: &str,
    value: &str,
    flags: u32,
) -> Result<(), OracleThinError> {
    write_bytes_with_two_lengths(payload, key.as_bytes())?;
    write_bytes_with_two_lengths(payload, value.as_bytes())?;
    write_ub4(payload, flags);
    Ok(())
}

fn write_bytes_with_two_lengths(
    payload: &mut Vec<u8>,
    value: &[u8],
) -> Result<(), OracleThinError> {
    write_ub4(payload, value.len() as u32);
    write_len_bytes(payload, value)
}

#[derive(Debug)]
struct AuthCredentials {
    session_key: String,
    speedy_key: Option<String>,
    password: String,
}

fn generate_auth_credentials(
    config: &OracleThinConfig,
    state: &mut AuthState,
) -> Result<AuthCredentials, OracleThinError> {
    match state.verifier_type {
        TNS_VERIFIER_TYPE_12C => generate_12c_auth_credentials(config, state),
        TNS_VERIFIER_TYPE_11G_1 | TNS_VERIFIER_TYPE_11G_2 => Err(OracleThinError::new(
            "Oracle 11g password verifier is not implemented in the Rust Oracle Thin auth layer yet",
        )),
        other => Err(OracleThinError::new(format!(
            "unsupported Oracle password verifier type 0x{other:x}"
        ))),
    }
}

fn generate_12c_auth_credentials(
    config: &OracleThinConfig,
    state: &mut AuthState,
) -> Result<AuthCredentials, OracleThinError> {
    let verifier_data = hex_decode(required_session_value(state, "AUTH_VFR_DATA")?)?;
    let iterations = required_session_value(state, "AUTH_PBKDF2_VGEN_COUNT")?
        .parse::<u32>()
        .map_err(|err| OracleThinError::new(format!("invalid AUTH_PBKDF2_VGEN_COUNT: {err}")))?;
    let mut verifier_salt = verifier_data.clone();
    verifier_salt.extend_from_slice(b"AUTH_PBKDF2_SPEEDY_KEY");
    let mut password_key = vec![0u8; 64];
    pbkdf2_hmac::<Sha512>(
        config.password.as_bytes(),
        &verifier_salt,
        iterations,
        &mut password_key,
    );

    let mut hasher = Sha512::new();
    hasher.update(&password_key);
    hasher.update(&verifier_data);
    let password_hash = hasher.finalize()[..32].to_vec();

    let encoded_server_key = hex_decode(required_session_value(state, "AUTH_SESSKEY")?)?;
    let session_key_part_a = aes_decrypt_cbc_no_padding(&password_hash, &encoded_server_key)?;
    let mut session_key_part_b = vec![0u8; session_key_part_a.len()];
    OsRng.fill_bytes(&mut session_key_part_b);
    let encoded_client_key = aes_encrypt_cbc_pkcs7(&password_hash, &session_key_part_b)?;
    let session_key = hex_encode_upper(&encoded_client_key[..session_key_part_b.len()]);

    let csk_salt = hex_decode(required_session_value(state, "AUTH_PBKDF2_CSK_SALT")?)?;
    let sder_count = required_session_value(state, "AUTH_PBKDF2_SDER_COUNT")?
        .parse::<u32>()
        .map_err(|err| OracleThinError::new(format!("invalid AUTH_PBKDF2_SDER_COUNT: {err}")))?;
    let key_len = 32;
    if session_key_part_a.len() < key_len || session_key_part_b.len() < key_len {
        return Err(OracleThinError::new(
            "Oracle authentication session key is shorter than expected",
        ));
    }
    let mut temp_key = Vec::with_capacity(key_len * 2);
    temp_key.extend_from_slice(&session_key_part_b[..key_len]);
    temp_key.extend_from_slice(&session_key_part_a[..key_len]);
    let temp_key_hex = hex_encode_upper(&temp_key);
    let mut combo_key = vec![0u8; key_len];
    pbkdf2_hmac::<Sha512>(
        temp_key_hex.as_bytes(),
        &csk_salt,
        sder_count,
        &mut combo_key,
    );
    state.combo_key = Some(combo_key.clone());

    let mut speedy_salt = [0u8; 16];
    OsRng.fill_bytes(&mut speedy_salt);
    let mut speedy_plain = Vec::with_capacity(16 + password_key.len());
    speedy_plain.extend_from_slice(&speedy_salt);
    speedy_plain.extend_from_slice(&password_key);
    let speedy_encrypted = aes_encrypt_cbc_pkcs7(&combo_key, &speedy_plain)?;
    let speedy_key = hex_encode_upper(&speedy_encrypted[..80]);

    let mut password_salt = [0u8; 16];
    OsRng.fill_bytes(&mut password_salt);
    let mut password_plain = Vec::with_capacity(16 + config.password.len());
    password_plain.extend_from_slice(&password_salt);
    password_plain.extend_from_slice(config.password.as_bytes());
    let encrypted_password = aes_encrypt_cbc_pkcs7(&combo_key, &password_plain)?;

    Ok(AuthCredentials {
        session_key,
        speedy_key: Some(speedy_key),
        password: hex_encode_upper(&encrypted_password),
    })
}

fn required_session_value<'a>(state: &'a AuthState, key: &str) -> Result<&'a str, OracleThinError> {
    state
        .session_data
        .get(key)
        .map(String::as_str)
        .ok_or_else(|| OracleThinError::new(format!("missing Oracle auth parameter {key}")))
}

fn process_auth_response(
    stream: &mut TcpStream,
    capabilities: &OracleThinCapabilities,
    state: &mut AuthState,
) -> Result<(), OracleThinError> {
    let packet = read_data_packet(stream, capabilities.protocol_version.unwrap_or(319))?;
    let mut cursor =
        PacketCursor::with_big_clr_chunks(&packet, capabilities.supports_big_clr_chunks);
    while cursor.remaining() > 0 {
        let message_type = cursor.read_u8()?;
        match message_type {
            TNS_MSG_TYPE_PARAMETER => process_auth_parameters(&mut cursor, state)?,
            TNS_MSG_TYPE_STATUS => {
                let _ = cursor.read_ub4()?;
                let _ = cursor.read_ub2()?;
            }
            TNS_MSG_TYPE_TOKEN => {
                let _ = cursor.read_ub8()?;
            }
            TNS_MSG_TYPE_ERROR | TNS_MSG_TYPE_WARNING => {
                state.saw_error = true;
                break;
            }
            TNS_MSG_TYPE_SERVER_SIDE_PIGGYBACK | TNS_MSG_TYPE_END_OF_RESPONSE => break,
            other => {
                return Err(OracleThinError::new(format!(
                    "unexpected Oracle auth response message type {other}"
                )));
            }
        }
    }
    if state.saw_error && !state.saw_auth_parameters {
        return Err(OracleThinError::new(
            "Oracle authentication failed before returning auth parameters",
        ));
    }
    Ok(())
}

fn process_auth_parameters(
    cursor: &mut PacketCursor<'_>,
    state: &mut AuthState,
) -> Result<(), OracleThinError> {
    let num_params = cursor.read_ub2()? as usize;
    for _ in 0..num_params {
        let key = cursor.read_str_with_ub4_length()?.unwrap_or_default();
        let value = cursor.read_str_with_ub4_length()?.unwrap_or_default();
        let flags = cursor.read_ub4()? as u32;
        if key == "AUTH_VFR_DATA" {
            state.verifier_type = flags;
        } else if key == "AUTH_VERSION_NO" {
            state.server_version = Some(value.clone());
        }
        state.session_data.insert(key, value);
    }
    state.saw_auth_parameters = true;
    Ok(())
}

fn verify_server_response(state: &AuthState) -> Result<(), OracleThinError> {
    let Some(encoded_response) = state.session_data.get("AUTH_SVR_RESPONSE") else {
        return Ok(());
    };
    let Some(combo_key) = state.combo_key.as_deref() else {
        return Ok(());
    };
    let response = aes_decrypt_cbc_no_padding(combo_key, &hex_decode(encoded_response)?)?;
    if response.len() < 32 || &response[16..32] != b"SERVER_TO_CLIENT" {
        return Err(OracleThinError::new(
            "Oracle authentication returned an invalid server response",
        ));
    }
    Ok(())
}

fn aes_encrypt_cbc_pkcs7(key: &[u8], plain_text: &[u8]) -> Result<Vec<u8>, OracleThinError> {
    let iv = [0u8; 16];
    let pos = plain_text.len();
    let mut buf = plain_text.to_vec();
    buf.resize(pos + 16, 0);
    let encrypted = match key.len() {
        24 => cbc::Encryptor::<Aes192>::new_from_slices(key, &iv)
            .map_err(|err| OracleThinError::new(format!("invalid AES-192 key: {err}")))?
            .encrypt_padded_mut::<Pkcs7>(&mut buf, pos)
            .map_err(|err| OracleThinError::new(format!("AES-CBC encrypt failed: {err}")))?,
        32 => cbc::Encryptor::<Aes256>::new_from_slices(key, &iv)
            .map_err(|err| OracleThinError::new(format!("invalid AES-256 key: {err}")))?
            .encrypt_padded_mut::<Pkcs7>(&mut buf, pos)
            .map_err(|err| OracleThinError::new(format!("AES-CBC encrypt failed: {err}")))?,
        len => {
            return Err(OracleThinError::new(format!(
                "unsupported AES key length {len}"
            )));
        }
    };
    Ok(encrypted.to_vec())
}

fn aes_decrypt_cbc_no_padding(
    key: &[u8],
    encrypted_text: &[u8],
) -> Result<Vec<u8>, OracleThinError> {
    let iv = [0u8; 16];
    let mut buf = encrypted_text.to_vec();
    let decrypted = match key.len() {
        24 => cbc::Decryptor::<Aes192>::new_from_slices(key, &iv)
            .map_err(|err| OracleThinError::new(format!("invalid AES-192 key: {err}")))?
            .decrypt_padded_mut::<NoPadding>(&mut buf)
            .map_err(|err| OracleThinError::new(format!("AES-CBC decrypt failed: {err}")))?,
        32 => cbc::Decryptor::<Aes256>::new_from_slices(key, &iv)
            .map_err(|err| OracleThinError::new(format!("invalid AES-256 key: {err}")))?
            .decrypt_padded_mut::<NoPadding>(&mut buf)
            .map_err(|err| OracleThinError::new(format!("AES-CBC decrypt failed: {err}")))?,
        len => {
            return Err(OracleThinError::new(format!(
                "unsupported AES key length {len}"
            )));
        }
    };
    Ok(decrypted.to_vec())
}

fn hex_decode(value: &str) -> Result<Vec<u8>, OracleThinError> {
    let bytes = value.as_bytes();
    if bytes.len() % 2 != 0 {
        return Err(OracleThinError::new("hex string has odd length"));
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        let high = hex_nibble(chunk[0])?;
        let low = hex_nibble(chunk[1])?;
        out.push((high << 4) | low);
    }
    Ok(out)
}

fn hex_nibble(byte: u8) -> Result<u8, OracleThinError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(OracleThinError::new("invalid hex digit")),
    }
}

fn hex_encode_upper(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn auth_connect_string(target: &ConnectTarget) -> String {
    format!(
        "(DESCRIPTION=(ADDRESS=(PROTOCOL=tcp)(HOST={})(PORT={}))(CONNECT_DATA=(SERVICE_NAME={})))",
        target.host, target.port, target.service_name
    )
}

fn alter_session_timezone_statement() -> String {
    let timezone = std::env::var("ORA_SDTZ").unwrap_or_else(|_| "+09:00".to_string());
    format!("ALTER SESSION SET TIME_ZONE='{timezone}'\0")
}

fn write_data_packet(
    stream: &mut TcpStream,
    protocol_version: u16,
    payload: &[u8],
) -> Result<(), OracleThinError> {
    if payload.len() > TNS_DATA_PACKET_CHUNK_SIZE {
        for chunk in payload.chunks(TNS_DATA_PACKET_CHUNK_SIZE) {
            write_single_data_packet(stream, protocol_version, chunk)?;
        }
        return Ok(());
    }
    write_single_data_packet(stream, protocol_version, payload)
}

fn write_single_data_packet(
    stream: &mut TcpStream,
    protocol_version: u16,
    payload: &[u8],
) -> Result<(), OracleThinError> {
    let size = 10usize + payload.len();
    let mut packet = vec![0u8; size];
    if protocol_version >= 315 {
        put_u32(
            &mut packet,
            0,
            u32::try_from(size).map_err(|_| {
                OracleThinError::new(format!("TNS data packet too large: {size} bytes"))
            })?,
        );
    } else {
        put_u16(
            &mut packet,
            0,
            u16::try_from(size).map_err(|_| {
                OracleThinError::new(format!("TNS data packet too large: {size} bytes"))
            })?,
        );
    }
    packet[4] = TNS_PACKET_TYPE_DATA;
    packet[10..].copy_from_slice(payload);
    stream.write_all(&packet)?;
    stream.flush()?;
    Ok(())
}

fn read_data_packet(
    stream: &mut TcpStream,
    protocol_version: u16,
) -> Result<Vec<u8>, OracleThinError> {
    read_data_packet_with_flags(stream, protocol_version).map(|(_, payload)| payload)
}

fn read_data_packet_with_flags(
    stream: &mut TcpStream,
    protocol_version: u16,
) -> Result<(u16, Vec<u8>), OracleThinError> {
    loop {
        let mut header = [0u8; 8];
        stream.read_exact(&mut header).map_err(|err| {
            OracleThinError::new(format!("failed to read TNS data header: {err}"))
        })?;
        let size = if protocol_version >= 315 {
            u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize
        } else {
            u16::from_be_bytes([header[0], header[1]]) as usize
        };
        if size < 8 {
            return Err(OracleThinError::new(format!(
                "invalid TNS packet length {size}"
            )));
        }
        let mut data = vec![0u8; size - 8];
        stream.read_exact(&mut data).map_err(|err| {
            OracleThinError::new(format!(
                "failed to read TNS packet body: {err}; packet_type={} size={} header={:02x?}",
                header[4], size, header
            ))
        })?;
        match header[4] {
            TNS_PACKET_TYPE_DATA => {
                if data.len() < 2 {
                    return Err(OracleThinError::new(format!(
                        "invalid TNS data packet length {size}"
                    )));
                }
                let data_flags = u16::from_be_bytes([data[0], data[1]]);
                return Ok((data_flags, data[2..].to_vec()));
            }
            TNS_PACKET_TYPE_MARKER | TNS_PACKET_TYPE_CONTROL => {
                if header[4] == TNS_PACKET_TYPE_MARKER && data.last() == Some(&1) {
                    write_marker_packet(stream, protocol_version, 2)?;
                    continue;
                }
                if header[4] == TNS_PACKET_TYPE_MARKER && data.last() == Some(&2) {
                    return Ok((
                        TNS_DATA_FLAGS_END_OF_RESPONSE,
                        vec![TNS_MSG_TYPE_END_OF_RESPONSE],
                    ));
                }
                return Err(OracleThinError::new(format!(
                    "received unsupported TNS packet type {} while waiting for data: {}",
                    header[4],
                    hex_encode_upper(&data)
                )));
            }
            other => {
                return Err(OracleThinError::new(format!(
                    "expected TNS data packet, got packet type {other}"
                )));
            }
        }
    }
}

fn write_marker_packet(
    stream: &mut TcpStream,
    protocol_version: u16,
    marker_type: u8,
) -> Result<(), OracleThinError> {
    let mut packet = vec![0u8; 11];
    if protocol_version >= 315 {
        put_u32(&mut packet, 0, 11);
    } else {
        put_u16(&mut packet, 0, 11);
    }
    packet[4] = TNS_PACKET_TYPE_MARKER;
    packet[8] = 1;
    packet[10] = marker_type;
    stream.write_all(&packet)?;
    stream.flush()?;
    Ok(())
}

fn process_protocol_message(
    cursor: &mut PacketCursor<'_>,
    capabilities: &mut OracleThinCapabilities,
) -> Result<(), OracleThinError> {
    let server_version = cursor.read_u8()?;
    cursor.skip(1)?;
    let banner = cursor.read_null_terminated_bytes()?;
    capabilities.charset_id = cursor.read_u16_le()?;
    let server_flags = cursor.read_u8()?;
    let num_elements = cursor.read_u16_le()? as usize;
    cursor.skip(num_elements.saturating_mul(5))?;
    let fdo_length = cursor.read_u16_be()? as usize;
    let fdo = cursor.read_raw(fdo_length)?;
    if fdo.len() >= 7 {
        let ix = 6usize
            .saturating_add(usize::from(fdo[5]))
            .saturating_add(usize::from(fdo[6]));
        if fdo.len() >= ix + 5 {
            capabilities.ncharset_id = u16::from_be_bytes([fdo[ix + 3], fdo[ix + 4]]);
        }
    }
    if let Some(server_compile_caps) = cursor.read_bytes()? {
        adjust_for_server_compile_caps(capabilities, &server_compile_caps);
    }
    if let Some(server_runtime_caps) = cursor.read_bytes()? {
        adjust_for_server_runtime_caps(capabilities, &server_runtime_caps);
    }
    log_connect_phase(
        "ttc-protocol-accept",
        &format!(
            "server_version={} flags=0x{:x} charset={} ncharset={} banner={}",
            server_version,
            server_flags,
            capabilities.charset_id,
            capabilities.ncharset_id,
            String::from_utf8_lossy(&banner)
        ),
    );
    Ok(())
}

fn adjust_for_server_compile_caps(capabilities: &mut OracleThinCapabilities, server_caps: &[u8]) {
    if let Some(server_field_version) = server_caps.get(TNS_CCAP_FIELD_VERSION).copied() {
        if server_field_version < capabilities.ttc_field_version {
            capabilities.ttc_field_version = server_field_version;
        }
    }
    capabilities.supports_sql_boolean = capabilities.ttc_field_version >= 17;
    if server_caps
        .get(TNS_CCAP_TTC4)
        .is_some_and(|value| value & TNS_CCAP_EXPLICIT_BOUNDARY != 0)
    {
        capabilities.supports_request_boundaries = true;
    }
    if server_caps.get(37).is_some_and(|value| value & 0x20 != 0) {
        capabilities.supports_big_clr_chunks = true;
    }
}

fn adjust_for_server_runtime_caps(capabilities: &mut OracleThinCapabilities, server_caps: &[u8]) {
    capabilities.max_string_size = if server_caps
        .get(TNS_RCAP_TTC)
        .is_some_and(|value| value & TNS_RCAP_TTC_32K != 0)
    {
        32767
    } else {
        4000
    };
}

fn client_compile_caps(capabilities: &OracleThinCapabilities) -> Result<Vec<u8>, OracleThinError> {
    let mut caps = vec![0u8; 53];
    caps[0] = 6;
    caps[4] = 8 | 2 | 32 | 64 | 0x80;
    caps[5] = 0x08 | 0x10;
    caps[7] = capabilities.ttc_field_version;
    caps[8] = 1;
    caps[9] = 1;
    caps[15] = 0x20 | 0x01 | 0x08;
    caps[16] = 0x10 | 0x80;
    caps[17] = 3;
    caps[18] = 7;
    caps[19] = 3;
    caps[21] = 1;
    caps[23] = 0x01 | 0x02 | 0x40 | 0x08 | 0x80 | 0x04;
    caps[26] = 0x04;
    caps[27] = 1;
    caps[31] = 0x10;
    caps[34] = 12;
    caps[35] = 0x20;
    caps[37] = 0x10 | 0x20 | 0x80 | 0x08;
    caps[39] = 8;
    caps[40] = 0x04 | 0x40;
    if capabilities.supports_end_of_response {
        caps[40] |= TNS_CCAP_END_OF_RESPONSE;
    }
    caps[42] = 0x01 | 0x04;
    caps[44] = 0x08 | 0x02 | 0x04 | 0x10 | 0x20;
    caps[45] = 0x02;
    caps[52] = 0x01 | 0x02;
    if caps.len() > 252 {
        return Err(OracleThinError::new("client compile caps too large"));
    }
    Ok(caps)
}

fn client_runtime_caps() -> Vec<u8> {
    let mut caps = vec![0u8; 11];
    caps[0] = 2;
    caps[6] = 0x01 | 0x04;
    caps
}

fn write_len_bytes(out: &mut Vec<u8>, value: &[u8]) -> Result<(), OracleThinError> {
    if value.len() > 252 {
        return Err(OracleThinError::new(format!(
            "short TTC byte field too large: {} bytes",
            value.len()
        )));
    }
    out.push(value.len() as u8);
    out.extend_from_slice(value);
    Ok(())
}

fn write_bytes_with_length(out: &mut Vec<u8>, value: &[u8]) -> Result<(), OracleThinError> {
    if value.len() <= 252 {
        out.push(value.len() as u8);
        out.extend_from_slice(value);
        return Ok(());
    }
    out.push(0xfe);
    for chunk in value.chunks(32_767) {
        write_ub4(out, chunk.len() as u32);
        out.extend_from_slice(chunk);
    }
    write_ub4(out, 0);
    Ok(())
}

fn write_ub2(out: &mut Vec<u8>, value: u16) {
    write_ub8(out, u64::from(value));
}

fn write_ub4(out: &mut Vec<u8>, value: u32) {
    write_ub8(out, u64::from(value));
}

fn write_ub8(out: &mut Vec<u8>, value: u64) {
    if value == 0 {
        out.push(0);
    } else if value <= u64::from(u8::MAX) {
        out.push(1);
        out.push(value as u8);
    } else if value <= u64::from(u16::MAX) {
        out.push(2);
        out.extend_from_slice(&(value as u16).to_be_bytes());
    } else if value <= u64::from(u32::MAX) {
        out.push(4);
        out.extend_from_slice(&(value as u32).to_be_bytes());
    } else {
        out.push(8);
        out.extend_from_slice(&value.to_be_bytes());
    }
}

fn put_u16_be_vec(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn put_u16_le_vec(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

const DATA_TYPE_REPRESENTATIONS: &[(u16, u16, u16)] = &[
    (1, 1, 1),
    (2, 2, 10),
    (8, 8, 1),
    (12, 12, 10),
    (23, 23, 1),
    (24, 24, 1),
    (25, 25, 1),
    (26, 26, 1),
    (27, 27, 10),
    (28, 28, 1),
    (29, 29, 1),
    (30, 30, 1),
    (31, 31, 1),
    (32, 32, 1),
    (33, 33, 1),
    (10, 10, 1),
    (11, 11, 1),
    (40, 40, 1),
    (41, 41, 1),
    (117, 117, 1),
    (120, 120, 1),
    (290, 290, 1),
    (291, 291, 1),
    (292, 292, 1),
    (293, 293, 1),
    (294, 294, 1),
    (298, 298, 1),
    (299, 299, 1),
    (300, 300, 1),
    (301, 301, 1),
    (302, 302, 1),
    (303, 303, 1),
    (304, 304, 1),
    (305, 305, 1),
    (306, 306, 1),
    (307, 307, 1),
    (308, 308, 1),
    (309, 309, 1),
    (310, 310, 1),
    (311, 311, 1),
    (312, 312, 1),
    (313, 313, 1),
    (315, 315, 1),
    (316, 316, 1),
    (317, 317, 1),
    (318, 318, 1),
    (319, 319, 1),
    (320, 320, 1),
    (321, 321, 1),
    (322, 322, 1),
    (323, 323, 1),
    (327, 327, 1),
    (328, 328, 1),
    (329, 329, 1),
    (331, 331, 1),
    (333, 333, 1),
    (334, 334, 1),
    (335, 335, 1),
    (336, 336, 1),
    (337, 337, 1),
    (338, 338, 1),
    (339, 339, 1),
    (340, 340, 1),
    (341, 341, 1),
    (342, 342, 1),
    (343, 343, 1),
    (344, 344, 1),
    (345, 345, 1),
    (346, 346, 1),
    (348, 348, 1),
    (349, 349, 1),
    (354, 354, 1),
    (355, 355, 1),
    (359, 359, 1),
    (363, 363, 1),
    (380, 380, 1),
    (381, 381, 1),
    (382, 382, 1),
    (383, 383, 1),
    (384, 384, 1),
    (385, 385, 1),
    (386, 386, 1),
    (387, 387, 1),
    (388, 388, 1),
    (389, 389, 1),
    (390, 390, 1),
    (391, 391, 1),
    (393, 393, 1),
    (394, 394, 1),
    (395, 395, 1),
    (396, 396, 1),
    (397, 397, 1),
    (398, 398, 1),
    (399, 399, 1),
    (400, 400, 1),
    (401, 401, 1),
    (404, 404, 1),
    (405, 405, 1),
    (406, 406, 1),
    (407, 407, 1),
    (413, 413, 1),
    (414, 414, 1),
    (415, 415, 1),
    (416, 416, 1),
    (417, 417, 1),
    (418, 418, 1),
    (419, 419, 1),
    (420, 420, 1),
    (421, 421, 1),
    (422, 422, 1),
    (423, 423, 1),
    (424, 424, 1),
    (425, 425, 1),
    (426, 426, 1),
    (427, 427, 1),
    (429, 429, 1),
    (430, 430, 1),
    (431, 431, 1),
    (432, 432, 1),
    (433, 433, 1),
    (449, 449, 1),
    (450, 450, 1),
    (454, 454, 1),
    (455, 455, 1),
    (456, 456, 1),
    (457, 457, 1),
    (458, 458, 1),
    (459, 459, 1),
    (460, 460, 1),
    (461, 461, 1),
    (462, 462, 1),
    (463, 463, 1),
    (466, 466, 1),
    (467, 467, 1),
    (468, 468, 1),
    (469, 469, 1),
    (470, 470, 1),
    (471, 471, 1),
    (472, 472, 1),
    (473, 473, 1),
    (474, 474, 1),
    (475, 475, 1),
    (476, 476, 1),
    (477, 477, 1),
    (478, 478, 1),
    (479, 479, 1),
    (480, 480, 1),
    (481, 481, 1),
    (482, 482, 1),
    (483, 483, 1),
    (484, 484, 1),
    (485, 485, 1),
    (486, 486, 1),
    (490, 490, 1),
    (491, 491, 1),
    (492, 492, 1),
    (493, 493, 1),
    (494, 494, 1),
    (495, 495, 1),
    (496, 496, 1),
    (498, 498, 1),
    (499, 499, 1),
    (500, 500, 1),
    (501, 501, 1),
    (502, 502, 1),
    (509, 509, 1),
    (510, 510, 1),
    (513, 513, 1),
    (514, 514, 1),
    (516, 516, 1),
    (517, 517, 1),
    (518, 518, 1),
    (519, 519, 1),
    (520, 520, 1),
    (521, 521, 1),
    (522, 522, 1),
    (523, 523, 1),
    (524, 524, 1),
    (525, 525, 1),
    (526, 526, 1),
    (527, 527, 1),
    (528, 528, 1),
    (529, 529, 1),
    (530, 530, 1),
    (531, 531, 1),
    (532, 532, 1),
    (533, 533, 1),
    (534, 534, 1),
    (535, 535, 1),
    (536, 536, 1),
    (537, 537, 1),
    (538, 538, 1),
    (539, 539, 1),
    (540, 540, 1),
    (541, 541, 1),
    (542, 542, 1),
    (543, 543, 1),
    (560, 560, 1),
    (565, 565, 1),
    (572, 572, 1),
    (573, 573, 1),
    (574, 574, 1),
    (575, 575, 1),
    (576, 576, 1),
    (578, 578, 1),
    (563, 563, 1),
    (564, 564, 1),
    (579, 579, 1),
    (580, 580, 1),
    (581, 581, 1),
    (582, 582, 1),
    (583, 583, 1),
    (584, 584, 1),
    (585, 585, 1),
    (3, 2, 10),
    (4, 2, 10),
    (5, 1, 1),
    (6, 2, 10),
    (7, 2, 10),
    (9, 1, 1),
    (15, 1, 1),
    (39, 39, 1),
    (68, 2, 10),
    (91, 2, 10),
    (94, 1, 1),
    (95, 23, 1),
    (96, 96, 1),
    (97, 96, 1),
    (100, 100, 1),
    (101, 101, 1),
    (102, 102, 1),
    (104, 11, 1),
    (106, 106, 1),
    (108, 109, 1),
    (109, 109, 1),
    (110, 111, 1),
    (111, 111, 1),
    (112, 112, 1),
    (113, 113, 1),
    (114, 114, 1),
    (115, 115, 1),
    (116, 102, 1),
    (119, 119, 1),
    (198, 198, 1),
    (146, 146, 1),
    (152, 2, 10),
    (153, 2, 10),
    (154, 2, 10),
    (155, 1, 1),
    (156, 12, 10),
    (172, 2, 10),
    (178, 178, 1),
    (179, 179, 1),
    (180, 180, 1),
    (181, 181, 1),
    (182, 182, 1),
    (183, 183, 1),
    (184, 12, 10),
    (185, 185, 1),
    (186, 186, 1),
    (187, 187, 1),
    (188, 188, 1),
    (189, 189, 1),
    (190, 190, 1),
    (195, 112, 1),
    (196, 113, 1),
    (197, 114, 1),
    (208, 208, 1),
    (231, 231, 1),
    (232, 231, 1),
    (233, 233, 1),
    (241, 109, 1),
    (252, 252, 1),
    (590, 590, 1),
    (591, 591, 1),
    (592, 592, 1),
    (613, 613, 1),
    (614, 614, 1),
    (615, 615, 1),
    (616, 616, 1),
    (611, 611, 1),
    (612, 612, 1),
    (593, 593, 1),
    (594, 594, 1),
    (595, 595, 1),
    (596, 596, 1),
    (597, 597, 1),
    (598, 598, 1),
    (599, 599, 1),
    (600, 600, 1),
    (601, 601, 1),
    (602, 602, 1),
    (603, 603, 1),
    (604, 604, 1),
    (605, 605, 1),
    (622, 622, 1),
    (623, 623, 1),
    (624, 624, 1),
    (625, 625, 1),
    (626, 626, 1),
    (627, 627, 1),
    (628, 628, 1),
    (629, 629, 1),
    (630, 630, 1),
    (631, 631, 1),
    (632, 632, 1),
    (637, 637, 1),
    (638, 638, 1),
    (636, 636, 1),
    (639, 639, 1),
    (663, 663, 1),
    (640, 640, 1),
    (652, 652, 1),
    (646, 646, 1),
    (647, 647, 1),
    (127, 127, 1),
    (660, 660, 1),
    (661, 661, 1),
    (665, 665, 1),
    (669, 669, 1),
    (670, 670, 1),
];

struct PacketCursor<'a> {
    data: &'a [u8],
    pos: usize,
    big_clr_chunks: bool,
}

impl<'a> PacketCursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            big_clr_chunks: true,
        }
    }

    fn with_big_clr_chunks(data: &'a [u8], big_clr_chunks: bool) -> Self {
        Self {
            data,
            pos: 0,
            big_clr_chunks,
        }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn read_u8(&mut self) -> Result<u8, OracleThinError> {
        let value = *self
            .data
            .get(self.pos)
            .ok_or_else(|| OracleThinError::new("short TTC packet while reading u8"))?;
        self.pos += 1;
        Ok(value)
    }

    fn read_i8(&mut self) -> Result<i8, OracleThinError> {
        Ok(self.read_u8()? as i8)
    }

    fn read_u16_be(&mut self) -> Result<u16, OracleThinError> {
        let bytes = self.read_raw(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn read_u16_le(&mut self) -> Result<u16, OracleThinError> {
        let bytes = self.read_raw(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn read_raw(&mut self, len: usize) -> Result<&'a [u8], OracleThinError> {
        let end = self.pos.saturating_add(len);
        let bytes = self
            .data
            .get(self.pos..end)
            .ok_or_else(|| OracleThinError::new("short TTC packet while reading bytes"))?;
        self.pos = end;
        Ok(bytes)
    }

    fn skip(&mut self, len: usize) -> Result<(), OracleThinError> {
        self.read_raw(len).map(|_| ())
    }

    fn read_null_terminated_bytes(&mut self) -> Result<&'a [u8], OracleThinError> {
        let start = self.pos;
        while self.pos < self.data.len() {
            if self.data[self.pos] == 0 {
                let bytes = &self.data[start..self.pos];
                self.pos += 1;
                return Ok(bytes);
            }
            self.pos += 1;
        }
        Err(OracleThinError::new(
            "unterminated TTC null-terminated byte field",
        ))
    }

    fn read_bytes(&mut self) -> Result<Option<Vec<u8>>, OracleThinError> {
        let len = self.read_u8()?;
        match len {
            0 | 0xff => Ok(None),
            0xfe => {
                let mut out = Vec::new();
                loop {
                    let chunk_len = if self.big_clr_chunks {
                        self.read_ub4()? as usize
                    } else {
                        self.read_u8()? as usize
                    };
                    if chunk_len == 0 {
                        break;
                    }
                    out.extend_from_slice(self.read_raw(chunk_len)?);
                }
                Ok(Some(out))
            }
            len => Ok(Some(self.read_raw(usize::from(len))?.to_vec())),
        }
    }

    fn read_ub2(&mut self) -> Result<u16, OracleThinError> {
        let value = self.read_universal_uint(2)?;
        u16::try_from(value)
            .map_err(|_| OracleThinError::new(format!("TTC ub2 out of range: {value}")))
    }

    fn read_sb2(&mut self) -> Result<i16, OracleThinError> {
        let len_byte = self.read_u8()?;
        let is_negative = len_byte & 0x80 != 0;
        let len = usize::from(len_byte & 0x7f);
        if len == 0 {
            return Ok(0);
        }
        if len > 2 {
            return Err(OracleThinError::new(format!(
                "invalid TTC signed ub2 length {len}"
            )));
        }
        let bytes = self.read_raw(len)?;
        let mut value = 0i16;
        for byte in bytes {
            value = (value << 8) | i16::from(*byte);
        }
        Ok(if is_negative { -value } else { value })
    }

    fn read_sb4(&mut self) -> Result<i32, OracleThinError> {
        let len_byte = self.read_u8()?;
        let is_negative = len_byte & 0x80 != 0;
        let len = usize::from(len_byte & 0x7f);
        if len == 0 {
            return Ok(0);
        }
        if len > 4 {
            return Err(OracleThinError::new(format!(
                "invalid TTC signed ub4 length {len}"
            )));
        }
        let bytes = self.read_raw(len)?;
        let mut value = 0i32;
        for byte in bytes {
            value = (value << 8) | i32::from(*byte);
        }
        Ok(if is_negative { -value } else { value })
    }

    fn read_ub4(&mut self) -> Result<u32, OracleThinError> {
        let value = self.read_universal_uint(4)?;
        u32::try_from(value)
            .map_err(|_| OracleThinError::new(format!("TTC ub4 out of range: {value}")))
    }

    fn read_ub8(&mut self) -> Result<u64, OracleThinError> {
        self.read_universal_uint(8)
    }

    fn read_universal_uint(&mut self, max_len: usize) -> Result<u64, OracleThinError> {
        let start = self.pos;
        let len = usize::from(self.read_u8()?);
        if len == 0 {
            return Ok(0);
        }
        if len > max_len || len > 8 {
            let context_end = self.data.len().min(start + 16);
            return Err(OracleThinError::new(format!(
                "invalid TTC universal integer length {len} at offset {start}; context={:02x?}",
                &self.data[start..context_end]
            )));
        }
        let bytes = self.read_raw(len)?;
        let mut value = 0u64;
        for byte in bytes {
            value = (value << 8) | u64::from(*byte);
        }
        Ok(value)
    }

    fn read_str_with_ub4_length(&mut self) -> Result<Option<String>, OracleThinError> {
        self.read_str_with_length()
    }

    fn skip_bytes_with_ub4_length(&mut self) -> Result<(), OracleThinError> {
        let expected_len = self.read_ub4()? as usize;
        if expected_len == 0 {
            return Ok(());
        }
        if let Some(bytes) = self.read_bytes()? {
            if bytes.len() < expected_len {
                return Err(OracleThinError::new("short TTC bytes-with-length field"));
            }
        }
        Ok(())
    }

    fn skip_bytes(&mut self) -> Result<(), OracleThinError> {
        let _ = self.read_bytes()?;
        Ok(())
    }

    fn read_str_with_length(&mut self) -> Result<Option<String>, OracleThinError> {
        let expected_len = self.read_ub4()? as usize;
        if expected_len == 0 {
            return Ok(None);
        }
        let Some(mut bytes) = self.read_bytes()? else {
            return Ok(None);
        };
        if bytes.len() > expected_len {
            bytes.truncate(expected_len);
        }
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|err| OracleThinError::new(format!("invalid UTF-8 TTC string: {err}")))
    }

    fn read_str(&mut self) -> Result<Option<String>, OracleThinError> {
        let Some(bytes) = self.read_bytes()? else {
            return Ok(None);
        };
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|err| OracleThinError::new(format!("invalid UTF-8 TTC string: {err}")))
    }
}

fn put_u16(buf: &mut [u8], offset: usize, value: u16) {
    buf[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

fn put_u32(buf: &mut [u8], offset: usize, value: u32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn default_ttc_field_version(protocol_version: u16) -> u8 {
    match protocol_version {
        0..=314 => 6,
        315 => 12,
        316 | 317 => 18,
        318 => 21,
        _ => 24,
    }
}

#[allow(dead_code)]
fn bind_count(request: &StatementRequest) -> usize {
    request
        .binds
        .iter()
        .filter(|bind| !matches!(bind, BindValue::Null(_)))
        .count()
}

#[cfg(test)]
mod tests {
    use super::decode_oracle_number;

    #[test]
    fn decode_oracle_number_trims_fractional_trailing_zeros() {
        assert_eq!(decode_oracle_number(&[0xc0, 0x33]).unwrap(), "0.5");
        assert_eq!(decode_oracle_number(&[0xc0, 0x51]).unwrap(), "0.8");
    }
}
