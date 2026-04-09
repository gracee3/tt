#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "$0")/../../lib" && pwd)/common.sh"
scenario_dir="$(cd "$(dirname "$0")" && pwd)"

e2e_load_scenario_metadata "$scenario_dir"
e2e_prepare_scenario_dirs "$NAME"
e2e_prepare_live_tt_environment "mrip" 6500

repo_root="$E2E_SCENARIO_ARTIFACTS_DIR/taskflow-repo"
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
inspect_stdout="$reports_dir/project-inspect.txt"
plan_stdout="$reports_dir/project-plan.txt"
plan_refresh_stdout="$reports_dir/project-plan-refresh.txt"
director_stdout="$reports_dir/project-director.txt"
cargo_test_stdout="$reports_dir/cargo-test.txt"

e2e_tt project init \
  --path "$repo_root" \
  --title "Taskflow" \
  --objective "Build a seeded Rust workflow runner under integration pressure" \
  --template rust-taskflow \
  >"$init_stdout"

e2e_start_codex_app_server_for_repo "$repo_root" "$app_server_log"

e2e_tt --cwd "$repo_root" project director \
  --scenario rust-taskflow-integration-pressure \
  --seed-file "$seed_file" \
  >"$director_stdout"

e2e_tt --cwd "$repo_root" project inspect >"$inspect_stdout"
e2e_tt --cwd "$repo_root" project plan show >"$plan_stdout"
e2e_tt --cwd "$repo_root" project plan refresh >"$plan_refresh_stdout"

grep -q "kind: rust-taskflow-integration-pressure" "$inspect_stdout"
grep -q "phase: completed" "$inspect_stdout"
grep -q "round: 4" "$inspect_stdout"
grep -q "completed: true" "$inspect_stdout"
grep -q "pending_approval: landing by director approved=true" "$inspect_stdout"
grep -q "fallback_handoffs:" "$inspect_stdout"
grep -q "liveness_policy:" "$inspect_stdout"
grep -q "progress_stream:" "$inspect_stdout"
grep -q "progress_events:" "$inspect_stdout"
grep -q "watchdog: state=" "$inspect_stdout"
grep -q "latest_round_summary: round 4 merge" "$inspect_stdout"
grep -q "managed project plan" "$plan_stdout"
grep -q "Plan file: $repo_root/.tt/plan.toml" "$plan_stdout"
grep -q "managed project" "$plan_refresh_stdout"

scenario_id="$(sed -n 's/^id: //p' "$inspect_stdout" | head -n 1)"
scenario_root="$repo_root/.tt/scenarios/$scenario_id"
test -n "$scenario_id"
test -d "$scenario_root"
test -f "$scenario_root/progress.jsonl"
grep -q '"event":"scenario-start"' "$scenario_root/progress.jsonl"
grep -q '"event":"worker-dispatch"' "$scenario_root/progress.jsonl"
grep -q '"event":"watchdog-progress"' "$scenario_root/progress.jsonl"
grep -q '"event":"round-summary"' "$scenario_root/progress.jsonl"
grep -q '"event":"scenario-complete"' "$scenario_root/progress.jsonl"

for round in 01 02 03 04; do
  test -f "$scenario_root/round-$round/director-prompt.txt"
  test -f "$scenario_root/round-$round/round-summary.md"
  grep -q "Round $((10#$round)) phase" "$scenario_root/round-$round/round-summary.md"
  for role in dev test integration; do
    test -f "$scenario_root/round-$round/$role-handoff-source.txt"
    test -f "$scenario_root/round-$round/$role-watchdog.txt"
    grep -Eq '^(extracted|seeded_fallback)$' "$scenario_root/round-$round/$role-handoff-source.txt"
    grep -q '^state: ' "$scenario_root/round-$round/$role-watchdog.txt"
  done
done

test -f "$repo_root/.tt/state.toml"
test -f "$repo_root/.tt/project.toml"
test -f "$repo_root/.tt/plan.toml"

if e2e_is_true "$REQUIRES_EXTRACTED_HANDOFFS"; then
  e2e_require_extracted_handoffs "$inspect_stdout" "$scenario_root"
fi

grep -q '"status": "complete"' "$scenario_root/round-03/integration-handoff.txt"
grep -q 'Request merge/landing approval from the director after the final review confirms CLI, docs, examples, and report schema are aligned.' "$scenario_root/round-03/integration-handoff.txt"
grep -q '"status": "blocked"' "$scenario_root/round-04/test-handoff.txt"
grep -q 'This worktree is still the initial scaffold (ada2b9c)' "$scenario_root/round-04/test-handoff.txt"
grep -q 'Switch to the integrated taskflow revision/worktree and rerun `cargo test` there' "$scenario_root/round-04/test-handoff.txt"
grep -q '"status": "complete"' "$scenario_root/round-04/integration-handoff.txt"
grep -q 'Request merge/landing approval from the director after the final review confirms CLI, docs, examples, and report schema are aligned.' "$scenario_root/round-04/integration-handoff.txt"

cargo test --quiet --manifest-path "$repo_root/Cargo.toml" >"$cargo_test_stdout"

echo "PASS"
