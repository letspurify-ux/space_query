use std::fs;
use std::path::Path;

fn read_script() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test_oracle_thin.sh");
    fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
}

#[test]
fn oracle_thin_script_defaults_to_aarch64_oci() {
    let script = read_script();

    assert!(
        script.contains("ORACLE_OCI_ARCH:-aarch64"),
        "test_oracle_thin.sh should default OCI architecture to aarch64"
    );
    assert!(
        script.contains("ORACLE_AARCH64_CLIENT_LIB_DIR:-/tmp/oqt_instantclient_23_26"),
        "test_oracle_thin.sh should default aarch64 OCI client to the local arm64 Instant Client"
    );
    assert!(
        script.contains("aarch64-apple-darwin"),
        "test_oracle_thin.sh should default macOS Cargo runs to an aarch64 target for aarch64 OCI"
    );
    assert!(
        script.contains("Oracle OCI client architecture mismatch"),
        "test_oracle_thin.sh should fail preflight on OCI client architecture mismatches"
    );
}

#[test]
fn oracle_thin_script_keeps_test_all_compare_harness_enabled_by_default() {
    let script = read_script();

    assert!(
        script.contains(r#"ORACLE_COMPARE_SQL="${ORACLE_COMPARE_SQL:-test/test_all.sql}""#),
        "test_oracle_thin.sh should default compare verification to test/test_all.sql"
    );
    assert!(
        script.contains("oracle_compare_test_all_live")
            && script.contains(r#""oracle_compare_test_all_protocol_${protocol}""#),
        "test_oracle_thin.sh should keep using the per-protocol test_all.sql compare harness"
    );
    assert!(
        script.contains(r#"RUN_COMPARE="${RUN_COMPARE:-1}""#),
        "test_oracle_thin.sh should run the compare harness by default"
    );
}
