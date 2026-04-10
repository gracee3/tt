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
e2e_codex_app_server_pid=""

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

  unset NAME MODE TAGS DEFAULT_ENABLED TIMEOUT_SECONDS REQUIRES_RUNTIME REQUIRES_NETWORK REQUIRES_CLEAN_GIT REQUIRES_EXTRACTED_HANDOFFS EXPECTED_LONG_BUILD REQUIRES_PROGRESS_UPDATES SOFT_SILENCE_SECONDS HARD_CEILING_SECONDS
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
  case "${REQUIRES_RUNTIME:-}" in
    true|false) ;;
    *) e2e_fail "scenario $scenario_name has invalid REQUIRES_RUNTIME=${REQUIRES_RUNTIME:-<unset>}" ;;
  esac
  case "${REQUIRES_NETWORK:-}" in
    true|false) ;;
    *) e2e_fail "scenario $scenario_name has invalid REQUIRES_NETWORK=${REQUIRES_NETWORK:-<unset>}" ;;
  esac
  case "${REQUIRES_CLEAN_GIT:-}" in
    true|false) ;;
    *) e2e_fail "scenario $scenario_name has invalid REQUIRES_CLEAN_GIT=${REQUIRES_CLEAN_GIT:-<unset>}" ;;
  esac
  case "${REQUIRES_EXTRACTED_HANDOFFS:-}" in
    true|false) ;;
    *) e2e_fail "scenario $scenario_name has invalid REQUIRES_EXTRACTED_HANDOFFS=${REQUIRES_EXTRACTED_HANDOFFS:-<unset>}" ;;
  esac
  case "${EXPECTED_LONG_BUILD:-}" in
    true|false) ;;
    *) e2e_fail "scenario $scenario_name has invalid EXPECTED_LONG_BUILD=${EXPECTED_LONG_BUILD:-<unset>}" ;;
  esac
  case "${REQUIRES_PROGRESS_UPDATES:-}" in
    true|false) ;;
    *) e2e_fail "scenario $scenario_name has invalid REQUIRES_PROGRESS_UPDATES=${REQUIRES_PROGRESS_UPDATES:-<unset>}" ;;
  esac
  [[ "${SOFT_SILENCE_SECONDS:-}" =~ ^[0-9]+$ ]] || e2e_fail "scenario $scenario_name has invalid SOFT_SILENCE_SECONDS=${SOFT_SILENCE_SECONDS:-<unset>}"
  [[ "${HARD_CEILING_SECONDS:-}" =~ ^[0-9]+$ ]] || e2e_fail "scenario $scenario_name has invalid HARD_CEILING_SECONDS=${HARD_CEILING_SECONDS:-<unset>}"
  [[ "${TIMEOUT_SECONDS:-}" =~ ^[0-9]+$ ]] || e2e_fail "scenario $scenario_name has invalid TIMEOUT_SECONDS=${TIMEOUT_SECONDS:-<unset>}"
  TAGS="${TAGS:-}"
  TAGS="${TAGS// /}"
}

e2e_require_extracted_handoffs() {
  local inspect_file="$1"
  local scenario_root="$2"

  grep -q "fallback_handoffs: 0" "$inspect_file" || {
    echo "strict handoff failure: project inspect reported fallback handoffs" >&2
    echo "inspect file: $inspect_file" >&2
    echo "scenario artifacts: $scenario_root" >&2
    return 1
  }

  local source_file
  while IFS= read -r source_file; do
    [[ -n "$source_file" ]] || continue
    if ! grep -qx 'extracted' "$source_file"; then
      echo "strict handoff failure: non-extracted handoff source in $source_file" >&2
      echo "scenario artifacts: $scenario_root" >&2
      local parse_error="${source_file%-source.txt}-parse-error.txt"
      local raw_file="${source_file%-source.txt}-raw.txt"
      [[ -f "$parse_error" ]] && { echo "--- parse error ---" >&2; cat "$parse_error" >&2; }
      [[ -f "$raw_file" ]] && { echo "--- raw handoff ---" >&2; cat "$raw_file" >&2; }
      return 1
    fi
  done < <(find "$scenario_root" -name '*-handoff-source.txt' -type f | sort)
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
    "$e2e_output_root/tt/$e2e_run_id" \
    "$e2e_output_root/xdg/$e2e_run_id"
}

e2e_ensure_symlink() {
  local target="$1"
  local link_path="$2"

  mkdir -p "$(dirname "$link_path")"
  if [[ -L "$link_path" && "$(readlink "$link_path")" == "$target" ]]; then
    return 0
  fi
  if [[ -e "$link_path" && ! -L "$link_path" ]]; then
    rm -rf "$link_path"
  fi
  ln -sfn "$target" "$link_path"
}

e2e_link_legacy_xdg_views() {
  mkdir -p "$E2E_SCENARIO_XDG_DATA_HOME" "$E2E_SCENARIO_XDG_CONFIG_HOME" "$E2E_SCENARIO_XDG_RUNTIME_HOME"
  mkdir -p "$E2E_SCENARIO_TT_HOME/logs" "$E2E_SCENARIO_TT_HOME/runtime"
  e2e_ensure_symlink "$E2E_SCENARIO_TT_HOME" "$E2E_SCENARIO_XDG_DATA_HOME/tt"
  e2e_ensure_symlink "$E2E_SCENARIO_TT_HOME" "$E2E_SCENARIO_XDG_CONFIG_HOME/tt"
  e2e_ensure_symlink "$E2E_SCENARIO_TT_HOME/runtime" "$E2E_SCENARIO_XDG_RUNTIME_HOME/tt"
}

e2e_sync_legacy_xdg_into_tt_home() {
  local xdg_data_home="$1"
  local xdg_config_home="$2"
  local xdg_runtime_home="$3"
  local tt_home="$4"

  mkdir -p "$tt_home/logs" "$tt_home/runtime"

  if [[ -d "$xdg_data_home/tt" && ! -L "$xdg_data_home/tt" ]]; then
    cp -a "$xdg_data_home/tt/." "$tt_home/"
  fi
  if [[ -d "$xdg_config_home/tt" && ! -L "$xdg_config_home/tt" ]]; then
    cp -a "$xdg_config_home/tt/." "$tt_home/"
  fi
  if [[ -d "$xdg_runtime_home/tt" && ! -L "$xdg_runtime_home/tt" ]]; then
    cp -a "$xdg_runtime_home/tt/." "$tt_home/runtime/"
  fi

  e2e_ensure_symlink "$tt_home" "$xdg_data_home/tt"
  e2e_ensure_symlink "$tt_home" "$xdg_config_home/tt"
  e2e_ensure_symlink "$tt_home/runtime" "$xdg_runtime_home/tt"
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
  if e2e_is_true "${TT_E2E_REUSE_CURRENT_XDG:-false}"; then
    [[ -n "${TT_E2E_SHARED_XDG_DIR:-}" ]] || e2e_fail "TT_E2E_SHARED_XDG_DIR is required when TT_E2E_REUSE_CURRENT_XDG=true"
    [[ -n "${TT_E2E_SHARED_TT_HOME:-}" ]] || e2e_fail "TT_E2E_SHARED_TT_HOME is required when TT_E2E_REUSE_CURRENT_XDG=true"
    [[ -n "${TT_E2E_SHARED_XDG_DATA_HOME:-}" ]] || e2e_fail "TT_E2E_SHARED_XDG_DATA_HOME is required when TT_E2E_REUSE_CURRENT_XDG=true"
    [[ -n "${TT_E2E_SHARED_XDG_CONFIG_HOME:-}" ]] || e2e_fail "TT_E2E_SHARED_XDG_CONFIG_HOME is required when TT_E2E_REUSE_CURRENT_XDG=true"
    [[ -n "${TT_E2E_SHARED_XDG_RUNTIME_HOME:-}" ]] || e2e_fail "TT_E2E_SHARED_XDG_RUNTIME_HOME is required when TT_E2E_REUSE_CURRENT_XDG=true"
    E2E_SCENARIO_XDG_DIR="$TT_E2E_SHARED_XDG_DIR"
    E2E_SCENARIO_XDG_DATA_HOME="$TT_E2E_SHARED_XDG_DATA_HOME"
    E2E_SCENARIO_XDG_CONFIG_HOME="$TT_E2E_SHARED_XDG_CONFIG_HOME"
    E2E_SCENARIO_XDG_RUNTIME_HOME="$TT_E2E_SHARED_XDG_RUNTIME_HOME"
    E2E_SCENARIO_TT_HOME="$TT_E2E_SHARED_TT_HOME"
  else
    E2E_SCENARIO_XDG_DIR="$e2e_output_root/xdg/$e2e_run_id/$scenario_name"
    E2E_SCENARIO_XDG_DATA_HOME="$E2E_SCENARIO_XDG_DIR/data"
    E2E_SCENARIO_XDG_CONFIG_HOME="$E2E_SCENARIO_XDG_DIR/config"
    E2E_SCENARIO_XDG_RUNTIME_HOME="$E2E_SCENARIO_XDG_DIR/runtime"
    E2E_SCENARIO_TT_HOME="$e2e_output_root/tt/$e2e_run_id/$scenario_name"
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
    E2E_SCENARIO_TT_HOME \
    TT_HOME="$E2E_SCENARIO_TT_HOME" \
    TT_E2E_TT_HOME="$E2E_SCENARIO_TT_HOME" \
    TT_E2E_XDG_DATA_HOME="$E2E_SCENARIO_XDG_DATA_HOME" \
    TT_E2E_XDG_CONFIG_HOME="$E2E_SCENARIO_XDG_CONFIG_HOME" \
    TT_E2E_XDG_RUNTIME_HOME="$E2E_SCENARIO_XDG_RUNTIME_HOME"

  mkdir -p \
    "$E2E_SCENARIO_LOGS_DIR" \
    "$E2E_SCENARIO_REPORTS_DIR" \
    "$E2E_SCENARIO_ARTIFACTS_DIR" \
    "$E2E_SCENARIO_WORKTREES_DIR" \
    "$E2E_SCENARIO_XDG_DATA_HOME" \
    "$E2E_SCENARIO_XDG_CONFIG_HOME" \
    "$E2E_SCENARIO_XDG_RUNTIME_HOME" \
    "$E2E_SCENARIO_TT_HOME/logs" \
    "$E2E_SCENARIO_TT_HOME/runtime"
  chmod 700 "$E2E_SCENARIO_XDG_RUNTIME_HOME" || true
  e2e_link_legacy_xdg_views
}

e2e_using_shared_lab() {
  e2e_is_true "${TT_E2E_REUSE_CURRENT_XDG:-false}"
}

e2e_use_short_xdg_paths() {
  local suffix="$1"
  [[ -n "$suffix" ]] || e2e_fail "short XDG path suffix is required"

  local run_hash
  run_hash="$(printf '%s' "$E2E_RUN_ID" | cksum | awk '{print $1}')"
  local short_root_base="${TMPDIR:-/tmp}/tt-e2e"
  local short_xdg_root="$short_root_base/${suffix}-${run_hash}"
  local short_xdg_data_home="$short_xdg_root/data"
  local short_xdg_config_home="$short_xdg_root/config"
  local short_xdg_runtime_home="$short_xdg_root/runtime"
  local short_tt_home="$short_root_base/${suffix}-${run_hash}-tt"

  rm -rf "$short_xdg_root" "$short_tt_home"
  mkdir -p \
    "$short_xdg_data_home" \
    "$short_xdg_config_home" \
    "$short_xdg_runtime_home"
  chmod 700 "$short_xdg_runtime_home" || true

  E2E_SCENARIO_XDG_DIR="$short_xdg_root"
  E2E_SCENARIO_XDG_DATA_HOME="$short_xdg_data_home"
  E2E_SCENARIO_XDG_CONFIG_HOME="$short_xdg_config_home"
  E2E_SCENARIO_XDG_RUNTIME_HOME="$short_xdg_runtime_home"
  E2E_SCENARIO_TT_HOME="$short_tt_home"

  export E2E_SCENARIO_XDG_DIR \
    E2E_SCENARIO_XDG_DATA_HOME \
    E2E_SCENARIO_XDG_CONFIG_HOME \
    E2E_SCENARIO_XDG_RUNTIME_HOME \
    E2E_SCENARIO_TT_HOME \
    TT_HOME="$short_tt_home" \
    TT_E2E_TT_HOME="$short_tt_home" \
    TT_E2E_XDG_DATA_HOME="$short_xdg_data_home" \
    TT_E2E_XDG_CONFIG_HOME="$short_xdg_config_home" \
    TT_E2E_XDG_RUNTIME_HOME="$short_xdg_runtime_home"
  e2e_link_legacy_xdg_views
}

e2e_require_local_supervisor_endpoint() {
  local base_url="${TT_E2E_SUPERVISOR_BASE_URL:-${TT_E2E_QWEN_BASE_URL:-}}"
  local model="${TT_E2E_SUPERVISOR_MODEL:-${TT_E2E_QWEN_MODEL:-}}"

  [[ -n "$base_url" ]] || e2e_fail "scenario $E2E_SCENARIO_NAME requires a local OpenAI-compatible supervisor endpoint; export TT_E2E_SUPERVISOR_BASE_URL=http://127.0.0.1:8000/v1"
  [[ -n "$model" ]] || e2e_fail "scenario $E2E_SCENARIO_NAME requires a served supervisor model name; export TT_E2E_SUPERVISOR_MODEL=<served-model-name>"

  local models_url="${base_url%/}/models"
  curl -sf "$models_url" >/dev/null 2>&1 || e2e_fail "scenario $E2E_SCENARIO_NAME could not reach the local supervisor endpoint at $models_url"

  export TT_SUPERVISOR_BASE_URL="$base_url"
  export TT_SUPERVISOR_MODEL="$model"
  export TT_SUPERVISOR_API_KEY_ENV="${TT_E2E_SUPERVISOR_API_KEY_ENV:-${TT_E2E_QWEN_API_KEY_ENV:-}}"
  export TT_SUPERVISOR_REASONING_EFFORT="${TT_E2E_SUPERVISOR_REASONING_EFFORT:-${TT_E2E_QWEN_REASONING_EFFORT:-}}"
  export TT_SUPERVISOR_MAX_OUTPUT_TOKENS="${TT_E2E_SUPERVISOR_MAX_OUTPUT_TOKENS:-${TT_E2E_QWEN_MAX_OUTPUT_TOKENS:-4096}}"
  export TT_SUPERVISOR_TEMPERATURE="${TT_E2E_SUPERVISOR_TEMPERATURE:-${TT_E2E_QWEN_TEMPERATURE:-0.0}}"
}

e2e_start_managed_daemon() {
  local daemon_log="$1"
  if e2e_is_true "${TT_E2E_REUSE_CURRENT_DAEMON:-false}"; then
    [[ -n "${TT_E2E_SHARED_SOCKET_FILE:-}" ]] || e2e_fail "TT_E2E_SHARED_SOCKET_FILE is required when TT_E2E_REUSE_CURRENT_DAEMON=true"
    e2e_tt status >"$daemon_log" 2>&1 || e2e_fail "shared lab daemon is not responsive"
    printf 'shared lab daemon reused via %s\n' "$TT_E2E_SHARED_SOCKET_FILE" >>"$daemon_log"
    e2e_daemon_pid=""
    return 0
  fi
  printf 'repo-local v2 daemon startup is on-demand; using tt-cli request fallback\n' >"$daemon_log"
  e2e_daemon_pid=""
}

e2e_resolve_codex_rs_root() {
  if [[ -n "${TT_E2E_CODEX_RS_ROOT:-}" ]]; then
    printf '%s\n' "$TT_E2E_CODEX_RS_ROOT"
    return 0
  fi

  local candidate="$e2e_repo_root/../codex/codex-rs"
  if [[ -d "$candidate" ]]; then
    printf '%s\n' "$candidate"
    return 0
  fi

  e2e_fail "could not resolve codex-rs root; export TT_E2E_CODEX_RS_ROOT=/path/to/codex-rs"
}

e2e_resolve_codex_app_server_bin() {
  local build_log="${1:-}"
  if [[ -n "${TT_E2E_CODEX_APP_SERVER_BIN:-}" ]]; then
    [[ -x "$TT_E2E_CODEX_APP_SERVER_BIN" ]] || e2e_fail "TT_E2E_CODEX_APP_SERVER_BIN is not executable: $TT_E2E_CODEX_APP_SERVER_BIN"
    printf '%s\n' "$TT_E2E_CODEX_APP_SERVER_BIN"
    return 0
  fi

  local codex_rs_root
  codex_rs_root="$(e2e_resolve_codex_rs_root)"
  local bin_path="$codex_rs_root/target/debug/codex-app-server"
  if [[ ! -x "$bin_path" ]]; then
    if [[ -n "$build_log" ]]; then
      printf 'building codex-app-server from %s\n' "$codex_rs_root" >>"$build_log"
      cargo build --manifest-path "$codex_rs_root/Cargo.toml" -p codex-app-server >>"$build_log" 2>&1
      printf 'finished building codex-app-server\n' >>"$build_log"
    else
      cargo build --manifest-path "$codex_rs_root/Cargo.toml" -p codex-app-server
    fi
  fi
  [[ -x "$bin_path" ]] || e2e_fail "codex-app-server binary not found after build: $bin_path"
  printf '%s\n' "$bin_path"
}

e2e_wait_for_ws_listen_url() {
  local listen_url="$1"
  local timeout_seconds="${2:-15}"
  local socket_addr="${listen_url#ws://}"
  local host="${socket_addr%%:*}"
  local port="${socket_addr##*:}"

  [[ "$listen_url" == ws://* ]] || e2e_fail "unsupported websocket listen URL: $listen_url"
  [[ -n "$host" && -n "$port" ]] || e2e_fail "invalid websocket listen URL: $listen_url"

  local deadline=$((SECONDS + timeout_seconds))
  while (( SECONDS < deadline )); do
    if bash -lc "exec 3<>/dev/tcp/$host/$port" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.2
  done

  return 1
}

e2e_start_codex_app_server_for_repo() {
  local repo_root="$1"
  local app_server_log="$2"

  if e2e_is_true "${TT_E2E_REUSE_CURRENT_DAEMON:-false}"; then
    [[ -n "${TT_RUNTIME_LISTEN_URL:-}" ]] || e2e_fail "TT_RUNTIME_LISTEN_URL is required when reusing a shared Codex app-server"
    e2e_wait_for_ws_listen_url "$TT_RUNTIME_LISTEN_URL" 5 \
      || e2e_fail "shared Codex app-server is not reachable at $TT_RUNTIME_LISTEN_URL"
    printf 'shared Codex app-server reused via %s\n' "$TT_RUNTIME_LISTEN_URL" >>"$app_server_log"
    e2e_codex_app_server_pid=""
    return 0
  fi

  [[ -n "${TT_RUNTIME_LISTEN_URL:-}" ]] || e2e_fail "TT_RUNTIME_LISTEN_URL is not set for live scenario $E2E_SCENARIO_NAME"
  [[ -d "$repo_root/.codex" ]] || e2e_fail "repo-local .codex home does not exist: $repo_root/.codex"

  local app_server_bin
  : >"$app_server_log"
  printf 'preparing codex-app-server for repo %s\n' "$repo_root" >>"$app_server_log"
  app_server_bin="$(e2e_resolve_codex_app_server_bin "$app_server_log")"
  [[ -f "$repo_root/.codex/auth.json" ]] || {
    [[ -f "$HOME/.codex/auth.json" ]] || e2e_fail "Codex auth file is missing: $repo_root/.codex/auth.json and no bootstrap auth exists at $HOME/.codex/auth.json"
    cp "$HOME/.codex/auth.json" "$repo_root/.codex/auth.json" \
      || e2e_fail "failed to seed repo-local Codex auth into $repo_root/.codex/auth.json"
  }

  printf 'starting codex-app-server via %s on %s\n' "$app_server_bin" "$TT_RUNTIME_LISTEN_URL" >>"$app_server_log"
  export TT_CODEX_APP_SERVER_LOG_PATH="$app_server_log"
  (
    export CODEX_HOME="$repo_root/.codex"
    export XDG_DATA_HOME="$E2E_SCENARIO_XDG_DATA_HOME"
    export XDG_CONFIG_HOME="$E2E_SCENARIO_XDG_CONFIG_HOME"
    export XDG_RUNTIME_DIR="$E2E_SCENARIO_XDG_RUNTIME_HOME"
    exec "$app_server_bin" --listen "$TT_RUNTIME_LISTEN_URL"
  ) >>"$app_server_log" 2>&1 &
  e2e_codex_app_server_pid="$!"

  e2e_wait_for_ws_listen_url "$TT_RUNTIME_LISTEN_URL" 15 || {
    echo "Codex app-server failed to become ready at $TT_RUNTIME_LISTEN_URL" >&2
    echo "app-server log: $app_server_log" >&2
    if [[ -n "$e2e_codex_app_server_pid" ]]; then
      kill "$e2e_codex_app_server_pid" >/dev/null 2>&1 || true
      wait "$e2e_codex_app_server_pid" >/dev/null 2>&1 || true
      e2e_codex_app_server_pid=""
    fi
    return 1
  }
  printf 'codex-app-server ready at %s\n' "$TT_RUNTIME_LISTEN_URL" >>"$app_server_log"
}

e2e_stop_managed_daemon() {
  if e2e_is_true "${TT_E2E_REUSE_CURRENT_DAEMON:-false}"; then
    return 0
  fi
  if [[ -n "$e2e_codex_app_server_pid" ]]; then
    kill "$e2e_codex_app_server_pid" >/dev/null 2>&1 || true
    wait "$e2e_codex_app_server_pid" >/dev/null 2>&1 || true
    e2e_codex_app_server_pid=""
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

e2e_require_tt() {
  if [[ -x "$e2e_repo_root/target/debug/tt-cli" ]]; then
    return 0
  fi
  if [[ -n "${TT_RUNTIME_BIN:-}" && -x "$TT_RUNTIME_BIN" ]]; then
    return 0
  fi
  command -v tt-cli >/dev/null 2>&1 || e2e_fail "scenario $E2E_SCENARIO_NAME requires tt-cli on PATH or TT_RUNTIME_BIN"
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

e2e_tt() {
  "$e2e_bin_dir/tt.sh" "$@"
}

e2e_field_value() {
  local key="$1"
  local file="$2"
  sed -n "s/^${key}: //p" "$file" | head -n1
}

e2e_prepare_live_tt_environment() {
  local suffix="$1"
  local listen_port_base="$2"
  local supervisor_max_output_tokens="${3:-${TT_SUPERVISOR_MAX_OUTPUT_TOKENS:-4096}}"

  if e2e_using_shared_lab; then
    return 0
  fi

  local listen_port="$((listen_port_base + ($(printf '%s' "$E2E_RUN_ID" | cksum | awk '{print $1}') % 1000)))"
  local listen_url="ws://127.0.0.1:$listen_port"
  local supervisor_base_url="${TT_SUPERVISOR_BASE_URL:-http://127.0.0.1:8000/v1}"
  local supervisor_model="${TT_SUPERVISOR_MODEL:-gpt-oss-20b}"
  local supervisor_api_key_env="${TT_SUPERVISOR_API_KEY_ENV:-}"
  local supervisor_reasoning_effort="${TT_SUPERVISOR_REASONING_EFFORT:-}"
  local supervisor_temperature="${TT_SUPERVISOR_TEMPERATURE:-0.0}"
  local tt_bin="${TT_RUNTIME_BIN:-$e2e_repo_root/target/debug/tt-cli}"

  e2e_use_short_xdg_paths "$suffix"
  mkdir -p "$E2E_SCENARIO_XDG_CONFIG_HOME/tt"

  cat >"$E2E_SCENARIO_XDG_CONFIG_HOME/tt/config.toml" <<EOF
[tt]
binary_path = "$tt_bin"
listen_url = "$listen_url"
connection_mode = "spawn_if_needed"
config_overrides = []

[tt.reconnect]
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

  if [[ -z "${TT_CODEX_BIN:-}" ]]; then
    local codex_bin
    codex_bin="$(command -v codex || true)"
    [[ -n "$codex_bin" ]] && export TT_CODEX_BIN="$codex_bin"
  fi
  if [[ -z "${TT_CODEX_APP_SERVER_BIN:-}" ]]; then
    export TT_CODEX_APP_SERVER_BIN
    TT_CODEX_APP_SERVER_BIN="$(e2e_resolve_codex_app_server_bin "$E2E_SCENARIO_LOGS_DIR/codex-app-server.log")"
  fi
  export TT_RUNTIME_LISTEN_URL="$listen_url"
  export TT_APP_SERVER_LISTEN_URL="$listen_url"
  export CODEX_APP_SERVER_LISTEN_URL="$listen_url"
  export TT_CODEX_TURN_SOFT_SILENCE_SECS="${SOFT_SILENCE_SECONDS:-900}"
  export TT_CODEX_TURN_HARD_CEILING_SECS="${HARD_CEILING_SECONDS:-7200}"
  export TT_CODEX_TURN_WAIT_TIMEOUT_SECS="${TT_CODEX_TURN_WAIT_TIMEOUT_SECS:-${TT_CODEX_TURN_HARD_CEILING_SECS}}"
  export TT_MANAGED_PROJECT_EXPECTED_LONG_BUILD="${EXPECTED_LONG_BUILD:-false}"
  export TT_MANAGED_PROJECT_REQUIRES_PROGRESS_UPDATES="${REQUIRES_PROGRESS_UPDATES:-true}"
  export TT_MANAGED_PROJECT_SOFT_SILENCE_SECONDS="${SOFT_SILENCE_SECONDS:-900}"
  export TT_MANAGED_PROJECT_HARD_CEILING_SECONDS="${HARD_CEILING_SECONDS:-7200}"
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
  git -C "$repo_root" config user.name "TT E2E"
  git -C "$repo_root" config user.email "tt-e2e@example.com"
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
  git -C "$repo_root" config user.name "TT E2E"
  git -C "$repo_root" config user.email "tt-e2e@example.com"

  cat >"$repo_root/README.md" <<EOF
# ${E2E_SCENARIO_NAME:-tt-e2e}

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

  e2e_tt workunit thread add \
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
    reports_output="$("$e2e_bin_dir/tt.sh" supervisor work reports list-for-workunit --workunit "$workunit_id" 2>/dev/null || true)"
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
    assignment_output="$("$e2e_bin_dir/tt.sh" supervisor work assignments get --assignment "$assignment_id" 2>/dev/null || true)"
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
  e2e_tt workstreams runtime get --workstream "$workstream_id" >"$output_file"
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
  e2e_tt tt threads list --workstream "$workstream_id" >"$output_file"
}

e2e_assert_managed_thread_count() {
  local output_file="$1"
  local expected_count="$2"
  local actual_count
  actual_count="$(grep -c 'management=managed' "$output_file" || true)"
  test "$actual_count" -eq "$expected_count"
  ! grep -q 'management=observed_unmanaged' "$output_file"
}
