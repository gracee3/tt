# Scenario: Repo-Local Logs

## Goal

Verify that Orcas writes its logs under the repo-local XDG data directory and that the harness can point to them directly.

## Steps

1. Start Orcas using the harness wrapper.
2. Run a basic diagnostic command.
3. Confirm the active log directory lives under `target/e2e/xdg/.../data/orcas/logs`.
4. Confirm `orcasd.log` and `orcas.log` exist under that path.

## Expected Result

- The logs are available only under the scenario-local XDG data directory.
