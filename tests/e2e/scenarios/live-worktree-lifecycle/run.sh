#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "$0")/../../lib" && pwd)/common.sh"
scenario_dir="$(cd "$(dirname "$0")" && pwd)"
fixture_dir="$scenario_dir/fixture"

e2e_load_scenario_metadata "$scenario_dir"
e2e_prepare_scenario_dirs "$NAME"

capture_bootstrap_churn() {
  local output_file="$1"
  {
    echo "git_status:"
    git -C "$worktree_path" status --short --untracked-files=all
    echo
    echo "temp_files:"
    find "$worktree_path" -maxdepth 1 -type f \
      \( -name '*.bak' -o -name '*~' -o -name '*.tmp' -o -name '*.swp' -o -name '*.swo' -o -name '.#*' \) \
      -printf '%f\n' | sort
  } >"$output_file"
}

fail_bootstrap_churn() {
  local reason="$1"
  echo "bootstrap boundedness failure: $reason" >&2
  echo "bootstrap assignment output:" >&2
  sed -n '1,220p' "$bootstrap_assignment_stdout" >&2 || true
  echo "bootstrap report:" >&2
  sed -n '1,220p' "$bootstrap_report_get_stdout" >&2 || true
  echo "bootstrap churn snapshot:" >&2
  sed -n '1,220p' "$bootstrap_churn_stdout" >&2 || true
  exit 1
}

e2e_prepare_live_codex_environment "lwl" 5800 16384

worktree_path="$E2E_SCENARIO_WORKTREES_DIR/lane"
repo_root="$E2E_SCENARIO_WORKTREES_DIR/lane-repo"
daemon_log="$E2E_SCENARIO_LOGS_DIR/orcasd.log"
reports_dir="$E2E_SCENARIO_REPORTS_DIR"
artifacts_dir="$E2E_SCENARIO_ARTIFACTS_DIR"
base_ref="${ORCAS_E2E_GIT_BASE_REF:-main}"
branch_suffix="${E2E_RUN_ID//[^a-zA-Z0-9]/-}"
branch_name="orcas/$NAME/$branch_suffix"

mkdir -p "$reports_dir" "$artifacts_dir" "$(dirname "$worktree_path")"
e2e_prepare_fixture_repo_with_worktree "$fixture_dir" "$repo_root" "$worktree_path" "$branch_name" "$base_ref" "$reports_dir"

e2e_start_managed_daemon "$daemon_log"
cleanup() {
  e2e_stop_managed_daemon
}
trap cleanup EXIT

sleep 5

workstream_output="$(
  e2e_orcas workstreams create \
    --title "Live worktree lifecycle" \
    --objective "Prove tracked-thread worktree landing and cleanup on a real live lane" \
    --priority normal
)"
workstream_id="$(printf '%s\n' "$workstream_output" | awk -F': ' '/^workstream_id:/ {print $2; exit}')"

workunit_output="$(
  e2e_orcas workunit create \
    --workstream "$workstream_id" \
    --title "Tracked thread worktree lifecycle lane" \
    --task "Inspect the tiny C program in the tracked-thread worktree. Make the smallest code change needed so make test passes. Edit main.c in place or with apply_patch only. Do not create backup files, temporary files, rename-based edits, editor swap files, or unrelated changes. Do not run cleanup commands, and do not perform landing, merge-prep, or prune steps in this assignment; leave those lifecycle actions for later tracked-thread operations." \
)"
workunit_id="$(printf '%s\n' "$workunit_output" | awk -F': ' '/^work_unit_id:/ {print $2; exit}')"

tracked_output="$(
  e2e_add_tracked_thread_workspace \
    "$workunit_id" \
    "Tracked thread worktree lifecycle" \
    "$repo_root" \
    "Dedicated tracked-thread worktree lane for live lifecycle validation" \
    "$repo_root" \
    "$worktree_path" \
    "$branch_name" \
    "$base_ref" \
    "$(git -C "$repo_root" rev-parse HEAD)" \
    "$base_ref" \
    rebase-before-completion \
    prune-after-merge \
    ready \
)"
tracked_thread_id="$(printf '%s\n' "$tracked_output" | awk -F': ' '/^tracked_thread_id:/ {print $2; exit}')"

tracked_before_stdout="$reports_dir/tracked-thread-before-live.txt"
runtime_before_stdout="$reports_dir/workstream-runtime-before-live.txt"
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$tracked_before_stdout"
e2e_capture_workstream_runtime "$workstream_id" "$runtime_before_stdout"
e2e_assert_workstream_runtime "$workstream_id" "$runtime_before_stdout"
e2e_assert_runtime_thread_count "$runtime_before_stdout" 0

bootstrap_assignment_stdout="$reports_dir/bootstrap-assignment-start.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" assignments start \
  --workunit "$workunit_id" \
  --worker live-worktree-lifecycle-worker \
  --worker-kind codex \
  --instructions "Inspect the tiny C program and shell test in the declared tracked-thread worktree. Make the smallest possible code change in main.c to make make test pass. Edit main.c in place or with apply_patch only. Do not create backup files, temporary files, rename-based edits, editor swap files, or unrelated code changes. Do not run cleanup commands of any kind. Do not land, merge, or prune the worktree. Leave the worktree clean and leave lifecycle actions for later tracked-thread operations." \
  --cwd "$worktree_path" \
  >"$bootstrap_assignment_stdout" 2>&1 &
bootstrap_assignment_start_pid=$!

e2e_wait_for_report_id "$workunit_id" bootstrap_report_id

bootstrap_report_get_stdout="$reports_dir/bootstrap-report-get.txt"
bootstrap_assignment_get_stdout="$reports_dir/bootstrap-assignment-get.txt"
bootstrap_make_test_stdout="$reports_dir/bootstrap-make-test.txt"
bootstrap_git_status_stdout="$reports_dir/bootstrap-git-status.txt"
bootstrap_churn_stdout="$reports_dir/bootstrap-churn.txt"

e2e_orcas supervisor work reports get --report "$bootstrap_report_id" >"$bootstrap_report_get_stdout"
bootstrap_assignment_id="$(e2e_field_value assignment_id "$bootstrap_report_get_stdout")"
bootstrap_report_parse_result="$(e2e_field_value parse_result "$bootstrap_report_get_stdout")"

e2e_orcas supervisor work assignments get --assignment "$bootstrap_assignment_id" >"$bootstrap_assignment_get_stdout"
bootstrap_assignment_status="$(e2e_field_value status "$bootstrap_assignment_get_stdout")"

case "$bootstrap_report_parse_result" in
  Parsed|Ambiguous) ;;
  *)
    fail_bootstrap_churn "bootstrap report parse_result=$bootstrap_report_parse_result"
    ;;
esac

make -C "$worktree_path" test >"$bootstrap_make_test_stdout"
make -C "$worktree_path" clean >/dev/null 2>&1 || true
git -C "$worktree_path" status --short >"$bootstrap_git_status_stdout"
capture_bootstrap_churn "$bootstrap_churn_stdout"
bootstrap_temp_files="$(awk '
  /^temp_files:$/ { in_temp=1; next }
  /^git_status:$/ { in_temp=0 }
  in_temp && NF { print }
' "$bootstrap_churn_stdout")"
bootstrap_git_status_after_clean="$(awk '
  /^git_status:$/ { in_git=1; next }
  /^temp_files:$/ { in_git=0 }
  in_git && NF { print }
' "$bootstrap_churn_stdout")"

test -z "$bootstrap_temp_files" || fail_bootstrap_churn "unexpected temp files detected: $bootstrap_temp_files"
test -z "$(printf '%s\n' "$bootstrap_git_status_after_clean" | sed '/^ M main.c$/d')" || fail_bootstrap_churn "unexpected git status entries detected"

test -n "$bootstrap_assignment_id"
test "$bootstrap_assignment_status" = "AwaitingDecision"
grep -q '^PASS$' "$bootstrap_make_test_stdout"
test "$(wc -l <"$bootstrap_git_status_stdout")" -eq 1
grep -qx ' M main.c' "$bootstrap_git_status_stdout"
grep -q 'Hello, Orcas!' "$worktree_path/main.c"
grep -q "assignment_id: $bootstrap_assignment_id" "$bootstrap_report_get_stdout"
grep -q "report_id: $bootstrap_report_id" "$bootstrap_assignment_get_stdout"
wait "$bootstrap_assignment_start_pid" >/dev/null 2>&1 || true

bootstrap_thread_id="$(e2e_field_value thread_id "$bootstrap_assignment_stdout")"
test -n "$bootstrap_thread_id"
tracked_thread_bound_stdout="$reports_dir/tracked-thread-bound.txt"
runtime_after_bootstrap_stdout="$reports_dir/workstream-runtime-after-bootstrap.txt"
threads_after_bootstrap_stdout="$reports_dir/workstream-threads-after-bootstrap.txt"
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$tracked_thread_bound_stdout"
e2e_capture_workstream_runtime "$workstream_id" "$runtime_after_bootstrap_stdout"
e2e_capture_workstream_threads "$workstream_id" "$threads_after_bootstrap_stdout"
e2e_assert_workstream_runtime "$workstream_id" "$runtime_after_bootstrap_stdout"
e2e_assert_runtime_thread_count "$runtime_after_bootstrap_stdout" 1
e2e_assert_managed_thread_count "$threads_after_bootstrap_stdout" 1
grep -q "binding_state: Bound" "$tracked_thread_bound_stdout"
grep -q "upstream_thread_id: $bootstrap_thread_id" "$tracked_thread_bound_stdout"
grep -q "workspace_worktree_path: $worktree_path" "$tracked_thread_bound_stdout"
grep -q "workspace_branch_name: $branch_name" "$tracked_thread_bound_stdout"

bootstrap_continue_stdout="$reports_dir/decision-continue-after-bootstrap.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" decisions apply \
  --workunit "$workunit_id" \
  --report "$bootstrap_report_id" \
  --type continue \
  --rationale "Open the tracked-thread lifecycle lane for the declared workspace after the bootstrap fix." \
  --instructions "Proceed with the next bounded tracked-thread workspace step for the declared worktree lane." \
  >"$bootstrap_continue_stdout" 2>&1

bootstrap_continue_decision_id="$(e2e_field_value decision_id "$bootstrap_continue_stdout")"
bootstrap_continue_next_assignment_id="$(e2e_field_value next_assignment_id "$bootstrap_continue_stdout")"
bootstrap_continue_workunit_status="$(e2e_field_value work_unit_status "$bootstrap_continue_stdout")"
test -n "$bootstrap_continue_decision_id"
test -n "$bootstrap_continue_next_assignment_id"
test "$bootstrap_continue_workunit_status" = "Ready"
grep -q "decision_type: Continue" "$bootstrap_continue_stdout"
grep -q "work_unit_status: Ready" "$bootstrap_continue_stdout"

prepare_workspace_stdout="$reports_dir/prepare-workspace.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" workunit workspace prepare \
  --tracked-thread "$tracked_thread_id" \
  --request-note "Confirm the tracked-thread worktree is clean after the bounded fix and report the current workspace state. Do not make additional code changes." \
  >"$prepare_workspace_stdout" 2>&1 || true

prepare_report_id="$(e2e_field_value workspace_operation_report_id "$prepare_workspace_stdout")"
prepare_assignment_id="$(e2e_field_value workspace_operation_assignment_id "$prepare_workspace_stdout")"
prepare_workspace_operation_status="$(e2e_field_value workspace_operation_status "$prepare_workspace_stdout")"

prepare_report_get_stdout="$reports_dir/prepare-report-get.txt"
prepare_assignment_get_stdout="$reports_dir/prepare-assignment-get.txt"
prepare_thread_get_stdout="$reports_dir/tracked-thread-after-prepare.txt"
prepare_make_test_stdout="$reports_dir/prepare-make-test.txt"
prepare_git_status_stdout="$reports_dir/prepare-git-status.txt"
prepare_git_log_stdout="$reports_dir/prepare-git-log.txt"

e2e_orcas supervisor work reports get --report "$prepare_report_id" >"$prepare_report_get_stdout"
prepare_report_assignment_id="$(e2e_field_value assignment_id "$prepare_report_get_stdout")"
prepare_report_parse_result="$(e2e_field_value parse_result "$prepare_report_get_stdout")"

e2e_orcas supervisor work assignments get --assignment "$prepare_assignment_id" >"$prepare_assignment_get_stdout"
prepare_assignment_status="$(e2e_field_value status "$prepare_assignment_get_stdout")"

make -C "$worktree_path" test >"$prepare_make_test_stdout"
make -C "$worktree_path" clean >/dev/null 2>&1 || true
git -C "$worktree_path" status --short >"$prepare_git_status_stdout"
git -C "$worktree_path" log -1 --oneline >"$prepare_git_log_stdout"
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$prepare_thread_get_stdout"

test -n "$prepare_report_id"
test -n "$prepare_assignment_id"
test -n "$prepare_report_assignment_id"
test -n "$prepare_report_parse_result"
test "$prepare_report_assignment_id" = "$prepare_assignment_id"
case "$prepare_workspace_operation_status" in
  Completed|Failed) ;;
  *)
    echo "unexpected prepare workspace status: $prepare_workspace_operation_status" >&2
    exit 1
    ;;
esac
test "$prepare_assignment_status" = "AwaitingDecision"
grep -q '^PASS$' "$prepare_make_test_stdout"
if [[ -s "$prepare_git_status_stdout" ]]; then
  test "$(wc -l <"$prepare_git_status_stdout")" -eq 1
  grep -qx ' M main.c' "$prepare_git_status_stdout"
fi
grep -q 'Hello, Orcas!' "$worktree_path/main.c"
grep -q "assignment_id: $prepare_assignment_id" "$prepare_report_get_stdout"
grep -q "report_id: $prepare_report_id" "$prepare_assignment_get_stdout"
grep -q "workspace_operation_report_id: $prepare_report_id" "$prepare_workspace_stdout"
grep -Eq "workspace_operation_status: (Completed|Failed)" "$prepare_workspace_stdout"
grep -Eq "parse_result: (Parsed|Ambiguous|Invalid)" "$prepare_report_get_stdout"

continue_after_prepare_stdout="$reports_dir/decision-continue-after-prepare.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" decisions apply \
  --workunit "$workunit_id" \
  --report "$prepare_report_id" \
  --type continue \
  --rationale "Open the lifecycle lane for merge-prep after the clean workspace preparation report." \
  --instructions "Proceed with the next bounded tracked-thread workspace step for the declared worktree lane." \
  >"$continue_after_prepare_stdout" 2>&1

continue_after_prepare_decision_id="$(e2e_field_value decision_id "$continue_after_prepare_stdout")"
continue_after_prepare_next_assignment_id="$(e2e_field_value next_assignment_id "$continue_after_prepare_stdout")"
continue_after_prepare_workunit_status="$(e2e_field_value work_unit_status "$continue_after_prepare_stdout")"
test -n "$continue_after_prepare_decision_id"
test -n "$continue_after_prepare_next_assignment_id"
test "$continue_after_prepare_workunit_status" = "Ready"
grep -q "decision_type: Continue" "$continue_after_prepare_stdout"
grep -q "work_unit_status: Ready" "$continue_after_prepare_stdout"

merge_prep_stdout="$reports_dir/merge-prep.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" workunit workspace merge-prep \
  --tracked-thread "$tracked_thread_id" \
  --request-note "Confirm the tracked-thread worktree is clean, bounded, and ready for landing review. Do not make any additional code changes." \
  >"$merge_prep_stdout" 2>&1 || true

merge_prep_report_id="$(e2e_field_value workspace_operation_report_id "$merge_prep_stdout")"
merge_prep_assignment_id="$(e2e_field_value workspace_operation_assignment_id "$merge_prep_stdout")"
merge_prep_readiness="$(e2e_field_value merge_prep_readiness "$merge_prep_stdout")"
merge_prep_report_get_stdout="$reports_dir/merge-prep-report-get.txt"
merge_prep_assignment_get_stdout="$reports_dir/merge-prep-assignment-get.txt"
merge_prep_thread_get_stdout="$reports_dir/tracked-thread-after-merge-prep.txt"

e2e_orcas supervisor work reports get --report "$merge_prep_report_id" >"$merge_prep_report_get_stdout"
e2e_orcas supervisor work assignments get --assignment "$merge_prep_assignment_id" >"$merge_prep_assignment_get_stdout"
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$merge_prep_thread_get_stdout"

merge_prep_report_assignment_id="$(e2e_field_value assignment_id "$merge_prep_report_get_stdout")"
merge_prep_report_parse_result="$(e2e_field_value parse_result "$merge_prep_report_get_stdout")"
merge_prep_assignment_status="$(e2e_field_value status "$merge_prep_assignment_get_stdout")"

test -n "$merge_prep_report_id"
test -n "$merge_prep_assignment_id"
test -n "$merge_prep_report_assignment_id"
test -n "$merge_prep_report_parse_result"
test "$merge_prep_report_assignment_id" = "$merge_prep_assignment_id"
case "$merge_prep_readiness" in
  Ready|Unknown) ;;
  *)
    echo "unexpected merge prep readiness: $merge_prep_readiness" >&2
    exit 1
    ;;
esac
test "$merge_prep_assignment_status" = "AwaitingDecision"
grep -Eq "merge_prep_readiness: (Ready|Unknown)" "$merge_prep_stdout"
grep -q "workspace_local_dirty: false" "$merge_prep_thread_get_stdout"
grep -Eq "parse_result: (Parsed|Ambiguous|Invalid)" "$merge_prep_report_get_stdout"

authorize_stdout="$reports_dir/authorize-merge.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" workunit workspace authorize-merge \
  --tracked-thread "$tracked_thread_id" \
  --request-note "Authorize landing for the already prepared tracked-thread worktree lane." \
  >"$authorize_stdout" 2>&1 || true

landing_authorization_id="$(e2e_field_value landing_authorization_id "$authorize_stdout")"
landing_authorization_status="$(e2e_field_value landing_authorization_status "$authorize_stdout")"
landing_authorization_is_current="$(e2e_field_value landing_authorization_is_current "$authorize_stdout")"
authorize_thread_get_stdout="$reports_dir/tracked-thread-after-authorize.txt"
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$authorize_thread_get_stdout"

test -n "$landing_authorization_id"
test "$landing_authorization_status" = "Authorized"
test "$landing_authorization_is_current" = "true"
grep -q "landing_authorization_status: Authorized" "$authorize_stdout"
grep -q "landing_authorization_is_current: true" "$authorize_stdout"

continue_after_merge_prep_stdout="$reports_dir/decision-continue-after-merge-prep.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" decisions apply \
  --workunit "$workunit_id" \
  --report "$merge_prep_report_id" \
  --type continue \
  --rationale "Open the lifecycle lane for landing execution after merge-prep is ready." \
  --instructions "Proceed with the next bounded tracked-thread workspace step for landing execution." \
  >"$continue_after_merge_prep_stdout" 2>&1

continue_after_merge_prep_decision_id="$(e2e_field_value decision_id "$continue_after_merge_prep_stdout")"
continue_after_merge_prep_next_assignment_id="$(e2e_field_value next_assignment_id "$continue_after_merge_prep_stdout")"
continue_after_merge_prep_workunit_status="$(e2e_field_value work_unit_status "$continue_after_merge_prep_stdout")"
test -n "$continue_after_merge_prep_decision_id"
test -n "$continue_after_merge_prep_next_assignment_id"
test "$continue_after_merge_prep_workunit_status" = "Ready"
grep -q "decision_type: Continue" "$continue_after_merge_prep_stdout"
grep -q "work_unit_status: Ready" "$continue_after_merge_prep_stdout"

landing_stdout="$reports_dir/execute-landing.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" workunit workspace execute-landing \
  --tracked-thread "$tracked_thread_id" \
  --request-note "Execute the authorized landing for the prepared tracked-thread worktree lane only." \
  >"$landing_stdout" 2>&1 || true

landing_execution_id="$(e2e_field_value landing_execution_id "$landing_stdout")"
landing_execution_status="$(e2e_field_value landing_execution_status "$landing_stdout")"
landing_execution_matches_basis="$(e2e_field_value landing_execution_matches_authorization_basis "$landing_stdout")"
landing_execution_result_status="$(e2e_field_value landing_execution_result_status "$landing_stdout")"
landing_execution_report_id="$(e2e_field_value landing_execution_report_id "$landing_stdout")"
landing_execution_thread_get_stdout="$reports_dir/tracked-thread-after-landing.txt"
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$landing_execution_thread_get_stdout"

test -n "$landing_execution_id"
test -n "$landing_execution_report_id"
test "$landing_execution_status" = "Completed"
test "$landing_execution_matches_basis" = "true"
test "$landing_execution_result_status" = "Succeeded"
grep -q "landing_execution_status: Completed" "$landing_stdout"
grep -q "landing_execution_matches_authorization_basis: true" "$landing_stdout"

continue_after_landing_stdout="$reports_dir/decision-continue-after-landing.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" decisions apply \
  --workunit "$workunit_id" \
  --report "$landing_execution_report_id" \
  --type continue \
  --rationale "Open the lifecycle lane for final prune after the landing completed cleanly." \
  --instructions "Proceed with the final bounded tracked-thread cleanup step for the declared worktree lane." \
  >"$continue_after_landing_stdout" 2>&1

continue_after_landing_decision_id="$(e2e_field_value decision_id "$continue_after_landing_stdout")"
continue_after_landing_next_assignment_id="$(e2e_field_value next_assignment_id "$continue_after_landing_stdout")"
continue_after_landing_workunit_status="$(e2e_field_value work_unit_status "$continue_after_landing_stdout")"
test -n "$continue_after_landing_decision_id"
test -n "$continue_after_landing_next_assignment_id"
test "$continue_after_landing_workunit_status" = "Ready"
grep -q "decision_type: Continue" "$continue_after_landing_stdout"
grep -q "work_unit_status: Ready" "$continue_after_landing_stdout"

prune_stdout="$reports_dir/prune-workspace.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" workunit workspace prune \
  --tracked-thread "$tracked_thread_id" \
  --request-note "Prune the tracked-thread workspace after the successful landing and report the observed cleanup state." \
  >"$prune_stdout" 2>&1 || true

prune_operation_id="$(e2e_field_value prune_workspace_operation_id "$prune_stdout")"
prune_workspace_result_status="$(e2e_field_value prune_workspace_result_status "$prune_stdout")"
prune_workspace_result_worktree_removed="$(e2e_field_value prune_workspace_result_worktree_removed "$prune_stdout")"
prune_workspace_result_branch_removed="$(e2e_field_value prune_workspace_result_branch_removed "$prune_stdout")"
prune_thread_get_stdout="$reports_dir/tracked-thread-after-prune.txt"
git_worktree_list_before_prune="$reports_dir/git-worktree-list-before-prune.txt"
git_worktree_list_after_prune="$reports_dir/git-worktree-list-after-prune.txt"

git -C "$repo_root" worktree list --porcelain >"$git_worktree_list_before_prune"
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$prune_thread_get_stdout"
git -C "$repo_root" worktree list --porcelain >"$git_worktree_list_after_prune"

test -n "$prune_operation_id"
test "$prune_workspace_result_status" = "Succeeded"
test "$prune_workspace_result_worktree_removed" = "true"
test ! -d "$worktree_path"
grep -q "prune_workspace_result_status: Succeeded" "$prune_stdout"
grep -q "prune_workspace_result_worktree_removed: true" "$prune_stdout"
if [[ -n "$prune_workspace_result_branch_removed" ]]; then
  grep -Eq "prune_workspace_result_branch_removed: (true|false)" "$prune_stdout"
fi
grep -q "workspace_status: Pruned" "$prune_thread_get_stdout"
grep -q "workspace_local_exists: false" "$prune_thread_get_stdout"

complete_after_prune_stdout="$reports_dir/decision-complete-after-prune.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" decisions apply \
  --workunit "$workunit_id" \
  --report "$prune_operation_id" \
  --type mark-complete \
  --rationale "Close the tracked-thread lifecycle work unit after prune completed cleanly." \
  >"$complete_after_prune_stdout" 2>&1

complete_after_prune_decision_id="$(e2e_field_value decision_id "$complete_after_prune_stdout")"
complete_after_prune_workunit_status="$(e2e_field_value work_unit_status "$complete_after_prune_stdout")"
test -n "$complete_after_prune_decision_id"
test "$complete_after_prune_workunit_status" = "Completed"
grep -q "decision_type: MarkComplete" "$complete_after_prune_stdout"
grep -q "work_unit_status: Completed" "$complete_after_prune_stdout"

echo "PASS"
