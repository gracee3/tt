#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "$0")/../../lib" && pwd)/common.sh"
scenario_dir="$(cd "$(dirname "$0")" && pwd)"

e2e_load_scenario_metadata "$scenario_dir"
e2e_prepare_scenario_dirs "$NAME"
e2e_prepare_live_tt_environment "mrip" 6500

repo_root="$E2E_SCENARIO_ARTIFACTS_DIR/taskflow-repo"
daemon_log="$E2E_SCENARIO_LOGS_DIR/tt-daemon.log"
reports_dir="$E2E_SCENARIO_REPORTS_DIR"
seed_file="$scenario_dir/taskflow-seed.toml"

mkdir -p "$reports_dir"
cargo build -q -p tt-cli -p tt-daemon

e2e_start_managed_daemon "$daemon_log"
cleanup() {
  e2e_stop_managed_daemon
}
trap cleanup EXIT

sleep 5

init_stdout="$reports_dir/project-init.txt"
inspect_stdout="$reports_dir/project-inspect.txt"
director_stdout="$reports_dir/project-director.txt"
cargo_test_stdout="$reports_dir/cargo-test.txt"

e2e_tt project init \
  --path "$repo_root" \
  --title "Taskflow" \
  --objective "Build a seeded Rust workflow runner under integration pressure" \
  --template rust-taskflow \
  >"$init_stdout"

e2e_tt --cwd "$repo_root" project director \
  --scenario rust-taskflow-integration-pressure \
  --seed-file "$seed_file" \
  >"$director_stdout"

e2e_tt --cwd "$repo_root" project inspect >"$inspect_stdout"

grep -q "kind: rust-taskflow-integration-pressure" "$inspect_stdout"
grep -q "phase: completed" "$inspect_stdout"
grep -q "round: 4" "$inspect_stdout"
grep -q "completed: true" "$inspect_stdout"
grep -q "pending_approval: landing by director approved=true" "$inspect_stdout"

scenario_id="$(sed -n 's/^id: //p' "$inspect_stdout" | head -n 1)"
scenario_root="$repo_root/.tt/scenarios/$scenario_id"
test -n "$scenario_id"
test -d "$scenario_root"

for round in 01 02 03 04; do
  test -f "$scenario_root/round-$round/director-prompt.txt"
  test -f "$scenario_root/round-$round/round-summary.md"
  grep -q "Round $((10#$round)) phase" "$scenario_root/round-$round/round-summary.md"
done

grep -q '"status": "blocked"' "$scenario_root/round-03/integration-handoff.txt"
grep -q 'merge-readiness is blocked until the report output path and retry example stay aligned across docs and CLI' "$scenario_root/round-03/integration-handoff.txt"
grep -q 'Resolve the integration mismatch, then return a merge-ready landing summary' "$scenario_root/round-03/integration-handoff.txt"
grep -q '"status": "complete"' "$scenario_root/round-04/integration-handoff.txt"
grep -q 'Land the branch set after operator approval' "$scenario_root/round-04/integration-handoff.txt"
grep -q '"cargo test"' "$scenario_root/round-04/test-handoff.txt"

cargo test --quiet --manifest-path "$repo_root/Cargo.toml" >"$cargo_test_stdout"

echo "PASS"
