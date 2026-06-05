// Portions of this file have been modified from, and reimplemented in Rust
// based on, the thin protocol implementation in python-oracledb
// (https://github.com/oracle/python-oracledb),
// Copyright (c) 2016, 2026, Oracle and/or its affiliates, used under the
// Apache License, Version 2.0. This is a modified work and is not the original
// python-oracledb software. Protocol constants were also cross-checked
// against go-ora (MIT License, Copyright (c) 2020 Samy Sultan).
// See THIRD_PARTY_NOTICES.md.

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::thread;
use std::time::Duration;

use crate::{log_connect_phase, OracleThinError};

const TNS_PACKET_TYPE_CONNECT: u8 = 1;
const TNS_PACKET_TYPE_ACCEPT: u8 = 2;
const TNS_PACKET_TYPE_REFUSE: u8 = 4;
const TNS_PACKET_TYPE_REDIRECT: u8 = 5;
const TNS_PACKET_TYPE_DATA: u8 = 6;
const TNS_PACKET_TYPE_RESEND: u8 = 11;
const TNS_PACKET_FLAG_REDIRECT: u8 = 0x04;
const TNS_GSO_DONT_CARE: u16 = 0x0001;
const TNS_GSO_CAN_RECV_ATTENTION: u16 = 0x0400;
const TNS_NSI_DISABLE_NA: u8 = 0x04;
const TNS_NSI_NA_REQUIRED: u8 = 0x10;
const TNS_NSI_SUPPORT_SECURITY_RENEG: u8 = 0x80;
const TNS_PROTOCOL_CHARACTERISTICS: u16 = 0x4f98;
const TNS_CHECK_OOB: u32 = 0x01;
const TNS_ACCEPT_FLAG_CHECK_OOB: u32 = 0x00000001;
const TNS_ACCEPT_FLAG_FAST_AUTH: u32 = 0x10000000;
const TNS_ACCEPT_FLAG_HAS_END_OF_RESPONSE: u32 = 0x02000000;
const TNS_MAX_CONNECT_DATA: usize = 230;
const TNS_MAX_REDIRECTS: usize = 5;
const TNS_MAX_RESENDS: usize = 3;
pub(crate) const TNS_MIN_SUPPORTED_PROTOCOL: u16 = 314;
pub(crate) const TNS_DEFAULT_SOCKET_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const TNS_DEFAULT_TCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
pub(crate) const TNS_DEFAULT_SDU: u32 = 8192;
const TNS_MIN_SDU: u32 = 512;
const TNS_MAX_SDU: u32 = 2_097_152;
const TNS_KEEPALIVE_INTERVAL_SECS: u32 = 6;
const TNS_KEEPALIVE_COUNT: u32 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectTarget {
    pub host: String,
    pub port: u16,
    pub service_name: String,
    pub sid: Option<String>,
    pub instance_name: Option<String>,
    pub server_type: Option<OracleNetServerType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OracleNetServerType {
    Dedicated,
    Shared,
    Pooled,
}

impl OracleNetServerType {
    fn descriptor_value(self) -> &'static str {
        match self {
            Self::Dedicated => "dedicated",
            Self::Shared => "shared",
            Self::Pooled => "pooled",
        }
    }
}

impl ConnectTarget {
    pub fn service_name(
        host: impl Into<String>,
        port: u16,
        service_name: impl Into<String>,
    ) -> Self {
        Self {
            host: host.into(),
            port,
            service_name: service_name.into(),
            sid: None,
            instance_name: None,
            server_type: None,
        }
    }

    pub fn sid(host: impl Into<String>, port: u16, sid: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            port,
            service_name: String::new(),
            sid: Some(sid.into()),
            instance_name: None,
            server_type: None,
        }
    }

    pub fn with_instance_name(mut self, instance_name: impl Into<String>) -> Self {
        self.instance_name = Some(instance_name.into());
        self
    }

    pub fn with_server_type(mut self, server_type: OracleNetServerType) -> Self {
        self.server_type = Some(server_type);
        self
    }

    pub fn easy_connect_string(&self) -> String {
        if !self.service_name.is_empty() {
            format!("//{}:{}/{}", self.host, self.port, self.service_name)
        } else if let Some(sid) = self.sid.as_deref() {
            format!("//{}:{}/?sid={}", self.host, self.port, sid)
        } else {
            format!("//{}:{}/", self.host, self.port)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectOptions {
    pub desired_protocol_version: u16,
    pub minimum_protocol_version: u16,
    pub desired_ttc_field_version: Option<u8>,
    pub disable_oob_probe: bool,
    pub tcp_connect_timeout: Duration,
    pub retry_count: u32,
    pub retry_delay: Duration,
    pub expire_time: u32,
    pub connection_id: Option<String>,
    pub connection_id_prefix: Option<String>,
    pub sdu: u32,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            desired_protocol_version: 319,
            minimum_protocol_version: TNS_MIN_SUPPORTED_PROTOCOL,
            desired_ttc_field_version: None,
            disable_oob_probe: true,
            tcp_connect_timeout: TNS_DEFAULT_TCP_CONNECT_TIMEOUT,
            retry_count: 0,
            retry_delay: Duration::from_secs(1),
            expire_time: 0,
            connection_id: None,
            connection_id_prefix: None,
            sdu: TNS_DEFAULT_SDU,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptInfo {
    pub protocol_version: u16,
    pub protocol_options: u16,
    pub sdu: u32,
    pub supports_full_packet_size: bool,
    pub flags2: u32,
}

#[derive(Debug, Clone)]
pub struct OracleNetConnector {
    options: ConnectOptions,
}

impl OracleNetConnector {
    pub fn new(options: ConnectOptions) -> Self {
        Self { options }
    }

    pub fn connect_tcp(
        &self,
        target: &ConnectTarget,
    ) -> Result<(TcpStream, AcceptInfo), OracleThinError> {
        let initial_connect_data = build_connect_data(target, &self.options)?;
        let mut last_error = None;

        for attempt in 0..=self.options.retry_count {
            let mut host = target.host.clone();
            let mut port = target.port;
            let mut connect_data = initial_connect_data.clone();
            let mut packet_flags = 0;

            let attempt_result = (|| {
                for _ in 0..=TNS_MAX_REDIRECTS {
                    log_connect_phase("tcp-connect", &format!("{host}:{port}"));
                    let stream = connect_socket(&host, port, self.options.tcp_connect_timeout)?;
                    configure_socket(&stream, &self.options)?;
                    let _ = stream.set_read_timeout(Some(TNS_DEFAULT_SOCKET_TIMEOUT));
                    let _ = stream.set_write_timeout(Some(TNS_DEFAULT_SOCKET_TIMEOUT));
                    match self.connect_tns(stream, &connect_data, packet_flags)? {
                        ConnectOutcome::Accepted(stream, accept) => return Ok((stream, accept)),
                        ConnectOutcome::Redirect(redirect_data) => {
                            let redirect = parse_redirect_data(&redirect_data)?;
                            host = redirect.host;
                            port = redirect.port;
                            connect_data = redirect.connect_data;
                            packet_flags = TNS_PACKET_FLAG_REDIRECT;
                        }
                    }
                }

                Err(OracleThinError::new(format!(
                    "Oracle listener redirected more than {TNS_MAX_REDIRECTS} times"
                )))
            })();

            match attempt_result {
                Ok(result) => return Ok(result),
                Err(err) => {
                    if attempt == self.options.retry_count {
                        return Err(err);
                    }
                    last_error = Some(err);
                    if !self.options.retry_delay.is_zero() {
                        thread::sleep(self.options.retry_delay);
                    }
                }
            };
        }

        Err(last_error.unwrap_or_else(|| OracleThinError::new("Oracle connection attempt failed")))
    }

    fn connect_tns(
        &self,
        mut stream: TcpStream,
        connect_data: &str,
        packet_flags: u8,
    ) -> Result<ConnectOutcome, OracleThinError> {
        let connect_packet =
            build_connect_packet(&self.options, connect_data.as_bytes(), packet_flags)?;
        log_connect_phase(
            "tns-connect",
            &format!(
                "desired={} minimum={} disable_oob={} flags=0x{:x} bytes={}",
                self.options.desired_protocol_version,
                self.options.minimum_protocol_version,
                self.options.disable_oob_probe,
                packet_flags,
                connect_packet.len()
            ),
        );
        if std::env::var_os("ORACLE_THIN_TRACE_EXEC").is_some() {
            eprintln!(
                "thin tns connect desired={} minimum={} disable_oob={}",
                self.options.desired_protocol_version,
                self.options.minimum_protocol_version,
                self.options.disable_oob_probe
            );
        }
        write_connect_packets(&mut stream, &connect_packet)?;

        for _ in 0..=TNS_MAX_RESENDS {
            let packet = read_tns_packet(&mut stream)?;
            match packet.packet_type {
                TNS_PACKET_TYPE_ACCEPT => {
                    let accept = parse_accept_packet(&packet.data)?;
                    if accept.protocol_version < self.options.minimum_protocol_version {
                        return Err(OracleThinError::new(format!(
                            "Oracle listener accepted TNS protocol {}, below requested minimum {}",
                            accept.protocol_version, self.options.minimum_protocol_version
                        )));
                    }
                    log_connect_phase(
                        "tns-accept",
                        &format!(
                            "protocol={} sdu={} flags2=0x{:x}",
                            accept.protocol_version, accept.sdu, accept.flags2
                        ),
                    );
                    return Ok(ConnectOutcome::Accepted(stream, accept));
                }
                TNS_PACKET_TYPE_REFUSE => {
                    return Err(listener_refuse_error(&packet.data));
                }
                TNS_PACKET_TYPE_REDIRECT => {
                    return Ok(ConnectOutcome::Redirect(read_redirect_data(
                        &mut stream,
                        &packet.data,
                    )?));
                }
                TNS_PACKET_TYPE_RESEND => {
                    log_connect_phase("tns-resend", "resending connect packet");
                    write_connect_packets(&mut stream, &connect_packet)?;
                }
                other => {
                    return Err(OracleThinError::new(format!(
                        "unexpected TNS packet type {other} during connect"
                    )));
                }
            }
        }

        Err(OracleThinError::new(format!(
            "Oracle listener requested more than {TNS_MAX_RESENDS} TNS connect resends"
        )))
    }
}

fn write_connect_packets(stream: &mut TcpStream, packets: &[u8]) -> Result<(), OracleThinError> {
    let mut offset = 0;
    while offset < packets.len() {
        if offset + 2 > packets.len() {
            return Err(OracleThinError::new("truncated TNS connect packet buffer"));
        }
        let packet_len = u16::from_be_bytes([packets[offset], packets[offset + 1]]) as usize;
        if packet_len == 0 || offset + packet_len > packets.len() {
            return Err(OracleThinError::new(
                "invalid TNS connect packet buffer length",
            ));
        }
        stream.write_all(&packets[offset..offset + packet_len])?;
        stream.flush()?;
        offset += packet_len;
    }
    Ok(())
}

fn connect_socket(host: &str, port: u16, timeout: Duration) -> Result<TcpStream, OracleThinError> {
    if timeout.is_zero() {
        return TcpStream::connect((host, port)).map_err(OracleThinError::from);
    }

    let mut last_error = None;
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => return Ok(stream),
            Err(err) => last_error = Some(err),
        }
    }

    Err(last_error
        .unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no socket addresses resolved"))
        .into())
}

fn configure_socket(stream: &TcpStream, options: &ConnectOptions) -> Result<(), OracleThinError> {
    stream.set_nodelay(true)?;
    set_tcp_keepalive(stream, options.expire_time)?;
    Ok(())
}

#[cfg(unix)]
fn set_tcp_keepalive(stream: &TcpStream, expire_time_minutes: u32) -> Result<(), OracleThinError> {
    if expire_time_minutes == 0 {
        return Ok(());
    }
    let fd = stream.as_raw_fd();
    set_sockopt_int(fd, libc::SOL_SOCKET, libc::SO_KEEPALIVE, 1)?;
    let idle_secs = sockopt_seconds(expire_time_minutes.saturating_mul(60));
    set_tcp_keepalive_idle(fd, idle_secs)?;
    set_tcp_keepalive_interval(fd, sockopt_seconds(TNS_KEEPALIVE_INTERVAL_SECS))?;
    set_tcp_keepalive_count(fd, sockopt_seconds(TNS_KEEPALIVE_COUNT))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_tcp_keepalive(
    _stream: &TcpStream,
    _expire_time_minutes: u32,
) -> Result<(), OracleThinError> {
    Ok(())
}

#[cfg(unix)]
fn sockopt_seconds(value: u32) -> libc::c_int {
    value.min(libc::c_int::MAX as u32) as libc::c_int
}

#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
fn set_tcp_keepalive_idle(
    fd: std::os::fd::RawFd,
    value: libc::c_int,
) -> Result<(), OracleThinError> {
    set_sockopt_int(fd, libc::IPPROTO_TCP, libc::TCP_KEEPIDLE, value)
}

#[cfg(all(unix, any(target_os = "macos", target_os = "ios")))]
fn set_tcp_keepalive_idle(
    fd: std::os::fd::RawFd,
    value: libc::c_int,
) -> Result<(), OracleThinError> {
    set_sockopt_int(fd, libc::IPPROTO_TCP, libc::TCP_KEEPALIVE, value)
}

#[cfg(all(
    unix,
    not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))
))]
fn set_tcp_keepalive_idle(
    _fd: std::os::fd::RawFd,
    _value: libc::c_int,
) -> Result<(), OracleThinError> {
    Ok(())
}

#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
fn set_tcp_keepalive_interval(
    fd: std::os::fd::RawFd,
    value: libc::c_int,
) -> Result<(), OracleThinError> {
    set_sockopt_int(fd, libc::IPPROTO_TCP, libc::TCP_KEEPINTVL, value)
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn set_tcp_keepalive_interval(
    _fd: std::os::fd::RawFd,
    _value: libc::c_int,
) -> Result<(), OracleThinError> {
    Ok(())
}

#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
fn set_tcp_keepalive_count(
    fd: std::os::fd::RawFd,
    value: libc::c_int,
) -> Result<(), OracleThinError> {
    set_sockopt_int(fd, libc::IPPROTO_TCP, libc::TCP_KEEPCNT, value)
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn set_tcp_keepalive_count(
    _fd: std::os::fd::RawFd,
    _value: libc::c_int,
) -> Result<(), OracleThinError> {
    Ok(())
}

#[cfg(unix)]
fn set_sockopt_int(
    fd: std::os::fd::RawFd,
    level: libc::c_int,
    name: libc::c_int,
    value: libc::c_int,
) -> Result<(), OracleThinError> {
    let result = unsafe {
        libc::setsockopt(
            fd,
            level,
            name,
            &value as *const libc::c_int as *const libc::c_void,
            std::mem::size_of_val(&value) as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(OracleThinError::from(io::Error::last_os_error()))
    }
}

enum ConnectOutcome {
    Accepted(TcpStream, AcceptInfo),
    Redirect(String),
}

struct RedirectInfo {
    host: String,
    port: u16,
    connect_data: String,
}

struct TnsPacket {
    packet_type: u8,
    data: Vec<u8>,
}

fn build_connect_data(
    target: &ConnectTarget,
    options: &ConnectOptions,
) -> Result<String, OracleThinError> {
    validate_connect_descriptor_value("host", &target.host)?;
    let description_options = connect_description_option_parts(options);
    let connect_data = connect_data_descriptor_parts(target, options.desired_protocol_version)?;
    let connection_id = connect_data_connection_id_part(options)?;
    Ok(format!(
        "(DESCRIPTION={}(ADDRESS=(PROTOCOL=TCP)(HOST={})(PORT={}))(CONNECT_DATA={}{}{}))",
        description_options,
        target.host,
        target.port,
        connect_data,
        "(CID=(PROGRAM=space-query-thin)(HOST=localhost)(USER=space-query))",
        connection_id
    ))
}

pub(crate) fn connect_data_descriptor_parts(
    target: &ConnectTarget,
    protocol_version: u16,
) -> Result<String, OracleThinError> {
    let mut out = String::new();
    if !target.service_name.is_empty() {
        validate_connect_descriptor_value("service_name", &target.service_name)?;
        out.push_str(&format!("(SERVICE_NAME={})", target.service_name));
    } else if protocol_version <= TNS_MIN_SUPPORTED_PROTOCOL {
        if let Some(sid) = target.sid.as_deref() {
            validate_connect_descriptor_value("sid", sid)?;
            out.push_str(&format!("(SID={sid})"));
        }
    }
    if let Some(instance_name) = target.instance_name.as_deref() {
        validate_connect_descriptor_value("instance_name", instance_name)?;
        out.push_str(&format!("(INSTANCE_NAME={instance_name})"));
    } else if protocol_version > TNS_MIN_SUPPORTED_PROTOCOL {
        if let Some(sid) = target.sid.as_deref() {
            validate_connect_descriptor_value("sid", sid)?;
            out.push_str(&format!("(SID={sid})"));
        }
    }
    if let Some(server_type) = target.server_type {
        out.push_str(&format!("(SERVER={})", server_type.descriptor_value()));
    }
    if out.is_empty() {
        return Err(OracleThinError::new(
            "Oracle connect descriptor requires a service name or SID",
        ));
    }
    Ok(out)
}

pub(crate) fn connect_data_connection_id_part(
    options: &ConnectOptions,
) -> Result<String, OracleThinError> {
    let Some(connection_id) = options.connection_id.as_deref() else {
        return Ok(String::new());
    };
    validate_connect_descriptor_value("connection_id", connection_id)?;
    let mut value = String::new();
    if let Some(prefix) = options.connection_id_prefix.as_deref() {
        validate_connect_descriptor_value("connection_id_prefix", prefix)?;
        value.push_str(prefix);
    }
    value.push_str(connection_id);
    Ok(format!("(CONNECTION_ID={value})"))
}

pub(crate) fn connect_description_option_parts(options: &ConnectOptions) -> String {
    let mut out = String::new();
    if options.expire_time != 0 {
        out.push_str(&format!("(EXPIRE_TIME={})", options.expire_time));
    }
    if options.tcp_connect_timeout != TNS_DEFAULT_TCP_CONNECT_TIMEOUT {
        out.push_str(&format!(
            "(TRANSPORT_CONNECT_TIMEOUT={})",
            connect_duration_descriptor_value(options.tcp_connect_timeout)
        ));
    }
    let sdu = options.sdu.clamp(TNS_MIN_SDU, TNS_MAX_SDU);
    if sdu != TNS_DEFAULT_SDU {
        out.push_str(&format!("(SDU={sdu})"));
    }
    out
}

fn connect_duration_descriptor_value(value: Duration) -> String {
    if value.subsec_nanos() != 0 {
        return format!("{}ms", value.as_millis());
    }
    let seconds = value.as_secs();
    if seconds % 60 == 0 {
        return format!("{}min", seconds / 60);
    }
    seconds.to_string()
}

pub(crate) fn validate_connect_descriptor_value(
    name: &str,
    value: &str,
) -> Result<(), OracleThinError> {
    if value.bytes().any(|byte| matches!(byte, b'(' | b')' | b'=')) {
        Err(OracleThinError::new(format!(
            "Oracle connect descriptor {name} contains invalid characters"
        )))
    } else {
        Ok(())
    }
}

fn build_connect_packet(
    options: &ConnectOptions,
    connect_data: &[u8],
    packet_flags: u8,
) -> Result<Vec<u8>, OracleThinError> {
    let connect_len = u16::try_from(connect_data.len()).map_err(|_| {
        OracleThinError::new(format!(
            "Oracle connect data is too large: {} bytes",
            connect_data.len()
        ))
    })?;
    let data_offset = 74u16;
    let inline_connect_data = connect_data.len() <= TNS_MAX_CONNECT_DATA;
    let packet_len = if inline_connect_data {
        usize::from(data_offset) + connect_data.len()
    } else {
        usize::from(data_offset)
    };
    let packet_len_u16 = u16::try_from(packet_len).map_err(|_| {
        OracleThinError::new(format!(
            "Oracle connect packet is too large: {packet_len} bytes"
        ))
    })?;
    let sdu = options.sdu.clamp(TNS_MIN_SDU, TNS_MAX_SDU);
    let legacy_sdu = u16::try_from(sdu).unwrap_or(u16::MAX);
    let mut packet = vec![0u8; packet_len];
    put_u16(&mut packet, 0, packet_len_u16);
    packet[4] = TNS_PACKET_TYPE_CONNECT;
    packet[5] = packet_flags;
    put_u16(&mut packet, 8, options.desired_protocol_version);
    put_u16(&mut packet, 10, options.minimum_protocol_version);

    let mut service_options = TNS_GSO_DONT_CARE;
    let mut connect_flags_2 = 0u32;
    if !options.disable_oob_probe {
        service_options |= TNS_GSO_CAN_RECV_ATTENTION;
        connect_flags_2 |= TNS_CHECK_OOB;
    }
    put_u16(&mut packet, 12, service_options);
    put_u16(&mut packet, 14, legacy_sdu);
    put_u16(&mut packet, 16, legacy_sdu);
    put_u16(&mut packet, 18, TNS_PROTOCOL_CHARACTERISTICS);
    put_u16(&mut packet, 22, 1);
    put_u16(&mut packet, 24, connect_len);
    put_u16(&mut packet, 26, data_offset);
    let nsi_flags = TNS_NSI_SUPPORT_SECURITY_RENEG | TNS_NSI_DISABLE_NA;
    packet[32] = nsi_flags;
    packet[33] = nsi_flags;
    put_u32(&mut packet, 58, sdu);
    put_u32(&mut packet, 62, sdu);
    put_u32(&mut packet, 70, connect_flags_2);
    if inline_connect_data {
        packet[usize::from(data_offset)..].copy_from_slice(connect_data);
    } else {
        let data_packet_len = 8 + connect_data.len();
        let data_packet_len_u16 = u16::try_from(data_packet_len).map_err(|_| {
            OracleThinError::new(format!(
                "Oracle connect data packet is too large: {data_packet_len} bytes"
            ))
        })?;
        let mut data_packet = vec![0u8; data_packet_len];
        put_u16(&mut data_packet, 0, data_packet_len_u16);
        data_packet[4] = TNS_PACKET_TYPE_DATA;
        data_packet[8..].copy_from_slice(connect_data);
        packet.extend_from_slice(&data_packet);
    }
    Ok(packet)
}

fn read_redirect_data(stream: &mut TcpStream, data: &[u8]) -> Result<String, OracleThinError> {
    if data.len() < 2 {
        return Err(OracleThinError::new("short TNS redirect packet"));
    }
    let redirect_len = read_u16(data, 0)? as usize;
    let mut bytes = data[2..].to_vec();
    while bytes.len() < redirect_len {
        let packet = read_tns_packet(stream)?;
        if packet.packet_type != TNS_PACKET_TYPE_DATA {
            return Err(OracleThinError::new(format!(
                "expected TNS data packet for redirect payload, got packet type {}",
                packet.packet_type
            )));
        }
        if packet.data.len() < 2 {
            return Err(OracleThinError::new("short TNS redirect data packet"));
        }
        bytes.extend_from_slice(&packet.data[2..]);
    }
    bytes.truncate(redirect_len);
    String::from_utf8(bytes)
        .map_err(|err| OracleThinError::new(format!("invalid UTF-8 TNS redirect data: {err}")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RefuseInfo {
    message: Option<String>,
    error_code: Option<u32>,
}

fn listener_refuse_error(data: &[u8]) -> OracleThinError {
    let info = parse_refuse_data(data);
    let message = info.message.as_deref();
    let detail = match (info.error_code, message) {
        (Some(12514), Some(message)) => {
            format!("rejected service name (ORA-12514): {message}")
        }
        (Some(12514), None) => "rejected service name (ORA-12514)".to_string(),
        (Some(12505), Some(message)) => format!("rejected SID (ORA-12505): {message}"),
        (Some(12505), None) => "rejected SID (ORA-12505)".to_string(),
        (Some(error_code), Some(message)) => {
            format!("refused connection with error {error_code}: {message}")
        }
        (Some(error_code), None) => format!("refused connection with error {error_code}"),
        (None, Some(message)) => format!("refused connection: {message}"),
        (None, None) => "refused connection without details".to_string(),
    };
    OracleThinError::new(format!("Oracle listener {detail}"))
}

fn parse_refuse_data(data: &[u8]) -> RefuseInfo {
    let message = refuse_message(data);
    let error_code = message.as_deref().and_then(refuse_error_code);
    RefuseInfo {
        message,
        error_code,
    }
}

fn refuse_message(data: &[u8]) -> Option<String> {
    if data.len() >= 4 {
        let message_len = u16::from_be_bytes([data[2], data[3]]) as usize;
        if message_len == 0 {
            return None;
        }
        if data.len() >= 4 + message_len {
            return Some(String::from_utf8_lossy(&data[4..4 + message_len]).into_owned());
        }
    }

    let message = String::from_utf8_lossy(data)
        .trim_matches(char::from(0))
        .trim()
        .to_string();
    (!message.is_empty()).then_some(message)
}

fn refuse_error_code(message: &str) -> Option<u32> {
    let start = message.find("(ERR=")? + 5;
    let end = message[start..].find(')')? + start;
    message[start..end].trim().parse().ok()
}

fn parse_redirect_data(value: &str) -> Result<RedirectInfo, OracleThinError> {
    let Some(split_at) = value.find('\0') else {
        return Err(OracleThinError::new(format!(
            "invalid TNS redirect data without reconnect payload: {value}"
        )));
    };
    let address = &value[..split_at];
    let connect_data = value[split_at + 1..].to_string();
    let host = tns_attribute(address, "HOST").ok_or_else(|| {
        OracleThinError::new(format!("TNS redirect address is missing HOST: {address}"))
    })?;
    let port = tns_attribute(address, "PORT")
        .ok_or_else(|| {
            OracleThinError::new(format!("TNS redirect address is missing PORT: {address}"))
        })?
        .parse::<u16>()
        .map_err(|err| OracleThinError::new(format!("invalid TNS redirect PORT: {err}")))?;
    if connect_data.is_empty() {
        return Err(OracleThinError::new(
            "TNS redirect reconnect payload is empty",
        ));
    }
    Ok(RedirectInfo {
        host,
        port,
        connect_data,
    })
}

fn tns_attribute(value: &str, key: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let key_bytes = key.as_bytes();
    let mut index = 0;
    while index + key_bytes.len() < bytes.len() {
        if is_tns_attribute_key_at(bytes, key_bytes, index) {
            let mut pos = index + key_bytes.len();
            while bytes.get(pos).is_some_and(u8::is_ascii_whitespace) {
                pos += 1;
            }
            if bytes.get(pos) == Some(&b'=') {
                pos += 1;
                while bytes.get(pos).is_some_and(u8::is_ascii_whitespace) {
                    pos += 1;
                }
                let start = pos;
                while bytes.get(pos).is_some_and(|byte| *byte != b')') {
                    pos += 1;
                }
                return Some(value[start..pos].trim().to_string());
            }
        }
        index += 1;
    }
    None
}

fn is_tns_attribute_key_at(bytes: &[u8], key: &[u8], index: usize) -> bool {
    if index != 0 {
        let mut previous = index;
        while previous > 0 && bytes[previous - 1].is_ascii_whitespace() {
            previous -= 1;
        }
        if previous != 0 && bytes[previous - 1] != b'(' {
            return false;
        }
    }
    bytes
        .get(index..index + key.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(key))
}

fn read_tns_packet(stream: &mut TcpStream) -> Result<TnsPacket, OracleThinError> {
    let mut header = [0u8; 8];
    stream.read_exact(&mut header)?;
    let packet_len = u16::from_be_bytes([header[0], header[1]]) as usize;
    if packet_len < header.len() {
        return Err(OracleThinError::new(format!(
            "invalid TNS packet length {packet_len}"
        )));
    }
    let mut data = vec![0u8; packet_len - header.len()];
    stream.read_exact(&mut data)?;
    Ok(TnsPacket {
        packet_type: header[4],
        data,
    })
}

fn parse_accept_packet(data: &[u8]) -> Result<AcceptInfo, OracleThinError> {
    if data.len() < 24 {
        return Err(OracleThinError::new(format!(
            "short TNS accept packet: {} bytes",
            data.len()
        )));
    }
    let protocol_version = read_u16(data, 0)?;
    let options = read_u16(data, 2)?;
    if data
        .get(14)
        .is_some_and(|flags| flags & TNS_NSI_NA_REQUIRED != 0)
    {
        return Err(OracleThinError::new(
            "Oracle listener requires Native Network Encryption/Data Integrity, which Oracle Thin does not support yet",
        ));
    }
    let mut sdu = if protocol_version >= 315 {
        if data.len() < 28 {
            return Err(OracleThinError::new(format!(
                "short TNS accept packet for protocol {protocol_version}: {} bytes",
                data.len()
            )));
        }
        read_u32(data, 24)?
    } else {
        let sdu = u32::from(read_u16(data, 4)?);
        let tdu = if data.len() >= 8 {
            u32::from(read_u16(data, 6)?)
        } else {
            0
        };
        if tdu != 0 && tdu < sdu {
            tdu
        } else {
            sdu
        }
    };
    if sdu == 0 {
        sdu = 8192;
    }
    let flags2 = if protocol_version >= 318 {
        if data.len() < 37 {
            return Err(OracleThinError::new(format!(
                "short TNS accept packet for protocol {protocol_version} flags: {} bytes",
                data.len()
            )));
        }
        read_u32(data, 33)?
    } else {
        0
    };
    Ok(AcceptInfo {
        protocol_version,
        protocol_options: options,
        sdu,
        supports_full_packet_size: true,
        flags2,
    })
}

fn put_u16(buf: &mut [u8], offset: usize, value: u16) {
    buf[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

fn put_u32(buf: &mut [u8], offset: usize, value: u32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn read_u16(buf: &[u8], offset: usize) -> Result<u16, OracleThinError> {
    let bytes = buf
        .get(offset..offset + 2)
        .ok_or_else(|| OracleThinError::new("short TNS packet while reading u16"))?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_u32(buf: &[u8], offset: usize) -> Result<u32, OracleThinError> {
    let bytes = buf
        .get(offset..offset + 4)
        .ok_or_else(|| OracleThinError::new("short TNS packet while reading u32"))?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

impl AcceptInfo {
    pub fn supports_oob_attention(&self) -> bool {
        self.protocol_options & TNS_GSO_CAN_RECV_ATTENTION != 0
    }

    pub fn supports_oob_check(&self) -> bool {
        self.flags2 & TNS_ACCEPT_FLAG_CHECK_OOB != 0
    }

    pub fn supports_fast_auth(&self) -> bool {
        self.flags2 & TNS_ACCEPT_FLAG_FAST_AUTH != 0
    }

    pub fn supports_end_of_response(&self) -> bool {
        self.flags2 & TNS_ACCEPT_FLAG_HAS_END_OF_RESPONSE != 0
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::Duration;

    use super::{
        build_connect_data, build_connect_packet, listener_refuse_error, parse_accept_packet,
        parse_redirect_data, parse_refuse_data, put_u16, put_u32, ConnectOptions, ConnectTarget,
        OracleNetConnector, OracleNetServerType, TNS_MAX_CONNECT_DATA, TNS_MIN_SUPPORTED_PROTOCOL,
        TNS_NSI_NA_REQUIRED, TNS_PACKET_FLAG_REDIRECT, TNS_PACKET_TYPE_ACCEPT,
        TNS_PACKET_TYPE_CONNECT, TNS_PACKET_TYPE_DATA, TNS_PACKET_TYPE_REFUSE,
        TNS_PACKET_TYPE_RESEND,
    };

    #[test]
    fn default_connect_options_request_protocol_314_or_newer() {
        let options = ConnectOptions::default();

        assert_eq!(options.minimum_protocol_version, TNS_MIN_SUPPORTED_PROTOCOL);
        assert_eq!(options.desired_protocol_version, 319);
        assert_eq!(
            options.tcp_connect_timeout,
            super::TNS_DEFAULT_TCP_CONNECT_TIMEOUT
        );
        assert_eq!(options.retry_count, 0);
        assert_eq!(options.expire_time, 0);
        assert_eq!(options.connection_id, None);
        assert_eq!(options.connection_id_prefix, None);
        assert_eq!(options.sdu, super::TNS_DEFAULT_SDU);
    }

    #[test]
    fn connect_packet_sets_redirect_packet_flag() {
        let packet = build_connect_packet(
            &ConnectOptions::default(),
            b"(DESCRIPTION=)",
            TNS_PACKET_FLAG_REDIRECT,
        )
        .unwrap();

        assert_eq!(packet[5], TNS_PACKET_FLAG_REDIRECT);
    }

    #[test]
    fn short_connect_data_stays_inline_in_connect_packet() {
        let connect_data = b"(DESCRIPTION=(CONNECT_DATA=(SERVICE_NAME=FREE)))";
        let packet = build_connect_packet(&ConnectOptions::default(), connect_data, 0).unwrap();
        let packet_len = u16::from_be_bytes([packet[0], packet[1]]) as usize;

        assert_eq!(packet[4], TNS_PACKET_TYPE_CONNECT);
        assert_eq!(packet_len, 74 + connect_data.len());
        assert_eq!(&packet[74..], connect_data);
    }

    #[test]
    fn connect_packet_writes_configured_sdu_like_python_oracledb() {
        let options = ConnectOptions {
            sdu: 131_072,
            ..ConnectOptions::default()
        };
        let packet = build_connect_packet(&options, b"(DESCRIPTION=)", 0).unwrap();

        assert_eq!(u16::from_be_bytes([packet[14], packet[15]]), u16::MAX);
        assert_eq!(u16::from_be_bytes([packet[16], packet[17]]), u16::MAX);
        assert_eq!(
            u32::from_be_bytes([packet[58], packet[59], packet[60], packet[61]]),
            131_072
        );
        assert_eq!(
            u32::from_be_bytes([packet[62], packet[63], packet[64], packet[65]]),
            131_072
        );
    }

    #[test]
    fn connect_packet_sanitizes_sdu_like_python_oracledb() {
        let too_small = ConnectOptions {
            sdu: 128,
            ..ConnectOptions::default()
        };
        let packet = build_connect_packet(&too_small, b"(DESCRIPTION=)", 0).unwrap();
        assert_eq!(u16::from_be_bytes([packet[14], packet[15]]), 512);
        assert_eq!(
            u32::from_be_bytes([packet[58], packet[59], packet[60], packet[61]]),
            512
        );

        let too_large = ConnectOptions {
            sdu: 4_194_304,
            ..ConnectOptions::default()
        };
        let packet = build_connect_packet(&too_large, b"(DESCRIPTION=)", 0).unwrap();
        assert_eq!(u16::from_be_bytes([packet[14], packet[15]]), u16::MAX);
        assert_eq!(
            u32::from_be_bytes([packet[58], packet[59], packet[60], packet[61]]),
            super::TNS_MAX_SDU
        );
    }

    #[test]
    fn connect_data_includes_description_options_like_python_oracledb() {
        let target = ConnectTarget::service_name("dbhost", 1521, "FREEPDB1");
        let options = ConnectOptions {
            expire_time: 10,
            tcp_connect_timeout: Duration::from_millis(1500),
            sdu: 131_072,
            ..ConnectOptions::default()
        };

        let connect_data = build_connect_data(&target, &options).unwrap();

        assert!(connect_data.starts_with(
            "(DESCRIPTION=(EXPIRE_TIME=10)\
             (TRANSPORT_CONNECT_TIMEOUT=1500ms)(SDU=131072)"
        ));
        assert!(!connect_data.contains("RETRY_COUNT="));
        assert!(connect_data.contains("(SERVICE_NAME=FREEPDB1)"));
    }

    #[test]
    fn connect_data_includes_connection_id_like_python_oracledb_when_configured() {
        let target = ConnectTarget::service_name("dbhost", 1521, "FREEPDB1");
        let options = ConnectOptions {
            connection_id: Some("abc123".to_string()),
            connection_id_prefix: Some("space-".to_string()),
            ..ConnectOptions::default()
        };

        let connect_data = build_connect_data(&target, &options).unwrap();

        assert!(connect_data.contains(
            "(CONNECT_DATA=(SERVICE_NAME=FREEPDB1)\
             (CID=(PROGRAM=space-query-thin)(HOST=localhost)(USER=space-query))\
             (CONNECTION_ID=space-abc123))"
        ));
    }

    #[test]
    fn connect_data_rejects_connection_id_descriptor_injection() {
        let target = ConnectTarget::service_name("dbhost", 1521, "FREEPDB1");
        let options = ConnectOptions {
            connection_id: Some("abc)(SERVER=shared".to_string()),
            ..ConnectOptions::default()
        };

        let err =
            build_connect_data(&target, &options).expect_err("invalid connection id should fail");

        assert!(err.to_string().contains("connection_id"));
    }

    #[test]
    fn refuse_data_extracts_vendor_error_payload_like_python_oracledb() {
        let message = b"(DESCRIPTION=(ERR=12514)(VSNNUM=0))";
        let mut data = vec![0, 0];
        data.extend_from_slice(&(message.len() as u16).to_be_bytes());
        data.extend_from_slice(message);

        let info = parse_refuse_data(&data);
        let err = listener_refuse_error(&data);

        assert_eq!(
            info.message.as_deref(),
            Some("(DESCRIPTION=(ERR=12514)(VSNNUM=0))")
        );
        assert_eq!(info.error_code, Some(12514));
        assert!(err.to_string().contains("ORA-12514"));
    }

    #[test]
    fn refuse_data_keeps_malformed_payload_as_details() {
        let info = parse_refuse_data(b"temporary");
        let err = listener_refuse_error(b"temporary");

        assert_eq!(info.message.as_deref(), Some("temporary"));
        assert_eq!(info.error_code, None);
        assert!(err.to_string().contains("temporary"));
    }

    #[test]
    fn refuse_data_reports_empty_vendor_payload_without_details() {
        let err = listener_refuse_error(&[0, 0, 0, 0]);

        assert!(err.to_string().contains("without details"));
    }

    #[test]
    fn connect_data_rejects_descriptor_injection_characters_like_python_oracledb() {
        let target = ConnectTarget::service_name("dbhost", 1521, "FREE)(SERVER=shared");

        let err = build_connect_data(&target, &ConnectOptions::default())
            .expect_err("invalid service name should fail");

        assert!(err.to_string().contains("service_name"));
    }

    #[test]
    fn connect_data_supports_sid_instance_and_server_type_like_vendors() {
        let target = ConnectTarget::sid("dbhost", 1521, "ORCL")
            .with_instance_name("inst1")
            .with_server_type(OracleNetServerType::Dedicated);

        let connect_data = build_connect_data(
            &target,
            &ConnectOptions {
                desired_protocol_version: 314,
                ..ConnectOptions::default()
            },
        )
        .unwrap();

        assert!(connect_data.contains("(SID=ORCL)"));
        assert!(connect_data.contains("(INSTANCE_NAME=inst1)"));
        assert!(connect_data.contains("(SERVER=dedicated)"));
        assert!(!connect_data.contains("SERVICE_NAME="));
    }

    #[test]
    fn modern_connect_data_prefers_instance_over_sid_like_python_oracledb() {
        let target = ConnectTarget::sid("dbhost", 1521, "ORCL")
            .with_instance_name("inst1")
            .with_server_type(OracleNetServerType::Dedicated);

        let connect_data = build_connect_data(&target, &ConnectOptions::default()).unwrap();

        assert!(connect_data.contains("(INSTANCE_NAME=inst1)"));
        assert!(connect_data.contains("(SERVER=dedicated)"));
        assert!(!connect_data.contains("(SID=ORCL)"));
        assert!(!connect_data.contains("SERVICE_NAME="));
    }

    #[test]
    fn service_name_connect_data_supports_python_oracledb_server_type() {
        let target = ConnectTarget::service_name("dbhost", 1521, "FREEPDB1")
            .with_server_type(OracleNetServerType::Shared);

        let connect_data = build_connect_data(&target, &ConnectOptions::default()).unwrap();

        assert!(connect_data.contains("(SERVICE_NAME=FREEPDB1)"));
        assert!(connect_data.contains("(SERVER=shared)"));
    }

    #[test]
    fn long_connect_data_uses_following_data_packet_like_python_oracledb() {
        let connect_data = vec![b'A'; TNS_MAX_CONNECT_DATA + 1];
        let packet = build_connect_packet(&ConnectOptions::default(), &connect_data, 0).unwrap();
        let connect_packet_len = u16::from_be_bytes([packet[0], packet[1]]) as usize;
        let data_packet = &packet[connect_packet_len..];
        let data_packet_len = u16::from_be_bytes([data_packet[0], data_packet[1]]) as usize;

        assert_eq!(connect_packet_len, 74);
        assert_eq!(packet[4], TNS_PACKET_TYPE_CONNECT);
        assert_eq!(data_packet[4], TNS_PACKET_TYPE_DATA);
        assert_eq!(data_packet_len, 8 + connect_data.len());
        assert_eq!(&data_packet[8..], connect_data);
    }

    #[test]
    fn tns_connect_resends_packet_when_listener_requests_code_11() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut server, _) = listener.accept().unwrap();
            let first_packet = read_wire_packet(&mut server);
            assert_eq!(first_packet[4], TNS_PACKET_TYPE_CONNECT);

            write_wire_packet(&mut server, TNS_PACKET_TYPE_RESEND, &[]);

            let second_packet = read_wire_packet(&mut server);
            assert_eq!(second_packet, first_packet);

            let mut accept_data = [0u8; 37];
            put_u16(&mut accept_data, 0, 319);
            put_u32(&mut accept_data, 24, 8192);
            write_wire_packet(&mut server, TNS_PACKET_TYPE_ACCEPT, &accept_data);
        });

        let stream = TcpStream::connect(addr).unwrap();
        let connector = OracleNetConnector::new(ConnectOptions::default());
        let outcome = connector.connect_tns(stream, "(DESCRIPTION=)", 0).unwrap();
        let super::ConnectOutcome::Accepted(_, accept) = outcome else {
            panic!("expected connect accept after resend");
        };

        assert_eq!(accept.protocol_version, 319);
        handle.join().unwrap();
    }

    #[test]
    fn tns_connect_sends_long_connect_data_as_following_data_packet() {
        let connect_data = "A".repeat(TNS_MAX_CONNECT_DATA + 1);
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let expected = connect_data.clone();
        let handle = thread::spawn(move || {
            let (mut server, _) = listener.accept().unwrap();
            let connect_packet = read_wire_packet(&mut server);
            assert_eq!(connect_packet[4], TNS_PACKET_TYPE_CONNECT);
            assert_eq!(connect_packet.len(), 74);

            let data_packet = read_wire_packet(&mut server);
            assert_eq!(data_packet[4], TNS_PACKET_TYPE_DATA);
            assert_eq!(&data_packet[8..], expected.as_bytes());

            let mut accept_data = [0u8; 37];
            put_u16(&mut accept_data, 0, 319);
            put_u32(&mut accept_data, 24, 8192);
            write_wire_packet(&mut server, TNS_PACKET_TYPE_ACCEPT, &accept_data);
        });

        let stream = TcpStream::connect(addr).unwrap();
        let connector = OracleNetConnector::new(ConnectOptions::default());
        let outcome = connector.connect_tns(stream, &connect_data, 0).unwrap();
        let super::ConnectOutcome::Accepted(_, accept) = outcome else {
            panic!("expected connect accept after long connect data");
        };

        assert_eq!(accept.protocol_version, 319);
        handle.join().unwrap();
    }

    #[test]
    fn tns_connect_rejects_accept_below_requested_minimum_protocol() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut server, _) = listener.accept().unwrap();
            let connect_packet = read_wire_packet(&mut server);
            assert_eq!(connect_packet[4], TNS_PACKET_TYPE_CONNECT);

            let mut accept_data = [0u8; 24];
            put_u16(&mut accept_data, 0, 314);
            put_u16(&mut accept_data, 4, 8192);
            write_wire_packet(&mut server, TNS_PACKET_TYPE_ACCEPT, &accept_data);
        });

        let stream = TcpStream::connect(addr).unwrap();
        let connector = OracleNetConnector::new(ConnectOptions {
            desired_protocol_version: 319,
            minimum_protocol_version: 319,
            ..ConnectOptions::default()
        });
        let err = match connector.connect_tns(stream, "(DESCRIPTION=)", 0) {
            Ok(_) => panic!("accepted protocol below requested minimum should fail"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("below requested minimum 319"));
        handle.join().unwrap();
    }

    #[test]
    fn connect_tcp_retries_listener_refuse_when_retry_count_is_set_like_python_oracledb() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            for attempt in 0..2 {
                let (mut server, _) = listener.accept().unwrap();
                let connect_packet = read_wire_packet(&mut server);
                assert_eq!(connect_packet[4], TNS_PACKET_TYPE_CONNECT);
                if attempt == 0 {
                    write_wire_packet(&mut server, TNS_PACKET_TYPE_REFUSE, b"temporary");
                } else {
                    let mut accept_data = [0u8; 37];
                    put_u16(&mut accept_data, 0, 319);
                    put_u32(&mut accept_data, 24, 8192);
                    write_wire_packet(&mut server, TNS_PACKET_TYPE_ACCEPT, &accept_data);
                }
            }
        });

        let connector = OracleNetConnector::new(ConnectOptions {
            retry_count: 1,
            retry_delay: Duration::ZERO,
            ..ConnectOptions::default()
        });
        let target = ConnectTarget::service_name("127.0.0.1", addr.port(), "FREE");
        let (_, accept) = connector.connect_tcp(&target).unwrap();

        assert_eq!(accept.protocol_version, 319);
        handle.join().unwrap();
    }

    #[test]
    fn accept_rejects_required_native_network_services() {
        let mut data = [0u8; 37];
        put_u16(&mut data, 0, 319);
        data[14] = TNS_NSI_NA_REQUIRED;

        let err = parse_accept_packet(&data).expect_err("required NNE should be rejected");

        assert!(err.to_string().contains("Native Network Encryption"));
    }

    #[test]
    fn accept_caps_sdu_to_smaller_tdu_like_go_ora_for_protocol_314() {
        let mut data = [0u8; 24];
        put_u16(&mut data, 0, 314);
        put_u16(&mut data, 4, 8192);
        put_u16(&mut data, 6, 4096);

        let accept = parse_accept_packet(&data).unwrap();

        assert_eq!(accept.sdu, 4096);
    }

    #[test]
    fn modern_accept_keeps_large_sdu_like_python_oracledb() {
        let mut data = [0u8; 37];
        put_u16(&mut data, 0, 319);
        put_u32(&mut data, 24, 131_072);
        put_u32(&mut data, 28, 65_536);

        let accept = parse_accept_packet(&data).unwrap();

        assert_eq!(accept.sdu, 131_072);
    }

    #[test]
    fn modern_accept_requires_large_sdu_like_python_oracledb() {
        let mut data = [0u8; 24];
        put_u16(&mut data, 0, 315);
        put_u16(&mut data, 4, 8192);

        let err = parse_accept_packet(&data).expect_err("short modern accept should fail");

        assert!(err.to_string().contains("protocol 315"));
    }

    #[test]
    fn protocol_318_accept_requires_flags2_like_python_oracledb() {
        let mut data = [0u8; 28];
        put_u16(&mut data, 0, 318);
        put_u32(&mut data, 24, 8192);

        let err = parse_accept_packet(&data).expect_err("short protocol 318 accept should fail");

        assert!(err.to_string().contains("protocol 318 flags"));
    }

    #[test]
    fn redirect_data_splits_address_and_reconnect_payload() {
        let redirect = "(ADDRESS=(PROTOCOL=tcp)(HOST=127.0.0.2)(PORT=1522))\0\
                        (DESCRIPTION=(CONNECT_DATA=(SERVICE_NAME=FREE)))";

        let parsed = parse_redirect_data(redirect).unwrap();

        assert_eq!(parsed.host, "127.0.0.2");
        assert_eq!(parsed.port, 1522);
        assert!(parsed.connect_data.contains("SERVICE_NAME=FREE"));
    }

    #[test]
    fn redirect_data_does_not_match_attribute_name_substrings() {
        let redirect = "(ADDRESS=(PROTOCOL=tcp)(MYHOST=bad)(XPORT=1522))\0\
                        (DESCRIPTION=(CONNECT_DATA=(SERVICE_NAME=FREE)))";

        let err = match parse_redirect_data(redirect) {
            Ok(_) => panic!("redirect HOST should be missing"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("missing HOST"));
    }

    #[test]
    fn redirect_data_uses_real_attribute_after_similar_name() {
        let redirect = "(ADDRESS=(PROTOCOL=tcp)(MYHOST=bad)(HOST=127.0.0.2)\
                        (XPORT=9999)(PORT=1522))\0\
                        (DESCRIPTION=(CONNECT_DATA=(SERVICE_NAME=FREE)))";

        let parsed = parse_redirect_data(redirect).unwrap();

        assert_eq!(parsed.host, "127.0.0.2");
        assert_eq!(parsed.port, 1522);
    }

    #[test]
    fn redirect_data_accepts_whitespace_before_attribute_name() {
        let redirect = "(ADDRESS=(PROTOCOL=tcp)( HOST = 127.0.0.2)( PORT = 1522))\0\
                        (DESCRIPTION=(CONNECT_DATA=(SERVICE_NAME=FREE)))";

        let parsed = parse_redirect_data(redirect).unwrap();

        assert_eq!(parsed.host, "127.0.0.2");
        assert_eq!(parsed.port, 1522);
    }

    fn read_wire_packet(stream: &mut TcpStream) -> Vec<u8> {
        let mut header = [0u8; 8];
        stream.read_exact(&mut header).unwrap();
        let packet_len = u16::from_be_bytes([header[0], header[1]]) as usize;
        let mut packet = header.to_vec();
        packet.resize(packet_len, 0);
        stream.read_exact(&mut packet[8..]).unwrap();
        packet
    }

    fn write_wire_packet(stream: &mut TcpStream, packet_type: u8, data: &[u8]) {
        let packet_len = 8 + data.len();
        let mut packet = vec![0u8; packet_len];
        put_u16(&mut packet, 0, packet_len as u16);
        packet[4] = packet_type;
        packet[8..].copy_from_slice(data);
        stream.write_all(&packet).unwrap();
        stream.flush().unwrap();
    }
}
