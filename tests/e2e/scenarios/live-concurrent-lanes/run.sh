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

wait_for_report_id() {
  local workunit_id="$1"
  local output_var="$2"
  local reports_output report_id
  report_id=""
  for _ in $(seq 1 180); do
    reports_output="$("$e2e_bin_dir/orcas.sh" reports list-for-workunit --workunit "$workunit_id" 2>/dev/null || true)"
    report_id="$(printf '%s\n' "$reports_output" | awk -F'\t' '/^report-/ {print $1; exit}')"
    [[ -n "$report_id" ]] && break
    sleep 5
  done
  test -n "$report_id"
  printf -v "$output_var" '%s' "$report_id"
}

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

  rm -rf "$repo_root" "$worktree_path"
  mkdir -p "$(dirname "$worktree_path")"
  cp -R "$fixture_dir/." "$repo_root/"

  git -C "$repo_root" init -b "$base_ref" >"$reports_dir/${prefix}-git-init.txt" 2>&1
  git -C "$repo_root" config user.name "Orcas E2E"
  git -C "$repo_root" config user.email "orcas-e2e@example.com"
  git -C "$repo_root" add .
  git -C "$repo_root" commit -m "Initial tracked-thread fixture" >"$reports_dir/${prefix}-git-initial-commit.txt" 2>&1
  git -C "$repo_root" worktree add -b "$branch_name" "$worktree_path" "$base_ref" >"$reports_dir/${prefix}-git-worktree-add.txt" 2>&1

  workunit_output="$(
    e2e_orcas workunit create \
      --workstream "$workstream_id" \
      --title "$lane_title" \
      --task "$lane_task"
  )"
  workunit_id="$(printf '%s\n' "$workunit_output" | awk -F': ' '/^work_unit_id:/ {print $2; exit}')"

  tracked_output="$(
    e2e_orcas workunit thread add \
      --workunit "$workunit_id" \
      --title "$lane_title" \
      --root-dir "$repo_root" \
      --notes "Dedicated tracked-thread worktree lane for live concurrent lane isolation validation" \
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
      --workspace-status ready
  )"
  tracked_thread_id="$(printf '%s\n' "$tracked_output" | awk -F': ' '/^tracked_thread_id:/ {print $2; exit}')"

  printf '%s=%q\n' "${prefix}_repo_root" "$repo_root"
  printf '%s=%q\n' "${prefix}_worktree_path" "$worktree_path"
  printf '%s=%q\n' "${prefix}_branch_name" "$branch_name"
  printf '%s=%q\n' "${prefix}_workunit_id" "$workunit_id"
  printf '%s=%q\n' "${prefix}_tracked_thread_id" "$tracked_thread_id"
  printf '%s=%q\n' "${prefix}_expected_string" "$lane_expected"
}

run_lane() {
  local prefix="$1"
  local worker_name="$2"
  local worker_instructions="$3"
  local tracked_thread_id_var="${prefix}_tracked_thread_id"
  local workunit_id_var="${prefix}_workunit_id"
  local worktree_path_var="${prefix}_worktree_path"
  local branch_name_var="${prefix}_branch_name"
  local expected_string_var="${prefix}_expected_string"
  local repo_root_var="${prefix}_repo_root"
  local tracked_thread_id="${!tracked_thread_id_var}"
  local workunit_id="${!workunit_id_var}"
  local worktree_path="${!worktree_path_var}"
  local branch_name="${!branch_name_var}"
  local expected_string="${!expected_string_var}"
  local repo_root="${!repo_root_var}"
  local assignment_stdout="$reports_dir/${prefix}-assignment-start.txt"
  local report_get_stdout="$reports_dir/${prefix}-report-get.txt"
  local assignment_get_stdout="$reports_dir/${prefix}-assignment-get.txt"
  local make_test_stdout="$reports_dir/${prefix}-make-test.txt"
  local tree_diff_stdout="$reports_dir/${prefix}-tree-diff.txt"
  local git_status_stdout="$reports_dir/${prefix}-git-status.txt"
  local tracked_thread_bind_stdout="$reports_dir/${prefix}-tracked-thread-bind.txt"
  local tracked_thread_after_stdout="$reports_dir/${prefix}-tracked-thread-after.txt"
  local decision_stdout="$reports_dir/${prefix}-decision-complete.txt"
  local report_id assignment_id report_parse_result assignment_status worker_session_id thread_id decision_id work_unit_status
  local changed_count

  timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" assignments start \
    --workunit "$workunit_id" \
    --worker "$worker_name" \
    --worker-kind codex \
    --instructions "$worker_instructions" \
    --cwd "$worktree_path" \
    >"$assignment_stdout" 2>&1 &
  local assignment_pid=$!

  wait_for_report_id "$workunit_id" report_id

  e2e_orcas supervisor work reports get --report "$report_id" >"$report_get_stdout"
  assignment_id="$(field_value assignment_id "$report_get_stdout")"
  report_parse_result="$(field_value parse_result "$report_get_stdout")"

  e2e_orcas supervisor work assignments get --assignment "$assignment_id" >"$assignment_get_stdout"
  assignment_status="$(field_value status "$assignment_get_stdout")"
  worker_session_id="$(field_value worker_session_id "$assignment_get_stdout")"
  thread_id="$(field_value thread_id "$assignment_stdout")"

  test -n "$assignment_id"
  test -n "$worker_session_id"
  test -n "$thread_id"
  test -n "$report_parse_result"
  test "$assignment_status" = "AwaitingDecision"

  make -C "$worktree_path" test >"$make_test_stdout"
  make -C "$worktree_path" clean >/dev/null 2>&1 || true
  git -C "$worktree_path" status --short --untracked-files=all >"$git_status_stdout"
  diff -qr --exclude=.git "$fixture_dir" "$worktree_path" >"$tree_diff_stdout" || true

  changed_count="$(sed '/^$/d' "$tree_diff_stdout" | wc -l | tr -d ' ')"
  test "$changed_count" -eq 2
  grep -q 'main.c' "$tree_diff_stdout"
  grep -q 'tests/test.sh' "$tree_diff_stdout"
  grep -q '^PASS$' "$make_test_stdout"
  grep -q "Hello, ${expected_string}" "$worktree_path/main.c"
  grep -q "Hello, ${expected_string}" "$worktree_path/tests/test.sh"
  grep -q "report_id: $report_id" "$report_get_stdout"
  grep -q "assignment_id: $assignment_id" "$report_get_stdout"
  grep -q "work_unit_id: $workunit_id" "$report_get_stdout"

  e2e_orcas workunit thread set \
    --tracked-thread "$tracked_thread_id" \
    --upstream-thread "$thread_id" \
    --binding-state bound \
    >"$tracked_thread_bind_stdout"

  e2e_orcas workunit thread get --tracked-thread "$tracked_thread_id" >"$tracked_thread_after_stdout"
  grep -q "binding_state: Bound" "$tracked_thread_after_stdout"
  grep -q "workspace_worktree_path: $worktree_path" "$tracked_thread_after_stdout"
  grep -q "workspace_branch_name: $branch_name" "$tracked_thread_after_stdout"

  timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" decisions apply \
    --workunit "$workunit_id" \
    --report "$report_id" \
    --type mark-complete \
    --rationale "Close the concurrent lane after the bounded live worker turn landed cleanly." \
    >"$decision_stdout" 2>&1

  decision_id="$(field_value decision_id "$decision_stdout")"
  work_unit_status="$(field_value work_unit_status "$decision_stdout")"
  test -n "$decision_id"
  test "$work_unit_status" = "Completed"
  grep -q "decision_type: MarkComplete" "$decision_stdout"
  grep -q "work_unit_status: Completed" "$decision_stdout"

  wait "$assignment_pid" >/dev/null 2>&1 || true

  printf '%s=%q\n' "${prefix}_report_id" "$report_id"
  printf '%s=%q\n' "${prefix}_assignment_id" "$assignment_id"
  printf '%s=%q\n' "${prefix}_thread_id" "$thread_id"
  printf '%s=%q\n' "${prefix}_decision_id" "$decision_id"
  printf '%s=%q\n' "${prefix}_report_parse_result" "$report_parse_result"
}

short_xdg_root="$e2e_output_root/xdg/$E2E_RUN_ID/cln"
short_xdg_data_home="$short_xdg_root/data"
short_xdg_config_home="$short_xdg_root/config"
short_xdg_runtime_home="$short_xdg_root/runtime"
listen_port="$((6100 + ($(printf '%s' "$E2E_RUN_ID" | cksum | awk '{print $1}') % 1000)))"
listen_url="ws://127.0.0.1:$listen_port"
supervisor_base_url="${ORCAS_SUPERVISOR_BASE_URL:-http://127.0.0.1:8000/v1}"
supervisor_model="${ORCAS_SUPERVISOR_MODEL:-gpt-oss-20b}"
supervisor_api_key_env="${ORCAS_SUPERVISOR_API_KEY_ENV:-}"
supervisor_reasoning_effort="${ORCAS_SUPERVISOR_REASONING_EFFORT:-}"
supervisor_max_output_tokens="${ORCAS_SUPERVISOR_MAX_OUTPUT_TOKENS:-8192}"
supervisor_temperature="${ORCAS_SUPERVISOR_TEMPERATURE:-0.0}"
codex_bin="${ORCAS_CODEX_BIN:-$(command -v codex)}"

base_ref="${ORCAS_E2E_GIT_BASE_REF:-main}"
branch_suffix="${E2E_RUN_ID//[^a-zA-Z0-9]/-}"
daemon_log="$E2E_SCENARIO_LOGS_DIR/orcasd.log"
reports_dir="$E2E_SCENARIO_REPORTS_DIR"
artifacts_dir="$E2E_SCENARIO_ARTIFACTS_DIR"

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

e2e_orcas daemon start --force-spawn >"$daemon_log" 2>&1 &
daemon_pid=$!
cleanup() {
  e2e_orcas daemon stop >/dev/null 2>&1 || true
  kill "$daemon_pid" >/dev/null 2>&1 || true
  wait "$daemon_pid" >/dev/null 2>&1 || true
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
  "Hello, Lane A!")"

eval "$(setup_lane lane_b "$workstream_id" \
  "Lane B" \
  "Lane B: change the greeting to exactly 'Hello, Lane B!' by updating main.c and tests/test.sh only. Keep the change bounded to those two files and do not touch lane A." \
  "Hello, Lane B!")"

lane_a_tracked_before_stdout="$reports_dir/lane-a-tracked-thread-before.txt"
lane_b_tracked_before_stdout="$reports_dir/lane-b-tracked-thread-before.txt"
e2e_orcas workunit thread get --tracked-thread "$lane_a_tracked_thread_id" >"$lane_a_tracked_before_stdout"
e2e_orcas workunit thread get --tracked-thread "$lane_b_tracked_thread_id" >"$lane_b_tracked_before_stdout"

lane_a_assignment_start_stdout="$reports_dir/lane-a-assignment-start.txt"
lane_b_assignment_start_stdout="$reports_dir/lane-b-assignment-start.txt"

timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" assignments start \
  --workunit "$lane_a_workunit_id" \
  --worker live-concurrent-lanes-a \
  --worker-kind codex \
  --instructions "Lane A: update the tiny C fixture so make test passes with the exact greeting 'Hello, Lane A!'. Edit only main.c and tests/test.sh. Do not touch lane B or create backup files. Return a brief summary of the exact lane A edits." \
  --cwd "$lane_a_worktree_path" \
  >"$lane_a_assignment_start_stdout" 2>&1 &
lane_a_assignment_start_pid=$!

wait_for_report_id "$lane_a_workunit_id" lane_a_report_id

lane_a_report_get_stdout="$reports_dir/lane-a-report-get.txt"
lane_b_report_get_stdout="$reports_dir/lane-b-report-get.txt"
lane_a_assignment_get_stdout="$reports_dir/lane-a-assignment-get.txt"
lane_b_assignment_get_stdout="$reports_dir/lane-b-assignment-get.txt"
lane_a_make_test_stdout="$reports_dir/lane-a-make-test.txt"
lane_b_make_test_stdout="$reports_dir/lane-b-make-test.txt"
lane_a_tree_diff_stdout="$reports_dir/lane-a-tree-diff.txt"
lane_b_tree_diff_stdout="$reports_dir/lane-b-tree-diff.txt"
lane_a_git_status_stdout="$reports_dir/lane-a-git-status.txt"
lane_b_git_status_stdout="$reports_dir/lane-b-git-status.txt"
lane_a_tracked_after_stdout="$reports_dir/lane-a-tracked-thread-after.txt"
lane_b_tracked_after_stdout="$reports_dir/lane-b-tracked-thread-after.txt"
lane_a_decision_stdout="$reports_dir/lane-a-decision-complete.txt"
lane_b_decision_stdout="$reports_dir/lane-b-decision-complete.txt"

e2e_orcas supervisor work reports get --report "$lane_a_report_id" >"$lane_a_report_get_stdout"
lane_a_assignment_id="$(field_value assignment_id "$lane_a_report_get_stdout")"
lane_a_report_workunit_id="$(field_value work_unit_id "$lane_a_report_get_stdout")"
e2e_orcas supervisor work assignments get --assignment "$lane_a_assignment_id" >"$lane_a_assignment_get_stdout"
lane_a_assignment_status="$(field_value status "$lane_a_assignment_get_stdout")"
lane_a_worker_session_id="$(field_value worker_session_id "$lane_a_assignment_get_stdout")"
lane_a_report_parse_result="$(field_value parse_result "$lane_a_report_get_stdout")"
lane_a_thread_id="$(field_value thread_id "$lane_a_assignment_start_stdout")"

timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" assignments start \
  --workunit "$lane_b_workunit_id" \
  --worker live-concurrent-lanes-b \
  --worker-kind codex \
  --instructions "Lane B: update the tiny C fixture so make test passes with the exact greeting 'Hello, Lane B!'. Edit only main.c and tests/test.sh. Do not touch lane A or create backup files. Return a brief summary of the exact lane B edits." \
  --cwd "$lane_b_worktree_path" \
  >"$lane_b_assignment_start_stdout" 2>&1 &
lane_b_assignment_start_pid=$!

wait_for_report_id "$lane_b_workunit_id" lane_b_report_id

e2e_orcas supervisor work reports get --report "$lane_b_report_id" >"$lane_b_report_get_stdout"
lane_b_assignment_id="$(field_value assignment_id "$lane_b_report_get_stdout")"
lane_b_report_workunit_id="$(field_value work_unit_id "$lane_b_report_get_stdout")"
e2e_orcas supervisor work assignments get --assignment "$lane_b_assignment_id" >"$lane_b_assignment_get_stdout"
lane_b_assignment_status="$(field_value status "$lane_b_assignment_get_stdout")"
lane_b_worker_session_id="$(field_value worker_session_id "$lane_b_assignment_get_stdout")"
lane_b_report_parse_result="$(field_value parse_result "$lane_b_report_get_stdout")"
lane_b_thread_id="$(field_value thread_id "$lane_b_assignment_start_stdout")"

test -n "$lane_a_assignment_id"
test -n "$lane_b_assignment_id"
test -n "$lane_a_worker_session_id"
test -n "$lane_b_worker_session_id"
test -n "$lane_a_thread_id"
test -n "$lane_b_thread_id"
test -n "$lane_a_report_parse_result"
test -n "$lane_b_report_parse_result"
test "$lane_a_assignment_status" = "AwaitingDecision"
test "$lane_b_assignment_status" = "AwaitingDecision"
test "$lane_a_report_workunit_id" = "$lane_a_workunit_id"
test "$lane_b_report_workunit_id" = "$lane_b_workunit_id"

make -C "$lane_a_worktree_path" test >"$lane_a_make_test_stdout"
make -C "$lane_b_worktree_path" test >"$lane_b_make_test_stdout"
make -C "$lane_a_worktree_path" clean >/dev/null 2>&1 || true
make -C "$lane_b_worktree_path" clean >/dev/null 2>&1 || true

git -C "$lane_a_worktree_path" status --short --untracked-files=all >"$lane_a_git_status_stdout"
git -C "$lane_b_worktree_path" status --short --untracked-files=all >"$lane_b_git_status_stdout"
diff -qr --exclude=.git "$fixture_dir" "$lane_a_worktree_path" >"$lane_a_tree_diff_stdout" || true
diff -qr --exclude=.git "$fixture_dir" "$lane_b_worktree_path" >"$lane_b_tree_diff_stdout" || true

lane_a_changed_count="$(sed '/^$/d' "$lane_a_tree_diff_stdout" | wc -l | tr -d ' ')"
lane_b_changed_count="$(sed '/^$/d' "$lane_b_tree_diff_stdout" | wc -l | tr -d ' ')"
test "$lane_a_changed_count" -eq 2
test "$lane_b_changed_count" -eq 2
grep -q 'main.c' "$lane_a_tree_diff_stdout"
grep -q 'tests/test.sh' "$lane_a_tree_diff_stdout"
grep -q 'main.c' "$lane_b_tree_diff_stdout"
grep -q 'tests/test.sh' "$lane_b_tree_diff_stdout"
grep -q '^PASS$' "$lane_a_make_test_stdout"
grep -q '^PASS$' "$lane_b_make_test_stdout"
grep -q "Hello, Lane A!" "$lane_a_worktree_path/main.c"
grep -q "Hello, Lane A!" "$lane_a_worktree_path/tests/test.sh"
grep -q "Hello, Lane B!" "$lane_b_worktree_path/main.c"
grep -q "Hello, Lane B!" "$lane_b_worktree_path/tests/test.sh"
! grep -q "Hello, Lane B!" "$lane_a_worktree_path/main.c"
! grep -q "Hello, Lane A!" "$lane_b_worktree_path/main.c"

e2e_orcas workunit thread set \
  --tracked-thread "$lane_a_tracked_thread_id" \
  --upstream-thread "$lane_a_thread_id" \
  --binding-state bound \
  >"$reports_dir/lane-a-tracked-thread-bind.txt"
e2e_orcas workunit thread set \
  --tracked-thread "$lane_b_tracked_thread_id" \
  --upstream-thread "$lane_b_thread_id" \
  --binding-state bound \
  >"$reports_dir/lane-b-tracked-thread-bind.txt"

e2e_orcas workunit thread get --tracked-thread "$lane_a_tracked_thread_id" >"$lane_a_tracked_after_stdout"
e2e_orcas workunit thread get --tracked-thread "$lane_b_tracked_thread_id" >"$lane_b_tracked_after_stdout"
grep -q "binding_state: Bound" "$lane_a_tracked_after_stdout"
grep -q "binding_state: Bound" "$lane_b_tracked_after_stdout"
grep -q "workspace_worktree_path: $lane_a_worktree_path" "$lane_a_tracked_after_stdout"
grep -q "workspace_worktree_path: $lane_b_worktree_path" "$lane_b_tracked_after_stdout"
grep -q "workspace_branch_name: $lane_a_branch_name" "$lane_a_tracked_after_stdout"
grep -q "workspace_branch_name: $lane_b_branch_name" "$lane_b_tracked_after_stdout"
lane_b_thread_id="$(field_value thread_id "$lane_b_assignment_start_stdout")"

test -n "$lane_b_assignment_id"
test -n "$lane_b_worker_session_id"
test -n "$lane_b_thread_id"
test -n "$lane_b_report_parse_result"
test "$lane_b_assignment_status" = "AwaitingDecision"
test "$lane_b_report_workunit_id" = "$lane_b_workunit_id"

make -C "$lane_b_worktree_path" test >"$lane_b_make_test_stdout"
make -C "$lane_b_worktree_path" clean >/dev/null 2>&1 || true

git -C "$lane_b_worktree_path" status --short --untracked-files=all >"$lane_b_git_status_stdout"
diff -qr --exclude=.git "$fixture_dir" "$lane_b_worktree_path" >"$lane_b_tree_diff_stdout" || true

lane_b_changed_count="$(sed '/^$/d' "$lane_b_tree_diff_stdout" | wc -l | tr -d ' ')"
test "$lane_b_changed_count" -eq 2
grep -q 'main.c' "$lane_b_tree_diff_stdout"
grep -q 'tests/test.sh' "$lane_b_tree_diff_stdout"
grep -q '^PASS$' "$lane_b_make_test_stdout"
grep -q "Hello, Lane B!" "$lane_b_worktree_path/main.c"
grep -q "Hello, Lane B!" "$lane_b_worktree_path/tests/test.sh"
! grep -q "Hello, Lane A!" "$lane_b_worktree_path/main.c"

timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" decisions apply \
  --workunit "$lane_a_workunit_id" \
  --report "$lane_a_report_id" \
  --type mark-complete \
  --rationale "Close lane A after its bounded live worker turn landed cleanly." \
  >"$lane_a_decision_stdout" 2>&1
timeout "${TIMEOUT_SECONDS}s" "$e2e_bin_dir/orcas.sh" decisions apply \
  --workunit "$lane_b_workunit_id" \
  --report "$lane_b_report_id" \
  --type mark-complete \
  --rationale "Close lane B after its bounded live worker turn landed cleanly." \
  >"$lane_b_decision_stdout" 2>&1

lane_a_decision_id="$(field_value decision_id "$lane_a_decision_stdout")"
lane_b_decision_id="$(field_value decision_id "$lane_b_decision_stdout")"
lane_a_workunit_status="$(field_value work_unit_status "$lane_a_decision_stdout")"
lane_b_workunit_status="$(field_value work_unit_status "$lane_b_decision_stdout")"
test -n "$lane_a_decision_id"
test -n "$lane_b_decision_id"
test "$lane_a_workunit_status" = "Completed"
test "$lane_b_workunit_status" = "Completed"
grep -q "decision_type: MarkComplete" "$lane_a_decision_stdout"
grep -q "decision_type: MarkComplete" "$lane_b_decision_stdout"
grep -q "work_unit_status: Completed" "$lane_a_decision_stdout"
grep -q "work_unit_status: Completed" "$lane_b_decision_stdout"

test "$lane_a_tracked_thread_id" != "$lane_b_tracked_thread_id"
test "$lane_a_worktree_path" != "$lane_b_worktree_path"
test "$lane_a_branch_name" != "$lane_b_branch_name"
test "$lane_a_workunit_id" != "$lane_b_workunit_id"
test "$lane_a_assignment_id" != "$lane_b_assignment_id"
test "$lane_a_report_id" != "$lane_b_report_id"

wait "$lane_a_assignment_start_pid" >/dev/null 2>&1 || true
wait "$lane_b_assignment_start_pid" >/dev/null 2>&1 || true

echo "PASS"
