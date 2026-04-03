pub mod authority;
pub mod collaboration;
pub mod communication;
#[cfg(not(target_arch = "wasm32"))]
pub mod config;
pub mod error;
pub mod events;
pub mod ipc;
pub mod jsonrpc;
#[cfg(not(target_arch = "wasm32"))]
pub mod logging;
#[cfg(not(target_arch = "wasm32"))]
pub mod paths;
pub mod planning;
pub mod session;
#[cfg(not(target_arch = "wasm32"))]
pub mod store;
pub mod supervisor;

pub const ORCAS_APP_SERVER_TAG_ENV: &str = "ORCAS_APP_SERVER_TAG";
pub const ORCAS_APP_SERVER_TAG_VALUE: &str = "orcas-managed";
pub const ORCAS_APP_SERVER_OWNER_KIND_ENV: &str = "ORCAS_APP_SERVER_OWNER_KIND";
pub const ORCAS_APP_SERVER_OWNER_PID_ENV: &str = "ORCAS_APP_SERVER_OWNER_PID";
pub const ORCAS_APP_SERVER_LISTEN_URL_ENV: &str = "ORCAS_APP_SERVER_LISTEN_URL";
pub const ORCAS_APP_SERVER_STARTED_AT_ENV: &str = "ORCAS_APP_SERVER_STARTED_AT";

pub use collaboration::{
    Assignment, AssignmentStatus, CodexThreadAssignment, CodexThreadAssignmentStatus,
    CodexThreadBootstrapState, CodexThreadSendPolicy, CollaborationState, Decision, DecisionType,
    LandingAuthorizationRecord, LandingAuthorizationStatus, LandingExecutionRecord,
    LandingExecutionStatus, PlanningSession, PlanningSessionResearchStatus, PlanningSessionStatus,
    PlanningSessionStructuredSummary, Report, ReportConfidence, ReportDisposition,
    ReportParseResult, SupervisorTurnDecision, SupervisorTurnDecisionKind,
    SupervisorTurnDecisionStatus, SupervisorTurnProposalKind, WorkUnit, WorkUnitStatus, Worker,
    WorkerSession, WorkerSessionAttachability, WorkerSessionRuntimeStatus, WorkerStatus,
    WorkspaceOperationRecord, Workstream, WorkstreamStatus,
};
pub use communication::{
    AcceptanceCriterionStatus, AcceptanceResult, AssignmentChangePolicy, AssignmentChecklistItem,
    AssignmentCommunicationPacket, AssignmentCommunicationPolicy, AssignmentCommunicationRecord,
    AssignmentCommunicationSeed, AssignmentContextBlock, AssignmentExecutionContext,
    AssignmentModeSpec, AssignmentScopeBoundary, AssignmentTaskMode, AssignmentWorkspaceContract,
    FileChangeKind, ImplementModePayload, ImplementModeSpec, PromptRenderArtifact,
    PromptRenderSpec, ReviewSignal, ReviewSignalLevel, TouchedFile,
    TrackedThreadLandingExecutionContract, TrackedThreadLandingExecutionResult,
    TrackedThreadLandingExecutionResultStatus, TrackedThreadPruneWorkspaceContract,
    TrackedThreadPruneWorkspaceResult, TrackedThreadPruneWorkspaceResultStatus,
    TrackedThreadWorkspaceOperationContract, TrackedThreadWorkspaceOperationKind,
    TrackedThreadWorkspaceOperationStatus, WorkerReportContract, WorkerReportEnvelope,
    WorkerReportModePayload, WorkerReportValidation, WorkerWorkspaceReport,
};
#[cfg(not(target_arch = "wasm32"))]
pub use config::{
    AppConfig, CodexConnectionMode, CodexDaemonConfig, ReconnectPolicy, SupervisorConfig,
    SupervisorProposalConfig,
};
pub use error::{OrcasError, OrcasResult};
pub use events::{CodexItemEvent, CodexTurnEvent, ConnectionState, EventEnvelope, OrcasEvent};
pub use ipc::*;
pub use jsonrpc::{
    JsonRpcError, JsonRpcErrorObject, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, RequestId,
};
#[cfg(not(target_arch = "wasm32"))]
pub use logging::init_file_logger;
#[cfg(not(target_arch = "wasm32"))]
pub use paths::AppPaths;
pub use planning::*;
pub use session::{ThreadDescriptor, ThreadMetadata, ThreadRegistry, TurnDescriptor};
#[cfg(not(target_arch = "wasm32"))]
pub use store::{JsonSessionStore, OrcasSessionStore, StoredState};
pub use supervisor::*;
