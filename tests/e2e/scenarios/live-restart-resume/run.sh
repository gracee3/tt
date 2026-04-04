#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "$0")/../../lib" && pwd)/common.sh"
scenario_dir="$(cd "$(dirname "$0")" && pwd)"
fixture_dir="$scenario_dir/fixture"

e2e_load_scenario_metadata "$scenario_dir"
e2e_prepare_scenario_dirs "$NAME"
e2e_prepare_live_codex_environment "lrs" 4700 16384

worktree_path="$E2E_SCENARIO_WORKTREES_DIR/lane"
repo_root="$E2E_SCENARIO_WORKTREES_DIR/lane-repo"
daemon_internal_log="$E2E_SCENARIO_ORCAS_HOME/logs/orcasd.log"
reports_dir="$E2E_SCENARIO_REPORTS_DIR"
artifacts_dir="$E2E_SCENARIO_ARTIFACTS_DIR"
base_ref="${ORCAS_E2E_GIT_BASE_REF:-main}"
branch_suffix="${E2E_RUN_ID//[^a-zA-Z0-9]/-}"
branch_name="orcas/$NAME/$branch_suffix"

mkdir -p "$reports_dir" "$artifacts_dir"
e2e_prepare_fixture_repo_with_worktree "$fixture_dir" "$repo_root" "$worktree_path" "$branch_name" "$base_ref" "$reports_dir" "lane"

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
    --objective "Prove the live tracked-thread lane can survive daemon interruption and resume cleanly" \
    --priority normal
)"
workstream_id="$(printf '%s\n' "$workstream_output" | awk -F': ' '/^workstream_id:/ {print $2; exit}')"

workunit_output="$(
  e2e_orcas workunit create \
    --workstream "$workstream_id" \
    --title "Fix the tiny greeting bug" \
    --task "Inspect the tiny C program and failing shell test in the declared tracked-thread worktree lane. Make the smallest code change needed so make test passes. Do not refactor unrelated code."
)"
workunit_id="$(printf '%s\n' "$workunit_output" | awk -F': ' '/^work_unit_id:/ {print $2; exit}')"

tracked_output="$(
  e2e_add_tracked_thread_workspace \
    "$workunit_id" \
    "Live restart resume lane" \
    "$repo_root" \
    "Dedicated tracked-thread worktree lane for restart and recovery validation" \
    "$repo_root" \
    "$worktree_path" \
    "$branch_name" \
    "$base_ref" \
    "$(git -C "$repo_root" rev-parse HEAD)" \
    "$base_ref" \
    manual \
    keep-until-campaign-closed \
    ready
)"
tracked_thread_id="$(printf '%s\n' "$tracked_output" | awk -F': ' '/^tracked_thread_id:/ {print $2; exit}')"

tracked_before_stdout="$reports_dir/tracked-thread-before-live.txt"
runtime_before_stdout="$reports_dir/workstream-runtime-before-live.txt"
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$tracked_before_stdout"
e2e_capture_workstream_runtime "$workstream_id" "$runtime_before_stdout"
e2e_assert_workstream_runtime "$workstream_id" "$runtime_before_stdout"
e2e_assert_runtime_thread_count "$runtime_before_stdout" 0

assignment_stdout="$reports_dir/assignment-start.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" supervisor work assignments start \
  --workunit "$workunit_id" \
  --worker live-restart-resume-worker \
  --worker-kind codex \
  --instructions "Inspect the tiny C program and failing shell test in the declared tracked-thread worktree lane. Make the smallest possible code change in main.c to make make test pass. Do not refactor unrelated code, do not touch the test script unless required, and keep the fix bounded to one file if possible." \
  --cwd "$worktree_path" \
  >"$assignment_stdout" 2>&1 &
assignment_start_pid=$!

active_turn_line=""
thread_id=""
turn_id=""
turns_active_stdout="$reports_dir/turns-active-before-stop.txt"
threads_read_stdout="$reports_dir/thread-read-before-stop.txt"
tracked_after_start_stdout="$reports_dir/tracked-thread-after-start.txt"
runtime_after_start_stdout="$reports_dir/workstream-runtime-after-start.txt"
threads_list_after_start_stdout="$reports_dir/workstream-threads-after-start.txt"
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
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$tracked_after_start_stdout"
e2e_capture_workstream_runtime "$workstream_id" "$runtime_after_start_stdout"
e2e_capture_workstream_threads "$workstream_id" "$threads_list_after_start_stdout"
e2e_assert_workstream_runtime "$workstream_id" "$runtime_after_start_stdout"
e2e_assert_runtime_thread_count "$runtime_after_start_stdout" 1
e2e_assert_managed_thread_count "$threads_list_after_start_stdout" 1

grep -q "turn_in_flight: true" "$threads_read_stdout"
grep -q "$thread_id" "$turns_active_stdout"
grep -q "$turn_id" "$turns_active_stdout"
grep -q "binding_state: Bound" "$tracked_after_start_stdout"
grep -q "upstream_thread_id: $thread_id" "$tracked_after_start_stdout"

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
git_status_stdout="$reports_dir/git-status.txt"
tree_diff_stdout="$reports_dir/tree-diff.txt"
tracked_after_restart_stdout="$reports_dir/tracked-thread-after-restart.txt"
runtime_after_restart_stdout="$reports_dir/workstream-runtime-after-restart.txt"
threads_list_after_restart_stdout="$reports_dir/workstream-threads-after-restart.txt"
for _ in $(seq 1 120); do
  "$e2e_bin_dir/orcas.sh" turns get --thread "$thread_id" --turn "$turn_id" \
    >"$turn_get_after_stdout" 2>&1 || true
  reports_output_final="$("$e2e_bin_dir/orcas.sh" supervisor work reports list-for-workunit --workunit "$workunit_id" 2>/dev/null || true)"
  report_id="$(printf '%s\n' "$reports_output_final" | awk -F'\t' '/^report-/ {print $1; exit}')"
  [[ -n "$report_id" ]] && break
  sleep 5
done

test -n "$report_id"
report_count="$(printf '%s\n' "$reports_output_final" | sed '/^$/d' | wc -l | tr -d ' ')"
test "$report_count" -eq 1
e2e_orcas supervisor work reports get --report "$report_id" >"$report_get_stdout"
assignment_id="$(e2e_field_value assignment_id "$report_get_stdout")"
report_parse_result="$(e2e_field_value parse_result "$report_get_stdout")"

e2e_orcas supervisor work assignments get --assignment "$assignment_id" >"$assignment_after_get_stdout"
assignment_status="$(e2e_field_value status "$assignment_after_get_stdout")"
worker_session_id="$(e2e_field_value worker_session_id "$assignment_after_get_stdout")"

make -C "$worktree_path" test >"$make_test_stdout"
make -C "$worktree_path" clean >/dev/null 2>&1 || true
git -C "$worktree_path" status --short >"$git_status_stdout"
diff -qr --exclude=.git "$fixture_dir" "$worktree_path" >"$tree_diff_stdout" || true
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$tracked_after_restart_stdout"
e2e_capture_workstream_runtime "$workstream_id" "$runtime_after_restart_stdout"
e2e_capture_workstream_threads "$workstream_id" "$threads_list_after_restart_stdout"
e2e_assert_workstream_runtime "$workstream_id" "$runtime_after_restart_stdout"
e2e_assert_runtime_thread_count "$runtime_after_restart_stdout" 1
e2e_assert_managed_thread_count "$threads_list_after_restart_stdout" 1

test -n "$assignment_id"
test -n "$worker_session_id"
test -n "$report_parse_result"
test "$assignment_status" = "AwaitingDecision"
test "$report_parse_result" != "Invalid"
test "$(sed '/^$/d' "$tree_diff_stdout" | wc -l | tr -d ' ')" -eq 1
grep -q '^PASS$' "$make_test_stdout"
grep -qx ' M main.c' "$git_status_stdout"
grep -q 'main.c' "$tree_diff_stdout"
grep -q "assignment_id: $assignment_id" "$assignment_after_get_stdout"
grep -q "report_id: $report_id" "$report_get_stdout"
grep -q "assignment_id: $assignment_id" "$report_get_stdout"
grep -q "work_unit_id: $workunit_id" "$report_get_stdout"
grep -q "status: AwaitingDecision" "$assignment_after_get_stdout"
grep -Eq "parse_result: (Parsed|Ambiguous)" "$report_get_stdout"
grep -q "binding_state: Bound" "$tracked_after_restart_stdout"
grep -q "upstream_thread_id: $thread_id" "$tracked_after_restart_stdout"
grep -q "lifecycle: completed" "$turn_get_after_stdout"
grep -q "terminal: true" "$turn_get_after_stdout"
e2e_orcas codex turns list-active >"$turns_active_after_stdout"
! grep -q "$turn_id" "$turns_active_after_stdout"

wait "$assignment_start_pid" >/dev/null 2>&1 || true

echo "PASS"
