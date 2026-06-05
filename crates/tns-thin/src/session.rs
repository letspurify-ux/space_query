// Portions of this file have been modified from, and reimplemented in Rust
// based on, the thin protocol implementation in python-oracledb
// (https://github.com/oracle/python-oracledb),
// Copyright (c) 2016, 2026, Oracle and/or its affiliates, used under the
// Apache License, Version 2.0. This is a modified work and is not the original
// python-oracledb software. Protocol constants were also cross-checked
// against go-ora (MIT License, Copyright (c) 2020 Samy Sultan).
// See THIRD_PARTY_NOTICES.md.

use std::collections::{HashMap, HashSet};
#[cfg(unix)]
use std::ffi::CString;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream};
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use aes::{Aes128, Aes192, Aes256};
use cbc::cipher::{
    block_padding::{NoPadding, Pkcs7},
    BlockDecryptMut, BlockEncryptMut, KeyIvInit,
};
use md5::Md5;
use once_cell::sync::OnceCell;
use pbkdf2::pbkdf2_hmac;
use rand::{rngs::OsRng, RngCore};
use serde_json::Value as JsonValue;
use sha1::Sha1;
use sha2::{Digest, Sha512};

use crate::connect::{
    connect_data_descriptor_parts, connect_description_option_parts,
    validate_connect_descriptor_value, AcceptInfo, ConnectOptions, ConnectTarget,
    OracleNetConnector, TNS_DEFAULT_SOCKET_TIMEOUT, TNS_MIN_SUPPORTED_PROTOCOL,
};
use crate::exec::{
    sql_is_dml_returning, BindInputValue, BindValue, ColumnMetadata, DescribedQueryResult,
    ExecuteWithImplicitResult, OracleColumnType, OracleIntervalDaySecond, OracleIntervalYearMonth,
    OracleValue, OracleVectorValue, OutBindResult, QueryResult, RefCursorValue, StatementRequest,
};
use crate::{log_connect_phase, OracleThinError};

const TNS_PACKET_TYPE_DATA: u8 = 6;
const TNS_PACKET_TYPE_MARKER: u8 = 12;
const TNS_PACKET_TYPE_CONTROL: u8 = 14;
const TNS_MARKER_TYPE_BREAK: u8 = 1;
const TNS_MARKER_TYPE_RESET: u8 = 2;
const TNS_MARKER_TYPE_INTERRUPT: u8 = 3;
const TNS_CONTROL_TYPE_INBAND_NOTIFICATION: u16 = 8;
const TNS_CONTROL_TYPE_RESET_OOB: u16 = 9;
const TNS_DEFAULT_SDU: usize = 8192;
const TNS_DATA_PACKET_OVERHEAD: usize = 64;
const TNS_DATA_FLAGS_EOF: u16 = 0x0040;
const TNS_DATA_FLAGS_END_OF_RESPONSE: u16 = 0x2000;
/// Per-read timeout while draining the server's break/reset response during a
/// graceful cancel. Kept well under the app's minimum cancel timeout (1s,
/// MIN_CANCEL_TIMEOUT_SECONDS) so the tier-2 force-close watchdog never
/// interrupts an in-progress graceful drain and wrongly marks it broken.
const CANCEL_RESET_DRAIN_TIMEOUT: Duration = Duration::from_millis(750);
/// Safety bound on packets drained during a cancel reset handshake.
const CANCEL_RESET_MAX_PACKETS: usize = 64;
const TNS_MSG_TYPE_PROTOCOL: u8 = 1;
const TNS_MSG_TYPE_DATA_TYPES: u8 = 2;
const TNS_MSG_TYPE_FUNCTION: u8 = 3;
const TNS_MSG_TYPE_ERROR: u8 = 4;
const TNS_MSG_TYPE_ROW_HEADER: u8 = 6;
const TNS_MSG_TYPE_ROW_DATA: u8 = 7;
const TNS_MSG_TYPE_PARAMETER: u8 = 8;
const TNS_MSG_TYPE_STATUS: u8 = 9;
const TNS_MSG_TYPE_IO_VECTOR: u8 = 11;
const TNS_MSG_TYPE_LOB_DATA: u8 = 14;
const TNS_MSG_TYPE_WARNING: u8 = 15;
const TNS_MSG_TYPE_DESCRIBE_INFO: u8 = 16;
const TNS_MSG_TYPE_PIGGYBACK: u8 = 17;
const TNS_MSG_TYPE_FLUSH_OUT_BINDS: u8 = 19;
const TNS_MSG_TYPE_BIT_VECTOR: u8 = 21;
const TNS_MSG_TYPE_SERVER_SIDE_PIGGYBACK: u8 = 23;
const TNS_MSG_TYPE_IMPLICIT_RESULTSET: u8 = 27;
const TNS_MSG_TYPE_END_OF_RESPONSE: u8 = 29;
const TNS_MSG_TYPE_TOKEN: u8 = 33;
const TNS_DEFAULT_TOKEN_NUM: u64 = 0;
const TNS_ERR_EXCEEDED_IDLE_TIME: u32 = 2396;
const TNS_ERR_SESSION_SHUTDOWN: u32 = 12572;
const TNS_ERR_INBAND_MESSAGE: u32 = 12573;
const TNS_EOCS_FLAGS_TXN_IN_PROGRESS: u32 = 0x00000002;
const TNS_WARN_COMPILATION_ERROR: u32 = 24344;
const TNS_SERVER_PIGGYBACK_QUERY_CACHE_INVALIDATION: u8 = 1;
const TNS_SERVER_PIGGYBACK_OS_PID_MTS: u8 = 2;
const TNS_SERVER_PIGGYBACK_TRACE_EVENT: u8 = 3;
const TNS_SERVER_PIGGYBACK_SESS_RET: u8 = 4;
const TNS_SERVER_PIGGYBACK_SYNC: u8 = 5;
const TNS_SERVER_PIGGYBACK_LTXID: u8 = 7;
const TNS_SERVER_PIGGYBACK_AC_REPLAY_CONTEXT: u8 = 8;
const TNS_SERVER_PIGGYBACK_EXT_SYNC: u8 = 9;
const TNS_SERVER_PIGGYBACK_SESS_SIGNATURE: u8 = 10;
const TNS_KEYWORD_NUM_CURRENT_SCHEMA: u16 = 168;
const TNS_KEYWORD_NUM_EDITION: u16 = 172;
const TNS_KEYWORD_NUM_TRANSACTION_ID: u16 = 201;
const TNS_TPC_TXNID_SYNC_SERVER: u8 = 0x01;
const TNS_TPC_TXNID_SYNC_SET: u8 = 0x40;
const TNS_TPC_TXNID_SYNC_UNSET: u8 = 0x80;
const TNS_SESSION_STATE_REQUEST_BEGIN: u8 = 0x04;
const TNS_SESSION_STATE_REQUEST_END: u8 = 0x08;
const TNS_SESSION_STATE_EXPLICIT_BOUNDARY: u8 = 0x40;
const TNS_FUNC_COMMIT: u8 = 14;
const TNS_FUNC_ROLLBACK: u8 = 15;
const TNS_FUNC_LOGOFF: u8 = 9;
const TNS_FUNC_FETCH: u8 = 5;
const TNS_FUNC_EXECUTE: u8 = 94;
const TNS_FUNC_LOB_OP: u8 = 96;
const TNS_FUNC_CLOSE_CURSORS: u8 = 105;
const TNS_FUNC_AUTH_PHASE_ONE: u8 = 118;
const TNS_FUNC_AUTH_PHASE_TWO: u8 = 115;
const TNS_FUNC_SET_END_TO_END_ATTR: u8 = 135;
const TNS_FUNC_PING: u8 = 147;
const TNS_FUNC_SET_SCHEMA: u8 = 152;
const TNS_FUNC_SESSION_STATE: u8 = 176;
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
const TNS_END_TO_END_CLIENT_IDENTIFIER: u32 = 0x0000_0001;
const TNS_END_TO_END_MODULE: u32 = 0x0000_0008;
const TNS_END_TO_END_ACTION: u32 = 0x0000_0010;
const TNS_END_TO_END_CLIENT_INFO: u32 = 0x0000_0100;
const TNS_END_TO_END_DBOP: u32 = 0x0000_0200;
const TNS_BIND_USE_INDICATORS: u8 = 0x01;
const TNS_BIND_DIR_INPUT: u8 = 32;
const TNS_MAX_LONG_LENGTH: u32 = 0x7fff_ffff;
const TNS_MAX_ROWID_LENGTH: u32 = 18;
const TNS_MAX_UROWID_LENGTH: u32 = 5267;
const TNS_CHARSET_UTF8: u16 = 873;
const TNS_ERR_NO_DATA_FOUND: u32 = 1403;
const TNS_AUTH_MODE_LOGON: u32 = 0x0000_0001;
const TNS_AUTH_MODE_CHANGE_PASSWORD: u32 = 0x0000_0002;
const TNS_AUTH_MODE_SYSDBA: u32 = 0x0000_0020;
const TNS_AUTH_MODE_SYSOPER: u32 = 0x0000_0040;
const TNS_AUTH_MODE_WITH_PASSWORD: u32 = 0x0000_0100;
const TNS_AUTH_MODE_SYSASM: u32 = 0x0040_0000;
const TNS_AUTH_MODE_SYSBKP: u32 = 0x0100_0000;
const TNS_AUTH_MODE_SYSDGD: u32 = 0x0200_0000;
const TNS_AUTH_MODE_SYSKMT: u32 = 0x0400_0000;
const TNS_AUTH_MODE_SYSRAC: u32 = 0x0800_0000;
const TNS_VERIFIER_TYPE_10G: u32 = 0x0939;
const TNS_VERIFIER_TYPE_11G_1: u32 = 0xb152;
const TNS_VERIFIER_TYPE_11G_2: u32 = 0x1b25;
const TNS_VERIFIER_TYPE_12C: u32 = 0x4815;
const TNS_LEGACY_DES_KEY: [u8; 8] = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
const DES_INITIAL_PERMUTATION: [u8; 64] = [
    58, 50, 42, 34, 26, 18, 10, 2, 60, 52, 44, 36, 28, 20, 12, 4, 62, 54, 46, 38, 30, 22, 14, 6,
    64, 56, 48, 40, 32, 24, 16, 8, 57, 49, 41, 33, 25, 17, 9, 1, 59, 51, 43, 35, 27, 19, 11, 3, 61,
    53, 45, 37, 29, 21, 13, 5, 63, 55, 47, 39, 31, 23, 15, 7,
];
const DES_FINAL_PERMUTATION: [u8; 64] = [
    40, 8, 48, 16, 56, 24, 64, 32, 39, 7, 47, 15, 55, 23, 63, 31, 38, 6, 46, 14, 54, 22, 62, 30,
    37, 5, 45, 13, 53, 21, 61, 29, 36, 4, 44, 12, 52, 20, 60, 28, 35, 3, 43, 11, 51, 19, 59, 27,
    34, 2, 42, 10, 50, 18, 58, 26, 33, 1, 41, 9, 49, 17, 57, 25,
];
const DES_PC1: [u8; 56] = [
    57, 49, 41, 33, 25, 17, 9, 1, 58, 50, 42, 34, 26, 18, 10, 2, 59, 51, 43, 35, 27, 19, 11, 3, 60,
    52, 44, 36, 63, 55, 47, 39, 31, 23, 15, 7, 62, 54, 46, 38, 30, 22, 14, 6, 61, 53, 45, 37, 29,
    21, 13, 5, 28, 20, 12, 4,
];
const DES_PC2: [u8; 48] = [
    14, 17, 11, 24, 1, 5, 3, 28, 15, 6, 21, 10, 23, 19, 12, 4, 26, 8, 16, 7, 27, 20, 13, 2, 41, 52,
    31, 37, 47, 55, 30, 40, 51, 45, 33, 48, 44, 49, 39, 56, 34, 53, 46, 42, 50, 36, 29, 32,
];
const DES_KEY_SHIFTS: [u8; 16] = [1, 1, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 2, 2, 2, 1];
const DES_EXPANSION: [u8; 48] = [
    32, 1, 2, 3, 4, 5, 4, 5, 6, 7, 8, 9, 8, 9, 10, 11, 12, 13, 12, 13, 14, 15, 16, 17, 16, 17, 18,
    19, 20, 21, 20, 21, 22, 23, 24, 25, 24, 25, 26, 27, 28, 29, 28, 29, 30, 31, 32, 1,
];
const DES_P_PERMUTATION: [u8; 32] = [
    16, 7, 20, 21, 29, 12, 28, 17, 1, 15, 23, 26, 5, 18, 31, 10, 2, 8, 24, 14, 32, 27, 3, 9, 19,
    13, 30, 6, 22, 11, 4, 25,
];
const DES_S_BOXES: [[u8; 64]; 8] = [
    [
        14, 4, 13, 1, 2, 15, 11, 8, 3, 10, 6, 12, 5, 9, 0, 7, 0, 15, 7, 4, 14, 2, 13, 1, 10, 6, 12,
        11, 9, 5, 3, 8, 4, 1, 14, 8, 13, 6, 2, 11, 15, 12, 9, 7, 3, 10, 5, 0, 15, 12, 8, 2, 4, 9,
        1, 7, 5, 11, 3, 14, 10, 0, 6, 13,
    ],
    [
        15, 1, 8, 14, 6, 11, 3, 4, 9, 7, 2, 13, 12, 0, 5, 10, 3, 13, 4, 7, 15, 2, 8, 14, 12, 0, 1,
        10, 6, 9, 11, 5, 0, 14, 7, 11, 10, 4, 13, 1, 5, 8, 12, 6, 9, 3, 2, 15, 13, 8, 10, 1, 3, 15,
        4, 2, 11, 6, 7, 12, 0, 5, 14, 9,
    ],
    [
        10, 0, 9, 14, 6, 3, 15, 5, 1, 13, 12, 7, 11, 4, 2, 8, 13, 7, 0, 9, 3, 4, 6, 10, 2, 8, 5,
        14, 12, 11, 15, 1, 13, 6, 4, 9, 8, 15, 3, 0, 11, 1, 2, 12, 5, 10, 14, 7, 1, 10, 13, 0, 6,
        9, 8, 7, 4, 15, 14, 3, 11, 5, 2, 12,
    ],
    [
        7, 13, 14, 3, 0, 6, 9, 10, 1, 2, 8, 5, 11, 12, 4, 15, 13, 8, 11, 5, 6, 15, 0, 3, 4, 7, 2,
        12, 1, 10, 14, 9, 10, 6, 9, 0, 12, 11, 7, 13, 15, 1, 3, 14, 5, 2, 8, 4, 3, 15, 0, 6, 10, 1,
        13, 8, 9, 4, 5, 11, 12, 7, 2, 14,
    ],
    [
        2, 12, 4, 1, 7, 10, 11, 6, 8, 5, 3, 15, 13, 0, 14, 9, 14, 11, 2, 12, 4, 7, 13, 1, 5, 0, 15,
        10, 3, 9, 8, 6, 4, 2, 1, 11, 10, 13, 7, 8, 15, 9, 12, 5, 6, 3, 0, 14, 11, 8, 12, 7, 1, 14,
        2, 13, 6, 15, 0, 9, 10, 4, 5, 3,
    ],
    [
        12, 1, 10, 15, 9, 2, 6, 8, 0, 13, 3, 4, 14, 7, 5, 11, 10, 15, 4, 2, 7, 12, 9, 5, 6, 1, 13,
        14, 0, 11, 3, 8, 9, 14, 15, 5, 2, 8, 12, 3, 7, 0, 4, 10, 1, 13, 11, 6, 4, 3, 2, 12, 9, 5,
        15, 10, 11, 14, 1, 7, 6, 0, 8, 13,
    ],
    [
        4, 11, 2, 14, 15, 0, 8, 13, 3, 12, 9, 7, 5, 10, 6, 1, 13, 0, 11, 7, 4, 9, 1, 10, 14, 3, 5,
        12, 2, 15, 8, 6, 1, 4, 11, 13, 12, 3, 7, 14, 10, 15, 6, 8, 0, 5, 9, 2, 6, 11, 13, 8, 1, 4,
        10, 7, 9, 5, 0, 15, 14, 2, 3, 12,
    ],
    [
        13, 2, 8, 4, 6, 15, 11, 1, 10, 9, 3, 14, 5, 0, 12, 7, 1, 15, 13, 8, 10, 3, 7, 4, 12, 5, 6,
        11, 0, 14, 9, 2, 7, 11, 4, 1, 9, 12, 14, 2, 0, 6, 10, 13, 15, 3, 5, 8, 2, 1, 14, 7, 4, 10,
        8, 13, 15, 12, 9, 0, 3, 5, 6, 11,
    ],
];
const TNS_CCAP_FIELD_VERSION: usize = 7;
const TNS_CCAP_FIELD_VERSION_12_2: u8 = 8;
const TNS_CCAP_FIELD_VERSION_12_2_EXT1: u8 = 9;
const TNS_CCAP_FIELD_VERSION_20_1: u8 = 14;
const TNS_CCAP_FIELD_VERSION_23_1: u8 = 17;
const TNS_CCAP_FIELD_VERSION_23_1_EXT_1: u8 = 18;
const TNS_CCAP_FIELD_VERSION_23_1_EXT_3: u8 = 20;
const TNS_CCAP_FIELD_VERSION_23_4: u8 = 24;
const TNS_CCAP_FIELD_VERSION_MAX: u8 = TNS_CCAP_FIELD_VERSION_23_4;
const TNS_CCAP_TTC1: usize = 15;
const TNS_CCAP_END_OF_CALL_STATUS: u8 = 0x01;
const TNS_CCAP_OCI1: usize = 16;
const TNS_CCAP_LEGACY_FAST_SESSION_ATTRIBUTES: u8 = 0x01;
const TNS_CCAP_TTC3: usize = 37;
const TNS_CCAP_BIG_CHUNK_CLR: u8 = 0x20;
const TNS_CCAP_TTC4: usize = 40;
const TNS_CCAP_EXPLICIT_BOUNDARY: u8 = 0x40;
const TNS_CCAP_END_OF_RESPONSE: u8 = 0x20;
const TNS_RCAP_TTC: usize = 6;
const TNS_RCAP_TTC_ZERO_COPY: u8 = 0x01;
const TNS_RCAP_TTC_32K: u8 = 0x04;
const TNS_RCAP_TTC_SESSION_STATE_OPS: u8 = 0x10;
const TNS_VERSION_MIN_ACCEPTED: u16 = 315;
const TNS_VERSION_MIN_END_OF_RESPONSE: u16 = 319;
const TNS_LEGACY_CLR_CHUNK_SIZE: usize = 0x40;
const TNS_BIG_CLR_CHUNK_SIZE: usize = 32_767;
const TNS_ESCAPE_CHAR: u8 = 0xfd;
const TNS_LEGACY_NULL_LENGTH_INDICATOR: u8 = 0xfd;
const ORA_TYPE_NUM_VARCHAR: u8 = 1;
const ORA_TYPE_NUM_NUMBER: u8 = 2;
const TNS_DATA_TYPE_BINARY_INTEGER: u8 = 3;
const TNS_DATA_TYPE_FLOAT: u8 = 4;
const TNS_DATA_TYPE_STR: u8 = 5;
const TNS_DATA_TYPE_VNU: u8 = 6;
const TNS_DATA_TYPE_PDN: u8 = 7;
const ORA_TYPE_NUM_LONG: u8 = 8;
const TNS_DATA_TYPE_VCS: u8 = 9;
const ORA_TYPE_NUM_ROWID: u8 = 11;
const ORA_TYPE_NUM_DATE: u8 = 12;
const TNS_DATA_TYPE_VBI: u8 = 15;
const TNS_DATA_TYPE_BFLOAT: u8 = 21;
const TNS_DATA_TYPE_BDOUBLE: u8 = 22;
const ORA_TYPE_NUM_RAW: u8 = 23;
const ORA_TYPE_NUM_LONG_RAW: u8 = 24;
const TNS_DATA_TYPE_OAC9: u8 = 39;
const TNS_DATA_TYPE_UIN: u8 = 68;
const TNS_DATA_TYPE_SLS: u8 = 91;
const TNS_DATA_TYPE_LVC: u8 = 94;
const TNS_DATA_TYPE_LVB: u8 = 95;
const ORA_TYPE_NUM_CHAR: u8 = 96;
const TNS_DATA_TYPE_CHARZ: u8 = 97;
const ORA_TYPE_NUM_BINARY_FLOAT: u8 = 100;
const ORA_TYPE_NUM_BINARY_DOUBLE: u8 = 101;
const ORA_TYPE_NUM_CURSOR: u8 = 102;
const TNS_DATA_TYPE_RDD: u8 = 104;
const TNS_DATA_TYPE_EXT_NAMED: u8 = 108;
const ORA_TYPE_NUM_OBJECT: u8 = 109;
const TNS_DATA_TYPE_EXT_REF: u8 = 110;
const TNS_DATA_TYPE_INT_REF: u8 = 111;
const ORA_TYPE_NUM_CLOB: u8 = 112;
const ORA_TYPE_NUM_BLOB: u8 = 113;
const ORA_TYPE_NUM_BFILE: u8 = 114;
const TNS_DATA_TYPE_CFILE: u8 = 115;
const TNS_DATA_TYPE_RSET: u8 = 116;
const ORA_TYPE_NUM_JSON: u8 = 119;
const ORA_TYPE_NUM_VECTOR: u8 = 127;
const TNS_DATA_TYPE_OAC: u16 = 646;
const TNS_DATA_TYPE_CLV: u8 = 146;
const TNS_DATA_TYPE_DTR: u8 = 152;
const TNS_DATA_TYPE_DUN: u8 = 153;
const TNS_DATA_TYPE_DOP: u8 = 154;
const TNS_DATA_TYPE_VST: u8 = 155;
const TNS_DATA_TYPE_ODT: u8 = 156;
const TNS_DATA_TYPE_DOL: u8 = 172;
const TNS_DATA_TYPE_TIME: u8 = 178;
const TNS_DATA_TYPE_TIME_TZ: u8 = 179;
const ORA_TYPE_NUM_TIMESTAMP: u8 = 180;
const ORA_TYPE_NUM_TIMESTAMP_TZ: u8 = 181;
const ORA_TYPE_NUM_INTERVAL_YM: u8 = 182;
const ORA_TYPE_NUM_INTERVAL_DS: u8 = 183;
const TNS_DATA_TYPE_EDATE: u8 = 184;
const TNS_DATA_TYPE_ETIME: u8 = 185;
const TNS_DATA_TYPE_ETTZ: u8 = 186;
const ORA_TYPE_NUM_TIMESTAMP_DTY: u8 = 187;
const ORA_TYPE_NUM_TIMESTAMP_TZ_EXT: u8 = 188;
const ORA_TYPE_NUM_INTERVAL_YM_DTY: u8 = 189;
const ORA_TYPE_NUM_INTERVAL_DS_DTY: u8 = 190;
const TNS_DATA_TYPE_DCLOB: u8 = 195;
const TNS_DATA_TYPE_DBLOB: u8 = 196;
const ORA_TYPE_NUM_DBFILE: u8 = 197;
const ORA_TYPE_NUM_DJSON: u8 = 198;
const ORA_TYPE_NUM_UROWID: u8 = 208;
const ORA_TYPE_NUM_TIMESTAMP_LTZ: u8 = 231;
const TNS_DATA_TYPE_ESITZ: u8 = 232;
const TNS_DATA_TYPE_UB8: u8 = 233;
const TNS_DATA_TYPE_PNTY: u8 = 241;
const ORA_TYPE_NUM_BOOLEAN: u8 = 252;
const TNS_LONG_LENGTH_INDICATOR: u8 = 0xfe;
const TNS_JSON_MAX_LENGTH: u32 = 32 * 1024 * 1024;
const TNS_VECTOR_MAX_LENGTH: u32 = 1_048_576;
const TNS_LOB_PREFETCH_FLAG: u64 = 0x0200_0000;
const TNS_LOB_OP_READ: u32 = 0x0002;
const TNS_LOB_OP_WRITE: u32 = 0x0040;
const TNS_LOB_OP_CREATE_TEMP: u32 = 0x0110;
const TNS_LOB_OP_FREE_TEMP: u32 = 0x0111;
const TNS_LOB_OP_ARRAY: u32 = 0x80000;
const TNS_VECTOR_MAGIC_BYTE: u8 = 0xdb;
const TNS_VECTOR_VERSION_BASE: u8 = 0;
const TNS_VECTOR_VERSION_WITH_BINARY: u8 = 1;
const TNS_VECTOR_VERSION_WITH_SPARSE: u8 = 2;
const TNS_VECTOR_FLAG_NORM: u16 = 0x0002;
const TNS_VECTOR_FLAG_NORM_RESERVED: u16 = 0x0010;
const TNS_VECTOR_FLAG_SPARSE: u16 = 0x0020;
const TNS_VECTOR_FORMAT_FLOAT32: u8 = 2;
const TNS_VECTOR_FORMAT_FLOAT64: u8 = 3;
const TNS_VECTOR_FORMAT_INT8: u8 = 4;
const TNS_VECTOR_FORMAT_BINARY: u8 = 5;
const TNS_OBJ_IS_DEGENERATE: u8 = 0x10;
const TNS_OBJ_NO_PREFIX_SEG: u8 = 0x04;
const TNS_OBJ_HAS_INDEXES: u8 = 0x10;
const TNS_XML_TYPE_LOB: u32 = 0x0001;
const TNS_XML_TYPE_STRING: u32 = 0x0004;
const TNS_XML_TYPE_FLAG_SKIP_NEXT_4: u32 = 0x0010_0000;
const TNS_UDS_FLAGS_IS_JSON: u32 = 0x0000_0100;
const TNS_UDS_FLAGS_IS_OSON: u32 = 0x0000_0800;
const TNS_DURATION_MID: i64 = 0x8000_0000;
const TNS_DURATION_OFFSET: i32 = 60;
const TNS_DURATION_SESSION: u32 = 10;
const CS_FORM_IMPLICIT: u8 = 1;
const CS_FORM_NCHAR: u8 = 2;
const ORACLE_CHARSET_US7ASCII: u16 = 1;
const ORACLE_CHARSET_UTF8: u16 = 0x367;
const ORACLE_CHARSET_AL32UTF8: u16 = 873;
const ORACLE_CHARSET_AL16UTF16: u16 = 2000;
const ORACLE_CHARSET_JA16EUC: u16 = 0x33e;
const ORACLE_CHARSET_JA16EUCTILDE: u16 = 0x345;
const ORACLE_CHARSET_JA16SJIS: u16 = 0x340;
const ORACLE_CHARSET_JA16SJISTILDE: u16 = 0x346;
const ORACLE_CHARSET_KO16KSC5601: u16 = 0x348;
const ORACLE_CHARSET_KO16MSWIN949: u16 = 0x34e;
const ORACLE_CHARSET_ZHS16GBK: u16 = 0x354;
const ORACLE_CHARSET_ZHT16BIG5: u16 = 0x361;
const ORACLE_CHARSET_ZHT16MSWIN950: u16 = 0x363;
const ORACLE_CHARSET_ZHT16HKSCS: u16 = 0x364;
const TNS_BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const TNS_JSON_MAGIC_BYTE_1: u8 = 0xff;
const TNS_JSON_MAGIC_BYTE_2: u8 = 0x4a;
const TNS_JSON_MAGIC_BYTE_3: u8 = 0x5a;
const TNS_JSON_VERSION_MAX_FNAME_255: u8 = 1;
const TNS_JSON_VERSION_MAX_FNAME_65535: u8 = 3;
const TNS_JSON_FLAG_HASH_ID_UINT8: u16 = 0x0100;
const TNS_JSON_FLAG_NUM_FNAMES_UINT16: u16 = 0x0400;
const TNS_JSON_FLAG_FNAMES_SEG_UINT32: u16 = 0x0800;
const TNS_JSON_FLAG_TREE_SEG_UINT32: u16 = 0x1000;
const TNS_JSON_FLAG_TINY_NODES_STAT: u16 = 0x2000;
const TNS_JSON_FLAG_REL_OFFSET_MODE: u16 = 0x01;
const TNS_JSON_FLAG_INLINE_LEAF: u16 = 0x02;
const TNS_JSON_FLAG_NUM_FNAMES_UINT32: u16 = 0x08;
const TNS_JSON_FLAG_IS_SCALAR: u16 = 0x10;
const TNS_JSON_FLAG_SEC_FNAMES_SEG_UINT16: u16 = 0x0100;
const TNS_LOB_QLOCATOR_VERSION: u16 = 4;
const TNS_LOB_LOC_FLAGS_BLOB: u8 = 0x01;
const TNS_LOB_LOC_FLAGS_VALUE_BASED: u8 = 0x20;
const TNS_LOB_LOC_FLAGS_ABSTRACT: u8 = 0x40;
const TNS_LOB_LOC_FLAGS_INIT: u8 = 0x08;
const TNS_LOB_LOC_OFFSET_FLAG_3: usize = 6;
const TNS_LOB_LOC_OFFSET_FLAG_4: usize = 7;
const TNS_LOB_LOC_FLAGS_VAR_LENGTH_CHARSET: u8 = 0x80;
const TNS_LOB_LOC_FLAGS_LITTLE_ENDIAN: u8 = 0x40;
const TNS_MAX_SHORT_LOB_INOUT_SIZE: usize = 32_767;
const TNS_JSON_TYPE_NULL: u8 = 0x30;
const TNS_JSON_TYPE_TRUE: u8 = 0x31;
const TNS_JSON_TYPE_FALSE: u8 = 0x32;
const TNS_JSON_TYPE_STRING_LENGTH_UINT8: u8 = 0x33;
const TNS_JSON_TYPE_NUMBER_LENGTH_UINT8: u8 = 0x34;
const TNS_JSON_TYPE_BINARY_DOUBLE: u8 = 0x36;
const TNS_JSON_TYPE_STRING_LENGTH_UINT16: u8 = 0x37;
const TNS_JSON_TYPE_STRING_LENGTH_UINT32: u8 = 0x38;
const TNS_JSON_TYPE_TIMESTAMP: u8 = 0x39;
const TNS_JSON_TYPE_BINARY_LENGTH_UINT16: u8 = 0x3a;
const TNS_JSON_TYPE_BINARY_LENGTH_UINT32: u8 = 0x3b;
const TNS_JSON_TYPE_DATE: u8 = 0x3c;
const TNS_JSON_TYPE_INTERVAL_YM: u8 = 0x3d;
const TNS_JSON_TYPE_INTERVAL_DS: u8 = 0x3e;
const TNS_JSON_TYPE_TIMESTAMP_TZ: u8 = 0x7c;
const TNS_JSON_TYPE_TIMESTAMP7: u8 = 0x7d;
const TNS_JSON_TYPE_ID: u8 = 0x7e;
const TNS_JSON_TYPE_BINARY_FLOAT: u8 = 0x7f;
const TNS_JSON_TYPE_OBJECT: u8 = 0x84;
const TNS_JSON_TYPE_ARRAY: u8 = 0xc0;
const TNS_JSON_TYPE_EXTENDED: u8 = 0x7b;
const TNS_JSON_TYPE_VECTOR: u8 = 0x01;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OracleThinConfig {
    pub target: ConnectTarget,
    pub username: String,
    pub password: String,
    pub connect_options: ConnectOptions,
    pub auth_mode: OracleThinAuthMode,
    pub proxy_user: Option<String>,
    pub edition: Option<String>,
    pub connection_class: Option<String>,
    pub purity: OracleThinPurity,
    pub driver_name: Option<String>,
    pub app_context: Vec<OracleThinAppContext>,
    pub debug_jdwp: Option<String>,
    pub terminal: String,
    pub program: String,
    pub machine: String,
    pub os_user: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OracleThinAppContext {
    pub namespace: String,
    pub name: String,
    pub value: String,
}

impl OracleThinAppContext {
    pub fn new(
        namespace: impl Into<String>,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OracleThinAuthMode {
    Default,
    SysDba,
    SysOper,
    SysAsm,
    SysBkp,
    SysDgd,
    SysKmt,
    SysRac,
}

impl OracleThinAuthMode {
    fn tns_bits(self) -> u32 {
        match self {
            Self::Default => 0,
            Self::SysDba => TNS_AUTH_MODE_SYSDBA,
            Self::SysOper => TNS_AUTH_MODE_SYSOPER,
            Self::SysAsm => TNS_AUTH_MODE_SYSASM,
            Self::SysBkp => TNS_AUTH_MODE_SYSBKP,
            Self::SysDgd => TNS_AUTH_MODE_SYSDGD,
            Self::SysKmt => TNS_AUTH_MODE_SYSKMT,
            Self::SysRac => TNS_AUTH_MODE_SYSRAC,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OracleThinPurity {
    Default,
    New,
    SelfConnection,
}

impl OracleThinPurity {
    fn tns_value(self) -> u8 {
        match self {
            Self::Default => 0,
            Self::New => 1,
            Self::SelfConnection => 2,
        }
    }
}

impl OracleThinConfig {
    pub fn new(
        target: ConnectTarget,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        let (username, proxy_user) = parse_user_and_proxy(username.into());
        Self {
            target,
            username,
            password: password.into(),
            connect_options: ConnectOptions::default(),
            auth_mode: OracleThinAuthMode::Default,
            proxy_user,
            edition: None,
            connection_class: None,
            purity: OracleThinPurity::Default,
            driver_name: None,
            app_context: Vec::new(),
            debug_jdwp: None,
            terminal: "unknown".to_string(),
            program: "space-query-thin".to_string(),
            machine: "localhost".to_string(),
            os_user: "space-query".to_string(),
        }
    }
}

fn parse_user_and_proxy(username: String) -> (String, Option<String>) {
    if let Some(start_pos) = username.find('[') {
        if start_pos > 0 && username.ends_with(']') {
            let proxy_user = username[start_pos + 1..username.len() - 1].to_string();
            return (username[..start_pos].to_string(), Some(proxy_user));
        }
    }
    (username, None)
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
    pub supports_oob_check: bool,
    pub supports_big_clr_chunks: bool,
    pub supports_oson_long_field_names: bool,
    auth_uses_pbkdf2_key_derivation: bool,
    sdu: usize,
    server_ttc_field_version: u8,
    supports_end_of_call_status: bool,
    supports_fast_session_attributes: bool,
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
            supports_oob_check: false,
            supports_big_clr_chunks: false,
            supports_oson_long_field_names: false,
            auth_uses_pbkdf2_key_derivation: false,
            sdu: TNS_DEFAULT_SDU,
            server_ttc_field_version: 0,
            supports_end_of_call_status: true,
            supports_fast_session_attributes: true,
            supports_implicit_resultsets: false,
        }
    }
}

impl OracleThinCapabilities {
    fn data_packet_chunk_size(&self) -> usize {
        self.sdu.max(TNS_DATA_PACKET_OVERHEAD + 1) - TNS_DATA_PACKET_OVERHEAD
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
    /// Tier 1 (graceful): ask the server to abort the running call but keep the
    /// socket open so the reader can run the break/reset handshake and the
    /// connection stays reusable. Mirrors python-oracledb `_break_external` and
    /// go-ora `BreakConnection`, neither of which closes the socket here.
    pub fn break_execution(&self) -> Result<(), OracleThinError> {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(stream) = &self.break_stream {
            let mut stream = stream
                .lock()
                .map_err(|_| OracleThinError::new("Oracle thin cancel stream lock poisoned"))?;
            // Mirror python-oracledb `_break_external`: out-of-band urgent data
            // and the in-band INTERRUPT marker are mutually exclusive. When OOB
            // is available the server is notified via the urgent byte alone;
            // otherwise an in-band marker is the only signal it will see.
            if self.supports_oob {
                let _ = send_oob_break(&stream);
            } else {
                write_marker_packet(
                    &mut stream,
                    self.protocol_version,
                    TNS_MARKER_TYPE_INTERRUPT,
                )?;
            }
        }
        Ok(())
    }

    /// Tier 2 (force): tear down the socket. Used only when tier 1 fails to
    /// release the call within the cancel timeout. The blocked reader unblocks
    /// with a socket error and the connection is marked broken/discarded.
    pub fn force_close(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(stream) = &self.break_stream {
            if let Ok(stream) = stream.lock() {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
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
    last_rows_by_cursor: HashMap<u32, Vec<OracleValue>>,
    cursor_columns_by_cursor: HashMap<u32, Vec<ThinColumn>>,
    ref_cursor_ids: HashSet<u32>,
    object_attrs_by_type: HashMap<(String, String), Vec<ThinColumn>>,
    collection_element_by_type: HashMap<(String, String), ThinColumn>,
    combo_key: Option<Vec<u8>>,
    deferred_cursor_closes: HashMap<u32, HashSet<u32>>,
    deferred_cursor_parent_by_child: HashMap<u32, u32>,
    pending_current_schema: Option<String>,
    pending_end_to_end: EndToEndAttributes,
    server_state: ServerSidePiggybackState,
    in_request: bool,
    cancel_flag: Arc<AtomicBool>,
    ttc_sequence: u8,
    closed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OracleThinWarning {
    pub code: u32,
    pub message: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct EndToEndAttributes {
    action: Option<Option<String>>,
    client_identifier: Option<Option<String>>,
    client_info: Option<Option<String>>,
    dbop: Option<Option<String>>,
    module: Option<Option<String>>,
}

impl EndToEndAttributes {
    fn is_empty(&self) -> bool {
        self.action.is_none()
            && self.client_identifier.is_none()
            && self.client_info.is_none()
            && self.dbop.is_none()
            && self.module.is_none()
    }
}

impl OracleThinSession {
    pub fn connect(config: OracleThinConfig) -> Result<Self, OracleThinError> {
        log_connect_phase("session-connect", &config.target.easy_connect_string());
        let connector = OracleNetConnector::new(config.connect_options.clone());
        let (mut stream, accept) = connector.connect_tcp(&config.target)?;
        validate_supported_protocol(&accept)?;
        let mut capabilities = capabilities_from_accept(&config.connect_options, &accept);
        probe_oob_reset_if_supported(&mut stream, &config.connect_options, &capabilities)?;
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
            last_rows_by_cursor: HashMap::new(),
            cursor_columns_by_cursor: HashMap::new(),
            ref_cursor_ids: HashSet::new(),
            object_attrs_by_type: HashMap::new(),
            collection_element_by_type: HashMap::new(),
            combo_key: auth.combo_key,
            deferred_cursor_closes: HashMap::new(),
            deferred_cursor_parent_by_child: HashMap::new(),
            pending_current_schema: None,
            pending_end_to_end: EndToEndAttributes::default(),
            server_state: auth.server_state,
            in_request: false,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            ttc_sequence: 3,
            closed: false,
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

    pub fn ltxid(&self) -> &[u8] {
        &self.server_state.ltxid
    }

    pub fn server_session_id(&self) -> Option<u32> {
        self.server_state.session_id
    }

    pub fn server_serial_num(&self) -> Option<u16> {
        self.server_state.serial_num
    }

    pub fn server_current_schema(&self) -> Option<&str> {
        self.server_state.current_schema.as_deref()
    }

    pub fn server_edition(&self) -> Option<&str> {
        self.server_state.edition.as_deref()
    }

    pub fn sessionless_transaction_id(&self) -> Option<&[u8]> {
        self.server_state.sessionless_transaction_id.as_deref()
    }

    pub fn sessionless_transaction_started_on_server(&self) -> bool {
        self.server_state.sessionless_started_on_server
    }

    pub fn set_action(&mut self, value: Option<String>) {
        if !self.supports_end_to_end_piggyback() {
            return;
        }
        self.pending_end_to_end.action = Some(value);
    }

    pub fn set_client_identifier(&mut self, value: Option<String>) {
        if !self.supports_end_to_end_piggyback() {
            return;
        }
        self.pending_end_to_end.client_identifier = Some(value);
    }

    pub fn set_client_info(&mut self, value: Option<String>) {
        if !self.supports_end_to_end_piggyback() {
            return;
        }
        self.pending_end_to_end.client_info = Some(value);
    }

    pub fn set_dbop(&mut self, value: Option<String>) {
        if !self.supports_end_to_end_piggyback() {
            return;
        }
        self.pending_end_to_end.dbop = Some(value);
    }

    pub fn set_current_schema(&mut self, value: impl Into<String>) {
        self.pending_current_schema = Some(value.into());
    }

    pub fn set_module(&mut self, value: Option<String>) {
        if !self.supports_end_to_end_piggyback() {
            return;
        }
        self.pending_end_to_end.module = Some(value);
        if self.pending_end_to_end.action.is_none() {
            self.pending_end_to_end.action = Some(None);
        }
    }

    fn supports_end_to_end_piggyback(&self) -> bool {
        self.capabilities.protocol_version.unwrap_or(319) >= 315
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

    /// Tier 1 (graceful) break. See [`OracleThinCancelHandle::break_execution`];
    /// the socket is left open so the reader can run the break/reset handshake.
    pub fn break_execution(&self) -> Result<(), OracleThinError> {
        self.cancel_flag.store(true, Ordering::SeqCst);
        // Mirror python-oracledb `_break_external`: OOB urgent data and the
        // in-band INTERRUPT marker are mutually exclusive (see
        // [`OracleThinCancelHandle::break_execution`]).
        if self.capabilities.supports_oob {
            let _ = send_oob_break(&self.stream);
        } else {
            let mut stream = self.stream.try_clone()?;
            write_marker_packet(
                &mut stream,
                self.capabilities.protocol_version.unwrap_or(319),
                TNS_MARKER_TYPE_INTERRUPT,
            )?;
        }
        Ok(())
    }

    /// Finishes a cancelled call. A tier-1 break leaves the server's break/reset
    /// response pending on the socket; this completes the handshake and drains
    /// it so the connection stays at a clean request boundary and can be reused
    /// (matching the OCI/MySQL cancel flow). If the handshake cannot complete
    /// cleanly — e.g. a tier-2 `force_close` already shut the socket down — the
    /// session is marked broken so the pool discards it. Always reports
    /// ORA-01013 to the caller.
    fn finish_cancelled_read(&mut self) -> OracleThinError {
        if self.drain_cancel_response().is_err() {
            self.broken = true;
        }
        OracleThinError::new("ORA-01013: user requested cancel of current operation")
    }

    /// Completes the break/reset handshake after a graceful cancel so the
    /// connection is left at a clean request boundary and stays reusable.
    ///
    /// The server answers a break by sending a RESET marker and then **waits for
    /// the client's RESET** before emitting the trailing ORA-01013 error data
    /// packet. The in-band reader ([`read_data_packet_with_flags_and_control`])
    /// returns on that server RESET marker without acknowledging it, so we must
    /// send the client RESET here, then drain up to the trailing data packet.
    /// Mirrors go-ora `processMarker` (send RESET after the marker, then read
    /// the data packet) and python-oracledb `_reset` (send RESET, read until the
    /// server reset, then the data packet).
    fn drain_cancel_response(&mut self) -> Result<(), OracleThinError> {
        let protocol_version = self.capabilities.protocol_version.unwrap_or(319);
        write_marker_packet(&mut self.stream, protocol_version, TNS_MARKER_TYPE_RESET)?;

        let prior_timeout = self.stream.read_timeout().ok().flatten();
        self.stream
            .set_read_timeout(Some(CANCEL_RESET_DRAIN_TIMEOUT))
            .map_err(|err| {
                OracleThinError::new(format!("failed to set Oracle cancel reset timeout: {err}"))
            })?;
        let outcome = self.drain_cancel_reset(protocol_version);
        let _ = self.stream.set_read_timeout(prior_timeout);
        outcome
    }

    fn drain_cancel_reset(&mut self, protocol_version: u16) -> Result<(), OracleThinError> {
        // After our client RESET the server emits the trailing ORA-01013 error
        // data packet, possibly preceded by its own RESET marker (python-oracledb
        // protocols 315/318/319) or none (go-ora protocol 314, which reads
        // exactly one data packet after the reset). We skip any markers — also
        // answering a residual BREAK with a RESET — and stop at the first data
        // packet, which marks the clean request boundary. A quiet socket (read
        // timeout at a boundary) is the fallback terminator.
        for _ in 0..CANCEL_RESET_MAX_PACKETS {
            match read_cancel_reset_packet(&mut self.stream, protocol_version)? {
                CancelResetPacket::Quiet => return Ok(()),
                CancelResetPacket::Packet(TNS_PACKET_TYPE_MARKER, data) => {
                    // Server still breaking: acknowledge with a reset marker.
                    if data.last().copied() == Some(TNS_MARKER_TYPE_BREAK) {
                        write_marker_packet(
                            &mut self.stream,
                            protocol_version,
                            TNS_MARKER_TYPE_RESET,
                        )?;
                    }
                }
                CancelResetPacket::Packet(TNS_PACKET_TYPE_CONTROL, _) => {}
                // Trailing error/end-of-response packet: clean request boundary.
                CancelResetPacket::Packet(TNS_PACKET_TYPE_DATA, _) => return Ok(()),
                CancelResetPacket::Packet(other, _) => {
                    return Err(OracleThinError::new(format!(
                        "unexpected TNS packet type {other} during cancel reset"
                    )));
                }
            }
        }
        Err(OracleThinError::new(
            "Oracle thin cancel reset did not reach a clean boundary",
        ))
    }

    pub fn set_call_timeout(&mut self, timeout: Option<Duration>) -> Result<(), OracleThinError> {
        let socket_timeout = Some(
            timeout
                .filter(|timeout| !timeout.is_zero())
                .unwrap_or(TNS_DEFAULT_SOCKET_TIMEOUT),
        );
        self.stream
            .set_read_timeout(socket_timeout)
            .map_err(|err| {
                OracleThinError::new(format!("failed to set Oracle read timeout: {err}"))
            })?;
        self.stream
            .set_write_timeout(socket_timeout)
            .map_err(|err| {
                OracleThinError::new(format!("failed to set Oracle write timeout: {err}"))
            })?;
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
        self.set_call_timeout(None)?;
        if self.broken {
            self.pending_cursor_closes.clear();
            self.last_rows_by_cursor.clear();
            self.cursor_columns_by_cursor.clear();
            self.ref_cursor_ids.clear();
            self.object_attrs_by_type.clear();
            self.collection_element_by_type.clear();
            self.deferred_cursor_closes.clear();
            self.deferred_cursor_parent_by_child.clear();
            self.in_request = false;
            Err(OracleThinError::new("Oracle thin session is broken"))
        } else {
            self.flush_pending_cursor_closes()?;
            self.rollback()?;
            self.end_request()?;
            Ok(())
        }
    }

    pub fn ping(&mut self) -> Result<(), OracleThinError> {
        if self.broken {
            Err(OracleThinError::new("Oracle thin session is broken"))
        } else {
            self.simple_ttc_call(TNS_FUNC_PING, "ping")
        }
    }

    pub fn status(&mut self) -> Result<(), OracleThinError> {
        self.ping()
    }

    pub fn begin_request(&mut self) -> Result<(), OracleThinError> {
        if !self.capabilities.supports_request_boundaries || self.in_request {
            return Ok(());
        }
        if self.broken {
            return Err(OracleThinError::new("Oracle thin session is broken"));
        }
        self.send_session_state(TNS_SESSION_STATE_REQUEST_BEGIN)?;
        self.in_request = true;
        Ok(())
    }

    pub fn end_request(&mut self) -> Result<(), OracleThinError> {
        if !self.capabilities.supports_request_boundaries || !self.in_request {
            return Ok(());
        }
        if self.broken {
            return Err(OracleThinError::new("Oracle thin session is broken"));
        }
        self.send_session_state(TNS_SESSION_STATE_REQUEST_END)?;
        self.in_request = false;
        Ok(())
    }

    pub fn commit(&mut self) -> Result<(), OracleThinError> {
        self.simple_ttc_call(TNS_FUNC_COMMIT, "commit")
    }

    pub fn rollback(&mut self) -> Result<(), OracleThinError> {
        self.simple_ttc_call(TNS_FUNC_ROLLBACK, "rollback")
    }

    pub fn change_password(
        &mut self,
        old_password: impl AsRef<str>,
        new_password: impl AsRef<str>,
    ) -> Result<(), OracleThinError> {
        let combo_key = self.combo_key.clone().ok_or_else(|| {
            OracleThinError::new("Oracle thin session does not have an authentication combo key")
        })?;
        let mut salt = [0u8; 16];
        OsRng.fill_bytes(&mut salt);
        let sequence = self.next_ttc_sequence();
        let payload = auth_change_password_payload(
            &self.config.username,
            old_password.as_ref(),
            new_password.as_ref(),
            &combo_key,
            &salt,
            &self.capabilities,
            sequence,
        )?;
        write_data_packet(
            &mut self.stream,
            self.capabilities.protocol_version.unwrap_or(319),
            self.capabilities.data_packet_chunk_size(),
            &payload,
        )?;
        let mut state = AuthState::default();
        process_auth_response(&mut self.stream, &self.capabilities, &mut state)
    }

    pub fn close(&mut self) -> Result<(), OracleThinError> {
        if self.closed {
            return Ok(());
        }
        if self.broken {
            self.closed = true;
            let _ = self.stream.shutdown(Shutdown::Both);
            return Ok(());
        }

        let result = self
            .simple_ttc_call(TNS_FUNC_LOGOFF, "logoff")
            .and_then(|_| {
                write_eof_data_packet(
                    &mut self.stream,
                    self.capabilities.protocol_version.unwrap_or(319),
                )
            });
        self.closed = true;
        let _ = self.stream.shutdown(Shutdown::Both);
        result
    }

    pub fn transaction_in_progress(&self) -> bool {
        self.server_state.transaction_in_progress
    }

    pub fn last_warning(&self) -> Option<&OracleThinWarning> {
        self.server_state.last_warning.as_ref()
    }

    pub fn query_drop(&mut self, sql: &str) -> Result<(), OracleThinError> {
        let result = self.execute_typed(&StatementRequest::statement(sql), &[])?;
        self.close_cursor_later(result.cursor_id);
        Ok(())
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
        column_types: &[OracleColumnType],
    ) -> Result<QueryResult, OracleThinError> {
        let result = self.execute_typed_response(request, column_types)?.result;
        self.remember_last_row_for_open_fetch(&result);
        Ok(result)
    }

    pub fn execute_typed_with_implicit(
        &mut self,
        request: &StatementRequest,
        column_types: &[OracleColumnType],
    ) -> Result<ExecuteWithImplicitResult, OracleThinError> {
        let response = self.execute_typed_response(request, column_types)?;
        let result = response.result;
        self.remember_last_row_for_open_fetch(&result);
        Ok(ExecuteWithImplicitResult {
            result,
            implicit_results: response.implicit_results,
        })
    }

    fn execute_typed_response(
        &mut self,
        request: &StatementRequest,
        column_types: &[OracleColumnType],
    ) -> Result<ExecuteResponse, OracleThinError> {
        if !column_types_require_define_fetch_for_values(column_types) {
            return self.execute_request(request);
        }

        let mut request_without_prefetch = request.clone();
        request_without_prefetch.prefetch_rows = 0;
        let mut response = self.execute_request_without_prefetch(&request_without_prefetch)?;
        let Some(cursor_id) = response.result.cursor_id else {
            return Ok(response);
        };
        if response.result.exhausted {
            return Ok(response);
        }

        let batch =
            match self.define_and_fetch_typed(cursor_id, request.fetch_array_size, column_types) {
                Ok(batch) => batch,
                Err(error) => {
                    self.close_cursor_after_partial_rows(
                        cursor_id,
                        &response.result.rows,
                        column_types_may_contain_ref_cursors(column_types),
                    );
                    return Err(error);
                }
            };
        let no_rows = batch.rows.is_empty();
        response.result.rows.extend(batch.rows);
        response.result.exhausted = batch.exhausted || batch.cursor_id.is_none() || no_rows;
        response.result.cursor_id = batch.cursor_id;
        Ok(response)
    }

    pub fn execute_typed_fetch_all(
        &mut self,
        request: &StatementRequest,
        column_types: &[OracleColumnType],
    ) -> Result<QueryResult, OracleThinError> {
        let requires_define_fetch = column_types_require_define_fetch_for_values(column_types);
        let mut result = if requires_define_fetch {
            let mut request = request.clone();
            request.prefetch_rows = 0;
            self.execute_request_without_prefetch(&request)?.result
        } else {
            self.execute_typed(request, column_types)?
        };
        let Some(cursor_id) = result.cursor_id else {
            return Ok(result);
        };
        let mut needs_define_fetch = requires_define_fetch;
        let rows_may_contain_ref_cursors = column_types_may_contain_ref_cursors(column_types);
        while !result.exhausted {
            let batch = if needs_define_fetch {
                needs_define_fetch = false;
                match self.define_and_fetch_typed(cursor_id, request.fetch_array_size, column_types)
                {
                    Ok(batch) => batch,
                    Err(error) => {
                        self.close_cursor_after_partial_rows(
                            cursor_id,
                            &result.rows,
                            rows_may_contain_ref_cursors,
                        );
                        return Err(error);
                    }
                }
            } else {
                match self.fetch_typed(cursor_id, request.fetch_array_size, column_types) {
                    Ok(batch) => batch,
                    Err(error) => {
                        self.close_cursor_after_partial_rows(
                            cursor_id,
                            &result.rows,
                            rows_may_contain_ref_cursors,
                        );
                        return Err(error);
                    }
                }
            };
            let no_rows = batch.rows.is_empty();
            result.rows.extend(batch.rows);
            result.exhausted = batch.exhausted || batch.cursor_id.is_none() || no_rows;
        }
        self.close_fully_fetched_cursor(cursor_id, &result.rows, rows_may_contain_ref_cursors)?;
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
        bind_types: &[OracleColumnType],
    ) -> Result<OutBindResult, OracleThinError> {
        let typed_request;
        let request = if request.binds.is_empty() && !bind_types.is_empty() {
            typed_request = request_with_out_bind_types(request, bind_types);
            &typed_request
        } else {
            request
        };
        let response = self.execute_request(request)?;
        let rows = if response.out_bind_rows.is_empty() {
            response
                .result
                .rows
                .first()
                .cloned()
                .map(|row| vec![row])
                .unwrap_or_default()
        } else {
            response.out_bind_rows
        };
        let values = rows.first().cloned().unwrap_or_default();
        Ok(OutBindResult {
            values,
            rows,
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
        let response = self.execute_request(&describe_request)?;
        self.close_cursor_later(response.result.cursor_id);
        Ok(response.columns)
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
        let described_columns = self.describe_request(request)?;
        let requires_define_fetch = columns_require_define_fetch_for_values(&described_columns);
        let requires_deferred_fetch =
            requires_define_fetch || columns_require_object_metadata_for_values(&described_columns);
        let mut result = if requires_define_fetch {
            self.query_described_without_prefetch_request(request)?
        } else if requires_deferred_fetch {
            let mut result = self.query_described_without_prefetch_request(request)?;
            result.columns = described_columns;
            result
        } else {
            self.query_described_fetch_all_request_legacy(request)?
        };
        self.remember_last_row_for_open_fetch(&result);
        let Some(cursor_id) = result.result.cursor_id else {
            return Ok(result);
        };
        let mut needs_define_fetch = requires_define_fetch;
        let rows_may_contain_ref_cursors = columns_may_contain_ref_cursors(&result.columns);
        while !result.result.exhausted {
            let batch = match self.fetch_ref_cursor_batch(
                cursor_id,
                &result.columns,
                request.fetch_array_size,
                needs_define_fetch,
            ) {
                Ok(batch) => batch,
                Err(error) => {
                    self.close_cursor_after_partial_rows(
                        cursor_id,
                        &result.result.rows,
                        rows_may_contain_ref_cursors,
                    );
                    return Err(error);
                }
            };
            needs_define_fetch = false;
            let no_rows = batch.rows.is_empty();
            result.result.rows.extend(batch.rows);
            result.result.exhausted = batch.exhausted || batch.cursor_id.is_none() || no_rows;
        }
        self.close_fully_fetched_cursor(
            cursor_id,
            &result.result.rows,
            rows_may_contain_ref_cursors,
        )?;
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

    fn query_described_without_prefetch_request(
        &mut self,
        request: &StatementRequest,
    ) -> Result<DescribedQueryResult, OracleThinError> {
        let mut request = request.clone();
        request.prefetch_rows = 0;
        let response = self.execute_request_without_prefetch(&request)?;
        Ok(DescribedQueryResult {
            columns: response.columns,
            result: response.result,
        })
    }

    pub fn query_described_initial_request(
        &mut self,
        request: &StatementRequest,
    ) -> Result<DescribedQueryResult, OracleThinError> {
        let described_columns = self.describe_request(request)?;
        let result = if columns_require_object_metadata_for_values(&described_columns) {
            let mut result = self.query_described_initial_without_prefetch_request(request)?;
            result.columns = described_columns;
            result
        } else {
            self.query_described_initial_request_legacy(request)?
        };
        self.remember_last_row_for_open_fetch(&result);
        Ok(result)
    }

    pub fn query_described_initial_without_prefetch_request(
        &mut self,
        request: &StatementRequest,
    ) -> Result<DescribedQueryResult, OracleThinError> {
        let mut request = request.clone();
        request.prefetch_rows = 0;
        let response = self.execute_request_without_prefetch(&request)?;
        Ok(DescribedQueryResult {
            columns: response.columns,
            result: response.result,
        })
    }

    fn query_described_initial_request_legacy(
        &mut self,
        request: &StatementRequest,
    ) -> Result<DescribedQueryResult, OracleThinError> {
        let response = self.execute_request(request)?;
        Ok(DescribedQueryResult {
            columns: response.columns,
            result: response.result,
        })
    }

    pub fn fetch_ref_cursor_all(
        &mut self,
        cursor_id: u32,
        columns: Vec<ColumnMetadata>,
        fetch_array_size: u32,
    ) -> Result<DescribedQueryResult, OracleThinError> {
        let mut rows = Vec::new();
        let mut needs_define_fetch = columns_require_define_fetch_for_values(&columns);
        let rows_may_contain_ref_cursors = columns_may_contain_ref_cursors(&columns);
        loop {
            let result = match self.fetch_ref_cursor_batch(
                cursor_id,
                &columns,
                fetch_array_size,
                needs_define_fetch,
            ) {
                Ok(result) => result,
                Err(error) => {
                    self.close_cursor_after_partial_rows(
                        cursor_id,
                        &rows,
                        rows_may_contain_ref_cursors,
                    );
                    return Err(error);
                }
            };
            needs_define_fetch = false;
            let no_rows = result.rows.is_empty();
            rows.extend(result.rows);
            if no_rows || result.exhausted || result.cursor_id.is_none() {
                break;
            }
        }
        self.close_fully_fetched_cursor(cursor_id, &rows, rows_may_contain_ref_cursors)?;
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
        let use_ref_cursor_execute_fetch = self.ref_cursor_ids.contains(&cursor_id);
        let effective_needs_define_fetch = needs_define_fetch;
        let fetch_columns =
            self.fetch_columns_for_cursor(cursor_id, columns, effective_needs_define_fetch);
        self.ensure_object_type_attrs_for_columns(&fetch_columns)?;
        let pending_cursor_closes = self.drain_pending_cursor_closes();
        let current_schema = self.pending_current_schema_for_write();
        let current_schema_sequence = if current_schema.is_some() {
            Some(self.next_ttc_sequence())
        } else {
            None
        };
        let close_sequence = if pending_cursor_closes.is_empty() {
            None
        } else {
            Some(self.next_ttc_sequence())
        };
        let end_to_end = self.pending_end_to_end_for_write();
        let end_to_end_sequence = if end_to_end.is_some() {
            Some(self.next_ttc_sequence())
        } else {
            None
        };
        let sequence = self.next_ttc_sequence();
        let write_result = if effective_needs_define_fetch {
            log_connect_phase("ttc-define-fetch-write", "");
            write_define_fetch_request(
                &mut self.stream,
                &self.capabilities,
                cursor_id,
                row_count,
                sequence,
                current_schema_sequence,
                current_schema.as_deref(),
                close_sequence,
                &pending_cursor_closes,
                end_to_end_sequence,
                end_to_end.as_ref(),
                &fetch_columns,
            )
        } else if use_ref_cursor_execute_fetch {
            log_connect_phase("ttc-ref-cursor-execute-fetch-write", "");
            write_ref_cursor_execute_fetch_request(
                &mut self.stream,
                &self.capabilities,
                cursor_id,
                row_count,
                sequence,
                current_schema_sequence,
                current_schema.as_deref(),
                close_sequence,
                &pending_cursor_closes,
                end_to_end_sequence,
                end_to_end.as_ref(),
            )
        } else {
            log_connect_phase("ttc-fetch-write", "");
            write_fetch_request(
                &mut self.stream,
                &self.capabilities,
                cursor_id,
                row_count,
                sequence,
                current_schema_sequence,
                current_schema.as_deref(),
                close_sequence,
                &pending_cursor_closes,
                end_to_end_sequence,
                end_to_end.as_ref(),
            )
        };
        if let Err(error) = write_result {
            self.requeue_pending_cursor_closes(&pending_cursor_closes);
            self.close_cursor_later(Some(cursor_id));
            return Err(error);
        }
        self.clear_pending_current_schema_if_written(current_schema.as_deref());
        self.clear_pending_end_to_end_if_written(end_to_end.as_ref());
        log_connect_phase("ttc-fetch-read", "");
        let mut state = ExecuteReadState::default();
        state.columns = fetch_columns;
        state.object_attrs_by_type = self.object_attrs_by_type.clone();
        state.collection_element_by_type = self.collection_element_by_type.clone();
        state.last_row = self.last_rows_by_cursor.get(&cursor_id).cloned();
        let request = StatementRequest::query("", row_count);
        let response = read_execute_response_with_state(
            &mut self.stream,
            &self.capabilities,
            &request,
            &mut self.server_state,
            state,
            close_sequence.is_some(),
        );
        if self.cancel_flag.swap(false, Ordering::SeqCst) {
            return Err(self.finish_cancelled_read());
        }
        let result = match response {
            Ok(mut response) => {
                self.remember_cursor_columns_from_response(&response);
                self.resolve_xml_lob_values(&response.thin_columns, &mut response.result.rows)?;
                response.result
            }
            Err(error) => {
                let error_cursor_id = error.cursor_id();
                self.close_cursor_later(error_cursor_id);
                if error_cursor_id != Some(cursor_id) {
                    self.close_cursor_later(Some(cursor_id));
                }
                return Err(error);
            }
        };
        if result.exhausted || result.cursor_id.is_none() {
            self.last_rows_by_cursor.remove(&cursor_id);
        } else {
            self.remember_last_row_for_open_fetch(&result);
        }
        Ok(result)
    }

    fn fetch_columns_for_cursor(
        &self,
        cursor_id: u32,
        columns: &[ColumnMetadata],
        needs_define_fetch: bool,
    ) -> Vec<ThinColumn> {
        if let Some(cached_columns) = self.cursor_columns_by_cursor.get(&cursor_id) {
            if needs_define_fetch {
                return cached_columns
                    .iter()
                    .map(|column| {
                        define_thin_column_metadata_for_capabilities(column, &self.capabilities)
                    })
                    .collect();
            }
            return cached_columns.clone();
        }

        columns
            .iter()
            .map(|column| {
                if needs_define_fetch {
                    define_column_metadata_for_capabilities(column, &self.capabilities)
                } else {
                    fetch_state_column_metadata(column, &self.capabilities)
                }
            })
            .collect()
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
                charset_form: bind_column_metadata(&BindValue::Null(*column_type)).charset_form,
                ora_type_num: 0,
                buffer_size: 0,
                schema_name: String::new(),
                type_name: String::new(),
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
                charset_form: bind_column_metadata(&BindValue::Null(*column_type)).charset_form,
                ora_type_num: 0,
                buffer_size: 0,
                schema_name: String::new(),
                type_name: String::new(),
            })
            .collect::<Vec<_>>();
        self.fetch_ref_cursor_batch(cursor_id, &columns, row_count, false)
    }

    pub fn close_cursor_later(&mut self, cursor_id: Option<u32>) {
        if let Some(cursor_id) = cursor_id {
            self.last_rows_by_cursor.remove(&cursor_id);
            self.cursor_columns_by_cursor.remove(&cursor_id);
            self.ref_cursor_ids.remove(&cursor_id);
            self.pending_cursor_closes.push(cursor_id);
            self.queue_deferred_parent_after_child_close(cursor_id);
        }
    }

    pub fn close_cursor_on_next_call(&mut self, cursor_id: Option<u32>) {
        self.close_cursor_later(cursor_id);
    }

    fn close_fully_fetched_cursor(
        &mut self,
        cursor_id: u32,
        rows: &[Vec<OracleValue>],
        rows_may_contain_ref_cursors: bool,
    ) -> Result<(), OracleThinError> {
        self.last_rows_by_cursor.remove(&cursor_id);
        self.cursor_columns_by_cursor.remove(&cursor_id);
        self.ref_cursor_ids.remove(&cursor_id);
        if rows_may_contain_ref_cursors {
            let child_cursor_ids = ref_cursor_ids_in_rows(rows);
            if !child_cursor_ids.is_empty() {
                for child_cursor_id in &child_cursor_ids {
                    self.deferred_cursor_parent_by_child
                        .insert(*child_cursor_id, cursor_id);
                }
                self.deferred_cursor_closes
                    .insert(cursor_id, child_cursor_ids);
                return Ok(());
            }
        }
        self.close_cursor_later(Some(cursor_id));
        self.flush_pending_cursor_closes()
    }

    fn queue_deferred_parent_after_child_close(&mut self, child_cursor_id: u32) {
        let Some(parent_cursor_id) = self
            .deferred_cursor_parent_by_child
            .remove(&child_cursor_id)
        else {
            return;
        };
        let parent_ready = match self.deferred_cursor_closes.get_mut(&parent_cursor_id) {
            Some(children) => {
                children.remove(&child_cursor_id);
                children.is_empty()
            }
            None => false,
        };
        if parent_ready {
            self.deferred_cursor_closes.remove(&parent_cursor_id);
            self.close_cursor_later(Some(parent_cursor_id));
        }
    }

    pub fn flush_pending_cursor_closes(&mut self) -> Result<(), OracleThinError> {
        if self.pending_cursor_closes.is_empty() {
            return Ok(());
        }
        let cursor_ids = self.drain_pending_cursor_closes();
        let close_sequence = self.next_ttc_sequence();
        let ping_sequence = self.next_ttc_sequence();
        let mut payload = Vec::new();
        if let Err(error) = write_close_cursors_piggyback(
            &mut payload,
            &self.capabilities,
            close_sequence,
            &cursor_ids,
        ) {
            self.requeue_pending_cursor_closes(&cursor_ids);
            return Err(error);
        }
        write_function_code(
            &mut payload,
            TNS_FUNC_PING,
            ping_sequence,
            &self.capabilities,
        );
        if let Err(error) = write_data_packet(
            &mut self.stream,
            self.capabilities.protocol_version.unwrap_or(319),
            self.capabilities.data_packet_chunk_size(),
            &payload,
        ) {
            self.requeue_pending_cursor_closes(&cursor_ids);
            return Err(error);
        }
        read_simple_response(
            &mut self.stream,
            &self.capabilities,
            &mut self.server_state,
            true,
        )
    }

    pub fn described_columns_require_define_fetch(columns: &[ColumnMetadata]) -> bool {
        columns_require_define_fetch_for_values(columns)
    }

    fn remember_last_row_for_open_fetch<T: LastRowSource>(&mut self, result: &T) {
        let result = result.query_result();
        let Some(cursor_id) = result.cursor_id else {
            return;
        };
        if result.exhausted {
            self.last_rows_by_cursor.remove(&cursor_id);
            return;
        }
        if let Some(row) = result.rows.last() {
            self.last_rows_by_cursor.insert(cursor_id, row.clone());
        }
    }

    fn close_cursor_after_partial_rows(
        &mut self,
        cursor_id: u32,
        rows: &[Vec<OracleValue>],
        rows_may_contain_ref_cursors: bool,
    ) {
        if rows_may_contain_ref_cursors {
            for child_cursor_id in ref_cursor_ids_in_rows(rows) {
                self.close_cursor_later(Some(child_cursor_id));
            }
        }
        self.close_cursor_later(Some(cursor_id));
    }

    fn resolve_xml_lob_values_in_response(
        &mut self,
        response: &mut ExecuteResponse,
    ) -> Result<(), OracleThinError> {
        self.resolve_xml_lob_values(&response.thin_columns, &mut response.result.rows)
    }

    fn resolve_lob_out_bind_values(
        &mut self,
        request: &StatementRequest,
        response: &mut ExecuteResponse,
    ) -> Result<(), OracleThinError> {
        if response.out_bind_rows.is_empty() {
            return Ok(());
        }
        let columns = request
            .binds
            .iter()
            .filter(|bind| bind_can_return_value(bind))
            .map(bind_column_metadata)
            .collect::<Vec<_>>();
        for row in &mut response.out_bind_rows {
            for (column, value) in columns.iter().zip(row.iter_mut()) {
                let OracleValue::Lob(locator) = value else {
                    continue;
                };
                match column.column_type {
                    OracleColumnType::Blob => {
                        *value =
                            OracleValue::Bytes(self.read_blob_locator_as_bytes(&locator.clone())?);
                    }
                    OracleColumnType::Clob | OracleColumnType::Nclob => {
                        *value =
                            OracleValue::Text(self.read_clob_locator_as_text(&locator.clone())?);
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn resolve_xml_lob_values(
        &mut self,
        columns: &[ThinColumn],
        rows: &mut [Vec<OracleValue>],
    ) -> Result<(), OracleThinError> {
        for row in rows {
            for (column, value) in columns.iter().zip(row.iter_mut()) {
                self.resolve_xml_lob_value_for_column(column, value)?;
            }
        }
        Ok(())
    }

    fn resolve_xml_lob_value_for_column(
        &mut self,
        column: &ThinColumn,
        value: &mut OracleValue,
    ) -> Result<(), OracleThinError> {
        match value {
            OracleValue::Lob(locator) if column.column_type == OracleColumnType::Xml => {
                *value = OracleValue::Text(self.read_clob_locator_as_text(&locator.clone())?);
            }
            OracleValue::Lob(locator) if column.column_type == OracleColumnType::Vector => {
                *value = OracleValue::Text(self.read_vector_locator_as_text(&locator.clone())?);
            }
            OracleValue::Object(attrs) => {
                let key = object_type_key(&column.schema_name, &column.type_name);
                let Some(attr_columns) = self.object_attrs_by_type.get(&key).cloned() else {
                    return Ok(());
                };
                for (attr_column, (_, attr_value)) in attr_columns.iter().zip(attrs.iter_mut()) {
                    self.resolve_xml_lob_value_for_column(attr_column, attr_value)?;
                }
            }
            OracleValue::Array(values) => {
                let key = object_type_key(&column.schema_name, &column.type_name);
                let Some(element_column) = self.collection_element_by_type.get(&key).cloned()
                else {
                    return Ok(());
                };
                for element_value in values {
                    self.resolve_xml_lob_value_for_column(&element_column, element_value)?;
                }
            }
            OracleValue::IndexedArray(values) => {
                let key = object_type_key(&column.schema_name, &column.type_name);
                let Some(element_column) = self.collection_element_by_type.get(&key).cloned()
                else {
                    return Ok(());
                };
                for (_, element_value) in values {
                    self.resolve_xml_lob_value_for_column(&element_column, element_value)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn read_clob_locator_as_text(&mut self, locator: &[u8]) -> Result<String, OracleThinError> {
        let mut offset = 1;
        let mut bytes = Vec::new();
        loop {
            let response =
                self.read_lob_locator(locator, offset, u64::from(TNS_MAX_LONG_LENGTH))?;
            if response.data.is_empty() {
                break;
            }
            if std::env::var_os("ORACLE_THIN_TRACE_EXEC").is_some() {
                eprintln!(
                    "thin lob read chunk offset={} bytes={} amount={:?}",
                    offset,
                    response.data.len(),
                    response.amount
                );
            }
            bytes.extend_from_slice(&response.data);
            let Some(amount) = response.amount.filter(|amount| *amount > 0) else {
                break;
            };
            offset += amount as u64;
        }
        decode_xml_clob_lob_text(&bytes, &self.capabilities)
    }

    fn read_blob_locator_as_bytes(&mut self, locator: &[u8]) -> Result<Vec<u8>, OracleThinError> {
        let mut offset = 1;
        let mut bytes = Vec::new();
        loop {
            let response =
                self.read_lob_locator(locator, offset, u64::from(TNS_MAX_LONG_LENGTH))?;
            if response.data.is_empty() {
                break;
            }
            bytes.extend_from_slice(&response.data);
            let Some(amount) = response.amount.filter(|amount| *amount > 0) else {
                break;
            };
            offset += amount as u64;
        }
        Ok(bytes)
    }

    fn read_vector_locator_as_text(&mut self, locator: &[u8]) -> Result<String, OracleThinError> {
        let mut offset = 1;
        let mut bytes = Vec::new();
        loop {
            let response =
                self.read_lob_locator(locator, offset, u64::from(TNS_MAX_LONG_LENGTH))?;
            if response.data.is_empty() {
                break;
            }
            bytes.extend_from_slice(&response.data);
            let Some(amount) = response.amount.filter(|amount| *amount > 0) else {
                break;
            };
            offset += amount as u64;
        }
        decode_oracle_vector(&bytes)
    }

    fn create_temp_blob(&mut self, value: &[u8]) -> Result<Vec<u8>, OracleThinError> {
        self.create_temp_lob(OracleColumnType::Blob, value.to_vec())
    }

    fn create_temp_clob(&mut self, value: &str) -> Result<Vec<u8>, OracleThinError> {
        let locator = self.create_temp_lob_locator(OracleColumnType::Clob)?;
        if value.is_empty() {
            return Ok(locator);
        }
        let bytes = match encode_temp_clob_text(value, &locator, &self.capabilities) {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = self.free_temp_lobs(&[locator]);
                return Err(error);
            }
        };
        self.write_temp_lob_or_free(locator, &bytes)
    }

    fn create_temp_nclob(&mut self, value: &str) -> Result<Vec<u8>, OracleThinError> {
        let locator = self.create_temp_lob_locator(OracleColumnType::Nclob)?;
        if value.is_empty() {
            return Ok(locator);
        }
        let bytes = match encode_oracle_nchar_text(
            value,
            self.capabilities.ncharset_id,
            self.capabilities.protocol_version,
        ) {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = self.free_temp_lobs(&[locator]);
                return Err(error);
            }
        };
        self.write_temp_lob_or_free(locator, &bytes)
    }

    fn create_temp_lob(
        &mut self,
        column_type: OracleColumnType,
        bytes: Vec<u8>,
    ) -> Result<Vec<u8>, OracleThinError> {
        let locator = self.create_temp_lob_locator(column_type)?;
        if bytes.is_empty() {
            return Ok(locator);
        }
        self.write_temp_lob_or_free(locator, &bytes)
    }

    fn create_temp_lob_locator(
        &mut self,
        column_type: OracleColumnType,
    ) -> Result<Vec<u8>, OracleThinError> {
        let mut locator = vec![0; 40];
        let sequence = self.next_ttc_sequence();
        let mut payload = Vec::new();
        let (charset_form, ora_type_num, charset_id) =
            temp_lob_type_metadata(column_type, &self.capabilities);
        write_lob_operation_request(
            &mut payload,
            &self.capabilities,
            sequence,
            &locator,
            TNS_LOB_OP_CREATE_TEMP,
            u64::from(charset_form),
            u64::from(ora_type_num),
            TNS_DURATION_SESSION,
            None,
            Some(charset_id.unwrap_or(TNS_CHARSET_UTF8)),
        )?;
        write_data_packet(
            &mut self.stream,
            self.capabilities.protocol_version.unwrap_or(319),
            self.capabilities.data_packet_chunk_size(),
            &payload,
        )?;
        locator = read_lob_operation_response(
            &mut self.stream,
            &self.capabilities,
            &mut self.server_state,
            locator.len(),
            false,
            true,
        )?
        .locator
        .ok_or_else(|| {
            OracleThinError::new("Oracle temporary LOB create did not return locator")
        })?;

        Ok(locator)
    }

    fn write_temp_lob(
        &mut self,
        mut locator: Vec<u8>,
        bytes: &[u8],
    ) -> Result<Vec<u8>, OracleThinError> {
        let sequence = self.next_ttc_sequence();
        let mut payload = Vec::new();
        write_lob_operation_request(
            &mut payload,
            &self.capabilities,
            sequence,
            &locator,
            TNS_LOB_OP_WRITE,
            1,
            0,
            0,
            Some(bytes),
            None,
        )?;
        write_data_packet(
            &mut self.stream,
            self.capabilities.protocol_version.unwrap_or(319),
            self.capabilities.data_packet_chunk_size(),
            &payload,
        )?;
        if let Some(updated_locator) = read_lob_operation_response(
            &mut self.stream,
            &self.capabilities,
            &mut self.server_state,
            locator.len(),
            false,
            false,
        )?
        .locator
        {
            locator = updated_locator;
        }
        Ok(locator)
    }

    fn write_temp_lob_or_free(
        &mut self,
        locator: Vec<u8>,
        bytes: &[u8],
    ) -> Result<Vec<u8>, OracleThinError> {
        match self.write_temp_lob(locator.clone(), bytes) {
            Ok(locator) => Ok(locator),
            Err(error) => {
                let _ = self.free_temp_lobs(&[locator]);
                Err(error)
            }
        }
    }

    fn read_lob_locator(
        &mut self,
        locator: &[u8],
        source_offset: u64,
        amount: u64,
    ) -> Result<LobReadResponse, OracleThinError> {
        let sequence = self.next_ttc_sequence();
        let mut payload = Vec::new();
        write_lob_read_request(
            &mut payload,
            &self.capabilities,
            sequence,
            locator,
            source_offset,
            amount,
        );
        if std::env::var_os("ORACLE_THIN_TRACE_EXEC").is_some() {
            eprintln!(
                "thin lob read request locator_len={} offset={} amount={} payload={}",
                locator.len(),
                source_offset,
                amount,
                hex_encode_upper(&payload)
            );
        }
        write_data_packet(
            &mut self.stream,
            self.capabilities.protocol_version.unwrap_or(319),
            self.capabilities.data_packet_chunk_size(),
            &payload,
        )?;
        read_lob_operation_response(
            &mut self.stream,
            &self.capabilities,
            &mut self.server_state,
            locator.len(),
            true,
            false,
        )
    }

    fn remember_cursor_columns_from_response(&mut self, response: &ExecuteResponse) {
        if let Some(cursor_id) = response.result.cursor_id {
            if !response.thin_columns.is_empty() {
                self.cursor_columns_by_cursor
                    .insert(cursor_id, response.thin_columns.clone());
            }
        }
        for (cursor_id, columns) in &response.cursor_columns {
            if *cursor_id != 0 && !columns.is_empty() {
                self.cursor_columns_by_cursor
                    .insert(*cursor_id, columns.clone());
                self.ref_cursor_ids.insert(*cursor_id);
            }
        }
    }

    fn ensure_object_type_attrs_for_columns(
        &mut self,
        columns: &[ThinColumn],
    ) -> Result<(), OracleThinError> {
        for column in columns {
            self.ensure_object_or_collection_metadata_for_column(column)?;
        }
        Ok(())
    }

    fn ensure_object_or_collection_metadata_for_column(
        &mut self,
        column: &ThinColumn,
    ) -> Result<(), OracleThinError> {
        if !is_decodable_object_column(column) {
            return Ok(());
        }
        let key = object_type_key(&column.schema_name, &column.type_name);
        if self.object_attrs_by_type.contains_key(&key)
            || self.collection_element_by_type.contains_key(&key)
        {
            return Ok(());
        }
        match self.load_named_typecode(&key.0, &key.1)?.as_deref() {
            Some("OBJECT") => {
                let attrs = self.load_object_type_attrs(&key.0, &key.1)?;
                self.object_attrs_by_type.insert(key, attrs);
                Ok(())
            }
            Some("COLLECTION") => {
                let element = self.load_collection_element_type(&key.0, &key.1)?;
                self.collection_element_by_type.insert(key, element);
                Ok(())
            }
            Some(typecode) => Err(OracleThinError::new(format!(
                "Oracle thin TTC cannot decode named type {}.{} with TYPECODE {typecode}",
                key.0, key.1
            ))),
            None => Err(OracleThinError::new(format!(
                "Oracle thin TTC cannot verify named type {}.{}",
                key.0, key.1
            ))),
        }
    }

    fn load_named_typecode(
        &mut self,
        schema_name: &str,
        type_name: &str,
    ) -> Result<Option<String>, OracleThinError> {
        let sql = format!(
            "SELECT typecode \
             FROM all_types \
             WHERE owner = '{}' AND type_name = '{}'",
            sql_string_literal(schema_name),
            sql_string_literal(type_name)
        );
        let result = self.query_described_fetch_all(sql, 1)?;
        let Some(row) = result.result.rows.first() else {
            return Ok(None);
        };
        match row.first() {
            Some(OracleValue::Text(value)) => Ok(Some(value.to_ascii_uppercase())),
            Some(OracleValue::Null) | None => Ok(None),
            other => Err(OracleThinError::new(format!(
                "unexpected Oracle named typecode metadata value {other:?}"
            ))),
        }
    }

    fn load_collection_element_type(
        &mut self,
        schema_name: &str,
        type_name: &str,
    ) -> Result<ThinColumn, OracleThinError> {
        let sql = format!(
            "SELECT c.elem_type_name, c.elem_type_owner, c.length, \
                    c.character_set_name, t.typecode \
             FROM all_coll_types c \
             LEFT JOIN all_types t \
               ON t.owner = c.elem_type_owner \
              AND t.type_name = c.elem_type_name \
             WHERE c.owner = '{}' AND c.type_name = '{}'",
            sql_string_literal(schema_name),
            sql_string_literal(type_name)
        );
        let result = self.query_described_fetch_all(sql, 1)?;
        let row = result.result.rows.first().ok_or_else(|| {
            OracleThinError::new(format!(
                "Oracle thin TTC cannot load collection element metadata for {schema_name}.{type_name}"
            ))
        })?;
        let elem_type_name = match row.first() {
            Some(OracleValue::Text(value)) => value.clone(),
            other => {
                return Err(OracleThinError::new(format!(
                    "unexpected collection elem_type_name metadata value {other:?}"
                )))
            }
        };
        let elem_type_owner = match row.get(1) {
            Some(OracleValue::Text(value)) => value.clone(),
            Some(OracleValue::Null) | None => String::new(),
            other => {
                return Err(OracleThinError::new(format!(
                    "unexpected collection elem_type_owner metadata value {other:?}"
                )))
            }
        };
        let buffer_size = match row.get(2) {
            Some(OracleValue::Number(value)) => value.parse::<u32>().unwrap_or(0),
            Some(OracleValue::Null) | None => 0,
            other => {
                return Err(OracleThinError::new(format!(
                    "unexpected collection length metadata value {other:?}"
                )))
            }
        };
        let charset_form = match row.get(3) {
            Some(OracleValue::Text(value)) if value.eq_ignore_ascii_case("NCHAR_CS") => {
                CS_FORM_NCHAR
            }
            _ => CS_FORM_IMPLICIT,
        };
        let elem_typecode = match row.get(4) {
            Some(OracleValue::Text(value)) => Some(value.clone()),
            Some(OracleValue::Null) | None => None,
            other => {
                return Err(OracleThinError::new(format!(
                    "unexpected collection element typecode metadata value {other:?}"
                )))
            }
        };
        let element = thin_column_from_object_attr(
            "ELEMENT".to_string(),
            elem_type_name,
            elem_type_owner,
            elem_typecode,
            buffer_size,
            charset_form,
        )?;
        self.ensure_object_or_collection_metadata_for_column(&element)?;
        Ok(element)
    }

    fn load_object_type_attrs(
        &mut self,
        schema_name: &str,
        type_name: &str,
    ) -> Result<Vec<ThinColumn>, OracleThinError> {
        let sql = format!(
            "SELECT a.attr_name, a.attr_type_name, a.attr_type_owner, a.length, \
                    a.character_set_name, t.typecode \
             FROM all_type_attrs a \
             LEFT JOIN all_types t \
               ON t.owner = a.attr_type_owner \
              AND t.type_name = a.attr_type_name \
             WHERE a.owner = '{}' AND a.type_name = '{}' \
             ORDER BY a.attr_no",
            sql_string_literal(schema_name),
            sql_string_literal(type_name)
        );
        let result = self.query_described_fetch_all(sql, 64)?;
        let mut attrs = Vec::with_capacity(result.result.rows.len());
        for row in result.result.rows {
            let attr_name = match row.first() {
                Some(OracleValue::Text(value)) => value.clone(),
                other => {
                    return Err(OracleThinError::new(format!(
                        "unexpected UDT attr_name metadata value {other:?}"
                    )))
                }
            };
            let attr_type_name = match row.get(1) {
                Some(OracleValue::Text(value)) => value.clone(),
                other => {
                    return Err(OracleThinError::new(format!(
                        "unexpected UDT attr_type_name metadata value {other:?}"
                    )))
                }
            };
            let attr_type_owner = match row.get(2) {
                Some(OracleValue::Text(value)) => value.clone(),
                Some(OracleValue::Null) | None => String::new(),
                other => {
                    return Err(OracleThinError::new(format!(
                        "unexpected UDT attr_type_owner metadata value {other:?}"
                    )))
                }
            };
            let buffer_size = match row.get(3) {
                Some(OracleValue::Number(value)) => value.parse::<u32>().unwrap_or(0),
                Some(OracleValue::Null) | None => 0,
                other => {
                    return Err(OracleThinError::new(format!(
                        "unexpected UDT length metadata value {other:?}"
                    )))
                }
            };
            let charset_form = match row.get(4) {
                Some(OracleValue::Text(value)) if value.eq_ignore_ascii_case("NCHAR_CS") => {
                    CS_FORM_NCHAR
                }
                _ => CS_FORM_IMPLICIT,
            };
            let attr_typecode = match row.get(5) {
                Some(OracleValue::Text(value)) => Some(value.clone()),
                Some(OracleValue::Null) | None => None,
                other => {
                    return Err(OracleThinError::new(format!(
                        "unexpected UDT typecode metadata value {other:?}"
                    )))
                }
            };
            attrs.push(thin_column_from_object_attr(
                attr_name,
                attr_type_name,
                attr_type_owner,
                attr_typecode,
                buffer_size,
                charset_form,
            )?);
        }
        for attr in &attrs {
            self.ensure_object_or_collection_metadata_for_column(attr)?;
        }
        Ok(attrs)
    }

    fn execute_request(
        &mut self,
        request: &StatementRequest,
    ) -> Result<ExecuteResponse, OracleThinError> {
        self.execute_request_inner(request, false)
    }

    fn execute_request_without_prefetch(
        &mut self,
        request: &StatementRequest,
    ) -> Result<ExecuteResponse, OracleThinError> {
        self.execute_request_inner(request, true)
    }

    fn execute_request_inner(
        &mut self,
        request: &StatementRequest,
        execute_without_prefetch: bool,
    ) -> Result<ExecuteResponse, OracleThinError> {
        log_connect_phase("ttc-execute-write", &request.sql);
        let materialized_lobs = if request.binds.iter().any(bind_needs_lob_materialization) {
            Some(self.materialize_large_lob_binds(request)?)
        } else {
            None
        };
        let temp_lob_locators = materialized_lobs
            .as_ref()
            .map(|(_, locators)| locators.clone())
            .unwrap_or_default();
        let request = materialized_lobs
            .as_ref()
            .map(|(request, _)| request)
            .unwrap_or(request);
        let pending_cursor_closes = self.drain_pending_cursor_closes();
        let current_schema = self.pending_current_schema_for_write();
        let current_schema_sequence = if current_schema.is_some() {
            Some(self.next_ttc_sequence())
        } else {
            None
        };
        let close_sequence = if pending_cursor_closes.is_empty() {
            None
        } else {
            Some(self.next_ttc_sequence())
        };
        let end_to_end = self.pending_end_to_end_for_write();
        let end_to_end_sequence = if end_to_end.is_some() {
            Some(self.next_ttc_sequence())
        } else {
            None
        };
        let sequence = self.next_ttc_sequence();
        if let Err(error) = write_execute_request(
            &mut self.stream,
            &self.capabilities,
            request,
            sequence,
            0,
            current_schema_sequence,
            current_schema.as_deref(),
            close_sequence,
            &pending_cursor_closes,
            end_to_end_sequence,
            end_to_end.as_ref(),
            execute_without_prefetch,
        ) {
            self.requeue_pending_cursor_closes(&pending_cursor_closes);
            let _ = self.free_temp_lobs(&temp_lob_locators);
            return Err(error);
        }
        self.clear_pending_current_schema_if_written(current_schema.as_deref());
        self.clear_pending_end_to_end_if_written(end_to_end.as_ref());
        log_connect_phase("ttc-execute-read", "");
        let skip_empty_end_of_response = close_sequence.is_some()
            || (request.is_query
                && request.prefetch_rows == 0
                && request.sql.contains("SQ_INTERNAL_ROWID"));
        let response = read_execute_response(
            &mut self.stream,
            &self.capabilities,
            request,
            &mut self.server_state,
            skip_empty_end_of_response,
        );
        if self.cancel_flag.swap(false, Ordering::SeqCst) {
            let error = self.finish_cancelled_read();
            let _ = self.free_temp_lobs(&temp_lob_locators);
            return Err(error);
        }
        match response {
            Ok(mut response) => {
                let result = match self.resolve_xml_lob_values_in_response(&mut response) {
                    Ok(()) => match self.resolve_lob_out_bind_values(request, &mut response) {
                        Ok(()) => {
                            self.remember_cursor_columns_from_response(&response);
                            Ok(response)
                        }
                        Err(error) => Err(error),
                    },
                    Err(error) => Err(error),
                };
                let free_result = self.free_temp_lobs(&temp_lob_locators);
                match (result, free_result) {
                    (Ok(response), Ok(())) => Ok(response),
                    (Err(error), _) => Err(error),
                    (Ok(_), Err(error)) => Err(error),
                }
            }
            Err(error) => {
                self.close_cursor_later(error.cursor_id());
                let _ = self.free_temp_lobs(&temp_lob_locators);
                Err(error)
            }
        }
    }

    fn materialize_large_lob_binds(
        &mut self,
        request: &StatementRequest,
    ) -> Result<(StatementRequest, Vec<Vec<u8>>), OracleThinError> {
        let mut materialized = request.clone();
        let mut temp_lob_locators = Vec::new();
        for bind in &mut materialized.binds {
            match bind {
                BindValue::Blob(bytes) => {
                    let locator = match self.create_temp_blob(bytes) {
                        Ok(locator) => locator,
                        Err(error) => {
                            let _ = self.free_temp_lobs(&temp_lob_locators);
                            return Err(error);
                        }
                    };
                    temp_lob_locators.push(locator.clone());
                    *bind = BindValue::LobLocator {
                        column_type: OracleColumnType::Blob,
                        locator,
                    };
                }
                BindValue::Clob(text) => {
                    let locator = match self.create_temp_clob(text) {
                        Ok(locator) => locator,
                        Err(error) => {
                            let _ = self.free_temp_lobs(&temp_lob_locators);
                            return Err(error);
                        }
                    };
                    temp_lob_locators.push(locator.clone());
                    *bind = BindValue::LobLocator {
                        column_type: OracleColumnType::Clob,
                        locator,
                    };
                }
                BindValue::Nclob(text) => {
                    let locator = match self.create_temp_nclob(text) {
                        Ok(locator) => locator,
                        Err(error) => {
                            let _ = self.free_temp_lobs(&temp_lob_locators);
                            return Err(error);
                        }
                    };
                    temp_lob_locators.push(locator.clone());
                    *bind = BindValue::LobLocator {
                        column_type: OracleColumnType::Nclob,
                        locator,
                    };
                }
                BindValue::InOut {
                    column_type: OracleColumnType::Blob,
                    max_len,
                    value: Some(BindInputValue::Bytes(bytes)),
                } if bytes.len() > TNS_MAX_SHORT_LOB_INOUT_SIZE => {
                    let max_len = *max_len;
                    let locator = match self.create_temp_blob(bytes) {
                        Ok(locator) => locator,
                        Err(error) => {
                            let _ = self.free_temp_lobs(&temp_lob_locators);
                            return Err(error);
                        }
                    };
                    temp_lob_locators.push(locator.clone());
                    *bind = BindValue::InOut {
                        column_type: OracleColumnType::Blob,
                        max_len,
                        value: Some(BindInputValue::LobLocator(locator)),
                    };
                }
                BindValue::InOut {
                    column_type: OracleColumnType::Clob,
                    max_len,
                    value: Some(BindInputValue::Text(text)),
                } if text.len() > TNS_MAX_SHORT_LOB_INOUT_SIZE => {
                    let max_len = *max_len;
                    let locator = match self.create_temp_clob(text) {
                        Ok(locator) => locator,
                        Err(error) => {
                            let _ = self.free_temp_lobs(&temp_lob_locators);
                            return Err(error);
                        }
                    };
                    temp_lob_locators.push(locator.clone());
                    *bind = BindValue::InOut {
                        column_type: OracleColumnType::Clob,
                        max_len,
                        value: Some(BindInputValue::LobLocator(locator)),
                    };
                }
                BindValue::InOut {
                    column_type: OracleColumnType::Nclob,
                    max_len,
                    value: Some(BindInputValue::Text(text)),
                } if text.len() > 4000 => {
                    let max_len = *max_len;
                    let locator = match self.create_temp_nclob(text) {
                        Ok(locator) => locator,
                        Err(error) => {
                            let _ = self.free_temp_lobs(&temp_lob_locators);
                            return Err(error);
                        }
                    };
                    temp_lob_locators.push(locator.clone());
                    *bind = BindValue::InOut {
                        column_type: OracleColumnType::Nclob,
                        max_len,
                        value: Some(BindInputValue::LobLocator(locator)),
                    };
                }
                _ => {}
            }
        }
        Ok((materialized, temp_lob_locators))
    }

    fn simple_ttc_call(
        &mut self,
        function_code: u8,
        operation: &str,
    ) -> Result<(), OracleThinError> {
        log_connect_phase(&format!("ttc-{operation}-write"), "");
        let mut payload = Vec::new();
        let pending_cursor_closes = self.drain_pending_cursor_closes();
        let current_schema = self.pending_current_schema_for_write();
        if let Some(schema) = current_schema.as_deref() {
            let sequence = self.next_ttc_sequence();
            if let Err(error) =
                write_current_schema_piggyback(&mut payload, &self.capabilities, sequence, schema)
            {
                self.requeue_pending_cursor_closes(&pending_cursor_closes);
                return Err(error);
            }
        }
        if !pending_cursor_closes.is_empty() {
            let close_sequence = self.next_ttc_sequence();
            if let Err(error) = write_close_cursors_piggyback(
                &mut payload,
                &self.capabilities,
                close_sequence,
                &pending_cursor_closes,
            ) {
                self.requeue_pending_cursor_closes(&pending_cursor_closes);
                return Err(error);
            }
        }
        let end_to_end = self.pending_end_to_end_for_write();
        if let Some(attrs) = end_to_end.as_ref() {
            let sequence = self.next_ttc_sequence();
            if let Err(error) =
                write_end_to_end_piggyback(&mut payload, &self.capabilities, sequence, attrs)
            {
                self.requeue_pending_cursor_closes(&pending_cursor_closes);
                return Err(error);
            }
        }
        let sequence = self.next_ttc_sequence();
        write_function_code(&mut payload, function_code, sequence, &self.capabilities);
        if let Err(error) = write_data_packet(
            &mut self.stream,
            self.capabilities.protocol_version.unwrap_or(319),
            self.capabilities.data_packet_chunk_size(),
            &payload,
        ) {
            self.requeue_pending_cursor_closes(&pending_cursor_closes);
            return Err(error);
        }
        self.clear_pending_current_schema_if_written(current_schema.as_deref());
        self.clear_pending_end_to_end_if_written(end_to_end.as_ref());
        log_connect_phase(&format!("ttc-{operation}-read"), "");
        let response = read_simple_response(
            &mut self.stream,
            &self.capabilities,
            &mut self.server_state,
            !pending_cursor_closes.is_empty(),
        );
        if self.cancel_flag.swap(false, Ordering::SeqCst) {
            return Err(self.finish_cancelled_read());
        }
        response
    }

    fn send_session_state(&mut self, session_state: u8) -> Result<(), OracleThinError> {
        let mut payload = Vec::new();
        let state_sequence = self.next_ttc_sequence();
        write_session_state_piggyback(
            &mut payload,
            &self.capabilities,
            state_sequence,
            session_state,
        );
        let ping_sequence = self.next_ttc_sequence();
        write_function_code(
            &mut payload,
            TNS_FUNC_PING,
            ping_sequence,
            &self.capabilities,
        );
        write_data_packet(
            &mut self.stream,
            self.capabilities.protocol_version.unwrap_or(319),
            self.capabilities.data_packet_chunk_size(),
            &payload,
        )?;
        read_simple_response(
            &mut self.stream,
            &self.capabilities,
            &mut self.server_state,
            false,
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

    fn pending_end_to_end_for_write(&self) -> Option<EndToEndAttributes> {
        if self.pending_end_to_end.is_empty() {
            None
        } else {
            Some(self.pending_end_to_end.clone())
        }
    }

    fn pending_current_schema_for_write(&self) -> Option<String> {
        self.pending_current_schema.clone()
    }

    fn clear_pending_end_to_end_if_written(&mut self, end_to_end: Option<&EndToEndAttributes>) {
        if end_to_end.is_some() {
            self.pending_end_to_end = EndToEndAttributes::default();
        }
    }

    fn clear_pending_current_schema_if_written(&mut self, current_schema: Option<&str>) {
        if current_schema.is_some() {
            self.pending_current_schema = None;
        }
    }

    fn drain_pending_cursor_closes(&mut self) -> Vec<u32> {
        if self.pending_cursor_closes.is_empty() {
            return Vec::new();
        }
        normalize_cursor_ids(std::mem::take(&mut self.pending_cursor_closes))
    }

    fn requeue_pending_cursor_closes(&mut self, cursor_ids: &[u32]) {
        if cursor_ids.is_empty() {
            return;
        }
        let mut merged = self.pending_cursor_closes.clone();
        merged.extend_from_slice(cursor_ids);
        self.pending_cursor_closes = normalize_cursor_ids(merged);
    }

    fn free_temp_lobs(&mut self, locators: &[Vec<u8>]) -> Result<(), OracleThinError> {
        if locators.is_empty() {
            return Ok(());
        }
        let mut payload = Vec::new();
        let pending_cursor_closes = self.drain_pending_cursor_closes();
        let current_schema = self.pending_current_schema_for_write();
        if let Some(schema) = current_schema.as_deref() {
            let sequence = self.next_ttc_sequence();
            if let Err(error) =
                write_current_schema_piggyback(&mut payload, &self.capabilities, sequence, schema)
            {
                self.requeue_pending_cursor_closes(&pending_cursor_closes);
                return Err(error);
            }
        }
        if !pending_cursor_closes.is_empty() {
            let close_sequence = self.next_ttc_sequence();
            if let Err(error) = write_close_cursors_piggyback(
                &mut payload,
                &self.capabilities,
                close_sequence,
                &pending_cursor_closes,
            ) {
                self.requeue_pending_cursor_closes(&pending_cursor_closes);
                return Err(error);
            }
        }
        let end_to_end = self.pending_end_to_end_for_write();
        if let Some(attrs) = end_to_end.as_ref() {
            let sequence = self.next_ttc_sequence();
            if let Err(error) =
                write_end_to_end_piggyback(&mut payload, &self.capabilities, sequence, attrs)
            {
                self.requeue_pending_cursor_closes(&pending_cursor_closes);
                return Err(error);
            }
        }
        let free_sequence = self.next_ttc_sequence();
        if let Err(error) = write_close_temp_lobs_piggyback(
            &mut payload,
            &self.capabilities,
            free_sequence,
            locators,
        ) {
            self.requeue_pending_cursor_closes(&pending_cursor_closes);
            return Err(error);
        }
        let ping_sequence = self.next_ttc_sequence();
        write_function_code(
            &mut payload,
            TNS_FUNC_PING,
            ping_sequence,
            &self.capabilities,
        );
        if let Err(error) = write_data_packet(
            &mut self.stream,
            self.capabilities.protocol_version.unwrap_or(319),
            self.capabilities.data_packet_chunk_size(),
            &payload,
        ) {
            self.requeue_pending_cursor_closes(&pending_cursor_closes);
            return Err(error);
        }
        self.clear_pending_current_schema_if_written(current_schema.as_deref());
        self.clear_pending_end_to_end_if_written(end_to_end.as_ref());
        read_simple_response(
            &mut self.stream,
            &self.capabilities,
            &mut self.server_state,
            !pending_cursor_closes.is_empty(),
        )
    }
}

impl Drop for OracleThinSession {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn normalize_cursor_ids(mut cursor_ids: Vec<u32>) -> Vec<u32> {
    if cursor_ids.is_empty() {
        return Vec::new();
    }
    cursor_ids.retain(|cursor_id| *cursor_id != 0);
    cursor_ids.sort_unstable();
    cursor_ids.dedup();
    cursor_ids
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
    let supports_end_of_response = accept.protocol_version >= TNS_VERSION_MIN_END_OF_RESPONSE
        && accept.supports_end_of_response();
    let supports_sql_boolean = if accept.protocol_version < TNS_VERSION_MIN_ACCEPTED {
        ttc_field_version >= 23
    } else {
        ttc_field_version >= TNS_CCAP_FIELD_VERSION_23_1
    };
    OracleThinCapabilities {
        protocol_version: Some(accept.protocol_version),
        ttc_field_version,
        charset_id: 0,
        ncharset_id: 0,
        max_string_size: 4000,
        supports_sql_boolean,
        supports_end_of_response,
        supports_request_boundaries: false,
        supports_fast_auth: accept.supports_fast_auth(),
        supports_oob: accept.supports_oob_attention(),
        supports_oob_check: accept.supports_oob_check(),
        supports_big_clr_chunks: accept.protocol_version >= TNS_VERSION_MIN_ACCEPTED,
        supports_oson_long_field_names: false,
        auth_uses_pbkdf2_key_derivation: false,
        sdu: usize::try_from(accept.sdu).unwrap_or(TNS_DEFAULT_SDU),
        server_ttc_field_version: 0,
        supports_end_of_call_status: true,
        supports_fast_session_attributes: true,
        supports_implicit_resultsets: accept.protocol_version >= TNS_VERSION_MIN_ACCEPTED,
    }
}

fn probe_oob_reset_if_supported(
    stream: &mut TcpStream,
    options: &ConnectOptions,
    capabilities: &OracleThinCapabilities,
) -> Result<(), OracleThinError> {
    if options.disable_oob_probe || !(capabilities.supports_oob && capabilities.supports_oob_check)
    {
        return Ok(());
    }
    send_oob_break(stream)?;
    write_marker_packet(
        stream,
        capabilities.protocol_version.unwrap_or(319),
        TNS_MARKER_TYPE_RESET,
    )
}

fn validate_supported_protocol(accept: &AcceptInfo) -> Result<(), OracleThinError> {
    if accept.protocol_version < TNS_MIN_SUPPORTED_PROTOCOL {
        return Err(OracleThinError::new(format!(
            "Oracle Thin supports TNS protocol {TNS_MIN_SUPPORTED_PROTOCOL} and newer; listener accepted {}",
            accept.protocol_version
        )));
    }
    Ok(())
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
        capabilities.data_packet_chunk_size(),
        &payload,
    )?;

    log_connect_phase("ttc-protocol-read", "");
    let (oob_reset_received, packet) =
        read_data_packet_with_control(stream, capabilities.protocol_version.unwrap_or(319))?;
    if oob_reset_received {
        capabilities.supports_oob = false;
    }
    let mut cursor = PacketCursor::with_capabilities(&packet, capabilities);
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
    write_data_type_representations(&mut payload, capabilities.protocol_version);
    write_data_packet(
        stream,
        capabilities.protocol_version.unwrap_or(319),
        capabilities.data_packet_chunk_size(),
        &payload,
    )?;

    log_connect_phase("ttc-data-types-read", "");
    let packet = read_data_packet(stream, capabilities.protocol_version.unwrap_or(319))?;
    let mut cursor = PacketCursor::with_capabilities(&packet, capabilities);
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

fn write_data_type_representations(payload: &mut Vec<u8>, protocol_version: Option<u16>) {
    for (data_type, conv_data_type, representation) in DATA_TYPE_REPRESENTATIONS {
        put_u16_be_vec(payload, *data_type);
        let conv_data_type = if protocol_uses_go_ora_legacy_mappings(protocol_version) {
            match *data_type {
                data_type if data_type == u16::from(TNS_DATA_TYPE_VBI) => {
                    u16::from(ORA_TYPE_NUM_RAW)
                }
                data_type if data_type == u16::from(TNS_DATA_TYPE_OAC9) => TNS_DATA_TYPE_OAC,
                _ => *conv_data_type,
            }
        } else {
            *conv_data_type
        };
        put_u16_be_vec(payload, conv_data_type);
        if conv_data_type != 0 {
            put_u16_be_vec(payload, *representation);
            put_u16_be_vec(payload, 0);
        }
    }
    if protocol_uses_python_oracledb_modern_mappings(protocol_version) {
        for (data_type, conv_data_type, representation) in
            PYTHON_ORACLEDB_MODERN_DATA_TYPE_REPRESENTATIONS
        {
            put_u16_be_vec(payload, *data_type);
            put_u16_be_vec(payload, *conv_data_type);
            put_u16_be_vec(payload, *representation);
            put_u16_be_vec(payload, 0);
        }
    }
    put_u16_be_vec(payload, 0);
}

fn protocol_uses_go_ora_legacy_mappings(protocol_version: Option<u16>) -> bool {
    protocol_version.is_some_and(|version| version < TNS_VERSION_MIN_ACCEPTED)
}

fn protocol_uses_python_oracledb_modern_mappings(protocol_version: Option<u16>) -> bool {
    !protocol_uses_go_ora_legacy_mappings(protocol_version)
}

#[derive(Debug, Clone)]
struct ExecuteResponse {
    columns: Vec<ColumnMetadata>,
    thin_columns: Vec<ThinColumn>,
    result: QueryResult,
    out_bind_rows: Vec<Vec<OracleValue>>,
    implicit_results: Vec<RefCursorValue>,
    cursor_columns: Vec<(u32, Vec<ThinColumn>)>,
}

#[derive(Debug, Clone)]
struct LobReadResponse {
    data: Vec<u8>,
    amount: Option<i64>,
    locator: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
struct ThinColumn {
    name: String,
    column_type: OracleColumnType,
    ora_type_num: u8,
    charset_form: u8,
    buffer_size: u32,
    schema_name: String,
    type_name: String,
}

#[derive(Debug, Default)]
struct ExecuteReadState {
    columns: Vec<ThinColumn>,
    rows: Vec<Vec<OracleValue>>,
    out_bind_columns: Vec<ThinColumn>,
    out_bind_rows: Vec<Vec<OracleValue>>,
    implicit_results: Vec<RefCursorValue>,
    cursor_columns: Vec<(u32, Vec<ThinColumn>)>,
    object_attrs_by_type: HashMap<(String, String), Vec<ThinColumn>>,
    collection_element_by_type: HashMap<(String, String), ThinColumn>,
    last_row: Option<Vec<OracleValue>>,
    bit_vector: Option<Vec<u8>>,
    reading_out_binds: bool,
    reading_dml_returning: bool,
    cursor_id: Option<u32>,
    exhausted: bool,
    done: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ServerSidePiggybackState {
    ltxid: Vec<u8>,
    session_id: Option<u32>,
    serial_num: Option<u16>,
    session_changed: bool,
    current_schema: Option<String>,
    edition: Option<String>,
    sessionless_transaction_id: Option<Vec<u8>>,
    sessionless_started_on_server: bool,
    transaction_in_progress: bool,
    last_warning: Option<OracleThinWarning>,
}

trait LastRowSource {
    fn query_result(&self) -> &QueryResult;
}

impl LastRowSource for QueryResult {
    fn query_result(&self) -> &QueryResult {
        self
    }
}

impl LastRowSource for DescribedQueryResult {
    fn query_result(&self) -> &QueryResult {
        &self.result
    }
}

fn ref_cursor_ids_in_rows(rows: &[Vec<OracleValue>]) -> HashSet<u32> {
    rows.iter()
        .flatten()
        .filter_map(|value| match value {
            OracleValue::Cursor(cursor) => Some(cursor.cursor_id),
            _ => None,
        })
        .collect()
}

fn columns_may_contain_ref_cursors(columns: &[ColumnMetadata]) -> bool {
    columns.iter().any(|column| {
        column.column_type == OracleColumnType::Cursor
            || matches!(
                column.ora_type_num,
                ORA_TYPE_NUM_CURSOR | TNS_DATA_TYPE_RSET
            )
    })
}

fn column_types_may_contain_ref_cursors(column_types: &[OracleColumnType]) -> bool {
    column_types.is_empty()
        || column_types
            .iter()
            .any(|column_type| *column_type == OracleColumnType::Cursor)
}

fn columns_require_define_fetch_for_values(columns: &[ColumnMetadata]) -> bool {
    columns.iter().any(column_requires_define_fetch_for_value)
}

fn columns_require_object_metadata_for_values(columns: &[ColumnMetadata]) -> bool {
    columns
        .iter()
        .any(column_requires_object_metadata_for_value)
}

fn column_types_require_define_fetch_for_values(column_types: &[OracleColumnType]) -> bool {
    column_types
        .iter()
        .copied()
        .any(column_type_requires_define_fetch_for_value)
}

fn column_type_requires_define_fetch_for_value(column_type: OracleColumnType) -> bool {
    matches!(
        column_type,
        OracleColumnType::Long
            | OracleColumnType::Clob
            | OracleColumnType::Nclob
            | OracleColumnType::Blob
            | OracleColumnType::Vector
            | OracleColumnType::Json
    )
}

fn column_requires_define_fetch_for_value(column: &ColumnMetadata) -> bool {
    wire_type_requires_define_fetch_for_value(column.ora_type_num)
        || column_type_requires_define_fetch_for_value(column.column_type)
}

fn column_requires_object_metadata_for_value(column: &ColumnMetadata) -> bool {
    matches!(
        column.ora_type_num,
        ORA_TYPE_NUM_OBJECT | TNS_DATA_TYPE_EXT_NAMED | TNS_DATA_TYPE_PNTY
    ) && column.column_type != OracleColumnType::Xml
        && !column.schema_name.is_empty()
        && !column.type_name.is_empty()
}

fn wire_type_requires_define_fetch_for_value(ora_type_num: u8) -> bool {
    matches!(
        ora_type_num,
        ORA_TYPE_NUM_LONG
            | ORA_TYPE_NUM_LONG_RAW
            | ORA_TYPE_NUM_CLOB
            | TNS_DATA_TYPE_DCLOB
            | ORA_TYPE_NUM_BLOB
            | TNS_DATA_TYPE_DBLOB
            | ORA_TYPE_NUM_VECTOR
            | ORA_TYPE_NUM_JSON
            | ORA_TYPE_NUM_DJSON
    )
}

fn is_decodable_object_column(column: &ThinColumn) -> bool {
    matches!(
        column.ora_type_num,
        ORA_TYPE_NUM_OBJECT | TNS_DATA_TYPE_EXT_NAMED | TNS_DATA_TYPE_PNTY
    ) && column.column_type != OracleColumnType::Xml
        && !column.schema_name.is_empty()
        && !column.type_name.is_empty()
}

fn object_type_key(schema_name: &str, type_name: &str) -> (String, String) {
    (
        schema_name.trim_matches('"').to_string(),
        type_name.trim_matches('"').to_string(),
    )
}

fn sql_string_literal(value: &str) -> String {
    value.replace('\'', "''")
}

fn thin_column_from_object_attr(
    name: String,
    attr_type_name: String,
    attr_type_owner: String,
    attr_typecode: Option<String>,
    buffer_size: u32,
    charset_form: u8,
) -> Result<ThinColumn, OracleThinError> {
    let attr_type_name = attr_type_name.trim_matches('"').to_string();
    let attr_type_owner = attr_type_owner.trim_matches('"').to_string();
    let attr_type = attr_type_name.to_ascii_uppercase();
    let attr_typecode = attr_typecode.map(|value| value.to_ascii_uppercase());
    let is_xmltype =
        matches!(attr_type_owner.as_str(), "PUBLIC" | "SYS") && attr_type_name == "XMLTYPE";
    if !attr_type_owner.is_empty() && !is_xmltype {
        let (column_type, ora_type_num, schema_name, type_name) = match attr_typecode.as_deref() {
            Some("OBJECT") => (
                OracleColumnType::Object,
                ORA_TYPE_NUM_OBJECT,
                attr_type_owner,
                attr_type_name,
            ),
            Some("COLLECTION") => (
                OracleColumnType::Object,
                ORA_TYPE_NUM_OBJECT,
                attr_type_owner,
                attr_type_name,
            ),
            Some(typecode) => {
                return Err(OracleThinError::new(format!(
                    "Oracle thin TTC cannot decode UDT attribute {name} named type {attr_type_name} with TYPECODE {typecode}"
                )))
            }
            None => {
                return Err(OracleThinError::new(format!(
                    "Oracle thin TTC cannot verify UDT attribute {name} named type {attr_type_name}"
                )))
            }
        };
        let buffer_size = if buffer_size == 0 {
            default_object_attr_buffer_size(ora_type_num)
        } else {
            buffer_size
        };
        return Ok(ThinColumn {
            name,
            column_type,
            ora_type_num,
            charset_form,
            buffer_size,
            schema_name,
            type_name,
        });
    }
    let (column_type, ora_type_num, schema_name, type_name) = match attr_type.as_str() {
        "VARCHAR2" | "VARCHAR" => (
            OracleColumnType::Varchar,
            ORA_TYPE_NUM_VARCHAR,
            String::new(),
            String::new(),
        ),
        "CHAR" => (
            OracleColumnType::Varchar,
            ORA_TYPE_NUM_CHAR,
            String::new(),
            String::new(),
        ),
        "LONG" | "LONG VARCHAR" | "LONG NVARCHAR" => (
            OracleColumnType::Long,
            ORA_TYPE_NUM_LONG,
            String::new(),
            String::new(),
        ),
        "NVARCHAR2" => (
            OracleColumnType::Varchar,
            ORA_TYPE_NUM_VARCHAR,
            String::new(),
            String::new(),
        ),
        "NCHAR" => (
            OracleColumnType::Varchar,
            ORA_TYPE_NUM_CHAR,
            String::new(),
            String::new(),
        ),
        "NUMBER" | "DECIMAL" | "DEC" | "NUMERIC" | "INTEGER" | "INT" | "SMALLINT" | "FLOAT"
        | "REAL" | "DOUBLE PRECISION" => (
            OracleColumnType::Number,
            ORA_TYPE_NUM_NUMBER,
            String::new(),
            String::new(),
        ),
        "PL/SQL PLS INTEGER" | "PL/SQL BINARY INTEGER" => (
            OracleColumnType::Number,
            TNS_DATA_TYPE_BINARY_INTEGER,
            String::new(),
            String::new(),
        ),
        "DATE" => (
            OracleColumnType::Date,
            ORA_TYPE_NUM_DATE,
            String::new(),
            String::new(),
        ),
        "TIMESTAMP" => (
            OracleColumnType::Timestamp,
            ORA_TYPE_NUM_TIMESTAMP,
            String::new(),
            String::new(),
        ),
        "TIMESTAMP WITH TIME ZONE" | "TIMESTAMP WITH TZ" => (
            OracleColumnType::Timestamp,
            ORA_TYPE_NUM_TIMESTAMP_TZ,
            String::new(),
            String::new(),
        ),
        "TIMESTAMP WITH LOCAL TIME ZONE" | "TIMESTAMP WITH LOCAL TZ" => (
            OracleColumnType::Timestamp,
            ORA_TYPE_NUM_TIMESTAMP_LTZ,
            String::new(),
            String::new(),
        ),
        "RAW" => (
            OracleColumnType::Raw,
            ORA_TYPE_NUM_RAW,
            String::new(),
            String::new(),
        ),
        "LONG RAW" => (
            OracleColumnType::Raw,
            ORA_TYPE_NUM_LONG_RAW,
            String::new(),
            String::new(),
        ),
        "CLOB" => (
            OracleColumnType::Clob,
            ORA_TYPE_NUM_CLOB,
            String::new(),
            String::new(),
        ),
        "NCLOB" => (
            OracleColumnType::Nclob,
            ORA_TYPE_NUM_CLOB,
            String::new(),
            String::new(),
        ),
        "BLOB" => (
            OracleColumnType::Blob,
            ORA_TYPE_NUM_BLOB,
            String::new(),
            String::new(),
        ),
        "BFILE" => (
            OracleColumnType::Bfile,
            ORA_TYPE_NUM_BFILE,
            String::new(),
            String::new(),
        ),
        "XMLTYPE" => (
            OracleColumnType::Xml,
            ORA_TYPE_NUM_OBJECT,
            attr_type_owner,
            attr_type,
        ),
        "BINARY_FLOAT" => (
            OracleColumnType::BinaryFloat,
            ORA_TYPE_NUM_BINARY_FLOAT,
            String::new(),
            String::new(),
        ),
        "BINARY_DOUBLE" => (
            OracleColumnType::BinaryDouble,
            ORA_TYPE_NUM_BINARY_DOUBLE,
            String::new(),
            String::new(),
        ),
        "BOOLEAN" | "PL/SQL BOOLEAN" => (
            OracleColumnType::Boolean,
            ORA_TYPE_NUM_BOOLEAN,
            String::new(),
            String::new(),
        ),
        "INTERVAL YEAR TO MONTH" => (
            OracleColumnType::IntervalYearMonth,
            ORA_TYPE_NUM_INTERVAL_YM,
            String::new(),
            String::new(),
        ),
        "INTERVAL DAY TO SECOND" => (
            OracleColumnType::IntervalDaySecond,
            ORA_TYPE_NUM_INTERVAL_DS,
            String::new(),
            String::new(),
        ),
        other => {
            return Err(OracleThinError::new(format!(
                "Oracle thin TTC cannot map UDT attribute {name} type {other}"
            )))
        }
    };
    let buffer_size = if buffer_size == 0 {
        default_object_attr_buffer_size(ora_type_num)
    } else {
        buffer_size
    };
    Ok(ThinColumn {
        name,
        column_type,
        ora_type_num,
        charset_form,
        buffer_size,
        schema_name,
        type_name,
    })
}

fn default_object_attr_buffer_size(ora_type_num: u8) -> u32 {
    match ora_type_num {
        ORA_TYPE_NUM_NUMBER => 22,
        TNS_DATA_TYPE_BINARY_INTEGER => 4,
        ORA_TYPE_NUM_DATE => 7,
        ORA_TYPE_NUM_TIMESTAMP | ORA_TYPE_NUM_TIMESTAMP_LTZ => 11,
        ORA_TYPE_NUM_TIMESTAMP_TZ => 13,
        ORA_TYPE_NUM_BINARY_FLOAT => 4,
        ORA_TYPE_NUM_BINARY_DOUBLE => 8,
        ORA_TYPE_NUM_BOOLEAN => 4,
        ORA_TYPE_NUM_INTERVAL_YM => 5,
        ORA_TYPE_NUM_INTERVAL_DS => 11,
        ORA_TYPE_NUM_LONG | ORA_TYPE_NUM_LONG_RAW => TNS_MAX_LONG_LENGTH,
        ORA_TYPE_NUM_BFILE => 1,
        ORA_TYPE_NUM_OBJECT => 1,
        _ => 0,
    }
}

#[derive(Debug, Default)]
struct ExecuteError {
    code: u32,
    cursor_id: u32,
    call_status: u32,
    _rowcount: u64,
    message: Option<String>,
    warning: Option<OracleThinWarning>,
}

fn execute_flags_for_request(parse_only_describe: bool, request: &StatementRequest) -> u32 {
    if parse_only_describe || !request_allows_implicit_resultsets(request) {
        0
    } else {
        TNS_EXEC_FLAGS_IMPLICIT_RESULTSET
    }
}

fn request_allows_implicit_resultsets(request: &StatementRequest) -> bool {
    request.implicit_resultsets
        && request.is_plsql
        && (request.binds.is_empty()
            || request
                .sql
                .to_ascii_uppercase()
                .contains("DBMS_SQL.RETURN_RESULT"))
}

fn request_is_dml_returning(request: &StatementRequest) -> bool {
    !request.is_query && !request.is_plsql && sql_is_dml_returning(&request.sql)
}

fn write_execute_request(
    stream: &mut TcpStream,
    capabilities: &OracleThinCapabilities,
    request: &StatementRequest,
    sequence: u8,
    cursor_id: u32,
    current_schema_sequence: Option<u8>,
    current_schema: Option<&str>,
    close_sequence: Option<u8>,
    close_cursor_ids: &[u32],
    end_to_end_sequence: Option<u8>,
    end_to_end: Option<&EndToEndAttributes>,
    execute_without_prefetch: bool,
) -> Result<(), OracleThinError> {
    if capabilities.ttc_field_version <= 6 {
        return write_legacy_execute_request(
            stream,
            capabilities,
            request,
            sequence,
            cursor_id,
            current_schema_sequence,
            current_schema,
            close_sequence,
            close_cursor_ids,
            end_to_end_sequence,
            end_to_end,
            execute_without_prefetch,
        );
    }

    let sql_bytes = request.sql.as_bytes();
    let parse_only_describe =
        request.is_query && request.prefetch_rows == 0 && !execute_without_prefetch;
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
    let exec_flags = execute_flags_for_request(parse_only_describe, request);

    let mut payload = Vec::new();
    if let (Some(sequence), Some(schema)) = (current_schema_sequence, current_schema) {
        write_current_schema_piggyback(&mut payload, capabilities, sequence, schema)?;
    }
    if let Some(close_sequence) = close_sequence {
        write_close_cursors_piggyback(
            &mut payload,
            capabilities,
            close_sequence,
            close_cursor_ids,
        )?;
    }
    if let (Some(sequence), Some(attrs)) = (end_to_end_sequence, end_to_end) {
        write_end_to_end_piggyback(&mut payload, capabilities, sequence, attrs)?;
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
    write_bytes_with_length_for_capabilities(&mut payload, sql_bytes, capabilities)?;
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
        write_bind_rows_for_request(&mut payload, capabilities, request)?;
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
        capabilities.data_packet_chunk_size(),
        &payload,
    )
}

fn write_legacy_execute_request(
    stream: &mut TcpStream,
    capabilities: &OracleThinCapabilities,
    request: &StatementRequest,
    sequence: u8,
    cursor_id: u32,
    current_schema_sequence: Option<u8>,
    current_schema: Option<&str>,
    close_sequence: Option<u8>,
    close_cursor_ids: &[u32],
    end_to_end_sequence: Option<u8>,
    end_to_end: Option<&EndToEndAttributes>,
    execute_without_prefetch: bool,
) -> Result<(), OracleThinError> {
    let sql_bytes = request.sql.as_bytes();
    let parse_only_describe =
        request.is_query && request.prefetch_rows == 0 && !execute_without_prefetch;
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
    if let (Some(sequence), Some(schema)) = (current_schema_sequence, current_schema) {
        write_current_schema_piggyback(&mut payload, capabilities, sequence, schema)?;
    }
    if let Some(close_sequence) = close_sequence {
        write_close_cursors_piggyback(
            &mut payload,
            capabilities,
            close_sequence,
            close_cursor_ids,
        )?;
    }
    if let (Some(sequence), Some(attrs)) = (end_to_end_sequence, end_to_end) {
        write_end_to_end_piggyback(&mut payload, capabilities, sequence, attrs)?;
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
    write_bytes_with_length_for_capabilities(&mut payload, sql_bytes, capabilities)?;

    let mut al8i4 = [0u32; 13];
    al8i4[0] = 1;
    al8i4[1] = if request.is_query { 0 } else { 1 };
    al8i4[7] = u32::from(request.is_query);
    al8i4[9] = execute_flags_for_request(parse_only_describe, request);
    for value in al8i4 {
        write_ub4(&mut payload, value);
    }
    if num_params > 0 {
        write_bind_metadata(&mut payload, capabilities, &request.binds)?;
        write_bind_rows_for_request(&mut payload, capabilities, request)?;
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
        capabilities.data_packet_chunk_size(),
        &payload,
    )
}

fn write_fetch_request(
    stream: &mut TcpStream,
    capabilities: &OracleThinCapabilities,
    cursor_id: u32,
    row_count: u32,
    sequence: u8,
    current_schema_sequence: Option<u8>,
    current_schema: Option<&str>,
    close_sequence: Option<u8>,
    close_cursor_ids: &[u32],
    end_to_end_sequence: Option<u8>,
    end_to_end: Option<&EndToEndAttributes>,
) -> Result<(), OracleThinError> {
    let mut payload = Vec::new();
    if let (Some(sequence), Some(schema)) = (current_schema_sequence, current_schema) {
        write_current_schema_piggyback(&mut payload, capabilities, sequence, schema)?;
    }
    if let Some(close_sequence) = close_sequence {
        write_close_cursors_piggyback(
            &mut payload,
            capabilities,
            close_sequence,
            close_cursor_ids,
        )?;
    }
    if let (Some(sequence), Some(attrs)) = (end_to_end_sequence, end_to_end) {
        write_end_to_end_piggyback(&mut payload, capabilities, sequence, attrs)?;
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
        capabilities.data_packet_chunk_size(),
        &payload,
    )
}

fn write_ref_cursor_execute_fetch_request(
    stream: &mut TcpStream,
    capabilities: &OracleThinCapabilities,
    cursor_id: u32,
    row_count: u32,
    sequence: u8,
    current_schema_sequence: Option<u8>,
    current_schema: Option<&str>,
    close_sequence: Option<u8>,
    close_cursor_ids: &[u32],
    end_to_end_sequence: Option<u8>,
    end_to_end: Option<&EndToEndAttributes>,
) -> Result<(), OracleThinError> {
    let mut payload = Vec::new();
    if let (Some(sequence), Some(schema)) = (current_schema_sequence, current_schema) {
        write_current_schema_piggyback(&mut payload, capabilities, sequence, schema)?;
    }
    if let Some(close_sequence) = close_sequence {
        write_close_cursors_piggyback(
            &mut payload,
            capabilities,
            close_sequence,
            close_cursor_ids,
        )?;
    }
    if let (Some(sequence), Some(attrs)) = (end_to_end_sequence, end_to_end) {
        write_end_to_end_piggyback(&mut payload, capabilities, sequence, attrs)?;
    }
    write_function_code(&mut payload, TNS_FUNC_EXECUTE, sequence, capabilities);
    let options = TNS_EXEC_OPTION_NOT_PLSQL | TNS_EXEC_OPTION_FETCH;
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
        let mut al8i4 = [0u32; 13];
        al8i4[1] = row_count;
        al8i4[7] = 1;
        for value in al8i4 {
            write_ub4(&mut payload, value);
        }
    }
    if std::env::var_os("ORACLE_THIN_TRACE_EXEC").is_some() {
        eprintln!(
            "thin ref cursor execute fetch request cursor={} rows={} payload={}",
            cursor_id,
            row_count,
            hex_encode_upper(&payload)
        );
    }
    write_data_packet(
        stream,
        capabilities.protocol_version.unwrap_or(319),
        capabilities.data_packet_chunk_size(),
        &payload,
    )
}

fn write_define_fetch_request(
    stream: &mut TcpStream,
    capabilities: &OracleThinCapabilities,
    cursor_id: u32,
    row_count: u32,
    sequence: u8,
    current_schema_sequence: Option<u8>,
    current_schema: Option<&str>,
    close_sequence: Option<u8>,
    close_cursor_ids: &[u32],
    end_to_end_sequence: Option<u8>,
    end_to_end: Option<&EndToEndAttributes>,
    columns: &[ThinColumn],
) -> Result<(), OracleThinError> {
    let mut payload = Vec::new();
    if let (Some(sequence), Some(schema)) = (current_schema_sequence, current_schema) {
        write_current_schema_piggyback(&mut payload, capabilities, sequence, schema)?;
    }
    if let Some(close_sequence) = close_sequence {
        write_close_cursors_piggyback(
            &mut payload,
            capabilities,
            close_sequence,
            close_cursor_ids,
        )?;
    }
    if let (Some(sequence), Some(attrs)) = (end_to_end_sequence, end_to_end) {
        write_end_to_end_piggyback(&mut payload, capabilities, sequence, attrs)?;
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
        capabilities.data_packet_chunk_size(),
        &payload,
    )
}

fn write_lob_read_request(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    sequence: u8,
    locator: &[u8],
    source_offset: u64,
    amount: u64,
) {
    write_function_code(payload, TNS_FUNC_LOB_OP, sequence, capabilities);
    payload.push(1);
    write_ub4(payload, locator.len() as u32);
    payload.push(0);
    write_ub4(payload, 0);
    write_ub4(payload, 0);
    write_ub4(payload, 0);
    payload.push(0);
    payload.push(0);
    payload.push(0);
    write_ub4(payload, TNS_LOB_OP_READ);
    payload.push(0);
    payload.push(0);
    write_ub8(payload, source_offset);
    write_ub8(payload, 0);
    payload.push(1);
    put_u16_be_vec(payload, 0);
    put_u16_be_vec(payload, 0);
    put_u16_be_vec(payload, 0);
    payload.extend_from_slice(locator);
    write_ub8(payload, amount);
}

fn write_lob_operation_request(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    sequence: u8,
    locator: &[u8],
    operation: u32,
    source_offset: u64,
    dest_offset: u64,
    dest_length: u32,
    data: Option<&[u8]>,
    charset_id: Option<u16>,
) -> Result<(), OracleThinError> {
    write_function_code(payload, TNS_FUNC_LOB_OP, sequence, capabilities);
    payload.push(1);
    write_ub4(payload, locator.len() as u32);
    payload.push(0);
    write_ub4(payload, dest_length);
    write_ub4(payload, 0);
    write_ub4(payload, 0);
    payload.push(u8::from(charset_id.is_some()));
    payload.push(0);
    payload.push(u8::from(operation == TNS_LOB_OP_CREATE_TEMP));
    write_ub4(payload, operation);
    payload.push(0);
    payload.push(0);
    write_ub8(payload, source_offset);
    write_ub8(payload, dest_offset);
    payload.push(0);
    put_u16_be_vec(payload, 0);
    put_u16_be_vec(payload, 0);
    put_u16_be_vec(payload, 0);
    payload.extend_from_slice(locator);
    if let Some(charset_id) = charset_id {
        write_ub4(payload, u32::from(charset_id));
    }
    if let Some(data) = data {
        payload.push(TNS_MSG_TYPE_LOB_DATA);
        write_bytes_with_length_for_capabilities(payload, data, capabilities)?;
    }
    Ok(())
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

fn write_current_schema_piggyback(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    sequence: u8,
    schema: &str,
) -> Result<(), OracleThinError> {
    write_piggyback_code(payload, TNS_FUNC_SET_SCHEMA, sequence, capabilities);
    payload.push(1);
    write_bytes_with_two_lengths(payload, schema.as_bytes())
}

fn write_end_to_end_piggyback(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    sequence: u8,
    attrs: &EndToEndAttributes,
) -> Result<(), OracleThinError> {
    if attrs.is_empty() {
        return Ok(());
    }
    let mut flags = 0;
    if attrs.action.is_some() {
        flags |= TNS_END_TO_END_ACTION;
    }
    if attrs.client_identifier.is_some() {
        flags |= TNS_END_TO_END_CLIENT_IDENTIFIER;
    }
    if attrs.client_info.is_some() {
        flags |= TNS_END_TO_END_CLIENT_INFO;
    }
    if attrs.dbop.is_some() {
        flags |= TNS_END_TO_END_DBOP;
    }
    if attrs.module.is_some() {
        flags |= TNS_END_TO_END_MODULE;
    }

    let client_identifier_bytes = attrs
        .client_identifier
        .as_ref()
        .and_then(|value| value.as_deref())
        .map(str::as_bytes);
    let module_bytes = attrs
        .module
        .as_ref()
        .and_then(|value| value.as_deref())
        .map(str::as_bytes);
    let action_bytes = attrs
        .action
        .as_ref()
        .and_then(|value| value.as_deref())
        .map(str::as_bytes);
    let client_info_bytes = attrs
        .client_info
        .as_ref()
        .and_then(|value| value.as_deref())
        .map(str::as_bytes);
    let dbop_bytes = attrs
        .dbop
        .as_ref()
        .and_then(|value| value.as_deref())
        .map(str::as_bytes);

    write_piggyback_code(
        payload,
        TNS_FUNC_SET_END_TO_END_ATTR,
        sequence,
        capabilities,
    );
    payload.push(0);
    payload.push(0);
    write_ub4(payload, flags);
    write_end_to_end_header(
        payload,
        attrs.client_identifier.is_some(),
        client_identifier_bytes,
    );
    write_end_to_end_header(payload, attrs.module.is_some(), module_bytes);
    write_end_to_end_header(payload, attrs.action.is_some(), action_bytes);
    payload.push(0);
    write_ub4(payload, 0);
    payload.push(0);
    write_ub4(payload, 0);
    write_end_to_end_header(payload, attrs.client_info.is_some(), client_info_bytes);
    payload.push(0);
    write_ub4(payload, 0);
    payload.push(0);
    write_ub4(payload, 0);
    write_end_to_end_header(payload, attrs.dbop.is_some(), dbop_bytes);

    for bytes in [
        client_identifier_bytes,
        module_bytes,
        action_bytes,
        client_info_bytes,
        dbop_bytes,
    ]
    .into_iter()
    .flatten()
    {
        write_bytes_with_length_for_capabilities(payload, bytes, capabilities)?;
    }
    Ok(())
}

fn write_end_to_end_header(payload: &mut Vec<u8>, modified: bool, value: Option<&[u8]>) {
    payload.push(u8::from(modified));
    write_ub4(payload, value.map_or(0, |bytes| bytes.len() as u32));
}

fn write_close_temp_lobs_piggyback(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    sequence: u8,
    locators: &[Vec<u8>],
) -> Result<(), OracleThinError> {
    if locators.is_empty() {
        return Ok(());
    }
    write_piggyback_code(payload, TNS_FUNC_LOB_OP, sequence, capabilities);
    payload.push(1);
    write_ub4(payload, locators.iter().map(Vec::len).sum::<usize>() as u32);
    payload.push(0);
    write_ub4(payload, 0);
    write_ub4(payload, 0);
    write_ub4(payload, 0);
    payload.push(0);
    payload.push(0);
    payload.push(0);
    write_ub4(payload, TNS_LOB_OP_FREE_TEMP | TNS_LOB_OP_ARRAY);
    payload.push(0);
    write_ub4(payload, 0);
    write_ub8(payload, 0);
    write_ub8(payload, 0);
    payload.push(0);
    payload.push(0);
    write_ub4(payload, 0);
    payload.push(0);
    write_ub4(payload, 0);
    payload.push(0);
    write_ub4(payload, 0);
    for locator in locators {
        payload.extend_from_slice(locator);
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
    columns: &[ThinColumn],
) -> Result<(), OracleThinError> {
    for column in columns {
        write_column_metadata(payload, capabilities, column)?;
    }
    Ok(())
}

fn write_column_metadata(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    column: &ThinColumn,
) -> Result<(), OracleThinError> {
    let mut ora_type_num = column.ora_type_num;
    let mut buffer_size = column.buffer_size;
    if matches!(ora_type_num, ORA_TYPE_NUM_ROWID | ORA_TYPE_NUM_UROWID) {
        ora_type_num = ORA_TYPE_NUM_VARCHAR;
        buffer_size = TNS_MAX_UROWID_LENGTH;
    }
    let (cont_flag, lob_prefetch_length) = match ora_type_num {
        ORA_TYPE_NUM_CLOB | TNS_DATA_TYPE_DCLOB | ORA_TYPE_NUM_BLOB | TNS_DATA_TYPE_DBLOB => {
            (TNS_LOB_PREFETCH_FLAG, 0)
        }
        ORA_TYPE_NUM_VECTOR => (TNS_LOB_PREFETCH_FLAG, TNS_VECTOR_MAX_LENGTH),
        ORA_TYPE_NUM_JSON => (TNS_LOB_PREFETCH_FLAG, TNS_JSON_MAX_LENGTH),
        _ => (0, 0),
    };
    payload.push(ora_type_num);
    payload.push(TNS_BIND_USE_INDICATORS);
    payload.push(0);
    payload.push(0);
    write_ub4(payload, buffer_size);
    write_ub4(payload, 0);
    if capabilities.ttc_field_version <= 6 {
        write_ub4(payload, cont_flag as u32);
        payload.push(0);
    } else {
        write_ub8(payload, cont_flag);
        write_ub4(payload, 0);
    }
    write_ub2(payload, 0);
    if column.charset_form != 0 {
        write_ub2(payload, TNS_CHARSET_UTF8);
    } else {
        write_ub2(payload, 0);
    }
    payload.push(column.charset_form);
    write_ub4(payload, lob_prefetch_length);
    if capabilities.ttc_field_version >= TNS_CCAP_FIELD_VERSION_12_2 {
        write_ub4(payload, 0);
    }
    Ok(())
}

fn write_bind_rows(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    is_plsql: bool,
    binds: &[BindValue],
) -> Result<(), OracleThinError> {
    payload.push(TNS_MSG_TYPE_ROW_DATA);
    write_bind_values_for_row(payload, capabilities, is_plsql, binds.iter())
}

fn write_bind_values_for_row<'a, I>(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    is_plsql: bool,
    binds: I,
) -> Result<(), OracleThinError>
where
    I: IntoIterator<Item = &'a BindValue>,
{
    let binds = binds.into_iter().collect::<Vec<_>>();
    for bind in binds
        .iter()
        .copied()
        .filter(|bind| !bind_is_deferred_long_for_sql(capabilities, is_plsql, bind))
    {
        write_bind_value(payload, capabilities, bind)?;
    }
    for bind in binds
        .iter()
        .copied()
        .filter(|bind| bind_is_deferred_long_for_sql(capabilities, is_plsql, bind))
    {
        write_bind_value(payload, capabilities, bind)?;
    }
    Ok(())
}

fn bind_is_deferred_long_for_sql(
    capabilities: &OracleThinCapabilities,
    is_plsql: bool,
    bind: &BindValue,
) -> bool {
    !is_plsql
        && !bind_is_value_based_lob_payload(bind)
        && bind_column_metadata(bind).buffer_size > capabilities.max_string_size
}

fn bind_is_value_based_lob_payload(bind: &BindValue) -> bool {
    matches!(
        bind,
        BindValue::Vector(_)
            | BindValue::Json(_)
            | BindValue::JsonBool(_)
            | BindValue::JsonNumber(_)
            | BindValue::JsonString(_)
            | BindValue::JsonRaw(_)
            | BindValue::JsonId(_)
            | BindValue::JsonDate(_)
            | BindValue::JsonTimestamp(_)
            | BindValue::JsonIntervalYearMonth(_)
            | BindValue::JsonIntervalDaySecond(_)
            | BindValue::JsonVector(_)
    )
}

fn write_bind_rows_for_request(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    request: &StatementRequest,
) -> Result<(), OracleThinError> {
    if !request_is_dml_returning(request) {
        return write_bind_rows(payload, capabilities, request.is_plsql, &request.binds);
    }
    let input_binds = request
        .binds
        .iter()
        .filter(|bind| !bind_can_return_value(bind))
        .collect::<Vec<_>>();
    if input_binds.is_empty() {
        return Ok(());
    }
    payload.push(TNS_MSG_TYPE_ROW_DATA);
    write_bind_values_for_row(payload, capabilities, request.is_plsql, input_binds)
}

fn bind_can_return_value(bind: &BindValue) -> bool {
    matches!(bind, BindValue::Out { .. } | BindValue::InOut { .. })
}

fn bind_needs_lob_materialization(bind: &BindValue) -> bool {
    match bind {
        BindValue::Blob(_) | BindValue::Clob(_) | BindValue::Nclob(_) => true,
        BindValue::InOut {
            column_type: OracleColumnType::Blob,
            value: Some(BindInputValue::Bytes(bytes)),
            ..
        } => bytes.len() > TNS_MAX_SHORT_LOB_INOUT_SIZE,
        BindValue::InOut {
            column_type: OracleColumnType::Clob,
            value: Some(BindInputValue::Text(text)),
            ..
        } => text.len() > TNS_MAX_SHORT_LOB_INOUT_SIZE,
        BindValue::InOut {
            column_type: OracleColumnType::Nclob,
            value: Some(BindInputValue::Text(text)),
            ..
        } => text.len() > 4000,
        _ => false,
    }
}

fn write_bind_value(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    bind: &BindValue,
) -> Result<(), OracleThinError> {
    match bind {
        BindValue::Null(OracleColumnType::Boolean)
        | BindValue::Out {
            column_type: OracleColumnType::Boolean,
            ..
        } => write_null_boolean(payload),
        BindValue::Out {
            column_type: OracleColumnType::Cursor,
            ..
        } => {
            payload.push(1);
            payload.push(0);
            Ok(())
        }
        BindValue::Null(_) | BindValue::Out { .. } => {
            payload.push(0);
            Ok(())
        }
        BindValue::Number(value) => write_oracle_number(payload, value),
        BindValue::BinaryFloat(value) => {
            write_bytes_with_length(payload, &encode_oracle_binary_float(*value))
        }
        BindValue::BinaryDouble(value) => {
            write_bytes_with_length(payload, &encode_oracle_binary_double(*value))
        }
        BindValue::Text(value) => {
            write_text_bind_value(payload, capabilities, value, CS_FORM_IMPLICIT)
        }
        BindValue::Bytes(value) => {
            write_bytes_with_length_for_capabilities(payload, value, capabilities)
        }
        BindValue::Rowid(value) | BindValue::Urowid(value) => {
            write_bytes_with_length_for_capabilities(payload, value.as_bytes(), capabilities)
        }
        BindValue::Boolean(value) => {
            if *value {
                write_bytes_with_length(payload, &[1, 1])
            } else {
                write_bytes_with_length(payload, &[0])
            }
        }
        BindValue::Date(value) => write_bytes_with_length(payload, &encode_oracle_date(value, 7)),
        BindValue::Timestamp(value) => {
            write_bytes_with_length(payload, &encode_oracle_timestamp_bind(value))
        }
        BindValue::IntervalYearMonth(value) => {
            let bytes = encode_oracle_interval_ym(value)?;
            write_bytes_with_length(payload, &bytes)
        }
        BindValue::IntervalDaySecond(value) => {
            let bytes = encode_oracle_interval_ds(value)?;
            write_bytes_with_length(payload, &bytes)
        }
        BindValue::Vector(value) => write_vector_bind_value(payload, capabilities, value),
        BindValue::Json(value) => write_json_bind_text(payload, capabilities, value),
        BindValue::JsonBool(value) => write_json_bind_bool(payload, capabilities, *value),
        BindValue::JsonNumber(value) => write_json_bind_number(payload, capabilities, value),
        BindValue::JsonString(value) => write_json_bind_string(payload, capabilities, value),
        BindValue::JsonRaw(value) => write_json_bind_raw(payload, capabilities, value),
        BindValue::JsonId(value) => write_json_bind_id(payload, capabilities, value),
        BindValue::JsonDate(value) => write_json_bind_date(payload, capabilities, value),
        BindValue::JsonTimestamp(value) => write_json_bind_timestamp(payload, capabilities, value),
        BindValue::JsonIntervalYearMonth(value) => {
            write_json_bind_interval_ym(payload, capabilities, value)
        }
        BindValue::JsonIntervalDaySecond(value) => {
            write_json_bind_interval_ds(payload, capabilities, value)
        }
        BindValue::JsonVector(value) => write_json_bind_vector(payload, capabilities, value),
        BindValue::Blob(value) => {
            write_bytes_with_length_for_capabilities(payload, value, capabilities)
        }
        BindValue::Clob(value) => {
            write_text_bind_value(payload, capabilities, value, CS_FORM_IMPLICIT)
        }
        BindValue::Nclob(value) => {
            write_text_bind_value(payload, capabilities, value, CS_FORM_NCHAR)
        }
        BindValue::Bfile {
            directory_alias,
            file_name,
        } => {
            let locator = encode_bfile_locator(directory_alias, file_name, capabilities)?;
            write_bytes_with_two_lengths(payload, &locator)
        }
        BindValue::LobLocator { locator, .. } => write_bytes_with_two_lengths(payload, locator),
        BindValue::InOut {
            column_type, value, ..
        } => match value {
            Some(BindInputValue::Number(value)) if *column_type == OracleColumnType::Json => {
                write_json_bind_number(payload, capabilities, value)
            }
            Some(BindInputValue::Number(value)) => write_oracle_number(payload, value),
            Some(BindInputValue::BinaryFloat(value)) => {
                if *column_type != OracleColumnType::BinaryFloat {
                    return Err(OracleThinError::new(
                        "Oracle BINARY_FLOAT bind input requires BINARY_FLOAT column type",
                    ));
                }
                write_bytes_with_length(payload, &encode_oracle_binary_float(*value))
            }
            Some(BindInputValue::BinaryDouble(value)) => {
                if *column_type != OracleColumnType::BinaryDouble {
                    return Err(OracleThinError::new(
                        "Oracle BINARY_DOUBLE bind input requires BINARY_DOUBLE column type",
                    ));
                }
                write_bytes_with_length(payload, &encode_oracle_binary_double(*value))
            }
            Some(BindInputValue::Text(value)) if *column_type == OracleColumnType::Json => {
                write_json_bind_text(payload, capabilities, value)
            }
            Some(BindInputValue::Text(value)) => write_text_bind_value(
                payload,
                capabilities,
                value,
                bind_text_charset_form(*column_type),
            ),
            Some(BindInputValue::Bytes(value)) if *column_type == OracleColumnType::Json => {
                write_json_bind_raw(payload, capabilities, value)
            }
            Some(BindInputValue::Bytes(value)) => {
                write_bytes_with_length_for_capabilities(payload, value, capabilities)
            }
            Some(BindInputValue::Rowid(value)) => {
                if *column_type != OracleColumnType::Rowid {
                    return Err(OracleThinError::new(
                        "Oracle ROWID bind input requires ROWID column type",
                    ));
                }
                write_bytes_with_length_for_capabilities(payload, value.as_bytes(), capabilities)
            }
            Some(BindInputValue::Urowid(value)) => {
                if *column_type != OracleColumnType::Urowid {
                    return Err(OracleThinError::new(
                        "Oracle UROWID bind input requires UROWID column type",
                    ));
                }
                write_bytes_with_length_for_capabilities(payload, value.as_bytes(), capabilities)
            }
            Some(BindInputValue::Boolean(value)) if *column_type == OracleColumnType::Json => {
                write_json_bind_bool(payload, capabilities, *value)
            }
            Some(BindInputValue::Boolean(value)) => {
                if *value {
                    write_bytes_with_length(payload, &[1, 1])
                } else {
                    write_bytes_with_length(payload, &[0])
                }
            }
            Some(BindInputValue::Date(value)) if *column_type == OracleColumnType::Json => {
                write_json_bind_date(payload, capabilities, value)
            }
            Some(BindInputValue::Date(value)) => {
                write_bytes_with_length(payload, &encode_oracle_date(value, 7))
            }
            Some(BindInputValue::Timestamp(value)) if *column_type == OracleColumnType::Json => {
                write_json_bind_timestamp(payload, capabilities, value)
            }
            Some(BindInputValue::Timestamp(value)) => {
                write_bytes_with_length(payload, &encode_oracle_timestamp_bind(value))
            }
            Some(BindInputValue::IntervalYearMonth(value))
                if *column_type == OracleColumnType::Json =>
            {
                write_json_bind_interval_ym(payload, capabilities, value)
            }
            Some(BindInputValue::IntervalYearMonth(value)) => {
                if *column_type != OracleColumnType::IntervalYearMonth {
                    return Err(OracleThinError::new(
                        "Oracle INTERVAL YEAR TO MONTH bind input requires INTERVAL YEAR TO MONTH column type",
                    ));
                }
                let bytes = encode_oracle_interval_ym(value)?;
                write_bytes_with_length(payload, &bytes)
            }
            Some(BindInputValue::IntervalDaySecond(value))
                if *column_type == OracleColumnType::Json =>
            {
                write_json_bind_interval_ds(payload, capabilities, value)
            }
            Some(BindInputValue::IntervalDaySecond(value)) => {
                if *column_type != OracleColumnType::IntervalDaySecond {
                    return Err(OracleThinError::new(
                        "Oracle INTERVAL DAY TO SECOND bind input requires INTERVAL DAY TO SECOND column type",
                    ));
                }
                let bytes = encode_oracle_interval_ds(value)?;
                write_bytes_with_length(payload, &bytes)
            }
            Some(BindInputValue::Vector(value)) => {
                if *column_type == OracleColumnType::Json {
                    return write_json_bind_vector(payload, capabilities, value);
                }
                if *column_type != OracleColumnType::Vector {
                    return Err(OracleThinError::new(
                        "Oracle VECTOR bind input requires VECTOR column type",
                    ));
                }
                write_vector_bind_value(payload, capabilities, value)
            }
            Some(BindInputValue::LobLocator(locator)) => {
                if !matches!(
                    *column_type,
                    OracleColumnType::Blob | OracleColumnType::Clob | OracleColumnType::Nclob
                ) {
                    return Err(OracleThinError::new(
                        "Oracle LOB locator bind input requires LOB column type",
                    ));
                }
                write_bytes_with_two_lengths(payload, locator)
            }
            None if matches!(
                bind,
                BindValue::InOut {
                    column_type: OracleColumnType::Boolean,
                    ..
                }
            ) =>
            {
                write_null_boolean(payload)
            }
            None => {
                payload.push(0);
                Ok(())
            }
        },
    }
}

fn write_text_bind_value(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    value: &str,
    charset_form: u8,
) -> Result<(), OracleThinError> {
    let bytes = encode_oracle_bind_text(value, charset_form, capabilities)?;
    write_bytes_with_length_for_capabilities(payload, &bytes, capabilities)
}

fn write_json_bind_text(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    value: &str,
) -> Result<(), OracleThinError> {
    let json = serde_json::from_str::<JsonValue>(value)
        .map_err(|err| OracleThinError::new(format!("invalid JSON bind value: {err}")))?;
    let bytes = encode_oson_json(&json, capabilities.supports_oson_long_field_names)?;
    write_value_based_blob_qlocator(payload, bytes.len())?;
    write_bytes_with_length_for_capabilities(payload, &bytes, capabilities)
}

fn write_json_bind_raw(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    value: &[u8],
) -> Result<(), OracleThinError> {
    let bytes = encode_oson_raw_json(value)?;
    write_value_based_blob_qlocator(payload, bytes.len())?;
    write_bytes_with_length_for_capabilities(payload, &bytes, capabilities)
}

fn write_json_bind_id(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    value: &[u8],
) -> Result<(), OracleThinError> {
    let bytes = encode_oson_id_json(value)?;
    write_value_based_blob_qlocator(payload, bytes.len())?;
    write_bytes_with_length_for_capabilities(payload, &bytes, capabilities)
}

fn write_json_bind_bool(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    value: bool,
) -> Result<(), OracleThinError> {
    let bytes = encode_oson_bool_json(value)?;
    write_value_based_blob_qlocator(payload, bytes.len())?;
    write_bytes_with_length_for_capabilities(payload, &bytes, capabilities)
}

fn write_json_bind_number(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    value: &str,
) -> Result<(), OracleThinError> {
    let bytes = encode_oson_number_json(value)?;
    write_value_based_blob_qlocator(payload, bytes.len())?;
    write_bytes_with_length_for_capabilities(payload, &bytes, capabilities)
}

fn write_json_bind_string(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    value: &str,
) -> Result<(), OracleThinError> {
    let bytes = encode_oson_string_json(value)?;
    write_value_based_blob_qlocator(payload, bytes.len())?;
    write_bytes_with_length_for_capabilities(payload, &bytes, capabilities)
}

fn write_json_bind_date(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    value: &crate::OracleDateTime,
) -> Result<(), OracleThinError> {
    let bytes = encode_oson_date_json(value)?;
    write_value_based_blob_qlocator(payload, bytes.len())?;
    write_bytes_with_length_for_capabilities(payload, &bytes, capabilities)
}

fn write_json_bind_timestamp(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    value: &crate::OracleDateTime,
) -> Result<(), OracleThinError> {
    let bytes = encode_oson_timestamp_json(value)?;
    write_value_based_blob_qlocator(payload, bytes.len())?;
    write_bytes_with_length_for_capabilities(payload, &bytes, capabilities)
}

fn write_json_bind_interval_ym(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    value: &OracleIntervalYearMonth,
) -> Result<(), OracleThinError> {
    let bytes = encode_oson_interval_ym_json(value)?;
    write_value_based_blob_qlocator(payload, bytes.len())?;
    write_bytes_with_length_for_capabilities(payload, &bytes, capabilities)
}

fn write_json_bind_interval_ds(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    value: &OracleIntervalDaySecond,
) -> Result<(), OracleThinError> {
    let bytes = encode_oson_interval_ds_json(value)?;
    write_value_based_blob_qlocator(payload, bytes.len())?;
    write_bytes_with_length_for_capabilities(payload, &bytes, capabilities)
}

fn write_json_bind_vector(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    value: &OracleVectorValue,
) -> Result<(), OracleThinError> {
    let bytes = encode_oson_vector_json(value)?;
    write_value_based_blob_qlocator(payload, bytes.len())?;
    write_bytes_with_length_for_capabilities(payload, &bytes, capabilities)
}

fn write_vector_bind_value(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    value: &OracleVectorValue,
) -> Result<(), OracleThinError> {
    let bytes = encode_vector(value)?;
    write_value_based_blob_qlocator(payload, bytes.len())?;
    write_bytes_with_length_for_capabilities(payload, &bytes, capabilities)
}

fn write_value_based_blob_qlocator(
    payload: &mut Vec<u8>,
    data_length: usize,
) -> Result<(), OracleThinError> {
    let data_length = u64::try_from(data_length)
        .map_err(|_| OracleThinError::new("Oracle value-based LOB payload is too large"))?;
    write_ub4(payload, 40);
    payload.push(40);
    push_be_u16(payload, 38);
    push_be_u16(payload, TNS_LOB_QLOCATOR_VERSION);
    payload
        .push(TNS_LOB_LOC_FLAGS_VALUE_BASED | TNS_LOB_LOC_FLAGS_BLOB | TNS_LOB_LOC_FLAGS_ABSTRACT);
    payload.push(TNS_LOB_LOC_FLAGS_INIT);
    push_be_u16(payload, 0);
    push_be_u16(payload, 1);
    push_be_u64(payload, data_length);
    push_be_u16(payload, 0);
    push_be_u16(payload, 0);
    push_be_u16(payload, 0);
    push_be_u64(payload, 0);
    push_be_u64(payload, 0);
    Ok(())
}

fn encode_oracle_bind_text(
    value: &str,
    charset_form: u8,
    capabilities: &OracleThinCapabilities,
) -> Result<Vec<u8>, OracleThinError> {
    if charset_form == CS_FORM_NCHAR {
        return encode_oracle_nchar_text(
            value,
            capabilities.ncharset_id,
            capabilities.protocol_version,
        );
    }
    Ok(value.as_bytes().to_vec())
}

fn temp_lob_type_metadata(
    column_type: OracleColumnType,
    capabilities: &OracleThinCapabilities,
) -> (u8, u8, Option<u16>) {
    match column_type {
        OracleColumnType::Blob => (0, ORA_TYPE_NUM_BLOB, None),
        OracleColumnType::Nclob => (
            CS_FORM_NCHAR,
            ORA_TYPE_NUM_CLOB,
            Some(match capabilities.ncharset_id {
                0 => ORACLE_CHARSET_AL16UTF16,
                charset_id => charset_id,
            }),
        ),
        _ => (CS_FORM_IMPLICIT, ORA_TYPE_NUM_CLOB, Some(TNS_CHARSET_UTF8)),
    }
}

fn encode_oracle_nchar_text(
    value: &str,
    ncharset_id: u16,
    protocol_version: Option<u16>,
) -> Result<Vec<u8>, OracleThinError> {
    match ncharset_id {
        0 | ORACLE_CHARSET_AL16UTF16 => Ok(encode_utf16be_oracle_text(value)),
        ORACLE_CHARSET_UTF8 | ORACLE_CHARSET_AL32UTF8 => Ok(value.as_bytes().to_vec()),
        _ => encode_oracle_native_text(value, ncharset_id, protocol_version)?.ok_or_else(|| {
            OracleThinError::new(format!(
                "Oracle national character set id {ncharset_id} is not supported for binds"
            ))
        }),
    }
}

fn encode_utf16be_oracle_text(value: &str) -> Vec<u8> {
    value.encode_utf16().flat_map(u16::to_be_bytes).collect()
}

fn encode_utf16le_oracle_text(value: &str) -> Vec<u8> {
    value.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn encode_temp_clob_text(
    value: &str,
    locator: &[u8],
    capabilities: &OracleThinCapabilities,
) -> Result<Vec<u8>, OracleThinError> {
    if locator
        .get(TNS_LOB_LOC_OFFSET_FLAG_3)
        .is_some_and(|flag| flag & TNS_LOB_LOC_FLAGS_VAR_LENGTH_CHARSET != 0)
    {
        return Ok(
            if locator
                .get(TNS_LOB_LOC_OFFSET_FLAG_4)
                .is_some_and(|flag| flag & TNS_LOB_LOC_FLAGS_LITTLE_ENDIAN != 0)
            {
                encode_utf16le_oracle_text(value)
            } else {
                encode_utf16be_oracle_text(value)
            },
        );
    }
    encode_oracle_bind_text(value, CS_FORM_IMPLICIT, capabilities)
}

fn encode_bfile_locator(
    directory_alias: &str,
    file_name: &str,
    capabilities: &OracleThinCapabilities,
) -> Result<Vec<u8>, OracleThinError> {
    let directory_alias = encode_oracle_bind_text(directory_alias, CS_FORM_IMPLICIT, capabilities)?;
    let file_name = encode_oracle_bind_text(file_name, CS_FORM_IMPLICIT, capabilities)?;
    let total_len = 20usize
        .checked_add(directory_alias.len())
        .and_then(|len| len.checked_add(file_name.len()))
        .ok_or_else(|| OracleThinError::new("Oracle BFILE locator is too large"))?;
    let locator_len = u16::try_from(total_len.saturating_sub(2))
        .map_err(|_| OracleThinError::new("Oracle BFILE locator is too large"))?;
    let directory_len = u16::try_from(directory_alias.len())
        .map_err(|_| OracleThinError::new("Oracle BFILE directory alias is too large"))?;
    let file_name_len = u16::try_from(file_name.len())
        .map_err(|_| OracleThinError::new("Oracle BFILE file name is too large"))?;

    let mut locator = Vec::with_capacity(total_len);
    locator.extend_from_slice(&locator_len.to_be_bytes());
    locator.extend_from_slice(&[0, 1, 8, 8, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    locator.extend_from_slice(&directory_len.to_be_bytes());
    locator.extend_from_slice(&directory_alias);
    locator.extend_from_slice(&file_name_len.to_be_bytes());
    locator.extend_from_slice(&file_name);
    Ok(locator)
}

fn bind_text_charset_form(column_type: OracleColumnType) -> u8 {
    match column_type {
        OracleColumnType::Nclob => CS_FORM_NCHAR,
        _ => CS_FORM_IMPLICIT,
    }
}

fn write_null_boolean(payload: &mut Vec<u8>) -> Result<(), OracleThinError> {
    payload.push(TNS_ESCAPE_CHAR);
    payload.push(1);
    Ok(())
}

fn bind_column_metadata(bind: &BindValue) -> ThinColumn {
    let (column_type, mut max_len) = match bind {
        BindValue::Null(column_type) => (*column_type, default_bind_len(*column_type)),
        BindValue::Number(_) => (OracleColumnType::Number, 22),
        BindValue::BinaryFloat(_) => (OracleColumnType::BinaryFloat, 4),
        BindValue::BinaryDouble(_) => (OracleColumnType::BinaryDouble, 8),
        BindValue::Text(value) => (
            OracleColumnType::Varchar,
            value.len().saturating_mul(4).max(1) as u32,
        ),
        BindValue::Bytes(value) => (OracleColumnType::Raw, value.len().max(1) as u32),
        BindValue::Rowid(_) => (OracleColumnType::Rowid, TNS_MAX_ROWID_LENGTH),
        BindValue::Urowid(_) => (OracleColumnType::Urowid, TNS_MAX_UROWID_LENGTH),
        BindValue::Boolean(_) => (OracleColumnType::Boolean, 4),
        BindValue::Date(_) => (OracleColumnType::Date, 7),
        BindValue::IntervalYearMonth(_) => (OracleColumnType::IntervalYearMonth, 5),
        BindValue::IntervalDaySecond(_) => (OracleColumnType::IntervalDaySecond, 11),
        BindValue::Vector(_) => (OracleColumnType::Vector, TNS_VECTOR_MAX_LENGTH),
        BindValue::Json(_)
        | BindValue::JsonBool(_)
        | BindValue::JsonNumber(_)
        | BindValue::JsonString(_)
        | BindValue::JsonRaw(_)
        | BindValue::JsonId(_)
        | BindValue::JsonDate(_)
        | BindValue::JsonTimestamp(_)
        | BindValue::JsonIntervalYearMonth(_)
        | BindValue::JsonIntervalDaySecond(_)
        | BindValue::JsonVector(_) => (OracleColumnType::Json, TNS_JSON_MAX_LENGTH),
        BindValue::Blob(value) => (OracleColumnType::Blob, value.len().max(1) as u32),
        BindValue::Clob(value) => (
            OracleColumnType::Clob,
            value.len().saturating_mul(4).max(1) as u32,
        ),
        BindValue::Nclob(value) => (
            OracleColumnType::Nclob,
            value.len().saturating_mul(4).max(1) as u32,
        ),
        BindValue::Bfile {
            directory_alias,
            file_name,
        } => (
            OracleColumnType::Bfile,
            (20 + directory_alias.len().saturating_mul(4) + file_name.len().saturating_mul(4))
                .max(1) as u32,
        ),
        BindValue::LobLocator {
            column_type,
            locator,
        } => (*column_type, locator.len().max(1) as u32),
        BindValue::Timestamp(value) => (
            OracleColumnType::Timestamp,
            if oracle_datetime_has_timezone(value) {
                13
            } else {
                11
            },
        ),
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
    if bind_uses_timestamp_tz(bind) {
        max_len = max_len.max(13);
    }
    let mut ora_type_num = match column_type {
        OracleColumnType::Varchar | OracleColumnType::Long | OracleColumnType::Clob => {
            ORA_TYPE_NUM_VARCHAR
        }
        OracleColumnType::Number => ORA_TYPE_NUM_NUMBER,
        OracleColumnType::BinaryFloat => ORA_TYPE_NUM_BINARY_FLOAT,
        OracleColumnType::BinaryDouble => ORA_TYPE_NUM_BINARY_DOUBLE,
        OracleColumnType::Date => ORA_TYPE_NUM_DATE,
        OracleColumnType::Timestamp => ORA_TYPE_NUM_TIMESTAMP,
        OracleColumnType::Boolean => ORA_TYPE_NUM_BOOLEAN,
        OracleColumnType::Raw | OracleColumnType::Blob => ORA_TYPE_NUM_RAW,
        OracleColumnType::Rowid => ORA_TYPE_NUM_ROWID,
        OracleColumnType::Urowid => ORA_TYPE_NUM_UROWID,
        OracleColumnType::Bfile => ORA_TYPE_NUM_BFILE,
        OracleColumnType::Vector => ORA_TYPE_NUM_VECTOR,
        OracleColumnType::Json => ORA_TYPE_NUM_JSON,
        OracleColumnType::Xml => ORA_TYPE_NUM_OBJECT,
        OracleColumnType::Object => ORA_TYPE_NUM_OBJECT,
        OracleColumnType::ObjectRef => TNS_DATA_TYPE_INT_REF,
        OracleColumnType::Nclob => ORA_TYPE_NUM_VARCHAR,
        OracleColumnType::Cursor => ORA_TYPE_NUM_CURSOR,
        OracleColumnType::IntervalYearMonth => ORA_TYPE_NUM_INTERVAL_YM,
        OracleColumnType::IntervalDaySecond => ORA_TYPE_NUM_INTERVAL_DS,
        OracleColumnType::Unsupported(ora_type_num) => ora_type_num,
    };
    if bind_uses_timestamp_tz(bind) {
        ora_type_num = ORA_TYPE_NUM_TIMESTAMP_TZ;
    }
    if let Some(lob_type) = bind_lob_locator_type(bind) {
        ora_type_num = match lob_type {
            OracleColumnType::Blob => ORA_TYPE_NUM_BLOB,
            OracleColumnType::Bfile => ORA_TYPE_NUM_BFILE,
            _ => ORA_TYPE_NUM_CLOB,
        };
    }
    let charset_form = match column_type {
        OracleColumnType::Varchar | OracleColumnType::Long | OracleColumnType::Clob => {
            CS_FORM_IMPLICIT
        }
        OracleColumnType::Rowid | OracleColumnType::Urowid => CS_FORM_IMPLICIT,
        OracleColumnType::Nclob => CS_FORM_NCHAR,
        _ => 0,
    };
    ThinColumn {
        name: String::new(),
        column_type,
        ora_type_num,
        charset_form,
        buffer_size: max_len,
        schema_name: String::new(),
        type_name: String::new(),
    }
}

fn bind_uses_timestamp_tz(bind: &BindValue) -> bool {
    match bind {
        BindValue::Timestamp(value) => oracle_datetime_has_timezone(value),
        BindValue::Out {
            column_type: OracleColumnType::Timestamp,
            max_len,
        } => *max_len > 11,
        BindValue::InOut {
            column_type: OracleColumnType::Timestamp,
            max_len,
            value,
        } => {
            *max_len > 11
                || matches!(
                    value,
                    Some(BindInputValue::Timestamp(value)) if oracle_datetime_has_timezone(value)
                )
        }
        _ => false,
    }
}

fn bind_lob_locator_type(bind: &BindValue) -> Option<OracleColumnType> {
    match bind {
        BindValue::Bfile { .. } => Some(OracleColumnType::Bfile),
        BindValue::LobLocator { column_type, .. } => Some(*column_type),
        BindValue::InOut {
            column_type,
            value: Some(BindInputValue::LobLocator(_)),
            ..
        } => Some(*column_type),
        _ => None,
    }
}

fn oracle_datetime_has_timezone(value: &crate::OracleDateTime) -> bool {
    value.timezone_offset_minutes.is_some() || value.timezone_region_id.is_some()
}

fn thin_column_from_column_metadata(column: &ColumnMetadata) -> ThinColumn {
    if column.ora_type_num != 0 {
        return ThinColumn {
            name: column.name.clone(),
            column_type: column.column_type,
            ora_type_num: column.ora_type_num,
            charset_form: column.charset_form,
            buffer_size: column.buffer_size,
            schema_name: column.schema_name.clone(),
            type_name: column.type_name.clone(),
        };
    }
    let bind_like = match column.column_type {
        OracleColumnType::Varchar => BindValue::Null(OracleColumnType::Varchar),
        OracleColumnType::Number => BindValue::Null(OracleColumnType::Number),
        OracleColumnType::BinaryFloat => BindValue::Null(OracleColumnType::BinaryFloat),
        OracleColumnType::BinaryDouble => BindValue::Null(OracleColumnType::BinaryDouble),
        OracleColumnType::Date => BindValue::Null(OracleColumnType::Date),
        OracleColumnType::Timestamp => BindValue::Null(OracleColumnType::Timestamp),
        OracleColumnType::Boolean => BindValue::Null(OracleColumnType::Boolean),
        OracleColumnType::Raw => BindValue::Null(OracleColumnType::Raw),
        OracleColumnType::Rowid => BindValue::Null(OracleColumnType::Rowid),
        OracleColumnType::Urowid => BindValue::Null(OracleColumnType::Urowid),
        OracleColumnType::Long => BindValue::Null(OracleColumnType::Long),
        OracleColumnType::Clob => BindValue::Null(OracleColumnType::Clob),
        OracleColumnType::Nclob => BindValue::Null(OracleColumnType::Nclob),
        OracleColumnType::Blob => BindValue::Null(OracleColumnType::Blob),
        OracleColumnType::Bfile => BindValue::Null(OracleColumnType::Bfile),
        OracleColumnType::Vector => BindValue::Null(OracleColumnType::Vector),
        OracleColumnType::Json => BindValue::Null(OracleColumnType::Json),
        OracleColumnType::Xml => BindValue::Null(OracleColumnType::Xml),
        OracleColumnType::Object => BindValue::Null(OracleColumnType::Object),
        OracleColumnType::ObjectRef => BindValue::Null(OracleColumnType::ObjectRef),
        OracleColumnType::Cursor => BindValue::Null(OracleColumnType::Cursor),
        OracleColumnType::IntervalYearMonth => BindValue::Null(OracleColumnType::IntervalYearMonth),
        OracleColumnType::IntervalDaySecond => BindValue::Null(OracleColumnType::IntervalDaySecond),
        OracleColumnType::Unsupported(ora_type_num) => {
            BindValue::Null(OracleColumnType::Unsupported(ora_type_num))
        }
    };
    let mut thin = bind_column_metadata(&bind_like);
    thin.name = column.name.clone();
    thin.schema_name = column.schema_name.clone();
    thin.type_name = column.type_name.clone();
    if column.charset_form != 0 {
        thin.charset_form = column.charset_form;
    }
    if column.column_type == OracleColumnType::Long {
        thin.ora_type_num = ORA_TYPE_NUM_LONG;
    }
    thin
}

fn define_column_metadata(column: &ColumnMetadata) -> ThinColumn {
    let mut thin = thin_column_from_column_metadata(column);
    if column.ora_type_num == ORA_TYPE_NUM_LONG_RAW {
        thin.ora_type_num = ORA_TYPE_NUM_LONG_RAW;
        thin.buffer_size = TNS_MAX_LONG_LENGTH;
        thin.charset_form = 0;
        thin.column_type = OracleColumnType::Raw;
        return thin;
    }
    match column.column_type {
        OracleColumnType::Long => {
            thin.ora_type_num = ORA_TYPE_NUM_LONG;
            thin.buffer_size = TNS_MAX_LONG_LENGTH;
            thin.charset_form = CS_FORM_IMPLICIT;
        }
        OracleColumnType::Clob => {
            thin.ora_type_num = ORA_TYPE_NUM_LONG;
            thin.buffer_size = TNS_MAX_LONG_LENGTH;
            thin.charset_form = CS_FORM_IMPLICIT;
            thin.column_type = OracleColumnType::Long;
        }
        OracleColumnType::Nclob => {
            thin.ora_type_num = ORA_TYPE_NUM_LONG;
            thin.buffer_size = TNS_MAX_LONG_LENGTH;
            thin.charset_form = CS_FORM_NCHAR;
            thin.column_type = OracleColumnType::Long;
        }
        OracleColumnType::Blob => {
            thin.ora_type_num = ORA_TYPE_NUM_LONG_RAW;
            thin.buffer_size = TNS_MAX_LONG_LENGTH;
            thin.column_type = OracleColumnType::Raw;
        }
        OracleColumnType::Bfile => {}
        OracleColumnType::Vector => {
            thin.ora_type_num = ORA_TYPE_NUM_VECTOR;
            thin.buffer_size = TNS_VECTOR_MAX_LENGTH;
        }
        OracleColumnType::Json => {
            thin.ora_type_num = ORA_TYPE_NUM_JSON;
            thin.buffer_size = TNS_JSON_MAX_LENGTH;
        }
        OracleColumnType::Xml => {
            thin.ora_type_num = ORA_TYPE_NUM_OBJECT;
        }
        OracleColumnType::Object => {
            thin.ora_type_num = ORA_TYPE_NUM_OBJECT;
        }
        OracleColumnType::ObjectRef => {
            thin.ora_type_num = TNS_DATA_TYPE_INT_REF;
        }
        _ => {}
    }
    thin
}

fn define_column_metadata_for_capabilities(
    column: &ColumnMetadata,
    _capabilities: &OracleThinCapabilities,
) -> ThinColumn {
    define_column_metadata(column)
}

fn define_thin_column_metadata_for_capabilities(
    column: &ThinColumn,
    _capabilities: &OracleThinCapabilities,
) -> ThinColumn {
    let mut thin = column.clone();
    if column.ora_type_num == ORA_TYPE_NUM_LONG_RAW {
        thin.ora_type_num = ORA_TYPE_NUM_LONG_RAW;
        thin.buffer_size = TNS_MAX_LONG_LENGTH;
        thin.charset_form = 0;
        thin.column_type = OracleColumnType::Raw;
        return thin;
    }
    match column.column_type {
        OracleColumnType::Long => {
            thin.ora_type_num = ORA_TYPE_NUM_LONG;
            thin.buffer_size = TNS_MAX_LONG_LENGTH;
            thin.charset_form = CS_FORM_IMPLICIT;
        }
        OracleColumnType::Clob => {
            thin.ora_type_num = ORA_TYPE_NUM_LONG;
            thin.buffer_size = TNS_MAX_LONG_LENGTH;
            thin.charset_form = CS_FORM_IMPLICIT;
            thin.column_type = OracleColumnType::Long;
        }
        OracleColumnType::Nclob => {
            thin.ora_type_num = ORA_TYPE_NUM_LONG;
            thin.buffer_size = TNS_MAX_LONG_LENGTH;
            thin.charset_form = CS_FORM_NCHAR;
            thin.column_type = OracleColumnType::Long;
        }
        OracleColumnType::Blob => {
            thin.ora_type_num = ORA_TYPE_NUM_LONG_RAW;
            thin.buffer_size = TNS_MAX_LONG_LENGTH;
            thin.column_type = OracleColumnType::Raw;
        }
        OracleColumnType::Vector => {
            thin.ora_type_num = ORA_TYPE_NUM_VECTOR;
            thin.buffer_size = TNS_VECTOR_MAX_LENGTH;
        }
        OracleColumnType::Json => {
            thin.ora_type_num = ORA_TYPE_NUM_JSON;
            thin.buffer_size = TNS_JSON_MAX_LENGTH;
        }
        OracleColumnType::Xml => {
            thin.ora_type_num = ORA_TYPE_NUM_OBJECT;
        }
        OracleColumnType::Object => {
            thin.ora_type_num = ORA_TYPE_NUM_OBJECT;
        }
        OracleColumnType::ObjectRef => {
            thin.ora_type_num = TNS_DATA_TYPE_INT_REF;
        }
        _ => {}
    }
    thin
}

fn fetch_state_column_metadata(
    column: &ColumnMetadata,
    _capabilities: &OracleThinCapabilities,
) -> ThinColumn {
    thin_column_from_column_metadata(column)
}

fn default_bind_len(column_type: OracleColumnType) -> u32 {
    match column_type {
        OracleColumnType::Varchar | OracleColumnType::Long | OracleColumnType::Clob => 4000,
        OracleColumnType::Number => 22,
        OracleColumnType::BinaryFloat => 4,
        OracleColumnType::BinaryDouble => 8,
        OracleColumnType::Date => 7,
        OracleColumnType::Timestamp => 11,
        OracleColumnType::Boolean => 4,
        OracleColumnType::Raw | OracleColumnType::Blob => 2000,
        OracleColumnType::Rowid => TNS_MAX_ROWID_LENGTH,
        OracleColumnType::Urowid => TNS_MAX_UROWID_LENGTH,
        OracleColumnType::Bfile => 1,
        OracleColumnType::Vector => TNS_VECTOR_MAX_LENGTH,
        OracleColumnType::Json => TNS_JSON_MAX_LENGTH,
        OracleColumnType::Xml => TNS_MAX_LONG_LENGTH,
        OracleColumnType::Object => 1,
        OracleColumnType::ObjectRef => 2000,
        OracleColumnType::Nclob => 4000,
        OracleColumnType::Cursor => 4,
        OracleColumnType::IntervalYearMonth => 5,
        OracleColumnType::IntervalDaySecond => 11,
        OracleColumnType::Unsupported(_) => 1,
    }
}

fn request_with_out_bind_types(
    request: &StatementRequest,
    bind_types: &[OracleColumnType],
) -> StatementRequest {
    let mut request = request.clone();
    request.binds = bind_types
        .iter()
        .map(|column_type| BindValue::Out {
            column_type: *column_type,
            max_len: default_bind_len(*column_type),
        })
        .collect();
    request
}

fn write_oracle_number(payload: &mut Vec<u8>, value: &str) -> Result<(), OracleThinError> {
    let bytes = encode_oracle_number(value)?;
    write_bytes_with_length(payload, &bytes)
}

fn encode_oracle_timestamp_bind(value: &crate::OracleDateTime) -> Vec<u8> {
    if oracle_datetime_has_timezone(value) {
        encode_oracle_date(value, 13)
    } else {
        encode_oracle_date(value, 11)
    }
}

fn encode_oracle_date(value: &crate::OracleDateTime, length: usize) -> Vec<u8> {
    let mut encoded = *value;
    let timezone_bytes = if length > 11 {
        let timezone_bytes = oracle_timestamp_timezone_bytes(value);
        if timezone_bytes.is_some() {
            if let Some(offset) = value.timezone_offset_minutes {
                apply_timezone_offset(&mut encoded, -offset);
            } else if let Some(region_id) = value.timezone_region_id {
                if let Some(offset) = timezone_region_local_offset_minutes(region_id, value) {
                    apply_timezone_offset(&mut encoded, -offset);
                }
            }
        }
        timezone_bytes
    } else {
        None
    };
    let mut bytes = vec![
        (encoded.year / 100) as u8 + 100,
        (encoded.year % 100) as u8 + 100,
        encoded.month,
        encoded.day,
        encoded.hour + 1,
        encoded.minute + 1,
        encoded.second + 1,
    ];
    if length > 7 {
        bytes.extend_from_slice(&encoded.nanosecond.to_be_bytes());
    }
    if let Some([zone_1, zone_2]) = timezone_bytes {
        bytes.push(zone_1);
        bytes.push(zone_2);
    }
    bytes
}

fn oracle_timestamp_timezone_bytes(value: &crate::OracleDateTime) -> Option<[u8; 2]> {
    if let Some(offset) = value.timezone_offset_minutes {
        let hour = offset / 60;
        let minute = offset % 60;
        return Some([(hour + 20) as u8, (minute + 60) as u8]);
    }
    let region_id = value.timezone_region_id?;
    Some([
        ((region_id & 0x1fc0) >> 6) as u8 | 0x80,
        ((region_id & 0x003f) << 2) as u8,
    ])
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
    server_state: &mut ServerSidePiggybackState,
    skip_empty_end_of_response: bool,
) -> Result<ExecuteResponse, OracleThinError> {
    read_execute_response_with_state(
        stream,
        capabilities,
        request,
        server_state,
        ExecuteReadState::default(),
        skip_empty_end_of_response,
    )
}

fn read_execute_response_with_state(
    stream: &mut TcpStream,
    capabilities: &OracleThinCapabilities,
    request: &StatementRequest,
    server_state: &mut ServerSidePiggybackState,
    mut state: ExecuteReadState,
    mut skip_empty_end_of_response: bool,
) -> Result<ExecuteResponse, OracleThinError> {
    server_state.last_warning = None;
    if request_is_dml_returning(request) && state.out_bind_columns.is_empty() {
        state.out_bind_columns = request
            .binds
            .iter()
            .filter(|bind| bind_can_return_value(bind))
            .map(bind_column_metadata)
            .collect();
        state.reading_out_binds = !state.out_bind_columns.is_empty();
        state.reading_dml_returning = state.reading_out_binds;
    }
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
        let mut cursor = PacketCursor::with_capabilities(&packet, capabilities);
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
                        capabilities.data_packet_chunk_size(),
                        &[TNS_MSG_TYPE_FLUSH_OUT_BINDS],
                    ),
                    TNS_MSG_TYPE_DESCRIBE_INFO => {
                        process_describe_info(&mut cursor, capabilities, &mut state)
                    }
                    TNS_MSG_TYPE_BIT_VECTOR => process_bit_vector(&mut cursor, &mut state),
                    TNS_MSG_TYPE_IMPLICIT_RESULTSET => {
                        process_implicit_results(&mut cursor, capabilities, &mut state)
                    }
                    TNS_MSG_TYPE_PARAMETER => {
                        process_return_parameters(&mut cursor, capabilities, server_state)
                    }
                    TNS_MSG_TYPE_STATUS => {
                        process_status(&mut cursor, capabilities, server_state)?;
                        if !capabilities.supports_end_of_response {
                            state.done = true;
                        }
                        Ok(())
                    }
                    TNS_MSG_TYPE_TOKEN => process_token(&mut cursor, TNS_DEFAULT_TOKEN_NUM),
                    TNS_MSG_TYPE_WARNING => {
                        if let Some(warning) = process_warning(&mut cursor, capabilities)? {
                            server_state.last_warning = Some(warning);
                        }
                        Ok(())
                    }
                    TNS_MSG_TYPE_ERROR => {
                        let error = process_execute_error(&mut cursor, capabilities)?;
                        update_transaction_status_from_call_status(
                            server_state,
                            capabilities,
                            error.call_status,
                        );
                        if let Some(warning) = error.warning.clone() {
                            server_state.last_warning = Some(warning);
                        }
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
                        process_server_side_piggyback(&mut cursor, capabilities, server_state)
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

    let thin_columns = state.columns.clone();
    let columns = state
        .columns
        .iter()
        .map(column_metadata_from_thin)
        .collect();
    Ok(ExecuteResponse {
        columns,
        thin_columns,
        result: QueryResult {
            cursor_id: state.cursor_id.filter(|_| !state.exhausted),
            exhausted: state.exhausted || !request.is_query,
            rows: state.rows,
        },
        out_bind_rows: state.out_bind_rows,
        implicit_results: state.implicit_results,
        cursor_columns: state.cursor_columns,
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
    server_state: &mut ServerSidePiggybackState,
    mut skip_empty_end_of_response: bool,
) -> Result<(), OracleThinError> {
    server_state.last_warning = None;
    let mut done = false;
    let mut response_had_content = false;
    while !done {
        let (data_flags, packet) =
            read_data_packet_with_flags(stream, capabilities.protocol_version.unwrap_or(319))?;
        let mut cursor = PacketCursor::with_capabilities(&packet, capabilities);
        let mut skipped_empty_end_of_response = false;
        while cursor.remaining() > 0 && !done {
            let message_type = cursor.read_u8()?;
            if message_type != TNS_MSG_TYPE_END_OF_RESPONSE {
                response_had_content = true;
            }
            match message_type {
                TNS_MSG_TYPE_STATUS => {
                    process_status(&mut cursor, capabilities, server_state)?;
                    if !capabilities.supports_end_of_response {
                        done = true;
                    }
                }
                TNS_MSG_TYPE_ERROR => {
                    let error = process_execute_error(&mut cursor, capabilities)?;
                    update_transaction_status_from_call_status(
                        server_state,
                        capabilities,
                        error.call_status,
                    );
                    if let Some(warning) = error.warning.clone() {
                        server_state.last_warning = Some(warning);
                    }
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
                    process_token(&mut cursor, TNS_DEFAULT_TOKEN_NUM)?;
                }
                TNS_MSG_TYPE_WARNING => {
                    if let Some(warning) = process_warning(&mut cursor, capabilities)? {
                        server_state.last_warning = Some(warning);
                    }
                }
                TNS_MSG_TYPE_PARAMETER => {
                    process_return_parameters(&mut cursor, capabilities, server_state)?
                }
                TNS_MSG_TYPE_SERVER_SIDE_PIGGYBACK => {
                    process_server_side_piggyback(&mut cursor, capabilities, server_state)?
                }
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

fn read_lob_operation_response(
    stream: &mut TcpStream,
    capabilities: &OracleThinCapabilities,
    server_state: &mut ServerSidePiggybackState,
    locator_len: usize,
    read_amount: bool,
    read_create_temp_tail: bool,
) -> Result<LobReadResponse, OracleThinError> {
    server_state.last_warning = None;
    let mut done = false;
    let mut response_had_content = false;
    let mut data = Vec::new();
    let mut amount = None;
    let mut locator = None;
    let mut pending_fragment = Vec::new();
    let mut pending_fragment_error = None;
    while !done {
        let (data_flags, packet) =
            read_data_packet_with_flags(stream, capabilities.protocol_version.unwrap_or(319))?;
        if std::env::var_os("ORACLE_THIN_TRACE_EXEC").is_some() {
            eprintln!(
                "thin lob response data_flags=0x{data_flags:04x} packet={}",
                hex_encode_upper(&packet)
            );
        }
        let packet = if pending_fragment.is_empty() {
            packet
        } else {
            pending_fragment.extend_from_slice(&packet);
            std::mem::take(&mut pending_fragment)
        };
        let mut cursor = PacketCursor::with_capabilities(&packet, capabilities);
        while cursor.remaining() > 0 && !done {
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
                    "thin lob response message type={} offset={} remaining={}",
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
                    TNS_MSG_TYPE_LOB_DATA => {
                        if let Some(bytes) = cursor.read_bytes()? {
                            data.extend_from_slice(&bytes);
                        }
                        Ok(())
                    }
                    TNS_MSG_TYPE_PARAMETER => {
                        let params = process_lob_return_parameters(
                            &mut cursor,
                            locator_len,
                            read_amount,
                            read_create_temp_tail,
                        )?;
                        if params.locator.is_some() {
                            locator = params.locator;
                        }
                        if params.amount.is_some() {
                            amount = params.amount;
                        }
                        Ok(())
                    }
                    TNS_MSG_TYPE_STATUS => {
                        process_status(&mut cursor, capabilities, server_state)?;
                        if !capabilities.supports_end_of_response {
                            done = true;
                        }
                        Ok(())
                    }
                    TNS_MSG_TYPE_ERROR => {
                        let error = process_execute_error(&mut cursor, capabilities)?;
                        update_transaction_status_from_call_status(
                            server_state,
                            capabilities,
                            error.call_status,
                        );
                        if let Some(warning) = error.warning.clone() {
                            server_state.last_warning = Some(warning);
                        }
                        if error.code != 0 {
                            Err(OracleThinError::new(error.message.unwrap_or_else(|| {
                                format!("Oracle error ORA-{:05}", error.code)
                            })))
                        } else {
                            if !capabilities.supports_end_of_response {
                                done = true;
                            }
                            Ok(())
                        }
                    }
                    TNS_MSG_TYPE_TOKEN => process_token(&mut cursor, TNS_DEFAULT_TOKEN_NUM),
                    TNS_MSG_TYPE_WARNING => {
                        if let Some(warning) = process_warning(&mut cursor, capabilities)? {
                            server_state.last_warning = Some(warning);
                        }
                        Ok(())
                    }
                    TNS_MSG_TYPE_SERVER_SIDE_PIGGYBACK => {
                        process_server_side_piggyback(&mut cursor, capabilities, server_state)
                    }
                    TNS_MSG_TYPE_END_OF_RESPONSE => {
                        if response_had_content {
                            done = true;
                        }
                        Ok(())
                    }
                    other => Err(OracleThinError::new(format!(
                        "unexpected Oracle LOB response message type {other}"
                    ))),
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
                    OracleThinError::new("incomplete Oracle LOB response at end of response")
                }));
            }
            continue;
        }
        if has_end_flag {
            done = true;
        }
    }
    Ok(LobReadResponse {
        data,
        amount,
        locator,
    })
}

#[derive(Debug, Default)]
struct LobReturnParameters {
    locator: Option<Vec<u8>>,
    amount: Option<i64>,
}

fn process_lob_return_parameters(
    cursor: &mut PacketCursor<'_>,
    locator_len: usize,
    read_amount: bool,
    read_create_temp_tail: bool,
) -> Result<LobReturnParameters, OracleThinError> {
    let mut params = LobReturnParameters::default();
    if locator_len > 0 {
        params.locator = Some(cursor.read_raw(locator_len)?.to_vec());
    }
    if read_create_temp_tail {
        let _ = cursor.read_ub2()?;
        let _ = cursor.read_u8()?;
    }
    if read_amount {
        params.amount = Some(cursor.read_sb8()?);
    }
    Ok(params)
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
    if capabilities.ttc_field_version >= 3 {
        let _ = cursor.read_ub4()?;
        let _ = cursor.read_ub4()?;
    }
    if capabilities.ttc_field_version >= 4 {
        let _ = cursor.read_ub4()?;
        let _ = cursor.read_ub4()?;
    }
    if capabilities.ttc_field_version >= 5 {
        cursor.skip_bytes_with_ub4_length()?;
    }
    state.columns = columns;
    Ok(())
}

fn adjust_columns_after_define(previous_columns: &[ThinColumn], columns: &mut [ThinColumn]) {
    for (previous, column) in previous_columns.iter().zip(columns.iter_mut()) {
        match (previous.ora_type_num, column.ora_type_num) {
            (ORA_TYPE_NUM_CHAR | ORA_TYPE_NUM_LONG | ORA_TYPE_NUM_VARCHAR, ORA_TYPE_NUM_CLOB) => {
                column.ora_type_num = ORA_TYPE_NUM_LONG;
                column.column_type = OracleColumnType::Long;
                column.charset_form = previous.charset_form;
            }
            (ORA_TYPE_NUM_RAW | ORA_TYPE_NUM_LONG_RAW, ORA_TYPE_NUM_BLOB) => {
                column.ora_type_num = ORA_TYPE_NUM_LONG_RAW;
                column.column_type = OracleColumnType::Raw;
            }
            _ => {}
        }
    }
}

fn is_xmltype_metadata(ora_type_num: u8, schema_name: &str, type_name: &str) -> bool {
    matches!(
        ora_type_num,
        ORA_TYPE_NUM_OBJECT | TNS_DATA_TYPE_EXT_NAMED | TNS_DATA_TYPE_PNTY
    ) && type_name.eq_ignore_ascii_case("XMLTYPE")
        && (schema_name.is_empty() || schema_name.eq_ignore_ascii_case("SYS"))
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
        state
            .cursor_columns
            .push((cursor_id, child_state.columns.clone()));
        let columns = child_state
            .columns
            .iter()
            .map(column_metadata_from_thin)
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
    let _ = cursor.read_bytes_with_ub4_length()?;
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
    let schema_name = cursor.read_str_with_length()?.unwrap_or_default();
    let type_name = cursor.read_str_with_length()?.unwrap_or_default();
    let _ = cursor.read_ub2()?;
    let uds_flags = cursor.read_ub4()?;
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
    let column_type = if is_xmltype_metadata(ora_type_num, &schema_name, &type_name) {
        OracleColumnType::Xml
    } else if uds_flags & (TNS_UDS_FLAGS_IS_JSON | TNS_UDS_FLAGS_IS_OSON) != 0 {
        OracleColumnType::Json
    } else {
        oracle_column_type_from_ora_type_for_protocol(ora_type_num, capabilities.protocol_version)
    };
    Ok(ThinColumn {
        name,
        column_type,
        charset_form: normalize_metadata_charset_form(column_type, charset_form),
        ora_type_num,
        buffer_size,
        schema_name,
        type_name,
    })
}

fn normalize_metadata_charset_form(column_type: OracleColumnType, charset_form: u8) -> u8 {
    match column_type {
        OracleColumnType::Varchar
        | OracleColumnType::Long
        | OracleColumnType::Clob
        | OracleColumnType::Nclob => charset_form,
        _ => 0,
    }
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
    if state.reading_dml_returning {
        return process_dml_returning_row_data(cursor, capabilities, state);
    }
    let reading_out_binds = state.reading_out_binds;
    let columns = if reading_out_binds {
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
                reading_out_binds,
                &mut state.cursor_columns,
                &state.object_attrs_by_type,
                &state.collection_element_by_type,
            )?);
        }
    }
    state.last_row = Some(row.clone());
    if reading_out_binds {
        state.out_bind_rows.push(row);
    } else {
        state.rows.push(row);
    }
    state.bit_vector = None;
    Ok(())
}

fn process_dml_returning_row_data(
    cursor: &mut PacketCursor<'_>,
    capabilities: &OracleThinCapabilities,
    state: &mut ExecuteReadState,
) -> Result<(), OracleThinError> {
    let mut returned_by_column = Vec::with_capacity(state.out_bind_columns.len());
    let mut max_rows = 0usize;
    for column in &state.out_bind_columns {
        let num_rows = cursor.read_ub4()? as usize;
        max_rows = max_rows.max(num_rows);
        let mut values = Vec::with_capacity(num_rows);
        for _ in 0..num_rows {
            values.push(read_column_value(
                cursor,
                capabilities,
                column,
                true,
                &mut state.cursor_columns,
                &state.object_attrs_by_type,
                &state.collection_element_by_type,
            )?);
        }
        returned_by_column.push(values);
    }

    for row_index in 0..max_rows {
        let row = returned_by_column
            .iter()
            .map(|values| values.get(row_index).cloned().unwrap_or(OracleValue::Null))
            .collect();
        state.out_bind_rows.push(row);
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
    cursor_columns: &mut Vec<(u32, Vec<ThinColumn>)>,
    object_attrs_by_type: &HashMap<(String, String), Vec<ThinColumn>>,
    collection_element_by_type: &HashMap<(String, String), ThinColumn>,
) -> Result<OracleValue, OracleThinError> {
    let value = if column.buffer_size == 0
        && !matches!(
            column.ora_type_num,
            ORA_TYPE_NUM_LONG
                | ORA_TYPE_NUM_LONG_RAW
                | ORA_TYPE_NUM_UROWID
                | ORA_TYPE_NUM_BFILE
                | ORA_TYPE_NUM_DBFILE
                | ORA_TYPE_NUM_VECTOR
        ) {
        OracleValue::Null
    } else {
        match column.ora_type_num {
            ORA_TYPE_NUM_NUMBER
            | TNS_DATA_TYPE_BINARY_INTEGER
            | TNS_DATA_TYPE_FLOAT
            | TNS_DATA_TYPE_VNU
            | TNS_DATA_TYPE_PDN
            | TNS_DATA_TYPE_UIN
            | TNS_DATA_TYPE_SLS
            | TNS_DATA_TYPE_DTR
            | TNS_DATA_TYPE_DUN
            | TNS_DATA_TYPE_DOP
            | TNS_DATA_TYPE_DOL => {
                let Some(bytes) = cursor.read_bytes()? else {
                    return finish_column_value(
                        cursor,
                        OracleValue::Null,
                        out_bind,
                        column.ora_type_num,
                    );
                };
                OracleValue::Number(decode_oracle_number(&bytes)?)
            }
            TNS_DATA_TYPE_UB8 => {
                let Some(bytes) = cursor.read_bytes()? else {
                    return finish_column_value(
                        cursor,
                        OracleValue::Null,
                        out_bind,
                        column.ora_type_num,
                    );
                };
                OracleValue::Number(decode_oracle_unsigned_integer(&bytes)?.to_string())
            }
            ORA_TYPE_NUM_LONG => {
                let value = match cursor.read_bytes()? {
                    Some(bytes) => OracleValue::Text(decode_oracle_text(
                        &bytes,
                        column.charset_form,
                        capabilities,
                    )?),
                    None => OracleValue::Null,
                };
                if !out_bind {
                    let _ = cursor.read_sb4()?;
                    let _ = cursor.read_ub4()?;
                    return Ok(value);
                }
                value
            }
            TNS_DATA_TYPE_VBI
                if protocol_uses_go_ora_legacy_mappings(capabilities.protocol_version) =>
            {
                cursor
                    .read_bytes()?
                    .map(OracleValue::Bytes)
                    .unwrap_or(OracleValue::Null)
            }
            ORA_TYPE_NUM_VARCHAR | TNS_DATA_TYPE_STR | TNS_DATA_TYPE_VCS | TNS_DATA_TYPE_VBI
            | TNS_DATA_TYPE_LVC | TNS_DATA_TYPE_VST | TNS_DATA_TYPE_CLV | ORA_TYPE_NUM_CHAR
            | TNS_DATA_TYPE_CHARZ => {
                let Some(bytes) = cursor.read_bytes()? else {
                    return finish_column_value(
                        cursor,
                        OracleValue::Null,
                        out_bind,
                        column.ora_type_num,
                    );
                };
                OracleValue::Text(decode_oracle_text(
                    &bytes,
                    column.charset_form,
                    capabilities,
                )?)
            }
            TNS_DATA_TYPE_TIME | TNS_DATA_TYPE_ETIME => {
                let Some(bytes) = cursor.read_bytes()? else {
                    return finish_column_value(
                        cursor,
                        OracleValue::Null,
                        out_bind,
                        column.ora_type_num,
                    );
                };
                OracleValue::Text(decode_oracle_time_text(&bytes)?)
            }
            TNS_DATA_TYPE_TIME_TZ | TNS_DATA_TYPE_ETTZ => {
                let Some(bytes) = cursor.read_bytes()? else {
                    return finish_column_value(
                        cursor,
                        OracleValue::Null,
                        out_bind,
                        column.ora_type_num,
                    );
                };
                OracleValue::Text(decode_oracle_time_text(&bytes)?)
            }
            ORA_TYPE_NUM_DATE | TNS_DATA_TYPE_ODT | TNS_DATA_TYPE_EDATE => {
                let Some(bytes) = cursor.read_bytes()? else {
                    return finish_column_value(
                        cursor,
                        OracleValue::Null,
                        out_bind,
                        column.ora_type_num,
                    );
                };
                OracleValue::DateTime(decode_oracle_datetime(&bytes)?)
            }
            ORA_TYPE_NUM_TIMESTAMP
            | ORA_TYPE_NUM_TIMESTAMP_TZ
            | ORA_TYPE_NUM_TIMESTAMP_DTY
            | ORA_TYPE_NUM_TIMESTAMP_TZ_EXT
            | ORA_TYPE_NUM_TIMESTAMP_LTZ
            | TNS_DATA_TYPE_ESITZ => {
                let Some(bytes) = cursor.read_bytes()? else {
                    return finish_column_value(
                        cursor,
                        OracleValue::Null,
                        out_bind,
                        column.ora_type_num,
                    );
                };
                OracleValue::Timestamp(decode_oracle_datetime(&bytes)?)
            }
            ORA_TYPE_NUM_RAW | TNS_DATA_TYPE_LVB => cursor
                .read_bytes()?
                .map(OracleValue::Bytes)
                .unwrap_or(OracleValue::Null),
            ORA_TYPE_NUM_LONG_RAW => {
                let value = match cursor.read_bytes()? {
                    Some(bytes) if column.column_type == OracleColumnType::Json => {
                        decode_json_payload_value(&bytes)?
                    }
                    Some(bytes) => OracleValue::Bytes(bytes),
                    None => OracleValue::Null,
                };
                if !out_bind {
                    let _ = cursor.read_sb4()?;
                    let _ = cursor.read_ub4()?;
                    return Ok(value);
                }
                value
            }
            ORA_TYPE_NUM_BOOLEAN => read_boolean_value(cursor)?,
            ORA_TYPE_NUM_CLOB | TNS_DATA_TYPE_DCLOB | ORA_TYPE_NUM_BLOB | TNS_DATA_TYPE_DBLOB => {
                if out_bind {
                    read_lob_with_length(cursor, column)?
                } else {
                    cursor
                        .read_bytes()?
                        .map(OracleValue::Lob)
                        .unwrap_or(OracleValue::Null)
                }
            }
            ORA_TYPE_NUM_JSON => read_json_value(cursor)?,
            ORA_TYPE_NUM_DJSON => read_json_value(cursor)?,
            ORA_TYPE_NUM_OBJECT | TNS_DATA_TYPE_EXT_NAMED | TNS_DATA_TYPE_PNTY
                if column.column_type == OracleColumnType::Xml =>
            {
                read_xmltype_value(cursor, capabilities)?
            }
            ORA_TYPE_NUM_OBJECT | TNS_DATA_TYPE_EXT_NAMED | TNS_DATA_TYPE_PNTY => {
                read_object_value(
                    cursor,
                    capabilities,
                    column,
                    object_attrs_by_type,
                    collection_element_by_type,
                )?
            }
            TNS_DATA_TYPE_EXT_REF | TNS_DATA_TYPE_INT_REF => cursor
                .read_bytes()?
                .map(OracleValue::Bytes)
                .unwrap_or(OracleValue::Null),
            ORA_TYPE_NUM_VECTOR => read_vector_value(cursor)?,
            ORA_TYPE_NUM_BFILE | TNS_DATA_TYPE_CFILE | ORA_TYPE_NUM_DBFILE => {
                read_bfile_locator(cursor)?
            }
            ORA_TYPE_NUM_ROWID | TNS_DATA_TYPE_RDD if !out_bind => read_rowid_value(cursor)?,
            ORA_TYPE_NUM_UROWID if !out_bind => read_urowid_value(cursor)?,
            ORA_TYPE_NUM_ROWID | TNS_DATA_TYPE_RDD | ORA_TYPE_NUM_UROWID => {
                let Some(bytes) = cursor.read_bytes()? else {
                    return finish_column_value(
                        cursor,
                        OracleValue::Null,
                        out_bind,
                        column.ora_type_num,
                    );
                };
                OracleValue::Text(decode_oracle_text(&bytes, CS_FORM_IMPLICIT, capabilities)?)
            }
            ORA_TYPE_NUM_BINARY_FLOAT | TNS_DATA_TYPE_BFLOAT => {
                let Some(bytes) = cursor.read_bytes()? else {
                    return finish_column_value(
                        cursor,
                        OracleValue::Null,
                        out_bind,
                        column.ora_type_num,
                    );
                };
                OracleValue::Number(decode_oracle_binary_float(&bytes)?)
            }
            ORA_TYPE_NUM_BINARY_DOUBLE | TNS_DATA_TYPE_BDOUBLE => {
                let Some(bytes) = cursor.read_bytes()? else {
                    return finish_column_value(
                        cursor,
                        OracleValue::Null,
                        out_bind,
                        column.ora_type_num,
                    );
                };
                OracleValue::Number(decode_oracle_binary_double(&bytes)?)
            }
            ORA_TYPE_NUM_INTERVAL_YM | ORA_TYPE_NUM_INTERVAL_YM_DTY => {
                let Some(bytes) = cursor.read_bytes()? else {
                    return finish_column_value(
                        cursor,
                        OracleValue::Null,
                        out_bind,
                        column.ora_type_num,
                    );
                };
                OracleValue::Text(decode_oracle_interval_ym(&bytes)?)
            }
            ORA_TYPE_NUM_INTERVAL_DS | ORA_TYPE_NUM_INTERVAL_DS_DTY => {
                let Some(bytes) = cursor.read_bytes()? else {
                    return finish_column_value(
                        cursor,
                        OracleValue::Null,
                        out_bind,
                        column.ora_type_num,
                    );
                };
                OracleValue::Text(decode_oracle_interval_ds(&bytes)?)
            }
            ORA_TYPE_NUM_CURSOR | TNS_DATA_TYPE_RSET => {
                let _ = cursor.read_u8()?;
                let mut child_state = ExecuteReadState::default();
                process_describe_body(cursor, capabilities, &mut child_state)?;
                let cursor_id = cursor.read_ub2()? as u32;
                cursor_columns.push((cursor_id, child_state.columns.clone()));
                let columns = child_state
                    .columns
                    .into_iter()
                    .map(|column| column_metadata_from_thin(&column))
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
    finish_column_value(cursor, value, out_bind, column.ora_type_num)
}

fn finish_column_value(
    cursor: &mut PacketCursor<'_>,
    value: OracleValue,
    out_bind: bool,
    ora_type_num: u8,
) -> Result<OracleValue, OracleThinError> {
    if out_bind {
        let actual_num_bytes = cursor.read_sb4()?;
        if actual_num_bytes < 0 && ora_type_num == ORA_TYPE_NUM_BOOLEAN {
            return Ok(OracleValue::Null);
        }
        if actual_num_bytes != 0 && !matches!(value, OracleValue::Null) {
            return Err(OracleThinError::new(format!(
                "Oracle OUT bind value truncated: actual length {actual_num_bytes}"
            )));
        }
    }
    Ok(value)
}

fn read_boolean_value(cursor: &mut PacketCursor<'_>) -> Result<OracleValue, OracleThinError> {
    let len = cursor.read_u8()?;
    match len {
        0 | 0xff => Ok(OracleValue::Null),
        TNS_LEGACY_NULL_LENGTH_INDICATOR if cursor.legacy_null_clr => Ok(OracleValue::Null),
        TNS_ESCAPE_CHAR => {
            let marker = cursor.read_u8()?;
            if marker == 1 {
                Ok(OracleValue::Null)
            } else {
                Err(OracleThinError::new(format!(
                    "unsupported Oracle BOOLEAN escape marker {marker}"
                )))
            }
        }
        0xfe => {
            let mut out = Vec::new();
            loop {
                let chunk_len = if cursor.big_clr_chunks {
                    cursor.read_ub4()? as usize
                } else {
                    cursor.read_u8()? as usize
                };
                if chunk_len == 0 {
                    break;
                }
                out.extend_from_slice(cursor.read_raw(chunk_len)?);
            }
            Ok(OracleValue::Boolean(out == [1, 1]))
        }
        len => Ok(OracleValue::Boolean(
            cursor.read_raw(usize::from(len))? == [1, 1],
        )),
    }
}

fn read_rowid_value(cursor: &mut PacketCursor<'_>) -> Result<OracleValue, OracleThinError> {
    let num_bytes = cursor.read_u8()?;
    if num_bytes == 0
        || num_bytes == 0xff
        || (cursor.legacy_null_clr && num_bytes == TNS_LEGACY_NULL_LENGTH_INDICATOR)
    {
        return Ok(OracleValue::Null);
    }
    encode_physical_rowid(
        cursor.read_ub4()?,
        cursor.read_ub2()?,
        {
            cursor.skip(1)?;
            cursor.read_ub4()?
        },
        cursor.read_ub2()?,
    )
}

fn read_urowid_value(cursor: &mut PacketCursor<'_>) -> Result<OracleValue, OracleThinError> {
    if cursor.read_bytes()?.is_none() {
        return Ok(OracleValue::Null);
    }
    let Some(bytes) = cursor.read_bytes()? else {
        return Ok(OracleValue::Null);
    };
    if bytes.len() < 13 {
        return Err(OracleThinError::new(format!(
            "short Oracle UROWID data: {} bytes",
            bytes.len()
        )));
    }
    if bytes[0] == 1 {
        return encode_physical_rowid(
            u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]),
            u16::from_be_bytes([bytes[5], bytes[6]]),
            u32::from_be_bytes([bytes[7], bytes[8], bytes[9], bytes[10]]),
            u16::from_be_bytes([bytes[11], bytes[12]]),
        );
    }

    let mut output = String::with_capacity(1 + base64_unpadded_len(bytes.len() - 1));
    output.push('*');
    encode_base64_unpadded(&bytes[1..], &mut output);
    Ok(OracleValue::Text(output))
}

fn encode_physical_rowid(
    rba: u32,
    partition_id: u16,
    block_num: u32,
    slot_num: u16,
) -> Result<OracleValue, OracleThinError> {
    if rba == 0 && partition_id == 0 && block_num == 0 && slot_num == 0 {
        return Ok(OracleValue::Null);
    }
    let mut output = String::with_capacity(18);
    encode_rowid_base64_value(u64::from(rba), 6, &mut output);
    encode_rowid_base64_value(u64::from(partition_id), 3, &mut output);
    encode_rowid_base64_value(u64::from(block_num), 6, &mut output);
    encode_rowid_base64_value(u64::from(slot_num), 3, &mut output);
    Ok(OracleValue::Text(output))
}

fn encode_rowid_base64_value(mut value: u64, size: usize, output: &mut String) {
    let mut bytes = vec![b'A'; size];
    for offset in (0..size).rev() {
        bytes[offset] = TNS_BASE64_ALPHABET[(value & 0x3f) as usize];
        value >>= 6;
    }
    for byte in bytes {
        output.push(byte as char);
    }
}

fn base64_unpadded_len(input_len: usize) -> usize {
    (input_len / 3) * 4
        + match input_len % 3 {
            0 => 0,
            1 => 2,
            _ => 3,
        }
}

fn encode_base64_unpadded(input: &[u8], output: &mut String) {
    for chunk in input.chunks(3) {
        let first = chunk[0];
        output.push(TNS_BASE64_ALPHABET[(first >> 2) as usize] as char);
        let second = (first & 0x03) << 4;
        if chunk.len() == 1 {
            output.push(TNS_BASE64_ALPHABET[second as usize] as char);
            break;
        }
        let second = second | (chunk[1] >> 4);
        output.push(TNS_BASE64_ALPHABET[second as usize] as char);
        let third = (chunk[1] & 0x0f) << 2;
        if chunk.len() == 2 {
            output.push(TNS_BASE64_ALPHABET[third as usize] as char);
            break;
        }
        let third = third | (chunk[2] >> 6);
        output.push(TNS_BASE64_ALPHABET[third as usize] as char);
        output.push(TNS_BASE64_ALPHABET[(chunk[2] & 0x3f) as usize] as char);
    }
}

fn read_bfile_locator(cursor: &mut PacketCursor<'_>) -> Result<OracleValue, OracleThinError> {
    let num_bytes = cursor.read_ub4()?;
    if num_bytes == 0 {
        return Ok(OracleValue::Null);
    }
    Ok(cursor
        .read_bytes()?
        .map(OracleValue::Lob)
        .unwrap_or(OracleValue::Null))
}

fn read_vector_value(cursor: &mut PacketCursor<'_>) -> Result<OracleValue, OracleThinError> {
    if cursor.peek_u8().is_some_and(|len| usize::from(len) > 8) {
        return Ok(cursor
            .read_bytes()?
            .map(OracleValue::Lob)
            .unwrap_or(OracleValue::Null));
    }
    let num_bytes = cursor.read_ub4()?;
    if num_bytes == 0 {
        return Ok(OracleValue::Null);
    }
    if cursor.peek_u8().is_some_and(|len| usize::from(len) > 8) {
        return Ok(cursor
            .read_bytes()?
            .map(OracleValue::Lob)
            .unwrap_or(OracleValue::Null));
    }
    let _ = cursor.read_ub8()?;
    let _ = cursor.read_ub4()?;
    let data = cursor.read_bytes()?;
    let _ = cursor.read_bytes()?;
    match data {
        Some(bytes) if !bytes.is_empty() => Ok(OracleValue::Text(decode_oracle_vector(&bytes)?)),
        _ => Ok(OracleValue::Null),
    }
}

fn read_json_value(cursor: &mut PacketCursor<'_>) -> Result<OracleValue, OracleThinError> {
    let num_bytes = cursor.read_ub4()?;
    if num_bytes == 0 {
        return Ok(OracleValue::Null);
    }
    let _ = cursor.read_ub8()?;
    let _ = cursor.read_ub4()?;
    let data = cursor.read_bytes()?;
    let _ = cursor.read_bytes()?;
    match data {
        Some(bytes) if !bytes.is_empty() => decode_json_payload_value(&bytes),
        _ => Ok(OracleValue::Null),
    }
}

fn read_object_value(
    cursor: &mut PacketCursor<'_>,
    capabilities: &OracleThinCapabilities,
    column: &ThinColumn,
    object_attrs_by_type: &HashMap<(String, String), Vec<ThinColumn>>,
    collection_element_by_type: &HashMap<(String, String), ThinColumn>,
) -> Result<OracleValue, OracleThinError> {
    cursor.skip_bytes_with_ub4_length()?; // type OID
    cursor.skip_bytes_with_ub4_length()?; // object OID
    cursor.skip_bytes_with_ub4_length()?; // snapshot
    let _ = cursor.read_ub2()?; // version
    let num_bytes = cursor.read_ub4()? as usize;
    let _ = cursor.read_ub2()?; // flags
    if num_bytes == 0 {
        return Ok(OracleValue::Null);
    }
    let Some(packed_data) = cursor.read_bytes()? else {
        return Ok(OracleValue::Null);
    };
    let key = object_type_key(&column.schema_name, &column.type_name);
    if let Some(element) = collection_element_by_type.get(&key) {
        return decode_collection_payload(
            &packed_data,
            capabilities,
            element,
            object_attrs_by_type,
            collection_element_by_type,
        );
    }
    decode_object_payload(
        &packed_data,
        capabilities,
        column,
        object_attrs_by_type,
        collection_element_by_type,
    )
}

fn decode_object_payload(
    bytes: &[u8],
    capabilities: &OracleThinCapabilities,
    column: &ThinColumn,
    object_attrs_by_type: &HashMap<(String, String), Vec<ThinColumn>>,
    collection_element_by_type: &HashMap<(String, String), ThinColumn>,
) -> Result<OracleValue, OracleThinError> {
    let key = object_type_key(&column.schema_name, &column.type_name);
    let attrs = object_attrs_by_type.get(&key).ok_or_else(|| {
        OracleThinError::new(format!(
            "Oracle thin TTC cannot decode Oracle object {}.{} without type metadata",
            key.0, key.1
        ))
    })?;
    let mut cursor = PacketCursor::with_capabilities(bytes, capabilities);
    let image_flags = cursor.read_u8()?;
    let _image_version = cursor.read_u8()?;
    skip_pickle_length(&mut cursor)?;
    if image_flags & TNS_OBJ_IS_DEGENERATE != 0 {
        return Err(OracleThinError::new(
            "Oracle DbObject stored in a LOB is not supported",
        ));
    }
    if image_flags & TNS_OBJ_NO_PREFIX_SEG == 0 {
        let prefix_seg_length = read_pickle_length(&mut cursor)?;
        cursor.skip(prefix_seg_length)?;
    }
    read_object_attrs(
        &mut cursor,
        capabilities,
        attrs,
        object_attrs_by_type,
        collection_element_by_type,
    )
}

fn read_object_attrs(
    cursor: &mut PacketCursor<'_>,
    capabilities: &OracleThinCapabilities,
    attrs: &[ThinColumn],
    object_attrs_by_type: &HashMap<(String, String), Vec<ThinColumn>>,
    collection_element_by_type: &HashMap<(String, String), ThinColumn>,
) -> Result<OracleValue, OracleThinError> {
    let mut values = Vec::with_capacity(attrs.len());
    for attr in attrs {
        let value = read_object_attr_value(
            cursor,
            capabilities,
            attr,
            object_attrs_by_type,
            collection_element_by_type,
            false,
        )?;
        values.push((attr.name.clone(), value));
    }
    Ok(OracleValue::Object(values))
}

fn read_object_attr_value(
    cursor: &mut PacketCursor<'_>,
    capabilities: &OracleThinCapabilities,
    attr: &ThinColumn,
    object_attrs_by_type: &HashMap<(String, String), Vec<ThinColumn>>,
    collection_element_by_type: &HashMap<(String, String), ThinColumn>,
    object_values_are_wrapped: bool,
) -> Result<OracleValue, OracleThinError> {
    match attr.ora_type_num {
        ORA_TYPE_NUM_NUMBER => read_object_pickle_bytes(cursor)?
            .map(|bytes| decode_oracle_number(&bytes).map(OracleValue::Number))
            .transpose()
            .map(|value| value.unwrap_or(OracleValue::Null)),
        TNS_DATA_TYPE_BINARY_INTEGER => read_object_pickle_bytes(cursor)?
            .map(|bytes| decode_object_binary_integer(&bytes).map(OracleValue::Number))
            .transpose()
            .map(|value| value.unwrap_or(OracleValue::Null)),
        ORA_TYPE_NUM_VARCHAR | ORA_TYPE_NUM_CHAR => read_object_pickle_bytes(cursor)?
            .map(|bytes| {
                decode_oracle_text(&bytes, attr.charset_form, capabilities).map(OracleValue::Text)
            })
            .transpose()
            .map(|value| value.unwrap_or(OracleValue::Null)),
        ORA_TYPE_NUM_LONG => read_object_pickle_bytes(cursor)?
            .map(|bytes| {
                decode_oracle_text(&bytes, attr.charset_form, capabilities).map(OracleValue::Text)
            })
            .transpose()
            .map(|value| value.unwrap_or(OracleValue::Null)),
        ORA_TYPE_NUM_RAW => Ok(read_object_pickle_bytes(cursor)?
            .map(OracleValue::Bytes)
            .unwrap_or(OracleValue::Null)),
        ORA_TYPE_NUM_LONG_RAW => Ok(read_object_pickle_bytes(cursor)?
            .map(OracleValue::Bytes)
            .unwrap_or(OracleValue::Null)),
        ORA_TYPE_NUM_DATE => read_object_pickle_bytes(cursor)?
            .map(|bytes| decode_oracle_datetime(&bytes).map(OracleValue::DateTime))
            .transpose()
            .map(|value| value.unwrap_or(OracleValue::Null)),
        ORA_TYPE_NUM_TIMESTAMP | ORA_TYPE_NUM_TIMESTAMP_TZ | ORA_TYPE_NUM_TIMESTAMP_LTZ => {
            read_object_pickle_bytes(cursor)?
                .map(|bytes| decode_oracle_datetime(&bytes).map(OracleValue::Timestamp))
                .transpose()
                .map(|value| value.unwrap_or(OracleValue::Null))
        }
        ORA_TYPE_NUM_BINARY_FLOAT => read_object_pickle_bytes(cursor)?
            .map(|bytes| decode_oracle_binary_float(&bytes).map(OracleValue::Number))
            .transpose()
            .map(|value| value.unwrap_or(OracleValue::Null)),
        ORA_TYPE_NUM_BINARY_DOUBLE => read_object_pickle_bytes(cursor)?
            .map(|bytes| decode_oracle_binary_double(&bytes).map(OracleValue::Number))
            .transpose()
            .map(|value| value.unwrap_or(OracleValue::Null)),
        ORA_TYPE_NUM_BOOLEAN => Ok(read_object_pickle_bytes(cursor)?
            .map(|bytes| OracleValue::Boolean(bytes.iter().any(|byte| *byte != 0)))
            .unwrap_or(OracleValue::Null)),
        ORA_TYPE_NUM_INTERVAL_YM | ORA_TYPE_NUM_INTERVAL_YM_DTY => {
            read_object_pickle_bytes(cursor)?
                .map(|bytes| decode_oracle_interval_ym(&bytes).map(OracleValue::Text))
                .transpose()
                .map(|value| value.unwrap_or(OracleValue::Null))
        }
        ORA_TYPE_NUM_INTERVAL_DS | ORA_TYPE_NUM_INTERVAL_DS_DTY => {
            read_object_pickle_bytes(cursor)?
                .map(|bytes| decode_oracle_interval_ds(&bytes).map(OracleValue::Text))
                .transpose()
                .map(|value| value.unwrap_or(OracleValue::Null))
        }
        ORA_TYPE_NUM_CLOB | ORA_TYPE_NUM_BLOB | ORA_TYPE_NUM_BFILE => {
            Ok(read_object_pickle_bytes(cursor)?
                .map(OracleValue::Lob)
                .unwrap_or(OracleValue::Null))
        }
        ORA_TYPE_NUM_OBJECT if attr.column_type == OracleColumnType::Xml => {
            read_object_pickle_bytes(cursor)?
                .map(|bytes| decode_xmltype_payload(&bytes, capabilities))
                .transpose()
                .map(|value| value.unwrap_or(OracleValue::Null))
        }
        ORA_TYPE_NUM_OBJECT => {
            let key = object_type_key(&attr.schema_name, &attr.type_name);
            if let Some(element) = collection_element_by_type.get(&key) {
                return read_object_pickle_bytes(cursor)?
                    .map(|bytes| {
                        decode_collection_payload(
                            &bytes,
                            capabilities,
                            element,
                            object_attrs_by_type,
                            collection_element_by_type,
                        )
                    })
                    .transpose()
                    .map(|value| value.unwrap_or(OracleValue::Null));
            }
            if object_values_are_wrapped {
                return read_object_pickle_bytes(cursor)?
                    .map(|bytes| {
                        decode_object_payload(
                            &bytes,
                            capabilities,
                            attr,
                            object_attrs_by_type,
                            collection_element_by_type,
                        )
                    })
                    .transpose()
                    .map(|value| value.unwrap_or(OracleValue::Null));
            }
            let attrs = object_attrs_by_type.get(&key).ok_or_else(|| {
                OracleThinError::new(format!(
                    "Oracle thin TTC cannot decode nested Oracle object {}.{} without type metadata",
                    key.0, key.1
                ))
            })?;
            let marker = cursor.read_u8()?;
            if marker == TNS_LEGACY_NULL_LENGTH_INDICATOR {
                return Ok(OracleValue::Null);
            }
            if marker != 0 {
                cursor.pos = cursor.pos.saturating_sub(1);
            }
            read_object_attrs(
                cursor,
                capabilities,
                attrs,
                object_attrs_by_type,
                collection_element_by_type,
            )
        }
        other => Err(OracleThinError::new(format!(
            "Oracle thin TTC cannot decode Oracle object attribute {} type {other}",
            attr.name
        ))),
    }
}

fn decode_collection_payload(
    bytes: &[u8],
    capabilities: &OracleThinCapabilities,
    element: &ThinColumn,
    object_attrs_by_type: &HashMap<(String, String), Vec<ThinColumn>>,
    collection_element_by_type: &HashMap<(String, String), ThinColumn>,
) -> Result<OracleValue, OracleThinError> {
    let mut cursor = PacketCursor::with_capabilities(bytes, capabilities);
    let image_flags = cursor.read_u8()?;
    let _image_version = cursor.read_u8()?;
    skip_pickle_length(&mut cursor)?;
    if image_flags & TNS_OBJ_IS_DEGENERATE != 0 {
        return Err(OracleThinError::new(
            "Oracle DbObject stored in a LOB is not supported",
        ));
    }
    if image_flags & TNS_OBJ_NO_PREFIX_SEG == 0 {
        let prefix_seg_length = read_pickle_length(&mut cursor)?;
        cursor.skip(prefix_seg_length)?;
    }
    let collection_flags = cursor.read_u8()?;
    let has_indexes = collection_flags & TNS_OBJ_HAS_INDEXES != 0;
    let num_elements = read_pickle_length(&mut cursor)?;
    if has_indexes {
        let mut values = Vec::with_capacity(num_elements);
        for _ in 0..num_elements {
            let index = read_object_collection_index(&mut cursor)?;
            let value = read_object_attr_value(
                &mut cursor,
                capabilities,
                element,
                object_attrs_by_type,
                collection_element_by_type,
                true,
            )?;
            values.push((index, value));
        }
        Ok(OracleValue::IndexedArray(values))
    } else {
        let mut values = Vec::with_capacity(num_elements);
        for _ in 0..num_elements {
            values.push(read_object_attr_value(
                &mut cursor,
                capabilities,
                element,
                object_attrs_by_type,
                collection_element_by_type,
                true,
            )?);
        }
        Ok(OracleValue::Array(values))
    }
}

fn read_object_collection_index(cursor: &mut PacketCursor<'_>) -> Result<i32, OracleThinError> {
    let bytes = cursor.read_raw(4)?;
    Ok(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn decode_object_binary_integer(bytes: &[u8]) -> Result<String, OracleThinError> {
    let len = bytes.len();
    if !(1..=8).contains(&len) {
        return Err(OracleThinError::new(format!(
            "invalid Oracle DbObject BINARY_INTEGER length {len}"
        )));
    }
    let value = bytes
        .iter()
        .fold(0_u64, |value, byte| (value << 8) | u64::from(*byte));
    Ok((value as i32).to_string())
}

fn read_object_pickle_bytes(
    cursor: &mut PacketCursor<'_>,
) -> Result<Option<Vec<u8>>, OracleThinError> {
    let len = cursor.read_u8()?;
    match len {
        0 | 0xff => Ok(None),
        TNS_LEGACY_NULL_LENGTH_INDICATOR if cursor.legacy_null_clr => Ok(None),
        TNS_LONG_LENGTH_INDICATOR => {
            let len = cursor.read_u32_be()? as usize;
            Ok(Some(cursor.read_raw(len)?.to_vec()))
        }
        len => Ok(Some(cursor.read_raw(usize::from(len))?.to_vec())),
    }
}

fn read_xmltype_value(
    cursor: &mut PacketCursor<'_>,
    capabilities: &OracleThinCapabilities,
) -> Result<OracleValue, OracleThinError> {
    cursor.skip_bytes_with_ub4_length()?;
    cursor.skip_bytes_with_ub4_length()?;
    cursor.skip_bytes_with_ub4_length()?;
    let _ = cursor.read_ub2()?;
    let num_bytes = cursor.read_ub4()? as usize;
    let _ = cursor.read_ub2()?;
    if num_bytes == 0 {
        return Ok(OracleValue::Null);
    }
    let Some(mut packed_data) = cursor.read_bytes()? else {
        return Ok(OracleValue::Null);
    };
    if packed_data.len() < num_bytes {
        return Err(OracleThinError::new(format!(
            "short Oracle XMLTYPE payload: expected {num_bytes} bytes, got {}",
            packed_data.len()
        )));
    }
    if packed_data.len() > num_bytes {
        packed_data.truncate(num_bytes);
    }
    decode_xmltype_payload(&packed_data, capabilities)
}

fn read_lob_with_length(
    cursor: &mut PacketCursor<'_>,
    column: &ThinColumn,
) -> Result<OracleValue, OracleThinError> {
    let num_bytes = cursor.read_ub4()?;
    if num_bytes == 0 {
        return Ok(OracleValue::Null);
    }
    if !matches!(
        column.ora_type_num,
        ORA_TYPE_NUM_BFILE | TNS_DATA_TYPE_CFILE | ORA_TYPE_NUM_DBFILE
    ) {
        let _size = cursor.read_ub8()?;
        let _chunk_size = cursor.read_ub4()?;
    }
    let Some(locator) = cursor.read_bytes()? else {
        return Ok(OracleValue::Null);
    };
    Ok(OracleValue::Lob(locator))
}

fn decode_xmltype_payload(
    bytes: &[u8],
    capabilities: &OracleThinCapabilities,
) -> Result<OracleValue, OracleThinError> {
    let mut cursor = PacketCursor::with_capabilities(bytes, capabilities);
    let image_flags = cursor.read_u8()?;
    let _image_version = cursor.read_u8()?;
    skip_pickle_length(&mut cursor)?;
    if image_flags & TNS_OBJ_NO_PREFIX_SEG == 0 && image_flags & TNS_OBJ_IS_DEGENERATE == 0 {
        let prefix_seg_length = read_pickle_length(&mut cursor)?;
        cursor.skip(prefix_seg_length)?;
    }
    cursor.skip(1)?;
    let xml_flag = cursor.read_u32_be()?;
    if xml_flag & TNS_XML_TYPE_FLAG_SKIP_NEXT_4 != 0 {
        cursor.skip(4)?;
    }
    let data = cursor.read_raw(cursor.remaining())?;
    if xml_flag & TNS_XML_TYPE_STRING != 0 {
        return Ok(OracleValue::Text(decode_oracle_text(
            data,
            CS_FORM_IMPLICIT,
            capabilities,
        )?));
    }
    if xml_flag & TNS_XML_TYPE_LOB != 0 {
        return Ok(OracleValue::Lob(data.to_vec()));
    }
    Err(OracleThinError::new(format!(
        "unexpected Oracle XMLTYPE flag 0x{xml_flag:x}"
    )))
}

fn decode_xml_clob_lob_text(
    bytes: &[u8],
    capabilities: &OracleThinCapabilities,
) -> Result<String, OracleThinError> {
    if looks_like_utf16be_text(bytes) {
        return decode_utf16be_oracle_text(bytes);
    }
    decode_oracle_text(bytes, CS_FORM_IMPLICIT, capabilities)
}

fn looks_like_utf16be_text(bytes: &[u8]) -> bool {
    bytes.len() >= 2
        && bytes.len() % 2 == 0
        && bytes
            .chunks_exact(2)
            .take(16)
            .filter(|chunk| chunk[0] == 0)
            .count()
            >= 8
}

fn read_pickle_length(cursor: &mut PacketCursor<'_>) -> Result<usize, OracleThinError> {
    let len = cursor.read_u8()?;
    if len == TNS_LONG_LENGTH_INDICATOR {
        Ok(cursor.read_u32_be()? as usize)
    } else {
        Ok(usize::from(len))
    }
}

fn skip_pickle_length(cursor: &mut PacketCursor<'_>) -> Result<(), OracleThinError> {
    let _ = read_pickle_length(cursor)?;
    Ok(())
}

fn decode_json_payload(bytes: &[u8]) -> Result<String, OracleThinError> {
    if bytes.starts_with(&[
        TNS_JSON_MAGIC_BYTE_1,
        TNS_JSON_MAGIC_BYTE_2,
        TNS_JSON_MAGIC_BYTE_3,
    ]) {
        decode_oson_to_json(bytes)
    } else {
        std::str::from_utf8(bytes)
            .map(str::to_string)
            .map_err(|err| OracleThinError::new(format!("invalid JSON text UTF-8: {err}")))
    }
}

fn decode_json_payload_value(bytes: &[u8]) -> Result<OracleValue, OracleThinError> {
    if bytes.starts_with(&[
        TNS_JSON_MAGIC_BYTE_1,
        TNS_JSON_MAGIC_BYTE_2,
        TNS_JSON_MAGIC_BYTE_3,
    ]) {
        if let Some(value) = decode_oson_top_level_special_scalar(bytes)? {
            return Ok(match value {
                OsonTopLevelSpecialScalar::JsonId(value) => OracleValue::JsonId(value),
                OsonTopLevelSpecialScalar::Bytes(value) => OracleValue::Bytes(value),
            });
        }
    }
    Ok(OracleValue::Text(decode_json_payload(bytes)?))
}

fn decode_oson_to_json(bytes: &[u8]) -> Result<String, OracleThinError> {
    OsonDecoder::new(bytes).decode()
}

enum OsonTopLevelSpecialScalar {
    JsonId(Vec<u8>),
    Bytes(Vec<u8>),
}

fn decode_oson_top_level_special_scalar(
    bytes: &[u8],
) -> Result<Option<OsonTopLevelSpecialScalar>, OracleThinError> {
    let mut decoder = OsonDecoder::new(bytes);
    let magic = decoder.read_raw(3)?;
    if magic
        != [
            TNS_JSON_MAGIC_BYTE_1,
            TNS_JSON_MAGIC_BYTE_2,
            TNS_JSON_MAGIC_BYTE_3,
        ]
    {
        return Ok(None);
    }
    let version = decoder.read_u8()?;
    if !matches!(
        version,
        TNS_JSON_VERSION_MAX_FNAME_255 | TNS_JSON_VERSION_MAX_FNAME_65535
    ) {
        return Err(OracleThinError::new(format!(
            "unsupported OSON version {version}"
        )));
    }
    let primary_flags = decoder.read_u16_be()?;
    if primary_flags & TNS_JSON_FLAG_IS_SCALAR == 0 {
        return Ok(None);
    }
    decoder.skip_tree_segment_size(primary_flags)?;
    match decoder.read_u8()? {
        TNS_JSON_TYPE_ID => {
            let len = usize::from(decoder.read_u8()?);
            Ok(Some(OsonTopLevelSpecialScalar::JsonId(
                decoder.read_raw(len)?.to_vec(),
            )))
        }
        TNS_JSON_TYPE_BINARY_LENGTH_UINT16 => {
            let len = usize::from(decoder.read_u16_be()?);
            Ok(Some(OsonTopLevelSpecialScalar::Bytes(
                decoder.read_raw(len)?.to_vec(),
            )))
        }
        TNS_JSON_TYPE_BINARY_LENGTH_UINT32 => {
            let len = decoder.read_u32_be()? as usize;
            Ok(Some(OsonTopLevelSpecialScalar::Bytes(
                decoder.read_raw(len)?.to_vec(),
            )))
        }
        _ => Ok(None),
    }
}

#[derive(Clone)]
struct OsonFieldName {
    name: String,
    bytes: Vec<u8>,
    hash_id: u32,
    offset: usize,
    field_id: usize,
}

fn encode_oson_json(
    value: &JsonValue,
    supports_long_field_names: bool,
) -> Result<Vec<u8>, OracleThinError> {
    let is_scalar = !matches!(value, JsonValue::Array(_) | JsonValue::Object(_));
    let mut field_names = Vec::new();
    collect_oson_field_names(value, &mut field_names)?;
    let mut short_field_names = Vec::new();
    let mut long_field_names = Vec::new();
    for field in field_names {
        if field.bytes.len() <= u8::MAX as usize {
            short_field_names.push(field);
        } else {
            if !supports_long_field_names {
                return Err(OracleThinError::new(
                    "Oracle JSON bind field names longer than 255 bytes require OSON long field name support",
                ));
            }
            long_field_names.push(field);
        }
    }
    sort_oson_field_names(&mut short_field_names);
    sort_oson_field_names(&mut long_field_names);
    for (index, field) in short_field_names.iter_mut().enumerate() {
        field.field_id = index + 1;
    }
    for (index, field) in long_field_names.iter_mut().enumerate() {
        field.field_id = short_field_names.len() + index + 1;
    }
    let mut field_names = short_field_names.clone();
    field_names.extend(long_field_names.clone());
    let field_id_size = oson_field_id_size(field_names.len());

    let (short_field_segment, short_field_names_bytes_len) = if short_field_names.is_empty() {
        (Vec::new(), 0)
    } else {
        encode_oson_field_names_segment(&short_field_names, false)?
    };
    let (long_field_segment, long_field_names_bytes_len) = if long_field_names.is_empty() {
        (Vec::new(), 0)
    } else {
        encode_oson_field_names_segment(&long_field_names, true)?
    };

    let mut tree = Vec::new();
    encode_oson_node(value, &mut tree, &field_names)?;

    let mut flags = TNS_JSON_FLAG_INLINE_LEAF;
    if is_scalar {
        flags |= TNS_JSON_FLAG_IS_SCALAR;
    } else {
        flags |= TNS_JSON_FLAG_HASH_ID_UINT8 | TNS_JSON_FLAG_TINY_NODES_STAT;
        if field_names.len() > u16::MAX as usize {
            flags |= TNS_JSON_FLAG_NUM_FNAMES_UINT32;
        } else if field_names.len() > u8::MAX as usize {
            flags |= TNS_JSON_FLAG_NUM_FNAMES_UINT16;
        }
    }
    if short_field_names_bytes_len > u16::MAX as usize {
        flags |= TNS_JSON_FLAG_FNAMES_SEG_UINT32;
    }
    if tree.len() > u16::MAX as usize {
        flags |= TNS_JSON_FLAG_TREE_SEG_UINT32;
    }
    let version = if long_field_names.is_empty() {
        TNS_JSON_VERSION_MAX_FNAME_255
    } else {
        TNS_JSON_VERSION_MAX_FNAME_65535
    };

    let mut out = Vec::new();
    out.extend_from_slice(&[
        TNS_JSON_MAGIC_BYTE_1,
        TNS_JSON_MAGIC_BYTE_2,
        TNS_JSON_MAGIC_BYTE_3,
        version,
    ]);
    push_be_u16(&mut out, flags);
    if !is_scalar {
        push_oson_count_with_size(&mut out, short_field_names.len(), field_id_size)?;
        push_oson_segment_len(
            &mut out,
            short_field_names_bytes_len,
            flags & TNS_JSON_FLAG_FNAMES_SEG_UINT32 != 0,
        )?;
        if version == TNS_JSON_VERSION_MAX_FNAME_65535 {
            let secondary_flags = if long_field_names_bytes_len <= u16::MAX as usize {
                TNS_JSON_FLAG_SEC_FNAMES_SEG_UINT16
            } else {
                0
            };
            push_be_u16(&mut out, secondary_flags);
            let long_count = u32::try_from(long_field_names.len())
                .map_err(|_| OracleThinError::new("Oracle JSON bind has too many long fields"))?;
            push_be_u32(&mut out, long_count);
            let long_len = u32::try_from(long_field_names_bytes_len).map_err(|_| {
                OracleThinError::new("Oracle JSON bind long field segment is too large")
            })?;
            push_be_u32(&mut out, long_len);
        }
    }
    push_oson_segment_len(
        &mut out,
        tree.len(),
        flags & TNS_JSON_FLAG_TREE_SEG_UINT32 != 0,
    )?;
    if !is_scalar {
        push_be_u16(&mut out, 0);
        out.extend_from_slice(&short_field_segment);
        out.extend_from_slice(&long_field_segment);
    }
    out.extend_from_slice(&tree);
    Ok(out)
}

fn encode_oson_raw_json(value: &[u8]) -> Result<Vec<u8>, OracleThinError> {
    let mut tree = Vec::with_capacity(value.len().saturating_add(5));
    if value.len() < 65536 {
        tree.push(TNS_JSON_TYPE_BINARY_LENGTH_UINT16);
        push_be_u16(&mut tree, value.len() as u16);
    } else {
        let len = u32::try_from(value.len())
            .map_err(|_| OracleThinError::new("Oracle JSON raw bind value is too large"))?;
        tree.push(TNS_JSON_TYPE_BINARY_LENGTH_UINT32);
        push_be_u32(&mut tree, len);
    }
    tree.extend_from_slice(value);
    encode_oson_scalar_tree(&tree)
}

fn encode_oson_id_json(value: &[u8]) -> Result<Vec<u8>, OracleThinError> {
    let len = u8::try_from(value.len())
        .map_err(|_| OracleThinError::new("Oracle JSON ID bind value is too large"))?;
    let mut tree = Vec::with_capacity(value.len().saturating_add(2));
    tree.push(TNS_JSON_TYPE_ID);
    tree.push(len);
    tree.extend_from_slice(value);
    encode_oson_scalar_tree(&tree)
}

fn encode_oson_bool_json(value: bool) -> Result<Vec<u8>, OracleThinError> {
    let tree = if value {
        [TNS_JSON_TYPE_TRUE]
    } else {
        [TNS_JSON_TYPE_FALSE]
    };
    encode_oson_scalar_tree(&tree)
}

fn encode_oson_number_json(value: &str) -> Result<Vec<u8>, OracleThinError> {
    let bytes = encode_oracle_number(value)?;
    let len = u8::try_from(bytes.len())
        .map_err(|_| OracleThinError::new("Oracle JSON number bind is too large"))?;
    let mut tree = Vec::with_capacity(bytes.len().saturating_add(2));
    tree.push(TNS_JSON_TYPE_NUMBER_LENGTH_UINT8);
    tree.push(len);
    tree.extend_from_slice(&bytes);
    encode_oson_scalar_tree(&tree)
}

fn encode_oson_string_json(value: &str) -> Result<Vec<u8>, OracleThinError> {
    let mut tree = Vec::with_capacity(value.len().saturating_add(5));
    encode_oson_string(value, &mut tree)?;
    encode_oson_scalar_tree(&tree)
}

fn encode_oson_date_json(value: &crate::OracleDateTime) -> Result<Vec<u8>, OracleThinError> {
    let mut tree = Vec::with_capacity(8);
    tree.push(TNS_JSON_TYPE_DATE);
    tree.extend_from_slice(&encode_oracle_date(value, 7));
    encode_oson_scalar_tree(&tree)
}

fn encode_oson_timestamp_json(value: &crate::OracleDateTime) -> Result<Vec<u8>, OracleThinError> {
    let mut tree = Vec::with_capacity(12);
    if value.nanosecond == 0 {
        tree.push(TNS_JSON_TYPE_TIMESTAMP7);
        tree.extend_from_slice(&encode_oracle_date(value, 7));
    } else {
        tree.push(TNS_JSON_TYPE_TIMESTAMP);
        tree.extend_from_slice(&encode_oracle_date(value, 11));
    }
    encode_oson_scalar_tree(&tree)
}

fn encode_oson_interval_ym_json(
    value: &OracleIntervalYearMonth,
) -> Result<Vec<u8>, OracleThinError> {
    let mut tree = Vec::with_capacity(6);
    tree.push(TNS_JSON_TYPE_INTERVAL_YM);
    tree.extend_from_slice(&encode_oracle_interval_ym(value)?);
    encode_oson_scalar_tree(&tree)
}

fn encode_oson_interval_ds_json(
    value: &OracleIntervalDaySecond,
) -> Result<Vec<u8>, OracleThinError> {
    let mut tree = Vec::with_capacity(12);
    tree.push(TNS_JSON_TYPE_INTERVAL_DS);
    tree.extend_from_slice(&encode_oracle_interval_ds(value)?);
    encode_oson_scalar_tree(&tree)
}

fn encode_oson_vector_json(value: &OracleVectorValue) -> Result<Vec<u8>, OracleThinError> {
    let vector = encode_vector(value)?;
    let len = u32::try_from(vector.len())
        .map_err(|_| OracleThinError::new("Oracle JSON vector bind value is too large"))?;
    let mut tree = Vec::with_capacity(vector.len().saturating_add(6));
    tree.push(TNS_JSON_TYPE_EXTENDED);
    tree.push(TNS_JSON_TYPE_VECTOR);
    push_be_u32(&mut tree, len);
    tree.extend_from_slice(&vector);
    encode_oson_scalar_tree(&tree)
}

fn encode_oson_scalar_tree(tree: &[u8]) -> Result<Vec<u8>, OracleThinError> {
    let mut flags = TNS_JSON_FLAG_INLINE_LEAF | TNS_JSON_FLAG_IS_SCALAR;
    if tree.len() > u16::MAX as usize {
        flags |= TNS_JSON_FLAG_TREE_SEG_UINT32;
    }

    let mut out = Vec::new();
    out.extend_from_slice(&[
        TNS_JSON_MAGIC_BYTE_1,
        TNS_JSON_MAGIC_BYTE_2,
        TNS_JSON_MAGIC_BYTE_3,
        TNS_JSON_VERSION_MAX_FNAME_255,
    ]);
    push_be_u16(&mut out, flags);
    push_oson_segment_len(
        &mut out,
        tree.len(),
        flags & TNS_JSON_FLAG_TREE_SEG_UINT32 != 0,
    )?;
    out.extend_from_slice(&tree);
    Ok(out)
}

fn sort_oson_field_names(fields: &mut [OsonFieldName]) {
    fields.sort_by(|left, right| {
        (left.hash_id & 0xff, left.bytes.len(), left.bytes.as_slice()).cmp(&(
            right.hash_id & 0xff,
            right.bytes.len(),
            right.bytes.as_slice(),
        ))
    });
}

fn collect_oson_field_names(
    value: &JsonValue,
    fields: &mut Vec<OsonFieldName>,
) -> Result<(), OracleThinError> {
    match value {
        JsonValue::Array(values) => {
            for value in values {
                collect_oson_field_names(value, fields)?;
            }
        }
        JsonValue::Object(values) => {
            for (name, value) in values {
                if !fields.iter().any(|field| field.name == *name) {
                    let bytes = name.as_bytes().to_vec();
                    if bytes.len() > u16::MAX as usize {
                        return Err(OracleThinError::new(
                            "Oracle JSON bind field names longer than 65535 bytes are not supported",
                        ));
                    }
                    fields.push(OsonFieldName {
                        name: name.clone(),
                        hash_id: oson_field_hash(&bytes),
                        bytes,
                        offset: 0,
                        field_id: 0,
                    });
                }
                collect_oson_field_names(value, fields)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn encode_oson_field_names_segment(
    fields: &[OsonFieldName],
    long_names: bool,
) -> Result<(Vec<u8>, usize), OracleThinError> {
    let mut names = Vec::new();
    let mut fields = fields.to_vec();
    for field in &mut fields {
        field.offset = names.len();
        if long_names {
            let len = u16::try_from(field.bytes.len()).map_err(|_| {
                OracleThinError::new("Oracle JSON bind long field name is too large")
            })?;
            push_be_u16(&mut names, len);
        } else {
            names.push(field.bytes.len() as u8);
        }
        names.extend_from_slice(&field.bytes);
    }

    let mut out = Vec::new();
    for field in &fields {
        if long_names {
            push_be_u16(&mut out, (field.hash_id & 0xffff) as u16);
        } else {
            out.push((field.hash_id & 0xff) as u8);
        }
    }
    let use_u32_offsets = names.len() > u16::MAX as usize;
    for field in &fields {
        if use_u32_offsets {
            let offset = u32::try_from(field.offset)
                .map_err(|_| OracleThinError::new("Oracle JSON bind field segment is too large"))?;
            push_be_u32(&mut out, offset);
        } else {
            let offset = u16::try_from(field.offset)
                .map_err(|_| OracleThinError::new("Oracle JSON bind field segment is too large"))?;
            push_be_u16(&mut out, offset);
        }
    }
    out.extend_from_slice(&names);
    Ok((out, names.len()))
}

fn encode_oson_node(
    value: &JsonValue,
    out: &mut Vec<u8>,
    fields: &[OsonFieldName],
) -> Result<(), OracleThinError> {
    match value {
        JsonValue::Null => out.push(TNS_JSON_TYPE_NULL),
        JsonValue::Bool(true) => out.push(TNS_JSON_TYPE_TRUE),
        JsonValue::Bool(false) => out.push(TNS_JSON_TYPE_FALSE),
        JsonValue::Number(value) => {
            let bytes = encode_oracle_number(&value.to_string())?;
            let len = u8::try_from(bytes.len())
                .map_err(|_| OracleThinError::new("Oracle JSON number bind is too large"))?;
            out.push(TNS_JSON_TYPE_NUMBER_LENGTH_UINT8);
            out.push(len);
            out.extend_from_slice(&bytes);
        }
        JsonValue::String(value) => encode_oson_string(value, out)?,
        JsonValue::Array(values) => encode_oson_array(values, out, fields)?,
        JsonValue::Object(values) => encode_oson_object(values, out, fields)?,
    }
    Ok(())
}

fn encode_oson_array(
    values: &[JsonValue],
    out: &mut Vec<u8>,
    fields: &[OsonFieldName],
) -> Result<(), OracleThinError> {
    let node_type = oson_container_node_type(TNS_JSON_TYPE_ARRAY, values.len());
    out.push(node_type);
    push_oson_child_count(out, values.len())?;
    let offsets_pos = out.len();
    out.resize(out.len() + values.len() * 4, 0);
    for (index, value) in values.iter().enumerate() {
        let offset = u32::try_from(out.len())
            .map_err(|_| OracleThinError::new("Oracle JSON bind tree segment is too large"))?;
        out[offsets_pos + index * 4..offsets_pos + index * 4 + 4]
            .copy_from_slice(&offset.to_be_bytes());
        encode_oson_node(value, out, fields)?;
    }
    Ok(())
}

fn encode_oson_object(
    values: &serde_json::Map<String, JsonValue>,
    out: &mut Vec<u8>,
    fields: &[OsonFieldName],
) -> Result<(), OracleThinError> {
    let node_type = oson_container_node_type(TNS_JSON_TYPE_OBJECT, values.len());
    let field_id_size = oson_field_id_size(fields.len());
    out.push(node_type);
    push_oson_child_count(out, values.len())?;
    let field_ids_pos = out.len();
    let offsets_pos = field_ids_pos + values.len() * field_id_size;
    out.resize(offsets_pos + values.len() * 4, 0);
    for (index, (name, value)) in values.iter().enumerate() {
        let field = fields
            .iter()
            .find(|field| field.name == *name)
            .ok_or_else(|| {
                OracleThinError::new(format!("missing Oracle JSON bind field {name}"))
            })?;
        write_oson_field_id(
            &mut out[field_ids_pos + index * field_id_size
                ..field_ids_pos + (index + 1) * field_id_size],
            field.field_id,
        )?;
        let offset = u32::try_from(out.len())
            .map_err(|_| OracleThinError::new("Oracle JSON bind tree segment is too large"))?;
        out[offsets_pos + index * 4..offsets_pos + index * 4 + 4]
            .copy_from_slice(&offset.to_be_bytes());
        encode_oson_node(value, out, fields)?;
    }
    Ok(())
}

fn encode_oson_string(value: &str, out: &mut Vec<u8>) -> Result<(), OracleThinError> {
    let bytes = value.as_bytes();
    if bytes.len() < 256 {
        out.push(TNS_JSON_TYPE_STRING_LENGTH_UINT8);
        out.push(bytes.len() as u8);
    } else if bytes.len() < 65_536 {
        out.push(TNS_JSON_TYPE_STRING_LENGTH_UINT16);
        push_be_u16(out, bytes.len() as u16);
    } else {
        out.push(TNS_JSON_TYPE_STRING_LENGTH_UINT32);
        let len = u32::try_from(bytes.len())
            .map_err(|_| OracleThinError::new("Oracle JSON string bind is too large"))?;
        push_be_u32(out, len);
    }
    out.extend_from_slice(bytes);
    Ok(())
}

fn oson_container_node_type(base_type: u8, num_children: usize) -> u8 {
    let mut node_type = base_type | 0x20;
    if num_children > u16::MAX as usize {
        node_type |= 0x10;
    } else if num_children > u8::MAX as usize {
        node_type |= 0x08;
    }
    node_type
}

fn push_oson_child_count(out: &mut Vec<u8>, count: usize) -> Result<(), OracleThinError> {
    if count < 256 {
        out.push(count as u8);
    } else if count < 65_536 {
        push_be_u16(out, count as u16);
    } else {
        let count = u32::try_from(count)
            .map_err(|_| OracleThinError::new("Oracle JSON bind has too many children"))?;
        push_be_u32(out, count);
    }
    Ok(())
}

fn push_oson_count_with_size(
    out: &mut Vec<u8>,
    count: usize,
    size: usize,
) -> Result<(), OracleThinError> {
    match size {
        1 => {
            let count = u8::try_from(count)
                .map_err(|_| OracleThinError::new("Oracle JSON bind field count is too large"))?;
            out.push(count);
        }
        2 => {
            let count = u16::try_from(count)
                .map_err(|_| OracleThinError::new("Oracle JSON bind field count is too large"))?;
            push_be_u16(out, count);
        }
        4 => {
            let count = u32::try_from(count)
                .map_err(|_| OracleThinError::new("Oracle JSON bind field count is too large"))?;
            push_be_u32(out, count);
        }
        _ => return Err(OracleThinError::new("invalid Oracle JSON bind count size")),
    }
    Ok(())
}

fn push_oson_segment_len(
    out: &mut Vec<u8>,
    len: usize,
    use_u32: bool,
) -> Result<(), OracleThinError> {
    if use_u32 {
        let len = u32::try_from(len)
            .map_err(|_| OracleThinError::new("Oracle JSON bind segment is too large"))?;
        push_be_u32(out, len);
    } else {
        let len = u16::try_from(len)
            .map_err(|_| OracleThinError::new("Oracle JSON bind segment is too large"))?;
        push_be_u16(out, len);
    }
    Ok(())
}

fn oson_field_id_size(num_fields: usize) -> usize {
    if num_fields <= u8::MAX as usize {
        1
    } else if num_fields <= u16::MAX as usize {
        2
    } else {
        4
    }
}

fn write_oson_field_id(out: &mut [u8], field_id: usize) -> Result<(), OracleThinError> {
    match out.len() {
        1 => {
            out[0] = u8::try_from(field_id)
                .map_err(|_| OracleThinError::new("Oracle JSON bind field id is too large"))?;
        }
        2 => out.copy_from_slice(
            &u16::try_from(field_id)
                .map_err(|_| OracleThinError::new("Oracle JSON bind field id is too large"))?
                .to_be_bytes(),
        ),
        4 => out.copy_from_slice(
            &u32::try_from(field_id)
                .map_err(|_| OracleThinError::new("Oracle JSON bind field id is too large"))?
                .to_be_bytes(),
        ),
        other => {
            return Err(OracleThinError::new(format!(
                "invalid Oracle JSON bind field id size {other}"
            )))
        }
    }
    Ok(())
}

fn oson_field_hash(bytes: &[u8]) -> u32 {
    let mut hash = 0x811c_9dc5u32;
    for byte in bytes {
        hash = (hash ^ u32::from(*byte)).wrapping_mul(16_777_619);
    }
    hash
}

fn push_be_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push_be_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push_be_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_be_bytes());
}

struct OsonDecoder<'a> {
    data: &'a [u8],
    pos: usize,
    field_names: Vec<String>,
    field_id_length: usize,
    tree_seg_pos: usize,
    relative_offsets: bool,
}

impl<'a> OsonDecoder<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            field_names: Vec::new(),
            field_id_length: 1,
            tree_seg_pos: 0,
            relative_offsets: false,
        }
    }

    fn decode(mut self) -> Result<String, OracleThinError> {
        let magic = self.read_raw(3)?;
        if magic
            != [
                TNS_JSON_MAGIC_BYTE_1,
                TNS_JSON_MAGIC_BYTE_2,
                TNS_JSON_MAGIC_BYTE_3,
            ]
        {
            return Err(OracleThinError::new(format!(
                "unexpected OSON magic bytes: {:02x?}",
                magic
            )));
        }
        let version = self.read_u8()?;
        if !matches!(
            version,
            TNS_JSON_VERSION_MAX_FNAME_255 | TNS_JSON_VERSION_MAX_FNAME_65535
        ) {
            return Err(OracleThinError::new(format!(
                "unsupported OSON version {version}"
            )));
        }
        let primary_flags = self.read_u16_be()?;
        self.relative_offsets = primary_flags & TNS_JSON_FLAG_REL_OFFSET_MODE != 0;

        if primary_flags & TNS_JSON_FLAG_IS_SCALAR != 0 {
            self.skip_tree_segment_size(primary_flags)?;
            self.tree_seg_pos = self.pos;
            return self.decode_node();
        }

        let num_short_field_names = if primary_flags & TNS_JSON_FLAG_NUM_FNAMES_UINT32 != 0 {
            self.field_id_length = 4;
            self.read_u32_be()?
        } else if primary_flags & TNS_JSON_FLAG_NUM_FNAMES_UINT16 != 0 {
            self.field_id_length = 2;
            u32::from(self.read_u16_be()?)
        } else {
            self.field_id_length = 1;
            u32::from(self.read_u8()?)
        };

        let short_offsets_size = if primary_flags & TNS_JSON_FLAG_FNAMES_SEG_UINT32 != 0 {
            4
        } else {
            2
        };
        let short_field_names_seg_size = if short_offsets_size == 4 {
            self.read_u32_be()? as usize
        } else {
            usize::from(self.read_u16_be()?)
        };

        let mut num_long_field_names = 0u32;
        let mut long_offsets_size = 0usize;
        let mut long_field_names_seg_size = 0usize;
        if version == TNS_JSON_VERSION_MAX_FNAME_65535 {
            let secondary_flags = self.read_u16_be()?;
            long_offsets_size = if secondary_flags & TNS_JSON_FLAG_SEC_FNAMES_SEG_UINT16 != 0 {
                2
            } else {
                4
            };
            num_long_field_names = self.read_u32_be()?;
            long_field_names_seg_size = self.read_u32_be()? as usize;
        }

        self.skip_tree_segment_size(primary_flags)?;
        let _ = self.read_u16_be()?;

        if num_short_field_names > 0 {
            let names = self.read_field_names_segment(
                num_short_field_names as usize,
                short_offsets_size,
                short_field_names_seg_size,
                false,
            )?;
            self.field_names.extend(names);
        }
        if num_long_field_names > 0 {
            let names = self.read_field_names_segment(
                num_long_field_names as usize,
                long_offsets_size,
                long_field_names_seg_size,
                true,
            )?;
            self.field_names.extend(names);
        }

        self.tree_seg_pos = self.pos;
        self.decode_node()
    }

    fn skip_tree_segment_size(&mut self, primary_flags: u16) -> Result<(), OracleThinError> {
        if primary_flags & TNS_JSON_FLAG_TREE_SEG_UINT32 != 0 {
            let _ = self.read_u32_be()?;
        } else {
            let _ = self.read_u16_be()?;
        }
        Ok(())
    }

    fn read_field_names_segment(
        &mut self,
        num_fields: usize,
        offsets_size: usize,
        segment_size: usize,
        long_names: bool,
    ) -> Result<Vec<String>, OracleThinError> {
        self.skip(num_fields * if long_names { 2 } else { 1 })?;
        let offsets_pos = self.pos;
        self.skip(num_fields * offsets_size)?;
        let segment = self.read_raw(segment_size)?.to_vec();
        let final_pos = self.pos;
        self.pos = offsets_pos;
        let mut names = Vec::with_capacity(num_fields);
        for _ in 0..num_fields {
            let offset = if offsets_size == 2 {
                usize::from(self.read_u16_be()?)
            } else {
                self.read_u32_be()? as usize
            };
            if long_names {
                let len_bytes = segment
                    .get(offset..offset + 2)
                    .ok_or_else(|| OracleThinError::new("short OSON long field name length"))?;
                let len = usize::from(u16::from_be_bytes([len_bytes[0], len_bytes[1]]));
                let name = segment
                    .get(offset + 2..offset + 2 + len)
                    .ok_or_else(|| OracleThinError::new("short OSON long field name bytes"))?;
                names.push(String::from_utf8(name.to_vec()).map_err(|err| {
                    OracleThinError::new(format!("invalid OSON long field name UTF-8: {err}"))
                })?);
            } else {
                let len = usize::from(
                    *segment
                        .get(offset)
                        .ok_or_else(|| OracleThinError::new("short OSON field name length"))?,
                );
                let name = segment
                    .get(offset + 1..offset + 1 + len)
                    .ok_or_else(|| OracleThinError::new("short OSON field name bytes"))?;
                names.push(String::from_utf8(name.to_vec()).map_err(|err| {
                    OracleThinError::new(format!("invalid OSON field name UTF-8: {err}"))
                })?);
            }
        }
        self.pos = final_pos;
        Ok(names)
    }

    fn decode_node(&mut self) -> Result<String, OracleThinError> {
        let node_type = self.read_u8()?;
        if node_type & 0x80 != 0 {
            return self.decode_container_node(node_type);
        }

        match node_type {
            TNS_JSON_TYPE_NULL => return Ok("null".to_string()),
            TNS_JSON_TYPE_TRUE => return Ok("true".to_string()),
            TNS_JSON_TYPE_FALSE => return Ok("false".to_string()),
            TNS_JSON_TYPE_DATE | TNS_JSON_TYPE_TIMESTAMP7 => {
                return Ok(json_quote(&oracle_datetime_to_json_string(
                    &decode_oracle_datetime(self.read_raw(7)?)?,
                )))
            }
            TNS_JSON_TYPE_TIMESTAMP => {
                return Ok(json_quote(&oracle_datetime_to_json_string(
                    &decode_oracle_datetime(self.read_raw(11)?)?,
                )))
            }
            TNS_JSON_TYPE_TIMESTAMP_TZ => {
                return Ok(json_quote(&oracle_datetime_to_json_string(
                    &decode_oracle_datetime(self.read_raw(13)?)?,
                )))
            }
            TNS_JSON_TYPE_BINARY_FLOAT => {
                let bytes = self.read_fixed::<4>()?;
                return decode_oracle_binary_float(&bytes);
            }
            TNS_JSON_TYPE_BINARY_DOUBLE => {
                let bytes = self.read_fixed::<8>()?;
                return decode_oracle_binary_double(&bytes);
            }
            TNS_JSON_TYPE_INTERVAL_DS => {
                return Ok(json_quote(&decode_oracle_interval_ds(self.read_raw(11)?)?));
            }
            TNS_JSON_TYPE_INTERVAL_YM => {
                return Ok(json_quote(&decode_oracle_interval_ym(self.read_raw(5)?)?));
            }
            TNS_JSON_TYPE_STRING_LENGTH_UINT8 => {
                let len = usize::from(self.read_u8()?);
                return Ok(json_quote(&self.read_utf8(len)?));
            }
            TNS_JSON_TYPE_STRING_LENGTH_UINT16 => {
                let len = usize::from(self.read_u16_be()?);
                return Ok(json_quote(&self.read_utf8(len)?));
            }
            TNS_JSON_TYPE_STRING_LENGTH_UINT32 => {
                let len = self.read_u32_be()? as usize;
                return Ok(json_quote(&self.read_utf8(len)?));
            }
            TNS_JSON_TYPE_NUMBER_LENGTH_UINT8 => {
                let len = usize::from(self.read_u8()?);
                return decode_oracle_number(self.read_raw(len)?);
            }
            TNS_JSON_TYPE_ID => {
                let len = usize::from(self.read_u8()?);
                return Ok(json_quote(&hex_string(self.read_raw(len)?)));
            }
            TNS_JSON_TYPE_BINARY_LENGTH_UINT16 => {
                let len = usize::from(self.read_u16_be()?);
                return Ok(json_rawhex_object(self.read_raw(len)?));
            }
            TNS_JSON_TYPE_BINARY_LENGTH_UINT32 => {
                let len = self.read_u32_be()? as usize;
                return Ok(json_rawhex_object(self.read_raw(len)?));
            }
            TNS_JSON_TYPE_EXTENDED => {
                let extended_type = self.read_u8()?;
                if extended_type == TNS_JSON_TYPE_VECTOR {
                    let len = self.read_u32_be()? as usize;
                    return decode_oracle_vector(self.read_raw(len)?);
                }
                return Err(OracleThinError::new(format!(
                    "unsupported OSON extended node type {extended_type}"
                )));
            }
            _ => {}
        }

        if (node_type & 0xf0) == 0x20 || (node_type & 0xf0) == 0x60 {
            let len = usize::from(node_type & 0x0f) + 1;
            return decode_oracle_number(self.read_raw(len)?);
        }
        if (node_type & 0xf0) == 0x40 || (node_type & 0xf0) == 0x50 {
            let len = usize::from(node_type & 0x0f);
            return decode_oracle_number(self.read_raw(len)?);
        }
        if (node_type & 0xe0) == 0 {
            let len = usize::from(node_type);
            return Ok(json_quote(&self.read_utf8(len)?));
        }

        Err(OracleThinError::new(format!(
            "unsupported OSON node type {node_type}"
        )))
    }

    fn decode_container_node(&mut self, node_type: u8) -> Result<String, OracleThinError> {
        let is_object = node_type & 0x40 == 0;
        let container_offset = self.pos - self.tree_seg_pos - 1;
        let (mut num_children, is_shared) = self.read_num_children(node_type)?;
        let mut field_ids_pos = 0usize;
        let offsets_pos;
        if is_shared {
            let offset = self.read_offset(node_type)?;
            offsets_pos = self.pos;
            self.pos = self.tree_seg_pos + offset;
            let shared_node_type = self.read_u8()?;
            let (shared_children, _) = self.read_num_children(shared_node_type)?;
            num_children = shared_children;
            field_ids_pos = self.pos;
        } else if is_object {
            field_ids_pos = self.pos;
            offsets_pos = self.pos + self.field_id_length * num_children;
        } else {
            offsets_pos = self.pos;
        }

        let mut next_field_ids_pos = field_ids_pos;
        let mut next_offsets_pos = offsets_pos;
        let mut values = Vec::with_capacity(num_children);
        for index in 0..num_children {
            let name = if is_object {
                self.pos = next_field_ids_pos;
                let field_id = match self.field_id_length {
                    1 => usize::from(self.read_u8()?),
                    2 => usize::from(self.read_u16_be()?),
                    4 => self.read_u32_be()? as usize,
                    other => {
                        return Err(OracleThinError::new(format!(
                            "invalid OSON field id length {other}"
                        )))
                    }
                };
                next_field_ids_pos = self.pos;
                Some(
                    self.field_names
                        .get(field_id.saturating_sub(1))
                        .ok_or_else(|| {
                            OracleThinError::new(format!("OSON field id {field_id} out of range"))
                        })?
                        .clone(),
                )
            } else {
                let _ = index;
                None
            };

            self.pos = next_offsets_pos;
            let mut offset = self.read_offset(node_type)?;
            if self.relative_offsets {
                offset += container_offset;
            }
            next_offsets_pos = self.pos;
            self.pos = self.tree_seg_pos + offset;
            let value = self.decode_node()?;
            if let Some(name) = name {
                values.push(format!("{}:{value}", json_quote(&name)));
            } else {
                values.push(value);
            }
        }

        if is_object {
            Ok(format!("{{{}}}", values.join(",")))
        } else {
            Ok(format!("[{}]", values.join(",")))
        }
    }

    fn read_num_children(&mut self, node_type: u8) -> Result<(usize, bool), OracleThinError> {
        match node_type & 0x18 {
            0 => Ok((usize::from(self.read_u8()?), false)),
            0x08 => Ok((usize::from(self.read_u16_be()?), false)),
            0x10 => Ok((self.read_u32_be()? as usize, false)),
            _ => Ok((0, true)),
        }
    }

    fn read_offset(&mut self, node_type: u8) -> Result<usize, OracleThinError> {
        if node_type & 0x20 != 0 {
            Ok(self.read_u32_be()? as usize)
        } else {
            Ok(usize::from(self.read_u16_be()?))
        }
    }

    fn read_utf8(&mut self, len: usize) -> Result<String, OracleThinError> {
        String::from_utf8(self.read_raw(len)?.to_vec())
            .map_err(|err| OracleThinError::new(format!("invalid OSON UTF-8 string: {err}")))
    }

    fn read_fixed<const N: usize>(&mut self) -> Result<[u8; N], OracleThinError> {
        let bytes = self.read_raw(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(bytes);
        Ok(out)
    }

    fn read_u8(&mut self) -> Result<u8, OracleThinError> {
        let value = *self
            .data
            .get(self.pos)
            .ok_or_else(|| OracleThinError::new("short OSON data while reading u8"))?;
        self.pos += 1;
        Ok(value)
    }

    fn read_u16_be(&mut self) -> Result<u16, OracleThinError> {
        let bytes = self.read_raw(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn read_u32_be(&mut self) -> Result<u32, OracleThinError> {
        let bytes = self.read_raw(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_raw(&mut self, len: usize) -> Result<&'a [u8], OracleThinError> {
        let end = self.pos.saturating_add(len);
        let bytes = self
            .data
            .get(self.pos..end)
            .ok_or_else(|| OracleThinError::new("short OSON data while reading bytes"))?;
        self.pos = end;
        Ok(bytes)
    }

    fn skip(&mut self, len: usize) -> Result<(), OracleThinError> {
        self.read_raw(len).map(|_| ())
    }
}

fn oracle_datetime_to_json_string(value: &crate::OracleDateTime) -> String {
    let mut out = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        value.year, value.month, value.day, value.hour, value.minute, value.second
    );
    if value.nanosecond > 0 {
        out.push_str(&format!(".{:09}", value.nanosecond));
        while out.ends_with('0') {
            out.pop();
        }
    }
    if let Some(suffix) = value.timezone_suffix() {
        out.push_str(&suffix);
    }
    out
}

fn json_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ch if ch <= '\u{1f}' => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn hex_string(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn json_rawhex_object(bytes: &[u8]) -> String {
    format!(r#"{{"$rawhex":"{}"}}"#, hex_string(bytes))
}

fn column_metadata_from_thin(column: &ThinColumn) -> ColumnMetadata {
    ColumnMetadata {
        name: column.name.clone(),
        column_type: column.column_type,
        charset_form: column.charset_form,
        ora_type_num: column.ora_type_num,
        buffer_size: column.buffer_size,
        schema_name: column.schema_name.clone(),
        type_name: column.type_name.clone(),
    }
}

#[cfg(test)]
fn oracle_column_type_from_ora_type(ora_type_num: u8) -> OracleColumnType {
    oracle_column_type_from_ora_type_for_protocol(ora_type_num, None)
}

fn oracle_column_type_from_ora_type_for_protocol(
    ora_type_num: u8,
    protocol_version: Option<u16>,
) -> OracleColumnType {
    if ora_type_num == TNS_DATA_TYPE_VBI && protocol_uses_go_ora_legacy_mappings(protocol_version) {
        return OracleColumnType::Raw;
    }
    match ora_type_num {
        ORA_TYPE_NUM_NUMBER
        | TNS_DATA_TYPE_BINARY_INTEGER
        | TNS_DATA_TYPE_FLOAT
        | TNS_DATA_TYPE_VNU
        | TNS_DATA_TYPE_PDN
        | TNS_DATA_TYPE_UIN
        | TNS_DATA_TYPE_SLS
        | TNS_DATA_TYPE_DTR
        | TNS_DATA_TYPE_DUN
        | TNS_DATA_TYPE_DOP
        | TNS_DATA_TYPE_DOL
        | TNS_DATA_TYPE_UB8 => OracleColumnType::Number,
        ORA_TYPE_NUM_DATE | TNS_DATA_TYPE_ODT | TNS_DATA_TYPE_EDATE => OracleColumnType::Date,
        ORA_TYPE_NUM_TIMESTAMP
        | ORA_TYPE_NUM_TIMESTAMP_TZ
        | ORA_TYPE_NUM_TIMESTAMP_DTY
        | ORA_TYPE_NUM_TIMESTAMP_TZ_EXT
        | ORA_TYPE_NUM_TIMESTAMP_LTZ
        | TNS_DATA_TYPE_ESITZ => OracleColumnType::Timestamp,
        ORA_TYPE_NUM_RAW | TNS_DATA_TYPE_LVB | ORA_TYPE_NUM_LONG_RAW => OracleColumnType::Raw,
        ORA_TYPE_NUM_BINARY_FLOAT | TNS_DATA_TYPE_BFLOAT => OracleColumnType::BinaryFloat,
        ORA_TYPE_NUM_BINARY_DOUBLE | TNS_DATA_TYPE_BDOUBLE => OracleColumnType::BinaryDouble,
        ORA_TYPE_NUM_LONG => OracleColumnType::Long,
        ORA_TYPE_NUM_CLOB | TNS_DATA_TYPE_DCLOB => OracleColumnType::Clob,
        ORA_TYPE_NUM_BLOB | TNS_DATA_TYPE_DBLOB => OracleColumnType::Blob,
        ORA_TYPE_NUM_BFILE | TNS_DATA_TYPE_CFILE | ORA_TYPE_NUM_DBFILE => OracleColumnType::Bfile,
        ORA_TYPE_NUM_VECTOR => OracleColumnType::Vector,
        ORA_TYPE_NUM_JSON | ORA_TYPE_NUM_DJSON => OracleColumnType::Json,
        ORA_TYPE_NUM_CURSOR | TNS_DATA_TYPE_RSET => OracleColumnType::Cursor,
        ORA_TYPE_NUM_BOOLEAN => OracleColumnType::Boolean,
        ORA_TYPE_NUM_INTERVAL_YM | ORA_TYPE_NUM_INTERVAL_YM_DTY => {
            OracleColumnType::IntervalYearMonth
        }
        ORA_TYPE_NUM_INTERVAL_DS | ORA_TYPE_NUM_INTERVAL_DS_DTY => {
            OracleColumnType::IntervalDaySecond
        }
        ORA_TYPE_NUM_ROWID | TNS_DATA_TYPE_RDD => OracleColumnType::Rowid,
        ORA_TYPE_NUM_UROWID => OracleColumnType::Urowid,
        ORA_TYPE_NUM_OBJECT | TNS_DATA_TYPE_EXT_NAMED | TNS_DATA_TYPE_PNTY => {
            OracleColumnType::Object
        }
        TNS_DATA_TYPE_EXT_REF | TNS_DATA_TYPE_INT_REF => OracleColumnType::ObjectRef,
        TNS_DATA_TYPE_STR
        | ORA_TYPE_NUM_VARCHAR
        | TNS_DATA_TYPE_VCS
        | TNS_DATA_TYPE_VBI
        | TNS_DATA_TYPE_LVC
        | TNS_DATA_TYPE_VST
        | TNS_DATA_TYPE_CLV
        | TNS_DATA_TYPE_TIME
        | TNS_DATA_TYPE_TIME_TZ
        | TNS_DATA_TYPE_ETIME
        | TNS_DATA_TYPE_ETTZ
        | ORA_TYPE_NUM_CHAR
        | TNS_DATA_TYPE_CHARZ => OracleColumnType::Varchar,
        _ => OracleColumnType::Unsupported(ora_type_num),
    }
}

fn decode_oracle_text(
    bytes: &[u8],
    charset_form: u8,
    capabilities: &OracleThinCapabilities,
) -> Result<String, OracleThinError> {
    if charset_form == CS_FORM_NCHAR {
        return decode_oracle_nchar_text(
            bytes,
            capabilities.ncharset_id,
            capabilities.protocol_version,
        );
    }
    match String::from_utf8(bytes.to_vec()) {
        Ok(text) => Ok(text),
        Err(err) => {
            if let Some(text) = decode_oracle_native_text(
                bytes,
                capabilities.charset_id,
                capabilities.protocol_version,
            )? {
                return Ok(text);
            }
            Err(OracleThinError::new(format!(
                "invalid UTF-8 Oracle text: {err}"
            )))
        }
    }
}

fn decode_oracle_nchar_text(
    bytes: &[u8],
    ncharset_id: u16,
    protocol_version: Option<u16>,
) -> Result<String, OracleThinError> {
    match ncharset_id {
        0 | ORACLE_CHARSET_AL16UTF16 => decode_utf16be_oracle_text(bytes),
        ORACLE_CHARSET_UTF8 | ORACLE_CHARSET_AL32UTF8 => String::from_utf8(bytes.to_vec())
            .map_err(|err| OracleThinError::new(format!("invalid UTF-8 Oracle NCHAR text: {err}"))),
        _ => decode_oracle_native_text(bytes, ncharset_id, protocol_version)?.ok_or_else(|| {
            OracleThinError::new(format!(
                "Oracle national character set id {ncharset_id} is not supported"
            ))
        }),
    }
}

fn decode_utf16be_oracle_text(bytes: &[u8]) -> Result<String, OracleThinError> {
    if bytes.len() % 2 != 0 {
        return Err(OracleThinError::new("odd-length Oracle NCHAR data"));
    }
    let units = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]));
    String::from_utf16(&units.collect::<Vec<_>>())
        .map_err(|err| OracleThinError::new(format!("invalid UTF-16 Oracle text: {err}")))
}

fn decode_oracle_native_text(
    bytes: &[u8],
    charset_id: u16,
    protocol_version: Option<u16>,
) -> Result<Option<String>, OracleThinError> {
    let Some(encodings) = oracle_native_iconv_encodings_for_protocol(charset_id, protocol_version)
    else {
        return Ok(None);
    };
    decode_with_iconv_any(bytes, encodings).map(Some)
}

fn encode_oracle_native_text(
    value: &str,
    charset_id: u16,
    protocol_version: Option<u16>,
) -> Result<Option<Vec<u8>>, OracleThinError> {
    let Some(encodings) = oracle_native_iconv_encodings_for_protocol(charset_id, protocol_version)
    else {
        return Ok(None);
    };
    encode_with_iconv_any(value.as_bytes(), encodings).map(Some)
}

fn oracle_native_iconv_encodings_for_protocol(
    charset_id: u16,
    protocol_version: Option<u16>,
) -> Option<&'static [&'static str]> {
    if protocol_uses_go_ora_legacy_mappings(protocol_version) {
        return oracle_native_iconv_encodings_go_ora(charset_id);
    }
    oracle_native_iconv_encodings_python_oracledb(charset_id)
        .or_else(|| oracle_native_iconv_encodings_go_ora(charset_id))
}

fn oracle_native_iconv_encodings_python_oracledb(
    charset_id: u16,
) -> Option<&'static [&'static str]> {
    match charset_id {
        ORACLE_CHARSET_US7ASCII => Some(&["US-ASCII", "ASCII"]),
        31 => Some(&["ISO-8859-1"]),
        32 => Some(&["ISO-8859-2"]),
        33 => Some(&["ISO-8859-3"]),
        34 => Some(&["ISO-8859-4"]),
        35 => Some(&["ISO-8859-5"]),
        36 => Some(&["ISO-8859-6"]),
        37 => Some(&["ISO-8859-7"]),
        38 => Some(&["ISO-8859-8"]),
        39 => Some(&["ISO-8859-9"]),
        40 => Some(&["ISO-8859-10"]),
        41 => Some(&["TIS-620"]),
        46 => Some(&["ISO-8859-15"]),
        47 => Some(&["ISO-8859-13"]),
        170 => Some(&["CP1250", "WINDOWS-1250"]),
        171 => Some(&["CP1251", "WINDOWS-1251"]),
        172 => Some(&["CP1253", "WINDOWS-1253"]),
        173 => Some(&["CP1254", "WINDOWS-1254"]),
        174 => Some(&["CP1255", "WINDOWS-1255"]),
        175 => Some(&["CP1256", "WINDOWS-1256"]),
        176 => Some(&["CP1257", "WINDOWS-1257"]),
        177 => Some(&["CP1258", "WINDOWS-1258"]),
        178 => Some(&["CP1252", "WINDOWS-1252"]),
        351 => Some(&["CP850"]),
        354 => Some(&["CP437"]),
        368 => Some(&["CP866"]),
        382 => Some(&["CP852"]),
        829 => Some(&["BIG5"]),
        830 => Some(&["EUC-KR"]),
        831 | 834 => Some(&["EUC-JP", "EUCJP"]),
        832 | 833 => Some(&["CP932", "WINDOWS-31J", "SHIFT_JIS"]),
        846 => Some(&["GBK", "CP936"]),
        850 => Some(&["BIG5-HKSCS", "BIG5"]),
        852 => Some(&["CP949", "MS949", "EUC-KR"]),
        854 => Some(&["BIG5"]),
        870 => Some(&["GB18030", "GBK", "CP936"]),
        ORACLE_CHARSET_AL16UTF16 => Some(&["UTF-16BE"]),
        _ => None,
    }
}

fn oracle_native_iconv_encodings_go_ora(charset_id: u16) -> Option<&'static [&'static str]> {
    match charset_id {
        ORACLE_CHARSET_US7ASCII => Some(&["US-ASCII", "ASCII"]),
        31 => Some(&["ISO-8859-1"]),
        32 => Some(&["ISO-8859-2"]),
        33 => Some(&["ISO-8859-3"]),
        34 => Some(&["ISO-8859-4"]),
        35 => Some(&["ISO-8859-5"]),
        36 => Some(&["ISO-8859-6"]),
        37 => Some(&["ISO-8859-7"]),
        38 => Some(&["ISO-8859-8"]),
        39 => Some(&["ISO-8859-9"]),
        40 => Some(&["ISO-8859-10"]),
        41 => Some(&["TIS-620"]),
        46 => Some(&["ISO-8859-15"]),
        47 => Some(&["ISO-8859-13"]),
        170 => Some(&["CP1250", "WINDOWS-1250"]),
        171 => Some(&["CP1251", "WINDOWS-1251"]),
        172 => Some(&["CP1253", "WINDOWS-1253"]),
        173 => Some(&["CP1254", "WINDOWS-1254"]),
        174 => Some(&["CP1255", "WINDOWS-1255"]),
        175 => Some(&["CP1256", "WINDOWS-1256"]),
        176 => Some(&["CP1257", "WINDOWS-1257"]),
        177 => Some(&["CP1258", "WINDOWS-1258"]),
        178 => Some(&["CP1252", "WINDOWS-1252"]),
        351 => Some(&["CP850"]),
        354 => Some(&["CP437"]),
        368 => Some(&["CP866"]),
        382 => Some(&["CP852"]),
        ORACLE_CHARSET_JA16EUC | ORACLE_CHARSET_JA16EUCTILDE => Some(&["EUC-JP", "EUCJP"]),
        ORACLE_CHARSET_JA16SJIS | ORACLE_CHARSET_JA16SJISTILDE => {
            Some(&["CP932", "WINDOWS-31J", "SHIFT_JIS"])
        }
        ORACLE_CHARSET_KO16KSC5601 => Some(&["EUC-KR"]),
        ORACLE_CHARSET_KO16MSWIN949 => Some(&["CP949", "MS949", "EUC-KR"]),
        ORACLE_CHARSET_ZHS16GBK => Some(&["GBK", "CP936"]),
        ORACLE_CHARSET_ZHT16BIG5 | ORACLE_CHARSET_ZHT16MSWIN950 => Some(&["BIG5"]),
        ORACLE_CHARSET_ZHT16HKSCS => Some(&["BIG5-HKSCS", "BIG5"]),
        ORACLE_CHARSET_AL16UTF16 => Some(&["UTF-16BE"]),
        _ => None,
    }
}

#[cfg(unix)]
fn decode_with_iconv_any(
    bytes: &[u8],
    encodings: &[&'static str],
) -> Result<String, OracleThinError> {
    let mut last_error = None;
    for encoding in encodings {
        match decode_with_iconv(bytes, encoding) {
            Ok(text) => return Ok(text),
            Err(err) => last_error = Some(err),
        }
    }
    Err(last_error.unwrap_or_else(|| OracleThinError::new("missing iconv encoding candidate")))
}

#[cfg(unix)]
fn encode_with_iconv_any(
    bytes: &[u8],
    encodings: &[&'static str],
) -> Result<Vec<u8>, OracleThinError> {
    let mut last_error = None;
    for encoding in encodings {
        match encode_with_iconv(bytes, encoding) {
            Ok(text) => return Ok(text),
            Err(err) => last_error = Some(err),
        }
    }
    Err(last_error.unwrap_or_else(|| OracleThinError::new("missing iconv encoding candidate")))
}

#[cfg(windows)]
fn decode_with_iconv_any(
    bytes: &[u8],
    encodings: &[&'static str],
) -> Result<String, OracleThinError> {
    let mut last_error = None;
    for encoding in encodings {
        let Some(code_pages) = windows_code_pages_for_encoding(encoding) else {
            last_error = Some(OracleThinError::new(format!(
                "Windows does not support Oracle text decoding for encoding {encoding}"
            )));
            continue;
        };
        for code_page in code_pages {
            match decode_with_windows_code_page(bytes, encoding, *code_page) {
                Ok(text) => return Ok(text),
                Err(err) => last_error = Some(err),
            }
        }
    }
    for encoding in encodings {
        let Some(code_pages) = windows_code_pages_for_encoding(encoding) else {
            continue;
        };
        for code_page in code_pages {
            match decode_with_windows_code_page_lossy(bytes, encoding, *code_page) {
                Ok(text) => return Ok(text),
                Err(err) => last_error = Some(err),
            }
        }
    }
    Err(last_error.unwrap_or_else(|| OracleThinError::new("missing Windows encoding candidate")))
}

#[cfg(not(any(unix, windows)))]
fn decode_with_iconv_any(
    _bytes: &[u8],
    encodings: &[&'static str],
) -> Result<String, OracleThinError> {
    Err(OracleThinError::new(format!(
        "Oracle native character set decoding requires iconv; tried {}",
        encodings.join(", ")
    )))
}

#[cfg(unix)]
#[allow(deprecated)]
fn decode_with_iconv(bytes: &[u8], encoding: &str) -> Result<String, OracleThinError> {
    let output = transcode_with_iconv(bytes, encoding, "UTF-8")?;
    String::from_utf8(output)
        .map_err(|err| OracleThinError::new(format!("iconv produced invalid UTF-8 text: {err}")))
}

#[cfg(unix)]
#[allow(deprecated)]
fn encode_with_iconv(bytes: &[u8], encoding: &str) -> Result<Vec<u8>, OracleThinError> {
    transcode_with_iconv(bytes, "UTF-8", encoding)
}

#[cfg(windows)]
fn encode_with_iconv_any(
    bytes: &[u8],
    encodings: &[&'static str],
) -> Result<Vec<u8>, OracleThinError> {
    let text = std::str::from_utf8(bytes).map_err(|err| {
        OracleThinError::new(format!(
            "Oracle native character set encoding input is not UTF-8: {err}"
        ))
    })?;
    let mut last_error = None;
    for encoding in encodings {
        let Some(code_pages) = windows_code_pages_for_encoding(encoding) else {
            last_error = Some(OracleThinError::new(format!(
                "Windows does not support Oracle text encoding for encoding {encoding}"
            )));
            continue;
        };
        for code_page in code_pages {
            match encode_with_windows_code_page(text, encoding, *code_page) {
                Ok(text) => return Ok(text),
                Err(err) => last_error = Some(err),
            }
        }
    }
    Err(last_error.unwrap_or_else(|| OracleThinError::new("missing Windows encoding candidate")))
}

#[cfg(not(any(unix, windows)))]
fn encode_with_iconv_any(
    _bytes: &[u8],
    encodings: &[&'static str],
) -> Result<Vec<u8>, OracleThinError> {
    Err(OracleThinError::new(format!(
        "Oracle native character set encoding requires iconv; tried {}",
        encodings.join(", ")
    )))
}

#[cfg(any(windows, test))]
fn windows_code_pages_for_encoding(encoding: &str) -> Option<&'static [u32]> {
    let normalized = encoding.trim().to_ascii_uppercase().replace('_', "-");
    match normalized.as_str() {
        "US-ASCII" | "ASCII" => Some(&[20127]),
        "ISO-8859-1" => Some(&[28591]),
        "ISO-8859-2" => Some(&[28592]),
        "ISO-8859-3" => Some(&[28593]),
        "ISO-8859-4" => Some(&[28594]),
        "ISO-8859-5" => Some(&[28595]),
        "ISO-8859-6" => Some(&[28596]),
        "ISO-8859-7" => Some(&[28597]),
        "ISO-8859-8" => Some(&[28598]),
        "ISO-8859-9" => Some(&[28599]),
        "ISO-8859-10" => Some(&[28600]),
        "ISO-8859-13" => Some(&[28603]),
        "ISO-8859-15" => Some(&[28605]),
        "TIS-620" => Some(&[874]),
        "CP1250" | "WINDOWS-1250" => Some(&[1250]),
        "CP1251" | "WINDOWS-1251" => Some(&[1251]),
        "CP1252" | "WINDOWS-1252" => Some(&[1252]),
        "CP1253" | "WINDOWS-1253" => Some(&[1253]),
        "CP1254" | "WINDOWS-1254" => Some(&[1254]),
        "CP1255" | "WINDOWS-1255" => Some(&[1255]),
        "CP1256" | "WINDOWS-1256" => Some(&[1256]),
        "CP1257" | "WINDOWS-1257" => Some(&[1257]),
        "CP1258" | "WINDOWS-1258" => Some(&[1258]),
        "CP437" => Some(&[437]),
        "CP850" => Some(&[850]),
        "CP852" => Some(&[852]),
        "CP866" => Some(&[866]),
        "EUC-JP" | "EUCJP" => Some(&[51932, 932]),
        "CP932" | "WINDOWS-31J" | "SHIFT-JIS" => Some(&[932]),
        "EUC-KR" => Some(&[51949, 949]),
        "CP949" | "MS949" => Some(&[949, 51949]),
        "GBK" | "CP936" => Some(&[936]),
        "BIG5" | "BIG5-HKSCS" => Some(&[950]),
        "UTF-16BE" => Some(&[1201]),
        _ => None,
    }
}

#[cfg(windows)]
fn decode_with_windows_code_page(
    bytes: &[u8],
    encoding: &str,
    code_page: u32,
) -> Result<String, OracleThinError> {
    use windows_sys::Win32::Globalization::{MultiByteToWideChar, MB_ERR_INVALID_CHARS};

    if code_page == 1201 {
        return decode_utf16be_oracle_text(bytes);
    }
    if bytes.is_empty() {
        return Ok(String::new());
    }
    let input_len = i32::try_from(bytes.len()).map_err(|_| {
        OracleThinError::new(format!(
            "Oracle text for {encoding} is too large for Windows code page conversion"
        ))
    })?;
    let needed = unsafe {
        MultiByteToWideChar(
            code_page,
            MB_ERR_INVALID_CHARS,
            bytes.as_ptr(),
            input_len,
            std::ptr::null_mut(),
            0,
        )
    };
    if needed <= 0 {
        return Err(OracleThinError::new(format!(
            "Windows failed to decode Oracle text as {encoding} with code page {code_page}: {}",
            std::io::Error::last_os_error()
        )));
    }
    let mut wide = vec![0u16; needed as usize];
    let written = unsafe {
        MultiByteToWideChar(
            code_page,
            MB_ERR_INVALID_CHARS,
            bytes.as_ptr(),
            input_len,
            wide.as_mut_ptr(),
            needed,
        )
    };
    if written <= 0 {
        return Err(OracleThinError::new(format!(
            "Windows failed to decode Oracle text as {encoding} with code page {code_page}: {}",
            std::io::Error::last_os_error()
        )));
    }
    wide.truncate(written as usize);
    String::from_utf16(&wide).map_err(|err| {
        OracleThinError::new(format!(
            "Windows decoded invalid UTF-16 Oracle text for {encoding}: {err}"
        ))
    })
}

#[cfg(windows)]
fn decode_with_windows_code_page_lossy(
    bytes: &[u8],
    encoding: &str,
    code_page: u32,
) -> Result<String, OracleThinError> {
    use windows_sys::Win32::Globalization::MultiByteToWideChar;

    if code_page == 1201 {
        return decode_utf16be_oracle_text(bytes);
    }
    if bytes.is_empty() {
        return Ok(String::new());
    }
    let input_len = i32::try_from(bytes.len()).map_err(|_| {
        OracleThinError::new(format!(
            "Oracle text for {encoding} is too large for Windows code page conversion"
        ))
    })?;
    let needed = unsafe {
        MultiByteToWideChar(
            code_page,
            0,
            bytes.as_ptr(),
            input_len,
            std::ptr::null_mut(),
            0,
        )
    };
    if needed <= 0 {
        return Err(OracleThinError::new(format!(
            "Windows failed to decode Oracle text as {encoding} with code page {code_page}: {}",
            std::io::Error::last_os_error()
        )));
    }
    let mut wide = vec![0u16; needed as usize];
    let written = unsafe {
        MultiByteToWideChar(
            code_page,
            0,
            bytes.as_ptr(),
            input_len,
            wide.as_mut_ptr(),
            needed,
        )
    };
    if written <= 0 {
        return Err(OracleThinError::new(format!(
            "Windows failed to decode Oracle text as {encoding} with code page {code_page}: {}",
            std::io::Error::last_os_error()
        )));
    }
    wide.truncate(written as usize);
    Ok(String::from_utf16_lossy(&wide))
}

#[cfg(windows)]
fn encode_with_windows_code_page(
    text: &str,
    encoding: &str,
    code_page: u32,
) -> Result<Vec<u8>, OracleThinError> {
    use windows_sys::Win32::Globalization::WideCharToMultiByte;

    if code_page == 1201 {
        let mut output = Vec::with_capacity(text.len() * 2);
        for unit in text.encode_utf16() {
            output.extend_from_slice(&unit.to_be_bytes());
        }
        return Ok(output);
    }
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let wide: Vec<u16> = text.encode_utf16().collect();
    let wide_len = i32::try_from(wide.len()).map_err(|_| {
        OracleThinError::new(format!(
            "Oracle bind text for {encoding} is too large for Windows code page conversion"
        ))
    })?;
    let mut used_default_char = 0;
    let needed = unsafe {
        WideCharToMultiByte(
            code_page,
            0,
            wide.as_ptr(),
            wide_len,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
            &mut used_default_char,
        )
    };
    if needed <= 0 {
        return Err(OracleThinError::new(format!(
            "Windows failed to encode Oracle text as {encoding}: {}",
            std::io::Error::last_os_error()
        )));
    }
    if used_default_char != 0 {
        return Err(OracleThinError::new(format!(
            "Oracle text contains characters not representable in {encoding}"
        )));
    }
    let mut output = vec![0u8; needed as usize];
    let mut used_default_char = 0;
    let written = unsafe {
        WideCharToMultiByte(
            code_page,
            0,
            wide.as_ptr(),
            wide_len,
            output.as_mut_ptr(),
            needed,
            std::ptr::null(),
            &mut used_default_char,
        )
    };
    if written <= 0 {
        return Err(OracleThinError::new(format!(
            "Windows failed to encode Oracle text as {encoding}: {}",
            std::io::Error::last_os_error()
        )));
    }
    if used_default_char != 0 {
        return Err(OracleThinError::new(format!(
            "Oracle text contains characters not representable in {encoding}"
        )));
    }
    output.truncate(written as usize);
    Ok(output)
}

#[cfg(unix)]
#[allow(deprecated)]
fn transcode_with_iconv(
    bytes: &[u8],
    from_encoding: &str,
    to_encoding: &str,
) -> Result<Vec<u8>, OracleThinError> {
    let to_encoding = CString::new(to_encoding)
        .map_err(|_| OracleThinError::new("iconv target encoding contains a NUL byte"))?;
    let from_encoding = CString::new(from_encoding)
        .map_err(|_| OracleThinError::new("iconv source encoding contains a NUL byte"))?;
    let cd = unsafe { libc::iconv_open(to_encoding.as_ptr(), from_encoding.as_ptr()) };
    if cd == (-1isize as libc::iconv_t) {
        return Err(OracleThinError::new(format!(
            "iconv does not support Oracle text transcoding: {}",
            std::io::Error::last_os_error()
        )));
    }

    let mut input = bytes.to_vec();
    let mut input_ptr = input.as_mut_ptr() as *mut libc::c_char;
    let mut input_left = input.len() as libc::size_t;
    let mut output = vec![0u8; bytes.len().saturating_mul(4).max(16)];
    let mut output_len = 0usize;

    loop {
        let mut output_ptr = unsafe { output.as_mut_ptr().add(output_len) as *mut libc::c_char };
        let mut output_left = (output.len() - output_len) as libc::size_t;
        let result = unsafe {
            libc::iconv(
                cd,
                &mut input_ptr,
                &mut input_left,
                &mut output_ptr,
                &mut output_left,
            )
        };
        output_len = output.len() - output_left as usize;
        if result != usize::MAX as libc::size_t {
            unsafe {
                libc::iconv_close(cd);
            }
            output.truncate(output_len);
            return Ok(output);
        }

        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::E2BIG) {
            output.resize(output.len().saturating_mul(2).max(output.len() + 16), 0);
            continue;
        }

        unsafe {
            libc::iconv_close(cd);
        }
        return Err(OracleThinError::new(format!(
            "failed to transcode Oracle text with iconv: {err}"
        )));
    }
}

fn decode_oracle_binary_float(bytes: &[u8]) -> Result<String, OracleThinError> {
    let bytes: [u8; 4] = bytes.try_into().map_err(|_| {
        OracleThinError::new(format!(
            "invalid Oracle BINARY_FLOAT length {}",
            bytes.len()
        ))
    })?;
    Ok(format_oracle_binary_float(f32::from_bits(
        decode_oracle_binary_float_bits(bytes),
    )))
}

fn decode_oracle_binary_double(bytes: &[u8]) -> Result<String, OracleThinError> {
    let bytes: [u8; 8] = bytes.try_into().map_err(|_| {
        OracleThinError::new(format!(
            "invalid Oracle BINARY_DOUBLE length {}",
            bytes.len()
        ))
    })?;
    Ok(format_oracle_binary_float(f64::from_bits(
        decode_oracle_binary_double_bits(bytes),
    )))
}

trait FloatFormatEdge: Copy {
    fn is_nan(self) -> bool;
    fn is_finite(self) -> bool;
}

impl FloatFormatEdge for f32 {
    fn is_nan(self) -> bool {
        f32::is_nan(self)
    }

    fn is_finite(self) -> bool {
        f32::is_finite(self)
    }
}

impl FloatFormatEdge for f64 {
    fn is_nan(self) -> bool {
        f64::is_nan(self)
    }

    fn is_finite(self) -> bool {
        f64::is_finite(self)
    }
}

fn format_oracle_binary_float<T>(value: T) -> String
where
    T: std::fmt::Display + FloatFormatEdge,
{
    if value.is_nan() {
        return "nan".to_string();
    }
    let mut text = value.to_string();
    if value.is_finite() && !text.contains('.') && !text.contains('e') && !text.contains('E') {
        text.push_str(".0");
    }
    text
}

fn decode_oracle_binary_float_bits(mut bytes: [u8; 4]) -> u32 {
    if bytes[0] & 0x80 != 0 {
        bytes[0] &= 0x7f;
    } else {
        for byte in &mut bytes {
            *byte = !*byte;
        }
    }
    u32::from_be_bytes(bytes)
}

fn decode_oracle_binary_double_bits(mut bytes: [u8; 8]) -> u64 {
    if bytes[0] & 0x80 != 0 {
        bytes[0] &= 0x7f;
    } else {
        for byte in &mut bytes {
            *byte = !*byte;
        }
    }
    u64::from_be_bytes(bytes)
}

fn encode_vector(value: &OracleVectorValue) -> Result<Vec<u8>, OracleThinError> {
    let (version, flags, format, num_elements, value_bytes_len) = match value {
        OracleVectorValue::Float32(values) => {
            validate_dense_vector(values.len())?;
            (
                TNS_VECTOR_VERSION_BASE,
                TNS_VECTOR_FLAG_NORM | TNS_VECTOR_FLAG_NORM_RESERVED,
                TNS_VECTOR_FORMAT_FLOAT32,
                values.len(),
                values.len() * 4,
            )
        }
        OracleVectorValue::Float64(values) => {
            validate_dense_vector(values.len())?;
            (
                TNS_VECTOR_VERSION_BASE,
                TNS_VECTOR_FLAG_NORM | TNS_VECTOR_FLAG_NORM_RESERVED,
                TNS_VECTOR_FORMAT_FLOAT64,
                values.len(),
                values.len() * 8,
            )
        }
        OracleVectorValue::Int8(values) => {
            validate_dense_vector(values.len())?;
            (
                TNS_VECTOR_VERSION_BASE,
                TNS_VECTOR_FLAG_NORM | TNS_VECTOR_FLAG_NORM_RESERVED,
                TNS_VECTOR_FORMAT_INT8,
                values.len(),
                values.len(),
            )
        }
        OracleVectorValue::Binary(values) => {
            validate_dense_vector(values.len())?;
            (
                TNS_VECTOR_VERSION_WITH_BINARY,
                TNS_VECTOR_FLAG_NORM_RESERVED,
                TNS_VECTOR_FORMAT_BINARY,
                values.len().checked_mul(8).ok_or_else(|| {
                    OracleThinError::new("Oracle VECTOR binary bind has too many dimensions")
                })?,
                values.len(),
            )
        }
        OracleVectorValue::SparseFloat32 {
            num_dimensions,
            indices,
            values,
        } => {
            validate_sparse_vector(indices.len(), values.len())?;
            (
                TNS_VECTOR_VERSION_WITH_SPARSE,
                TNS_VECTOR_FLAG_SPARSE | TNS_VECTOR_FLAG_NORM | TNS_VECTOR_FLAG_NORM_RESERVED,
                TNS_VECTOR_FORMAT_FLOAT32,
                *num_dimensions as usize,
                2 + indices.len() * 4 + values.len() * 4,
            )
        }
        OracleVectorValue::SparseFloat64 {
            num_dimensions,
            indices,
            values,
        } => {
            validate_sparse_vector(indices.len(), values.len())?;
            (
                TNS_VECTOR_VERSION_WITH_SPARSE,
                TNS_VECTOR_FLAG_SPARSE | TNS_VECTOR_FLAG_NORM | TNS_VECTOR_FLAG_NORM_RESERVED,
                TNS_VECTOR_FORMAT_FLOAT64,
                *num_dimensions as usize,
                2 + indices.len() * 4 + values.len() * 8,
            )
        }
        OracleVectorValue::SparseInt8 {
            num_dimensions,
            indices,
            values,
        } => {
            validate_sparse_vector(indices.len(), values.len())?;
            (
                TNS_VECTOR_VERSION_WITH_SPARSE,
                TNS_VECTOR_FLAG_SPARSE | TNS_VECTOR_FLAG_NORM | TNS_VECTOR_FLAG_NORM_RESERVED,
                TNS_VECTOR_FORMAT_INT8,
                *num_dimensions as usize,
                2 + indices.len() * 4 + values.len(),
            )
        }
    };
    let num_elements = u32::try_from(num_elements)
        .map_err(|_| OracleThinError::new("Oracle VECTOR bind has too many elements"))?;
    let mut out = Vec::with_capacity(17 + value_bytes_len);
    out.push(TNS_VECTOR_MAGIC_BYTE);
    out.push(version);
    push_be_u16(&mut out, flags);
    out.push(format);
    push_be_u32(&mut out, num_elements);
    push_be_u64(&mut out, 0);
    match value {
        OracleVectorValue::Float32(values) => {
            for value in values {
                out.extend_from_slice(&encode_oracle_binary_float(*value));
            }
        }
        OracleVectorValue::Float64(values) => {
            for value in values {
                out.extend_from_slice(&encode_oracle_binary_double(*value));
            }
        }
        OracleVectorValue::Int8(values) => {
            out.extend(values.iter().map(|value| *value as u8));
        }
        OracleVectorValue::Binary(values) => {
            out.extend_from_slice(values);
        }
        OracleVectorValue::SparseFloat32 {
            indices, values, ..
        } => {
            write_sparse_vector_indices(&mut out, indices)?;
            for value in values {
                out.extend_from_slice(&encode_oracle_binary_float(*value));
            }
        }
        OracleVectorValue::SparseFloat64 {
            indices, values, ..
        } => {
            write_sparse_vector_indices(&mut out, indices)?;
            for value in values {
                out.extend_from_slice(&encode_oracle_binary_double(*value));
            }
        }
        OracleVectorValue::SparseInt8 {
            indices, values, ..
        } => {
            write_sparse_vector_indices(&mut out, indices)?;
            out.extend(values.iter().map(|value| *value as u8));
        }
    }
    Ok(out)
}

fn validate_dense_vector(num_elements: usize) -> Result<(), OracleThinError> {
    if num_elements == 0 {
        return Err(OracleThinError::new(
            "Oracle VECTOR bind cannot contain zero dimensions",
        ));
    }
    Ok(())
}

fn validate_sparse_vector(num_indices: usize, num_values: usize) -> Result<(), OracleThinError> {
    if num_indices != num_values {
        return Err(OracleThinError::new(
            "Oracle sparse VECTOR bind requires the same number of indices and values",
        ));
    }
    u16::try_from(num_indices)
        .map(|_| ())
        .map_err(|_| OracleThinError::new("Oracle sparse VECTOR bind has too many elements"))
}

fn write_sparse_vector_indices(out: &mut Vec<u8>, indices: &[u32]) -> Result<(), OracleThinError> {
    let count = u16::try_from(indices.len())
        .map_err(|_| OracleThinError::new("Oracle sparse VECTOR bind has too many elements"))?;
    push_be_u16(out, count);
    for index in indices {
        push_be_u32(out, *index);
    }
    Ok(())
}

fn encode_oracle_binary_float(value: f32) -> [u8; 4] {
    let mut bytes = value.to_bits().to_be_bytes();
    if bytes[0] & 0x80 != 0 {
        for byte in &mut bytes {
            *byte = !*byte;
        }
    } else {
        bytes[0] |= 0x80;
    }
    bytes
}

fn encode_oracle_binary_double(value: f64) -> [u8; 8] {
    let mut bytes = value.to_bits().to_be_bytes();
    if bytes[0] & 0x80 != 0 {
        for byte in &mut bytes {
            *byte = !*byte;
        }
    } else {
        bytes[0] |= 0x80;
    }
    bytes
}

fn decode_oracle_vector(bytes: &[u8]) -> Result<String, OracleThinError> {
    let mut offset = 0;
    let magic = read_vector_u8(bytes, &mut offset)?;
    if magic != TNS_VECTOR_MAGIC_BYTE {
        return Err(OracleThinError::new(format!(
            "invalid Oracle VECTOR magic byte 0x{magic:02x}"
        )));
    }
    let version = read_vector_u8(bytes, &mut offset)?;
    if version > TNS_VECTOR_VERSION_WITH_SPARSE {
        return Err(OracleThinError::new(format!(
            "unsupported Oracle VECTOR version {version}"
        )));
    }
    let flags = read_vector_u16(bytes, &mut offset)?;
    let vector_format = read_vector_u8(bytes, &mut offset)?;
    let num_elements = read_vector_u32(bytes, &mut offset)? as usize;
    if flags & (TNS_VECTOR_FLAG_NORM | TNS_VECTOR_FLAG_NORM_RESERVED) != 0 {
        read_vector_slice(bytes, &mut offset, 8)?;
    }

    if flags & TNS_VECTOR_FLAG_SPARSE != 0 {
        let num_sparse_elements = read_vector_u16(bytes, &mut offset)? as usize;
        let mut indices = Vec::with_capacity(num_sparse_elements);
        for _ in 0..num_sparse_elements {
            indices.push(read_vector_u32(bytes, &mut offset)?.to_string());
        }
        let values =
            decode_oracle_vector_values(bytes, &mut offset, num_sparse_elements, vector_format)?;
        return Ok(format!(
            "SparseVector(dimensions={}, indices=[{}], values=[{}])",
            num_elements,
            indices.join(", "),
            values.join(", ")
        ));
    }

    let values = decode_oracle_vector_values(bytes, &mut offset, num_elements, vector_format)?;
    Ok(format!("[{}]", values.join(", ")))
}

fn decode_oracle_vector_values(
    bytes: &[u8],
    offset: &mut usize,
    num_elements: usize,
    vector_format: u8,
) -> Result<Vec<String>, OracleThinError> {
    let mut values = Vec::new();
    match vector_format {
        TNS_VECTOR_FORMAT_FLOAT32 => {
            values.reserve(num_elements);
            for _ in 0..num_elements {
                let value = read_vector_array::<4>(bytes, offset)?;
                values.push(format_oracle_binary_float(f32::from_bits(
                    decode_oracle_binary_float_bits(value),
                )));
            }
        }
        TNS_VECTOR_FORMAT_FLOAT64 => {
            values.reserve(num_elements);
            for _ in 0..num_elements {
                let value = read_vector_array::<8>(bytes, offset)?;
                values.push(format_oracle_binary_float(f64::from_bits(
                    decode_oracle_binary_double_bits(value),
                )));
            }
        }
        TNS_VECTOR_FORMAT_INT8 => {
            values.reserve(num_elements);
            for _ in 0..num_elements {
                let value = read_vector_u8(bytes, offset)? as i8;
                values.push(value.to_string());
            }
        }
        TNS_VECTOR_FORMAT_BINARY => {
            let num_bytes = num_elements / 8;
            values.reserve(num_bytes);
            for _ in 0..num_bytes {
                values.push(read_vector_u8(bytes, offset)?.to_string());
            }
        }
        other => {
            return Err(OracleThinError::new(format!(
                "unsupported Oracle VECTOR format {other}"
            )))
        }
    }
    Ok(values)
}

fn read_vector_u8(bytes: &[u8], offset: &mut usize) -> Result<u8, OracleThinError> {
    let [value] = read_vector_array::<1>(bytes, offset)?;
    Ok(value)
}

fn read_vector_u16(bytes: &[u8], offset: &mut usize) -> Result<u16, OracleThinError> {
    let value = read_vector_array::<2>(bytes, offset)?;
    Ok(u16::from_be_bytes(value))
}

fn read_vector_u32(bytes: &[u8], offset: &mut usize) -> Result<u32, OracleThinError> {
    let value = read_vector_array::<4>(bytes, offset)?;
    Ok(u32::from_be_bytes(value))
}

fn read_vector_array<const N: usize>(
    bytes: &[u8],
    offset: &mut usize,
) -> Result<[u8; N], OracleThinError> {
    let slice = read_vector_slice(bytes, offset, N)?;
    slice.try_into().map_err(|_| {
        OracleThinError::new(format!(
            "invalid Oracle VECTOR payload at offset {}",
            offset.saturating_sub(N)
        ))
    })
}

fn read_vector_slice<'a>(
    bytes: &'a [u8],
    offset: &mut usize,
    len: usize,
) -> Result<&'a [u8], OracleThinError> {
    let start = *offset;
    let end = start.checked_add(len).ok_or_else(|| {
        OracleThinError::new(format!("invalid Oracle VECTOR payload offset {start}"))
    })?;
    let slice = bytes.get(start..end).ok_or_else(|| {
        OracleThinError::new(format!(
            "truncated Oracle VECTOR payload at offset {start} length {len}"
        ))
    })?;
    *offset = end;
    Ok(slice)
}

fn decode_oracle_interval_ym(bytes: &[u8]) -> Result<String, OracleThinError> {
    if bytes.len() != 5 {
        return Err(OracleThinError::new(format!(
            "invalid Oracle INTERVAL YEAR TO MONTH length {}",
            bytes.len()
        )));
    }
    let years =
        i64::from(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])) - TNS_DURATION_MID;
    let months = i32::from(bytes[4]) - TNS_DURATION_OFFSET;
    let sign = if years < 0 || months < 0 { '-' } else { '+' };
    Ok(format!("{sign}{:02}-{:02}", years.abs(), months.abs()))
}

fn encode_oracle_interval_ym(value: &OracleIntervalYearMonth) -> Result<Vec<u8>, OracleThinError> {
    let years = encode_duration_u32(i64::from(value.years), "INTERVAL YEAR TO MONTH years")?;
    let months = encode_duration_u8(i32::from(value.months), "INTERVAL YEAR TO MONTH months")?;
    let mut out = Vec::with_capacity(5);
    out.extend_from_slice(&years.to_be_bytes());
    out.push(months);
    Ok(out)
}

fn decode_oracle_interval_ds(bytes: &[u8]) -> Result<String, OracleThinError> {
    if bytes.len() != 11 {
        return Err(OracleThinError::new(format!(
            "invalid Oracle INTERVAL DAY TO SECOND length {}",
            bytes.len()
        )));
    }
    let days =
        i64::from(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])) - TNS_DURATION_MID;
    let hours = i32::from(bytes[4]) - TNS_DURATION_OFFSET;
    let minutes = i32::from(bytes[5]) - TNS_DURATION_OFFSET;
    let seconds = i32::from(bytes[6]) - TNS_DURATION_OFFSET;
    let fseconds = i64::from(u32::from_be_bytes([
        bytes[7], bytes[8], bytes[9], bytes[10],
    ])) - TNS_DURATION_MID;
    let sign = if days < 0 || hours < 0 || minutes < 0 || seconds < 0 || fseconds < 0 {
        '-'
    } else {
        '+'
    };
    Ok(format!(
        "{sign}{:02} {:02}:{:02}:{:02}.{:06}",
        days.abs(),
        hours.abs(),
        minutes.abs(),
        seconds.abs(),
        (fseconds / 1_000).abs()
    ))
}

fn encode_oracle_interval_ds(value: &OracleIntervalDaySecond) -> Result<Vec<u8>, OracleThinError> {
    let days = encode_duration_u32(i64::from(value.days), "INTERVAL DAY TO SECOND days")?;
    let hours = encode_duration_u8(i32::from(value.hours), "INTERVAL DAY TO SECOND hours")?;
    let minutes = encode_duration_u8(i32::from(value.minutes), "INTERVAL DAY TO SECOND minutes")?;
    let seconds = encode_duration_u8(i32::from(value.seconds), "INTERVAL DAY TO SECOND seconds")?;
    let fseconds = encode_duration_u32(
        i64::from(value.nanoseconds),
        "INTERVAL DAY TO SECOND fractional seconds",
    )?;
    let mut out = Vec::with_capacity(11);
    out.extend_from_slice(&days.to_be_bytes());
    out.push(hours);
    out.push(minutes);
    out.push(seconds);
    out.extend_from_slice(&fseconds.to_be_bytes());
    Ok(out)
}

fn encode_duration_u32(value: i64, label: &str) -> Result<u32, OracleThinError> {
    let encoded = value + TNS_DURATION_MID;
    u32::try_from(encoded)
        .map_err(|_| OracleThinError::new(format!("Oracle {label} is out of range")))
}

fn encode_duration_u8(value: i32, label: &str) -> Result<u8, OracleThinError> {
    let encoded = value + TNS_DURATION_OFFSET;
    u8::try_from(encoded)
        .map_err(|_| OracleThinError::new(format!("Oracle {label} is out of range")))
}

fn decode_oracle_unsigned_integer(bytes: &[u8]) -> Result<u64, OracleThinError> {
    if bytes.len() > 8 {
        return Err(OracleThinError::new(format!(
            "invalid Oracle UB8 length {}",
            bytes.len()
        )));
    }
    let mut value = 0_u64;
    for byte in bytes {
        value = (value << 8) | u64::from(*byte);
    }
    Ok(value)
}

fn decode_oracle_time_text(bytes: &[u8]) -> Result<String, OracleThinError> {
    let value = decode_oracle_datetime(bytes)?;
    let mut out = format!("{:02}:{:02}:{:02}", value.hour, value.minute, value.second);
    if value.nanosecond > 0 {
        out.push_str(&format!(".{:09}", value.nanosecond));
        while out.ends_with('0') {
            out.pop();
        }
    }
    if let Some(suffix) = value.timezone_suffix() {
        out.push_str(&suffix);
    }
    Ok(out)
}

fn decode_oracle_datetime(bytes: &[u8]) -> Result<crate::OracleDateTime, OracleThinError> {
    if bytes.len() < 7 {
        return Err(OracleThinError::new("short Oracle date/time value"));
    }
    let signed_year = (i16::from(bytes[0]) - 100) * 100 + i16::from(bytes[1]) - 100;
    let year = validate_oracle_datetime_year(signed_year)?;
    let nanosecond = if bytes.len() > 10 {
        u32::from_be_bytes([bytes[7], bytes[8], bytes[9], bytes[10]])
    } else {
        0
    };
    let mut timezone_region_id = None;
    let mut timezone_region_time_in_zone = false;
    let timezone_offset_minutes = if bytes.len() > 12 && bytes[11] != 0 && bytes[12] != 0 {
        if bytes[11] & 0x80 != 0 {
            timezone_region_id =
                Some((u16::from(bytes[11] & 0x7f) << 6) | (u16::from(bytes[12] & 0xfc) >> 2));
            timezone_region_time_in_zone = bytes[12] & 0x01 != 0;
            None
        } else {
            let hour = i16::from(bytes[11]) - 20;
            let minute = i16::from(bytes[12]) - 60;
            Some(hour * 60 + minute)
        }
    } else {
        None
    };
    let mut value = crate::OracleDateTime {
        year,
        month: bytes[2],
        day: bytes[3],
        hour: bytes[4].saturating_sub(1),
        minute: bytes[5].saturating_sub(1),
        second: bytes[6].saturating_sub(1),
        nanosecond,
        timezone_offset_minutes,
        timezone_region_id,
    };
    if let Some(offset) = timezone_offset_minutes {
        apply_timezone_offset(&mut value, offset);
    } else if let Some(region_id) = timezone_region_id {
        if !timezone_region_time_in_zone {
            if let Some(offset) = timezone_region_offset_minutes(region_id, &value) {
                apply_timezone_offset(&mut value, offset);
            }
        }
    }
    validate_oracle_datetime_year(i16::try_from(value.year).unwrap_or(i16::MAX))?;
    Ok(value)
}

fn validate_oracle_datetime_year(year: i16) -> Result<u16, OracleThinError> {
    u16::try_from(year)
        .ok()
        .filter(|year| (1..=9999).contains(year))
        .ok_or_else(|| {
            OracleThinError::new(format!(
                "Oracle date/time year {year} is outside supported range 1..=9999"
            ))
        })
}

#[derive(Debug, Clone)]
struct ZoneInfo {
    transitions: Vec<(i64, usize)>,
    offsets: Vec<i32>,
}

impl ZoneInfo {
    fn offset_at(&self, unix_seconds: i64) -> Option<i32> {
        let mut type_index = 0;
        for (transition, index) in &self.transitions {
            if unix_seconds < *transition {
                break;
            }
            type_index = *index;
        }
        self.offsets.get(type_index).copied()
    }
}

static ZONE_INFO_CACHE: OnceCell<Mutex<HashMap<&'static str, Option<ZoneInfo>>>> = OnceCell::new();

fn timezone_region_offset_minutes(region_id: u16, value: &crate::OracleDateTime) -> Option<i16> {
    let zone_name = crate::oracle_zones::oracle_zone_name(region_id)?;
    let unix_seconds = oracle_datetime_to_unix_seconds(value)?;
    let zone_info = cached_zone_info(zone_name)?;
    let offset_seconds = zone_info.offset_at(unix_seconds)?;
    i16::try_from(offset_seconds / 60).ok()
}

fn timezone_region_local_offset_minutes(
    region_id: u16,
    value: &crate::OracleDateTime,
) -> Option<i16> {
    let zone_name = crate::oracle_zones::oracle_zone_name(region_id)?;
    let local_seconds = oracle_datetime_to_unix_seconds(value)?;
    let zone_info = cached_zone_info(zone_name)?;
    for offset_seconds in &zone_info.offsets {
        let utc_seconds = local_seconds - i64::from(*offset_seconds);
        if zone_info.offset_at(utc_seconds) == Some(*offset_seconds) {
            return i16::try_from(offset_seconds / 60).ok();
        }
    }
    let offset_seconds = zone_info.offset_at(local_seconds)?;
    i16::try_from(offset_seconds / 60).ok()
}

fn cached_zone_info(zone_name: &'static str) -> Option<ZoneInfo> {
    let cache = ZONE_INFO_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().ok()?;
    if let Some(zone_info) = guard.get(zone_name) {
        return zone_info.clone();
    }
    let zone_info = load_zone_info(zone_name);
    guard.insert(zone_name, zone_info.clone());
    zone_info
}

fn load_zone_info(zone_name: &str) -> Option<ZoneInfo> {
    for base in ["/usr/share/zoneinfo", "/var/db/timezone/zoneinfo"] {
        let path = Path::new(base).join(zone_name);
        if let Ok(bytes) = fs::read(path) {
            if let Some(zone_info) = parse_tzif(&bytes) {
                return Some(zone_info);
            }
        }
    }
    None
}

fn parse_tzif(bytes: &[u8]) -> Option<ZoneInfo> {
    let first = parse_tzif_section(bytes, 0, 4)?;
    match bytes.get(4).copied() {
        Some(b'2' | b'3' | b'4') => parse_tzif_section(bytes, first.next_offset, 8).map(|s| s.info),
        _ => Some(first.info),
    }
}

struct TzifSection {
    info: ZoneInfo,
    next_offset: usize,
}

fn parse_tzif_section(bytes: &[u8], start: usize, time_size: usize) -> Option<TzifSection> {
    if bytes.get(start..start + 4)? != b"TZif" {
        return None;
    }
    let mut pos = start + 20;
    let ttisgmtcnt = read_tzif_u32(bytes, &mut pos)? as usize;
    let ttisstdcnt = read_tzif_u32(bytes, &mut pos)? as usize;
    let leapcnt = read_tzif_u32(bytes, &mut pos)? as usize;
    let timecnt = read_tzif_u32(bytes, &mut pos)? as usize;
    let typecnt = read_tzif_u32(bytes, &mut pos)? as usize;
    let charcnt = read_tzif_u32(bytes, &mut pos)? as usize;

    let mut transition_times = Vec::with_capacity(timecnt);
    for _ in 0..timecnt {
        transition_times.push(if time_size == 8 {
            read_tzif_i64(bytes, &mut pos)?
        } else {
            i64::from(read_tzif_i32(bytes, &mut pos)?)
        });
    }

    let transition_indices = bytes.get(pos..pos + timecnt)?;
    pos += timecnt;

    let mut offsets = Vec::with_capacity(typecnt);
    for _ in 0..typecnt {
        offsets.push(read_tzif_i32(bytes, &mut pos)?);
        pos += 2;
    }

    pos += charcnt;
    pos += leapcnt * (time_size + 4);
    pos += ttisstdcnt;
    pos += ttisgmtcnt;
    bytes.get(pos.saturating_sub(1)..pos)?;

    let transitions = transition_times
        .into_iter()
        .zip(transition_indices.iter().map(|index| usize::from(*index)))
        .collect();
    Some(TzifSection {
        info: ZoneInfo {
            transitions,
            offsets,
        },
        next_offset: pos,
    })
}

fn read_tzif_u32(bytes: &[u8], pos: &mut usize) -> Option<u32> {
    let raw = bytes.get(*pos..*pos + 4)?;
    *pos += 4;
    Some(u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn read_tzif_i32(bytes: &[u8], pos: &mut usize) -> Option<i32> {
    read_tzif_u32(bytes, pos).map(|value| value as i32)
}

fn read_tzif_i64(bytes: &[u8], pos: &mut usize) -> Option<i64> {
    let raw = bytes.get(*pos..*pos + 8)?;
    *pos += 8;
    Some(i64::from_be_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}

fn oracle_datetime_to_unix_seconds(value: &crate::OracleDateTime) -> Option<i64> {
    let days = days_from_civil(
        i32::from(value.year),
        u32::from(value.month),
        u32::from(value.day),
    )?;
    Some(
        days * 86_400
            + i64::from(value.hour) * 3_600
            + i64::from(value.minute) * 60
            + i64::from(value.second),
    )
}

fn days_from_civil(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year = year - i32::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let month = i32::try_from(month).ok()?;
    let day = i32::try_from(day).ok()?;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(i64::from(era * 146_097 + doe - 719_468))
}

fn apply_timezone_offset(value: &mut crate::OracleDateTime, offset_minutes: i16) {
    let total_minutes =
        i32::from(value.hour) * 60 + i32::from(value.minute) + i32::from(offset_minutes);
    let day_delta = total_minutes.div_euclid(24 * 60);
    let minute_of_day = total_minutes.rem_euclid(24 * 60);
    value.hour = (minute_of_day / 60) as u8;
    value.minute = (minute_of_day % 60) as u8;
    shift_date_by_days(value, day_delta);
}

fn shift_date_by_days(value: &mut crate::OracleDateTime, day_delta: i32) {
    if day_delta > 0 {
        for _ in 0..day_delta {
            let days_in_month = days_in_month(value.year, value.month);
            if value.day < days_in_month {
                value.day += 1;
            } else {
                value.day = 1;
                if value.month < 12 {
                    value.month += 1;
                } else {
                    value.month = 1;
                    value.year += 1;
                }
            }
        }
    } else {
        for _ in 0..day_delta.unsigned_abs() {
            if value.day > 1 {
                value.day -= 1;
            } else if value.month > 1 {
                value.month -= 1;
                value.day = days_in_month(value.year, value.month);
            } else {
                value.year = value.year.saturating_sub(1);
                value.month = 12;
                value.day = days_in_month(value.year, value.month);
            }
        }
    }
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 31,
    }
}

fn is_leap_year(year: u16) -> bool {
    year % 4 == 0 && year % 100 != 0 || year % 400 == 0
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

fn process_return_parameters(
    cursor: &mut PacketCursor<'_>,
    capabilities: &OracleThinCapabilities,
    state: &mut ServerSidePiggybackState,
) -> Result<(), OracleThinError> {
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
        let mut text_value = None;
        let mut binary_value = None;
        let text_len = cursor.read_ub2()?;
        if text_len > 0 {
            if let Some(bytes) = cursor.read_bytes()? {
                text_value = Some(decode_oracle_text(&bytes, CS_FORM_IMPLICIT, capabilities)?);
            }
        }
        let binary_len = cursor.read_ub2()?;
        if binary_len > 0 {
            binary_value = cursor.read_bytes()?;
        }
        let keyword_num = cursor.read_ub2()?;
        match keyword_num {
            TNS_KEYWORD_NUM_CURRENT_SCHEMA => {
                state.current_schema = text_value;
            }
            TNS_KEYWORD_NUM_EDITION => {
                state.edition = text_value;
            }
            TNS_KEYWORD_NUM_TRANSACTION_ID => {
                if let Some(value) = binary_value {
                    update_sessionless_transaction_state(state, &value)?;
                }
            }
            _ => {}
        }
    }
    let num_bytes = cursor.read_ub2()? as usize;
    if num_bytes > 0 {
        cursor.skip(num_bytes)?;
    }
    Ok(())
}

fn update_sessionless_transaction_state(
    state: &mut ServerSidePiggybackState,
    data: &[u8],
) -> Result<(), OracleThinError> {
    if data.len() < 2 {
        return Err(OracleThinError::new(
            "short Oracle sessionless transaction sync data",
        ));
    }
    let transaction_id = &data[..data.len() - 2];
    let sessionless_state = data[data.len() - 2];
    let sync_version = data[data.len() - 1];
    if sync_version != 1 {
        return Err(OracleThinError::new(format!(
            "unknown Oracle sessionless transaction sync version {sync_version}"
        )));
    }
    if sessionless_state & TNS_TPC_TXNID_SYNC_UNSET != 0 {
        state.sessionless_transaction_id = None;
        state.sessionless_started_on_server = false;
        state.transaction_in_progress = false;
    } else if sessionless_state & TNS_TPC_TXNID_SYNC_SET != 0 {
        state.sessionless_transaction_id = Some(transaction_id.to_vec());
        state.sessionless_started_on_server = sessionless_state & TNS_TPC_TXNID_SYNC_SERVER != 0;
        state.transaction_in_progress = true;
    }
    Ok(())
}

fn process_server_side_piggyback(
    cursor: &mut PacketCursor<'_>,
    capabilities: &OracleThinCapabilities,
    state: &mut ServerSidePiggybackState,
) -> Result<(), OracleThinError> {
    let opcode = cursor.read_u8()?;
    match opcode {
        TNS_SERVER_PIGGYBACK_LTXID => {
            state.ltxid = cursor.read_bytes_with_ub4_length()?.unwrap_or_default();
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
            process_keyword_value_pairs(cursor, num_elements, capabilities, state)?;
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
            let flags = cursor.read_ub4()?;
            state.session_changed = flags & 4 != 0;
            state.session_id = Some(cursor.read_ub4()?);
            state.serial_num = Some(cursor.read_ub2()?);
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
    capabilities: &OracleThinCapabilities,
    state: &mut ServerSidePiggybackState,
) -> Result<(), OracleThinError> {
    for _ in 0..num_pairs {
        let mut text_value = None;
        let mut binary_value = None;
        if cursor.read_ub2()? > 0 {
            if let Some(bytes) = cursor.read_bytes()? {
                text_value = Some(decode_oracle_text(&bytes, CS_FORM_IMPLICIT, capabilities)?);
            }
        }
        if cursor.read_ub2()? > 0 {
            binary_value = cursor.read_bytes()?;
        }
        let keyword_num = cursor.read_ub2()?;
        match keyword_num {
            TNS_KEYWORD_NUM_CURRENT_SCHEMA => {
                state.current_schema = text_value;
            }
            TNS_KEYWORD_NUM_EDITION => {
                state.edition = text_value;
            }
            TNS_KEYWORD_NUM_TRANSACTION_ID => {
                if let Some(value) = binary_value {
                    update_sessionless_transaction_state(state, &value)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn process_warning(
    cursor: &mut PacketCursor<'_>,
    capabilities: &OracleThinCapabilities,
) -> Result<Option<OracleThinWarning>, OracleThinError> {
    let code = u32::from(cursor.read_ub2()?);
    let num_bytes = cursor.read_ub2()?;
    let _ = cursor.read_ub2()?;
    if code == 0 || num_bytes == 0 {
        return Ok(None);
    }
    let bytes = cursor.read_raw(num_bytes as usize)?;
    let message = decode_oracle_text(bytes, CS_FORM_IMPLICIT, capabilities)?
        .trim_end()
        .to_string();
    Ok(Some(OracleThinWarning { code, message }))
}

fn process_status(
    cursor: &mut PacketCursor<'_>,
    capabilities: &OracleThinCapabilities,
    state: &mut ServerSidePiggybackState,
) -> Result<(), OracleThinError> {
    let call_status = cursor.read_ub4()?;
    let _ = cursor.read_ub2()?;
    update_transaction_status_from_call_status(state, capabilities, call_status);
    Ok(())
}

fn update_transaction_status_from_call_status(
    state: &mut ServerSidePiggybackState,
    capabilities: &OracleThinCapabilities,
    call_status: u32,
) {
    if capabilities.supports_end_of_call_status {
        state.transaction_in_progress = call_status & TNS_EOCS_FLAGS_TXN_IN_PROGRESS != 0;
    }
}

fn process_token(cursor: &mut PacketCursor<'_>, expected: u64) -> Result<(), OracleThinError> {
    let token = cursor.read_ub8()?;
    if token != expected {
        return Err(OracleThinError::new(format!(
            "mismatched Oracle token number {token}; expected {expected}"
        )));
    }
    Ok(())
}

fn process_execute_error(
    cursor: &mut PacketCursor<'_>,
    capabilities: &OracleThinCapabilities,
) -> Result<ExecuteError, OracleThinError> {
    if capabilities
        .protocol_version
        .is_some_and(|version| version < TNS_VERSION_MIN_ACCEPTED)
    {
        return process_legacy_execute_error(cursor, capabilities);
    }

    let call_status = cursor.read_ub4()?;
    let _ = cursor.read_ub2()?;
    let _ = cursor.read_ub4()?;
    let _ = cursor.read_ub2()?;
    let _ = cursor.read_ub2()?;
    let _ = cursor.read_ub2()?;
    let cursor_id = cursor.read_ub2()? as u32;
    let error_pos = cursor.read_sb2()?;
    cursor.skip(5)?;
    let flags = cursor.read_u8()?;
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
    let message = execute_error_message(cursor, code, error_pos)?;
    let warning = if flags & 0x20 != 0 {
        Some(OracleThinWarning {
            code: TNS_WARN_COMPILATION_ERROR,
            message: "creation succeeded with compilation errors".to_string(),
        })
    } else {
        None
    };
    Ok(ExecuteError {
        code,
        cursor_id,
        call_status,
        _rowcount: rowcount,
        message,
        warning,
    })
}

fn process_legacy_execute_error(
    cursor: &mut PacketCursor<'_>,
    capabilities: &OracleThinCapabilities,
) -> Result<ExecuteError, OracleThinError> {
    let call_status = if capabilities.supports_end_of_call_status {
        cursor.read_ub4()?
    } else {
        0
    };
    if capabilities.ttc_field_version >= 3 && capabilities.supports_fast_session_attributes {
        let _ = cursor.read_ub2()?;
    }
    let cur_row_number = u64::from(cursor.read_ub4()?);
    let initial_code = cursor.read_ub2()? as u32;
    let _ = cursor.read_ub2()?;
    let _ = cursor.read_ub2()?;
    let cursor_id = cursor.read_ub2()? as u32;
    let _error_pos = cursor.read_sb2()?;
    cursor.skip(2)?;
    cursor.skip(2)?;
    cursor.skip(2)?;
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

    let (mut code, mut rowcount) = if capabilities.ttc_field_version < 7 {
        cursor.skip_bytes_with_ub4_length()?;
        cursor.skip_bytes_with_ub4_length()?;
        cursor.skip_bytes_with_ub4_length()?;
        (initial_code, cur_row_number)
    } else {
        let num_errors = cursor.read_ub2()? as usize;
        if num_errors > 0 {
            let first_byte = cursor.read_u8()?;
            for _ in 0..num_errors {
                if first_byte == 0xfe {
                    if capabilities.supports_big_clr_chunks {
                        let _ = cursor.read_ub4()?;
                    } else {
                        let _ = cursor.read_u8()?;
                    }
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
                    if capabilities.supports_big_clr_chunks {
                        let _ = cursor.read_ub4()?;
                    } else {
                        let _ = cursor.read_u8()?;
                    }
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

        (cursor.read_ub4()?, cursor.read_ub8()?)
    };
    if capabilities.ttc_field_version < 7 && code != 0 && capabilities.server_ttc_field_version >= 7
    {
        code = cursor.read_ub4()?;
        rowcount = cursor.read_ub8()?;
    }
    if capabilities.ttc_field_version >= TNS_CCAP_FIELD_VERSION_20_1
        || (capabilities.ttc_field_version < 7
            && code != 0
            && capabilities.server_ttc_field_version >= TNS_CCAP_FIELD_VERSION_20_1)
    {
        let _ = cursor.read_ub4()?;
        let _ = cursor.read_ub4()?;
    }
    let message = execute_legacy_error_message(cursor, code)?;
    Ok(ExecuteError {
        code,
        cursor_id,
        call_status,
        _rowcount: rowcount,
        message,
        warning: None,
    })
}

fn execute_error_message(
    cursor: &mut PacketCursor<'_>,
    code: u32,
    error_pos: i16,
) -> Result<Option<String>, OracleThinError> {
    if code == 0 {
        return Ok(None);
    }
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
    Ok(Some(message))
}

fn execute_legacy_error_message(
    cursor: &mut PacketCursor<'_>,
    code: u32,
) -> Result<Option<String>, OracleThinError> {
    if code == 0 {
        return Ok(None);
    }
    let mut message = cursor.read_str()?.unwrap_or_default();
    while message.ends_with(char::is_whitespace) {
        message.pop();
    }
    Ok(Some(message))
}

#[derive(Debug, Default)]
struct AuthResult {
    server_version: Option<String>,
    combo_key: Option<Vec<u8>>,
    server_state: ServerSidePiggybackState,
}

#[derive(Debug, Default)]
struct AuthState {
    session_data: HashMap<String, String>,
    verifier_type: u32,
    combo_key: Option<Vec<u8>>,
    server_version: Option<String>,
    server_state: ServerSidePiggybackState,
    auth_uses_pbkdf2_key_derivation: bool,
}

fn authenticate(
    stream: &mut TcpStream,
    config: &OracleThinConfig,
    capabilities: &OracleThinCapabilities,
) -> Result<AuthResult, OracleThinError> {
    let mut state = AuthState {
        auth_uses_pbkdf2_key_derivation: capabilities.auth_uses_pbkdf2_key_derivation,
        ..AuthState::default()
    };
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
        combo_key: state.combo_key,
        server_state: state.server_state,
    })
}

fn write_auth_phase_one(
    stream: &mut TcpStream,
    config: &OracleThinConfig,
    capabilities: &OracleThinCapabilities,
) -> Result<(), OracleThinError> {
    let payload = auth_phase_one_payload(config, capabilities)?;
    write_data_packet(
        stream,
        capabilities.protocol_version.unwrap_or(319),
        capabilities.data_packet_chunk_size(),
        &payload,
    )
}

fn auth_phase_one_payload(
    config: &OracleThinConfig,
    capabilities: &OracleThinCapabilities,
) -> Result<Vec<u8>, OracleThinError> {
    let mut payload = Vec::new();
    write_function_code(&mut payload, TNS_FUNC_AUTH_PHASE_ONE, 1, capabilities);
    write_auth_header(
        &mut payload,
        &config.username,
        TNS_AUTH_MODE_LOGON | config.auth_mode.tns_bits(),
        5,
    )?;
    write_key_value(&mut payload, "AUTH_TERMINAL", &config.terminal, 0)?;
    write_key_value(&mut payload, "AUTH_PROGRAM_NM", &config.program, 0)?;
    write_key_value(&mut payload, "AUTH_MACHINE", &config.machine, 0)?;
    write_key_value(&mut payload, "AUTH_PID", &std::process::id().to_string(), 0)?;
    write_key_value(&mut payload, "AUTH_SID", &config.os_user, 0)?;
    Ok(payload)
}

fn write_auth_phase_two(
    stream: &mut TcpStream,
    config: &OracleThinConfig,
    capabilities: &OracleThinCapabilities,
    credentials: &AuthCredentials,
) -> Result<(), OracleThinError> {
    let payload = auth_phase_two_payload(config, capabilities, credentials)?;
    write_data_packet(
        stream,
        capabilities.protocol_version.unwrap_or(319),
        capabilities.data_packet_chunk_size(),
        &payload,
    )
}

fn auth_phase_two_payload(
    config: &OracleThinConfig,
    capabilities: &OracleThinCapabilities,
    credentials: &AuthCredentials,
) -> Result<Vec<u8>, OracleThinError> {
    let mut payload = Vec::new();
    write_function_code(&mut payload, TNS_FUNC_AUTH_PHASE_TWO, 2, capabilities);
    let mut num_pairs = 7;
    if credentials.speedy_key.is_some() {
        num_pairs += 1;
    }
    if config.proxy_user.is_some() {
        num_pairs += 1;
    }
    if config.connection_class.is_some() {
        num_pairs += 1;
    }
    if config.purity.tns_value() != 0 {
        num_pairs += 1;
    }
    if config.edition.is_some() {
        num_pairs += 1;
    }
    num_pairs += (config.app_context.len() * 3) as u32;
    if credentials.debug_jdwp_data.is_some() {
        num_pairs += 1;
    }
    write_auth_header(
        &mut payload,
        &config.username,
        TNS_AUTH_MODE_LOGON | TNS_AUTH_MODE_WITH_PASSWORD | config.auth_mode.tns_bits(),
        num_pairs,
    )?;
    if let Some(proxy_user) = config.proxy_user.as_deref() {
        write_key_value(&mut payload, "PROXY_CLIENT_NAME", proxy_user, 0)?;
    }
    write_key_value(&mut payload, "AUTH_SESSKEY", &credentials.session_key, 1)?;
    if let Some(speedy_key) = credentials.speedy_key.as_deref() {
        write_key_value(&mut payload, "AUTH_PBKDF2_SPEEDY_KEY", speedy_key, 0)?;
    }
    let driver_name = config
        .driver_name
        .clone()
        .unwrap_or_else(oracle_thin_driver_name);
    write_key_value(&mut payload, "AUTH_PASSWORD", &credentials.password, 0)?;
    write_key_value(&mut payload, "SESSION_CLIENT_CHARSET", "873", 0)?;
    write_key_value(&mut payload, "SESSION_CLIENT_DRIVER_NAME", &driver_name, 0)?;
    write_key_value(&mut payload, "SESSION_CLIENT_VERSION", "0", 0)?;
    write_key_value(
        &mut payload,
        "AUTH_ALTER_SESSION",
        &alter_session_timezone_statement(),
        1,
    )?;
    if let Some(connection_class) = config.connection_class.as_deref() {
        write_key_value(&mut payload, "AUTH_KPPL_CONN_CLASS", connection_class, 0)?;
    }
    let purity = config.purity.tns_value();
    if purity != 0 {
        write_key_value(&mut payload, "AUTH_KPPL_PURITY", &purity.to_string(), 1)?;
    }
    if let Some(edition) = config.edition.as_deref() {
        write_key_value(&mut payload, "AUTH_ORA_EDITION", edition, 0)?;
    }
    for entry in &config.app_context {
        write_key_value(&mut payload, "AUTH_APPCTX_NSPACE\0", &entry.namespace, 0)?;
        write_key_value(&mut payload, "AUTH_APPCTX_ATTR\0", &entry.name, 0)?;
        write_key_value(&mut payload, "AUTH_APPCTX_VALUE\0", &entry.value, 0)?;
    }
    if let Some(debug_jdwp_data) = credentials.debug_jdwp_data.as_deref() {
        write_key_value(&mut payload, "AUTH_ORA_DEBUG_JDWP", debug_jdwp_data, 0)?;
    }
    write_key_value(
        &mut payload,
        "AUTH_CONNECT_STRING",
        &auth_connect_string(config)?,
        0,
    )?;
    Ok(payload)
}

fn auth_change_password_payload(
    username: &str,
    old_password: &str,
    new_password: &str,
    combo_key: &[u8],
    salt: &[u8; 16],
    capabilities: &OracleThinCapabilities,
    sequence: u8,
) -> Result<Vec<u8>, OracleThinError> {
    let mut payload = Vec::new();
    write_function_code(
        &mut payload,
        TNS_FUNC_AUTH_PHASE_TWO,
        sequence,
        capabilities,
    );
    write_auth_header(
        &mut payload,
        username,
        TNS_AUTH_MODE_WITH_PASSWORD | TNS_AUTH_MODE_CHANGE_PASSWORD,
        2,
    )?;
    let encoded_password = encode_auth_password(combo_key, old_password.as_bytes(), salt)?;
    let encoded_new_password = encode_auth_password(combo_key, new_password.as_bytes(), salt)?;
    write_key_value(&mut payload, "AUTH_PASSWORD", &encoded_password, 0)?;
    write_key_value(&mut payload, "AUTH_NEWPASSWORD", &encoded_new_password, 0)?;
    Ok(payload)
}

fn oracle_thin_driver_name() -> String {
    format!("space-query-thin thn : {}", env!("CARGO_PKG_VERSION"))
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
    if capabilities.ttc_field_version >= TNS_CCAP_FIELD_VERSION_23_1_EXT_1 {
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
    if capabilities.ttc_field_version >= TNS_CCAP_FIELD_VERSION_23_1_EXT_1 {
        write_ub8(payload, 0);
    }
}

fn write_session_state_piggyback(
    payload: &mut Vec<u8>,
    capabilities: &OracleThinCapabilities,
    sequence: u8,
    state: u8,
) {
    write_piggyback_code(payload, TNS_FUNC_SESSION_STATE, sequence, capabilities);
    write_ub8(
        payload,
        u64::from(state | TNS_SESSION_STATE_EXPLICIT_BOUNDARY),
    );
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
    debug_jdwp_data: Option<String>,
}

fn generate_auth_credentials(
    config: &OracleThinConfig,
    state: &mut AuthState,
) -> Result<AuthCredentials, OracleThinError> {
    match state.verifier_type {
        TNS_VERIFIER_TYPE_12C => generate_12c_auth_credentials(config, state),
        TNS_VERIFIER_TYPE_10G => generate_10g_auth_credentials(config, state),
        TNS_VERIFIER_TYPE_11G_1 | TNS_VERIFIER_TYPE_11G_2 => {
            generate_11g_auth_credentials(config, state)
        }
        other => Err(OracleThinError::new(format!(
            "unsupported Oracle password verifier type 0x{other:x}; supported types are 0x939, 0x1b25, 0xb152, and 0x4815"
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

    generate_auth_credentials_from_password_hash(
        config.password.as_bytes(),
        config.debug_jdwp.as_deref(),
        state,
        &password_hash,
        32,
        Some(&password_key),
    )
}

fn generate_11g_auth_credentials(
    config: &OracleThinConfig,
    state: &mut AuthState,
) -> Result<AuthCredentials, OracleThinError> {
    let verifier_data = hex_decode(required_session_value(state, "AUTH_VFR_DATA")?)?;
    let password_hash = generate_11g_password_hash(config.password.as_bytes(), &verifier_data);
    generate_auth_credentials_from_password_hash(
        config.password.as_bytes(),
        config.debug_jdwp.as_deref(),
        state,
        &password_hash,
        24,
        None,
    )
}

fn generate_10g_auth_credentials(
    config: &OracleThinConfig,
    state: &mut AuthState,
) -> Result<AuthCredentials, OracleThinError> {
    let password_hash = generate_10g_password_hash(&config.username, &config.password);
    generate_auth_credentials_from_password_hash(
        config.password.as_bytes(),
        config.debug_jdwp.as_deref(),
        state,
        &password_hash,
        16,
        None,
    )
}

fn generate_auth_credentials_from_password_hash(
    password: &[u8],
    debug_jdwp: Option<&str>,
    state: &mut AuthState,
    password_hash: &[u8],
    key_len: usize,
    password_key_for_speedy: Option<&[u8]>,
) -> Result<AuthCredentials, OracleThinError> {
    let encoded_server_key = hex_decode(required_session_value(state, "AUTH_SESSKEY")?)?;
    let session_key_part_a = aes_decrypt_cbc_no_padding(password_hash, &encoded_server_key)?;
    let mut session_key_part_b = vec![0u8; session_key_part_a.len()];
    OsRng.fill_bytes(&mut session_key_part_b);
    generate_auth_credentials_from_session_key_parts(
        password,
        debug_jdwp,
        state,
        password_hash,
        key_len,
        password_key_for_speedy,
        &session_key_part_a,
        &session_key_part_b,
    )
}

fn generate_auth_credentials_from_session_key_parts(
    password: &[u8],
    debug_jdwp: Option<&str>,
    state: &mut AuthState,
    password_hash: &[u8],
    key_len: usize,
    password_key_for_speedy: Option<&[u8]>,
    session_key_part_a: &[u8],
    session_key_part_b: &[u8],
) -> Result<AuthCredentials, OracleThinError> {
    let encoded_client_key = aes_encrypt_cbc_pkcs7(password_hash, session_key_part_b)?;
    let session_key = client_session_key_hex(&encoded_client_key, session_key_part_a.len())?;
    let combo_key = derive_auth_combo_key(state, session_key_part_a, session_key_part_b, key_len)?;
    state.combo_key = Some(combo_key.clone());

    let speedy_key = if let Some(password_key) = password_key_for_speedy {
        let mut speedy_salt = [0u8; 16];
        OsRng.fill_bytes(&mut speedy_salt);
        let mut speedy_plain = Vec::with_capacity(16 + password_key.len());
        speedy_plain.extend_from_slice(&speedy_salt);
        speedy_plain.extend_from_slice(password_key);
        let speedy_encrypted = aes_encrypt_cbc_pkcs7(&combo_key, &speedy_plain)?;
        Some(hex_encode_upper(&speedy_encrypted[..80]))
    } else {
        None
    };

    let mut password_salt = [0u8; 16];
    OsRng.fill_bytes(&mut password_salt);
    let mut password_plain = Vec::with_capacity(16 + password.len());
    password_plain.extend_from_slice(&password_salt);
    password_plain.extend_from_slice(password);
    let encrypted_password = aes_encrypt_cbc_pkcs7(&combo_key, &password_plain)?;
    let debug_jdwp_data = encode_debug_jdwp_data(debug_jdwp, &combo_key)?;

    Ok(AuthCredentials {
        session_key,
        speedy_key,
        password: hex_encode_upper(&encrypted_password),
        debug_jdwp_data,
    })
}

fn encode_debug_jdwp_data(
    debug_jdwp: Option<&str>,
    combo_key: &[u8],
) -> Result<Option<String>, OracleThinError> {
    let Some(debug_jdwp) = debug_jdwp else {
        return Ok(None);
    };
    let encrypted = aes_encrypt_cbc_zero_padding(combo_key, debug_jdwp.as_bytes())?;
    Ok(Some(format!("{}01", hex_encode_upper(&encrypted))))
}

fn encode_auth_password(
    combo_key: &[u8],
    password: &[u8],
    salt: &[u8; 16],
) -> Result<String, OracleThinError> {
    let mut plain = Vec::with_capacity(salt.len() + password.len());
    plain.extend_from_slice(salt);
    plain.extend_from_slice(password);
    let encrypted = aes_encrypt_cbc_pkcs7(combo_key, &plain)?;
    Ok(hex_encode_upper(&encrypted))
}

fn generate_11g_password_hash(password: &[u8], verifier_data: &[u8]) -> Vec<u8> {
    let mut hasher = Sha1::new();
    hasher.update(password);
    hasher.update(verifier_data);
    let mut password_hash = hasher.finalize().to_vec();
    password_hash.extend_from_slice(&[0, 0, 0, 0]);
    password_hash
}

fn generate_10g_password_hash(username: &str, password: &str) -> Vec<u8> {
    let mut buffer = Vec::with_capacity((username.len() + password.len()) * 2 + 8);
    append_10g_password_part(&mut buffer, username);
    append_10g_password_part(&mut buffer, password);
    while buffer.len() % 8 != 0 {
        buffer.push(0);
    }

    let first_key = des_cbc_checksum(&buffer, TNS_LEGACY_DES_KEY);
    let second_key = des_cbc_checksum(&buffer, first_key);
    let mut password_hash = Vec::with_capacity(16);
    password_hash.extend_from_slice(&second_key);
    password_hash.extend_from_slice(&[0; 8]);
    password_hash
}

fn append_10g_password_part(buffer: &mut Vec<u8>, value: &str) {
    for byte in value.bytes() {
        buffer.push(0);
        buffer.push(byte.to_ascii_uppercase());
    }
}

fn des_cbc_checksum(input: &[u8], key: [u8; 8]) -> [u8; 8] {
    let mut block = [0u8; 8];
    for chunk in input.chunks_exact(8) {
        for i in 0..8 {
            block[i] ^= chunk[i];
        }
        block = des_encrypt_block(block, key);
    }
    block
}

fn des_encrypt_block(block: [u8; 8], key: [u8; 8]) -> [u8; 8] {
    let subkeys = des_subkeys(key);
    let permuted = des_permute(u64::from_be_bytes(block), 64, &DES_INITIAL_PERMUTATION);
    let mut left = (permuted >> 32) as u32;
    let mut right = permuted as u32;
    for subkey in subkeys {
        let next_left = right;
        right = left ^ des_round(right, subkey);
        left = next_left;
    }
    let preoutput = (u64::from(right) << 32) | u64::from(left);
    des_permute(preoutput, 64, &DES_FINAL_PERMUTATION).to_be_bytes()
}

fn des_subkeys(key: [u8; 8]) -> [u64; 16] {
    let mut subkeys = [0u64; 16];
    let permuted = des_permute(u64::from_be_bytes(key), 64, &DES_PC1);
    let mut left = ((permuted >> 28) & 0x0fff_ffff) as u32;
    let mut right = (permuted & 0x0fff_ffff) as u32;
    for (index, shift) in DES_KEY_SHIFTS.iter().enumerate() {
        left = des_rotate_28(left, *shift);
        right = des_rotate_28(right, *shift);
        let joined = (u64::from(left) << 28) | u64::from(right);
        subkeys[index] = des_permute(joined, 56, &DES_PC2);
    }
    subkeys
}

fn des_rotate_28(value: u32, shift: u8) -> u32 {
    ((value << shift) | (value >> (28 - shift))) & 0x0fff_ffff
}

fn des_round(right: u32, subkey: u64) -> u32 {
    let expanded = des_permute(u64::from(right), 32, &DES_EXPANSION) ^ subkey;
    let mut substituted = 0u32;
    for box_index in 0..8 {
        let shift = 42 - (box_index * 6);
        let chunk = ((expanded >> shift) & 0x3f) as u8;
        let row = ((chunk & 0x20) >> 4) | (chunk & 0x01);
        let column = (chunk >> 1) & 0x0f;
        let value = DES_S_BOXES[box_index][usize::from(row * 16 + column)];
        substituted = (substituted << 4) | u32::from(value);
    }
    des_permute(u64::from(substituted), 32, &DES_P_PERMUTATION) as u32
}

fn des_permute(input: u64, input_bits: u8, table: &[u8]) -> u64 {
    let mut output = 0u64;
    for position in table {
        output <<= 1;
        output |= (input >> (input_bits - position)) & 1;
    }
    output
}

fn derive_auth_combo_key(
    state: &AuthState,
    session_key_part_a: &[u8],
    session_key_part_b: &[u8],
    key_len: usize,
) -> Result<Vec<u8>, OracleThinError> {
    if state.auth_uses_pbkdf2_key_derivation {
        return derive_auth_combo_key_pbkdf2(
            state,
            session_key_part_a,
            session_key_part_b,
            key_len,
        );
    }
    derive_auth_combo_key_legacy_md5(session_key_part_a, session_key_part_b, key_len)
}

fn derive_auth_combo_key_legacy_md5(
    session_key_part_a: &[u8],
    session_key_part_b: &[u8],
    key_len: usize,
) -> Result<Vec<u8>, OracleThinError> {
    let start = 16;
    let xor_len = if key_len > 16 { 24 } else { 16 };
    if session_key_part_a.len() < start + xor_len || session_key_part_b.len() < start + xor_len {
        return Err(OracleThinError::new(
            "Oracle authentication session key is shorter than expected",
        ));
    }

    let mut xor_bytes = Vec::with_capacity(xor_len);
    for i in start..start + xor_len {
        xor_bytes.push(session_key_part_a[i] ^ session_key_part_b[i]);
    }
    let part1 = Md5::digest(&xor_bytes[..16]);
    let mut combo_key = Vec::with_capacity(32);
    combo_key.extend_from_slice(&part1);
    if key_len > 16 {
        let part2 = Md5::digest(&xor_bytes[16..]);
        combo_key.extend_from_slice(&part2);
    }
    combo_key.truncate(key_len);
    Ok(combo_key)
}

fn derive_auth_combo_key_pbkdf2(
    state: &AuthState,
    session_key_part_a: &[u8],
    session_key_part_b: &[u8],
    key_len: usize,
) -> Result<Vec<u8>, OracleThinError> {
    let temp_key = match state.verifier_type {
        TNS_VERIFIER_TYPE_10G => {
            let half_len = session_key_part_a.len() / 2;
            if half_len == 0 || session_key_part_b.len() < half_len {
                return Err(OracleThinError::new(
                    "Oracle authentication session key is shorter than expected",
                ));
            }
            let mut temp_key = Vec::with_capacity(half_len * 2);
            temp_key.extend_from_slice(&session_key_part_b[..half_len]);
            temp_key.extend_from_slice(&session_key_part_a[..half_len]);
            temp_key
        }
        TNS_VERIFIER_TYPE_11G_1 | TNS_VERIFIER_TYPE_11G_2 | TNS_VERIFIER_TYPE_12C => {
            if session_key_part_a.len() < key_len || session_key_part_b.len() < key_len {
                return Err(OracleThinError::new(
                    "Oracle authentication session key is shorter than expected",
                ));
            }
            let mut temp_key = Vec::with_capacity(key_len * 2);
            temp_key.extend_from_slice(&session_key_part_b[..key_len]);
            temp_key.extend_from_slice(&session_key_part_a[..key_len]);
            temp_key
        }
        other => {
            return Err(OracleThinError::new(format!(
                "unsupported Oracle password verifier type 0x{other:x}"
            )))
        }
    };
    let csk_salt = hex_decode(required_session_value(state, "AUTH_PBKDF2_CSK_SALT")?)?;
    let sder_count = required_session_value(state, "AUTH_PBKDF2_SDER_COUNT")?
        .parse::<u32>()
        .map_err(|err| OracleThinError::new(format!("invalid AUTH_PBKDF2_SDER_COUNT: {err}")))?;
    let temp_key_hex = hex_encode_upper(&temp_key);
    let mut combo_key = vec![0u8; key_len];
    pbkdf2_hmac::<Sha512>(
        temp_key_hex.as_bytes(),
        &csk_salt,
        sder_count,
        &mut combo_key,
    );
    Ok(combo_key)
}

fn client_session_key_hex(
    encoded_client_key: &[u8],
    session_key_part_len: usize,
) -> Result<String, OracleThinError> {
    let required_len = if session_key_part_len == 48 { 48 } else { 32 };
    if encoded_client_key.len() < required_len {
        return Err(OracleThinError::new(
            "Oracle encrypted client session key is shorter than expected",
        ));
    }
    Ok(hex_encode_upper(&encoded_client_key[..required_len]))
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
    process_auth_payload(&packet, capabilities, state)
}

fn process_auth_payload(
    packet: &[u8],
    capabilities: &OracleThinCapabilities,
    state: &mut AuthState,
) -> Result<(), OracleThinError> {
    let mut cursor = PacketCursor::with_capabilities(packet, capabilities);
    while cursor.remaining() > 0 {
        let message_type = cursor.read_u8()?;
        match message_type {
            TNS_MSG_TYPE_PARAMETER => process_auth_parameters(&mut cursor, state)?,
            TNS_MSG_TYPE_STATUS => {
                process_status(&mut cursor, capabilities, &mut state.server_state)?;
            }
            TNS_MSG_TYPE_TOKEN => {
                process_token(&mut cursor, TNS_DEFAULT_TOKEN_NUM)?;
            }
            TNS_MSG_TYPE_ERROR => {
                let error = process_execute_error(&mut cursor, capabilities)?;
                update_transaction_status_from_call_status(
                    &mut state.server_state,
                    capabilities,
                    error.call_status,
                );
                if let Some(warning) = error.warning.clone() {
                    state.server_state.last_warning = Some(warning);
                }
                if error.code != 0 {
                    return Err(OracleThinError::new(
                        error
                            .message
                            .unwrap_or_else(|| format!("Oracle error ORA-{:05}", error.code)),
                    ));
                }
                break;
            }
            TNS_MSG_TYPE_WARNING => {
                if let Some(warning) = process_warning(&mut cursor, capabilities)? {
                    state.server_state.last_warning = Some(warning);
                }
            }
            TNS_MSG_TYPE_SERVER_SIDE_PIGGYBACK => {
                process_server_side_piggyback(&mut cursor, capabilities, &mut state.server_state)?
            }
            TNS_MSG_TYPE_END_OF_RESPONSE => break,
            other => {
                return Err(OracleThinError::new(format!(
                    "unexpected Oracle auth response message type {other}"
                )));
            }
        }
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
    Ok(())
}

fn verify_server_response(state: &AuthState) -> Result<(), OracleThinError> {
    let Some(combo_key) = state.combo_key.as_deref() else {
        return Ok(());
    };
    let Some(encoded_response) = state.session_data.get("AUTH_SVR_RESPONSE") else {
        return Err(OracleThinError::new(
            "Oracle authentication did not return a server response",
        ));
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
        16 => cbc::Encryptor::<Aes128>::new_from_slices(key, &iv)
            .map_err(|err| OracleThinError::new(format!("invalid AES-128 key: {err}")))?
            .encrypt_padded_mut::<Pkcs7>(&mut buf, pos)
            .map_err(|err| OracleThinError::new(format!("AES-CBC encrypt failed: {err}")))?,
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

fn aes_encrypt_cbc_zero_padding(key: &[u8], plain_text: &[u8]) -> Result<Vec<u8>, OracleThinError> {
    let iv = [0u8; 16];
    let pos = plain_text.len();
    let padded_len = pos + (16 - (pos % 16));
    let mut buf = plain_text.to_vec();
    buf.resize(padded_len, 0);
    let encrypted = match key.len() {
        16 => cbc::Encryptor::<Aes128>::new_from_slices(key, &iv)
            .map_err(|err| OracleThinError::new(format!("invalid AES-128 key: {err}")))?
            .encrypt_padded_mut::<NoPadding>(&mut buf, padded_len)
            .map_err(|err| OracleThinError::new(format!("AES-CBC encrypt failed: {err}")))?,
        24 => cbc::Encryptor::<Aes192>::new_from_slices(key, &iv)
            .map_err(|err| OracleThinError::new(format!("invalid AES-192 key: {err}")))?
            .encrypt_padded_mut::<NoPadding>(&mut buf, padded_len)
            .map_err(|err| OracleThinError::new(format!("AES-CBC encrypt failed: {err}")))?,
        32 => cbc::Encryptor::<Aes256>::new_from_slices(key, &iv)
            .map_err(|err| OracleThinError::new(format!("invalid AES-256 key: {err}")))?
            .encrypt_padded_mut::<NoPadding>(&mut buf, padded_len)
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
        16 => cbc::Decryptor::<Aes128>::new_from_slices(key, &iv)
            .map_err(|err| OracleThinError::new(format!("invalid AES-128 key: {err}")))?
            .decrypt_padded_mut::<NoPadding>(&mut buf)
            .map_err(|err| OracleThinError::new(format!("AES-CBC decrypt failed: {err}")))?,
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

fn auth_connect_string(config: &OracleThinConfig) -> Result<String, OracleThinError> {
    let target = &config.target;
    validate_connect_descriptor_value("host", &target.host)?;
    validate_connect_descriptor_value("program", &config.program)?;
    validate_connect_descriptor_value("machine", &config.machine)?;
    validate_connect_descriptor_value("os_user", &config.os_user)?;
    let description_options = connect_description_option_parts(&config.connect_options);
    let connect_data =
        connect_data_descriptor_parts(target, config.connect_options.desired_protocol_version)?;
    Ok(format!(
        "(DESCRIPTION={}(ADDRESS=(PROTOCOL=tcp)(HOST={})(PORT={}))(CONNECT_DATA={}(CID=(PROGRAM={})(HOST={})(USER={}))))",
        description_options,
        target.host,
        target.port,
        connect_data,
        config.program,
        config.machine,
        config.os_user
    ))
}

fn alter_session_timezone_statement() -> String {
    let timezone = std::env::var("ORA_SDTZ").unwrap_or_else(|_| local_timezone_offset_string());
    format!("ALTER SESSION SET TIME_ZONE='{timezone}'\0")
}

fn local_timezone_offset_string() -> String {
    let seconds = chrono::Local::now().offset().local_minus_utc();
    let sign = if seconds < 0 { '-' } else { '+' };
    let absolute = seconds.abs();
    let hours = absolute / 3600;
    let minutes = (absolute % 3600) / 60;
    format!("{sign}{hours:02}:{minutes:02}")
}

fn write_data_packet(
    stream: &mut TcpStream,
    protocol_version: u16,
    chunk_size: usize,
    payload: &[u8],
) -> Result<(), OracleThinError> {
    if payload.len() > chunk_size {
        for chunk in payload.chunks(chunk_size.max(1)) {
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

fn write_eof_data_packet(
    stream: &mut TcpStream,
    protocol_version: u16,
) -> Result<(), OracleThinError> {
    let size = 10usize;
    let mut packet = vec![0u8; size];
    if protocol_version >= 315 {
        put_u32(&mut packet, 0, size as u32);
    } else {
        put_u16(&mut packet, 0, size as u16);
    }
    packet[4] = TNS_PACKET_TYPE_DATA;
    packet[8..10].copy_from_slice(&TNS_DATA_FLAGS_EOF.to_be_bytes());
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

fn read_data_packet_with_control(
    stream: &mut TcpStream,
    protocol_version: u16,
) -> Result<(bool, Vec<u8>), OracleThinError> {
    read_data_packet_with_flags_and_control(stream, protocol_version)
        .map(|(oob_reset_received, _, payload)| (oob_reset_received, payload))
}

fn read_data_packet_with_flags(
    stream: &mut TcpStream,
    protocol_version: u16,
) -> Result<(u16, Vec<u8>), OracleThinError> {
    read_data_packet_with_flags_and_control(stream, protocol_version)
        .map(|(_, data_flags, payload)| (data_flags, payload))
}

fn read_data_packet_with_flags_and_control(
    stream: &mut TcpStream,
    protocol_version: u16,
) -> Result<(bool, u16, Vec<u8>), OracleThinError> {
    let mut oob_reset_received = false;
    loop {
        let mut header = [0u8; 8];
        stream
            .read_exact(&mut header)
            .map_err(|err| read_packet_error("TNS data header", err))?;
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
        stream
            .read_exact(&mut data)
            .map_err(|err| read_packet_body_error(err, header[4], size, &header))?;
        match header[4] {
            TNS_PACKET_TYPE_DATA => {
                if data.len() < 2 {
                    return Err(OracleThinError::new(format!(
                        "invalid TNS data packet length {size}"
                    )));
                }
                let data_flags = u16::from_be_bytes([data[0], data[1]]);
                return Ok((oob_reset_received, data_flags, data[2..].to_vec()));
            }
            TNS_PACKET_TYPE_MARKER => {
                if data.last() == Some(&TNS_MARKER_TYPE_BREAK) {
                    write_marker_packet(stream, protocol_version, TNS_MARKER_TYPE_RESET)?;
                    continue;
                }
                if data.last() == Some(&TNS_MARKER_TYPE_RESET) {
                    return Ok((
                        oob_reset_received,
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
            TNS_PACKET_TYPE_CONTROL => {
                oob_reset_received |= process_control_packet(&data)?.oob_reset_received;
                continue;
            }
            other => {
                return Err(OracleThinError::new(format!(
                    "expected TNS data packet, got packet type {other}"
                )));
            }
        }
    }
}

#[derive(Debug, Default)]
struct ControlPacketStatus {
    oob_reset_received: bool,
}

fn process_control_packet(data: &[u8]) -> Result<ControlPacketStatus, OracleThinError> {
    if data.len() < 2 {
        return Err(OracleThinError::new(format!(
            "invalid TNS control packet length {}",
            data.len() + 8
        )));
    }
    let control_type = u16::from_be_bytes([data[0], data[1]]);
    match control_type {
        TNS_CONTROL_TYPE_RESET_OOB => Ok(ControlPacketStatus {
            oob_reset_received: true,
        }),
        TNS_CONTROL_TYPE_INBAND_NOTIFICATION => {
            if data.len() < 10 {
                return Err(OracleThinError::new(format!(
                    "invalid TNS in-band notification packet length {}",
                    data.len() + 8
                )));
            }
            let pending_error_num = u32::from_be_bytes([data[6], data[7], data[8], data[9]]);
            match pending_error_num {
                0 | TNS_ERR_SESSION_SHUTDOWN | TNS_ERR_INBAND_MESSAGE => {
                    Ok(ControlPacketStatus::default())
                }
                TNS_ERR_EXCEEDED_IDLE_TIME => Err(OracleThinError::new(
                    "Oracle session exceeded the configured idle time",
                )),
                other => Err(OracleThinError::new(format!(
                    "unsupported TNS in-band notification error {other}"
                ))),
            }
        }
        other => Err(OracleThinError::new(format!(
            "received unsupported TNS control packet type {other}: {}",
            hex_encode_upper(data)
        ))),
    }
}

fn read_packet_error(context: &str, error: std::io::Error) -> OracleThinError {
    if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) {
        OracleThinError::new(format!("Read timeout while reading {context}: {error}"))
    } else {
        OracleThinError::new(format!("failed to read {context}: {error}"))
    }
}

fn read_packet_body_error(
    error: std::io::Error,
    packet_type: u8,
    size: usize,
    header: &[u8; 8],
) -> OracleThinError {
    let detail = format!("packet_type={packet_type} size={size} header={header:02x?}");
    if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) {
        OracleThinError::new(format!(
            "Read timeout while reading TNS packet body: {error}; {detail}"
        ))
    } else {
        OracleThinError::new(format!("failed to read TNS packet body: {error}; {detail}"))
    }
}

/// One packet read while draining a cancel reset, or `Quiet` when the read
/// timed out at a packet boundary (no bytes pending — the socket is idle).
enum CancelResetPacket {
    Packet(u8, Vec<u8>),
    Quiet,
}

/// Reads a single whole TNS packet during a cancel reset drain. A read timeout
/// on the packet header means the socket is quiet at a request boundary, which
/// is the normal way the drain terminates.
fn read_cancel_reset_packet(
    stream: &mut TcpStream,
    protocol_version: u16,
) -> Result<CancelResetPacket, OracleThinError> {
    let mut header = [0u8; 8];
    if let Err(err) = stream.read_exact(&mut header) {
        if matches!(err.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) {
            return Ok(CancelResetPacket::Quiet);
        }
        return Err(read_packet_error("TNS cancel reset header", err));
    }
    let size = if protocol_version >= 315 {
        u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize
    } else {
        u16::from_be_bytes([header[0], header[1]]) as usize
    };
    if size < 8 {
        return Err(OracleThinError::new(format!(
            "invalid TNS packet length {size} during cancel reset"
        )));
    }
    let mut data = vec![0u8; size - 8];
    stream
        .read_exact(&mut data)
        .map_err(|err| read_packet_body_error(err, header[4], size, &header))?;
    Ok(CancelResetPacket::Packet(header[4], data))
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
    let element_bytes = num_elements.checked_mul(5).ok_or_else(|| {
        OracleThinError::new(format!(
            "Oracle protocol FDO element count is too large: {num_elements}"
        ))
    })?;
    cursor.skip(element_bytes)?;
    let fdo_length = cursor.read_u16_be()? as usize;
    let fdo = cursor.read_raw(fdo_length)?;
    if fdo.len() < 7 {
        return Err(OracleThinError::new(format!(
            "short Oracle protocol FDO: {} bytes",
            fdo.len()
        )));
    }
    let ix = 6usize
        .checked_add(usize::from(fdo[5]))
        .and_then(|value| value.checked_add(usize::from(fdo[6])))
        .ok_or_else(|| OracleThinError::new("Oracle protocol FDO offset overflow"))?;
    if fdo.len() < ix + 5 {
        return Err(OracleThinError::new(format!(
            "short Oracle protocol FDO for ncharset: {} bytes, need {}",
            fdo.len(),
            ix + 5
        )));
    }
    capabilities.ncharset_id = u16::from_be_bytes([fdo[ix + 3], fdo[ix + 4]]);
    let server_compile_caps = cursor
        .read_bytes()?
        .ok_or_else(|| OracleThinError::new("missing Oracle server compile capabilities"))?;
    if server_compile_caps.len() <= TNS_CCAP_FIELD_VERSION {
        return Err(OracleThinError::new(format!(
            "short Oracle server compile capabilities: {} bytes",
            server_compile_caps.len()
        )));
    }
    adjust_for_server_compile_caps(capabilities, &server_compile_caps);
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
        capabilities.server_ttc_field_version = server_field_version;
        if server_field_version < capabilities.ttc_field_version {
            capabilities.ttc_field_version = server_field_version;
        }
    }
    capabilities.supports_sql_boolean = capabilities.ttc_field_version >= 17;
    capabilities.supports_oson_long_field_names =
        capabilities.ttc_field_version >= TNS_CCAP_FIELD_VERSION_23_1;
    capabilities.supports_end_of_call_status = server_caps
        .get(TNS_CCAP_TTC1)
        .is_some_and(|value| value & TNS_CCAP_END_OF_CALL_STATUS != 0);
    capabilities.supports_fast_session_attributes = server_caps
        .get(TNS_CCAP_OCI1)
        .is_some_and(|value| value & TNS_CCAP_LEGACY_FAST_SESSION_ATTRIBUTES != 0);
    capabilities.auth_uses_pbkdf2_key_derivation =
        server_caps.get(4).is_some_and(|value| value & 0x20 != 0);
    if server_caps
        .get(TNS_CCAP_TTC4)
        .is_some_and(|value| value & TNS_CCAP_EXPLICIT_BOUNDARY != 0)
    {
        capabilities.supports_request_boundaries = true;
    }
    if server_caps
        .get(TNS_CCAP_TTC3)
        .is_some_and(|value| value & TNS_CCAP_BIG_CHUNK_CLR != 0)
    {
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
    if !server_caps
        .get(TNS_RCAP_TTC)
        .is_some_and(|value| value & TNS_RCAP_TTC_SESSION_STATE_OPS != 0)
    {
        capabilities.supports_request_boundaries = false;
    }
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
    caps[6] = TNS_RCAP_TTC_ZERO_COPY | TNS_RCAP_TTC_32K;
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
    write_bytes_with_length_with_big_chunks(out, value, true)
}

fn write_bytes_with_length_for_capabilities(
    out: &mut Vec<u8>,
    value: &[u8],
    capabilities: &OracleThinCapabilities,
) -> Result<(), OracleThinError> {
    write_bytes_with_length_with_big_chunks(out, value, capabilities.supports_big_clr_chunks)
}

fn write_bytes_with_length_with_big_chunks(
    out: &mut Vec<u8>,
    value: &[u8],
    big_clr_chunks: bool,
) -> Result<(), OracleThinError> {
    if value.len() <= 252 {
        out.push(value.len() as u8);
        out.extend_from_slice(value);
        return Ok(());
    }
    out.push(0xfe);
    if big_clr_chunks {
        for chunk in value.chunks(TNS_BIG_CLR_CHUNK_SIZE) {
            write_ub4(out, chunk.len() as u32);
            out.extend_from_slice(chunk);
        }
        write_ub4(out, 0);
    } else {
        for chunk in value.chunks(TNS_LEGACY_CLR_CHUNK_SIZE) {
            out.push(chunk.len() as u8);
            out.extend_from_slice(chunk);
        }
        out.push(0);
    }
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
    (562, 562, 1),
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
    (13, 0, 0),
    (14, 0, 0),
    (15, 1, 1),
    (16, 0, 0),
    (17, 0, 0),
    (18, 0, 0),
    (19, 0, 0),
    (20, 0, 0),
    (21, 0, 0),
    (22, 0, 0),
    (39, 39, 1),
    (58, 0, 0),
    (68, 2, 10),
    (69, 0, 0),
    (70, 0, 0),
    (74, 0, 0),
    (76, 0, 0),
    (91, 2, 10),
    (94, 1, 1),
    (95, 23, 1),
    (96, 96, 1),
    (97, 96, 1),
    (100, 100, 1),
    (101, 101, 1),
    (102, 102, 1),
    (104, 11, 1),
    (105, 0, 0),
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
    (118, 0, 0),
    (119, 119, 1),
    (121, 0, 0),
    (122, 0, 0),
    (123, 0, 0),
    (136, 0, 0),
    (147, 0, 0),
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
    (191, 0, 0),
    (192, 0, 0),
    (195, 112, 1),
    (196, 113, 1),
    (197, 114, 1),
    (208, 208, 1),
    (209, 0, 0),
    (231, 231, 1),
    (232, 231, 1),
    (233, 233, 1),
    (241, 109, 1),
    (252, 252, 1),
    (515, 0, 0),
    (590, 590, 1),
    (591, 591, 1),
    (592, 592, 1),
    (613, 613, 1),
    (614, 614, 1),
    (615, 615, 1),
    (616, 616, 1),
    (611, 611, 1),
    (612, 612, 1),
    (617, 617, 1),
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
    (899, 899, 1),
    (900, 900, 1),
    (901, 901, 1),
    (652, 652, 1),
    (662, 662, 1),
    (646, 646, 1),
    (647, 647, 1),
    (127, 127, 1),
    (660, 660, 1),
    (661, 661, 1),
    (665, 665, 1),
    (669, 669, 1),
    (670, 670, 1),
];

const PYTHON_ORACLEDB_MODERN_DATA_TYPE_REPRESENTATIONS: &[(u16, u16, u16)] = &[
    (34, 34, 1),
    (35, 35, 1),
    (36, 36, 1),
    (37, 37, 1),
    (38, 38, 1),
    (42, 42, 1),
    (43, 43, 1),
    (44, 44, 1),
    (45, 45, 1),
    (46, 46, 1),
    (47, 47, 1),
    (48, 48, 1),
    (49, 49, 1),
    (50, 50, 1),
    (51, 51, 1),
    (52, 52, 1),
    (53, 53, 1),
    (54, 54, 1),
    (55, 55, 1),
    (56, 56, 1),
    (57, 57, 1),
    (59, 59, 1),
    (60, 60, 1),
    (61, 61, 1),
    (62, 62, 1),
    (63, 63, 1),
    (64, 64, 1),
    (65, 65, 1),
    (66, 66, 1),
    (67, 67, 1),
    (71, 71, 1),
    (72, 72, 1),
    (73, 73, 1),
    (75, 75, 1),
    (77, 77, 1),
    (78, 78, 1),
    (79, 79, 1),
    (80, 80, 1),
    (81, 81, 1),
    (82, 82, 1),
    (83, 83, 1),
    (84, 84, 1),
    (85, 85, 1),
    (86, 86, 1),
    (87, 87, 1),
    (88, 88, 1),
    (89, 89, 1),
    (90, 90, 1),
    (92, 92, 1),
    (93, 93, 1),
    (98, 98, 1),
    (99, 99, 1),
    (103, 103, 1),
    (107, 107, 1),
    (124, 124, 1),
    (125, 125, 1),
    (126, 126, 1),
    (128, 128, 1),
    (129, 129, 1),
    (130, 130, 1),
    (131, 131, 1),
    (132, 132, 1),
    (133, 133, 1),
    (134, 134, 1),
    (135, 135, 1),
    (137, 137, 1),
    (138, 138, 1),
    (139, 139, 1),
    (140, 140, 1),
    (141, 141, 1),
    (142, 142, 1),
    (143, 143, 1),
    (144, 144, 1),
    (145, 145, 1),
    (148, 148, 1),
    (149, 149, 1),
    (150, 150, 1),
    (151, 151, 1),
    (157, 157, 1),
    (158, 158, 1),
    (159, 159, 1),
    (160, 160, 1),
    (161, 161, 1),
    (162, 162, 1),
    (163, 163, 1),
    (164, 164, 1),
    (165, 165, 1),
    (166, 166, 1),
    (167, 167, 1),
    (168, 168, 1),
    (169, 169, 1),
    (170, 170, 1),
    (171, 171, 1),
    (173, 173, 1),
    (174, 174, 1),
    (175, 175, 1),
    (176, 176, 1),
    (177, 177, 1),
    (193, 193, 1),
    (194, 194, 1),
    (199, 199, 1),
    (200, 200, 1),
    (201, 201, 1),
    (202, 202, 1),
    (203, 203, 1),
    (204, 204, 1),
    (205, 205, 1),
    (206, 206, 1),
    (207, 207, 1),
    (210, 210, 1),
    (211, 211, 1),
    (212, 212, 1),
    (213, 213, 1),
    (214, 214, 1),
    (215, 215, 1),
    (216, 216, 1),
    (217, 217, 1),
    (218, 218, 1),
    (219, 219, 1),
    (220, 220, 1),
    (221, 221, 1),
    (222, 222, 1),
    (223, 223, 1),
    (224, 224, 1),
    (225, 225, 1),
    (226, 226, 1),
    (227, 227, 1),
    (228, 228, 1),
    (229, 229, 1),
    (230, 230, 1),
    (234, 234, 1),
    (235, 235, 1),
    (236, 236, 1),
    (237, 237, 1),
    (238, 238, 1),
    (239, 239, 1),
    (240, 240, 1),
    (242, 242, 1),
    (243, 243, 1),
    (244, 244, 1),
    (245, 245, 1),
    (246, 246, 1),
    (253, 253, 1),
    (254, 254, 1),
];

struct PacketCursor<'a> {
    data: &'a [u8],
    pos: usize,
    big_clr_chunks: bool,
    legacy_null_clr: bool,
}

impl<'a> PacketCursor<'a> {
    fn with_capabilities(data: &'a [u8], capabilities: &OracleThinCapabilities) -> Self {
        Self {
            data,
            pos: 0,
            big_clr_chunks: capabilities.supports_big_clr_chunks,
            legacy_null_clr: capabilities
                .protocol_version
                .is_some_and(|version| version < TNS_VERSION_MIN_ACCEPTED),
        }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn peek_u8(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
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

    fn read_u32_be(&mut self) -> Result<u32, OracleThinError> {
        let bytes = self.read_raw(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
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
            TNS_LEGACY_NULL_LENGTH_INDICATOR if self.legacy_null_clr => Ok(None),
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

    fn read_sb8(&mut self) -> Result<i64, OracleThinError> {
        let len_byte = self.read_u8()?;
        let is_negative = len_byte & 0x80 != 0;
        let len = usize::from(len_byte & 0x7f);
        if len == 0 {
            return Ok(0);
        }
        if len > 8 {
            return Err(OracleThinError::new(format!(
                "invalid TTC signed ub8 length {len}"
            )));
        }
        let bytes = self.read_raw(len)?;
        let mut value = 0i64;
        for byte in bytes {
            value = (value << 8) | i64::from(*byte);
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

    fn read_bytes_with_ub4_length(&mut self) -> Result<Option<Vec<u8>>, OracleThinError> {
        let expected_len = self.read_ub4()? as usize;
        if expected_len == 0 {
            return Ok(None);
        }
        let Some(mut bytes) = self.read_bytes()? else {
            return Ok(None);
        };
        if bytes.len() < expected_len {
            return Err(OracleThinError::new("short TTC bytes-with-length field"));
        }
        if bytes.len() > expected_len {
            bytes.truncate(expected_len);
        }
        Ok(Some(bytes))
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
        _ => TNS_CCAP_FIELD_VERSION_MAX,
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
    use std::collections::{HashMap, HashSet};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{atomic::AtomicBool, Arc};
    use std::time::{Duration, Instant};

    use super::CANCEL_RESET_DRAIN_TIMEOUT;
    use super::{
        adjust_columns_after_define, bind_column_metadata, column_metadata_from_thin,
        column_types_may_contain_ref_cursors, column_types_require_define_fetch_for_values,
        columns_may_contain_ref_cursors, columns_require_define_fetch_for_values,
        oracle_thin_driver_name, put_u16_be_vec, write_auth_header,
        write_close_temp_lobs_piggyback, write_lob_operation_request, ExecuteReadState,
        DATA_TYPE_REPRESENTATIONS, ORA_TYPE_NUM_BFILE, ORA_TYPE_NUM_BINARY_DOUBLE,
        ORA_TYPE_NUM_BINARY_FLOAT, ORA_TYPE_NUM_BLOB, ORA_TYPE_NUM_BOOLEAN, ORA_TYPE_NUM_CHAR,
        ORA_TYPE_NUM_CLOB, ORA_TYPE_NUM_CURSOR, ORA_TYPE_NUM_DATE, ORA_TYPE_NUM_DBFILE,
        ORA_TYPE_NUM_DJSON, ORA_TYPE_NUM_INTERVAL_DS, ORA_TYPE_NUM_INTERVAL_YM, ORA_TYPE_NUM_JSON,
        ORA_TYPE_NUM_LONG, ORA_TYPE_NUM_LONG_RAW, ORA_TYPE_NUM_NUMBER, ORA_TYPE_NUM_OBJECT,
        ORA_TYPE_NUM_RAW, ORA_TYPE_NUM_ROWID, ORA_TYPE_NUM_TIMESTAMP_LTZ,
        ORA_TYPE_NUM_TIMESTAMP_TZ, ORA_TYPE_NUM_TIMESTAMP_TZ_EXT, ORA_TYPE_NUM_UROWID,
        ORA_TYPE_NUM_VARCHAR, ORA_TYPE_NUM_VECTOR,
        PYTHON_ORACLEDB_MODERN_DATA_TYPE_REPRESENTATIONS, TNS_AUTH_MODE_CHANGE_PASSWORD,
        TNS_AUTH_MODE_LOGON, TNS_AUTH_MODE_SYSASM, TNS_AUTH_MODE_SYSBKP, TNS_AUTH_MODE_SYSDBA,
        TNS_AUTH_MODE_SYSDGD, TNS_AUTH_MODE_SYSKMT, TNS_AUTH_MODE_SYSOPER, TNS_AUTH_MODE_SYSRAC,
        TNS_AUTH_MODE_WITH_PASSWORD, TNS_BIND_USE_INDICATORS, TNS_CHARSET_UTF8,
        TNS_CONTROL_TYPE_INBAND_NOTIFICATION, TNS_CONTROL_TYPE_RESET_OOB,
        TNS_DATA_FLAGS_END_OF_RESPONSE, TNS_DATA_FLAGS_EOF, TNS_DATA_TYPE_BDOUBLE,
        TNS_DATA_TYPE_BFLOAT, TNS_DATA_TYPE_BINARY_INTEGER, TNS_DATA_TYPE_CFILE, TNS_DATA_TYPE_CLV,
        TNS_DATA_TYPE_DBLOB, TNS_DATA_TYPE_DCLOB, TNS_DATA_TYPE_DOL, TNS_DATA_TYPE_DOP,
        TNS_DATA_TYPE_DTR, TNS_DATA_TYPE_DUN, TNS_DATA_TYPE_EDATE, TNS_DATA_TYPE_ESITZ,
        TNS_DATA_TYPE_EXT_NAMED, TNS_DATA_TYPE_EXT_REF, TNS_DATA_TYPE_FLOAT, TNS_DATA_TYPE_INT_REF,
        TNS_DATA_TYPE_LVB, TNS_DATA_TYPE_LVC, TNS_DATA_TYPE_OAC, TNS_DATA_TYPE_OAC9,
        TNS_DATA_TYPE_ODT, TNS_DATA_TYPE_PDN, TNS_DATA_TYPE_PNTY, TNS_DATA_TYPE_RDD,
        TNS_DATA_TYPE_RSET, TNS_DATA_TYPE_SLS, TNS_DATA_TYPE_STR, TNS_DATA_TYPE_TIME,
        TNS_DATA_TYPE_TIME_TZ, TNS_DATA_TYPE_UB8, TNS_DATA_TYPE_UIN, TNS_DATA_TYPE_VBI,
        TNS_DATA_TYPE_VCS, TNS_DATA_TYPE_VNU, TNS_DATA_TYPE_VST, TNS_DEFAULT_SDU,
        TNS_DURATION_SESSION, TNS_END_TO_END_ACTION, TNS_END_TO_END_CLIENT_IDENTIFIER,
        TNS_END_TO_END_CLIENT_INFO, TNS_END_TO_END_DBOP, TNS_END_TO_END_MODULE,
        TNS_EOCS_FLAGS_TXN_IN_PROGRESS, TNS_ERR_INBAND_MESSAGE, TNS_EXEC_FLAGS_IMPLICIT_RESULTSET,
        TNS_FUNC_AUTH_PHASE_ONE, TNS_FUNC_AUTH_PHASE_TWO, TNS_FUNC_LOB_OP, TNS_FUNC_PING,
        TNS_FUNC_SESSION_STATE, TNS_FUNC_SET_END_TO_END_ATTR, TNS_FUNC_SET_SCHEMA,
        TNS_JSON_TYPE_ID, TNS_LOB_LOC_FLAGS_LITTLE_ENDIAN, TNS_LOB_LOC_FLAGS_VAR_LENGTH_CHARSET,
        TNS_LOB_LOC_OFFSET_FLAG_3, TNS_LOB_LOC_OFFSET_FLAG_4, TNS_LOB_OP_ARRAY,
        TNS_LOB_OP_CREATE_TEMP, TNS_LOB_OP_FREE_TEMP, TNS_LOB_PREFETCH_FLAG, TNS_MARKER_TYPE_BREAK,
        TNS_MARKER_TYPE_INTERRUPT, TNS_MARKER_TYPE_RESET, TNS_MAX_LONG_LENGTH,
        TNS_MSG_TYPE_END_OF_RESPONSE, TNS_MSG_TYPE_FUNCTION, TNS_MSG_TYPE_PIGGYBACK,
        TNS_MSG_TYPE_STATUS, TNS_PACKET_TYPE_CONTROL, TNS_PACKET_TYPE_DATA, TNS_PACKET_TYPE_MARKER,
        TNS_SESSION_STATE_EXPLICIT_BOUNDARY, TNS_SESSION_STATE_REQUEST_BEGIN,
        TNS_SESSION_STATE_REQUEST_END, TNS_VECTOR_FLAG_NORM, TNS_VECTOR_FLAG_NORM_RESERVED,
        TNS_VECTOR_FLAG_SPARSE, TNS_VECTOR_FORMAT_BINARY, TNS_VECTOR_FORMAT_FLOAT32,
        TNS_VECTOR_FORMAT_FLOAT64, TNS_VECTOR_FORMAT_INT8, TNS_VECTOR_MAGIC_BYTE,
        TNS_VECTOR_VERSION_BASE, TNS_VECTOR_VERSION_WITH_BINARY, TNS_VECTOR_VERSION_WITH_SPARSE,
    };
    use super::{
        adjust_for_server_compile_caps, adjust_for_server_runtime_caps,
        alter_session_timezone_statement, auth_change_password_payload, auth_connect_string,
        auth_phase_one_payload, auth_phase_two_payload, capabilities_from_accept,
        client_compile_caps, client_runtime_caps, decode_collection_payload, decode_json_payload,
        decode_json_payload_value, decode_object_payload, decode_oracle_binary_double,
        decode_oracle_binary_float, decode_oracle_datetime, decode_oracle_interval_ds,
        decode_oracle_interval_ym, decode_oracle_number, decode_oracle_text, decode_oracle_vector,
        decode_oson_to_json, default_ttc_field_version, define_column_metadata,
        derive_auth_combo_key, des_encrypt_block, encode_auth_password, encode_bfile_locator,
        encode_debug_jdwp_data, encode_oracle_binary_double, encode_oracle_binary_float,
        encode_oracle_bind_text, encode_oracle_number, encode_oracle_timestamp_bind,
        encode_oson_bool_json, encode_oson_date_json, encode_oson_id_json,
        encode_oson_interval_ds_json, encode_oson_interval_ym_json, encode_oson_json,
        encode_oson_number_json, encode_oson_raw_json, encode_oson_string_json,
        encode_oson_timestamp_json, encode_oson_vector_json, encode_physical_rowid,
        encode_temp_clob_text, encode_vector, execute_flags_for_request,
        generate_10g_password_hash, generate_11g_password_hash,
        generate_auth_credentials_from_session_key_parts, hex_encode_upper,
        local_timezone_offset_string, normalize_cursor_ids, normalize_metadata_charset_form,
        oracle_column_type_from_ora_type, oracle_column_type_from_ora_type_for_protocol,
        process_auth_payload, process_describe_body, process_legacy_execute_error,
        process_protocol_message, process_return_parameters, process_row_data,
        process_server_side_piggyback, process_token, process_warning, read_boolean_value,
        read_data_packet_with_control, read_data_packet_with_flags, read_rowid_value,
        read_urowid_value, request_is_dml_returning, request_with_out_bind_types,
        thin_column_from_column_metadata, thin_column_from_object_attr,
        validate_supported_protocol, verify_server_response, windows_code_pages_for_encoding,
        write_bind_rows_for_request, write_bind_value, write_bytes_with_length_for_capabilities,
        write_bytes_with_two_lengths, write_column_metadata, write_current_schema_piggyback,
        write_data_packet, write_data_type_representations, write_end_to_end_piggyback,
        write_eof_data_packet, write_function_code, write_session_state_piggyback, write_ub2,
        write_ub4, write_ub8, AuthCredentials, AuthState, EndToEndAttributes, OracleThinAppContext,
        OracleThinAuthMode, OracleThinCapabilities, OracleThinConfig, OracleThinPurity,
        OracleThinSession, OracleValue, PacketCursor, ServerSidePiggybackState, ThinColumn,
        CS_FORM_IMPLICIT, CS_FORM_NCHAR, ORACLE_CHARSET_AL32UTF8, ORACLE_CHARSET_JA16SJIS,
        ORACLE_CHARSET_KO16KSC5601, ORACLE_CHARSET_KO16MSWIN949, ORACLE_CHARSET_UTF8,
        ORACLE_CHARSET_ZHS16GBK, ORACLE_CHARSET_ZHT16BIG5, TNS_CCAP_END_OF_CALL_STATUS,
        TNS_CCAP_END_OF_RESPONSE, TNS_CCAP_EXPLICIT_BOUNDARY, TNS_CCAP_FIELD_VERSION,
        TNS_CCAP_FIELD_VERSION_20_1, TNS_CCAP_FIELD_VERSION_23_1,
        TNS_CCAP_FIELD_VERSION_23_1_EXT_1, TNS_CCAP_LEGACY_FAST_SESSION_ATTRIBUTES, TNS_CCAP_OCI1,
        TNS_CCAP_TTC1, TNS_CCAP_TTC4, TNS_ESCAPE_CHAR, TNS_FUNC_COMMIT, TNS_FUNC_LOGOFF,
        TNS_FUNC_ROLLBACK, TNS_JSON_MAX_LENGTH, TNS_KEYWORD_NUM_CURRENT_SCHEMA,
        TNS_KEYWORD_NUM_EDITION, TNS_KEYWORD_NUM_TRANSACTION_ID, TNS_LEGACY_CLR_CHUNK_SIZE,
        TNS_MAX_ROWID_LENGTH, TNS_MAX_UROWID_LENGTH, TNS_MSG_TYPE_ERROR, TNS_MSG_TYPE_PARAMETER,
        TNS_MSG_TYPE_PROTOCOL, TNS_MSG_TYPE_ROW_DATA, TNS_MSG_TYPE_SERVER_SIDE_PIGGYBACK,
        TNS_OBJ_HAS_INDEXES, TNS_OBJ_IS_DEGENERATE, TNS_OBJ_NO_PREFIX_SEG, TNS_RCAP_TTC,
        TNS_RCAP_TTC_32K, TNS_RCAP_TTC_SESSION_STATE_OPS, TNS_RCAP_TTC_ZERO_COPY,
        TNS_SERVER_PIGGYBACK_LTXID, TNS_SERVER_PIGGYBACK_SESS_RET,
        TNS_SERVER_PIGGYBACK_TRACE_EVENT, TNS_TPC_TXNID_SYNC_SERVER, TNS_TPC_TXNID_SYNC_SET,
        TNS_TPC_TXNID_SYNC_UNSET, TNS_VERIFIER_TYPE_10G, TNS_VERIFIER_TYPE_11G_1,
        TNS_VERIFIER_TYPE_11G_2, TNS_VERIFIER_TYPE_12C, TNS_XML_TYPE_LOB, TNS_XML_TYPE_STRING,
    };
    use crate::connect::{AcceptInfo, ConnectOptions, ConnectTarget, OracleNetServerType};
    use crate::exec::{
        BindInputValue, BindValue, ColumnMetadata, OracleColumnType, OracleIntervalDaySecond,
        OracleIntervalYearMonth, OracleVectorValue, RefCursorValue, StatementRequest,
    };

    fn test_session_with_stream(stream: TcpStream) -> OracleThinSession {
        OracleThinSession {
            stream,
            config: OracleThinConfig::new(
                ConnectTarget::service_name("127.0.0.1", 1521, "XE"),
                "user",
                "password",
            ),
            capabilities: OracleThinCapabilities::default(),
            server_version: None,
            broken: false,
            call_timeout: None,
            pending_cursor_closes: Vec::new(),
            last_rows_by_cursor: HashMap::new(),
            cursor_columns_by_cursor: HashMap::new(),
            ref_cursor_ids: HashSet::new(),
            object_attrs_by_type: HashMap::new(),
            collection_element_by_type: HashMap::new(),
            combo_key: Some(vec![0; 16]),
            deferred_cursor_closes: HashMap::new(),
            deferred_cursor_parent_by_child: HashMap::new(),
            pending_current_schema: None,
            pending_end_to_end: EndToEndAttributes::default(),
            server_state: ServerSidePiggybackState::default(),
            in_request: false,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            ttc_sequence: 3,
            closed: true,
        }
    }

    fn tns_test_packet(protocol_version: u16, packet_type: u8, body: &[u8]) -> Vec<u8> {
        let size = 8 + body.len();
        let mut packet = vec![0u8; 8];
        if protocol_version >= 315 {
            packet[0..4].copy_from_slice(&(size as u32).to_be_bytes());
        } else {
            packet[0..2].copy_from_slice(&(size as u16).to_be_bytes());
        }
        packet[4] = packet_type;
        packet.extend_from_slice(body);
        packet
    }

    fn read_one_tns_test_packet(stream: &mut TcpStream, protocol_version: u16) {
        let _ = read_tns_test_packet(stream, protocol_version);
    }

    fn read_tns_test_packet(stream: &mut TcpStream, protocol_version: u16) -> Vec<u8> {
        let mut header = [0u8; 8];
        stream.read_exact(&mut header).unwrap();
        read_tns_test_packet_body_after_header(stream, protocol_version, header)
    }

    fn read_tns_test_packet_body_after_header(
        stream: &mut TcpStream,
        protocol_version: u16,
        header: [u8; 8],
    ) -> Vec<u8> {
        let size = if protocol_version >= 315 {
            u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize
        } else {
            u16::from_be_bytes([header[0], header[1]]) as usize
        };
        let mut body = vec![0u8; size - 8];
        stream.read_exact(&mut body).unwrap();
        body
    }

    fn write_tns_status_response(stream: &mut TcpStream, protocol_version: u16) {
        write_tns_status_response_with_call_status(stream, protocol_version, 0);
    }

    fn write_tns_status_response_with_call_status(
        stream: &mut TcpStream,
        protocol_version: u16,
        call_status: u32,
    ) {
        let mut response = vec![0, 0, TNS_MSG_TYPE_STATUS];
        write_ub4(&mut response, call_status);
        write_ub2(&mut response, 0);
        let packet = tns_test_packet(protocol_version, TNS_PACKET_TYPE_DATA, &response);
        stream.write_all(&packet).unwrap();
    }

    #[test]
    fn write_data_packet_uses_protocol_specific_length_and_data_flags_for_chunks() {
        for protocol_version in [314, 315, 318, 319] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut packets = Vec::new();
                for _ in 0..3 {
                    let mut header = [0u8; 8];
                    stream.read_exact(&mut header).unwrap();
                    let size = if protocol_version >= 315 {
                        u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize
                    } else {
                        u16::from_be_bytes([header[0], header[1]]) as usize
                    };
                    let mut body = vec![0u8; size - 8];
                    stream.read_exact(&mut body).unwrap();
                    packets.push((header, body));
                }
                packets
            });
            let mut stream = TcpStream::connect(addr).unwrap();

            write_data_packet(&mut stream, protocol_version, 2, &[1, 2, 3, 4, 5]).unwrap();
            drop(stream);
            let packets = server.join().unwrap();

            for (index, ((header, body), expected_payload)) in packets
                .iter()
                .zip([&[1, 2][..], &[3, 4][..], &[5][..]])
                .enumerate()
            {
                let expected_size = 10 + expected_payload.len();
                if protocol_version >= 315 {
                    assert_eq!(
                        u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize,
                        expected_size,
                        "{protocol_version} packet {index}"
                    );
                } else {
                    assert_eq!(
                        u16::from_be_bytes([header[0], header[1]]) as usize,
                        expected_size,
                        "{protocol_version} packet {index}"
                    );
                }
                assert_eq!(
                    header[4], TNS_PACKET_TYPE_DATA,
                    "{protocol_version} packet {index}"
                );
                assert_eq!(&body[0..2], &[0, 0], "{protocol_version} packet {index}");
                assert_eq!(
                    &body[2..],
                    expected_payload,
                    "{protocol_version} packet {index}"
                );
            }
        }
    }

    #[test]
    fn write_eof_data_packet_matches_python_oracledb_logoff_close_packet() {
        for protocol_version in [314, 315, 318, 319] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut header = [0u8; 8];
                stream.read_exact(&mut header).unwrap();
                let body =
                    read_tns_test_packet_body_after_header(&mut stream, protocol_version, header);
                (header, body)
            });
            let mut stream = TcpStream::connect(addr).unwrap();

            write_eof_data_packet(&mut stream, protocol_version).unwrap();
            drop(stream);
            let (header, body) = server.join().unwrap();

            let size = if protocol_version >= 315 {
                u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize
            } else {
                u16::from_be_bytes([header[0], header[1]]) as usize
            };
            assert_eq!(size, 10, "{protocol_version}");
            assert_eq!(header[4], TNS_PACKET_TYPE_DATA, "{protocol_version}");
            assert_eq!(body, TNS_DATA_FLAGS_EOF.to_be_bytes(), "{protocol_version}");
        }
    }

    #[test]
    fn close_sends_logoff_then_eof_like_python_oracledb() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let logoff_body = read_tns_test_packet(&mut stream, 319);
            assert_eq!(&logoff_body[..2], &[0, 0]);
            assert_eq!(
                &logoff_body[2..],
                &[TNS_MSG_TYPE_FUNCTION, TNS_FUNC_LOGOFF, 3]
            );

            let mut response = vec![0, 0, TNS_MSG_TYPE_STATUS];
            response.extend_from_slice(&0u32.to_be_bytes());
            response.extend_from_slice(&0u16.to_be_bytes());
            let packet = tns_test_packet(319, TNS_PACKET_TYPE_DATA, &response);
            stream.write_all(&packet).unwrap();

            read_tns_test_packet(&mut stream, 319)
        });
        let stream = TcpStream::connect(addr).unwrap();
        let mut session = test_session_with_stream(stream);
        session.closed = false;

        session.close().unwrap();
        let eof_body = server.join().unwrap();

        assert_eq!(eof_body, TNS_DATA_FLAGS_EOF.to_be_bytes());
    }

    #[test]
    fn ping_sends_tns_ping_function_like_python_oracledb() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let ping_body = read_tns_test_packet(&mut stream, 319);

            let mut response = vec![0, 0, TNS_MSG_TYPE_STATUS];
            response.extend_from_slice(&0u32.to_be_bytes());
            response.extend_from_slice(&0u16.to_be_bytes());
            let packet = tns_test_packet(319, TNS_PACKET_TYPE_DATA, &response);
            stream.write_all(&packet).unwrap();

            ping_body
        });
        let stream = TcpStream::connect(addr).unwrap();
        let mut session = test_session_with_stream(stream);

        session.ping().unwrap();
        let ping_body = server.join().unwrap();

        assert_eq!(
            &ping_body[..],
            &[0, 0, TNS_MSG_TYPE_FUNCTION, TNS_FUNC_PING, 3]
        );
    }

    #[test]
    fn status_call_status_updates_transaction_in_progress_like_python_oracledb() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_one_tns_test_packet(&mut stream, 319);
            write_tns_status_response_with_call_status(
                &mut stream,
                319,
                TNS_EOCS_FLAGS_TXN_IN_PROGRESS,
            );
            read_one_tns_test_packet(&mut stream, 319);
            write_tns_status_response_with_call_status(&mut stream, 319, 0);
        });
        let stream = TcpStream::connect(addr).unwrap();
        let mut session = test_session_with_stream(stream);
        session.capabilities.supports_end_of_call_status = true;

        session.ping().unwrap();
        assert!(session.transaction_in_progress());
        session.ping().unwrap();
        assert!(!session.transaction_in_progress());
        server.join().unwrap();
    }

    #[test]
    fn warning_message_is_preserved_like_python_oracledb() {
        let caps = OracleThinCapabilities::default();
        let mut packet = Vec::new();
        write_ub2(&mut packet, 28002);
        let message = b"ORA-28002: password will expire soon   ";
        write_ub2(&mut packet, message.len() as u16);
        write_ub2(&mut packet, 0);
        packet.extend_from_slice(message);
        let mut cursor = PacketCursor::with_capabilities(&packet, &caps);

        let warning = process_warning(&mut cursor, &caps).unwrap().unwrap();

        assert_eq!(warning.code, 28002);
        assert_eq!(warning.message, "ORA-28002: password will expire soon");
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn session_state_piggyback_matches_python_oracledb_wire_shape() {
        let caps = OracleThinCapabilities::default();
        let mut payload = Vec::new();

        write_session_state_piggyback(&mut payload, &caps, 7, TNS_SESSION_STATE_REQUEST_BEGIN);

        assert_eq!(
            payload,
            vec![
                TNS_MSG_TYPE_PIGGYBACK,
                TNS_FUNC_SESSION_STATE,
                7,
                1,
                TNS_SESSION_STATE_REQUEST_BEGIN | TNS_SESSION_STATE_EXPLICIT_BOUNDARY
            ]
        );
    }

    #[test]
    fn begin_and_end_request_send_session_state_piggybacks_like_python_oracledb() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let begin_body = read_tns_test_packet(&mut stream, 319);
            write_tns_status_response(&mut stream, 319);
            let end_body = read_tns_test_packet(&mut stream, 319);
            write_tns_status_response(&mut stream, 319);
            (begin_body, end_body)
        });
        let stream = TcpStream::connect(addr).unwrap();
        let mut session = test_session_with_stream(stream);
        session.capabilities.supports_request_boundaries = true;

        session.begin_request().unwrap();
        assert!(session.in_request);
        session.end_request().unwrap();
        assert!(!session.in_request);
        let (begin_body, end_body) = server.join().unwrap();

        assert_eq!(
            begin_body,
            vec![
                0,
                0,
                TNS_MSG_TYPE_PIGGYBACK,
                TNS_FUNC_SESSION_STATE,
                3,
                1,
                TNS_SESSION_STATE_REQUEST_BEGIN | TNS_SESSION_STATE_EXPLICIT_BOUNDARY,
                TNS_MSG_TYPE_FUNCTION,
                TNS_FUNC_PING,
                4
            ]
        );
        assert_eq!(
            end_body,
            vec![
                0,
                0,
                TNS_MSG_TYPE_PIGGYBACK,
                TNS_FUNC_SESSION_STATE,
                5,
                1,
                TNS_SESSION_STATE_REQUEST_END | TNS_SESSION_STATE_EXPLICIT_BOUNDARY,
                TNS_MSG_TYPE_FUNCTION,
                TNS_FUNC_PING,
                6
            ]
        );
    }

    #[test]
    fn reset_before_reuse_rolls_back_before_ending_request() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let rollback_body = read_tns_test_packet(&mut stream, 319);
            write_tns_status_response(&mut stream, 319);
            let end_body = read_tns_test_packet(&mut stream, 319);
            write_tns_status_response(&mut stream, 319);
            (rollback_body, end_body)
        });
        let stream = TcpStream::connect(addr).unwrap();
        let mut session = test_session_with_stream(stream);
        session.capabilities.supports_request_boundaries = true;
        session.in_request = true;

        session.reset_before_reuse().unwrap();
        assert!(!session.in_request);
        let (rollback_body, end_body) = server.join().unwrap();

        assert_eq!(
            rollback_body,
            vec![0, 0, TNS_MSG_TYPE_FUNCTION, TNS_FUNC_ROLLBACK, 3]
        );
        assert_eq!(
            end_body,
            vec![
                0,
                0,
                TNS_MSG_TYPE_PIGGYBACK,
                TNS_FUNC_SESSION_STATE,
                4,
                1,
                TNS_SESSION_STATE_REQUEST_END | TNS_SESSION_STATE_EXPLICIT_BOUNDARY,
                TNS_MSG_TYPE_FUNCTION,
                TNS_FUNC_PING,
                5
            ]
        );
    }

    #[test]
    fn read_data_packet_converts_reset_marker_to_end_of_response_for_all_protocols() {
        for protocol_version in [314, 315, 318, 319] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .write_all(&tns_test_packet(
                        protocol_version,
                        TNS_PACKET_TYPE_MARKER,
                        &[1, 0, TNS_MARKER_TYPE_RESET],
                    ))
                    .unwrap();
            });
            let stream = TcpStream::connect(addr).unwrap();
            let mut session = test_session_with_stream(stream);

            let (flags, payload) =
                read_data_packet_with_flags(&mut session.stream, protocol_version).unwrap();

            assert_eq!(flags, TNS_DATA_FLAGS_END_OF_RESPONSE, "{protocol_version}");
            assert_eq!(
                payload,
                vec![TNS_MSG_TYPE_END_OF_RESPONSE],
                "{protocol_version}"
            );
            server.join().unwrap();
        }
    }

    #[test]
    fn read_data_packet_replies_to_break_marker_and_continues_for_all_protocols() {
        for protocol_version in [314, 315, 318, 319] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .write_all(&tns_test_packet(
                        protocol_version,
                        TNS_PACKET_TYPE_MARKER,
                        &[1, 0, TNS_MARKER_TYPE_BREAK],
                    ))
                    .unwrap();

                let mut header = [0u8; 8];
                stream.read_exact(&mut header).unwrap();
                let size = if protocol_version >= 315 {
                    u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize
                } else {
                    u16::from_be_bytes([header[0], header[1]]) as usize
                };
                let mut body = vec![0u8; size - 8];
                stream.read_exact(&mut body).unwrap();
                assert_eq!(header[4], TNS_PACKET_TYPE_MARKER, "{protocol_version}");
                assert_eq!(
                    body,
                    vec![1, 0, TNS_MARKER_TYPE_RESET],
                    "{protocol_version}"
                );

                let mut data_body = Vec::new();
                data_body.extend_from_slice(&TNS_DATA_FLAGS_EOF.to_be_bytes());
                data_body.extend_from_slice(&[0xaa, 0xbb]);
                stream
                    .write_all(&tns_test_packet(
                        protocol_version,
                        TNS_PACKET_TYPE_DATA,
                        &data_body,
                    ))
                    .unwrap();
            });
            let stream = TcpStream::connect(addr).unwrap();
            let mut session = test_session_with_stream(stream);

            let (flags, payload) =
                read_data_packet_with_flags(&mut session.stream, protocol_version).unwrap();

            assert_eq!(flags, TNS_DATA_FLAGS_EOF, "{protocol_version}");
            assert_eq!(payload, vec![0xaa, 0xbb], "{protocol_version}");
            server.join().unwrap();
        }
    }

    #[test]
    fn cancelled_execute_read_error_marks_session_broken_and_returns_ora_01013() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_one_tns_test_packet(&mut stream, 319);
        });
        let stream = TcpStream::connect(addr).unwrap();
        let mut session = test_session_with_stream(stream);
        // Tier-1 cancel pending; the mock server drops the socket so the reset
        // handshake cannot complete and the session must be marked broken.
        session
            .cancel_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let err = session
            .execute_request(&StatementRequest::query("SELECT * FROM dual", 1))
            .expect_err("cancelled execute read should fail");

        assert!(
            err.to_string().contains("ORA-01013"),
            "unexpected cancel error: {err}"
        );
        assert!(session.is_broken());
        server.join().unwrap();
    }

    #[test]
    fn cancelled_fetch_read_error_marks_session_broken_and_returns_ora_01013() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_one_tns_test_packet(&mut stream, 319);
        });
        let stream = TcpStream::connect(addr).unwrap();
        let mut session = test_session_with_stream(stream);
        // Tier-1 cancel pending; the mock server drops the socket so the reset
        // handshake cannot complete and the session must be marked broken.
        session
            .cancel_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let err = session
            .fetch_ref_cursor_batch(7, &[], 1, false)
            .expect_err("cancelled fetch read should fail");

        assert!(
            err.to_string().contains("ORA-01013"),
            "unexpected cancel error: {err}"
        );
        assert!(session.is_broken());
        server.join().unwrap();
    }

    #[test]
    fn cancelled_simple_ttc_call_read_error_marks_session_broken_and_returns_ora_01013() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_one_tns_test_packet(&mut stream, 319);
        });
        let stream = TcpStream::connect(addr).unwrap();
        let mut session = test_session_with_stream(stream);
        // Tier-1 cancel pending; the mock server drops the socket so the reset
        // handshake cannot complete and the session must be marked broken.
        session
            .cancel_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let err = session
            .simple_ttc_call(TNS_FUNC_COMMIT, "commit")
            .expect_err("cancelled simple TTC call should fail");

        assert!(
            err.to_string().contains("ORA-01013"),
            "unexpected cancel error: {err}"
        );
        assert!(session.is_broken());
        server.join().unwrap();
    }

    #[test]
    fn cancel_reset_drains_break_response_and_keeps_connection_reusable() {
        for protocol_version in [314, 315, 318, 319] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                // python-oracledb (315/318/319) style: the drain opens with the
                // client RESET (we read it back), then the server answers with
                // its own RESET marker followed by the trailing ORA-01013
                // error/end-of-response data packet that closes the boundary.
                read_one_tns_test_packet(&mut stream, protocol_version);
                stream
                    .write_all(&tns_test_packet(
                        protocol_version,
                        TNS_PACKET_TYPE_MARKER,
                        &[1, 0, TNS_MARKER_TYPE_RESET],
                    ))
                    .unwrap();
                let mut data_body = Vec::new();
                data_body.extend_from_slice(&TNS_DATA_FLAGS_END_OF_RESPONSE.to_be_bytes());
                data_body.extend_from_slice(&[TNS_MSG_TYPE_ERROR]);
                stream
                    .write_all(&tns_test_packet(
                        protocol_version,
                        TNS_PACKET_TYPE_DATA,
                        &data_body,
                    ))
                    .unwrap();
                // Connection stays open after a graceful cancel (reused).
                std::thread::sleep(Duration::from_millis(200));
            });
            let stream = TcpStream::connect(addr).unwrap();
            let mut session = test_session_with_stream(stream);
            session.capabilities.protocol_version = Some(protocol_version);

            session
                .drain_cancel_response()
                .unwrap_or_else(|err| panic!("protocol {protocol_version} drain failed: {err}"));
            assert!(
                !session.is_broken(),
                "protocol {protocol_version}: connection should remain reusable"
            );
            server.join().unwrap();
        }
    }

    #[test]
    fn break_execution_without_oob_sends_single_interrupt_marker() {
        // Mirrors python-oracledb `_break_external`: with OOB unavailable the
        // graceful break is signalled by exactly one in-band INTERRUPT marker.
        for protocol_version in [314, 315, 318, 319] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut header = [0u8; 8];
                stream.read_exact(&mut header).unwrap();
                let size = if protocol_version >= 315 {
                    u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize
                } else {
                    u16::from_be_bytes([header[0], header[1]]) as usize
                };
                let mut body = vec![0u8; size - 8];
                stream.read_exact(&mut body).unwrap();
                // No further bytes should follow the single marker packet.
                stream
                    .set_read_timeout(Some(Duration::from_millis(100)))
                    .unwrap();
                let mut extra = [0u8; 1];
                let trailing = stream.read(&mut extra);
                (header[4], body, trailing.unwrap_or(0))
            });
            let stream = TcpStream::connect(addr).unwrap();
            let mut session = test_session_with_stream(stream);
            session.capabilities.protocol_version = Some(protocol_version);
            session.capabilities.supports_oob = false;

            session
                .break_execution()
                .unwrap_or_else(|err| panic!("protocol {protocol_version} break failed: {err}"));

            let (packet_type, body, trailing) = server.join().unwrap();
            assert_eq!(
                packet_type, TNS_PACKET_TYPE_MARKER,
                "protocol {protocol_version}: expected a marker packet"
            );
            assert_eq!(
                body.last().copied(),
                Some(TNS_MARKER_TYPE_INTERRUPT),
                "protocol {protocol_version}: expected an INTERRUPT marker"
            );
            assert_eq!(
                trailing, 0,
                "protocol {protocol_version}: only the marker should be sent"
            );
        }
    }

    #[test]
    fn cancel_reset_terminates_on_trailing_data_without_server_reset() {
        // go-ora (protocol 314) style: after the client RESET the server sends
        // exactly one data packet (no server RESET marker). The drain must stop
        // on that data packet rather than waiting for the quiet timeout.
        for protocol_version in [314, 315, 318, 319] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                // client RESET that opens the handshake
                read_one_tns_test_packet(&mut stream, protocol_version);
                let mut data_body = Vec::new();
                data_body.extend_from_slice(&TNS_DATA_FLAGS_END_OF_RESPONSE.to_be_bytes());
                data_body.extend_from_slice(&[TNS_MSG_TYPE_ERROR]);
                stream
                    .write_all(&tns_test_packet(
                        protocol_version,
                        TNS_PACKET_TYPE_DATA,
                        &data_body,
                    ))
                    .unwrap();
                std::thread::sleep(Duration::from_millis(200));
            });
            let stream = TcpStream::connect(addr).unwrap();
            let mut session = test_session_with_stream(stream);
            session.capabilities.protocol_version = Some(protocol_version);

            let started = std::time::Instant::now();
            session
                .drain_cancel_response()
                .unwrap_or_else(|err| panic!("protocol {protocol_version} drain failed: {err}"));
            assert!(
                started.elapsed() < CANCEL_RESET_DRAIN_TIMEOUT,
                "protocol {protocol_version}: drain should stop on the data packet, not time out"
            );
            assert!(
                !session.is_broken(),
                "protocol {protocol_version}: connection should remain reusable"
            );
            server.join().unwrap();
        }
    }

    #[test]
    fn pending_cursor_close_ids_are_normalized_when_drained_and_requeued() {
        assert_eq!(normalize_cursor_ids(vec![0, 9, 3, 9, 1]), vec![1, 3, 9]);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            drop(stream);
        });
        let stream = TcpStream::connect(addr).unwrap();
        let mut session = test_session_with_stream(stream);

        session.pending_cursor_closes = vec![3, 0, 3, 1];
        assert_eq!(session.drain_pending_cursor_closes(), vec![1, 3]);
        assert!(session.pending_cursor_closes.is_empty());

        session.pending_cursor_closes = vec![7, 5];
        session.requeue_pending_cursor_closes(&[0, 3, 7, 3]);
        assert_eq!(session.pending_cursor_closes, vec![3, 5, 7]);

        drop(session);
        server.join().unwrap();
    }

    #[test]
    fn close_temp_lobs_piggyback_uses_free_temp_array_operation() {
        let mut payload = Vec::new();
        let locators = [vec![1, 2, 3], vec![4, 5]];
        write_close_temp_lobs_piggyback(
            &mut payload,
            &OracleThinCapabilities::default(),
            7,
            &locators,
        )
        .unwrap();
        let mut expected = vec![TNS_MSG_TYPE_PIGGYBACK, TNS_FUNC_LOB_OP, 7, 1];
        write_ub4(&mut expected, 5);
        expected.push(0);
        write_ub4(&mut expected, 0);
        write_ub4(&mut expected, 0);
        write_ub4(&mut expected, 0);
        expected.extend_from_slice(&[0, 0, 0]);
        write_ub4(&mut expected, TNS_LOB_OP_FREE_TEMP | TNS_LOB_OP_ARRAY);
        expected.push(0);
        write_ub4(&mut expected, 0);
        write_ub8(&mut expected, 0);
        write_ub8(&mut expected, 0);
        expected.extend_from_slice(&[0, 0]);
        write_ub4(&mut expected, 0);
        expected.push(0);
        write_ub4(&mut expected, 0);
        expected.push(0);
        write_ub4(&mut expected, 0);
        expected.extend_from_slice(&[1, 2, 3, 4, 5]);

        assert_eq!(payload, expected);
    }

    #[test]
    fn current_schema_piggyback_matches_python_oracledb_wire_shape() {
        let mut payload = Vec::new();
        write_current_schema_piggyback(
            &mut payload,
            &OracleThinCapabilities::default(),
            8,
            "APP_USER",
        )
        .unwrap();

        let mut cursor = PacketCursor::with_capabilities(
            &payload,
            &OracleThinCapabilities {
                protocol_version: Some(319),
                ..OracleThinCapabilities::default()
            },
        );
        assert_eq!(cursor.read_u8().unwrap(), TNS_MSG_TYPE_PIGGYBACK);
        assert_eq!(cursor.read_u8().unwrap(), TNS_FUNC_SET_SCHEMA);
        assert_eq!(cursor.read_u8().unwrap(), 8);
        assert_eq!(cursor.read_u8().unwrap(), 1);
        assert_eq!(cursor.read_ub4().unwrap(), 8);
        assert_eq!(
            String::from_utf8(cursor.read_bytes().unwrap().unwrap()).unwrap(),
            "APP_USER"
        );
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn end_to_end_piggyback_matches_python_oracledb_wire_order() {
        let attrs = EndToEndAttributes {
            action: Some(Some("open".to_string())),
            client_identifier: Some(Some("alice".to_string())),
            client_info: Some(None),
            dbop: Some(Some("dashboard".to_string())),
            module: Some(Some("space-query".to_string())),
        };
        let mut payload = Vec::new();
        write_end_to_end_piggyback(&mut payload, &OracleThinCapabilities::default(), 9, &attrs)
            .unwrap();

        let mut cursor = PacketCursor::with_capabilities(
            &payload,
            &OracleThinCapabilities {
                protocol_version: Some(319),
                ..OracleThinCapabilities::default()
            },
        );
        assert_eq!(cursor.read_u8().unwrap(), TNS_MSG_TYPE_PIGGYBACK);
        assert_eq!(cursor.read_u8().unwrap(), TNS_FUNC_SET_END_TO_END_ATTR);
        assert_eq!(cursor.read_u8().unwrap(), 9);
        assert_eq!(cursor.read_u8().unwrap(), 0);
        assert_eq!(cursor.read_u8().unwrap(), 0);
        assert_eq!(
            cursor.read_ub4().unwrap(),
            TNS_END_TO_END_ACTION
                | TNS_END_TO_END_CLIENT_IDENTIFIER
                | TNS_END_TO_END_CLIENT_INFO
                | TNS_END_TO_END_DBOP
                | TNS_END_TO_END_MODULE
        );
        assert_eq!(cursor.read_u8().unwrap(), 1);
        assert_eq!(cursor.read_ub4().unwrap(), 5);
        assert_eq!(cursor.read_u8().unwrap(), 1);
        assert_eq!(cursor.read_ub4().unwrap(), 11);
        assert_eq!(cursor.read_u8().unwrap(), 1);
        assert_eq!(cursor.read_ub4().unwrap(), 4);
        assert_eq!(cursor.read_u8().unwrap(), 0);
        assert_eq!(cursor.read_ub4().unwrap(), 0);
        assert_eq!(cursor.read_u8().unwrap(), 0);
        assert_eq!(cursor.read_ub4().unwrap(), 0);
        assert_eq!(cursor.read_u8().unwrap(), 1);
        assert_eq!(cursor.read_ub4().unwrap(), 0);
        assert_eq!(cursor.read_u8().unwrap(), 0);
        assert_eq!(cursor.read_ub4().unwrap(), 0);
        assert_eq!(cursor.read_u8().unwrap(), 0);
        assert_eq!(cursor.read_ub4().unwrap(), 0);
        assert_eq!(cursor.read_u8().unwrap(), 1);
        assert_eq!(cursor.read_ub4().unwrap(), 9);
        assert_eq!(
            String::from_utf8(cursor.read_bytes().unwrap().unwrap()).unwrap(),
            "alice"
        );
        assert_eq!(
            String::from_utf8(cursor.read_bytes().unwrap().unwrap()).unwrap(),
            "space-query"
        );
        assert_eq!(
            String::from_utf8(cursor.read_bytes().unwrap().unwrap()).unwrap(),
            "open"
        );
        assert_eq!(
            String::from_utf8(cursor.read_bytes().unwrap().unwrap()).unwrap(),
            "dashboard"
        );
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn end_to_end_setters_are_noop_for_go_ora_protocol_314() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let _ = listener.accept().unwrap();
        });
        let stream = TcpStream::connect(addr).unwrap();
        let mut session = test_session_with_stream(stream);
        session.capabilities.protocol_version = Some(314);

        session.set_module(Some("module".to_string()));
        session.set_action(Some("action".to_string()));
        session.set_client_identifier(Some("client".to_string()));
        session.set_client_info(Some("info".to_string()));
        session.set_dbop(Some("dbop".to_string()));

        assert!(session.pending_end_to_end.is_empty());
        drop(session);
        server.join().unwrap();
    }

    #[test]
    fn create_temp_lob_request_includes_charset_pointer_like_python_oracledb() {
        let locator = vec![0; 40];
        let mut payload = Vec::new();
        write_lob_operation_request(
            &mut payload,
            &OracleThinCapabilities::default(),
            9,
            &locator,
            TNS_LOB_OP_CREATE_TEMP,
            u64::from(CS_FORM_IMPLICIT),
            u64::from(ORA_TYPE_NUM_BLOB),
            TNS_DURATION_SESSION,
            None,
            Some(TNS_CHARSET_UTF8),
        )
        .unwrap();

        let mut expected = vec![TNS_MSG_TYPE_FUNCTION, TNS_FUNC_LOB_OP, 9, 1];
        write_ub4(&mut expected, locator.len() as u32);
        expected.push(0);
        write_ub4(&mut expected, TNS_DURATION_SESSION);
        write_ub4(&mut expected, 0);
        write_ub4(&mut expected, 0);
        expected.extend_from_slice(&[1, 0, 1]);
        write_ub4(&mut expected, TNS_LOB_OP_CREATE_TEMP);
        expected.extend_from_slice(&[0, 0]);
        write_ub8(&mut expected, u64::from(CS_FORM_IMPLICIT));
        write_ub8(&mut expected, u64::from(ORA_TYPE_NUM_BLOB));
        expected.push(0);
        put_u16_be_vec(&mut expected, 0);
        put_u16_be_vec(&mut expected, 0);
        put_u16_be_vec(&mut expected, 0);
        expected.extend_from_slice(&locator);
        write_ub4(&mut expected, u32::from(TNS_CHARSET_UTF8));

        assert_eq!(payload, expected);
    }

    #[test]
    fn temp_lob_create_failure_frees_created_locator() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let locator = vec![0x42; 40];
        let expected_locator = locator.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _create_request = read_tns_test_packet(&mut stream, 319);

            let mut create_response = vec![TNS_MSG_TYPE_PARAMETER];
            create_response.extend_from_slice(&locator);
            write_ub2(&mut create_response, 0);
            create_response.push(0);
            create_response.push(TNS_MSG_TYPE_STATUS);
            write_ub4(&mut create_response, 0);
            write_ub2(&mut create_response, 0);
            write_data_packet(&mut stream, 319, TNS_DEFAULT_SDU, &create_response).unwrap();

            let free_request = read_tns_test_packet(&mut stream, 319);
            let mut free_response = vec![TNS_MSG_TYPE_STATUS];
            write_ub4(&mut free_response, 0);
            write_ub2(&mut free_response, 0);
            write_data_packet(&mut stream, 319, TNS_DEFAULT_SDU, &free_response).unwrap();
            free_request
        });
        let stream = TcpStream::connect(addr).unwrap();
        let mut session = test_session_with_stream(stream);
        session.capabilities.protocol_version = Some(319);
        session.capabilities.ncharset_id = 9999;

        let err = session
            .create_temp_nclob("\u{D55C}")
            .expect_err("unsupported ncharset should fail after temp LOB create");
        assert!(err
            .to_string()
            .contains("national character set id 9999 is not supported"));
        drop(session);

        let free_request = server.join().unwrap();
        let mut cursor =
            PacketCursor::with_capabilities(&free_request[2..], &OracleThinCapabilities::default());
        assert_eq!(cursor.read_u8().unwrap(), TNS_MSG_TYPE_PIGGYBACK);
        assert_eq!(cursor.read_u8().unwrap(), TNS_FUNC_LOB_OP);
        let _sequence = cursor.read_u8().unwrap();
        assert_eq!(cursor.read_u8().unwrap(), 1);
        assert_eq!(cursor.read_ub4().unwrap(), expected_locator.len() as u32);
        assert_eq!(cursor.read_u8().unwrap(), 0);
        assert_eq!(cursor.read_ub4().unwrap(), 0);
        assert_eq!(cursor.read_ub4().unwrap(), 0);
        assert_eq!(cursor.read_ub4().unwrap(), 0);
        assert_eq!(cursor.read_u8().unwrap(), 0);
        assert_eq!(cursor.read_u8().unwrap(), 0);
        assert_eq!(cursor.read_u8().unwrap(), 0);
        assert_eq!(
            cursor.read_ub4().unwrap(),
            TNS_LOB_OP_FREE_TEMP | TNS_LOB_OP_ARRAY
        );
        assert!(free_request
            .windows(expected_locator.len())
            .any(|window| window == expected_locator.as_slice()));
        assert!(free_request
            .windows(2)
            .any(|window| { window == [TNS_MSG_TYPE_FUNCTION, TNS_FUNC_PING] }));
    }

    #[test]
    fn nested_cursor_parent_close_is_queued_after_all_children_close() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            drop(stream);
        });
        let stream = TcpStream::connect(addr).unwrap();
        let mut session = test_session_with_stream(stream);
        session
            .deferred_cursor_closes
            .insert(10, HashSet::from([11, 12]));
        session.deferred_cursor_parent_by_child.insert(11, 10);
        session.deferred_cursor_parent_by_child.insert(12, 10);

        session.close_cursor_later(Some(11));
        assert_eq!(session.pending_cursor_closes, vec![11]);
        assert!(session.deferred_cursor_closes.contains_key(&10));

        session.close_cursor_later(Some(12));
        assert_eq!(session.pending_cursor_closes, vec![11, 12, 10]);
        assert!(!session.deferred_cursor_closes.contains_key(&10));
        assert!(!session.deferred_cursor_parent_by_child.contains_key(&12));

        drop(session);
        server.join().unwrap();
    }

    #[test]
    fn partial_fetch_error_queues_child_cursors_before_parent_close() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            drop(stream);
        });
        let stream = TcpStream::connect(addr).unwrap();
        let mut session = test_session_with_stream(stream);
        let rows = vec![vec![OracleValue::Cursor(RefCursorValue {
            cursor_id: 11,
            columns: Vec::new(),
        })]];

        session.close_cursor_after_partial_rows(10, &rows, true);

        assert_eq!(session.pending_cursor_closes, vec![11, 10]);
        drop(session);
        server.join().unwrap();
    }

    #[test]
    fn scalar_cursor_close_skips_ref_cursor_row_scan() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            drop(stream);
        });
        let stream = TcpStream::connect(addr).unwrap();
        let mut session = test_session_with_stream(stream);
        let rows = vec![vec![OracleValue::Cursor(RefCursorValue {
            cursor_id: 11,
            columns: Vec::new(),
        })]];

        let _ = session.close_fully_fetched_cursor(10, &rows, false);

        assert!(session.deferred_cursor_closes.is_empty());
        assert!(session.deferred_cursor_parent_by_child.is_empty());
        assert!(!session.pending_cursor_closes.contains(&11));
        drop(session);
        server.join().unwrap();
    }

    #[test]
    fn ref_cursor_column_detection_controls_row_scan_fast_path() {
        let scalar_columns = [ColumnMetadata {
            name: "N".to_string(),
            column_type: OracleColumnType::Number,
            charset_form: 0,
            ora_type_num: ORA_TYPE_NUM_NUMBER,
            buffer_size: 22,
            schema_name: String::new(),
            type_name: String::new(),
        }];
        let ref_cursor_columns = [ColumnMetadata {
            name: "RC".to_string(),
            column_type: OracleColumnType::Cursor,
            charset_form: 0,
            ora_type_num: ORA_TYPE_NUM_CURSOR,
            buffer_size: 4,
            schema_name: String::new(),
            type_name: String::new(),
        }];

        assert!(!columns_may_contain_ref_cursors(&scalar_columns));
        assert!(columns_may_contain_ref_cursors(&ref_cursor_columns));
        assert!(!column_types_may_contain_ref_cursors(&[
            OracleColumnType::Number,
            OracleColumnType::Varchar,
        ]));
        assert!(column_types_may_contain_ref_cursors(&[]));
        assert!(column_types_may_contain_ref_cursors(&[
            OracleColumnType::Number,
            OracleColumnType::Cursor,
        ]));
    }

    #[test]
    fn fetch_read_error_queues_active_cursor_close_for_all_protocols() {
        for protocol_version in [314, 315, 318, 319] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                read_one_tns_test_packet(&mut stream, protocol_version);
            });
            let stream = TcpStream::connect(addr).unwrap();
            let mut session = test_session_with_stream(stream);
            session.capabilities.protocol_version = Some(protocol_version);
            session.capabilities.ttc_field_version = default_ttc_field_version(protocol_version);

            let _err = session
                .fetch_ref_cursor_batch(7, &[], 1, false)
                .expect_err("fetch read error should fail");

            assert_eq!(session.pending_cursor_closes, vec![7], "{protocol_version}");
            drop(session);
            server.join().unwrap();
        }
    }

    #[test]
    fn execute_request_requeues_pending_closes_when_payload_build_fails() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            drop(stream);
        });
        let stream = TcpStream::connect(addr).unwrap();
        let mut session = test_session_with_stream(stream);
        session.pending_cursor_closes = vec![9, 3, 9];

        let mut request = StatementRequest::statement("BEGIN NULL; END;");
        request.binds.push(BindValue::Number(String::new()));
        let err = session
            .execute_request(&request)
            .expect_err("invalid bind should fail before request write");

        assert!(
            err.to_string().contains("empty Oracle NUMBER bind value"),
            "unexpected error: {err}"
        );
        assert_eq!(session.pending_cursor_closes, vec![3, 9]);

        drop(session);
        server.join().unwrap();
    }

    #[test]
    fn call_timeout_limits_tns_read_wait() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(500));
            drop(stream);
        });
        let stream = TcpStream::connect(addr).unwrap();
        let mut session = test_session_with_stream(stream);

        session
            .set_call_timeout(Some(Duration::from_millis(50)))
            .expect("set thin call timeout");
        let started = Instant::now();
        let err = read_data_packet_with_flags(&mut session.stream, 319).unwrap_err();

        assert!(
            err.to_string()
                .contains("Read timeout while reading TNS data header"),
            "unexpected timeout error: {err}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "read did not honor the configured call timeout"
        );
        server.join().unwrap();
    }

    #[test]
    fn read_data_packet_skips_supported_control_packets_for_all_supported_protocols() {
        for (protocol_version, control_type) in [
            (314, TNS_CONTROL_TYPE_RESET_OOB),
            (315, TNS_CONTROL_TYPE_INBAND_NOTIFICATION),
            (318, TNS_CONTROL_TYPE_RESET_OOB),
            (319, TNS_CONTROL_TYPE_INBAND_NOTIFICATION),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut control_body = Vec::new();
                control_body.extend_from_slice(&control_type.to_be_bytes());
                if control_type == TNS_CONTROL_TYPE_INBAND_NOTIFICATION {
                    control_body.extend_from_slice(&0u32.to_be_bytes());
                    control_body.extend_from_slice(&TNS_ERR_INBAND_MESSAGE.to_be_bytes());
                }
                stream
                    .write_all(&tns_test_packet(
                        protocol_version,
                        TNS_PACKET_TYPE_CONTROL,
                        &control_body,
                    ))
                    .unwrap();

                let payload = [0xaa, 0xbb, 0xcc];
                let mut data_body = Vec::new();
                data_body.extend_from_slice(&TNS_DATA_FLAGS_EOF.to_be_bytes());
                data_body.extend_from_slice(&payload);
                stream
                    .write_all(&tns_test_packet(
                        protocol_version,
                        TNS_PACKET_TYPE_DATA,
                        &data_body,
                    ))
                    .unwrap();
            });
            let stream = TcpStream::connect(addr).unwrap();
            let mut session = test_session_with_stream(stream);
            session
                .set_call_timeout(Some(Duration::from_secs(1)))
                .expect("set thin call timeout");

            let (flags, payload) =
                read_data_packet_with_flags(&mut session.stream, protocol_version).unwrap();

            assert_eq!(flags, TNS_DATA_FLAGS_EOF, "{protocol_version}");
            assert_eq!(payload, vec![0xaa, 0xbb, 0xcc], "{protocol_version}");
            server.join().unwrap();
        }
    }

    #[test]
    fn read_data_packet_reports_oob_reset_control_packet() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut control_body = Vec::new();
            control_body.extend_from_slice(&TNS_CONTROL_TYPE_RESET_OOB.to_be_bytes());
            stream
                .write_all(&tns_test_packet(
                    319,
                    TNS_PACKET_TYPE_CONTROL,
                    &control_body,
                ))
                .unwrap();

            let mut data_body = Vec::new();
            data_body.extend_from_slice(&TNS_DATA_FLAGS_EOF.to_be_bytes());
            data_body.push(TNS_MSG_TYPE_PROTOCOL);
            stream
                .write_all(&tns_test_packet(319, TNS_PACKET_TYPE_DATA, &data_body))
                .unwrap();
        });
        let stream = TcpStream::connect(addr).unwrap();
        let mut session = test_session_with_stream(stream);
        session
            .set_call_timeout(Some(Duration::from_secs(1)))
            .expect("set thin call timeout");

        let (oob_reset_received, payload) =
            read_data_packet_with_control(&mut session.stream, 319).unwrap();

        assert!(oob_reset_received);
        assert_eq!(payload, vec![TNS_MSG_TYPE_PROTOCOL]);
        server.join().unwrap();
    }

    #[test]
    fn decode_oracle_number_trims_fractional_trailing_zeros() {
        assert_eq!(decode_oracle_number(&[0xc0, 0x33]).unwrap(), "0.5");
        assert_eq!(decode_oracle_number(&[0xc0, 0x51]).unwrap(), "0.8");
    }

    #[test]
    fn encodes_oracle_number_edge_values_like_go_ora_vendor_fixtures() {
        for (value, expected) in [
            ("0", &[128][..]),
            ("1", &[193, 2][..]),
            ("-123.4", &[61, 100, 78, 61, 102][..]),
            ("0.0098765", &[191, 99, 77, 51][..]),
            ("-0.0098765", &[64, 3, 25, 51, 102][..]),
            ("1E+30", &[208, 2][..]),
            ("1E-125", &[130, 11][..]),
        ] {
            assert_eq!(encode_oracle_number(value).unwrap(), expected, "{value}");
        }
    }

    #[test]
    fn decodes_vendor_binary_float_and_double_bytes() {
        assert_eq!(
            decode_oracle_binary_float(&[195, 6, 115, 51]).unwrap(),
            "134.45"
        );
        assert_eq!(
            decode_oracle_binary_float(&[60, 249, 140, 204]).unwrap(),
            "-134.45"
        );
        assert_eq!(
            decode_oracle_binary_double(&[192, 96, 206, 102, 102, 102, 102, 102]).unwrap(),
            "134.45"
        );
        assert_eq!(
            decode_oracle_binary_double(&[63, 159, 49, 153, 153, 153, 153, 153]).unwrap(),
            "-134.45"
        );
        assert_eq!(
            decode_oracle_binary_float(&encode_oracle_binary_float(5.0)).unwrap(),
            "5.0"
        );
        assert_eq!(
            decode_oracle_binary_double(&encode_oracle_binary_double(0.0)).unwrap(),
            "0.0"
        );
        assert_eq!(
            decode_oracle_binary_double(&encode_oracle_binary_double(f64::NAN)).unwrap(),
            "nan"
        );
        assert_eq!(
            decode_oracle_binary_double(&encode_oracle_binary_double(f64::INFINITY)).unwrap(),
            "inf"
        );
    }

    #[test]
    fn encodes_binary_float_and_double_binds_like_python_oracledb() {
        let binary_float = BindValue::BinaryFloat(134.45);
        let metadata = bind_column_metadata(&binary_float);
        assert_eq!(metadata.ora_type_num, ORA_TYPE_NUM_BINARY_FLOAT);
        assert_eq!(metadata.buffer_size, 4);
        let mut payload = Vec::new();
        write_bind_value(
            &mut payload,
            &OracleThinCapabilities::default(),
            &binary_float,
        )
        .unwrap();
        assert_eq!(payload, vec![4, 195, 6, 115, 51]);

        let binary_double = BindValue::BinaryDouble(-134.45);
        let metadata = bind_column_metadata(&binary_double);
        assert_eq!(metadata.ora_type_num, ORA_TYPE_NUM_BINARY_DOUBLE);
        assert_eq!(metadata.buffer_size, 8);
        let mut payload = Vec::new();
        write_bind_value(
            &mut payload,
            &OracleThinCapabilities::default(),
            &binary_double,
        )
        .unwrap();
        assert_eq!(payload, vec![8, 63, 159, 49, 153, 153, 153, 153, 153]);
    }

    #[test]
    fn decodes_vendor_vector_float32_bytes() {
        let vector = [
            0xdb, 0, 0, 0x12, 2, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 195, 6, 115, 51, 60, 249, 140,
            204,
        ];

        assert_eq!(decode_oracle_vector(&vector).unwrap(), "[134.45, -134.45]");
    }

    #[test]
    fn encodes_vector_binds_like_python_oracledb() {
        let cases = [
            (
                OracleVectorValue::Float32(vec![34.6, 77.8]),
                TNS_VECTOR_VERSION_BASE,
                TNS_VECTOR_FLAG_NORM | TNS_VECTOR_FLAG_NORM_RESERVED,
                TNS_VECTOR_FORMAT_FLOAT32,
                2u32,
                "[34.6, 77.8]",
            ),
            (
                OracleVectorValue::Float64(vec![34.6, 77.8]),
                TNS_VECTOR_VERSION_BASE,
                TNS_VECTOR_FLAG_NORM | TNS_VECTOR_FLAG_NORM_RESERVED,
                TNS_VECTOR_FORMAT_FLOAT64,
                2u32,
                "[34.6, 77.8]",
            ),
            (
                OracleVectorValue::Float32(vec![5.0, 1.0]),
                TNS_VECTOR_VERSION_BASE,
                TNS_VECTOR_FLAG_NORM | TNS_VECTOR_FLAG_NORM_RESERVED,
                TNS_VECTOR_FORMAT_FLOAT32,
                2u32,
                "[5.0, 1.0]",
            ),
            (
                OracleVectorValue::Float64(vec![5.0, 1.0]),
                TNS_VECTOR_VERSION_BASE,
                TNS_VECTOR_FLAG_NORM | TNS_VECTOR_FLAG_NORM_RESERVED,
                TNS_VECTOR_FORMAT_FLOAT64,
                2u32,
                "[5.0, 1.0]",
            ),
            (
                OracleVectorValue::Int8(vec![34, -77]),
                TNS_VECTOR_VERSION_BASE,
                TNS_VECTOR_FLAG_NORM | TNS_VECTOR_FLAG_NORM_RESERVED,
                TNS_VECTOR_FORMAT_INT8,
                2u32,
                "[34, -77]",
            ),
            (
                OracleVectorValue::Binary(vec![3, 2, 3]),
                TNS_VECTOR_VERSION_WITH_BINARY,
                TNS_VECTOR_FLAG_NORM_RESERVED,
                TNS_VECTOR_FORMAT_BINARY,
                24u32,
                "[3, 2, 3]",
            ),
            (
                OracleVectorValue::SparseFloat32 {
                    num_dimensions: 16,
                    indices: vec![1, 3, 5],
                    values: vec![1.0, 0.0, 5.0],
                },
                TNS_VECTOR_VERSION_WITH_SPARSE,
                TNS_VECTOR_FLAG_SPARSE | TNS_VECTOR_FLAG_NORM | TNS_VECTOR_FLAG_NORM_RESERVED,
                TNS_VECTOR_FORMAT_FLOAT32,
                16u32,
                "SparseVector(dimensions=16, indices=[1, 3, 5], values=[1.0, 0.0, 5.0])",
            ),
            (
                OracleVectorValue::SparseFloat64 {
                    num_dimensions: 16,
                    indices: vec![1, 3, 5],
                    values: vec![1.0, 0.0, 5.0],
                },
                TNS_VECTOR_VERSION_WITH_SPARSE,
                TNS_VECTOR_FLAG_SPARSE | TNS_VECTOR_FLAG_NORM | TNS_VECTOR_FLAG_NORM_RESERVED,
                TNS_VECTOR_FORMAT_FLOAT64,
                16u32,
                "SparseVector(dimensions=16, indices=[1, 3, 5], values=[1.0, 0.0, 5.0])",
            ),
            (
                OracleVectorValue::SparseInt8 {
                    num_dimensions: 16,
                    indices: vec![1, 3, 5],
                    values: vec![1, 0, 5],
                },
                TNS_VECTOR_VERSION_WITH_SPARSE,
                TNS_VECTOR_FLAG_SPARSE | TNS_VECTOR_FLAG_NORM | TNS_VECTOR_FLAG_NORM_RESERVED,
                TNS_VECTOR_FORMAT_INT8,
                16u32,
                "SparseVector(dimensions=16, indices=[1, 3, 5], values=[1, 0, 5])",
            ),
        ];

        for (value, expected_version, expected_flags, expected_format, expected_count, expected) in
            cases
        {
            let encoded = encode_vector(&value).unwrap();
            assert_eq!(encoded[0], TNS_VECTOR_MAGIC_BYTE);
            assert_eq!(encoded[1], expected_version);
            assert_eq!(&encoded[2..4], &expected_flags.to_be_bytes());
            assert_eq!(encoded[4], expected_format);
            assert_eq!(&encoded[5..9], &expected_count.to_be_bytes());
            assert_eq!(&encoded[9..17], &0u64.to_be_bytes());
            assert_eq!(decode_oracle_vector(&encoded).unwrap(), expected);
            assert_eq!(
                decode_oson_to_json(&encode_oson_vector_json(&value).unwrap()).unwrap(),
                expected
            );

            let mut payload = Vec::new();
            write_bind_value(
                &mut payload,
                &OracleThinCapabilities::default(),
                &BindValue::Vector(value.clone()),
            )
            .unwrap();
            assert!(payload.starts_with(&[1, 40, 40]));
            assert_eq!(&payload[3..7], &[0, 38, 0, 4]);
            assert_eq!(decode_oracle_vector(&payload[44..]).unwrap(), expected);

            let mut payload = Vec::new();
            write_bind_value(
                &mut payload,
                &OracleThinCapabilities::default(),
                &BindValue::JsonVector(value),
            )
            .unwrap();
            assert!(payload.starts_with(&[1, 40, 40]));
            assert_eq!(&payload[3..7], &[0, 38, 0, 4]);
            assert_eq!(decode_oson_to_json(&payload[44..]).unwrap(), expected);
        }
    }

    #[test]
    fn dense_vector_binds_reject_zero_dimensions_like_python_oracledb() {
        for value in [
            OracleVectorValue::Float32(vec![]),
            OracleVectorValue::Float64(vec![]),
            OracleVectorValue::Int8(vec![]),
            OracleVectorValue::Binary(vec![]),
        ] {
            let err = encode_vector(&value).expect_err("empty dense VECTOR should fail");
            assert!(
                err.to_string().contains("zero dimensions"),
                "unexpected encode_vector error: {err}"
            );

            let mut payload = Vec::new();
            let err = write_bind_value(
                &mut payload,
                &OracleThinCapabilities::default(),
                &BindValue::Vector(value.clone()),
            )
            .expect_err("empty dense VECTOR bind should fail");
            assert!(
                err.to_string().contains("zero dimensions"),
                "unexpected VECTOR bind error: {err}"
            );

            let mut payload = Vec::new();
            let err = write_bind_value(
                &mut payload,
                &OracleThinCapabilities::default(),
                &BindValue::JsonVector(value),
            )
            .expect_err("empty dense JSON VECTOR bind should fail");
            assert!(
                err.to_string().contains("zero dimensions"),
                "unexpected JSON VECTOR bind error: {err}"
            );
        }
    }

    #[test]
    fn temp_clob_text_encoding_follows_locator_charset_flags() {
        let caps = OracleThinCapabilities::default();
        let mut locator = vec![0; 40];
        assert_eq!(
            encode_temp_clob_text("A\u{D55C}", &locator, &caps).unwrap(),
            "A\u{D55C}".as_bytes()
        );

        locator[TNS_LOB_LOC_OFFSET_FLAG_3] = TNS_LOB_LOC_FLAGS_VAR_LENGTH_CHARSET;
        assert_eq!(
            encode_temp_clob_text("A\u{D55C}", &locator, &caps).unwrap(),
            vec![0x00, 0x41, 0xd5, 0x5c]
        );

        locator[TNS_LOB_LOC_OFFSET_FLAG_4] = TNS_LOB_LOC_FLAGS_LITTLE_ENDIAN;
        assert_eq!(
            encode_temp_clob_text("A\u{D55C}", &locator, &caps).unwrap(),
            vec![0x41, 0x00, 0x5c, 0xd5]
        );
    }

    #[test]
    fn decodes_vendor_interval_year_month_bytes() {
        assert_eq!(
            decode_oracle_interval_ym(&[128, 0, 7, 229, 70]).unwrap(),
            "+2021-10"
        );
        assert_eq!(
            decode_oracle_interval_ym(&[127, 255, 255, 251, 57]).unwrap(),
            "-05-03"
        );
    }

    #[test]
    fn decodes_vendor_interval_day_second_bytes() {
        assert_eq!(
            decode_oracle_interval_ds(&[128, 0, 0, 2, 72, 83, 94, 155, 46, 2, 0]).unwrap(),
            "+02 12:23:34.456000"
        );
        assert_eq!(
            decode_oracle_interval_ds(&[128, 0, 0, 0, 50, 40, 30, 100, 197, 243, 248]).unwrap(),
            "-00 10:20:30.456789"
        );
    }

    #[test]
    fn encodes_interval_binds_like_python_oracledb() {
        let ym_pos = BindValue::IntervalYearMonth(OracleIntervalYearMonth {
            years: 2021,
            months: 10,
        });
        let metadata = bind_column_metadata(&ym_pos);
        assert_eq!(metadata.ora_type_num, ORA_TYPE_NUM_INTERVAL_YM);
        assert_eq!(metadata.buffer_size, 5);
        let mut payload = Vec::new();
        write_bind_value(&mut payload, &OracleThinCapabilities::default(), &ym_pos).unwrap();
        assert_eq!(payload, vec![5, 128, 0, 7, 229, 70]);
        assert_eq!(
            decode_oracle_interval_ym(&payload[1..]).unwrap(),
            "+2021-10"
        );

        let ym_neg = BindValue::IntervalYearMonth(OracleIntervalYearMonth {
            years: -5,
            months: -3,
        });
        let mut payload = Vec::new();
        write_bind_value(&mut payload, &OracleThinCapabilities::default(), &ym_neg).unwrap();
        assert_eq!(payload, vec![5, 127, 255, 255, 251, 57]);
        assert_eq!(decode_oracle_interval_ym(&payload[1..]).unwrap(), "-05-03");

        let ds_pos = BindValue::IntervalDaySecond(OracleIntervalDaySecond {
            days: 2,
            hours: 12,
            minutes: 23,
            seconds: 34,
            nanoseconds: 456_000_000,
        });
        let metadata = bind_column_metadata(&ds_pos);
        assert_eq!(metadata.ora_type_num, ORA_TYPE_NUM_INTERVAL_DS);
        assert_eq!(metadata.buffer_size, 11);
        let mut payload = Vec::new();
        write_bind_value(&mut payload, &OracleThinCapabilities::default(), &ds_pos).unwrap();
        assert_eq!(payload, vec![11, 128, 0, 0, 2, 72, 83, 94, 155, 46, 2, 0]);
        assert_eq!(
            decode_oracle_interval_ds(&payload[1..]).unwrap(),
            "+02 12:23:34.456000"
        );

        let ds_neg = BindValue::IntervalDaySecond(OracleIntervalDaySecond {
            days: 0,
            hours: -10,
            minutes: -20,
            seconds: -30,
            nanoseconds: -456_789_000,
        });
        let mut payload = Vec::new();
        write_bind_value(&mut payload, &OracleThinCapabilities::default(), &ds_neg).unwrap();
        assert_eq!(
            payload,
            vec![11, 128, 0, 0, 0, 50, 40, 30, 100, 197, 243, 248]
        );
        assert_eq!(
            decode_oracle_interval_ds(&payload[1..]).unwrap(),
            "-00 10:20:30.456789"
        );
    }

    #[test]
    fn encodes_rowid_binds_like_python_oracledb() {
        let rowid = "AAAWn6AAEAAAAFfAAA".to_string();
        let rowid_bind = BindValue::Rowid(rowid.clone());
        let metadata = bind_column_metadata(&rowid_bind);
        assert_eq!(metadata.ora_type_num, ORA_TYPE_NUM_ROWID);
        assert_eq!(metadata.buffer_size, TNS_MAX_ROWID_LENGTH);
        assert_eq!(metadata.charset_form, CS_FORM_IMPLICIT);
        let mut metadata_payload = Vec::new();
        write_column_metadata(
            &mut metadata_payload,
            &OracleThinCapabilities::default(),
            &metadata,
        )
        .unwrap();
        let mut cursor =
            PacketCursor::with_capabilities(&metadata_payload, &OracleThinCapabilities::default());
        assert_eq!(cursor.read_u8().unwrap(), ORA_TYPE_NUM_VARCHAR);
        assert_eq!(cursor.read_u8().unwrap(), TNS_BIND_USE_INDICATORS);
        assert_eq!(cursor.read_u8().unwrap(), 0);
        assert_eq!(cursor.read_u8().unwrap(), 0);
        assert_eq!(cursor.read_ub4().unwrap(), TNS_MAX_UROWID_LENGTH);
        let mut payload = Vec::new();
        write_bind_value(
            &mut payload,
            &OracleThinCapabilities::default(),
            &rowid_bind,
        )
        .unwrap();
        assert_eq!(payload[0], rowid.len() as u8);
        assert_eq!(&payload[1..], rowid.as_bytes());

        let urowid_bind = BindValue::Urowid(rowid.clone());
        let metadata = bind_column_metadata(&urowid_bind);
        assert_eq!(metadata.ora_type_num, ORA_TYPE_NUM_UROWID);
        assert_eq!(metadata.buffer_size, TNS_MAX_UROWID_LENGTH);
        assert_eq!(metadata.charset_form, CS_FORM_IMPLICIT);
        let mut metadata_payload = Vec::new();
        write_column_metadata(
            &mut metadata_payload,
            &OracleThinCapabilities::default(),
            &metadata,
        )
        .unwrap();
        let mut cursor =
            PacketCursor::with_capabilities(&metadata_payload, &OracleThinCapabilities::default());
        assert_eq!(cursor.read_u8().unwrap(), ORA_TYPE_NUM_VARCHAR);
        assert_eq!(cursor.read_u8().unwrap(), TNS_BIND_USE_INDICATORS);
        assert_eq!(cursor.read_u8().unwrap(), 0);
        assert_eq!(cursor.read_u8().unwrap(), 0);
        assert_eq!(cursor.read_ub4().unwrap(), TNS_MAX_UROWID_LENGTH);
        let mut payload = Vec::new();
        write_bind_value(
            &mut payload,
            &OracleThinCapabilities::default(),
            &urowid_bind,
        )
        .unwrap();
        assert_eq!(payload[0], rowid.len() as u8);
        assert_eq!(&payload[1..], rowid.as_bytes());
    }

    #[test]
    fn encodes_bfile_binds_like_go_ora_locator_shape() {
        let caps = OracleThinCapabilities::default();
        let bind = BindValue::Bfile {
            directory_alias: "dir".to_string(),
            file_name: "file".to_string(),
        };
        let locator = encode_bfile_locator("dir", "file", &caps).unwrap();

        assert_eq!(
            locator,
            vec![
                0, 25, 0, 1, 8, 8, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 3, b'd', b'i', b'r', 0, 4,
                b'f', b'i', b'l', b'e',
            ]
        );
        let metadata = bind_column_metadata(&bind);
        assert_eq!(metadata.column_type, OracleColumnType::Bfile);
        assert_eq!(metadata.ora_type_num, ORA_TYPE_NUM_BFILE);

        let mut expected = Vec::new();
        write_bytes_with_two_lengths(&mut expected, &locator).unwrap();
        let mut payload = Vec::new();
        write_bind_value(&mut payload, &caps, &bind).unwrap();
        assert_eq!(payload, expected);
    }

    #[test]
    fn bfile_lob_locator_bind_metadata_uses_bfile_type() {
        let bind = BindValue::LobLocator {
            column_type: OracleColumnType::Bfile,
            locator: vec![1, 2, 3],
        };
        let metadata = bind_column_metadata(&bind);

        assert_eq!(metadata.column_type, OracleColumnType::Bfile);
        assert_eq!(metadata.ora_type_num, ORA_TYPE_NUM_BFILE);
        assert_eq!(metadata.buffer_size, 3);
    }

    #[test]
    fn boolean_null_binds_use_vendor_escape_marker() {
        let caps = OracleThinCapabilities::default();
        for bind in [
            BindValue::Null(OracleColumnType::Boolean),
            BindValue::Out {
                column_type: OracleColumnType::Boolean,
                max_len: 4,
            },
            BindValue::InOut {
                column_type: OracleColumnType::Boolean,
                max_len: 4,
                value: None,
            },
        ] {
            let mut out = Vec::new();
            write_bind_value(&mut out, &caps, &bind).unwrap();
            assert_eq!(out, vec![TNS_ESCAPE_CHAR, 1]);
        }
    }

    #[test]
    fn ref_cursor_out_bind_rows_use_vendor_empty_cursor_handle() {
        let mut request = StatementRequest::statement("BEGIN p(:1); END;");
        request.binds.push(BindValue::Out {
            column_type: OracleColumnType::Cursor,
            max_len: 4,
        });

        let mut payload = Vec::new();
        write_bind_rows_for_request(&mut payload, &OracleThinCapabilities::default(), &request)
            .expect("write REF CURSOR OUT bind rows");

        assert_eq!(payload, vec![TNS_MSG_TYPE_ROW_DATA, 1, 0]);
    }

    #[test]
    fn ref_cursor_out_bind_metadata_uses_vendor_cursor_shape() {
        let column = bind_column_metadata(&BindValue::Out {
            column_type: OracleColumnType::Cursor,
            max_len: 1,
        });

        assert_eq!(column.ora_type_num, ORA_TYPE_NUM_CURSOR);
        assert_eq!(column.buffer_size, 4);
        assert_eq!(column.charset_form, 0);
    }

    #[test]
    fn out_bind_type_helper_builds_out_binds_when_request_has_none() {
        let request = StatementRequest::statement("BEGIN p(:1, :2); END;");
        let request = request_with_out_bind_types(
            &request,
            &[OracleColumnType::Varchar, OracleColumnType::Cursor],
        );

        assert_eq!(
            request.binds,
            vec![
                BindValue::Out {
                    column_type: OracleColumnType::Varchar,
                    max_len: 4000,
                },
                BindValue::Out {
                    column_type: OracleColumnType::Cursor,
                    max_len: 4,
                },
            ]
        );
    }

    #[test]
    fn typed_value_fetch_define_requirements_match_vendor_no_prefetch_types() {
        assert!(column_types_require_define_fetch_for_values(&[
            OracleColumnType::Varchar,
            OracleColumnType::Clob,
        ]));
        assert!(column_types_require_define_fetch_for_values(&[
            OracleColumnType::Blob,
            OracleColumnType::Json,
            OracleColumnType::Vector,
        ]));
        assert!(!column_types_require_define_fetch_for_values(&[
            OracleColumnType::Varchar,
            OracleColumnType::Number,
            OracleColumnType::Timestamp,
            OracleColumnType::Bfile,
            OracleColumnType::Cursor,
        ]));
    }

    #[test]
    fn adjust_columns_after_define_matches_vendor_lob_redescribe_rules() {
        let previous_columns = vec![
            ThinColumn {
                name: "C_CHAR".to_string(),
                column_type: OracleColumnType::Varchar,
                ora_type_num: ORA_TYPE_NUM_CHAR,
                charset_form: CS_FORM_NCHAR,
                buffer_size: 10,
                schema_name: String::new(),
                type_name: String::new(),
            },
            ThinColumn {
                name: "C_VARCHAR".to_string(),
                column_type: OracleColumnType::Varchar,
                ora_type_num: ORA_TYPE_NUM_VARCHAR,
                charset_form: CS_FORM_IMPLICIT,
                buffer_size: 10,
                schema_name: String::new(),
                type_name: String::new(),
            },
            ThinColumn {
                name: "B_RAW".to_string(),
                column_type: OracleColumnType::Raw,
                ora_type_num: ORA_TYPE_NUM_RAW,
                charset_form: 0,
                buffer_size: 10,
                schema_name: String::new(),
                type_name: String::new(),
            },
            ThinColumn {
                name: "B_LONG_RAW".to_string(),
                column_type: OracleColumnType::Raw,
                ora_type_num: ORA_TYPE_NUM_LONG_RAW,
                charset_form: 0,
                buffer_size: 10,
                schema_name: String::new(),
                type_name: String::new(),
            },
        ];
        let mut columns = vec![
            ThinColumn {
                name: "C_CHAR".to_string(),
                column_type: OracleColumnType::Clob,
                ora_type_num: ORA_TYPE_NUM_CLOB,
                charset_form: CS_FORM_IMPLICIT,
                buffer_size: 112,
                schema_name: String::new(),
                type_name: String::new(),
            },
            ThinColumn {
                name: "C_VARCHAR".to_string(),
                column_type: OracleColumnType::Clob,
                ora_type_num: ORA_TYPE_NUM_CLOB,
                charset_form: CS_FORM_NCHAR,
                buffer_size: 112,
                schema_name: String::new(),
                type_name: String::new(),
            },
            ThinColumn {
                name: "B_RAW".to_string(),
                column_type: OracleColumnType::Blob,
                ora_type_num: ORA_TYPE_NUM_BLOB,
                charset_form: 0,
                buffer_size: 112,
                schema_name: String::new(),
                type_name: String::new(),
            },
            ThinColumn {
                name: "B_LONG_RAW".to_string(),
                column_type: OracleColumnType::Blob,
                ora_type_num: ORA_TYPE_NUM_BLOB,
                charset_form: 0,
                buffer_size: 112,
                schema_name: String::new(),
                type_name: String::new(),
            },
        ];

        adjust_columns_after_define(&previous_columns, &mut columns);

        assert_eq!(
            columns
                .iter()
                .map(|column| (column.ora_type_num, column.column_type, column.charset_form))
                .collect::<Vec<_>>(),
            vec![
                (ORA_TYPE_NUM_LONG, OracleColumnType::Long, CS_FORM_NCHAR),
                (ORA_TYPE_NUM_LONG, OracleColumnType::Long, CS_FORM_IMPLICIT),
                (ORA_TYPE_NUM_LONG_RAW, OracleColumnType::Raw, 0),
                (ORA_TYPE_NUM_LONG_RAW, OracleColumnType::Raw, 0),
            ]
        );
    }

    #[test]
    fn execute_flags_follow_implicit_resultset_request() {
        let mut request = StatementRequest::statement("BEGIN NULL; END;");
        request.implicit_resultsets = false;

        assert_eq!(execute_flags_for_request(false, &request), 0);

        request.implicit_resultsets = true;
        assert_eq!(
            execute_flags_for_request(false, &request),
            TNS_EXEC_FLAGS_IMPLICIT_RESULTSET
        );
        assert_eq!(execute_flags_for_request(true, &request), 0);

        request.binds.push(BindValue::Out {
            column_type: OracleColumnType::Varchar,
            max_len: 10,
        });
        assert_eq!(execute_flags_for_request(false, &request), 0);

        request.sql =
            "DECLARE rc SYS_REFCURSOR; BEGIN DBMS_SQL.RETURN_RESULT(rc); END;".to_string();
        assert_eq!(
            execute_flags_for_request(false, &request),
            TNS_EXEC_FLAGS_IMPLICIT_RESULTSET
        );
    }

    #[test]
    fn detects_dml_returning_without_matching_json_returning() {
        let returning = StatementRequest::statement(
            "INSERT INTO t(id, name) VALUES (:1, :2) RETURNING id INTO :3",
        );
        assert!(request_is_dml_returning(&returning));

        let json_returning = StatementRequest::statement(
            "INSERT INTO t(doc) SELECT JSON_OBJECT(KEY 'id' VALUE :1 RETURNING CLOB) FROM dual",
        );
        assert!(!request_is_dml_returning(&json_returning));

        let commented = StatementRequest::statement(
            "UPDATE t SET note = 'RETURNING id INTO :x' /* RETURNING y INTO :z */ WHERE id = :1",
        );
        assert!(!request_is_dml_returning(&commented));

        let q_quoted = StatementRequest::statement(
            "UPDATE t SET note = q'[RETURNING id INTO :x]' WHERE id = :1",
        );
        assert!(!request_is_dml_returning(&q_quoted));

        let nq_quoted = StatementRequest::statement(
            "UPDATE t SET note = nq'{RETURNING id INTO :x}' WHERE id = :1",
        );
        assert!(!request_is_dml_returning(&nq_quoted));

        let returning_after_q_quote = StatementRequest::statement(
            "UPDATE t SET note = q'[RETURNING id INTO :x]' WHERE id = :1 RETURNING id INTO :2",
        );
        assert!(request_is_dml_returning(&returning_after_q_quote));

        let no_space_returning =
            StatementRequest::statement("INSERT INTO t(id) VALUES (:1)returning(id)into :2");
        assert!(request_is_dml_returning(&no_space_returning));
    }

    #[test]
    fn dml_returning_bind_rows_skip_output_capable_return_binds() {
        let mut request =
            StatementRequest::statement("INSERT INTO t(id) VALUES (:1) RETURNING id INTO :2");
        request.binds.push(BindValue::Number("1".to_string()));
        request.binds.push(BindValue::InOut {
            column_type: OracleColumnType::Number,
            max_len: 22,
            value: Some(BindInputValue::Number("999".to_string())),
        });

        let mut payload = Vec::new();
        write_bind_rows_for_request(&mut payload, &OracleThinCapabilities::default(), &request)
            .expect("write DML RETURNING bind rows");

        assert_eq!(hex_encode_upper(&payload), "0702C102");
    }

    #[test]
    fn non_plsql_bind_rows_write_long_values_after_non_long_values() {
        let caps = OracleThinCapabilities::default();
        let large = BindValue::Text("x".repeat(1001));
        let small = BindValue::Text("S".to_string());
        let mut request = StatementRequest::query("SELECT :1, :2 FROM dual", 1);
        request.binds.push(large.clone());
        request.binds.push(small.clone());

        let mut expected = vec![TNS_MSG_TYPE_ROW_DATA];
        write_bind_value(&mut expected, &caps, &small).expect("write small bind");
        write_bind_value(&mut expected, &caps, &large).expect("write large bind");

        let mut payload = Vec::new();
        write_bind_rows_for_request(&mut payload, &caps, &request)
            .expect("write non-PL/SQL bind rows");

        assert_eq!(payload, expected);
    }

    #[test]
    fn non_plsql_bind_rows_keep_value_based_lob_payloads_with_their_locators() {
        let caps = OracleThinCapabilities::default();
        let json = BindValue::Json(r#"{"a":1}"#.to_string());
        let small = BindValue::Number("2".to_string());
        let vector = BindValue::Vector(OracleVectorValue::Float32(vec![1.0, 2.0]));
        let mut request = StatementRequest::statement("UPDATE t SET j = :1, v = :3 WHERE id = :2");
        request.binds.push(json.clone());
        request.binds.push(small.clone());
        request.binds.push(vector.clone());

        let mut expected = vec![TNS_MSG_TYPE_ROW_DATA];
        write_bind_value(&mut expected, &caps, &json).expect("write JSON bind");
        write_bind_value(&mut expected, &caps, &small).expect("write small bind");
        write_bind_value(&mut expected, &caps, &vector).expect("write VECTOR bind");

        let mut payload = Vec::new();
        write_bind_rows_for_request(&mut payload, &caps, &request)
            .expect("write non-PL/SQL value-based LOB bind rows");

        assert_eq!(payload, expected);
    }

    #[test]
    fn plsql_bind_rows_keep_declared_order_for_long_values() {
        let caps = OracleThinCapabilities::default();
        let large = BindValue::Text("x".repeat(1001));
        let small = BindValue::Text("S".to_string());
        let mut request = StatementRequest::statement("BEGIN p(:1, :2); END;");
        request.binds.push(large.clone());
        request.binds.push(small.clone());

        let mut expected = vec![TNS_MSG_TYPE_ROW_DATA];
        write_bind_value(&mut expected, &caps, &large).expect("write large bind");
        write_bind_value(&mut expected, &caps, &small).expect("write small bind");

        let mut payload = Vec::new();
        write_bind_rows_for_request(&mut payload, &caps, &request).expect("write PL/SQL bind rows");

        assert_eq!(payload, expected);
    }

    #[test]
    fn boolean_reader_decodes_vendor_escape_null_and_values() {
        let caps = OracleThinCapabilities::default();
        let mut null_cursor = PacketCursor::with_capabilities(&[TNS_ESCAPE_CHAR, 1], &caps);
        assert_eq!(
            read_boolean_value(&mut null_cursor).unwrap(),
            OracleValue::Null
        );

        let mut true_cursor = PacketCursor::with_capabilities(&[2, 1, 1], &caps);
        assert_eq!(
            read_boolean_value(&mut true_cursor).unwrap(),
            OracleValue::Boolean(true)
        );

        let mut false_cursor = PacketCursor::with_capabilities(&[1, 0], &caps);
        assert_eq!(
            read_boolean_value(&mut false_cursor).unwrap(),
            OracleValue::Boolean(false)
        );
    }

    #[test]
    fn timestamptz_binds_preserve_fixed_offset_metadata_and_payload() {
        let value = crate::OracleDateTime {
            year: 2024,
            month: 1,
            day: 2,
            hour: 3,
            minute: 4,
            second: 5,
            nanosecond: 123_456_000,
            timezone_offset_minutes: Some(345),
            timezone_region_id: None,
        };
        let bind = BindValue::Timestamp(value);
        let column = bind_column_metadata(&bind);

        assert_eq!(column.ora_type_num, ORA_TYPE_NUM_TIMESTAMP_TZ);
        assert_eq!(column.buffer_size, 13);
        assert_eq!(
            encode_oracle_timestamp_bind(&value),
            vec![120, 124, 1, 1, 22, 20, 6, 7, 91, 202, 0, 25, 105,]
        );
    }

    #[test]
    fn timestamptz_binds_preserve_region_metadata_and_payload() {
        let value = crate::OracleDateTime {
            year: 2024,
            month: 1,
            day: 2,
            hour: 3,
            minute: 4,
            second: 5,
            nanosecond: 123_456_000,
            timezone_offset_minutes: None,
            timezone_region_id: Some(273),
        };
        let bind = BindValue::Timestamp(value);
        let column = bind_column_metadata(&bind);

        assert_eq!(column.ora_type_num, ORA_TYPE_NUM_TIMESTAMP_TZ);
        assert_eq!(column.buffer_size, 13);
        assert_eq!(
            encode_oracle_timestamp_bind(&value),
            vec![120, 124, 1, 1, 19, 5, 6, 7, 91, 202, 0, 0x84, 0x44,]
        );
    }

    #[test]
    fn nclob_inout_text_binds_encode_as_utf16be_nchar_bytes() {
        let caps = OracleThinCapabilities::default();
        let bind = BindValue::InOut {
            column_type: OracleColumnType::Nclob,
            max_len: 20,
            value: Some(BindInputValue::Text("\u{D55C}".to_string())),
        };
        let mut out = Vec::new();

        write_bind_value(&mut out, &caps, &bind).unwrap();

        assert_eq!(out, vec![2, 0xd5, 0x5c]);
    }

    #[test]
    fn nclob_inout_text_binds_keep_utf8_when_nchar_charset_is_utf8() {
        let mut caps = OracleThinCapabilities::default();
        caps.ncharset_id = ORACLE_CHARSET_AL32UTF8;
        let bind = BindValue::InOut {
            column_type: OracleColumnType::Nclob,
            max_len: 20,
            value: Some(BindInputValue::Text("\u{D55C}".to_string())),
        };
        let mut out = Vec::new();

        write_bind_value(&mut out, &caps, &bind).unwrap();

        let mut expected = vec![3];
        expected.extend_from_slice("\u{D55C}".as_bytes());
        assert_eq!(out, expected);
    }

    #[test]
    fn decodes_korean_text_from_negotiated_utf8_and_nchar_utf16() {
        let caps = OracleThinCapabilities::default();

        assert_eq!(
            decode_oracle_text("한글".as_bytes(), CS_FORM_IMPLICIT, &caps).unwrap(),
            "한글"
        );
        assert_eq!(
            decode_oracle_text(&[0xd5, 0x5c, 0xae, 0x00], CS_FORM_NCHAR, &caps).unwrap(),
            "한글"
        );
    }

    #[test]
    fn native_ko16_bytes_remain_invalid_without_server_charset() {
        let caps = OracleThinCapabilities::default();
        let err = decode_oracle_text(&[0xc7, 0xd1, 0xb1, 0xdb], CS_FORM_IMPLICIT, &caps)
            .expect_err("native KO16 bytes should not decode without a matching server charset");

        assert!(err.to_string().contains("invalid UTF-8"));
    }

    #[test]
    fn native_charset_windows_code_page_mapping_covers_oracle_candidates() {
        assert_eq!(
            windows_code_pages_for_encoding("EUC-KR"),
            Some(&[51949, 949][..])
        );
        assert_eq!(
            windows_code_pages_for_encoding("MS949"),
            Some(&[949, 51949][..])
        );
        assert_eq!(windows_code_pages_for_encoding("CP932"), Some(&[932][..]));
        assert_eq!(windows_code_pages_for_encoding("GBK"), Some(&[936][..]));
        assert_eq!(windows_code_pages_for_encoding("BIG5"), Some(&[950][..]));
        assert_eq!(
            windows_code_pages_for_encoding("WINDOWS-1252"),
            Some(&[1252][..])
        );
        assert_eq!(
            windows_code_pages_for_encoding("UTF-16BE"),
            Some(&[1201][..])
        );
        assert_eq!(windows_code_pages_for_encoding("unknown"), None);
    }

    #[cfg(unix)]
    #[test]
    fn decodes_korean_native_charset_bytes_with_negotiated_server_charset() {
        let mut ksc_caps = OracleThinCapabilities::default();
        ksc_caps.protocol_version = Some(314);
        ksc_caps.charset_id = ORACLE_CHARSET_KO16KSC5601;
        assert_eq!(
            decode_oracle_text("한글".as_bytes(), CS_FORM_IMPLICIT, &ksc_caps).unwrap(),
            "한글"
        );
        assert_eq!(
            decode_oracle_text(&[0xc7, 0xd1, 0xb1, 0xdb], CS_FORM_IMPLICIT, &ksc_caps).unwrap(),
            "한글"
        );

        let mut mswin_caps = OracleThinCapabilities::default();
        mswin_caps.protocol_version = Some(314);
        mswin_caps.charset_id = ORACLE_CHARSET_KO16MSWIN949;
        assert_eq!(
            decode_oracle_text("한글".as_bytes(), CS_FORM_IMPLICIT, &mswin_caps).unwrap(),
            "한글"
        );
        assert_eq!(
            decode_oracle_text(&[0xc7, 0xd1, 0xb1, 0xdb], CS_FORM_IMPLICIT, &mswin_caps).unwrap(),
            "한글"
        );
        assert_eq!(
            decode_oracle_text(&[0xc6, 0x52], CS_FORM_IMPLICIT, &mswin_caps).unwrap(),
            "힣"
        );
    }

    #[cfg(unix)]
    #[test]
    fn encodes_korean_varchar_binds_as_utf8_even_with_native_server_charset() {
        let mut ksc_caps = OracleThinCapabilities::default();
        ksc_caps.charset_id = ORACLE_CHARSET_KO16KSC5601;
        assert_eq!(
            encode_oracle_bind_text("한글", CS_FORM_IMPLICIT, &ksc_caps).unwrap(),
            "한글".as_bytes()
        );

        let mut mswin_caps = OracleThinCapabilities::default();
        mswin_caps.charset_id = ORACLE_CHARSET_KO16MSWIN949;
        assert_eq!(
            encode_oracle_bind_text("힣", CS_FORM_IMPLICIT, &mswin_caps).unwrap(),
            "힣".as_bytes()
        );
    }

    #[cfg(unix)]
    #[test]
    fn decodes_vendor_native_charset_table_bytes_with_negotiated_server_charset() {
        let mut cp1252_caps = OracleThinCapabilities::default();
        cp1252_caps.charset_id = 178;
        assert_eq!(
            decode_oracle_text(&[0xc3, 0xa9], CS_FORM_IMPLICIT, &cp1252_caps).unwrap(),
            "é"
        );
        assert_eq!(
            decode_oracle_text(&[0x93, b'H', b'i', 0x94], CS_FORM_IMPLICIT, &cp1252_caps).unwrap(),
            "“Hi”"
        );

        let mut sjis_caps = OracleThinCapabilities::default();
        sjis_caps.charset_id = ORACLE_CHARSET_JA16SJIS;
        assert_eq!(
            decode_oracle_text(&[0x93, 0xfa, 0x96, 0x7b], CS_FORM_IMPLICIT, &sjis_caps).unwrap(),
            "日本"
        );

        let mut gbk_caps = OracleThinCapabilities::default();
        gbk_caps.protocol_version = Some(314);
        gbk_caps.charset_id = ORACLE_CHARSET_ZHS16GBK;
        assert_eq!(
            decode_oracle_text(&[0xba, 0xba, 0xd7, 0xd6], CS_FORM_IMPLICIT, &gbk_caps).unwrap(),
            "汉字"
        );

        let mut big5_caps = OracleThinCapabilities::default();
        big5_caps.protocol_version = Some(314);
        big5_caps.charset_id = ORACLE_CHARSET_ZHT16BIG5;
        assert_eq!(
            decode_oracle_text(&[0xa4, 0xa4, 0xa4, 0xe5], CS_FORM_IMPLICIT, &big5_caps).unwrap(),
            "中文"
        );
    }

    #[cfg(unix)]
    #[test]
    fn native_charset_mapping_uses_go_ora_for_314_and_python_oracledb_for_modern_protocols() {
        let mut legacy_euc_jp_caps = OracleThinCapabilities::default();
        legacy_euc_jp_caps.protocol_version = Some(314);
        legacy_euc_jp_caps.charset_id = 830;
        assert_eq!(
            decode_oracle_text(
                &[0xc6, 0xfc, 0xcb, 0xdc],
                CS_FORM_IMPLICIT,
                &legacy_euc_jp_caps
            )
            .unwrap(),
            "日本"
        );

        let mut modern_euc_kr_caps = OracleThinCapabilities::default();
        modern_euc_kr_caps.protocol_version = Some(315);
        modern_euc_kr_caps.charset_id = 830;
        assert_eq!(
            decode_oracle_text(
                &[0xc7, 0xd1, 0xb1, 0xdb],
                CS_FORM_IMPLICIT,
                &modern_euc_kr_caps
            )
            .unwrap(),
            "한글"
        );

        let mut modern_gbk_caps = OracleThinCapabilities::default();
        modern_gbk_caps.protocol_version = Some(318);
        modern_gbk_caps.charset_id = 846;
        assert_eq!(
            decode_oracle_text(
                &[0xba, 0xba, 0xd7, 0xd6],
                CS_FORM_IMPLICIT,
                &modern_gbk_caps
            )
            .unwrap(),
            "汉字"
        );

        let mut modern_cp949_caps = OracleThinCapabilities::default();
        modern_cp949_caps.protocol_version = Some(319);
        modern_cp949_caps.charset_id = 852;
        assert_eq!(
            decode_oracle_text(
                &[0xc7, 0xd1, 0xb1, 0xdb],
                CS_FORM_IMPLICIT,
                &modern_cp949_caps
            )
            .unwrap(),
            "한글"
        );

        let mut modern_big5_caps = OracleThinCapabilities::default();
        modern_big5_caps.protocol_version = Some(315);
        modern_big5_caps.charset_id = 829;
        assert_eq!(
            decode_oracle_text(
                &[0xa4, 0xa4, 0xa4, 0xe5],
                CS_FORM_IMPLICIT,
                &modern_big5_caps
            )
            .unwrap(),
            "中文"
        );

        let mut modern_gb18030_caps = OracleThinCapabilities::default();
        modern_gb18030_caps.protocol_version = Some(315);
        modern_gb18030_caps.charset_id = 870;
        assert_eq!(
            decode_oracle_text(
                &[0xba, 0xba, 0xd7, 0xd6],
                CS_FORM_IMPLICIT,
                &modern_gb18030_caps
            )
            .unwrap(),
            "汉字"
        );
    }

    #[cfg(unix)]
    #[test]
    fn nchar_native_charset_mapping_follows_modern_python_oracledb_table() {
        let mut caps = OracleThinCapabilities::default();
        caps.protocol_version = Some(315);
        caps.ncharset_id = 830;

        assert_eq!(
            decode_oracle_text(&[0xc7, 0xd1, 0xb1, 0xdb], CS_FORM_NCHAR, &caps).unwrap(),
            "한글"
        );
        assert_eq!(
            encode_oracle_bind_text("한글", CS_FORM_NCHAR, &caps).unwrap(),
            vec![0xc7, 0xd1, 0xb1, 0xdb]
        );
    }

    #[test]
    fn decodes_utf8_nchar_when_server_reports_utf8_national_charset() {
        let mut utf8_caps = OracleThinCapabilities::default();
        utf8_caps.ncharset_id = ORACLE_CHARSET_UTF8;
        assert_eq!(
            decode_oracle_text("한".as_bytes(), CS_FORM_NCHAR, &utf8_caps).unwrap(),
            "한"
        );

        utf8_caps.ncharset_id = ORACLE_CHARSET_AL32UTF8;
        assert_eq!(
            decode_oracle_text("글".as_bytes(), CS_FORM_NCHAR, &utf8_caps).unwrap(),
            "글"
        );
    }

    #[test]
    fn rejects_oracle_datetime_years_outside_supported_range() {
        for bytes in [
            [99, 99, 1, 1, 1, 1, 1],
            [100, 100, 1, 1, 1, 1, 1],
            [200, 100, 1, 1, 1, 1, 1],
        ] {
            let err = decode_oracle_datetime(&bytes).expect_err("out-of-range Oracle year");
            assert!(
                err.to_string().contains("outside supported range 1..=9999"),
                "unexpected Oracle datetime error: {err}"
            );
        }
    }

    #[test]
    fn decodes_timestamp_tz_fixed_offset_bytes() {
        let value =
            decode_oracle_datetime(&[120, 124, 1, 1, 22, 20, 6, 7, 91, 202, 0, 25, 105]).unwrap();

        assert_eq!(
            value,
            crate::OracleDateTime {
                year: 2024,
                month: 1,
                day: 2,
                hour: 3,
                minute: 4,
                second: 5,
                nanosecond: 123_456_000,
                timezone_offset_minutes: Some(345),
                timezone_region_id: None,
            }
        );
    }

    #[test]
    fn decodes_named_timestamp_tz_region_id() {
        let value =
            decode_oracle_datetime(&[120, 124, 1, 2, 4, 5, 6, 7, 91, 202, 0, 0x84, 0x45]).unwrap();

        assert_eq!(
            value,
            crate::OracleDateTime {
                year: 2024,
                month: 1,
                day: 2,
                hour: 3,
                minute: 4,
                second: 5,
                nanosecond: 123_456_000,
                timezone_offset_minutes: None,
                timezone_region_id: Some(273),
            }
        );
        assert_eq!(value.timezone_suffix().unwrap(), " Asia/Seoul");
    }

    #[test]
    fn row_scanner_decodes_interval_day_second_columns() {
        let mut state = ExecuteReadState::default();
        state.columns = vec![ThinColumn {
            name: "DURATION".to_string(),
            column_type: OracleColumnType::IntervalDaySecond,
            ora_type_num: ORA_TYPE_NUM_INTERVAL_DS,
            charset_form: 0,
            buffer_size: 11,
            schema_name: String::new(),
            type_name: String::new(),
        }];
        let mut row = vec![11];
        row.extend_from_slice(&[128, 0, 0, 2, 72, 83, 94, 155, 46, 2, 0]);
        let mut cursor = PacketCursor::with_capabilities(&row, &OracleThinCapabilities::default());

        process_row_data(&mut cursor, &OracleThinCapabilities::default(), &mut state).unwrap();

        assert_eq!(
            state.rows,
            vec![vec![OracleValue::Text("+02 12:23:34.456000".to_string())]]
        );
    }

    #[test]
    fn row_scanner_decodes_binary_float_columns_as_numbers() {
        let mut state = ExecuteReadState::default();
        state.columns = vec![ThinColumn {
            name: "VALUE".to_string(),
            column_type: OracleColumnType::BinaryFloat,
            ora_type_num: ORA_TYPE_NUM_BINARY_FLOAT,
            charset_form: 0,
            buffer_size: 4,
            schema_name: String::new(),
            type_name: String::new(),
        }];
        let mut row = vec![4];
        row.extend_from_slice(&[195, 6, 115, 51]);
        let mut cursor = PacketCursor::with_capabilities(&row, &OracleThinCapabilities::default());

        process_row_data(&mut cursor, &OracleThinCapabilities::default(), &mut state).unwrap();

        assert_eq!(
            state.rows,
            vec![vec![OracleValue::Number("134.45".to_string())]]
        );
    }

    #[test]
    fn row_scanner_decodes_go_ora_binary_float_alias_columns_as_numbers() {
        let mut state = ExecuteReadState::default();
        state.columns = vec![
            ThinColumn {
                name: "BF".to_string(),
                column_type: OracleColumnType::BinaryFloat,
                ora_type_num: TNS_DATA_TYPE_BFLOAT,
                charset_form: 0,
                buffer_size: 4,
                schema_name: String::new(),
                type_name: String::new(),
            },
            ThinColumn {
                name: "BD".to_string(),
                column_type: OracleColumnType::BinaryDouble,
                ora_type_num: TNS_DATA_TYPE_BDOUBLE,
                charset_form: 0,
                buffer_size: 8,
                schema_name: String::new(),
                type_name: String::new(),
            },
        ];
        let mut row = vec![4];
        row.extend_from_slice(&[195, 6, 115, 51]);
        row.push(8);
        row.extend_from_slice(&[192, 96, 206, 102, 102, 102, 102, 102]);
        let mut cursor = PacketCursor::with_capabilities(&row, &OracleThinCapabilities::default());

        process_row_data(&mut cursor, &OracleThinCapabilities::default(), &mut state).unwrap();

        assert_eq!(
            state.rows,
            vec![vec![
                OracleValue::Number("134.45".to_string()),
                OracleValue::Number("134.45".to_string())
            ]]
        );
    }

    #[test]
    fn metadata_maps_vendor_negotiated_alias_types_to_decoders() {
        assert_eq!(
            oracle_column_type_from_ora_type(TNS_DATA_TYPE_DTR),
            OracleColumnType::Number
        );
        assert_eq!(
            oracle_column_type_from_ora_type(ORA_TYPE_NUM_VARCHAR),
            OracleColumnType::Varchar
        );
        assert_eq!(
            oracle_column_type_from_ora_type(TNS_DATA_TYPE_VCS),
            OracleColumnType::Varchar
        );
        assert_eq!(
            oracle_column_type_from_ora_type(TNS_DATA_TYPE_LVB),
            OracleColumnType::Raw
        );
        assert_eq!(
            oracle_column_type_from_ora_type(TNS_DATA_TYPE_ODT),
            OracleColumnType::Date
        );
        assert_eq!(
            oracle_column_type_from_ora_type(TNS_DATA_TYPE_DCLOB),
            OracleColumnType::Clob
        );
        assert_eq!(
            oracle_column_type_from_ora_type(TNS_DATA_TYPE_RSET),
            OracleColumnType::Cursor
        );
        assert_eq!(
            oracle_column_type_from_ora_type(ORA_TYPE_NUM_ROWID),
            OracleColumnType::Rowid
        );
        assert_eq!(
            oracle_column_type_from_ora_type(TNS_DATA_TYPE_RDD),
            OracleColumnType::Rowid
        );
        assert_eq!(
            oracle_column_type_from_ora_type(ORA_TYPE_NUM_UROWID),
            OracleColumnType::Urowid
        );
        assert_eq!(
            oracle_column_type_from_ora_type(ORA_TYPE_NUM_BINARY_FLOAT),
            OracleColumnType::BinaryFloat
        );
        assert_eq!(
            oracle_column_type_from_ora_type(TNS_DATA_TYPE_BFLOAT),
            OracleColumnType::BinaryFloat
        );
        assert_eq!(
            oracle_column_type_from_ora_type(ORA_TYPE_NUM_BINARY_DOUBLE),
            OracleColumnType::BinaryDouble
        );
        assert_eq!(
            oracle_column_type_from_ora_type(TNS_DATA_TYPE_BDOUBLE),
            OracleColumnType::BinaryDouble
        );
        assert_eq!(
            oracle_column_type_from_ora_type(TNS_DATA_TYPE_CFILE),
            OracleColumnType::Bfile
        );
        assert_eq!(
            oracle_column_type_from_ora_type(TNS_DATA_TYPE_CLV),
            OracleColumnType::Varchar
        );
        assert_eq!(
            oracle_column_type_from_ora_type(TNS_DATA_TYPE_TIME),
            OracleColumnType::Varchar
        );
        assert_eq!(
            oracle_column_type_from_ora_type(TNS_DATA_TYPE_TIME_TZ),
            OracleColumnType::Varchar
        );
        assert_eq!(
            oracle_column_type_from_ora_type(TNS_DATA_TYPE_UB8),
            OracleColumnType::Number
        );
        assert_eq!(
            oracle_column_type_from_ora_type(ORA_TYPE_NUM_OBJECT),
            OracleColumnType::Object
        );
        assert_eq!(
            oracle_column_type_from_ora_type(TNS_DATA_TYPE_EXT_NAMED),
            OracleColumnType::Object
        );
        assert_eq!(
            oracle_column_type_from_ora_type(TNS_DATA_TYPE_PNTY),
            OracleColumnType::Object
        );
        assert_eq!(
            oracle_column_type_from_ora_type(TNS_DATA_TYPE_EXT_REF),
            OracleColumnType::ObjectRef
        );
        assert_eq!(
            oracle_column_type_from_ora_type(TNS_DATA_TYPE_INT_REF),
            OracleColumnType::ObjectRef
        );
        assert_eq!(
            oracle_column_type_from_ora_type(250),
            OracleColumnType::Unsupported(250)
        );
    }

    #[test]
    fn object_attr_number_synonyms_map_to_number_like_vendor() {
        for attr_type in [
            "DECIMAL",
            "DEC",
            "NUMERIC",
            "INTEGER",
            "INT",
            "SMALLINT",
            "FLOAT",
            "REAL",
            "DOUBLE PRECISION",
        ] {
            let column = thin_column_from_object_attr(
                "VALUE".to_string(),
                attr_type.to_string(),
                String::new(),
                None,
                0,
                0,
            )
            .unwrap();

            assert_eq!(column.column_type, OracleColumnType::Number, "{attr_type}");
            assert_eq!(column.ora_type_num, ORA_TYPE_NUM_NUMBER, "{attr_type}");
            assert_eq!(column.buffer_size, 22, "{attr_type}");
        }
    }

    #[test]
    fn object_attr_plsql_integer_aliases_map_to_binary_integer_like_python_oracledb() {
        for attr_type in ["PL/SQL PLS INTEGER", "PL/SQL BINARY INTEGER"] {
            let column = thin_column_from_object_attr(
                "VALUE".to_string(),
                attr_type.to_string(),
                String::new(),
                None,
                0,
                0,
            )
            .unwrap();

            assert_eq!(column.column_type, OracleColumnType::Number, "{attr_type}");
            assert_eq!(
                column.ora_type_num, TNS_DATA_TYPE_BINARY_INTEGER,
                "{attr_type}"
            );
            assert_eq!(column.buffer_size, 4, "{attr_type}");
        }
    }

    #[test]
    fn object_attr_plsql_boolean_alias_maps_to_boolean_like_python_oracledb() {
        let column = thin_column_from_object_attr(
            "VALUE".to_string(),
            "PL/SQL BOOLEAN".to_string(),
            String::new(),
            None,
            0,
            0,
        )
        .unwrap();

        assert_eq!(column.column_type, OracleColumnType::Boolean);
        assert_eq!(column.ora_type_num, ORA_TYPE_NUM_BOOLEAN);
        assert_eq!(column.buffer_size, 4);
    }

    #[test]
    fn object_attr_long_types_map_like_python_oracledb() {
        for (attr_type, charset_form) in [
            ("LONG", CS_FORM_IMPLICIT),
            ("LONG VARCHAR", CS_FORM_IMPLICIT),
            ("LONG NVARCHAR", CS_FORM_NCHAR),
        ] {
            let column = thin_column_from_object_attr(
                "VALUE".to_string(),
                attr_type.to_string(),
                String::new(),
                None,
                0,
                charset_form,
            )
            .unwrap();

            assert_eq!(column.column_type, OracleColumnType::Long, "{attr_type}");
            assert_eq!(column.ora_type_num, ORA_TYPE_NUM_LONG, "{attr_type}");
            assert_eq!(column.buffer_size, TNS_MAX_LONG_LENGTH, "{attr_type}");
            assert_eq!(column.charset_form, charset_form, "{attr_type}");
        }

        let column = thin_column_from_object_attr(
            "VALUE".to_string(),
            "LONG RAW".to_string(),
            String::new(),
            None,
            0,
            0,
        )
        .unwrap();

        assert_eq!(column.column_type, OracleColumnType::Raw);
        assert_eq!(column.ora_type_num, ORA_TYPE_NUM_LONG_RAW);
        assert_eq!(column.buffer_size, TNS_MAX_LONG_LENGTH);
    }

    #[test]
    fn object_attr_named_type_takes_precedence_over_builtin_name_collision() {
        let column = thin_column_from_object_attr(
            "PAYLOAD".to_string(),
            "number".to_string(),
            "SYSTEM".to_string(),
            Some("OBJECT".to_string()),
            0,
            0,
        )
        .unwrap();

        assert_eq!(column.column_type, OracleColumnType::Object);
        assert_eq!(column.ora_type_num, ORA_TYPE_NUM_OBJECT);
        assert_eq!(column.schema_name, "SYSTEM");
        assert_eq!(column.type_name, "number");
        assert_eq!(column.buffer_size, 1);
    }

    #[test]
    fn object_attr_named_collection_takes_precedence_over_builtin_name_collision() {
        let column = thin_column_from_object_attr(
            "ITEMS".to_string(),
            "varchar2".to_string(),
            "SYSTEM".to_string(),
            Some("COLLECTION".to_string()),
            0,
            0,
        )
        .unwrap();

        assert_eq!(column.column_type, OracleColumnType::Object);
        assert_eq!(column.ora_type_num, ORA_TYPE_NUM_OBJECT);
        assert_eq!(column.schema_name, "SYSTEM");
        assert_eq!(column.type_name, "varchar2");
        assert_eq!(column.buffer_size, 1);
    }

    #[test]
    fn object_attr_sys_xmltype_keeps_xmltype_special_case() {
        for owner in ["PUBLIC", "SYS"] {
            let column = thin_column_from_object_attr(
                "DOC".to_string(),
                "XMLTYPE".to_string(),
                owner.to_string(),
                Some("OBJECT".to_string()),
                0,
                0,
            )
            .unwrap();

            assert_eq!(column.column_type, OracleColumnType::Xml, "{owner}");
            assert_eq!(column.ora_type_num, ORA_TYPE_NUM_OBJECT, "{owner}");
            assert_eq!(column.schema_name, owner, "{owner}");
            assert_eq!(column.type_name, "XMLTYPE", "{owner}");
            assert_eq!(column.buffer_size, 1, "{owner}");
        }
    }

    #[test]
    fn object_attr_timestamp_tz_aliases_map_like_vendor() {
        for (attr_type, expected_ora_type, expected_buffer_size) in [
            ("TIMESTAMP WITH TIME ZONE", ORA_TYPE_NUM_TIMESTAMP_TZ, 13),
            ("TIMESTAMP WITH TZ", ORA_TYPE_NUM_TIMESTAMP_TZ, 13),
            (
                "TIMESTAMP WITH LOCAL TIME ZONE",
                ORA_TYPE_NUM_TIMESTAMP_LTZ,
                11,
            ),
            ("TIMESTAMP WITH LOCAL TZ", ORA_TYPE_NUM_TIMESTAMP_LTZ, 11),
        ] {
            let column = thin_column_from_object_attr(
                "VALUE".to_string(),
                attr_type.to_string(),
                String::new(),
                None,
                0,
                0,
            )
            .unwrap();

            assert_eq!(
                column.column_type,
                OracleColumnType::Timestamp,
                "{attr_type}"
            );
            assert_eq!(column.ora_type_num, expected_ora_type, "{attr_type}");
            assert_eq!(column.buffer_size, expected_buffer_size, "{attr_type}");
        }
    }

    #[test]
    fn data_type_negotiation_keeps_vendor_314_plus_entries() {
        for expected in [
            (13, 0, 0),
            (u16::from(TNS_DATA_TYPE_BFLOAT), 0, 0),
            (u16::from(TNS_DATA_TYPE_BDOUBLE), 0, 0),
            (191, 0, 0),
            (515, 0, 0),
            (562, 562, 1),
            (617, 617, 1),
            (662, 662, 1),
            (899, 899, 1),
            (900, 900, 1),
            (901, 901, 1),
            (
                u16::from(TNS_DATA_TYPE_CFILE),
                u16::from(TNS_DATA_TYPE_CFILE),
                1,
            ),
        ] {
            assert!(
                DATA_TYPE_REPRESENTATIONS.contains(&expected),
                "missing data type negotiation entry {expected:?}"
            );
        }
    }

    #[test]
    fn data_type_negotiation_keeps_python_oracledb_decoder_mappings() {
        for expected in [
            (
                u16::from(TNS_DATA_TYPE_BINARY_INTEGER),
                u16::from(ORA_TYPE_NUM_NUMBER),
                10,
            ),
            (
                u16::from(TNS_DATA_TYPE_FLOAT),
                u16::from(ORA_TYPE_NUM_NUMBER),
                10,
            ),
            (
                u16::from(TNS_DATA_TYPE_STR),
                u16::from(ORA_TYPE_NUM_VARCHAR),
                1,
            ),
            (
                u16::from(TNS_DATA_TYPE_VNU),
                u16::from(ORA_TYPE_NUM_NUMBER),
                10,
            ),
            (
                u16::from(TNS_DATA_TYPE_PDN),
                u16::from(ORA_TYPE_NUM_NUMBER),
                10,
            ),
            (
                u16::from(TNS_DATA_TYPE_VCS),
                u16::from(ORA_TYPE_NUM_VARCHAR),
                1,
            ),
            (
                u16::from(TNS_DATA_TYPE_VBI),
                u16::from(ORA_TYPE_NUM_VARCHAR),
                1,
            ),
            (
                u16::from(TNS_DATA_TYPE_UIN),
                u16::from(ORA_TYPE_NUM_NUMBER),
                10,
            ),
            (
                u16::from(TNS_DATA_TYPE_SLS),
                u16::from(ORA_TYPE_NUM_NUMBER),
                10,
            ),
            (
                u16::from(TNS_DATA_TYPE_LVC),
                u16::from(ORA_TYPE_NUM_VARCHAR),
                1,
            ),
            (u16::from(TNS_DATA_TYPE_LVB), u16::from(ORA_TYPE_NUM_RAW), 1),
            (27, 27, 10),
            (39, 39, 1),
            (
                u16::from(TNS_DATA_TYPE_RDD),
                u16::from(ORA_TYPE_NUM_ROWID),
                1,
            ),
            (
                u16::from(TNS_DATA_TYPE_EXT_NAMED),
                u16::from(ORA_TYPE_NUM_OBJECT),
                1,
            ),
            (
                u16::from(TNS_DATA_TYPE_CFILE),
                u16::from(TNS_DATA_TYPE_CFILE),
                1,
            ),
            (
                u16::from(TNS_DATA_TYPE_RSET),
                u16::from(ORA_TYPE_NUM_CURSOR),
                1,
            ),
            (
                u16::from(ORA_TYPE_NUM_JSON),
                u16::from(ORA_TYPE_NUM_JSON),
                1,
            ),
            (
                u16::from(ORA_TYPE_NUM_DJSON),
                u16::from(ORA_TYPE_NUM_DJSON),
                1,
            ),
            (
                u16::from(TNS_DATA_TYPE_CLV),
                u16::from(TNS_DATA_TYPE_CLV),
                1,
            ),
            (
                u16::from(TNS_DATA_TYPE_DTR),
                u16::from(ORA_TYPE_NUM_NUMBER),
                10,
            ),
            (
                u16::from(TNS_DATA_TYPE_DUN),
                u16::from(ORA_TYPE_NUM_NUMBER),
                10,
            ),
            (
                u16::from(TNS_DATA_TYPE_DOP),
                u16::from(ORA_TYPE_NUM_NUMBER),
                10,
            ),
            (
                u16::from(TNS_DATA_TYPE_VST),
                u16::from(ORA_TYPE_NUM_VARCHAR),
                1,
            ),
            (
                u16::from(TNS_DATA_TYPE_ODT),
                u16::from(ORA_TYPE_NUM_DATE),
                10,
            ),
            (
                u16::from(TNS_DATA_TYPE_DOL),
                u16::from(ORA_TYPE_NUM_NUMBER),
                10,
            ),
            (
                u16::from(TNS_DATA_TYPE_EDATE),
                u16::from(ORA_TYPE_NUM_DATE),
                10,
            ),
            (
                u16::from(ORA_TYPE_NUM_TIMESTAMP_LTZ),
                u16::from(ORA_TYPE_NUM_TIMESTAMP_LTZ),
                1,
            ),
            (
                u16::from(TNS_DATA_TYPE_ESITZ),
                u16::from(ORA_TYPE_NUM_TIMESTAMP_LTZ),
                1,
            ),
            (
                u16::from(TNS_DATA_TYPE_DCLOB),
                u16::from(ORA_TYPE_NUM_CLOB),
                1,
            ),
            (
                u16::from(TNS_DATA_TYPE_DBLOB),
                u16::from(ORA_TYPE_NUM_BLOB),
                1,
            ),
            (
                u16::from(ORA_TYPE_NUM_DBFILE),
                u16::from(ORA_TYPE_NUM_BFILE),
                1,
            ),
            (
                u16::from(TNS_DATA_TYPE_PNTY),
                u16::from(ORA_TYPE_NUM_OBJECT),
                1,
            ),
            (
                u16::from(ORA_TYPE_NUM_VECTOR),
                u16::from(ORA_TYPE_NUM_VECTOR),
                1,
            ),
        ] {
            assert!(
                DATA_TYPE_REPRESENTATIONS.contains(&expected),
                "missing python-oracledb data type negotiation entry {expected:?}"
            );
        }
        assert!(
            !DATA_TYPE_REPRESENTATIONS.contains(&(
                u16::from(ORA_TYPE_NUM_DJSON),
                u16::from(ORA_TYPE_NUM_VECTOR),
                1,
            )),
            "DJSON must negotiate as JSON; VECTOR has its own Oracle type 127"
        );
    }

    #[test]
    fn data_type_negotiation_serializes_native_zero_mappings_without_terminating() {
        let mut payload = Vec::new();
        write_data_type_representations(&mut payload, None);
        let mut pos = 0;
        let mut entries = Vec::new();
        loop {
            let data_type = u16::from_be_bytes([payload[pos], payload[pos + 1]]);
            pos += 2;
            if data_type == 0 {
                break;
            }
            let conv_data_type = u16::from_be_bytes([payload[pos], payload[pos + 1]]);
            pos += 2;
            entries.push((data_type, conv_data_type));
            if conv_data_type != 0 {
                pos += 4;
            }
        }

        assert!(entries.contains(&(13, 0)));
        assert!(entries.contains(&(14, 0)));
        assert!(entries.contains(&(515, 0)));
        assert!(entries.contains(&(899, 899)));
        assert_eq!(pos, payload.len());
    }

    #[test]
    fn data_type_negotiation_keeps_timestamp_ltz_alias_like_vendors() {
        let mut payload = Vec::new();
        write_data_type_representations(&mut payload, None);
        let mut pos = 0;
        let mut entries = Vec::new();
        loop {
            let data_type = u16::from_be_bytes([payload[pos], payload[pos + 1]]);
            pos += 2;
            if data_type == 0 {
                break;
            }
            let conv_data_type = u16::from_be_bytes([payload[pos], payload[pos + 1]]);
            pos += 2;
            entries.push((data_type, conv_data_type));
            if conv_data_type != 0 {
                pos += 4;
            }
        }

        assert!(entries.contains(&(
            u16::from(ORA_TYPE_NUM_TIMESTAMP_LTZ),
            u16::from(ORA_TYPE_NUM_TIMESTAMP_LTZ)
        )));
        assert!(entries.contains(&(
            u16::from(TNS_DATA_TYPE_ESITZ),
            u16::from(ORA_TYPE_NUM_TIMESTAMP_LTZ)
        )));
    }

    #[test]
    fn data_type_negotiation_serializes_vbi_like_go_ora_for_protocol_314() {
        let mut payload = Vec::new();
        write_data_type_representations(&mut payload, Some(314));
        let mut pos = 0;
        let mut entries = Vec::new();
        loop {
            let data_type = u16::from_be_bytes([payload[pos], payload[pos + 1]]);
            pos += 2;
            if data_type == 0 {
                break;
            }
            let conv_data_type = u16::from_be_bytes([payload[pos], payload[pos + 1]]);
            pos += 2;
            entries.push((data_type, conv_data_type));
            if conv_data_type != 0 {
                pos += 4;
            }
        }

        assert!(entries.contains(&(u16::from(TNS_DATA_TYPE_VBI), u16::from(ORA_TYPE_NUM_RAW))));
    }

    #[test]
    fn data_type_negotiation_serializes_oac9_like_go_ora_for_protocol_314() {
        let mut payload = Vec::new();
        write_data_type_representations(&mut payload, Some(314));
        let mut pos = 0;
        let mut entries = Vec::new();
        loop {
            let data_type = u16::from_be_bytes([payload[pos], payload[pos + 1]]);
            pos += 2;
            if data_type == 0 {
                break;
            }
            let conv_data_type = u16::from_be_bytes([payload[pos], payload[pos + 1]]);
            pos += 2;
            entries.push((data_type, conv_data_type));
            if conv_data_type != 0 {
                pos += 4;
            }
        }

        assert!(entries.contains(&(u16::from(TNS_DATA_TYPE_OAC9), TNS_DATA_TYPE_OAC)));
    }

    #[test]
    fn data_type_negotiation_keeps_oac9_like_python_for_modern_protocols() {
        let mut payload = Vec::new();
        write_data_type_representations(&mut payload, Some(315));
        let mut pos = 0;
        let mut entries = Vec::new();
        loop {
            let data_type = u16::from_be_bytes([payload[pos], payload[pos + 1]]);
            pos += 2;
            if data_type == 0 {
                break;
            }
            let conv_data_type = u16::from_be_bytes([payload[pos], payload[pos + 1]]);
            pos += 2;
            entries.push((data_type, conv_data_type));
            if conv_data_type != 0 {
                pos += 4;
            }
        }

        assert!(entries.contains(&(u16::from(TNS_DATA_TYPE_OAC9), u16::from(TNS_DATA_TYPE_OAC9))));
    }

    #[test]
    fn data_type_negotiation_adds_python_oracledb_modern_internal_mappings_only_for_modern_protocols(
    ) {
        let mut modern_payload = Vec::new();
        write_data_type_representations(&mut modern_payload, Some(315));
        let mut pos = 0;
        let mut modern_entries = Vec::new();
        loop {
            let data_type = u16::from_be_bytes([modern_payload[pos], modern_payload[pos + 1]]);
            pos += 2;
            if data_type == 0 {
                break;
            }
            let conv_data_type = u16::from_be_bytes([modern_payload[pos], modern_payload[pos + 1]]);
            pos += 2;
            let representation = if conv_data_type != 0 {
                let representation =
                    u16::from_be_bytes([modern_payload[pos], modern_payload[pos + 1]]);
                pos += 4;
                representation
            } else {
                0
            };
            modern_entries.push((data_type, conv_data_type, representation));
        }

        for expected in PYTHON_ORACLEDB_MODERN_DATA_TYPE_REPRESENTATIONS {
            assert!(
                modern_entries.contains(expected),
                "missing python-oracledb modern data type negotiation entry {expected:?}"
            );
        }

        let mut legacy_payload = Vec::new();
        write_data_type_representations(&mut legacy_payload, Some(314));
        let mut pos = 0;
        let mut legacy_entries = Vec::new();
        loop {
            let data_type = u16::from_be_bytes([legacy_payload[pos], legacy_payload[pos + 1]]);
            pos += 2;
            if data_type == 0 {
                break;
            }
            let conv_data_type = u16::from_be_bytes([legacy_payload[pos], legacy_payload[pos + 1]]);
            pos += 2;
            let representation = if conv_data_type != 0 {
                let representation =
                    u16::from_be_bytes([legacy_payload[pos], legacy_payload[pos + 1]]);
                pos += 4;
                representation
            } else {
                0
            };
            legacy_entries.push((data_type, conv_data_type, representation));
        }

        assert!(
            !legacy_entries.contains(&(34, 34, 1)),
            "protocol 314 should keep the go-ora data type negotiation table"
        );
    }

    #[test]
    fn metadata_maps_vbi_by_protocol_like_vendors() {
        assert_eq!(
            oracle_column_type_from_ora_type_for_protocol(TNS_DATA_TYPE_VBI, Some(314)),
            OracleColumnType::Raw
        );
        assert_eq!(
            oracle_column_type_from_ora_type_for_protocol(TNS_DATA_TYPE_VBI, Some(315)),
            OracleColumnType::Varchar
        );
        assert_eq!(
            oracle_column_type_from_ora_type(TNS_DATA_TYPE_VBI),
            OracleColumnType::Varchar
        );
    }

    #[test]
    fn row_scanner_decodes_vendor_negotiated_alias_types() {
        let mut state = ExecuteReadState::default();
        state.columns = vec![
            ThinColumn {
                name: "N".to_string(),
                column_type: OracleColumnType::Number,
                ora_type_num: TNS_DATA_TYPE_DTR,
                charset_form: 0,
                buffer_size: 22,
                schema_name: String::new(),
                type_name: String::new(),
            },
            ThinColumn {
                name: "S".to_string(),
                column_type: OracleColumnType::Varchar,
                ora_type_num: TNS_DATA_TYPE_VCS,
                charset_form: CS_FORM_IMPLICIT,
                buffer_size: 10,
                schema_name: String::new(),
                type_name: String::new(),
            },
            ThinColumn {
                name: "CLV".to_string(),
                column_type: OracleColumnType::Varchar,
                ora_type_num: TNS_DATA_TYPE_CLV,
                charset_form: CS_FORM_IMPLICIT,
                buffer_size: 10,
                schema_name: String::new(),
                type_name: String::new(),
            },
            ThinColumn {
                name: "R".to_string(),
                column_type: OracleColumnType::Raw,
                ora_type_num: TNS_DATA_TYPE_LVB,
                charset_form: 0,
                buffer_size: 3,
                schema_name: String::new(),
                type_name: String::new(),
            },
            ThinColumn {
                name: "D".to_string(),
                column_type: OracleColumnType::Date,
                ora_type_num: TNS_DATA_TYPE_ODT,
                charset_form: 0,
                buffer_size: 7,
                schema_name: String::new(),
                type_name: String::new(),
            },
            ThinColumn {
                name: "C".to_string(),
                column_type: OracleColumnType::Clob,
                ora_type_num: TNS_DATA_TYPE_DCLOB,
                charset_form: 0,
                buffer_size: 4,
                schema_name: String::new(),
                type_name: String::new(),
            },
            ThinColumn {
                name: "U".to_string(),
                column_type: OracleColumnType::Number,
                ora_type_num: TNS_DATA_TYPE_UB8,
                charset_form: 0,
                buffer_size: 8,
                schema_name: String::new(),
                type_name: String::new(),
            },
            ThinColumn {
                name: "T".to_string(),
                column_type: OracleColumnType::Varchar,
                ora_type_num: TNS_DATA_TYPE_TIME,
                charset_form: 0,
                buffer_size: 11,
                schema_name: String::new(),
                type_name: String::new(),
            },
            ThinColumn {
                name: "TTZ".to_string(),
                column_type: OracleColumnType::Varchar,
                ora_type_num: TNS_DATA_TYPE_TIME_TZ,
                charset_form: 0,
                buffer_size: 13,
                schema_name: String::new(),
                type_name: String::new(),
            },
            ThinColumn {
                name: "RID".to_string(),
                column_type: OracleColumnType::Varchar,
                ora_type_num: TNS_DATA_TYPE_RDD,
                charset_form: 0,
                buffer_size: 18,
                schema_name: String::new(),
                type_name: String::new(),
            },
        ];
        let caps = OracleThinCapabilities::default();
        let mut row = Vec::new();
        write_bytes_with_length_for_capabilities(
            &mut row,
            &encode_oracle_number("42").unwrap(),
            &caps,
        )
        .unwrap();
        write_bytes_with_length_for_capabilities(&mut row, b"ok", &caps).unwrap();
        write_bytes_with_length_for_capabilities(&mut row, b"clv", &caps).unwrap();
        write_bytes_with_length_for_capabilities(&mut row, &[1, 2, 3], &caps).unwrap();
        row.extend_from_slice(&[7, 120, 124, 1, 2, 4, 5, 6]);
        write_bytes_with_length_for_capabilities(&mut row, b"clob", &caps).unwrap();
        write_bytes_with_length_for_capabilities(&mut row, &[1, 2, 3, 4, 5, 6, 7, 8], &caps)
            .unwrap();
        write_bytes_with_length_for_capabilities(
            &mut row,
            &[120, 124, 1, 2, 4, 5, 6, 7, 91, 202, 0],
            &caps,
        )
        .unwrap();
        write_bytes_with_length_for_capabilities(
            &mut row,
            &[120, 124, 1, 1, 22, 20, 6, 7, 91, 202, 0, 25, 105],
            &caps,
        )
        .unwrap();
        row.extend_from_slice(&[18, 1, 1, 1, 2, 0, 1, 3, 1, 4]);
        let mut cursor = PacketCursor::with_capabilities(&row, &caps);

        process_row_data(&mut cursor, &caps, &mut state).unwrap();

        assert_eq!(
            state.rows,
            vec![vec![
                OracleValue::Number("42".to_string()),
                OracleValue::Text("ok".to_string()),
                OracleValue::Text("clv".to_string()),
                OracleValue::Bytes(vec![1, 2, 3]),
                OracleValue::DateTime(crate::OracleDateTime {
                    year: 2024,
                    month: 1,
                    day: 2,
                    hour: 3,
                    minute: 4,
                    second: 5,
                    nanosecond: 0,
                    timezone_offset_minutes: None,
                    timezone_region_id: None,
                }),
                OracleValue::Lob(b"clob".to_vec()),
                OracleValue::Number("72623859790382856".to_string()),
                OracleValue::Text("03:04:05.123456".to_string()),
                OracleValue::Text("03:04:05.123456+05:45".to_string()),
                OracleValue::Text("AAAAABAACAAAAADAAE".to_string()),
            ]]
        );
    }

    #[test]
    fn row_scanner_decodes_vbi_as_raw_like_go_ora_for_protocol_314() {
        let mut state = ExecuteReadState::default();
        state.columns = vec![ThinColumn {
            name: "VBI".to_string(),
            column_type: OracleColumnType::Raw,
            ora_type_num: TNS_DATA_TYPE_VBI,
            charset_form: 0,
            buffer_size: 3,
            schema_name: String::new(),
            type_name: String::new(),
        }];
        let mut caps = OracleThinCapabilities {
            protocol_version: Some(314),
            ..OracleThinCapabilities::default()
        };
        caps.supports_big_clr_chunks = false;
        let mut row = Vec::new();
        write_bytes_with_length_for_capabilities(&mut row, &[0xff, 0x00, 0x80], &caps).unwrap();
        let mut cursor = PacketCursor::with_capabilities(&row, &caps);

        process_row_data(&mut cursor, &caps, &mut state).unwrap();

        assert_eq!(
            state.rows,
            vec![vec![OracleValue::Bytes(vec![0xff, 0x00, 0x80])]]
        );
    }

    #[test]
    fn row_scanner_decodes_vbi_as_text_like_python_oracledb_for_modern_protocols() {
        let mut state = ExecuteReadState::default();
        state.columns = vec![ThinColumn {
            name: "VBI".to_string(),
            column_type: OracleColumnType::Varchar,
            ora_type_num: TNS_DATA_TYPE_VBI,
            charset_form: CS_FORM_IMPLICIT,
            buffer_size: 3,
            schema_name: String::new(),
            type_name: String::new(),
        }];
        let caps = OracleThinCapabilities {
            protocol_version: Some(315),
            ..OracleThinCapabilities::default()
        };
        let mut row = Vec::new();
        write_bytes_with_length_for_capabilities(&mut row, b"abc", &caps).unwrap();
        let mut cursor = PacketCursor::with_capabilities(&row, &caps);

        process_row_data(&mut cursor, &caps, &mut state).unwrap();

        assert_eq!(state.rows, vec![vec![OracleValue::Text("abc".to_string())]]);
    }

    #[test]
    fn out_bind_row_scanner_rejects_truncated_values_like_python_oracledb() {
        let mut state = ExecuteReadState {
            reading_out_binds: true,
            out_bind_columns: vec![ThinColumn {
                name: "P".to_string(),
                column_type: OracleColumnType::Varchar,
                ora_type_num: ORA_TYPE_NUM_VARCHAR,
                charset_form: CS_FORM_IMPLICIT,
                buffer_size: 3,
                schema_name: String::new(),
                type_name: String::new(),
            }],
            ..ExecuteReadState::default()
        };
        let caps = OracleThinCapabilities::default();
        let mut row = Vec::new();
        write_bytes_with_length_for_capabilities(&mut row, b"abc", &caps).unwrap();
        row.extend_from_slice(&[1, 10]);
        let mut cursor = PacketCursor::with_capabilities(&row, &caps);

        let err = process_row_data(&mut cursor, &caps, &mut state)
            .expect_err("truncated OUT bind should fail");

        assert!(err
            .to_string()
            .contains("Oracle OUT bind value truncated: actual length 10"));
    }

    #[test]
    fn out_bind_row_scanner_maps_negative_boolean_actual_length_to_null_like_python_oracledb() {
        let mut state = ExecuteReadState {
            reading_out_binds: true,
            out_bind_columns: vec![ThinColumn {
                name: "P".to_string(),
                column_type: OracleColumnType::Boolean,
                ora_type_num: ORA_TYPE_NUM_BOOLEAN,
                charset_form: 0,
                buffer_size: 4,
                schema_name: String::new(),
                type_name: String::new(),
            }],
            ..ExecuteReadState::default()
        };
        let row = [2, 1, 1, 0x81, 1];
        let caps = OracleThinCapabilities::default();
        let mut cursor = PacketCursor::with_capabilities(&row, &caps);

        process_row_data(&mut cursor, &caps, &mut state).unwrap();

        assert_eq!(state.out_bind_rows, vec![vec![OracleValue::Null]]);
    }

    #[test]
    fn row_scanner_decodes_timestamp_ltz_columns_as_timestamps() {
        let mut state = ExecuteReadState::default();
        state.columns = vec![ThinColumn {
            name: "TS_LTZ".to_string(),
            column_type: OracleColumnType::Timestamp,
            ora_type_num: ORA_TYPE_NUM_TIMESTAMP_LTZ,
            charset_form: 0,
            buffer_size: 11,
            schema_name: String::new(),
            type_name: String::new(),
        }];
        let row = [11, 120, 124, 1, 2, 4, 5, 6, 0, 1, 226, 64];
        let mut cursor = PacketCursor::with_capabilities(&row, &OracleThinCapabilities::default());

        process_row_data(&mut cursor, &OracleThinCapabilities::default(), &mut state).unwrap();

        assert_eq!(
            state.rows,
            vec![vec![OracleValue::Timestamp(crate::OracleDateTime {
                year: 2024,
                month: 1,
                day: 2,
                hour: 3,
                minute: 4,
                second: 5,
                nanosecond: 123_456,
                timezone_offset_minutes: None,
                timezone_region_id: None,
            })]]
        );
    }

    #[test]
    fn row_scanner_decodes_extended_timestamptz_columns_as_timestamps() {
        let mut state = ExecuteReadState::default();
        state.columns = vec![ThinColumn {
            name: "TS_TZ".to_string(),
            column_type: OracleColumnType::Timestamp,
            ora_type_num: ORA_TYPE_NUM_TIMESTAMP_TZ_EXT,
            charset_form: 0,
            buffer_size: 13,
            schema_name: String::new(),
            type_name: String::new(),
        }];
        let row = [13, 120, 124, 1, 2, 4, 5, 6, 7, 91, 202, 0, 25, 105];
        let mut cursor = PacketCursor::with_capabilities(&row, &OracleThinCapabilities::default());

        process_row_data(&mut cursor, &OracleThinCapabilities::default(), &mut state).unwrap();

        assert_eq!(
            state.rows,
            vec![vec![OracleValue::Timestamp(crate::OracleDateTime {
                year: 2024,
                month: 1,
                day: 2,
                hour: 8,
                minute: 49,
                second: 5,
                nanosecond: 123_456_000,
                timezone_offset_minutes: Some(345),
                timezone_region_id: None,
            })]]
        );
    }

    #[test]
    fn column_metadata_preserves_nchar_charset_for_later_fetches() {
        let thin = thin_column_from_column_metadata(&ColumnMetadata {
            name: "NV".to_string(),
            column_type: OracleColumnType::Varchar,
            charset_form: CS_FORM_NCHAR,
            ora_type_num: 0,
            buffer_size: 0,
            schema_name: String::new(),
            type_name: String::new(),
        });

        assert_eq!(thin.charset_form, CS_FORM_NCHAR);
        assert_eq!(thin.ora_type_num, ORA_TYPE_NUM_VARCHAR);
    }

    #[test]
    fn nclob_define_fetch_keeps_nchar_charset_form() {
        let thin = define_column_metadata(&ColumnMetadata {
            name: "NC".to_string(),
            column_type: OracleColumnType::Nclob,
            charset_form: CS_FORM_NCHAR,
            ora_type_num: 0,
            buffer_size: 0,
            schema_name: String::new(),
            type_name: String::new(),
        });

        assert_eq!(thin.charset_form, CS_FORM_NCHAR);
        assert_eq!(thin.ora_type_num, ORA_TYPE_NUM_LONG);
        assert_eq!(thin.column_type, OracleColumnType::Long);
    }

    #[test]
    fn metadata_charset_form_is_ignored_for_non_character_types_like_python_oracledb() {
        assert_eq!(
            normalize_metadata_charset_form(OracleColumnType::Number, CS_FORM_NCHAR),
            0
        );
        assert_eq!(
            normalize_metadata_charset_form(OracleColumnType::Object, CS_FORM_NCHAR),
            0
        );
        assert_eq!(
            normalize_metadata_charset_form(OracleColumnType::Cursor, CS_FORM_IMPLICIT),
            0
        );
        assert_eq!(
            normalize_metadata_charset_form(OracleColumnType::Varchar, CS_FORM_NCHAR),
            CS_FORM_NCHAR
        );
        assert_eq!(
            normalize_metadata_charset_form(OracleColumnType::Clob, CS_FORM_NCHAR),
            CS_FORM_NCHAR
        );
        assert_eq!(
            normalize_metadata_charset_form(OracleColumnType::Nclob, CS_FORM_NCHAR),
            CS_FORM_NCHAR
        );
    }

    #[test]
    fn described_column_metadata_round_trips_wire_type_shape() {
        let original = ThinColumn {
            name: "TS_TZ".to_string(),
            column_type: OracleColumnType::Timestamp,
            ora_type_num: ORA_TYPE_NUM_TIMESTAMP_TZ_EXT,
            charset_form: 0,
            buffer_size: 13,
            schema_name: "APP".to_string(),
            type_name: "EVENT_OBJ".to_string(),
        };
        let public = column_metadata_from_thin(&original);
        let restored = thin_column_from_column_metadata(&public);

        assert_eq!(restored.name, original.name);
        assert_eq!(restored.column_type, original.column_type);
        assert_eq!(restored.ora_type_num, ORA_TYPE_NUM_TIMESTAMP_TZ_EXT);
        assert_eq!(restored.buffer_size, 13);
        assert_eq!(public.schema_name, "APP");
        assert_eq!(public.type_name, "EVENT_OBJ");
        assert_eq!(restored.schema_name, "APP");
        assert_eq!(restored.type_name, "EVENT_OBJ");
    }

    #[test]
    fn described_ref_cursor_fetch_columns_keep_exact_wire_type_before_define() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || listener.accept().unwrap().0);
        let stream = TcpStream::connect(addr).unwrap();
        let mut session = test_session_with_stream(stream);
        let _server = handle.join().unwrap();
        session.cursor_columns_by_cursor.insert(
            77,
            vec![ThinColumn {
                name: "TS_TZ".to_string(),
                column_type: OracleColumnType::Timestamp,
                ora_type_num: ORA_TYPE_NUM_TIMESTAMP_TZ_EXT,
                charset_form: 0,
                buffer_size: 13,
                schema_name: String::new(),
                type_name: String::new(),
            }],
        );

        let fetch_columns = session.fetch_columns_for_cursor(77, &[], false);
        assert_eq!(fetch_columns[0].ora_type_num, ORA_TYPE_NUM_TIMESTAMP_TZ_EXT);
        assert_eq!(fetch_columns[0].buffer_size, 13);

        let define_columns = session.fetch_columns_for_cursor(77, &[], true);
        assert_eq!(
            define_columns[0].ora_type_num,
            ORA_TYPE_NUM_TIMESTAMP_TZ_EXT
        );
        assert_eq!(define_columns[0].buffer_size, 13);
    }

    #[test]
    fn long_raw_wire_type_requires_define_even_when_public_type_is_raw() {
        let column = ColumnMetadata {
            name: "LONG_RAW_VALUE".to_string(),
            column_type: OracleColumnType::Raw,
            charset_form: 0,
            ora_type_num: ORA_TYPE_NUM_LONG_RAW,
            buffer_size: 0,
            schema_name: String::new(),
            type_name: String::new(),
        };

        assert!(columns_require_define_fetch_for_values(&[column.clone()]));

        let thin = define_column_metadata(&column);
        assert_eq!(thin.column_type, OracleColumnType::Raw);
        assert_eq!(thin.ora_type_num, ORA_TYPE_NUM_LONG_RAW);
        assert_eq!(thin.buffer_size, TNS_MAX_LONG_LENGTH);
    }

    #[test]
    fn lob_metadata_writer_sets_vendor_prefetch_flag_for_lob_types() {
        for ora_type_num in [
            ORA_TYPE_NUM_CLOB,
            TNS_DATA_TYPE_DCLOB,
            ORA_TYPE_NUM_BLOB,
            TNS_DATA_TYPE_DBLOB,
        ] {
            let mut caps = OracleThinCapabilities::default();
            caps.ttc_field_version = TNS_CCAP_FIELD_VERSION_20_1;
            let column = ThinColumn {
                name: "LOB".to_string(),
                column_type: OracleColumnType::Clob,
                ora_type_num,
                charset_form: 0,
                buffer_size: 4000,
                schema_name: String::new(),
                type_name: String::new(),
            };
            let mut payload = Vec::new();

            write_column_metadata(&mut payload, &caps, &column).unwrap();

            let mut cursor = PacketCursor::with_capabilities(&payload, &caps);
            assert_eq!(cursor.read_u8().unwrap(), ora_type_num);
            assert_eq!(cursor.read_u8().unwrap(), TNS_BIND_USE_INDICATORS);
            assert_eq!(cursor.read_u8().unwrap(), 0);
            assert_eq!(cursor.read_u8().unwrap(), 0);
            assert_eq!(cursor.read_ub4().unwrap(), 4000);
            assert_eq!(cursor.read_ub4().unwrap(), 0);
            assert_eq!(cursor.read_ub8().unwrap(), TNS_LOB_PREFETCH_FLAG);
            assert_eq!(cursor.read_ub4().unwrap(), 0);
            assert_eq!(cursor.read_ub2().unwrap(), 0);
            assert_eq!(cursor.read_ub2().unwrap(), 0);
            assert_eq!(cursor.read_u8().unwrap(), 0);
            assert_eq!(cursor.read_ub4().unwrap(), 0);
            assert_eq!(cursor.read_ub4().unwrap(), 0);
        }
    }

    #[test]
    fn json_metadata_writer_uses_python_oracledb_json_tail() {
        let mut caps = OracleThinCapabilities::default();
        caps.ttc_field_version = TNS_CCAP_FIELD_VERSION_20_1;
        let column = ThinColumn {
            name: "JSON".to_string(),
            column_type: OracleColumnType::Json,
            ora_type_num: ORA_TYPE_NUM_JSON,
            charset_form: 0,
            buffer_size: TNS_JSON_MAX_LENGTH,
            schema_name: String::new(),
            type_name: String::new(),
        };
        let mut payload = Vec::new();

        write_column_metadata(&mut payload, &caps, &column).unwrap();

        let expected = [
            ORA_TYPE_NUM_JSON,
            TNS_BIND_USE_INDICATORS,
            0,
            0,
            4,
            0x02,
            0x00,
            0x00,
            0x00,
            0,
            4,
            0x02,
            0x00,
            0x00,
            0x00,
            0,
            0,
            0,
            0,
            4,
            0x02,
            0x00,
            0x00,
            0x00,
            0,
        ];
        assert_eq!(payload, expected);
    }

    #[test]
    fn physical_rowid_values_encode_with_oracle_base64_alphabet() {
        assert_eq!(
            encode_physical_rowid(1, 2, 3, 4).unwrap(),
            OracleValue::Text("AAAAABAACAAAAADAAE".to_string())
        );
        assert_eq!(
            encode_physical_rowid(0, 0, 0, 0).unwrap(),
            OracleValue::Null
        );
    }

    #[test]
    fn rowid_fetch_decodes_physical_rowid_fields() {
        let rowid_bytes = [18, 1, 1, 1, 2, 0, 1, 3, 1, 4];
        let mut cursor =
            PacketCursor::with_capabilities(&rowid_bytes, &OracleThinCapabilities::default());

        assert_eq!(
            read_rowid_value(&mut cursor).unwrap(),
            OracleValue::Text("AAAAABAACAAAAADAAE".to_string())
        );
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn urowid_fetch_decodes_physical_and_logical_forms() {
        let mut cursor = PacketCursor::with_capabilities(&[0], &OracleThinCapabilities::default());
        assert_eq!(read_urowid_value(&mut cursor).unwrap(), OracleValue::Null);
        assert_eq!(cursor.remaining(), 0);

        let physical = [1, 0, 13, 1, 0, 0, 0, 1, 0, 2, 0, 0, 0, 3, 0, 4];
        let mut cursor =
            PacketCursor::with_capabilities(&physical, &OracleThinCapabilities::default());
        assert_eq!(
            read_urowid_value(&mut cursor).unwrap(),
            OracleValue::Text("AAAAABAACAAAAADAAE".to_string())
        );
        assert_eq!(cursor.remaining(), 0);

        let logical = [1, 0, 13, 2, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let mut cursor =
            PacketCursor::with_capabilities(&logical, &OracleThinCapabilities::default());
        assert_eq!(
            read_urowid_value(&mut cursor).unwrap(),
            OracleValue::Text("*AQIDBAUGBwgJCgsM".to_string())
        );
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn row_scanner_decodes_rowid_and_urowid_columns_as_text() {
        let mut state = ExecuteReadState::default();
        state.columns = vec![
            ThinColumn {
                name: "RID".to_string(),
                column_type: OracleColumnType::Rowid,
                ora_type_num: ORA_TYPE_NUM_ROWID,
                charset_form: 0,
                buffer_size: 18,
                schema_name: String::new(),
                type_name: String::new(),
            },
            ThinColumn {
                name: "URID".to_string(),
                column_type: OracleColumnType::Urowid,
                ora_type_num: ORA_TYPE_NUM_UROWID,
                charset_form: 0,
                buffer_size: 18,
                schema_name: String::new(),
                type_name: String::new(),
            },
        ];
        let row = [
            18, 1, 1, 1, 2, 0, 1, 3, 1, 4, 1, 0, 13, 1, 0, 0, 0, 1, 0, 2, 0, 0, 0, 3, 0, 4,
        ];
        let mut cursor = PacketCursor::with_capabilities(&row, &OracleThinCapabilities::default());

        process_row_data(&mut cursor, &OracleThinCapabilities::default(), &mut state).unwrap();

        assert_eq!(
            state.rows,
            vec![vec![
                OracleValue::Text("AAAAABAACAAAAADAAE".to_string()),
                OracleValue::Text("AAAAABAACAAAAADAAE".to_string())
            ]]
        );
    }

    #[test]
    fn row_scanner_decodes_null_rowid_and_urowid_columns() {
        let mut state = ExecuteReadState::default();
        state.columns = vec![
            ThinColumn {
                name: "RID".to_string(),
                column_type: OracleColumnType::Rowid,
                ora_type_num: ORA_TYPE_NUM_ROWID,
                charset_form: 0,
                buffer_size: 18,
                schema_name: String::new(),
                type_name: String::new(),
            },
            ThinColumn {
                name: "URID".to_string(),
                column_type: OracleColumnType::Urowid,
                ora_type_num: ORA_TYPE_NUM_UROWID,
                charset_form: 0,
                buffer_size: 18,
                schema_name: String::new(),
                type_name: String::new(),
            },
        ];
        let row = [0, 0];
        let mut cursor = PacketCursor::with_capabilities(&row, &OracleThinCapabilities::default());

        process_row_data(&mut cursor, &OracleThinCapabilities::default(), &mut state).unwrap();

        assert_eq!(state.rows, vec![vec![OracleValue::Null, OracleValue::Null]]);
    }

    #[test]
    fn decodes_oson_scalar_and_container_values_as_json_text() {
        let scalar = [
            0xff, 0x4a, 0x5a, 0x01, 0x00, 0x12, 0x00, 0x03, 0x02, b'o', b'k',
        ];
        assert_eq!(decode_oson_to_json(&scalar).unwrap(), "\"ok\"");

        let object = [
            0xff, 0x4a, 0x5a, 0x01, 0x21, 0x02, 0x01, 0x00, 0x02, 0x00, 0x08, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x01, b'a', 0xa4, 0x01, 0x01, 0x00, 0x00, 0x00, 0x07, 0x31,
        ];
        assert_eq!(decode_oson_to_json(&object).unwrap(), "{\"a\":true}");

        let array = [
            0xff, 0x4a, 0x5a, 0x01, 0x21, 0x02, 0x00, 0x00, 0x00, 0x00, 0x11, 0x00, 0x00, 0xe0,
            0x03, 0x00, 0x00, 0x00, 0x0e, 0x00, 0x00, 0x00, 0x0f, 0x00, 0x00, 0x00, 0x10, 0x31,
            0x32, 0x30,
        ];
        assert_eq!(decode_oson_to_json(&array).unwrap(), "[true,false,null]");

        let raw = [
            0xff, 0x4a, 0x5a, 0x01, 0x00, 0x12, 0x00, 0x06, 0x3a, 0x00, 0x03, b'r', b'a', b'w',
        ];
        assert_eq!(
            decode_oson_to_json(&raw).unwrap(),
            r#"{"$rawhex":"726177"}"#
        );
    }

    #[test]
    fn encodes_json_bind_text_as_oson_payload() {
        let simple = serde_json::json!({"a": true});
        assert_eq!(
            encode_oson_json(&simple, false).unwrap(),
            vec![
                0xff, 0x4a, 0x5a, 0x01, 0x21, 0x02, 0x01, 0x00, 0x02, 0x00, 0x08, 0x00, 0x00, 0x2c,
                0x00, 0x00, 0x01, b'a', 0xa4, 0x01, 0x01, 0x00, 0x00, 0x00, 0x07, 0x31
            ]
        );

        let empty_array = serde_json::json!([]);
        assert_eq!(
            encode_oson_json(&empty_array, false).unwrap(),
            vec![
                0xff, 0x4a, 0x5a, 0x01, 0x21, 0x02, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0xe0,
                0x00
            ]
        );

        let value = serde_json::json!({
            "input": "ok",
            "added": 2,
            "nested": [true, false, null]
        });
        let encoded = encode_oson_json(&value, false).unwrap();

        assert_eq!(
            decode_oson_to_json(&encoded).unwrap(),
            r#"{"added":2,"input":"ok","nested":[true,false,null]}"#
        );

        let raw = b"A raw value";
        let raw_encoded = encode_oson_raw_json(raw).unwrap();
        assert_eq!(
            decode_oson_to_json(&raw_encoded).unwrap(),
            r#"{"$rawhex":"41207261772076616c7565"}"#
        );
        assert_eq!(
            decode_json_payload_value(&raw_encoded).unwrap(),
            OracleValue::Bytes(raw.to_vec())
        );
        assert_eq!(
            raw_encoded,
            vec![
                0xff, 0x4a, 0x5a, 0x01, 0x00, 0x12, 0x00, 0x0e, 0x3a, 0x00, 0x0b, b'A', b' ', b'r',
                b'a', b'w', b' ', b'v', b'a', b'l', b'u', b'e',
            ]
        );

        let json_id = [0x01, 0x23, 0xab];
        let id_encoded = encode_oson_id_json(&json_id).unwrap();
        assert_eq!(decode_oson_to_json(&id_encoded).unwrap(), r#""0123ab""#);
        assert_eq!(
            decode_json_payload_value(&id_encoded).unwrap(),
            OracleValue::JsonId(json_id.to_vec())
        );
        assert_eq!(
            id_encoded,
            vec![
                0xff,
                0x4a,
                0x5a,
                0x01,
                0x00,
                0x12,
                0x00,
                0x05,
                TNS_JSON_TYPE_ID,
                0x03,
                0x01,
                0x23,
                0xab,
            ]
        );

        assert_eq!(
            decode_oson_to_json(&encode_oson_bool_json(true).unwrap()).unwrap(),
            "true"
        );
        assert_eq!(
            decode_oson_to_json(&encode_oson_number_json("25.25").unwrap()).unwrap(),
            "25.25"
        );
        assert_eq!(
            decode_oson_to_json(&encode_oson_string_json("String 1").unwrap()).unwrap(),
            r#""String 1""#
        );

        let date = crate::OracleDateTime {
            year: 2002,
            month: 12,
            day: 13,
            hour: 0,
            minute: 0,
            second: 0,
            nanosecond: 0,
            timezone_offset_minutes: None,
            timezone_region_id: None,
        };
        assert_eq!(
            decode_oson_to_json(&encode_oson_date_json(&date).unwrap()).unwrap(),
            r#""2002-12-13T00:00:00""#
        );

        let timestamp = crate::OracleDateTime {
            year: 2020,
            month: 12,
            day: 2,
            hour: 13,
            minute: 29,
            second: 14,
            nanosecond: 123_456_000,
            timezone_offset_minutes: None,
            timezone_region_id: None,
        };
        assert_eq!(
            decode_oson_to_json(&encode_oson_timestamp_json(&timestamp).unwrap()).unwrap(),
            r#""2020-12-02T13:29:14.123456""#
        );

        let interval_ym = OracleIntervalYearMonth {
            years: 2,
            months: 3,
        };
        assert_eq!(
            decode_oson_to_json(&encode_oson_interval_ym_json(&interval_ym).unwrap()).unwrap(),
            r#""+02-03""#
        );

        let interval_ds = OracleIntervalDaySecond {
            days: 8,
            hours: 12,
            minutes: 0,
            seconds: 0,
            nanoseconds: 0,
        };
        assert_eq!(
            decode_oson_to_json(&encode_oson_interval_ds_json(&interval_ds).unwrap()).unwrap(),
            r#""+08 12:00:00.000000""#
        );

        let long_name = "k".repeat(300);
        let long_name_value = serde_json::json!({
            long_name.clone(): "ok",
            "short": 1
        });
        assert!(encode_oson_json(&long_name_value, false)
            .unwrap_err()
            .to_string()
            .contains("OSON long field name support"));
        let encoded = encode_oson_json(&long_name_value, true).unwrap();
        assert_eq!(encoded[3], 3);
        let decoded = decode_oson_to_json(&encoded).unwrap();
        let decoded: serde_json::Value = serde_json::from_str(&decoded).unwrap();
        assert_eq!(
            decoded.get(&long_name).and_then(|value| value.as_str()),
            Some("ok")
        );
        assert_eq!(
            decoded.get("short").and_then(|value| value.as_i64()),
            Some(1)
        );

        let mut bind_payload = Vec::new();
        write_bind_value(
            &mut bind_payload,
            &OracleThinCapabilities::default(),
            &BindValue::InOut {
                column_type: OracleColumnType::Json,
                max_len: 1024,
                value: Some(BindInputValue::Text(r#"{"input":"ok"}"#.to_string())),
            },
        )
        .unwrap();
        assert!(bind_payload.starts_with(&[1, 40, 40]));
        assert_eq!(&bind_payload[3..7], &[0, 38, 0, 4]);
        assert_eq!(
            decode_oson_to_json(&bind_payload[44..]).unwrap(),
            r#"{"input":"ok"}"#
        );

        let bind = BindValue::Json(r#"{"input":"ok"}"#.to_string());
        let metadata = bind_column_metadata(&bind);
        assert_eq!(metadata.column_type, OracleColumnType::Json);
        assert_eq!(metadata.ora_type_num, ORA_TYPE_NUM_JSON);
        assert_eq!(metadata.buffer_size, TNS_JSON_MAX_LENGTH);
        let mut bind_payload = Vec::new();
        write_bind_value(&mut bind_payload, &OracleThinCapabilities::default(), &bind).unwrap();
        assert!(bind_payload.starts_with(&[1, 40, 40]));
        assert_eq!(&bind_payload[3..7], &[0, 38, 0, 4]);
        assert_eq!(
            decode_oson_to_json(&bind_payload[44..]).unwrap(),
            r#"{"input":"ok"}"#
        );

        let bind = BindValue::JsonRaw(raw.to_vec());
        let metadata = bind_column_metadata(&bind);
        assert_eq!(metadata.column_type, OracleColumnType::Json);
        assert_eq!(metadata.ora_type_num, ORA_TYPE_NUM_JSON);
        assert_eq!(metadata.buffer_size, TNS_JSON_MAX_LENGTH);
        let mut bind_payload = Vec::new();
        write_bind_value(&mut bind_payload, &OracleThinCapabilities::default(), &bind).unwrap();
        assert!(bind_payload.starts_with(&[1, 40, 40]));
        assert_eq!(&bind_payload[3..7], &[0, 38, 0, 4]);
        assert_eq!(
            decode_oson_to_json(&bind_payload[44..]).unwrap(),
            r#"{"$rawhex":"41207261772076616c7565"}"#
        );

        let bind = BindValue::JsonId(json_id.to_vec());
        let metadata = bind_column_metadata(&bind);
        assert_eq!(metadata.column_type, OracleColumnType::Json);
        assert_eq!(metadata.ora_type_num, ORA_TYPE_NUM_JSON);
        assert_eq!(metadata.buffer_size, TNS_JSON_MAX_LENGTH);
        let mut bind_payload = Vec::new();
        write_bind_value(&mut bind_payload, &OracleThinCapabilities::default(), &bind).unwrap();
        assert!(bind_payload.starts_with(&[1, 40, 40]));
        assert_eq!(&bind_payload[3..7], &[0, 38, 0, 4]);
        assert_eq!(
            decode_oson_to_json(&bind_payload[44..]).unwrap(),
            r#""0123ab""#
        );

        let bind = BindValue::JsonString("String 1".to_string());
        let metadata = bind_column_metadata(&bind);
        assert_eq!(metadata.column_type, OracleColumnType::Json);
        assert_eq!(metadata.ora_type_num, ORA_TYPE_NUM_JSON);
        assert_eq!(metadata.buffer_size, TNS_JSON_MAX_LENGTH);
        let mut bind_payload = Vec::new();
        write_bind_value(&mut bind_payload, &OracleThinCapabilities::default(), &bind).unwrap();
        assert!(bind_payload.starts_with(&[1, 40, 40]));
        assert_eq!(&bind_payload[3..7], &[0, 38, 0, 4]);
        assert_eq!(
            decode_oson_to_json(&bind_payload[44..]).unwrap(),
            r#""String 1""#
        );

        let bind = BindValue::JsonNumber("25.25".to_string());
        let metadata = bind_column_metadata(&bind);
        assert_eq!(metadata.column_type, OracleColumnType::Json);
        assert_eq!(metadata.ora_type_num, ORA_TYPE_NUM_JSON);
        assert_eq!(metadata.buffer_size, TNS_JSON_MAX_LENGTH);
        let mut bind_payload = Vec::new();
        write_bind_value(&mut bind_payload, &OracleThinCapabilities::default(), &bind).unwrap();
        assert!(bind_payload.starts_with(&[1, 40, 40]));
        assert_eq!(&bind_payload[3..7], &[0, 38, 0, 4]);
        assert_eq!(decode_oson_to_json(&bind_payload[44..]).unwrap(), "25.25");

        let bind = BindValue::JsonBool(false);
        let metadata = bind_column_metadata(&bind);
        assert_eq!(metadata.column_type, OracleColumnType::Json);
        assert_eq!(metadata.ora_type_num, ORA_TYPE_NUM_JSON);
        assert_eq!(metadata.buffer_size, TNS_JSON_MAX_LENGTH);
        let mut bind_payload = Vec::new();
        write_bind_value(&mut bind_payload, &OracleThinCapabilities::default(), &bind).unwrap();
        assert!(bind_payload.starts_with(&[1, 40, 40]));
        assert_eq!(&bind_payload[3..7], &[0, 38, 0, 4]);
        assert_eq!(decode_oson_to_json(&bind_payload[44..]).unwrap(), "false");

        let bind = BindValue::JsonTimestamp(timestamp);
        let metadata = bind_column_metadata(&bind);
        assert_eq!(metadata.column_type, OracleColumnType::Json);
        assert_eq!(metadata.ora_type_num, ORA_TYPE_NUM_JSON);
        assert_eq!(metadata.buffer_size, TNS_JSON_MAX_LENGTH);
        let mut bind_payload = Vec::new();
        write_bind_value(&mut bind_payload, &OracleThinCapabilities::default(), &bind).unwrap();
        assert!(bind_payload.starts_with(&[1, 40, 40]));
        assert_eq!(&bind_payload[3..7], &[0, 38, 0, 4]);
        assert_eq!(
            decode_oson_to_json(&bind_payload[44..]).unwrap(),
            r#""2020-12-02T13:29:14.123456""#
        );

        let mut bind_payload = Vec::new();
        write_bind_value(
            &mut bind_payload,
            &OracleThinCapabilities::default(),
            &BindValue::InOut {
                column_type: OracleColumnType::Json,
                max_len: 1024,
                value: Some(BindInputValue::Bytes(raw.to_vec())),
            },
        )
        .unwrap();
        assert!(bind_payload.starts_with(&[1, 40, 40]));
        assert_eq!(&bind_payload[3..7], &[0, 38, 0, 4]);
        assert_eq!(
            decode_oson_to_json(&bind_payload[44..]).unwrap(),
            r#"{"$rawhex":"41207261772076616c7565"}"#
        );

        let mut bind_payload = Vec::new();
        write_bind_value(
            &mut bind_payload,
            &OracleThinCapabilities::default(),
            &BindValue::InOut {
                column_type: OracleColumnType::Json,
                max_len: 1024,
                value: Some(BindInputValue::Number("25.25".to_string())),
            },
        )
        .unwrap();
        assert!(bind_payload.starts_with(&[1, 40, 40]));
        assert_eq!(&bind_payload[3..7], &[0, 38, 0, 4]);
        assert_eq!(decode_oson_to_json(&bind_payload[44..]).unwrap(), "25.25");

        let mut bind_payload = Vec::new();
        write_bind_value(
            &mut bind_payload,
            &OracleThinCapabilities::default(),
            &BindValue::InOut {
                column_type: OracleColumnType::Json,
                max_len: 1024,
                value: Some(BindInputValue::Boolean(true)),
            },
        )
        .unwrap();
        assert!(bind_payload.starts_with(&[1, 40, 40]));
        assert_eq!(&bind_payload[3..7], &[0, 38, 0, 4]);
        assert_eq!(decode_oson_to_json(&bind_payload[44..]).unwrap(), "true");

        let mut bind_payload = Vec::new();
        write_bind_value(
            &mut bind_payload,
            &OracleThinCapabilities::default(),
            &BindValue::InOut {
                column_type: OracleColumnType::Json,
                max_len: 1024,
                value: Some(BindInputValue::IntervalDaySecond(interval_ds)),
            },
        )
        .unwrap();
        assert!(bind_payload.starts_with(&[1, 40, 40]));
        assert_eq!(&bind_payload[3..7], &[0, 38, 0, 4]);
        assert_eq!(
            decode_oson_to_json(&bind_payload[44..]).unwrap(),
            r#""+08 12:00:00.000000""#
        );
    }

    #[test]
    fn decodes_json_text_payload_when_legacy_ttc_returns_text_instead_of_oson() {
        assert_eq!(
            decode_json_payload(br#"{"a":1,"b":[2,"x"],"flag":true}"#).unwrap(),
            r#"{"a":1,"b":[2,"x"],"flag":true}"#
        );
    }

    #[test]
    fn row_scanner_decodes_json_columns_from_prefetched_oson() {
        let mut state = ExecuteReadState::default();
        state.columns = vec![ThinColumn {
            name: "J".to_string(),
            column_type: OracleColumnType::Json,
            ora_type_num: ORA_TYPE_NUM_JSON,
            charset_form: 0,
            buffer_size: TNS_JSON_MAX_LENGTH,
            schema_name: String::new(),
            type_name: String::new(),
        }];
        let oson = [
            0xff, 0x4a, 0x5a, 0x01, 0x00, 0x12, 0x00, 0x03, 0x02, b'o', b'k',
        ];
        let mut row = Vec::new();
        write_ub4(&mut row, oson.len() as u32);
        write_ub8(&mut row, oson.len() as u64);
        write_ub4(&mut row, oson.len() as u32);
        write_bytes_with_length_for_capabilities(
            &mut row,
            &oson,
            &OracleThinCapabilities::default(),
        )
        .unwrap();
        write_bytes_with_length_for_capabilities(
            &mut row,
            b"json-locator",
            &OracleThinCapabilities::default(),
        )
        .unwrap();
        let mut cursor = PacketCursor::with_capabilities(&row, &OracleThinCapabilities::default());

        process_row_data(&mut cursor, &OracleThinCapabilities::default(), &mut state).unwrap();

        assert_eq!(
            state.rows,
            vec![vec![OracleValue::Text("\"ok\"".to_string())]]
        );
    }

    #[test]
    fn row_scanner_decodes_flagged_djson_columns_as_json() {
        let mut state = ExecuteReadState::default();
        state.columns = vec![ThinColumn {
            name: "J".to_string(),
            column_type: oracle_column_type_from_ora_type(ORA_TYPE_NUM_DJSON),
            ora_type_num: ORA_TYPE_NUM_DJSON,
            charset_form: 0,
            buffer_size: TNS_JSON_MAX_LENGTH,
            schema_name: String::new(),
            type_name: String::new(),
        }];
        let json = br#"{"k":"v"}"#;
        let mut row = Vec::new();
        write_ub4(&mut row, json.len() as u32);
        write_ub8(&mut row, json.len() as u64);
        write_ub4(&mut row, json.len() as u32);
        write_bytes_with_length_for_capabilities(
            &mut row,
            json,
            &OracleThinCapabilities::default(),
        )
        .unwrap();
        write_bytes_with_length_for_capabilities(
            &mut row,
            b"json-locator",
            &OracleThinCapabilities::default(),
        )
        .unwrap();
        let mut cursor = PacketCursor::with_capabilities(&row, &OracleThinCapabilities::default());

        process_row_data(&mut cursor, &OracleThinCapabilities::default(), &mut state).unwrap();

        assert_eq!(
            state.rows,
            vec![vec![OracleValue::Text(r#"{"k":"v"}"#.to_string())]]
        );
    }

    #[test]
    fn row_scanner_decodes_xmltype_string_payload_as_text() {
        let mut state = ExecuteReadState::default();
        state.columns = vec![ThinColumn {
            name: "X".to_string(),
            column_type: OracleColumnType::Xml,
            ora_type_num: ORA_TYPE_NUM_OBJECT,
            charset_form: 0,
            buffer_size: TNS_MAX_LONG_LENGTH,
            schema_name: String::new(),
            type_name: String::new(),
        }];
        let mut xml_payload = vec![TNS_OBJ_NO_PREFIX_SEG, 1, 0, 1];
        xml_payload.extend_from_slice(&TNS_XML_TYPE_STRING.to_be_bytes());
        xml_payload.extend_from_slice(b"<root>ok</root>");

        let mut row = Vec::new();
        write_ub4(&mut row, 0);
        write_ub4(&mut row, 0);
        write_ub4(&mut row, 0);
        write_ub2(&mut row, 0);
        write_ub4(&mut row, xml_payload.len() as u32);
        write_ub2(&mut row, 0);
        write_bytes_with_length_for_capabilities(
            &mut row,
            &xml_payload,
            &OracleThinCapabilities::default(),
        )
        .unwrap();
        let mut cursor = PacketCursor::with_capabilities(&row, &OracleThinCapabilities::default());

        process_row_data(&mut cursor, &OracleThinCapabilities::default(), &mut state).unwrap();

        assert_eq!(
            state.rows,
            vec![vec![OracleValue::Text("<root>ok</root>".to_string())]]
        );
    }

    #[test]
    fn row_scanner_decodes_xmltype_lob_payload_as_locator() {
        let mut state = ExecuteReadState::default();
        state.columns = vec![ThinColumn {
            name: "X".to_string(),
            column_type: OracleColumnType::Xml,
            ora_type_num: ORA_TYPE_NUM_OBJECT,
            charset_form: 0,
            buffer_size: TNS_MAX_LONG_LENGTH,
            schema_name: String::new(),
            type_name: String::new(),
        }];
        let mut xml_payload = vec![TNS_OBJ_NO_PREFIX_SEG, 1, 0, 1];
        xml_payload.extend_from_slice(&TNS_XML_TYPE_LOB.to_be_bytes());
        xml_payload.extend_from_slice(b"xml-locator");

        let mut row = Vec::new();
        write_ub4(&mut row, 0);
        write_ub4(&mut row, 0);
        write_ub4(&mut row, 0);
        write_ub2(&mut row, 0);
        write_ub4(&mut row, xml_payload.len() as u32);
        write_ub2(&mut row, 0);
        write_bytes_with_length_for_capabilities(
            &mut row,
            &xml_payload,
            &OracleThinCapabilities::default(),
        )
        .unwrap();
        let mut cursor = PacketCursor::with_capabilities(&row, &OracleThinCapabilities::default());

        process_row_data(&mut cursor, &OracleThinCapabilities::default(), &mut state).unwrap();

        assert_eq!(
            state.rows,
            vec![vec![OracleValue::Lob(b"xml-locator".to_vec())]]
        );
    }

    #[test]
    fn row_scanner_decodes_degenerate_xmltype_lob_payload_as_locator() {
        let mut state = ExecuteReadState::default();
        state.columns = vec![ThinColumn {
            name: "X".to_string(),
            column_type: OracleColumnType::Xml,
            ora_type_num: ORA_TYPE_NUM_OBJECT,
            charset_form: 0,
            buffer_size: TNS_MAX_LONG_LENGTH,
            schema_name: String::new(),
            type_name: String::new(),
        }];
        let mut xml_payload = vec![TNS_OBJ_IS_DEGENERATE, 1, 0, 1];
        xml_payload.extend_from_slice(&TNS_XML_TYPE_LOB.to_be_bytes());
        xml_payload.extend_from_slice(b"degenerate-xml-locator");

        let mut row = Vec::new();
        write_ub4(&mut row, 0);
        write_ub4(&mut row, 0);
        write_ub4(&mut row, 0);
        write_ub2(&mut row, 0);
        write_ub4(&mut row, xml_payload.len() as u32);
        write_ub2(&mut row, 0);
        write_bytes_with_length_for_capabilities(
            &mut row,
            &xml_payload,
            &OracleThinCapabilities::default(),
        )
        .unwrap();
        let mut cursor = PacketCursor::with_capabilities(&row, &OracleThinCapabilities::default());

        process_row_data(&mut cursor, &OracleThinCapabilities::default(), &mut state).unwrap();

        assert_eq!(
            state.rows,
            vec![vec![OracleValue::Lob(b"degenerate-xml-locator".to_vec())]]
        );
    }

    #[test]
    fn object_payload_rejects_degenerate_lob_stored_object_like_python_oracledb() {
        let column = ThinColumn {
            name: "O".to_string(),
            column_type: OracleColumnType::Object,
            ora_type_num: ORA_TYPE_NUM_OBJECT,
            charset_form: 0,
            buffer_size: 1,
            schema_name: "APP".to_string(),
            type_name: "OBJ_T".to_string(),
        };
        let mut object_attrs_by_type = HashMap::new();
        object_attrs_by_type.insert(
            ("APP".to_string(), "OBJ_T".to_string()),
            vec![ThinColumn {
                name: "A".to_string(),
                column_type: OracleColumnType::Varchar,
                ora_type_num: ORA_TYPE_NUM_VARCHAR,
                charset_form: CS_FORM_IMPLICIT,
                buffer_size: 1,
                schema_name: String::new(),
                type_name: String::new(),
            }],
        );
        let payload = vec![TNS_OBJ_IS_DEGENERATE, 1, 0, 1];

        let err = decode_object_payload(
            &payload,
            &OracleThinCapabilities::default(),
            &column,
            &object_attrs_by_type,
            &HashMap::new(),
        )
        .expect_err("degenerate DbObject payload should not decode as Null");

        assert!(err
            .to_string()
            .contains("DbObject stored in a LOB is not supported"));
    }

    #[test]
    fn collection_payload_rejects_degenerate_lob_stored_object_like_python_oracledb() {
        let element = ThinColumn {
            name: "E".to_string(),
            column_type: OracleColumnType::Varchar,
            ora_type_num: ORA_TYPE_NUM_VARCHAR,
            charset_form: CS_FORM_IMPLICIT,
            buffer_size: 1,
            schema_name: String::new(),
            type_name: String::new(),
        };
        let payload = vec![TNS_OBJ_IS_DEGENERATE, 1, 0, 1];

        let err = decode_collection_payload(
            &payload,
            &OracleThinCapabilities::default(),
            &element,
            &HashMap::new(),
            &HashMap::new(),
        )
        .expect_err("degenerate collection payload should not decode as Null");

        assert!(err
            .to_string()
            .contains("DbObject stored in a LOB is not supported"));
    }

    #[test]
    fn collection_payload_decodes_indexed_associative_array_like_python_oracledb() {
        let element = ThinColumn {
            name: "E".to_string(),
            column_type: OracleColumnType::Varchar,
            ora_type_num: ORA_TYPE_NUM_VARCHAR,
            charset_form: CS_FORM_IMPLICIT,
            buffer_size: 1,
            schema_name: String::new(),
            type_name: String::new(),
        };
        let mut payload = vec![TNS_OBJ_NO_PREFIX_SEG, 1, 0, TNS_OBJ_HAS_INDEXES, 1];
        payload.extend_from_slice(&5_i32.to_be_bytes());
        payload.push(1);
        payload.push(b'x');

        let value = decode_collection_payload(
            &payload,
            &OracleThinCapabilities::default(),
            &element,
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap();

        assert_eq!(
            value,
            OracleValue::IndexedArray(vec![(5, OracleValue::Text("x".to_string()))])
        );
    }

    #[test]
    fn object_payload_decodes_binary_integer_attribute_like_python_oracledb() {
        let column = ThinColumn {
            name: "O".to_string(),
            column_type: OracleColumnType::Object,
            ora_type_num: ORA_TYPE_NUM_OBJECT,
            charset_form: 0,
            buffer_size: 1,
            schema_name: "APP".to_string(),
            type_name: "OBJ_T".to_string(),
        };
        let attr = ThinColumn {
            name: "BI".to_string(),
            column_type: OracleColumnType::Number,
            ora_type_num: TNS_DATA_TYPE_BINARY_INTEGER,
            charset_form: 0,
            buffer_size: 4,
            schema_name: String::new(),
            type_name: String::new(),
        };
        let mut object_attrs_by_type = HashMap::new();
        object_attrs_by_type.insert(("APP".to_string(), "OBJ_T".to_string()), vec![attr]);
        let mut payload = vec![TNS_OBJ_NO_PREFIX_SEG, 1, 0, 4];
        payload.extend_from_slice(&(-2_i32).to_be_bytes());

        let value = decode_object_payload(
            &payload,
            &OracleThinCapabilities::default(),
            &column,
            &object_attrs_by_type,
            &HashMap::new(),
        )
        .unwrap();

        assert_eq!(
            value,
            OracleValue::Object(vec![(
                "BI".to_string(),
                OracleValue::Number("-2".to_string())
            )])
        );
    }

    #[test]
    fn object_payload_decodes_boolean_attribute_nonzero_like_live_oracle() {
        let column = ThinColumn {
            name: "O".to_string(),
            column_type: OracleColumnType::Object,
            ora_type_num: ORA_TYPE_NUM_OBJECT,
            charset_form: 0,
            buffer_size: 1,
            schema_name: "APP".to_string(),
            type_name: "OBJ_T".to_string(),
        };
        let attrs = vec![
            ThinColumn {
                name: "LEADING_TRUE".to_string(),
                column_type: OracleColumnType::Boolean,
                ora_type_num: ORA_TYPE_NUM_BOOLEAN,
                charset_form: 0,
                buffer_size: 4,
                schema_name: String::new(),
                type_name: String::new(),
            },
            ThinColumn {
                name: "TRUE_VALUE".to_string(),
                column_type: OracleColumnType::Boolean,
                ora_type_num: ORA_TYPE_NUM_BOOLEAN,
                charset_form: 0,
                buffer_size: 4,
                schema_name: String::new(),
                type_name: String::new(),
            },
        ];
        let mut object_attrs_by_type = HashMap::new();
        object_attrs_by_type.insert(("APP".to_string(), "OBJ_T".to_string()), attrs);
        let mut payload = vec![TNS_OBJ_NO_PREFIX_SEG, 1, 0, 4];
        payload.extend_from_slice(&[1, 0, 0, 0]);
        payload.push(4);
        payload.extend_from_slice(&[0, 0, 0, 1]);

        let value = decode_object_payload(
            &payload,
            &OracleThinCapabilities::default(),
            &column,
            &object_attrs_by_type,
            &HashMap::new(),
        )
        .unwrap();

        assert_eq!(
            value,
            OracleValue::Object(vec![
                ("LEADING_TRUE".to_string(), OracleValue::Boolean(true)),
                ("TRUE_VALUE".to_string(), OracleValue::Boolean(true))
            ])
        );
    }

    #[test]
    fn object_payload_decodes_long_attribute_like_python_oracledb() {
        let column = ThinColumn {
            name: "O".to_string(),
            column_type: OracleColumnType::Object,
            ora_type_num: ORA_TYPE_NUM_OBJECT,
            charset_form: 0,
            buffer_size: 1,
            schema_name: "APP".to_string(),
            type_name: "OBJ_T".to_string(),
        };
        let attr = ThinColumn {
            name: "PAYLOAD".to_string(),
            column_type: OracleColumnType::Long,
            ora_type_num: ORA_TYPE_NUM_LONG,
            charset_form: CS_FORM_IMPLICIT,
            buffer_size: TNS_MAX_LONG_LENGTH,
            schema_name: String::new(),
            type_name: String::new(),
        };
        let mut object_attrs_by_type = HashMap::new();
        object_attrs_by_type.insert(("APP".to_string(), "OBJ_T".to_string()), vec![attr]);
        let mut payload = vec![TNS_OBJ_NO_PREFIX_SEG, 1, 0, 12];
        payload.extend_from_slice(b"long text ok");

        let value = decode_object_payload(
            &payload,
            &OracleThinCapabilities::default(),
            &column,
            &object_attrs_by_type,
            &HashMap::new(),
        )
        .unwrap();

        assert_eq!(
            value,
            OracleValue::Object(vec![(
                "PAYLOAD".to_string(),
                OracleValue::Text("long text ok".to_string())
            )])
        );
    }

    #[test]
    fn object_payload_decodes_long_nvarchar_attribute_like_python_oracledb() {
        let column = ThinColumn {
            name: "O".to_string(),
            column_type: OracleColumnType::Object,
            ora_type_num: ORA_TYPE_NUM_OBJECT,
            charset_form: 0,
            buffer_size: 1,
            schema_name: "APP".to_string(),
            type_name: "OBJ_T".to_string(),
        };
        let attr = thin_column_from_object_attr(
            "PAYLOAD".to_string(),
            "LONG NVARCHAR".to_string(),
            String::new(),
            None,
            0,
            CS_FORM_NCHAR,
        )
        .unwrap();
        let mut object_attrs_by_type = HashMap::new();
        object_attrs_by_type.insert(("APP".to_string(), "OBJ_T".to_string()), vec![attr]);
        let mut payload = vec![TNS_OBJ_NO_PREFIX_SEG, 1, 0, 4];
        payload.extend_from_slice(&[0xd5, 0x5c, 0xae, 0x00]);

        let value = decode_object_payload(
            &payload,
            &OracleThinCapabilities::default(),
            &column,
            &object_attrs_by_type,
            &HashMap::new(),
        )
        .unwrap();

        assert_eq!(
            value,
            OracleValue::Object(vec![(
                "PAYLOAD".to_string(),
                OracleValue::Text("한글".to_string())
            )])
        );
    }

    #[test]
    fn object_payload_decodes_long_raw_attribute_like_python_oracledb() {
        let column = ThinColumn {
            name: "O".to_string(),
            column_type: OracleColumnType::Object,
            ora_type_num: ORA_TYPE_NUM_OBJECT,
            charset_form: 0,
            buffer_size: 1,
            schema_name: "APP".to_string(),
            type_name: "OBJ_T".to_string(),
        };
        let attr = ThinColumn {
            name: "PAYLOAD".to_string(),
            column_type: OracleColumnType::Raw,
            ora_type_num: ORA_TYPE_NUM_LONG_RAW,
            charset_form: 0,
            buffer_size: TNS_MAX_LONG_LENGTH,
            schema_name: String::new(),
            type_name: String::new(),
        };
        let mut object_attrs_by_type = HashMap::new();
        object_attrs_by_type.insert(("APP".to_string(), "OBJ_T".to_string()), vec![attr]);
        let mut payload = vec![TNS_OBJ_NO_PREFIX_SEG, 1, 0, 4];
        payload.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);

        let value = decode_object_payload(
            &payload,
            &OracleThinCapabilities::default(),
            &column,
            &object_attrs_by_type,
            &HashMap::new(),
        )
        .unwrap();

        assert_eq!(
            value,
            OracleValue::Object(vec![(
                "PAYLOAD".to_string(),
                OracleValue::Bytes(vec![0xde, 0xad, 0xbe, 0xef])
            )])
        );
    }

    #[test]
    fn nested_object_payload_keeps_null_first_attribute_as_child_object_like_python_oracledb() {
        let column = ThinColumn {
            name: "P".to_string(),
            column_type: OracleColumnType::Object,
            ora_type_num: ORA_TYPE_NUM_OBJECT,
            charset_form: 0,
            buffer_size: 1,
            schema_name: "APP".to_string(),
            type_name: "PARENT_T".to_string(),
        };
        let child_attr = ThinColumn {
            name: "CHILD".to_string(),
            column_type: OracleColumnType::Object,
            ora_type_num: ORA_TYPE_NUM_OBJECT,
            charset_form: 0,
            buffer_size: 1,
            schema_name: "APP".to_string(),
            type_name: "CHILD_T".to_string(),
        };
        let child_value_attr = ThinColumn {
            name: "A".to_string(),
            column_type: OracleColumnType::Varchar,
            ora_type_num: ORA_TYPE_NUM_VARCHAR,
            charset_form: CS_FORM_IMPLICIT,
            buffer_size: 1,
            schema_name: String::new(),
            type_name: String::new(),
        };
        let mut object_attrs_by_type = HashMap::new();
        object_attrs_by_type.insert(
            ("APP".to_string(), "PARENT_T".to_string()),
            vec![child_attr],
        );
        object_attrs_by_type.insert(
            ("APP".to_string(), "CHILD_T".to_string()),
            vec![child_value_attr],
        );
        let payload = vec![TNS_OBJ_NO_PREFIX_SEG, 1, 0, 0xff];

        let value = decode_object_payload(
            &payload,
            &OracleThinCapabilities::default(),
            &column,
            &object_attrs_by_type,
            &HashMap::new(),
        )
        .unwrap();

        assert_eq!(
            value,
            OracleValue::Object(vec![(
                "CHILD".to_string(),
                OracleValue::Object(vec![("A".to_string(), OracleValue::Null)])
            )])
        );
    }

    #[test]
    fn collection_payload_decodes_binary_integer_elements_like_python_oracledb() {
        let element = ThinColumn {
            name: "E".to_string(),
            column_type: OracleColumnType::Number,
            ora_type_num: TNS_DATA_TYPE_BINARY_INTEGER,
            charset_form: 0,
            buffer_size: 4,
            schema_name: String::new(),
            type_name: String::new(),
        };
        let mut payload = vec![TNS_OBJ_NO_PREFIX_SEG, 1, 0, 0, 2];
        payload.push(4);
        payload.extend_from_slice(&42_i32.to_be_bytes());
        payload.push(4);
        payload.extend_from_slice(&(-2_i32).to_be_bytes());

        let value = decode_collection_payload(
            &payload,
            &OracleThinCapabilities::default(),
            &element,
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap();

        assert_eq!(
            value,
            OracleValue::Array(vec![
                OracleValue::Number("42".to_string()),
                OracleValue::Number("-2".to_string())
            ])
        );
    }

    #[test]
    fn row_scanner_decodes_bfile_columns_as_locator_lobs() {
        let mut state = ExecuteReadState::default();
        state.columns = vec![ThinColumn {
            name: "BF".to_string(),
            column_type: OracleColumnType::Bfile,
            ora_type_num: ORA_TYPE_NUM_BFILE,
            charset_form: 0,
            buffer_size: 1,
            schema_name: String::new(),
            type_name: String::new(),
        }];
        let mut row = Vec::new();
        write_ub4(&mut row, 1);
        write_bytes_with_length_for_capabilities(
            &mut row,
            b"bfile-locator",
            &OracleThinCapabilities::default(),
        )
        .unwrap();
        let mut cursor = PacketCursor::with_capabilities(&row, &OracleThinCapabilities::default());

        process_row_data(&mut cursor, &OracleThinCapabilities::default(), &mut state).unwrap();

        assert_eq!(
            state.rows,
            vec![vec![OracleValue::Lob(b"bfile-locator".to_vec())]]
        );
    }

    #[test]
    fn row_scanner_decodes_vector_columns_as_text() {
        let mut state = ExecuteReadState::default();
        state.columns = vec![ThinColumn {
            name: "V".to_string(),
            column_type: OracleColumnType::Vector,
            ora_type_num: ORA_TYPE_NUM_VECTOR,
            charset_form: 0,
            buffer_size: 1,
            schema_name: String::new(),
            type_name: String::new(),
        }];
        let vector = [
            0xdb, 0, 0, 0x12, 2, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 195, 6, 115, 51, 60, 249, 140,
            204,
        ];
        let mut row = Vec::new();
        write_ub4(&mut row, 1);
        write_ub8(&mut row, vector.len() as u64);
        write_ub4(&mut row, vector.len() as u32);
        write_bytes_with_length_for_capabilities(
            &mut row,
            &vector,
            &OracleThinCapabilities::default(),
        )
        .unwrap();
        write_bytes_with_length_for_capabilities(
            &mut row,
            b"vector-locator",
            &OracleThinCapabilities::default(),
        )
        .unwrap();
        let mut cursor = PacketCursor::with_capabilities(&row, &OracleThinCapabilities::default());

        process_row_data(&mut cursor, &OracleThinCapabilities::default(), &mut state).unwrap();

        assert_eq!(
            state.rows,
            vec![vec![OracleValue::Text("[134.45, -134.45]".to_string())]]
        );
    }

    #[test]
    fn modern_protocols_start_with_python_oracledb_max_ttc_field_version() {
        assert_eq!(default_ttc_field_version(314), 6);
        assert_eq!(default_ttc_field_version(315), 24);
        assert_eq!(default_ttc_field_version(318), 24);
        assert_eq!(default_ttc_field_version(319), 24);
    }

    #[test]
    fn client_capabilities_keep_python_oracledb_314_plus_defaults() {
        let mut capabilities = OracleThinCapabilities::default();
        capabilities.ttc_field_version = default_ttc_field_version(319);
        capabilities.supports_end_of_response = true;

        let compile_caps = client_compile_caps(&capabilities).unwrap();
        assert_eq!(compile_caps.len(), 53);
        assert_eq!(
            compile_caps[TNS_CCAP_FIELD_VERSION],
            default_ttc_field_version(319)
        );
        assert_eq!(
            compile_caps[TNS_CCAP_TTC1],
            TNS_CCAP_END_OF_CALL_STATUS | 0x08 | 0x20
        );
        assert_eq!(
            compile_caps[TNS_CCAP_TTC4],
            0x04 | TNS_CCAP_EXPLICIT_BOUNDARY | TNS_CCAP_END_OF_RESPONSE
        );
        assert_eq!(compile_caps[23], 0x01 | 0x02 | 0x04 | 0x08 | 0x40 | 0x80);
        assert_eq!(compile_caps[37], 0x08 | 0x10 | 0x20 | 0x80);
        assert_eq!(compile_caps[44], 0x02 | 0x04 | 0x08 | 0x10 | 0x20);
        assert_eq!(compile_caps[52], 0x01 | 0x02);

        let runtime_caps = client_runtime_caps();
        assert_eq!(runtime_caps.len(), 11);
        assert_eq!(
            runtime_caps[TNS_RCAP_TTC],
            TNS_RCAP_TTC_ZERO_COPY | TNS_RCAP_TTC_32K
        );
    }

    #[test]
    fn protocol_below_314_is_rejected_explicitly() {
        let accept = AcceptInfo {
            protocol_version: 313,
            protocol_options: 0,
            sdu: 8192,
            supports_full_packet_size: true,
            flags2: 0,
        };

        let error =
            validate_supported_protocol(&accept).expect_err("protocol 313 should be rejected");

        assert!(
            error.to_string().contains("protocol 314 and newer"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn protocol_314_sql_boolean_threshold_remains_legacy() {
        let options = ConnectOptions {
            desired_ttc_field_version: Some(20),
            ..ConnectOptions::default()
        };
        let accept = AcceptInfo {
            protocol_version: 314,
            protocol_options: 0,
            sdu: 8192,
            supports_full_packet_size: true,
            flags2: 0,
        };
        let caps = capabilities_from_accept(&options, &accept);
        assert!(!caps.supports_sql_boolean);

        let accept = AcceptInfo {
            protocol_version: 315,
            ..accept
        };
        let caps = capabilities_from_accept(&options, &accept);
        assert!(caps.supports_sql_boolean);
    }

    #[test]
    fn implicit_resultsets_start_at_protocol_315() {
        let options = ConnectOptions::default();
        let accept = AcceptInfo {
            protocol_version: 314,
            protocol_options: 0,
            sdu: 8192,
            supports_full_packet_size: true,
            flags2: 0,
        };
        let caps = capabilities_from_accept(&options, &accept);
        assert!(!caps.supports_implicit_resultsets);

        let accept = AcceptInfo {
            protocol_version: 315,
            ..accept
        };
        let caps = capabilities_from_accept(&options, &accept);
        assert!(caps.supports_implicit_resultsets);
    }

    #[test]
    fn accepted_sdu_controls_data_packet_chunk_size() {
        let accept = AcceptInfo {
            protocol_version: 319,
            protocol_options: 0,
            sdu: 16_384,
            supports_full_packet_size: true,
            flags2: 0,
        };
        let caps = capabilities_from_accept(&ConnectOptions::default(), &accept);

        assert_eq!(caps.data_packet_chunk_size(), 16_384 - 64);
    }

    #[test]
    fn oob_probe_requires_attention_and_check_flag() {
        let options = ConnectOptions {
            disable_oob_probe: false,
            ..ConnectOptions::default()
        };
        let accept = AcceptInfo {
            protocol_version: 319,
            protocol_options: 0x0400,
            sdu: 8192,
            supports_full_packet_size: true,
            flags2: 0,
        };
        let caps = capabilities_from_accept(&options, &accept);
        assert!(caps.supports_oob);
        assert!(!caps.supports_oob_check);

        let accept = AcceptInfo {
            flags2: 0x0000_0001,
            ..accept
        };
        let caps = capabilities_from_accept(&options, &accept);
        assert!(caps.supports_oob);
        assert!(caps.supports_oob_check);
    }

    #[test]
    fn auth_requires_server_response_after_generating_combo_key() {
        let state = AuthState {
            combo_key: Some(vec![0; 32]),
            ..AuthState::default()
        };

        let err =
            verify_server_response(&state).expect_err("missing AUTH_SVR_RESPONSE should fail auth");

        assert!(err.to_string().contains("server response"));
    }

    #[test]
    fn auth_11g_password_hash_matches_vendor_algorithm() {
        let password_hash = generate_11g_password_hash(b"password", &[1, 2, 3, 4]);

        assert_eq!(
            hex_encode_upper(&password_hash),
            "A2E51942D15B86442B7C8278DEAC75EFA59F8F8600000000"
        );
    }

    #[test]
    fn auth_legacy_des_block_matches_standard_vector() {
        let encrypted = des_encrypt_block(
            [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef],
            [0x13, 0x34, 0x57, 0x79, 0x9b, 0xbc, 0xdf, 0xf1],
        );

        assert_eq!(hex_encode_upper(&encrypted), "85E813540F0AB405");
    }

    #[test]
    fn auth_10g_password_hash_matches_go_ora_legacy_algorithm() {
        let password_hash = generate_10g_password_hash("scott", "tiger");

        assert_eq!(
            hex_encode_upper(&password_hash),
            "F894844C34402B670000000000000000"
        );
    }

    #[test]
    fn auth_48_byte_session_key_uses_md5_xor_combo_key() {
        let state = AuthState::default();
        let session_key_part_a = (0u8..48).collect::<Vec<_>>();
        let session_key_part_b = (0u8..48).rev().collect::<Vec<_>>();

        let combo_11g =
            derive_auth_combo_key(&state, &session_key_part_a, &session_key_part_b, 24).unwrap();
        assert_eq!(
            hex_encode_upper(&combo_11g),
            "1C011309F405946F1DF826604313B4E6E8E4E5B0B909CFE7"
        );

        let combo_12c =
            derive_auth_combo_key(&state, &session_key_part_a, &session_key_part_b, 32).unwrap();
        assert_eq!(
            hex_encode_upper(&combo_12c),
            "1C011309F405946F1DF826604313B4E6E8E4E5B0B909CFE76053A79413131711"
        );
    }

    #[test]
    fn auth_10g_legacy_32_byte_session_key_uses_go_ora_md5_xor_combo_key() {
        let state = AuthState {
            verifier_type: TNS_VERIFIER_TYPE_10G,
            ..AuthState::default()
        };
        let session_key_part_a = (0u8..32).collect::<Vec<_>>();
        let session_key_part_b = (0u8..32).rev().collect::<Vec<_>>();

        let combo_key =
            derive_auth_combo_key(&state, &session_key_part_a, &session_key_part_b, 16).unwrap();

        assert_eq!(
            hex_encode_upper(&combo_key),
            "6A1E2C6EB1B7AA9C4380252AEA8C215E"
        );
    }

    #[test]
    fn auth_combo_key_uses_server_compile_cap_pbkdf2_bit() {
        let mut caps = OracleThinCapabilities::default();
        let mut compile_caps = vec![0u8; 8];
        adjust_for_server_compile_caps(&mut caps, &compile_caps);
        assert!(!caps.auth_uses_pbkdf2_key_derivation);

        compile_caps[4] = 0x20;
        adjust_for_server_compile_caps(&mut caps, &compile_caps);
        assert!(caps.auth_uses_pbkdf2_key_derivation);
    }

    #[test]
    fn auth_client_driver_name_uses_crate_version_like_python_oracledb_metadata() {
        assert_eq!(
            oracle_thin_driver_name(),
            format!("space-query-thin thn : {}", env!("CARGO_PKG_VERSION"))
        );
    }

    fn read_auth_key_values(
        payload: &[u8],
        capabilities: &OracleThinCapabilities,
        expected_function_code: u8,
        expected_sequence: u8,
    ) -> (u32, Vec<(String, String, u32)>) {
        let mut cursor = PacketCursor::with_capabilities(payload, capabilities);
        assert_eq!(cursor.read_u8().unwrap(), TNS_MSG_TYPE_FUNCTION);
        assert_eq!(cursor.read_u8().unwrap(), expected_function_code);
        assert_eq!(cursor.read_u8().unwrap(), expected_sequence);
        if capabilities.ttc_field_version >= TNS_CCAP_FIELD_VERSION_23_1_EXT_1 {
            assert_eq!(cursor.read_ub8().unwrap(), 0);
        }
        assert_eq!(cursor.read_u8().unwrap(), 1);
        let user_len = cursor.read_ub4().unwrap();
        let auth_mode = cursor.read_ub4().unwrap();
        assert_eq!(cursor.read_u8().unwrap(), 1);
        let num_pairs = cursor.read_ub4().unwrap();
        assert_eq!(cursor.read_u8().unwrap(), 1);
        assert_eq!(cursor.read_u8().unwrap(), 1);
        let user = cursor.read_bytes().unwrap().unwrap();
        assert_eq!(user_len, user.len() as u32);

        let mut pairs = Vec::new();
        for _ in 0..num_pairs {
            let key_len = cursor.read_ub4().unwrap();
            let key = cursor.read_bytes().unwrap().unwrap();
            assert_eq!(key_len, key.len() as u32);
            let value_len = cursor.read_ub4().unwrap();
            let value = cursor.read_bytes().unwrap().unwrap();
            assert_eq!(value_len, value.len() as u32);
            let flags = cursor.read_ub4().unwrap();
            pairs.push((
                String::from_utf8(key).unwrap(),
                String::from_utf8(value).unwrap(),
                flags,
            ));
        }
        assert_eq!(cursor.remaining(), 0);
        (auth_mode, pairs)
    }

    #[test]
    fn function_code_writes_token_slot_starting_at_python_oracledb_23_1_ext1() {
        let mut before_caps = OracleThinCapabilities {
            ttc_field_version: TNS_CCAP_FIELD_VERSION_23_1,
            ..OracleThinCapabilities::default()
        };
        let mut payload = Vec::new();
        write_function_code(&mut payload, TNS_FUNC_PING, 7, &before_caps);
        assert_eq!(payload, vec![TNS_MSG_TYPE_FUNCTION, TNS_FUNC_PING, 7]);

        before_caps.ttc_field_version = TNS_CCAP_FIELD_VERSION_23_1_EXT_1;
        let mut payload = Vec::new();
        write_function_code(&mut payload, TNS_FUNC_PING, 7, &before_caps);

        assert_eq!(payload.len(), 4);
        assert_eq!(&payload[..3], &[TNS_MSG_TYPE_FUNCTION, TNS_FUNC_PING, 7]);
        assert_eq!(payload[3], 0);
    }

    #[test]
    fn auth_phase_one_writes_terminal_and_process_metadata_like_python_oracledb() {
        let mut config = OracleThinConfig::new(
            ConnectTarget::service_name("dbhost", 1521, "XE"),
            "system",
            "",
        );
        config.terminal = "tty001".to_string();
        config.program = "space-query".to_string();
        config.machine = "devhost".to_string();
        config.os_user = "iceblue".to_string();

        let payload = auth_phase_one_payload(&config, &OracleThinCapabilities::default()).unwrap();
        let (auth_mode, pairs) = read_auth_key_values(
            &payload,
            &OracleThinCapabilities::default(),
            TNS_FUNC_AUTH_PHASE_ONE,
            1,
        );

        assert_eq!(auth_mode, TNS_AUTH_MODE_LOGON);
        assert_eq!(
            pairs,
            vec![
                ("AUTH_TERMINAL".to_string(), "tty001".to_string(), 0),
                ("AUTH_PROGRAM_NM".to_string(), "space-query".to_string(), 0),
                ("AUTH_MACHINE".to_string(), "devhost".to_string(), 0),
                ("AUTH_PID".to_string(), std::process::id().to_string(), 0),
                ("AUTH_SID".to_string(), "iceblue".to_string(), 0),
            ]
        );
    }

    #[test]
    fn config_new_parses_proxy_user_suffix_like_python_oracledb() {
        let config = OracleThinConfig::new(
            ConnectTarget::service_name("dbhost", 1521, "FREEPDB1"),
            "app_user[real_user]",
            "password",
        );

        assert_eq!(config.username, "app_user");
        assert_eq!(config.proxy_user.as_deref(), Some("real_user"));
    }

    #[test]
    fn auth_payload_uses_parsed_proxy_user_suffix_like_python_oracledb() {
        let config = OracleThinConfig::new(
            ConnectTarget::service_name("dbhost", 1521, "FREEPDB1"),
            "app_user[real_user]",
            "password",
        );
        let credentials = AuthCredentials {
            session_key: "session-key".to_string(),
            speedy_key: Some("speedy-key".to_string()),
            password: "encoded-password".to_string(),
            debug_jdwp_data: None,
        };

        let payload =
            auth_phase_two_payload(&config, &OracleThinCapabilities::default(), &credentials)
                .unwrap();
        let (auth_mode, pairs) = read_auth_key_values(
            &payload,
            &OracleThinCapabilities::default(),
            TNS_FUNC_AUTH_PHASE_TWO,
            2,
        );

        assert_eq!(auth_mode, TNS_AUTH_MODE_LOGON | TNS_AUTH_MODE_WITH_PASSWORD);
        assert_eq!(config.username, "app_user");
        assert_eq!(
            pairs
                .iter()
                .find(|(key, _, _)| key == "PROXY_CLIENT_NAME")
                .map(|(_, value, flags)| (value.as_str(), *flags)),
            Some(("real_user", 0))
        );
    }

    #[test]
    fn auth_phase_two_writes_optional_connection_metadata_like_python_oracledb() {
        let mut config = OracleThinConfig::new(
            ConnectTarget::service_name("dbhost", 1521, "FREEPDB1"),
            "system",
            "password",
        );
        config.proxy_user = Some("app_proxy".to_string());
        config.edition = Some("ora$base".to_string());
        config.connection_class = Some("SPACE_QUERY".to_string());
        config.purity = OracleThinPurity::SelfConnection;
        config.driver_name = Some("custom thin driver".to_string());
        config.app_context.push(OracleThinAppContext::new(
            "SPACE_CTX",
            "tenant_id",
            "tenant-42",
        ));
        let credentials = AuthCredentials {
            session_key: "session-key".to_string(),
            speedy_key: Some("speedy-key".to_string()),
            password: "encoded-password".to_string(),
            debug_jdwp_data: Some("JDWPHEX01".to_string()),
        };

        let payload =
            auth_phase_two_payload(&config, &OracleThinCapabilities::default(), &credentials)
                .unwrap();
        let (auth_mode, pairs) = read_auth_key_values(
            &payload,
            &OracleThinCapabilities::default(),
            TNS_FUNC_AUTH_PHASE_TWO,
            2,
        );

        assert_eq!(auth_mode, TNS_AUTH_MODE_LOGON | TNS_AUTH_MODE_WITH_PASSWORD);
        assert_eq!(
            pairs,
            vec![
                (
                    "PROXY_CLIENT_NAME".to_string(),
                    "app_proxy".to_string(),
                    0
                ),
                ("AUTH_SESSKEY".to_string(), "session-key".to_string(), 1),
                (
                    "AUTH_PBKDF2_SPEEDY_KEY".to_string(),
                    "speedy-key".to_string(),
                    0
                ),
                (
                    "AUTH_PASSWORD".to_string(),
                    "encoded-password".to_string(),
                    0
                ),
                ("SESSION_CLIENT_CHARSET".to_string(), "873".to_string(), 0),
                (
                    "SESSION_CLIENT_DRIVER_NAME".to_string(),
                    "custom thin driver".to_string(),
                    0
                ),
                ("SESSION_CLIENT_VERSION".to_string(), "0".to_string(), 0),
                (
                    "AUTH_ALTER_SESSION".to_string(),
                    alter_session_timezone_statement(),
                    1
                ),
                (
                    "AUTH_KPPL_CONN_CLASS".to_string(),
                    "SPACE_QUERY".to_string(),
                    0
                ),
                ("AUTH_KPPL_PURITY".to_string(), "2".to_string(), 1),
                (
                    "AUTH_ORA_EDITION".to_string(),
                    "ora$base".to_string(),
                    0
                ),
                (
                    "AUTH_APPCTX_NSPACE\0".to_string(),
                    "SPACE_CTX".to_string(),
                    0
                ),
                (
                    "AUTH_APPCTX_ATTR\0".to_string(),
                    "tenant_id".to_string(),
                    0
                ),
                (
                    "AUTH_APPCTX_VALUE\0".to_string(),
                    "tenant-42".to_string(),
                    0
                ),
                (
                    "AUTH_ORA_DEBUG_JDWP".to_string(),
                    "JDWPHEX01".to_string(),
                    0
                ),
                (
                    "AUTH_CONNECT_STRING".to_string(),
                    "(DESCRIPTION=(ADDRESS=(PROTOCOL=tcp)(HOST=dbhost)(PORT=1521))(CONNECT_DATA=(SERVICE_NAME=FREEPDB1)(CID=(PROGRAM=space-query-thin)(HOST=localhost)(USER=space-query))))".to_string(),
                    0
                ),
            ]
        );
    }

    #[test]
    fn auth_connect_string_rejects_descriptor_injection_characters() {
        let mut config = OracleThinConfig::new(
            ConnectTarget::service_name("dbhost", 1521, "FREEPDB1"),
            "system",
            "password",
        );
        config.program = "space-query)(SERVER=shared".to_string();
        let credentials = AuthCredentials {
            session_key: "session-key".to_string(),
            speedy_key: None,
            password: "encoded-password".to_string(),
            debug_jdwp_data: None,
        };

        let err = auth_phase_two_payload(&config, &OracleThinCapabilities::default(), &credentials)
            .expect_err("invalid program should fail before writing auth connect string");

        assert!(err.to_string().contains("program"));
    }

    #[test]
    fn auth_connect_string_includes_description_options_like_python_oracledb() {
        let mut config = OracleThinConfig::new(
            ConnectTarget::service_name("dbhost", 1521, "FREEPDB1"),
            "system",
            "password",
        );
        config.connect_options.expire_time = 10;
        config.connect_options.tcp_connect_timeout = Duration::from_millis(1500);
        config.connect_options.sdu = 131_072;

        let connect_string = auth_connect_string(&config).unwrap();

        assert!(connect_string.starts_with(
            "(DESCRIPTION=(EXPIRE_TIME=10)\
             (TRANSPORT_CONNECT_TIMEOUT=1500ms)(SDU=131072)"
        ));
        assert!(!connect_string.contains("RETRY_COUNT="));
        assert!(connect_string.contains("(CONNECT_DATA=(SERVICE_NAME=FREEPDB1)(CID="));
    }

    #[test]
    fn auth_connect_string_keeps_connection_id_out_of_auth_payload() {
        let mut config = OracleThinConfig::new(
            ConnectTarget::service_name("dbhost", 1521, "FREEPDB1"),
            "system",
            "password",
        );
        config.connect_options.connection_id = Some("abc123".to_string());
        config.connect_options.connection_id_prefix = Some("space-".to_string());

        let connect_string = auth_connect_string(&config).unwrap();

        assert!(connect_string.contains(
            "(CONNECT_DATA=(SERVICE_NAME=FREEPDB1)\
             (CID=(PROGRAM=space-query-thin)(HOST=localhost)(USER=space-query)))"
        ));
        assert!(!connect_string.contains("CONNECTION_ID="));
    }

    #[test]
    fn modern_auth_connect_string_prefers_instance_over_sid_like_python_oracledb() {
        let config = OracleThinConfig::new(
            ConnectTarget::sid("dbhost", 1521, "ORCL")
                .with_instance_name("inst1")
                .with_server_type(OracleNetServerType::Pooled),
            "system",
            "password",
        );
        let credentials = AuthCredentials {
            session_key: "session-key".to_string(),
            speedy_key: None,
            password: "encoded-password".to_string(),
            debug_jdwp_data: None,
        };

        let payload =
            auth_phase_two_payload(&config, &OracleThinCapabilities::default(), &credentials)
                .unwrap();
        let (_, pairs) = read_auth_key_values(
            &payload,
            &OracleThinCapabilities::default(),
            TNS_FUNC_AUTH_PHASE_TWO,
            2,
        );
        let (_, connect_string, _) = pairs
            .into_iter()
            .find(|(key, _, _)| key == "AUTH_CONNECT_STRING")
            .expect("AUTH_CONNECT_STRING should be present");

        assert!(connect_string.contains("(INSTANCE_NAME=inst1)"));
        assert!(connect_string.contains("(SERVER=pooled)"));
        assert!(!connect_string.contains("(SID=ORCL)"));
        assert!(!connect_string.contains("SERVICE_NAME="));
    }

    #[test]
    fn protocol_314_auth_connect_string_keeps_sid_and_instance_like_go_ora() {
        let mut config = OracleThinConfig::new(
            ConnectTarget::sid("dbhost", 1521, "ORCL")
                .with_instance_name("inst1")
                .with_server_type(OracleNetServerType::Pooled),
            "system",
            "password",
        );
        config.connect_options.desired_protocol_version = 314;
        let credentials = AuthCredentials {
            session_key: "session-key".to_string(),
            speedy_key: None,
            password: "encoded-password".to_string(),
            debug_jdwp_data: None,
        };

        let payload =
            auth_phase_two_payload(&config, &OracleThinCapabilities::default(), &credentials)
                .unwrap();
        let (_, pairs) = read_auth_key_values(
            &payload,
            &OracleThinCapabilities::default(),
            TNS_FUNC_AUTH_PHASE_TWO,
            2,
        );
        let (_, connect_string, _) = pairs
            .into_iter()
            .find(|(key, _, _)| key == "AUTH_CONNECT_STRING")
            .expect("AUTH_CONNECT_STRING should be present");

        assert!(connect_string.contains("(SID=ORCL)"));
        assert!(connect_string.contains("(INSTANCE_NAME=inst1)"));
        assert!(connect_string.contains("(SERVER=pooled)"));
        assert!(!connect_string.contains("SERVICE_NAME="));
    }

    #[test]
    fn auth_change_password_payload_matches_python_oracledb_wire_shape() {
        let combo_key = (0u8..16).collect::<Vec<_>>();
        let salt: [u8; 16] = (16u8..32).collect::<Vec<_>>().try_into().unwrap();

        assert_eq!(
            encode_auth_password(&combo_key, b"old-password", &salt).unwrap(),
            "07FEEF74E1D5036E900EEE118E949293A3451F5DF2C27E7F102F3647E8D21800"
        );
        assert_eq!(
            encode_auth_password(&combo_key, b"new-password", &salt).unwrap(),
            "07FEEF74E1D5036E900EEE118E9492932B5C4F512DAC2BBF74BAA0FF8B09425F"
        );

        let payload = auth_change_password_payload(
            "system",
            "old-password",
            "new-password",
            &combo_key,
            &salt,
            &OracleThinCapabilities::default(),
            7,
        )
        .unwrap();
        let (auth_mode, pairs) = read_auth_key_values(
            &payload,
            &OracleThinCapabilities::default(),
            TNS_FUNC_AUTH_PHASE_TWO,
            7,
        );

        assert_eq!(
            auth_mode,
            TNS_AUTH_MODE_WITH_PASSWORD | TNS_AUTH_MODE_CHANGE_PASSWORD
        );
        assert_eq!(
            pairs,
            vec![
                (
                    "AUTH_PASSWORD".to_string(),
                    "07FEEF74E1D5036E900EEE118E949293A3451F5DF2C27E7F102F3647E8D21800".to_string(),
                    0
                ),
                (
                    "AUTH_NEWPASSWORD".to_string(),
                    "07FEEF74E1D5036E900EEE118E9492932B5C4F512DAC2BBF74BAA0FF8B09425F".to_string(),
                    0
                ),
            ]
        );
    }

    #[test]
    fn auth_debug_jdwp_uses_python_oracledb_zero_padding_and_aes_marker() {
        let combo_key = (0u8..16).collect::<Vec<_>>();

        let encoded = encode_debug_jdwp_data(Some("host=127.0.0.1;port=4000"), &combo_key).unwrap();

        assert_eq!(
            encoded.as_deref(),
            Some("25221BA4D3ECB5E62FEA070A1DB49A1CF8169B61FBA147DD978AA340D093CE2E01")
        );
    }

    #[test]
    fn privileged_auth_modes_use_python_oracledb_tns_bits() {
        for (auth_mode, expected_bits) in [
            (OracleThinAuthMode::Default, 0),
            (OracleThinAuthMode::SysDba, TNS_AUTH_MODE_SYSDBA),
            (OracleThinAuthMode::SysOper, TNS_AUTH_MODE_SYSOPER),
            (OracleThinAuthMode::SysAsm, TNS_AUTH_MODE_SYSASM),
            (OracleThinAuthMode::SysBkp, TNS_AUTH_MODE_SYSBKP),
            (OracleThinAuthMode::SysDgd, TNS_AUTH_MODE_SYSDGD),
            (OracleThinAuthMode::SysKmt, TNS_AUTH_MODE_SYSKMT),
            (OracleThinAuthMode::SysRac, TNS_AUTH_MODE_SYSRAC),
        ] {
            let mut phase_one_payload = Vec::new();
            write_auth_header(
                &mut phase_one_payload,
                "system",
                TNS_AUTH_MODE_LOGON | auth_mode.tns_bits(),
                5,
            )
            .unwrap();
            let mut cursor = PacketCursor::with_capabilities(
                &phase_one_payload,
                &OracleThinCapabilities::default(),
            );
            assert_eq!(cursor.read_u8().unwrap(), 1);
            assert_eq!(cursor.read_ub4().unwrap(), 6);
            assert_eq!(
                cursor.read_ub4().unwrap(),
                TNS_AUTH_MODE_LOGON | expected_bits
            );

            let mut phase_two_payload = Vec::new();
            write_auth_header(
                &mut phase_two_payload,
                "system",
                TNS_AUTH_MODE_LOGON | TNS_AUTH_MODE_WITH_PASSWORD | auth_mode.tns_bits(),
                7,
            )
            .unwrap();
            let mut cursor = PacketCursor::with_capabilities(
                &phase_two_payload,
                &OracleThinCapabilities::default(),
            );
            assert_eq!(cursor.read_u8().unwrap(), 1);
            assert_eq!(cursor.read_ub4().unwrap(), 6);
            assert_eq!(
                cursor.read_ub4().unwrap(),
                TNS_AUTH_MODE_LOGON | TNS_AUTH_MODE_WITH_PASSWORD | expected_bits
            );
        }
    }

    #[test]
    fn auth_pbkdf2_combo_keys_match_vendor_temp_key_ordering() {
        let session_key_part_a = (0u8..32).collect::<Vec<_>>();
        let session_key_part_b = (0u8..32).rev().collect::<Vec<_>>();

        for (verifier_type, key_len, expected) in [
            (
                TNS_VERIFIER_TYPE_10G,
                16,
                "9E0D7F59932149D8E55CDB73F6A83E70",
            ),
            (
                TNS_VERIFIER_TYPE_11G_1,
                24,
                "B7083D2369C40839C6F1D83DB460A878ED240A923A44EDB1",
            ),
            (
                TNS_VERIFIER_TYPE_12C,
                32,
                "B42122FDFA6546989A46FAC166F52B2F02FD1B37E6CE14A1DFB2D923988187F7",
            ),
        ] {
            let state = AuthState {
                verifier_type,
                session_data: HashMap::from([
                    (
                        "AUTH_PBKDF2_CSK_SALT".to_string(),
                        "0102030405060708".to_string(),
                    ),
                    ("AUTH_PBKDF2_SDER_COUNT".to_string(), "7".to_string()),
                ]),
                auth_uses_pbkdf2_key_derivation: true,
                ..AuthState::default()
            };

            let combo_key =
                derive_auth_combo_key(&state, &session_key_part_a, &session_key_part_b, key_len)
                    .unwrap();

            assert_eq!(hex_encode_upper(&combo_key), expected);
        }
    }

    #[test]
    fn auth_11g_credentials_use_aes192_and_do_not_send_speedy_key() {
        let password_hash = generate_11g_password_hash(b"password", &[1, 2, 3, 4]);
        let session_key_part_a = (0u8..48).collect::<Vec<_>>();
        let session_key_part_b = (0u8..48).rev().collect::<Vec<_>>();
        let mut state = AuthState {
            verifier_type: TNS_VERIFIER_TYPE_11G_2,
            ..AuthState::default()
        };

        let credentials = generate_auth_credentials_from_session_key_parts(
            b"password",
            None,
            &mut state,
            &password_hash,
            24,
            None,
            &session_key_part_a,
            &session_key_part_b,
        )
        .unwrap();

        assert_eq!(credentials.session_key.len(), 96);
        assert!(credentials.speedy_key.is_none());
        assert!(!credentials.password.is_empty());
        assert_eq!(state.combo_key.as_ref().unwrap().len(), 24);
    }

    #[test]
    fn auth_10g_credentials_use_aes128_and_do_not_send_speedy_key() {
        let password_hash = generate_10g_password_hash("scott", "tiger");
        let session_key_part_a = (0u8..48).collect::<Vec<_>>();
        let session_key_part_b = (0u8..48).rev().collect::<Vec<_>>();
        let mut state = AuthState {
            verifier_type: TNS_VERIFIER_TYPE_10G,
            ..AuthState::default()
        };

        let credentials = generate_auth_credentials_from_session_key_parts(
            b"tiger",
            None,
            &mut state,
            &password_hash,
            16,
            None,
            &session_key_part_a,
            &session_key_part_b,
        )
        .unwrap();

        assert_eq!(credentials.session_key.len(), 96);
        assert!(credentials.speedy_key.is_none());
        assert!(!credentials.password.is_empty());
        assert_eq!(state.combo_key.as_ref().unwrap().len(), 16);
    }

    #[test]
    fn auth_legacy_code_zero_summary_stops_before_padding() {
        let caps = OracleThinCapabilities {
            protocol_version: Some(314),
            ttc_field_version: 6,
            supports_end_of_call_status: true,
            supports_fast_session_attributes: true,
            supports_big_clr_chunks: false,
            ..OracleThinCapabilities::default()
        };
        let mut packet = vec![TNS_MSG_TYPE_ERROR];
        write_ub4(&mut packet, 0x0102_0304);
        write_ub2(&mut packet, 0x1234);
        push_legacy_summary_prefix(&mut packet, 3, 0, 42, 0);
        for _ in 0..4 {
            write_ub4(&mut packet, 0);
        }
        packet.push(0);
        let mut state = AuthState::default();

        process_auth_payload(&packet, &caps, &mut state).unwrap();
    }

    #[test]
    fn auth_response_processes_server_side_piggyback_before_parameters() {
        let mut packet = vec![
            TNS_MSG_TYPE_SERVER_SIDE_PIGGYBACK,
            TNS_SERVER_PIGGYBACK_TRACE_EVENT,
            TNS_MSG_TYPE_PARAMETER,
        ];
        write_ub2(&mut packet, 1);
        write_bytes_with_two_lengths(&mut packet, b"AUTH_VERSION_NO").unwrap();
        write_bytes_with_two_lengths(&mut packet, b"23.0.0.0.0").unwrap();
        write_ub4(&mut packet, 0);
        let mut state = AuthState::default();

        process_auth_payload(&packet, &OracleThinCapabilities::default(), &mut state).unwrap();

        assert_eq!(state.server_version.as_deref(), Some("23.0.0.0.0"));
        assert_eq!(
            state
                .session_data
                .get("AUTH_VERSION_NO")
                .map(String::as_str),
            Some("23.0.0.0.0")
        );
    }

    #[test]
    fn server_side_piggyback_stores_ltxid_like_python_oracledb() {
        let mut packet = vec![TNS_SERVER_PIGGYBACK_LTXID];
        write_bytes_with_two_lengths(&mut packet, b"ltxid-123").unwrap();
        let mut cursor =
            PacketCursor::with_capabilities(&packet, &OracleThinCapabilities::default());
        let mut state = ServerSidePiggybackState::default();

        process_server_side_piggyback(&mut cursor, &OracleThinCapabilities::default(), &mut state)
            .unwrap();

        assert_eq!(state.ltxid, b"ltxid-123");
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn server_side_piggyback_stores_session_return_state_like_python_oracledb() {
        let mut packet = vec![TNS_SERVER_PIGGYBACK_SESS_RET];
        write_ub2(&mut packet, 0);
        packet.push(0);
        write_ub2(&mut packet, 1);
        packet.push(1);
        write_ub2(&mut packet, 3);
        packet.extend_from_slice(&[3, b'k', b'e', b'y']);
        write_ub2(&mut packet, 5);
        packet.extend_from_slice(&[5, b'v', b'a', b'l', b'u', b'e']);
        write_ub2(&mut packet, 0);
        write_ub4(&mut packet, 4);
        write_ub4(&mut packet, 0x0102_0304);
        write_ub2(&mut packet, 0x1122);
        let mut cursor =
            PacketCursor::with_capabilities(&packet, &OracleThinCapabilities::default());
        let mut state = ServerSidePiggybackState::default();

        process_server_side_piggyback(&mut cursor, &OracleThinCapabilities::default(), &mut state)
            .unwrap();

        assert!(state.session_changed);
        assert_eq!(state.session_id, Some(0x0102_0304));
        assert_eq!(state.serial_num, Some(0x1122));
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn return_parameters_update_schema_edition_and_sessionless_state_like_python_oracledb() {
        let mut transaction_value = b"txn-123".to_vec();
        transaction_value
            .extend_from_slice(&[TNS_TPC_TXNID_SYNC_SET | TNS_TPC_TXNID_SYNC_SERVER, 1]);
        let mut packet = Vec::new();
        push_return_parameter_prefix(&mut packet, 3);
        push_keyword_pair(
            &mut packet,
            Some("APP_SCHEMA"),
            None,
            TNS_KEYWORD_NUM_CURRENT_SCHEMA,
        );
        push_keyword_pair(&mut packet, Some("ORA$BASE"), None, TNS_KEYWORD_NUM_EDITION);
        push_keyword_pair(
            &mut packet,
            None,
            Some(&transaction_value),
            TNS_KEYWORD_NUM_TRANSACTION_ID,
        );
        write_ub2(&mut packet, 0);
        let mut cursor =
            PacketCursor::with_capabilities(&packet, &OracleThinCapabilities::default());
        let mut state = ServerSidePiggybackState::default();

        process_return_parameters(&mut cursor, &OracleThinCapabilities::default(), &mut state)
            .unwrap();

        assert_eq!(state.current_schema.as_deref(), Some("APP_SCHEMA"));
        assert_eq!(state.edition.as_deref(), Some("ORA$BASE"));
        assert_eq!(
            state.sessionless_transaction_id.as_deref(),
            Some(&b"txn-123"[..])
        );
        assert!(state.sessionless_started_on_server);
        assert!(state.transaction_in_progress);
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn return_parameters_clear_sessionless_state_like_python_oracledb() {
        let mut transaction_value = b"txn-123".to_vec();
        transaction_value.extend_from_slice(&[TNS_TPC_TXNID_SYNC_UNSET, 1]);
        let mut packet = Vec::new();
        push_return_parameter_prefix(&mut packet, 1);
        push_keyword_pair(
            &mut packet,
            None,
            Some(&transaction_value),
            TNS_KEYWORD_NUM_TRANSACTION_ID,
        );
        write_ub2(&mut packet, 0);
        let mut cursor =
            PacketCursor::with_capabilities(&packet, &OracleThinCapabilities::default());
        let mut state = ServerSidePiggybackState {
            sessionless_transaction_id: Some(b"txn-123".to_vec()),
            sessionless_started_on_server: true,
            transaction_in_progress: true,
            ..ServerSidePiggybackState::default()
        };

        process_return_parameters(&mut cursor, &OracleThinCapabilities::default(), &mut state)
            .unwrap();

        assert!(state.sessionless_transaction_id.is_none());
        assert!(!state.sessionless_started_on_server);
        assert!(!state.transaction_in_progress);
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn return_parameters_reject_unknown_sessionless_sync_version_like_python_oracledb() {
        let mut transaction_value = b"txn-123".to_vec();
        transaction_value.extend_from_slice(&[TNS_TPC_TXNID_SYNC_SET, 2]);
        let mut packet = Vec::new();
        push_return_parameter_prefix(&mut packet, 1);
        push_keyword_pair(
            &mut packet,
            None,
            Some(&transaction_value),
            TNS_KEYWORD_NUM_TRANSACTION_ID,
        );
        write_ub2(&mut packet, 0);
        let mut cursor =
            PacketCursor::with_capabilities(&packet, &OracleThinCapabilities::default());
        let mut state = ServerSidePiggybackState::default();

        let err =
            process_return_parameters(&mut cursor, &OracleThinCapabilities::default(), &mut state)
                .expect_err("unknown sync version should fail");

        assert!(
            err.to_string()
                .contains("unknown Oracle sessionless transaction sync version 2"),
            "{err}"
        );
    }

    fn push_return_parameter_prefix(out: &mut Vec<u8>, num_pairs: u16) {
        write_ub2(out, 0);
        write_ub2(out, 0);
        write_ub2(out, num_pairs);
    }

    fn push_keyword_pair(
        out: &mut Vec<u8>,
        text: Option<&str>,
        binary: Option<&[u8]>,
        keyword_num: u16,
    ) {
        if let Some(text) = text {
            write_ub2(out, text.len() as u16);
            out.push(text.len() as u8);
            out.extend_from_slice(text.as_bytes());
        } else {
            write_ub2(out, 0);
        }
        if let Some(binary) = binary {
            write_ub2(out, binary.len() as u16);
            out.push(binary.len() as u8);
            out.extend_from_slice(binary);
        } else {
            write_ub2(out, 0);
        }
        write_ub2(out, keyword_num);
    }

    #[test]
    fn token_message_rejects_mismatched_token_like_python_oracledb() {
        let caps = OracleThinCapabilities::default();
        let mut matching = Vec::new();
        write_ub8(&mut matching, 0);
        let mut cursor = PacketCursor::with_capabilities(&matching, &caps);

        process_token(&mut cursor, 0).unwrap();

        let mut mismatched = Vec::new();
        write_ub8(&mut mismatched, 7);
        let mut cursor = PacketCursor::with_capabilities(&mismatched, &caps);
        let err = process_token(&mut cursor, 0).expect_err("mismatched token should fail");

        assert!(err.to_string().contains("mismatched Oracle token"));
    }

    #[test]
    fn local_timezone_offset_matches_oracle_offset_literal_shape() {
        let offset = local_timezone_offset_string();

        assert_eq!(offset.len(), 6);
        assert!(matches!(offset.as_bytes()[0], b'+' | b'-'));
        assert_eq!(offset.as_bytes()[3], b':');
        assert!(offset.as_bytes()[1..3].iter().all(u8::is_ascii_digit));
        assert!(offset.as_bytes()[4..6].iter().all(u8::is_ascii_digit));
    }

    #[test]
    fn end_of_response_requires_protocol_319() {
        let options = ConnectOptions::default();
        let accept = AcceptInfo {
            protocol_version: 318,
            protocol_options: 0,
            sdu: 8192,
            supports_full_packet_size: true,
            flags2: 0x0200_0000,
        };
        let caps = capabilities_from_accept(&options, &accept);
        assert!(!caps.supports_end_of_response);

        let accept = AcceptInfo {
            protocol_version: 319,
            ..accept
        };
        let caps = capabilities_from_accept(&options, &accept);
        assert!(caps.supports_end_of_response);
    }

    #[test]
    fn runtime_caps_can_disable_request_boundaries_after_compile_caps_enable_them() {
        let mut caps = OracleThinCapabilities::default();
        let mut compile_caps = vec![0u8; 53];
        compile_caps[TNS_CCAP_TTC4] = TNS_CCAP_EXPLICIT_BOUNDARY;
        adjust_for_server_compile_caps(&mut caps, &compile_caps);
        assert!(caps.supports_request_boundaries);

        let runtime_caps = vec![0u8; 11];
        adjust_for_server_runtime_caps(&mut caps, &runtime_caps);
        assert!(!caps.supports_request_boundaries);

        adjust_for_server_compile_caps(&mut caps, &compile_caps);
        let mut runtime_caps = vec![0u8; 11];
        runtime_caps[TNS_RCAP_TTC] = TNS_RCAP_TTC_SESSION_STATE_OPS;
        adjust_for_server_runtime_caps(&mut caps, &runtime_caps);
        assert!(caps.supports_request_boundaries);
    }

    #[test]
    fn protocol_314_summary_caps_follow_go_ora_server_compile_bits() {
        let mut caps = OracleThinCapabilities::default();
        let compile_caps = vec![0u8; 17];
        adjust_for_server_compile_caps(&mut caps, &compile_caps);
        assert!(!caps.supports_end_of_call_status);
        assert!(!caps.supports_fast_session_attributes);

        let mut compile_caps = vec![0u8; 17];
        compile_caps[TNS_CCAP_TTC1] = TNS_CCAP_END_OF_CALL_STATUS;
        compile_caps[TNS_CCAP_OCI1] = TNS_CCAP_LEGACY_FAST_SESSION_ATTRIBUTES;
        adjust_for_server_compile_caps(&mut caps, &compile_caps);
        assert!(caps.supports_end_of_call_status);
        assert!(caps.supports_fast_session_attributes);
    }

    #[test]
    fn modern_protocols_scan_long_clr_chunks_with_python_oracledb_ub4_lengths() {
        let options = ConnectOptions::default();
        let accept = AcceptInfo {
            protocol_version: 315,
            protocol_options: 0,
            sdu: 8192,
            supports_full_packet_size: true,
            flags2: 0,
        };
        let caps = capabilities_from_accept(&options, &accept);
        assert!(caps.supports_big_clr_chunks);

        let data = [0xfe, 1, 4, b't', b'e', b's', b't', 0];
        let mut cursor = PacketCursor::with_capabilities(&data, &caps);
        assert_eq!(cursor.read_bytes().unwrap(), Some(b"test".to_vec()));
    }

    #[test]
    fn protocol_314_scans_go_ora_legacy_clr_chunks_and_null_indicator() {
        let caps = OracleThinCapabilities {
            protocol_version: Some(314),
            supports_big_clr_chunks: false,
            ..OracleThinCapabilities::default()
        };

        let mut cursor =
            PacketCursor::with_capabilities(&[0xfe, 4, b't', b'e', b's', b't', 0], &caps);
        assert_eq!(cursor.read_bytes().unwrap(), Some(b"test".to_vec()));

        let mut cursor = PacketCursor::with_capabilities(&[0xfd], &caps);
        assert_eq!(cursor.read_bytes().unwrap(), None);
    }

    #[test]
    fn modern_protocols_do_not_treat_go_ora_legacy_null_indicator_as_null() {
        let caps = OracleThinCapabilities {
            protocol_version: Some(315),
            supports_big_clr_chunks: true,
            ..OracleThinCapabilities::default()
        };
        let mut cursor = PacketCursor::with_capabilities(&[0xfd], &caps);
        assert!(cursor.read_bytes().is_err());
    }

    #[test]
    fn protocol_314_writes_go_ora_legacy_clr_chunks_without_big_clr_capability() {
        let caps = OracleThinCapabilities {
            protocol_version: Some(314),
            supports_big_clr_chunks: false,
            ..OracleThinCapabilities::default()
        };
        let value = vec![b'x'; 253];
        let mut out = Vec::new();
        write_bytes_with_length_for_capabilities(&mut out, &value, &caps).unwrap();

        assert_eq!(out.first().copied(), Some(0xfe));
        let mut pos = 1;
        let mut chunks = 0;
        while out[pos] != 0 {
            let len = usize::from(out[pos]);
            assert!(len <= TNS_LEGACY_CLR_CHUNK_SIZE);
            pos += 1 + len;
            chunks += 1;
        }
        assert_eq!(chunks, 4);
        assert_eq!(pos, out.len() - 1);

        let mut cursor = PacketCursor::with_capabilities(&out, &caps);
        assert_eq!(cursor.read_bytes().unwrap(), Some(value));
    }

    #[test]
    fn protocol_314_writes_and_reads_varchar2_4000_legacy_clr_chunks() {
        let caps = OracleThinCapabilities {
            protocol_version: Some(314),
            supports_big_clr_chunks: false,
            ..OracleThinCapabilities::default()
        };
        let value = vec![b'x'; 4000];
        let mut out = Vec::new();
        write_bytes_with_length_for_capabilities(&mut out, &value, &caps).unwrap();

        assert_eq!(out.first().copied(), Some(0xfe));
        let mut pos = 1;
        let mut chunks = 0;
        while out[pos] != 0 {
            let len = usize::from(out[pos]);
            assert!(len <= TNS_LEGACY_CLR_CHUNK_SIZE);
            pos += 1 + len;
            chunks += 1;
        }
        assert_eq!(chunks, 63);

        let mut cursor = PacketCursor::with_capabilities(&out, &caps);
        assert_eq!(cursor.read_bytes().unwrap(), Some(value));
    }

    #[test]
    fn modern_protocols_keep_existing_big_clr_write_format() {
        let caps = OracleThinCapabilities {
            protocol_version: Some(315),
            supports_big_clr_chunks: true,
            ..OracleThinCapabilities::default()
        };
        let value = vec![b'x'; 253];
        let mut out = Vec::new();
        write_bytes_with_length_for_capabilities(&mut out, &value, &caps).unwrap();

        assert_eq!(out[0], 0xfe);
        assert_eq!(&out[1..3], &[1, 253]);
        let mut cursor = PacketCursor::with_capabilities(&out, &caps);
        assert_eq!(cursor.read_bytes().unwrap(), Some(value));
    }

    #[test]
    fn modern_protocols_write_and_read_varchar2_4000_big_clr_chunk() {
        let caps = OracleThinCapabilities {
            protocol_version: Some(315),
            supports_big_clr_chunks: true,
            ..OracleThinCapabilities::default()
        };
        let value = vec![b'x'; 4000];
        let mut out = Vec::new();
        write_bytes_with_length_for_capabilities(&mut out, &value, &caps).unwrap();

        assert_eq!(out.first().copied(), Some(0xfe));
        let mut length_cursor = PacketCursor::with_capabilities(&out[1..], &caps);
        assert_eq!(length_cursor.read_ub4().unwrap(), 4000);

        let mut cursor = PacketCursor::with_capabilities(&out, &caps);
        assert_eq!(cursor.read_bytes().unwrap(), Some(value));
    }

    #[test]
    fn describe_body_uses_go_ora_ttc_version_tail_gates_for_protocol_314() {
        let mut caps = OracleThinCapabilities {
            protocol_version: Some(314),
            ttc_field_version: 2,
            ..OracleThinCapabilities::default()
        };
        let mut state = ExecuteReadState::default();
        let mut cursor = PacketCursor::with_capabilities(&[0, 0, 0], &caps);
        process_describe_body(&mut cursor, &caps, &mut state).unwrap();
        assert_eq!(cursor.remaining(), 0);

        caps.ttc_field_version = 6;
        let mut cursor = PacketCursor::with_capabilities(&[0, 0, 0, 0, 0, 0, 0, 0], &caps);
        process_describe_body(&mut cursor, &caps, &mut state).unwrap();
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn protocol_negotiation_rejects_short_fdo_instead_of_guessing_ncharset() {
        let mut caps = OracleThinCapabilities {
            protocol_version: Some(314),
            ..OracleThinCapabilities::default()
        };
        let mut packet = vec![6, 0, b'x', 0, 1, 0, 0, 0, 0];
        packet.extend_from_slice(&6u16.to_be_bytes());
        packet.extend_from_slice(&[0; 6]);
        let mut cursor = PacketCursor::with_capabilities(&packet, &caps);

        assert!(process_protocol_message(&mut cursor, &mut caps).is_err());
    }

    #[test]
    fn protocol_negotiation_requires_go_ora_minimum_compile_caps_length() {
        let mut caps = OracleThinCapabilities {
            protocol_version: Some(314),
            ..OracleThinCapabilities::default()
        };
        let mut packet = vec![6, 0, b'x', 0, 1, 0, 0, 0, 0];
        packet.extend_from_slice(&11u16.to_be_bytes());
        packet.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 232]);
        packet.extend_from_slice(&[7, 0, 0, 0, 0, 0, 0, 6]);
        let mut cursor = PacketCursor::with_capabilities(&packet, &caps);

        assert!(process_protocol_message(&mut cursor, &mut caps).is_err());
    }

    #[test]
    fn protocol_314_summary_ttc6_uses_go_ora_initial_retcode_tail() {
        let caps = OracleThinCapabilities {
            protocol_version: Some(314),
            ttc_field_version: 6,
            supports_end_of_call_status: false,
            supports_fast_session_attributes: false,
            supports_big_clr_chunks: false,
            ..OracleThinCapabilities::default()
        };
        let mut packet = Vec::new();
        push_legacy_summary_prefix(&mut packet, 12, 942, 77, 5);
        for _ in 0..4 {
            write_ub4(&mut packet, 0);
        }
        write_bytes_with_length_for_capabilities(
            &mut packet,
            b"ORA-00942: table or view does not exist\n",
            &caps,
        )
        .unwrap();

        let mut cursor = PacketCursor::with_capabilities(&packet, &caps);
        let error = process_legacy_execute_error(&mut cursor, &caps).unwrap();
        assert_eq!(error.code, 942);
        assert_eq!(error.cursor_id, 77);
        assert_eq!(error._rowcount, 12);
        assert_eq!(
            error.message.as_deref(),
            Some("ORA-00942: table or view does not exist")
        );
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn protocol_314_summary_consumes_go_ora_eos_and_fsap_prefixes() {
        let caps = OracleThinCapabilities {
            protocol_version: Some(314),
            ttc_field_version: 6,
            supports_end_of_call_status: true,
            supports_fast_session_attributes: true,
            supports_big_clr_chunks: false,
            ..OracleThinCapabilities::default()
        };
        let mut packet = Vec::new();
        write_ub4(&mut packet, 0x0102_0304);
        write_ub2(&mut packet, 0x1234);
        push_legacy_summary_prefix(&mut packet, 3, 0, 42, 0);
        for _ in 0..4 {
            write_ub4(&mut packet, 0);
        }

        let mut cursor = PacketCursor::with_capabilities(&packet, &caps);
        let error = process_legacy_execute_error(&mut cursor, &caps).unwrap();
        assert_eq!(error.code, 0);
        assert_eq!(error.cursor_id, 42);
        assert_eq!(error._rowcount, 3);
        assert_eq!(error.message, None);
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn protocol_314_summary_ttc7_uses_go_ora_extended_retcode_tail() {
        let caps = OracleThinCapabilities {
            protocol_version: Some(314),
            ttc_field_version: 7,
            supports_end_of_call_status: false,
            supports_fast_session_attributes: false,
            supports_big_clr_chunks: false,
            ..OracleThinCapabilities::default()
        };
        let mut packet = Vec::new();
        push_legacy_summary_prefix(&mut packet, 1, 999, 13, 0);
        write_ub4(&mut packet, 0);
        write_ub2(&mut packet, 1);
        packet.push(0xfe);
        packet.push(2);
        write_ub2(&mut packet, 1);
        packet.push(0);
        write_ub4(&mut packet, 1);
        packet.push(0xfe);
        packet.push(4);
        write_ub4(&mut packet, 25);
        packet.push(0);
        write_ub2(&mut packet, 1);
        packet.push(0xfe);
        write_ub2(&mut packet, 123);
        write_bytes_with_length_for_capabilities(&mut packet, b"bind", &caps).unwrap();
        packet.extend_from_slice(&[0, 0]);
        write_ub4(&mut packet, 1555);
        write_ub8(&mut packet, 25);
        write_bytes_with_length_for_capabilities(&mut packet, b"ORA-01555: forced error\n", &caps)
            .unwrap();

        let mut cursor = PacketCursor::with_capabilities(&packet, &caps);
        let error = process_legacy_execute_error(&mut cursor, &caps).unwrap();
        assert_eq!(error.code, 1555);
        assert_eq!(error.cursor_id, 13);
        assert_eq!(error._rowcount, 25);
        assert_eq!(error.message.as_deref(), Some("ORA-01555: forced error"));
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn protocol_314_summary_ttc14_skips_python_oracledb_20c_tail_fields() {
        let caps = OracleThinCapabilities {
            protocol_version: Some(314),
            ttc_field_version: TNS_CCAP_FIELD_VERSION_20_1,
            supports_end_of_call_status: false,
            supports_fast_session_attributes: false,
            supports_big_clr_chunks: false,
            ..OracleThinCapabilities::default()
        };
        let mut packet = Vec::new();
        push_legacy_summary_prefix(&mut packet, 0, 1445, 174, 19);
        write_ub4(&mut packet, 0);
        write_ub2(&mut packet, 0);
        write_ub4(&mut packet, 0);
        write_ub2(&mut packet, 0);
        write_ub4(&mut packet, 1445);
        write_ub8(&mut packet, 0);
        write_ub4(&mut packet, 3);
        write_ub4(&mut packet, 0);
        write_bytes_with_length_for_capabilities(
            &mut packet,
            b"ORA-01445: cannot select ROWID from a join view\n",
            &caps,
        )
        .unwrap();

        let mut cursor = PacketCursor::with_capabilities(&packet, &caps);
        let error = process_legacy_execute_error(&mut cursor, &caps).unwrap();
        assert_eq!(error.code, 1445);
        assert_eq!(error.cursor_id, 174);
        assert_eq!(error._rowcount, 0);
        assert_eq!(
            error.message.as_deref(),
            Some("ORA-01445: cannot select ROWID from a join view")
        );
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn protocol_314_summary_ttc6_uses_server_field_version_for_error_tail() {
        let caps = OracleThinCapabilities {
            protocol_version: Some(314),
            ttc_field_version: 6,
            server_ttc_field_version: TNS_CCAP_FIELD_VERSION_20_1,
            supports_end_of_call_status: true,
            supports_fast_session_attributes: true,
            supports_big_clr_chunks: false,
            ..OracleThinCapabilities::default()
        };
        let mut packet = vec![
            0x01, 0x01, 0x02, 0xf3, 0xae, 0x00, 0x02, 0x05, 0xa5, 0x00, 0x00, 0x01, 0xae, 0x01,
            0x13, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x86, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x05, 0xa5, 0x00, 0x01, 0x03, 0x00,
            0x5a,
        ];
        packet.extend_from_slice(
            b"ORA-01445: cannot select ROWID from, or sample, \
              a join view without a key-preserved table\n",
        );

        let mut cursor = PacketCursor::with_capabilities(&packet, &caps);
        let error = process_legacy_execute_error(&mut cursor, &caps).unwrap();
        assert_eq!(error.code, 1445);
        assert_eq!(error.cursor_id, 174);
        assert_eq!(error._rowcount, 0);
        assert_eq!(
            error.message.as_deref(),
            Some(
                "ORA-01445: cannot select ROWID from, or sample, \
                 a join view without a key-preserved table"
            )
        );
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn protocol_314_summary_ttc6_keeps_go_ora_success_tail_with_modern_server_caps() {
        let caps = OracleThinCapabilities {
            protocol_version: Some(314),
            ttc_field_version: 6,
            server_ttc_field_version: TNS_CCAP_FIELD_VERSION_20_1,
            supports_end_of_call_status: false,
            supports_fast_session_attributes: false,
            supports_big_clr_chunks: false,
            ..OracleThinCapabilities::default()
        };
        let mut packet = Vec::new();
        push_legacy_summary_prefix(&mut packet, 3, 0, 42, 0);
        for _ in 0..4 {
            write_ub4(&mut packet, 0);
        }

        let mut cursor = PacketCursor::with_capabilities(&packet, &caps);
        let error = process_legacy_execute_error(&mut cursor, &caps).unwrap();
        assert_eq!(error.code, 0);
        assert_eq!(error.cursor_id, 42);
        assert_eq!(error._rowcount, 3);
        assert_eq!(error.message, None);
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn row_scanner_reuses_previous_batch_row_for_duplicate_bit_vector_columns() {
        let mut state = ExecuteReadState::default();
        state.columns = vec![
            ThinColumn {
                name: "A".to_string(),
                column_type: OracleColumnType::Varchar,
                ora_type_num: ORA_TYPE_NUM_VARCHAR,
                charset_form: 1,
                buffer_size: 10,
                schema_name: String::new(),
                type_name: String::new(),
            },
            ThinColumn {
                name: "B".to_string(),
                column_type: OracleColumnType::Varchar,
                ora_type_num: ORA_TYPE_NUM_VARCHAR,
                charset_form: 1,
                buffer_size: 10,
                schema_name: String::new(),
                type_name: String::new(),
            },
        ];
        state.last_row = Some(vec![
            OracleValue::Text("old-a".to_string()),
            OracleValue::Text("old-b".to_string()),
        ]);
        state.bit_vector = Some(vec![0b0000_0010]);

        let mut cursor = PacketCursor::with_capabilities(
            &[3, b'n', b'e', b'w'],
            &OracleThinCapabilities::default(),
        );
        process_row_data(&mut cursor, &OracleThinCapabilities::default(), &mut state).unwrap();

        assert_eq!(
            state.rows,
            vec![vec![
                OracleValue::Text("old-a".to_string()),
                OracleValue::Text("new".to_string())
            ]]
        );
    }

    fn push_legacy_summary_prefix(
        out: &mut Vec<u8>,
        row_number: u32,
        ret_code: u16,
        cursor_id: u16,
        error_pos: i16,
    ) {
        write_ub4(out, row_number);
        write_ub2(out, ret_code);
        write_ub2(out, 0);
        write_ub2(out, 0);
        write_ub2(out, cursor_id);
        write_sb2(out, error_pos);
        out.extend_from_slice(&[0x2a, 0]);
        out.extend_from_slice(&[0x20, 0]);
        out.extend_from_slice(&[0, 0]);
        write_ub4(out, 0);
        write_ub2(out, 0);
        out.push(0);
        write_ub4(out, 0);
        write_ub2(out, 0);
        write_ub4(out, 0);
        out.extend_from_slice(&[0, 0]);
        write_ub2(out, 0);
        write_ub4(out, 0);
    }

    fn write_sb2(out: &mut Vec<u8>, value: i16) {
        if value == 0 {
            out.push(0);
            return;
        }
        let magnitude = value.unsigned_abs();
        let mut bytes = if magnitude <= u16::from(u8::MAX) {
            vec![magnitude as u8]
        } else {
            magnitude.to_be_bytes().to_vec()
        };
        while bytes.first() == Some(&0) {
            bytes.remove(0);
        }
        let len = bytes.len() as u8;
        out.push(if value < 0 { len | 0x80 } else { len });
        out.extend(bytes);
    }
}
