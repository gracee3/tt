#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "$0")/../../lib" && pwd)/common.sh"
scenario_dir="$(cd "$(dirname "$0")" && pwd)"
e2e_load_scenario_metadata "$scenario_dir"
e2e_prepare_scenario_dirs "$NAME"

orcas_cli() {
  e2e_orcas --connect-only "$@"
}

cp "$scenario_dir/seed_state.json" "$E2E_SCENARIO_XDG_DATA_HOME/orcas/state.json"
rm -f "$E2E_SCENARIO_XDG_DATA_HOME/orcas/state.db" "$E2E_SCENARIO_XDG_DATA_HOME/orcas/state.db-wal" "$E2E_SCENARIO_XDG_DATA_HOME/orcas/state.db-shm"

daemon_log="$E2E_SCENARIO_LOGS_DIR/orcasd.log"
e2e_orcas daemon start --force-spawn >"$daemon_log" 2>&1 &
daemon_pid=$!
cleanup() {
  kill "$daemon_pid" >/dev/null 2>&1 || true
}
trap cleanup EXIT

sleep 5

workstream_id="ws-proposals"
workunit_id="wu-proposals"
report_id="report-proposals"
proposal_id="proposal-proposals"

orcas_cli proposals get --proposal "$proposal_id" >"$E2E_SCENARIO_REPORTS_DIR/proposal-get.txt"
approve_output="$(orcas_cli proposals approve --proposal "$proposal_id" --reviewed-by harness --review-note "Looks good" --rationale "Proposal is valid" --type accept)"
approved_decision_id="$(printf '%s\n' "$approve_output" | awk -F': ' '/^approved_decision_id:/ {print $2; exit}')"

test -n "$workstream_id"
test -n "$workunit_id"
test -n "$report_id"
test -n "$proposal_id"
test -n "$approved_decision_id"

echo "PASS"
