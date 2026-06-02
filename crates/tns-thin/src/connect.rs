// Portions of this file are derived from the thin protocol implementation in
// python-oracledb (https://github.com/oracle/python-oracledb),
// Copyright (c) 2016, 2026, Oracle and/or its affiliates, used under the
// Apache License, Version 2.0. Protocol constants were also cross-checked
// against go-ora (MIT License, Copyright (c) 2020 Samy Sultan).
// See THIRD_PARTY_NOTICES.md.

use std::io::{Read, Write};
use std::net::TcpStream;
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
const TNS_MAX_REDIRECTS: usize = 5;
const TNS_MAX_RESENDS: usize = 3;
pub(crate) const TNS_MIN_SUPPORTED_PROTOCOL: u16 = 314;
pub(crate) const TNS_DEFAULT_SOCKET_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectTarget {
    pub host: String,
    pub port: u16,
    pub service_name: String,
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
        }
    }

    pub fn easy_connect_string(&self) -> String {
        format!("//{}:{}/{}", self.host, self.port, self.service_name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectOptions {
    pub desired_protocol_version: u16,
    pub minimum_protocol_version: u16,
    pub desired_ttc_field_version: Option<u8>,
    pub disable_oob_probe: bool,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            desired_protocol_version: 319,
            minimum_protocol_version: TNS_MIN_SUPPORTED_PROTOCOL,
            desired_ttc_field_version: None,
            disable_oob_probe: true,
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
        let mut host = target.host.clone();
        let mut port = target.port;
        let mut connect_data = build_connect_data(target);
        let mut packet_flags = 0;

        for _ in 0..=TNS_MAX_REDIRECTS {
            log_connect_phase("tcp-connect", &format!("{host}:{port}"));
            let stream = TcpStream::connect((host.as_str(), port))?;
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
        stream.write_all(&connect_packet)?;
        stream.flush()?;

        for _ in 0..=TNS_MAX_RESENDS {
            let packet = read_tns_packet(&mut stream)?;
            match packet.packet_type {
                TNS_PACKET_TYPE_ACCEPT => {
                    let accept = parse_accept_packet(&packet.data)?;
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
                    return Err(OracleThinError::new(format!(
                        "Oracle listener refused connection: {}",
                        String::from_utf8_lossy(&packet.data)
                    )));
                }
                TNS_PACKET_TYPE_REDIRECT => {
                    return Ok(ConnectOutcome::Redirect(read_redirect_data(
                        &mut stream,
                        &packet.data,
                    )?));
                }
                TNS_PACKET_TYPE_RESEND => {
                    log_connect_phase("tns-resend", "resending connect packet");
                    stream.write_all(&connect_packet)?;
                    stream.flush()?;
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

fn build_connect_data(target: &ConnectTarget) -> String {
    format!(
        "(DESCRIPTION=(ADDRESS=(PROTOCOL=TCP)(HOST={})(PORT={}))(CONNECT_DATA=(SERVICE_NAME={})(CID=(PROGRAM=space-query-thin)(HOST=localhost)(USER=space-query))))",
        target.host, target.port, target.service_name
    )
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
    let packet_len = usize::from(data_offset) + connect_data.len();
    let packet_len_u16 = u16::try_from(packet_len).map_err(|_| {
        OracleThinError::new(format!(
            "Oracle connect packet is too large: {packet_len} bytes"
        ))
    })?;
    let sdu = 8192u16;
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
    put_u16(&mut packet, 14, sdu);
    put_u16(&mut packet, 16, sdu);
    put_u16(&mut packet, 18, TNS_PROTOCOL_CHARACTERISTICS);
    put_u16(&mut packet, 22, 1);
    put_u16(&mut packet, 24, connect_len);
    put_u16(&mut packet, 26, data_offset);
    let nsi_flags = TNS_NSI_SUPPORT_SECURITY_RENEG | TNS_NSI_DISABLE_NA;
    packet[32] = nsi_flags;
    packet[33] = nsi_flags;
    put_u32(&mut packet, 58, u32::from(sdu));
    put_u32(&mut packet, 62, u32::from(sdu));
    put_u32(&mut packet, 70, connect_flags_2);
    packet[usize::from(data_offset)..].copy_from_slice(connect_data);
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
        if bytes[index..].starts_with(key_bytes)
            || bytes[index..]
                .get(..key_bytes.len())
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(key_bytes))
        {
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
    let mut sdu = if protocol_version >= 315 && data.len() >= 32 {
        read_u32(data, 24)?
    } else {
        u32::from(read_u16(data, 4)?)
    };
    if sdu == 0 {
        sdu = 8192;
    }
    let flags2 = if protocol_version >= 318 && data.len() >= 37 {
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

    use super::{
        build_connect_packet, parse_accept_packet, parse_redirect_data, put_u16, put_u32,
        ConnectOptions, OracleNetConnector, TNS_MIN_SUPPORTED_PROTOCOL, TNS_NSI_NA_REQUIRED,
        TNS_PACKET_FLAG_REDIRECT, TNS_PACKET_TYPE_ACCEPT, TNS_PACKET_TYPE_CONNECT,
        TNS_PACKET_TYPE_RESEND,
    };

    #[test]
    fn default_connect_options_request_protocol_314_or_newer() {
        let options = ConnectOptions::default();

        assert_eq!(options.minimum_protocol_version, TNS_MIN_SUPPORTED_PROTOCOL);
        assert_eq!(options.desired_protocol_version, 319);
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
    fn accept_rejects_required_native_network_services() {
        let mut data = [0u8; 37];
        put_u16(&mut data, 0, 319);
        data[14] = TNS_NSI_NA_REQUIRED;

        let err = parse_accept_packet(&data).expect_err("required NNE should be rejected");

        assert!(err.to_string().contains("Native Network Encryption"));
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
