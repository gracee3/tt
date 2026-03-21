#!/usr/bin/env bash
set -euo pipefail

expected="Hello, Orcas!"
binary="$(mktemp "${TMPDIR:-/tmp}/live-supervisor-micro-proposal.XXXXXX")"
trap 'rm -f "$binary"' EXIT
cc main.c -o "$binary"
output="$("$binary")"
if [[ "$output" == "$expected" ]]; then
  echo "PASS"
else
  echo "FAIL: got: '$output'" >&2
  exit 1
fi
