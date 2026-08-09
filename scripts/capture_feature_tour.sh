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
  local source_path="/tmp/space-query-${source_name}.ppm"
  local output_path="docs/images/${output_name}.png"
  local magic source_width source_height max_value
  {
    read -r magic
    read -r source_width source_height
    read -r max_value
  } <"$source_path"
  if [[ "$magic" != "P6" || "$max_value" != "255" ]]; then
    echo "Unexpected PPM header in $source_path" >&2
    exit 1
  fi

  # PNG compression is lossless. Do not resize the captured pixel buffer.
  sips -s format png "$source_path" --out "$output_path" >/dev/null

  local output_width=""
  local output_height=""
  while read -r property value; do
    case "$property" in
      pixelWidth:) output_width="$value" ;;
      pixelHeight:) output_height="$value" ;;
    esac
  done < <(sips -g pixelWidth -g pixelHeight "$output_path")
  if [[ "$output_width" != "$source_width" || "$output_height" != "$source_height" ]]; then
    echo "Capture resolution changed: ${source_width}x${source_height} -> ${output_width}x${output_height}" >&2
    exit 1
  fi
}

# Every single-scene mode needs a branch here. Without one it falls through to
# the full list at the bottom, which converts whatever stale PPMs happen to be
# left in /tmp and silently overwrites unrelated images.
if [[ "$capture_mode" == "connection-dialog" ]]; then
  convert_capture connect connection-dialog
  exit 0
fi

if [[ "$capture_mode" == "column-layout" ]]; then
  convert_capture column-layout column-layout
  exit 0
fi

if [[ "$capture_mode" == "value-filter" ]]; then
  convert_capture value-filter value-filter
  exit 0
fi

if [[ "$capture_mode" == "grid-sort" ]]; then
  convert_capture grid-sort grid-sort
  exit 0
fi

if [[ "$capture_mode" == "tree-columns" ]]; then
  convert_capture tree-columns tree-columns
  exit 0
fi

if [[ "$capture_mode" == "settings-dialog" ]]; then
  convert_capture settings settings
  exit 0
fi

if [[ "$capture_mode" == "object-browser" ]]; then
  convert_capture object-browser object-browser
  exit 0
fi

if [[ "$capture_mode" == "grid-sql-export" ]]; then
  convert_capture grid-sql-export grid-sql-export
  exit 0
fi

if [[ "$capture_mode" == "result-export" ]]; then
  convert_capture result-export result-export
  exit 0
fi

if [[ "$capture_mode" == "table-import" ]]; then
  convert_capture table-import table-import
  exit 0
fi

if [[ "$capture_mode" == "bind-parameters" ]]; then
  convert_capture bind-parameters bind-parameters
  exit 0
fi

if [[ "$capture_mode" == "grid-search" ]]; then
  convert_capture grid-search grid-search
  exit 0
fi

if [[ "$capture_mode" == "selection-summary" ]]; then
  convert_capture selection-summary selection-summary
  exit 0
fi

if [[ "$capture_mode" == "code-snippets" ]]; then
  convert_capture code-snippets code-snippets
  convert_capture snippet-reference snippet-reference
  exit 0
fi

if [[ "$capture_mode" == "explain-plan" ]]; then
  convert_capture explain-plan explain-plan
  exit 0
fi

if [[ "$capture_mode" == "object-search" ]]; then
  convert_capture object-search object-search
  exit 0
fi

if [[ "$capture_mode" == "soft-wrap" ]]; then
  convert_capture soft-wrap soft-wrap
  exit 0
fi

if [[ "$capture_mode" == "value-viewer" ]]; then
  convert_capture value-viewer value-viewer
  exit 0
fi

if [[ "$capture_mode" == "connection-color" ]]; then
  convert_capture connection-color connection-color
  exit 0
fi

if [[ "$capture_mode" == "connection-color-tabs" ]]; then
  convert_capture connection-color-tabs connection-color-tabs
  exit 0
fi

if [[ "$capture_mode" == "object-drop-confirmation" ]]; then
  convert_capture object-drop-confirmation object-drop-confirmation
  exit 0
fi

if [[ "$capture_mode" == "table-browse-popup" ]]; then
  convert_capture table-browse table-browse
  convert_capture table-browse-popup-100 table-browse-popup
  convert_capture table-browse-order-popup-100 table-browse-order-popup
  exit 0
fi

if [[ "$capture_mode" == "table-browse-input-regression" ]]; then
  exit 0
fi

convert_capture main main-window
convert_capture connect connection-dialog
convert_capture connection-color connection-color
convert_capture connection-color-tabs connection-color-tabs
convert_capture code-completion code-completion
convert_capture signature signature-popup
convert_capture object-browser object-browser
convert_capture formatting-before sql-formatting-before
convert_capture formatting-after sql-formatting-after
convert_capture result-grid result-grid
convert_capture value-viewer value-viewer
convert_capture grid-search grid-search
convert_capture selection-summary selection-summary
convert_capture code-snippets code-snippets
convert_capture snippet-reference snippet-reference
convert_capture soft-wrap soft-wrap
convert_capture explain-plan explain-plan
convert_capture object-search object-search
convert_capture object-drop-confirmation object-drop-confirmation
convert_capture grid-sql-export grid-sql-export
convert_capture result-export result-export
convert_capture table-import table-import
convert_capture bind-parameters bind-parameters
convert_capture table-browse table-browse
convert_capture table-browse-popup-100 table-browse-popup
convert_capture table-browse-order-popup-100 table-browse-order-popup
convert_capture result-editing result-grid-editing
convert_capture settings settings
convert_capture query-history query-history
convert_capture session-activity session-activity
convert_capture application-log application-log
convert_capture column-layout column-layout
convert_capture value-filter value-filter
convert_capture grid-sort grid-sort
convert_capture tree-columns tree-columns
