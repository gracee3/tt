#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "$0")/../../lib" && pwd)/common.sh"
scenario_dir="$(cd "$(dirname "$0")" && pwd)"

e2e_load_scenario_metadata "$scenario_dir"
e2e_prepare_scenario_dirs "$NAME"
e2e_prepare_live_codex_environment "gwt" 6000

scenario_name="git-worktrees"
base_ref="${ORCAS_E2E_GIT_BASE_REF:-main}"
artifacts_dir="$E2E_SCENARIO_ARTIFACTS_DIR"
reports_dir="$E2E_SCENARIO_REPORTS_DIR"
worktree_path="$E2E_SCENARIO_WORKTREES_DIR/lane"
repo_root="$E2E_SCENARIO_WORKTREES_DIR/lane-repo"
branch_suffix="${E2E_RUN_ID//[^a-zA-Z0-9]/-}"
branch_name="orcas/$scenario_name/$branch_suffix"
daemon_log="$E2E_SCENARIO_LOGS_DIR/orcasd.log"
prompt_dir="$artifacts_dir/prompts"

mkdir -p "$artifacts_dir" "$reports_dir" "$prompt_dir" "$(dirname "$worktree_path")"

cat >"$prompt_dir/operator.txt" <<'EOF'
Open a tracked-thread worktree lane, keep the lifecycle bounded, and verify the resulting workspace state matches what Orcas reports.
EOF

cat >"$prompt_dir/supervisor.txt" <<'EOF'
Keep the work on the declared tracked-thread worktree lane only. First create the tiny project in that lane, then use explicit lifecycle steps to prepare, land, and prune it.
EOF

cat >"$prompt_dir/agent.txt" <<'EOF'
Use the declared tracked-thread workspace path as the source of truth. Create a tiny C program and Makefile there, keep the project buildable, and leave lifecycle actions to later Orcas workspace operations.
EOF

e2e_prepare_empty_repo_with_worktree "$repo_root" "$worktree_path" "$branch_name" "$base_ref" "$reports_dir" "lane"

e2e_start_managed_daemon "$daemon_log"
cleanup() {
  e2e_stop_managed_daemon
}
trap cleanup EXIT

sleep 5

workstream_output="$(
  e2e_orcas workstreams create \
    --title "Git worktrees live lifecycle" \
    --objective "Validate tracked-thread worktree materialization, runtime binding, and lifecycle cleanup on a real live lane" \
    --priority normal
)"
workstream_id="$(printf '%s\n' "$workstream_output" | awk -F': ' '/^workstream_id:/ {print $2; exit}')"

workunit_output="$(
  e2e_orcas workunit create \
    --workstream "$workstream_id" \
    --title "Tracked thread worktree lifecycle lane" \
    --task "Create a tiny C project inside the declared tracked-thread worktree lane, keep it buildable, and leave landing and cleanup to later tracked-thread workspace operations." \
)"
workunit_id="$(printf '%s\n' "$workunit_output" | awk -F': ' '/^work_unit_id:/ {print $2; exit}')"

tracked_output="$(
  e2e_add_tracked_thread_workspace \
    "$workunit_id" \
    "Git worktree live lane" \
    "$repo_root" \
    "Dedicated tracked-thread worktree lane for the live git-worktrees lifecycle scenario" \
    "$repo_root" \
    "$worktree_path" \
    "$branch_name" \
    "$base_ref" \
    "$(git -C "$repo_root" rev-parse HEAD)" \
    "$base_ref" \
    rebase-before-completion \
    prune-after-merge \
    ready
)"
tracked_thread_id="$(printf '%s\n' "$tracked_output" | awk -F': ' '/^tracked_thread_id:/ {print $2; exit}')"

tracked_before_stdout="$reports_dir/tracked-thread-before-live.txt"
runtime_before_stdout="$reports_dir/workstream-runtime-before-live.txt"
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$tracked_before_stdout"
e2e_capture_workstream_runtime "$workstream_id" "$runtime_before_stdout"
e2e_assert_workstream_runtime "$workstream_id" "$runtime_before_stdout"
e2e_assert_runtime_thread_count "$runtime_before_stdout" 0

bootstrap_assignment_stdout="$reports_dir/bootstrap-assignment-start.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" supervisor work assignments start \
  --workunit "$workunit_id" \
  --worker git-worktrees-worker \
  --worker-kind codex \
  --instructions "Use the declared tracked-thread worktree lane only. Create a tiny C project by adding main.c and a Makefile in the declared worktree. main.c should print a short greeting. The Makefile must support make test by compiling and running the binary. Clean build artifacts before finishing, stage only README.md if it changed plus main.c and Makefile if needed for later lifecycle steps, and do not create any other source files. Do not run landing, merge, prune, or cleanup workflows." \
  --cwd "$worktree_path" \
  >"$bootstrap_assignment_stdout" 2>&1 &
bootstrap_assignment_start_pid=$!

e2e_wait_for_report_id "$workunit_id" bootstrap_report_id

bootstrap_report_get_stdout="$reports_dir/bootstrap-report-get.txt"
bootstrap_assignment_get_stdout="$reports_dir/bootstrap-assignment-get.txt"
bootstrap_make_test_stdout="$reports_dir/bootstrap-make-test.txt"
bootstrap_git_status_stdout="$reports_dir/bootstrap-git-status.txt"
bootstrap_tracked_stdout="$reports_dir/tracked-thread-after-bootstrap.txt"
runtime_after_bootstrap_stdout="$reports_dir/workstream-runtime-after-bootstrap.txt"
threads_after_bootstrap_stdout="$reports_dir/workstream-threads-after-bootstrap.txt"

e2e_orcas supervisor work reports get --report "$bootstrap_report_id" >"$bootstrap_report_get_stdout"
bootstrap_assignment_id="$(e2e_field_value assignment_id "$bootstrap_report_get_stdout")"
bootstrap_report_parse_result="$(e2e_field_value parse_result "$bootstrap_report_get_stdout")"

e2e_orcas supervisor work assignments get --assignment "$bootstrap_assignment_id" >"$bootstrap_assignment_get_stdout"
bootstrap_assignment_status="$(e2e_field_value status "$bootstrap_assignment_get_stdout")"
bootstrap_thread_id="$(e2e_field_value thread_id "$bootstrap_assignment_stdout")"

make -C "$worktree_path" test >"$bootstrap_make_test_stdout"
make -C "$worktree_path" clean >/dev/null 2>&1 || true
git -C "$worktree_path" status --short >"$bootstrap_git_status_stdout"
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$bootstrap_tracked_stdout"
e2e_capture_workstream_runtime "$workstream_id" "$runtime_after_bootstrap_stdout"
e2e_capture_workstream_threads "$workstream_id" "$threads_after_bootstrap_stdout"
e2e_assert_workstream_runtime "$workstream_id" "$runtime_after_bootstrap_stdout"
e2e_assert_runtime_thread_count "$runtime_after_bootstrap_stdout" 1
e2e_assert_managed_thread_count "$threads_after_bootstrap_stdout" 1

test -n "$bootstrap_assignment_id"
test -n "$bootstrap_thread_id"
test "$bootstrap_assignment_status" = "AwaitingDecision"
test "$bootstrap_report_parse_result" != "Invalid"
test -f "$prompt_dir/operator.txt"
test -f "$prompt_dir/supervisor.txt"
test -f "$prompt_dir/agent.txt"
test -f "$worktree_path/main.c"
test -f "$worktree_path/Makefile"
grep -q '^PASS$' "$bootstrap_make_test_stdout"
grep -q "binding_state: Bound" "$bootstrap_tracked_stdout"
grep -q "upstream_thread_id: $bootstrap_thread_id" "$bootstrap_tracked_stdout"
grep -q "workspace_worktree_path: $worktree_path" "$bootstrap_tracked_stdout"
grep -q "workspace_branch_name: $branch_name" "$bootstrap_tracked_stdout"
grep -q '^A  Makefile$\|^\?\? Makefile$' "$bootstrap_git_status_stdout"
grep -q '^A  main.c$\|^\?\? main.c$' "$bootstrap_git_status_stdout"

wait "$bootstrap_assignment_start_pid" >/dev/null 2>&1 || true

continue_after_bootstrap_stdout="$reports_dir/decision-continue-after-bootstrap.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" supervisor work decisions apply \
  --workunit "$workunit_id" \
  --report "$bootstrap_report_id" \
  --type continue \
  --rationale "Open the git-worktrees lane for tracked-thread workspace lifecycle validation after the project was created cleanly." \
  --instructions "Proceed with the next bounded tracked-thread workspace lifecycle step for the declared worktree lane." \
  >"$continue_after_bootstrap_stdout" 2>&1

test -n "$(e2e_field_value decision_id "$continue_after_bootstrap_stdout")"
test "$(e2e_field_value work_unit_status "$continue_after_bootstrap_stdout")" = "Ready"

prepare_workspace_stdout="$reports_dir/prepare-workspace.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" workunit workspace prepare-workspace \
  --tracked-thread "$tracked_thread_id" \
  --request-note "Confirm the created project is present, bounded, and ready for lifecycle validation. Do not change code." \
  >"$prepare_workspace_stdout" 2>&1 || true

prepare_report_id="$(e2e_field_value workspace_operation_report_id "$prepare_workspace_stdout")"
prepare_assignment_id="$(e2e_field_value workspace_operation_assignment_id "$prepare_workspace_stdout")"
test -n "$prepare_report_id"
test -n "$prepare_assignment_id"
e2e_orcas supervisor work reports get --report "$prepare_report_id" >"$reports_dir/prepare-report-get.txt"
e2e_orcas supervisor work assignments get --assignment "$prepare_assignment_id" >"$reports_dir/prepare-assignment-get.txt"
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$reports_dir/tracked-thread-after-prepare.txt"
grep -Eq "workspace_operation_status: (Completed|Failed)" "$prepare_workspace_stdout"

continue_after_prepare_stdout="$reports_dir/decision-continue-after-prepare.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" supervisor work decisions apply \
  --workunit "$workunit_id" \
  --report "$prepare_report_id" \
  --type continue \
  --rationale "Refresh the tracked-thread lane before merge preparation." \
  --instructions "Proceed with the next bounded tracked-thread workspace lifecycle step for the declared worktree lane." \
  >"$continue_after_prepare_stdout" 2>&1

test -n "$(e2e_field_value decision_id "$continue_after_prepare_stdout")"
test "$(e2e_field_value work_unit_status "$continue_after_prepare_stdout")" = "Ready"

refresh_workspace_stdout="$reports_dir/refresh-workspace.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" workunit workspace refresh-workspace \
  --tracked-thread "$tracked_thread_id" \
  --request-note "Refresh the tracked-thread workspace state after prepare and report the current observed repo state. Do not change code." \
  >"$refresh_workspace_stdout" 2>&1 || true

refresh_report_id="$(e2e_field_value workspace_operation_report_id "$refresh_workspace_stdout")"
refresh_assignment_id="$(e2e_field_value workspace_operation_assignment_id "$refresh_workspace_stdout")"
test -n "$refresh_report_id"
test -n "$refresh_assignment_id"
e2e_orcas supervisor work reports get --report "$refresh_report_id" >"$reports_dir/refresh-report-get.txt"
e2e_orcas supervisor work assignments get --assignment "$refresh_assignment_id" >"$reports_dir/refresh-assignment-get.txt"
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$reports_dir/tracked-thread-after-refresh.txt"
grep -Eq "workspace_operation_status: (Completed|Failed)" "$refresh_workspace_stdout"

continue_after_refresh_stdout="$reports_dir/decision-continue-after-refresh.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" supervisor work decisions apply \
  --workunit "$workunit_id" \
  --report "$refresh_report_id" \
  --type continue \
  --rationale "Move the tracked-thread worktree lane into merge preparation." \
  --instructions "Proceed with the next bounded tracked-thread workspace lifecycle step for merge preparation." \
  >"$continue_after_refresh_stdout" 2>&1

test -n "$(e2e_field_value decision_id "$continue_after_refresh_stdout")"
test "$(e2e_field_value work_unit_status "$continue_after_refresh_stdout")" = "Ready"

merge_prep_stdout="$reports_dir/merge-prep.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" workunit workspace merge-prep \
  --tracked-thread "$tracked_thread_id" \
  --request-note "Prepare the tracked-thread worktree lane for landing review without making unrelated changes." \
  >"$merge_prep_stdout" 2>&1 || true

grep -Eq "workspace_operation_status: (Completed|Failed)" "$merge_prep_stdout"
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$reports_dir/tracked-thread-after-merge-prep.txt"

authorize_merge_stdout="$reports_dir/authorize-merge.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" workunit workspace authorize-merge \
  --tracked-thread "$tracked_thread_id" \
  --request-note "Authorize landing for the prepared git-worktrees lane." \
  >"$authorize_merge_stdout" 2>&1 || true

grep -Eq "landing_authorization_status: (Authorized|Rejected)" "$authorize_merge_stdout"
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$reports_dir/tracked-thread-after-authorize.txt"

continue_after_authorize_stdout="$reports_dir/decision-continue-after-authorize.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" supervisor work decisions apply \
  --workunit "$workunit_id" \
  --report "$refresh_report_id" \
  --type continue \
  --rationale "Execute landing for the authorized git-worktrees lane." \
  --instructions "Proceed with the next bounded tracked-thread workspace step for landing execution." \
  >"$continue_after_authorize_stdout" 2>&1

test -n "$(e2e_field_value decision_id "$continue_after_authorize_stdout")"
test "$(e2e_field_value work_unit_status "$continue_after_authorize_stdout")" = "Ready"

execute_landing_stdout="$reports_dir/execute-landing.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" workunit workspace execute-landing \
  --tracked-thread "$tracked_thread_id" \
  --request-note "Execute the authorized landing for the git-worktrees tracked-thread lane." \
  >"$execute_landing_stdout" 2>&1 || true

grep -Eq "landing_execution_status: (Completed|Failed)" "$execute_landing_stdout"
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$reports_dir/tracked-thread-after-landing.txt"

continue_after_landing_stdout="$reports_dir/decision-continue-after-landing.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" supervisor work decisions apply \
  --workunit "$workunit_id" \
  --report "$refresh_report_id" \
  --type continue \
  --rationale "Prune the tracked-thread workspace after landing execution." \
  --instructions "Proceed with the final bounded tracked-thread workspace cleanup step." \
  >"$continue_after_landing_stdout" 2>&1

test -n "$(e2e_field_value decision_id "$continue_after_landing_stdout")"
test "$(e2e_field_value work_unit_status "$continue_after_landing_stdout")" = "Ready"

prune_workspace_stdout="$reports_dir/prune-workspace.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" workunit workspace prune-workspace \
  --tracked-thread "$tracked_thread_id" \
  --request-note "Prune the tracked-thread workspace after landing and report the observed cleanup outcome." \
  >"$prune_workspace_stdout" 2>&1 || true

grep -Eq "workspace_operation_status: (Completed|Failed)" "$prune_workspace_stdout"
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$reports_dir/tracked-thread-after-prune.txt"
e2e_orcas workunit get --workunit "$workunit_id" >"$reports_dir/workunit-before-complete.txt"

complete_stdout="$reports_dir/decision-complete.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" supervisor work decisions apply \
  --workunit "$workunit_id" \
  --report "$refresh_report_id" \
  --type mark-complete \
  --rationale "Close the git-worktrees lifecycle lane after the tracked-thread workspace operations completed." \
  >"$complete_stdout" 2>&1

test -n "$(e2e_field_value decision_id "$complete_stdout")"
test "$(e2e_field_value work_unit_status "$complete_stdout")" = "Completed"
grep -q "tracked_threads: 1" "$reports_dir/workunit-before-complete.txt"
grep -q "$tracked_thread_id" "$reports_dir/workunit-before-complete.txt"

echo "PASS"
