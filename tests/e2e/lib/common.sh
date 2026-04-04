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

e2e_daemon_pid=""

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
    "$e2e_output_root/orcas/$e2e_run_id" \
    "$e2e_output_root/xdg/$e2e_run_id"
}

e2e_link_legacy_xdg_views() {
  mkdir -p "$E2E_SCENARIO_XDG_DATA_HOME" "$E2E_SCENARIO_XDG_CONFIG_HOME" "$E2E_SCENARIO_XDG_RUNTIME_HOME"
  mkdir -p "$E2E_SCENARIO_ORCAS_HOME/logs" "$E2E_SCENARIO_ORCAS_HOME/runtime"
  rm -rf "$E2E_SCENARIO_XDG_DATA_HOME/orcas" "$E2E_SCENARIO_XDG_CONFIG_HOME/orcas" "$E2E_SCENARIO_XDG_RUNTIME_HOME/orcas"
  ln -s "$E2E_SCENARIO_ORCAS_HOME" "$E2E_SCENARIO_XDG_DATA_HOME/orcas"
  ln -s "$E2E_SCENARIO_ORCAS_HOME" "$E2E_SCENARIO_XDG_CONFIG_HOME/orcas"
  ln -s "$E2E_SCENARIO_ORCAS_HOME/runtime" "$E2E_SCENARIO_XDG_RUNTIME_HOME/orcas"
}

e2e_sync_legacy_xdg_into_orcas_home() {
  local xdg_data_home="$1"
  local xdg_config_home="$2"
  local xdg_runtime_home="$3"
  local orcas_home="$4"

  mkdir -p "$orcas_home/logs" "$orcas_home/runtime"

  if [[ -d "$xdg_data_home/orcas" && ! -L "$xdg_data_home/orcas" ]]; then
    cp -a "$xdg_data_home/orcas/." "$orcas_home/"
  fi
  if [[ -d "$xdg_config_home/orcas" && ! -L "$xdg_config_home/orcas" ]]; then
    cp -a "$xdg_config_home/orcas/." "$orcas_home/"
  fi
  if [[ -d "$xdg_runtime_home/orcas" && ! -L "$xdg_runtime_home/orcas" ]]; then
    cp -a "$xdg_runtime_home/orcas/." "$orcas_home/runtime/"
  fi

  rm -rf "$xdg_data_home/orcas" "$xdg_config_home/orcas" "$xdg_runtime_home/orcas"
  ln -s "$orcas_home" "$xdg_data_home/orcas"
  ln -s "$orcas_home" "$xdg_config_home/orcas"
  ln -s "$orcas_home/runtime" "$xdg_runtime_home/orcas"
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
  if e2e_is_true "${ORCAS_E2E_REUSE_CURRENT_XDG:-false}"; then
    [[ -n "${ORCAS_E2E_SHARED_XDG_DIR:-}" ]] || e2e_fail "ORCAS_E2E_SHARED_XDG_DIR is required when ORCAS_E2E_REUSE_CURRENT_XDG=true"
    [[ -n "${ORCAS_E2E_SHARED_ORCAS_HOME:-}" ]] || e2e_fail "ORCAS_E2E_SHARED_ORCAS_HOME is required when ORCAS_E2E_REUSE_CURRENT_XDG=true"
    [[ -n "${ORCAS_E2E_SHARED_XDG_DATA_HOME:-}" ]] || e2e_fail "ORCAS_E2E_SHARED_XDG_DATA_HOME is required when ORCAS_E2E_REUSE_CURRENT_XDG=true"
    [[ -n "${ORCAS_E2E_SHARED_XDG_CONFIG_HOME:-}" ]] || e2e_fail "ORCAS_E2E_SHARED_XDG_CONFIG_HOME is required when ORCAS_E2E_REUSE_CURRENT_XDG=true"
    [[ -n "${ORCAS_E2E_SHARED_XDG_RUNTIME_HOME:-}" ]] || e2e_fail "ORCAS_E2E_SHARED_XDG_RUNTIME_HOME is required when ORCAS_E2E_REUSE_CURRENT_XDG=true"
    E2E_SCENARIO_XDG_DIR="$ORCAS_E2E_SHARED_XDG_DIR"
    E2E_SCENARIO_XDG_DATA_HOME="$ORCAS_E2E_SHARED_XDG_DATA_HOME"
    E2E_SCENARIO_XDG_CONFIG_HOME="$ORCAS_E2E_SHARED_XDG_CONFIG_HOME"
    E2E_SCENARIO_XDG_RUNTIME_HOME="$ORCAS_E2E_SHARED_XDG_RUNTIME_HOME"
    E2E_SCENARIO_ORCAS_HOME="$ORCAS_E2E_SHARED_ORCAS_HOME"
  else
    E2E_SCENARIO_XDG_DIR="$e2e_output_root/xdg/$e2e_run_id/$scenario_name"
    E2E_SCENARIO_XDG_DATA_HOME="$E2E_SCENARIO_XDG_DIR/data"
    E2E_SCENARIO_XDG_CONFIG_HOME="$E2E_SCENARIO_XDG_DIR/config"
    E2E_SCENARIO_XDG_RUNTIME_HOME="$E2E_SCENARIO_XDG_DIR/runtime"
    E2E_SCENARIO_ORCAS_HOME="$e2e_output_root/orcas/$e2e_run_id/$scenario_name"
  fi

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
    E2E_SCENARIO_ORCAS_HOME \
    ORCAS_HOME="$E2E_SCENARIO_ORCAS_HOME" \
    ORCAS_E2E_ORCAS_HOME="$E2E_SCENARIO_ORCAS_HOME" \
    ORCAS_E2E_XDG_DATA_HOME="$E2E_SCENARIO_XDG_DATA_HOME" \
    ORCAS_E2E_XDG_CONFIG_HOME="$E2E_SCENARIO_XDG_CONFIG_HOME" \
    ORCAS_E2E_XDG_RUNTIME_HOME="$E2E_SCENARIO_XDG_RUNTIME_HOME"

  mkdir -p \
    "$E2E_SCENARIO_LOGS_DIR" \
    "$E2E_SCENARIO_REPORTS_DIR" \
    "$E2E_SCENARIO_ARTIFACTS_DIR" \
    "$E2E_SCENARIO_WORKTREES_DIR"
  chmod 700 "$E2E_SCENARIO_XDG_RUNTIME_HOME" || true
  e2e_link_legacy_xdg_views
}

e2e_using_shared_lab() {
  e2e_is_true "${ORCAS_E2E_REUSE_CURRENT_XDG:-false}"
}

e2e_use_short_xdg_paths() {
  local suffix="$1"
  [[ -n "$suffix" ]] || e2e_fail "short XDG path suffix is required"

  local short_xdg_root="$e2e_output_root/xdg/$E2E_RUN_ID/$suffix"
  local short_xdg_data_home="$short_xdg_root/data"
  local short_xdg_config_home="$short_xdg_root/config"
  local short_xdg_runtime_home="$short_xdg_root/runtime"
  local short_orcas_home="$e2e_output_root/orcas/$E2E_RUN_ID/$suffix"

  rm -rf "$short_xdg_root" "$short_orcas_home"
  mkdir -p \
    "$short_xdg_data_home" \
    "$short_xdg_config_home" \
    "$short_xdg_runtime_home"
  chmod 700 "$short_xdg_runtime_home" || true

  E2E_SCENARIO_XDG_DIR="$short_xdg_root"
  E2E_SCENARIO_XDG_DATA_HOME="$short_xdg_data_home"
  E2E_SCENARIO_XDG_CONFIG_HOME="$short_xdg_config_home"
  E2E_SCENARIO_XDG_RUNTIME_HOME="$short_xdg_runtime_home"
  E2E_SCENARIO_ORCAS_HOME="$short_orcas_home"

  export E2E_SCENARIO_XDG_DIR \
    E2E_SCENARIO_XDG_DATA_HOME \
    E2E_SCENARIO_XDG_CONFIG_HOME \
    E2E_SCENARIO_XDG_RUNTIME_HOME \
    E2E_SCENARIO_ORCAS_HOME \
    ORCAS_HOME="$short_orcas_home" \
    ORCAS_E2E_ORCAS_HOME="$short_orcas_home" \
    ORCAS_E2E_XDG_DATA_HOME="$short_xdg_data_home" \
    ORCAS_E2E_XDG_CONFIG_HOME="$short_xdg_config_home" \
    ORCAS_E2E_XDG_RUNTIME_HOME="$short_xdg_runtime_home"
  e2e_link_legacy_xdg_views
}

e2e_require_local_supervisor_endpoint() {
  local base_url="${ORCAS_E2E_SUPERVISOR_BASE_URL:-${ORCAS_E2E_QWEN_BASE_URL:-}}"
  local model="${ORCAS_E2E_SUPERVISOR_MODEL:-${ORCAS_E2E_QWEN_MODEL:-}}"

  [[ -n "$base_url" ]] || e2e_fail "scenario $E2E_SCENARIO_NAME requires a local OpenAI-compatible supervisor endpoint; export ORCAS_E2E_SUPERVISOR_BASE_URL=http://127.0.0.1:8000/v1"
  [[ -n "$model" ]] || e2e_fail "scenario $E2E_SCENARIO_NAME requires a served supervisor model name; export ORCAS_E2E_SUPERVISOR_MODEL=<served-model-name>"

  local models_url="${base_url%/}/models"
  curl -sf "$models_url" >/dev/null 2>&1 || e2e_fail "scenario $E2E_SCENARIO_NAME could not reach the local supervisor endpoint at $models_url"

  export ORCAS_SUPERVISOR_BASE_URL="$base_url"
  export ORCAS_SUPERVISOR_MODEL="$model"
  export ORCAS_SUPERVISOR_API_KEY_ENV="${ORCAS_E2E_SUPERVISOR_API_KEY_ENV:-${ORCAS_E2E_QWEN_API_KEY_ENV:-}}"
  export ORCAS_SUPERVISOR_REASONING_EFFORT="${ORCAS_E2E_SUPERVISOR_REASONING_EFFORT:-${ORCAS_E2E_QWEN_REASONING_EFFORT:-}}"
  export ORCAS_SUPERVISOR_MAX_OUTPUT_TOKENS="${ORCAS_E2E_SUPERVISOR_MAX_OUTPUT_TOKENS:-${ORCAS_E2E_QWEN_MAX_OUTPUT_TOKENS:-4096}}"
  export ORCAS_SUPERVISOR_TEMPERATURE="${ORCAS_E2E_SUPERVISOR_TEMPERATURE:-${ORCAS_E2E_QWEN_TEMPERATURE:-0.0}}"
}

e2e_start_managed_daemon() {
  local daemon_log="$1"
  if e2e_is_true "${ORCAS_E2E_REUSE_CURRENT_DAEMON:-false}"; then
    [[ -n "${ORCAS_E2E_SHARED_SOCKET_FILE:-}" ]] || e2e_fail "ORCAS_E2E_SHARED_SOCKET_FILE is required when ORCAS_E2E_REUSE_CURRENT_DAEMON=true"
    e2e_orcas daemon status >"$daemon_log" 2>&1 || e2e_fail "shared lab daemon is not responsive"
    printf 'shared lab daemon reused via %s\n' "$ORCAS_E2E_SHARED_SOCKET_FILE" >>"$daemon_log"
    e2e_daemon_pid=""
    return 0
  fi
  e2e_orcas daemon start --force-spawn >"$daemon_log" 2>&1 &
  e2e_daemon_pid=$!
}

e2e_stop_managed_daemon() {
  if e2e_is_true "${ORCAS_E2E_REUSE_CURRENT_DAEMON:-false}"; then
    return 0
  fi
  if [[ -n "$e2e_daemon_pid" ]]; then
    kill "$e2e_daemon_pid" >/dev/null 2>&1 || true
    wait "$e2e_daemon_pid" >/dev/null 2>&1 || true
    e2e_daemon_pid=""
  fi
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

e2e_orcas_state_seed() {
  local seed_bin="$e2e_repo_root/target/debug/orcas-state-seed"

  if [[ ! -x "$seed_bin" || "${ORCAS_E2E_FORCE_CARGO_RUN:-0}" == "1" ]]; then
    cargo build -q --manifest-path "$e2e_repo_root/Cargo.toml" -p orcas -p orcasd
  fi

  "$seed_bin" "$@"
}

e2e_normalize_state_json() {
  local input="$1"
  local output="${2:-$1}"

  e2e_orcas_state_seed --input "$input" --output "$output"
}

e2e_orcasd() {
  local orcasd_bin="$e2e_repo_root/target/debug/orcasd"
  local xdg_data_home="${ORCAS_E2E_XDG_DATA_HOME:-$e2e_output_root/xdg/default/data}"
  local xdg_config_home="${ORCAS_E2E_XDG_CONFIG_HOME:-$e2e_output_root/xdg/default/config}"
  local xdg_runtime_home="${ORCAS_E2E_XDG_RUNTIME_HOME:-$e2e_output_root/xdg/default/runtime}"
  local orcas_home="${ORCAS_E2E_ORCAS_HOME:-$e2e_output_root/orcas/default}"

  mkdir -p "$xdg_data_home" "$xdg_config_home" "$xdg_runtime_home"
  e2e_sync_legacy_xdg_into_orcas_home "$xdg_data_home" "$xdg_config_home" "$xdg_runtime_home" "$orcas_home"
  chmod 700 "$xdg_runtime_home" || true

  if [[ ! -x "$orcasd_bin" || "${ORCAS_E2E_FORCE_CARGO_RUN:-0}" == "1" ]]; then
    cargo build -q --manifest-path "$e2e_repo_root/Cargo.toml" -p orcas -p orcasd
  fi

  ORCAS_HOME="$orcas_home" \
    "$orcasd_bin" "$@"
}
