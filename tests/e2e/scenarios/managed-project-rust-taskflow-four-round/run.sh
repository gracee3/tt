#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "$0")/../../lib" && pwd)/common.sh"
scenario_dir="$(cd "$(dirname "$0")" && pwd)"

e2e_load_scenario_metadata "$scenario_dir"
e2e_prepare_scenario_dirs "$NAME"
e2e_prepare_live_tt_environment "mprt" 6400

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
  --objective "Build a seeded multi-round Rust workflow runner" \
  --template rust-taskflow \
  >"$init_stdout"

e2e_tt --cwd "$repo_root" project director \
  --scenario rust-taskflow-four-round \
  --seed-file "$seed_file" \
  >"$director_stdout"

e2e_tt --cwd "$repo_root" project inspect >"$inspect_stdout"

grep -q "kind: rust-taskflow-four-round" "$inspect_stdout"
grep -q "phase: completed" "$inspect_stdout"
grep -q "round: 4" "$inspect_stdout"
grep -q "completed: true" "$inspect_stdout"
grep -q "pending_approval: landing by director approved=true" "$inspect_stdout"
grep -q "director |" "$inspect_stdout"
grep -q "dev |" "$inspect_stdout"
grep -q "test |" "$inspect_stdout"
grep -q "integration |" "$inspect_stdout"

test -f "$repo_root/.tt/managed-project.toml"
test -f "$repo_root/.tt/contracts/worker-contract.md"
test -f "$repo_root/Cargo.toml"
test -f "$repo_root/src/main.rs"
test -f "$repo_root/src/lib.rs"
test -f "$repo_root/README.md"

scenario_id="$(sed -n 's/^id: //p' "$inspect_stdout" | head -n 1)"
scenario_root="$repo_root/.tt/scenarios/$scenario_id"
test -n "$scenario_id"
test -d "$scenario_root"

for round in 01 02 03 04; do
  test -f "$scenario_root/round-$round/director-prompt.txt"
  test -f "$scenario_root/round-$round/round-summary.md"
  for role in dev test integration; do
    handoff="$scenario_root/round-$round/$role-handoff.txt"
    prompt="$scenario_root/round-$round/$role-prompt.txt"
    test -f "$handoff"
    test -f "$prompt"
    grep -q '"status"' "$handoff"
    grep -q '"changed_files"' "$handoff"
    grep -q '"tests_run"' "$handoff"
    grep -q '"blockers"' "$handoff"
    grep -q '"next_step"' "$handoff"
  done
done

cargo test --quiet --manifest-path "$repo_root/Cargo.toml" >"$cargo_test_stdout"

echo "PASS"
