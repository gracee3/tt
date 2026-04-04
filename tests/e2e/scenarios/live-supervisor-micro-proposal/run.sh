#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "$0")/../../lib" && pwd)/common.sh"
scenario_dir="$(cd "$(dirname "$0")" && pwd)"
fixture_dir="$scenario_dir/fixture"

e2e_load_scenario_metadata "$scenario_dir"
e2e_prepare_scenario_dirs "$NAME"
e2e_prepare_live_codex_environment "lsmp" 5600 16384
e2e_require_local_supervisor_endpoint

worktree_path="$E2E_SCENARIO_WORKTREES_DIR/lane"
repo_root="$E2E_SCENARIO_WORKTREES_DIR/lane-repo"
daemon_log="$E2E_SCENARIO_LOGS_DIR/orcasd.log"
reports_dir="$E2E_SCENARIO_REPORTS_DIR"
artifacts_dir="$E2E_SCENARIO_ARTIFACTS_DIR"
base_ref="${ORCAS_E2E_GIT_BASE_REF:-main}"
branch_suffix="${E2E_RUN_ID//[^a-zA-Z0-9]/-}"
branch_name="orcas/$NAME/$branch_suffix"

mkdir -p "$reports_dir" "$artifacts_dir"
e2e_prepare_fixture_repo_with_worktree "$fixture_dir" "$repo_root" "$worktree_path" "$branch_name" "$base_ref" "$reports_dir" "lane"

e2e_start_managed_daemon "$daemon_log"
cleanup() {
  e2e_stop_managed_daemon
}
trap cleanup EXIT

sleep 5

workstream_output="$(
  e2e_orcas workstreams create \
    --title "Live supervisor micro proposal" \
    --objective "Prove the live worker-to-supervisor loop on one bounded tracked-thread lane" \
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
    "Live supervisor micro proposal lane" \
    "$repo_root" \
    "Dedicated tracked-thread worktree lane for the supervisor micro proposal scenario" \
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
  --worker live-supervisor-micro-proposal-worker \
  --worker-kind codex \
  --instructions "Inspect the tiny C program and failing shell test in the declared tracked-thread worktree lane. Make the smallest possible code change in main.c to make make test pass. Do not refactor unrelated code, do not touch the test script unless required, and keep the fix bounded to one file if possible." \
  --cwd "$worktree_path" \
  >"$assignment_stdout" 2>&1 &
assignment_start_pid=$!

e2e_wait_for_report_id "$workunit_id" report_id

assignment_get_stdout="$reports_dir/assignment-get.txt"
report_get_stdout="$reports_dir/report-get.txt"
proposal_create_stdout="$reports_dir/proposal-create.txt"
proposal_get_stdout="$reports_dir/proposal-get.txt"
proposal_summary_stdout="$reports_dir/proposal-artifact-summary.txt"
proposal_approve_stdout="$reports_dir/proposal-approve.txt"
next_assignment_get_stdout="$reports_dir/next-assignment-get.txt"
make_test_stdout="$reports_dir/make-test.txt"
git_status_stdout="$reports_dir/git-status.txt"
tree_diff_stdout="$reports_dir/tree-diff.txt"
tracked_after_stdout="$reports_dir/tracked-thread-after-live.txt"
runtime_after_stdout="$reports_dir/workstream-runtime-after-live.txt"
threads_after_stdout="$reports_dir/workstream-threads-after-live.txt"

e2e_orcas supervisor work reports get --report "$report_id" >"$report_get_stdout"
assignment_id="$(e2e_field_value assignment_id "$report_get_stdout")"
report_parse_result="$(e2e_field_value parse_result "$report_get_stdout")"

e2e_orcas supervisor work assignments get --assignment "$assignment_id" >"$assignment_get_stdout"
assignment_status="$(e2e_field_value status "$assignment_get_stdout")"
worker_session_id="$(e2e_field_value worker_session_id "$assignment_get_stdout")"
thread_id="$(e2e_field_value thread_id "$assignment_stdout")"

make -C "$worktree_path" test >"$make_test_stdout"
make -C "$worktree_path" clean >/dev/null 2>&1 || true
git -C "$worktree_path" status --short >"$git_status_stdout"
diff -qr --exclude=.git "$fixture_dir" "$worktree_path" >"$tree_diff_stdout" || true
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$tracked_after_stdout"
e2e_capture_workstream_runtime "$workstream_id" "$runtime_after_stdout"
e2e_capture_workstream_threads "$workstream_id" "$threads_after_stdout"
e2e_assert_workstream_runtime "$workstream_id" "$runtime_after_stdout"
e2e_assert_runtime_thread_count "$runtime_after_stdout" 1
e2e_assert_managed_thread_count "$threads_after_stdout" 1

test -n "$assignment_id"
test -n "$worker_session_id"
test -n "$thread_id"
test -n "$report_parse_result"
test "$assignment_status" = "AwaitingDecision"
test "$report_parse_result" != "Invalid"
test "$(sed '/^$/d' "$tree_diff_stdout" | wc -l | tr -d ' ')" -eq 1
grep -q '^PASS$' "$make_test_stdout"
grep -qx ' M main.c' "$git_status_stdout"
grep -q 'main.c' "$tree_diff_stdout"
grep -q "assignment_id: $assignment_id" "$assignment_get_stdout"
grep -q "report_id: $report_id" "$report_get_stdout"
grep -q "assignment_id: $assignment_id" "$report_get_stdout"
grep -q "work_unit_id: $workunit_id" "$report_get_stdout"
grep -q "status: AwaitingDecision" "$assignment_get_stdout"
grep -Eq "parse_result: (Parsed|Ambiguous)" "$report_get_stdout"
grep -q "binding_state: Bound" "$tracked_after_stdout"
grep -q "upstream_thread_id: $thread_id" "$tracked_after_stdout"
grep -q "workspace_worktree_path: $worktree_path" "$tracked_after_stdout"
grep -q "workspace_branch_name: $branch_name" "$tracked_after_stdout"

proposal_create_output="$(
  e2e_orcas supervisor work proposals create \
    --workunit "$workunit_id" \
    --report "$report_id" \
    --requested-by live-supervisor-micro-proposal \
    --note "Generate a bounded continue proposal for one tiny follow-up test on the greeting fix. Keep every field terse. Use exactly 2 instructions, exactly 2 acceptance criteria, exactly 2 stop conditions, exactly 2 expected report fields, and a concise boundedness note. Set plan_assessment and plan_revision_proposal to null. Do not escalate or mark the work complete." \
  | tee "$proposal_create_stdout"
)"
proposal_id="$(printf '%s\n' "$proposal_create_output" | awk -F': ' '/^proposal_id:/ {print $2; exit}')"

e2e_orcas supervisor work proposals get --proposal "$proposal_id" >"$proposal_get_stdout"
proposal_status="$(e2e_field_value status "$proposal_get_stdout")"
model_summary_headline="$(e2e_field_value model_summary_headline "$proposal_get_stdout")"
model_proposed_decision_type="$(e2e_field_value model_proposed_decision_type "$proposal_get_stdout")"
source_report_id="$(e2e_field_value source_report_id "$proposal_get_stdout")"

test -n "$proposal_id"
test "$proposal_status" = "Open"
test -n "$model_summary_headline"
test -n "$model_proposed_decision_type"
test "$source_report_id" = "$report_id"
grep -q "work_unit_id: $workunit_id" "$proposal_get_stdout"
grep -q "status: Open" "$proposal_get_stdout"
grep -q '^model_summary_headline:' "$proposal_get_stdout"
grep -q '^model_summary_situation:' "$proposal_get_stdout"
grep -q '^model_proposed_decision_type:' "$proposal_get_stdout"
grep -q '^model_requires_assignment:' "$proposal_get_stdout"

e2e_orcas supervisor work proposals artifact-summary --proposal "$proposal_id" >"$proposal_summary_stdout"
grep -q '^prompt_artifact_present:' "$proposal_summary_stdout"
grep -q '^response_artifact_present:' "$proposal_summary_stdout"

proposal_approve_output="$(
  e2e_orcas supervisor work proposals approve \
    --proposal "$proposal_id" \
    --reviewed-by live-supervisor-micro-proposal \
    --review-note "Approve the bounded follow-up generated from the live report." \
    --type continue \
  | tee "$proposal_approve_stdout"
)"
decision_id="$(printf '%s\n' "$proposal_approve_output" | awk -F': ' '/^decision_id:/ {print $2; exit}')"
next_assignment_id="$(printf '%s\n' "$proposal_approve_output" | awk -F': ' '/^next_assignment_id:/ {print $2; exit}')"

test -n "$decision_id"
test -n "$next_assignment_id"
grep -q "proposal_id: $proposal_id" "$proposal_approve_stdout"
grep -q "status: Approved" "$proposal_approve_stdout"
grep -q "decision_type: Continue" "$proposal_approve_stdout"
grep -q "next_assignment_id: $next_assignment_id" "$proposal_approve_stdout"

e2e_orcas supervisor work proposals get --proposal "$proposal_id" >"$proposal_get_stdout"
grep -q "status: Approved" "$proposal_get_stdout"
grep -q "approved_decision_id: $decision_id" "$proposal_get_stdout"
grep -q "approved_assignment_id: $next_assignment_id" "$proposal_get_stdout"
grep -q "approval_edits_present: true" "$proposal_get_stdout"
grep -q "approval_edit_decision_type: Continue" "$proposal_get_stdout"
grep -q "approved_proposed_decision_type: Continue" "$proposal_get_stdout"

e2e_orcas supervisor work assignments get --assignment "$next_assignment_id" >"$next_assignment_get_stdout"
next_assignment_work_unit_id="$(e2e_field_value work_unit_id "$next_assignment_get_stdout")"
next_assignment_status="$(e2e_field_value status "$next_assignment_get_stdout")"
next_assignment_attempt="$(e2e_field_value attempt "$next_assignment_get_stdout")"

test "$next_assignment_work_unit_id" = "$workunit_id"
test "$next_assignment_status" = "Created"
test "$next_assignment_attempt" = "2"
grep -q "work_unit_id: $workunit_id" "$next_assignment_get_stdout"
grep -q "status: Created" "$next_assignment_get_stdout"
grep -q "attempt: 2" "$next_assignment_get_stdout"

wait "$assignment_start_pid" >/dev/null 2>&1 || true

echo "PASS"
