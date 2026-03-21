#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")/lib" && pwd)/common.sh"

passed=0
failed=0
skipped=0
selected=0

mapfile -t scenario_dirs < <(e2e_list_scenario_dirs)
for scenario_dir in "${scenario_dirs[@]}"; do
  e2e_load_scenario_metadata "$scenario_dir"
  if ! e2e_scenario_matches_filters "$scenario_dir"; then
    ((skipped += 1))
    continue
  fi

  ((selected += 1))
  if "$e2e_tests_root/run_scenario.sh" "$scenario_dir"; then
    ((passed += 1))
  else
    failed=1
    break
  fi
done

if [[ "$selected" -eq 0 ]]; then
  echo "e2e: no scenarios matched suite=$E2E_SUITE scenario=${E2E_SCENARIO:-<none>} tag=${E2E_TAG:-<none>}" >&2
  exit 1
fi

echo "e2e summary: selected=$selected passed=$passed skipped=$skipped"
[[ "$failed" -eq 0 ]]
