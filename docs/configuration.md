# Configuration

## Overview

Orcas uses a user-level TOML config file at `~/.orcas/config.toml` for persistent defaults.

The runtime model on this branch is:

1. one host/home `orcasd`
2. one shared Codex app-server per host/home
3. `orcasd` attaches to that app-server and does not spawn it
4. `orcas app-server ...` is the recommended lifecycle surface

Use the config file for durable defaults, environment variables for per-process overrides, and CLI flags for one-off operator sessions.

## Recommended Shared Runtime Example

This is the primary host/home setup. It uses an Orcas-managed shared app-server and leaves direct Codex/OpenAI auth to the host environment unless you explicitly override it.

```toml
[codex]
binary_path = "/path/to/codex"
connection_mode = "connect_only"
config_overrides = []

[codex.reconnect]
initial_delay_ms = 150
max_delay_ms = 5000
multiplier = 2.0

[codex.app_server.default]
enabled = true
owner = "orcas"
transport = "websocket"
listen_url = "ws://127.0.0.1:4500"

[codex.responses]
base_url = "https://api.openai.com/v1"

[codex.direct_api]
# auth_file = "~/.codex/auth.json"

[supervisor]
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
model = "gpt-5.4"
reasoning_effort = "high"
max_output_tokens = 2000

[supervisor.proposals]
auto_create_on_report_recorded = false

[defaults]
model = "gpt-5"
```

Operationally:

1. `orcas app-server add default` ensures the shared app-server definition exists.
2. `orcas app-server start default` launches the shared listener.
3. `orcas daemon start` starts `orcasd`, which connects to the configured app-server endpoint.
4. `orcas app-server status default` and `orcas app-server info default` show the shared runtime endpoint and listener details.

Orcas also refreshes the checked-in repo `.codex/` template into the shared app-server `CODEX_HOME` when the app-server is added or started. In practice that means:

- source template: `./.codex/`
- runtime target: `~/.orcas/app-server/default/codex-home/.codex`

That gives the shared runtime a managed `config.toml` and lane-agent defaults without turning the pack into per-workstream state.

## Local Provider Example

Profiles and provider definitions let you keep the shared app-server model while selecting a different model backend for specific roles or workstreams.

```toml
[codex.profiles.local]
model_provider = "vllm"
model = "local-model"

[codex.model_providers.vllm]
name = "vLLM"
base_url = "http://127.0.0.1:8000/v1"
wire_api = "responses"
```

This example is additive. Keep the shared app-server configuration above and layer local provider selection through role, workstream, or CLI overrides.

## Auth Behavior

The default documented path is host auth:

1. Orcas uses the host’s existing Codex/OpenAI auth state unless you explicitly override it.
2. The main shared-runtime example leaves `codex.direct_api.auth_file` unset.
3. If you need an explicit file override, set `codex.direct_api.auth_file` yourself.

Orcas does not require a dedicated auth file in the primary recommended setup.

## Public Config Shape

The public nested `codex` shape on this branch is:

1. `[codex]`
2. `[codex.reconnect]`
3. `[codex.app_server.default]`
4. `[codex.responses]`
5. `[codex.direct_api]`
6. `[codex.profiles.<name>]`
7. `[codex.model_providers.<name>]`

Important fields are:

1. `codex.binary_path`
2. `codex.connection_mode`
3. `codex.config_overrides`
4. `codex.app_server.default.owner`
5. `codex.app_server.default.transport`
6. `codex.app_server.default.listen_url`
7. `codex.responses.base_url`
8. `codex.direct_api.auth_file`
9. `defaults.cwd`
10. `defaults.model`

The generated default config follows this shape.

## Environment Variables

### Logging

`RUST_LOG` controls the tracing filter for all binaries.

Examples:

```bash
RUST_LOG=info orcasd
RUST_LOG=orcasd=debug,tokio=info orcasd
RUST_LOG=orcas=debug orcas doctor
```

`ORCAS_LOG_RUNTIME_CYCLE` enables runtime-cycle logging when set to `1`, `true`, `yes`, or `on`.

### Runtime Overrides

The daemon and CLI process manager recognize:

1. `ORCAS_CODEX_BIN`
2. `ORCAS_CODEX_LISTEN_URL`
3. `ORCAS_DEFAULT_CWD`
4. `ORCAS_DEFAULT_MODEL`
5. `ORCAS_CONNECTION_MODE`
6. `ORCAS_DAEMON_BINARY_PATH`
7. `ORCAS_DAEMON_BUILD_FINGERPRINT`

`ORCAS_CONNECTION_MODE` is still available as a process override, but the documented shared-runtime configuration uses `connect_only`.

If you override the listen URL, keep the CLI, daemon, and shared app-server pointed at the same endpoint.

## Logging And Paths

Orcas writes structured logs under:

```bash
~/.orcas/logs/
```

The main files are:

1. `orcasd.log`
2. `orcas.log`
3. `app-server-default.log` for the Orcas-managed shared app-server

The daemon socket lives under:

```bash
${ORCAS_HOME:-~/.orcas}/runtime/orcasd.sock
```

`orcasd` and the shared app-server both use the same host/home root unless you explicitly change `ORCAS_HOME`.

## Role Pack

The repo includes a checked-in `.codex/` scaffold that acts as the template for the shared app-server home, and Orcas copies it into `~/.orcas/app-server/default/codex-home/.codex`.

Orcas copies the `.codex` subtree from that pack into the shared app-server `CODEX_HOME`:

- source template: `.codex/`
- runtime target: `~/.orcas/app-server/default/codex-home/.codex`

The runtime target is managed by Orcas and refreshed on `orcas app-server add` and `orcas app-server start`.
