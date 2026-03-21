#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")/lib" && pwd)/common.sh"

scenario_input="${1:-}"
[[ -n "$scenario_input" ]] || e2e_fail "usage: $0 <scenario-name-or-path>"

scenario_dir="$(e2e_resolve_scenario_dir "$scenario_input")"
[[ -d "$scenario_dir" ]] || e2e_fail "scenario not found: $scenario_dir"

e2e_load_scenario_metadata "$scenario_dir"
e2e_prepare_scenario_dirs "$NAME"

if e2e_is_true "$REQUIRES_CLEAN_GIT"; then
  e2e_require_clean_git
fi

if e2e_is_true "$REQUIRES_CODEX"; then
  e2e_require_codex
fi

e2e_print_scenario_begin
if "$scenario_dir/run.sh"; then
  status=0
else
  status=$?
fi
e2e_print_scenario_end "$status"
exit "$status"
