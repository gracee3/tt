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

short_xdg_root="$e2e_output_root/xdg/$E2E_RUN_ID/lwdp"
short_xdg_data_home="$short_xdg_root/data"
short_xdg_config_home="$short_xdg_root/config"
short_xdg_runtime_home="$short_xdg_root/runtime"
listen_port="$((4600 + ($(printf '%s' "$E2E_RUN_ID" | cksum | awk '{print $1}') % 1000)))"
listen_url="ws://127.0.0.1:$listen_port"

rm -rf "$short_xdg_root"
mkdir -p "$short_xdg_data_home/orcas" "$short_xdg_config_home/orcas" "$short_xdg_runtime_home/orcas"
chmod 700 "$short_xdg_runtime_home" || true

export E2E_SCENARIO_XDG_DIR="$short_xdg_root"
export E2E_SCENARIO_XDG_DATA_HOME="$short_xdg_data_home"
export E2E_SCENARIO_XDG_CONFIG_HOME="$short_xdg_config_home"
export E2E_SCENARIO_XDG_RUNTIME_HOME="$short_xdg_runtime_home"
export ORCAS_E2E_XDG_DATA_HOME="$short_xdg_data_home"
export ORCAS_E2E_XDG_CONFIG_HOME="$short_xdg_config_home"
export ORCAS_E2E_XDG_RUNTIME_HOME="$short_xdg_runtime_home"
export ORCAS_CODEX_LISTEN_URL="$listen_url"

fixture_repo="$E2E_SCENARIO_WORKTREES_DIR/lane"
daemon_log="$E2E_SCENARIO_LOGS_DIR/orcasd.log"
reports_dir="$E2E_SCENARIO_REPORTS_DIR"
artifacts_dir="$E2E_SCENARIO_ARTIFACTS_DIR"

rm -rf "$fixture_repo"
mkdir -p "$fixture_repo" "$reports_dir" "$artifacts_dir"
cp -R "$fixture_dir/." "$fixture_repo/"

e2e_orcas daemon start --force-spawn >"$daemon_log" 2>&1 &
daemon_pid=$!
cleanup() {
  kill "$daemon_pid" >/dev/null 2>&1 || true
}
trap cleanup EXIT

sleep 5

workstream_output="$(
  e2e_orcas workstreams create \
    --title "Live worker direct patch" \
    --objective "Prove one real live worker turn can land a bounded code fix on disk" \
    --priority normal
)"
workstream_id="$(printf '%s\n' "$workstream_output" | awk -F': ' '/^workstream_id:/ {print $2; exit}')"

workunit_output="$(
  e2e_orcas workunits create \
    --workstream "$workstream_id" \
    --title "Fix the tiny greeting bug" \
    --task "Inspect the tiny C program and failing shell test in the fixture repo. Make the smallest code change needed so make test passes. Do not refactor unrelated code."
)"
workunit_id="$(printf '%s\n' "$workunit_output" | awk -F': ' '/^work_unit_id:/ {print $2; exit}')"

assignment_stdout="$reports_dir/assignment-start.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" assignments start \
  --workunit "$workunit_id" \
  --worker live-worker-direct-patch-worker \
  --worker-kind codex \
  --instructions "Inspect the tiny C program and failing shell test. Make the smallest possible code change in main.c to make make test pass. Do not refactor unrelated code, do not touch the test script unless required, and keep the fix bounded to one file if possible." \
  --cwd "$fixture_repo" \
  >"$assignment_stdout" 2>&1 &
assignment_start_pid=$!

report_id=""
for _ in $(seq 1 120); do
  reports_output="$("$e2e_bin_dir/orcas.sh" reports list-for-workunit --workunit "$workunit_id" 2>/dev/null || true)"
  report_id="$(printf '%s\n' "$reports_output" | awk -F'\t' '/^report-/ {print $1; exit}')"
  [[ -n "$report_id" ]] && break
  sleep 5
done

test -n "$workstream_id"
test -n "$workunit_id"
test -n "$report_id"

assignment_get_stdout="$reports_dir/assignment-get.txt"
report_get_stdout="$reports_dir/report-get.txt"
make_test_stdout="$reports_dir/make-test.txt"
tree_diff_stdout="$reports_dir/tree-diff.txt"

e2e_orcas reports get --report "$report_id" >"$report_get_stdout"
assignment_id="$(field_value assignment_id "$report_get_stdout")"
report_parse_result="$(field_value parse_result "$report_get_stdout")"

make -C "$fixture_repo" test >"$make_test_stdout"
diff -qr "$fixture_dir" "$fixture_repo" >"$tree_diff_stdout" || true

test -f "$fixture_repo/main.c"

changed_count="$(sed '/^$/d' "$tree_diff_stdout" | wc -l | tr -d ' ')"
test "$changed_count" -eq 1
grep -q 'main.c' "$tree_diff_stdout"

e2e_orcas assignments get --assignment "$assignment_id" >"$assignment_get_stdout"

assignment_status="$(field_value status "$assignment_get_stdout")"
worker_session_id="$(field_value worker_session_id "$assignment_get_stdout")"

test -n "$assignment_id"
test -n "$worker_session_id"
test -n "$report_parse_result"
test "$assignment_status" = "AwaitingDecision"
test "$report_parse_result" != "Invalid"

grep -q '^PASS$' "$make_test_stdout"
grep -q "assignment_id: $assignment_id" "$assignment_get_stdout"
grep -q "report_id: $report_id" "$report_get_stdout"
grep -q "assignment_id: $assignment_id" "$report_get_stdout"
grep -q "work_unit_id: $workunit_id" "$report_get_stdout"
grep -q "status: AwaitingDecision" "$assignment_get_stdout"
grep -Eq "parse_result: (Parsed|Ambiguous)" "$report_get_stdout"
grep -q "main.c" "$tree_diff_stdout"

wait "$assignment_start_pid" >/dev/null 2>&1 || true

echo "PASS"
