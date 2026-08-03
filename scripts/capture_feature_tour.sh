#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This capture helper currently requires macOS and sips." >&2
  exit 1
fi

cargo build --bin capture_feature_tour

capture_root="${TMPDIR:-/tmp}/space-query-feature-tour"
capture_config_dir="$capture_root/config"
capture_data_dir="$capture_root/data"
capture_mode="${1:-readme}"
mkdir -p "$capture_config_dir" "$capture_data_dir" docs/images

SPACE_QUERY_CONFIG_DIR="$capture_config_dir" \
  SPACE_QUERY_DATA_DIR="$capture_data_dir" \
  target/debug/capture_feature_tour "$capture_mode"

convert_capture() {
  local source_name="$1"
  local output_name="$2"
  sips -s format png "/tmp/space-query-${source_name}.ppm" \
    --out "docs/images/${output_name}.png" >/dev/null
}

if [[ "$capture_mode" == "object-browser" ]]; then
  convert_capture object-browser object-browser
  exit 0
fi

convert_capture main main-window
convert_capture connect connection-dialog
convert_capture intellisense intellisense
convert_capture signature signature-popup
convert_capture object-browser object-browser
convert_capture formatting-before sql-formatting-before
convert_capture formatting-after sql-formatting-after
convert_capture result-grid result-grid
convert_capture result-editing result-grid-editing
convert_capture settings settings
convert_capture query-history query-history
convert_capture session-activity session-activity
convert_capture application-log application-log
