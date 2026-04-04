#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "$0")/../../lib" && pwd)/common.sh"
scenario_dir="$(cd "$(dirname "$0")" && pwd)"

e2e_load_scenario_metadata "$scenario_dir"
e2e_prepare_scenario_dirs "$NAME"
e2e_prepare_live_codex_environment "pfib" 6100

phase_label() {
  printf '%02d-%s' "$1" "$2"
}

write_phase_prompts() {
  local phase_index="$1"
  local phase_name="$2"
  local assignment_id="$3"
  local objective="$4"
  local instructions="$5"
  local rationale="$6"
  local phase_dir="$prompt_root/$(phase_label "$phase_index" "$phase_name")"

  mkdir -p "$phase_dir"

  cat >"$phase_dir/operator-prompt.txt" <<EOF
Operator phase ${phase_index}: ${phase_name}

Approval rationale:
${rationale}

Decision rule:
- Stay on the current tracked-thread lane.
- Approve only the next bounded step.
- Reject any drift beyond the declared phase.
EOF

  cat >"$phase_dir/supervisor-prompt.txt" <<EOF
Supervisor phase ${phase_index}: ${phase_name}

Current objective:
- ${objective}

Guidance:
- Keep the work inside the declared tracked-thread worktree lane.
- Keep the code buildable after each coding phase.
- Leave future phases for later operator decisions.
EOF

  cat >"$phase_dir/agent-prompt.txt" <<EOF
Agent phase ${phase_index}: ${phase_name}

Assignment id:
- ${assignment_id}

Current objective:
- ${objective}

Worker instruction:
${instructions}

Operating rules:
- Use the declared tracked-thread workspace path only.
- Avoid unrelated file changes.
- Leave a concise report that matches the current phase.
EOF
}

scenario_name="phased-fibonacci"
base_ref="${ORCAS_E2E_GIT_BASE_REF:-main}"
artifacts_dir="$E2E_SCENARIO_ARTIFACTS_DIR"
reports_dir="$E2E_SCENARIO_REPORTS_DIR"
prompt_root="$artifacts_dir/phases"
worktree_path="$E2E_SCENARIO_WORKTREES_DIR/lane"
repo_root="$E2E_SCENARIO_WORKTREES_DIR/lane-repo"
branch_suffix="${E2E_RUN_ID//[^a-zA-Z0-9]/-}"
branch_name="orcas/$scenario_name/$branch_suffix"
daemon_log="$E2E_SCENARIO_LOGS_DIR/orcasd.log"
worker_id="phased-fibonacci-worker"

phase_titles=(
  "scope"
  "build-core"
  "tests-and-polish"
)
phase_objectives=(
  "Read plan.md and summarize the implementation order, risks, and constraints without editing files."
  "Build the Fibonacci CLI core in the declared tracked-thread worktree lane."
  "Add repeatable tests, tighten the build, and finish the Fibonacci project."
)
phase_instructions=(
  "Read plan.md in the declared tracked-thread worktree lane. Produce a concise scoping report that names the phase order, likely risks, and implementation constraints. Do not edit files, create files, or change git state in this phase."
  "Implement the core Fibonacci CLI in the declared tracked-thread worktree lane. Create main.c, fib.c, fib.h, and a Makefile. Support --count and --separator with clear validation. Keep the Makefile buildable and provide make test that at least builds the binary and exercises one smoke invocation. Leave deeper test coverage and polish for the next phase."
  "Stay on the same tracked-thread worktree lane. Add repeatable tests under tests/, tighten warnings or obvious rough edges, and finish the Fibonacci CLI so make test passes cleanly. Do not create a new worktree or move to another thread."
)
phase_rationales=(
  "The scoped plan is ready and the next bounded implementation step is clear."
  "The Fibonacci CLI core exists and the final bounded polish step is ready."
  "The implementation is complete and the work unit should be closed."
)

mkdir -p "$artifacts_dir" "$reports_dir" "$prompt_root" "$(dirname "$worktree_path")"
cp "$scenario_dir/plan.md" "$artifacts_dir/plan.md"

e2e_prepare_empty_repo_with_worktree "$repo_root" "$worktree_path" "$branch_name" "$base_ref" "$reports_dir" "lane"
cp "$scenario_dir/plan.md" "$worktree_path/plan.md"
git -C "$worktree_path" add plan.md
git -C "$worktree_path" commit -m "Add phased Fibonacci plan" >"$reports_dir/lane-git-plan-commit.txt" 2>&1

e2e_start_managed_daemon "$daemon_log"
cleanup() {
  e2e_stop_managed_daemon
}
trap cleanup EXIT

sleep 5

workstream_output="$(
  e2e_orcas workstreams create \
    --title "Phased Fibonacci live lane" \
    --objective "Validate a multi-phase tracked-thread workflow on one real live Codex lane" \
    --priority high
)"
workstream_id="$(printf '%s\n' "$workstream_output" | awk -F': ' '/^workstream_id:/ {print $2; exit}')"

workunit_output="$(
  e2e_orcas workunit create \
    --workstream "$workstream_id" \
    --title "Phased Fibonacci tracked-thread lane" \
    --task "Build a small Fibonacci CLI in bounded phases on one tracked-thread worktree lane. Keep the lane continuous from scope through completion." \
)"
workunit_id="$(printf '%s\n' "$workunit_output" | awk -F': ' '/^work_unit_id:/ {print $2; exit}')"

tracked_output="$(
  e2e_add_tracked_thread_workspace \
    "$workunit_id" \
    "Phased Fibonacci live lane" \
    "$repo_root" \
    "Dedicated tracked-thread worktree lane for the phased Fibonacci live scenario" \
    "$repo_root" \
    "$worktree_path" \
    "$branch_name" \
    "$base_ref" \
    "$(git -C "$worktree_path" rev-parse HEAD)" \
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

phase1_assignment_stdout="$reports_dir/phase1-assignment-start.txt"
write_phase_prompts 1 "${phase_titles[0]}" pending "${phase_objectives[0]}" "${phase_instructions[0]}" "${phase_rationales[0]}"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" supervisor work assignments start \
  --workunit "$workunit_id" \
  --worker "$worker_id" \
  --worker-kind codex \
  --instructions "${phase_instructions[0]}" \
  --cwd "$worktree_path" \
  >"$phase1_assignment_stdout" 2>&1 &
phase1_assignment_start_pid=$!

e2e_wait_for_report_id "$workunit_id" phase1_report_id

phase1_report_get_stdout="$reports_dir/phase1-report-get.txt"
phase1_assignment_get_stdout="$reports_dir/phase1-assignment-get.txt"
phase1_git_status_stdout="$reports_dir/phase1-git-status.txt"
phase1_tracked_stdout="$reports_dir/tracked-thread-after-phase1.txt"
runtime_after_phase1_stdout="$reports_dir/workstream-runtime-after-phase1.txt"
threads_after_phase1_stdout="$reports_dir/workstream-threads-after-phase1.txt"

e2e_orcas supervisor work reports get --report "$phase1_report_id" >"$phase1_report_get_stdout"
phase1_assignment_id="$(e2e_field_value assignment_id "$phase1_report_get_stdout")"
phase1_report_parse_result="$(e2e_field_value parse_result "$phase1_report_get_stdout")"

e2e_orcas supervisor work assignments get --assignment "$phase1_assignment_id" >"$phase1_assignment_get_stdout"
phase1_assignment_status="$(e2e_field_value status "$phase1_assignment_get_stdout")"
phase1_thread_id="$(e2e_field_value thread_id "$phase1_assignment_stdout")"

git -C "$worktree_path" status --short | grep -v '^?? \.codex$' >"$phase1_git_status_stdout" || true
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$phase1_tracked_stdout"
e2e_capture_workstream_runtime "$workstream_id" "$runtime_after_phase1_stdout"
e2e_capture_workstream_threads "$workstream_id" "$threads_after_phase1_stdout"
e2e_assert_workstream_runtime "$workstream_id" "$runtime_after_phase1_stdout"
e2e_assert_runtime_thread_count "$runtime_after_phase1_stdout" 1
e2e_assert_managed_thread_count "$threads_after_phase1_stdout" 1

test -n "$phase1_assignment_id"
test -n "$phase1_thread_id"
test "$phase1_assignment_status" = "AwaitingDecision"
test "$phase1_report_parse_result" != "Invalid"
test ! -s "$phase1_git_status_stdout"
grep -q "binding_state: Bound" "$phase1_tracked_stdout"
grep -q "upstream_thread_id: $phase1_thread_id" "$phase1_tracked_stdout"
grep -q "workspace_worktree_path: $worktree_path" "$phase1_tracked_stdout"
grep -q "workspace_branch_name: $branch_name" "$phase1_tracked_stdout"

wait "$phase1_assignment_start_pid" >/dev/null 2>&1 || true

continue_after_phase1_stdout="$reports_dir/decision-continue-after-phase1.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" supervisor work decisions apply \
  --workunit "$workunit_id" \
  --report "$phase1_report_id" \
  --type continue \
  --rationale "${phase_rationales[0]}" \
  --instructions "${phase_instructions[1]}" \
  >"$continue_after_phase1_stdout" 2>&1

phase2_assignment_id="$(e2e_field_value next_assignment_id "$continue_after_phase1_stdout")"
test -n "$phase2_assignment_id"
test "$(e2e_field_value work_unit_status "$continue_after_phase1_stdout")" = "Ready"
write_phase_prompts 2 "${phase_titles[1]}" "$phase2_assignment_id" "${phase_objectives[1]}" "${phase_instructions[1]}" "${phase_rationales[1]}"

phase2_assignment_stdout="$reports_dir/phase2-assignment-start.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" supervisor work assignments start \
  --workunit "$workunit_id" \
  --worker "$worker_id" \
  --worker-kind codex \
  --instructions "${phase_instructions[1]}" \
  --cwd "$worktree_path" \
  >"$phase2_assignment_stdout" 2>&1 &
phase2_assignment_start_pid=$!

e2e_wait_for_assignment_report_id "$phase2_assignment_id" phase2_report_id

phase2_report_get_stdout="$reports_dir/phase2-report-get.txt"
phase2_assignment_get_stdout="$reports_dir/phase2-assignment-get.txt"
phase2_make_test_stdout="$reports_dir/phase2-make-test.txt"
phase2_default_stdout="$reports_dir/phase2-default-output.txt"
phase2_custom_stdout="$reports_dir/phase2-custom-output.txt"
phase2_tracked_stdout="$reports_dir/tracked-thread-after-phase2.txt"
runtime_after_phase2_stdout="$reports_dir/workstream-runtime-after-phase2.txt"
threads_after_phase2_stdout="$reports_dir/workstream-threads-after-phase2.txt"

e2e_orcas supervisor work reports get --report "$phase2_report_id" >"$phase2_report_get_stdout"
phase2_report_assignment_id="$(e2e_field_value assignment_id "$phase2_report_get_stdout")"
phase2_report_parse_result="$(e2e_field_value parse_result "$phase2_report_get_stdout")"

e2e_orcas supervisor work assignments get --assignment "$phase2_assignment_id" >"$phase2_assignment_get_stdout"
phase2_assignment_status="$(e2e_field_value status "$phase2_assignment_get_stdout")"
phase2_thread_id="$(e2e_field_value thread_id "$phase2_assignment_stdout")"

make -C "$worktree_path" test >"$phase2_make_test_stdout"
(cd "$worktree_path" && ./fibonacci --count 7) >"$phase2_default_stdout"
(cd "$worktree_path" && ./fibonacci --count 5 --separator ,) >"$phase2_custom_stdout"
make -C "$worktree_path" clean >/dev/null 2>&1 || true
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$phase2_tracked_stdout"
e2e_capture_workstream_runtime "$workstream_id" "$runtime_after_phase2_stdout"
e2e_capture_workstream_threads "$workstream_id" "$threads_after_phase2_stdout"
e2e_assert_workstream_runtime "$workstream_id" "$runtime_after_phase2_stdout"
e2e_assert_runtime_thread_count "$runtime_after_phase2_stdout" 1
e2e_assert_managed_thread_count "$threads_after_phase2_stdout" 1

test "$phase2_report_assignment_id" = "$phase2_assignment_id"
test "$phase2_assignment_status" = "AwaitingDecision"
test "$phase2_report_parse_result" != "Invalid"
test "$phase2_thread_id" = "$phase1_thread_id"
test -f "$worktree_path/main.c"
test -f "$worktree_path/fib.c"
test -f "$worktree_path/fib.h"
test -f "$worktree_path/Makefile"
grep -q "binding_state: Bound" "$phase2_tracked_stdout"
grep -q "upstream_thread_id: $phase1_thread_id" "$phase2_tracked_stdout"
grep -q '^PASS$' "$phase2_make_test_stdout"
grep -q '0 1 1 2 3 5 8' "$phase2_default_stdout"
grep -q '0,1,1,2,3' "$phase2_custom_stdout"

wait "$phase2_assignment_start_pid" >/dev/null 2>&1 || true

continue_after_phase2_stdout="$reports_dir/decision-continue-after-phase2.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" supervisor work decisions apply \
  --workunit "$workunit_id" \
  --report "$phase2_report_id" \
  --type continue \
  --rationale "${phase_rationales[1]}" \
  --instructions "${phase_instructions[2]}" \
  >"$continue_after_phase2_stdout" 2>&1

phase3_assignment_id="$(e2e_field_value next_assignment_id "$continue_after_phase2_stdout")"
test -n "$phase3_assignment_id"
test "$(e2e_field_value work_unit_status "$continue_after_phase2_stdout")" = "Ready"
write_phase_prompts 3 "${phase_titles[2]}" "$phase3_assignment_id" "${phase_objectives[2]}" "${phase_instructions[2]}" "${phase_rationales[2]}"

phase3_assignment_stdout="$reports_dir/phase3-assignment-start.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" supervisor work assignments start \
  --workunit "$workunit_id" \
  --worker "$worker_id" \
  --worker-kind codex \
  --instructions "${phase_instructions[2]}" \
  --cwd "$worktree_path" \
  >"$phase3_assignment_stdout" 2>&1 &
phase3_assignment_start_pid=$!

e2e_wait_for_assignment_report_id "$phase3_assignment_id" phase3_report_id

phase3_report_get_stdout="$reports_dir/phase3-report-get.txt"
phase3_assignment_get_stdout="$reports_dir/phase3-assignment-get.txt"
phase3_make_test_stdout="$reports_dir/phase3-make-test.txt"
phase3_git_status_stdout="$reports_dir/phase3-git-status.txt"
phase3_tracked_stdout="$reports_dir/tracked-thread-after-phase3.txt"
runtime_after_phase3_stdout="$reports_dir/workstream-runtime-after-phase3.txt"
threads_after_phase3_stdout="$reports_dir/workstream-threads-after-phase3.txt"

e2e_orcas supervisor work reports get --report "$phase3_report_id" >"$phase3_report_get_stdout"
phase3_report_assignment_id="$(e2e_field_value assignment_id "$phase3_report_get_stdout")"
phase3_report_parse_result="$(e2e_field_value parse_result "$phase3_report_get_stdout")"

e2e_orcas supervisor work assignments get --assignment "$phase3_assignment_id" >"$phase3_assignment_get_stdout"
phase3_assignment_status="$(e2e_field_value status "$phase3_assignment_get_stdout")"
phase3_thread_id="$(e2e_field_value thread_id "$phase3_assignment_stdout")"

make -C "$worktree_path" test >"$phase3_make_test_stdout"
git -C "$worktree_path" status --short >"$phase3_git_status_stdout"
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$phase3_tracked_stdout"
e2e_capture_workstream_runtime "$workstream_id" "$runtime_after_phase3_stdout"
e2e_capture_workstream_threads "$workstream_id" "$threads_after_phase3_stdout"
e2e_assert_workstream_runtime "$workstream_id" "$runtime_after_phase3_stdout"
e2e_assert_runtime_thread_count "$runtime_after_phase3_stdout" 1
e2e_assert_managed_thread_count "$threads_after_phase3_stdout" 1

test "$phase3_report_assignment_id" = "$phase3_assignment_id"
test "$phase3_assignment_status" = "AwaitingDecision"
test "$phase3_report_parse_result" != "Invalid"
test "$phase3_thread_id" = "$phase1_thread_id"
test -f "$worktree_path/tests/test_fibonacci.sh"
grep -q "binding_state: Bound" "$phase3_tracked_stdout"
grep -q "upstream_thread_id: $phase1_thread_id" "$phase3_tracked_stdout"
grep -q '^PASS$' "$phase3_make_test_stdout"

wait "$phase3_assignment_start_pid" >/dev/null 2>&1 || true

complete_stdout="$reports_dir/decision-complete.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" supervisor work decisions apply \
  --workunit "$workunit_id" \
  --report "$phase3_report_id" \
  --type mark-complete \
  --rationale "${phase_rationales[2]}" \
  >"$complete_stdout" 2>&1

test -n "$(e2e_field_value decision_id "$complete_stdout")"
test "$(e2e_field_value work_unit_status "$complete_stdout")" = "Completed"
e2e_orcas workunit get --workunit "$workunit_id" >"$reports_dir/workunit-after-completion.txt"
grep -q "tracked_threads: 1" "$reports_dir/workunit-after-completion.txt"
grep -q "$tracked_thread_id" "$reports_dir/workunit-after-completion.txt"

echo "PASS"
