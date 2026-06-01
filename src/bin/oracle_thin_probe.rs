use std::env;
use std::fmt::Display;
use std::str::FromStr;

use oracle_thin::connect::{ConnectOptions, ConnectTarget, OracleNetConnector};
use oracle_thin::exec::StatementRequest;
use oracle_thin::session::{OracleThinConfig, OracleThinSession};

fn parse_env_value<T>(name: &str) -> Option<T>
where
    T: FromStr,
    T::Err: Display,
{
    match env::var(name) {
        Ok(value) => match value.parse() {
            Ok(parsed) => Some(parsed),
            Err(err) => {
                eprintln!("invalid {name}: {err}");
                std::process::exit(2);
            }
        },
        Err(_) => None,
    }
}

fn main() {
    let host = env::var("ORACLE_THIN_TEST_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = env::var("ORACLE_THIN_TEST_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(1521);
    let service = env::var("ORACLE_THIN_TEST_SERVICE").unwrap_or_else(|_| "FREE".to_string());
    let username = env::var("ORACLE_THIN_TEST_USERNAME").unwrap_or_else(|_| "system".to_string());
    let password = env::var("ORACLE_THIN_TEST_PASSWORD").unwrap_or_else(|_| "password".to_string());
    let target = ConnectTarget::service_name(host, port, service);
    let mut connect_options = ConnectOptions {
        disable_oob_probe: true,
        ..ConnectOptions::default()
    };
    if let Some(value) = parse_env_value("ORACLE_THIN_DESIRED_PROTOCOL") {
        connect_options.desired_protocol_version = value;
    }
    if let Some(value) = parse_env_value("ORACLE_THIN_MINIMUM_PROTOCOL") {
        connect_options.minimum_protocol_version = value;
    }
    if let Some(value) = parse_env_value("ORACLE_THIN_TTC_FIELD_VERSION") {
        connect_options.desired_ttc_field_version = Some(value);
    }

    let connector = OracleNetConnector::new(connect_options.clone());
    match connector.connect_tcp(&target) {
        Ok((_stream, accept)) => {
            eprintln!(
                "accept protocol={} sdu={} full_packet={} flags2=0x{:x}",
                accept.protocol_version,
                accept.sdu,
                accept.supports_full_packet_size,
                accept.flags2
            );
        }
        Err(err) => {
            eprintln!("accept error: {err:?}");
            return;
        }
    }

    let mut config = OracleThinConfig::new(target, username, password);
    config.connect_options = connect_options;
    match OracleThinSession::connect(config) {
        Ok(mut session) => {
            let caps = session.capabilities();
            eprintln!(
                "session version={:?} caps protocol={:?} ttc_field_version={} max_string_size={} sql_boolean={} eor={} request_boundaries={} fast_auth={} oob={}",
                session.server_version(),
                caps.protocol_version,
                caps.ttc_field_version,
                caps.max_string_size,
                caps.supports_sql_boolean,
                caps.supports_end_of_response,
                caps.supports_request_boundaries,
                caps.supports_fast_auth,
                caps.supports_oob,
            );
            let mut request = StatementRequest::select_one_from_dual();
            request.prefetch_rows = 0;
            match session.execute(&request, 1) {
                Ok(result) => eprintln!("execute no prefetch ok: {:?}", result.rows),
                Err(err) => eprintln!("execute no prefetch error: {err:?}"),
            }
            match session.query("select 1 from dual", 1) {
                Ok(result) => eprintln!("query ok: {:?}", result.rows),
                Err(err) => eprintln!("query error: {err:?}"),
            }
        }
        Err(err) => eprintln!("session connect error: {err:?}"),
    }
}
