# Configuration

## Configuration Overview

Orcas uses a small TOML configuration file for durable defaults and environment variables for runtime overrides. The current user-level config file is created automatically on first launch at `~/.orcas/config.toml`.

The practical rule is:

1. Use the config file for persistent defaults.
2. Use environment variables for per-process overrides.
3. Use CLI flags for one-off operator sessions.

The current packaged systemd unit is a user service. It inherits the same user-scoped config, data, log, and runtime directories as the CLI rather than introducing a separate root-owned config layer.

## Environment Variables

### Logging

`RUST_LOG` controls the tracing filter for all binaries.

Examples:

```bash
RUST_LOG=info orcasd
RUST_LOG=orcasd=debug,tokio=info orcasd
RUST_LOG=orcas=debug orcas doctor
```

The logging layer also understands the following Orcas-specific flags:

1. `ORCAS_LOG_RUNTIME_CYCLE` enables runtime-cycle logging when set to `1`, `true`, `yes`, or `on`.

### Daemon Runtime Overrides

The daemon and supervisor process manager recognize these environment variables:

1. `ORCAS_CODEX_BIN` sets the path to the local Codex binary.
2. `ORCAS_CODEX_LISTEN_URL` sets the upstream Codex WebSocket URL.
3. `ORCAS_DEFAULT_CWD` sets the default working directory for spawned work.
4. `ORCAS_DEFAULT_MODEL` sets the default model used for new work.
5. `ORCAS_CONNECTION_MODE` accepts `connect_only` or `spawn_always`.
6. `ORCAS_DAEMON_BINARY_PATH` is written by the launcher into daemon runtime metadata.
7. `ORCAS_DAEMON_BUILD_FINGERPRINT` is written by the launcher into daemon runtime metadata.

If `ORCAS_CONNECTION_MODE` is unset, Orcas keeps the configured or default `spawn_if_needed` behavior.

The service unit in this repository sets `RUST_LOG=info` by default.

## Config File Shape

The config file is TOML. The current top-level sections are `codex`, `supervisor`, and `defaults`.

Important current fields are:

1. `codex.binary_path` points at the local Codex executable.
2. `codex.listen_url` is the upstream socket URL, defaulting to `ws://127.0.0.1:4500`.
3. `codex.connection_mode` controls whether Orcas connects only or spawns the upstream worker.
4. `codex.reconnect` configures retry backoff.
5. `codex.config_overrides` carries passthrough Codex settings.
6. `supervisor.base_url` defaults to the OpenAI API base URL.
7. `supervisor.api_key_env` defaults to `OPENAI_API_KEY`.
8. `supervisor.model`, `supervisor.reasoning_effort`, and `supervisor.max_output_tokens` define the default reasoning profile.
9. `supervisor.proposals.auto_create_on_report_recorded` is disabled by default.
10. `defaults.cwd` and `defaults.model` define process defaults when no CLI override is supplied.

`codex.connection_mode` currently defaults to `spawn_if_needed`. The one-shot CLI flags `--connect-only` and `--force-spawn` override that setting for a single process launch and are intentionally mutually exclusive.

The current source tree ships a development-oriented default for `codex.binary_path`. For a real deployment, set that value to the path of the Codex binary you actually installed.

## Logging Behavior

Orcas binaries write structured tracing output to per-component log files under the data directory rather than to stdout or stderr.

The current default log directory is:

```bash
~/.orcas/logs/
```

The current files are:

1. `orcasd.log` for the daemon component log.
2. `orcas.log` for the operator CLI log.
3. `codex-app-server.log` for raw Codex app-server stdout/stderr diagnostics.

The log, config, data, and runtime directories are created automatically on startup.

When launched under the packaged systemd user service, the daemon still writes its application logs to those files. The user journal will mainly contain service lifecycle messages and any early startup failure that occurs before the file logger is initialized.

To increase verbosity, set `RUST_LOG=debug` or use a component-specific filter such as `RUST_LOG=orcasd=debug,tokio=info`.

## Networking And IPC Assumptions

Orcas itself does not listen on a public network port. The daemon binds a local Unix domain socket under `ORCAS_HOME/runtime`, with `ORCAS_HOME` defaulting to `~/.orcas`.

The current socket path pattern is:

```bash
${ORCAS_HOME:-~/.orcas}/runtime/orcasd.sock
```

The daemon also writes runtime metadata next to the socket path. The upstream worker connection is separate from Orcas IPC and defaults to a localhost WebSocket URL for the Codex app-server. Test harnesses and lab scripts isolate state by setting `ORCAS_HOME` to a temporary root.

If you change the upstream worker endpoint, update either the config file or `ORCAS_CODEX_LISTEN_URL` so the daemon and supervisor agree on the same target.
