//! Canonical authority planning domain, commands, events, and read/write
//! vocabulary.
//!
//! This module defines the planning-side model that owns workstreams,
//! work-units, and tracked-thread binding records. It is the canonical source
//! of truth for planning hierarchy data; read `collaboration.rs` for the
//! execution/runtime model and `ipc.rs` for the public RPC and event surfaces
//! that expose the authority vocabulary to clients.
//!
//! Tracked threads are Orcas-owned local binding records. They may reference an
//! upstream thread identifier, but that does not imply upstream ownership or
//! daemon-owned PTY session ownership.

use std::fmt::{Display, Formatter};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{OrcasError, OrcasResult, WorkUnitStatus, WorkstreamStatus};

fn require_non_empty(value: impl Into<String>, field: &str) -> OrcasResult<String> {
    let value = value.into();
    if value.trim().is_empty() {
        Err(OrcasError::Protocol(format!("{field} cannot be empty")))
    } else {
        Ok(value)
    }
}

macro_rules! uuid_backed_type {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7().to_string())
            }

            pub fn parse(value: impl Into<String>) -> OrcasResult<Self> {
                Ok(Self(require_non_empty(value, stringify!($name))?))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Display for $name {
            fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

macro_rules! non_empty_string_type {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> OrcasResult<Self> {
                Ok(Self(require_non_empty(value, stringify!($name))?))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Display for $name {
            fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

uuid_backed_type!(WorkstreamId);
uuid_backed_type!(WorkUnitId);
uuid_backed_type!(TrackedThreadId);
uuid_backed_type!(CommandId);
uuid_backed_type!(EventId);
uuid_backed_type!(OriginNodeId);
uuid_backed_type!(CorrelationId);
uuid_backed_type!(CausationId);
uuid_backed_type!(DeleteToken);
non_empty_string_type!(CommandActor);

/// Monotonic planning revision used for optimistic concurrency and tombstone
/// ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Revision(u64);

impl Revision {
    #[must_use]
    pub const fn initial() -> Self {
        Self(1)
    }

    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

impl Default for Revision {
    fn default() -> Self {
        Self::initial()
    }
}

/// Planning aggregate kinds supported by the authority model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateType {
    Workstream,
    WorkUnit,
    TrackedThread,
}

/// Canonical command verbs for authority-owned planning aggregates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    CreateWorkstream,
    EditWorkstream,
    DeleteWorkstream,
    CreateWorkUnit,
    EditWorkUnit,
    DeleteWorkUnit,
    CreateTrackedThread,
    EditTrackedThread,
    DeleteTrackedThread,
}

/// Canonical event verbs emitted by the authority store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    WorkstreamCreated,
    WorkstreamEdited,
    WorkstreamDeleted,
    WorkUnitCreated,
    WorkUnitEdited,
    WorkUnitDeleted,
    TrackedThreadCreated,
    TrackedThreadEdited,
    TrackedThreadDeleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AggregateKey {
    pub aggregate_type: AggregateType,
    pub aggregate_id: String,
}

impl AggregateKey {
    #[must_use]
    pub fn workstream(id: &WorkstreamId) -> Self {
        Self {
            aggregate_type: AggregateType::Workstream,
            aggregate_id: id.to_string(),
        }
    }

    #[must_use]
    pub fn work_unit(id: &WorkUnitId) -> Self {
        Self {
            aggregate_type: AggregateType::WorkUnit,
            aggregate_id: id.to_string(),
        }
    }

    #[must_use]
    pub fn tracked_thread(id: &TrackedThreadId) -> Self {
        Self {
            aggregate_type: AggregateType::TrackedThread,
            aggregate_id: id.to_string(),
        }
    }
}

/// Metadata carried by every authority command.
///
/// This identifies the command, actor, origin, and correlation context used by
/// the command store and event store. It is part of the canonical planning
/// vocabulary, not the collaboration/runtime model.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CommandMetadata {
    pub command_id: CommandId,
    pub issued_at: DateTime<Utc>,
    pub origin_node_id: OriginNodeId,
    pub actor: CommandActor,
    pub correlation_id: Option<CorrelationId>,
}

impl CommandMetadata {
    #[must_use]
    pub fn new(origin_node_id: OriginNodeId, actor: CommandActor) -> Self {
        Self {
            command_id: CommandId::new(),
            issued_at: Utc::now(),
            origin_node_id,
            actor,
            correlation_id: None,
        }
    }
}

/// Metadata carried by every authority event.
///
/// The aggregate version and causation/correlation fields are what make the
/// authority model suitable for optimistic concurrency and tombstone ordering.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventMetadata {
    pub event_id: EventId,
    pub command_id: CommandId,
    pub aggregate_type: AggregateType,
    pub aggregate_id: String,
    pub aggregate_version: Revision,
    pub occurred_at: DateTime<Utc>,
    pub origin_node_id: OriginNodeId,
    pub causation_id: Option<CausationId>,
    pub correlation_id: Option<CorrelationId>,
}

impl EventMetadata {
    #[must_use]
    pub fn new(
        command_id: CommandId,
        aggregate_key: AggregateKey,
        aggregate_version: Revision,
        origin_node_id: OriginNodeId,
    ) -> Self {
        Self {
            event_id: EventId::new(),
            command_id,
            aggregate_type: aggregate_key.aggregate_type,
            aggregate_id: aggregate_key.aggregate_id,
            aggregate_version,
            occurred_at: Utc::now(),
            origin_node_id,
            causation_id: None,
            correlation_id: None,
        }
    }
}

/// Patch for a canonical workstream record.
///
/// Empty patches are meaningful as validation inputs, but not as updates.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkstreamPatch {
    pub title: Option<String>,
    pub objective: Option<String>,
    pub status: Option<WorkstreamStatus>,
    pub priority: Option<String>,
}

impl WorkstreamPatch {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.objective.is_none()
            && self.status.is_none()
            && self.priority.is_none()
    }
}

/// Patch for a canonical work-unit record.
///
/// Empty patches are meaningful as validation inputs, but not as updates.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkUnitPatch {
    pub title: Option<String>,
    pub task_statement: Option<String>,
    pub status: Option<WorkUnitStatus>,
}

impl WorkUnitPatch {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.title.is_none() && self.task_statement.is_none() && self.status.is_none()
    }
}

/// Backend families supported by tracked-thread binding records.
///
/// The backend kind identifies the upstream integration, not PTY ownership.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrackedThreadBackendKind {
    #[default]
    Codex,
}

/// Binding state for a tracked thread record.
///
/// This captures how the Orcas-owned binding relates to an upstream thread
/// identifier. It does not imply that Orcas owns the upstream thread or the
/// PTY session used to attach to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrackedThreadBindingState {
    #[default]
    Unbound,
    Bound,
    Detached,
    Missing,
}

/// Patch for a tracked-thread binding record.
///
/// `upstream_thread_id` and the other optional fields are optional updates, not
/// claims of ownership over the upstream thread or PTY session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TrackedThreadPatch {
    pub title: Option<String>,
    pub notes: Option<Option<String>>,
    pub backend_kind: Option<TrackedThreadBackendKind>,
    pub upstream_thread_id: Option<Option<String>>,
    pub binding_state: Option<TrackedThreadBindingState>,
    pub preferred_cwd: Option<Option<String>>,
    pub preferred_model: Option<Option<String>>,
    pub last_seen_turn_id: Option<Option<String>>,
}

impl TrackedThreadPatch {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.notes.is_none()
            && self.backend_kind.is_none()
            && self.upstream_thread_id.is_none()
            && self.binding_state.is_none()
            && self.preferred_cwd.is_none()
            && self.preferred_model.is_none()
            && self.last_seen_turn_id.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CreateWorkstream {
    pub metadata: CommandMetadata,
    pub workstream_id: WorkstreamId,
    pub title: String,
    pub objective: String,
    pub status: WorkstreamStatus,
    pub priority: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EditWorkstream {
    pub metadata: CommandMetadata,
    pub workstream_id: WorkstreamId,
    pub expected_revision: Revision,
    pub changes: WorkstreamPatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeleteWorkstream {
    pub metadata: CommandMetadata,
    pub workstream_id: WorkstreamId,
    pub expected_revision: Revision,
    pub delete_token: DeleteToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CreateWorkUnit {
    pub metadata: CommandMetadata,
    pub work_unit_id: WorkUnitId,
    pub workstream_id: WorkstreamId,
    pub title: String,
    pub task_statement: String,
    pub status: WorkUnitStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EditWorkUnit {
    pub metadata: CommandMetadata,
    pub work_unit_id: WorkUnitId,
    pub expected_revision: Revision,
    pub changes: WorkUnitPatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeleteWorkUnit {
    pub metadata: CommandMetadata,
    pub work_unit_id: WorkUnitId,
    pub expected_revision: Revision,
    pub delete_token: DeleteToken,
}

/// Command that creates a tracked-thread binding record.
///
/// `upstream_thread_id` is an optional reference to an upstream thread, not a
/// claim that Orcas owns that thread or its PTY session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CreateTrackedThread {
    pub metadata: CommandMetadata,
    pub tracked_thread_id: TrackedThreadId,
    pub work_unit_id: WorkUnitId,
    pub title: String,
    pub notes: Option<String>,
    pub backend_kind: TrackedThreadBackendKind,
    pub upstream_thread_id: Option<String>,
    pub preferred_cwd: Option<String>,
    pub preferred_model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EditTrackedThread {
    pub metadata: CommandMetadata,
    pub tracked_thread_id: TrackedThreadId,
    pub expected_revision: Revision,
    pub changes: TrackedThreadPatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeleteTrackedThread {
    pub metadata: CommandMetadata,
    pub tracked_thread_id: TrackedThreadId,
    pub expected_revision: Revision,
    pub delete_token: DeleteToken,
}

/// Canonical command carrier for planning hierarchy mutations.
///
/// These commands own the authority-side write vocabulary and are the only
/// supported path for planning hierarchy creation, edit, and delete behavior.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthorityCommand {
    CreateWorkstream(CreateWorkstream),
    EditWorkstream(EditWorkstream),
    DeleteWorkstream(DeleteWorkstream),
    CreateWorkUnit(CreateWorkUnit),
    EditWorkUnit(EditWorkUnit),
    DeleteWorkUnit(DeleteWorkUnit),
    CreateTrackedThread(CreateTrackedThread),
    EditTrackedThread(EditTrackedThread),
    DeleteTrackedThread(DeleteTrackedThread),
}

impl AuthorityCommand {
    #[must_use]
    pub fn kind(&self) -> CommandKind {
        match self {
            Self::CreateWorkstream(_) => CommandKind::CreateWorkstream,
            Self::EditWorkstream(_) => CommandKind::EditWorkstream,
            Self::DeleteWorkstream(_) => CommandKind::DeleteWorkstream,
            Self::CreateWorkUnit(_) => CommandKind::CreateWorkUnit,
            Self::EditWorkUnit(_) => CommandKind::EditWorkUnit,
            Self::DeleteWorkUnit(_) => CommandKind::DeleteWorkUnit,
            Self::CreateTrackedThread(_) => CommandKind::CreateTrackedThread,
            Self::EditTrackedThread(_) => CommandKind::EditTrackedThread,
            Self::DeleteTrackedThread(_) => CommandKind::DeleteTrackedThread,
        }
    }

    #[must_use]
    pub fn metadata(&self) -> &CommandMetadata {
        match self {
            Self::CreateWorkstream(command) => &command.metadata,
            Self::EditWorkstream(command) => &command.metadata,
            Self::DeleteWorkstream(command) => &command.metadata,
            Self::CreateWorkUnit(command) => &command.metadata,
            Self::EditWorkUnit(command) => &command.metadata,
            Self::DeleteWorkUnit(command) => &command.metadata,
            Self::CreateTrackedThread(command) => &command.metadata,
            Self::EditTrackedThread(command) => &command.metadata,
            Self::DeleteTrackedThread(command) => &command.metadata,
        }
    }

    #[must_use]
    pub fn aggregate_key(&self) -> AggregateKey {
        match self {
            Self::CreateWorkstream(command) => AggregateKey::workstream(&command.workstream_id),
            Self::EditWorkstream(command) => AggregateKey::workstream(&command.workstream_id),
            Self::DeleteWorkstream(command) => AggregateKey::workstream(&command.workstream_id),
            Self::CreateWorkUnit(command) => AggregateKey::work_unit(&command.work_unit_id),
            Self::EditWorkUnit(command) => AggregateKey::work_unit(&command.work_unit_id),
            Self::DeleteWorkUnit(command) => AggregateKey::work_unit(&command.work_unit_id),
            Self::CreateTrackedThread(command) => {
                AggregateKey::tracked_thread(&command.tracked_thread_id)
            }
            Self::EditTrackedThread(command) => {
                AggregateKey::tracked_thread(&command.tracked_thread_id)
            }
            Self::DeleteTrackedThread(command) => {
                AggregateKey::tracked_thread(&command.tracked_thread_id)
            }
        }
    }

    #[must_use]
    pub fn expected_revision(&self) -> Option<Revision> {
        match self {
            Self::CreateWorkstream(_) | Self::CreateWorkUnit(_) | Self::CreateTrackedThread(_) => {
                None
            }
            Self::EditWorkstream(command) => Some(command.expected_revision),
            Self::DeleteWorkstream(command) => Some(command.expected_revision),
            Self::EditWorkUnit(command) => Some(command.expected_revision),
            Self::DeleteWorkUnit(command) => Some(command.expected_revision),
            Self::EditTrackedThread(command) => Some(command.expected_revision),
            Self::DeleteTrackedThread(command) => Some(command.expected_revision),
        }
    }
}

/// Canonical record for a workstream in the planning hierarchy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkstreamRecord {
    pub id: WorkstreamId,
    pub title: String,
    pub objective: String,
    pub status: WorkstreamStatus,
    pub priority: String,
    pub revision: Revision,
    pub origin_node_id: OriginNodeId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

/// Canonical summary of a workstream record used in hierarchy snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkstreamSummary {
    pub id: WorkstreamId,
    pub title: String,
    pub objective: String,
    pub status: WorkstreamStatus,
    pub priority: String,
    pub revision: Revision,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

impl From<&WorkstreamRecord> for WorkstreamSummary {
    fn from(record: &WorkstreamRecord) -> Self {
        Self {
            id: record.id.clone(),
            title: record.title.clone(),
            objective: record.objective.clone(),
            status: record.status,
            priority: record.priority.clone(),
            revision: record.revision,
            updated_at: record.updated_at,
            deleted_at: record.deleted_at,
        }
    }
}

/// Canonical record for a work unit in the planning hierarchy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkUnitRecord {
    pub id: WorkUnitId,
    pub workstream_id: WorkstreamId,
    pub title: String,
    pub task_statement: String,
    pub status: WorkUnitStatus,
    pub revision: Revision,
    pub origin_node_id: OriginNodeId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

/// Canonical summary of a work unit record used in hierarchy snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkUnitSummary {
    pub id: WorkUnitId,
    pub workstream_id: WorkstreamId,
    pub title: String,
    pub status: WorkUnitStatus,
    pub revision: Revision,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

impl From<&WorkUnitRecord> for WorkUnitSummary {
    fn from(record: &WorkUnitRecord) -> Self {
        Self {
            id: record.id.clone(),
            workstream_id: record.workstream_id.clone(),
            title: record.title.clone(),
            status: record.status,
            revision: record.revision,
            updated_at: record.updated_at,
            deleted_at: record.deleted_at,
        }
    }
}

/// Canonical planning record for a tracked-thread binding.
///
/// This is an Orcas-owned local binding record that may reference an upstream
/// thread identifier. It is part of planning authority, but it is distinct from
/// TUI-local PTY resume state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedThreadRecord {
    pub id: TrackedThreadId,
    pub work_unit_id: WorkUnitId,
    pub title: String,
    pub notes: Option<String>,
    pub backend_kind: TrackedThreadBackendKind,
    pub upstream_thread_id: Option<String>,
    pub binding_state: TrackedThreadBindingState,
    pub preferred_cwd: Option<String>,
    pub preferred_model: Option<String>,
    pub last_seen_turn_id: Option<String>,
    pub revision: Revision,
    pub origin_node_id: OriginNodeId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

impl TrackedThreadRecord {
    #[must_use]
    pub fn has_upstream_binding(&self) -> bool {
        self.upstream_thread_id.is_some()
    }
}

/// Canonical summary view for a tracked-thread binding record.
///
/// Like the record, this is an Orcas-owned local binding summary rather than an
/// upstream thread ownership claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedThreadSummary {
    pub id: TrackedThreadId,
    pub work_unit_id: WorkUnitId,
    pub title: String,
    pub backend_kind: TrackedThreadBackendKind,
    pub upstream_thread_id: Option<String>,
    pub binding_state: TrackedThreadBindingState,
    pub revision: Revision,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

impl From<&TrackedThreadRecord> for TrackedThreadSummary {
    fn from(record: &TrackedThreadRecord) -> Self {
        Self {
            id: record.id.clone(),
            work_unit_id: record.work_unit_id.clone(),
            title: record.title.clone(),
            backend_kind: record.backend_kind,
            upstream_thread_id: record.upstream_thread_id.clone(),
            binding_state: record.binding_state,
            revision: record.revision,
            updated_at: record.updated_at,
            deleted_at: record.deleted_at,
        }
    }
}

/// Node in the canonical authority hierarchy snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkUnitNode {
    pub work_unit: WorkUnitSummary,
    pub tracked_threads: Vec<TrackedThreadSummary>,
}

/// Workstream node in the canonical authority hierarchy snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkstreamNode {
    pub workstream: WorkstreamSummary,
    pub work_units: Vec<WorkUnitNode>,
}

/// Canonical hierarchy snapshot returned by authority reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HierarchySnapshot {
    pub workstreams: Vec<WorkstreamNode>,
}

/// Canonical event payload for a created workstream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkstreamCreated {
    pub workstream: WorkstreamRecord,
}

/// Canonical event payload for an edited workstream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkstreamEdited {
    pub workstream_id: WorkstreamId,
    pub changes: WorkstreamPatch,
}

/// Canonical event payload for a deleted workstream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkstreamDeleted {
    pub workstream_id: WorkstreamId,
}

/// Canonical event payload for a created work unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkUnitCreated {
    pub work_unit: WorkUnitRecord,
}

/// Canonical event payload for an edited work unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkUnitEdited {
    pub work_unit_id: WorkUnitId,
    pub changes: WorkUnitPatch,
}

/// Canonical event payload for a deleted work unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkUnitDeleted {
    pub work_unit_id: WorkUnitId,
}

/// Canonical event payload for a created tracked-thread binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedThreadCreated {
    pub tracked_thread: TrackedThreadRecord,
}

/// Canonical event payload for an edited tracked-thread binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedThreadEdited {
    pub tracked_thread_id: TrackedThreadId,
    pub changes: TrackedThreadPatch,
}

/// Canonical event payload for a deleted tracked-thread binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedThreadDeleted {
    pub tracked_thread_id: TrackedThreadId,
}

/// Canonical event carrier for planning hierarchy mutations.
///
/// These events are append-only history for the authority store. They are not
/// collaboration/runtime events and they do not replace the public daemon
/// visibility stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthorityEvent {
    WorkstreamCreated(WorkstreamCreated),
    WorkstreamEdited(WorkstreamEdited),
    WorkstreamDeleted(WorkstreamDeleted),
    WorkUnitCreated(WorkUnitCreated),
    WorkUnitEdited(WorkUnitEdited),
    WorkUnitDeleted(WorkUnitDeleted),
    TrackedThreadCreated(TrackedThreadCreated),
    TrackedThreadEdited(TrackedThreadEdited),
    TrackedThreadDeleted(TrackedThreadDeleted),
}

impl AuthorityEvent {
    #[must_use]
    pub fn kind(&self) -> EventKind {
        match self {
            Self::WorkstreamCreated(_) => EventKind::WorkstreamCreated,
            Self::WorkstreamEdited(_) => EventKind::WorkstreamEdited,
            Self::WorkstreamDeleted(_) => EventKind::WorkstreamDeleted,
            Self::WorkUnitCreated(_) => EventKind::WorkUnitCreated,
            Self::WorkUnitEdited(_) => EventKind::WorkUnitEdited,
            Self::WorkUnitDeleted(_) => EventKind::WorkUnitDeleted,
            Self::TrackedThreadCreated(_) => EventKind::TrackedThreadCreated,
            Self::TrackedThreadEdited(_) => EventKind::TrackedThreadEdited,
            Self::TrackedThreadDeleted(_) => EventKind::TrackedThreadDeleted,
        }
    }
}

/// Authority event envelope with canonical metadata and append-only payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityEventEnvelope {
    pub metadata: EventMetadata,
    pub event: AuthorityEvent,
}

/// High-level target of an authority delete plan.
///
/// Delete plans are the explicit confirmation layer before tombstoning one of
/// the canonical planning aggregates.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeleteTarget {
    Workstream { workstream_id: WorkstreamId },
    WorkUnit { work_unit_id: WorkUnitId },
    TrackedThread { tracked_thread_id: TrackedThreadId },
}

impl DeleteTarget {
    #[must_use]
    pub fn aggregate_key(&self) -> AggregateKey {
        match self {
            Self::Workstream { workstream_id } => AggregateKey::workstream(workstream_id),
            Self::WorkUnit { work_unit_id } => AggregateKey::work_unit(work_unit_id),
            Self::TrackedThread { tracked_thread_id } => {
                AggregateKey::tracked_thread(tracked_thread_id)
            }
        }
    }
}

/// Human-readable description of a delete target used in delete plans.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletePlanTarget {
    pub aggregate_key: AggregateKey,
    pub label: String,
}

/// Authority delete plan returned before a destructive delete command is
/// executed.
///
/// The plan explains the aggregate target, revision expectation, cascade size,
/// and whether extra confirmation is required.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletePlan {
    pub target: DeletePlanTarget,
    pub expected_revision: Revision,
    pub affected_work_units: u64,
    pub affected_tracked_threads: u64,
    pub has_upstream_bindings: bool,
    pub confirmation_token: DeleteToken,
    pub requires_typed_confirmation: bool,
    pub expires_at: DateTime<Utc>,
}

/// Receipt returned by the authority command store when a command is accepted
/// or rejected.
///
/// Receipts support idempotency and operator visibility; they are not the same
/// as the append-only event history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandReceipt {
    pub command_id: CommandId,
    pub command_kind: CommandKind,
    pub aggregate_key: AggregateKey,
    pub accepted: bool,
    pub response_json: Option<Value>,
    pub recorded_at: DateTime<Utc>,
}

/// Stored authority event with a monotonically increasing sequence number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredAuthorityEvent {
    pub sequence: u64,
    pub envelope: AuthorityEventEnvelope,
}

/// Projection checkpoint for an authority-derived read model.
///
/// This is a projection maintenance detail, not a canonical planning record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectionCheckpoint {
    pub projection_name: String,
    pub last_applied_sequence: u64,
}

/// Command store boundary for authority writes and idempotency receipts.
#[async_trait]
pub trait AuthorityCommandStore: Send + Sync {
    async fn accept_command(&self, command: &AuthorityCommand) -> OrcasResult<CommandReceipt>;
    async fn get_command(&self, command_id: &CommandId) -> OrcasResult<Option<CommandReceipt>>;
}

/// Append-only event store boundary for canonical planning history.
#[async_trait]
pub trait AuthorityEventStore: Send + Sync {
    async fn append_events(
        &self,
        events: &[AuthorityEventEnvelope],
    ) -> OrcasResult<Vec<StoredAuthorityEvent>>;

    async fn list_events(
        &self,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> OrcasResult<Vec<StoredAuthorityEvent>>;
}

/// Checkpoint store boundary for authority-derived projections.
#[async_trait]
pub trait AuthorityProjectionStore: Send + Sync {
    async fn load_projection_checkpoint(
        &self,
        projection_name: &str,
    ) -> OrcasResult<Option<ProjectionCheckpoint>>;

    async fn save_projection_checkpoint(
        &self,
        checkpoint: &ProjectionCheckpoint,
    ) -> OrcasResult<()>;
}

/// Projector boundary that applies stored authority events to read models.
#[async_trait]
pub trait AuthorityProjector: Send + Sync {
    async fn apply(&self, event: &StoredAuthorityEvent) -> OrcasResult<()>;
}

/// Read-only authority hierarchy and detail surface.
///
/// Implementations should return canonical planning records and summaries, not
/// collaboration/runtime mirrors.
#[async_trait]
pub trait AuthorityQueryStore: Send + Sync {
    async fn hierarchy_snapshot(&self, include_deleted: bool) -> OrcasResult<HierarchySnapshot>;
    async fn list_workstreams(&self, include_deleted: bool) -> OrcasResult<Vec<WorkstreamSummary>>;
    async fn get_workstream(&self, id: &WorkstreamId) -> OrcasResult<Option<WorkstreamRecord>>;
    async fn list_work_units(
        &self,
        workstream_id: Option<&WorkstreamId>,
        include_deleted: bool,
    ) -> OrcasResult<Vec<WorkUnitSummary>>;
    async fn get_work_unit(&self, id: &WorkUnitId) -> OrcasResult<Option<WorkUnitRecord>>;
    async fn list_tracked_threads(
        &self,
        work_unit_id: &WorkUnitId,
        include_deleted: bool,
    ) -> OrcasResult<Vec<TrackedThreadSummary>>;
    async fn get_tracked_thread(
        &self,
        id: &TrackedThreadId,
    ) -> OrcasResult<Option<TrackedThreadRecord>>;
    async fn delete_plan(&self, target: &DeleteTarget) -> OrcasResult<Option<DeletePlan>>;
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::{Value, json};

    use super::*;

    fn actor() -> CommandActor {
        CommandActor::parse("tui_operator").expect("actor")
    }

    fn origin() -> OriginNodeId {
        OriginNodeId::parse("origin-local").expect("origin")
    }

    fn command_metadata() -> CommandMetadata {
        CommandMetadata {
            command_id: CommandId::parse("command-1").expect("command id"),
            issued_at: Utc
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("timestamp"),
            origin_node_id: origin(),
            actor: actor(),
            correlation_id: Some(CorrelationId::parse("corr-1").expect("correlation")),
        }
    }

    #[test]
    fn tracked_thread_command_round_trips_with_envelope_metadata() {
        let command = AuthorityCommand::CreateTrackedThread(CreateTrackedThread {
            metadata: command_metadata(),
            tracked_thread_id: TrackedThreadId::parse("tracked-thread-1").expect("thread id"),
            work_unit_id: WorkUnitId::parse("work-unit-1").expect("work unit id"),
            title: "Plan implementation lane".to_string(),
            notes: Some("Operator-owned tracking record".to_string()),
            backend_kind: TrackedThreadBackendKind::Codex,
            upstream_thread_id: Some("codex-thread-123".to_string()),
            preferred_cwd: Some("/tmp/orcas".to_string()),
            preferred_model: Some("gpt-5.4".to_string()),
        });

        let json = serde_json::to_value(&command).expect("serialize command");
        let round_trip = serde_json::from_value::<AuthorityCommand>(json).expect("deserialize");

        assert_eq!(round_trip, command);
        assert_eq!(command.kind(), CommandKind::CreateTrackedThread);
        assert_eq!(
            command.aggregate_key(),
            AggregateKey::tracked_thread(
                &TrackedThreadId::parse("tracked-thread-1").expect("thread id")
            )
        );
        assert!(command.expected_revision().is_none());
    }

    #[test]
    fn event_envelope_round_trips_with_explicit_aggregate_metadata() {
        let record = WorkstreamRecord {
            id: WorkstreamId::parse("workstream-1").expect("workstream id"),
            title: "Stabilize local backend".to_string(),
            objective: "Prepare pass 2".to_string(),
            status: WorkstreamStatus::Active,
            priority: "high".to_string(),
            revision: Revision::initial(),
            origin_node_id: origin(),
            created_at: Utc
                .with_ymd_and_hms(2026, 3, 18, 12, 5, 0)
                .single()
                .expect("timestamp"),
            updated_at: Utc
                .with_ymd_and_hms(2026, 3, 18, 12, 5, 0)
                .single()
                .expect("timestamp"),
            deleted_at: None,
        };
        let envelope = AuthorityEventEnvelope {
            metadata: EventMetadata {
                event_id: EventId::parse("event-1").expect("event id"),
                command_id: CommandId::parse("command-1").expect("command id"),
                aggregate_type: AggregateType::Workstream,
                aggregate_id: record.id.to_string(),
                aggregate_version: Revision::initial(),
                occurred_at: Utc
                    .with_ymd_and_hms(2026, 3, 18, 12, 6, 0)
                    .single()
                    .expect("timestamp"),
                origin_node_id: origin(),
                causation_id: Some(CausationId::parse("cause-1").expect("causation")),
                correlation_id: Some(CorrelationId::parse("corr-1").expect("correlation")),
            },
            event: AuthorityEvent::WorkstreamCreated(WorkstreamCreated { workstream: record }),
        };

        let json = serde_json::to_value(&envelope).expect("serialize event");
        let round_trip =
            serde_json::from_value::<AuthorityEventEnvelope>(json).expect("deserialize event");

        assert_eq!(round_trip, envelope);
        assert_eq!(envelope.event.kind(), EventKind::WorkstreamCreated);
        assert_eq!(envelope.metadata.aggregate_type, AggregateType::Workstream);
    }

    #[test]
    fn tracked_thread_delete_shapes_do_not_claim_upstream_hard_delete() {
        let command = AuthorityCommand::DeleteTrackedThread(DeleteTrackedThread {
            metadata: command_metadata(),
            tracked_thread_id: TrackedThreadId::parse("tracked-thread-9").expect("thread id"),
            expected_revision: Revision::new(4),
            delete_token: DeleteToken::parse("delete-token-9").expect("delete token"),
        });
        let event = AuthorityEvent::TrackedThreadDeleted(TrackedThreadDeleted {
            tracked_thread_id: TrackedThreadId::parse("tracked-thread-9").expect("thread id"),
        });

        let command_json = serde_json::to_value(&command).expect("serialize command");
        let event_json = serde_json::to_value(&event).expect("serialize event");

        assert_eq!(command.kind(), CommandKind::DeleteTrackedThread);
        assert_eq!(event.kind(), EventKind::TrackedThreadDeleted);
        assert!(find_json_key(&command_json, "upstream_thread_id").is_none());
        assert!(find_json_key(&event_json, "upstream_thread_id").is_none());
        assert!(find_json_key(&command_json, "hard_delete_upstream").is_none());
        assert!(find_json_key(&event_json, "hard_delete_upstream").is_none());
    }

    #[test]
    fn tracked_thread_record_is_local_and_optional_binding_only() {
        let record = TrackedThreadRecord {
            id: TrackedThreadId::parse("tracked-thread-2").expect("thread id"),
            work_unit_id: WorkUnitId::parse("work-unit-2").expect("work unit id"),
            title: "Review lane".to_string(),
            notes: None,
            backend_kind: TrackedThreadBackendKind::Codex,
            upstream_thread_id: Some("codex-thread-2".to_string()),
            binding_state: TrackedThreadBindingState::Bound,
            preferred_cwd: None,
            preferred_model: None,
            last_seen_turn_id: None,
            revision: Revision::initial(),
            origin_node_id: origin(),
            created_at: Utc
                .with_ymd_and_hms(2026, 3, 18, 12, 7, 0)
                .single()
                .expect("timestamp"),
            updated_at: Utc
                .with_ymd_and_hms(2026, 3, 18, 12, 7, 0)
                .single()
                .expect("timestamp"),
            deleted_at: None,
        };

        let summary = TrackedThreadSummary::from(&record);

        assert!(record.has_upstream_binding());
        assert_eq!(summary.id, record.id);
        assert_eq!(
            summary.upstream_thread_id.as_deref(),
            Some("codex-thread-2")
        );
        assert_eq!(summary.binding_state, TrackedThreadBindingState::Bound);
    }

    #[test]
    fn delete_target_serializes_with_explicit_kind() {
        let target = DeleteTarget::WorkUnit {
            work_unit_id: WorkUnitId::parse("work-unit-7").expect("work unit id"),
        };

        let json = serde_json::to_value(&target).expect("serialize");

        assert_eq!(
            json,
            json!({
                "kind": "work_unit",
                "work_unit_id": "work-unit-7"
            })
        );
    }

    fn find_json_key<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
        match value {
            Value::Object(map) => map
                .get(key)
                .or_else(|| map.values().find_map(|value| find_json_key(value, key))),
            Value::Array(values) => values.iter().find_map(|value| find_json_key(value, key)),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
        }
    }
}
