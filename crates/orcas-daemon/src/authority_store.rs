use std::fs;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use orcas_core::authority::{
    self, AggregateKey, AggregateType, AuthorityCommand, AuthorityCommandStore, AuthorityEvent,
    AuthorityEventEnvelope, AuthorityEventStore, AuthorityProjectionStore, AuthorityProjector,
    AuthorityQueryStore, CausationId, CommandId, CommandReceipt, CorrelationId, DeletePlan,
    DeletePlanTarget, DeleteTarget, EventMetadata, HierarchySnapshot, OriginNodeId,
    ProjectionCheckpoint, Revision, StoredAuthorityEvent, TrackedThreadBindingState,
    TrackedThreadRecord, TrackedThreadSummary, WorkUnitNode, WorkUnitRecord, WorkUnitSummary,
    WorkstreamNode, WorkstreamRecord, WorkstreamSummary,
};
use orcas_core::{AppPaths, OrcasError, OrcasResult, StoredState};

const AUTHORITY_PROJECTION: &str = "authority_current";
const META_ORIGIN_NODE_ID: &str = "origin_node_id";
const META_JSON_IMPORT_STATUS: &str = "json_import_status";
const META_JSON_IMPORT_COMPLETED_AT: &str = "json_import_completed_at";

const INITIAL_SCHEMA: &str = r#"
create table if not exists store_meta (
  key text primary key,
  value text not null
);

create table if not exists command_receipts (
  command_id text primary key,
  command_kind text not null,
  aggregate_type text not null,
  aggregate_id text not null,
  accepted integer not null,
  response_json text,
  recorded_at text not null
);

create table if not exists event_log (
  seq integer primary key autoincrement,
  event_id text not null unique,
  command_id text not null,
  aggregate_type text not null,
  aggregate_id text not null,
  aggregate_version integer not null,
  event_kind text not null,
  occurred_at text not null,
  origin_node_id text not null,
  causation_id text,
  correlation_id text,
  body_json text not null
);

create index if not exists idx_event_log_aggregate
  on event_log (aggregate_type, aggregate_id, aggregate_version);

create index if not exists idx_event_log_command
  on event_log (command_id);

create table if not exists projection_checkpoint (
  projection_name text primary key,
  last_applied_sequence integer not null
);

create table if not exists workstreams (
  id text primary key,
  title text not null,
  objective text not null,
  status text not null,
  priority text not null,
  revision integer not null,
  origin_node_id text not null,
  created_at text not null,
  updated_at text not null,
  deleted_at text
);

create table if not exists work_units (
  id text primary key,
  workstream_id text not null,
  title text not null,
  task_statement text not null,
  status text not null,
  revision integer not null,
  origin_node_id text not null,
  created_at text not null,
  updated_at text not null,
  deleted_at text,
  foreign key (workstream_id) references workstreams(id)
);

create index if not exists idx_work_units_workstream
  on work_units (workstream_id, deleted_at, updated_at, id);

create table if not exists tracked_threads (
  id text primary key,
  work_unit_id text not null,
  title text not null,
  notes text,
  backend_kind text not null,
  upstream_thread_id text,
  binding_state text not null,
  preferred_cwd text,
  preferred_model text,
  last_seen_turn_id text,
  revision integer not null,
  origin_node_id text not null,
  created_at text not null,
  updated_at text not null,
  deleted_at text,
  foreign key (work_unit_id) references work_units(id)
);

create index if not exists idx_tracked_threads_work_unit
  on tracked_threads (work_unit_id, deleted_at, updated_at, id);

create unique index if not exists idx_tracked_threads_upstream_active
  on tracked_threads (upstream_thread_id)
  where upstream_thread_id is not null and deleted_at is null;
"#;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AuthorityMutationResult {
    Workstream(WorkstreamRecord),
    WorkUnit(WorkUnitRecord),
    TrackedThread(TrackedThreadRecord),
}

pub struct AuthoritySqliteStore {
    #[cfg(test)]
    paths: AppPaths,
    connection: Mutex<Connection>,
}

impl AuthoritySqliteStore {
    pub fn open(paths: AppPaths) -> OrcasResult<Self> {
        let mut connection = Connection::open(&paths.state_db_file)
            .map_err(|error| store_error(format!("open authority db: {error}")))?;
        connection
            .execute_batch(
                "pragma journal_mode = wal;
                 pragma synchronous = full;
                 pragma foreign_keys = on;
                 pragma busy_timeout = 5000;",
            )
            .map_err(|error| store_error(format!("configure authority db: {error}")))?;
        Self::migrate(&mut connection)?;
        let origin_node_id = Self::ensure_origin_node_id(&connection)?;
        Self::bootstrap_from_json_if_needed(&paths, &mut connection, &origin_node_id)?;
        Ok(Self {
            #[cfg(test)]
            paths,
            connection: Mutex::new(connection),
        })
    }

    #[cfg(test)]
    pub fn database_path(&self) -> &std::path::Path {
        &self.paths.state_db_file
    }

    #[cfg(test)]
    pub fn origin_node_id(&self) -> OrcasResult<OriginNodeId> {
        self.with_connection(|connection| Self::ensure_origin_node_id(connection))
    }

    pub async fn execute_command(
        &self,
        command: AuthorityCommand,
    ) -> OrcasResult<AuthorityMutationResult> {
        self.with_connection(|connection| {
            let transaction = connection
                .transaction()
                .map_err(|error| store_error(format!("start authority transaction: {error}")))?;

            if let Some(existing) =
                Self::load_command_receipt_tx(&transaction, &command.metadata().command_id)?
            {
                let response_json = existing.response_json.ok_or_else(|| {
                    store_error("stored authority command receipt missing response json")
                })?;
                let result = serde_json::from_value::<AuthorityMutationResult>(response_json)
                    .map_err(|error| {
                        store_error(format!("decode stored authority response: {error}"))
                    })?;
                transaction.commit().map_err(|error| {
                    store_error(format!("commit duplicate authority transaction: {error}"))
                })?;
                return Ok(result);
            }

            let result = Self::execute_command_tx(&transaction, &command)?;
            let receipt = CommandReceipt {
                command_id: command.metadata().command_id.clone(),
                command_kind: command.kind(),
                aggregate_key: command.aggregate_key(),
                accepted: true,
                response_json: Some(serde_json::to_value(&result).map_err(|error| {
                    store_error(format!("serialize authority response: {error}"))
                })?),
                recorded_at: Utc::now(),
            };
            Self::insert_command_receipt_tx(&transaction, &receipt)?;
            transaction
                .commit()
                .map_err(|error| store_error(format!("commit authority transaction: {error}")))?;
            Ok(result)
        })
    }

    fn execute_command_tx(
        transaction: &Transaction<'_>,
        command: &AuthorityCommand,
    ) -> OrcasResult<AuthorityMutationResult> {
        match command {
            AuthorityCommand::CreateWorkstream(command) => {
                let record = WorkstreamRecord {
                    id: command.workstream_id.clone(),
                    title: require_non_empty(command.title.clone(), "title")?,
                    objective: require_non_empty(command.objective.clone(), "objective")?,
                    status: command.status,
                    priority: require_non_empty(command.priority.clone(), "priority")?,
                    revision: Revision::initial(),
                    origin_node_id: command.metadata.origin_node_id.clone(),
                    created_at: command.metadata.issued_at,
                    updated_at: command.metadata.issued_at,
                    deleted_at: None,
                };
                if Self::load_workstream_tx(transaction, &record.id)?.is_some() {
                    return Err(OrcasError::Protocol(format!(
                        "workstream `{}` already exists",
                        record.id
                    )));
                }
                let event = AuthorityEventEnvelope {
                    metadata: EventMetadata {
                        event_id: authority::EventId::new(),
                        command_id: command.metadata.command_id.clone(),
                        aggregate_type: AggregateType::Workstream,
                        aggregate_id: record.id.to_string(),
                        aggregate_version: record.revision,
                        occurred_at: command.metadata.issued_at,
                        origin_node_id: command.metadata.origin_node_id.clone(),
                        causation_id: None,
                        correlation_id: command.metadata.correlation_id.clone(),
                    },
                    event: AuthorityEvent::WorkstreamCreated(authority::WorkstreamCreated {
                        workstream: record.clone(),
                    }),
                };
                Self::append_event_envelope_tx(transaction, &event)?;
                Ok(AuthorityMutationResult::Workstream(record))
            }
            AuthorityCommand::EditWorkstream(command) => {
                if command.changes.is_empty() {
                    return Err(OrcasError::Protocol(
                        "edit workstream requires at least one changed field".to_string(),
                    ));
                }
                let mut record = Self::load_workstream_tx(transaction, &command.workstream_id)?
                    .ok_or_else(|| {
                        OrcasError::Protocol(format!(
                            "unknown workstream `{}`",
                            command.workstream_id
                        ))
                    })?;
                ensure_active("workstream", &record.deleted_at, record.id.as_str())?;
                ensure_revision(
                    "workstream",
                    record.id.as_str(),
                    record.revision,
                    command.expected_revision,
                )?;
                if let Some(title) = &command.changes.title {
                    record.title = require_non_empty(title.clone(), "title")?;
                }
                if let Some(objective) = &command.changes.objective {
                    record.objective = require_non_empty(objective.clone(), "objective")?;
                }
                if let Some(status) = command.changes.status {
                    record.status = status;
                }
                if let Some(priority) = &command.changes.priority {
                    record.priority = require_non_empty(priority.clone(), "priority")?;
                }
                record.revision = record.revision.next();
                record.updated_at = command.metadata.issued_at;
                let event = AuthorityEventEnvelope {
                    metadata: EventMetadata {
                        event_id: authority::EventId::new(),
                        command_id: command.metadata.command_id.clone(),
                        aggregate_type: AggregateType::Workstream,
                        aggregate_id: record.id.to_string(),
                        aggregate_version: record.revision,
                        occurred_at: command.metadata.issued_at,
                        origin_node_id: command.metadata.origin_node_id.clone(),
                        causation_id: None,
                        correlation_id: command.metadata.correlation_id.clone(),
                    },
                    event: AuthorityEvent::WorkstreamEdited(authority::WorkstreamEdited {
                        workstream_id: record.id.clone(),
                        changes: command.changes.clone(),
                    }),
                };
                Self::append_event_envelope_tx(transaction, &event)?;
                Ok(AuthorityMutationResult::Workstream(record))
            }
            AuthorityCommand::DeleteWorkstream(command) => {
                let root = Self::load_workstream_tx(transaction, &command.workstream_id)?
                    .ok_or_else(|| {
                        OrcasError::Protocol(format!(
                            "unknown workstream `{}`",
                            command.workstream_id
                        ))
                    })?;
                ensure_active("workstream", &root.deleted_at, root.id.as_str())?;
                ensure_revision(
                    "workstream",
                    root.id.as_str(),
                    root.revision,
                    command.expected_revision,
                )?;
                let work_units = Self::list_work_units_tx(transaction, Some(&root.id), false)?;
                let mut tracked_threads = Vec::new();
                for work_unit in &work_units {
                    tracked_threads.extend(Self::list_tracked_threads_tx(
                        transaction,
                        &work_unit.id,
                        false,
                    )?);
                }

                let deleted_root = WorkstreamRecord {
                    deleted_at: Some(command.metadata.issued_at),
                    updated_at: command.metadata.issued_at,
                    revision: root.revision.next(),
                    ..root
                };
                let root_event = AuthorityEventEnvelope {
                    metadata: EventMetadata {
                        event_id: authority::EventId::new(),
                        command_id: command.metadata.command_id.clone(),
                        aggregate_type: AggregateType::Workstream,
                        aggregate_id: deleted_root.id.to_string(),
                        aggregate_version: deleted_root.revision,
                        occurred_at: command.metadata.issued_at,
                        origin_node_id: command.metadata.origin_node_id.clone(),
                        causation_id: None,
                        correlation_id: command.metadata.correlation_id.clone(),
                    },
                    event: AuthorityEvent::WorkstreamDeleted(authority::WorkstreamDeleted {
                        workstream_id: deleted_root.id.clone(),
                    }),
                };
                Self::append_event_envelope_tx(transaction, &root_event)?;

                for work_unit in &work_units {
                    let event = AuthorityEventEnvelope {
                        metadata: EventMetadata {
                            event_id: authority::EventId::new(),
                            command_id: command.metadata.command_id.clone(),
                            aggregate_type: AggregateType::WorkUnit,
                            aggregate_id: work_unit.id.to_string(),
                            aggregate_version: work_unit.revision.next(),
                            occurred_at: command.metadata.issued_at,
                            origin_node_id: command.metadata.origin_node_id.clone(),
                            causation_id: Some(CausationId::new()),
                            correlation_id: command.metadata.correlation_id.clone(),
                        },
                        event: AuthorityEvent::WorkUnitDeleted(authority::WorkUnitDeleted {
                            work_unit_id: work_unit.id.clone(),
                        }),
                    };
                    Self::append_event_envelope_tx(transaction, &event)?;
                }
                for tracked_thread in &tracked_threads {
                    let event = AuthorityEventEnvelope {
                        metadata: EventMetadata {
                            event_id: authority::EventId::new(),
                            command_id: command.metadata.command_id.clone(),
                            aggregate_type: AggregateType::TrackedThread,
                            aggregate_id: tracked_thread.id.to_string(),
                            aggregate_version: tracked_thread.revision.next(),
                            occurred_at: command.metadata.issued_at,
                            origin_node_id: command.metadata.origin_node_id.clone(),
                            causation_id: Some(CausationId::new()),
                            correlation_id: command.metadata.correlation_id.clone(),
                        },
                        event: AuthorityEvent::TrackedThreadDeleted(
                            authority::TrackedThreadDeleted {
                                tracked_thread_id: tracked_thread.id.clone(),
                            },
                        ),
                    };
                    Self::append_event_envelope_tx(transaction, &event)?;
                }

                Ok(AuthorityMutationResult::Workstream(deleted_root))
            }
            AuthorityCommand::CreateWorkUnit(command) => {
                let parent = Self::load_workstream_tx(transaction, &command.workstream_id)?
                    .ok_or_else(|| {
                        OrcasError::Protocol(format!(
                            "unknown workstream `{}`",
                            command.workstream_id
                        ))
                    })?;
                ensure_active("workstream", &parent.deleted_at, parent.id.as_str())?;
                let record = WorkUnitRecord {
                    id: command.work_unit_id.clone(),
                    workstream_id: command.workstream_id.clone(),
                    title: require_non_empty(command.title.clone(), "title")?,
                    task_statement: require_non_empty(
                        command.task_statement.clone(),
                        "task_statement",
                    )?,
                    status: command.status,
                    revision: Revision::initial(),
                    origin_node_id: command.metadata.origin_node_id.clone(),
                    created_at: command.metadata.issued_at,
                    updated_at: command.metadata.issued_at,
                    deleted_at: None,
                };
                if Self::load_work_unit_tx(transaction, &record.id)?.is_some() {
                    return Err(OrcasError::Protocol(format!(
                        "work unit `{}` already exists",
                        record.id
                    )));
                }
                let event = AuthorityEventEnvelope {
                    metadata: EventMetadata {
                        event_id: authority::EventId::new(),
                        command_id: command.metadata.command_id.clone(),
                        aggregate_type: AggregateType::WorkUnit,
                        aggregate_id: record.id.to_string(),
                        aggregate_version: record.revision,
                        occurred_at: command.metadata.issued_at,
                        origin_node_id: command.metadata.origin_node_id.clone(),
                        causation_id: None,
                        correlation_id: command.metadata.correlation_id.clone(),
                    },
                    event: AuthorityEvent::WorkUnitCreated(authority::WorkUnitCreated {
                        work_unit: record.clone(),
                    }),
                };
                Self::append_event_envelope_tx(transaction, &event)?;
                Ok(AuthorityMutationResult::WorkUnit(record))
            }
            AuthorityCommand::EditWorkUnit(command) => {
                if command.changes.is_empty() {
                    return Err(OrcasError::Protocol(
                        "edit work unit requires at least one changed field".to_string(),
                    ));
                }
                let mut record = Self::load_work_unit_tx(transaction, &command.work_unit_id)?
                    .ok_or_else(|| {
                        OrcasError::Protocol(format!(
                            "unknown work unit `{}`",
                            command.work_unit_id
                        ))
                    })?;
                ensure_active("work unit", &record.deleted_at, record.id.as_str())?;
                ensure_revision(
                    "work unit",
                    record.id.as_str(),
                    record.revision,
                    command.expected_revision,
                )?;
                if let Some(title) = &command.changes.title {
                    record.title = require_non_empty(title.clone(), "title")?;
                }
                if let Some(statement) = &command.changes.task_statement {
                    record.task_statement = require_non_empty(statement.clone(), "task_statement")?;
                }
                if let Some(status) = command.changes.status {
                    record.status = status;
                }
                record.revision = record.revision.next();
                record.updated_at = command.metadata.issued_at;
                let event = AuthorityEventEnvelope {
                    metadata: EventMetadata {
                        event_id: authority::EventId::new(),
                        command_id: command.metadata.command_id.clone(),
                        aggregate_type: AggregateType::WorkUnit,
                        aggregate_id: record.id.to_string(),
                        aggregate_version: record.revision,
                        occurred_at: command.metadata.issued_at,
                        origin_node_id: command.metadata.origin_node_id.clone(),
                        causation_id: None,
                        correlation_id: command.metadata.correlation_id.clone(),
                    },
                    event: AuthorityEvent::WorkUnitEdited(authority::WorkUnitEdited {
                        work_unit_id: record.id.clone(),
                        changes: command.changes.clone(),
                    }),
                };
                Self::append_event_envelope_tx(transaction, &event)?;
                Ok(AuthorityMutationResult::WorkUnit(record))
            }
            AuthorityCommand::DeleteWorkUnit(command) => {
                let root = Self::load_work_unit_tx(transaction, &command.work_unit_id)?
                    .ok_or_else(|| {
                        OrcasError::Protocol(format!(
                            "unknown work unit `{}`",
                            command.work_unit_id
                        ))
                    })?;
                ensure_active("work unit", &root.deleted_at, root.id.as_str())?;
                ensure_revision(
                    "work unit",
                    root.id.as_str(),
                    root.revision,
                    command.expected_revision,
                )?;
                let tracked_threads = Self::list_tracked_threads_tx(transaction, &root.id, false)?;
                let deleted_root = WorkUnitRecord {
                    deleted_at: Some(command.metadata.issued_at),
                    updated_at: command.metadata.issued_at,
                    revision: root.revision.next(),
                    ..root
                };
                let root_event = AuthorityEventEnvelope {
                    metadata: EventMetadata {
                        event_id: authority::EventId::new(),
                        command_id: command.metadata.command_id.clone(),
                        aggregate_type: AggregateType::WorkUnit,
                        aggregate_id: deleted_root.id.to_string(),
                        aggregate_version: deleted_root.revision,
                        occurred_at: command.metadata.issued_at,
                        origin_node_id: command.metadata.origin_node_id.clone(),
                        causation_id: None,
                        correlation_id: command.metadata.correlation_id.clone(),
                    },
                    event: AuthorityEvent::WorkUnitDeleted(authority::WorkUnitDeleted {
                        work_unit_id: deleted_root.id.clone(),
                    }),
                };
                Self::append_event_envelope_tx(transaction, &root_event)?;
                for tracked_thread in &tracked_threads {
                    let event = AuthorityEventEnvelope {
                        metadata: EventMetadata {
                            event_id: authority::EventId::new(),
                            command_id: command.metadata.command_id.clone(),
                            aggregate_type: AggregateType::TrackedThread,
                            aggregate_id: tracked_thread.id.to_string(),
                            aggregate_version: tracked_thread.revision.next(),
                            occurred_at: command.metadata.issued_at,
                            origin_node_id: command.metadata.origin_node_id.clone(),
                            causation_id: Some(CausationId::new()),
                            correlation_id: command.metadata.correlation_id.clone(),
                        },
                        event: AuthorityEvent::TrackedThreadDeleted(
                            authority::TrackedThreadDeleted {
                                tracked_thread_id: tracked_thread.id.clone(),
                            },
                        ),
                    };
                    Self::append_event_envelope_tx(transaction, &event)?;
                }
                Ok(AuthorityMutationResult::WorkUnit(deleted_root))
            }
            AuthorityCommand::CreateTrackedThread(command) => {
                let parent = Self::load_work_unit_tx(transaction, &command.work_unit_id)?
                    .ok_or_else(|| {
                        OrcasError::Protocol(format!(
                            "unknown work unit `{}`",
                            command.work_unit_id
                        ))
                    })?;
                ensure_active("work unit", &parent.deleted_at, parent.id.as_str())?;
                let record = TrackedThreadRecord {
                    id: command.tracked_thread_id.clone(),
                    work_unit_id: command.work_unit_id.clone(),
                    title: require_non_empty(command.title.clone(), "title")?,
                    notes: command.notes.clone(),
                    backend_kind: command.backend_kind,
                    upstream_thread_id: command.upstream_thread_id.clone(),
                    binding_state: if command.upstream_thread_id.is_some() {
                        TrackedThreadBindingState::Bound
                    } else {
                        TrackedThreadBindingState::Unbound
                    },
                    preferred_cwd: command.preferred_cwd.clone(),
                    preferred_model: command.preferred_model.clone(),
                    last_seen_turn_id: None,
                    revision: Revision::initial(),
                    origin_node_id: command.metadata.origin_node_id.clone(),
                    created_at: command.metadata.issued_at,
                    updated_at: command.metadata.issued_at,
                    deleted_at: None,
                };
                if Self::load_tracked_thread_tx(transaction, &record.id)?.is_some() {
                    return Err(OrcasError::Protocol(format!(
                        "tracked thread `{}` already exists",
                        record.id
                    )));
                }
                Self::ensure_upstream_binding_available_tx(
                    transaction,
                    record.upstream_thread_id.as_deref(),
                    None,
                )?;
                let event = AuthorityEventEnvelope {
                    metadata: EventMetadata {
                        event_id: authority::EventId::new(),
                        command_id: command.metadata.command_id.clone(),
                        aggregate_type: AggregateType::TrackedThread,
                        aggregate_id: record.id.to_string(),
                        aggregate_version: record.revision,
                        occurred_at: command.metadata.issued_at,
                        origin_node_id: command.metadata.origin_node_id.clone(),
                        causation_id: None,
                        correlation_id: command.metadata.correlation_id.clone(),
                    },
                    event: AuthorityEvent::TrackedThreadCreated(authority::TrackedThreadCreated {
                        tracked_thread: record.clone(),
                    }),
                };
                Self::append_event_envelope_tx(transaction, &event)?;
                Ok(AuthorityMutationResult::TrackedThread(record))
            }
            AuthorityCommand::EditTrackedThread(command) => {
                if command.changes.is_empty() {
                    return Err(OrcasError::Protocol(
                        "edit tracked thread requires at least one changed field".to_string(),
                    ));
                }
                let mut record =
                    Self::load_tracked_thread_tx(transaction, &command.tracked_thread_id)?
                        .ok_or_else(|| {
                            OrcasError::Protocol(format!(
                                "unknown tracked thread `{}`",
                                command.tracked_thread_id
                            ))
                        })?;
                ensure_active("tracked thread", &record.deleted_at, record.id.as_str())?;
                ensure_revision(
                    "tracked thread",
                    record.id.as_str(),
                    record.revision,
                    command.expected_revision,
                )?;
                if let Some(title) = &command.changes.title {
                    record.title = require_non_empty(title.clone(), "title")?;
                }
                if let Some(notes) = &command.changes.notes {
                    record.notes = notes.clone();
                }
                if let Some(backend_kind) = command.changes.backend_kind {
                    record.backend_kind = backend_kind;
                }
                if let Some(upstream_thread_id) = &command.changes.upstream_thread_id {
                    record.upstream_thread_id = upstream_thread_id.clone();
                }
                if let Some(binding_state) = command.changes.binding_state {
                    record.binding_state = binding_state;
                } else if command.changes.upstream_thread_id.is_some() {
                    record.binding_state = if record.upstream_thread_id.is_some() {
                        TrackedThreadBindingState::Bound
                    } else {
                        TrackedThreadBindingState::Unbound
                    };
                }
                if let Some(preferred_cwd) = &command.changes.preferred_cwd {
                    record.preferred_cwd = preferred_cwd.clone();
                }
                if let Some(preferred_model) = &command.changes.preferred_model {
                    record.preferred_model = preferred_model.clone();
                }
                if let Some(last_seen_turn_id) = &command.changes.last_seen_turn_id {
                    record.last_seen_turn_id = last_seen_turn_id.clone();
                }
                Self::ensure_upstream_binding_available_tx(
                    transaction,
                    record.upstream_thread_id.as_deref(),
                    Some(&record.id),
                )?;
                record.revision = record.revision.next();
                record.updated_at = command.metadata.issued_at;
                let event = AuthorityEventEnvelope {
                    metadata: EventMetadata {
                        event_id: authority::EventId::new(),
                        command_id: command.metadata.command_id.clone(),
                        aggregate_type: AggregateType::TrackedThread,
                        aggregate_id: record.id.to_string(),
                        aggregate_version: record.revision,
                        occurred_at: command.metadata.issued_at,
                        origin_node_id: command.metadata.origin_node_id.clone(),
                        causation_id: None,
                        correlation_id: command.metadata.correlation_id.clone(),
                    },
                    event: AuthorityEvent::TrackedThreadEdited(authority::TrackedThreadEdited {
                        tracked_thread_id: record.id.clone(),
                        changes: command.changes.clone(),
                    }),
                };
                Self::append_event_envelope_tx(transaction, &event)?;
                Ok(AuthorityMutationResult::TrackedThread(record))
            }
            AuthorityCommand::DeleteTrackedThread(command) => {
                let root = Self::load_tracked_thread_tx(transaction, &command.tracked_thread_id)?
                    .ok_or_else(|| {
                    OrcasError::Protocol(format!(
                        "unknown tracked thread `{}`",
                        command.tracked_thread_id
                    ))
                })?;
                ensure_active("tracked thread", &root.deleted_at, root.id.as_str())?;
                ensure_revision(
                    "tracked thread",
                    root.id.as_str(),
                    root.revision,
                    command.expected_revision,
                )?;
                let deleted_root = TrackedThreadRecord {
                    deleted_at: Some(command.metadata.issued_at),
                    updated_at: command.metadata.issued_at,
                    revision: root.revision.next(),
                    ..root
                };
                let event = AuthorityEventEnvelope {
                    metadata: EventMetadata {
                        event_id: authority::EventId::new(),
                        command_id: command.metadata.command_id.clone(),
                        aggregate_type: AggregateType::TrackedThread,
                        aggregate_id: deleted_root.id.to_string(),
                        aggregate_version: deleted_root.revision,
                        occurred_at: command.metadata.issued_at,
                        origin_node_id: command.metadata.origin_node_id.clone(),
                        causation_id: None,
                        correlation_id: command.metadata.correlation_id.clone(),
                    },
                    event: AuthorityEvent::TrackedThreadDeleted(authority::TrackedThreadDeleted {
                        tracked_thread_id: deleted_root.id.clone(),
                    }),
                };
                Self::append_event_envelope_tx(transaction, &event)?;
                Ok(AuthorityMutationResult::TrackedThread(deleted_root))
            }
        }
    }

    fn append_event_envelope_tx(
        transaction: &Transaction<'_>,
        envelope: &AuthorityEventEnvelope,
    ) -> OrcasResult<()> {
        let event_kind = enum_to_storage(envelope.event.kind())?;
        let aggregate_type = enum_to_storage(envelope.metadata.aggregate_type)?;
        let body_json = serde_json::to_string(&envelope.event)
            .map_err(|error| store_error(format!("serialize authority event: {error}")))?;
        transaction
            .execute(
                "insert into event_log (
                    event_id,
                    command_id,
                    aggregate_type,
                    aggregate_id,
                    aggregate_version,
                    event_kind,
                    occurred_at,
                    origin_node_id,
                    causation_id,
                    correlation_id,
                    body_json
                 ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    envelope.metadata.event_id.as_str(),
                    envelope.metadata.command_id.as_str(),
                    aggregate_type,
                    envelope.metadata.aggregate_id,
                    i64::try_from(envelope.metadata.aggregate_version.get()).map_err(|error| {
                        store_error(format!("store revision overflow: {error}"))
                    })?,
                    event_kind,
                    encode_datetime(envelope.metadata.occurred_at),
                    envelope.metadata.origin_node_id.as_str(),
                    envelope
                        .metadata
                        .causation_id
                        .as_ref()
                        .map(CausationId::as_str),
                    envelope
                        .metadata
                        .correlation_id
                        .as_ref()
                        .map(CorrelationId::as_str),
                    body_json
                ],
            )
            .map_err(map_sql_error)?;
        Self::apply_event_projection_tx(transaction, envelope)?;
        Ok(())
    }

    fn apply_event_projection_tx(
        transaction: &Transaction<'_>,
        envelope: &AuthorityEventEnvelope,
    ) -> OrcasResult<()> {
        match &envelope.event {
            AuthorityEvent::WorkstreamCreated(event) => {
                let record = &event.workstream;
                transaction
                    .execute(
                        "insert into workstreams (
                            id, title, objective, status, priority, revision, origin_node_id,
                            created_at, updated_at, deleted_at
                         ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        params![
                            record.id.as_str(),
                            record.title,
                            record.objective,
                            enum_to_storage(record.status)?,
                            record.priority,
                            i64::try_from(record.revision.get()).map_err(|error| store_error(
                                format!("store revision overflow: {error}")
                            ))?,
                            record.origin_node_id.as_str(),
                            encode_datetime(record.created_at),
                            encode_datetime(record.updated_at),
                            option_datetime(record.deleted_at),
                        ],
                    )
                    .map_err(map_sql_error)?;
            }
            AuthorityEvent::WorkstreamEdited(event) => {
                let current = Self::load_workstream_tx(transaction, &event.workstream_id)?
                    .ok_or_else(|| {
                        store_error(format!(
                            "projection missing workstream `{}`",
                            event.workstream_id
                        ))
                    })?;
                let updated = apply_workstream_patch(
                    current,
                    &event.changes,
                    envelope.metadata.aggregate_version,
                    envelope.metadata.occurred_at,
                )?;
                Self::upsert_workstream_tx(transaction, &updated)?;
            }
            AuthorityEvent::WorkstreamDeleted(event) => {
                let current = Self::load_workstream_tx(transaction, &event.workstream_id)?
                    .ok_or_else(|| {
                        store_error(format!(
                            "projection missing workstream `{}`",
                            event.workstream_id
                        ))
                    })?;
                let updated = WorkstreamRecord {
                    deleted_at: Some(envelope.metadata.occurred_at),
                    updated_at: envelope.metadata.occurred_at,
                    revision: envelope.metadata.aggregate_version,
                    ..current
                };
                Self::upsert_workstream_tx(transaction, &updated)?;
            }
            AuthorityEvent::WorkUnitCreated(event) => {
                Self::upsert_work_unit_tx(transaction, &event.work_unit)?;
            }
            AuthorityEvent::WorkUnitEdited(event) => {
                let current = Self::load_work_unit_tx(transaction, &event.work_unit_id)?
                    .ok_or_else(|| {
                        store_error(format!(
                            "projection missing work unit `{}`",
                            event.work_unit_id
                        ))
                    })?;
                let updated = apply_work_unit_patch(
                    current,
                    &event.changes,
                    envelope.metadata.aggregate_version,
                    envelope.metadata.occurred_at,
                )?;
                Self::upsert_work_unit_tx(transaction, &updated)?;
            }
            AuthorityEvent::WorkUnitDeleted(event) => {
                let current = Self::load_work_unit_tx(transaction, &event.work_unit_id)?
                    .ok_or_else(|| {
                        store_error(format!(
                            "projection missing work unit `{}`",
                            event.work_unit_id
                        ))
                    })?;
                let updated = WorkUnitRecord {
                    deleted_at: Some(envelope.metadata.occurred_at),
                    updated_at: envelope.metadata.occurred_at,
                    revision: envelope.metadata.aggregate_version,
                    ..current
                };
                Self::upsert_work_unit_tx(transaction, &updated)?;
            }
            AuthorityEvent::TrackedThreadCreated(event) => {
                Self::upsert_tracked_thread_tx(transaction, &event.tracked_thread)?;
            }
            AuthorityEvent::TrackedThreadEdited(event) => {
                let current = Self::load_tracked_thread_tx(transaction, &event.tracked_thread_id)?
                    .ok_or_else(|| {
                        store_error(format!(
                            "projection missing tracked thread `{}`",
                            event.tracked_thread_id
                        ))
                    })?;
                let updated = apply_tracked_thread_patch(
                    current,
                    &event.changes,
                    envelope.metadata.aggregate_version,
                    envelope.metadata.occurred_at,
                )?;
                Self::upsert_tracked_thread_tx(transaction, &updated)?;
            }
            AuthorityEvent::TrackedThreadDeleted(event) => {
                let current = Self::load_tracked_thread_tx(transaction, &event.tracked_thread_id)?
                    .ok_or_else(|| {
                        store_error(format!(
                            "projection missing tracked thread `{}`",
                            event.tracked_thread_id
                        ))
                    })?;
                let updated = TrackedThreadRecord {
                    deleted_at: Some(envelope.metadata.occurred_at),
                    updated_at: envelope.metadata.occurred_at,
                    revision: envelope.metadata.aggregate_version,
                    ..current
                };
                Self::upsert_tracked_thread_tx(transaction, &updated)?;
            }
        }

        let sequence = transaction.last_insert_rowid();
        transaction
            .execute(
                "insert into projection_checkpoint (projection_name, last_applied_sequence)
                 values (?1, ?2)
                 on conflict(projection_name)
                 do update set last_applied_sequence = excluded.last_applied_sequence",
                params![AUTHORITY_PROJECTION, sequence],
            )
            .map_err(map_sql_error)?;
        Ok(())
    }

    fn migrate(connection: &mut Connection) -> OrcasResult<()> {
        let user_version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|error| store_error(format!("read authority schema version: {error}")))?;
        match user_version {
            0 => {
                connection.execute_batch(INITIAL_SCHEMA).map_err(|error| {
                    store_error(format!("initialize authority schema: {error}"))
                })?;
                connection
                    .pragma_update(None, "user_version", 1_i64)
                    .map_err(|error| {
                        store_error(format!("set authority schema version: {error}"))
                    })?;
            }
            1 => {}
            other => {
                return Err(OrcasError::Store(format!(
                    "unsupported authority schema version `{other}`"
                )));
            }
        }
        Ok(())
    }

    fn ensure_origin_node_id(connection: &Connection) -> OrcasResult<OriginNodeId> {
        if let Some(value) = Self::meta_value(connection, META_ORIGIN_NODE_ID)? {
            return OriginNodeId::parse(value);
        }
        let origin_node_id = OriginNodeId::new();
        connection
            .execute(
                "insert or replace into store_meta (key, value) values (?1, ?2)",
                params![META_ORIGIN_NODE_ID, origin_node_id.as_str()],
            )
            .map_err(map_sql_error)?;
        Ok(origin_node_id)
    }

    fn bootstrap_from_json_if_needed(
        paths: &AppPaths,
        connection: &mut Connection,
        origin_node_id: &OriginNodeId,
    ) -> OrcasResult<()> {
        if Self::meta_value(connection, META_JSON_IMPORT_STATUS)?.is_some() {
            return Ok(());
        }
        if Self::has_existing_authority_data(connection)? {
            Self::set_meta(connection, META_JSON_IMPORT_STATUS, "existing_db")?;
            Self::set_meta(
                connection,
                META_JSON_IMPORT_COMPLETED_AT,
                &Utc::now().to_rfc3339(),
            )?;
            return Ok(());
        }
        if !paths.state_file.exists() {
            Self::set_meta(connection, META_JSON_IMPORT_STATUS, "no_state_json")?;
            Self::set_meta(
                connection,
                META_JSON_IMPORT_COMPLETED_AT,
                &Utc::now().to_rfc3339(),
            )?;
            return Ok(());
        }

        let raw = fs::read_to_string(&paths.state_file).map_err(OrcasError::Io)?;
        let stored: StoredState = serde_json::from_str(&raw)?;
        let transaction = connection
            .transaction()
            .map_err(|error| store_error(format!("start authority import transaction: {error}")))?;

        let mut workstreams = stored
            .collaboration
            .workstreams
            .into_values()
            .collect::<Vec<_>>();
        workstreams.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        for legacy in workstreams {
            let record = WorkstreamRecord {
                id: authority::WorkstreamId::parse(legacy.id)?,
                title: legacy.title,
                objective: legacy.objective,
                status: legacy.status,
                priority: legacy.priority,
                revision: Revision::initial(),
                origin_node_id: origin_node_id.clone(),
                created_at: legacy.created_at,
                updated_at: legacy.updated_at,
                deleted_at: None,
            };
            let envelope = AuthorityEventEnvelope {
                metadata: EventMetadata {
                    event_id: authority::EventId::new(),
                    command_id: CommandId::new(),
                    aggregate_type: AggregateType::Workstream,
                    aggregate_id: record.id.to_string(),
                    aggregate_version: Revision::initial(),
                    occurred_at: record.created_at,
                    origin_node_id: origin_node_id.clone(),
                    causation_id: None,
                    correlation_id: None,
                },
                event: AuthorityEvent::WorkstreamCreated(authority::WorkstreamCreated {
                    workstream: record,
                }),
            };
            Self::append_event_envelope_tx(&transaction, &envelope)?;
        }

        let mut work_units = stored
            .collaboration
            .work_units
            .into_values()
            .collect::<Vec<_>>();
        work_units.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        for legacy in work_units {
            let record = WorkUnitRecord {
                id: authority::WorkUnitId::parse(legacy.id)?,
                workstream_id: authority::WorkstreamId::parse(legacy.workstream_id)?,
                title: legacy.title,
                task_statement: legacy.task_statement,
                status: legacy.status,
                revision: Revision::initial(),
                origin_node_id: origin_node_id.clone(),
                created_at: legacy.created_at,
                updated_at: legacy.updated_at,
                deleted_at: None,
            };
            let envelope = AuthorityEventEnvelope {
                metadata: EventMetadata {
                    event_id: authority::EventId::new(),
                    command_id: CommandId::new(),
                    aggregate_type: AggregateType::WorkUnit,
                    aggregate_id: record.id.to_string(),
                    aggregate_version: Revision::initial(),
                    occurred_at: record.created_at,
                    origin_node_id: origin_node_id.clone(),
                    causation_id: None,
                    correlation_id: None,
                },
                event: AuthorityEvent::WorkUnitCreated(authority::WorkUnitCreated {
                    work_unit: record,
                }),
            };
            Self::append_event_envelope_tx(&transaction, &envelope)?;
        }

        transaction.commit().map_err(|error| {
            store_error(format!("commit authority import transaction: {error}"))
        })?;
        Self::set_meta(
            connection,
            META_JSON_IMPORT_STATUS,
            "imported_workstreams_work_units",
        )?;
        Self::set_meta(
            connection,
            META_JSON_IMPORT_COMPLETED_AT,
            &Utc::now().to_rfc3339(),
        )?;
        Ok(())
    }

    fn has_existing_authority_data(connection: &Connection) -> OrcasResult<bool> {
        for table in ["event_log", "workstreams", "work_units", "tracked_threads"] {
            let count = connection
                .query_row(&format!("select count(*) from {table}"), [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(map_sql_error)?;
            if count > 0 {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn set_meta(connection: &Connection, key: &str, value: &str) -> OrcasResult<()> {
        connection
            .execute(
                "insert or replace into store_meta (key, value) values (?1, ?2)",
                params![key, value],
            )
            .map_err(map_sql_error)?;
        Ok(())
    }

    fn meta_value(connection: &Connection, key: &str) -> OrcasResult<Option<String>> {
        connection
            .query_row(
                "select value from store_meta where key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sql_error)
    }

    fn load_command_receipt_tx(
        transaction: &Transaction<'_>,
        command_id: &CommandId,
    ) -> OrcasResult<Option<CommandReceipt>> {
        transaction
            .query_row(
                "select command_id, command_kind, aggregate_type, aggregate_id, accepted, response_json, recorded_at
                 from command_receipts
                 where command_id = ?1",
                params![command_id.as_str()],
                Self::read_command_receipt_row,
            )
            .optional()
            .map_err(map_sql_error)
    }

    fn insert_command_receipt_tx(
        transaction: &Transaction<'_>,
        receipt: &CommandReceipt,
    ) -> OrcasResult<()> {
        let response_json = receipt
            .response_json
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| store_error(format!("serialize command receipt response: {error}")))?;
        transaction
            .execute(
                "insert into command_receipts (
                    command_id, command_kind, aggregate_type, aggregate_id, accepted, response_json, recorded_at
                 ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    receipt.command_id.as_str(),
                    enum_to_storage(receipt.command_kind)?,
                    enum_to_storage(receipt.aggregate_key.aggregate_type)?,
                    receipt.aggregate_key.aggregate_id,
                    if receipt.accepted { 1_i64 } else { 0_i64 },
                    response_json,
                    encode_datetime(receipt.recorded_at)
                ],
            )
            .map_err(map_sql_error)?;
        Ok(())
    }

    fn upsert_workstream_tx(
        transaction: &Transaction<'_>,
        record: &WorkstreamRecord,
    ) -> OrcasResult<()> {
        transaction
            .execute(
                "insert into workstreams (
                    id, title, objective, status, priority, revision, origin_node_id, created_at, updated_at, deleted_at
                 ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 on conflict(id) do update set
                    title = excluded.title,
                    objective = excluded.objective,
                    status = excluded.status,
                    priority = excluded.priority,
                    revision = excluded.revision,
                    origin_node_id = excluded.origin_node_id,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    deleted_at = excluded.deleted_at",
                params![
                    record.id.as_str(),
                    record.title,
                    record.objective,
                    enum_to_storage(record.status)?,
                    record.priority,
                    i64::try_from(record.revision.get())
                        .map_err(|error| store_error(format!("store revision overflow: {error}")))?,
                    record.origin_node_id.as_str(),
                    encode_datetime(record.created_at),
                    encode_datetime(record.updated_at),
                    option_datetime(record.deleted_at),
                ],
            )
            .map_err(map_sql_error)?;
        Ok(())
    }

    fn upsert_work_unit_tx(
        transaction: &Transaction<'_>,
        record: &WorkUnitRecord,
    ) -> OrcasResult<()> {
        transaction
            .execute(
                "insert into work_units (
                    id, workstream_id, title, task_statement, status, revision, origin_node_id, created_at, updated_at, deleted_at
                 ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 on conflict(id) do update set
                    workstream_id = excluded.workstream_id,
                    title = excluded.title,
                    task_statement = excluded.task_statement,
                    status = excluded.status,
                    revision = excluded.revision,
                    origin_node_id = excluded.origin_node_id,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    deleted_at = excluded.deleted_at",
                params![
                    record.id.as_str(),
                    record.workstream_id.as_str(),
                    record.title,
                    record.task_statement,
                    enum_to_storage(record.status)?,
                    i64::try_from(record.revision.get())
                        .map_err(|error| store_error(format!("store revision overflow: {error}")))?,
                    record.origin_node_id.as_str(),
                    encode_datetime(record.created_at),
                    encode_datetime(record.updated_at),
                    option_datetime(record.deleted_at),
                ],
            )
            .map_err(map_sql_error)?;
        Ok(())
    }

    fn upsert_tracked_thread_tx(
        transaction: &Transaction<'_>,
        record: &TrackedThreadRecord,
    ) -> OrcasResult<()> {
        transaction
            .execute(
                "insert into tracked_threads (
                    id, work_unit_id, title, notes, backend_kind, upstream_thread_id, binding_state,
                    preferred_cwd, preferred_model, last_seen_turn_id, revision, origin_node_id,
                    created_at, updated_at, deleted_at
                 ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                 on conflict(id) do update set
                    work_unit_id = excluded.work_unit_id,
                    title = excluded.title,
                    notes = excluded.notes,
                    backend_kind = excluded.backend_kind,
                    upstream_thread_id = excluded.upstream_thread_id,
                    binding_state = excluded.binding_state,
                    preferred_cwd = excluded.preferred_cwd,
                    preferred_model = excluded.preferred_model,
                    last_seen_turn_id = excluded.last_seen_turn_id,
                    revision = excluded.revision,
                    origin_node_id = excluded.origin_node_id,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    deleted_at = excluded.deleted_at",
                params![
                    record.id.as_str(),
                    record.work_unit_id.as_str(),
                    record.title,
                    record.notes,
                    enum_to_storage(record.backend_kind)?,
                    record.upstream_thread_id,
                    enum_to_storage(record.binding_state)?,
                    record.preferred_cwd,
                    record.preferred_model,
                    record.last_seen_turn_id,
                    i64::try_from(record.revision.get()).map_err(|error| store_error(format!(
                        "store revision overflow: {error}"
                    )))?,
                    record.origin_node_id.as_str(),
                    encode_datetime(record.created_at),
                    encode_datetime(record.updated_at),
                    option_datetime(record.deleted_at),
                ],
            )
            .map_err(map_sql_error)?;
        Ok(())
    }

    fn load_workstream_tx(
        transaction: &Transaction<'_>,
        id: &authority::WorkstreamId,
    ) -> OrcasResult<Option<WorkstreamRecord>> {
        transaction
            .query_row(
                "select id, title, objective, status, priority, revision, origin_node_id, created_at, updated_at, deleted_at
                 from workstreams where id = ?1",
                params![id.as_str()],
                read_workstream_row,
            )
            .optional()
            .map_err(map_sql_error)
    }

    fn load_work_unit_tx(
        transaction: &Transaction<'_>,
        id: &authority::WorkUnitId,
    ) -> OrcasResult<Option<WorkUnitRecord>> {
        transaction
            .query_row(
                "select id, workstream_id, title, task_statement, status, revision, origin_node_id, created_at, updated_at, deleted_at
                 from work_units where id = ?1",
                params![id.as_str()],
                read_work_unit_row,
            )
            .optional()
            .map_err(map_sql_error)
    }

    fn load_tracked_thread_tx(
        transaction: &Transaction<'_>,
        id: &authority::TrackedThreadId,
    ) -> OrcasResult<Option<TrackedThreadRecord>> {
        transaction
            .query_row(
                "select id, work_unit_id, title, notes, backend_kind, upstream_thread_id, binding_state,
                        preferred_cwd, preferred_model, last_seen_turn_id, revision, origin_node_id,
                        created_at, updated_at, deleted_at
                 from tracked_threads where id = ?1",
                params![id.as_str()],
                read_tracked_thread_row,
            )
            .optional()
            .map_err(map_sql_error)
    }

    fn list_work_units_tx(
        transaction: &Transaction<'_>,
        workstream_id: Option<&authority::WorkstreamId>,
        include_deleted: bool,
    ) -> OrcasResult<Vec<WorkUnitSummary>> {
        let mut sql = "select id, workstream_id, title, task_statement, status, revision, origin_node_id, created_at, updated_at, deleted_at from work_units".to_string();
        match (workstream_id, include_deleted) {
            (Some(_), false) => {
                sql.push_str(" where workstream_id = ?1 and deleted_at is null");
            }
            (Some(_), true) => {
                sql.push_str(" where workstream_id = ?1");
            }
            (None, false) => {
                sql.push_str(" where deleted_at is null");
            }
            (None, true) => {}
        }
        sql.push_str(" order by updated_at desc, id asc");
        let mut statement = transaction.prepare(&sql).map_err(map_sql_error)?;
        let rows = if let Some(workstream_id) = workstream_id {
            statement
                .query_map(params![workstream_id.as_str()], read_work_unit_row)
                .map_err(map_sql_error)?
        } else {
            statement
                .query_map([], read_work_unit_row)
                .map_err(map_sql_error)?
        };
        rows.map(|row| row.map(|record| WorkUnitSummary::from(&record)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sql_error)
    }

    fn list_tracked_threads_tx(
        transaction: &Transaction<'_>,
        work_unit_id: &authority::WorkUnitId,
        include_deleted: bool,
    ) -> OrcasResult<Vec<TrackedThreadSummary>> {
        let sql = if include_deleted {
            "select id, work_unit_id, title, notes, backend_kind, upstream_thread_id, binding_state,
                    preferred_cwd, preferred_model, last_seen_turn_id, revision, origin_node_id,
                    created_at, updated_at, deleted_at
             from tracked_threads
             where work_unit_id = ?1
             order by updated_at desc, id asc"
        } else {
            "select id, work_unit_id, title, notes, backend_kind, upstream_thread_id, binding_state,
                    preferred_cwd, preferred_model, last_seen_turn_id, revision, origin_node_id,
                    created_at, updated_at, deleted_at
             from tracked_threads
             where work_unit_id = ?1 and deleted_at is null
             order by updated_at desc, id asc"
        };
        let mut statement = transaction.prepare(sql).map_err(map_sql_error)?;
        statement
            .query_map(params![work_unit_id.as_str()], read_tracked_thread_row)
            .map_err(map_sql_error)?
            .map(|row| row.map(|record| TrackedThreadSummary::from(&record)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sql_error)
    }

    fn ensure_upstream_binding_available_tx(
        transaction: &Transaction<'_>,
        upstream_thread_id: Option<&str>,
        excluding_id: Option<&authority::TrackedThreadId>,
    ) -> OrcasResult<()> {
        let Some(upstream_thread_id) = upstream_thread_id else {
            return Ok(());
        };
        let sql = if excluding_id.is_some() {
            "select id from tracked_threads
             where upstream_thread_id = ?1 and deleted_at is null and id != ?2
             limit 1"
        } else {
            "select id from tracked_threads
             where upstream_thread_id = ?1 and deleted_at is null
             limit 1"
        };
        let existing = if let Some(excluding_id) = excluding_id {
            transaction
                .query_row(
                    sql,
                    params![upstream_thread_id, excluding_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sql_error)?
        } else {
            transaction
                .query_row(sql, params![upstream_thread_id], |row| {
                    row.get::<_, String>(0)
                })
                .optional()
                .map_err(map_sql_error)?
        };
        if let Some(existing) = existing {
            return Err(OrcasError::Protocol(format!(
                "upstream thread `{upstream_thread_id}` is already tracked by `{existing}`"
            )));
        }
        Ok(())
    }

    fn with_connection<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> OrcasResult<T>,
    ) -> OrcasResult<T> {
        let mut guard = self
            .connection
            .lock()
            .map_err(|_| store_error("authority connection mutex poisoned"))?;
        f(&mut guard)
    }

    fn read_command_receipt_row(row: &Row<'_>) -> rusqlite::Result<CommandReceipt> {
        let response_json = row
            .get::<_, Option<String>>(5)?
            .map(|value| serde_json::from_str::<Value>(&value))
            .transpose()
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    5,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
        Ok(CommandReceipt {
            command_id: CommandId::parse(row.get::<_, String>(0)?)
                .map_err(protocol_to_sql_error(0))?,
            command_kind: enum_from_storage(&row.get::<_, String>(1)?)
                .map_err(protocol_to_sql_error(1))?,
            aggregate_key: AggregateKey {
                aggregate_type: enum_from_storage(&row.get::<_, String>(2)?)
                    .map_err(protocol_to_sql_error(2))?,
                aggregate_id: row.get(3)?,
            },
            accepted: row.get::<_, i64>(4)? != 0,
            response_json,
            recorded_at: decode_datetime(&row.get::<_, String>(6)?)
                .map_err(protocol_to_sql_error(6))?,
        })
    }
}

#[async_trait]
impl AuthorityCommandStore for AuthoritySqliteStore {
    async fn accept_command(&self, command: &AuthorityCommand) -> OrcasResult<CommandReceipt> {
        let _ = self.execute_command(command.clone()).await?;
        self.get_command(&command.metadata().command_id)
            .await?
            .ok_or_else(|| {
                OrcasError::Store(format!(
                    "authority command receipt missing for `{}`",
                    command.metadata().command_id
                ))
            })
    }

    async fn get_command(&self, command_id: &CommandId) -> OrcasResult<Option<CommandReceipt>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "select command_id, command_kind, aggregate_type, aggregate_id, accepted, response_json, recorded_at
                     from command_receipts
                     where command_id = ?1",
                    params![command_id.as_str()],
                    AuthoritySqliteStore::read_command_receipt_row,
                )
                .optional()
                .map_err(map_sql_error)
        })
    }
}

#[async_trait]
impl AuthorityEventStore for AuthoritySqliteStore {
    async fn append_events(
        &self,
        events: &[AuthorityEventEnvelope],
    ) -> OrcasResult<Vec<StoredAuthorityEvent>> {
        self.with_connection(|connection| {
            let transaction = connection.transaction().map_err(|error| {
                store_error(format!("start append events transaction: {error}"))
            })?;
            for event in events {
                Self::append_event_envelope_tx(&transaction, event)?;
            }
            let stored = Self::list_events_tx(&transaction, None, events.len())?;
            transaction.commit().map_err(|error| {
                store_error(format!("commit append events transaction: {error}"))
            })?;
            Ok(stored.into_iter().rev().take(events.len()).collect())
        })
    }

    async fn list_events(
        &self,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> OrcasResult<Vec<StoredAuthorityEvent>> {
        self.with_connection(|connection| {
            let transaction = connection
                .transaction()
                .map_err(|error| store_error(format!("start list events transaction: {error}")))?;
            let events = Self::list_events_tx(&transaction, after_sequence, limit)?;
            transaction
                .commit()
                .map_err(|error| store_error(format!("commit list events transaction: {error}")))?;
            Ok(events)
        })
    }
}

impl AuthoritySqliteStore {
    fn list_events_tx(
        transaction: &Transaction<'_>,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> OrcasResult<Vec<StoredAuthorityEvent>> {
        let limit = i64::try_from(limit.max(1))
            .map_err(|error| store_error(format!("event page limit overflow: {error}")))?;
        let sql = if after_sequence.is_some() {
            "select seq, event_id, command_id, aggregate_type, aggregate_id, aggregate_version,
                    event_kind, occurred_at, origin_node_id, causation_id, correlation_id, body_json
             from event_log
             where seq > ?1
             order by seq asc
             limit ?2"
        } else {
            "select seq, event_id, command_id, aggregate_type, aggregate_id, aggregate_version,
                    event_kind, occurred_at, origin_node_id, causation_id, correlation_id, body_json
             from event_log
             order by seq asc
             limit ?1"
        };
        let mut statement = transaction.prepare(sql).map_err(map_sql_error)?;
        let mapped = if let Some(after_sequence) = after_sequence {
            statement
                .query_map(
                    params![
                        i64::try_from(after_sequence).map_err(|error| {
                            store_error(format!("event sequence overflow: {error}"))
                        })?,
                        limit
                    ],
                    read_stored_event_row,
                )
                .map_err(map_sql_error)?
        } else {
            statement
                .query_map(params![limit], read_stored_event_row)
                .map_err(map_sql_error)?
        };
        mapped.collect::<Result<Vec<_>, _>>().map_err(map_sql_error)
    }
}

#[async_trait]
impl AuthorityProjectionStore for AuthoritySqliteStore {
    async fn load_projection_checkpoint(
        &self,
        projection_name: &str,
    ) -> OrcasResult<Option<ProjectionCheckpoint>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "select projection_name, last_applied_sequence
                     from projection_checkpoint where projection_name = ?1",
                    params![projection_name],
                    |row| {
                        Ok(ProjectionCheckpoint {
                            projection_name: row.get(0)?,
                            last_applied_sequence: u64::try_from(row.get::<_, i64>(1)?).map_err(
                                |error| {
                                    rusqlite::Error::FromSqlConversionFailure(
                                        1,
                                        rusqlite::types::Type::Integer,
                                        Box::new(error),
                                    )
                                },
                            )?,
                        })
                    },
                )
                .optional()
                .map_err(map_sql_error)
        })
    }

    async fn save_projection_checkpoint(
        &self,
        checkpoint: &ProjectionCheckpoint,
    ) -> OrcasResult<()> {
        self.with_connection(|connection| {
            connection
                .execute(
                    "insert into projection_checkpoint (projection_name, last_applied_sequence)
                     values (?1, ?2)
                     on conflict(projection_name)
                     do update set last_applied_sequence = excluded.last_applied_sequence",
                    params![
                        checkpoint.projection_name,
                        i64::try_from(checkpoint.last_applied_sequence).map_err(|error| {
                            store_error(format!("projection checkpoint overflow: {error}"))
                        })?
                    ],
                )
                .map_err(map_sql_error)?;
            Ok(())
        })
    }
}

#[async_trait]
impl AuthorityProjector for AuthoritySqliteStore {
    async fn apply(&self, event: &StoredAuthorityEvent) -> OrcasResult<()> {
        self.with_connection(|connection| {
            let transaction = connection
                .transaction()
                .map_err(|error| store_error(format!("start projector transaction: {error}")))?;
            Self::append_event_envelope_tx(&transaction, &event.envelope)?;
            transaction
                .commit()
                .map_err(|error| store_error(format!("commit projector transaction: {error}")))?;
            Ok(())
        })
    }
}

#[async_trait]
impl AuthorityQueryStore for AuthoritySqliteStore {
    async fn hierarchy_snapshot(&self, include_deleted: bool) -> OrcasResult<HierarchySnapshot> {
        let workstreams = self.list_workstreams(include_deleted).await?;
        let mut nodes = Vec::with_capacity(workstreams.len());
        for workstream in workstreams {
            let work_units = self
                .list_work_units(Some(&workstream.id), include_deleted)
                .await?;
            let mut work_unit_nodes = Vec::with_capacity(work_units.len());
            for work_unit in work_units {
                let tracked_threads = self
                    .list_tracked_threads(&work_unit.id, include_deleted)
                    .await?;
                work_unit_nodes.push(WorkUnitNode {
                    work_unit,
                    tracked_threads,
                });
            }
            nodes.push(WorkstreamNode {
                workstream,
                work_units: work_unit_nodes,
            });
        }
        Ok(HierarchySnapshot { workstreams: nodes })
    }

    async fn list_workstreams(&self, include_deleted: bool) -> OrcasResult<Vec<WorkstreamSummary>> {
        self.with_connection(|connection| {
            let sql = if include_deleted {
                "select id, title, objective, status, priority, revision, origin_node_id, created_at, updated_at, deleted_at
                 from workstreams
                 order by updated_at desc, id asc"
            } else {
                "select id, title, objective, status, priority, revision, origin_node_id, created_at, updated_at, deleted_at
                 from workstreams
                 where deleted_at is null
                 order by updated_at desc, id asc"
            };
            let mut statement = connection.prepare(sql).map_err(map_sql_error)?;
            statement
                .query_map([], read_workstream_row)
                .map_err(map_sql_error)?
                .map(|row| row.map(|record| WorkstreamSummary::from(&record)))
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sql_error)
        })
    }

    async fn get_workstream(
        &self,
        id: &authority::WorkstreamId,
    ) -> OrcasResult<Option<WorkstreamRecord>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "select id, title, objective, status, priority, revision, origin_node_id, created_at, updated_at, deleted_at
                     from workstreams where id = ?1",
                    params![id.as_str()],
                    read_workstream_row,
                )
                .optional()
                .map_err(map_sql_error)
        })
    }

    async fn list_work_units(
        &self,
        workstream_id: Option<&authority::WorkstreamId>,
        include_deleted: bool,
    ) -> OrcasResult<Vec<WorkUnitSummary>> {
        self.with_connection(|connection| {
            let transaction = connection.transaction().map_err(|error| {
                store_error(format!("start list work units transaction: {error}"))
            })?;
            let rows = Self::list_work_units_tx(&transaction, workstream_id, include_deleted)?;
            transaction.commit().map_err(|error| {
                store_error(format!("commit list work units transaction: {error}"))
            })?;
            Ok(rows)
        })
    }

    async fn get_work_unit(
        &self,
        id: &authority::WorkUnitId,
    ) -> OrcasResult<Option<WorkUnitRecord>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "select id, workstream_id, title, task_statement, status, revision, origin_node_id, created_at, updated_at, deleted_at
                     from work_units where id = ?1",
                    params![id.as_str()],
                    read_work_unit_row,
                )
                .optional()
                .map_err(map_sql_error)
        })
    }

    async fn list_tracked_threads(
        &self,
        work_unit_id: &authority::WorkUnitId,
        include_deleted: bool,
    ) -> OrcasResult<Vec<TrackedThreadSummary>> {
        self.with_connection(|connection| {
            let transaction = connection.transaction().map_err(|error| {
                store_error(format!("start list tracked threads transaction: {error}"))
            })?;
            let rows = Self::list_tracked_threads_tx(&transaction, work_unit_id, include_deleted)?;
            transaction.commit().map_err(|error| {
                store_error(format!("commit list tracked threads transaction: {error}"))
            })?;
            Ok(rows)
        })
    }

    async fn get_tracked_thread(
        &self,
        id: &authority::TrackedThreadId,
    ) -> OrcasResult<Option<TrackedThreadRecord>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "select id, work_unit_id, title, notes, backend_kind, upstream_thread_id, binding_state,
                            preferred_cwd, preferred_model, last_seen_turn_id, revision, origin_node_id,
                            created_at, updated_at, deleted_at
                     from tracked_threads where id = ?1",
                    params![id.as_str()],
                    read_tracked_thread_row,
                )
                .optional()
                .map_err(map_sql_error)
        })
    }

    async fn delete_plan(&self, target: &DeleteTarget) -> OrcasResult<Option<DeletePlan>> {
        self.with_connection(|connection| {
            let transaction = connection
                .transaction()
                .map_err(|error| store_error(format!("start delete plan transaction: {error}")))?;
            let plan = match target {
                DeleteTarget::Workstream { workstream_id } => {
                    let workstream = Self::load_workstream_tx(&transaction, workstream_id)?;
                    let Some(workstream) = workstream else {
                        transaction.commit().map_err(|error| {
                            store_error(format!("commit empty delete plan: {error}"))
                        })?;
                        return Ok(None);
                    };
                    let work_units =
                        Self::list_work_units_tx(&transaction, Some(workstream_id), false)?;
                    let mut tracked_threads = 0_u64;
                    let mut has_upstream_bindings = false;
                    for work_unit in &work_units {
                        let threads =
                            Self::list_tracked_threads_tx(&transaction, &work_unit.id, false)?;
                        tracked_threads += u64::try_from(threads.len()).map_err(|error| {
                            store_error(format!("tracked thread count overflow: {error}"))
                        })?;
                        has_upstream_bindings |= threads.iter().any(|thread| {
                            thread.upstream_thread_id.is_some() && thread.deleted_at.is_none()
                        });
                    }
                    Some(DeletePlan {
                        target: DeletePlanTarget {
                            aggregate_key: AggregateKey::workstream(workstream_id),
                            label: workstream.title,
                        },
                        expected_revision: workstream.revision,
                        affected_work_units: u64::try_from(work_units.len()).map_err(|error| {
                            store_error(format!("work unit count overflow: {error}"))
                        })?,
                        affected_tracked_threads: tracked_threads,
                        has_upstream_bindings,
                        confirmation_token: authority::DeleteToken::new(),
                        requires_typed_confirmation: !work_units.is_empty() || tracked_threads > 0,
                        expires_at: Utc::now() + chrono::TimeDelta::minutes(5),
                    })
                }
                DeleteTarget::WorkUnit { work_unit_id } => {
                    let work_unit = Self::load_work_unit_tx(&transaction, work_unit_id)?;
                    let Some(work_unit) = work_unit else {
                        transaction.commit().map_err(|error| {
                            store_error(format!("commit empty delete plan: {error}"))
                        })?;
                        return Ok(None);
                    };
                    let tracked_threads =
                        Self::list_tracked_threads_tx(&transaction, work_unit_id, false)?;
                    Some(DeletePlan {
                        target: DeletePlanTarget {
                            aggregate_key: AggregateKey::work_unit(work_unit_id),
                            label: work_unit.title,
                        },
                        expected_revision: work_unit.revision,
                        affected_work_units: 0,
                        affected_tracked_threads: u64::try_from(tracked_threads.len()).map_err(
                            |error| store_error(format!("tracked thread count overflow: {error}")),
                        )?,
                        has_upstream_bindings: tracked_threads
                            .iter()
                            .any(|thread| thread.upstream_thread_id.is_some()),
                        confirmation_token: authority::DeleteToken::new(),
                        requires_typed_confirmation: !tracked_threads.is_empty(),
                        expires_at: Utc::now() + chrono::TimeDelta::minutes(5),
                    })
                }
                DeleteTarget::TrackedThread { tracked_thread_id } => {
                    let tracked_thread =
                        Self::load_tracked_thread_tx(&transaction, tracked_thread_id)?;
                    let Some(tracked_thread) = tracked_thread else {
                        transaction.commit().map_err(|error| {
                            store_error(format!("commit empty delete plan: {error}"))
                        })?;
                        return Ok(None);
                    };
                    Some(DeletePlan {
                        target: DeletePlanTarget {
                            aggregate_key: AggregateKey::tracked_thread(tracked_thread_id),
                            label: tracked_thread.title,
                        },
                        expected_revision: tracked_thread.revision,
                        affected_work_units: 0,
                        affected_tracked_threads: 0,
                        has_upstream_bindings: tracked_thread.upstream_thread_id.is_some(),
                        confirmation_token: authority::DeleteToken::new(),
                        requires_typed_confirmation: false,
                        expires_at: Utc::now() + chrono::TimeDelta::minutes(5),
                    })
                }
            };
            transaction
                .commit()
                .map_err(|error| store_error(format!("commit delete plan transaction: {error}")))?;
            Ok(plan)
        })
    }
}

fn apply_workstream_patch(
    mut current: WorkstreamRecord,
    changes: &authority::WorkstreamPatch,
    revision: Revision,
    updated_at: DateTime<Utc>,
) -> OrcasResult<WorkstreamRecord> {
    if let Some(title) = &changes.title {
        current.title = require_non_empty(title.clone(), "title")?;
    }
    if let Some(objective) = &changes.objective {
        current.objective = require_non_empty(objective.clone(), "objective")?;
    }
    if let Some(status) = changes.status {
        current.status = status;
    }
    if let Some(priority) = &changes.priority {
        current.priority = require_non_empty(priority.clone(), "priority")?;
    }
    current.revision = revision;
    current.updated_at = updated_at;
    Ok(current)
}

fn apply_work_unit_patch(
    mut current: WorkUnitRecord,
    changes: &authority::WorkUnitPatch,
    revision: Revision,
    updated_at: DateTime<Utc>,
) -> OrcasResult<WorkUnitRecord> {
    if let Some(title) = &changes.title {
        current.title = require_non_empty(title.clone(), "title")?;
    }
    if let Some(task_statement) = &changes.task_statement {
        current.task_statement = require_non_empty(task_statement.clone(), "task_statement")?;
    }
    if let Some(status) = changes.status {
        current.status = status;
    }
    current.revision = revision;
    current.updated_at = updated_at;
    Ok(current)
}

fn apply_tracked_thread_patch(
    mut current: TrackedThreadRecord,
    changes: &authority::TrackedThreadPatch,
    revision: Revision,
    updated_at: DateTime<Utc>,
) -> OrcasResult<TrackedThreadRecord> {
    if let Some(title) = &changes.title {
        current.title = require_non_empty(title.clone(), "title")?;
    }
    if let Some(notes) = &changes.notes {
        current.notes = notes.clone();
    }
    if let Some(backend_kind) = changes.backend_kind {
        current.backend_kind = backend_kind;
    }
    if let Some(upstream_thread_id) = &changes.upstream_thread_id {
        current.upstream_thread_id = upstream_thread_id.clone();
        current.binding_state = if current.upstream_thread_id.is_some() {
            TrackedThreadBindingState::Bound
        } else {
            TrackedThreadBindingState::Unbound
        };
    }
    if let Some(binding_state) = changes.binding_state {
        current.binding_state = binding_state;
    }
    if let Some(preferred_cwd) = &changes.preferred_cwd {
        current.preferred_cwd = preferred_cwd.clone();
    }
    if let Some(preferred_model) = &changes.preferred_model {
        current.preferred_model = preferred_model.clone();
    }
    if let Some(last_seen_turn_id) = &changes.last_seen_turn_id {
        current.last_seen_turn_id = last_seen_turn_id.clone();
    }
    current.revision = revision;
    current.updated_at = updated_at;
    Ok(current)
}

fn ensure_active(kind: &str, deleted_at: &Option<DateTime<Utc>>, id: &str) -> OrcasResult<()> {
    if deleted_at.is_some() {
        return Err(OrcasError::Protocol(format!(
            "{kind} `{id}` has already been deleted locally"
        )));
    }
    Ok(())
}

fn ensure_revision(kind: &str, id: &str, current: Revision, expected: Revision) -> OrcasResult<()> {
    if current != expected {
        return Err(OrcasError::Protocol(format!(
            "{kind} `{id}` revision mismatch: expected {}, current {}",
            expected.get(),
            current.get()
        )));
    }
    Ok(())
}

fn require_non_empty(value: String, field: &str) -> OrcasResult<String> {
    if value.trim().is_empty() {
        Err(OrcasError::Protocol(format!("{field} cannot be empty")))
    } else {
        Ok(value)
    }
}

fn read_workstream_row(row: &Row<'_>) -> rusqlite::Result<WorkstreamRecord> {
    Ok(WorkstreamRecord {
        id: authority::WorkstreamId::parse(row.get::<_, String>(0)?)
            .map_err(protocol_to_sql_error(0))?,
        title: row.get(1)?,
        objective: row.get(2)?,
        status: enum_from_storage(&row.get::<_, String>(3)?).map_err(protocol_to_sql_error(3))?,
        priority: row.get(4)?,
        revision: Revision::new(u64::try_from(row.get::<_, i64>(5)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?),
        origin_node_id: OriginNodeId::parse(row.get::<_, String>(6)?)
            .map_err(protocol_to_sql_error(6))?,
        created_at: decode_datetime(&row.get::<_, String>(7)?).map_err(protocol_to_sql_error(7))?,
        updated_at: decode_datetime(&row.get::<_, String>(8)?).map_err(protocol_to_sql_error(8))?,
        deleted_at: row
            .get::<_, Option<String>>(9)?
            .map(|value| decode_datetime(&value))
            .transpose()
            .map_err(protocol_to_sql_error(9))?,
    })
}

fn read_work_unit_row(row: &Row<'_>) -> rusqlite::Result<WorkUnitRecord> {
    Ok(WorkUnitRecord {
        id: authority::WorkUnitId::parse(row.get::<_, String>(0)?)
            .map_err(protocol_to_sql_error(0))?,
        workstream_id: authority::WorkstreamId::parse(row.get::<_, String>(1)?)
            .map_err(protocol_to_sql_error(1))?,
        title: row.get(2)?,
        task_statement: row.get(3)?,
        status: enum_from_storage(&row.get::<_, String>(4)?).map_err(protocol_to_sql_error(4))?,
        revision: Revision::new(u64::try_from(row.get::<_, i64>(5)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?),
        origin_node_id: OriginNodeId::parse(row.get::<_, String>(6)?)
            .map_err(protocol_to_sql_error(6))?,
        created_at: decode_datetime(&row.get::<_, String>(7)?).map_err(protocol_to_sql_error(7))?,
        updated_at: decode_datetime(&row.get::<_, String>(8)?).map_err(protocol_to_sql_error(8))?,
        deleted_at: row
            .get::<_, Option<String>>(9)?
            .map(|value| decode_datetime(&value))
            .transpose()
            .map_err(protocol_to_sql_error(9))?,
    })
}

fn read_tracked_thread_row(row: &Row<'_>) -> rusqlite::Result<TrackedThreadRecord> {
    Ok(TrackedThreadRecord {
        id: authority::TrackedThreadId::parse(row.get::<_, String>(0)?)
            .map_err(protocol_to_sql_error(0))?,
        work_unit_id: authority::WorkUnitId::parse(row.get::<_, String>(1)?)
            .map_err(protocol_to_sql_error(1))?,
        title: row.get(2)?,
        notes: row.get(3)?,
        backend_kind: enum_from_storage(&row.get::<_, String>(4)?)
            .map_err(protocol_to_sql_error(4))?,
        upstream_thread_id: row.get(5)?,
        binding_state: enum_from_storage(&row.get::<_, String>(6)?)
            .map_err(protocol_to_sql_error(6))?,
        preferred_cwd: row.get(7)?,
        preferred_model: row.get(8)?,
        last_seen_turn_id: row.get(9)?,
        revision: Revision::new(u64::try_from(row.get::<_, i64>(10)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                10,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?),
        origin_node_id: OriginNodeId::parse(row.get::<_, String>(11)?)
            .map_err(protocol_to_sql_error(11))?,
        created_at: decode_datetime(&row.get::<_, String>(12)?)
            .map_err(protocol_to_sql_error(12))?,
        updated_at: decode_datetime(&row.get::<_, String>(13)?)
            .map_err(protocol_to_sql_error(13))?,
        deleted_at: row
            .get::<_, Option<String>>(14)?
            .map(|value| decode_datetime(&value))
            .transpose()
            .map_err(protocol_to_sql_error(14))?,
    })
}

fn read_stored_event_row(row: &Row<'_>) -> rusqlite::Result<StoredAuthorityEvent> {
    let body_json = row.get::<_, String>(11)?;
    let event = serde_json::from_str::<AuthorityEvent>(&body_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(11, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(StoredAuthorityEvent {
        sequence: u64::try_from(row.get::<_, i64>(0)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?,
        envelope: AuthorityEventEnvelope {
            metadata: EventMetadata {
                event_id: authority::EventId::parse(row.get::<_, String>(1)?)
                    .map_err(protocol_to_sql_error(1))?,
                command_id: CommandId::parse(row.get::<_, String>(2)?)
                    .map_err(protocol_to_sql_error(2))?,
                aggregate_type: enum_from_storage(&row.get::<_, String>(3)?)
                    .map_err(protocol_to_sql_error(3))?,
                aggregate_id: row.get(4)?,
                aggregate_version: Revision::new(u64::try_from(row.get::<_, i64>(5)?).map_err(
                    |error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            5,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    },
                )?),
                occurred_at: decode_datetime(&row.get::<_, String>(7)?)
                    .map_err(protocol_to_sql_error(7))?,
                origin_node_id: OriginNodeId::parse(row.get::<_, String>(8)?)
                    .map_err(protocol_to_sql_error(8))?,
                causation_id: row
                    .get::<_, Option<String>>(9)?
                    .map(CausationId::parse)
                    .transpose()
                    .map_err(protocol_to_sql_error(9))?,
                correlation_id: row
                    .get::<_, Option<String>>(10)?
                    .map(CorrelationId::parse)
                    .transpose()
                    .map_err(protocol_to_sql_error(10))?,
            },
            event,
        },
    })
}

fn option_datetime(value: Option<DateTime<Utc>>) -> Option<String> {
    value.map(encode_datetime)
}

fn encode_datetime(value: DateTime<Utc>) -> String {
    value.to_rfc3339()
}

fn decode_datetime(value: &str) -> OrcasResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| OrcasError::Store(format!("invalid stored datetime `{value}`: {error}")))
}

fn enum_to_storage<T: Serialize>(value: T) -> OrcasResult<String> {
    let value = serde_json::to_value(value)
        .map_err(|error| store_error(format!("encode enum: {error}")))?;
    value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
        OrcasError::Store("expected enum storage value to serialize as string".to_string())
    })
}

fn enum_from_storage<T: DeserializeOwned>(value: &str) -> OrcasResult<T> {
    serde_json::from_value(Value::String(value.to_string()))
        .map_err(|error| store_error(format!("decode enum `{value}`: {error}")))
}

fn map_sql_error(error: rusqlite::Error) -> OrcasError {
    store_error(error.to_string())
}

fn protocol_to_sql_error(index: usize) -> impl FnOnce(OrcasError) -> rusqlite::Error {
    move |error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::other(error.to_string())),
        )
    }
}

fn store_error(message: impl Into<String>) -> OrcasError {
    OrcasError::Store(message.into())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::TimeZone;

    use super::*;
    use orcas_core::authority::{
        CommandActor, CommandMetadata, CreateTrackedThread, CreateWorkUnit, CreateWorkstream,
        DeleteTrackedThread, DeleteWorkUnit, DeleteWorkstream, EditTrackedThread, EditWorkUnit,
        EditWorkstream, TrackedThreadBackendKind, TrackedThreadPatch, WorkUnitPatch,
        WorkstreamPatch,
    };
    use orcas_core::collaboration::{CollaborationState, WorkUnit, Workstream};
    use orcas_core::{WorkUnitStatus, WorkstreamStatus};

    fn temp_paths(name: &str) -> AppPaths {
        let root = std::env::temp_dir().join(format!(
            "orcas-authority-store-{name}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        AppPaths::from_roots(root.join("config"), root.join("data"), root.join("runtime"))
    }

    fn metadata(origin_node_id: &OriginNodeId, suffix: &str) -> CommandMetadata {
        CommandMetadata {
            command_id: CommandId::parse(format!("command-{suffix}")).expect("command id"),
            issued_at: Utc
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("timestamp"),
            origin_node_id: origin_node_id.clone(),
            actor: CommandActor::parse("test_operator").expect("actor"),
            correlation_id: Some(
                CorrelationId::parse(format!("corr-{suffix}")).expect("correlation id"),
            ),
        }
    }

    fn fresh_store(name: &str) -> AuthoritySqliteStore {
        let paths = temp_paths(name);
        std::fs::create_dir_all(&paths.data_dir).expect("data dir");
        AuthoritySqliteStore::open(paths).expect("authority store")
    }

    #[test]
    fn schema_bootstrap_creates_state_db_and_origin_node_id() {
        let store = fresh_store("bootstrap");

        assert!(store.database_path().exists());
        let origin_node_id = store.origin_node_id().expect("origin node id");
        assert!(!origin_node_id.as_str().is_empty());
    }

    #[tokio::test]
    async fn create_edit_delete_workstream_work_unit_and_tracked_thread_persist() {
        let store = fresh_store("crud");
        let origin_node_id = store.origin_node_id().expect("origin");

        let workstream = match store
            .execute_command(AuthorityCommand::CreateWorkstream(CreateWorkstream {
                metadata: metadata(&origin_node_id, "ws-create"),
                workstream_id: authority::WorkstreamId::parse("ws-1").expect("workstream id"),
                title: "Authority MVP".to_string(),
                objective: "Make state.db real".to_string(),
                status: WorkstreamStatus::Active,
                priority: "high".to_string(),
            }))
            .await
            .expect("create workstream")
        {
            AuthorityMutationResult::Workstream(record) => record,
            _ => panic!("unexpected mutation result"),
        };

        let workstream = match store
            .execute_command(AuthorityCommand::EditWorkstream(EditWorkstream {
                metadata: metadata(&origin_node_id, "ws-edit"),
                workstream_id: workstream.id.clone(),
                expected_revision: workstream.revision,
                changes: WorkstreamPatch {
                    title: Some("Authority MVP backend".to_string()),
                    objective: None,
                    status: None,
                    priority: None,
                },
            }))
            .await
            .expect("edit workstream")
        {
            AuthorityMutationResult::Workstream(record) => record,
            _ => panic!("unexpected mutation result"),
        };

        let work_unit = match store
            .execute_command(AuthorityCommand::CreateWorkUnit(CreateWorkUnit {
                metadata: metadata(&origin_node_id, "wu-create"),
                work_unit_id: authority::WorkUnitId::parse("wu-1").expect("work unit id"),
                workstream_id: workstream.id.clone(),
                title: "Implement store".to_string(),
                task_statement: "Use SQLite and projections".to_string(),
                status: WorkUnitStatus::Ready,
            }))
            .await
            .expect("create work unit")
        {
            AuthorityMutationResult::WorkUnit(record) => record,
            _ => panic!("unexpected mutation result"),
        };

        let work_unit = match store
            .execute_command(AuthorityCommand::EditWorkUnit(EditWorkUnit {
                metadata: metadata(&origin_node_id, "wu-edit"),
                work_unit_id: work_unit.id.clone(),
                expected_revision: work_unit.revision,
                changes: WorkUnitPatch {
                    title: None,
                    task_statement: Some(
                        "Use SQLite, projections, and explicit events".to_string(),
                    ),
                    status: Some(WorkUnitStatus::Running),
                },
            }))
            .await
            .expect("edit work unit")
        {
            AuthorityMutationResult::WorkUnit(record) => record,
            _ => panic!("unexpected mutation result"),
        };

        let tracked_thread = match store
            .execute_command(AuthorityCommand::CreateTrackedThread(CreateTrackedThread {
                metadata: metadata(&origin_node_id, "tt-create"),
                tracked_thread_id: authority::TrackedThreadId::parse("tt-1")
                    .expect("tracked thread id"),
                work_unit_id: work_unit.id.clone(),
                title: "Codex lane".to_string(),
                notes: Some("Local record only".to_string()),
                backend_kind: TrackedThreadBackendKind::Codex,
                upstream_thread_id: Some("upstream-1".to_string()),
                preferred_cwd: Some("/tmp/orcas".to_string()),
                preferred_model: Some("gpt-5.4".to_string()),
            }))
            .await
            .expect("create tracked thread")
        {
            AuthorityMutationResult::TrackedThread(record) => record,
            _ => panic!("unexpected mutation result"),
        };

        let tracked_thread = match store
            .execute_command(AuthorityCommand::EditTrackedThread(EditTrackedThread {
                metadata: metadata(&origin_node_id, "tt-edit"),
                tracked_thread_id: tracked_thread.id.clone(),
                expected_revision: tracked_thread.revision,
                changes: TrackedThreadPatch {
                    title: None,
                    notes: Some(Some("Updated local notes".to_string())),
                    backend_kind: None,
                    upstream_thread_id: None,
                    binding_state: Some(TrackedThreadBindingState::Detached),
                    preferred_cwd: None,
                    preferred_model: None,
                    last_seen_turn_id: Some(Some("turn-2".to_string())),
                },
            }))
            .await
            .expect("edit tracked thread")
        {
            AuthorityMutationResult::TrackedThread(record) => record,
            _ => panic!("unexpected mutation result"),
        };

        let hierarchy = store
            .hierarchy_snapshot(false)
            .await
            .expect("hierarchy snapshot");
        assert_eq!(hierarchy.workstreams.len(), 1);
        assert_eq!(hierarchy.workstreams[0].work_units.len(), 1);
        assert_eq!(
            hierarchy.workstreams[0].work_units[0].tracked_threads.len(),
            1
        );
        assert_eq!(
            hierarchy.workstreams[0].workstream.title,
            "Authority MVP backend"
        );
        assert_eq!(
            hierarchy.workstreams[0].work_units[0].work_unit.status,
            WorkUnitStatus::Running
        );
        assert_eq!(
            hierarchy.workstreams[0].work_units[0].tracked_threads[0].binding_state,
            TrackedThreadBindingState::Detached
        );

        let deleted_thread = store
            .execute_command(AuthorityCommand::DeleteTrackedThread(DeleteTrackedThread {
                metadata: metadata(&origin_node_id, "tt-delete"),
                tracked_thread_id: tracked_thread.id.clone(),
                expected_revision: tracked_thread.revision,
                delete_token: authority::DeleteToken::parse("tt-delete-token")
                    .expect("delete token"),
            }))
            .await
            .expect("delete tracked thread");
        let deleted_work_unit = store
            .execute_command(AuthorityCommand::DeleteWorkUnit(DeleteWorkUnit {
                metadata: metadata(&origin_node_id, "wu-delete"),
                work_unit_id: work_unit.id.clone(),
                expected_revision: work_unit.revision,
                delete_token: authority::DeleteToken::parse("wu-delete-token")
                    .expect("delete token"),
            }))
            .await
            .expect("delete work unit");
        let deleted_workstream = store
            .execute_command(AuthorityCommand::DeleteWorkstream(DeleteWorkstream {
                metadata: metadata(&origin_node_id, "ws-delete"),
                workstream_id: workstream.id.clone(),
                expected_revision: workstream.revision,
                delete_token: authority::DeleteToken::parse("ws-delete-token")
                    .expect("delete token"),
            }))
            .await
            .expect("delete workstream");

        match deleted_thread {
            AuthorityMutationResult::TrackedThread(record) => {
                assert!(record.deleted_at.is_some());
                assert_eq!(record.upstream_thread_id.as_deref(), Some("upstream-1"));
            }
            _ => panic!("unexpected mutation result"),
        }
        match deleted_work_unit {
            AuthorityMutationResult::WorkUnit(record) => assert!(record.deleted_at.is_some()),
            _ => panic!("unexpected mutation result"),
        }
        match deleted_workstream {
            AuthorityMutationResult::Workstream(record) => assert!(record.deleted_at.is_some()),
            _ => panic!("unexpected mutation result"),
        }
        assert!(
            store
                .hierarchy_snapshot(false)
                .await
                .expect("active hierarchy")
                .workstreams
                .is_empty()
        );
        assert_eq!(
            store.list_events(None, 64).await.expect("event log").len(),
            9
        );
    }

    #[tokio::test]
    async fn parent_delete_cascades_to_children_as_explicit_events() {
        let store = fresh_store("cascade");
        let origin_node_id = store.origin_node_id().expect("origin");
        let workstream_id = authority::WorkstreamId::parse("ws-cascade").expect("workstream id");
        let work_unit_id = authority::WorkUnitId::parse("wu-cascade").expect("work unit id");
        let tracked_thread_id =
            authority::TrackedThreadId::parse("tt-cascade").expect("tracked thread id");

        store
            .execute_command(AuthorityCommand::CreateWorkstream(CreateWorkstream {
                metadata: metadata(&origin_node_id, "cascade-ws-create"),
                workstream_id: workstream_id.clone(),
                title: "Cascade".to_string(),
                objective: "Delete tree".to_string(),
                status: WorkstreamStatus::Active,
                priority: "normal".to_string(),
            }))
            .await
            .expect("create workstream");
        let work_unit = match store
            .execute_command(AuthorityCommand::CreateWorkUnit(CreateWorkUnit {
                metadata: metadata(&origin_node_id, "cascade-wu-create"),
                work_unit_id: work_unit_id.clone(),
                workstream_id: workstream_id.clone(),
                title: "Leaf".to_string(),
                task_statement: "Delete".to_string(),
                status: WorkUnitStatus::Ready,
            }))
            .await
            .expect("create work unit")
        {
            AuthorityMutationResult::WorkUnit(record) => record,
            _ => panic!("unexpected mutation result"),
        };
        store
            .execute_command(AuthorityCommand::CreateTrackedThread(CreateTrackedThread {
                metadata: metadata(&origin_node_id, "cascade-tt-create"),
                tracked_thread_id: tracked_thread_id.clone(),
                work_unit_id: work_unit_id.clone(),
                title: "Thread".to_string(),
                notes: None,
                backend_kind: TrackedThreadBackendKind::Codex,
                upstream_thread_id: Some("upstream-cascade".to_string()),
                preferred_cwd: None,
                preferred_model: None,
            }))
            .await
            .expect("create tracked thread");

        let _ = store
            .execute_command(AuthorityCommand::DeleteWorkstream(DeleteWorkstream {
                metadata: metadata(&origin_node_id, "cascade-ws-delete"),
                workstream_id: workstream_id.clone(),
                expected_revision: Revision::initial(),
                delete_token: authority::DeleteToken::parse("cascade-delete")
                    .expect("delete token"),
            }))
            .await
            .expect("delete workstream");

        let events = store.list_events(None, 32).await.expect("events");
        assert!(
            events
                .iter()
                .any(|event| matches!(event.envelope.event, AuthorityEvent::WorkstreamDeleted(_)))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event.envelope.event, AuthorityEvent::WorkUnitDeleted(_)))
        );
        assert!(events.iter().any(|event| matches!(
            event.envelope.event,
            AuthorityEvent::TrackedThreadDeleted(_)
        )));
        assert!(
            store
                .get_work_unit(&work_unit.id)
                .await
                .expect("load work unit")
                .expect("work unit")
                .deleted_at
                .is_some()
        );
    }

    #[tokio::test]
    async fn invalid_expected_revision_is_rejected() {
        let store = fresh_store("revision");
        let origin_node_id = store.origin_node_id().expect("origin");
        let workstream = match store
            .execute_command(AuthorityCommand::CreateWorkstream(CreateWorkstream {
                metadata: metadata(&origin_node_id, "revision-create"),
                workstream_id: authority::WorkstreamId::parse("ws-revision")
                    .expect("workstream id"),
                title: "Revision".to_string(),
                objective: "Reject stale edits".to_string(),
                status: WorkstreamStatus::Active,
                priority: "normal".to_string(),
            }))
            .await
            .expect("create workstream")
        {
            AuthorityMutationResult::Workstream(record) => record,
            _ => panic!("unexpected mutation result"),
        };

        let error = store
            .execute_command(AuthorityCommand::EditWorkstream(EditWorkstream {
                metadata: metadata(&origin_node_id, "revision-edit"),
                workstream_id: workstream.id.clone(),
                expected_revision: Revision::new(7),
                changes: WorkstreamPatch {
                    title: Some("wrong".to_string()),
                    objective: None,
                    status: None,
                    priority: None,
                },
            }))
            .await
            .expect_err("stale revision should fail");

        assert!(error.to_string().contains("revision mismatch"));
    }

    #[tokio::test]
    async fn restart_reloads_projection_state_from_sqlite() {
        let paths = temp_paths("restart");
        std::fs::create_dir_all(&paths.data_dir).expect("data dir");
        let store = AuthoritySqliteStore::open(paths.clone()).expect("store");
        let origin_node_id = store.origin_node_id().expect("origin");
        let _ = store
            .execute_command(AuthorityCommand::CreateWorkstream(CreateWorkstream {
                metadata: metadata(&origin_node_id, "restart-create"),
                workstream_id: authority::WorkstreamId::parse("ws-restart").expect("workstream id"),
                title: "Restart".to_string(),
                objective: "Persist".to_string(),
                status: WorkstreamStatus::Active,
                priority: "normal".to_string(),
            }))
            .await
            .expect("create workstream");
        drop(store);

        let reopened = AuthoritySqliteStore::open(paths).expect("reopen store");
        let workstreams = reopened.list_workstreams(false).await.expect("workstreams");
        assert_eq!(workstreams.len(), 1);
        assert_eq!(workstreams[0].id.as_str(), "ws-restart");
    }

    #[test]
    fn one_time_import_bootstraps_workstreams_and_work_units_from_state_json() {
        let paths = temp_paths("import");
        std::fs::create_dir_all(&paths.data_dir).expect("data dir");
        let state = StoredState {
            collaboration: CollaborationState {
                workstreams: BTreeMap::from([(
                    "ws-import".to_string(),
                    Workstream {
                        id: "ws-import".to_string(),
                        title: "Imported stream".to_string(),
                        objective: "Bootstrap".to_string(),
                        status: WorkstreamStatus::Active,
                        priority: "normal".to_string(),
                        created_at: Utc
                            .with_ymd_and_hms(2026, 3, 18, 10, 0, 0)
                            .single()
                            .expect("timestamp"),
                        updated_at: Utc
                            .with_ymd_and_hms(2026, 3, 18, 10, 0, 0)
                            .single()
                            .expect("timestamp"),
                    },
                )]),
                work_units: BTreeMap::from([(
                    "wu-import".to_string(),
                    WorkUnit {
                        id: "wu-import".to_string(),
                        workstream_id: "ws-import".to_string(),
                        title: "Imported unit".to_string(),
                        task_statement: "Bootstrap unit".to_string(),
                        status: WorkUnitStatus::Ready,
                        dependencies: Vec::new(),
                        latest_report_id: None,
                        current_assignment_id: None,
                        created_at: Utc
                            .with_ymd_and_hms(2026, 3, 18, 10, 5, 0)
                            .single()
                            .expect("timestamp"),
                        updated_at: Utc
                            .with_ymd_and_hms(2026, 3, 18, 10, 5, 0)
                            .single()
                            .expect("timestamp"),
                    },
                )]),
                ..CollaborationState::default()
            },
            ..StoredState::default()
        };
        std::fs::write(
            &paths.state_file,
            serde_json::to_string_pretty(&state).expect("serialize state"),
        )
        .expect("write state");

        let store = AuthoritySqliteStore::open(paths.clone()).expect("store");
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let workstreams = runtime
            .block_on(store.list_workstreams(false))
            .expect("workstreams");
        let hierarchy = runtime
            .block_on(store.hierarchy_snapshot(false))
            .expect("hierarchy");
        drop(store);

        assert_eq!(workstreams.len(), 1);
        assert_eq!(workstreams[0].id.as_str(), "ws-import");
        assert_eq!(hierarchy.workstreams.len(), 1);
        assert_eq!(hierarchy.workstreams[0].work_units.len(), 1);

        let reopened = AuthoritySqliteStore::open(paths).expect("reopen store");
        let workstreams = runtime
            .block_on(reopened.list_workstreams(false))
            .expect("workstreams after reopen");
        assert_eq!(workstreams.len(), 1);
    }
}
