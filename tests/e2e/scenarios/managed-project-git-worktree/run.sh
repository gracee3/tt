#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "$0")/../../lib" && pwd)/common.sh"
scenario_dir="$(cd "$(dirname "$0")" && pwd)"

e2e_load_scenario_metadata "$scenario_dir"
e2e_prepare_scenario_dirs "$NAME"
e2e_prepare_live_tt_environment "mgtw" 6300

scenario_name="managed-project-git-worktree"
base_ref="${TT_E2E_GIT_BASE_REF:-main}"
artifacts_dir="$E2E_SCENARIO_ARTIFACTS_DIR"
reports_dir="$E2E_SCENARIO_REPORTS_DIR"
repo_root="$E2E_SCENARIO_WORKTREES_DIR/${scenario_name}-repo"
worktree_path="$E2E_SCENARIO_WORKTREES_DIR/${scenario_name}-worktree"
branch_suffix="${E2E_RUN_ID//[^a-zA-Z0-9]/-}"
branch_name="tt/$scenario_name/$branch_suffix"
daemon_log="$E2E_SCENARIO_LOGS_DIR/tt-daemon.log"
app_server_log="$E2E_SCENARIO_LOGS_DIR/codex-app-server.log"

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
contract_path="$repo_root/.tt/contracts/worker-contract.md"
director_agent_path="$repo_root/.codex/agents/director.toml"
dev_agent_path="$repo_root/.codex/agents/dev.toml"
test_agent_path="$repo_root/.codex/agents/test.toml"
integration_agent_path="$repo_root/.codex/agents/integration.toml"
director_stdout="$reports_dir/project-director.txt"
inspect_after_worktree_stdout="$reports_dir/project-inspect-after-worktree.txt"
status_after_worktree_stdout="$reports_dir/project-status-after-worktree.txt"
repo_after_worktree_stdout="$reports_dir/repo-after-worktree.txt"

e2e_tt --cwd "$worktree_path" project open \
  --title "Managed Project Git Worktree" \
  --objective "Prove managed-project commands work from a linked child worktree path" \
  >"$open_stdout"

project_root="$repo_root"
if [ -f "$worktree_path/.tt/state.toml" ]; then
  project_root="$worktree_path"
fi

contract_path="$project_root/.tt/contracts/worker-contract.md"
director_agent_path="$project_root/.codex/agents/director.toml"
dev_agent_path="$project_root/.codex/agents/dev.toml"
test_agent_path="$project_root/.codex/agents/test.toml"
integration_agent_path="$project_root/.codex/agents/integration.toml"

e2e_start_codex_app_server_for_repo "$project_root" "$app_server_log"

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
test -f "$project_root/.tt/project.toml"
test -f "$project_root/.tt/plan.toml"

e2e_tt --cwd "$worktree_path" project inspect >"$inspect_before_stdout"
grep -q "state: scaffolded (0/4)" "$inspect_before_stdout"

e2e_tt --cwd "$worktree_path" project director >"$director_stdout"

e2e_tt --cwd "$worktree_path" project inspect >"$inspect_after_worktree_stdout"
e2e_tt --cwd "$worktree_path" project status >"$status_after_worktree_stdout"
e2e_tt --cwd "$worktree_path" repo >"$repo_after_worktree_stdout"

grep -q "state: attached (4/4)" "$inspect_after_worktree_stdout"
grep -q "state: attached (4/4)" "$status_after_worktree_stdout"
grep -q "Repository" "$inspect_after_worktree_stdout"
grep -q "repository" "$repo_after_worktree_stdout"
! grep -q 'thread=<none>' "$inspect_after_worktree_stdout"

echo "PASS"
