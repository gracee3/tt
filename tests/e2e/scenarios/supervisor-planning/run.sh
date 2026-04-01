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

workunit_output="$(e2e_orcas workunits create --workstream "$workstream_id" --title "Planning work unit" --task "Draft a short implementation plan before execution")"
workunit_id="$(printf '%s\n' "$workunit_output" | awk -F': ' '/^work_unit_id:/ {print $2; exit}')"

assignment_start_log="$E2E_SCENARIO_REPORTS_DIR/assignment-start.txt"
e2e_orcas assignments start --workunit "$workunit_id" --worker harness-worker --worker-kind harness --instructions "Draft a planning report with at least two steps." --cwd "$e2e_repo_root" >"$assignment_start_log" 2>&1 &
assignment_start_pid=$!
assignment_start_cleanup() {
  kill "$assignment_start_pid" >/dev/null 2>&1 || true
}
trap assignment_start_cleanup EXIT
sleep 5
report_id=""

if [[ -z "$report_id" ]]; then
  for _ in $(seq 1 20); do
    reports_output="$(e2e_orcas reports list-for-workunit --workunit "$workunit_id" 2>/dev/null || true)"
    report_id="$(printf '%s\n' "$reports_output" | awk -F'\t' '/^[0-9a-f]/ {print $1; exit}')"
    [[ -n "$report_id" ]] && break
    sleep 2
  done
fi

e2e_orcas workunits edit --workunit "$workunit_id" --status awaiting-decision >"$E2E_SCENARIO_REPORTS_DIR/workunit-edit.txt"

proposal_output="$(e2e_orcas proposals create --workunit "$workunit_id" --report "$report_id" --requested-by harness --note "Draft a two-step plan before execution.")"
proposal_id="$(printf '%s\n' "$proposal_output" | awk -F': ' '/^proposal_id:/ {print $2; exit}')"

e2e_orcas proposals get --proposal "$proposal_id" >"$E2E_SCENARIO_REPORTS_DIR/proposal-get.txt"
approve_output="$(e2e_orcas proposals approve --proposal "$proposal_id" --reviewed-by harness --review-note "Planning looks coherent" --rationale "Plan is inspectable and actionable" --type continue)"
approved_decision_id="$(printf '%s\n' "$approve_output" | awk -F': ' '/^approved_decision_id:/ {print $2; exit}')"

test -n "$workstream_id"
test -n "$workunit_id"
test -n "$report_id"
test -n "$proposal_id"
test -n "$approved_decision_id"

wait "$assignment_start_pid" >/dev/null 2>&1 || true

echo "PASS"
