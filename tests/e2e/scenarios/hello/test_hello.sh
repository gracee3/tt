#!/usr/bin/env bash
set -euo pipefail

binary_path="${1:-./hello}"
output="$("$binary_path")"
if [[ "$output" == "Hello, Orcas!" ]]; then
  echo "PASS"
else
  echo "FAIL: got: '$output'" >&2
  exit 1
fi
