# Codex App-Server Notes

These notes are grounded in the local checkout at `/home/emmy/git/codex`.
Current Orcas reference target: Codex `v0.115.0`.

## Confirmed WebSocket Entry Point

Codex CLI accepts:

```bash
codex app-server --listen ws://127.0.0.1:4500
```

Relevant local source:

- `codex-rs/cli/src/main.rs`
- `codex-rs/app-server/src/transport.rs`

Important detail: the listen URL must be `ws://IP:PORT`; Codex rejects hostnames like `ws://localhost:1234` in its parser/tests.

## Initialize Handshake

Expected sequence from local source/tests:

1. client sends `initialize`
2. server returns initialize response
3. client sends `initialized`

Relevant local source:

- `codex-rs/app-server/src/message_processor.rs`
- `codex-rs/app-server/tests/suite/v2/initialize.rs`
- `sdk/python/src/codex_app_server/client.py`

## Narrow Request Set Implemented In Orcas

The current Orcas protocol slice covers:

- `initialize`
- `thread/start`
- `thread/resume`
- `thread/read`
- `thread/list`
- `turn/start`
- `turn/interrupt`
- `model/list`

The schema references live under:

- `codex-rs/app-server-protocol/schema/typescript/v2/`
- `codex-rs/app-server-protocol/schema/typescript/InitializeResponse.ts`

## Narrow Notification Set Implemented In Orcas

Current Orcas event mapping covers:

- `thread/started`
- `thread/status/changed`
- `turn/started`
- `turn/completed`
- `item/started`
- `item/completed`
- `item/agentMessage/delta`

## Approval / Server Requests

Codex app-server can send server-originated JSON-RPC requests for approvals and elicitation.

Examples from local source:

- command execution approval
- file change approval
- tool/user-input requests
- permissions approval
- MCP elicitation

Relevant local source:

- `codex-rs/app-server/src/bespoke_event_handling.rs`
- `codex-rs/app-server-test-client/src/lib.rs`

Current Orcas behavior is deliberately conservative:

- surface the request as an Orcas event
- route it through the `ApprovalRouter` boundary
- reject by default until Orcas has a typed approvals UX

## Initialize Response Note

Local Codex source and tests define `InitializeResponse` with:

- `userAgent`
- `platformFamily`
- `platformOs`

Relevant files:

- `codex-rs/app-server/src/message_processor.rs`
- `codex-rs/app-server/tests/suite/v2/initialize.rs`
- `sdk/python/src/codex_app_server/models.py`

During Orcas validation on this host, the strict response model was too rigid for the observed response shape, so Orcas currently deserializes this response permissively with optional fields. That keeps the handshake resilient while the protocol layer remains intentionally narrow.
