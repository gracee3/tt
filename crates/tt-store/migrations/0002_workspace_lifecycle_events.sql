create table if not exists workspace_lifecycle_events (
  id text primary key,
  workspace_binding_id text not null references workspace_bindings(id) on delete cascade,
  source_workspace_binding_id text references workspace_bindings(id) on delete set null,
  kind text not null,
  note text,
  created_at text not null
);

create index if not exists workspace_lifecycle_events_binding_idx
  on workspace_lifecycle_events(workspace_binding_id, created_at desc);
