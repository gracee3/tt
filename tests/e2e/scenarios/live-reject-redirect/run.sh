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

short_xdg_root="$e2e_output_root/xdg/$E2E_RUN_ID/lrr"
short_xdg_data_home="$short_xdg_root/data"
short_xdg_config_home="$short_xdg_root/config"
short_xdg_runtime_home="$short_xdg_root/runtime"
listen_port="$((5600 + ($(printf '%s' "$E2E_RUN_ID" | cksum | awk '{print $1}') % 1000)))"
listen_url="ws://127.0.0.1:$listen_port"
supervisor_base_url="${ORCAS_SUPERVISOR_BASE_URL:-http://127.0.0.1:8000/v1}"
supervisor_model="${ORCAS_SUPERVISOR_MODEL:-gpt-oss-20b}"
supervisor_api_key_env="${ORCAS_SUPERVISOR_API_KEY_ENV:-}"
supervisor_reasoning_effort="${ORCAS_SUPERVISOR_REASONING_EFFORT:-}"
supervisor_max_output_tokens="${ORCAS_SUPERVISOR_MAX_OUTPUT_TOKENS:-16384}"

if ! e2e_using_shared_lab; then
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
fi

fixture_repo="$E2E_SCENARIO_WORKTREES_DIR/lane"
daemon_log="$E2E_SCENARIO_LOGS_DIR/orcasd.log"
reports_dir="$E2E_SCENARIO_REPORTS_DIR"
artifacts_dir="$E2E_SCENARIO_ARTIFACTS_DIR"

rm -rf "$fixture_repo"
mkdir -p "$fixture_repo" "$reports_dir" "$artifacts_dir"
cp -R "$fixture_dir/." "$fixture_repo/"

e2e_start_managed_daemon "$daemon_log"
cleanup() {
  e2e_stop_managed_daemon
}
trap cleanup EXIT

sleep 5

workstream_output="$(
  e2e_orcas workstreams create \
    --title "Live reject redirect" \
    --objective "Prove the governed live worker-to-supervisor loop on one bounded change" \
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
  --worker live-reject-redirect-worker \
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
proposal_create_stdout="$reports_dir/proposal-create.txt"
proposal_get_stdout="$reports_dir/proposal-get.txt"
proposal_summary_stdout="$reports_dir/proposal-artifact-summary.txt"
proposal_approve_stdout="$reports_dir/proposal-approve.txt"
next_assignment_get_stdout="$reports_dir/next-assignment-get.txt"
make_test_stdout="$reports_dir/make-test.txt"
tree_diff_stdout="$reports_dir/tree-diff.txt"

e2e_orcas supervisor work reports get --report "$report_id" >"$report_get_stdout"
assignment_id="$(field_value assignment_id "$report_get_stdout")"
report_parse_result="$(field_value parse_result "$report_get_stdout")"

e2e_orcas supervisor work assignments get --assignment "$assignment_id" >"$assignment_get_stdout"
assignment_status="$(field_value status "$assignment_get_stdout")"
worker_session_id="$(field_value worker_session_id "$assignment_get_stdout")"

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
grep -q "assignment_id: $assignment_id" "$assignment_get_stdout"
grep -q "report_id: $report_id" "$report_get_stdout"
grep -q "assignment_id: $assignment_id" "$report_get_stdout"
grep -q "work_unit_id: $workunit_id" "$report_get_stdout"
grep -q "status: AwaitingDecision" "$assignment_get_stdout"
grep -Eq "parse_result: (Parsed|Ambiguous)" "$report_get_stdout"

proposal_create_output="$(
  e2e_orcas supervisor work proposals create \
    --workunit "$workunit_id" \
    --report "$report_id" \
    --requested-by live-reject-redirect \
    --note "Generate a bounded continue proposal for one tiny follow-up test on the greeting fix. Keep every field terse. Use exactly 2 instructions, exactly 2 acceptance criteria, exactly 2 stop conditions, exactly 2 expected report fields, and a concise boundedness note. Set plan_assessment and plan_revision_proposal to null. Do not escalate or mark the work complete." \
  | tee "$proposal_create_stdout"
)"
proposal_id="$(printf '%s\n' "$proposal_create_output" | awk -F': ' '/^proposal_id:/ {print $2; exit}')"

e2e_orcas supervisor work proposals get --proposal "$proposal_id" >"$proposal_get_stdout"
proposal_status="$(field_value status "$proposal_get_stdout")"
model_summary_headline="$(field_value model_summary_headline "$proposal_get_stdout")"
model_proposed_decision_type="$(field_value model_proposed_decision_type "$proposal_get_stdout")"
model_draft_assignment_objective="$(field_value model_draft_assignment_objective "$proposal_get_stdout")"
source_report_id="$(field_value source_report_id "$proposal_get_stdout")"

test -n "$proposal_id"
test "$proposal_status" = "Open"
test -n "$model_summary_headline"
test -n "$model_proposed_decision_type"
test -n "$model_draft_assignment_objective"
test "$source_report_id" = "$report_id"
grep -q "work_unit_id: $workunit_id" "$proposal_get_stdout"
grep -q "status: Open" "$proposal_get_stdout"
grep -q "^model_summary_headline:" "$proposal_get_stdout"
grep -q "^model_summary_situation:" "$proposal_get_stdout"
grep -q "^model_proposed_decision_type:" "$proposal_get_stdout"
grep -q "^model_requires_assignment:" "$proposal_get_stdout"
grep -q "^model_draft_assignment_objective:" "$proposal_get_stdout"

e2e_orcas supervisor work proposals artifact-summary --proposal "$proposal_id" >"$proposal_summary_stdout"
grep -q '^prompt_artifact_present:' "$proposal_summary_stdout"
grep -q '^response_artifact_present:' "$proposal_summary_stdout"

proposal_approve_output="$(
  e2e_orcas supervisor work proposals approve \
    --proposal "$proposal_id" \
    --reviewed-by live-reject-redirect \
    --review-note "Redirect this into a test-only follow-up that stays smaller than the original proposal." \
    --type redirect \
    --objective "Add one regression test file that checks the exact greeting string and keep the code change bounded." \
    --instruction "Add the narrow regression test only." \
    --instruction "Do not change main.c or broaden beyond one file if possible." \
    --acceptance "The next assignment is limited to a test-only follow-up." \
    --acceptance "The redirect is clearly operator-authored and bounded." \
    --stop-condition "Stop if the redirect would require unrelated refactoring." \
    --stop-condition "Stop if more than one file would be required." \
    --expected-report-field summary \
    --expected-report-field findings \
  | tee "$proposal_approve_stdout"
)"
decision_id="$(printf '%s\n' "$proposal_approve_output" | awk -F': ' '/^decision_id:/ {print $2; exit}')"
next_assignment_id="$(printf '%s\n' "$proposal_approve_output" | awk -F': ' '/^next_assignment_id:/ {print $2; exit}')"

test -n "$decision_id"
test -n "$next_assignment_id"
grep -q "proposal_id: $proposal_id" "$proposal_approve_stdout"
grep -q "status: Approved" "$proposal_approve_stdout"
grep -q "decision_type: Redirect" "$proposal_approve_stdout"
grep -q "next_assignment_id: $next_assignment_id" "$proposal_approve_stdout"

e2e_orcas supervisor work proposals get --proposal "$proposal_id" >"$proposal_get_stdout"
approved_draft_assignment_objective="$(field_value approved_draft_assignment_objective "$proposal_get_stdout")"
grep -q "status: Approved" "$proposal_get_stdout"
grep -q "approved_decision_id: $decision_id" "$proposal_get_stdout"
grep -q "approved_assignment_id: $next_assignment_id" "$proposal_get_stdout"
grep -q "approval_edits_present: true" "$proposal_get_stdout"
grep -q "approval_edit_decision_type: Redirect" "$proposal_get_stdout"
grep -q "approval_edit_objective: Add one regression test file that checks the exact greeting string and keep the code change bounded." "$proposal_get_stdout"
grep -q "approved_proposed_decision_type: Redirect" "$proposal_get_stdout"
grep -q "approved_draft_assignment_objective: Add one regression test file that checks the exact greeting string and keep the code change bounded." "$proposal_get_stdout"
grep -q "approved_draft_assignment_derived_from_decision_type: Redirect" "$proposal_get_stdout"

test "$approved_draft_assignment_objective" = "Add one regression test file that checks the exact greeting string and keep the code change bounded."
test "$approved_draft_assignment_objective" != "$model_draft_assignment_objective"

e2e_orcas supervisor work assignments get --assignment "$next_assignment_id" >"$next_assignment_get_stdout"
next_assignment_work_unit_id="$(field_value work_unit_id "$next_assignment_get_stdout")"
next_assignment_status="$(field_value status "$next_assignment_get_stdout")"
next_assignment_attempt="$(field_value attempt "$next_assignment_get_stdout")"

test "$next_assignment_work_unit_id" = "$workunit_id"
test "$next_assignment_status" = "Created"
test "$next_assignment_attempt" = "2"
grep -q "work_unit_id: $workunit_id" "$next_assignment_get_stdout"
grep -q "status: Created" "$next_assignment_get_stdout"
grep -q "attempt: 2" "$next_assignment_get_stdout"

wait "$assignment_start_pid" >/dev/null 2>&1 || true

echo "PASS"
