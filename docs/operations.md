# Operations

## Starting The System

Start the daemon directly when you want to run it in the foreground.

```bash
orcasd
```

Use the systemd user manager when you want the daemon managed as a service that shares the same XDG paths as the CLI and TUI.

```bash
systemctl --user start orcas-daemon.service
systemctl --user enable orcas-daemon.service
```

The Orcas CLI can also request that the daemon be started on demand.

```bash
orcas daemon start
```

## Checking Status

Check the unit state with the user manager.

```bash
systemctl --user status orcas-daemon.service
```

Check Orcas-level daemon state through the CLI.

```bash
orcas daemon status
orcas doctor
```

The doctor command reports the config path, `state.json`, `state.db`, runtime directory, socket path, daemon log path, and current Codex endpoint.

## Logs

Use `journalctl --user` for unit lifecycle events and startup failures.

```bash
journalctl --user -u orcas-daemon.service -e
journalctl --user -u orcas-daemon.service -f
```

Use the file logs for the application’s own tracing output.

```bash
tail -f ~/.local/share/orcas/logs/orcasd.log
tail -f ~/.local/share/orcas/logs/orcas.log
tail -f ~/.local/share/orcas/logs/orcas-tui.log
```

Common log patterns include socket bind failures, stale runtime cleanup, upstream connection failures, and request validation errors. If a client cannot connect, check the daemon log first, then confirm the socket path exists and is responsive. Use `codex-app-server.log` only when you need raw upstream subprocess output.

## Restarting And Stopping

```bash
systemctl --user restart orcas-daemon.service
systemctl --user stop orcas-daemon.service
```

The CLI exposes the same operations through the daemon API.

```bash
orcas daemon restart
orcas daemon stop
```

## Common Issues

### Daemon Not Starting

Check the daemon log and the unit status. The usual causes are a bad Codex binary path, a missing runtime directory, or a failure to bind the local socket.

```bash
systemctl --user status orcas-daemon.service
tail -n 100 ~/.local/share/orcas/logs/orcasd.log
```

### Socket Conflict Or Stale Socket

Orcas uses a Unix socket, not a TCP port. If another process already owns the socket path, or if a stale socket file remains after a crash, the daemon cannot bind cleanly. Stop the old process or remove the stale runtime artifacts after confirming nothing is still running.

```bash
orcas daemon status
systemctl --user stop orcas-daemon.service
```

### Permission Issues

If the daemon cannot create its config, data, log, or runtime directories, check the ownership of your user-scoped XDG paths and whether the user service inherited the expected environment.

### Binary Not Found In `PATH`

If `orcas`, `orcasd`, or `orcas-tui` are not found, install them into a directory on your `PATH` or invoke them with an absolute path.

```bash
install -m 0755 bin/orcas ~/.local/bin/orcas
install -m 0755 bin/orcasd ~/.local/bin/orcasd
```

## Debugging Workflow

When something fails, isolate the daemon from the operator CLI.

1. Run the daemon in the foreground and watch its log file.
2. Increase verbosity with `RUST_LOG=debug`.
3. Check whether the CLI can connect to the local socket.
4. Verify whether the upstream Codex endpoint is reachable from the daemon.

Example:

```bash
RUST_LOG=debug orcasd
orcas daemon status
orcas doctor
```

If the CLI can talk to the daemon but the daemon reports an upstream failure, the problem is usually in the Codex endpoint or the local Codex binary path. If the CLI cannot reach the daemon at all, focus on the socket path, unit state, and daemon log first.

## Upgrade Considerations

Replacing Orcas binaries is normally a file swap followed by a daemon restart. Keep the config and state directories in place so the daemon can reuse the existing workflow state.

```bash
systemctl --user stop orcas-daemon.service
sudo install -m 0755 ./orcasd /usr/local/bin/orcasd
sudo install -m 0755 ./orcas /usr/local/bin/orcas
systemctl --user start orcas-daemon.service
```

If the unit file changed, reload systemd before restarting.

```bash
systemctl --user daemon-reload
systemctl --user restart orcas-daemon.service
```
