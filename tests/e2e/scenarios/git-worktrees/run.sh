#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "$0")/../../lib" && pwd)/common.sh"
scenario_dir="$(cd "$(dirname "$0")" && pwd)"
e2e_load_scenario_metadata "$scenario_dir"
e2e_prepare_scenario_dirs "$NAME"

scenario_name="git-worktrees"
base_ref="${ORCAS_E2E_GIT_BASE_REF:-$(git -C "$e2e_repo_root" symbolic-ref --quiet --short HEAD 2>/dev/null || echo main)}"
artifacts_dir="$E2E_SCENARIO_ARTIFACTS_DIR"
reports_dir="$E2E_SCENARIO_REPORTS_DIR"
worktree_path="$E2E_SCENARIO_WORKTREES_DIR/lane"
branch_name="orcas/$scenario_name/lane"
daemon_log="$E2E_SCENARIO_LOGS_DIR/orcasd.log"
prompt_dir="$artifacts_dir/prompts"

mkdir -p "$artifacts_dir" "$reports_dir" "$prompt_dir" "$(dirname "$worktree_path")"

e2e_orcas daemon start --force-spawn >"$daemon_log" 2>&1 &
daemon_pid=$!
cleanup() {
  kill "$daemon_pid" >/dev/null 2>&1 || true
}
trap cleanup EXIT

sleep 5

cat >"$prompt_dir/operator.txt" <<'EOF'
Declare a tracked-thread worktree, materialize it on disk, and build a tiny C program there.
EOF

cat >"$prompt_dir/supervisor.txt" <<'EOF'
Confirm the tracked-thread workspace contract is present, the worktree exists, and the compile/test artifacts are on disk.
EOF

cat >"$prompt_dir/agent.txt" <<'EOF'
Use the declared tracked-thread workspace path as the source of truth, create the worktree, add a tiny C program, and run the build.
EOF

workstream_output="$(e2e_orcas workstreams create \
  --title "E2E Worktrees" \
  --objective "Validate tracked-thread worktree creation and cleanup" \
  --priority normal)"
workstream_id="$(printf '%s\n' "$workstream_output" | awk -F': ' '/^workstream_id:/ {print $2; exit}')"

workunit_output="$(e2e_orcas workunits create \
  --workstream "$workstream_id" \
  --title "Tracked thread worktree lane" \
  --task "Prepare a dedicated tracked-thread worktree and write a small C program there")"
workunit_id="$(printf '%s\n' "$workunit_output" | awk -F': ' '/^work_unit_id:/ {print $2; exit}')"

tracked_output="$(e2e_orcas tracked-threads create \
  --workunit "$workunit_id" \
  --title "Worktree lane" \
  --root-dir "$e2e_repo_root" \
  --notes "Dedicated worktree lane for e2e validation" \
  --workspace-repository-root "$e2e_repo_root" \
  --workspace-worktree-path "$worktree_path" \
  --workspace-branch-name "$branch_name" \
  --workspace-base-ref "$base_ref" \
  --workspace-landing-target "$base_ref" \
  --workspace-strategy dedicated-thread-worktree \
  --workspace-landing-policy merge-to-main \
  --workspace-sync-policy rebase-before-completion \
  --workspace-cleanup-policy prune-after-merge \
  --workspace-status requested)"
tracked_thread_id="$(printf '%s\n' "$tracked_output" | awk -F': ' '/^tracked_thread_id:/ {print $2; exit}')"

e2e_orcas tracked-threads get --tracked-thread "$tracked_thread_id" >"$reports_dir/tracked-thread-before-write.txt"

test -n "$workstream_id"
test -n "$workunit_id"
test -n "$tracked_thread_id"
test -f "$prompt_dir/operator.txt"
test -f "$prompt_dir/supervisor.txt"
test -f "$prompt_dir/agent.txt"

rm -rf "$worktree_path"
git worktree add -b "$branch_name" "$worktree_path" "$base_ref" >"$reports_dir/git-worktree-add.txt"
test -d "$worktree_path"

cat >"$worktree_path/main.c" <<'EOF'
#include <stdio.h>

int main(void) {
    puts("Hello from the tracked-thread worktree.");
    return 0;
}
EOF

cat >"$worktree_path/Makefile" <<'EOF'
CC ?= cc
CFLAGS ?= -O2 -Wall -Wextra -pedantic

.PHONY: test clean

test: hello
	./hello

hello: main.c
	$(CC) $(CFLAGS) main.c -o hello

clean:
	rm -f hello
EOF

make -C "$worktree_path" test >"$reports_dir/build-and-test.txt"
git -C "$worktree_path" status --short >"$reports_dir/git-status.txt"
e2e_orcas tracked-threads get --tracked-thread "$tracked_thread_id" >"$reports_dir/tracked-thread-after-write.txt"

echo "PASS"
