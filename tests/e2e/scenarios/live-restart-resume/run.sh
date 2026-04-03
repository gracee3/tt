#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "$0")/../../lib" && pwd)/common.sh"
scenario_dir="$(cd "$(dirname "$0")" && pwd)"
fixture_dir="$scenario_dir/fixture"

e2e_load_scenario_metadata "$scenario_dir"
e2e_prepare_scenario_dirs "$NAME"

field_value() {
  local key="$1"
  local file="$2"
  sed -n "s/^${key}: //p" "$file" | head -n1
}

short_xdg_root="$e2e_output_root/xdg/$E2E_RUN_ID/lrs"
short_xdg_data_home="$short_xdg_root/data"
short_xdg_config_home="$short_xdg_root/config"
short_xdg_runtime_home="$short_xdg_root/runtime"
listen_port="$((4700 + ($(printf '%s' "$E2E_RUN_ID" | cksum | awk '{print $1}') % 1000)))"
listen_url="ws://127.0.0.1:$listen_port"
supervisor_base_url="${ORCAS_SUPERVISOR_BASE_URL:-http://127.0.0.1:8000/v1}"
supervisor_model="${ORCAS_SUPERVISOR_MODEL:-gpt-oss-20b}"
supervisor_api_key_env="${ORCAS_SUPERVISOR_API_KEY_ENV:-}"
supervisor_reasoning_effort="${ORCAS_SUPERVISOR_REASONING_EFFORT:-}"
supervisor_max_output_tokens="${ORCAS_SUPERVISOR_MAX_OUTPUT_TOKENS:-16384}"

rm -rf "$short_xdg_root"
mkdir -p "$short_xdg_data_home/orcas" "$short_xdg_config_home/orcas" "$short_xdg_runtime_home/orcas"
chmod 700 "$short_xdg_runtime_home" || true

cat >"$short_xdg_config_home/orcas/config.toml" <<EOF
[codex]
binary_path = "/home/emmy/git/codex/codex-rs/target/debug/codex"
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
temperature = ${ORCAS_SUPERVISOR_TEMPERATURE:-0.0}
max_output_tokens = $supervisor_max_output_tokens

[supervisor.proposals]
auto_create_on_report_recorded = false
EOF

export E2E_SCENARIO_XDG_DIR="$short_xdg_root"
export E2E_SCENARIO_XDG_DATA_HOME="$short_xdg_data_home"
export E2E_SCENARIO_XDG_CONFIG_HOME="$short_xdg_config_home"
export E2E_SCENARIO_XDG_RUNTIME_HOME="$short_xdg_runtime_home"
export ORCAS_E2E_XDG_DATA_HOME="$short_xdg_data_home"
export ORCAS_E2E_XDG_CONFIG_HOME="$short_xdg_config_home"
export ORCAS_E2E_XDG_RUNTIME_HOME="$short_xdg_runtime_home"
export ORCAS_CODEX_LISTEN_URL="$listen_url"

fixture_repo="$E2E_SCENARIO_WORKTREES_DIR/lane"
daemon_internal_log="$short_xdg_data_home/orcas/logs/orcasd.log"
reports_dir="$E2E_SCENARIO_REPORTS_DIR"
artifacts_dir="$E2E_SCENARIO_ARTIFACTS_DIR"

rm -rf "$fixture_repo"
mkdir -p "$fixture_repo" "$reports_dir" "$artifacts_dir"
cp -R "$fixture_dir/." "$fixture_repo/"

start_daemon() {
  local stdout_file="$1"
  e2e_orcas daemon start --force-spawn >"$stdout_file" 2>&1 &
  daemon_pid=$!
}

wait_for_daemon_exit() {
  local pid="$1"
  for _ in $(seq 1 60); do
    if ! kill -0 "$pid" >/dev/null 2>&1; then
      wait "$pid" >/dev/null 2>&1 || true
      return 0
    fi
    sleep 1
  done
  return 1
}

start_daemon "$reports_dir/daemon-start-phase1.txt"
cleanup() {
  kill "$daemon_pid" >/dev/null 2>&1 || true
}
trap cleanup EXIT

sleep 5

workstream_output="$(
  e2e_orcas workstreams create \
    --title "Live restart resume" \
    --objective "Prove the live worker turn can survive daemon interruption and resume cleanly" \
    --priority normal
)"
workstream_id="$(printf '%s\n' "$workstream_output" | awk -F': ' '/^workstream_id:/ {print $2; exit}')"

workunit_output="$(
  e2e_orcas workunit create \
    --workstream "$workstream_id" \
    --title "Fix the tiny greeting bug" \
    --task "Inspect the tiny C program and failing shell test in the fixture repo. Make the smallest code change needed so make test passes. Do not refactor unrelated code."
)"
workunit_id="$(printf '%s\n' "$workunit_output" | awk -F': ' '/^work_unit_id:/ {print $2; exit}')"

assignment_stdout="$reports_dir/assignment-start.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" assignments start \
  --workunit "$workunit_id" \
  --worker live-restart-resume-worker \
  --worker-kind codex \
  --instructions "Inspect the tiny C program and failing shell test. Make the smallest possible code change in main.c to make make test pass. Do not refactor unrelated code, do not touch the test script unless required, and keep the fix bounded to one file if possible." \
  --cwd "$fixture_repo" \
  >"$assignment_stdout" 2>&1 &
assignment_start_pid=$!

active_turn_line=""
thread_id=""
turn_id=""
turns_active_stdout="$reports_dir/turns-active-before-stop.txt"
threads_read_stdout="$reports_dir/thread-read-before-stop.txt"
for _ in $(seq 1 120); do
  active_turns_output="$("$e2e_bin_dir/orcas.sh" turns list-active 2>/dev/null || true)"
  printf '%s\n' "$active_turns_output" >"$turns_active_stdout"
  active_turn_line="$(printf '%s\n' "$active_turns_output" | sed -n '1p')"
  if [[ -n "$active_turn_line" ]] && [[ "$active_turn_line" != "no active attachable turns" ]]; then
    thread_id="$(printf '%s\n' "$active_turn_line" | awk -F'\t' '{print $1}')"
    turn_id="$(printf '%s\n' "$active_turn_line" | awk -F'\t' '{print $2}')"
    [[ -n "$thread_id" && -n "$turn_id" ]] && break
  fi
  sleep 2
done

test -n "$workstream_id"
test -n "$workunit_id"
test -n "$thread_id"
test -n "$turn_id"

e2e_orcas codex threads read --thread "$thread_id" >"$threads_read_stdout"

grep -q "turn_in_flight: true" "$threads_read_stdout"
grep -q "$thread_id" "$turns_active_stdout"
grep -q "$turn_id" "$turns_active_stdout"

stop_stdout="$reports_dir/daemon-stop.txt"
e2e_orcas daemon stop >"$stop_stdout"
wait_for_daemon_exit "$daemon_pid"
cp "$daemon_internal_log" "$reports_dir/orcasd-before-restart.log"

start_daemon "$reports_dir/daemon-start-phase2.txt"
sleep 5
cp "$daemon_internal_log" "$reports_dir/orcasd-after-restart.log"

report_id=""
reports_output_final=""
report_get_stdout="$reports_dir/report-get.txt"
assignment_after_get_stdout="$reports_dir/assignment-get-after-restart.txt"
turns_active_after_stdout="$reports_dir/turns-active-after-restart.txt"
turn_get_after_stdout="$reports_dir/turn-get-after-restart.txt"
make_test_stdout="$reports_dir/make-test.txt"
tree_diff_stdout="$reports_dir/tree-diff.txt"
for _ in $(seq 1 120); do
  "$e2e_bin_dir/orcas.sh" turns get --thread "$thread_id" --turn "$turn_id" \
    >"$turn_get_after_stdout" 2>&1 || true
  reports_output_final="$("$e2e_bin_dir/orcas.sh" reports list-for-workunit --workunit "$workunit_id" 2>/dev/null || true)"
  report_id="$(printf '%s\n' "$reports_output_final" | awk -F'\t' '/^report-/ {print $1; exit}')"
  [[ -n "$report_id" ]] && break
  sleep 5
done

test -n "$report_id"
report_count="$(printf '%s\n' "$reports_output_final" | sed '/^$/d' | wc -l | tr -d ' ')"
test "$report_count" -eq 1
e2e_orcas supervisor work reports get --report "$report_id" >"$report_get_stdout"
assignment_id="$(field_value assignment_id "$report_get_stdout")"
report_parse_result="$(field_value parse_result "$report_get_stdout")"

e2e_orcas supervisor work assignments get --assignment "$assignment_id" >"$assignment_after_get_stdout"
assignment_status="$(field_value status "$assignment_after_get_stdout")"
worker_session_id="$(field_value worker_session_id "$assignment_after_get_stdout")"

make -C "$fixture_repo" test >"$make_test_stdout"
diff -qr "$fixture_dir" "$fixture_repo" >"$tree_diff_stdout" || true

test -f "$fixture_repo/main.c"
changed_count="$(sed '/^$/d' "$tree_diff_stdout" | wc -l | tr -d ' ')"
test "$changed_count" -eq 1
grep -q 'main.c' "$tree_diff_stdout"

test -n "$assignment_id"
test -n "$worker_session_id"
test -n "$report_parse_result"
test "$assignment_status" = "AwaitingDecision"
test "$report_parse_result" != "Invalid"
grep -q '^PASS$' "$make_test_stdout"
grep -q "assignment_id: $assignment_id" "$assignment_after_get_stdout"
grep -q "report_id: $report_id" "$report_get_stdout"
grep -q "assignment_id: $assignment_id" "$report_get_stdout"
grep -q "work_unit_id: $workunit_id" "$report_get_stdout"
grep -q "status: AwaitingDecision" "$assignment_after_get_stdout"
grep -Eq "parse_result: (Parsed|Ambiguous)" "$report_get_stdout"
grep -q "lifecycle: completed" "$turn_get_after_stdout"
grep -q "terminal: true" "$turn_get_after_stdout"
e2e_orcas codex turns list-active >"$turns_active_after_stdout"
! grep -q "$turn_id" "$turns_active_after_stdout"

wait "$assignment_start_pid" >/dev/null 2>&1 || true

echo "PASS"
