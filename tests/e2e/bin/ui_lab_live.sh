#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
lab_root="${ORCAS_UI_E2E_LAB_ROOT:-$repo_root/target/ui-e2e-lab}"
lab_config_home="$lab_root/config"
lab_data_home="$lab_root/data"
lab_runtime_home="$lab_root/runtime"
lab_socket_file="$lab_runtime_home/orcas/orcasd.sock"
lab_server_pid_file="$lab_root/orcas-server.pid"
lab_daemon_pid_file="$lab_root/orcasd.pid"
lab_server_bind="${ORCAS_UI_E2E_SERVER_BIND:-127.0.0.1:3000}"
lab_default_listen_url="${ORCAS_UI_E2E_CODEX_LISTEN_URL:-ws://127.0.0.1:4510}"
supported_scenarios=(
  live-worker-direct-patch
  live-supervisor-micro-proposal
  live-reject-redirect
  live-worktree-lifecycle
  supervisor-planning
)

usage() {
  cat <<EOF
usage: $0 <command> [args]

Commands:
  reset               Recreate the shared UI lab state and config.
  start               Start the lab daemon and lab orcas-server.
  stop                Stop the lab daemon and lab orcas-server.
  restart             Reset and then start the lab.
  run <scenario>      Run one supported scenario into the shared UI lab.
  run-all             Run all supported scenarios sequentially into the shared UI lab.
  env                 Print the environment block used for shared-lab scenario runs.
  list                Print the supported shared-lab scenarios.
EOF
}

ensure_lab_dirs() {
  mkdir -p "$lab_config_home/orcas" "$lab_data_home/orcas" "$lab_runtime_home/orcas"
  mkdir -p "$lab_data_home/orcas/logs"
  chmod 700 "$lab_runtime_home" || true
}

write_lab_config() {
  local user_config="$HOME/.config/orcas/config.toml"
  if [[ -f "$user_config" ]]; then
    cp "$user_config" "$lab_config_home/orcas/config.toml"
    python3 - "$lab_config_home/orcas/config.toml" "$lab_default_listen_url" <<'PY'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
listen_url = sys.argv[2]
raw = path.read_text()
lines = raw.splitlines()
out = []
in_codex = False
listen_done = False
for line in lines:
    stripped = line.strip()
    if stripped.startswith("[") and stripped.endswith("]"):
        if in_codex and not listen_done:
            out.append(f'listen_url = "{listen_url}"')
            listen_done = True
        in_codex = stripped == "[codex]"
        out.append(line)
        continue
    if in_codex and stripped.startswith("listen_url ="):
        out.append(f'listen_url = "{listen_url}"')
        listen_done = True
    else:
        out.append(line)
if in_codex and not listen_done:
    out.append(f'listen_url = "{listen_url}"')
path.write_text("\n".join(out) + "\n")
PY
  else
    cat >"$lab_config_home/orcas/config.toml" <<EOF
[codex]
binary_path = "/home/emmy/git/codex/codex-rs/target/debug/codex"
listen_url = "$lab_default_listen_url"
connection_mode = "spawn_if_needed"
config_overrides = []

[codex.reconnect]
initial_delay_ms = 150
max_delay_ms = 5000
multiplier = 2.0

[defaults]
model = "gpt-5"

[supervisor]
base_url = "http://127.0.0.1:8000/v1"
api_key_env = ""
model = "gpt-oss-20b"
reasoning_effort = ""
max_output_tokens = 2048

[supervisor.proposals]
auto_create_on_report_recorded = false
EOF
  fi
}

kill_from_pid_file() {
  local pid_file="$1"
  if [[ -f "$pid_file" ]]; then
    local pid
    pid="$(cat "$pid_file")"
    if [[ -n "$pid" ]] && kill -0 "$pid" >/dev/null 2>&1; then
      kill "$pid" >/dev/null 2>&1 || true
      wait "$pid" >/dev/null 2>&1 || true
    fi
    rm -f "$pid_file"
  fi
}

stop_lab() {
  kill_from_pid_file "$lab_server_pid_file"
  kill_from_pid_file "$lab_daemon_pid_file"
}

wait_for_socket() {
  local socket_path="$1"
  local attempts="${2:-30}"
  for _ in $(seq 1 "$attempts"); do
    [[ -S "$socket_path" ]] && return 0
    sleep 1
  done
  return 1
}

wait_for_http() {
  local url="$1"
  local attempts="${2:-30}"
  for _ in $(seq 1 "$attempts"); do
    if curl -sf -H 'content-type: application/json' -d '{}' "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  return 1
}

reset_lab() {
  stop_lab
  rm -rf "$lab_root"
  ensure_lab_dirs
  write_lab_config
}

start_lab() {
  stop_lab
  ensure_lab_dirs
  nohup env \
    XDG_CONFIG_HOME="$lab_config_home" \
    XDG_DATA_HOME="$lab_data_home" \
    XDG_RUNTIME_DIR="$lab_runtime_home" \
    "$repo_root/target/debug/orcasd" \
    >"$lab_data_home/orcas/logs/ui-e2e-lab-orcasd.stdout.log" 2>&1 </dev/null &
  echo $! >"$lab_daemon_pid_file"
  wait_for_socket "$lab_socket_file" || {
    cat "$lab_data_home/orcas/logs/ui-e2e-lab-orcasd.stdout.log" >&2 || true
    echo "shared UI lab daemon did not create $lab_socket_file" >&2
    return 1
  }
  env \
    XDG_CONFIG_HOME="$lab_config_home" \
    XDG_DATA_HOME="$lab_data_home" \
    XDG_RUNTIME_DIR="$lab_runtime_home" \
    "$repo_root/target/debug/orcas" daemon status >/dev/null 2>&1 || {
      cat "$lab_data_home/orcas/logs/ui-e2e-lab-orcasd.stdout.log" >&2 || true
      echo "shared UI lab daemon is not responsive" >&2
      return 1
    }
  nohup env \
    XDG_CONFIG_HOME="$lab_config_home" \
    XDG_DATA_HOME="$lab_data_home" \
    XDG_RUNTIME_DIR="$lab_runtime_home" \
    "$repo_root/target/debug/orcas-server" --bind "$lab_server_bind" \
    >"$lab_data_home/orcas/logs/ui-e2e-lab-orcas-server.stdout.log" 2>&1 </dev/null &
  echo $! >"$lab_server_pid_file"
  wait_for_http "http://$lab_server_bind/operator-runtime/planning-sessions/list" || {
    cat "$lab_data_home/orcas/logs/ui-e2e-lab-orcas-server.stdout.log" >&2 || true
    echo "shared UI lab server is not responding on $lab_server_bind" >&2
    return 1
  }
}

print_env() {
  cat <<EOF
ORCAS_E2E_REUSE_CURRENT_XDG=true
ORCAS_E2E_REUSE_CURRENT_DAEMON=true
ORCAS_E2E_SHARED_XDG_DIR=$lab_root
ORCAS_E2E_SHARED_XDG_DATA_HOME=$lab_data_home
ORCAS_E2E_SHARED_XDG_CONFIG_HOME=$lab_config_home
ORCAS_E2E_SHARED_XDG_RUNTIME_HOME=$lab_runtime_home
ORCAS_E2E_SHARED_SOCKET_FILE=$lab_socket_file
EOF
}

run_scenario() {
  local scenario="$1"
  ORCAS_E2E_REUSE_CURRENT_XDG=true \
    ORCAS_E2E_REUSE_CURRENT_DAEMON=true \
    ORCAS_E2E_SHARED_XDG_DIR="$lab_root" \
    ORCAS_E2E_SHARED_XDG_DATA_HOME="$lab_data_home" \
    ORCAS_E2E_SHARED_XDG_CONFIG_HOME="$lab_config_home" \
    ORCAS_E2E_SHARED_XDG_RUNTIME_HOME="$lab_runtime_home" \
    ORCAS_E2E_SHARED_SOCKET_FILE="$lab_socket_file" \
    make test-e2e-live SCENARIO="$scenario"
}

case "${1:-}" in
  reset)
    reset_lab
    ;;
  start)
    start_lab
    ;;
  stop)
    stop_lab
    ;;
  restart)
    reset_lab
    start_lab
    ;;
  run)
    [[ -n "${2:-}" ]] || usage
    run_scenario "$2"
    ;;
  run-all)
    for scenario in "${supported_scenarios[@]}"; do
      run_scenario "$scenario"
    done
    ;;
  env)
    print_env
    ;;
  list)
    printf '%s\n' "${supported_scenarios[@]}"
    ;;
  *)
    usage
    exit 2
    ;;
esac
