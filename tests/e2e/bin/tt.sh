#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")/../lib" && pwd)/common.sh"

tt_bin="$e2e_repo_root/target/debug/tt"
xdg_data_home="${TT_E2E_XDG_DATA_HOME:-$e2e_output_root/xdg/default/data}"
xdg_config_home="${TT_E2E_XDG_CONFIG_HOME:-$e2e_output_root/xdg/default/config}"
xdg_runtime_home="${TT_E2E_XDG_RUNTIME_HOME:-$e2e_output_root/xdg/default/runtime}"
tt_home="${TT_E2E_TT_HOME:-$e2e_output_root/tt/default}"

mkdir -p "$xdg_data_home" "$xdg_config_home" "$xdg_runtime_home"
e2e_sync_legacy_xdg_into_tt_home "$xdg_data_home" "$xdg_config_home" "$xdg_runtime_home" "$tt_home"
chmod 700 "$xdg_runtime_home" || true

export TT_HOME="$tt_home"

if [[ ! -x "$tt_bin" || "${TT_E2E_FORCE_CARGO_RUN:-0}" == "1" ]]; then
  cargo build -q --manifest-path "$e2e_repo_root/Cargo.toml" -p tt -p ttd
fi

exec "$tt_bin" "$@"
