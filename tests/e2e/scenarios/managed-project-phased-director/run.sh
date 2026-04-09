#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "$0")/../../lib" && pwd)/common.sh"
scenario_dir="$(cd "$(dirname "$0")" && pwd)"

e2e_load_scenario_metadata "$scenario_dir"
e2e_prepare_scenario_dirs "$NAME"
e2e_prepare_live_tt_environment "mpd" 6200

scenario_name="managed-project-phased-director"
base_ref="${TT_E2E_GIT_BASE_REF:-main}"
artifacts_dir="$E2E_SCENARIO_ARTIFACTS_DIR"
reports_dir="$E2E_SCENARIO_REPORTS_DIR"
repo_root="$E2E_SCENARIO_WORKTREES_DIR/${scenario_name}-repo"
worktree_path="$E2E_SCENARIO_WORKTREES_DIR/${scenario_name}-worktree"
branch_suffix="${E2E_RUN_ID//[^a-zA-Z0-9]/-}"
branch_name="tt/$scenario_name/$branch_suffix"
daemon_log="$E2E_SCENARIO_LOGS_DIR/tt-daemon.log"

mkdir -p "$artifacts_dir" "$reports_dir"
e2e_prepare_empty_repo_with_worktree "$repo_root" "$worktree_path" "$branch_name" "$base_ref" "$reports_dir" "repo"
cargo build -q -p tt-cli -p tt-daemon

e2e_start_managed_daemon "$daemon_log"
cleanup() {
  e2e_stop_managed_daemon
}
trap cleanup EXIT

sleep 5

open_stdout="$reports_dir/project-open.txt"
inspect_before_stdout="$reports_dir/project-inspect-before-director.txt"
status_before_stdout="$reports_dir/project-status-before-director.txt"
contract_path="$repo_root/.tt/contracts/worker-contract.md"
director_agent_path="$repo_root/.codex/agents/director.toml"
dev_agent_path="$repo_root/.codex/agents/dev.toml"
test_agent_path="$repo_root/.codex/agents/test.toml"
integration_agent_path="$repo_root/.codex/agents/integration.toml"
director_partial_stdout="$reports_dir/project-director-partial.txt"
inspect_partial_stdout="$reports_dir/project-inspect-after-partial.txt"
status_partial_stdout="$reports_dir/project-status-after-partial.txt"
director_final_stdout="$reports_dir/project-director-final.txt"
inspect_final_stdout="$reports_dir/project-inspect-after-final.txt"
status_final_stdout="$reports_dir/project-status-after-final.txt"
bindings_stdout="$reports_dir/thread-bindings.txt"
workspaces_stdout="$reports_dir/workspace-bindings.txt"

e2e_tt --cwd "$repo_root" project open \
  --title "Managed Project Phased Director" \
  --objective "Prove phased director-controlled role activation on a small git repo" \
  >"$open_stdout"

project_root="$repo_root"
if [ -f "$worktree_path/.tt/managed-project.toml" ]; then
  project_root="$worktree_path"
fi

contract_path="$project_root/.tt/contracts/worker-contract.md"
director_agent_path="$project_root/.codex/agents/director.toml"
dev_agent_path="$project_root/.codex/agents/dev.toml"
test_agent_path="$project_root/.codex/agents/test.toml"
integration_agent_path="$project_root/.codex/agents/integration.toml"

grep -q "The operator talks to the director." "$contract_path"
grep -q "Workers only communicate with the director." "$contract_path"
grep -q "## Phase Vocabulary" "$contract_path"
grep -q "director: coordinates the operator, plans the project, dispatches work, and owns handoffs." "$contract_path"
grep -q "dev: implements the assigned code slice only and reports concrete changes." "$contract_path"
grep -q "test: validates the assigned changes and reports exact failures." "$contract_path"
grep -q "integration: prepares landing, merge readiness, and cleanup." "$contract_path"
grep -q "Project protocol:" "$director_agent_path"
grep -q "The operator talks to the director." "$director_agent_path"
grep -q "Role roster:" "$director_agent_path"
grep -q "You report to the director, not to other workers or the operator." "$dev_agent_path"
grep -q "You report to the director, not to other workers or the operator." "$test_agent_path"
grep -q "You report to the director, not to other workers or the operator." "$integration_agent_path"

e2e_tt --cwd "$repo_root" project inspect >"$inspect_before_stdout"
e2e_tt --cwd "$repo_root" project status >"$status_before_stdout"

grep -q "state: scaffolded (0/4)" "$inspect_before_stdout"
grep -q "state: scaffolded (0/4)" "$status_before_stdout"

e2e_tt --cwd "$repo_root" project director \
  --role director \
  --role dev \
  --role test \
  >"$director_partial_stdout"

e2e_tt --cwd "$repo_root" project inspect >"$inspect_partial_stdout"
e2e_tt --cwd "$repo_root" project status >"$status_partial_stdout"

grep -q "state: partial (3/4)" "$inspect_partial_stdout"
grep -q "state: partial (3/4)" "$status_partial_stdout"
grep -q "director |" "$inspect_partial_stdout"
grep -q "dev |" "$inspect_partial_stdout"
grep -q "test |" "$inspect_partial_stdout"
grep -q "integration |" "$inspect_partial_stdout"
grep -Eq '^integration \|.*thread=<none>' "$inspect_partial_stdout"

e2e_tt --cwd "$repo_root" project director --role integration >"$director_final_stdout"
e2e_tt --cwd "$repo_root" project inspect >"$inspect_final_stdout"
e2e_tt --cwd "$repo_root" project status >"$status_final_stdout"
e2e_tt --cwd "$repo_root" records thread-binding list >"$bindings_stdout"
e2e_tt --cwd "$repo_root" workspace binding list >"$workspaces_stdout"

grep -q "state: attached (4/4)" "$inspect_final_stdout"
grep -q "state: attached (4/4)" "$status_final_stdout"
grep -q "director |" "$inspect_final_stdout"
grep -q "dev |" "$inspect_final_stdout"
grep -q "test |" "$inspect_final_stdout"
grep -q "integration |" "$inspect_final_stdout"
! grep -q 'thread=<none>' "$inspect_final_stdout"
test "$(sed '/^$/d' "$bindings_stdout" | wc -l | tr -d ' ')" -eq 4
test "$(sed '/^$/d' "$workspaces_stdout" | wc -l | tr -d ' ')" -eq 4
grep -q "Repository" "$inspect_final_stdout"

echo "PASS"
