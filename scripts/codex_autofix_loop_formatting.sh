#!/usr/bin/env bash
set -euo pipefail

# Repeatedly invoke Codex CLI to review/fix the repository until the duration expires.
#
# Quick examples:
#   1) 기본 30분 실행
#      scripts/codex_autofix_loop.sh
#   2) 45분 동안 실행
#      scripts/codex_autofix_loop.sh 45
#   3) 모델 지정
#      CODEX_MODEL='gpt-5.3-codex-spark xhigh' scripts/codex_autofix_loop.sh 30
#      CODEX_MODEL='gpt-5.3-codex-spark' CODEX_REASONING_EFFORT='xhigh' scripts/codex_autofix_loop.sh 30
#   4) 자동 커밋 활성화
#      AUTO_COMMIT=1 scripts/codex_autofix_loop.sh 60

print_usage() {
  cat <<'USAGE'
Usage:
  scripts/codex_autofix_loop.sh [duration_minutes]

Environment variables:
  CODEX_CMD      Codex CLI executable (default: codex)
  CODEX_MODEL    Model name passed to Codex (optional)
                 If includes reasoning effort (e.g. "gpt-5.3-codex-spark xhigh"),
                 it will be parsed as model + reasoning effort.
  CODEX_REASONING_EFFORT  Optional reasoning effort (ex: xhigh, medium, low)
  CODEX_RUST_LOG Optional rust log filter for codex (default: off)
  AUTO_COMMIT    Set to 1 to auto-commit each Codex patch
  COMMIT_PREFIX  Commit prefix when AUTO_COMMIT=1 (default: "fix: auto-fix")
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  print_usage
  exit 0
fi

DURATION_MINUTES="${1:-30}"
START_TS="$(date +%s)"
END_TS="$((START_TS + DURATION_MINUTES * 60))"

CODEX_CMD="${CODEX_CMD:-codex}"
CODEX_MODEL="${CODEX_MODEL:-}"
CODEX_REASONING_EFFORT="${CODEX_REASONING_EFFORT:-}"
CODEX_RUST_LOG="${CODEX_RUST_LOG:-off}"
AUTO_COMMIT="${AUTO_COMMIT:-0}"
COMMIT_PREFIX="${COMMIT_PREFIX:-fix: auto-fix}"

run_codex_fix() {
  local prompt model reasoning_effort
  local -a cmd_args=()
  prompt=rompt="자동 포멧팅 기능에 버그 있을지 검토해줘. 버그가 있다면 다른 유사 케이스도 고려해서 모두 커버 되도록 일반화해서 근본적으로 개선해줘. 버그 발생시 항상 먼저 frame stack 구조로 depth 관리해서 해결 가능한지 우선 검토해줘. 그리고 토큰 순서대로 처리해야 나중에 문제가 안생겨. formatting.md 원칙에 맞게 수정 후 cargo test 검토 해줘."
  if [[ -n "$CODEX_MODEL" ]]; then
    if [[ "$CODEX_MODEL" == *" "* ]]; then
      model="${CODEX_MODEL%% *}"
      reasoning_effort="${CODEX_MODEL#* }"
      if [[ -n "$CODEX_REASONING_EFFORT" ]]; then
        reasoning_effort="$CODEX_REASONING_EFFORT"
      fi
      cmd_args+=(--model "$model")
      if [[ -n "$reasoning_effort" ]]; then
        cmd_args+=(-c "reasoning_effort=$reasoning_effort")
      fi
    elif [[ -n "$CODEX_REASONING_EFFORT" ]]; then
      cmd_args+=(--model "$CODEX_MODEL")
      cmd_args+=(-c "reasoning_effort=$CODEX_REASONING_EFFORT")
    else
      cmd_args+=(--model "$CODEX_MODEL")
    fi
  else
    cmd_args=()
  fi

  RUST_LOG="$CODEX_RUST_LOG" "$CODEX_CMD" "${cmd_args[@]}" exec -- "$prompt"
}

iteration=1
while [[ "$(date +%s)" -lt "$END_TS" ]]; do
  echo "=== Iteration $iteration ==="

  if ! run_codex_fix; then
    echo "[WARN] Codex invocation failed in iteration $iteration"
  fi

  echo "=== Iteration $iteration finished ==="
  sleep 60
  iteration="$((iteration + 1))"
done

echo "[TIMEOUT] Reached ${DURATION_MINUTES} minutes."
exit 0
