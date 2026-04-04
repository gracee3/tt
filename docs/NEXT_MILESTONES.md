# Next Milestones

- Keep the Codex contract snapshot in `crates/orcas-codex/contracts/codex-contract-index.json` aligned with the local Codex checkout.
- Add any future Orcas-facing typed fields to `orcas-codex::contract` before wiring them into runtime code.
- When Codex updates land upstream, regenerate the snapshot with `cargo run -p orcas-codex --bin codex-contract-sync -- --root /home/emmy/openai/codex/codex-rs --out crates/orcas-codex/contracts/codex-contract-index.json` and run the contract drift test.
