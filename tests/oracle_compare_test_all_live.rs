#![allow(
    clippy::cargo,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic,
    clippy::unwrap_used
)]

use std::path::Path;
use std::process::Command;

const TEST_ALL_SQL: &str = "test/test_all.sql";

#[test]
#[ignore = "requires local Oracle plus ORACLE_TEST_* and ORACLE_CLIENT_LIB_DIR environment variables"]
fn oracle_compare_test_all_protocol_314() -> Result<(), String> {
    run_oracle_compare_test_all_for_protocol(314)
}

#[test]
#[ignore = "requires local Oracle plus ORACLE_TEST_* and ORACLE_CLIENT_LIB_DIR environment variables"]
fn oracle_compare_test_all_protocol_315() -> Result<(), String> {
    run_oracle_compare_test_all_for_protocol(315)
}

#[test]
#[ignore = "requires local Oracle plus ORACLE_TEST_* and ORACLE_CLIENT_LIB_DIR environment variables"]
fn oracle_compare_test_all_protocol_318() -> Result<(), String> {
    run_oracle_compare_test_all_for_protocol(318)
}

#[test]
#[ignore = "requires local Oracle plus ORACLE_TEST_* and ORACLE_CLIENT_LIB_DIR environment variables"]
fn oracle_compare_test_all_protocol_319() -> Result<(), String> {
    run_oracle_compare_test_all_for_protocol(319)
}

fn run_oracle_compare_test_all_for_protocol(protocol: u16) -> Result<(), String> {
    if !Path::new(TEST_ALL_SQL).is_file() {
        return Err(format!("{TEST_ALL_SQL} does not exist"));
    }

    let output = Command::new(ORACLE_COMPARE_TEST_ALL_BIN)
        .arg(TEST_ALL_SQL)
        .env("ORACLE_THIN_DESIRED_PROTOCOL", protocol.to_string())
        .output()
        .map_err(|err| format!("failed to run oracle_compare_test_all for {protocol}: {err}"))?;

    if output.status.success() {
        return Ok(());
    }

    Err(format!(
        "oracle_compare_test_all failed for protocol {protocol} with status {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        output_text(&output.stdout),
        output_text(&output.stderr)
    ))
}

const ORACLE_COMPARE_TEST_ALL_BIN: &str = env!("CARGO_BIN_EXE_oracle_compare_test_all");

fn output_text(output: &[u8]) -> String {
    String::from_utf8_lossy(output).into_owned()
}
