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
    "$E2E_SCENARIO_WORKTREES_DIR" \
    "$E2E_SCENARIO_XDG_DATA_HOME" \
    "$E2E_SCENARIO_XDG_CONFIG_HOME" \
    "$E2E_SCENARIO_XDG_RUNTIME_HOME" \
    "$E2E_SCENARIO_ORCAS_HOME/logs" \
    "$E2E_SCENARIO_ORCAS_HOME/runtime"
  chmod 700 "$E2E_SCENARIO_XDG_RUNTIME_HOME" || true
  e2e_link_legacy_xdg_views
}

e2e_using_shared_lab() {
  e2e_is_true "${ORCAS_E2E_REUSE_CURRENT_XDG:-false}"
}

e2e_use_short_xdg_paths() {
  local suffix="$1"
  [[ -n "$suffix" ]] || e2e_fail "short XDG path suffix is required"

  local run_hash
  run_hash="$(printf '%s' "$E2E_RUN_ID" | cksum | awk '{print $1}')"
  local short_root_base="${TMPDIR:-/tmp}/orcas-e2e"
  local short_xdg_root="$short_root_base/${suffix}-${run_hash}"
  local short_xdg_data_home="$short_xdg_root/data"
  local short_xdg_config_home="$short_xdg_root/config"
  local short_xdg_runtime_home="$short_xdg_root/runtime"
  local short_orcas_home="$short_root_base/${suffix}-${run_hash}-orcas"

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

e2e_field_value() {
  local key="$1"
  local file="$2"
  sed -n "s/^${key}: //p" "$file" | head -n1
}

e2e_prepare_live_codex_environment() {
  local suffix="$1"
  local listen_port_base="$2"
  local supervisor_max_output_tokens="${3:-${ORCAS_SUPERVISOR_MAX_OUTPUT_TOKENS:-4096}}"

  if e2e_using_shared_lab; then
    return 0
  fi

  local listen_port="$((listen_port_base + ($(printf '%s' "$E2E_RUN_ID" | cksum | awk '{print $1}') % 1000)))"
  local listen_url="ws://127.0.0.1:$listen_port"
  local supervisor_base_url="${ORCAS_SUPERVISOR_BASE_URL:-http://127.0.0.1:8000/v1}"
  local supervisor_model="${ORCAS_SUPERVISOR_MODEL:-gpt-oss-20b}"
  local supervisor_api_key_env="${ORCAS_SUPERVISOR_API_KEY_ENV:-}"
  local supervisor_reasoning_effort="${ORCAS_SUPERVISOR_REASONING_EFFORT:-}"
  local supervisor_temperature="${ORCAS_SUPERVISOR_TEMPERATURE:-0.0}"
  local codex_bin="${ORCAS_CODEX_BIN:-$(command -v codex)}"

  e2e_use_short_xdg_paths "$suffix"
  mkdir -p "$E2E_SCENARIO_XDG_CONFIG_HOME/orcas"

  cat >"$E2E_SCENARIO_XDG_CONFIG_HOME/orcas/config.toml" <<EOF
[codex]
binary_path = "$codex_bin"
listen_url = "$listen_url"
connection_mode = "spawn_if_needed"
config_overrides = []

[codex.reconnect]
initial_delay_ms = 150
max_delay_ms = 5000
multiplier = 2.0

[supervisor]
base_url = "$supervisor_base_url"
api_key_env = "$supervisor_api_key_env"
model = "$supervisor_model"
reasoning_effort = "$supervisor_reasoning_effort"
temperature = $supervisor_temperature
max_output_tokens = $supervisor_max_output_tokens

[supervisor.proposals]
auto_create_on_report_recorded = false
EOF

  export ORCAS_CODEX_LISTEN_URL="$listen_url"
}

e2e_prepare_fixture_repo_with_worktree() {
  local fixture_dir="$1"
  local repo_root="$2"
  local worktree_path="$3"
  local branch_name="$4"
  local base_ref="$5"
  local reports_dir="$6"
  local prefix="${7:-git}"

  rm -rf "$repo_root" "$worktree_path"
  mkdir -p "$repo_root" "$(dirname "$worktree_path")"
  cp -R "$fixture_dir/." "$repo_root/"

  git -C "$repo_root" init -b "$base_ref" >"$reports_dir/${prefix}-git-init.txt" 2>&1
  git -C "$repo_root" config user.name "Orcas E2E"
  git -C "$repo_root" config user.email "orcas-e2e@example.com"
  git -C "$repo_root" add .
  git -C "$repo_root" commit -m "Initial tracked-thread fixture" >"$reports_dir/${prefix}-git-initial-commit.txt" 2>&1
  git -C "$repo_root" worktree add -b "$branch_name" "$worktree_path" "$base_ref" >"$reports_dir/${prefix}-git-worktree-add.txt" 2>&1
}

e2e_prepare_empty_repo_with_worktree() {
  local repo_root="$1"
  local worktree_path="$2"
  local branch_name="$3"
  local base_ref="$4"
  local reports_dir="$5"
  local prefix="${6:-git}"

  rm -rf "$repo_root" "$worktree_path"
  mkdir -p "$repo_root" "$(dirname "$worktree_path")"

  git -C "$repo_root" init -b "$base_ref" >"$reports_dir/${prefix}-git-init.txt" 2>&1
  git -C "$repo_root" config user.name "Orcas E2E"
  git -C "$repo_root" config user.email "orcas-e2e@example.com"

  cat >"$repo_root/README.md" <<EOF
# ${E2E_SCENARIO_NAME:-orcas-e2e}

Scenario seed repository for the tracked-thread worktree lane.
EOF

  git -C "$repo_root" add README.md
  git -C "$repo_root" commit -m "Initial tracked-thread seed" >"$reports_dir/${prefix}-git-initial-commit.txt" 2>&1
  git -C "$repo_root" worktree add -b "$branch_name" "$worktree_path" "$base_ref" >"$reports_dir/${prefix}-git-worktree-add.txt" 2>&1
}

e2e_add_tracked_thread_workspace() {
  local workunit_id="$1"
  local title="$2"
  local root_dir="$3"
  local notes="$4"
  local repository_root="$5"
  local worktree_path="$6"
  local branch_name="$7"
  local base_ref="$8"
  local base_commit="$9"
  local landing_target="${10}"
  local sync_policy="${11}"
  local cleanup_policy="${12}"
  local workspace_status="${13}"

  e2e_orcas workunit thread add \
    --workunit "$workunit_id" \
    --title "$title" \
    --root-dir "$root_dir" \
    --notes "$notes" \
    --workspace-repository-root "$repository_root" \
    --workspace-worktree-path "$worktree_path" \
    --workspace-branch-name "$branch_name" \
    --workspace-base-ref "$base_ref" \
    --workspace-base-commit "$base_commit" \
    --workspace-landing-target "$landing_target" \
    --workspace-strategy dedicated-thread-worktree \
    --workspace-landing-policy merge-to-main \
    --workspace-sync-policy "$sync_policy" \
    --workspace-cleanup-policy "$cleanup_policy" \
    --workspace-status "$workspace_status"
}

e2e_wait_for_report_id() {
  local workunit_id="$1"
  local output_var="$2"
  local attempts="${3:-120}"
  local delay_seconds="${4:-5}"
  local reports_output=""
  local report_id=""

  for _ in $(seq 1 "$attempts"); do
    reports_output="$("$e2e_bin_dir/orcas.sh" supervisor work reports list-for-workunit --workunit "$workunit_id" 2>/dev/null || true)"
    report_id="$(printf '%s\n' "$reports_output" | awk -F'\t' '/^report-/ {print $1; exit}')"
    [[ -n "$report_id" ]] && break
    sleep "$delay_seconds"
  done

  test -n "$report_id"
  printf -v "$output_var" '%s' "$report_id"
}

e2e_wait_for_assignment_report_id() {
  local assignment_id="$1"
  local output_var="$2"
  local attempts="${3:-120}"
  local delay_seconds="${4:-5}"
  local assignment_output=""
  local report_id=""

  for _ in $(seq 1 "$attempts"); do
    assignment_output="$("$e2e_bin_dir/orcas.sh" supervisor work assignments get --assignment "$assignment_id" 2>/dev/null || true)"
    report_id="$(printf '%s\n' "$assignment_output" | awk -F': ' '/^report_id:/ {print $2; exit}')"
    [[ -n "$report_id" ]] && break
    sleep "$delay_seconds"
  done

  test -n "$report_id"
  printf -v "$output_var" '%s' "$report_id"
}

e2e_capture_workstream_runtime() {
  local workstream_id="$1"
  local output_file="$2"
  e2e_orcas workstreams runtime get --workstream "$workstream_id" >"$output_file"
}

e2e_assert_workstream_runtime() {
  local workstream_id="$1"
  local output_file="$2"
  grep -q "runtime_workstream_id: $workstream_id" "$output_file"
  grep -q '^runtime_status: ' "$output_file"
  grep -q '^runtime_thread_count: ' "$output_file"
}

e2e_assert_runtime_thread_count() {
  local output_file="$1"
  local expected_count="$2"
  local actual_count
  actual_count="$(e2e_field_value runtime_thread_count "$output_file")"
  test "$actual_count" = "$expected_count"
}

e2e_capture_workstream_threads() {
  local workstream_id="$1"
  local output_file="$2"
  e2e_orcas codex threads list --workstream "$workstream_id" >"$output_file"
}

e2e_assert_managed_thread_count() {
  local output_file="$1"
  local expected_count="$2"
  local actual_count
  actual_count="$(grep -c 'management=managed' "$output_file" || true)"
  test "$actual_count" -eq "$expected_count"
  ! grep -q 'management=observed_unmanaged' "$output_file"
}
