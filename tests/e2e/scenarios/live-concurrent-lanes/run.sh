#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "$0")/../../lib" && pwd)/common.sh"
scenario_dir="$(cd "$(dirname "$0")" && pwd)"
fixture_dir="$scenario_dir/fixture"

e2e_load_scenario_metadata "$scenario_dir"
e2e_prepare_scenario_dirs "$NAME"
e2e_prepare_live_codex_environment "cln" 6100 8192

base_ref="${ORCAS_E2E_GIT_BASE_REF:-main}"
branch_suffix="${E2E_RUN_ID//[^a-zA-Z0-9]/-}"
daemon_log="$E2E_SCENARIO_LOGS_DIR/orcasd.log"
reports_dir="$E2E_SCENARIO_REPORTS_DIR"
artifacts_dir="$E2E_SCENARIO_ARTIFACTS_DIR"

mkdir -p "$reports_dir" "$artifacts_dir"

setup_lane() {
  local prefix="$1"
  local workstream_id="$2"
  local lane_title="$3"
  local lane_task="$4"
  local lane_expected="$5"
  local repo_root="$E2E_SCENARIO_WORKTREES_DIR/${prefix}-repo"
  local worktree_path="$E2E_SCENARIO_WORKTREES_DIR/$prefix"
  local branch_name="orcas/$NAME/$prefix/$branch_suffix"
  local workunit_output tracked_output workunit_id tracked_thread_id

  e2e_prepare_fixture_repo_with_worktree "$fixture_dir" "$repo_root" "$worktree_path" "$branch_name" "$base_ref" "$reports_dir" "$prefix"

  workunit_output="$(
    e2e_orcas workunit create \
      --workstream "$workstream_id" \
      --title "$lane_title" \
      --task "$lane_task"
  )"
  workunit_id="$(printf '%s\n' "$workunit_output" | awk -F': ' '/^work_unit_id:/ {print $2; exit}')"

  tracked_output="$(
    e2e_add_tracked_thread_workspace \
      "$workunit_id" \
      "$lane_title" \
      "$repo_root" \
      "Dedicated tracked-thread worktree lane for live concurrent lane isolation validation" \
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

  printf '%s=%q\n' "${prefix}_repo_root" "$repo_root"
  printf '%s=%q\n' "${prefix}_worktree_path" "$worktree_path"
  printf '%s=%q\n' "${prefix}_branch_name" "$branch_name"
  printf '%s=%q\n' "${prefix}_workunit_id" "$workunit_id"
  printf '%s=%q\n' "${prefix}_tracked_thread_id" "$tracked_thread_id"
  printf '%s=%q\n' "${prefix}_expected_string" "$lane_expected"
}

collect_lane_results() {
  local prefix="$1"
  local workunit_id_var="${prefix}_workunit_id"
  local worktree_path_var="${prefix}_worktree_path"
  local branch_name_var="${prefix}_branch_name"
  local expected_string_var="${prefix}_expected_string"
  local tracked_thread_id_var="${prefix}_tracked_thread_id"
  local workunit_id="${!workunit_id_var}"
  local worktree_path="${!worktree_path_var}"
  local branch_name="${!branch_name_var}"
  local expected_string="${!expected_string_var}"
  local tracked_thread_id="${!tracked_thread_id_var}"
  local report_id_var="${prefix}_report_id"
  local assignment_stdout="$reports_dir/${prefix}-assignment-start.txt"
  local report_get_stdout="$reports_dir/${prefix}-report-get.txt"
  local assignment_get_stdout="$reports_dir/${prefix}-assignment-get.txt"
  local make_test_stdout="$reports_dir/${prefix}-make-test.txt"
  local git_status_stdout="$reports_dir/${prefix}-git-status.txt"
  local tree_diff_stdout="$reports_dir/${prefix}-tree-diff.txt"
  local tracked_after_stdout="$reports_dir/${prefix}-tracked-thread-after.txt"
  local report_id="${!report_id_var}"
  local assignment_id assignment_status worker_session_id report_parse_result thread_id changed_count

  e2e_orcas supervisor work reports get --report "$report_id" >"$report_get_stdout"
  assignment_id="$(e2e_field_value assignment_id "$report_get_stdout")"
  report_parse_result="$(e2e_field_value parse_result "$report_get_stdout")"

  e2e_orcas supervisor work assignments get --assignment "$assignment_id" >"$assignment_get_stdout"
  assignment_status="$(e2e_field_value status "$assignment_get_stdout")"
  worker_session_id="$(e2e_field_value worker_session_id "$assignment_get_stdout")"
  thread_id="$(e2e_field_value thread_id "$assignment_stdout")"

  make -C "$worktree_path" test >"$make_test_stdout"
  make -C "$worktree_path" clean >/dev/null 2>&1 || true
  git -C "$worktree_path" status --short --untracked-files=all >"$git_status_stdout"
  diff -qr --exclude=.git "$fixture_dir" "$worktree_path" >"$tree_diff_stdout" || true
  e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$tracked_after_stdout"

  changed_count="$(sed '/^$/d' "$tree_diff_stdout" | wc -l | tr -d ' ')"
  test -n "$assignment_id"
  test -n "$worker_session_id"
  test -n "$thread_id"
  test -n "$report_parse_result"
  test "$assignment_status" = "AwaitingDecision"
  test "$changed_count" -eq 2
  grep -q '^PASS$' "$make_test_stdout"
  grep -q 'main.c' "$tree_diff_stdout"
  grep -q 'tests/test.sh' "$tree_diff_stdout"
  grep -q "Hello, ${expected_string}" "$worktree_path/main.c"
  grep -q "Hello, ${expected_string}" "$worktree_path/tests/test.sh"
  grep -q "assignment_id: $assignment_id" "$report_get_stdout"
  grep -q "work_unit_id: $workunit_id" "$report_get_stdout"
  grep -q "status: AwaitingDecision" "$assignment_get_stdout"
  grep -q "binding_state: Bound" "$tracked_after_stdout"
  grep -q "upstream_thread_id: $thread_id" "$tracked_after_stdout"
  grep -q "workspace_worktree_path: $worktree_path" "$tracked_after_stdout"
  grep -q "workspace_branch_name: $branch_name" "$tracked_after_stdout"

  printf '%s=%q\n' "${prefix}_assignment_id" "$assignment_id"
  printf '%s=%q\n' "${prefix}_thread_id" "$thread_id"
  printf '%s=%q\n' "${prefix}_report_parse_result" "$report_parse_result"
}

e2e_start_managed_daemon "$daemon_log"
cleanup() {
  e2e_stop_managed_daemon
}
trap cleanup EXIT

sleep 5

workstream_output="$(
  e2e_orcas workstreams create \
    --title "Live concurrent lanes" \
    --objective "Prove two tracked-thread worktree lanes can run concurrently without crossing identity or lineage" \
    --priority normal
)"
workstream_id="$(printf '%s\n' "$workstream_output" | awk -F': ' '/^workstream_id:/ {print $2; exit}')"

eval "$(setup_lane lane_a "$workstream_id" \
  "Lane A" \
  "Lane A: change the greeting to exactly 'Hello, Lane A!' by updating main.c and tests/test.sh only. Keep the change bounded to those two files and do not touch lane B." \
  "Lane A!")"

eval "$(setup_lane lane_b "$workstream_id" \
  "Lane B" \
  "Lane B: change the greeting to exactly 'Hello, Lane B!' by updating main.c and tests/test.sh only. Keep the change bounded to those two files and do not touch lane A." \
  "Lane B!")"

lane_a_tracked_before_stdout="$reports_dir/lane-a-tracked-thread-before.txt"
lane_b_tracked_before_stdout="$reports_dir/lane-b-tracked-thread-before.txt"
runtime_before_stdout="$reports_dir/workstream-runtime-before-live.txt"
e2e_orcas workunit thread get --tracked-thread "$lane_a_tracked_thread_id" >"$lane_a_tracked_before_stdout"
e2e_orcas workunit thread get --tracked-thread "$lane_b_tracked_thread_id" >"$lane_b_tracked_before_stdout"
e2e_capture_workstream_runtime "$workstream_id" "$runtime_before_stdout"
e2e_assert_workstream_runtime "$workstream_id" "$runtime_before_stdout"
e2e_assert_runtime_thread_count "$runtime_before_stdout" 0

lane_a_assignment_start_stdout="$reports_dir/lane_a-assignment-start.txt"
lane_b_assignment_start_stdout="$reports_dir/lane_b-assignment-start.txt"

timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" supervisor work assignments start \
  --workunit "$lane_a_workunit_id" \
  --worker live-concurrent-lanes-a \
  --worker-kind codex \
  --instructions "Lane A: update the tiny C fixture so make test passes with the exact greeting 'Hello, Lane A!'. Edit only main.c and tests/test.sh. Do not touch lane B or create backup files. Return a brief summary of the exact lane A edits." \
  --cwd "$lane_a_worktree_path" \
  >"$lane_a_assignment_start_stdout" 2>&1 &
lane_a_assignment_start_pid=$!

timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" supervisor work assignments start \
  --workunit "$lane_b_workunit_id" \
  --worker live-concurrent-lanes-b \
  --worker-kind codex \
  --instructions "Lane B: update the tiny C fixture so make test passes with the exact greeting 'Hello, Lane B!'. Edit only main.c and tests/test.sh. Do not touch lane A or create backup files. Return a brief summary of the exact lane B edits." \
  --cwd "$lane_b_worktree_path" \
  >"$lane_b_assignment_start_stdout" 2>&1 &
lane_b_assignment_start_pid=$!

e2e_wait_for_report_id "$lane_a_workunit_id" lane_a_report_id
e2e_wait_for_report_id "$lane_b_workunit_id" lane_b_report_id

eval "$(collect_lane_results lane_a)"
eval "$(collect_lane_results lane_b)"

runtime_after_stdout="$reports_dir/workstream-runtime-after-live.txt"
threads_after_stdout="$reports_dir/workstream-threads-after-live.txt"
lane_a_decision_stdout="$reports_dir/lane-a-decision-complete.txt"
lane_b_decision_stdout="$reports_dir/lane-b-decision-complete.txt"

e2e_capture_workstream_runtime "$workstream_id" "$runtime_after_stdout"
e2e_capture_workstream_threads "$workstream_id" "$threads_after_stdout"
e2e_assert_workstream_runtime "$workstream_id" "$runtime_after_stdout"
e2e_assert_runtime_thread_count "$runtime_after_stdout" 2
e2e_assert_managed_thread_count "$threads_after_stdout" 2

test "$lane_a_tracked_thread_id" != "$lane_b_tracked_thread_id"
test "$lane_a_worktree_path" != "$lane_b_worktree_path"
test "$lane_a_branch_name" != "$lane_b_branch_name"
test "$lane_a_workunit_id" != "$lane_b_workunit_id"
test "$lane_a_assignment_id" != "$lane_b_assignment_id"
test "$lane_a_report_id" != "$lane_b_report_id"
test "$lane_a_thread_id" != "$lane_b_thread_id"
! grep -q "Hello, Lane B!" "$lane_a_worktree_path/main.c"
! grep -q "Hello, Lane A!" "$lane_b_worktree_path/main.c"

timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" supervisor work decisions apply \
  --workunit "$lane_a_workunit_id" \
  --report "$lane_a_report_id" \
  --type mark-complete \
  --rationale "Close lane A after its bounded live worker turn landed cleanly." \
  >"$lane_a_decision_stdout" 2>&1
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" supervisor work decisions apply \
  --workunit "$lane_b_workunit_id" \
  --report "$lane_b_report_id" \
  --type mark-complete \
  --rationale "Close lane B after its bounded live worker turn landed cleanly." \
  >"$lane_b_decision_stdout" 2>&1

lane_a_decision_id="$(e2e_field_value decision_id "$lane_a_decision_stdout")"
lane_b_decision_id="$(e2e_field_value decision_id "$lane_b_decision_stdout")"
lane_a_workunit_status="$(e2e_field_value work_unit_status "$lane_a_decision_stdout")"
lane_b_workunit_status="$(e2e_field_value work_unit_status "$lane_b_decision_stdout")"
test -n "$lane_a_decision_id"
test -n "$lane_b_decision_id"
test "$lane_a_workunit_status" = "Completed"
test "$lane_b_workunit_status" = "Completed"
grep -q "decision_type: MarkComplete" "$lane_a_decision_stdout"
grep -q "decision_type: MarkComplete" "$lane_b_decision_stdout"
grep -q "work_unit_status: Completed" "$lane_a_decision_stdout"
grep -q "work_unit_status: Completed" "$lane_b_decision_stdout"

wait "$lane_a_assignment_start_pid" >/dev/null 2>&1 || true
wait "$lane_b_assignment_start_pid" >/dev/null 2>&1 || true

echo "PASS"
