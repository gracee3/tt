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

mkdir -p "$reports_dir"
cargo build -q -p tt-cli -p tt-daemon

e2e_start_managed_daemon "$daemon_log"
cleanup() {
  e2e_stop_managed_daemon
}
trap cleanup EXIT

sleep 5

init_stdout="$reports_dir/project-init.txt"
spawn_stdout="$reports_dir/project-spawn.txt"
inspect_spawn_stdout="$reports_dir/project-inspect-spawn.txt"
pause_control_stdout="$reports_dir/project-control-pause.txt"
inspect_control_stdout="$reports_dir/project-inspect-control.txt"
resume_control_stdout="$reports_dir/project-control-resume.txt"
inspect_resume_stdout="$reports_dir/project-inspect-resume.txt"

e2e_tt project init \
  --path "$repo_root" \
  --title "Taskflow Control Demo" \
  --objective "Demonstrate per-thread manual takeover and director resume" \
  --template rust-taskflow \
  >"$init_stdout"

e2e_start_codex_app_server_for_repo "$repo_root" "$app_server_log"

e2e_tt --cwd "$repo_root" project spawn \
  --role test \
  >"$spawn_stdout"

e2e_tt --cwd "$repo_root" project inspect >"$inspect_spawn_stdout"

grep -q "state: partial" "$inspect_spawn_stdout"
grep -q "test |" "$inspect_spawn_stdout"
grep -Eq 'thread=[^<]' "$inspect_spawn_stdout"
grep -q "control=director" "$inspect_spawn_stdout"

e2e_tt --cwd "$repo_root" project control \
  --role test \
  --mode manual_next_turn \
  >"$pause_control_stdout"

e2e_tt --cwd "$repo_root" project inspect >"$inspect_control_stdout"

grep -q "test |" "$inspect_control_stdout"
grep -q "control=manual_next_turn" "$inspect_control_stdout"
grep -q "state: partial" "$inspect_control_stdout"

e2e_tt --cwd "$repo_root" project control \
  --role test \
  --mode director \
  >"$resume_control_stdout"

e2e_tt --cwd "$repo_root" project inspect >"$inspect_resume_stdout"

grep -q "test |" "$inspect_resume_stdout"
grep -q "control=director" "$inspect_resume_stdout"
grep -q "state: partial" "$inspect_resume_stdout"

test -f "$repo_root/.tt/state.toml"
test -f "$repo_root/.tt/project.toml"
test -f "$repo_root/.tt/plan.toml"
test -f "$repo_root/.tt/contracts/worker-contract.md"
test -f "$repo_root/Cargo.toml"
test -f "$repo_root/src/main.rs"
test -f "$repo_root/src/lib.rs"
test -f "$repo_root/README.md"

echo "PASS"
