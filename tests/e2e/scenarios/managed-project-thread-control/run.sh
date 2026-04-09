#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "$0")/../../lib" && pwd)/common.sh"
scenario_dir="$(cd "$(dirname "$0")" && pwd)"

e2e_load_scenario_metadata "$scenario_dir"
e2e_prepare_scenario_dirs "$NAME"
e2e_prepare_live_tt_environment "mptc" 6600

repo_root="$E2E_SCENARIO_ARTIFACTS_DIR/taskflow-control-repo"
daemon_log="$E2E_SCENARIO_LOGS_DIR/tt-daemon.log"
app_server_log="$E2E_SCENARIO_LOGS_DIR/codex-app-server.log"
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
pause_control_stdout="$reports_dir/project-control-pause.txt"
inspect_control_stdout="$reports_dir/project-inspect-control.txt"
pause_director_stdout="$reports_dir/project-director-pause.txt"
inspect_paused_stdout="$reports_dir/project-inspect-paused.txt"
resume_control_stdout="$reports_dir/project-control-resume.txt"
inspect_resume_stdout="$reports_dir/project-inspect-resume.txt"
resume_director_stdout="$reports_dir/project-director-resume.txt"
inspect_final_stdout="$reports_dir/project-inspect-final.txt"

e2e_tt project init \
  --path "$repo_root" \
  --title "Taskflow Control Demo" \
  --objective "Demonstrate per-thread manual takeover and director resume" \
  --template rust-taskflow \
  >"$init_stdout"

e2e_start_codex_app_server_for_repo "$repo_root" "$app_server_log"

e2e_tt --cwd "$repo_root" project control \
  --role test \
  --mode manual_next_turn \
  >"$pause_control_stdout"

e2e_tt --cwd "$repo_root" project inspect >"$inspect_control_stdout"

grep -q "test |" "$inspect_control_stdout"
grep -q "control=manual_next_turn" "$inspect_control_stdout"
grep -q "kind: rust-taskflow-four-round" "$inspect_control_stdout"

e2e_tt --cwd "$repo_root" project director \
  --scenario rust-taskflow-four-round \
  --seed-file "$seed_file" \
  >"$pause_director_stdout"

e2e_tt --cwd "$repo_root" project inspect >"$inspect_paused_stdout"

grep -q "kind: rust-taskflow-four-round" "$inspect_paused_stdout"
grep -q "phase: manual-override-test" "$inspect_paused_stdout"
grep -q "completed: false" "$inspect_paused_stdout"
grep -q "test |" "$inspect_paused_stdout"
grep -q "control=manual" "$inspect_paused_stdout"
grep -q "latest_round_summary: round 1 plan" "$inspect_paused_stdout"

scenario_id="$(sed -n 's/^id: //p' "$inspect_paused_stdout" | head -n 1)"
scenario_root="$repo_root/.tt/scenarios/$scenario_id"
test -n "$scenario_id"
test -d "$scenario_root"
test -f "$scenario_root/progress.jsonl"
grep -q '"event":"manual-takeover-pending"' "$scenario_root/progress.jsonl"

e2e_tt --cwd "$repo_root" project control \
  --role test \
  --mode director \
  >"$resume_control_stdout"

e2e_tt --cwd "$repo_root" project inspect >"$inspect_resume_stdout"

grep -q "test |" "$inspect_resume_stdout"
grep -q "control=director" "$inspect_resume_stdout"
grep -q "kind: rust-taskflow-four-round" "$inspect_resume_stdout"

e2e_tt --cwd "$repo_root" project director \
  --scenario rust-taskflow-four-round \
  --seed-file "$seed_file" \
  >"$resume_director_stdout"

e2e_tt --cwd "$repo_root" project inspect >"$inspect_final_stdout"

grep -q "kind: rust-taskflow-four-round" "$inspect_final_stdout"
grep -q "phase: completed" "$inspect_final_stdout"
grep -q "round: 4" "$inspect_final_stdout"
grep -q "completed: true" "$inspect_final_stdout"
grep -q "control=director" "$inspect_final_stdout"
grep -q "fallback_handoffs:" "$inspect_final_stdout"
grep -q "progress_stream:" "$inspect_final_stdout"
grep -q "progress_events:" "$inspect_final_stdout"
grep -q '"event":"scenario-complete"' "$scenario_root/progress.jsonl"

test -f "$repo_root/.tt/managed-project.toml"
test -f "$repo_root/.tt/project.toml"
test -f "$repo_root/.tt/plan.toml"
test -f "$repo_root/.tt/contracts/worker-contract.md"
test -f "$repo_root/Cargo.toml"
test -f "$repo_root/src/main.rs"
test -f "$repo_root/src/lib.rs"
test -f "$repo_root/README.md"

echo "PASS"
