#!/usr/bin/env bash
set -euo pipefail

e2e_script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
e2e_tests_root="$(cd "$e2e_script_dir/.." && pwd)"
e2e_repo_root="$(cd "$e2e_tests_root/../.." && pwd)"
e2e_scenarios_root="$e2e_tests_root/scenarios"
e2e_bin_dir="$e2e_tests_root/bin"
e2e_output_root="${E2E_OUTPUT_ROOT:-$e2e_repo_root/target/e2e}"
e2e_run_id="${E2E_RUN_ID:-$(date +%Y%m%d-%H%M%S)-$$}"
e2e_suite="${E2E_SUITE:-deterministic}"
e2e_tag_filter="${E2E_TAG:-}"
e2e_requested_scenario="${E2E_SCENARIO:-}"

export E2E_OUTPUT_ROOT="$e2e_output_root"
export E2E_RUN_ID="$e2e_run_id"
export E2E_SUITE="$e2e_suite"
export E2E_TAG="$e2e_tag_filter"
export E2E_SCENARIO="$e2e_requested_scenario"

e2e_fail() {
  echo "e2e: $*" >&2
  exit 2
}

e2e_is_true() {
  case "${1:-}" in
    true|TRUE|True|1|yes|YES|Yes) return 0 ;;
    false|FALSE|False|0|no|NO|No) return 1 ;;
    *) e2e_fail "invalid boolean value: ${1:-<unset>}" ;;
  esac
}

e2e_list_scenario_dirs() {
  find "$e2e_scenarios_root" -mindepth 1 -maxdepth 1 -type d | sort
}

e2e_resolve_scenario_dir() {
  local scenario="$1"
  case "$scenario" in
    /*) printf '%s\n' "$scenario" ;;
    *) printf '%s/%s\n' "$e2e_scenarios_root" "$scenario" ;;
  esac
}

e2e_load_scenario_metadata() {
  local scenario_dir="$1"
  local scenario_env="$scenario_dir/scenario.env"
  [[ -f "$scenario_env" ]] || e2e_fail "missing scenario metadata: $scenario_env"

  unset NAME MODE TAGS DEFAULT_ENABLED TIMEOUT_SECONDS REQUIRES_CODEX REQUIRES_NETWORK REQUIRES_CLEAN_GIT
  # shellcheck disable=SC1090
  source "$scenario_env"

  local scenario_name
  scenario_name="$(basename "$scenario_dir")"

  [[ "${NAME:-}" == "$scenario_name" ]] || e2e_fail "scenario.env NAME must match directory name for $scenario_name"
  case "${MODE:-}" in
    deterministic|hybrid-live|full-live|recovery) ;;
    *) e2e_fail "scenario $scenario_name has invalid MODE=${MODE:-<unset>}" ;;
  esac
  case "${DEFAULT_ENABLED:-}" in
    true|false) ;;
    *) e2e_fail "scenario $scenario_name has invalid DEFAULT_ENABLED=${DEFAULT_ENABLED:-<unset>}" ;;
  esac
  case "${REQUIRES_CODEX:-}" in
    true|false) ;;
    *) e2e_fail "scenario $scenario_name has invalid REQUIRES_CODEX=${REQUIRES_CODEX:-<unset>}" ;;
  esac
  case "${REQUIRES_NETWORK:-}" in
    true|false) ;;
    *) e2e_fail "scenario $scenario_name has invalid REQUIRES_NETWORK=${REQUIRES_NETWORK:-<unset>}" ;;
  esac
  case "${REQUIRES_CLEAN_GIT:-}" in
    true|false) ;;
    *) e2e_fail "scenario $scenario_name has invalid REQUIRES_CLEAN_GIT=${REQUIRES_CLEAN_GIT:-<unset>}" ;;
  esac
  [[ "${TIMEOUT_SECONDS:-}" =~ ^[0-9]+$ ]] || e2e_fail "scenario $scenario_name has invalid TIMEOUT_SECONDS=${TIMEOUT_SECONDS:-<unset>}"
  TAGS="${TAGS:-}"
  TAGS="${TAGS// /}"
}

e2e_scenario_has_tag() {
  local tag="$1"
  local value
  IFS=, read -ra value <<<"${TAGS:-}"
  local item
  for item in "${value[@]}"; do
    [[ -n "$item" && "$item" == "$tag" ]] && return 0
  done
  return 1
}

e2e_scenario_matches_suite() {
  case "$e2e_suite" in
    deterministic)
      e2e_is_true "${DEFAULT_ENABLED:-false}" || return 1
      case "${MODE:-}" in
        deterministic|hybrid-live) ;;
        *) return 1 ;;
      esac
      if e2e_scenario_has_tag live; then
        return 1
      fi
      ;;
    live)
      case "${MODE:-}" in
        full-live|recovery) return 0 ;;
      esac
      e2e_scenario_has_tag live
      ;;
    long)
      e2e_scenario_has_tag long
      ;;
    *)
      e2e_fail "unknown suite: $e2e_suite"
      ;;
  esac
}

e2e_scenario_matches_filters() {
  if [[ -n "$e2e_requested_scenario" ]]; then
    [[ "$(basename "$1")" == "$e2e_requested_scenario" ]]
    return
  fi
  if [[ -n "$e2e_tag_filter" ]] && ! e2e_scenario_has_tag "$e2e_tag_filter"; then
    return 1
  fi
  e2e_scenario_matches_suite
}

e2e_prepare_output_dirs() {
  mkdir -p \
    "$e2e_output_root/logs/$e2e_run_id" \
    "$e2e_output_root/reports/$e2e_run_id" \
    "$e2e_output_root/artifacts/$e2e_run_id" \
    "$e2e_output_root/worktrees/$e2e_run_id" \
    "$e2e_output_root/xdg/$e2e_run_id"
}

e2e_prepare_scenario_dirs() {
  local scenario_name="$1"

  e2e_prepare_output_dirs

  E2E_SCENARIO_NAME="$scenario_name"
  E2E_SCENARIO_DIR="$e2e_scenarios_root/$scenario_name"
  E2E_SCENARIO_OUTPUT_DIR="$e2e_output_root"
  E2E_SCENARIO_LOGS_DIR="$e2e_output_root/logs/$e2e_run_id/$scenario_name"
  E2E_SCENARIO_REPORTS_DIR="$e2e_output_root/reports/$e2e_run_id/$scenario_name"
  E2E_SCENARIO_ARTIFACTS_DIR="$e2e_output_root/artifacts/$e2e_run_id/$scenario_name"
  E2E_SCENARIO_WORKTREES_DIR="$e2e_output_root/worktrees/$e2e_run_id/$scenario_name"
  E2E_SCENARIO_XDG_DIR="$e2e_output_root/xdg/$e2e_run_id/$scenario_name"
  E2E_SCENARIO_XDG_DATA_HOME="$E2E_SCENARIO_XDG_DIR/data"
  E2E_SCENARIO_XDG_CONFIG_HOME="$E2E_SCENARIO_XDG_DIR/config"
  E2E_SCENARIO_XDG_RUNTIME_HOME="$E2E_SCENARIO_XDG_DIR/runtime"

  export E2E_SCENARIO_NAME \
    E2E_SCENARIO_DIR \
    E2E_SCENARIO_OUTPUT_DIR \
    E2E_SCENARIO_LOGS_DIR \
    E2E_SCENARIO_REPORTS_DIR \
    E2E_SCENARIO_ARTIFACTS_DIR \
    E2E_SCENARIO_WORKTREES_DIR \
    E2E_SCENARIO_XDG_DIR \
    E2E_SCENARIO_XDG_DATA_HOME \
    E2E_SCENARIO_XDG_CONFIG_HOME \
    E2E_SCENARIO_XDG_RUNTIME_HOME \
    ORCAS_E2E_XDG_DATA_HOME="$E2E_SCENARIO_XDG_DATA_HOME" \
    ORCAS_E2E_XDG_CONFIG_HOME="$E2E_SCENARIO_XDG_CONFIG_HOME" \
    ORCAS_E2E_XDG_RUNTIME_HOME="$E2E_SCENARIO_XDG_RUNTIME_HOME"

  mkdir -p \
    "$E2E_SCENARIO_LOGS_DIR" \
    "$E2E_SCENARIO_REPORTS_DIR" \
    "$E2E_SCENARIO_ARTIFACTS_DIR" \
    "$E2E_SCENARIO_WORKTREES_DIR" \
    "$E2E_SCENARIO_XDG_DATA_HOME/orcas" \
    "$E2E_SCENARIO_XDG_CONFIG_HOME/orcas" \
    "$E2E_SCENARIO_XDG_RUNTIME_HOME/orcas"
  chmod 700 "$E2E_SCENARIO_XDG_RUNTIME_HOME" || true
}

e2e_require_clean_git() {
  local status
  status="$(git -C "$e2e_repo_root" status --porcelain)"
  [[ -z "$status" ]] || e2e_fail "scenario $E2E_SCENARIO_NAME requires a clean git tree"
}

e2e_require_codex() {
  if [[ -n "${ORCAS_CODEX_BIN:-}" && -x "$ORCAS_CODEX_BIN" ]]; then
    return 0
  fi
  command -v codex >/dev/null 2>&1 || e2e_fail "scenario $E2E_SCENARIO_NAME requires codex on PATH or ORCAS_CODEX_BIN"
}

e2e_print_scenario_begin() {
  echo "==> BEGIN $E2E_SCENARIO_NAME [$E2E_SUITE] mode=$MODE tags=$TAGS timeout=${TIMEOUT_SECONDS}s"
}

e2e_print_scenario_end() {
  local status="$1"
  if [[ "$status" -eq 0 ]]; then
    echo "<== END $E2E_SCENARIO_NAME PASS"
  else
    echo "<== END $E2E_SCENARIO_NAME FAIL (exit $status)" >&2
  fi
}

e2e_orcas() {
  "$e2e_bin_dir/orcas.sh" "$@"
}
