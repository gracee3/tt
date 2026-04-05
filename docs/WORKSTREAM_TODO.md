# Workstream Todo

Tracked backlog for the active todo skill thread.

## Inbox

- Add a dedicated `todo` skill for iterative note ingestion, review, and planning.
- Use a tracked project-level backlog file on disk as the shared source of truth between exchanges.
- Preserve pasted notes, bugs, and text with enough detail to recreate the issue or context later.
- Support a commit-frequent workflow where the todo skill updates the backlog and returns a diff/changelog to the user.
- Keep clarification questions small and related, and let the user continue steering the backlog as needed.
- Allow an `insert` mode for normal note intake.
- Allow a `review` mode that prompts for missing requirements, undefined edges, and open gaps.
- Allow a `plan` mode that uses repo recon and source inspection to produce an implementation plan.
- Let the user view and revise the plan as work evolves.

## Now

- Define the todo skill interface and keep the tracked backlog current.

## Next

- Normalize future pasted notes into this file.
- Ask only the smallest useful follow-up questions when the backlog has gaps.

## Later

- Expand the backlog into a planning phase once requirements are sufficiently complete.

## Open Questions

- Which exact file path should remain canonical if additional workstream-specific todo files are introduced later?
- How frequently should backlog updates be committed relative to the cadence of note intake?

## Completed

- Identified the need for a dedicated todo skill.
- Established the project-level tracked backlog file for the todo workflow.
