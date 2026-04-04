#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")/../lib" && pwd)/common.sh"

orcas_bin="$e2e_repo_root/target/debug/orcas"
xdg_data_home="${ORCAS_E2E_XDG_DATA_HOME:-$e2e_output_root/xdg/default/data}"
xdg_config_home="${ORCAS_E2E_XDG_CONFIG_HOME:-$e2e_output_root/xdg/default/config}"
xdg_runtime_home="${ORCAS_E2E_XDG_RUNTIME_HOME:-$e2e_output_root/xdg/default/runtime}"
orcas_home="${ORCAS_E2E_ORCAS_HOME:-$e2e_output_root/orcas/default}"

mkdir -p "$xdg_data_home" "$xdg_config_home" "$xdg_runtime_home"
e2e_sync_legacy_xdg_into_orcas_home "$xdg_data_home" "$xdg_config_home" "$xdg_runtime_home" "$orcas_home"
chmod 700 "$xdg_runtime_home" || true

export ORCAS_HOME="$orcas_home"

if [[ ! -x "$orcas_bin" || "${ORCAS_E2E_FORCE_CARGO_RUN:-0}" == "1" ]]; then
  cargo build -q --manifest-path "$e2e_repo_root/Cargo.toml" -p orcas -p orcasd
fi

exec "$orcas_bin" "$@"
