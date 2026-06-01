use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use crate::{log_connect_phase, OracleThinError};

const TNS_PACKET_TYPE_CONNECT: u8 = 1;
const TNS_PACKET_TYPE_ACCEPT: u8 = 2;
const TNS_PACKET_TYPE_REFUSE: u8 = 4;
const TNS_PACKET_TYPE_REDIRECT: u8 = 5;
const TNS_GSO_DONT_CARE: u16 = 0x0001;
const TNS_GSO_CAN_RECV_ATTENTION: u16 = 0x0400;
const TNS_NSI_DISABLE_NA: u8 = 0x04;
const TNS_NSI_SUPPORT_SECURITY_RENEG: u8 = 0x80;
const TNS_PROTOCOL_CHARACTERISTICS: u16 = 0x4f98;
const TNS_CHECK_OOB: u32 = 0x01;
const TNS_ACCEPT_FLAG_CHECK_OOB: u32 = 0x00000001;
const TNS_ACCEPT_FLAG_FAST_AUTH: u32 = 0x10000000;
const TNS_ACCEPT_FLAG_HAS_END_OF_RESPONSE: u32 = 0x02000000;

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
            minimum_protocol_version: 300,
            desired_ttc_field_version: None,
            disable_oob_probe: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptInfo {
    pub protocol_version: u16,
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
        log_connect_phase("tcp-connect", &format!("{}:{}", target.host, target.port));
        let stream = TcpStream::connect((target.host.as_str(), target.port))?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
        self.connect_tns(stream, target)
    }

    fn connect_tns(
        &self,
        mut stream: TcpStream,
        target: &ConnectTarget,
    ) -> Result<(TcpStream, AcceptInfo), OracleThinError> {
        let connect_data = build_connect_data(target);
        let connect_packet = build_connect_packet(&self.options, connect_data.as_bytes())?;
        log_connect_phase(
            "tns-connect",
            &format!(
                "desired={} minimum={} disable_oob={} bytes={}",
                self.options.desired_protocol_version,
                self.options.minimum_protocol_version,
                self.options.disable_oob_probe,
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
                Ok((stream, accept))
            }
            TNS_PACKET_TYPE_REFUSE => Err(OracleThinError::new(format!(
                "Oracle listener refused connection: {}",
                String::from_utf8_lossy(&packet.data)
            ))),
            TNS_PACKET_TYPE_REDIRECT => Err(OracleThinError::new(
                "Oracle listener redirect is not implemented yet",
            )),
            other => Err(OracleThinError::new(format!(
                "unexpected TNS packet type {other} during connect"
            ))),
        }
    }
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
        sdu,
        supports_full_packet_size: true,
        flags2: flags2
            | if options & TNS_GSO_CAN_RECV_ATTENTION != 0 {
                TNS_ACCEPT_FLAG_CHECK_OOB
            } else {
                0
            },
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
