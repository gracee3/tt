#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "$0")/../../lib" && pwd)/common.sh"
scenario_dir="$(cd "$(dirname "$0")" && pwd)"
e2e_load_scenario_metadata "$scenario_dir"
e2e_prepare_scenario_dirs "$NAME"

daemon_log="$E2E_SCENARIO_LOGS_DIR/orcasd.log"
e2e_start_managed_daemon "$daemon_log"
cleanup() {
  e2e_stop_managed_daemon
}
trap cleanup EXIT

sleep 5

workstream_output="$(e2e_orcas workstreams create --title "E2E Planning" --objective "Validate supervisor planning flow" --priority normal)"
workstream_id="$(printf '%s\n' "$workstream_output" | awk -F': ' '/^workstream_id:/ {print $2; exit}')"

workunit_output="$(e2e_orcas workunit create --workstream "$workstream_id" --title "Planning work unit" --task "Draft a short implementation plan before execution")"
workunit_id="$(printf '%s\n' "$workunit_output" | awk -F': ' '/^work_unit_id:/ {print $2; exit}')"

planning_create_output="$E2E_SCENARIO_REPORTS_DIR/planning-create.txt"
e2e_orcas supervisor plan create \
  --workstream "$workstream_id" \
  --objective "Review the planning lane and decide whether bounded research is needed before implementation." \
  --open-question "What is the smallest coherent first implementation slice?" \
  --constraint "Stay strictly pre-execution." \
  --non-goal "Do not modify code in this planning session." \
  --draft-plan-summary "Draft a short implementation plan and identify whether one bounded research turn is needed." \
  --created-by harness \
  --request-note "Shared lab planning validation" \
  >"$planning_create_output"

planning_session_id="$(awk -F': ' '/^planning_session_id:/ {print $2; exit}' "$planning_create_output")"
planning_thread_id="$(awk -F': ' '/^planning_session_thread_id:/ {print $2; exit}' "$planning_create_output")"
planning_status="$(awk -F': ' '/^planning_session_status:/ {print $2; exit}' "$planning_create_output")"

test -n "$planning_session_id"
test -n "$planning_thread_id"
test -n "$planning_status"

e2e_orcas supervisor plan request-supervisor-context \
  --session "$planning_session_id" \
  --requested-by harness \
  --note "Prime the planning session for operator review." \
  >"$E2E_SCENARIO_REPORTS_DIR/planning-request-context.txt"

e2e_orcas supervisor plan mark-ready-for-review \
  --session "$planning_session_id" \
  --updated-by harness \
  --note "Planning summary is ready for approval." \
  >"$E2E_SCENARIO_REPORTS_DIR/planning-mark-ready.txt"

e2e_orcas supervisor plan approve \
  --session "$planning_session_id" \
  --approved-by harness \
  --review-note "Planning looks coherent" \
  >"$E2E_SCENARIO_REPORTS_DIR/planning-approve.txt"

e2e_orcas supervisor plan get --session "$planning_session_id" \
  >"$E2E_SCENARIO_REPORTS_DIR/planning-get.txt"

final_status="$(awk -F': ' '/^planning_session_status:/ {print $2; exit}' "$E2E_SCENARIO_REPORTS_DIR/planning-get.txt")"
approved_plan_revision_id="$(awk -F': ' '/^planning_revision_proposal_id:/ {print $2; exit}' "$E2E_SCENARIO_REPORTS_DIR/planning-approve.txt")"

test -n "$workstream_id"
test -n "$workunit_id"
test -n "$planning_session_id"
test -n "$planning_thread_id"
test "$final_status" = "Approved"
test -n "$approved_plan_revision_id"

echo "PASS"
