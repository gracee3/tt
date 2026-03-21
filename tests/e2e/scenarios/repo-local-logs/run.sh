#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "$0")/../../lib" && pwd)/common.sh"
scenario_dir="$(cd "$(dirname "$0")" && pwd)"
e2e_load_scenario_metadata "$scenario_dir"
e2e_prepare_scenario_dirs "$NAME"

daemon_log="$E2E_SCENARIO_LOGS_DIR/orcasd.log"

e2e_orcas daemon start --force-spawn >"$daemon_log" 2>&1 &
daemon_pid=$!
cleanup() {
  kill "$daemon_pid" >/dev/null 2>&1 || true
}
trap cleanup EXIT

sleep 5

e2e_orcas doctor >/dev/null

log_dir="$E2E_SCENARIO_XDG_DATA_HOME/orcas/logs"
test -d "$log_dir"
test -f "$log_dir/orcasd.log"
test -f "$log_dir/orcas.log"

echo "PASS"
