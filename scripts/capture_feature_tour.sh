#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This capture helper currently requires macOS and sips." >&2
  exit 1
fi

cargo build --bin capture_feature_tour

capture_home="${TMPDIR:-/tmp}/space-query-feature-tour"
mkdir -p "$capture_home" docs/images

HOME="$capture_home" target/debug/capture_feature_tour "${1:-}"

convert_capture() {
  local source_name="$1"
  local output_name="$2"
  sips -s format png "/tmp/space-query-${source_name}.ppm" \
    --out "docs/images/${output_name}.png" >/dev/null
}

if [[ "${1:-}" == "object-browser" ]]; then
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
