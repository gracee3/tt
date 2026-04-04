# Logging

## Overview

Orcas uses Rust `tracing` with `tracing_subscriber` and plain-text file logs.

The logging model is intentionally simple:

1. Each binary writes to its own component log file.
2. Runtime verbosity is controlled with `RUST_LOG`.
3. Raw Codex app-server stdout/stderr goes to a separate diagnostic log file.

Orcas no longer maintains a merged aggregate log.

## Runtime Log Level Control

`RUST_LOG` controls the tracing filter for all Orcas binaries.

If `RUST_LOG` is unset or invalid, Orcas falls back to:

```text
{component}=info,info,tokio=info
```

Examples:

```bash
RUST_LOG=info orcasd
RUST_LOG=debug orcasd
RUST_LOG=orcasd=debug,tokio=info orcasd
RUST_LOG=orcas=debug orcas doctor
```

Orcas also supports one logging-related boolean env var:

1. `ORCAS_LOG_RUNTIME_CYCLE` enables extra runtime-cycle logs when set to `1`, `true`, `yes`, or `on`.

## Log Locations

Orcas keeps logs under its home root.

The logs directory is:

```bash
${ORCAS_HOME:-~/.orcas}/logs/
```

Current log files:

1. `orcasd.log` for the daemon.
2. `orcas.log` for the operator CLI.
3. `codex-app-server.log` for raw Codex app-server stdout/stderr diagnostics.

The log, config, data, and runtime directories are created automatically on startup.

## Recommended Debug Workflow

Start with the component log that matches the failing surface:

1. `orcasd.log` for daemon startup, IPC, persistence, authority-store, and upstream lifecycle issues.
2. `orcas.log` for CLI command issues.

Use `codex-app-server.log` only when the semantic daemon logs point to an upstream Codex app-server problem and you need the raw subprocess output.

Good first steps:

```bash
tail -f ~/.orcas/logs/orcasd.log
RUST_LOG=debug orcasd
orcas daemon status
orcas doctor
```

For targeted debugging, prefer component-specific filters over global `debug`.
