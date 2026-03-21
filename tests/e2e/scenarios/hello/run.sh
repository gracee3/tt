#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "$0")/../../lib" && pwd)/common.sh"
scenario_dir="$(cd "$(dirname "$0")" && pwd)"
e2e_load_scenario_metadata "$scenario_dir"
e2e_prepare_scenario_dirs "$NAME"

binary_path="$E2E_SCENARIO_ARTIFACTS_DIR/hello"
cc "$scenario_dir/hello.c" -o "$binary_path"
"$scenario_dir/test_hello.sh" "$binary_path"
