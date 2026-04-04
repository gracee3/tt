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

short_xdg_root="$e2e_output_root/xdg/$E2E_RUN_ID/lmpl"
short_xdg_data_home="$short_xdg_root/data"
short_xdg_config_home="$short_xdg_root/config"
short_xdg_runtime_home="$short_xdg_root/runtime"
listen_port="$((5900 + ($(printf '%s' "$E2E_RUN_ID" | cksum | awk '{print $1}') % 1000)))"
listen_url="ws://127.0.0.1:$listen_port"
supervisor_base_url="${ORCAS_SUPERVISOR_BASE_URL:-http://127.0.0.1:8000/v1}"
supervisor_model="${ORCAS_SUPERVISOR_MODEL:-gpt-oss-20b}"
supervisor_api_key_env="${ORCAS_SUPERVISOR_API_KEY_ENV:-}"
supervisor_reasoning_effort="${ORCAS_SUPERVISOR_REASONING_EFFORT:-}"
supervisor_max_output_tokens="${ORCAS_SUPERVISOR_MAX_OUTPUT_TOKENS:-16384}"
supervisor_temperature="${ORCAS_SUPERVISOR_TEMPERATURE:-0.0}"
codex_bin="${ORCAS_CODEX_BIN:-$(command -v codex)}"

worktree_path="$E2E_SCENARIO_WORKTREES_DIR/lane"
repo_root="$E2E_SCENARIO_WORKTREES_DIR/lane-repo"
daemon_log="$E2E_SCENARIO_LOGS_DIR/orcasd.log"
reports_dir="$E2E_SCENARIO_REPORTS_DIR"
artifacts_dir="$E2E_SCENARIO_ARTIFACTS_DIR"
base_ref="${ORCAS_E2E_GIT_BASE_REF:-main}"
branch_suffix="${E2E_RUN_ID//[^a-zA-Z0-9]/-}"
branch_name="orcas/$NAME/$branch_suffix"

rm -rf "$short_xdg_root"
mkdir -p "$short_xdg_data_home/orcas" "$short_xdg_config_home/orcas" "$short_xdg_runtime_home/orcas"
chmod 700 "$short_xdg_runtime_home" || true

cat >"$short_xdg_config_home/orcas/config.toml" <<EOF
[codex]
binary_path = "$codex_bin"
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
temperature = $supervisor_temperature
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

rm -rf "$worktree_path" "$repo_root"
mkdir -p "$reports_dir" "$artifacts_dir" "$(dirname "$worktree_path")"

mkdir -p "$repo_root"
cp -R "$fixture_dir/." "$repo_root/"
git -C "$repo_root" init -b "$base_ref" >"$reports_dir/git-init.txt" 2>&1
git -C "$repo_root" config user.name "Orcas E2E"
git -C "$repo_root" config user.email "orcas-e2e@example.com"
git -C "$repo_root" add .
git -C "$repo_root" commit -m "Initial tracked-thread fixture" >"$reports_dir/git-initial-commit.txt" 2>&1
git -C "$repo_root" worktree add -b "$branch_name" "$worktree_path" "$base_ref" >"$reports_dir/git-worktree-add.txt" 2>&1

e2e_orcas daemon start --force-spawn >"$daemon_log" 2>&1 &
daemon_pid=$!
cleanup() {
  kill "$daemon_pid" >/dev/null 2>&1 || true
}
trap cleanup EXIT

sleep 5

workstream_output="$(
  e2e_orcas workstreams create \
    --title "Live multi-phase lane" \
    --objective "Prove one tracked-thread worktree lane can survive multiple live review steps without losing continuity" \
    --priority normal
)"
workstream_id="$(printf '%s\n' "$workstream_output" | awk -F': ' '/^workstream_id:/ {print $2; exit}')"

workunit_output="$(
  e2e_orcas workunit create \
    --workstream "$workstream_id" \
    --title "Multi-phase tracked-thread lane" \
    --task "Phase 1: fix the greeting bug in main.c so make test passes. Keep the change bounded to main.c only. Phase 2 will be a separate bounded follow-up on the same tracked-thread worktree lane." \
)"
workunit_id="$(printf '%s\n' "$workunit_output" | awk -F': ' '/^work_unit_id:/ {print $2; exit}')"

tracked_output="$(
  e2e_orcas workunit thread add \
    --workunit "$workunit_id" \
    --title "Live multi-phase lane" \
    --root-dir "$repo_root" \
    --notes "Dedicated tracked-thread worktree lane for live multi-phase continuity validation" \
    --workspace-repository-root "$repo_root" \
    --workspace-worktree-path "$worktree_path" \
    --workspace-branch-name "$branch_name" \
    --workspace-base-ref "$base_ref" \
    --workspace-base-commit "$(git -C "$repo_root" rev-parse HEAD)" \
    --workspace-landing-target "$base_ref" \
    --workspace-strategy dedicated-thread-worktree \
    --workspace-landing-policy merge-to-main \
    --workspace-sync-policy manual \
    --workspace-cleanup-policy keep-until-campaign-closed \
    --workspace-status ready \
)"
tracked_thread_id="$(printf '%s\n' "$tracked_output" | awk -F': ' '/^tracked_thread_id:/ {print $2; exit}')"

tracked_before_stdout="$reports_dir/tracked-thread-before-phase1.txt"
e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$tracked_before_stdout"

phase1_assignment_stdout="$reports_dir/phase1-assignment-start.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" assignments start \
  --workunit "$workunit_id" \
  --worker live-multi-phase-lane-worker \
  --worker-kind codex \
  --instructions "Phase 1: inspect the tiny C program and shell test in the declared tracked-thread worktree lane. Make the smallest possible code change in main.c to make make test pass. Edit main.c in place or with apply_patch only. Do not create backup files, temporary files, rename-based edits, editor swap files, or unrelated code changes. Do not touch tests/test.sh. Do not mention any later phase in the report. Return a brief summary of the exact main.c change." \
  --cwd "$worktree_path" \
  >"$phase1_assignment_stdout" 2>&1 &
phase1_assignment_start_pid=$!

phase1_report_id=""
for _ in $(seq 1 120); do
  reports_output="$("$e2e_bin_dir/orcas.sh" reports list-for-workunit --workunit "$workunit_id" 2>/dev/null || true)"
  phase1_report_id="$(printf '%s\n' "$reports_output" | awk -F'\t' '/^report-/ {print $1; exit}')"
  [[ -n "$phase1_report_id" ]] && break
  sleep 5
done
test -n "$phase1_report_id"

phase1_report_get_stdout="$reports_dir/phase1-report-get.txt"
phase1_assignment_get_stdout="$reports_dir/phase1-assignment-get.txt"
phase1_make_test_stdout="$reports_dir/phase1-make-test.txt"
phase1_tree_diff_stdout="$reports_dir/phase1-tree-diff.txt"
phase1_git_status_stdout="$reports_dir/phase1-git-status.txt"
phase1_tracked_thread_bind_stdout="$reports_dir/tracked-thread-bind.txt"
phase1_tracked_thread_after_stdout="$reports_dir/tracked-thread-after-phase1.txt"

e2e_orcas supervisor work reports get --report "$phase1_report_id" >"$phase1_report_get_stdout"
phase1_assignment_id="$(field_value assignment_id "$phase1_report_get_stdout")"
phase1_report_parse_result="$(field_value parse_result "$phase1_report_get_stdout")"

e2e_orcas supervisor work assignments get --assignment "$phase1_assignment_id" >"$phase1_assignment_get_stdout"
phase1_assignment_status="$(field_value status "$phase1_assignment_get_stdout")"
phase1_worker_session_id="$(field_value worker_session_id "$phase1_assignment_get_stdout")"
phase1_thread_id="$(field_value thread_id "$phase1_assignment_stdout")"

test -n "$phase1_assignment_id"
test -n "$phase1_worker_session_id"
test -n "$phase1_thread_id"
test -n "$phase1_report_parse_result"
test "$phase1_assignment_status" = "AwaitingDecision"
test "$phase1_report_parse_result" != "Invalid"

make -C "$worktree_path" test >"$phase1_make_test_stdout"
make -C "$worktree_path" clean >/dev/null 2>&1 || true
git -C "$worktree_path" status --short >"$phase1_git_status_stdout"
diff -qr --exclude=.git "$fixture_dir" "$worktree_path" >"$phase1_tree_diff_stdout" || true

phase1_changed_count="$(sed '/^$/d' "$phase1_tree_diff_stdout" | wc -l | tr -d ' ')"
test "$phase1_changed_count" -eq 1
grep -q 'main.c' "$phase1_tree_diff_stdout"
grep -q '^PASS$' "$phase1_make_test_stdout"
grep -qx ' M main.c' "$phase1_git_status_stdout"
grep -q 'Hello, Orcas!' "$worktree_path/main.c"
grep -q "assignment_id: $phase1_assignment_id" "$phase1_report_get_stdout"
grep -q "report_id: $phase1_report_id" "$phase1_report_get_stdout"
grep -q "status: AwaitingDecision" "$phase1_assignment_get_stdout"

e2e_orcas workunit thread set \
  --tracked-thread "$tracked_thread_id" \
  --upstream-thread "$phase1_thread_id" \
  --binding-state bound \
  >"$phase1_tracked_thread_bind_stdout"

e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$phase1_tracked_thread_after_stdout"
grep -q "binding_state: Bound" "$phase1_tracked_thread_after_stdout"
grep -q "workspace_worktree_path: $worktree_path" "$phase1_tracked_thread_after_stdout"
grep -q "workspace_branch_name: $branch_name" "$phase1_tracked_thread_after_stdout"

phase1_proposal_create_stdout="$reports_dir/phase1-proposal-create.txt"
phase1_proposal_get_stdout="$reports_dir/phase1-proposal-get.txt"
phase1_proposal_approve_stdout="$reports_dir/phase1-proposal-approve.txt"

phase1_proposal_create_output="$(
  e2e_orcas supervisor work proposals create \
    --workunit "$workunit_id" \
    --report "$phase1_report_id" \
    --requested-by live-multi-phase-lane \
    --note "Generate a bounded redirect proposal for one tiny test-only follow-up on the greeting fix. Keep every field terse. Use exactly 2 instructions, exactly 2 acceptance criteria, exactly 2 stop conditions, exactly 2 expected report fields, and a concise boundedness note. Set plan_assessment and plan_revision_proposal to null. Do not escalate or mark the work complete." \
  | tee "$phase1_proposal_create_stdout"
)"
phase1_proposal_id="$(printf '%s\n' "$phase1_proposal_create_output" | awk -F': ' '/^proposal_id:/ {print $2; exit}')"

e2e_orcas supervisor work proposals get --proposal "$phase1_proposal_id" >"$phase1_proposal_get_stdout"
grep -q "status: Open" "$phase1_proposal_get_stdout"
grep -q "source_report_id: $phase1_report_id" "$phase1_proposal_get_stdout"
grep -q "^model_summary_headline:" "$phase1_proposal_get_stdout"
grep -q "^model_draft_assignment_objective:" "$phase1_proposal_get_stdout"

phase1_proposal_approve_output="$(
  e2e_orcas supervisor work proposals approve \
    --proposal "$phase1_proposal_id" \
    --reviewed-by live-multi-phase-lane \
    --review-note "Redirect this into a test-only follow-up that stays smaller than the original fix." \
    --type redirect \
    --objective "Add one explanatory comment to main.c without changing behavior." \
    --instruction "Add one explanatory comment to main.c." \
    --instruction "Do not change any other file." \
    --acceptance "Only main.c is modified." \
    --acceptance "make test still passes." \
    --stop-condition "Stop if any other file would need to change." \
    --stop-condition "Stop if the comment would change behavior." \
    --expected-report-field summary \
    --expected-report-field findings \
  | tee "$phase1_proposal_approve_stdout"
)"
phase1_decision_id="$(printf '%s\n' "$phase1_proposal_approve_output" | awk -F': ' '/^decision_id:/ {print $2; exit}')"
phase1_next_assignment_id="$(printf '%s\n' "$phase1_proposal_approve_output" | awk -F': ' '/^next_assignment_id:/ {print $2; exit}')"
test -n "$phase1_decision_id"
test -n "$phase1_next_assignment_id"
grep -q "status: Approved" "$phase1_proposal_approve_stdout"
grep -q "decision_type: Redirect" "$phase1_proposal_approve_stdout"
grep -q "next_assignment_id: $phase1_next_assignment_id" "$phase1_proposal_approve_stdout"

e2e_orcas supervisor work proposals get --proposal "$phase1_proposal_id" >"$phase1_proposal_get_stdout"
grep -q "status: Approved" "$phase1_proposal_get_stdout"
grep -q "approved_decision_id: $phase1_decision_id" "$phase1_proposal_get_stdout"
grep -q "approved_assignment_id: $phase1_next_assignment_id" "$phase1_proposal_get_stdout"
grep -q "approval_edit_decision_type: Redirect" "$phase1_proposal_get_stdout"
grep -q "approved_proposed_decision_type: Redirect" "$phase1_proposal_get_stdout"
grep -q "approved_draft_assignment_objective: Add one explanatory comment to main.c without changing behavior." "$phase1_proposal_get_stdout"

phase2_assignment_stdout="$reports_dir/phase2-assignment-start.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" assignments start \
  --workunit "$workunit_id" \
  --worker live-multi-phase-lane-worker \
  --worker-kind codex \
  --instructions "Run the redirected follow-up on the same tracked-thread worktree lane. Add one explanatory comment to main.c without changing behavior. Edit only main.c in place or with apply_patch. Do not create a new worktree or touch any other file. Return a brief summary of the exact main.c comment change." \
  --cwd "$worktree_path" \
  >"$phase2_assignment_stdout" 2>&1 &
phase2_assignment_start_pid=$!

phase2_report_id=""
for _ in $(seq 1 120); do
  e2e_orcas supervisor work assignments get --assignment "$phase1_next_assignment_id" >"$reports_dir/phase2-assignment-get.txt" 2>/dev/null || true
  phase2_report_id="$(field_value report_id "$reports_dir/phase2-assignment-get.txt")"
  [[ -n "$phase2_report_id" ]] && break
  sleep 5
done
test -n "$phase2_report_id"

phase2_report_get_stdout="$reports_dir/phase2-report-get.txt"
phase2_assignment_get_stdout="$reports_dir/phase2-assignment-get.txt"
phase2_make_test_stdout="$reports_dir/phase2-make-test.txt"
phase2_tree_diff_stdout="$reports_dir/phase2-tree-diff.txt"
phase2_tracked_thread_after_stdout="$reports_dir/tracked-thread-after-phase2.txt"

e2e_orcas supervisor work reports get --report "$phase2_report_id" >"$phase2_report_get_stdout"
phase2_assignment_id="$(field_value assignment_id "$phase2_report_get_stdout")"
phase2_report_parse_result="$(field_value parse_result "$phase2_report_get_stdout")"

e2e_orcas supervisor work assignments get --assignment "$phase2_assignment_id" >"$phase2_assignment_get_stdout"
phase2_assignment_status="$(field_value status "$phase2_assignment_get_stdout")"
phase2_worker_session_id="$(field_value worker_session_id "$phase2_assignment_get_stdout")"

test "$phase2_assignment_id" = "$phase1_next_assignment_id"
test -n "$phase2_worker_session_id"
test -n "$phase2_report_parse_result"
test "$phase2_assignment_status" = "AwaitingDecision"

make -C "$worktree_path" test >"$phase2_make_test_stdout"
make -C "$worktree_path" clean >/dev/null 2>&1 || true
diff -qr --exclude=.git "$fixture_dir" "$worktree_path" >"$phase2_tree_diff_stdout" || true

phase2_changed_count="$(sed '/^$/d' "$phase2_tree_diff_stdout" | wc -l | tr -d ' ')"
test "$phase2_changed_count" -eq 1
grep -q 'main.c' "$phase2_tree_diff_stdout"
grep -q '^PASS$' "$phase2_make_test_stdout"

e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$phase2_tracked_thread_after_stdout"
grep -q "binding_state: Bound" "$phase2_tracked_thread_after_stdout"
grep -q "workspace_worktree_path: $worktree_path" "$phase2_tracked_thread_after_stdout"
grep -q "workspace_branch_name: $branch_name" "$phase2_tracked_thread_after_stdout"

phase2_complete_stdout="$reports_dir/decision-complete-after-phase2.txt"
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" decisions apply \
  --workunit "$workunit_id" \
  --report "$phase2_report_id" \
  --type mark-complete \
  --rationale "Close the multi-phase lane after the second bounded assignment landed cleanly." \
  >"$phase2_complete_stdout" 2>&1

phase2_complete_decision_id="$(field_value decision_id "$phase2_complete_stdout")"
phase2_complete_workunit_status="$(field_value work_unit_status "$phase2_complete_stdout")"
test -n "$phase2_complete_decision_id"
test "$phase2_complete_workunit_status" = "Completed"
grep -q "decision_type: MarkComplete" "$phase2_complete_stdout"
grep -q "work_unit_status: Completed" "$phase2_complete_stdout"

e2e_orcas workunit get --workunit "$workunit_id" >"$reports_dir/workunit-after-completion.txt"
grep -q "tracked_threads: 1" "$reports_dir/workunit-after-completion.txt"
grep -q "$tracked_thread_id" "$reports_dir/workunit-after-completion.txt"

wait "$phase1_assignment_start_pid" >/dev/null 2>&1 || true
wait "$phase2_assignment_start_pid" >/dev/null 2>&1 || true

echo "PASS"
