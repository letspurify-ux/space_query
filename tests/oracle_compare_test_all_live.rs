#![allow(
    clippy::cargo,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic,
    clippy::unwrap_used
)]

use std::fs;
use std::path::{Path, PathBuf};
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

#[test]
#[ignore = "requires local Oracle plus ORACLE_TEST_* and ORACLE_CLIENT_LIB_DIR environment variables"]
fn oracle_compare_all_fixture_files_protocol_314() -> Result<(), String> {
    run_oracle_compare_all_fixture_files_for_protocol(314)
}

#[test]
#[ignore = "requires local Oracle plus ORACLE_TEST_* and ORACLE_CLIENT_LIB_DIR environment variables"]
fn oracle_compare_all_fixture_files_protocol_315() -> Result<(), String> {
    run_oracle_compare_all_fixture_files_for_protocol(315)
}

#[test]
#[ignore = "requires local Oracle plus ORACLE_TEST_* and ORACLE_CLIENT_LIB_DIR environment variables"]
fn oracle_compare_all_fixture_files_protocol_318() -> Result<(), String> {
    run_oracle_compare_all_fixture_files_for_protocol(318)
}

#[test]
#[ignore = "requires local Oracle plus ORACLE_TEST_* and ORACLE_CLIENT_LIB_DIR environment variables"]
fn oracle_compare_all_fixture_files_protocol_319() -> Result<(), String> {
    run_oracle_compare_all_fixture_files_for_protocol(319)
}

fn run_oracle_compare_test_all_for_protocol(protocol: u16) -> Result<(), String> {
    run_oracle_compare_for_path(Path::new(TEST_ALL_SQL), protocol)
}

fn run_oracle_compare_all_fixture_files_for_protocol(protocol: u16) -> Result<(), String> {
    for path in oracle_fixture_paths()? {
        run_oracle_compare_for_path(&path, protocol)?;
    }
    Ok(())
}

fn oracle_fixture_paths() -> Result<Vec<PathBuf>, String> {
    let mut paths = fs::read_dir("test")
        .map_err(|err| format!("failed to read test directory: {err}"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to read test directory entry: {err}"))?;
    paths.retain(|path| {
        path.is_file()
            && matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("sql" | "txt")
            )
    });
    paths.sort();
    Ok(paths)
}

fn run_oracle_compare_for_path(path: &Path, protocol: u16) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("{} does not exist", path.display()));
    }

    let output = Command::new(ORACLE_COMPARE_TEST_ALL_BIN)
        .arg(path)
        .env("ORACLE_THIN_DESIRED_PROTOCOL", protocol.to_string())
        .output()
        .map_err(|err| {
            format!(
                "failed to run oracle_compare_test_all for protocol {protocol}, {}: {err}",
                path.display()
            )
        })?;

    if output.status.success() {
        return Ok(());
    }

    Err(format!(
        "oracle_compare_test_all failed for protocol {protocol}, {} with status {}\nstdout:\n{}\nstderr:\n{}",
        path.display(),
        output.status,
        output_text(&output.stdout),
        output_text(&output.stderr)
    ))
}

const ORACLE_COMPARE_TEST_ALL_BIN: &str = env!("CARGO_BIN_EXE_oracle_compare_test_all");

fn output_text(output: &[u8]) -> String {
    String::from_utf8_lossy(output).into_owned()
}
