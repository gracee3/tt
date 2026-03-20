# Configuration

## Configuration Overview

Orcas uses a small TOML configuration file for durable defaults and environment variables for runtime overrides. The current user-level config file is created automatically on first launch at `~/.config/orcas/config.toml`.

The practical rule is:

1. Use the config file for persistent defaults.
2. Use environment variables for per-process overrides.
3. Use CLI flags for one-off operator sessions.

For site-wide packaging, the conventional optional locations are `/etc/orcas/config.toml` and `/etc/orcas/`. Those paths are not required for a working local install, but they are the right place to add a system-level layer if packaging grows one later.

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
2. `ORCAS_LOG_AGGREGATE_DAEMON` controls the aggregate daemon log file.
3. `ORCAS_LOG_AGGREGATE_SUPERVISOR` controls the aggregate supervisor log file.
4. `ORCAS_LOG_AGGREGATE_TUI` controls the aggregate TUI log file.
5. `ORCAS_LOG_AGGREGATE_APP_SERVER` controls aggregate logging for the Codex app-server stream.

### Daemon Runtime Overrides

The daemon and supervisor process manager recognize these environment variables:

1. `ORCAS_CODEX_BIN` sets the path to the local Codex binary.
2. `ORCAS_CODEX_LISTEN_URL` sets the upstream Codex WebSocket URL.
3. `ORCAS_DEFAULT_CWD` sets the default working directory for spawned work.
4. `ORCAS_DEFAULT_MODEL` sets the default model used for new work.
5. `ORCAS_CONNECTION_MODE` accepts `connect_only` or `spawn_always`.
6. `ORCAS_DAEMON_BINARY_PATH` is written by the launcher into daemon runtime metadata.
7. `ORCAS_DAEMON_BUILD_FINGERPRINT` is written by the launcher into daemon runtime metadata.

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

The current source tree ships a development-oriented default for `codex.binary_path`. For a real deployment, set that value to the path of the Codex binary you actually installed.

## Logging Behavior

Orcas binaries write structured tracing output to log files under the data directory rather than to stdout or stderr. For the daemon, the primary log is `orcasd.log` and the aggregate log is `orcas.log`, both under the logs directory derived from XDG data paths.

The current log directory is:

```bash
${XDG_DATA_HOME:-~/.local/share}/orcas/logs/
```

The current files are:

1. `orcasd.log` for the daemon component log.
2. `orcas-tui.log` for the TUI component log.
3. `orcas.log` for the operator CLI log and the aggregate cross-component log.
4. `codex-app-server.log` for raw Codex app-server stdout/stderr diagnostics.

Aggregate logging is enabled by default for the daemon, TUI, and supervisor/CLI components. It is disabled by default for the raw app-server stream unless `ORCAS_LOG_AGGREGATE_APP_SERVER` is enabled.

The log, config, data, and runtime directories are created automatically on startup.

When launched under systemd, the daemon still writes its application logs to those files. The system journal will mainly contain service lifecycle messages and any early startup failure that occurs before the file logger is initialized.

To increase verbosity, set `RUST_LOG=debug` or use a component-specific filter such as `RUST_LOG=orcasd=debug,tokio=info`.

## Networking And IPC Assumptions

Orcas itself does not listen on a public network port. The daemon binds a local Unix domain socket at the runtime path derived from the XDG runtime directory, with a fallback under the data directory when no runtime directory is available.

The current socket path pattern is:

```bash
${XDG_RUNTIME_DIR:-~/.local/share/orcas/runtime}/orcas/orcasd.sock
```

The daemon also writes runtime metadata next to the socket path. The upstream worker connection is separate from Orcas IPC and defaults to a localhost WebSocket URL for the Codex app-server.

If you change the upstream worker endpoint, update either the config file or `ORCAS_CODEX_LISTEN_URL` so the daemon and supervisor agree on the same target.
