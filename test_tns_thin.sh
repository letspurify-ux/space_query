#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  ./test_tns_thin.sh [protocol ...]

Defaults:
  Runs TNS thin live DB tests and test/test_all.sql compare tests for protocols: 314 315 318 319

Environment:
  ORACLE_TEST_HOST             default: 127.0.0.1
  ORACLE_TEST_PORT             default: 1521
  ORACLE_TEST_SERVICE_NAME     default: FREE
  ORACLE_TEST_USERNAME         default: system
  ORACLE_TEST_PASSWORD         default: password
  ORACLE_OCI_ARCH              default: aarch64
  ORACLE_CLIENT_LIB_DIR        default: aarch64 Instant Client path
  ORACLE_AARCH64_CLIENT_LIB_DIR default: /tmp/oqt_instantclient_23_26
  CARGO_BUILD_TARGET           default: aarch64-apple-darwin on macOS for aarch64 OCI
  ORACLE_THIN_LIVE_PROTOCOLS   optional space-separated protocol list
  INCLUDE_LARGE_TYPES=1        include large_chunk_candidate_types live_tns tests
  RUN_MAIN_CRATE=0             skip space_query TNS thin ignored live tests
  RUN_LIVE_TNS=0               skip tns-thin crate live_tns ignored live tests
  RUN_UNIT_REGRESSION=0        skip focused non-live regression tests
  RUN_COMPARE=0                skip oracle_compare_test_all live test for each protocol
  SKIP_PREFLIGHT=1             skip listener/client-library preflight checks
  ORACLE_COMPARE_SQL           default: test/test_all.sql
  ORACLE_COMPARE_TIMEOUT_SECS  default used by harness if set

Examples:
  ./test_tns_thin.sh
  ./test_tns_thin.sh 314
  INCLUDE_LARGE_TYPES=1 ./test_tns_thin.sh 318
  RUN_COMPARE=0 ./test_tns_thin.sh 314 315 318 319
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

host_os() {
  uname -s 2>/dev/null || echo unknown
}

normalize_oci_arch() {
  case "$1" in
    aarch64 | arm64)
      echo "aarch64"
      ;;
    x86_64 | amd64)
      echo "x86_64"
      ;;
    *)
      echo "$1"
      ;;
  esac
}

default_client_lib_dir_for_arch() {
  case "$1" in
    aarch64)
      echo "${ORACLE_AARCH64_CLIENT_LIB_DIR:-/tmp/oqt_instantclient_23_26}"
      ;;
    x86_64)
      echo "${ORACLE_X86_64_CLIENT_LIB_DIR:-/tmp/oqt_instantclient_x86_64}"
      ;;
    *)
      echo ""
      ;;
  esac
}

cargo_target_for_oci_arch() {
  local os="$1"
  local arch="$2"

  case "$os:$arch" in
    Darwin:aarch64)
      echo "aarch64-apple-darwin"
      ;;
    Darwin:x86_64)
      echo "x86_64-apple-darwin"
      ;;
    Linux:aarch64)
      echo "aarch64-unknown-linux-gnu"
      ;;
    Linux:x86_64)
      echo "x86_64-unknown-linux-gnu"
      ;;
    *)
      echo ""
      ;;
  esac
}

client_library_name() {
  case "$1" in
    Darwin)
      echo "libclntsh.dylib"
      ;;
    Linux)
      echo "libclntsh.so"
      ;;
    MINGW* | MSYS* | CYGWIN*)
      echo "oci.dll"
      ;;
    *)
      echo "libclntsh.dylib"
      ;;
  esac
}

expected_file_arch_description() {
  case "$1:$2" in
    Darwin:aarch64)
      echo "arm64"
      ;;
    Linux:aarch64)
      echo "aarch64"
      ;;
    Darwin:x86_64)
      echo "x86_64"
      ;;
    Linux:x86_64)
      echo "x86-64"
      ;;
    *)
      echo "$2"
      ;;
  esac
}

file_info_matches_oci_arch() {
  local os="$1"
  local arch="$2"
  local file_info="$3"

  case "$os:$arch" in
    Darwin:aarch64)
      [[ "$file_info" == *"arm64"* ]]
      ;;
    Linux:aarch64)
      [[ "$file_info" == *"aarch64"* || "$file_info" == *"ARM64"* ]]
      ;;
    Darwin:x86_64)
      [[ "$file_info" == *"x86_64"* ]]
      ;;
    Linux:x86_64)
      [[ "$file_info" == *"x86-64"* || "$file_info" == *"x86_64"* ]]
      ;;
    *)
      [[ "$file_info" == *"$arch"* ]]
      ;;
  esac
}

if [[ "$#" -gt 0 ]]; then
  PROTOCOLS=("$@")
elif [[ -n "${ORACLE_THIN_LIVE_PROTOCOLS:-}" ]]; then
  # shellcheck disable=SC2206
  PROTOCOLS=(${ORACLE_THIN_LIVE_PROTOCOLS})
else
  PROTOCOLS=(314 315 318 319)
fi

export ORACLE_TEST_HOST="${ORACLE_TEST_HOST:-127.0.0.1}"
export ORACLE_TEST_PORT="${ORACLE_TEST_PORT:-1521}"
export ORACLE_TEST_SERVICE_NAME="${ORACLE_TEST_SERVICE_NAME:-FREE}"
export ORACLE_TEST_SERVICE="${ORACLE_TEST_SERVICE:-$ORACLE_TEST_SERVICE_NAME}"
export ORACLE_TEST_USERNAME="${ORACLE_TEST_USERNAME:-system}"
export ORACLE_TEST_PASSWORD="${ORACLE_TEST_PASSWORD:-password}"

HOST_OS="$(host_os)"
export ORACLE_OCI_ARCH="$(normalize_oci_arch "${ORACLE_OCI_ARCH:-aarch64}")"
DEFAULT_ORACLE_CLIENT_LIB_DIR="$(default_client_lib_dir_for_arch "$ORACLE_OCI_ARCH")"
if [[ -z "$DEFAULT_ORACLE_CLIENT_LIB_DIR" ]]; then
  echo "unsupported ORACLE_OCI_ARCH: $ORACLE_OCI_ARCH" >&2
  exit 1
fi
export ORACLE_CLIENT_LIB_DIR="${ORACLE_CLIENT_LIB_DIR:-$DEFAULT_ORACLE_CLIENT_LIB_DIR}"

if [[ -z "${CARGO_BUILD_TARGET:-}" ]]; then
  DEFAULT_CARGO_BUILD_TARGET="$(cargo_target_for_oci_arch "$HOST_OS" "$ORACLE_OCI_ARCH")"
  if [[ -n "$DEFAULT_CARGO_BUILD_TARGET" ]]; then
    export CARGO_BUILD_TARGET="$DEFAULT_CARGO_BUILD_TARGET"
  fi
fi

export ORACLE_THIN_TEST_HOST="${ORACLE_THIN_TEST_HOST:-$ORACLE_TEST_HOST}"
export ORACLE_THIN_TEST_PORT="${ORACLE_THIN_TEST_PORT:-$ORACLE_TEST_PORT}"
export ORACLE_THIN_TEST_SERVICE="${ORACLE_THIN_TEST_SERVICE:-$ORACLE_TEST_SERVICE_NAME}"
export ORACLE_THIN_TEST_USERNAME="${ORACLE_THIN_TEST_USERNAME:-$ORACLE_TEST_USERNAME}"
export ORACLE_THIN_TEST_PASSWORD="${ORACLE_THIN_TEST_PASSWORD:-$ORACLE_TEST_PASSWORD}"

RUN_MAIN_CRATE="${RUN_MAIN_CRATE:-1}"
RUN_LIVE_TNS="${RUN_LIVE_TNS:-1}"
RUN_UNIT_REGRESSION="${RUN_UNIT_REGRESSION:-1}"
RUN_COMPARE="${RUN_COMPARE:-1}"
SKIP_PREFLIGHT="${SKIP_PREFLIGHT:-0}"
ORACLE_COMPARE_SQL="${ORACLE_COMPARE_SQL:-test/test_all.sql}"

LIVE_TNS_SKIP_ARGS=()
if [[ "${INCLUDE_LARGE_TYPES:-0}" != "1" ]]; then
  LIVE_TNS_SKIP_ARGS+=(--skip large_chunk_candidate_types)
fi

print_config() {
  cat <<CONFIG
TNS thin live test configuration
  protocols: ${PROTOCOLS[*]}
  host: $ORACLE_TEST_HOST
  port: $ORACLE_TEST_PORT
  service: $ORACLE_TEST_SERVICE_NAME
  username: $ORACLE_TEST_USERNAME
  OCI arch: $ORACLE_OCI_ARCH
  client lib: $ORACLE_CLIENT_LIB_DIR
  cargo target: ${CARGO_BUILD_TARGET:-host default}
  include large types: ${INCLUDE_LARGE_TYPES:-0}
  run tns-thin live_tns: $RUN_LIVE_TNS
  run space_query TNS thin: $RUN_MAIN_CRATE
  run unit regression: $RUN_UNIT_REGRESSION
  run compare harness: $RUN_COMPARE
  compare SQL: $ORACLE_COMPARE_SQL
  skip preflight: $SKIP_PREFLIGHT
CONFIG
}

check_prereqs() {
  if [[ "$SKIP_PREFLIGHT" == "1" ]]; then
    echo "Skipping preflight checks."
    return
  fi
  if ! command -v cargo >/dev/null 2>&1; then
    echo "cargo is required but was not found in PATH" >&2
    exit 1
  fi
  if command -v nc >/dev/null 2>&1; then
    if ! nc -vz "$ORACLE_TEST_HOST" "$ORACLE_TEST_PORT"; then
      echo "Oracle listener preflight failed for $ORACLE_TEST_HOST:$ORACLE_TEST_PORT" >&2
      echo "Set ORACLE_TEST_HOST/ORACLE_TEST_PORT correctly, or use SKIP_PREFLIGHT=1." >&2
      exit 1
    fi
  else
    echo "warning: nc not found; skipping listener TCP preflight" >&2
  fi
  if [[ "$RUN_MAIN_CRATE" == "1" || "$RUN_COMPARE" == "1" ]]; then
    local client_lib="$ORACLE_CLIENT_LIB_DIR/$(client_library_name "$HOST_OS")"
    if [[ ! -r "$client_lib" ]]; then
      echo "warning: $client_lib is not readable" >&2
      echo "         OCI-backed tests or compare harness may fail if they need Instant Client." >&2
      return
    fi

    if [[ "$HOST_OS" == "Darwin" && -n "${CARGO_BUILD_TARGET:-}" ]]; then
      case "$ORACLE_OCI_ARCH:$CARGO_BUILD_TARGET" in
        aarch64:aarch64-* | x86_64:x86_64-*)
          ;;
        *)
          echo "Oracle OCI architecture mismatch: ORACLE_OCI_ARCH=$ORACLE_OCI_ARCH but CARGO_BUILD_TARGET=$CARGO_BUILD_TARGET" >&2
          echo "Use a matching Cargo target or override ORACLE_OCI_ARCH/ORACLE_CLIENT_LIB_DIR together." >&2
          exit 1
          ;;
      esac
    fi

    if command -v file >/dev/null 2>&1; then
      local expected_file_arch
      local client_file_info
      expected_file_arch="$(expected_file_arch_description "$HOST_OS" "$ORACLE_OCI_ARCH")"
      client_file_info="$(file "$client_lib")"
      if ! file_info_matches_oci_arch "$HOST_OS" "$ORACLE_OCI_ARCH" "$client_file_info"; then
        echo "Oracle OCI client architecture mismatch for $client_lib" >&2
        echo "expected: $expected_file_arch" >&2
        echo "actual:   $client_file_info" >&2
        exit 1
      fi
    fi
  fi
}

run_unit_regression() {
  echo
  echo "== Unit regression: described bind projection =="
  cargo test --manifest-path crates/tns-thin/Cargo.toml described_
}

run_live_tns_for_protocol() {
  local protocol="$1"
  echo
  echo "== tns-thin live_tns protocol $protocol =="
  ORACLE_THIN_DESIRED_PROTOCOL="$protocol" \
    cargo test --manifest-path crates/tns-thin/Cargo.toml --test live_tns -- \
      --ignored \
      --nocapture \
      --test-threads=1 \
      "${LIVE_TNS_SKIP_ARGS[@]}"
}

run_main_crate_for_protocol() {
  local protocol="$1"
  echo
  echo "== space_query TNS thin ignored live tests protocol $protocol =="
  ORACLE_THIN_DESIRED_PROTOCOL="$protocol" \
    cargo test oracle_thin --lib -- \
      --ignored \
      --nocapture \
      --test-threads=1
}

run_compare_for_protocol() {
  local protocol="$1"
  echo
  if [[ "$ORACLE_COMPARE_SQL" == "test/test_all.sql" ]]; then
    echo "== oracle_compare_test_all live test protocol $protocol =="
    ORACLE_THIN_DESIRED_PROTOCOL="$protocol" \
      cargo test --test oracle_compare_test_all_live \
        "oracle_compare_test_all_protocol_${protocol}" -- \
        --ignored \
        --nocapture \
        --test-threads=1
  else
    echo "== oracle_compare_test_all protocol $protocol ($ORACLE_COMPARE_SQL) =="
    ORACLE_THIN_DESIRED_PROTOCOL="$protocol" \
      cargo run --bin oracle_compare_test_all -- "$ORACLE_COMPARE_SQL"
  fi
}

print_config
check_prereqs

if [[ "$RUN_UNIT_REGRESSION" == "1" ]]; then
  run_unit_regression
fi

for protocol in "${PROTOCOLS[@]}"; do
  if [[ ! "$protocol" =~ ^[0-9]+$ ]]; then
    echo "invalid protocol value: $protocol" >&2
    exit 1
  fi
  if [[ "$RUN_LIVE_TNS" == "1" ]]; then
    run_live_tns_for_protocol "$protocol"
  fi
  if [[ "$RUN_MAIN_CRATE" == "1" ]]; then
    run_main_crate_for_protocol "$protocol"
  fi
  if [[ "$RUN_COMPARE" == "1" ]]; then
    run_compare_for_protocol "$protocol"
  fi
done

echo
echo "All requested TNS thin tests passed."
